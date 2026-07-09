const std = @import("std");
const common = @import("common.zig");
const bootstrap = @import("bootstrap.zig");
const font = @import("font.zig");
const pty = @import("pty.zig");
const session = @import("session.zig");
const snapshot = @import("snapshot.zig");
const frame = @import("frame.zig");
const input = @import("input.zig");
const selection = @import("selection.zig");

const ghostty = common.ghostty;
const allocator = common.allocator;

const TerminalOptions = common.TerminalOptions;
const ScrollbarState = common.ScrollbarState;
const TerminalBytes = common.TerminalBytes;
const FramePlan = common.FramePlan;
const CellMetrics = common.CellMetrics;
const TerminalSession = session.TerminalSession;

const sync_output_mode = common.sync_output_mode;
const focus_event_mode = common.focus_event_mode;
const bracketed_paste_mode = common.bracketed_paste_mode;
const repeat_interval_ns = common.repeat_interval_ns;
const repeat_distance_px = common.repeat_distance_px;
const mouse_tracking_data = common.mouse_tracking_data;
const title_data = common.title_data;
const pwd_data = common.pwd_data;
const default_scrollback = common.default_scrollback;

pub export fn tako_terminal_core_engine_version(out_buf: ?[*]u8, cap: usize) usize {
    var version: ghostty.GhosttyString = undefined;
    const result = ghostty.ghostty_build_info(
        ghostty.GHOSTTY_BUILD_INFO_VERSION_STRING,
        &version,
    );
    if (result != ghostty.GHOSTTY_SUCCESS or version.ptr == null) return 0;
    return common.writeOptionalBytes(version.ptr[0..version.len], out_buf, cap);
}

pub export fn tako_terminal_bytes_free(bytes: TerminalBytes) void {
    const ptr = bytes.ptr orelse return;
    if (bytes.len == 0) return;
    allocator.free(@constCast(ptr)[0..bytes.len]);
}

pub export fn tako_terminal_session_new(options: ?*const TerminalOptions) ?*TerminalSession {
    const terminal_options = options orelse return null;
    const p = pty.PtySession.spawn(terminal_options) orelse return null;
    const sess = bootstrap.createSession(.{
        .cols = terminal_options.cols,
        .rows = terminal_options.rows,
        .font_path = terminal_options.font_path,
        .font_family = terminal_options.font_family,
        .pixel_height = terminal_options.pixel_height,
        .dpr = terminal_options.dpr,
        .max_scrollback = if (terminal_options.max_scrollback == 0)
            default_scrollback
        else
            terminal_options.max_scrollback,
        .pty = p,
    });
    if (sess == null) {
        p.destroy();
    }
    return sess;
}

pub export fn tako_terminal_session_destroy(s: ?*TerminalSession) void {
    bootstrap.destroySession(s);
}

