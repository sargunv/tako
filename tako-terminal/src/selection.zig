const std = @import("std");
const common = @import("common.zig");
const session = @import("session.zig");

const ghostty = common.ghostty;
const allocator = common.allocator;
const MouseGeometry = common.MouseGeometry;

const TerminalBytes = common.TerminalBytes;
const TerminalSession = session.TerminalSession;

pub fn scrollViewport(s: ?*TerminalSession, behavior: ghostty.GhosttyTerminalScrollViewport) void {
    const t = session.terminalHandle(s);
    if (t == null) return;

    ghostty.ghostty_terminal_scroll_viewport(t, behavior);
    session.markNeedsReplan(s);
}

pub fn gridRefAtPixels(s: ?*TerminalSession, x_px: f32, y_px: f32) ?ghostty.GhosttyGridRef {
    const t = session.terminalHandle(s);
    if (t == null) return null;

    var geometry: MouseGeometry = undefined;
    if (!session.sessionMouseGeometry(s, &geometry)) return null;
    const cell_width = geometry.cell_width;
    const cell_height = geometry.cell_height;
    if (cell_width == 0 or cell_height == 0) return null;

    const cols = @max(@divFloor(geometry.screen_width, cell_width), 1);
    const rows = @max(@divFloor(geometry.screen_height, cell_height), 1);
    const local_x = @max(x_px - @as(f32, @floatFromInt(geometry.padding_left)), 0.0);
    const local_y = @max(y_px - @as(f32, @floatFromInt(geometry.padding_top)), 0.0);
    const raw_col: u32 = @intFromFloat(@floor(local_x / @as(f32, @floatFromInt(cell_width))));
    const raw_row: u32 = @intFromFloat(@floor(local_y / @as(f32, @floatFromInt(cell_height))));
    const col: u16 = @intCast(@min(raw_col, @min(cols - 1, std.math.maxInt(u16))));
    const row: u32 = @min(raw_row, rows - 1);

    const point = ghostty.GhosttyPoint{
        .tag = @intCast(ghostty.GHOSTTY_POINT_TAG_VIEWPORT),
        .value = .{
            .coordinate = .{ .x = col, .y = row },
        },
    };
    var ref = ghostty.GhosttyGridRef{
        .size = @sizeOf(ghostty.GhosttyGridRef),
        .node = null,
        .x = 0,
        .y = 0,
    };
    const result = ghostty.ghostty_terminal_grid_ref(t, point, &ref);
    if (result != ghostty.GHOSTTY_SUCCESS) return null;
    return ref;
}

pub fn emptyGridRef() ghostty.GhosttyGridRef {
    return .{
        .size = @sizeOf(ghostty.GhosttyGridRef),
        .node = null,
        .x = 0,
        .y = 0,
    };
}

pub fn emptySelection() ghostty.GhosttySelection {
    return .{
        .size = @sizeOf(ghostty.GhosttySelection),
        .start = emptyGridRef(),
        .end = emptyGridRef(),
        .rectangle = false,
    };
}

pub fn installSelection(s: ?*TerminalSession, sel: *const ghostty.GhosttySelection) i32 {
    const t = session.terminalHandle(s);
    if (t == null) return 0;

    const result = ghostty.ghostty_terminal_set(
        t,
        @intCast(ghostty.GHOSTTY_TERMINAL_OPT_SELECTION),
        sel,
    );
    if (result != ghostty.GHOSTTY_SUCCESS) return 0;
    session.markNeedsReplan(s);
    return 1;
}

