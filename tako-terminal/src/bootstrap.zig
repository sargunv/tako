const std = @import("std");
const common = @import("common.zig");
const font = @import("font.zig");
const atlas = @import("atlas.zig");
const pty = @import("pty.zig");
const session = @import("session.zig");
const snapshot = @import("snapshot.zig");
const selection = @import("selection.zig");
const input = @import("input.zig");

const ghostty = common.ghostty;
const allocator = common.allocator;

const TerminalSession = session.TerminalSession;

pub const SessionConfig = struct {
    cols: u16,
    rows: u16,
    font_path: ?[*:0]const u8 = null,
    font_family: ?[*:0]const u8 = null,
    pixel_height: u32,
    dpr: f32,
    max_scrollback: usize = common.default_scrollback,
    pty: ?*pty.PtySession = null,
};

const SessionBuild = struct {
    surface: ?*font.FontCore = null,
    terminal: ghostty.GhosttyTerminal = null,
    render_state: ghostty.GhosttyRenderState = null,
    pty: ?*pty.PtySession = null,
    key_encoder: ghostty.GhosttyKeyEncoder = null,
    key_event: ghostty.GhosttyKeyEvent = null,
    mouse_encoder: ghostty.GhosttyMouseEncoder = null,
    mouse_event: ghostty.GhosttyMouseEvent = null,
    selection_gesture: ghostty.GhosttySelectionGesture = null,
    selection_press: ghostty.GhosttySelectionGestureEvent = null,
    selection_drag: ghostty.GhosttySelectionGestureEvent = null,
    selection_release: ghostty.GhosttySelectionGestureEvent = null,
    selection_autoscroll_tick: ghostty.GhosttySelectionGestureEvent = null,

    fn deinit(self: *SessionBuild) void {
        selection.freeSelectionResources(
            self.terminal,
            self.selection_gesture,
            self.selection_press,
            self.selection_drag,
            self.selection_release,
            self.selection_autoscroll_tick,
        );
        if (self.mouse_event != null) ghostty.ghostty_mouse_event_free(self.mouse_event);
        if (self.mouse_encoder != null) ghostty.ghostty_mouse_encoder_free(self.mouse_encoder);
        if (self.key_event != null) ghostty.ghostty_key_event_free(self.key_event);
        if (self.key_encoder != null) ghostty.ghostty_key_encoder_free(self.key_encoder);
        if (self.pty) |p| {
            p.destroy();
            self.pty = null;
        }
        snapshot.freeTerminalCore(self.terminal, self.render_state);
        if (self.surface) |surface| font.fontCoreDestroy(surface);
    }

    fn initSelectionEvents(self: *SessionBuild) bool {
        self.selection_gesture = null;
        const result = ghostty.ghostty_selection_gesture_new(null, &self.selection_gesture);
        if (result != ghostty.GHOSTTY_SUCCESS or self.selection_gesture == null) return false;

        self.selection_press = selection.newSelectionEvent(
            @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_PRESS),
        ) orelse return false;
        self.selection_drag = selection.newSelectionEvent(
            @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_DRAG),
        ) orelse return false;
        self.selection_release = selection.newSelectionEvent(
            @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_RELEASE),
        ) orelse return false;
        self.selection_autoscroll_tick = selection.newSelectionEvent(
            @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_AUTOSCROLL_TICK),
        ) orelse return false;
        return true;
    }
};

