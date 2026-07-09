const std = @import("std");
const core = @import("core.zig");
const bootstrap = @import("bootstrap.zig");
const common = @import("common.zig");
const selection = @import("selection.zig");
const session = @import("session.zig");

const ghostty = common.ghostty;

const FramePlan = common.FramePlan;
const ScrollbarState = common.ScrollbarState;
const TerminalSession = session.TerminalSession;

const TestSession = struct {
    session: *TerminalSession,

    fn init(cols: u16, rows: u16) !TestSession {
        const sess = bootstrap.createSession(.{
            .cols = cols,
            .rows = rows,
            .pixel_height = 14,
            .dpr = 1.0,
            .max_scrollback = 200,
        }) orelse return error.SessionUnavailable;
        return .{ .session = sess };
    }

    fn deinit(self: *TestSession) void {
        bootstrap.destroySession(self.session);
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
