const std = @import("std");
const common = @import("common.zig");
const session = @import("session.zig");
const selection = @import("selection.zig");

const ghostty = common.ghostty;
const allocator = common.allocator;

const TerminalSession = session.TerminalSession;
const EncodeResult = ghostty.GhosttyResult;
const EncodeFn = *const fn (*anyopaque, ?*u8, usize, *usize) EncodeResult;

fn encodeToSession(
    ctx: *anyopaque,
    encode: EncodeFn,
    stack_buf: []u8,
    deliver: *const fn (*TerminalSession, []const u8) void,
    sess: *TerminalSession,
) void {
    var written: usize = 0;
    const result = encode(ctx, @ptrCast(stack_buf.ptr), stack_buf.len, &written);
    if (result == ghostty.GHOSTTY_SUCCESS) {
        if (written > 0) deliver(sess, stack_buf[0..written]);
        return;
    }
    if (result != ghostty.GHOSTTY_OUT_OF_SPACE or written == 0) return;

    const out = allocator.alloc(u8, written) catch return;
    defer allocator.free(out);
    var written2: usize = 0;
    const result2 = encode(ctx, @ptrCast(out.ptr), out.len, &written2);
    if (result2 == ghostty.GHOSTTY_SUCCESS and written2 > 0) {
        deliver(sess, out[0..written2]);
    }
}

fn clearSelectionThenWrite(sess: *TerminalSession, bytes: []const u8) void {
    selection.clearSelectionSession(sess);
    session.writeSessionBytes(sess, bytes);
}

fn keyConst(comptime key: anytype) ghostty.GhosttyKey {
    return @as(ghostty.GhosttyKey, @intCast(key));
}

pub fn unshiftedCodepoint(key: ghostty.GhosttyKey) u32 {
    return switch (key) {
        keyConst(ghostty.GHOSTTY_KEY_A) => 'a',
        keyConst(ghostty.GHOSTTY_KEY_B) => 'b',
        keyConst(ghostty.GHOSTTY_KEY_C) => 'c',
        keyConst(ghostty.GHOSTTY_KEY_D) => 'd',
        keyConst(ghostty.GHOSTTY_KEY_E) => 'e',
        keyConst(ghostty.GHOSTTY_KEY_F) => 'f',
        keyConst(ghostty.GHOSTTY_KEY_G) => 'g',
        keyConst(ghostty.GHOSTTY_KEY_H) => 'h',
        keyConst(ghostty.GHOSTTY_KEY_I) => 'i',
        keyConst(ghostty.GHOSTTY_KEY_J) => 'j',
        keyConst(ghostty.GHOSTTY_KEY_K) => 'k',
        keyConst(ghostty.GHOSTTY_KEY_L) => 'l',
        keyConst(ghostty.GHOSTTY_KEY_M) => 'm',
        keyConst(ghostty.GHOSTTY_KEY_N) => 'n',
        keyConst(ghostty.GHOSTTY_KEY_O) => 'o',
        keyConst(ghostty.GHOSTTY_KEY_P) => 'p',
        keyConst(ghostty.GHOSTTY_KEY_Q) => 'q',
        keyConst(ghostty.GHOSTTY_KEY_R) => 'r',
        keyConst(ghostty.GHOSTTY_KEY_S) => 's',
        keyConst(ghostty.GHOSTTY_KEY_T) => 't',
        keyConst(ghostty.GHOSTTY_KEY_U) => 'u',
        keyConst(ghostty.GHOSTTY_KEY_V) => 'v',
        keyConst(ghostty.GHOSTTY_KEY_W) => 'w',
        keyConst(ghostty.GHOSTTY_KEY_X) => 'x',
        keyConst(ghostty.GHOSTTY_KEY_Y) => 'y',
        keyConst(ghostty.GHOSTTY_KEY_Z) => 'z',
        keyConst(ghostty.GHOSTTY_KEY_DIGIT_0) => '0',
        keyConst(ghostty.GHOSTTY_KEY_DIGIT_1) => '1',
        keyConst(ghostty.GHOSTTY_KEY_DIGIT_2) => '2',
        keyConst(ghostty.GHOSTTY_KEY_DIGIT_3) => '3',
        keyConst(ghostty.GHOSTTY_KEY_DIGIT_4) => '4',
        keyConst(ghostty.GHOSTTY_KEY_DIGIT_5) => '5',
        keyConst(ghostty.GHOSTTY_KEY_DIGIT_6) => '6',
        keyConst(ghostty.GHOSTTY_KEY_DIGIT_7) => '7',
        keyConst(ghostty.GHOSTTY_KEY_DIGIT_8) => '8',
        keyConst(ghostty.GHOSTTY_KEY_DIGIT_9) => '9',
        keyConst(ghostty.GHOSTTY_KEY_SEMICOLON) => ';',
        keyConst(ghostty.GHOSTTY_KEY_SPACE) => ' ',
        keyConst(ghostty.GHOSTTY_KEY_QUOTE) => '\'',
        keyConst(ghostty.GHOSTTY_KEY_COMMA) => ',',
        keyConst(ghostty.GHOSTTY_KEY_BACKQUOTE) => '`',
        keyConst(ghostty.GHOSTTY_KEY_PERIOD) => '.',
        keyConst(ghostty.GHOSTTY_KEY_SLASH) => '/',
        keyConst(ghostty.GHOSTTY_KEY_MINUS) => '-',
        keyConst(ghostty.GHOSTTY_KEY_EQUAL) => '=',
        keyConst(ghostty.GHOSTTY_KEY_BRACKET_LEFT) => '[',
        keyConst(ghostty.GHOSTTY_KEY_BRACKET_RIGHT) => ']',
        keyConst(ghostty.GHOSTTY_KEY_BACKSLASH) => '\\',
        keyConst(ghostty.GHOSTTY_KEY_TAB) => '\t',
        keyConst(ghostty.GHOSTTY_KEY_NUMPAD_0) => '0',
        keyConst(ghostty.GHOSTTY_KEY_NUMPAD_1) => '1',
        keyConst(ghostty.GHOSTTY_KEY_NUMPAD_2) => '2',
        keyConst(ghostty.GHOSTTY_KEY_NUMPAD_3) => '3',
        keyConst(ghostty.GHOSTTY_KEY_NUMPAD_4) => '4',
        keyConst(ghostty.GHOSTTY_KEY_NUMPAD_5) => '5',
        keyConst(ghostty.GHOSTTY_KEY_NUMPAD_6) => '6',
        keyConst(ghostty.GHOSTTY_KEY_NUMPAD_7) => '7',
        keyConst(ghostty.GHOSTTY_KEY_NUMPAD_8) => '8',
        keyConst(ghostty.GHOSTTY_KEY_NUMPAD_9) => '9',
        keyConst(ghostty.GHOSTTY_KEY_NUMPAD_DECIMAL) => '.',
        keyConst(ghostty.GHOSTTY_KEY_NUMPAD_DIVIDE) => '/',
        keyConst(ghostty.GHOSTTY_KEY_NUMPAD_MULTIPLY) => '*',
        keyConst(ghostty.GHOSTTY_KEY_NUMPAD_SUBTRACT) => '-',
        keyConst(ghostty.GHOSTTY_KEY_NUMPAD_ADD) => '+',
        keyConst(ghostty.GHOSTTY_KEY_NUMPAD_EQUAL) => '=',
        else => 0,
    };
}