pub export fn tako_terminal_session_tick(s: ?*TerminalSession, out: ?*FramePlan) bool {
    if (s) |sess| {
        if (sess.pty) |p| {
            _ = p.drainIntoTerminal(sess.terminal);
            session.flushPtyResponses(sess);
        }
    }
    if (session.terminalMode(s, sync_output_mode)) {
        frame.writeLastPlan(s, out);
        return false;
    }
    const force_replan = if (s) |sess| sess.needs_replan else false;

    const frame_state = snapshot.captureFrameState(s) orelse {
        frame.writeLastPlan(s, out);
        return false;
    };

    const focused = if (s) |sess| sess.focused else false;
    const cursor_blink_visible = if (s) |sess| sess.cursor_blink_visible else true;
    const next_cursor = frame.presentedCursorState(frame_state.cursor, focused, cursor_blink_visible);

    var should_build = frame_state.content_dirty or force_replan;
    if (s) |sess| {
        should_build = should_build or !frame.cursorStatesEqual(sess.last_cursor, next_cursor);
    }
    if (!should_build) {
        if (s) |sess| {
            sess.needs_replan = false;
            sess.last_cursor = next_cursor;
        }
        frame.writeLastPlan(s, out);
        return false;
    }

    var snapshot_buffers = snapshot.SnapshotBuffers{};
    defer snapshot_buffers.deinit();
    const snap = snapshot.captureFrameSnapshot(s, frame_state, &snapshot_buffers) orelse {
        if (s) |sess| {
            sess.needs_replan = true;
        }
        frame.writeLastPlan(s, out);
        return false;
    };

    var next_plan: FramePlan = std.mem.zeroes(FramePlan);
    if (s) |sess| {
        sess.last_cursor = next_cursor;
        const built = frame.finalizeFramePlan(sess, &next_plan, &snap, next_cursor);
        if (built) {
            sess.needs_replan = false;
            sess.last_plan = next_plan;
            snapshot.clearRenderStateDirty(sess.render_state);
            if (out) |target| target.* = next_plan;
        } else {
            sess.needs_replan = true;
            frame.writeLastPlan(sess, out);
        }
        return built;
    }
    return false;
}

pub export fn tako_terminal_session_notify_fd(s: ?*TerminalSession) i32 {
    const sess = s orelse return -1;
    const p = sess.pty orelse return -1;
    return p.notifyFd();
}

pub export fn tako_terminal_session_exited(s: ?*TerminalSession) i32 {
    const sess = s orelse return 0;
    const p = sess.pty orelse return 1;
    return if (p.isExited()) 1 else 0;
}

pub export fn tako_terminal_session_resize_pixels(
    s: ?*TerminalSession,
    width_px: u32,
    height_px: u32,
) void {
    if (s) |sess| {
        var cell: CellMetrics = undefined;
        if (session.sessionCellMetrics(sess, &cell)) {
            const grid = session.gridForPixels(cell, width_px, height_px);
            const changed = grid.cols != sess.cols or grid.rows != sess.rows;
            sess.cols = grid.cols;
            sess.rows = grid.rows;
            if (changed) {
                if (sess.pty) |p| p.resize(sess.cols, sess.rows);
                _ = ghostty.ghostty_terminal_resize(
                    sess.terminal,
                    sess.cols,
                    sess.rows,
                    cell.cell_width,
                    cell.cell_height,
                );
            }
        }
    }
    input.syncMouseGeometry(s);
}

pub export fn tako_terminal_session_set_dpr(s: ?*TerminalSession, dpr: f32) void {
    font.fontCoreSetDpr(session.sessionSurface(s), dpr);
    if (s) |sess| sess.glyph_atlas.reset();
    session.markNeedsReplan(s);
    input.syncMouseGeometry(s);
}

pub export fn tako_terminal_session_set_focused(s: ?*TerminalSession, focused: bool) void {
    session.setSessionFocused(s, focused);
}

pub export fn tako_terminal_session_set_cursor_blink_visible(
    s: ?*TerminalSession,
    visible: bool,
) void {
    session.setSessionCursorBlinkVisible(s, visible);
}

pub export fn tako_terminal_session_set_preedit(
    s: ?*TerminalSession,
    data: ?[*]const u8,
    len: usize,
    cursor_byte: usize,
) void {
    session.setSessionPreedit(s, data, len, cursor_byte);
}

pub export fn tako_terminal_session_set_default_color(
    s: ?*TerminalSession,
    role: u32,
    enabled: bool,
    r: u8,
    g: u8,
    b: u8,
) i32 {
    const t = session.terminalHandle(s);
    const opt = session.terminalOptionForColorRole(role) orelse return 0;
    if (t == null) return 0;

    var color = ghostty.GhosttyColorRgb{ .r = r, .g = g, .b = b };
    const value: ?*const anyopaque = if (enabled) @ptrCast(&color) else null;
    const result = ghostty.ghostty_terminal_set(t, opt, value);
    if (result != ghostty.GHOSTTY_SUCCESS) return 0;
    session.markNeedsReplan(s);
    return 1;
}

