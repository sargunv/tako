const std = @import("std");
const common = @import("common.zig");

const ghostty = common.ghostty;
const ft = common.ft;
const hb = common.hb;
const backend = common.backend;
const allocator = common.allocator;

const SurfaceOptions = common.SurfaceOptions;
const CellMetrics = common.CellMetrics;
const BackendShapedGlyph = common.BackendShapedGlyph;
const BackendShapedText = common.BackendShapedText;
const BackendRasterizedGlyph = common.BackendRasterizedGlyph;

const ShapeCache = struct {
    map: std.StringHashMap([]BackendShapedGlyph),
    order: std.ArrayList([]u8) = .empty,
    cap: usize = 4096,

    fn init() ShapeCache {
        return .{ .map = std.StringHashMap([]BackendShapedGlyph).init(allocator) };
    }

    fn deinit(self: *ShapeCache) void {
        var it = self.map.iterator();
        while (it.next()) |entry| {
            allocator.free(entry.key_ptr.*);
            allocator.free(entry.value_ptr.*);
        }
        self.map.deinit();
        self.order.deinit(allocator);
    }

    fn clear(self: *ShapeCache) void {
        var it = self.map.iterator();
        while (it.next()) |entry| {
            allocator.free(entry.key_ptr.*);
            allocator.free(entry.value_ptr.*);
        }
        self.map.clearRetainingCapacity();
        self.order.clearRetainingCapacity();
    }

    fn get(self: *ShapeCache, text: []const u8) ?[]BackendShapedGlyph {
        return self.map.get(text);
    }

    fn put(self: *ShapeCache, text: []const u8, glyphs: []BackendShapedGlyph) !void {
        if (!self.map.contains(text)) {
            while (self.map.count() >= self.cap and self.order.items.len > 0) {
                const oldest = self.order.orderedRemove(0);
                if (self.map.fetchRemove(oldest)) |entry| {
                    allocator.free(entry.key);
                    allocator.free(entry.value);
                    break;
                }
                allocator.free(oldest);
            }
            const key = try allocator.dupe(u8, text);
            errdefer allocator.free(key);
            try self.order.append(allocator, key);
            try self.map.put(key, glyphs);
        } else {
            allocator.free(glyphs);
        }
    }
};

