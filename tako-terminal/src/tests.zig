const std = @import("std");
const core = @import("core.zig");
const common = @import("common.zig");
const font = @import("font.zig");
const atlas = @import("atlas.zig");
const selection = @import("selection.zig");
const session = @import("session.zig");
const input = @import("input.zig");

const ghostty = common.ghostty;

const FramePlan = common.FramePlan;
const ScrollbarState = common.ScrollbarState;
const TerminalSession = session.TerminalSession;

const TestSession = struct {
    session: *TerminalSession,

    fn init(cols: u16, rows: u16) !TestSession {
        const resolved_font = font.resolveFontPath(null, null) orelse return error.FontUnavailable;
        defer common.allocator.free(resolved_font);

        const surface_options = common.SurfaceOptions{
            .font_path = resolved_font.ptr,
            .pixel_height = 14,
            .dpr = 1.0,
        };
        const surface = font.fontCoreCreateWithOptions(&surface_options) orelse return error.FontCoreUnavailable;
        errdefer font.fontCoreDestroy(surface);

        var terminal: ghostty.GhosttyTerminal = null;
        var result = ghostty.ghostty_terminal_new(null, &terminal, .{
            .cols = cols,
            .rows = rows,
            .max_scrollback = 200,
        });
        if (result != ghostty.GHOSTTY_SUCCESS or terminal == null) return error.TerminalUnavailable;
        errdefer ghostty.ghostty_terminal_free(terminal);

        var render_state: ghostty.GhosttyRenderState = null;
        result = ghostty.ghostty_render_state_new(null, &render_state);
        if (result != ghostty.GHOSTTY_SUCCESS or render_state == null) return error.RenderStateUnavailable;
        errdefer ghostty.ghostty_render_state_free(render_state);

        var key_encoder: ghostty.GhosttyKeyEncoder = null;
        result = ghostty.ghostty_key_encoder_new(null, &key_encoder);
        if (result != ghostty.GHOSTTY_SUCCESS or key_encoder == null) return error.KeyEncoderUnavailable;
        errdefer ghostty.ghostty_key_encoder_free(key_encoder);

        var key_event: ghostty.GhosttyKeyEvent = null;
        result = ghostty.ghostty_key_event_new(null, &key_event);
        if (result != ghostty.GHOSTTY_SUCCESS or key_event == null) return error.KeyEventUnavailable;
        errdefer ghostty.ghostty_key_event_free(key_event);

        var mouse_encoder: ghostty.GhosttyMouseEncoder = null;
        result = ghostty.ghostty_mouse_encoder_new(null, &mouse_encoder);
        if (result != ghostty.GHOSTTY_SUCCESS or mouse_encoder == null) return error.MouseEncoderUnavailable;
        errdefer ghostty.ghostty_mouse_encoder_free(mouse_encoder);

        var mouse_event: ghostty.GhosttyMouseEvent = null;
        result = ghostty.ghostty_mouse_event_new(null, &mouse_event);
        if (result != ghostty.GHOSTTY_SUCCESS or mouse_event == null) return error.MouseEventUnavailable;
        errdefer ghostty.ghostty_mouse_event_free(mouse_event);

        var selection_gesture: ghostty.GhosttySelectionGesture = null;
        result = ghostty.ghostty_selection_gesture_new(null, &selection_gesture);
        if (result != ghostty.GHOSTTY_SUCCESS or selection_gesture == null) return error.SelectionGestureUnavailable;
        errdefer ghostty.ghostty_selection_gesture_free(selection_gesture, terminal);

        const selection_press = selection.newSelectionEvent(@intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_PRESS)) orelse return error.SelectionEventUnavailable;
        errdefer ghostty.ghostty_selection_gesture_event_free(selection_press);
        const selection_drag = selection.newSelectionEvent(@intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_DRAG)) orelse return error.SelectionEventUnavailable;
        errdefer ghostty.ghostty_selection_gesture_event_free(selection_drag);
        const selection_release = selection.newSelectionEvent(@intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_RELEASE)) orelse return error.SelectionEventUnavailable;
        errdefer ghostty.ghostty_selection_gesture_event_free(selection_release);
        const selection_autoscroll_tick = selection.newSelectionEvent(@intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_AUTOSCROLL_TICK)) orelse return error.SelectionEventUnavailable;
        errdefer ghostty.ghostty_selection_gesture_event_free(selection_autoscroll_tick);

        const sess = try common.allocator.create(TerminalSession);
        sess.* = .{
            .terminal = terminal,
            .render_state = render_state,
            .surface = surface,
            .cols = cols,
            .rows = rows,
            .pty = null,
            .pty_response = std.ArrayList(u8).empty,
            .key_encoder = key_encoder,
            .key_event = key_event,
            .mouse_encoder = mouse_encoder,
            .mouse_event = mouse_event,
            .selection_gesture = selection_gesture,
            .selection_press = selection_press,
            .selection_drag = selection_drag,
            .selection_release = selection_release,
            .selection_autoscroll_tick = selection_autoscroll_tick,
            .glyph_atlas = atlas.OwnedGlyphAtlas.init(),
        };
        session.registerTerminalEffects(sess);
        input.syncMouseGeometry(sess);
        return .{ .session = sess };
    }

    fn deinit(self: *TestSession) void {
        core.tako_terminal_session_destroy(self.session);
    }

    fn write(self: *TestSession, bytes: []const u8) void {
        ghostty.ghostty_terminal_vt_write(self.session.terminal, bytes.ptr, bytes.len);
    }

    fn tick(self: *TestSession) FramePlan {
        var plan: FramePlan = std.mem.zeroes(FramePlan);
        _ = core.tako_terminal_session_tick(self.session, &plan);
        return plan;
    }
};