pub export fn tako_terminal_session_set_default_palette(
    s: ?*TerminalSession,
    enabled: bool,
    rgb_triplets: ?[*]const u8,
    len: usize,
) i32 {
    const t = session.terminalHandle(s);
    if (t == null) return 0;

    if (!enabled) {
        const result = ghostty.ghostty_terminal_set(
            t,
            @intCast(ghostty.GHOSTTY_TERMINAL_OPT_COLOR_PALETTE),
            null,
        );
        if (result != ghostty.GHOSTTY_SUCCESS) return 0;
        session.markNeedsReplan(s);
        return 1;
    }

    if (len != 256 * 3) return 0;
    const bytes = rgb_triplets orelse return 0;
    var palette: [256]ghostty.GhosttyColorRgb = undefined;
    var i: usize = 0;
    while (i < palette.len) : (i += 1) {
        const base = i * 3;
        palette[i] = ghostty.GhosttyColorRgb{
            .r = bytes[base],
            .g = bytes[base + 1],
            .b = bytes[base + 2],
        };
    }

    const result = ghostty.ghostty_terminal_set(
        t,
        @intCast(ghostty.GHOSTTY_TERMINAL_OPT_COLOR_PALETTE),
        &palette,
    );
    if (result != ghostty.GHOSTTY_SUCCESS) return 0;
    session.markNeedsReplan(s);
    return 1;
}

pub export fn tako_terminal_session_set_default_cursor(
    s: ?*TerminalSession,
    style: u32,
    blink: bool,
) i32 {
    const t = session.terminalHandle(s);
    if (t == null) return 0;

    var mapped_style = session.cursorStyle(style);
    var mapped_blink = blink;
    const style_result = ghostty.ghostty_terminal_set(
        t,
        @intCast(ghostty.GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_STYLE),
        &mapped_style,
    );
    const blink_result = ghostty.ghostty_terminal_set(
        t,
        @intCast(ghostty.GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_BLINK),
        &mapped_blink,
    );
    if (style_result != ghostty.GHOSTTY_SUCCESS or blink_result != ghostty.GHOSTTY_SUCCESS) {
        return 0;
    }
    session.markNeedsReplan(s);
    return 1;
}

pub export fn tako_terminal_session_set_font(
    s: ?*TerminalSession,
    font_path: ?[*:0]const u8,
    font_family: ?[*:0]const u8,
    pixel_height: u32,
) i32 {
    const resolved_font = font.resolveFontPath(font_path, font_family) orelse return 0;
    defer allocator.free(resolved_font);

    const result = font.fontCoreSetFont(session.sessionSurface(s), resolved_font.ptr, pixel_height);
    if (result != 0) {
        if (s) |sess| sess.glyph_atlas.reset();
        session.markNeedsReplan(s);
        input.syncMouseGeometry(s);
    }
    return result;
}

pub export fn tako_terminal_session_write(
    s: ?*TerminalSession,
    data: ?[*]const u8,
    len: usize,
) void {
    if (len == 0) return;
    const sess = s orelse return;
    const p = sess.pty orelse return;
    const bytes = data orelse return;
    p.write(bytes[0..len]);
}

pub export fn tako_terminal_session_take_bell_count(s: ?*TerminalSession) u32 {
    const sess = s orelse return 0;
    const count = sess.pending_bell_count;
    sess.pending_bell_count = 0;
    return count;
}

pub export fn tako_terminal_session_hyperlink_at(
    s: ?*TerminalSession,
    x_px: f32,
    y_px: f32,
    out_buf: ?[*]u8,
    cap: usize,
) usize {
    return selection.writeHyperlinkAt(s, x_px, y_px, out_buf, cap);
}