pub fn activeCursorSelection(t: ghostty.GhosttyTerminal) ?ghostty.GhosttySelection {
    var x: u16 = 0;
    const x_result = ghostty.ghostty_terminal_get(
        t,
        @intCast(ghostty.GHOSTTY_TERMINAL_DATA_CURSOR_X),
        &x,
    );
    if (x_result != ghostty.GHOSTTY_SUCCESS) return null;

    var y: u16 = 0;
    const y_result = ghostty.ghostty_terminal_get(
        t,
        @intCast(ghostty.GHOSTTY_TERMINAL_DATA_CURSOR_Y),
        &y,
    );
    if (y_result != ghostty.GHOSTTY_SUCCESS) return null;

    const point = ghostty.GhosttyPoint{
        .tag = @intCast(ghostty.GHOSTTY_POINT_TAG_ACTIVE),
        .value = .{
            .coordinate = .{ .x = x, .y = y },
        },
    };
    var ref = emptyGridRef();
    const ref_result = ghostty.ghostty_terminal_grid_ref(t, point, &ref);
    if (ref_result != ghostty.GHOSTTY_SUCCESS) return null;

    return .{
        .size = @sizeOf(ghostty.GhosttySelection),
        .start = ref,
        .end = ref,
        .rectangle = false,
    };
}

pub fn currentSelectionOrCursor(t: ghostty.GhosttyTerminal) ?ghostty.GhosttySelection {
    var sel = emptySelection();
    const selection_result = ghostty.ghostty_terminal_get(
        t,
        @intCast(ghostty.GHOSTTY_TERMINAL_DATA_SELECTION),
        &sel,
    );
    if (selection_result == ghostty.GHOSTTY_SUCCESS) return sel;
    return activeCursorSelection(t);
}

pub fn clearInstalledSelection(s: ?*TerminalSession) void {
    const t = session.terminalHandle(s);
    if (t == null) return;
    const result = ghostty.ghostty_terminal_set(
        t,
        @intCast(ghostty.GHOSTTY_TERMINAL_OPT_SELECTION),
        null,
    );
    if (result == ghostty.GHOSTTY_SUCCESS) session.markNeedsReplan(s);
}

pub fn resetSelectionGesture(s: ?*TerminalSession) void {
    const sess = s orelse return;
    if (sess.selection_gesture == null) return;
    ghostty.ghostty_selection_gesture_reset(sess.selection_gesture, session.terminalHandle(s));
}

pub fn clearSelectionSession(s: ?*TerminalSession) void {
    clearInstalledSelection(s);
    resetSelectionGesture(s);
}

pub fn viewportCoordinateAtPixels(geometry: MouseGeometry, x_px: f32, y_px: f32) ?ghostty.GhosttyPointCoordinate {
    const cell_width = geometry.cell_width;
    const cell_height = geometry.cell_height;
    if (cell_width == 0 or cell_height == 0) return null;

    const cols = @max(@divFloor(geometry.screen_width, cell_width), 1);
    const rows = @max(@divFloor(geometry.screen_height, cell_height), 1);
    const local_x = @max(x_px - @as(f32, @floatFromInt(geometry.padding_left)), 0.0);
    const local_y = @max(y_px - @as(f32, @floatFromInt(geometry.padding_top)), 0.0);
    const raw_col: u32 = @intFromFloat(@floor(local_x / @as(f32, @floatFromInt(cell_width))));
    const raw_row: u32 = @intFromFloat(@floor(local_y / @as(f32, @floatFromInt(cell_height))));
    return .{
        .x = @intCast(@min(raw_col, @min(cols - 1, std.math.maxInt(u16)))),
        .y = @min(raw_row, rows - 1),
    };
}

pub fn selectionGestureGeometry(geometry: MouseGeometry) ?ghostty.GhosttySelectionGestureGeometry {
    if (geometry.cell_width == 0 or geometry.cell_height == 0) return null;
    return .{
        .columns = @max(@divFloor(geometry.screen_width, geometry.cell_width), 1),
        .cell_width = geometry.cell_width,
        .padding_left = geometry.padding_left,
        .screen_height = geometry.screen_height,
    };
}

fn setGestureOption(
    event: ghostty.GhosttySelectionGestureEvent,
    option: ghostty.GhosttySelectionGestureEventOption,
    value: *const anyopaque,
) void {
    _ = ghostty.ghostty_selection_gesture_event_set(event, option, value);
}