pub fn createSession(config: SessionConfig) ?*TerminalSession {
    const resolved_font = font.resolveFontPath(config.font_path, config.font_family) orelse return null;
    defer allocator.free(resolved_font);

    var build: SessionBuild = .{ .pty = config.pty };
    errdefer build.deinit();

    const backend_options = common.SurfaceOptions{
        .font_path = resolved_font.ptr,
        .pixel_height = config.pixel_height,
        .dpr = config.dpr,
    };
    build.surface = font.fontCoreCreateWithOptions(&backend_options) orelse return null;

    const terminal_init_options = ghostty.GhosttyTerminalOptions{
        .cols = config.cols,
        .rows = config.rows,
        .max_scrollback = if (config.max_scrollback == 0)
            common.default_scrollback
        else
            config.max_scrollback,
    };
    var result = ghostty.ghostty_terminal_new(null, &build.terminal, terminal_init_options);
    if (result != ghostty.GHOSTTY_SUCCESS or build.terminal == null) return null;

    result = ghostty.ghostty_render_state_new(null, &build.render_state);
    if (result != ghostty.GHOSTTY_SUCCESS or build.render_state == null) return null;

    result = ghostty.ghostty_key_encoder_new(null, &build.key_encoder);
    if (result != ghostty.GHOSTTY_SUCCESS or build.key_encoder == null) return null;

    result = ghostty.ghostty_key_event_new(null, &build.key_event);
    if (result != ghostty.GHOSTTY_SUCCESS or build.key_event == null) return null;

    result = ghostty.ghostty_mouse_encoder_new(null, &build.mouse_encoder);
    if (result != ghostty.GHOSTTY_SUCCESS or build.mouse_encoder == null) return null;

    result = ghostty.ghostty_mouse_event_new(null, &build.mouse_event);
    if (result != ghostty.GHOSTTY_SUCCESS or build.mouse_event == null) return null;

    if (!build.initSelectionEvents()) return null;

    const sess = allocator.create(TerminalSession) catch return null;
    errdefer allocator.destroy(sess);

    sess.* = .{
        .terminal = build.terminal,
        .render_state = build.render_state,
        .surface = build.surface,
        .cols = config.cols,
        .rows = config.rows,
        .pty = build.pty,
        .pty_response = std.ArrayList(u8).empty,
        .key_encoder = build.key_encoder,
        .key_event = build.key_event,
        .mouse_encoder = build.mouse_encoder,
        .mouse_event = build.mouse_event,
        .selection_gesture = build.selection_gesture,
        .selection_press = build.selection_press,
        .selection_drag = build.selection_drag,
        .selection_release = build.selection_release,
        .selection_autoscroll_tick = build.selection_autoscroll_tick,
        .glyph_atlas = atlas.OwnedGlyphAtlas.init(),
    };

    build.surface = null;
    build.terminal = null;
    build.render_state = null;
    build.pty = null;
    build.key_encoder = null;
    build.key_event = null;
    build.mouse_encoder = null;
    build.mouse_event = null;
    build.selection_gesture = null;
    build.selection_press = null;
    build.selection_drag = null;
    build.selection_release = null;
    build.selection_autoscroll_tick = null;

    session.registerTerminalEffects(sess);
    input.syncMouseGeometry(sess);
    return sess;
}

pub fn destroySession(s: ?*TerminalSession) void {
    const sess = s orelse return;
    session.freeOptionalBytes(&sess.title);
    session.freeOptionalBytes(&sess.pwd);
    session.freeOptionalBytes(&sess.preedit);
    if (sess.key_event != null) {
        ghostty.ghostty_key_event_free(sess.key_event);
        sess.key_event = null;
    }
    if (sess.key_encoder != null) {
        ghostty.ghostty_key_encoder_free(sess.key_encoder);
        sess.key_encoder = null;
    }
    if (sess.mouse_event != null) {
        ghostty.ghostty_mouse_event_free(sess.mouse_event);
        sess.mouse_event = null;
    }
    if (sess.mouse_encoder != null) {
        ghostty.ghostty_mouse_encoder_free(sess.mouse_encoder);
        sess.mouse_encoder = null;
    }
    selection.freeSelectionResources(
        session.terminalHandle(s),
        sess.selection_gesture,
        sess.selection_press,
        sess.selection_drag,
        sess.selection_release,
        sess.selection_autoscroll_tick,
    );
    sess.selection_gesture = null;
    sess.selection_press = null;
    sess.selection_drag = null;
    sess.selection_release = null;
    sess.selection_autoscroll_tick = null;
    if (sess.pty) |p| {
        p.destroy();
        sess.pty = null;
    }
    sess.pty_response.deinit(allocator);
    sess.frame_vertices.deinit(allocator);
    sess.glyph_atlas.deinit();
    if (sess.render_state != null) {
        ghostty.ghostty_render_state_free(sess.render_state);
        sess.render_state = null;
    }
    if (sess.terminal != null) {
        ghostty.ghostty_terminal_free(sess.terminal);
        sess.terminal = null;
    }
    if (sess.surface) |surface| {
        font.fontCoreDestroy(surface);
        sess.surface = null;
    }
    allocator.destroy(sess);
}