fn expectSelectionText(s: ?*TerminalSession, expected: []const u8) !void {
    var buf: [256]u8 = undefined;
    const len = core.tako_terminal_session_selection_text(s, &buf, buf.len);
    try std.testing.expectEqualStrings(expected, buf[0..len]);
}

test "Zig core builds frames for text, default colors, cursor, and atlas" {
    var ts = try TestSession.init(40, 6);
    defer ts.deinit();

    try std.testing.expectEqual(@as(i32, 1), core.tako_terminal_session_set_default_color(ts.session, 1, true, 1, 2, 3));
    ts.write("hello");
    const plan = ts.tick();

    try std.testing.expectEqual(@as(u32, 40), plan.cols);
    try std.testing.expectEqual(@as(u32, 6), plan.rows);
    try std.testing.expect(plan.cell_w > 0);
    try std.testing.expect(plan.cell_h > 0);
    try std.testing.expect(plan.vertex_count > 0);
    try std.testing.expect(plan.atlas_h > 0);
    try std.testing.expect(plan.atlas_pixels != null);
    try std.testing.expectEqual(@as(u8, 1), plan.cursor_present);
    try std.testing.expectEqual(@as(u32, 5), plan.cursor_x);
}

test "Zig core exposes title, cwd, bell, and hyperlinks" {
    var ts = try TestSession.init(40, 6);
    defer ts.deinit();

    ts.write("\x1b]0;Tako Test\x07\x1b]7;file://localhost/tmp\x07\x07");
    _ = ts.tick();

    var buf: [256]u8 = undefined;
    var len = core.tako_terminal_session_take_title(ts.session, &buf, buf.len);
    try std.testing.expectEqualStrings("Tako Test", buf[0..len]);
    len = core.tako_terminal_session_take_pwd(ts.session, &buf, buf.len);
    try std.testing.expect(std.mem.endsWith(u8, buf[0..len], "/tmp"));
    try std.testing.expectEqual(@as(u32, 1), core.tako_terminal_session_take_bell_count(ts.session));

    ts.write("\r\n\x1b]8;;https://example.test\x07link\x1b]8;;\x07");
    const plan = ts.tick();
    len = core.tako_terminal_session_hyperlink_at(ts.session, plan.cell_w * 0.5, plan.cell_h * 1.5, &buf, buf.len);
    try std.testing.expectEqualStrings("https://example.test", buf[0..len]);
}

test "Zig core selection formatting and clearing work" {
    var ts = try TestSession.init(40, 6);
    defer ts.deinit();

    ts.write("alpha beta");
    _ = ts.tick();

    try std.testing.expectEqual(@as(i32, 1), core.tako_terminal_session_selection_all(ts.session));
    try expectSelectionText(ts.session, "alpha beta");
    _ = ts.tick();

    core.tako_terminal_session_selection_clear(ts.session);
    try expectSelectionText(ts.session, "");
}

test "Zig core scrollback viewport reports state and replans" {
    var ts = try TestSession.init(20, 3);
    defer ts.deinit();

    ts.write("one\r\ntwo\r\nthree\r\nfour\r\nfive");
    _ = ts.tick();

    var state: ScrollbarState = std.mem.zeroes(ScrollbarState);
    try std.testing.expect(core.tako_terminal_session_scrollbar_state(ts.session, &state));
    try std.testing.expect(state.total >= state.len);

    core.tako_terminal_session_scroll_to_top(ts.session);
    const top_plan = ts.tick();
    try std.testing.expectEqual(@as(u32, 20), top_plan.cols);

    core.tako_terminal_session_scroll_to_bottom(ts.session);
    const bottom_plan = ts.tick();
    try std.testing.expectEqual(@as(u32, 3), bottom_plan.rows);
}

test "Zig core gates frames during synchronized output" {
    var ts = try TestSession.init(40, 6);
    defer ts.deinit();

    _ = ts.tick();
    ts.write("\x1b[?2026hhidden while syncing");
    var plan: FramePlan = std.mem.zeroes(FramePlan);
    try std.testing.expect(!core.tako_terminal_session_tick(ts.session, &plan));

    ts.write("\x1b[?2026lshown");
    try std.testing.expect(core.tako_terminal_session_tick(ts.session, &plan));
    try std.testing.expect(plan.vertex_count > 0);
}