pub fn clearGestureOption(
    event: ghostty.GhosttySelectionGestureEvent,
    option: ghostty.GhosttySelectionGestureEventOption,
) void {
    _ = ghostty.ghostty_selection_gesture_event_set(event, option, null);
}

pub fn setGestureRef(event: ghostty.GhosttySelectionGestureEvent, ref: *const ghostty.GhosttyGridRef) void {
    setGestureOption(
        event,
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF),
        ref,
    );
}

pub fn setGesturePosition(event: ghostty.GhosttySelectionGestureEvent, x_px: f32, y_px: f32) void {
    const position = ghostty.GhosttySurfacePosition{ .x = x_px, .y = y_px };
    setGestureOption(
        event,
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_OPT_POSITION),
        &position,
    );
}

pub fn setGestureGeometry(
    event: ghostty.GhosttySelectionGestureEvent,
    geometry: *const ghostty.GhosttySelectionGestureGeometry,
) void {
    setGestureOption(
        event,
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_OPT_GEOMETRY),
        geometry,
    );
}

pub fn setGestureRectangle(event: ghostty.GhosttySelectionGestureEvent, rectangle: *const bool) void {
    setGestureOption(
        event,
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_OPT_RECTANGLE),
        rectangle,
    );
}

pub fn setGestureViewport(
    event: ghostty.GhosttySelectionGestureEvent,
    coord: *const ghostty.GhosttyPointCoordinate,
) void {
    setGestureOption(
        event,
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_OPT_VIEWPORT),
        coord,
    );
}

pub fn setGestureOptionValue(
    event: ghostty.GhosttySelectionGestureEvent,
    option: ghostty.GhosttySelectionGestureEventOption,
    value: *const anyopaque,
) void {
    setGestureOption(event, option, value);
}

pub fn sanitizeGestureBehavior(value: u32) ghostty.GhosttySelectionGestureBehavior {
    return switch (value) {
        ghostty.GHOSTTY_SELECTION_GESTURE_BEHAVIOR_WORD => @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_BEHAVIOR_WORD),
        ghostty.GHOSTTY_SELECTION_GESTURE_BEHAVIOR_LINE => @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_BEHAVIOR_LINE),
        ghostty.GHOSTTY_SELECTION_GESTURE_BEHAVIOR_OUTPUT => @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_BEHAVIOR_OUTPUT),
        else => @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_BEHAVIOR_CELL),
    };
}

pub fn dispatchSelectionGesture(
    sess: *TerminalSession,
    event: ghostty.GhosttySelectionGestureEvent,
) ?ghostty.GhosttySelection {
    const t = session.terminalHandle(sess);
    if (t == null or sess.selection_gesture == null or event == null) return null;

    var sel = emptySelection();
    const result = ghostty.ghostty_selection_gesture_event(
        sess.selection_gesture,
        t,
        event,
        &sel,
    );
    if (result != ghostty.GHOSTTY_SUCCESS) return null;
    return sel;
}

pub fn gestureAutoscroll(s: ?*TerminalSession) i32 {
    const sess = s orelse return 0;
    const t = session.terminalHandle(s);
    if (t == null or sess.selection_gesture == null) return 0;

    var autoscroll: ghostty.GhosttySelectionGestureAutoscroll =
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_AUTOSCROLL_NONE);
    const result = ghostty.ghostty_selection_gesture_get(
        sess.selection_gesture,
        t,
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_DATA_AUTOSCROLL),
        &autoscroll,
    );
    if (result != ghostty.GHOSTTY_SUCCESS) return 0;
    return @intCast(autoscroll);
}

pub fn writeHyperlinkAt(
    s: ?*TerminalSession,
    x_px: f32,
    y_px: f32,
    out_buf: ?[*]u8,
    cap: usize,
) usize {
    const out = out_buf orelse return 0;
    if (cap == 0) return 0;

    var ref = gridRefAtPixels(s, x_px, y_px) orelse return 0;
    var required: usize = 0;
    const probe = ghostty.ghostty_grid_ref_hyperlink_uri(&ref, null, 0, &required);
    if (probe == ghostty.GHOSTTY_SUCCESS and required == 0) return 0;
    if (probe != ghostty.GHOSTTY_OUT_OF_SPACE or required == 0) return 0;
    if (required + 1 > cap) return 0;

    var written: usize = 0;
    const result = ghostty.ghostty_grid_ref_hyperlink_uri(&ref, out, cap, &written);
    if (result != ghostty.GHOSTTY_SUCCESS or written == 0 or written >= cap) return 0;
    out[written] = 0;
    return written;
}

