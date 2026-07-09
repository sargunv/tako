const std = @import("std");
const common = @import("common.zig");
const session = @import("session.zig");
const font = @import("font.zig");
const atlas = @import("atlas.zig");

const ghostty = common.ghostty;
const allocator = common.allocator;
const flat_uv = common.flat_uv;

const FramePlan = common.FramePlan;
const Vertex = common.Vertex;
const CursorState = common.CursorState;
const CellMetrics = common.CellMetrics;
const BackendRgb = common.BackendRgb;
const BackendShapedText = common.BackendShapedText;
const BackendFrameSnapshot = common.BackendFrameSnapshot;

const TerminalSession = session.TerminalSession;

pub fn writeLastPlan(s: ?*TerminalSession, out: ?*FramePlan) void {
    const target = out orelse return;
    const sess = s orelse {
        target.* = std.mem.zeroes(FramePlan);
        return;
    };
    target.* = sess.last_plan;
}

pub fn finalizeFramePlan(
    sess: *TerminalSession,
    plan: *FramePlan,
    snapshot: *const BackendFrameSnapshot,
    cursor: CursorState,
) bool {
    sess.frame_vertices.clearRetainingCapacity();

    var cell_w: f32 = 1.0;
    var cell_h: f32 = 1.0;
    var cell_ascent: f32 = 1.0;
    var cell: CellMetrics = undefined;
    if (session.sessionCellMetrics(sess, &cell)) {
        cell_w = @floatFromInt(cell.cell_width);
        cell_h = @floatFromInt(cell.cell_height);
        cell_ascent = @floatFromInt(cell.cell_ascent);
    }

    appendCellBackgroundVertices(sess, snapshot, cell_w, cell_h) catch return false;
    appendCellGlyphVertices(sess, snapshot, cell_w, cell_h, cell_ascent) catch return false;
    appendCellDecorationVertices(sess, snapshot, cell_w, cell_h) catch return false;
    appendPreeditGlyphVertices(sess, snapshot, cursor, cell_w, cell_h, cell_ascent) catch return false;
    appendPreeditOverlayVertices(sess, snapshot, cursor, cell_w, cell_h) catch return false;
    appendCursorVertices(sess, snapshot, cursor, cell_w, cell_h) catch return false;

    plan.clear_color = .{
        snapshot.colors.background.r,
        snapshot.colors.background.g,
        snapshot.colors.background.b,
        255,
    };
    plan.cell_w = cell_w;
    plan.cell_h = cell_h;
    plan.cols = snapshot.cols;
    plan.rows = snapshot.rows;
    plan.cursor_x = if (cursor.viewport_present) cursor.viewport_x else 0;
    plan.cursor_y = if (cursor.viewport_present) cursor.viewport_y else 0;
    plan.cursor_present = @intFromBool(cursor.viewport_present);
    plan.vertices = if (sess.frame_vertices.items.len == 0)
        null
    else
        sess.frame_vertices.items.ptr;
    plan.vertex_count = sess.frame_vertices.items.len;
    plan.atlas_w = sess.glyph_atlas.width;
    plan.atlas_h = sess.glyph_atlas.height;
    plan.atlas_pixels = if (sess.glyph_atlas.pixels.items.len == 0)
        null
    else
        sess.glyph_atlas.pixels.items.ptr;
    plan.atlas_generation = sess.glyph_atlas.generation;
    return true;
}

fn appendCellGlyphVertices(
    sess: *TerminalSession,
    snapshot: *const BackendFrameSnapshot,
    cell_w: f32,
    cell_h: f32,
    cell_ascent: f32,
) !void {
    if (snapshot.rows_ptr == null or snapshot.row_count == 0) return;

    const rows = snapshot.rows_ptr[0..snapshot.row_count];
    for (rows, 0..) |row, row_i| {
        if (row.cells == null or row.cell_count == 0) continue;

        const cells = row.cells[0..row.cell_count];
        const row_y = @as(f32, @floatFromInt(row_i)) * cell_h;
        const baseline = row_y + cell_ascent;
        for (cells, 0..) |cell, col_i| {
            if (!cell.text_visible or cell.grapheme_ptr == null or cell.grapheme_len == 0) continue;
            const col_x = @as(f32, @floatFromInt(col_i)) * cell_w;
            const bytes = cell.grapheme_ptr[0..cell.grapheme_len];
            try appendTextRunGlyphVertices(
                sess,
                bytes,
                font.fontStyleFromCell(cell.style),
                col_x,
                baseline,
                cell.text_fg,
                false,
            );
        }
    }
}