pub export fn tako_terminal_session_paste(
    s: ?*TerminalSession,
    data: ?[*]const u8,
    len: usize,
) void {
    const bracketed = session.terminalMode(s, bracketed_paste_mode);

    if (len == 0) {
        if (bracketed) {
            const empty_bracketed = "\x1b[200~\x1b[201~";
            session.writeSessionBytes(s, empty_bracketed);
        }
        return;
    }

    const in = data orelse return;
    const scratch = allocator.alloc(u8, len) catch return;
    defer allocator.free(scratch);
    @memcpy(scratch, in[0..len]);

    var required: usize = 0;
    const probe = ghostty.ghostty_paste_encode(
        @ptrCast(scratch.ptr),
        scratch.len,
        bracketed,
        null,
        0,
        &required,
    );
    if (probe != ghostty.GHOSTTY_OUT_OF_SPACE or required == 0) return;

    const out = allocator.alloc(u8, required) catch return;
    defer allocator.free(out);
    var written: usize = 0;
    const result = ghostty.ghostty_paste_encode(
        @ptrCast(scratch.ptr),
        scratch.len,
        bracketed,
        @ptrCast(out.ptr),
        out.len,
        &written,
    );
    if (result != ghostty.GHOSTTY_SUCCESS or written == 0) return;
    session.writeSessionBytes(s, out[0..written]);
}

pub export fn tako_terminal_session_scroll(s: ?*TerminalSession, delta_rows: i64) void {
    selection.scrollViewport(s, ghostty.GhosttyTerminalScrollViewport{
        .tag = @intCast(ghostty.GHOSTTY_SCROLL_VIEWPORT_DELTA),
        .value = .{ .delta = @intCast(delta_rows) },
    });
}

pub export fn tako_terminal_session_scroll_to_top(s: ?*TerminalSession) void {
    selection.scrollViewport(s, ghostty.GhosttyTerminalScrollViewport{
        .tag = @intCast(ghostty.GHOSTTY_SCROLL_VIEWPORT_TOP),
        .value = .{ .delta = 0 },
    });
}

pub export fn tako_terminal_session_scroll_to_bottom(s: ?*TerminalSession) void {
    selection.scrollViewport(s, ghostty.GhosttyTerminalScrollViewport{
        .tag = @intCast(ghostty.GHOSTTY_SCROLL_VIEWPORT_BOTTOM),
        .value = .{ .delta = 0 },
    });
}

pub export fn tako_terminal_session_scroll_to_row(s: ?*TerminalSession, row: u64) void {
    selection.scrollViewport(s, ghostty.GhosttyTerminalScrollViewport{
        .tag = @intCast(ghostty.GHOSTTY_SCROLL_VIEWPORT_ROW),
        .value = .{ .row = @intCast(row) },
    });
}

pub export fn tako_terminal_session_scrollbar_state(
    s: ?*TerminalSession,
    out: ?*ScrollbarState,
) bool {
    const state = out orelse return false;
    const t = session.terminalHandle(s);
    if (t == null) return false;

    var scrollbar: ghostty.GhosttyTerminalScrollbar = undefined;
    const result = ghostty.ghostty_terminal_get(
        t,
        @intCast(ghostty.GHOSTTY_TERMINAL_DATA_SCROLLBAR),
        &scrollbar,
    );
    if (result != ghostty.GHOSTTY_SUCCESS) return false;

    var viewport_active = false;
    const active_result = ghostty.ghostty_terminal_get(
        t,
        @intCast(ghostty.GHOSTTY_TERMINAL_DATA_VIEWPORT_ACTIVE),
        &viewport_active,
    );

    state.* = .{
        .total = scrollbar.total,
        .offset = scrollbar.offset,
        .len = scrollbar.len,
        .viewport_active = @intFromBool(active_result == ghostty.GHOSTTY_SUCCESS and viewport_active),
    };
    return true;
}