pub const FontCore = struct {
    font_path: [:0]u8,
    logical_pixel_height: u32,
    dpr: f32,
    physical_pixel_height: u32,
    library: ft.FT_Library,
    face: ft.FT_Face,
    font_bytes: []u8,
    hb_blob: ?*hb.hb_blob_t,
    hb_face: ?*hb.hb_face_t,
    hb_font: ?*hb.hb_font_t,
    units_per_em: u32,
    cell: CellMetrics,
    shape_cache: ShapeCache,
    shaped_buf: std.ArrayList(BackendShapedGlyph) = .empty,
    raster_pixels: std.ArrayList(u8) = .empty,

    pub fn create(font_path: [*:0]const u8, logical_pixel_height: u32, dpr: f32) ?*FontCore {
        const owned_path = allocator.dupeZ(u8, std.mem.span(font_path)) catch return null;
        errdefer allocator.free(owned_path);

        var library: ft.FT_Library = null;
        if (ft.FT_Init_FreeType(&library) != 0 or library == null) return null;
        errdefer _ = ft.FT_Done_FreeType(library);

        var face: ft.FT_Face = null;
        if (ft.FT_New_Face(library, owned_path.ptr, 0, &face) != 0 or face == null) return null;
        errdefer _ = ft.FT_Done_Face(face);

        const physical_px = physicalFontSize(logical_pixel_height, dpr);
        if (ft.FT_Set_Pixel_Sizes(face, physical_px, physical_px) != 0) return null;

        const font_bytes = std.fs.openFileAbsolute(owned_path, .{}) catch return null;
        defer font_bytes.close();
        const bytes = font_bytes.readToEndAlloc(allocator, 64 * 1024 * 1024) catch return null;
        errdefer allocator.free(bytes);

        const blob = hb.hb_blob_create(
            @ptrCast(bytes.ptr),
            @intCast(bytes.len),
            hb.HB_MEMORY_MODE_READONLY,
            null,
            null,
        ) orelse return null;
        errdefer hb.hb_blob_destroy(blob);

        const hb_face = hb.hb_face_create(blob, 0) orelse return null;
        errdefer hb.hb_face_destroy(hb_face);

        const hb_font = hb.hb_font_create(hb_face) orelse return null;
        errdefer hb.hb_font_destroy(hb_font);
        hb.hb_ot_font_set_funcs(hb_font);

        const upem = @max(hb.hb_face_get_upem(hb_face), 1);
        hb.hb_font_set_scale(hb_font, @intCast(upem), @intCast(upem));

        const core = allocator.create(FontCore) catch return null;
        core.* = .{
            .font_path = owned_path,
            .logical_pixel_height = logical_pixel_height,
            .dpr = dpr,
            .physical_pixel_height = physical_px,
            .library = library,
            .face = face,
            .font_bytes = bytes,
            .hb_blob = blob,
            .hb_face = hb_face,
            .hb_font = hb_font,
            .units_per_em = upem,
            .cell = computeCellMetrics(face),
            .shape_cache = ShapeCache.init(),
        };
        return core;
    }

    pub fn destroy(self: *FontCore) void {
        self.shape_cache.deinit();
        self.shaped_buf.deinit(allocator);
        self.raster_pixels.deinit(allocator);
        if (self.hb_font) |font| hb.hb_font_destroy(font);
        if (self.hb_face) |face| hb.hb_face_destroy(face);
        if (self.hb_blob) |blob| hb.hb_blob_destroy(blob);
        _ = ft.FT_Done_Face(self.face);
        _ = ft.FT_Done_FreeType(self.library);
        allocator.free(self.font_bytes);
        allocator.free(self.font_path);
        allocator.destroy(self);
    }

    pub fn setDpr(self: *FontCore, dpr: f32) void {
        if (@abs(dpr - self.dpr) < 0.01) return;
        self.dpr = dpr;
        self.physical_pixel_height = physicalFontSize(self.logical_pixel_height, dpr);
        if (ft.FT_Set_Pixel_Sizes(self.face, self.physical_pixel_height, self.physical_pixel_height) != 0) return;
        self.cell = computeCellMetrics(self.face);
        self.shape_cache.clear();
        self.raster_pixels.clearRetainingCapacity();
    }

    pub fn setFont(self: *FontCore, font_path: [*:0]const u8, logical_pixel_height: u32) bool {
        const replacement = FontCore.create(font_path, logical_pixel_height, self.dpr) orelse return false;
        const old = self.*;
        self.* = replacement.*;
        allocator.destroy(replacement);
        var old_copy = old;
        old_copy.destroyMoved();
        return true;
    }

    fn destroyMoved(self: *FontCore) void {
        self.shape_cache.deinit();
        self.shaped_buf.deinit(allocator);
        self.raster_pixels.deinit(allocator);
        if (self.hb_font) |font| hb.hb_font_destroy(font);
        if (self.hb_face) |face| hb.hb_face_destroy(face);
        if (self.hb_blob) |blob| hb.hb_blob_destroy(blob);
        _ = ft.FT_Done_Face(self.face);
        _ = ft.FT_Done_FreeType(self.library);
        allocator.free(self.font_bytes);
        allocator.free(self.font_path);
    }

    pub fn shapeText(self: *FontCore, text: []const u8) bool {
        self.shaped_buf.clearRetainingCapacity();
        if (self.shape_cache.get(text)) |cached| {
            self.shaped_buf.appendSlice(allocator, cached) catch return false;
            return true;
        }

        const buffer = hb.hb_buffer_create() orelse return false;
        defer hb.hb_buffer_destroy(buffer);
        hb.hb_buffer_add_utf8(buffer, @ptrCast(text.ptr), @intCast(text.len), 0, @intCast(text.len));
        hb.hb_buffer_guess_segment_properties(buffer);
        hb.hb_shape(self.hb_font, buffer, null, 0);

        var count: c_uint = 0;
        const infos = hb.hb_buffer_get_glyph_infos(buffer, &count);
        const positions = hb.hb_buffer_get_glyph_positions(buffer, &count);
        if (infos == null or positions == null) return false;

        const scale = @as(f32, @floatFromInt(self.physical_pixel_height)) / @as(f32, @floatFromInt(self.units_per_em));
        var shaped = allocator.alloc(BackendShapedGlyph, count) catch return false;
        errdefer allocator.free(shaped);
        var i: usize = 0;
        while (i < count) : (i += 1) {
            shaped[i] = .{
                .glyph_id = infos[i].codepoint,
                .x_offset = @as(f32, @floatFromInt(positions[i].x_offset)) * scale,
                .y_offset = @as(f32, @floatFromInt(positions[i].y_offset)) * scale,
                .x_advance = @as(f32, @floatFromInt(positions[i].x_advance)) * scale,
            };
        }
        self.shaped_buf.appendSlice(allocator, shaped) catch return false;
        self.shape_cache.put(text, shaped) catch return false;
        return true;
    }

    pub fn rasterizeGlyph(self: *FontCore, glyph_id: u32) BackendRasterizedGlyph {
        self.raster_pixels.clearRetainingCapacity();
        if (ft.FT_Load_Glyph(self.face, glyph_id, ft.FT_LOAD_RENDER) != 0) {
            return .{ .glyph_id = glyph_id, .width = 0, .height = 0, .left_bearing = 0, .top_bearing = 0, .pixels = null, .pixel_len = 0 };
        }
        const slot = self.face.*.glyph;
        const bitmap = slot.*.bitmap;
        const width: u32 = @intCast(@max(bitmap.width, 0));
        const height: u32 = @intCast(@max(bitmap.rows, 0));
        if (width > 0 and height > 0 and bitmap.buffer != null) {
            const pitch_abs: usize = @intCast(if (bitmap.pitch < 0) -bitmap.pitch else bitmap.pitch);
            self.raster_pixels.ensureTotalCapacity(allocator, @as(usize, width) * @as(usize, height)) catch {};
            var row: usize = 0;
            while (row < height) : (row += 1) {
                const src_start = if (bitmap.pitch >= 0)
                    row * pitch_abs
                else
                    (@as(usize, height) - 1 - row) * pitch_abs;
                self.raster_pixels.appendSlice(allocator, bitmap.buffer[src_start .. src_start + width]) catch {};
            }
        }
        return .{
            .glyph_id = glyph_id,
            .width = width,
            .height = height,
            .left_bearing = slot.*.bitmap_left,
            .top_bearing = slot.*.bitmap_top,
            .pixels = if (self.raster_pixels.items.len == 0) null else self.raster_pixels.items.ptr,
            .pixel_len = self.raster_pixels.items.len,
        };
    }
};