pub fn textContainsControl(bytes: []const u8) bool {
    for (bytes) |byte| {
        if (byte < 0x20 or byte == 0x7f) return true;
    }
    return false;
}

fn encodeKey(ctx: *anyopaque, out: ?*u8, cap: usize, written: *usize) EncodeResult {
    const sess: *TerminalSession = @ptrCast(@alignCast(ctx));
    return ghostty.ghostty_key_encoder_encode(
        sess.key_encoder,
        sess.key_event,
        out,
        cap,
        written,
    );
}

fn encodeMouse(ctx: *anyopaque, out: ?*u8, cap: usize, written: *usize) EncodeResult {
    const sess: *TerminalSession = @ptrCast(@alignCast(ctx));
    return ghostty.ghostty_mouse_encoder_encode(
        sess.mouse_encoder,
        sess.mouse_event,
        out,
        cap,
        written,
    );
}

pub fn writeEncodedKey(sess: *TerminalSession) void {
    const t = session.terminalHandle(sess);
    if (t == null or sess.key_encoder == null or sess.key_event == null) return;

    ghostty.ghostty_key_encoder_setopt_from_terminal(sess.key_encoder, t);

    var buf: [128]u8 = undefined;
    encodeToSession(sess, encodeKey, &buf, clearSelectionThenWrite, sess);
}

pub fn syncMouseGeometry(s: ?*TerminalSession) void {
    const sess = s orelse return;
    if (sess.mouse_encoder == null) return;

    var geometry: common.MouseGeometry = undefined;
    if (!session.sessionMouseGeometry(s, &geometry)) return;
    var size = ghostty.GhosttyMouseEncoderSize{
        .size = @sizeOf(ghostty.GhosttyMouseEncoderSize),
        .screen_width = geometry.screen_width,
        .screen_height = geometry.screen_height,
        .cell_width = geometry.cell_width,
        .cell_height = geometry.cell_height,
        .padding_top = geometry.padding_top,
        .padding_bottom = geometry.padding_bottom,
        .padding_right = geometry.padding_right,
        .padding_left = geometry.padding_left,
    };
    ghostty.ghostty_mouse_encoder_setopt(
        sess.mouse_encoder,
        @intCast(ghostty.GHOSTTY_MOUSE_ENCODER_OPT_SIZE),
        &size,
    );
}

pub fn writeEncodedMouse(sess: *TerminalSession) void {
    const t = session.terminalHandle(sess);
    if (t == null or sess.mouse_encoder == null or sess.mouse_event == null) return;

    ghostty.ghostty_mouse_encoder_setopt_from_terminal(sess.mouse_encoder, t);

    var buf: [64]u8 = undefined;
    encodeToSession(sess, encodeMouse, &buf, session.writeSessionBytes, sess);
}