pub export fn tako_terminal_session_mouse_tracking(s: ?*TerminalSession) i32 {
    return @intFromBool(session.terminalDataBool(s, mouse_tracking_data));
}

pub export fn tako_terminal_session_mouse_set_any_button(
    s: ?*TerminalSession,
    pressed: bool,
) void {
    const sess = s orelse return;
    if (sess.mouse_encoder == null) return;
    ghostty.ghostty_mouse_encoder_setopt(
        sess.mouse_encoder,
        @intCast(ghostty.GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED),
        &pressed,
    );
}

pub export fn tako_terminal_session_key_event(
    s: ?*TerminalSession,
    action: u32,
    key: u32,
    mods: u16,
    consumed_mods: u16,
    text: ?[*]const u8,
    text_len: usize,
) void {
    const sess = s orelse return;
    if (sess.key_event == null) return;

    const key_value: ghostty.GhosttyKey = @intCast(key);
    ghostty.ghostty_key_event_set_action(sess.key_event, @intCast(action));
    ghostty.ghostty_key_event_set_key(sess.key_event, key_value);
    ghostty.ghostty_key_event_set_mods(sess.key_event, mods);
    ghostty.ghostty_key_event_set_consumed_mods(sess.key_event, consumed_mods);
    ghostty.ghostty_key_event_set_unshifted_codepoint(sess.key_event, input.unshiftedCodepoint(key_value));

    if (text) |ptr| {
        const bytes = ptr[0..text_len];
        if (text_len > 0 and !input.textContainsControl(bytes)) {
            ghostty.ghostty_key_event_set_utf8(sess.key_event, @ptrCast(ptr), text_len);
        } else {
            ghostty.ghostty_key_event_set_utf8(sess.key_event, null, 0);
        }
    } else {
        ghostty.ghostty_key_event_set_utf8(sess.key_event, null, 0);
    }

    input.writeEncodedKey(sess);
}

pub export fn tako_terminal_session_mouse_event(
    s: ?*TerminalSession,
    action: u32,
    button: u32,
    x_px: f32,
    y_px: f32,
    mods: u16,
) void {
    const sess = s orelse return;
    if (sess.mouse_event == null) return;

    // Tracked mouse mode owns pointer events; abort any view-side selection
    // gesture before reporting the event to the PTY.
    selection.clearSelectionSession(s);

    ghostty.ghostty_mouse_event_set_action(sess.mouse_event, @intCast(action));
    if (button == 0) {
        ghostty.ghostty_mouse_event_clear_button(sess.mouse_event);
    } else {
        ghostty.ghostty_mouse_event_set_button(sess.mouse_event, @intCast(button));
    }
    ghostty.ghostty_mouse_event_set_mods(sess.mouse_event, mods);
    ghostty.ghostty_mouse_event_set_position(
        sess.mouse_event,
        ghostty.GhosttyMousePosition{ .x = x_px, .y = y_px },
    );
    input.writeEncodedMouse(sess);
}

pub export fn tako_terminal_session_selection_begin(
    s: ?*TerminalSession,
    x_px: f32,
    y_px: f32,
    time_ns: u64,
    mods: u16,
    single_click: u32,
    double_click: u32,
    triple_click: u32,
) void {
    const sess = s orelse return;
    if (sess.selection_press == null) return;
    const ref = selection.gridRefAtPixels(s, x_px, y_px) orelse return;
    const rectangle = (mods & @as(u16, @intCast(ghostty.GHOSTTY_MODS_ALT))) != 0;
    const behaviors = ghostty.GhosttySelectionGestureBehaviors{
        .single_click = selection.sanitizeGestureBehavior(single_click),
        .double_click = selection.sanitizeGestureBehavior(double_click),
        .triple_click = selection.sanitizeGestureBehavior(triple_click),
    };

    selection.setGestureRef(sess.selection_press, &ref);
    selection.setGesturePosition(sess.selection_press, x_px, y_px);
    selection.setGestureOption(
        sess.selection_press,
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_OPT_TIME_NS),
        &time_ns,
    );
    selection.setGestureOption(
        sess.selection_press,
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REPEAT_INTERVAL_NS),
        &repeat_interval_ns,
    );
    selection.setGestureOption(
        sess.selection_press,
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REPEAT_DISTANCE),
        &repeat_distance_px,
    );
    selection.setGestureOption(
        sess.selection_press,
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_OPT_BEHAVIORS),
        &behaviors,
    );
    selection.setGestureRectangle(sess.selection_press, &rectangle);

    if (selection.dispatchSelectionGesture(sess, sess.selection_press)) |sel| {
        _ = selection.installSelection(s, &sel);
    } else {
        selection.clearInstalledSelection(s);
    }
}