pub fn fontCoreCreateWithOptions(options: ?*const SurfaceOptions) ?*FontCore {
    const opts = options orelse return null;
    const path = opts.font_path orelse return null;
    return FontCore.create(path, opts.pixel_height, opts.dpr);
}

pub fn fontCoreDestroy(surface: ?*FontCore) void {
    const s = surface orelse return;
    s.destroy();
}

pub fn fontCoreShapeText(
    surface: ?*FontCore,
    text: ?[*]const u8,
    text_len: usize,
    out: ?*BackendShapedText,
) bool {
    const s = surface orelse return false;
    const target = out orelse return false;
    if (text_len > 0 and text == null) return false;
    const bytes = if (text_len == 0) "" else text.?[0..text_len];
    if (!s.shapeText(bytes)) return false;
    target.* = .{
        .glyphs = if (s.shaped_buf.items.len == 0) null else s.shaped_buf.items.ptr,
        .glyph_count = s.shaped_buf.items.len,
    };
    return true;
}

pub fn fontCoreRasterizeGlyph(
    surface: ?*FontCore,
    glyph_id: u32,
    out: ?*BackendRasterizedGlyph,
) bool {
    const s = surface orelse return false;
    const target = out orelse return false;
    target.* = s.rasterizeGlyph(glyph_id);
    return true;
}

