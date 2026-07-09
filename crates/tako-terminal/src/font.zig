const std = @import("std");
const common = @import("common.zig");

const ft = common.ft;
const hb = common.hb;
const fc = common.fc;
const allocator = common.allocator;

const SurfaceOptions = common.SurfaceOptions;
const CellMetrics = common.CellMetrics;
const BackendCellStyle = common.BackendCellStyle;
const BackendShapedGlyph = common.BackendShapedGlyph;
const BackendShapedText = common.BackendShapedText;
const BackendRasterizedGlyph = common.BackendRasterizedGlyph;

const style_count = 4;

pub const FontStyle = enum(u8) {
    regular = 0,
    bold = 1,
    italic = 2,
    bold_italic = 3,
};

const FaceDescriptor = struct {
    path: [:0]u8,
    index: c_long = 0,
    transform: ?ft.FT_Matrix = null,

    fn clone(self: FaceDescriptor) ?FaceDescriptor {
        return .{
            .path = allocator.dupeZ(u8, self.path) catch return null,
            .index = self.index,
            .transform = self.transform,
        };
    }

    fn deinit(self: *FaceDescriptor) void {
        allocator.free(self.path);
    }
};

const FaceSet = struct {
    descriptors: [style_count]FaceDescriptor,

    fn deinit(self: *FaceSet) void {
        for (&self.descriptors) |*descriptor| descriptor.deinit();
    }
};

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

const LoadedFace = struct {
    font_path: [:0]u8,
    index: c_long,
    transform: ?ft.FT_Matrix,
    face: ft.FT_Face,
    font_bytes: []u8,
    hb_blob: ?*hb.hb_blob_t,
    hb_face: ?*hb.hb_face_t,
    hb_font: ?*hb.hb_font_t,
    units_per_em: u32,
    shape_cache: ShapeCache,

    fn create(
        library: ft.FT_Library,
        descriptor: FaceDescriptor,
        physical_pixel_height: u32,
    ) ?LoadedFace {
        const owned_path = allocator.dupeZ(u8, descriptor.path) catch return null;
        errdefer allocator.free(owned_path);

        var face: ft.FT_Face = null;
        if (ft.FT_New_Face(library, owned_path.ptr, descriptor.index, &face) != 0 or face == null) return null;
        errdefer _ = ft.FT_Done_Face(face);

        if (ft.FT_Set_Pixel_Sizes(face, physical_pixel_height, physical_pixel_height) != 0) return null;
        applyFaceTransform(face, descriptor.transform);

        const font_file = std.fs.openFileAbsolute(owned_path, .{}) catch return null;
        defer font_file.close();
        const bytes = font_file.readToEndAlloc(allocator, 64 * 1024 * 1024) catch return null;
        errdefer allocator.free(bytes);

        const blob = hb.hb_blob_create(
            @ptrCast(bytes.ptr),
            @intCast(bytes.len),
            hb.HB_MEMORY_MODE_READONLY,
            null,
            null,
        ) orelse return null;
        errdefer hb.hb_blob_destroy(blob);

        const hb_face = hb.hb_face_create(blob, @intCast(descriptor.index)) orelse return null;
        errdefer hb.hb_face_destroy(hb_face);

        const hb_font = hb.hb_font_create(hb_face) orelse return null;
        errdefer hb.hb_font_destroy(hb_font);
        hb.hb_ot_font_set_funcs(hb_font);

        const upem = @max(hb.hb_face_get_upem(hb_face), 1);
        hb.hb_font_set_scale(hb_font, @intCast(upem), @intCast(upem));

        return .{
            .font_path = owned_path,
            .index = descriptor.index,
            .transform = descriptor.transform,
            .face = face,
            .font_bytes = bytes,
            .hb_blob = blob,
            .hb_face = hb_face,
            .hb_font = hb_font,
            .units_per_em = upem,
            .shape_cache = ShapeCache.init(),
        };
    }

    fn deinit(self: *LoadedFace) void {
        self.shape_cache.deinit();
        if (self.hb_font) |font| hb.hb_font_destroy(font);
        if (self.hb_face) |face| hb.hb_face_destroy(face);
        if (self.hb_blob) |blob| hb.hb_blob_destroy(blob);
        _ = ft.FT_Done_Face(self.face);
        allocator.free(self.font_bytes);
        allocator.free(self.font_path);
    }

    fn setPixelHeight(self: *LoadedFace, physical_pixel_height: u32) bool {
        if (ft.FT_Set_Pixel_Sizes(self.face, physical_pixel_height, physical_pixel_height) != 0) return false;
        applyFaceTransform(self.face, self.transform);
        self.shape_cache.clear();
        return true;
    }
};

