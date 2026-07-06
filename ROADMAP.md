# Tako — Roadmap

A native KDE terminal for AI coding agents. Built in Rust on Qt6/QML/Kirigami,
with [libghostty-vt](https://github.com/ghostty-org/ghostty) as the terminal
core. The product reference is [cmux](https://github.com/manaflow-ai/cmux)
(macOS/Swift); Tako is an independent KDE-native implementation, not a port of
cmux's code.

The metaphor: one shell, many arms. Tako is the surface where a developer runs
many concurrent coding agents (Claude Code, Codex, OpenCode, Grok, Gemini,
Aider, …), sees each one's state at a glance in a vertical sidebar, and is
notified only when an agent actually needs them.

---

## 1. Goals & scope

### Core (must-have for v1)

- **KDE-native.** Qt6/QML/Kirigami shell, KConfig/KConfigXT settings,
  KNotification, KGlobalAccel, KWindowSystem, Plasma themes/icons, Wayland +
  X11, `.desktop`/metainfo packaging. Idiomatic KDE behavior over
  cmux-feature-parity.
- **Fast, modern terminal.** libghostty-vt for VT fidelity; GPU rendering via
  Qt Quick RHI; reads `~/.config/ghostty/config` for themes/fonts/colors.
- **cmux-style UI layout.** Vertical sidebar (workspaces) + horizontal tab bar
  (surfaces inside panes) + binary split tree (panes). Per-row metadata: cwd,
  git branch + dirty dot, listening ports, latest notification text.
- **AI agent integration.** Sidebar status pill per agent; agent state machine
  (idle/working/needsInput/ended); session resume after restart; auto-naming.
- **Notifications.** OSC 9/99/777 + agent hooks → pane rings, sidebar unread
  badges, notification panel, Plasma desktop notifications, "jump to unread".
- **Basic git integration.** Branch + dirty detection (direct `.git` reads via
  `gix`); PR status/number via GitHub API polling.
- **Programmable.** `tako` CLI + D-Bus control interface. Every action exposed.

### In scope, past v1

- **Deeper git.** File explorer with per-path git status; diff viewer with
  per-repo review comments; agent-baseline diffing.
- **Embedded browser.** Qt WebEngine pane, scriptable via the control API
  (agent-browser-port-spec). Optional/plugin to keep the base app lean.

### Not early (revisit after v1)

- **Remote daemon / vault.** cmux's `cmuxd-remote` (Go) provides persistent
  remote PTYs, reverse-tunneled CLI relay, and cloud WebSocket transport —
  this is the substrate of `cmux ssh`. cmux's `vault` syncs local agent
  transcripts to cmux cloud storage. **Neither is core to a local KDE
  terminal.** When Tako adds SSH workspaces, reuse `cmuxd-remote` (it already
  ships linux/amd64+arm64 binaries) or reimplement in Rust. Vault depends on a
  cloud backend Tako doesn't have; skip until a cloud product exists.
- **Mobile / web.** Built on the same D-Bus control interface + event stream
  (no separate protocol). Web client = thin WebSocket bridge to D-Bus. Mobile
  = QtQuick Android (the Qt stack already supports it) or KDE-Connect-style
  pairing. Both consume the same control methods.

---

## 2. Architecture

### 2.1 Stack

| Layer | Choice |
|---|---|
| Host language | Rust (edition 2021+) |
| Terminal core | **libghostty-vt** (C ABI; `bindgen` from Rust) |
| PTY | `portable-pty` crate |
| Font shaping | freetype + harfbuzz + fontconfig (ghostty's stack), via Rust crates |
| Terminal renderer | Custom `QQuickItem` via **Qt Quick RHI** (Metal/Vulkan/GL/D3D11) |
| Qt bridge | **cxx-qt** (safe Rust↔Qt via CXX) |
| Shell | Qt6 + **QtQuick/QML** + **Kirigami** chrome |
| Async runtime | `tokio` (D-Bus server, git/PR polling, port scans, file watching) |
| Git | `gix` crate (direct `.git/` reads) + `reqwest` (GitHub PR polling) |
| Ports | `procfs` crate (`/proc/net/tcp` + `/proc/<pid>/fd` readlink) |
| Notifications | `KNotification` / `org.freedesktop.Notifications` D-Bus |
| Settings | `KConfig` + `KConfigXT` (app); JSON (project-scoped, agent-editable) |
| IPC | D-Bus on session bus; p2p escape hatch if event stream ever saturates |
| KDE Frameworks | KNotification, KConfig, KGlobalAccel, KWindowSystem, KIconLoader,
                   KColorScheme, KIO (phase 7), KWallet (optional) |

### 2.2 Why libghostty-vt (not alacritty_terminal, not embedded libghostty)

Ghostty ships **two** separate libraries. They have different Linux stories:

| Lib | What | Linux status |
|---|---|---|
| `libghostty-internal` (cmux uses) | Full embedded app: surfaces + renderer + `apprt/embedded` | macOS-coupled (OpenGL embedded path is a no-op; no DMA-BUF export; Metal-only inspector). |
| **`libghostty-vt`** | Terminal *core* as a portable C library: VT/OSC/SGR parsers, grid, scrollback, modes, reflow, **RenderState with dirty tracking**, key/mouse/focus encoders | **Clean, portable, with cross-platform C examples** at `ghostty/example/c-vt-*`. |

Tako uses **libghostty-vt**. The `example/c-vt-render` program is the embedding
contract: `ghostty_terminal_new` → `ghostty_terminal_vt_write` (feed PTY
bytes) → `ghostty_render_state_update` (snapshot) → check dirty (full/partial/
false) → iterate dirty rows/cells → draw.

What Tako (not libghostty-vt) owns: PTY, font shaping, window/surface
lifecycle, the renderer, the entire shell. All things you want to own anyway.

Caveat: `vt.h` is marked *"work-in-progress API, not yet stable"*. Pin a
ghostty commit, bind against it, bump deliberately.

### 2.3 Repo layout

```
tako/
├── crates/
│   ├── tako-term/      # libghostty-vt bindgen wrapper, PTY bridge, OSC dispatch
│   ├── tako-render/    # QQuickItem RHI terminal renderer (cxx-qt-exposed)
│   ├── tako-model/     # Window/Workspace/Split/Pane/Surface/Panel tree (Rust-owned)
│   ├── tako-bonsplit/  # Binary split tree (orientation + divider 0..1 + first/second)
│   ├── tako-dbus/      # D-Bus server + client (zbus), session-bus service
│   ├── tako-cli/       # `tako` binary (clap + serde_json) → D-Bus
│   ├── tako-git/       # gix-based branch/dirty/index + inotify; reqwest PR polling
│   ├── tako-net/       # procfs port scanner + per-workspace attribution
│   ├── tako-notify/    # OSC ingest + notification store + KNotification bridge
│   ├── tako-hooks/     # agent hook installers + hook-session store + state machine
│   ├── tako-session/   # serde snapshot to ~/.local/state/tako/ (XDG paths)
│   ├── tako-config/    # KConfig bridge + ghostty config reader + project JSON
│   └── tako-app/       # cxx-qt bridge: registers Rust model to QML, main entry
├── qml/                # Sidebar, tabs, splits, notification panel, settings UI
├── kcfg/               # takorc.kcfg schema + .kcfgc codegen
├── data/               # .desktop, metainfo, icons, D-Bus service file
└── src/main.rs         # loads QML, starts D-Bus server, drives the model
```

---

## 3. Data model

Direct port of cmux's tree (`cmux/Sources/Workspace.swift:2222`,
`cmux/Packages/macOS/CmuxPanes/`). The Rust side owns all mutable state; cxx-qt
exposes **immutable snapshots + action closures** to QML. This bakes in cmux's
"snapshot-boundary" rule (`cmux/AGENTS.md` "Pitfalls") by construction: no
QML view below a list boundary holds a reference to a store, so an orthogonal
state change can never invalidate every row.

```rust
struct Window   { id: WindowId, frame: Rect, workspaces: Vec<WorkspaceId>, selected: usize }
struct Workspace {                       // == "Tab" in cmux's vocabulary
    id: WorkspaceId,                     // volatile across restore (matches cmux)
    title: Option<String>,
    custom_title_source: TitleSource,    // User | Auto  — User is never overwritten
    main_split: SplitTree,               // binary tree of Panes
    dock: Option<DockSplitStore>,        // right-sidebar split tree (lazy)
    sidebar: SidebarMetadata,            // status pills, log, progress, latest notif
    env: HashMap<String, String>,        // workspace env injected into every shell
    remote: Option<RemoteConfig>,
}
enum SplitNode {
    Pane(Pane),
    Split { axis: Axis, divider: f32, a: Box<SplitNode>, b: Box<SplitNode> },
}
struct Pane     { id: PaneId, surfaces: Vec<SurfaceId>, selected: usize }
struct Surface  {                       // == "tab" inside a pane; the durable identity
    id: SurfaceId,                      // STABLE across restore — what agents bind to
    panel: Panel,                       // 1:1 with surface
}
enum Panel {
    Terminal(TerminalPanel),
    Browser(..),       // phase 8
    Markdown(..),
    FilePreview(..),
    AgentSession(..),
}
struct TerminalPanel {
    terminal: ghostty_vt::Terminal,
    render_state: ghostty_vt::RenderState,
    pty: portable_pty::PtyPair,
    cwd: PathBuf,
    title: Option<String>,
    shell_activity: ShellActivity,      // Idle | Prompt | CommandRunning
    agent_state: Option<AgentState>,    // from hooks; drives the status pill
}
```

**Stable surface ids are load-bearing.** Agent hooks bind to them via the
`TAKO_SURFACE_ID` env var; on restore, surface ids are rehydrated verbatim
(matching `cmux/docs/agent-session-tracking-spec.md:115-124`). Workspace ids
are regenerated on restore.

### Glossary (cmux's overloaded terms, made precise)

| Term | Type | Meaning |
|---|---|---|
| Window | `Window` | A Qt window. Owns the chrome + workspaces. |
| Workspace | `Workspace` | A row in the sidebar. Owns one main split tree. (cmux alias: "Tab".) |
| Pane | `Pane` | A tiled leaf in the split tree. Holds a stack of surfaces. |
| Surface | `Surface` | One selectable "tab" inside a pane. The durable identity. |
| Panel | `Panel` | The content object behind a surface (terminal, browser, …). 1:1 with surface. |
| Split | internal `SplitNode` | A binary split (axis + divider) in the tree. |
| Dock | `DockSplitStore` | A separate right-sidebar split tree. |

---

## 4. Terminal subsystem

### 4.1 Render loop

Per terminal surface:

1. PTY bytes arrive on a tokio task → `ghostty_terminal_vt_write`.
2. Wake the render thread → `ghostty_render_state_update(render_state, terminal)`.
3. Read `GHOSTTY_RENDER_STATE_DATA_DIRTY` → `False` (skip frame) / `Partial`
   (redraw dirty rows) / `Full` (redraw all).
4. On the RHI render pass: glyph atlas (built once per font/dpi, updated when
   new glyphs appear), per-row cell quads from the dirty iterator, cursor quad
   per `GHOSTTY_RENDER_STATE_CURSOR_*`, background per `render_state_colors`.
5. Reset dirty bits.

RHI is the abstraction layer that libghostty lacks — it picks Metal/Vulkan/GL
automatically per platform. On Plasma 6/Wayland that's usually Vulkan via
GraphicsApp; on X11 usually GL. Either way Tako's code is the same.

### 4.2 Input

- **Keyboard**: xkbcommon (Qt already links it) → `ghostty_input_key_s`
  equivalent. Keysym → ghostty key mapping table built once. Modifiers use
  native scan codes to disambiguate left/right Ctrl/Alt/Shift/Super.
- **Mouse**: Qt mouse events → libghostty-vt mouse encoder. Fractional
  coordinates in points (libghostty multiplies by content scale internally).
- **Selection / clipboard**: PRIMARY (X11 middle-click) + CLIPBOARD.
  libghostty-vt's paste-safety utilities gate large/unsafe pastes.
- **IME**: `QInputMethodEvent` → libghostty-vt preedit/commit calls. Cursor
  rect from the render state positions the candidate window.

### 4.3 OSC dispatch

libghostty-vt's `vt/osc.h` parser emits typed commands. Tako routes them:

| OSC | Command type | Tako consumer |
|---|---|---|
| 0 / 1 / 2 | change window title | surface title → tab bar + sidebar |
| 7 | current working directory (file://host/path) | `Panel.cwd` → git probe, sidebar |
| 9 / 99 / 777 | notification (iTerm / urxvt variants) | `tako-notify` ingest |
| 133 | FinalTerm prompt marks | shell activity (Idle/Prompt/CommandRunning) |
| 8 | hyperlinks | rendered inline (handled by glyph pass) |

Plus the explicit shell-integration path (installed by `tako`): zsh/bash/fish
hooks call into the control API for `report_pwd`, `report_tty`, `ports_kick`
(attributing cwd/tty/ports to surfaces — see cmux's
`Resources/shell-integration/cmux-zsh-integration.zsh`).

---

## 5. AI agent integration

This is the core differentiator. The signal path mirrors cmux's, which is
well-specified at `cmux/docs/agent-session-tracking-spec.md`.

### 5.1 Five detection mechanisms (in priority order)

1. **Hooks (primary, authoritative).** `tako hooks setup <agent>` writes
   per-agent hook config files:
   - `~/.codex/hooks.json`
   - `~/.grok/hooks/cmux-session.json`
   - `~/.config/opencode/plugins/cmux-session.js`
   - `~/.cursor/hooks.json`
   - `~/.gemini/settings.json`
   - `~/.kiro/agents/cmux.json`
   - `~/.rovodev/config.yml`
   - `~/.pi/agent/extensions/cmux-session.ts`
   - Claude Code is handled by a PATH-shim wrapper (next) rather than this list.

   Each hook calls `tako hooks <agent> <event>` with a JSON payload on stdin.
   Events: `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`,
   `Stop`, `Notification`, `PermissionRequest`, `AskUserQuestion`,
   `ExitPlanMode`, `SessionEnd`.

2. **Surface binding.** Resolve which surface an event belongs to, in order:
   (a) explicit `--surface` flag; (b) inherited `TAKO_SURFACE_ID` /
   `TAKO_WORKSPACE_ID` env; (c) tty → surface (the terminal's PTY slave name
   → surface id, via a `debug.terminals`-equivalent table); (d) process-tree
   walk (agent pid under a surface's `top_level_pids`).
   Record the binding in `~/.local/state/tako/<agent>-hook-sessions.json`.

3. **PATH-shim wrappers (Claude, Codex).** Tako prepends a shim dir to `PATH`
   when spawning shells. Typing `claude`/`codex` hits the shim, which emits
   the session-start signal (carrying `TAKO_SURFACE_ID`, cwd, child pid)
   before exec'ing the real binary. Detection without agent cooperation.

4. **Process detection (Task Manager view).** procfs walk: read
   `/proc/<pid>/cmdline`, match against per-agent basename/argument needles
   (cmux's registry at `cmux/Sources/TaskManagerTypes.swift:539-749` is a
   complete reference). Use this for the `tako top` view and the agent-status
   textbox UI; do *not* use it as authoritative session binding.

5. **Transcript enumeration (resume list).** For the Sessions sidebar panel,
   scan each agent's session store:
   - Claude `~/.claude/` (JSONL transcripts)
   - Codex rollout DB (SQLite)
   - Grok `~/.grok/sessions/<encoded-cwd>/chat_history.jsonl`
   - OpenCode, Pi, RovoDev, Antigravity, etc.
   - ripgrep prefilter when a needle is supplied, then JSONL metadata extract.

### 5.2 Agent state machine

Driven by hook events. One state per `(surface_id, session_id)`:

```
sessionStart          → Idle
userPromptSubmit      → Working
preToolUse            → Working
postToolUse           → Working
todoWrite             → Working
permissionRequest     → NeedsInput
askUserQuestion       → NeedsInput
exitPlanMode          → NeedsInput
notification          → NeedsInput
stop                  → Idle
sessionEnd            → Ended
```

The status pill in the sidebar reflects this. `NeedsInput` triggers a
notification (pane ring, unread badge, Plasma notification).

### 5.3 Auto-naming

Optional, off by default. On an agent's turn-end hook:

1. Probe the setting (`workspace.set_auto_title { probe: true }` D-Bus call).
   Abort if disabled or if the workspace has a user-owned title.
2. Read the agent's transcript JSONL.
3. Throttle: skip if `< 12` lines, in-flight marker not expired (60s + 15s
   grace), `< 180s` since last attempt, `< 6` lines since last naming. First
   naming always qualifies.
4. Spawn a **no-tools / isolated** summarizer:
   - `claude -p` (haiku or `ANTHROPIC_SMALL_FAST_MODEL`)
   - `codex exec --output-last-message`
   - `grok --prompt-file` (tools + web disabled)
   - `opencode run --pure`
   - `pi --print --no-tools`
5. Sanitize to ≤ 50 chars. Apply via D-Bus, which enforces the user-provenance
   rule app-side.

User-set titles (`custom_title_source = User`) are never overwritten.

### 5.4 Resume

Surface ids are stable across restore, so when an agent restarts in the same
surface, Tako re-runs the agent's resume command (e.g. `claude --resume <id>`,
`codex resume <id>`, `grok -r <id>`). The hook-session store carries
`sessionId`, `cwd`, `transcriptPath`, `pid`, `launchCommand`,
`agentLifecycle`. **Settings > Terminal > Resume Agent Sessions on Reopen**
controls this; opt-out per surface via `tako surface resume set/clear`.

### 5.5 Feed (later)

cmux's Feed is a two-way inline approval surface (Approve/Deny buttons that
block the agent). The notification panel covers the one-way "agent needs
input" case. Defer Feed until you want inline approval cards.

---

## 6. Notifications

Pipeline:

```
OSC 9/99/777 (in-band)  ┐
agent hook "Notification" event ─┤
`tako notify` CLI call ─┤
                        ├─→ notification store ─→ pane ring (QML)
                        │                     ├─→ sidebar unread badge + latest text
                        │                     ├─→ notification panel
                        │                     └─→ Plasma notification (KNotification)
```

Notification fields: `id`, `workspace_id`, `surface_id`, `title`, `subtitle`,
`body`, `created_at`, `read_at`. The store is the source of truth for the
sidebar's per-row "latest notification text" projection.

Delivery gates (from `cmux/docs/notifications.md`): by default a banner is
auto-withdrawn when its workspace becomes visible. Optional
`suppressOnlyFocusedSurface` mode auto-withdraws only the exact focused
surface, so non-focused surfaces in a visible workspace keep their banners.

Notifications fire a desktop notification (KNotification wraps
`org.freedesktop.Notifications` on the session bus). Jumping to unread uses
**`xdg-activation-v1`** on Wayland (or `_NET_ACTIVE_WINDOW` on X11) to focus
the target window — this is the bit that's easy to miss; without an activation
token, "jump to unread" silently fails to steal focus on Wayland.

---

## 7. Sidebar metadata pipeline

Per workspace row, the sidebar shows: cwd, git branch + dirty dot, listening
ports, latest notification text. Plus an optional PR badge.

### 7.1 Git (basic)

Direct `.git/` reads via `gix` (no `git` subprocess). Mirrors
`cmux/Packages/macOS/CmuxGit/`:

- **Repository resolution**: walk upward from cwd; handle `.git` dir, `.git`
  *file* (worktree/submodule pointer), `commondir`.
- **Branch**: parse `.git/HEAD` for `ref: refs/heads/<name>`.
- **Dirty**: parse the git index (DIRC magic; v2/v3/v4 incl. v4 path
  compression), `lstat` each tracked entry, compare size/mode/mtime — git's
  own stat check. Content signature (FNV-1a over path/mode/OID) rebaselines a
  clean tree across index rewrites.
- **GitHub slug**: parse `config` remotes (no `git remote -v` subprocess).

Refresh: `notify` crate (inotify on Linux) over `.git/HEAD`, `.git/index`,
packed-refs, every reachable `config`, plus the worktree root and submodule
gitlinks. 5-minute fallback poll. Initial retry offsets `[0, 0.5, 1.5, 3, 6,
10]s` for startup races.

### 7.2 PR status

Pure HTTPS polling `api.github.com/repos/<slug>/pulls`. Mirror cmux's cadence:
10s selected panel / 60s background ±10% jitter, 15-min terminal-state sweep,
14-day stale for merged PRs. Default branches (main/master) skipped. Auth via
`GH_TOKEN`/`GITHUB_TOKEN` env, else `gh auth token` shelled out once per
batch. Per-repo 15s cache. Index by head branch, "preferred" = open > merged >
closed, then most-recently-updated, then highest number.

### 7.3 Listening ports

`procfs` crate on Linux — *cleaner* than cmux's `lsof`/`ps` shelling:

- Read `/proc/net/tcp` + `/proc/net/tcp6` for LISTEN sockets.
- For each, find owning pid via `/proc/<pid>/fd` readlink → `socket:[inode]`.
- Attribute to workspace two ways:
  - **TTY-bound**: terminal's PTY slave → pid → ports.
  - **Agent process tree**: tracked agent root pids → BFS over
    `/proc/<pid>/stat` (ppid) → ports. 2-second rescan timer.

Coalesce shell `ports_kick` signals (200ms), then a burst of 6 scans at
`[0.5, 1.5, 3, 5, 7.5, 10]s` (cmux's pattern).

### 7.4 Working directory

OSC 7 (parsed by libghostty-vt) is the primary source — validate hostname is
local. Backup: shell-integration hook calls the control API on `precmd`/`chpwd`.

---

## 8. Session persistence

Mirror cmux's schema (`cmux/Sources/SessionPersistence.swift`), XDG paths:

- **Snapshot**: `~/.local/state/tako/session.json` (atomic rename).
- **Event log**: `~/.local/state/tako/events.jsonl` (16 MiB rotation, one
  archive at `.1`).
- **Hook sessions**: `~/.local/state/tako/<agent>-hook-sessions.json`.
- **Agent transcripts** (for vault, much later): out of scope.

Snapshot shape (abbreviated):

```
AppSessionSnapshot
├─ version
├─ created_at
└─ windows[]
   ├─ frame, display
   └─ workspaces[]
      ├─ workspace_id (volatile), title, custom_title_source,
      │  description, color, is_pinned, group_id, env
      ├─ layout: SplitNode (recursive pane/split with axis + divider)
      ├─ panels[]
      │  ├─ id (= surface id, STABLE), type, title, cwd, git_branch, ports, tty
      │  └─ oneOf: terminal { scrollback } | browser { url, history } | …
      ├─ status_entries / log_entries / progress
      └─ notifications[]
```

Policy (mirrors cmux): autosave 8s, max 12 windows, 128 workspaces/window,
512 panels/workspace, ~400 KB scrollback per terminal. Strip OSC color
sequences from saved scrollback so they can't override the live theme on
restore.

---

## 9. Control interface (D-Bus)

Service: `org.tako.Control` on the session bus. Object path
`/org/tako/Control`. Auth via D-Bus UID matching + polkit for sensitive
methods (`surface.send_text`, `surface.send_key`, `surface.exec`,
`workspace.close`, `notification.create_for_surface`).

### Methods (mirror cmux's v2 catalog at `cmux/docs/cli-contract.md`)

| Group | Methods |
|---|---|
| Windows | `window.list`, `window.current`, `window.create`, `window.focus`, `window.close`, `workspace.move_to_window` |
| Workspaces | `workspace.list`, `workspace.create`, `workspace.select`, `workspace.current`, `workspace.close`, `workspace.rename`, `workspace.reorder`, `workspace.reorder_many`, `workspace.set_auto_title`, `workspace.group.*` |
| Surfaces | `surface.list`, `surface.focus`, `surface.split`, `surface.create`, `surface.close`, `surface.move`, `surface.drag_to_split`, `surface.refresh`, `surface.health`, `surface.send_text`, `surface.send_key`, `surface.trigger_flash`, `surface.read_screen` |
| Panes | `pane.list`, `pane.focus`, `pane.surfaces`, `pane.create` |
| Input | `surface.send_text`, `surface.send_key` |
| Notifications | `notification.create`, `notification.create_for_surface`, `notification.list`, `notification.clear`, `notification.dismiss`, `notification.mark_read`, `notification.open`, `notification.jump_to_unread` |
| Sidebar | `sidebar.status.set`, `sidebar.status.clear`, `sidebar.progress.set`, `sidebar.progress.clear`, `sidebar.log.append`, `sidebar.log.clear`, `sidebar.snapshot` |
| Browser (phase 8) | `browser.open_split`, `browser.navigate`, `browser.back/forward/reload`, `browser.url.get`, `browser.snapshot`, `browser.eval`, `browser.click/type/fill/press`, `browser.screenshot` |
| Events | `events.subscribe(after_seq, filters) → subscription_id` |
| System | `ping`, `capabilities`, `identify`, `reload_config` |

### Signals

| Signal | Payload |
|---|---|
| `EventEmitted` | `(subscription_id: u64, seq: u64, boot_id: s, occurred_at: s, name: s, category: s, source: s, workspace_id: s, surface_id: s, payload_json: s)` |
| `Heartbeat` | `(subscription_id: u64, latest_seq: u64, occurred_at: s)` — every 15s |
| `SubscriptionDropped` | `(subscription_id: u64, reason: s, last_seq: u64)` — slow consumer or shutdown |

### Event stream contract

- Monotonic `seq` per process; `boot_id` (UUID) changes on restart.
- In-memory replay ring: 4096 events. Event frame cap: 16 KiB; oversized
  payloads replaced with `{ payload_truncated: true }`.
- Per-subscriber pending queue: 1024 events; over → `SubscriptionDropped`
  with `reason="slow_consumer"`.
- Client loop: `events.subscribe(last_seq)` → process events → persist `seq`
  after each side effect succeeds → on `SubscriptionDropped` reconnect with
  last persisted `seq`. If `ack.resume.gap` is true (cursor too old or past
  current boot), refresh state via `snapshot`-style methods (`workspace.list`,
  `notification.list`, `tree`, `sidebar.snapshot`).
- Durably logged to `~/.local/state/tako/events.jsonl`.

### Escape hatch

If the session bus ever saturates under a heavy event stream (unlikely), use
**p2p D-Bus** (`dbus_server_listen("unix:abstract=tako")`) for the event
stream only. Same wire protocol, same client code; bypasses `dbus-daemon`.
Don't pre-build it.

---

## 10. Settings

- **App settings**: `KConfig` + `KConfigXT`. Schema in `kcfg/takorc.kcfg`;
  generated `Tako::Settings` class exposed to Rust via cxx-qt. Cascading
  defaults, change notifications, integrates with KDE System Settings if you
  ship a KCM. Stored at `~/.config/takorc`.
- **Project-scoped settings**: JSON at `.tako/tako.json` (and parent
  directories, merged). Agent-editable, portable, KDE-independent. Defines
  custom commands, workspace templates, notification hooks, env. Mirrors
  cmux's `.cmux/cmux.json`.
- **Ghostty compat**: read `~/.config/ghostty/config` for theme/font/colors/
  cursor. Tako writes managed additions to `~/.config/tako/config.ghostty`,
  never to the user's ghostty config.

---

## 11. KDE integration specifics

| Concern | Mechanism |
|---|---|
| Desktop notifications | `KNotification` (wraps `org.freedesktop.Notifications`) |
| Window urgency (taskbar) | `KWindowSystem::demandAttention` / `_NET_WM_STATE_DEMANDS_ATTENTION` (X11); `xdg_toplevel.set urgents` (Wayland) |
| Focus stealing ("jump to unread") | **`xdg-activation-v1` token** (Wayland); `_NET_ACTIVE_WINDOW` (X11) |
| Taskbar progress | Unity launcher API / KDE Taskbar progress via `KJob` |
| System-wide shortcuts | `KGlobalAccel` |
| Theme / icons / colors | `KColorScheme`, `KIconLoader`, `Kirigami.Theme` |
| Window blur | KWin `_KDE_NET_WM_BLUR_BEHIND_REGION` |
| File operations (phase 7) | `KIO` |
| Wallet (optional) | `KWallet` for any stored credentials |
| Shell integration | `.desktop` file, metainfo XML, D-Bus service file, MIME handler for `tako://` URLs |

---

## 12. Phased roadmap

Time estimates assume one strong Rust+Qt engineer full-time. Each phase ends
in something dogfoodable.

### Phase 0 — Render spike & stack proof *(~1–2 weeks, GO/NO-GO)*

- cxx-qt hello world: one QML window calling into Rust.
- `bindgen` on `libghostty-vt/include/ghostty/vt.h`; link `libghostty-vt.a`.
- Embed a Terminal, drive a PTY, snapshot RenderState, render dirty rows in a
  `QQuickItem` via Qt RHI. Glyph atlas: one freetype+harfbuzz pass per font.
- Measure type-to-pixel latency. Target < 16 ms, no dropped frames under
  `yes` / `cat big.log`.
- Fallback ladder (only if needed): raw GL via `QQuickFramebufferObject` →
  `rustybuzz`+`ab_glyph` if freetype/harfbuzz binding is painful →
  `alacritty_terminal` as a last-resort terminal core if libghostty-vt ABI
  churn is unmanageable.
- **Gate:** latency acceptable → continue. Not → re-scope before further
  investment.

### Phase 1 — Working native terminal *(~3–5 weeks)*

- One window, one workspace, one terminal surface. Spawn shell, read config.
- Input: keyboard (xkbcommon keysym mapping table), mouse, selection,
  clipboard (PRIMARY + CLIPBOARD), IME, resize, DPI/scale.
- OSC 7 (cwd), OSC 0/2 (title) wired.
- Shell-integration script (zsh/bash/fish) installed by Tako.
- **Deliverable:** a usable native terminal. Dogfood daily.

### Phase 2 — Sidebar, tabs, splits, workspaces *(~3–4 weeks)*

- Model layer (§3) in Rust; cxx-qt exposes immutable snapshots to QML.
- Vertical sidebar (workspace list) with cwd + title.
- Horizontal tab bar in panes; multiple surfaces per pane.
- `tako-bonsplit`: binary split tree in Rust — split right/down, focus
  directional, resize (keyboard + drag), equalize.
- Workspace create/select/close/rename; session persistence v1 (layout +
  cwd + scrollback, no agents yet).
- **Deliverable:** looks like cmux's shell, minus agent/notification chrome.

### Phase 3 — Notifications + AI agent integration *(~3–4 weeks)*

- Notification pipeline: pane rings (QML), sidebar unread badge + latest
  text, notification panel, Plasma notification. OSC 9/99/777 ingest via
  libghostty-vt.
- Minimal `tako notify` CLI (talks in-process initially; full D-Bus in
  Phase 5).
- `tako-hooks`: `tako hooks setup/uninstall`, per-agent installers (start
  with Claude, Codex, OpenCode, Grok). Hook-session store at
  `~/.local/state/tako/<agent>-hook-sessions.json`.
- Surface binding (§5.1 mechanism 2); agent state machine (§5.2); sidebar
  status pill.
- Auto-naming pipeline (§5.3).
- Agent resume: per-agent transcript scanning for the Sessions panel +
  surface-id-stable resume on restart (§5.4).
- **Deliverable:** run Claude Code in a pane; sidebar shows its state;
  jump-to-unread works; restarts restore its session.

### Phase 4 — Basic git + ports *(~1–2 weeks)*

- `tako-git`: `gix`-based branch + dirty detection. `notify` (inotify)
  watcher over `.git` paths + 5-min fallback poll.
- Sidebar shows: branch + dirty dot, cwd, latest notification text.
- PR status: `reqwest` polling GitHub API, same cadence as cmux.
- `tako-net`: `procfs`-based port scanner attributed via TTY and agent
  process tree.
- **Deliverable:** sidebar matches cmux's per-row metadata.

### Phase 5 — D-Bus control interface + CLI *(~3–4 weeks)*

- `tako-dbus`: `org.tako.Control` on session bus, polkit policies for
  sensitive methods, full method catalog (§9).
- Event stream with the resume contract verbatim (§9 "Event stream
  contract").
- `tako-cli`: clap-based `tako` binary mirroring `cmux/docs/cli-contract.md`
  (workspace/surface/pane verbs, `notify`, `send`, `send-key`, `list-*`,
  `tree`, `top`, `set-status`, hooks subcommands, themes, etc.).
- **Deliverable:** fully scriptable; agents can self-drive via D-Bus.

### Phase 6 — KDE polish pass *(~2–3 weeks, woven throughout)*

- `KGlobalAccel`, `KWindowSystem` urgency + taskbar progress,
  `KColorScheme`/Plasma theme/icons, `.desktop` + metainfo, KWin blur,
  **Wayland `xdg-activation`** for focus-stealing, `KIO` for file ops.
- Settings UI (KConfig-backed KCM, optional System Settings integration).
- Flatpak/Snap consideration; distro packaging.

### Phase 7 — Deeper git *(~3–4 weeks)*

- File explorer: Qt outline view + per-path git status via `gix` porcelain.
  `inotify` watch on the worktree.
- Diff viewer: native Qt diff widget (lighter) or Qt WebEngine view reusing
  cmux's React diff webview (heavier, more featureful). Per-repo comment
  store. Agent-baseline resolver.

### Phase 8 — Embedded browser *(~3–5 weeks)*

- Qt WebEngine pane. Scriptable via D-Bus (agent-browser-port-spec). Cookie
  / profile import from Chrome/Firefox/etc. **Optional / plugin** so the base
  terminal stays lean.

### Phase 9 — Remote/SSH, then mobile/web *(much later)*

- `tako ssh`: reuse `cmuxd-remote` (Go, already linux/amd64+arm64) with
  rebranded env vars, or reimplement persistent-PTY + reverse-relay in Rust.
- Mobile/web: thin clients over the Phase 5 control interface + event stream.

**Realistic path to dogfoodable v1:** ~12–16 weeks (end of Phase 5).

---

## 13. Risk register

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| libghostty-vt ABI churn (it's marked "not yet stable") | Medium | Medium | Pin a commit; bind against it; bump deliberately. Phase 0 spike confirms fitness. |
| RHI terminal renderer latency | Medium | Project-killing | Phase 0 spike; predefined fallback ladder. |
| Snapshot-boundary / typing-latency regressions (the bug class that pegged cmux's CPU at 100%, `cmux/AGENTS.md` "Pitfalls") | High if ignored | High | Rust model + immutable snapshots to QML by construction; never mutate from view bodies; benchmark keystroke path from day one. |
| Wayland focus-stealing broken (jump-to-unread silently no-ops) | High if missed | Medium | Use `xdg-activation-v1` tokens from Phase 6; don't assume X11. |
| Qt WebEngine memory cost erodes the "lean" pitch (Phase 8) | Medium | Medium | Browser pane optional/plugin. |
| Agent hook ecosystem churn | Medium | Low | Per-agent installer isolation; version the hook-session store schema. |
| GitHub API rate limits | Medium | Low | Authenticated reqs by default; 15s per-repo cache; respect `X-RateLimit`. |
| cxx-qt maturity on hot paths | Low | Medium | Keep terminal rendering on the Rust side of the bridge; cxx-qt carries model + actions, not per-frame work. |

---

## 14. Open decisions

| Decision | Default | Revisit when |
|---|---|---|
| Pin ghostty commit for libghostty-vt | latest stable at Phase 0 start | each Tako release |
| Project-scoped config location | `.tako/tako.json` (cmux compat inverted) | before v1 |
| Custom-sidebar interpreter (cmux's data-driven sidebar extension) | Defer | after v1; it's an additive extension point |
| PATH-shim wrappers for Claude/Codex | Yes, in Phase 3 | — |
| Feed (inline two-way approval cards) | Defer | when users want Approve/Deny in Tako itself |
| Browser pane bundling | Optional/plugin | Phase 8 |
| SSH work | Reuse cmuxd-remote vs Rust rewrite | Phase 9 |
| KCM in System Settings | Ship, don't ship | Phase 6 |

---

## 15. References

The cmux repo at `cmux/` is the authoritative product reference. Key sources
that informed this roadmap:

- `cmux/README.md` — product pitch, feature list.
- `cmux/AGENTS.md` — performance pitfalls (typing-latency-sensitive paths,
  snapshot-boundary rule, list subtree constraints).
- `cmux/docs/cli-contract.md` — full CLI command catalog.
- `cmux/docs/events.md` — event stream protocol (reused for Tako's event
  contract).
- `cmux/docs/v2-api-migration.md` — JSON-RPC method catalog.
- `cmux/docs/agent-session-tracking-spec.md` — agent binding state machine.
- `cmux/docs/notifications.md`, `cmux/docs/feed.md` — notification + approval
  pipelines.
- `cmux/docs/workspace-auto-naming.md` — auto-naming pipeline.
- `cmux/Sources/Workspace.swift`, `cmux/Sources/DockSplitStore.swift`,
  `cmux/Packages/macOS/CmuxPanes/` — data model.
- `cmux/Packages/macOS/CmuxGit/` — direct-`.git`-read pipeline.
- `cmux/Sources/PortScanner.swift` — port attribution (Tako uses procfs
  instead of lsof/ps).
- `cmux/ghostty/example/c-vt-render/src/main.c` — the libghostty-vt render
  loop skeleton Tako's renderer is modeled on.
- `cmux/ghostty/include/ghostty/vt.h` — the C API Tako binds.
- `cmux/daemon/remote/README.md`, `cmux/vault/README.md` — remote daemon +
  vault purpose (both deferred).