pub export fn tako_terminal_session_selection_extend(
    s: ?*TerminalSession,
    x_px: f32,
    y_px: f32,
    mods: u16,
) i32 {
    const sess = s orelse return 0;
    if (sess.selection_drag == null) return 0;
    var geometry: common.MouseGeometry = undefined;
    if (!session.sessionMouseGeometry(s, &geometry)) return 0;
    const gesture_geometry = selection.selectionGestureGeometry(geometry) orelse return 0;
    const ref = selection.gridRefAtPixels(s, x_px, y_px) orelse return 0;
    const rectangle = (mods & @as(u16, @intCast(ghostty.GHOSTTY_MODS_ALT))) != 0;

    selection.setGestureRef(sess.selection_drag, &ref);
    selection.setGesturePosition(sess.selection_drag, x_px, y_px);
    selection.setGestureGeometry(sess.selection_drag, &gesture_geometry);
    selection.setGestureRectangle(sess.selection_drag, &rectangle);

    const sel = selection.dispatchSelectionGesture(sess, sess.selection_drag) orelse return 0;
    return selection.installSelection(s, &sel);
}

pub export fn tako_terminal_session_selection_autoscroll(s: ?*TerminalSession) i32 {
    return selection.gestureAutoscroll(s);
}

pub export fn tako_terminal_session_selection_autoscroll_tick(
    s: ?*TerminalSession,
    x_px: f32,
    y_px: f32,
    mods: u16,
) i32 {
    const sess = s orelse return 0;
    if (sess.selection_autoscroll_tick == null) return 0;
    var geometry: common.MouseGeometry = undefined;
    if (!session.sessionMouseGeometry(s, &geometry)) return 0;
    const gesture_geometry = selection.selectionGestureGeometry(geometry) orelse return 0;
    const coord = selection.viewportCoordinateAtPixels(geometry, x_px, y_px) orelse return 0;
    const rectangle = (mods & @as(u16, @intCast(ghostty.GHOSTTY_MODS_ALT))) != 0;

    selection.setGestureViewport(sess.selection_autoscroll_tick, &coord);
    selection.setGesturePosition(sess.selection_autoscroll_tick, x_px, y_px);
    selection.setGestureGeometry(sess.selection_autoscroll_tick, &gesture_geometry);
    selection.setGestureRectangle(sess.selection_autoscroll_tick, &rectangle);

    const sel =
        selection.dispatchSelectionGesture(sess, sess.selection_autoscroll_tick) orelse return 0;
    return selection.installSelection(s, &sel);
}

pub export fn tako_terminal_session_selection_end_owned(
    s: ?*TerminalSession,
    x_px: f32,
    y_px: f32,
) TerminalBytes {
    selection.finishReleaseGesture(s, x_px, y_px);
    return selection.allocFormattedSelection(s);
}

pub export fn tako_terminal_session_selection_text(
    s: ?*TerminalSession,
    out_buf: ?[*]u8,
    cap: usize,
) usize {
    return selection.writeFormattedSelection(s, out_buf, cap);
}