pub const FontCore = struct {
    logical_pixel_height: u32,
    dpr: f32,
    physical_pixel_height: u32,
    library: ft.FT_Library,
    faces: [style_count]LoadedFace,
    cell: CellMetrics,
    shaped_buf: std.ArrayList(BackendShapedGlyph) = .empty,
    raster_pixels: std.ArrayList(u8) = .empty,

    pub fn create(
        font_path: ?[*:0]const u8,
        font_family: ?[*:0]const u8,
        logical_pixel_height: u32,
        dpr: f32,
    ) ?*FontCore {
        var descriptors = resolveFaceSet(font_path, font_family) orelse return null;
        defer descriptors.deinit();

        var library: ft.FT_Library = null;
        if (ft.FT_Init_FreeType(&library) != 0 or library == null) return null;
        errdefer _ = ft.FT_Done_FreeType(library);

        const physical_px = physicalFontSize(logical_pixel_height, dpr);
        var faces: [style_count]LoadedFace = undefined;
        var loaded: usize = 0;
        errdefer {
            var i: usize = 0;
            while (i < loaded) : (i += 1) faces[i].deinit();
        }

        while (loaded < style_count) : (loaded += 1) {
            faces[loaded] = LoadedFace.create(
                library,
                descriptors.descriptors[loaded],
                physical_px,
            ) orelse return null;
        }

        const core = allocator.create(FontCore) catch return null;
        core.* = .{
            .logical_pixel_height = logical_pixel_height,
            .dpr = dpr,
            .physical_pixel_height = physical_px,
            .library = library,
            .faces = faces,
            .cell = computeCellMetrics(faces[@intFromEnum(FontStyle.regular)].face),
        };
        return core;
    }

    pub fn destroy(self: *FontCore) void {
        self.destroyMoved();
        allocator.destroy(self);
    }

    pub fn setDpr(self: *FontCore, dpr: f32) void {
        if (@abs(dpr - self.dpr) < 0.01) return;
        self.dpr = dpr;
        self.physical_pixel_height = physicalFontSize(self.logical_pixel_height, dpr);
        for (&self.faces) |*face| _ = face.setPixelHeight(self.physical_pixel_height);
        self.cell = computeCellMetrics(self.faces[@intFromEnum(FontStyle.regular)].face);
        self.raster_pixels.clearRetainingCapacity();
    }

    pub fn setFont(
        self: *FontCore,
        font_path: ?[*:0]const u8,
        font_family: ?[*:0]const u8,
        logical_pixel_height: u32,
    ) bool {
        const replacement = FontCore.create(font_path, font_family, logical_pixel_height, self.dpr) orelse return false;
        const old = self.*;
        self.* = replacement.*;
        allocator.destroy(replacement);
        var old_copy = old;
        old_copy.destroyMoved();
        return true;
    }

    fn destroyMoved(self: *FontCore) void {
        self.shaped_buf.deinit(allocator);
        self.raster_pixels.deinit(allocator);
        for (&self.faces) |*face| face.deinit();
        _ = ft.FT_Done_FreeType(self.library);
    }

    pub fn shapeText(self: *FontCore, style: FontStyle, text: []const u8) bool {
        self.shaped_buf.clearRetainingCapacity();
        const face = &self.faces[@intFromEnum(style)];
        if (face.shape_cache.get(text)) |cached| {
            self.shaped_buf.appendSlice(allocator, cached) catch return false;
            return true;
        }

        const buffer = hb.hb_buffer_create() orelse return false;
        defer hb.hb_buffer_destroy(buffer);
        hb.hb_buffer_add_utf8(buffer, @ptrCast(text.ptr), @intCast(text.len), 0, @intCast(text.len));
        hb.hb_buffer_guess_segment_properties(buffer);
        hb.hb_shape(face.hb_font, buffer, null, 0);

        var count: c_uint = 0;
        const infos = hb.hb_buffer_get_glyph_infos(buffer, &count);
        const positions = hb.hb_buffer_get_glyph_positions(buffer, &count);
        if (infos == null or positions == null) return false;

        const scale = @as(f32, @floatFromInt(self.physical_pixel_height)) / @as(f32, @floatFromInt(face.units_per_em));
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
        face.shape_cache.put(text, shaped) catch return false;
        return true;
    }

    pub fn rasterizeGlyph(self: *FontCore, style: FontStyle, glyph_id: u32) BackendRasterizedGlyph {
        self.raster_pixels.clearRetainingCapacity();
        const face = &self.faces[@intFromEnum(style)];
        if (ft.FT_Load_Glyph(face.face, glyph_id, ft.FT_LOAD_RENDER) != 0) {
            return .{ .glyph_id = glyph_id, .width = 0, .height = 0, .left_bearing = 0, .top_bearing = 0, .pixels = null, .pixel_len = 0 };
        }
        const slot = face.face.*.glyph;
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
    return FontCore.create(opts.font_path, opts.font_family, opts.pixel_height, opts.dpr);
}

pub fn fontCoreDestroy(surface: ?*FontCore) void {
    const s = surface orelse return;
    s.destroy();
}

pub fn fontCoreShapeText(
    surface: ?*FontCore,
    style: FontStyle,
    text: ?[*]const u8,
    text_len: usize,
    out: ?*BackendShapedText,
) bool {
    const s = surface orelse return false;
    const target = out orelse return false;
    if (text_len > 0 and text == null) return false;
    const bytes = if (text_len == 0) "" else text.?[0..text_len];
    if (!s.shapeText(style, bytes)) return false;
    target.* = .{
        .glyphs = if (s.shaped_buf.items.len == 0) null else s.shaped_buf.items.ptr,
        .glyph_count = s.shaped_buf.items.len,
    };
    return true;
}

pub fn fontCoreRasterizeGlyph(
    surface: ?*FontCore,
    style: FontStyle,
    glyph_id: u32,
    out: ?*BackendRasterizedGlyph,
) bool {
    const s = surface orelse return false;
    const target = out orelse return false;
    target.* = s.rasterizeGlyph(style, glyph_id);
    return true;
}

pub fn fontCoreSetDpr(surface: ?*FontCore, dpr: f32) void {
    const s = surface orelse return;
    s.setDpr(dpr);
}

pub fn fontCoreSetFont(
    surface: ?*FontCore,
    font_path: ?[*:0]const u8,
    font_family: ?[*:0]const u8,
    pixel_height: u32,
) i32 {
    const s = surface orelse return 0;
    return if (s.setFont(font_path, font_family, pixel_height)) 1 else 0;
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

pub fn fontStyleFromCell(style: BackendCellStyle) FontStyle {
    if (style.bold and style.italic) return .bold_italic;
    if (style.bold) return .bold;
    if (style.italic) return .italic;
    return .regular;
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

fn applyFaceTransform(face: ft.FT_Face, transform: ?ft.FT_Matrix) void {
    if (transform) |value| {
        var matrix = value;
        ft.FT_Set_Transform(face, &matrix, null);
        return;
    }
    ft.FT_Set_Transform(face, null, null);
}

fn fontconfigMatrixToFreeType(matrix: *const fc.FcMatrix) ?ft.FT_Matrix {
    const result = ft.FT_Matrix{
        .xx = fixedFromDouble(matrix.xx),
        .xy = fixedFromDouble(matrix.xy),
        .yx = fixedFromDouble(matrix.yx),
        .yy = fixedFromDouble(matrix.yy),
    };
    if (result.xx == fixedFromDouble(1.0) and
        result.xy == 0 and
        result.yx == 0 and
        result.yy == fixedFromDouble(1.0))
    {
        return null;
    }
    return result;
}

fn fixedFromDouble(value: f64) ft.FT_Fixed {
    return @intFromFloat(@round(value * 65536.0));
}

fn resolveFaceSet(
    explicit_path: ?[*:0]const u8,
    family: ?[*:0]const u8,
) ?FaceSet {
    if (common.optionalCString(explicit_path)) |path| {
        const regular = FaceDescriptor{ .path = allocator.dupeZ(u8, path) catch return null };
        errdefer {
            var owned = regular;
            owned.deinit();
        }
        var descriptors: [style_count]FaceDescriptor = undefined;
        var initialized: usize = 0;
        errdefer {
            var i: usize = 0;
            while (i < initialized) : (i += 1) descriptors[i].deinit();
        }
        while (initialized < style_count) : (initialized += 1) {
            descriptors[initialized] = regular.clone() orelse return null;
        }
        var owned_regular = regular;
        owned_regular.deinit();
        return .{ .descriptors = descriptors };
    }

    const requested = common.optionalCString(family) orelse "monospace";
    const regular = resolveFontconfigFace(requested, fc.FC_WEIGHT_REGULAR, fc.FC_SLANT_ROMAN) orelse return null;
    errdefer {
        var owned = regular;
        owned.deinit();
    }

    const bold = resolveFontconfigFace(requested, fc.FC_WEIGHT_BOLD, fc.FC_SLANT_ROMAN) orelse
        (regular.clone() orelse return null);
    errdefer {
        var owned = bold;
        owned.deinit();
    }

    const italic = resolveFontconfigFace(requested, fc.FC_WEIGHT_REGULAR, fc.FC_SLANT_ITALIC) orelse
        (regular.clone() orelse return null);
    errdefer {
        var owned = italic;
        owned.deinit();
    }

    const bold_italic = resolveFontconfigFace(requested, fc.FC_WEIGHT_BOLD, fc.FC_SLANT_ITALIC) orelse
        (italic.clone() orelse bold.clone() orelse regular.clone() orelse return null);

    return .{ .descriptors = .{ regular, bold, italic, bold_italic } };
}

fn resolveFontconfigFace(
    family: []const u8,
    weight: c_int,
    slant: c_int,
) ?FaceDescriptor {
    if (fc.FcInit() == 0) {
        std.log.warn("fontconfig initialization failed", .{});
        return null;
    }

    const requested = allocator.dupeZ(u8, family) catch return null;
    defer allocator.free(requested);

    const pattern = fc.FcPatternCreate() orelse return null;
    defer fc.FcPatternDestroy(pattern);

    if (fc.FcPatternAddString(pattern, fc.FC_FAMILY, @ptrCast(requested.ptr)) == 0) return null;
    if (fc.FcPatternAddInteger(pattern, fc.FC_WEIGHT, weight) == 0) return null;
    if (fc.FcPatternAddInteger(pattern, fc.FC_SLANT, slant) == 0) return null;
    if (fc.FcConfigSubstitute(null, pattern, fc.FcMatchPattern) == 0) return null;
    fc.FcDefaultSubstitute(pattern);

    var result: fc.FcResult = undefined;
    const matched = fc.FcFontMatch(null, pattern, &result) orelse return null;
    defer fc.FcPatternDestroy(matched);

    var file_ptr: [*c]fc.FcChar8 = null;
    if (fc.FcPatternGetString(matched, fc.FC_FILE, 0, &file_ptr) != fc.FcResultMatch or file_ptr == null) {
        std.log.warn("fontconfig returned no file for font family '{s}'", .{family});
        return null;
    }

    var index: c_int = 0;
    if (fc.FcPatternGetInteger(matched, fc.FC_INDEX, 0, &index) != fc.FcResultMatch) {
        index = 0;
    }

    var matrix_ptr: [*c]fc.FcMatrix = null;
    const transform = if (fc.FcPatternGetMatrix(matched, fc.FC_MATRIX, 0, &matrix_ptr) == fc.FcResultMatch and matrix_ptr != null)
        fontconfigMatrixToFreeType(matrix_ptr)
    else
        null;

    const path = std.mem.span(@as([*:0]const u8, @ptrCast(file_ptr)));
    if (path.len == 0) return null;
    return .{
        .path = allocator.dupeZ(u8, path) catch return null,
        .index = @intCast(index),
        .transform = transform,
    };
}

test "font style mapping" {
    var style: BackendCellStyle = std.mem.zeroes(BackendCellStyle);
    try std.testing.expectEqual(FontStyle.regular, fontStyleFromCell(style));
    style.bold = true;
    try std.testing.expectEqual(FontStyle.bold, fontStyleFromCell(style));
    style.italic = true;
    try std.testing.expectEqual(FontStyle.bold_italic, fontStyleFromCell(style));
    style.bold = false;
    try std.testing.expectEqual(FontStyle.italic, fontStyleFromCell(style));
}