pub fn writeFormattedSelection(s: ?*TerminalSession, out_buf: ?[*]u8, cap: usize) usize {
    const t = session.terminalHandle(s);
    const out = out_buf orelse return 0;
    if (t == null or cap == 0) return 0;

    const options = ghostty.GhosttyTerminalSelectionFormatOptions{
        .size = @sizeOf(ghostty.GhosttyTerminalSelectionFormatOptions),
        .emit = @intCast(ghostty.GHOSTTY_FORMATTER_FORMAT_PLAIN),
        .unwrap = true,
        .trim = true,
        .selection = null,
    };
    var written: usize = 0;
    const result = ghostty.ghostty_terminal_selection_format_buf(
        t,
        options,
        out,
        cap,
        &written,
    );
    if (result != ghostty.GHOSTTY_SUCCESS or written == 0) return 0;
    if (written >= cap) return 0;
    out[written] = 0;
    return written;
}

fn emptyBytes() TerminalBytes {
    return .{ .ptr = null, .len = 0 };
}

fn formattedSelectionOptions() ghostty.GhosttyTerminalSelectionFormatOptions {
    return .{
        .size = @sizeOf(ghostty.GhosttyTerminalSelectionFormatOptions),
        .emit = @intCast(ghostty.GHOSTTY_FORMATTER_FORMAT_PLAIN),
        .unwrap = true,
        .trim = true,
        .selection = null,
    };
}

pub fn allocFormattedSelection(s: ?*TerminalSession) TerminalBytes {
    const t = session.terminalHandle(s);
    if (t == null) return emptyBytes();

    var ptr: ?[*]u8 = null;
    var len: usize = 0;
    const result = ghostty.ghostty_terminal_selection_format_alloc(
        t,
        null,
        formattedSelectionOptions(),
        &ptr,
        &len,
    );
    if (result != ghostty.GHOSTTY_SUCCESS or ptr == null or len == 0) {
        ghostty.ghostty_free(null, ptr, len);
        return emptyBytes();
    }

    const source = ptr.?[0..len];
    const owned = allocator.alloc(u8, len) catch {
        ghostty.ghostty_free(null, ptr, len);
        return emptyBytes();
    };
    @memcpy(owned, source);
    ghostty.ghostty_free(null, ptr, len);
    return .{ .ptr = owned.ptr, .len = owned.len };
}

pub fn newSelectionEvent(
    event_type: ghostty.GhosttySelectionGestureEventType,
) ?ghostty.GhosttySelectionGestureEvent {
    var event: ghostty.GhosttySelectionGestureEvent = null;
    const result = ghostty.ghostty_selection_gesture_event_new(null, &event, event_type);
    if (result != ghostty.GHOSTTY_SUCCESS or event == null) return null;
    return event;
}

pub fn freeSelectionResources(
    t: ghostty.GhosttyTerminal,
    gesture: ghostty.GhosttySelectionGesture,
    press: ghostty.GhosttySelectionGestureEvent,
    drag: ghostty.GhosttySelectionGestureEvent,
    release: ghostty.GhosttySelectionGestureEvent,
    autoscroll_tick: ghostty.GhosttySelectionGestureEvent,
) void {
    ghostty.ghostty_selection_gesture_event_free(autoscroll_tick);
    ghostty.ghostty_selection_gesture_event_free(release);
    ghostty.ghostty_selection_gesture_event_free(drag);
    ghostty.ghostty_selection_gesture_event_free(press);
    ghostty.ghostty_selection_gesture_free(gesture, t);
}
