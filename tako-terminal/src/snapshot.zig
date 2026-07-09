const std = @import("std");
const common = @import("common.zig");
const session = @import("session.zig");

const ghostty = common.ghostty;
const allocator = common.allocator;

const CursorState = common.CursorState;
const FrameState = common.FrameState;
const RowSelection = common.RowSelection;
const BackendRgb = common.BackendRgb;
const BackendColors = common.BackendColors;
const BackendCell = common.BackendCell;
const BackendRow = common.BackendRow;
const BackendFrameSnapshot = common.BackendFrameSnapshot;

const TerminalSession = session.TerminalSession;

const GraphemeBytes = struct {
    bytes: []u8,
    len: usize,
};

pub const SnapshotBuffers = struct {
    rows: std.ArrayList(BackendRow) = .empty,
    cell_chunks: std.ArrayList([]BackendCell) = .empty,
    grapheme_chunks: std.ArrayList([]u8) = .empty,

    pub fn deinit(self: *SnapshotBuffers) void {
        for (self.grapheme_chunks.items) |bytes| {
            allocator.free(bytes);
        }
        for (self.cell_chunks.items) |cells| {
            allocator.free(cells);
        }
        self.rows.deinit(allocator);
        self.cell_chunks.deinit(allocator);
        self.grapheme_chunks.deinit(allocator);
    }
};

pub fn freeTerminalCore(
    t: ghostty.GhosttyTerminal,
    render_state: ghostty.GhosttyRenderState,
) void {
    if (render_state != null) {
        ghostty.ghostty_render_state_free(render_state);
    }
    if (t != null) {
        ghostty.ghostty_terminal_free(t);
    }
}

pub fn clearRenderStateDirty(render_state: ghostty.GhosttyRenderState) void {
    if (render_state == null) return;
    var clean: ghostty.GhosttyRenderStateDirty = @intCast(ghostty.GHOSTTY_RENDER_STATE_DIRTY_FALSE);
    const result = ghostty.ghostty_render_state_set(
        render_state,
        @intCast(ghostty.GHOSTTY_RENDER_STATE_OPTION_DIRTY),
        &clean,
    );
    if (result != ghostty.GHOSTTY_SUCCESS) {
        std.log.warn("ghostty_render_state_set(DIRTY=false) failed: {d}", .{result});
    }
}

pub fn updateRenderState(
    render_state: ghostty.GhosttyRenderState,
    t: ghostty.GhosttyTerminal,
) bool {
    if (render_state == null or t == null) return false;
    const result = ghostty.ghostty_render_state_update(render_state, t);
    if (result != ghostty.GHOSTTY_SUCCESS) {
        std.log.warn("ghostty_render_state_update failed: {d}", .{result});
        return false;
    }
    return true;
}

fn renderStateGet(
    render_state: ghostty.GhosttyRenderState,
    data: ghostty.GhosttyRenderStateData,
    out: *anyopaque,
) bool {
    const result = ghostty.ghostty_render_state_get(render_state, data, out);
    if (result != ghostty.GHOSTTY_SUCCESS) {
        std.log.warn("ghostty_render_state_get({d}) failed: {d}", .{ data, result });
        return false;
    }
    return true;
}

fn renderStateGetBool(
    render_state: ghostty.GhosttyRenderState,
    data: ghostty.GhosttyRenderStateData,
) ?bool {
    var value = false;
    if (!renderStateGet(render_state, data, &value)) return null;
    return value;
}

fn renderStateGetU16(
    render_state: ghostty.GhosttyRenderState,
    data: ghostty.GhosttyRenderStateData,
) ?u16 {
    var value: u16 = 0;
    if (!renderStateGet(render_state, data, &value)) return null;
    return value;
}

fn renderStateGetDirty(render_state: ghostty.GhosttyRenderState) ?ghostty.GhosttyRenderStateDirty {
    var value: ghostty.GhosttyRenderStateDirty = @intCast(ghostty.GHOSTTY_RENDER_STATE_DIRTY_FALSE);
    if (!renderStateGet(
        render_state,
        @intCast(ghostty.GHOSTTY_RENDER_STATE_DATA_DIRTY),
        &value,
    )) return null;
    return value;
}