pub export fn tako_terminal_session_selection_text_owned(s: ?*TerminalSession) TerminalBytes {
    return selection.allocFormattedSelection(s);
}

pub export fn tako_terminal_session_selection_clear(s: ?*TerminalSession) void {
    selection.clearSelectionSession(s);
}

pub export fn tako_terminal_session_selection_all(s: ?*TerminalSession) i32 {
    const t = session.terminalHandle(s);
    if (t == null) return 0;

    var sel = selection.emptySelection();
    const result = ghostty.ghostty_terminal_select_all(t, &sel);
    if (result != ghostty.GHOSTTY_SUCCESS) return 0;
    return selection.installSelection(s, &sel);
}

pub export fn tako_terminal_session_selection_output_at(
    s: ?*TerminalSession,
    x_px: f32,
    y_px: f32,
) i32 {
    const t = session.terminalHandle(s);
    if (t == null) return 0;
    const ref = selection.gridRefAtPixels(s, x_px, y_px) orelse return 0;

    var sel = selection.emptySelection();
    const result = ghostty.ghostty_terminal_select_output(t, ref, &sel);
    if (result != ghostty.GHOSTTY_SUCCESS) return 0;
    return selection.installSelection(s, &sel);
}

pub export fn tako_terminal_session_selection_input_at(
    s: ?*TerminalSession,
    x_px: f32,
    y_px: f32,
) i32 {
    const t = session.terminalHandle(s);
    if (t == null) return 0;
    const ref = selection.gridRefAtPixels(s, x_px, y_px) orelse return 0;

    const options = ghostty.GhosttyTerminalSelectLineOptions{
        .size = @sizeOf(ghostty.GhosttyTerminalSelectLineOptions),
        .ref = ref,
        .whitespace = null,
        .whitespace_len = 0,
        .semantic_prompt_boundary = true,
    };
    var sel = selection.emptySelection();
    const result = ghostty.ghostty_terminal_select_line(t, &options, &sel);
    if (result != ghostty.GHOSTTY_SUCCESS) return 0;
    return selection.installSelection(s, &sel);
}

pub export fn tako_terminal_session_selection_adjust(
    s: ?*TerminalSession,
    adjustment: u32,
) i32 {
    const t = session.terminalHandle(s);
    if (t == null) return 0;

    var sel = selection.currentSelectionOrCursor(t) orelse return 0;
    _ = ghostty.ghostty_terminal_selection_adjust(
        t,
        &sel,
        @intCast(adjustment),
    );
    return selection.installSelection(s, &sel);
}

pub export fn tako_terminal_session_focus_event(s: ?*TerminalSession, gained: bool) void {
    session.setSessionFocused(s, gained);
    if (!session.terminalMode(s, focus_event_mode)) return;

    const event: ghostty.GhosttyFocusEvent =
        @intCast(if (gained) ghostty.GHOSTTY_FOCUS_GAINED else ghostty.GHOSTTY_FOCUS_LOST);
    var buf: [8]u8 = undefined;
    var written: usize = 0;
    const result = ghostty.ghostty_focus_encode(
        event,
        @ptrCast(&buf),
        buf.len,
        &written,
    );
    if (result != ghostty.GHOSTTY_SUCCESS or written == 0) return;
    session.writeSessionBytes(s, buf[0..written]);
}

pub export fn tako_terminal_session_take_title(
    s: ?*TerminalSession,
    out_buf: ?[*]u8,
    cap: usize,
) usize {
    const sess = s orelse return 0;
    return session.takeChangedTerminalString(sess, title_data, &sess.title, out_buf, cap);
}

pub export fn tako_terminal_session_take_pwd(
    s: ?*TerminalSession,
    out_buf: ?[*]u8,
    cap: usize,
) usize {
    const sess = s orelse return 0;
    return session.takeChangedTerminalString(sess, pwd_data, &sess.pwd, out_buf, cap);
}
