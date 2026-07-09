const std = @import("std");
const common = @import("common.zig");
const font = @import("font.zig");
const atlas = @import("atlas.zig");
const pty = @import("pty.zig");

const ghostty = common.ghostty;
const allocator = common.allocator;

const FramePlan = common.FramePlan;
const Vertex = common.Vertex;
const CursorState = common.CursorState;
const MouseGeometry = common.MouseGeometry;
const CellMetrics = common.CellMetrics;
const version_string = common.version_string;
const enq_response = common.enq_response;

pub const TerminalSession = struct {
    terminal: ghostty.GhosttyTerminal,
    render_state: ghostty.GhosttyRenderState,
    surface: ?*font.FontCore,
    cols: u16,
    rows: u16,
    pty: ?*pty.PtySession,
    pty_response: std.ArrayList(u8),
    key_encoder: ghostty.GhosttyKeyEncoder,
    key_event: ghostty.GhosttyKeyEvent,
    mouse_encoder: ghostty.GhosttyMouseEncoder,
    mouse_event: ghostty.GhosttyMouseEvent,
    selection_gesture: ghostty.GhosttySelectionGesture,
    selection_press: ghostty.GhosttySelectionGestureEvent,
    selection_drag: ghostty.GhosttySelectionGestureEvent,
    selection_release: ghostty.GhosttySelectionGestureEvent,
    selection_autoscroll_tick: ghostty.GhosttySelectionGestureEvent,
    title: ?[]u8 = null,
    pwd: ?[]u8 = null,
    focused: bool = false,
    cursor_blink_visible: bool = true,
    preedit: ?[]u8 = null,
    preedit_cursor_byte: usize = 0,
    needs_replan: bool = true,
    last_cursor: CursorState = std.mem.zeroes(CursorState),
    last_plan: FramePlan = std.mem.zeroes(FramePlan),
    frame_vertices: std.ArrayList(Vertex) = .empty,
    glyph_atlas: atlas.OwnedGlyphAtlas,
    pending_bell_count: u32 = 0,
};

pub fn terminalHandle(session: ?*TerminalSession) ghostty.GhosttyTerminal {
    const ptr = userdata orelse return null;
    return @ptrCast(@alignCast(ptr));
}

fn effectWritePty(
    _: ghostty.GhosttyTerminal,
    userdata: ?*anyopaque,
    data: ?[*]const u8,
    len: usize,
) callconv(.c) void {
    if (len == 0) return;
    const s = sessionFromUserdata(userdata) orelse return;
    const ptr = data orelse return;
    s.pty_response.appendSlice(allocator, ptr[0..len]) catch {};
}

fn effectBell(_: ghostty.GhosttyTerminal, userdata: ?*anyopaque) callconv(.c) void {
    addPendingBellCount(sessionFromUserdata(userdata), 1);
}

fn effectChanged(_: ghostty.GhosttyTerminal, _: ?*anyopaque) callconv(.c) void {}

fn effectXtversion(_: ghostty.GhosttyTerminal, _: ?*anyopaque) callconv(.c) ghostty.GhosttyString {
    return .{ .ptr = version_string.ptr, .len = version_string.len };
}

fn effectEnquiry(_: ghostty.GhosttyTerminal, _: ?*anyopaque) callconv(.c) ghostty.GhosttyString {
    return .{ .ptr = enq_response.ptr, .len = enq_response.len };
}

fn effectDeviceAttributes(
    _: ghostty.GhosttyTerminal,
    _: ?*anyopaque,
    out: ?*ghostty.GhosttyDeviceAttributes,
) callconv(.c) bool {
    const attrs = out orelse return false;
    attrs.primary.conformance_level = 62;
    attrs.primary.num_features = 1;
    attrs.primary.features[0] = 22;
    attrs.secondary.device_type = 1;
    attrs.secondary.firmware_version = 1;
    attrs.secondary.rom_cartridge = 0;
    attrs.tertiary.unit_id = 0;
    return true;
}

fn effectSize(
    _: ghostty.GhosttyTerminal,
    userdata: ?*anyopaque,
    out: ?*ghostty.GhosttySizeReportSize,
) callconv(.c) bool {
    const s = sessionFromUserdata(userdata) orelse return false;
    const size = out orelse return false;
    var geometry: MouseGeometry = undefined;
    if (!sessionMouseGeometry(s, &geometry)) return false;
    size.columns = @intCast(@max(@divFloor(geometry.screen_width, @max(geometry.cell_width, 1)), 1));
    size.rows = @intCast(@max(@divFloor(geometry.screen_height, @max(geometry.cell_height, 1)), 1));
    size.cell_width = geometry.cell_width;
    size.cell_height = geometry.cell_height;
    return true;
}