fn renderStateGetCursorStyle(render_state: ghostty.GhosttyRenderState) ?u32 {
    var value: ghostty.GhosttyRenderStateCursorVisualStyle =
        @intCast(ghostty.GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK);
    if (!renderStateGet(
        render_state,
        @intCast(ghostty.GHOSTTY_RENDER_STATE_DATA_CURSOR_VISUAL_STYLE),
        &value,
    )) return null;
    return switch (value) {
        ghostty.GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BAR => 0,
        ghostty.GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK => 1,
        ghostty.GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE => 2,
        ghostty.GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK_HOLLOW => 3,
        else => 1,
    };
}

pub fn captureFrameState(s: ?*TerminalSession) ?FrameState {
    const sess = s orelse return null;
    if (!updateRenderState(sess.render_state, sess.terminal)) return null;

    const dirty = renderStateGetDirty(sess.render_state) orelse return null;
    const visible = renderStateGetBool(
        sess.render_state,
        @intCast(ghostty.GHOSTTY_RENDER_STATE_DATA_CURSOR_VISIBLE),
    ) orelse return null;
    const viewport_present = renderStateGetBool(
        sess.render_state,
        @intCast(ghostty.GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_HAS_VALUE),
    ) orelse return null;
    const viewport_x = if (viewport_present)
        renderStateGetU16(
            sess.render_state,
            @intCast(ghostty.GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_X),
        ) orelse return null
    else
        0;
    const viewport_y = if (viewport_present)
        renderStateGetU16(
            sess.render_state,
            @intCast(ghostty.GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_Y),
        ) orelse return null
    else
        0;
    const wide_tail = viewport_present and (renderStateGetBool(
        sess.render_state,
        @intCast(ghostty.GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_WIDE_TAIL),
    ) orelse return null);
    const style = renderStateGetCursorStyle(sess.render_state) orelse return null;
    const blinking = renderStateGetBool(
        sess.render_state,
        @intCast(ghostty.GHOSTTY_RENDER_STATE_DATA_CURSOR_BLINKING),
    ) orelse return null;
    const password_input = renderStateGetBool(
        sess.render_state,
        @intCast(ghostty.GHOSTTY_RENDER_STATE_DATA_CURSOR_PASSWORD_INPUT),
    ) orelse return null;

    return .{
        .dirty = @intCast(dirty),
        .content_dirty = dirty != ghostty.GHOSTTY_RENDER_STATE_DIRTY_FALSE,
        .cursor = .{
            .valid = true,
            .visible = visible,
            .viewport_present = viewport_present,
            .viewport_x = viewport_x,
            .viewport_y = viewport_y,
            .wide_tail = wide_tail,
            .style = style,
            .blinking = blinking,
            .password_input = password_input,
        },
    };
}

pub fn captureFrameSnapshot(
    s: ?*TerminalSession,
    frame_state: FrameState,
    buffers: *SnapshotBuffers,
) ?BackendFrameSnapshot {
    const sess = s orelse return null;
    const render_state = sess.render_state;
    if (render_state == null) return null;

    const cols = renderStateGetU16(
        render_state,
        @intCast(ghostty.GHOSTTY_RENDER_STATE_DATA_COLS),
    ) orelse return null;
    const rows = renderStateGetU16(
        render_state,
        @intCast(ghostty.GHOSTTY_RENDER_STATE_DATA_ROWS),
    ) orelse return null;

    const colors = captureBackendColors(render_state) orelse return null;
    if (!captureBackendRows(render_state, buffers, colors)) return null;

    return .{
        .cols = cols,
        .rows = rows,
        .dirty = frame_state.dirty,
        .colors = colors,
        .rows_ptr = if (buffers.rows.items.len == 0) null else buffers.rows.items.ptr,
        .row_count = buffers.rows.items.len,
    };
}

fn captureBackendColors(render_state: ghostty.GhosttyRenderState) ?BackendColors {
    var raw: ghostty.GhosttyRenderStateColors = std.mem.zeroes(ghostty.GhosttyRenderStateColors);
    raw.size = @sizeOf(ghostty.GhosttyRenderStateColors);
    const result = ghostty.ghostty_render_state_colors_get(render_state, &raw);
    if (result != ghostty.GHOSTTY_SUCCESS) {
        std.log.warn("ghostty_render_state_colors_get failed: {d}", .{result});
        return null;
    }

    var palette: [256]BackendRgb = undefined;
    for (&palette, raw.palette) |*dst, src| {
        dst.* = common.rgbToBackend(src);
    }

    return .{
        .foreground = common.rgbToBackend(raw.foreground),
        .background = common.rgbToBackend(raw.background),
        .cursor_present = raw.cursor_has_value,
        .cursor = common.rgbToBackend(raw.cursor),
        .palette = palette,
    };
}