fn appendPreeditGlyphVertices(
    sess: *TerminalSession,
    snapshot: *const BackendFrameSnapshot,
    cursor: CursorState,
    cell_w: f32,
    cell_h: f32,
    cell_ascent: f32,
) !void {
    const preedit = sess.preedit orelse return;
    if (preedit.len == 0 or !cursor.viewport_present) return;
    if (cursor.viewport_x >= snapshot.cols or cursor.viewport_y >= snapshot.rows) return;

    const max_cells = @as(usize, snapshot.cols - cursor.viewport_x);
    if (max_cells == 0) return;

    const px = @as(f32, @floatFromInt(cursor.viewport_x)) * cell_w;
    const py = @as(f32, @floatFromInt(cursor.viewport_y)) * cell_h;
    try appendTextRunGlyphVertices(
        sess,
        preedit,
        .regular,
        px,
        py + cell_ascent,
        snapshot.colors.foreground,
        true,
    );
}

fn appendTextRunGlyphVertices(
    sess: *TerminalSession,
    text: []const u8,
    style: font.FontStyle,
    origin_x: f32,
    baseline: f32,
    color: BackendRgb,
    apply_offsets: bool,
) !void {
    if (text.len == 0) return;

    var shaped: BackendShapedText = std.mem.zeroes(BackendShapedText);
    if (!font.fontCoreShapeText(sess.surface, style, text.ptr, text.len, &shaped)) return;
    if (shaped.glyphs == null or shaped.glyph_count == 0) return;

    var pen_x = origin_x;
    const glyphs = shaped.glyphs[0..shaped.glyph_count];
    for (glyphs) |glyph| {
        const offset_x = if (apply_offsets) glyph.x_offset else 0.0;
        const offset_y = if (apply_offsets) glyph.y_offset else 0.0;
        const atlas_glyph = sess.glyph_atlas.ensureGlyph(sess.surface, style, glyph.glyph_id) catch {
            pen_x += glyph.x_advance;
            continue;
        };
        if (atlas_glyph.w > 0 and atlas_glyph.h > 0 and sess.glyph_atlas.height > 0) {
            const inv_w = 1.0 / @as(f32, @floatFromInt(sess.glyph_atlas.width));
            const inv_h = 1.0 / @as(f32, @floatFromInt(sess.glyph_atlas.height));
            const tex_u0 = @as(f32, @floatFromInt(atlas_glyph.x)) * inv_w;
            const tex_v0 = @as(f32, @floatFromInt(atlas_glyph.y)) * inv_h;
            const tex_u1 = @as(f32, @floatFromInt(atlas_glyph.x + atlas_glyph.w)) * inv_w;
            const tex_v1 = @as(f32, @floatFromInt(atlas_glyph.y + atlas_glyph.h)) * inv_h;
            try pushTexturedQuad(
                &sess.frame_vertices,
                pen_x + @as(f32, @floatFromInt(atlas_glyph.left_bearing)) + offset_x,
                baseline - @as(f32, @floatFromInt(atlas_glyph.top_bearing)) - offset_y,
                @floatFromInt(atlas_glyph.w),
                @floatFromInt(atlas_glyph.h),
                tex_u0,
                tex_v0,
                tex_u1,
                tex_v1,
                color,
            );
        }
        pen_x += glyph.x_advance;
    }
}

fn appendCellBackgroundVertices(
    sess: *TerminalSession,
    snapshot: *const BackendFrameSnapshot,
    cell_w: f32,
    cell_h: f32,
) !void {
    if (snapshot.rows_ptr == null or snapshot.row_count == 0) return;

    const defaults = common.EffectiveColors{
        .fg = snapshot.colors.foreground,
        .bg = snapshot.colors.background,
    };
    const rows = snapshot.rows_ptr[0..snapshot.row_count];
    for (rows, 0..) |row, row_i| {
        if (row.cells == null or row.cell_count == 0) continue;

        const cells = row.cells[0..row.cell_count];
        const row_y = @as(f32, @floatFromInt(row_i)) * cell_h;
        for (cells, 0..) |cell, col_i| {
            const colors = common.effectiveCellColors(cell, common.rowCellSelected(row, col_i), defaults);
            if (common.rgbEqual(colors.bg, defaults.bg)) continue;

            try pushFlatQuad(
                &sess.frame_vertices,
                @as(f32, @floatFromInt(col_i)) * cell_w,
                row_y,
                cell_w,
                cell_h,
                colors.bg,
            );
        }
    }
}

