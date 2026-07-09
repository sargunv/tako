const std = @import("std");

pub const ghostty = @cImport({
    @cDefine("GHOSTTY_STATIC", "1");
    @cInclude("ghostty/vt/build_info.h");
    @cInclude("ghostty/vt/color.h");
    @cInclude("ghostty/vt/focus.h");
    @cInclude("ghostty/vt/grid_ref.h");
    @cInclude("ghostty/vt/key/encoder.h");
    @cInclude("ghostty/vt/key/event.h");
    @cInclude("ghostty/vt/modes.h");
    @cInclude("ghostty/vt/mouse/encoder.h");
    @cInclude("ghostty/vt/mouse/event.h");
    @cInclude("ghostty/vt/paste.h");
    @cInclude("ghostty/vt/point.h");
    @cInclude("ghostty/vt/render.h");
    @cInclude("ghostty/vt/selection.h");
    @cInclude("ghostty/vt/terminal.h");
    @cInclude("ghostty/vt/unicode.h");
});

pub const ft = @cImport({
    @cInclude("ft2build.h");
    @cInclude("freetype/freetype.h");
});

pub const hb = @cImport({
    @cInclude("hb.h");
    @cInclude("hb-ot.h");
});

pub const core_abi = @cImport({
    @cInclude("tako_terminal_core.h");
});

pub const backend = @cImport({
    @cInclude("tako_terminal_backend.h");
});

pub const c = @cImport({
    @cDefine("_GNU_SOURCE", "1");
    @cInclude("errno.h");
    @cInclude("fcntl.h");
    @cInclude("signal.h");
    @cInclude("stdlib.h");
    @cInclude("sys/ioctl.h");
    @cInclude("sys/wait.h");
    @cInclude("unistd.h");
});

pub const FramePlan = core_abi.FramePlan;
pub const Vertex = core_abi.Vertex;
pub const TerminalOptions = core_abi.TakoTerminalOptions;
pub const ScrollbarState = core_abi.TakoTerminalScrollbarState;
pub const TerminalBytes = core_abi.TakoTerminalBytes;
pub const SurfaceOptions = backend.TakoTerminalSurfaceOptions;
pub const CellMetrics = backend.TakoTerminalCellMetrics;
pub const BackendRgb = backend.TakoTerminalRgb;
pub const BackendColors = backend.TakoTerminalColors;
pub const BackendCellStyle = backend.TakoTerminalCellStyle;
pub const BackendCell = backend.TakoTerminalCell;
pub const BackendRow = backend.TakoTerminalRow;
pub const BackendFrameSnapshot = backend.TakoTerminalFrameSnapshot;
pub const BackendShapedGlyph = backend.TakoTerminalShapedGlyph;
pub const BackendShapedText = backend.TakoTerminalShapedText;
pub const BackendRasterizedGlyph = backend.TakoTerminalRasterizedGlyph;

pub extern fn getenv(name: [*:0]const u8) ?[*:0]const u8;

pub const allocator = std.heap.page_allocator;

pub const CursorState = struct {
    valid: bool = false,
    visible: bool = false,
    viewport_present: bool = false,
    viewport_x: u16 = 0,
    viewport_y: u16 = 0,
    wide_tail: bool = false,
    style: u32 = 0,
    blinking: bool = false,
    password_input: bool = false,
};

pub const FrameState = struct {
    dirty: u32 = 0,
    content_dirty: bool = false,
    cursor: CursorState = .{},
};

pub const RowSelection = struct {
    start_x: u16,
    end_x: u16,
};

pub const MouseGeometry = struct {
    screen_width: u32,
    screen_height: u32,
    cell_width: u32,
    cell_height: u32,
    padding_top: u32 = 0,
    padding_bottom: u32 = 0,
    padding_left: u32 = 0,
    padding_right: u32 = 0,
};

pub const EffectiveColors = struct {
    fg: BackendRgb,
    bg: BackendRgb,
};

pub const focus_event_mode: ghostty.GhosttyMode = 1004;
pub const bracketed_paste_mode: ghostty.GhosttyMode = 2004;
pub const sync_output_mode: ghostty.GhosttyMode = 2026;
pub const repeat_interval_ns: u64 = 500_000_000;
pub const repeat_distance_px: f64 = 8.0;
pub const flat_uv: f32 = -1.0;
pub const title_data: ghostty.GhosttyTerminalData = @intCast(ghostty.GHOSTTY_TERMINAL_DATA_TITLE);
pub const pwd_data: ghostty.GhosttyTerminalData = @intCast(ghostty.GHOSTTY_TERMINAL_DATA_PWD);
pub const mouse_tracking_data: ghostty.GhosttyTerminalData =
    @intCast(ghostty.GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING);
pub const default_scrollback: usize = 10_000;
pub const version_string = "tako 0.1.0";
pub const enq_response = "\x1b[?1;2c";

pub fn writeOptionalBytes(bytes: []const u8, out_buf: ?[*]u8, cap: usize) usize {
    const out = out_buf orelse return 0;
    if (cap == 0 or bytes.len + 1 > cap) return 0;
    @memcpy(out[0..bytes.len], bytes);
    out[bytes.len] = 0;
    return bytes.len;
}

pub fn optionalCString(ptr: ?[*:0]const u8) ?[]const u8 {
    const p = ptr orelse return null;
    const value = std.mem.span(p);
    return if (std.mem.trim(u8, value, " \t\r\n").len == 0) null else value;
}

pub fn envVar(name: [*:0]const u8) ?[]const u8 {
    const value = getenv(name) orelse return null;
    return std.mem.span(value);
}

pub fn setNonblocking(fd: c_int) void {
    const flags = c.fcntl(fd, c.F_GETFL, @as(c_int, 0));
    if (flags >= 0) _ = c.fcntl(fd, c.F_SETFL, flags | c.O_NONBLOCK);
}

pub fn errnoIsAgain() bool {
    const value = std.c._errno().*;
    return value == c.EAGAIN or value == c.EWOULDBLOCK;
}

pub fn rgbToBackend(rgb: ghostty.GhosttyColorRgb) BackendRgb {
    return .{ .r = rgb.r, .g = rgb.g, .b = rgb.b };
}

pub fn rgbZero() BackendRgb {
    return .{ .r = 0, .g = 0, .b = 0 };
}

pub fn swapRgb(a: *BackendRgb, b: *BackendRgb) void {
    const tmp = a.*;
    a.* = b.*;
    b.* = tmp;
}

pub fn rgbEqual(a: BackendRgb, b: BackendRgb) bool {
    return a.r == b.r and a.g == b.g and a.b == b.b;
}

pub fn effectiveCellColors(cell: BackendCell, selected: bool, defaults: EffectiveColors) EffectiveColors {
    var fg = if (cell.fg_present) cell.fg else defaults.fg;
    var bg = if (cell.bg_present) cell.bg else defaults.bg;

    if (cell.style.inverse) {
        swapRgb(&fg, &bg);
    }
    if (selected) {
        swapRgb(&fg, &bg);
    }
    if (cell.style.faint) {
        fg = .{
            .r = fg.r / 2,
            .g = fg.g / 2,
            .b = fg.b / 2,
        };
    }

    return .{ .fg = fg, .bg = bg };
}

pub fn rowCellSelected(row: BackendRow, col: usize) bool {
    if (!row.selection_present) return false;
    return rowCellSelectedRaw(.{
        .start_x = row.selection_start_x,
        .end_x = row.selection_end_x,
    }, col);
}

pub fn rowCellSelectedRaw(selection: RowSelection, col: usize) bool {
    return col >= selection.start_x and col <= selection.end_x;
}