fn captureBackendRows(
    render_state: ghostty.GhosttyRenderState,
    buffers: *SnapshotBuffers,
    colors: BackendColors,
) bool {
    var iter: ghostty.GhosttyRenderStateRowIterator = null;
    var result = ghostty.ghostty_render_state_row_iterator_new(null, &iter);
    if (result != ghostty.GHOSTTY_SUCCESS or iter == null) {
        std.log.warn("ghostty_render_state_row_iterator_new failed: {d}", .{result});
        return false;
    }
    defer ghostty.ghostty_render_state_row_iterator_free(iter);

    result = ghostty.ghostty_render_state_get(
        render_state,
        @intCast(ghostty.GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR),
        @ptrCast(&iter),
    );
    if (result != ghostty.GHOSTTY_SUCCESS) {
        std.log.warn("render_state_get(ROW_ITERATOR) failed: {d}", .{result});
        return false;
    }

    var cells_handle: ghostty.GhosttyRenderStateRowCells = null;
    result = ghostty.ghostty_render_state_row_cells_new(null, &cells_handle);
    if (result != ghostty.GHOSTTY_SUCCESS or cells_handle == null) {
        std.log.warn("ghostty_render_state_row_cells_new failed: {d}", .{result});
        return false;
    }
    defer ghostty.ghostty_render_state_row_cells_free(cells_handle);

    while (ghostty.ghostty_render_state_row_iterator_next(iter)) {
        const dirty = rowGetBool(iter, @intCast(ghostty.GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY)) orelse return false;

        result = ghostty.ghostty_render_state_row_get(
            iter,
            @intCast(ghostty.GHOSTTY_RENDER_STATE_ROW_DATA_CELLS),
            @ptrCast(&cells_handle),
        );
        if (result != ghostty.GHOSTTY_SUCCESS) {
            std.log.warn("row_get(CELLS) failed: {d}", .{result});
            return false;
        }

        const selection = rowGetSelection(iter);
        var row_cells: std.ArrayList(BackendCell) = .empty;
        while (ghostty.ghostty_render_state_row_cells_next(cells_handle)) {
            const col = row_cells.items.len;
            const cell = captureBackendCell(
                cells_handle,
                buffers,
                colors,
                selection != null and common.rowCellSelectedRaw(selection.?, col),
            ) orelse {
                row_cells.deinit(allocator);
                return false;
            };
            row_cells.append(allocator, cell) catch {
                row_cells.deinit(allocator);
                return false;
            };
        }
        const cells = row_cells.toOwnedSlice(allocator) catch {
            row_cells.deinit(allocator);
            return false;
        };
        buffers.cell_chunks.append(allocator, cells) catch {
            allocator.free(cells);
            return false;
        };

        const clean = false;
        _ = ghostty.ghostty_render_state_row_set(
            iter,
            @intCast(ghostty.GHOSTTY_RENDER_STATE_ROW_OPTION_DIRTY),
            &clean,
        );

        buffers.rows.append(allocator, .{
            .dirty = dirty,
            .selection_present = selection != null,
            .selection_start_x = if (selection) |sel| sel.start_x else 0,
            .selection_end_x = if (selection) |sel| sel.end_x else 0,
            .cells = if (cells.len == 0) null else cells.ptr,
            .cell_count = cells.len,
        }) catch return false;
    }

    return true;
}

