const std = @import("std");
const common = @import("common.zig");

const ghostty = common.ghostty;
const c = common.c;
const allocator = common.allocator;
const TerminalOptions = common.TerminalOptions;

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

pub const PtySession = struct {
    master_fd: c_int,
    pid: std.posix.pid_t,
    exited: bool = false,
    runtime_root: ?[]u8 = null,

    pub fn spawn(options: *const TerminalOptions) ?*PtySession {
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
        common.setNonblocking(master);

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

    pub fn destroy(self: *PtySession) void {
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

    pub fn drainIntoTerminal(self: *PtySession, terminal: ghostty.GhosttyTerminal) usize {
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

    pub fn write(self: *PtySession, bytes: []const u8) void {
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

    pub fn resize(self: *PtySession, cols: u16, rows: u16) void {
        var size = std.posix.winsize{
            .row = rows,
            .col = cols,
            .xpixel = 0,
            .ypixel = 0,
        };
        _ = c.ioctl(self.master_fd, c.TIOCSWINSZ, &size);
    }

    pub fn notifyFd(self: *PtySession) i32 {
        return if (self.exited) -1 else self.master_fd;
    }

    pub fn isExited(self: *PtySession) bool {
        self.reap();
        return self.exited;
    }

    fn reap(self: *PtySession) void {
        if (self.exited) return;
        const result = std.posix.waitpid(self.pid, std.posix.W.NOHANG);
        if (result.pid == self.pid) self.exited = true;
    }
};

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
    const base = common.envVar("XDG_RUNTIME_DIR") orelse common.envVar("TMPDIR") orelse "/tmp";
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
        break :blk common.envVar("SHELL") orelse "/bin/sh";
    } else common.envVar("SHELL") orelse "/bin/sh";

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
            try appendEnv(env, &envc, "TAKO_ORIGINAL_ZDOTDIR", common.envVar("ZDOTDIR") orelse common.envVar("HOME") orelse "");
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