fn appendCellDecorationVertices(
    sess: *TerminalSession,
    snapshot: *const BackendFrameSnapshot,
    cell_w: f32,
    cell_h: f32,
) !void {
    if (snapshot.rows_ptr == null or snapshot.row_count == 0) return;

    const defaults = common.EffectiveColors{
        .fg = snapshot.colors.foreground,
        .bg = snapshot.colors.background,
    };
    const decoration_h = @max(cell_h * 0.075, 1.0);
    const rows = snapshot.rows_ptr[0..snapshot.row_count];
    for (rows, 0..) |row, row_i| {
        if (row.cells == null or row.cell_count == 0) continue;

        const cells = row.cells[0..row.cell_count];
        const row_y = @as(f32, @floatFromInt(row_i)) * cell_h;
        for (cells, 0..) |cell, col_i| {
            if (cell.style.invisible) continue;
            if (!cell.style.overline and !cell.style.strikethrough and !cell.style.underline) {
                continue;
            }

            const color = common.effectiveCellColors(cell, common.rowCellSelected(row, col_i), defaults).fg;
            const col_x = @as(f32, @floatFromInt(col_i)) * cell_w;
            if (cell.style.overline) {
                try pushFlatQuad(&sess.frame_vertices, col_x, row_y, cell_w, decoration_h, color);
            }
            if (cell.style.strikethrough) {
                try pushFlatQuad(
                    &sess.frame_vertices,
                    col_x,
                    row_y + (cell_h * 0.55),
                    cell_w,
                    decoration_h,
                    color,
                );
            }
            if (cell.style.underline) {
                try pushFlatQuad(
                    &sess.frame_vertices,
                    col_x,
                    row_y + cell_h - decoration_h,
                    cell_w,
                    decoration_h,
                    color,
                );
            }
        }
    }
}

fn appendPreeditOverlayVertices(
    sess: *TerminalSession,
    snapshot: *const BackendFrameSnapshot,
    cursor: CursorState,
    cell_w: f32,
    cell_h: f32,
) !void {
    const preedit = sess.preedit orelse return;
    if (preedit.len == 0 or !cursor.viewport_present) return;
    if (cursor.viewport_x >= snapshot.cols or cursor.viewport_y >= snapshot.rows) return;

    const max_cells = @as(usize, snapshot.cols - cursor.viewport_x);
    if (max_cells == 0) return;

    const px = @as(f32, @floatFromInt(cursor.viewport_x)) * cell_w;
    const py = @as(f32, @floatFromInt(cursor.viewport_y)) * cell_h;
    const fg = snapshot.colors.foreground;
    const underline_h = @max(cell_h * 0.08, 1.0);
    const text_cells = @min(@max(terminalDisplayWidth(preedit), 1), max_cells);

    try pushFlatQuad(
        &sess.frame_vertices,
        px,
        py + cell_h - underline_h,
        @as(f32, @floatFromInt(text_cells)) * cell_w,
        underline_h,
        fg,
    );

    const prefix_len = validUtf8PrefixLen(preedit, @min(sess.preedit_cursor_byte, preedit.len));
    const cursor_cells = @min(terminalDisplayWidth(preedit[0..prefix_len]), max_cells);
    try pushFlatQuad(
        &sess.frame_vertices,
        px + @as(f32, @floatFromInt(cursor_cells)) * cell_w,
        py,
        @max(cell_w * 0.1, 1.0),
        cell_h,
        fg,
    );
}

fn terminalDisplayWidth(bytes: []const u8) usize {
    if (bytes.len == 0) return 0;

    const view = std.unicode.Utf8View.init(bytes) catch return bytes.len;
    var iter = view.iterator();
    var codepoints = std.ArrayList(u32).empty;
    defer codepoints.deinit(allocator);

    while (iter.nextCodepoint()) |cp| {
        codepoints.append(allocator, @as(u32, @intCast(cp))) catch return bytes.len;
    }

    var total: usize = 0;
    var i: usize = 0;
    while (i < codepoints.items.len) {
        var width: u8 = 0;
        const consumed = ghostty.ghostty_unicode_grapheme_width(
            codepoints.items[i..].ptr,
            codepoints.items.len - i,
            &width,
        );
        if (consumed == 0) break;
        total += width;
        i += consumed;
    }
    return total;
}

fn validUtf8PrefixLen(bytes: []const u8, requested_len: usize) usize {
    var len = @min(requested_len, bytes.len);
    while (len > 0 and !std.unicode.utf8ValidateSlice(bytes[0..len])) {
        len -= 1;
    }
    return len;
}