fn captureBackendCell(
    cells: ghostty.GhosttyRenderStateRowCells,
    buffers: *SnapshotBuffers,
    colors: BackendColors,
    selected: bool,
) ?BackendCell {
    const grapheme = captureGraphemeBytes(cells) orelse return null;
    if (grapheme.bytes.len > 0) {
        buffers.grapheme_chunks.append(allocator, grapheme.bytes) catch {
            allocator.free(grapheme.bytes);
            return null;
        };
    }

    var style: ghostty.GhosttyStyle = std.mem.zeroes(ghostty.GhosttyStyle);
    style.size = @sizeOf(ghostty.GhosttyStyle);
    const result = ghostty.ghostty_render_state_row_cells_get(
        cells,
        @intCast(ghostty.GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE),
        &style,
    );
    if (result != ghostty.GHOSTTY_SUCCESS) {
        std.log.warn("row_cells_get(STYLE) failed: {d}", .{result});
        return null;
    }

    const fg = cellColor(cells, @intCast(ghostty.GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR));
    const bg = cellColor(cells, @intCast(ghostty.GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR));

    var out = BackendCell{
        .grapheme_ptr = if (grapheme.len == 0) null else grapheme.bytes.ptr,
        .grapheme_len = grapheme.len,
        .style = .{
            .bold = style.bold,
            .italic = style.italic,
            .faint = style.faint,
            .blink = style.blink,
            .inverse = style.inverse,
            .invisible = style.invisible,
            .strikethrough = style.strikethrough,
            .overline = style.overline,
            .underline = style.underline != 0,
        },
        .text_visible = false,
        .text_fg = common.rgbZero(),
        .fg_present = fg != null,
        .fg = fg orelse common.rgbZero(),
        .bg_present = bg != null,
        .bg = bg orelse common.rgbZero(),
    };
    const defaults = common.EffectiveColors{ .fg = colors.foreground, .bg = colors.background };
    const effective = common.effectiveCellColors(out, selected, defaults);
    out.text_visible = !out.style.invisible and out.grapheme_len > 0;
    out.text_fg = effective.fg;
    return out;
}

fn captureGraphemeBytes(cells: ghostty.GhosttyRenderStateRowCells) ?GraphemeBytes {
    var probe = ghostty.GhosttyBuffer{
        .ptr = null,
        .cap = 0,
        .len = 0,
    };
    _ = ghostty.ghostty_render_state_row_cells_get(
        cells,
        @intCast(ghostty.GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8),
        &probe,
    );
    if (probe.len == 0) {
        return .{ .bytes = &[_]u8{}, .len = 0 };
    }

    const bytes = allocator.alloc(u8, probe.len) catch return null;
    var buf = ghostty.GhosttyBuffer{
        .ptr = bytes.ptr,
        .cap = bytes.len,
        .len = 0,
    };
    const result = ghostty.ghostty_render_state_row_cells_get(
        cells,
        @intCast(ghostty.GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8),
        &buf,
    );
    if (result != ghostty.GHOSTTY_SUCCESS) {
        allocator.free(bytes);
        std.log.warn("row_cells_get(GRAPHEMES_UTF8) failed: {d}", .{result});
        return null;
    }
    return .{ .bytes = bytes, .len = @min(buf.len, bytes.len) };
}

fn cellColor(
    cells: ghostty.GhosttyRenderStateRowCells,
    data: ghostty.GhosttyRenderStateRowCellsData,
) ?BackendRgb {
    var raw: ghostty.GhosttyColorRgb = std.mem.zeroes(ghostty.GhosttyColorRgb);
    const result = ghostty.ghostty_render_state_row_cells_get(cells, data, &raw);
    if (result == ghostty.GHOSTTY_SUCCESS) {
        return common.rgbToBackend(raw);
    }
    return null;
}

fn rowGetSelection(iter: ghostty.GhosttyRenderStateRowIterator) ?RowSelection {
    var raw: ghostty.GhosttyRenderStateRowSelection = std.mem.zeroes(ghostty.GhosttyRenderStateRowSelection);
    raw.size = @sizeOf(ghostty.GhosttyRenderStateRowSelection);
    const result = ghostty.ghostty_render_state_row_get(
        iter,
        @intCast(ghostty.GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION),
        &raw,
    );
    if (result == ghostty.GHOSTTY_SUCCESS) {
        return .{ .start_x = raw.start_x, .end_x = raw.end_x };
    }
    return null;
}

fn rowGetBool(
    iter: ghostty.GhosttyRenderStateRowIterator,
    data: ghostty.GhosttyRenderStateRowData,
) ?bool {
    var value = false;
    const result = ghostty.ghostty_render_state_row_get(iter, data, &value);
    if (result != ghostty.GHOSTTY_SUCCESS) {
        std.log.warn("row_get(bool {d}) failed: {d}", .{ data, result });
        return null;
    }
    return value;
}
