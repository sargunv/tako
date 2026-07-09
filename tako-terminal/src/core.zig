const std = @import("std");

const ghostty = @cImport({
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

const ft = @cImport({
    @cInclude("ft2build.h");
    @cInclude("freetype/freetype.h");
});

const hb = @cImport({
    @cInclude("hb.h");
    @cInclude("hb-ot.h");
});

const core_abi = @cImport({
    @cInclude("tako_terminal_core.h");
});

const backend = @cImport({
    @cInclude("tako_terminal_backend.h");
});

const c = @cImport({
    @cDefine("_GNU_SOURCE", "1");
    @cInclude("errno.h");
    @cInclude("fcntl.h");
    @cInclude("signal.h");
    @cInclude("stdlib.h");
    @cInclude("sys/ioctl.h");
    @cInclude("sys/wait.h");
    @cInclude("unistd.h");
});

const FramePlan = core_abi.FramePlan;
const Vertex = core_abi.Vertex;
const TerminalOptions = core_abi.TakoTerminalOptions;
const ScrollbarState = core_abi.TakoTerminalScrollbarState;
const TerminalBytes = core_abi.TakoTerminalBytes;
const SurfaceOptions = backend.TakoTerminalSurfaceOptions;
const CellMetrics = backend.TakoTerminalCellMetrics;
const BackendRgb = backend.TakoTerminalRgb;
const BackendColors = backend.TakoTerminalColors;
const BackendCellStyle = backend.TakoTerminalCellStyle;
const BackendCell = backend.TakoTerminalCell;
const BackendRow = backend.TakoTerminalRow;
const BackendFrameSnapshot = backend.TakoTerminalFrameSnapshot;
const BackendShapedGlyph = backend.TakoTerminalShapedGlyph;
const BackendShapedText = backend.TakoTerminalShapedText;
const BackendRasterizedGlyph = backend.TakoTerminalRasterizedGlyph;

const CursorState = struct {
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

const FrameState = struct {
    dirty: u32 = 0,
    content_dirty: bool = false,
    cursor: CursorState = .{},
};

const GraphemeBytes = struct {
    bytes: []u8,
    len: usize,
};

const RowSelection = struct {
    start_x: u16,
    end_x: u16,
};

const atlas_width: u32 = 512;

const AtlasGlyph = struct {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    left_bearing: i32,
    top_bearing: i32,
};

const OwnedGlyphAtlas = struct {
    glyphs: std.AutoHashMap(u32, AtlasGlyph),
    pixels: std.ArrayList(u8) = .empty,
    width: u32 = atlas_width,
    height: u32 = 0,
    next_x: u32 = 0,
    next_y: u32 = 0,
    row_h: u32 = 0,
    generation: u64 = 0,

    fn init() OwnedGlyphAtlas {
        return .{ .glyphs = std.AutoHashMap(u32, AtlasGlyph).init(allocator) };
    }

    fn deinit(self: *OwnedGlyphAtlas) void {
        self.glyphs.deinit();
        self.pixels.deinit(allocator);
    }

    fn reset(self: *OwnedGlyphAtlas) void {
        self.glyphs.clearRetainingCapacity();
        self.pixels.clearRetainingCapacity();
        self.height = 0;
        self.next_x = 0;
        self.next_y = 0;
        self.row_h = 0;
        self.generation = self.generation +% 1;
    }

    fn ensureGlyph(
        self: *OwnedGlyphAtlas,
        surface: ?*FontCore,
        glyph_id: u32,
    ) !AtlasGlyph {
        if (self.glyphs.get(glyph_id)) |glyph| return glyph;

        var raster: BackendRasterizedGlyph = std.mem.zeroes(BackendRasterizedGlyph);
        if (!fontCoreRasterizeGlyph(surface, glyph_id, &raster)) {
            return error.RasterizeFailed;
        }

        var glyph = AtlasGlyph{
            .x = 0,
            .y = 0,
            .w = raster.width,
            .h = raster.height,
            .left_bearing = raster.left_bearing,
            .top_bearing = raster.top_bearing,
        };

        if (raster.width == 0 or raster.height == 0 or raster.pixels == null) {
            try self.glyphs.put(glyph_id, glyph);
            return glyph;
        }

        if (raster.width > self.width) {
            try self.glyphs.put(glyph_id, glyph);
            return glyph;
        }

        if (self.next_x + raster.width > self.width) {
            self.next_y += self.row_h;
            self.next_x = 0;
            self.row_h = 0;
        }

        glyph.x = self.next_x;
        glyph.y = self.next_y;
        self.next_x += raster.width;
        self.row_h = @max(self.row_h, raster.height);
        const needed_h = @max(self.height, glyph.y + raster.height);
        try self.ensureHeight(needed_h);

        const expected_len = @as(usize, raster.width) * @as(usize, raster.height);
        const src_len = @min(expected_len, raster.pixel_len);
        const src = raster.pixels[0..src_len];
        var row: u32 = 0;
        while (row < raster.height) : (row += 1) {
            const src_start = @as(usize, row) * @as(usize, raster.width);
            if (src_start >= src.len) break;
            const src_end = @min(src_start + raster.width, src.len);
            const dst_start =
                (@as(usize, glyph.y + row) * @as(usize, self.width)) + @as(usize, glyph.x);
            @memcpy(self.pixels.items[dst_start .. dst_start + (src_end - src_start)], src[src_start..src_end]);
        }

        try self.glyphs.put(glyph_id, glyph);
        self.generation = self.generation +% 1;
        return glyph;
    }

    fn ensureHeight(self: *OwnedGlyphAtlas, height: u32) !void {
        if (height <= self.height) return;
        const old_len = self.pixels.items.len;
        const new_len = @as(usize, self.width) * @as(usize, height);
        try self.pixels.resize(allocator, new_len);
        @memset(self.pixels.items[old_len..], 0);
        self.height = height;
    }
};

const SnapshotBuffers = struct {
    rows: std.ArrayList(BackendRow) = .empty,
    cell_chunks: std.ArrayList([]BackendCell) = .empty,
    grapheme_chunks: std.ArrayList([]u8) = .empty,

    fn deinit(self: *SnapshotBuffers) void {
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

const MouseGeometry = struct {
    screen_width: u32,
    screen_height: u32,
    cell_width: u32,
    cell_height: u32,
    padding_top: u32 = 0,
    padding_bottom: u32 = 0,
    padding_left: u32 = 0,
    padding_right: u32 = 0,
};

const Autorun = struct {
    cmd: ?[]u8 = null,
    start_ms: i64 = 0,
    delay_ms: i64 = 2000,
    fired: bool = false,
};

const EnvVar = struct {
    name: [:0]u8,
    value: [:0]u8,
};

const SpawnCommand = struct {
    program: [:0]u8,
    cwd: ?[:0]u8,
    argv: []?[*:0]u8,
    env: []EnvVar,
    runtime_root: ?[]u8,

    fn deinit(self: *SpawnCommand) void {
        for (self.argv[0 .. self.argv.len - 1]) |arg| {
            if (arg) |ptr| allocator.free(std.mem.span(ptr));
        }
        allocator.free(self.argv);
        for (self.env) |entry| {
            allocator.free(entry.name);
            allocator.free(entry.value);
        }
        allocator.free(self.env);
        if (self.cwd) |cwd| allocator.free(cwd);
    }
};

const PtySession = struct {
    master_fd: c_int,
    pid: std.posix.pid_t,
    exited: bool = false,
    runtime_root: ?[]u8 = null,

    fn spawn(options: *const TerminalOptions) ?*PtySession {
        var command = buildSpawnCommand(options) catch return null;
        defer command.deinit();

        const master = c.posix_openpt(c.O_RDWR | c.O_NOCTTY | c.O_CLOEXEC);
        if (master < 0) return null;
        errdefer _ = c.close(master);
        if (c.grantpt(master) != 0) return null;
        if (c.unlockpt(master) != 0) return null;

        var slave_name: [std.fs.max_path_bytes]u8 = undefined;
        if (c.ptsname_r(master, &slave_name, slave_name.len) != 0) return null;
        const slave = c.open(&slave_name, c.O_RDWR | c.O_NOCTTY | c.O_CLOEXEC);
        if (slave < 0) return null;
        errdefer _ = c.close(slave);

        const pid = std.posix.fork() catch return null;
        if (pid == 0) {
            childExec(master, slave, &command);
        }

        _ = c.close(slave);
        setNonblocking(master);

        const session = allocator.create(PtySession) catch {
            _ = c.kill(pid, c.SIGKILL);
            _ = c.close(master);
            return null;
        };
        session.* = .{
            .master_fd = master,
            .pid = pid,
            .runtime_root = command.runtime_root,
        };
        command.runtime_root = null;
        return session;
    }

    fn destroy(self: *PtySession) void {
        if (!self.exited) {
            _ = c.kill(self.pid, c.SIGTERM);
        }
        self.reap();
        _ = c.close(self.master_fd);
        if (self.runtime_root) |root| {
            std.fs.deleteTreeAbsolute(root) catch {};
            allocator.free(root);
            self.runtime_root = null;
        }
        allocator.destroy(self);
    }

    fn drainIntoTerminal(self: *PtySession, terminal: ghostty.GhosttyTerminal) usize {
        if (terminal == null) return 0;
        var byte_count: usize = 0;
        var buf: [8192]u8 = undefined;
        while (!self.exited) {
            const n = std.posix.read(self.master_fd, &buf) catch |err| switch (err) {
                error.WouldBlock => break,
                error.InputOutput => {
                    self.exited = true;
                    break;
                },
                else => {
                    self.exited = true;
                    break;
                },
            };
            if (n == 0) {
                self.exited = true;
                break;
            }
            ghostty.ghostty_terminal_vt_write(terminal, &buf, n);
            byte_count += n;
        }
        self.reap();
        return byte_count;
    }

    fn write(self: *PtySession, bytes: []const u8) void {
        var offset: usize = 0;
        while (offset < bytes.len and !self.exited) {
            const n = std.posix.write(self.master_fd, bytes[offset..]) catch |err| switch (err) {
                error.WouldBlock => break,
                error.BrokenPipe => {
                    self.exited = true;
                    break;
                },
                else => {
                    self.exited = true;
                    break;
                },
            };
            if (n == 0) break;
            offset += n;
        }
    }

    fn resize(self: *PtySession, cols: u16, rows: u16) void {
        var size = std.posix.winsize{
            .row = rows,
            .col = cols,
            .xpixel = 0,
            .ypixel = 0,
        };
        _ = c.ioctl(self.master_fd, c.TIOCSWINSZ, &size);
    }

    fn notifyFd(self: *PtySession) i32 {
        return if (self.exited) -1 else self.master_fd;
    }

    fn isExited(self: *PtySession) bool {
        self.reap();
        return self.exited;
    }

    fn reap(self: *PtySession) void {
        if (self.exited) return;
        const result = std.posix.waitpid(self.pid, std.posix.W.NOHANG);
        if (result.pid == self.pid) self.exited = true;
    }
};

fn fontCoreCreateWithOptions(options: ?*const SurfaceOptions) ?*FontCore {
    const opts = options orelse return null;
    const path = opts.font_path orelse return null;
    return FontCore.create(path, opts.pixel_height, opts.dpr);
}

fn fontCoreDestroy(surface: ?*FontCore) void {
    const s = surface orelse return;
    s.destroy();
}

fn fontCoreShapeText(
    surface: ?*FontCore,
    text: ?[*]const u8,
    text_len: usize,
    out: ?*BackendShapedText,
) bool {
    const s = surface orelse return false;
    const target = out orelse return false;
    if (text_len > 0 and text == null) return false;
    const bytes = if (text_len == 0) "" else text.?[0..text_len];
    if (!s.shapeText(bytes)) return false;
    target.* = .{
        .glyphs = if (s.shaped_buf.items.len == 0) null else s.shaped_buf.items.ptr,
        .glyph_count = s.shaped_buf.items.len,
    };
    return true;
}

fn fontCoreRasterizeGlyph(
    surface: ?*FontCore,
    glyph_id: u32,
    out: ?*BackendRasterizedGlyph,
) bool {
    const s = surface orelse return false;
    const target = out orelse return false;
    target.* = s.rasterizeGlyph(glyph_id);
    return true;
}

fn fontCoreSetDpr(surface: ?*FontCore, dpr: f32) void {
    const s = surface orelse return;
    s.setDpr(dpr);
}

fn fontCoreSetFont(
    surface: ?*FontCore,
    font_path: ?[*:0]const u8,
    pixel_height: u32,
) i32 {
    const s = surface orelse return 0;
    const path = font_path orelse return 0;
    return if (s.setFont(path, pixel_height)) 1 else 0;
}

fn fontCoreCellMetrics(
    surface: ?*FontCore,
    out: ?*CellMetrics,
) bool {
    const s = surface orelse return false;
    const target = out orelse return false;
    target.* = s.cell;
    return true;
}

extern fn getenv(name: [*:0]const u8) ?[*:0]const u8;

const allocator = std.heap.page_allocator;

const bash_integration =
    \\if [ -n "${TAKO_SHELL_INTEGRATION_ACTIVE:-}" ]; then
    \\  return
    \\fi
    \\export TAKO_SHELL_INTEGRATION_ACTIVE=1
    \\__tako_osc() { printf '\033]%s\007' "$1"; }
    \\__tako_command_started=0
    \\__tako_preexec() {
    \\  case "${BASH_COMMAND:-}" in __tako_prompt_command*) return ;; esac
    \\  if [ "$__tako_command_started" = 0 ]; then
    \\    __tako_command_started=1
    \\    __tako_osc '133;C'
    \\  fi
    \\}
    \\__tako_prompt_command() {
    \\  local status=$?
    \\  trap - DEBUG
    \\  if [ "$__tako_command_started" = 1 ]; then __tako_osc "133;D;$status"; fi
    \\  __tako_command_started=0
    \\  if [ -n "${__tako_original_prompt_command:-}" ]; then eval "$__tako_original_prompt_command"; fi
    \\  trap '__tako_preexec' DEBUG
    \\  return "$status"
    \\}
    \\__tako_prompt_start='\[\033]133;A\007\]'
    \\__tako_prompt_end='\[\033]133;B\007\]'
    \\case "${PS1:-}" in *'133;A'*'133;B'*) ;; *) PS1="${__tako_prompt_start}${PS1:-\$ }${__tako_prompt_end}" ;; esac
    \\__tako_original_prompt_command="${PROMPT_COMMAND:-}"
    \\PROMPT_COMMAND=__tako_prompt_command
    \\trap '__tako_preexec' DEBUG
    \\
;

const zsh_integration =
    \\if [[ -n ${TAKO_SHELL_INTEGRATION_ACTIVE:-} ]]; then
    \\  return
    \\fi
    \\export TAKO_SHELL_INTEGRATION_ACTIVE=1
    \\__tako_osc() { printf '\033]%s\007' "$1"; }
    \\__tako_command_started=0
    \\__tako_preexec() { __tako_command_started=1; __tako_osc '133;C'; }
    \\__tako_wrap_prompt() {
    \\  if [[ ${PROMPT:-} != *'133;A'*'133;B'* ]]; then
    \\    PROMPT=$'%{\033]133;A\007%}'"${PROMPT:-%# }"$'%{\033]133;B\007%}'
    \\  fi
    \\}
    \\__tako_precmd() {
    \\  local __tako_status=$?
    \\  if [[ $__tako_command_started == 1 ]]; then __tako_osc "133;D;$__tako_status"; fi
    \\  __tako_command_started=0
    \\  __tako_wrap_prompt
    \\}
    \\autoload -Uz add-zsh-hook
    \\add-zsh-hook preexec __tako_preexec
    \\add-zsh-hook precmd __tako_precmd
    \\__tako_wrap_prompt
    \\
;

const fish_integration =
    \\if set -q TAKO_SHELL_INTEGRATION_ACTIVE
    \\    return 0
    \\end
    \\set -gx TAKO_SHELL_INTEGRATION_ACTIVE 1
    \\function __tako_osc
    \\    printf '\033]%s\007' $argv[1]
    \\end
    \\if functions -q fish_prompt
    \\    functions -c fish_prompt __tako_original_fish_prompt
    \\end
    \\function fish_prompt
    \\    __tako_osc '133;A'
    \\    if functions -q __tako_original_fish_prompt
    \\        __tako_original_fish_prompt
    \\    end
    \\    __tako_osc '133;B'
    \\end
    \\function __tako_preexec --on-event fish_preexec
    \\    __tako_osc '133;C'
    \\end
    \\function __tako_postexec --on-event fish_postexec
    \\    __tako_osc "133;D;$status"
    \\end
    \\
;

fn saturatingAddU32(a: u32, b: u32) u32 {
    const max = std.math.maxInt(u32);
    return if (b > max - a) max else a + b;
}

fn errnoIsAgain() bool {
    const value = std.c._errno().*;
    return value == c.EAGAIN or value == c.EWOULDBLOCK;
}

fn setNonblocking(fd: c_int) void {
    const flags = c.fcntl(fd, c.F_GETFL, @as(c_int, 0));
    if (flags >= 0) _ = c.fcntl(fd, c.F_SETFL, flags | c.O_NONBLOCK);
}

fn basename(path: []const u8) []const u8 {
    if (std.mem.lastIndexOfScalar(u8, path, '/')) |idx| return path[idx + 1 ..];
    return path;
}

fn shellKind(program: []const u8) enum { bash, zsh, fish, none } {
    const name = std.mem.trimLeft(u8, basename(program), "-");
    if (std.mem.eql(u8, name, "bash")) return .bash;
    if (std.mem.eql(u8, name, "zsh")) return .zsh;
    if (std.mem.eql(u8, name, "fish")) return .fish;
    return .none;
}

fn createRuntimeDir() ![]u8 {
    const base = envVar("XDG_RUNTIME_DIR") orelse envVar("TMPDIR") orelse "/tmp";
    const pid = c.getpid();
    const stamp = std.time.nanoTimestamp();
    var attempt: u32 = 0;
    while (attempt < 32) : (attempt += 1) {
        const path = try std.fmt.allocPrint(
            allocator,
            "{s}/tako-shell-{d}-{d}-{d}",
            .{ base, pid, stamp, attempt },
        );
        std.fs.makeDirAbsolute(path) catch |err| {
            allocator.free(path);
            if (err == error.PathAlreadyExists) continue;
            return err;
        };
        return path;
    }
    return error.PathAlreadyExists;
}

fn writeAbsolute(path: []const u8, data: []const u8) !void {
    try std.fs.cwd().writeFile(.{ .sub_path = path, .data = data });
}

fn shellQuote(path: []const u8) ![]u8 {
    return std.fmt.allocPrint(allocator, "'{s}'", .{path});
}

fn appendArg(argv: []?[*:0]u8, count: *usize, value: []const u8) !void {
    const owned = try allocator.dupeZ(u8, value);
    argv[count.*] = owned.ptr;
    count.* += 1;
}

fn appendEnv(env: []EnvVar, count: *usize, name: []const u8, value: []const u8) !void {
    env[count.*] = .{
        .name = try allocator.dupeZ(u8, name),
        .value = try allocator.dupeZ(u8, value),
    };
    count.* += 1;
}

fn buildSpawnCommand(options: *const TerminalOptions) !SpawnCommand {
    const raw_program = if (options.program) |p| blk: {
        const value = std.mem.span(p);
        if (value.len > 0) break :blk value;
        break :blk envVar("SHELL") orelse "/bin/sh";
    } else envVar("SHELL") orelse "/bin/sh";

    const max_args = 5;
    var argv = try allocator.alloc(?[*:0]u8, max_args);
    errdefer allocator.free(argv);
    var argc: usize = 0;
    try appendArg(argv, &argc, raw_program);

    var env = try allocator.alloc(EnvVar, 4);
    errdefer allocator.free(env);
    var envc: usize = 0;

    var runtime_root: ?[]u8 = null;
    errdefer if (runtime_root) |root| {
        std.fs.deleteTreeAbsolute(root) catch {};
        allocator.free(root);
    };

    if (options.shell_integration) switch (shellKind(raw_program)) {
        .bash => {
            const root = try createRuntimeDir();
            runtime_root = root;
            const script = try std.fmt.allocPrint(allocator, "{s}/tako.bash", .{root});
            defer allocator.free(script);
            const rcfile = try std.fmt.allocPrint(allocator, "{s}/bashrc", .{root});
            defer allocator.free(rcfile);
            try writeAbsolute(script, bash_integration);
            const quoted = try shellQuote(script);
            defer allocator.free(quoted);
            const rc = try std.fmt.allocPrint(
                allocator,
                "if [ -r \"$HOME/.bashrc\" ]; then . \"$HOME/.bashrc\"; fi\n. {s}\n",
                .{quoted},
            );
            defer allocator.free(rc);
            try writeAbsolute(rcfile, rc);
            try appendEnv(env, &envc, "TAKO_SHELL_INTEGRATION", "1");
            try appendArg(argv, &argc, "--rcfile");
            try appendArg(argv, &argc, rcfile);
            try appendArg(argv, &argc, "-i");
        },
        .zsh => {
            const root = try createRuntimeDir();
            runtime_root = root;
            const script = try std.fmt.allocPrint(allocator, "{s}/tako.zsh", .{root});
            defer allocator.free(script);
            const zshenv = try std.fmt.allocPrint(allocator, "{s}/.zshenv", .{root});
            defer allocator.free(zshenv);
            try writeAbsolute(script, zsh_integration);
            const quoted = try shellQuote(script);
            defer allocator.free(quoted);
            const rc = try std.fmt.allocPrint(
                allocator,
                "export ZDOTDIR=\"$TAKO_ORIGINAL_ZDOTDIR\"\nif [ -n \"${{TAKO_ORIGINAL_ZDOTDIR:-}}\" ] && [ -r \"$TAKO_ORIGINAL_ZDOTDIR/.zshenv\" ]; then . \"$TAKO_ORIGINAL_ZDOTDIR/.zshenv\"; fi\n. {s}\n",
                .{quoted},
            );
            defer allocator.free(rc);
            try writeAbsolute(zshenv, rc);
            try appendEnv(env, &envc, "TAKO_SHELL_INTEGRATION", "1");
            try appendEnv(env, &envc, "TAKO_ORIGINAL_ZDOTDIR", envVar("ZDOTDIR") orelse envVar("HOME") orelse "");
            try appendEnv(env, &envc, "ZDOTDIR", root);
        },
        .fish => {
            const root = try createRuntimeDir();
            runtime_root = root;
            const script = try std.fmt.allocPrint(allocator, "{s}/tako.fish", .{root});
            defer allocator.free(script);
            try writeAbsolute(script, fish_integration);
            const quoted = try shellQuote(script);
            defer allocator.free(quoted);
            const init = try std.fmt.allocPrint(allocator, "source {s}", .{quoted});
            defer allocator.free(init);
            try appendEnv(env, &envc, "TAKO_SHELL_INTEGRATION", "1");
            try appendArg(argv, &argc, "--init-command");
            try appendArg(argv, &argc, init);
        },
        .none => {},
    };

    argv[argc] = null;
    argv = try allocator.realloc(argv, argc + 1);
    env = try allocator.realloc(env, envc);

    const cwd = if (options.working_directory) |p| blk: {
        const value = std.mem.span(p);
        if (value.len == 0) break :blk null;
        break :blk try allocator.dupeZ(u8, value);
    } else null;

    return .{
        .program = std.mem.span(argv[0].?),
        .cwd = cwd,
        .argv = argv,
        .env = env,
        .runtime_root = runtime_root,
    };
}

fn childExec(master: c_int, slave: c_int, command: *const SpawnCommand) noreturn {
    _ = c.close(master);
    _ = c.setsid();
    _ = c.ioctl(slave, c.TIOCSCTTY, @as(c_int, 0));
    _ = c.dup2(slave, 0);
    _ = c.dup2(slave, 1);
    _ = c.dup2(slave, 2);
    if (slave > 2) _ = c.close(slave);
    if (command.cwd) |cwd| _ = c.chdir(cwd.ptr);
    _ = c.setenv("TERM", "xterm-256color", 1);
    for (command.env) |entry| {
        _ = c.setenv(entry.name.ptr, entry.value.ptr, 1);
    }
    _ = c.execvp(command.program.ptr, @ptrCast(command.argv.ptr));
    c._exit(127);
}

const TerminalSession = struct {
    terminal: ghostty.GhosttyTerminal,
    render_state: ghostty.GhosttyRenderState,
    surface: ?*FontCore,
    cols: u16,
    rows: u16,
    pty: ?*PtySession,
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
    glyph_atlas: OwnedGlyphAtlas,
    pending_bell_count: u32 = 0,
    autorun: Autorun = .{},
};

const focus_event_mode: ghostty.GhosttyMode = 1004;
const bracketed_paste_mode: ghostty.GhosttyMode = 2004;
const sync_output_mode: ghostty.GhosttyMode = 2026;
const repeat_interval_ns: u64 = 500_000_000;
const repeat_distance_px: f64 = 8.0;
const flat_uv: f32 = -1.0;
const title_data: ghostty.GhosttyTerminalData = @intCast(ghostty.GHOSTTY_TERMINAL_DATA_TITLE);
const pwd_data: ghostty.GhosttyTerminalData = @intCast(ghostty.GHOSTTY_TERMINAL_DATA_PWD);
const mouse_tracking_data: ghostty.GhosttyTerminalData =
    @intCast(ghostty.GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING);
const default_scrollback: usize = 10_000;
const version_string = "tako 0.1.0";
const enq_response = "\x1b[?1;2c";

const ShapeCache = struct {
    map: std.StringHashMap([]BackendShapedGlyph),
    order: std.ArrayList([]u8) = .empty,
    cap: usize = 4096,

    fn init() ShapeCache {
        return .{ .map = std.StringHashMap([]BackendShapedGlyph).init(allocator) };
    }

    fn deinit(self: *ShapeCache) void {
        var it = self.map.iterator();
        while (it.next()) |entry| {
            allocator.free(entry.key_ptr.*);
            allocator.free(entry.value_ptr.*);
        }
        self.map.deinit();
        self.order.deinit(allocator);
    }

    fn clear(self: *ShapeCache) void {
        var it = self.map.iterator();
        while (it.next()) |entry| {
            allocator.free(entry.key_ptr.*);
            allocator.free(entry.value_ptr.*);
        }
        self.map.clearRetainingCapacity();
        self.order.clearRetainingCapacity();
    }

    fn get(self: *ShapeCache, text: []const u8) ?[]BackendShapedGlyph {
        return self.map.get(text);
    }

    fn put(self: *ShapeCache, text: []const u8, glyphs: []BackendShapedGlyph) !void {
        if (!self.map.contains(text)) {
            while (self.map.count() >= self.cap and self.order.items.len > 0) {
                const oldest = self.order.orderedRemove(0);
                if (self.map.fetchRemove(oldest)) |entry| {
                    allocator.free(entry.key);
                    allocator.free(entry.value);
                    break;
                }
                allocator.free(oldest);
            }
            const key = try allocator.dupe(u8, text);
            errdefer allocator.free(key);
            try self.order.append(allocator, key);
            try self.map.put(key, glyphs);
        } else {
            allocator.free(glyphs);
        }
    }
};

const FontCore = struct {
    font_path: [:0]u8,
    logical_pixel_height: u32,
    dpr: f32,
    physical_pixel_height: u32,
    library: ft.FT_Library,
    face: ft.FT_Face,
    font_bytes: []u8,
    hb_blob: ?*hb.hb_blob_t,
    hb_face: ?*hb.hb_face_t,
    hb_font: ?*hb.hb_font_t,
    units_per_em: u32,
    cell: CellMetrics,
    shape_cache: ShapeCache,
    shaped_buf: std.ArrayList(BackendShapedGlyph) = .empty,
    raster_pixels: std.ArrayList(u8) = .empty,

    fn create(font_path: [*:0]const u8, logical_pixel_height: u32, dpr: f32) ?*FontCore {
        const owned_path = allocator.dupeZ(u8, std.mem.span(font_path)) catch return null;
        errdefer allocator.free(owned_path);

        var library: ft.FT_Library = null;
        if (ft.FT_Init_FreeType(&library) != 0 or library == null) return null;
        errdefer _ = ft.FT_Done_FreeType(library);

        var face: ft.FT_Face = null;
        if (ft.FT_New_Face(library, owned_path.ptr, 0, &face) != 0 or face == null) return null;
        errdefer _ = ft.FT_Done_Face(face);

        const physical_px = physicalFontSize(logical_pixel_height, dpr);
        if (ft.FT_Set_Pixel_Sizes(face, physical_px, physical_px) != 0) return null;

        const font_bytes = std.fs.openFileAbsolute(owned_path, .{}) catch return null;
        defer font_bytes.close();
        const bytes = font_bytes.readToEndAlloc(allocator, 64 * 1024 * 1024) catch return null;
        errdefer allocator.free(bytes);

        const blob = hb.hb_blob_create(
            @ptrCast(bytes.ptr),
            @intCast(bytes.len),
            hb.HB_MEMORY_MODE_READONLY,
            null,
            null,
        ) orelse return null;
        errdefer hb.hb_blob_destroy(blob);

        const hb_face = hb.hb_face_create(blob, 0) orelse return null;
        errdefer hb.hb_face_destroy(hb_face);

        const hb_font = hb.hb_font_create(hb_face) orelse return null;
        errdefer hb.hb_font_destroy(hb_font);
        hb.hb_ot_font_set_funcs(hb_font);

        const upem = @max(hb.hb_face_get_upem(hb_face), 1);
        hb.hb_font_set_scale(hb_font, @intCast(upem), @intCast(upem));

        const core = allocator.create(FontCore) catch return null;
        core.* = .{
            .font_path = owned_path,
            .logical_pixel_height = logical_pixel_height,
            .dpr = dpr,
            .physical_pixel_height = physical_px,
            .library = library,
            .face = face,
            .font_bytes = bytes,
            .hb_blob = blob,
            .hb_face = hb_face,
            .hb_font = hb_font,
            .units_per_em = upem,
            .cell = computeCellMetrics(face),
            .shape_cache = ShapeCache.init(),
        };
        return core;
    }

    fn destroy(self: *FontCore) void {
        self.shape_cache.deinit();
        self.shaped_buf.deinit(allocator);
        self.raster_pixels.deinit(allocator);
        if (self.hb_font) |font| hb.hb_font_destroy(font);
        if (self.hb_face) |face| hb.hb_face_destroy(face);
        if (self.hb_blob) |blob| hb.hb_blob_destroy(blob);
        _ = ft.FT_Done_Face(self.face);
        _ = ft.FT_Done_FreeType(self.library);
        allocator.free(self.font_bytes);
        allocator.free(self.font_path);
        allocator.destroy(self);
    }

    fn setDpr(self: *FontCore, dpr: f32) void {
        if (@abs(dpr - self.dpr) < 0.01) return;
        self.dpr = dpr;
        self.physical_pixel_height = physicalFontSize(self.logical_pixel_height, dpr);
        if (ft.FT_Set_Pixel_Sizes(self.face, self.physical_pixel_height, self.physical_pixel_height) != 0) return;
        self.cell = computeCellMetrics(self.face);
        self.shape_cache.clear();
        self.raster_pixels.clearRetainingCapacity();
    }

    fn setFont(self: *FontCore, font_path: [*:0]const u8, logical_pixel_height: u32) bool {
        const replacement = FontCore.create(font_path, logical_pixel_height, self.dpr) orelse return false;
        const old = self.*;
        self.* = replacement.*;
        allocator.destroy(replacement);
        var old_copy = old;
        old_copy.destroyMoved();
        return true;
    }

    fn destroyMoved(self: *FontCore) void {
        self.shape_cache.deinit();
        self.shaped_buf.deinit(allocator);
        self.raster_pixels.deinit(allocator);
        if (self.hb_font) |font| hb.hb_font_destroy(font);
        if (self.hb_face) |face| hb.hb_face_destroy(face);
        if (self.hb_blob) |blob| hb.hb_blob_destroy(blob);
        _ = ft.FT_Done_Face(self.face);
        _ = ft.FT_Done_FreeType(self.library);
        allocator.free(self.font_bytes);
        allocator.free(self.font_path);
    }

    fn shapeText(self: *FontCore, text: []const u8) bool {
        self.shaped_buf.clearRetainingCapacity();
        if (self.shape_cache.get(text)) |cached| {
            self.shaped_buf.appendSlice(allocator, cached) catch return false;
            return true;
        }

        const buffer = hb.hb_buffer_create() orelse return false;
        defer hb.hb_buffer_destroy(buffer);
        hb.hb_buffer_add_utf8(buffer, @ptrCast(text.ptr), @intCast(text.len), 0, @intCast(text.len));
        hb.hb_buffer_guess_segment_properties(buffer);
        hb.hb_shape(self.hb_font, buffer, null, 0);

        var count: c_uint = 0;
        const infos = hb.hb_buffer_get_glyph_infos(buffer, &count);
        const positions = hb.hb_buffer_get_glyph_positions(buffer, &count);
        if (infos == null or positions == null) return false;

        const scale = @as(f32, @floatFromInt(self.physical_pixel_height)) / @as(f32, @floatFromInt(self.units_per_em));
        var shaped = allocator.alloc(BackendShapedGlyph, count) catch return false;
        errdefer allocator.free(shaped);
        var i: usize = 0;
        while (i < count) : (i += 1) {
            shaped[i] = .{
                .glyph_id = infos[i].codepoint,
                .x_offset = @as(f32, @floatFromInt(positions[i].x_offset)) * scale,
                .y_offset = @as(f32, @floatFromInt(positions[i].y_offset)) * scale,
                .x_advance = @as(f32, @floatFromInt(positions[i].x_advance)) * scale,
            };
        }
        self.shaped_buf.appendSlice(allocator, shaped) catch return false;
        self.shape_cache.put(text, shaped) catch return false;
        return true;
    }

    fn rasterizeGlyph(self: *FontCore, glyph_id: u32) BackendRasterizedGlyph {
        self.raster_pixels.clearRetainingCapacity();
        if (ft.FT_Load_Glyph(self.face, glyph_id, ft.FT_LOAD_RENDER) != 0) {
            return .{ .glyph_id = glyph_id, .width = 0, .height = 0, .left_bearing = 0, .top_bearing = 0, .pixels = null, .pixel_len = 0 };
        }
        const slot = self.face.*.glyph;
        const bitmap = slot.*.bitmap;
        const width: u32 = @intCast(@max(bitmap.width, 0));
        const height: u32 = @intCast(@max(bitmap.rows, 0));
        if (width > 0 and height > 0 and bitmap.buffer != null) {
            const pitch_abs: usize = @intCast(if (bitmap.pitch < 0) -bitmap.pitch else bitmap.pitch);
            self.raster_pixels.ensureTotalCapacity(allocator, @as(usize, width) * @as(usize, height)) catch {};
            var row: usize = 0;
            while (row < height) : (row += 1) {
                const src_start = if (bitmap.pitch >= 0)
                    row * pitch_abs
                else
                    (@as(usize, height) - 1 - row) * pitch_abs;
                self.raster_pixels.appendSlice(allocator, bitmap.buffer[src_start .. src_start + width]) catch {};
            }
        }
        return .{
            .glyph_id = glyph_id,
            .width = width,
            .height = height,
            .left_bearing = slot.*.bitmap_left,
            .top_bearing = slot.*.bitmap_top,
            .pixels = if (self.raster_pixels.items.len == 0) null else self.raster_pixels.items.ptr,
            .pixel_len = self.raster_pixels.items.len,
        };
    }
};

fn sessionFromUserdata(userdata: ?*anyopaque) ?*TerminalSession {
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
    terminal: ghostty.GhosttyTerminal,
    option: ghostty.GhosttyTerminalOption,
    callback: anytype,
) void {
    _ = ghostty.ghostty_terminal_set(terminal, option, @ptrCast(callback));
}

fn registerTerminalEffects(session: *TerminalSession) void {
    const terminal = session.terminal;
    _ = ghostty.ghostty_terminal_set(
        terminal,
        @intCast(ghostty.GHOSTTY_TERMINAL_OPT_USERDATA),
        @ptrCast(session),
    );
    setTerminalCallback(terminal, @intCast(ghostty.GHOSTTY_TERMINAL_OPT_WRITE_PTY), &effectWritePty);
    setTerminalCallback(terminal, @intCast(ghostty.GHOSTTY_TERMINAL_OPT_BELL), &effectBell);
    setTerminalCallback(terminal, @intCast(ghostty.GHOSTTY_TERMINAL_OPT_TITLE_CHANGED), &effectChanged);
    setTerminalCallback(terminal, @intCast(ghostty.GHOSTTY_TERMINAL_OPT_PWD_CHANGED), &effectChanged);
    setTerminalCallback(terminal, @intCast(ghostty.GHOSTTY_TERMINAL_OPT_XTVERSION), &effectXtversion);
    setTerminalCallback(terminal, @intCast(ghostty.GHOSTTY_TERMINAL_OPT_ENQUIRY), &effectEnquiry);
    setTerminalCallback(terminal, @intCast(ghostty.GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES), &effectDeviceAttributes);
    setTerminalCallback(terminal, @intCast(ghostty.GHOSTTY_TERMINAL_OPT_SIZE), &effectSize);
    setTerminalCallback(terminal, @intCast(ghostty.GHOSTTY_TERMINAL_OPT_COLOR_SCHEME), &effectColorScheme);
}

fn flushPtyResponses(session: *TerminalSession) void {
    if (session.pty_response.items.len == 0) return;
    const pty = session.pty orelse return;
    pty.write(session.pty_response.items);
    session.pty_response.clearRetainingCapacity();
}

fn writeOptionalBytes(bytes: []const u8, out_buf: ?[*]u8, cap: usize) usize {
    const out = out_buf orelse return 0;
    if (cap == 0 or bytes.len + 1 > cap) return 0;
    @memcpy(out[0..bytes.len], bytes);
    out[bytes.len] = 0;
    return bytes.len;
}

fn optionalCString(ptr: ?[*:0]const u8) ?[]const u8 {
    const p = ptr orelse return null;
    const value = std.mem.span(p);
    return if (std.mem.trim(u8, value, " \t\r\n").len == 0) null else value;
}

fn physicalFontSize(logical_pixel_height: u32, dpr: f32) u32 {
    const px: u32 = @intFromFloat(@round(@as(f32, @floatFromInt(logical_pixel_height)) * dpr));
    return @max(px, 1);
}

fn computeCellMetrics(face: ft.FT_Face) CellMetrics {
    var ascent: i32 = 1;
    var descent: i32 = 0;
    var height: u32 = 1;
    if (face != null and face.*.size != null) {
        const metrics = face.*.size.*.metrics;
        ascent = @intCast(metrics.ascender >> 6);
        descent = @intCast(metrics.descender >> 6);
        height = @intCast(@max((metrics.height + 32) >> 6, 1));
    }
    return .{
        .cell_width = @intCast(@max(glyphAdvancePx(face, 'M'), 1)),
        .cell_height = height,
        .cell_ascent = ascent,
        .cell_descent = descent,
    };
}

fn glyphAdvancePx(face: ft.FT_Face, codepoint: u32) i32 {
    if (face == null) return 0;
    const glyph_index = ft.FT_Get_Char_Index(face, codepoint);
    if (glyph_index == 0) return 0;
    if (ft.FT_Load_Glyph(face, glyph_index, ft.FT_LOAD_DEFAULT) != 0) return 0;
    return @intCast(face.*.glyph.*.advance.x >> 6);
}

fn resolveFontPath(
    explicit_path: ?[*:0]const u8,
    family: ?[*:0]const u8,
) ?[:0]u8 {
    if (optionalCString(explicit_path)) |path| {
        return allocator.dupeZ(u8, path) catch null;
    }

    const requested = optionalCString(family) orelse "monospace";
    const argv = [_][]const u8{ "fc-match", "-f", "%{file}", requested };
    const result = std.process.Child.run(.{
        .allocator = allocator,
        .argv = &argv,
        .max_output_bytes = std.fs.max_path_bytes,
    }) catch |err| {
        std.log.warn("fc-match failed for font family '{s}': {s}", .{ requested, @errorName(err) });
        return null;
    };
    defer allocator.free(result.stdout);
    defer allocator.free(result.stderr);

    switch (result.term) {
        .Exited => |code| if (code != 0) {
            std.log.warn("fc-match returned {d} for font family '{s}'", .{ code, requested });
            return null;
        },
        else => {
            std.log.warn("fc-match did not exit normally for font family '{s}'", .{requested});
            return null;
        },
    }

    const path = std.mem.trim(u8, result.stdout, " \t\r\n");
    if (path.len == 0) {
        std.log.warn("fc-match returned empty path for font family '{s}'", .{requested});
        return null;
    }
    return allocator.dupeZ(u8, path) catch null;
}

fn writeFormattedSelection(session: ?*TerminalSession, out_buf: ?[*]u8, cap: usize) usize {
    const terminal = terminalHandle(session);
    const out = out_buf orelse return 0;
    if (terminal == null or cap == 0) return 0;

    const options = ghostty.GhosttyTerminalSelectionFormatOptions{
        .size = @sizeOf(ghostty.GhosttyTerminalSelectionFormatOptions),
        .emit = @intCast(ghostty.GHOSTTY_FORMATTER_FORMAT_PLAIN),
        .unwrap = true,
        .trim = true,
        .selection = null,
    };
    var written: usize = 0;
    const result = ghostty.ghostty_terminal_selection_format_buf(
        terminal,
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

fn allocFormattedSelection(session: ?*TerminalSession) TerminalBytes {
    const terminal = terminalHandle(session);
    if (terminal == null) return emptyBytes();

    var ptr: ?[*]u8 = null;
    var len: usize = 0;
    const result = ghostty.ghostty_terminal_selection_format_alloc(
        terminal,
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

fn sessionSurface(session: ?*TerminalSession) ?*FontCore {
    const s = session orelse return null;
    return s.surface;
}

fn sessionCellMetrics(session: ?*TerminalSession, out: *CellMetrics) bool {
    const s = session orelse return false;
    return fontCoreCellMetrics(s.surface, out);
}

fn sessionMouseGeometry(session: ?*TerminalSession, out: *MouseGeometry) bool {
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

fn gridForPixels(cell: CellMetrics, width_px: u32, height_px: u32) struct { cols: u16, rows: u16 } {
    const cw = @max(cell.cell_width, 1);
    const ch = @max(cell.cell_height, 1);
    const max_u16 = std.math.maxInt(u16);
    return .{
        .cols = @intCast(@min(@max(@divFloor(width_px, cw), 1), max_u16)),
        .rows = @intCast(@min(@max(@divFloor(height_px, ch), 1), max_u16)),
    };
}

fn terminalHandle(session: ?*TerminalSession) ghostty.GhosttyTerminal {
    const s = session orelse return null;
    return s.terminal;
}

fn renderStateHandle(session: ?*TerminalSession) ghostty.GhosttyRenderState {
    const s = session orelse return null;
    return s.render_state;
}

fn freeTerminalCore(
    terminal: ghostty.GhosttyTerminal,
    render_state: ghostty.GhosttyRenderState,
) void {
    if (render_state != null) {
        ghostty.ghostty_render_state_free(render_state);
    }
    if (terminal != null) {
        ghostty.ghostty_terminal_free(terminal);
    }
}

fn clearRenderStateDirty(render_state: ghostty.GhosttyRenderState) void {
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

fn updateRenderState(
    render_state: ghostty.GhosttyRenderState,
    terminal: ghostty.GhosttyTerminal,
) bool {
    if (render_state == null or terminal == null) return false;
    const result = ghostty.ghostty_render_state_update(render_state, terminal);
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

fn captureFrameState(session: ?*TerminalSession) ?FrameState {
    const s = session orelse return null;
    if (!updateRenderState(s.render_state, s.terminal)) return null;

    const dirty = renderStateGetDirty(s.render_state) orelse return null;
    const visible = renderStateGetBool(
        s.render_state,
        @intCast(ghostty.GHOSTTY_RENDER_STATE_DATA_CURSOR_VISIBLE),
    ) orelse return null;
    const viewport_present = renderStateGetBool(
        s.render_state,
        @intCast(ghostty.GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_HAS_VALUE),
    ) orelse return null;
    const viewport_x = if (viewport_present)
        renderStateGetU16(
            s.render_state,
            @intCast(ghostty.GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_X),
        ) orelse return null
    else
        0;
    const viewport_y = if (viewport_present)
        renderStateGetU16(
            s.render_state,
            @intCast(ghostty.GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_Y),
        ) orelse return null
    else
        0;
    const wide_tail = viewport_present and (renderStateGetBool(
        s.render_state,
        @intCast(ghostty.GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_WIDE_TAIL),
    ) orelse return null);
    const style = renderStateGetCursorStyle(s.render_state) orelse return null;
    const blinking = renderStateGetBool(
        s.render_state,
        @intCast(ghostty.GHOSTTY_RENDER_STATE_DATA_CURSOR_BLINKING),
    ) orelse return null;
    const password_input = renderStateGetBool(
        s.render_state,
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

fn captureFrameSnapshot(
    session: ?*TerminalSession,
    frame_state: FrameState,
    buffers: *SnapshotBuffers,
) ?BackendFrameSnapshot {
    const s = session orelse return null;
    const render_state = s.render_state;
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
        dst.* = rgbToBackend(src);
    }

    return .{
        .foreground = rgbToBackend(raw.foreground),
        .background = rgbToBackend(raw.background),
        .cursor_present = raw.cursor_has_value,
        .cursor = rgbToBackend(raw.cursor),
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
                selection != null and rowCellSelectedRaw(selection.?, col),
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
        .text_fg = rgbZero(),
        .fg_present = fg != null,
        .fg = fg orelse rgbZero(),
        .bg_present = bg != null,
        .bg = bg orelse rgbZero(),
    };
    const defaults = EffectiveColors{ .fg = colors.foreground, .bg = colors.background };
    const effective = effectiveCellColors(out, selected, defaults);
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
        return rgbToBackend(raw);
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

fn rgbToBackend(rgb: ghostty.GhosttyColorRgb) BackendRgb {
    return .{ .r = rgb.r, .g = rgb.g, .b = rgb.b };
}

fn rgbZero() BackendRgb {
    return .{ .r = 0, .g = 0, .b = 0 };
}

fn markNeedsReplan(session: ?*TerminalSession) void {
    const s = session orelse return;
    s.needs_replan = true;
}

fn writeLastPlan(session: ?*TerminalSession, out: ?*FramePlan) void {
    const target = out orelse return;
    const s = session orelse {
        target.* = std.mem.zeroes(FramePlan);
        return;
    };
    target.* = s.last_plan;
}

fn finalizeFramePlan(
    session: *TerminalSession,
    plan: *FramePlan,
    snapshot: *const BackendFrameSnapshot,
    cursor: CursorState,
) bool {
    session.frame_vertices.clearRetainingCapacity();

    var cell_w: f32 = 1.0;
    var cell_h: f32 = 1.0;
    var cell_ascent: f32 = 1.0;
    var cell: CellMetrics = undefined;
    if (sessionCellMetrics(session, &cell)) {
        cell_w = @floatFromInt(cell.cell_width);
        cell_h = @floatFromInt(cell.cell_height);
        cell_ascent = @floatFromInt(cell.cell_ascent);
    }

    appendCellBackgroundVertices(session, snapshot, cell_w, cell_h) catch return false;
    appendCellGlyphVertices(session, snapshot, cell_w, cell_h, cell_ascent) catch return false;
    appendCellDecorationVertices(session, snapshot, cell_w, cell_h) catch return false;
    appendPreeditGlyphVertices(session, snapshot, cursor, cell_w, cell_h, cell_ascent) catch return false;
    appendPreeditOverlayVertices(session, snapshot, cursor, cell_w, cell_h) catch return false;
    appendCursorVertices(session, snapshot, cursor, cell_w, cell_h) catch return false;

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
    plan.vertices = if (session.frame_vertices.items.len == 0)
        null
    else
        session.frame_vertices.items.ptr;
    plan.vertex_count = session.frame_vertices.items.len;
    plan.atlas_w = session.glyph_atlas.width;
    plan.atlas_h = session.glyph_atlas.height;
    plan.atlas_pixels = if (session.glyph_atlas.pixels.items.len == 0)
        null
    else
        session.glyph_atlas.pixels.items.ptr;
    plan.atlas_generation = session.glyph_atlas.generation;
    return true;
}

fn appendCellGlyphVertices(
    session: *TerminalSession,
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
                session,
                bytes,
                col_x,
                baseline,
                cell.text_fg,
                false,
            );
        }
    }
}

fn appendPreeditGlyphVertices(
    session: *TerminalSession,
    snapshot: *const BackendFrameSnapshot,
    cursor: CursorState,
    cell_w: f32,
    cell_h: f32,
    cell_ascent: f32,
) !void {
    const preedit = session.preedit orelse return;
    if (preedit.len == 0 or !cursor.viewport_present) return;
    if (cursor.viewport_x >= snapshot.cols or cursor.viewport_y >= snapshot.rows) return;

    const max_cells = @as(usize, snapshot.cols - cursor.viewport_x);
    if (max_cells == 0) return;

    const px = @as(f32, @floatFromInt(cursor.viewport_x)) * cell_w;
    const py = @as(f32, @floatFromInt(cursor.viewport_y)) * cell_h;
    try appendTextRunGlyphVertices(
        session,
        preedit,
        px,
        py + cell_ascent,
        snapshot.colors.foreground,
        true,
    );
}

fn appendTextRunGlyphVertices(
    session: *TerminalSession,
    text: []const u8,
    origin_x: f32,
    baseline: f32,
    color: BackendRgb,
    apply_offsets: bool,
) !void {
    if (text.len == 0) return;

    var shaped: BackendShapedText = std.mem.zeroes(BackendShapedText);
    if (!fontCoreShapeText(session.surface, text.ptr, text.len, &shaped)) return;
    if (shaped.glyphs == null or shaped.glyph_count == 0) return;

    var pen_x = origin_x;
    const glyphs = shaped.glyphs[0..shaped.glyph_count];
    for (glyphs) |glyph| {
        const offset_x = if (apply_offsets) glyph.x_offset else 0.0;
        const offset_y = if (apply_offsets) glyph.y_offset else 0.0;
        const atlas_glyph = session.glyph_atlas.ensureGlyph(session.surface, glyph.glyph_id) catch {
            pen_x += glyph.x_advance;
            continue;
        };
        if (atlas_glyph.w > 0 and atlas_glyph.h > 0 and session.glyph_atlas.height > 0) {
            const inv_w = 1.0 / @as(f32, @floatFromInt(session.glyph_atlas.width));
            const inv_h = 1.0 / @as(f32, @floatFromInt(session.glyph_atlas.height));
            const tex_u0 = @as(f32, @floatFromInt(atlas_glyph.x)) * inv_w;
            const tex_v0 = @as(f32, @floatFromInt(atlas_glyph.y)) * inv_h;
            const tex_u1 = @as(f32, @floatFromInt(atlas_glyph.x + atlas_glyph.w)) * inv_w;
            const tex_v1 = @as(f32, @floatFromInt(atlas_glyph.y + atlas_glyph.h)) * inv_h;
            try pushTexturedQuad(
                &session.frame_vertices,
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
    session: *TerminalSession,
    snapshot: *const BackendFrameSnapshot,
    cell_w: f32,
    cell_h: f32,
) !void {
    if (snapshot.rows_ptr == null or snapshot.row_count == 0) return;

    const defaults = EffectiveColors{
        .fg = snapshot.colors.foreground,
        .bg = snapshot.colors.background,
    };
    const rows = snapshot.rows_ptr[0..snapshot.row_count];
    for (rows, 0..) |row, row_i| {
        if (row.cells == null or row.cell_count == 0) continue;

        const cells = row.cells[0..row.cell_count];
        const row_y = @as(f32, @floatFromInt(row_i)) * cell_h;
        for (cells, 0..) |cell, col_i| {
            const colors = effectiveCellColors(cell, rowCellSelected(row, col_i), defaults);
            if (rgbEqual(colors.bg, defaults.bg)) continue;

            try pushFlatQuad(
                &session.frame_vertices,
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
    session: *TerminalSession,
    snapshot: *const BackendFrameSnapshot,
    cell_w: f32,
    cell_h: f32,
) !void {
    if (snapshot.rows_ptr == null or snapshot.row_count == 0) return;

    const defaults = EffectiveColors{
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

            const color = effectiveCellColors(cell, rowCellSelected(row, col_i), defaults).fg;
            const col_x = @as(f32, @floatFromInt(col_i)) * cell_w;
            if (cell.style.overline) {
                try pushFlatQuad(&session.frame_vertices, col_x, row_y, cell_w, decoration_h, color);
            }
            if (cell.style.strikethrough) {
                try pushFlatQuad(
                    &session.frame_vertices,
                    col_x,
                    row_y + (cell_h * 0.55),
                    cell_w,
                    decoration_h,
                    color,
                );
            }
            if (cell.style.underline) {
                try pushFlatQuad(
                    &session.frame_vertices,
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

const EffectiveColors = struct {
    fg: BackendRgb,
    bg: BackendRgb,
};

fn effectiveCellColors(cell: BackendCell, selected: bool, defaults: EffectiveColors) EffectiveColors {
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

fn rowCellSelected(row: BackendRow, col: usize) bool {
    if (!row.selection_present) return false;
    return rowCellSelectedRaw(.{
        .start_x = row.selection_start_x,
        .end_x = row.selection_end_x,
    }, col);
}

fn rowCellSelectedRaw(selection: RowSelection, col: usize) bool {
    return col >= selection.start_x and col <= selection.end_x;
}

fn swapRgb(a: *BackendRgb, b: *BackendRgb) void {
    const tmp = a.*;
    a.* = b.*;
    b.* = tmp;
}

fn rgbEqual(a: BackendRgb, b: BackendRgb) bool {
    return a.r == b.r and a.g == b.g and a.b == b.b;
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

fn appendPreeditOverlayVertices(
    session: *TerminalSession,
    snapshot: *const BackendFrameSnapshot,
    cursor: CursorState,
    cell_w: f32,
    cell_h: f32,
) !void {
    const preedit = session.preedit orelse return;
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
        &session.frame_vertices,
        px,
        py + cell_h - underline_h,
        @as(f32, @floatFromInt(text_cells)) * cell_w,
        underline_h,
        fg,
    );

    const prefix_len = validUtf8PrefixLen(preedit, @min(session.preedit_cursor_byte, preedit.len));
    const cursor_cells = @min(terminalDisplayWidth(preedit[0..prefix_len]), max_cells);
    try pushFlatQuad(
        &session.frame_vertices,
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
    session: *TerminalSession,
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
            &session.frame_vertices,
            px,
            py,
            @max(cell_w * 0.125, 1.0),
            cell_h,
            color,
        ),
        2 => try pushFlatQuad(
            &session.frame_vertices,
            px,
            py + cell_h - @max(cell_h * 0.125, 1.0),
            cell_w,
            @max(cell_h * 0.125, 1.0),
            color,
        ),
        3 => {
            const border = snapshot.colors.foreground;
            const thickness = @max(@min(cell_w, cell_h) * 0.1, 1.0);
            try pushFlatQuad(&session.frame_vertices, px, py, cell_w, thickness, border);
            try pushFlatQuad(
                &session.frame_vertices,
                px,
                py + cell_h - thickness,
                cell_w,
                thickness,
                border,
            );
            try pushFlatQuad(
                &session.frame_vertices,
                px,
                py + thickness,
                thickness,
                cell_h - 2.0 * thickness,
                border,
            );
            try pushFlatQuad(
                &session.frame_vertices,
                px + cell_w - thickness,
                py + thickness,
                thickness,
                cell_h - 2.0 * thickness,
                border,
            );
        },
        else => try pushFlatQuad(&session.frame_vertices, px, py, cell_w, cell_h, color),
    }
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

fn cursorStatesEqual(a: CursorState, b: CursorState) bool {
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

fn presentedCursorState(cursor: CursorState, focused: bool, blink_visible: bool) CursorState {
    var presented = cursor;
    if (cursorShouldBeHidden(cursor, focused, blink_visible)) {
        presented.visible = false;
    }
    return presented;
}

fn setSessionFocused(session: ?*TerminalSession, focused: bool) void {
    const s = session orelse return;
    if (s.focused == focused) return;
    s.focused = focused;
    markNeedsReplan(s);
}

fn setSessionCursorBlinkVisible(session: ?*TerminalSession, visible: bool) void {
    const s = session orelse return;
    if (s.cursor_blink_visible == visible) return;
    s.cursor_blink_visible = visible;
    markNeedsReplan(s);
}

fn setSessionPreedit(
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

fn writeSessionBytes(session: ?*TerminalSession, bytes: []const u8) void {
    if (bytes.len == 0) return;
    const s = session orelse return;
    const pty = s.pty orelse return;
    pty.write(bytes);
}

fn terminalMode(session: ?*TerminalSession, mode: ghostty.GhosttyMode) bool {
    const terminal = terminalHandle(session);
    if (terminal == null) return false;

    var enabled = false;
    const result = ghostty.ghostty_terminal_mode_get(terminal, mode, &enabled);
    return result == ghostty.GHOSTTY_SUCCESS and enabled;
}

fn envVar(name: [*:0]const u8) ?[]const u8 {
    const value = getenv(name) orelse return null;
    return std.mem.span(value);
}

fn buildAutorun() Autorun {
    const raw_cmd = envVar("TAKO_AUTORUN") orelse return .{};
    if (raw_cmd.len == 0) return .{};

    var cmd = allocator.alloc(u8, raw_cmd.len + 1) catch return .{};
    @memcpy(cmd[0..raw_cmd.len], raw_cmd);
    cmd[raw_cmd.len] = '\n';

    const delay = delay: {
        const raw_delay = envVar("TAKO_AUTORUN_DELAY_MS") orelse break :delay 2000;
        break :delay std.fmt.parseInt(i64, raw_delay, 10) catch 2000;
    };

    return .{
        .cmd = cmd,
        .start_ms = std.time.milliTimestamp(),
        .delay_ms = @max(delay, 0),
    };
}

fn freeAutorun(autorun: *Autorun) void {
    if (autorun.cmd) |cmd| {
        allocator.free(cmd);
        autorun.cmd = null;
    }
}

fn maybeFireAutorun(session: ?*TerminalSession) void {
    const s = session orelse return;
    if (s.autorun.fired) return;
    const cmd = s.autorun.cmd orelse return;
    if (std.time.milliTimestamp() - s.autorun.start_ms < s.autorun.delay_ms) return;

    s.autorun.fired = true;
    writeSessionBytes(s, cmd);
}

fn addPendingBellCount(session: ?*TerminalSession, count: u32) void {
    if (count == 0) return;
    const s = session orelse return;
    const max = std.math.maxInt(u32);
    s.pending_bell_count = if (count > max - s.pending_bell_count)
        max
    else
        s.pending_bell_count + count;
}

fn terminalOptionForColorRole(role: u32) ?ghostty.GhosttyTerminalOption {
    return switch (role) {
        0 => @intCast(ghostty.GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND),
        1 => @intCast(ghostty.GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND),
        2 => @intCast(ghostty.GHOSTTY_TERMINAL_OPT_COLOR_CURSOR),
        else => null,
    };
}

fn cursorStyle(style: u32) ghostty.GhosttyTerminalCursorStyle {
    return switch (style) {
        0 => @intCast(ghostty.GHOSTTY_TERMINAL_CURSOR_STYLE_BAR),
        2 => @intCast(ghostty.GHOSTTY_TERMINAL_CURSOR_STYLE_UNDERLINE),
        3 => @intCast(ghostty.GHOSTTY_TERMINAL_CURSOR_STYLE_BLOCK_HOLLOW),
        else => @intCast(ghostty.GHOSTTY_TERMINAL_CURSOR_STYLE_BLOCK),
    };
}

fn scrollViewport(session: ?*TerminalSession, behavior: ghostty.GhosttyTerminalScrollViewport) void {
    const terminal = terminalHandle(session);
    if (terminal == null) return;

    ghostty.ghostty_terminal_scroll_viewport(terminal, behavior);
    markNeedsReplan(session);
}

fn terminalDataBool(session: ?*TerminalSession, data: ghostty.GhosttyTerminalData) bool {
    const terminal = terminalHandle(session);
    if (terminal == null) return false;

    var enabled = false;
    const result = ghostty.ghostty_terminal_get(terminal, data, &enabled);
    return result == ghostty.GHOSTTY_SUCCESS and enabled;
}

fn terminalDataString(session: ?*TerminalSession, data: ghostty.GhosttyTerminalData) ?[]const u8 {
    const terminal = terminalHandle(session);
    if (terminal == null) return null;

    var value: ghostty.GhosttyString = undefined;
    const result = ghostty.ghostty_terminal_get(terminal, data, &value);
    if (result != ghostty.GHOSTTY_SUCCESS or value.ptr == null) return null;
    return value.ptr[0..value.len];
}

fn gridRefAtPixels(session: ?*TerminalSession, x_px: f32, y_px: f32) ?ghostty.GhosttyGridRef {
    const terminal = terminalHandle(session);
    if (terminal == null) return null;

    var geometry: MouseGeometry = undefined;
    if (!sessionMouseGeometry(session, &geometry)) return null;
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
    const result = ghostty.ghostty_terminal_grid_ref(terminal, point, &ref);
    if (result != ghostty.GHOSTTY_SUCCESS) return null;
    return ref;
}

fn emptyGridRef() ghostty.GhosttyGridRef {
    return .{
        .size = @sizeOf(ghostty.GhosttyGridRef),
        .node = null,
        .x = 0,
        .y = 0,
    };
}

fn emptySelection() ghostty.GhosttySelection {
    return .{
        .size = @sizeOf(ghostty.GhosttySelection),
        .start = emptyGridRef(),
        .end = emptyGridRef(),
        .rectangle = false,
    };
}

fn installSelection(session: ?*TerminalSession, selection: *const ghostty.GhosttySelection) i32 {
    const terminal = terminalHandle(session);
    if (terminal == null) return 0;

    const result = ghostty.ghostty_terminal_set(
        terminal,
        @intCast(ghostty.GHOSTTY_TERMINAL_OPT_SELECTION),
        selection,
    );
    if (result != ghostty.GHOSTTY_SUCCESS) return 0;
    markNeedsReplan(session);
    return 1;
}

fn activeCursorSelection(terminal: ghostty.GhosttyTerminal) ?ghostty.GhosttySelection {
    var x: u16 = 0;
    const x_result = ghostty.ghostty_terminal_get(
        terminal,
        @intCast(ghostty.GHOSTTY_TERMINAL_DATA_CURSOR_X),
        &x,
    );
    if (x_result != ghostty.GHOSTTY_SUCCESS) return null;

    var y: u16 = 0;
    const y_result = ghostty.ghostty_terminal_get(
        terminal,
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
    const ref_result = ghostty.ghostty_terminal_grid_ref(terminal, point, &ref);
    if (ref_result != ghostty.GHOSTTY_SUCCESS) return null;

    return .{
        .size = @sizeOf(ghostty.GhosttySelection),
        .start = ref,
        .end = ref,
        .rectangle = false,
    };
}

fn currentSelectionOrCursor(terminal: ghostty.GhosttyTerminal) ?ghostty.GhosttySelection {
    var selection = emptySelection();
    const selection_result = ghostty.ghostty_terminal_get(
        terminal,
        @intCast(ghostty.GHOSTTY_TERMINAL_DATA_SELECTION),
        &selection,
    );
    if (selection_result == ghostty.GHOSTTY_SUCCESS) return selection;
    return activeCursorSelection(terminal);
}

fn clearInstalledSelection(session: ?*TerminalSession) void {
    const terminal = terminalHandle(session);
    if (terminal == null) return;
    const result = ghostty.ghostty_terminal_set(
        terminal,
        @intCast(ghostty.GHOSTTY_TERMINAL_OPT_SELECTION),
        null,
    );
    if (result == ghostty.GHOSTTY_SUCCESS) markNeedsReplan(session);
}

fn resetSelectionGesture(session: ?*TerminalSession) void {
    const s = session orelse return;
    if (s.selection_gesture == null) return;
    ghostty.ghostty_selection_gesture_reset(s.selection_gesture, terminalHandle(s));
}

fn clearSelectionSession(session: ?*TerminalSession) void {
    const s = session orelse return;
    clearInstalledSelection(s);
    resetSelectionGesture(s);
}

fn viewportCoordinateAtPixels(geometry: MouseGeometry, x_px: f32, y_px: f32) ?ghostty.GhosttyPointCoordinate {
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

fn selectionGestureGeometry(geometry: MouseGeometry) ?ghostty.GhosttySelectionGestureGeometry {
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

fn clearGestureOption(
    event: ghostty.GhosttySelectionGestureEvent,
    option: ghostty.GhosttySelectionGestureEventOption,
) void {
    _ = ghostty.ghostty_selection_gesture_event_set(event, option, null);
}

fn setGestureRef(event: ghostty.GhosttySelectionGestureEvent, ref: *const ghostty.GhosttyGridRef) void {
    setGestureOption(
        event,
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF),
        ref,
    );
}

fn setGesturePosition(event: ghostty.GhosttySelectionGestureEvent, x_px: f32, y_px: f32) void {
    const position = ghostty.GhosttySurfacePosition{ .x = x_px, .y = y_px };
    setGestureOption(
        event,
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_OPT_POSITION),
        &position,
    );
}

fn setGestureGeometry(
    event: ghostty.GhosttySelectionGestureEvent,
    geometry: *const ghostty.GhosttySelectionGestureGeometry,
) void {
    setGestureOption(
        event,
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_OPT_GEOMETRY),
        geometry,
    );
}

fn setGestureRectangle(event: ghostty.GhosttySelectionGestureEvent, rectangle: *const bool) void {
    setGestureOption(
        event,
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_OPT_RECTANGLE),
        rectangle,
    );
}

fn setGestureViewport(
    event: ghostty.GhosttySelectionGestureEvent,
    coord: *const ghostty.GhosttyPointCoordinate,
) void {
    setGestureOption(
        event,
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_OPT_VIEWPORT),
        coord,
    );
}

fn sanitizeGestureBehavior(value: u32) ghostty.GhosttySelectionGestureBehavior {
    return switch (value) {
        ghostty.GHOSTTY_SELECTION_GESTURE_BEHAVIOR_WORD => @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_BEHAVIOR_WORD),
        ghostty.GHOSTTY_SELECTION_GESTURE_BEHAVIOR_LINE => @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_BEHAVIOR_LINE),
        ghostty.GHOSTTY_SELECTION_GESTURE_BEHAVIOR_OUTPUT => @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_BEHAVIOR_OUTPUT),
        else => @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_BEHAVIOR_CELL),
    };
}

fn dispatchSelectionGesture(
    session: *TerminalSession,
    event: ghostty.GhosttySelectionGestureEvent,
) ?ghostty.GhosttySelection {
    const terminal = terminalHandle(session);
    if (terminal == null or session.selection_gesture == null or event == null) return null;

    var selection = emptySelection();
    const result = ghostty.ghostty_selection_gesture_event(
        session.selection_gesture,
        terminal,
        event,
        &selection,
    );
    if (result != ghostty.GHOSTTY_SUCCESS) return null;
    return selection;
}

fn gestureAutoscroll(session: ?*TerminalSession) i32 {
    const s = session orelse return 0;
    const terminal = terminalHandle(s);
    if (terminal == null or s.selection_gesture == null) return 0;

    var autoscroll: ghostty.GhosttySelectionGestureAutoscroll =
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_AUTOSCROLL_NONE);
    const result = ghostty.ghostty_selection_gesture_get(
        s.selection_gesture,
        terminal,
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_DATA_AUTOSCROLL),
        &autoscroll,
    );
    if (result != ghostty.GHOSTTY_SUCCESS) return 0;
    return @intCast(autoscroll);
}

fn writeHyperlinkAt(
    session: ?*TerminalSession,
    x_px: f32,
    y_px: f32,
    out_buf: ?[*]u8,
    cap: usize,
) usize {
    const out = out_buf orelse return 0;
    if (cap == 0) return 0;

    var ref = gridRefAtPixels(session, x_px, y_px) orelse return 0;
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

fn freeOptionalBytes(bytes: *?[]u8) void {
    if (bytes.*) |owned| {
        allocator.free(owned);
        bytes.* = null;
    }
}

fn takeChangedTerminalString(
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

fn keyConst(comptime key: anytype) ghostty.GhosttyKey {
    return @as(ghostty.GhosttyKey, @intCast(key));
}

fn unshiftedCodepoint(key: ghostty.GhosttyKey) u32 {
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

fn textContainsControl(bytes: []const u8) bool {
    for (bytes) |byte| {
        if (byte < 0x20 or byte == 0x7f) return true;
    }
    return false;
}

fn writeEncodedKey(session: *TerminalSession) void {
    const terminal = terminalHandle(session);
    if (terminal == null or session.key_encoder == null or session.key_event == null) return;

    ghostty.ghostty_key_encoder_setopt_from_terminal(session.key_encoder, terminal);

    var buf: [128]u8 = undefined;
    var written: usize = 0;
    const result = ghostty.ghostty_key_encoder_encode(
        session.key_encoder,
        session.key_event,
        @ptrCast(&buf),
        buf.len,
        &written,
    );
    if (result == ghostty.GHOSTTY_SUCCESS) {
        if (written > 0) {
            clearSelectionSession(session);
            writeSessionBytes(session, buf[0..written]);
        }
        return;
    }
    if (result != ghostty.GHOSTTY_OUT_OF_SPACE or written == 0) return;

    const out = allocator.alloc(u8, written) catch return;
    defer allocator.free(out);
    var written2: usize = 0;
    const result2 = ghostty.ghostty_key_encoder_encode(
        session.key_encoder,
        session.key_event,
        @ptrCast(out.ptr),
        out.len,
        &written2,
    );
    if (result2 == ghostty.GHOSTTY_SUCCESS and written2 > 0) {
        clearSelectionSession(session);
        writeSessionBytes(session, out[0..written2]);
    }
}

fn syncMouseGeometry(session: ?*TerminalSession) void {
    const s = session orelse return;
    if (s.mouse_encoder == null) return;

    var geometry: MouseGeometry = undefined;
    if (!sessionMouseGeometry(s, &geometry)) return;
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
        s.mouse_encoder,
        @intCast(ghostty.GHOSTTY_MOUSE_ENCODER_OPT_SIZE),
        &size,
    );
}

fn writeEncodedMouse(session: *TerminalSession) void {
    const terminal = terminalHandle(session);
    if (terminal == null or session.mouse_encoder == null or session.mouse_event == null) return;

    ghostty.ghostty_mouse_encoder_setopt_from_terminal(session.mouse_encoder, terminal);

    var buf: [64]u8 = undefined;
    var written: usize = 0;
    const result = ghostty.ghostty_mouse_encoder_encode(
        session.mouse_encoder,
        session.mouse_event,
        @ptrCast(&buf),
        buf.len,
        &written,
    );
    if (result == ghostty.GHOSTTY_SUCCESS) {
        if (written > 0) writeSessionBytes(session, buf[0..written]);
        return;
    }
    if (result != ghostty.GHOSTTY_OUT_OF_SPACE or written == 0) return;

    const out = allocator.alloc(u8, written) catch return;
    defer allocator.free(out);
    var written2: usize = 0;
    const result2 = ghostty.ghostty_mouse_encoder_encode(
        session.mouse_encoder,
        session.mouse_event,
        @ptrCast(out.ptr),
        out.len,
        &written2,
    );
    if (result2 == ghostty.GHOSTTY_SUCCESS and written2 > 0) {
        writeSessionBytes(session, out[0..written2]);
    }
}

fn newSelectionEvent(
    event_type: ghostty.GhosttySelectionGestureEventType,
) ?ghostty.GhosttySelectionGestureEvent {
    var event: ghostty.GhosttySelectionGestureEvent = null;
    const result = ghostty.ghostty_selection_gesture_event_new(null, &event, event_type);
    if (result != ghostty.GHOSTTY_SUCCESS or event == null) return null;
    return event;
}

fn freeSelectionResources(
    terminal: ghostty.GhosttyTerminal,
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
    ghostty.ghostty_selection_gesture_free(gesture, terminal);
}

pub export fn tako_terminal_core_engine_version(out_buf: ?[*]u8, cap: usize) usize {
    var version: ghostty.GhosttyString = undefined;
    const result = ghostty.ghostty_build_info(
        ghostty.GHOSTTY_BUILD_INFO_VERSION_STRING,
        &version,
    );
    if (result != ghostty.GHOSTTY_SUCCESS or version.ptr == null) return 0;
    return writeOptionalBytes(version.ptr[0..version.len], out_buf, cap);
}

pub export fn tako_terminal_bytes_free(bytes: TerminalBytes) void {
    const ptr = bytes.ptr orelse return;
    if (bytes.len == 0) return;
    allocator.free(@constCast(ptr)[0..bytes.len]);
}

pub export fn tako_terminal_session_new(options: ?*const TerminalOptions) ?*TerminalSession {
    const terminal_options = options orelse return null;
    const resolved_font = resolveFontPath(
        terminal_options.font_path,
        terminal_options.font_family,
    ) orelse return null;
    defer allocator.free(resolved_font);

    const backend_options = SurfaceOptions{
        .font_path = resolved_font.ptr,
        .pixel_height = terminal_options.pixel_height,
        .dpr = terminal_options.dpr,
    };
    const surface = fontCoreCreateWithOptions(&backend_options) orelse return null;
    var terminal: ghostty.GhosttyTerminal = null;
    const terminal_init_options = ghostty.GhosttyTerminalOptions{
        .cols = terminal_options.cols,
        .rows = terminal_options.rows,
        .max_scrollback = if (terminal_options.max_scrollback == 0)
            default_scrollback
        else
            terminal_options.max_scrollback,
    };
    var result = ghostty.ghostty_terminal_new(null, &terminal, terminal_init_options);
    if (result != ghostty.GHOSTTY_SUCCESS or terminal == null) {
        fontCoreDestroy(surface);
        return null;
    }
    var render_state: ghostty.GhosttyRenderState = null;
    result = ghostty.ghostty_render_state_new(null, &render_state);
    if (result != ghostty.GHOSTTY_SUCCESS or render_state == null) {
        ghostty.ghostty_terminal_free(terminal);
        fontCoreDestroy(surface);
        return null;
    }
    const pty = PtySession.spawn(terminal_options) orelse {
        freeTerminalCore(terminal, render_state);
        fontCoreDestroy(surface);
        return null;
    };
    var key_encoder: ghostty.GhosttyKeyEncoder = null;
    result = ghostty.ghostty_key_encoder_new(null, &key_encoder);
    if (result != ghostty.GHOSTTY_SUCCESS or key_encoder == null) {
        pty.destroy();
        freeTerminalCore(terminal, render_state);
        fontCoreDestroy(surface);
        return null;
    }
    var key_event: ghostty.GhosttyKeyEvent = null;
    result = ghostty.ghostty_key_event_new(null, &key_event);
    if (result != ghostty.GHOSTTY_SUCCESS or key_event == null) {
        ghostty.ghostty_key_encoder_free(key_encoder);
        pty.destroy();
        freeTerminalCore(terminal, render_state);
        fontCoreDestroy(surface);
        return null;
    }

    var mouse_encoder: ghostty.GhosttyMouseEncoder = null;
    result = ghostty.ghostty_mouse_encoder_new(null, &mouse_encoder);
    if (result != ghostty.GHOSTTY_SUCCESS or mouse_encoder == null) {
        ghostty.ghostty_key_event_free(key_event);
        ghostty.ghostty_key_encoder_free(key_encoder);
        pty.destroy();
        freeTerminalCore(terminal, render_state);
        fontCoreDestroy(surface);
        return null;
    }
    var mouse_event: ghostty.GhosttyMouseEvent = null;
    result = ghostty.ghostty_mouse_event_new(null, &mouse_event);
    if (result != ghostty.GHOSTTY_SUCCESS or mouse_event == null) {
        ghostty.ghostty_mouse_encoder_free(mouse_encoder);
        ghostty.ghostty_key_event_free(key_event);
        ghostty.ghostty_key_encoder_free(key_encoder);
        pty.destroy();
        freeTerminalCore(terminal, render_state);
        fontCoreDestroy(surface);
        return null;
    }

    var selection_gesture: ghostty.GhosttySelectionGesture = null;
    result = ghostty.ghostty_selection_gesture_new(null, &selection_gesture);
    if (result != ghostty.GHOSTTY_SUCCESS or selection_gesture == null) {
        ghostty.ghostty_mouse_event_free(mouse_event);
        ghostty.ghostty_mouse_encoder_free(mouse_encoder);
        ghostty.ghostty_key_event_free(key_event);
        ghostty.ghostty_key_encoder_free(key_encoder);
        pty.destroy();
        freeTerminalCore(terminal, render_state);
        fontCoreDestroy(surface);
        return null;
    }
    const selection_press = newSelectionEvent(
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_PRESS),
    ) orelse {
        freeSelectionResources(terminal, selection_gesture, null, null, null, null);
        ghostty.ghostty_mouse_event_free(mouse_event);
        ghostty.ghostty_mouse_encoder_free(mouse_encoder);
        ghostty.ghostty_key_event_free(key_event);
        ghostty.ghostty_key_encoder_free(key_encoder);
        pty.destroy();
        freeTerminalCore(terminal, render_state);
        fontCoreDestroy(surface);
        return null;
    };
    const selection_drag = newSelectionEvent(
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_DRAG),
    ) orelse {
        freeSelectionResources(terminal, selection_gesture, selection_press, null, null, null);
        ghostty.ghostty_mouse_event_free(mouse_event);
        ghostty.ghostty_mouse_encoder_free(mouse_encoder);
        ghostty.ghostty_key_event_free(key_event);
        ghostty.ghostty_key_encoder_free(key_encoder);
        pty.destroy();
        freeTerminalCore(terminal, render_state);
        fontCoreDestroy(surface);
        return null;
    };
    const selection_release = newSelectionEvent(
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_RELEASE),
    ) orelse {
        freeSelectionResources(terminal, selection_gesture, selection_press, selection_drag, null, null);
        ghostty.ghostty_mouse_event_free(mouse_event);
        ghostty.ghostty_mouse_encoder_free(mouse_encoder);
        ghostty.ghostty_key_event_free(key_event);
        ghostty.ghostty_key_encoder_free(key_encoder);
        pty.destroy();
        freeTerminalCore(terminal, render_state);
        fontCoreDestroy(surface);
        return null;
    };
    const selection_autoscroll_tick = newSelectionEvent(
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_AUTOSCROLL_TICK),
    ) orelse {
        freeSelectionResources(
            terminal,
            selection_gesture,
            selection_press,
            selection_drag,
            selection_release,
            null,
        );
        ghostty.ghostty_mouse_event_free(mouse_event);
        ghostty.ghostty_mouse_encoder_free(mouse_encoder);
        ghostty.ghostty_key_event_free(key_event);
        ghostty.ghostty_key_encoder_free(key_encoder);
        pty.destroy();
        freeTerminalCore(terminal, render_state);
        fontCoreDestroy(surface);
        return null;
    };

    const session = allocator.create(TerminalSession) catch {
        freeSelectionResources(
            terminal,
            selection_gesture,
            selection_press,
            selection_drag,
            selection_release,
            selection_autoscroll_tick,
        );
        ghostty.ghostty_mouse_event_free(mouse_event);
        ghostty.ghostty_mouse_encoder_free(mouse_encoder);
        ghostty.ghostty_key_event_free(key_event);
        ghostty.ghostty_key_encoder_free(key_encoder);
        pty.destroy();
        freeTerminalCore(terminal, render_state);
        fontCoreDestroy(surface);
        return null;
    };
    session.* = .{
        .terminal = terminal,
        .render_state = render_state,
        .surface = surface,
        .cols = terminal_options.cols,
        .rows = terminal_options.rows,
        .pty = pty,
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
        .glyph_atlas = OwnedGlyphAtlas.init(),
        .autorun = buildAutorun(),
    };
    registerTerminalEffects(session);
    syncMouseGeometry(session);
    return session;
}

pub export fn tako_terminal_session_destroy(session: ?*TerminalSession) void {
    const s = session orelse return;
    freeAutorun(&s.autorun);
    freeOptionalBytes(&s.title);
    freeOptionalBytes(&s.pwd);
    freeOptionalBytes(&s.preedit);
    if (s.key_event != null) {
        ghostty.ghostty_key_event_free(s.key_event);
        s.key_event = null;
    }
    if (s.key_encoder != null) {
        ghostty.ghostty_key_encoder_free(s.key_encoder);
        s.key_encoder = null;
    }
    if (s.mouse_event != null) {
        ghostty.ghostty_mouse_event_free(s.mouse_event);
        s.mouse_event = null;
    }
    if (s.mouse_encoder != null) {
        ghostty.ghostty_mouse_encoder_free(s.mouse_encoder);
        s.mouse_encoder = null;
    }
    freeSelectionResources(
        terminalHandle(s),
        s.selection_gesture,
        s.selection_press,
        s.selection_drag,
        s.selection_release,
        s.selection_autoscroll_tick,
    );
    s.selection_gesture = null;
    s.selection_press = null;
    s.selection_drag = null;
    s.selection_release = null;
    s.selection_autoscroll_tick = null;
    if (s.pty) |pty| {
        pty.destroy();
        s.pty = null;
    }
    s.pty_response.deinit(allocator);
    s.frame_vertices.deinit(allocator);
    s.glyph_atlas.deinit();
    if (s.render_state != null) {
        ghostty.ghostty_render_state_free(s.render_state);
        s.render_state = null;
    }
    if (s.terminal != null) {
        ghostty.ghostty_terminal_free(s.terminal);
        s.terminal = null;
    }
    if (s.surface) |surface| {
        fontCoreDestroy(surface);
        s.surface = null;
    }
    allocator.destroy(s);
}

pub export fn tako_terminal_session_tick(session: ?*TerminalSession, out: ?*FramePlan) bool {
    maybeFireAutorun(session);
    if (session) |s| {
        if (s.pty) |pty| {
            _ = pty.drainIntoTerminal(s.terminal);
            flushPtyResponses(s);
        }
    }
    if (terminalMode(session, sync_output_mode)) {
        writeLastPlan(session, out);
        return false;
    }
    const force_replan = if (session) |s| s.needs_replan else false;

    const frame_state = captureFrameState(session) orelse {
        writeLastPlan(session, out);
        return false;
    };

    const focused = if (session) |s| s.focused else false;
    const cursor_blink_visible = if (session) |s| s.cursor_blink_visible else true;
    const next_cursor = presentedCursorState(frame_state.cursor, focused, cursor_blink_visible);

    var should_build = frame_state.content_dirty or force_replan;
    if (session) |s| {
        should_build = should_build or !cursorStatesEqual(s.last_cursor, next_cursor);
    }
    if (!should_build) {
        if (session) |s| {
            s.needs_replan = false;
            s.last_cursor = next_cursor;
        }
        writeLastPlan(session, out);
        return false;
    }

    var snapshot_buffers = SnapshotBuffers{};
    defer snapshot_buffers.deinit();
    const snapshot = captureFrameSnapshot(session, frame_state, &snapshot_buffers) orelse {
        if (session) |s| {
            s.needs_replan = true;
        }
        writeLastPlan(session, out);
        return false;
    };

    var next_plan: FramePlan = std.mem.zeroes(FramePlan);
    if (session) |s| {
        s.last_cursor = next_cursor;
        const built = finalizeFramePlan(s, &next_plan, &snapshot, next_cursor);
        if (built) {
            s.needs_replan = false;
            s.last_plan = next_plan;
            clearRenderStateDirty(s.render_state);
            if (out) |target| target.* = next_plan;
        } else {
            s.needs_replan = true;
            writeLastPlan(s, out);
        }
        return built;
    }
    return false;
}

pub export fn tako_terminal_session_notify_fd(session: ?*TerminalSession) i32 {
    const s = session orelse return -1;
    const pty = s.pty orelse return -1;
    return pty.notifyFd();
}

pub export fn tako_terminal_session_exited(session: ?*TerminalSession) i32 {
    const s = session orelse return 0;
    const pty = s.pty orelse return 1;
    return if (pty.isExited()) 1 else 0;
}

pub export fn tako_terminal_session_drain_notify(session: ?*TerminalSession) void {
    _ = session;
}

pub export fn tako_terminal_session_resize_pixels(
    session: ?*TerminalSession,
    width_px: u32,
    height_px: u32,
) void {
    if (session) |s| {
        var cell: CellMetrics = undefined;
        if (sessionCellMetrics(s, &cell)) {
            const grid = gridForPixels(cell, width_px, height_px);
            const changed = grid.cols != s.cols or grid.rows != s.rows;
            s.cols = grid.cols;
            s.rows = grid.rows;
            if (changed) {
                if (s.pty) |pty| pty.resize(s.cols, s.rows);
                _ = ghostty.ghostty_terminal_resize(
                    s.terminal,
                    s.cols,
                    s.rows,
                    cell.cell_width,
                    cell.cell_height,
                );
            }
        }
    }
    syncMouseGeometry(session);
}

pub export fn tako_terminal_session_set_dpr(session: ?*TerminalSession, dpr: f32) void {
    fontCoreSetDpr(sessionSurface(session), dpr);
    if (session) |s| s.glyph_atlas.reset();
    markNeedsReplan(session);
    syncMouseGeometry(session);
}

pub export fn tako_terminal_session_set_focused(session: ?*TerminalSession, focused: bool) void {
    setSessionFocused(session, focused);
}

pub export fn tako_terminal_session_set_cursor_blink_visible(
    session: ?*TerminalSession,
    visible: bool,
) void {
    setSessionCursorBlinkVisible(session, visible);
}

pub export fn tako_terminal_session_set_preedit(
    session: ?*TerminalSession,
    data: ?[*]const u8,
    len: usize,
    cursor_byte: usize,
) void {
    setSessionPreedit(session, data, len, cursor_byte);
}

pub export fn tako_terminal_session_set_default_color(
    session: ?*TerminalSession,
    role: u32,
    enabled: bool,
    r: u8,
    g: u8,
    b: u8,
) i32 {
    const terminal = terminalHandle(session);
    const opt = terminalOptionForColorRole(role) orelse return 0;
    if (terminal == null) return 0;

    var color = ghostty.GhosttyColorRgb{ .r = r, .g = g, .b = b };
    const value: ?*const anyopaque = if (enabled) @ptrCast(&color) else null;
    const result = ghostty.ghostty_terminal_set(terminal, opt, value);
    if (result != ghostty.GHOSTTY_SUCCESS) return 0;
    markNeedsReplan(session);
    return 1;
}

pub export fn tako_terminal_session_set_default_palette(
    session: ?*TerminalSession,
    enabled: bool,
    rgb_triplets: ?[*]const u8,
    len: usize,
) i32 {
    const terminal = terminalHandle(session);
    if (terminal == null) return 0;

    if (!enabled) {
        const result = ghostty.ghostty_terminal_set(
            terminal,
            @intCast(ghostty.GHOSTTY_TERMINAL_OPT_COLOR_PALETTE),
            null,
        );
        if (result != ghostty.GHOSTTY_SUCCESS) return 0;
        markNeedsReplan(session);
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
        terminal,
        @intCast(ghostty.GHOSTTY_TERMINAL_OPT_COLOR_PALETTE),
        &palette,
    );
    if (result != ghostty.GHOSTTY_SUCCESS) return 0;
    markNeedsReplan(session);
    return 1;
}

pub export fn tako_terminal_session_set_default_cursor(
    session: ?*TerminalSession,
    style: u32,
    blink: bool,
) i32 {
    const terminal = terminalHandle(session);
    if (terminal == null) return 0;

    var mapped_style = cursorStyle(style);
    var mapped_blink = blink;
    const style_result = ghostty.ghostty_terminal_set(
        terminal,
        @intCast(ghostty.GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_STYLE),
        &mapped_style,
    );
    const blink_result = ghostty.ghostty_terminal_set(
        terminal,
        @intCast(ghostty.GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_BLINK),
        &mapped_blink,
    );
    if (style_result != ghostty.GHOSTTY_SUCCESS or blink_result != ghostty.GHOSTTY_SUCCESS) {
        return 0;
    }
    markNeedsReplan(session);
    return 1;
}

pub export fn tako_terminal_session_set_font(
    session: ?*TerminalSession,
    font_path: ?[*:0]const u8,
    font_family: ?[*:0]const u8,
    pixel_height: u32,
) i32 {
    const resolved_font = resolveFontPath(font_path, font_family) orelse return 0;
    defer allocator.free(resolved_font);

    const result = fontCoreSetFont(sessionSurface(session), resolved_font.ptr, pixel_height);
    if (result != 0) {
        if (session) |s| s.glyph_atlas.reset();
        markNeedsReplan(session);
        syncMouseGeometry(session);
    }
    return result;
}

pub export fn tako_terminal_session_write(
    session: ?*TerminalSession,
    data: ?[*]const u8,
    len: usize,
) void {
    if (len == 0) return;
    const s = session orelse return;
    const pty = s.pty orelse return;
    const bytes = data orelse return;
    pty.write(bytes[0..len]);
}

pub export fn tako_terminal_session_take_bell_count(session: ?*TerminalSession) u32 {
    const s = session orelse return 0;
    const count = s.pending_bell_count;
    s.pending_bell_count = 0;
    return count;
}

pub export fn tako_terminal_session_hyperlink_at(
    session: ?*TerminalSession,
    x_px: f32,
    y_px: f32,
    out_buf: ?[*]u8,
    cap: usize,
) usize {
    return writeHyperlinkAt(session, x_px, y_px, out_buf, cap);
}

pub export fn tako_terminal_session_paste(
    session: ?*TerminalSession,
    data: ?[*]const u8,
    len: usize,
) void {
    const bracketed = terminalMode(session, bracketed_paste_mode);

    if (len == 0) {
        if (bracketed) {
            const empty_bracketed = "\x1b[200~\x1b[201~";
            writeSessionBytes(session, empty_bracketed);
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
    writeSessionBytes(session, out[0..written]);
}

pub export fn tako_terminal_session_scroll(session: ?*TerminalSession, delta_rows: i64) void {
    scrollViewport(session, ghostty.GhosttyTerminalScrollViewport{
        .tag = @intCast(ghostty.GHOSTTY_SCROLL_VIEWPORT_DELTA),
        .value = .{ .delta = @intCast(delta_rows) },
    });
}

pub export fn tako_terminal_session_scroll_to_top(session: ?*TerminalSession) void {
    scrollViewport(session, ghostty.GhosttyTerminalScrollViewport{
        .tag = @intCast(ghostty.GHOSTTY_SCROLL_VIEWPORT_TOP),
        .value = .{ .delta = 0 },
    });
}

pub export fn tako_terminal_session_scroll_to_bottom(session: ?*TerminalSession) void {
    scrollViewport(session, ghostty.GhosttyTerminalScrollViewport{
        .tag = @intCast(ghostty.GHOSTTY_SCROLL_VIEWPORT_BOTTOM),
        .value = .{ .delta = 0 },
    });
}

pub export fn tako_terminal_session_scroll_to_row(session: ?*TerminalSession, row: u64) void {
    scrollViewport(session, ghostty.GhosttyTerminalScrollViewport{
        .tag = @intCast(ghostty.GHOSTTY_SCROLL_VIEWPORT_ROW),
        .value = .{ .row = @intCast(row) },
    });
}

pub export fn tako_terminal_session_scrollbar_state(
    session: ?*TerminalSession,
    out: ?*ScrollbarState,
) bool {
    const state = out orelse return false;
    const terminal = terminalHandle(session);
    if (terminal == null) return false;

    var scrollbar: ghostty.GhosttyTerminalScrollbar = undefined;
    const result = ghostty.ghostty_terminal_get(
        terminal,
        @intCast(ghostty.GHOSTTY_TERMINAL_DATA_SCROLLBAR),
        &scrollbar,
    );
    if (result != ghostty.GHOSTTY_SUCCESS) return false;

    var viewport_active = false;
    const active_result = ghostty.ghostty_terminal_get(
        terminal,
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

pub export fn tako_terminal_session_mouse_tracking(session: ?*TerminalSession) i32 {
    return @intFromBool(terminalDataBool(session, mouse_tracking_data));
}

pub export fn tako_terminal_session_mouse_set_any_button(
    session: ?*TerminalSession,
    pressed: bool,
) void {
    const s = session orelse return;
    if (s.mouse_encoder == null) return;
    ghostty.ghostty_mouse_encoder_setopt(
        s.mouse_encoder,
        @intCast(ghostty.GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED),
        &pressed,
    );
}

pub export fn tako_terminal_session_key_event(
    session: ?*TerminalSession,
    action: u32,
    key: u32,
    mods: u16,
    consumed_mods: u16,
    text: ?[*]const u8,
    text_len: usize,
) void {
    const s = session orelse return;
    if (s.key_event == null) return;

    const key_value: ghostty.GhosttyKey = @intCast(key);
    ghostty.ghostty_key_event_set_action(s.key_event, @intCast(action));
    ghostty.ghostty_key_event_set_key(s.key_event, key_value);
    ghostty.ghostty_key_event_set_mods(s.key_event, mods);
    ghostty.ghostty_key_event_set_consumed_mods(s.key_event, consumed_mods);
    ghostty.ghostty_key_event_set_unshifted_codepoint(s.key_event, unshiftedCodepoint(key_value));

    if (text) |ptr| {
        const bytes = ptr[0..text_len];
        if (text_len > 0 and !textContainsControl(bytes)) {
            ghostty.ghostty_key_event_set_utf8(s.key_event, @ptrCast(ptr), text_len);
        } else {
            ghostty.ghostty_key_event_set_utf8(s.key_event, null, 0);
        }
    } else {
        ghostty.ghostty_key_event_set_utf8(s.key_event, null, 0);
    }

    writeEncodedKey(s);
}

pub export fn tako_terminal_session_mouse_event(
    session: ?*TerminalSession,
    action: u32,
    button: u32,
    x_px: f32,
    y_px: f32,
    mods: u16,
) void {
    const s = session orelse return;
    if (s.mouse_event == null) return;

    // Tracked mouse mode owns pointer events; abort any view-side selection
    // gesture before reporting the event to the PTY.
    clearSelectionSession(s);

    ghostty.ghostty_mouse_event_set_action(s.mouse_event, @intCast(action));
    if (button == 0) {
        ghostty.ghostty_mouse_event_clear_button(s.mouse_event);
    } else {
        ghostty.ghostty_mouse_event_set_button(s.mouse_event, @intCast(button));
    }
    ghostty.ghostty_mouse_event_set_mods(s.mouse_event, mods);
    ghostty.ghostty_mouse_event_set_position(
        s.mouse_event,
        ghostty.GhosttyMousePosition{ .x = x_px, .y = y_px },
    );
    writeEncodedMouse(s);
}

pub export fn tako_terminal_session_selection_begin(
    session: ?*TerminalSession,
    x_px: f32,
    y_px: f32,
    time_ns: u64,
    mods: u16,
    single_click: u32,
    double_click: u32,
    triple_click: u32,
) void {
    const s = session orelse return;
    if (s.selection_press == null) return;
    const ref = gridRefAtPixels(s, x_px, y_px) orelse return;
    const rectangle = (mods & @as(u16, @intCast(ghostty.GHOSTTY_MODS_ALT))) != 0;
    const behaviors = ghostty.GhosttySelectionGestureBehaviors{
        .single_click = sanitizeGestureBehavior(single_click),
        .double_click = sanitizeGestureBehavior(double_click),
        .triple_click = sanitizeGestureBehavior(triple_click),
    };

    setGestureRef(s.selection_press, &ref);
    setGesturePosition(s.selection_press, x_px, y_px);
    setGestureOption(
        s.selection_press,
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_OPT_TIME_NS),
        &time_ns,
    );
    setGestureOption(
        s.selection_press,
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REPEAT_INTERVAL_NS),
        &repeat_interval_ns,
    );
    setGestureOption(
        s.selection_press,
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REPEAT_DISTANCE),
        &repeat_distance_px,
    );
    setGestureOption(
        s.selection_press,
        @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_OPT_BEHAVIORS),
        &behaviors,
    );
    setGestureRectangle(s.selection_press, &rectangle);

    if (dispatchSelectionGesture(s, s.selection_press)) |selection| {
        _ = installSelection(s, &selection);
    } else {
        clearInstalledSelection(s);
    }
}

pub export fn tako_terminal_session_selection_extend(
    session: ?*TerminalSession,
    x_px: f32,
    y_px: f32,
    mods: u16,
) i32 {
    const s = session orelse return 0;
    if (s.selection_drag == null) return 0;
    var geometry: MouseGeometry = undefined;
    if (!sessionMouseGeometry(s, &geometry)) return 0;
    const gesture_geometry = selectionGestureGeometry(geometry) orelse return 0;
    const ref = gridRefAtPixels(s, x_px, y_px) orelse return 0;
    const rectangle = (mods & @as(u16, @intCast(ghostty.GHOSTTY_MODS_ALT))) != 0;

    setGestureRef(s.selection_drag, &ref);
    setGesturePosition(s.selection_drag, x_px, y_px);
    setGestureGeometry(s.selection_drag, &gesture_geometry);
    setGestureRectangle(s.selection_drag, &rectangle);

    const selection = dispatchSelectionGesture(s, s.selection_drag) orelse return 0;
    return installSelection(s, &selection);
}

pub export fn tako_terminal_session_selection_autoscroll(session: ?*TerminalSession) i32 {
    return gestureAutoscroll(session);
}

pub export fn tako_terminal_session_selection_autoscroll_tick(
    session: ?*TerminalSession,
    x_px: f32,
    y_px: f32,
    mods: u16,
) i32 {
    const s = session orelse return 0;
    if (s.selection_autoscroll_tick == null) return 0;
    var geometry: MouseGeometry = undefined;
    if (!sessionMouseGeometry(s, &geometry)) return 0;
    const gesture_geometry = selectionGestureGeometry(geometry) orelse return 0;
    const coord = viewportCoordinateAtPixels(geometry, x_px, y_px) orelse return 0;
    const rectangle = (mods & @as(u16, @intCast(ghostty.GHOSTTY_MODS_ALT))) != 0;

    setGestureViewport(s.selection_autoscroll_tick, &coord);
    setGesturePosition(s.selection_autoscroll_tick, x_px, y_px);
    setGestureGeometry(s.selection_autoscroll_tick, &gesture_geometry);
    setGestureRectangle(s.selection_autoscroll_tick, &rectangle);

    const selection =
        dispatchSelectionGesture(s, s.selection_autoscroll_tick) orelse return 0;
    return installSelection(s, &selection);
}

pub export fn tako_terminal_session_selection_end(
    session: ?*TerminalSession,
    x_px: f32,
    y_px: f32,
    out_buf: ?[*]u8,
    cap: usize,
) usize {
    const s = session orelse return 0;
    if (s.selection_release != null) {
        if (gridRefAtPixels(s, x_px, y_px)) |ref| {
            setGestureRef(s.selection_release, &ref);
        } else {
            clearGestureOption(
                s.selection_release,
                @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF),
            );
        }
        _ = dispatchSelectionGesture(s, s.selection_release);
    }
    return writeFormattedSelection(s, out_buf, cap);
}

pub export fn tako_terminal_session_selection_text(
    session: ?*TerminalSession,
    out_buf: ?[*]u8,
    cap: usize,
) usize {
    return writeFormattedSelection(session, out_buf, cap);
}

pub export fn tako_terminal_session_selection_end_owned(
    session: ?*TerminalSession,
    x_px: f32,
    y_px: f32,
) TerminalBytes {
    const s = session orelse return emptyBytes();
    if (s.selection_release != null) {
        if (gridRefAtPixels(s, x_px, y_px)) |ref| {
            setGestureRef(s.selection_release, &ref);
        } else {
            clearGestureOption(
                s.selection_release,
                @intCast(ghostty.GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF),
            );
        }
        _ = dispatchSelectionGesture(s, s.selection_release);
    }
    return allocFormattedSelection(s);
}

pub export fn tako_terminal_session_selection_text_owned(session: ?*TerminalSession) TerminalBytes {
    return allocFormattedSelection(session);
}

pub export fn tako_terminal_session_selection_clear(session: ?*TerminalSession) void {
    clearSelectionSession(session);
}

pub export fn tako_terminal_session_selection_all(session: ?*TerminalSession) i32 {
    const terminal = terminalHandle(session);
    if (terminal == null) return 0;

    var selection = emptySelection();
    const result = ghostty.ghostty_terminal_select_all(terminal, &selection);
    if (result != ghostty.GHOSTTY_SUCCESS) return 0;
    return installSelection(session, &selection);
}

pub export fn tako_terminal_session_selection_output_at(
    session: ?*TerminalSession,
    x_px: f32,
    y_px: f32,
) i32 {
    const terminal = terminalHandle(session);
    if (terminal == null) return 0;
    const ref = gridRefAtPixels(session, x_px, y_px) orelse return 0;

    var selection = emptySelection();
    const result = ghostty.ghostty_terminal_select_output(terminal, ref, &selection);
    if (result != ghostty.GHOSTTY_SUCCESS) return 0;
    return installSelection(session, &selection);
}

pub export fn tako_terminal_session_selection_input_at(
    session: ?*TerminalSession,
    x_px: f32,
    y_px: f32,
) i32 {
    const terminal = terminalHandle(session);
    if (terminal == null) return 0;
    const ref = gridRefAtPixels(session, x_px, y_px) orelse return 0;

    const options = ghostty.GhosttyTerminalSelectLineOptions{
        .size = @sizeOf(ghostty.GhosttyTerminalSelectLineOptions),
        .ref = ref,
        .whitespace = null,
        .whitespace_len = 0,
        .semantic_prompt_boundary = true,
    };
    var selection = emptySelection();
    const result = ghostty.ghostty_terminal_select_line(terminal, &options, &selection);
    if (result != ghostty.GHOSTTY_SUCCESS) return 0;
    return installSelection(session, &selection);
}

pub export fn tako_terminal_session_selection_adjust(
    session: ?*TerminalSession,
    adjustment: u32,
) i32 {
    const terminal = terminalHandle(session);
    if (terminal == null) return 0;

    var selection = currentSelectionOrCursor(terminal) orelse return 0;
    _ = ghostty.ghostty_terminal_selection_adjust(
        terminal,
        &selection,
        @intCast(adjustment),
    );
    return installSelection(session, &selection);
}

pub export fn tako_terminal_session_focus_event(session: ?*TerminalSession, gained: bool) void {
    setSessionFocused(session, gained);
    if (!terminalMode(session, focus_event_mode)) return;

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
    writeSessionBytes(session, buf[0..written]);
}

pub export fn tako_terminal_session_take_title(
    session: ?*TerminalSession,
    out_buf: ?[*]u8,
    cap: usize,
) usize {
    const s = session orelse return 0;
    return takeChangedTerminalString(s, title_data, &s.title, out_buf, cap);
}

pub export fn tako_terminal_session_take_pwd(
    session: ?*TerminalSession,
    out_buf: ?[*]u8,
    cap: usize,
) usize {
    const s = session orelse return 0;
    return takeChangedTerminalString(s, pwd_data, &s.pwd, out_buf, cap);
}