fn effectColorScheme(
    _: ghostty.GhosttyTerminal,
    _: ?*anyopaque,
    out: ?*ghostty.GhosttyColorScheme,
) callconv(.c) bool {
    const scheme = out orelse return false;
    scheme.* = @intCast(ghostty.GHOSTTY_COLOR_SCHEME_DARK);
    return true;
}

fn setTerminalCallback(
    t: ghostty.GhosttyTerminal,
    option: ghostty.GhosttyTerminalOption,
    callback: anytype,
) void {
    _ = ghostty.ghostty_terminal_set(t, option, @ptrCast(callback));
}

pub fn registerTerminalEffects(session: *TerminalSession) void {
    const t = session.terminal;
    _ = ghostty.ghostty_terminal_set(
        t,
        @intCast(ghostty.GHOSTTY_TERMINAL_OPT_USERDATA),
        @ptrCast(session),
    );
    setTerminalCallback(t, @intCast(ghostty.GHOSTTY_TERMINAL_OPT_WRITE_PTY), &effectWritePty);
    setTerminalCallback(t, @intCast(ghostty.GHOSTTY_TERMINAL_OPT_BELL), &effectBell);
    setTerminalCallback(t, @intCast(ghostty.GHOSTTY_TERMINAL_OPT_TITLE_CHANGED), &effectChanged);
    setTerminalCallback(t, @intCast(ghostty.GHOSTTY_TERMINAL_OPT_PWD_CHANGED), &effectChanged);
    setTerminalCallback(t, @intCast(ghostty.GHOSTTY_TERMINAL_OPT_XTVERSION), &effectXtversion);
    setTerminalCallback(t, @intCast(ghostty.GHOSTTY_TERMINAL_OPT_ENQUIRY), &effectEnquiry);
    setTerminalCallback(t, @intCast(ghostty.GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES), &effectDeviceAttributes);
    setTerminalCallback(t, @intCast(ghostty.GHOSTTY_TERMINAL_OPT_SIZE), &effectSize);
    setTerminalCallback(t, @intCast(ghostty.GHOSTTY_TERMINAL_OPT_COLOR_SCHEME), &effectColorScheme);
}

pub fn flushPtyResponses(session: *TerminalSession) void {
    if (session.pty_response.items.len == 0) return;
    const p = session.pty orelse return;
    p.write(session.pty_response.items);
    session.pty_response.clearRetainingCapacity();
}

pub fn sessionSurface(session: ?*TerminalSession) ?*font.FontCore {
    const s = session orelse return null;
    return s.surface;
}

pub fn sessionCellMetrics(session: ?*TerminalSession, out: *CellMetrics) bool {
    const s = session orelse return false;
    return font.fontCoreCellMetrics(s.surface, out);
}

pub fn sessionMouseGeometry(session: ?*TerminalSession, out: *MouseGeometry) bool {
    const s = session orelse return false;
    var cell: CellMetrics = undefined;
    if (!sessionCellMetrics(s, &cell)) return false;
    out.* = .{
        .screen_width = @as(u32, s.cols) * cell.cell_width,
        .screen_height = @as(u32, s.rows) * cell.cell_height,
        .cell_width = cell.cell_width,
        .cell_height = cell.cell_height,
    };
    return true;
}

pub fn gridForPixels(cell: CellMetrics, width_px: u32, height_px: u32) struct { cols: u16, rows: u16 } {
    const cw = @max(cell.cell_width, 1);
    const ch = @max(cell.cell_height, 1);
    const max_u16 = std.math.maxInt(u16);
    return .{
        .cols = @intCast(@min(@max(@divFloor(width_px, cw), 1), max_u16)),
        .rows = @intCast(@min(@max(@divFloor(height_px, ch), 1), max_u16)),
    };
}

pub fn markNeedsReplan(session: ?*TerminalSession) void {
    const s = session orelse return;
    s.needs_replan = true;
}

pub fn setSessionFocused(session: ?*TerminalSession, focused: bool) void {
    const s = session orelse return;
    if (s.focused == focused) return;
    s.focused = focused;
    markNeedsReplan(s);
}

pub fn setSessionCursorBlinkVisible(session: ?*TerminalSession, visible: bool) void {
    const s = session orelse return;
    if (s.cursor_blink_visible == visible) return;
    s.cursor_blink_visible = visible;
    markNeedsReplan(s);
}

