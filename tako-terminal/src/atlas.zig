const std = @import("std");
const common = @import("common.zig");
const font = @import("font.zig");

const allocator = common.allocator;
const BackendRasterizedGlyph = common.BackendRasterizedGlyph;

const atlas_width: u32 = 512;

pub const AtlasGlyph = struct {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    left_bearing: i32,
    top_bearing: i32,
};

pub const OwnedGlyphAtlas = struct {
    glyphs: std.AutoHashMap(u64, AtlasGlyph),
    pixels: std.ArrayList(u8) = .empty,
    width: u32 = atlas_width,
    height: u32 = 0,
    next_x: u32 = 0,
    next_y: u32 = 0,
    row_h: u32 = 0,
    generation: u64 = 0,

    pub fn init() OwnedGlyphAtlas {
        return .{ .glyphs = std.AutoHashMap(u64, AtlasGlyph).init(allocator) };
    }

    pub fn deinit(self: *OwnedGlyphAtlas) void {
        self.glyphs.deinit();
        self.pixels.deinit(allocator);
    }

    pub fn reset(self: *OwnedGlyphAtlas) void {
        self.glyphs.clearRetainingCapacity();
        self.pixels.clearRetainingCapacity();
        self.height = 0;
        self.next_x = 0;
        self.next_y = 0;
        self.row_h = 0;
        self.generation = self.generation +% 1;
    }

    pub fn ensureGlyph(
        self: *OwnedGlyphAtlas,
        surface: ?*font.FontCore,
        style: font.FontStyle,
        glyph_id: u32,
    ) !AtlasGlyph {
        const key = glyphKey(style, glyph_id);
        if (self.glyphs.get(key)) |glyph| return glyph;

        var raster: BackendRasterizedGlyph = std.mem.zeroes(BackendRasterizedGlyph);
        if (!font.fontCoreRasterizeGlyph(surface, style, glyph_id, &raster)) {
            return error.RasterizeFailed;
        }

        var glyph = AtlasGlyph{
            .x = 0,
            .y = 0,
            .w = raster.width,
            .h = raster.height,
            .left_bearing = raster.left_bearing,
            .top_bearing = raster.top_bearing,
        };

        if (raster.width == 0 or raster.height == 0 or raster.pixels == null) {
            try self.glyphs.put(key, glyph);
            return glyph;
        }

        if (raster.width > self.width) {
            try self.glyphs.put(key, glyph);
            return glyph;
        }

        if (self.next_x + raster.width > self.width) {
            self.next_y += self.row_h;
            self.next_x = 0;
            self.row_h = 0;
        }

        glyph.x = self.next_x;
        glyph.y = self.next_y;
        self.next_x += raster.width;
        self.row_h = @max(self.row_h, raster.height);
        const needed_h = @max(self.height, glyph.y + raster.height);
        try self.ensureHeight(needed_h);

        const expected_len = @as(usize, raster.width) * @as(usize, raster.height);
        const src_len = @min(expected_len, raster.pixel_len);
        const src = raster.pixels[0..src_len];
        var row: u32 = 0;
        while (row < raster.height) : (row += 1) {
            const src_start = @as(usize, row) * @as(usize, raster.width);
            if (src_start >= src.len) break;
            const src_end = @min(src_start + raster.width, src.len);
            const dst_start =
                (@as(usize, glyph.y + row) * @as(usize, self.width)) + @as(usize, glyph.x);
            @memcpy(self.pixels.items[dst_start .. dst_start + (src_end - src_start)], src[src_start..src_end]);
        }

        try self.glyphs.put(key, glyph);
        self.generation = self.generation +% 1;
        return glyph;
    }

    fn glyphKey(style: font.FontStyle, glyph_id: u32) u64 {
        return (@as(u64, @intFromEnum(style)) << 32) | @as(u64, glyph_id);
    }

    fn ensureHeight(self: *OwnedGlyphAtlas, height: u32) !void {
        if (height <= self.height) return;
        const old_len = self.pixels.items.len;
        const new_len = @as(usize, self.width) * @as(usize, height);
        try self.pixels.resize(allocator, new_len);
        @memset(self.pixels.items[old_len..], 0);
        self.height = height;
    }
};