fn appendCursorVertices(
    sess: *TerminalSession,
    snapshot: *const BackendFrameSnapshot,
    cursor: CursorState,
    cell_w: f32,
    cell_h: f32,
) !void {
    if (!cursor.visible or !cursor.viewport_present) return;

    const px = @as(f32, @floatFromInt(cursor.viewport_x)) * cell_w;
    const py = @as(f32, @floatFromInt(cursor.viewport_y)) * cell_h;
    const color = if (snapshot.colors.cursor_present)
        snapshot.colors.cursor
    else
        snapshot.colors.foreground;

    switch (cursor.style) {
        0 => try pushFlatQuad(
            &sess.frame_vertices,
            px,
            py,
            @max(cell_w * 0.125, 1.0),
            cell_h,
            color,
        ),
        2 => try pushFlatQuad(
            &sess.frame_vertices,
            px,
            py + cell_h - @max(cell_h * 0.125, 1.0),
            cell_w,
            @max(cell_h * 0.125, 1.0),
            color,
        ),
        3 => {
            const border = snapshot.colors.foreground;
            const thickness = @max(@min(cell_w, cell_h) * 0.1, 1.0);
            try pushFlatQuad(&sess.frame_vertices, px, py, cell_w, thickness, border);
            try pushFlatQuad(
                &sess.frame_vertices,
                px,
                py + cell_h - thickness,
                cell_w,
                thickness,
                border,
            );
            try pushFlatQuad(
                &sess.frame_vertices,
                px,
                py + thickness,
                thickness,
                cell_h - 2.0 * thickness,
                border,
            );
            try pushFlatQuad(
                &sess.frame_vertices,
                px + cell_w - thickness,
                py + thickness,
                thickness,
                cell_h - 2.0 * thickness,
                border,
            );
        },
        else => try pushFlatQuad(&sess.frame_vertices, px, py, cell_w, cell_h, color),
    }
}

fn pushTexturedQuad(
    vertices: *std.ArrayList(Vertex),
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    tex_u0: f32,
    tex_v0: f32,
    tex_u1: f32,
    tex_v1: f32,
    color: BackendRgb,
) !void {
    try vertices.append(allocator, .{
        .x = x,
        .y = y,
        .u = tex_u0,
        .v = tex_v0,
        .r = color.r,
        .g = color.g,
        .b = color.b,
        .a = 255,
    });
    try vertices.append(allocator, .{
        .x = x + w,
        .y = y,
        .u = tex_u1,
        .v = tex_v0,
        .r = color.r,
        .g = color.g,
        .b = color.b,
        .a = 255,
    });
    try vertices.append(allocator, .{
        .x = x + w,
        .y = y + h,
        .u = tex_u1,
        .v = tex_v1,
        .r = color.r,
        .g = color.g,
        .b = color.b,
        .a = 255,
    });
    try vertices.append(allocator, .{
        .x = x,
        .y = y + h,
        .u = tex_u0,
        .v = tex_v1,
        .r = color.r,
        .g = color.g,
        .b = color.b,
        .a = 255,
    });
}

fn pushFlatQuad(
    vertices: *std.ArrayList(Vertex),
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: BackendRgb,
) !void {
    try vertices.append(allocator, .{
        .x = x,
        .y = y,
        .u = flat_uv,
        .v = flat_uv,
        .r = color.r,
        .g = color.g,
        .b = color.b,
        .a = 255,
    });
    try vertices.append(allocator, .{
        .x = x + w,
        .y = y,
        .u = flat_uv,
        .v = flat_uv,
        .r = color.r,
        .g = color.g,
        .b = color.b,
        .a = 255,
    });
    try vertices.append(allocator, .{
        .x = x + w,
        .y = y + h,
        .u = flat_uv,
        .v = flat_uv,
        .r = color.r,
        .g = color.g,
        .b = color.b,
        .a = 255,
    });
    try vertices.append(allocator, .{
        .x = x,
        .y = y + h,
        .u = flat_uv,
        .v = flat_uv,
        .r = color.r,
        .g = color.g,
        .b = color.b,
        .a = 255,
    });
}

pub fn cursorStatesEqual(a: CursorState, b: CursorState) bool {
    if (!a.valid or !b.valid) return false;
    return a.visible == b.visible and
        a.viewport_present == b.viewport_present and
        a.viewport_x == b.viewport_x and
        a.viewport_y == b.viewport_y and
        a.wide_tail == b.wide_tail and
        a.style == b.style and
        a.blinking == b.blinking and
        a.password_input == b.password_input;
}

fn cursorShouldBeHidden(cursor: CursorState, focused: bool, blink_visible: bool) bool {
    return cursor.valid and
        cursor.visible and
        cursor.blinking and
        focused and
        !cursor.password_input and
        !blink_visible;
}

pub fn presentedCursorState(cursor: CursorState, focused: bool, blink_visible: bool) CursorState {
    var presented = cursor;
    if (cursorShouldBeHidden(cursor, focused, blink_visible)) {
        presented.visible = false;
    }
    return presented;
}