pub fn setSessionPreedit(
    session: ?*TerminalSession,
    data: ?[*]const u8,
    len: usize,
    cursor_byte: usize,
) void {
    const s = session orelse return;
    if (len == 0) {
        if (s.preedit == null and s.preedit_cursor_byte == 0) return;
        freeOptionalBytes(&s.preedit);
        s.preedit_cursor_byte = 0;
        markNeedsReplan(s);
        return;
    }
    const ptr = data orelse return;
    const incoming = ptr[0..len];
    if (s.preedit) |current| {
        if (std.mem.eql(u8, current, incoming) and s.preedit_cursor_byte == cursor_byte) return;
    }
    const owned = allocator.dupe(u8, incoming) catch return;
    freeOptionalBytes(&s.preedit);
    s.preedit = owned;
    s.preedit_cursor_byte = @min(cursor_byte, owned.len);
    markNeedsReplan(s);
}

pub fn writeSessionBytes(session: ?*TerminalSession, bytes: []const u8) void {
    if (bytes.len == 0) return;
    const s = session orelse return;
    const p = s.pty orelse return;
    p.write(bytes);
}

pub fn terminalMode(session: ?*TerminalSession, mode: ghostty.GhosttyMode) bool {
    const t = terminalHandle(session);
    if (t == null) return false;

    var enabled = false;
    const result = ghostty.ghostty_terminal_mode_get(t, mode, &enabled);
    return result == ghostty.GHOSTTY_SUCCESS and enabled;
}

pub fn addPendingBellCount(session: ?*TerminalSession, count: u32) void {
    if (count == 0) return;
    const s = session orelse return;
    const max = std.math.maxInt(u32);
    s.pending_bell_count = if (count > max - s.pending_bell_count)
        max
    else
        s.pending_bell_count + count;
}

pub fn terminalOptionForColorRole(role: u32) ?ghostty.GhosttyTerminalOption {
    return switch (role) {
        0 => @intCast(ghostty.GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND),
        1 => @intCast(ghostty.GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND),
        2 => @intCast(ghostty.GHOSTTY_TERMINAL_OPT_COLOR_CURSOR),
        else => null,
    };
}

pub fn cursorStyle(style: u32) ghostty.GhosttyTerminalCursorStyle {
    return switch (style) {
        0 => @intCast(ghostty.GHOSTTY_TERMINAL_CURSOR_STYLE_BAR),
        2 => @intCast(ghostty.GHOSTTY_TERMINAL_CURSOR_STYLE_UNDERLINE),
        3 => @intCast(ghostty.GHOSTTY_TERMINAL_CURSOR_STYLE_BLOCK_HOLLOW),
        else => @intCast(ghostty.GHOSTTY_TERMINAL_CURSOR_STYLE_BLOCK),
    };
}

pub fn terminalDataBool(session: ?*TerminalSession, data: ghostty.GhosttyTerminalData) bool {
    const t = terminalHandle(session);
    if (t == null) return false;

    var enabled = false;
    const result = ghostty.ghostty_terminal_get(t, data, &enabled);
    return result == ghostty.GHOSTTY_SUCCESS and enabled;
}

pub fn terminalDataString(session: ?*TerminalSession, data: ghostty.GhosttyTerminalData) ?[]const u8 {
    const t = terminalHandle(session);
    if (t == null) return null;

    var value: ghostty.GhosttyString = undefined;
    const result = ghostty.ghostty_terminal_get(t, data, &value);
    if (result != ghostty.GHOSTTY_SUCCESS or value.ptr == null) return null;
    return value.ptr[0..value.len];
}

pub fn freeOptionalBytes(bytes: *?[]u8) void {
    if (bytes.*) |owned| {
        allocator.free(owned);
        bytes.* = null;
    }
}

pub fn takeChangedTerminalString(
    session: ?*TerminalSession,
    data: ghostty.GhosttyTerminalData,
    cache: *?[]u8,
    out_buf: ?[*]u8,
    cap: usize,
) usize {
    const current = terminalDataString(session, data) orelse return 0;
    if (cache.*) |cached| {
        if (std.mem.eql(u8, cached, current)) return 0;
    } else if (current.len == 0) {
        return 0;
    }

    const out = out_buf orelse return 0;
    if (cap == 0 or current.len + 1 > cap) return 0;
    const owned = allocator.dupe(u8, current) catch return 0;
    errdefer allocator.free(owned);

    @memcpy(out[0..current.len], current);
    out[current.len] = 0;

    freeOptionalBytes(cache);
    cache.* = owned;
    return current.len;
}