pub fn fontCoreSetDpr(surface: ?*FontCore, dpr: f32) void {
    const s = surface orelse return;
    s.setDpr(dpr);
}

pub fn fontCoreSetFont(
    surface: ?*FontCore,
    font_path: ?[*:0]const u8,
    pixel_height: u32,
) i32 {
    const s = surface orelse return 0;
    const path = font_path orelse return 0;
    return if (s.setFont(path, pixel_height)) 1 else 0;
}

pub fn fontCoreCellMetrics(
    surface: ?*FontCore,
    out: ?*CellMetrics,
) bool {
    const s = surface orelse return false;
    const target = out orelse return false;
    target.* = s.cell;
    return true;
}

pub fn physicalFontSize(logical_pixel_height: u32, dpr: f32) u32 {
    const px: u32 = @intFromFloat(@round(@as(f32, @floatFromInt(logical_pixel_height)) * dpr));
    return @max(px, 1);
}

pub fn computeCellMetrics(face: ft.FT_Face) CellMetrics {
    var ascent: i32 = 1;
    var descent: i32 = 0;
    var height: u32 = 1;
    if (face != null and face.*.size != null) {
        const metrics = face.*.size.*.metrics;
        ascent = @intCast(metrics.ascender >> 6);
        descent = @intCast(metrics.descender >> 6);
        height = @intCast(@max((metrics.height + 32) >> 6, 1));
    }
    return .{
        .cell_width = @intCast(@max(glyphAdvancePx(face, 'M'), 1)),
        .cell_height = height,
        .cell_ascent = ascent,
        .cell_descent = descent,
    };
}

pub fn glyphAdvancePx(face: ft.FT_Face, codepoint: u32) i32 {
    if (face == null) return 0;
    const glyph_index = ft.FT_Get_Char_Index(face, codepoint);
    if (glyph_index == 0) return 0;
    if (ft.FT_Load_Glyph(face, glyph_index, ft.FT_LOAD_DEFAULT) != 0) return 0;
    return @intCast(face.*.glyph.*.advance.x >> 6);
}

pub fn resolveFontPath(
    explicit_path: ?[*:0]const u8,
    family: ?[*:0]const u8,
) ?[:0]u8 {
    if (common.optionalCString(explicit_path)) |path| {
        return allocator.dupeZ(u8, path) catch null;
    }

    const requested = common.optionalCString(family) orelse "monospace";
    const argv = [_][]const u8{ "fc-match", "-f", "%{file}", requested };
    const result = std.process.Child.run(.{
        .allocator = allocator,
        .argv = &argv,
        .max_output_bytes = std.fs.max_path_bytes,
    }) catch |err| {
        std.log.warn("fc-match failed for font family '{s}': {s}", .{ requested, @errorName(err) });
        return null;
    };
    defer allocator.free(result.stdout);
    defer allocator.free(result.stderr);

    switch (result.term) {
        .Exited => |code| if (code != 0) {
            std.log.warn("fc-match returned {d} for font family '{s}'", .{ code, requested });
            return null;
        },
        else => {
            std.log.warn("fc-match did not exit normally for font family '{s}'", .{requested});
            return null;
        },
    }

    const path = std.mem.trim(u8, result.stdout, " \t\r\n");
    if (path.len == 0) {
        std.log.warn("fc-match returned empty path for font family '{s}'", .{requested});
        return null;
    }
    return allocator.dupeZ(u8, path) catch null;
}
