# Tako

A native KDE terminal for AI coding agents. Built in Rust on Qt6/QML/Kirigami,
with libghostty-vt as the terminal core.

## Project map

```
tako/
├── crates/
│   ├── tako-app/       cxx-qt bridge + `tako` binary entry (QML <-> Rust)
│   └── tako-config/    startup terminal defaults parser (Tako/Ghostty-style config)
├── tako-terminal/      embeddable Qt Quick TerminalView package (C++ facade + Zig core)
├── kcfg/               (future) takorc.kcfg schema + .kcfgc codegen
├── data/               (future) .desktop, metainfo, icons, D-Bus service file
├── ROADMAP.md          the authoritative design document
└── cmux/               (gitignored) product reference checkout
```

Crates for later phases (`tako-model`, `tako-bonsplit`, `tako-dbus`, `tako-cli`,
`tako-git`, `tako-net`, `tako-notify`, `tako-hooks`, `tako-session`) are created
when their phase starts, not pre-scaffolded.

See ROADMAP.md for the full architecture, data model, and phased roadmap.

## Dev tool commands

The project uses [mise](https://mise.jdx.dev) to bootstrap the toolchain
([hk](https://hk.jdx.dev), [dprint](https://dprint.dev), rust, zig), recorded in
`mise.toml` / `mise.lock`.

- `mise install` — install pinned toolchain.
- `mise run check` — run `hk check --all` (dprint check + cargo clippy).
- `mise run fix` — run `hk fix --all` (dprint fmt + cargo clippy --fix).
- `timeout 5 mise run tako` — run `tako` with a 5-second timeout to
  automatically terminate the gui app.
- `cargo build` / `cargo test` — usual Rust workspace commands.

`tako-terminal` owns the app-facing libghostty-vt fetch/build/link path. Its
first build fetches the pinned ghostty tarball (~37 MB) and runs
`zig build -Demit-lib-vt` (several minutes) when the cached static library is
missing. The result is cached under `~/.cache/tako/ghostty-vt/<commit>/` so
later builds skip it. The same build script generates Rust bindgen bindings for
the test-only libghostty-vt wrappers; bindgen needs `libclang` (Fedora:
`clang-devel`, or the runtime `clang-libs` suffices).

Native system libraries (Qt6/KDE Frameworks, freetype, harfbuzz, fontconfig) are
expected from the host for now; pixi/conda-forge packaging is deferred.
`tako-terminal/build.rs` uses pkg-config to link system **freetype2** and
**harfbuzz** for the Zig core. Zig owns font loading, shaping, rasterization,
cell metrics, atlas packing, and render-frame planning in production.

## Project invariants

<!-- List non-negotiable rules for the project as they emerge. -->

- **`unsafe_code` is denied workspace-wide.** The permitted exceptions are: a
  cxx-qt `#[cxx_qt::bridge]` module (`unsafe extern` blocks, edition 2024 FFI
  syntax); `crates/tako-app/build.rs` and `tako-terminal/build.rs` (an `unsafe`
  `cc_builder` closure — cxx-qt-build 0.9's only flag-passing API); the
  test-only libghostty-vt wrappers in
  `tako-terminal/src/{terminal,effects,gesture,grid_ref,input,modes,point,selection,snapshot}.rs`;
  the production libghostty-vt row walker, PTY/session core, FreeType/HarfBuzz
  font service, glyph atlas, and renderer planning in
  `tako-terminal/src/core.zig`. Scope every relaxation with a module-level
  `#![allow(unsafe_code)]` — never relax at workspace level.
- **The terminal backend is single-threaded by construction.** Zig
  `TerminalSession` owns the live `GhosttyTerminal` plus PTY/session state on
  the GUI thread; effects fire synchronously inside `ghostty_terminal_vt_write`
  on that same thread and append to session-local buffers. The Rust
  `Terminal`/effects wrappers are test scaffolding only. Don't add `Send` bounds
  or thread-spawning here without reworking the ownership model.
- **The terminal implementation object is GUI-thread-owned; `createRenderer` is
  not.** `QQuickFramebufferObject::createRenderer()` runs on the QSG render
  thread. The implementation object (`TerminalSession` in Zig) and its
  `QSocketNotifier` must be created on the GUI thread — currently in
  `TakoTerminalView::componentComplete()` after QML properties are applied —
  never in `createRenderer`, or the notifier would parent cross-thread _and_ the
  GUI-thread timer would touch single-threaded terminal state from the wrong
  thread. `createRenderer` only spawns the render-thread `TakoTerminalRenderer`,
  which reads `view->plan()` during `synchronize()` (called with the GUI thread
  blocked — the only safe render-thread→item touch).
- **`tako-terminal` is the embeddable component boundary.** Public API belongs
  on the Qt/QML facade in `tako-terminal/src/tako_terminal_view.*`: properties,
  signals, invokables, lifecycle, focus/input extraction, clipboard, DPR/window
  hooks, QML registration, and render-thread Qt glue. The private implementation
  ABI is `tako-terminal/src/tako_terminal_core.h`, implemented by
  `tako-terminal/src/core.zig`. Zig owns libghostty-vt terminal creation/free,
  effects callbacks, PTY/session lifecycle, shell bootstrap,
  keyboard/focus/paste/mouse-event encoding, scrollback viewport navigation,
  font-family/default resolution to concrete font paths, font-size reload
  requests, direct libghostty-vt mode/data queries including OSC
  title/current-directory readout, keyboard selection adjustment,
  select-all/semantic selection derives, selection formatting/copy-out, the
  libghostty-vt render-state handle, render-state row/cell walking, live default
  fg/bg/cursor colors, default cursor style/blink, and terminal-derived flat
  geometry for cell backgrounds, text decorations, and cursor, plus final
  text-glyph foreground/visibility resolution, font shaping/rasterization,
  shaped glyph buffers, cell metrics, terminal/preedit text traversal, glyph
  atlas packing, preedit underline/cursor geometry, final `FramePlan` metadata,
  and public vertex-buffer assembly. Rendering is owned by the C++
  `TakoTerminalRenderer` on Qt's render thread. C++ must not call implementation
  font/snapshot helpers directly. `tako-app` depends on `tako-terminal`, calls
  `tako_terminal::register_qml_types()`, imports `org.tako.terminal`, and keeps
  app state/orchestration in Rust+cxx-qt. Do not make the app or QML reach into
  backend internals.
- **Terminal theme/cursor defaults go through libghostty-vt.** The Qt facade
  exposes default `foregroundColor`, `backgroundColor`, `cursorColor`,
  full-table `colorPalette`, `cursorStyle`, and `cursorBlink`; Zig applies them
  with `ghostty_terminal_set()` and requests a replan. Do not special-case these
  in the renderer: they are terminal defaults, and OSC overrides remain owned by
  libghostty-vt. Palette config is all-or-reset, matching libghostty-vt's
  256-entry default palette API. `scrollbackLimit` is creation-time config and
  only applies to new/restarted sessions.
- **Config is app/model-owned, not terminal-owned.** `tako-config` parses
  startup terminal defaults from `TAKO_CONFIG`, then Ghostty and Tako config
  files under XDG config home. `tako-app` passes those values to QML as initial
  root properties bound to `TerminalView`. Keep `TerminalView` embeddable and
  property-driven; do not make `tako-terminal` read user dotfiles directly.
- **Shell integration is spawn-time bootstrap, not parser logic.**
  `TerminalView` exposes creation-time `shellIntegration` (default true). The
  Zig implementation core creates per-session temporary startup files for bash/
  zsh/fish that emit OSC 133 markers. For zsh, the temp `.zshenv` restores the
  user's real `ZDOTDIR` before normal `.zshrc` loading so plugin managers see
  the expected config directory; Tako's prompt wrapper runs from `precmd` rather
  than rewriting the prompt before themes load. libghostty-vt owns OSC 133
  parsing and semantic selection; do not parse semantic prompt sequences in
  Qt/Zig/Rust host code. Keep the temp-file guard alive for the child session
  and remove it when the PTY session is destroyed.
- **`TerminalView` owns session lifecycle policy.** `autoStart` defaults to true
  to preserve demo behavior; embedders can set it false and call `start()`
  explicitly after binding properties. `running` means a session exists and has
  not exited. `stop()` destroys the session and clears host-visible
  title/cwd/hover/scrollbar state without emitting `exited()`; natural PTY exit
  still emits `exited()`. `restart()` is `stop()` followed by `start()`.
- **Font shaping/rasterization is Zig-owned.** Zig owns the live
  `GhosttyTerminal`, `GhosttyRenderState`, FreeType/HarfBuzz font loading and
  reload, cell metrics, shaped glyph buffers, rasterized glyph bitmaps, glyph
  atlas packing, libghostty render-state row/cell walking, live grid dimensions,
  and mouse/selection hit-testing geometry. Zig also owns presentation/idle-skip
  state such as focus, cursor blink phase, IME preedit text, `needs_replan`, and
  the previous presented cursor snapshot. Zig updates its owned
  `GhosttyRenderState`, reads the dirty bit and cursor fields directly, walks
  rows/cells into `TakoTerminalFrameSnapshot`, applies focus/blink visibility
  policy, compares that presented cursor against session state, stamps each
  snapshot cell with final `text_visible` and `text_fg`, shapes UTF-8 text runs,
  rasterizes missing glyph IDs, emits all terminal/preedit/cursor quads,
  finalizes `FramePlan` metadata, records `TerminalSession.last_plan`, and
  clears the Zig-owned render state's global dirty flag. The Rust `terminal.rs`
  wrapper and sibling libghostty helpers are compiled only for tests; production
  must not add new uses of them.
- **Text decoration rendering is view-only and Zig-owned.**
  `TakoTerminalFrameSnapshot` style data comes from libghostty-vt render state;
  Zig turns underline/strikethrough/overline into flat-color quads using the
  effective foreground after inverse/selection/faint resolution, and treats
  invisible/conceal as foreground suppression while preserving backgrounds. Keep
  those as renderer primitives, not terminal state. Bold/italic still require
  real font-face selection; don't fake them by smearing glyphs unless the
  project deliberately accepts that tradeoff.
- **Qt is discovered via `qmake` on PATH.** `mise.toml` sets
  `QT_VERSION_MAJOR=6` so cxx-qt-build picks the Qt6 `qmake6`. Keep that env var
  when adding any Qt-linking crate.
- **The ghostty pin must carry the full libghostty-vt C API** (`render.h`,
  `terminal.h`, `build_info.h`, static-lib build). The latest stable tag
  (v1.3.1) lacks these — they landed upstream on `main` after v1.3.1. Tako pins
  an upstream `main` commit in `tako-terminal/build.rs`; bump it deliberately
  and re-verify the C, Zig import, and bindgen surfaces.
- **cxx-qt-build does not register hand-written C++ `QML_ELEMENT` classes.** Its
  compiled `org.tako` QML module only registers types generated from the cxx-qt
  bridge (`#[qml_element]`). A C++ `QQuickItem` subclass added via
  `CxxQtBuilder::cpp_file` must register itself imperatively with
  `qmlRegisterType<T>("org.tako.<sub>", ...)` under a **separate** URI (the
  compiled module's qmldir takes precedence over same-URI imperative
  registrations). The registration C ABI lives in the C++ file; the safe Rust
  wrapper is `tako_terminal::register_qml_types()`, called from
  `tako_app::run()` before `engine.load`.
- **cxx-qt is for Rust-owned app models, not the terminal component facade.** As
  of 2026-07, cxx-qt can generate QObject subclasses and selected virtual
  overrides, but it does not remove the need for a C++ subclass of the
  non-QObject `QQuickFramebufferObject::Renderer`, nor does `cxx-qt-lib` expose
  the full Qt Quick/input/clipboard/GL API surface `TakoTerminalView` needs.
  Keep `TakoTerminalView` as Qt-side component glue and keep `tako-app`'s Rust
  bridge focused on model snapshots/actions. Do not introduce `qmetaobject`
  alongside cxx-qt unless a dedicated spike proves it solves a specific problem
  better than the component facade.
- **QML/Kirigami remains the shell language.** Rust owns durable model state and
  action methods; QML owns the reactive KDE shell (workspaces, panes, tabs,
  sidebars, notification UI, settings). Do not replace QML with imperative Rust
  UI plumbing unless the project deliberately abandons Qt Quick/Kirigami or a
  future Rust Qt binding provides complete, idiomatic coverage.
- **bindgen needs the clang resource include path.** On systems with
  `clang-libs` but not `clang-devel` (e.g. Fedora default), libclang can't
  locate `<limits.h>` and bindgen fails with
  `fatal error: 'limits.h' file not
  found`. `tako-terminal/build.rs` probes
  `/usr/lib/clang/<major>/include/` and passes `-resource-dir=<parent>` so the
  built-in headers are found. Adding new libghostty-vt headers that pull in more
  stdarg/stdint types can re-trip this on a fresh toolchain.
- **C++ terminal facade code includes libghostty-vt enum headers.** The C++
  `tako-terminal/src/tako_terminal_view.cpp` pulls in `<ghostty/vt/key/event.h>`
  and `<ghostty/vt/mouse/event.h>` for the enum constants (`GHOSTTY_KEY_*`,
  `GHOSTTY_MODS_*`, `GHOSTTY_MOUSE_*`). It also includes
  `<ghostty/vt/selection.h>` for `GHOSTTY_SELECTION_ADJUST_*`; because Qt
  defines `emit` as a macro and `selection.h` has a field named `emit`, wrap
  that include in `#pragma push_macro("emit")` / `#undef emit` /
  `#pragma pop_macro("emit")`. `tako-terminal/build.rs` resolves/fetches the
  include path and passes it via `CxxQtBuilder::include_dir`. If you add new
  enum usage to C++, keep the pinned Ghostty headers in sync with the Zig
  imports.
- **Terminal ABI structs are terminal-owned, not hand-mirrored.**
  `tako-terminal/src/tako_terminal_core.h` defines the public C++↔Zig terminal
  ABI structs (`TakoTerminalOptions`, `TakoTerminalScrollbarState`,
  `TakoTerminalBytes`) and is imported directly by `core.zig`.
  `tako-terminal/src/tako_terminal_frame.h` defines `Vertex` and `FramePlan` for
  the frame ABI, with static layout assertions.
  `tako-terminal/src/tako_terminal_backend.h` defines private Zig-only
  implementation structs (`TakoTerminalSurfaceOptions`,
  `TakoTerminalCellMetrics`, frame snapshot rows/cells,
  `TakoTerminalShapedGlyph`, `TakoTerminalShapedText`,
  `TakoTerminalRasterizedGlyph`). Zig owns live grid dimensions, pixel-to-grid
  sizing, dirty/cursor frame-state capture, row/cell snapshot capture, final
  text glyph foreground/visibility, font family/default resolution, text
  positioning, shaping/rasterization, glyph atlas packing, final `Vertex`
  assembly, and full mouse/selection geometry. `tako-terminal/build.rs` no
  longer bindgens this private implementation header for Rust. The C++ facade
  consumes `tako_terminal_core.h` and `tako_terminal_frame.h`; it must not
  include `tako_terminal_backend.h` or call Zig's internal font helpers
  directly.
- **PTY output is event-driven, not polled.** Zig `PtySession` owns the PTY
  master fd and child process. The C++ side watches that master fd with
  `QSocketNotifier`; on wake it calls `tako_terminal_session_drain_notify`
  (currently a no-op because reading the master clears readiness) and
  `tako_terminal_session_tick`. Each tick drains the nonblocking PTY master
  directly into Zig's `GhosttyTerminal`, forwards libghostty-generated response
  bytes collected by Zig effects back to the PTY, reaps the child, and then
  builds a new frame plan if needed, using Zig-owned font shaping,
  rasterization, and atlas data. Zig owns the session's current `FramePlan` copy
  and returns it on idle/sync-output ticks. A 100 ms safety `QTimer` backstops
  notifier misses and drives the env-test `TAKO_AUTORUN` harness, whose one-shot
  command/delay state lives in Zig `TerminalSession`.
  `tako_terminal_session_tick` returns `bool` — `true` only when a new frame was
  built — so the C++ skips `update()` (no GPU work) on idle ticks.
- **Wayland fractional DPR is delivered late; react to
  `ItemDevicePixelRatioHasChanged`.** A window is created with the integer DPR
  (e.g. 2) and the compositor's preferred fractional scale (e.g. 1.7) arrives
  asynchronously as a `wp_fractional_scale` preferred_scale event. Qt surfaces
  it as `QQuickItem::itemChange(ItemDevicePixelRatioHasChanged,
  {realValue})`
  — _not_ via `screenChanged` (which only fires on a monitor switch) and _not_
  via `activeFocusItemChanged` (which catches it incidentally and races
  per-monitor, leaving the terminal rendered at the wrong size). Zig marks
  `TerminalSession.needs_replan` because a DPR change reloads the font and GL
  viewport but doesn't dirty the terminal content nor change cols/rows — without
  it the idle-skip would suppress `update()` and the host would draw new big
  glyphs into the stale viewport. (General rule: the idle-skip's "did anything
  change" signal must cover every state the plan/viewport depend on, not just
  terminal content.)
- **Cursor blink is host-clocked view state.** The C++ facade owns the 530 ms
  `QTimer` and sends the current blink phase/focus state through Zig. Zig stores
  those values on `TerminalSession`, marks `TerminalSession.needs_replan` when
  they change, and computes whether the presented cursor should be hidden. Zig
  hides only when libghostty-vt marks the cursor blinkable, the item is focused,
  the phase is off, and the cursor is not `password_input`; Zig appends the
  terminal cursor quads while finalizing the frame. Blink/focus changes are
  view-state-only; they must not dirty terminal content. On focus loss, stop the
  timer and force the cursor visible.
- **IME is Qt-facade-owned host behavior.** `TerminalView::inputMethodEvent`
  sends committed `QInputMethodEvent` text through the same private Zig write
  path as typed input. `inputMethodQuery(Qt::ImCursorRectangle)` uses
  `FramePlan` cursor metadata (`cursor_x`, `cursor_y`, `cursor_present` plus
  `cell_w`/`cell_h`) converted from physical pixels to item DIPs by the current
  DPR. Inline preedit rendering is host-rendered view state: the Qt facade
  forwards `preeditString()` plus the preedit cursor byte offset through Zig.
  Zig owns the live preedit bytes/cursor byte on `TerminalSession`, marks
  `TerminalSession.needs_replan` when they change, shapes the preedit UTF-8 run
  through the Zig-owned font service, and uses libghostty-vt Unicode width
  helpers itself when drawing the preedit underline/cursor flat quads. Do not
  look for a libghostty-vt API to own the preedit string itself.
- **DEC 2026 synchronized output is libghostty mode state.** Do not parse the
  escape sequence in Qt/Zig/Rust host code. `tako_terminal_session_tick()` first
  drains Zig's PTY master into the Zig-owned terminal, then Zig checks
  `ghostty_terminal_mode_get(..., 2026)` directly. While it is set, Zig returns
  `TerminalSession.last_plan` with `changed = false`; otherwise it updates and
  reads the Zig-owned render state, decides whether the dirty bit, cursor delta,
  or `needs_replan` requires a new frame, then uses the Zig-owned font/atlas
  service while building glyph vertices for a needed frame. This check must
  happen before snapshot capture, because snapshot capture clears per-row dirty
  flags as it walks. When the mode resets, the next tick captures and presents
  the accumulated terminal state.
- **Scrollback viewport movement is view state.** Zig owns local scrollback
  navigation via `ghostty_terminal_scroll_viewport`; after moving the viewport,
  it must mark `TerminalSession.needs_replan` so the next frame-plan build
  captures the new visible rows even if terminal content dirty bits are
  unchanged. The Qt wheel handler pumps immediately after local or tracked wheel
  scrolling; do not leave scroll feedback to the 100 ms safety timer. Scrollbar
  geometry is also Zig-owned host-visible view state: Zig queries libghostty's
  `GHOSTTY_TERMINAL_DATA_SCROLLBAR` plus viewport-active bit directly from the
  live terminal handle, and `TerminalView` caches it as Qt properties
  (`scrollbarTotal`, `scrollbarOffset`, `scrollbarLength`, `viewportAtBottom`).
  QML embedders should use those properties and the
  `scrollLines`/`scrollToTop`/`scrollToBottom`/`scrollToRow` invokables rather
  than reaching into backend handles.
- **OSC 8 hyperlinks are libghostty grid-ref state.** Do not parse OSC 8 in the
  host. `TerminalView` owns pointer modifiers, hover cursor, the
  `hoveredHyperlink` Qt property, and Qt URL opening. The implementation core
  maps physical pointer coordinates to a viewport `GridRef` and asks
  `ghostty_grid_ref_hyperlink_uri`. Keep hyperlink lookup as a query through the
  private C++↔Zig ABI; do not bake URL storage into `FramePlan` unless a future
  renderer needs underline/hover decoration.
- **BEL is an embedder event, not render state.** libghostty fires the bell
  effect during `vt_write`; Zig's `TerminalSession` effect callback coalesces it
  until drained, and `TerminalView` emits `bell(count)`. Higher layers decide
  whether to play sound, flash, badge a tab/sidebar row, or raise KNotification.
- **OSC 52 is still deferred because the current public C API is insufficient.**
  The pinned libghostty internals parse OSC 52, but `terminal.h` exposes no
  clipboard effect callback and `osc.h` exposes
  `GHOSTTY_OSC_COMMAND_CLIPBOARD_CONTENTS` only as a command type, not readable
  payload data. Do not add an ad hoc host parser; prefer an upstreamable/public
  libghostty-vt API bump.
- **Selection is driven by `GhosttySelectionGesture`, not hand-rolled; selection
  state is view-only and must trip the idle-skip.** At our pinned ghostty
  commit, libghostty-vt ships the full gesture state machine
  (`GhosttySelectionGesture`: press/drag/release/autoscroll/deep-press +
  multi-click behaviors) plus semantic derives and one-shot clipboard
  formatting. Zig owns the `GhosttySelectionGesture` handle plus reusable
  press/drag/release/autoscroll event handles in `TerminalSession`, and frees
  them before destroying the Zig-owned terminal and font state. Selections are
  _installed_ into the terminal (`GHOSTTY_TERMINAL_OPT_SELECTION`) so the
  render-state machinery owns the per-row highlight ranges
  (`GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION`), which `FrameSnapshot::Row` reads
  as plain data. A selection change is view-state-only — it does not dirty
  terminal content — so it MUST set `TerminalSession.needs_replan` or the
  idle-skip suppresses the highlight redraw (same trap as the DPR invariant).
  Drag-to-edge autoscroll is also libghostty-driven: Zig dispatches
  `AUTOSCROLL_TICK`, while the C++ facade owns a 50 ms `QTimer` that
  starts/stops from the gesture's `AUTOSCROLL` state and feeds the latest
  physical pointer position/modifiers back through the Zig ABI. Mouse positions
  should resolve to `Point::viewport` grid refs even when the viewport is
  scrolled back; Ghostty maps those through the current viewport and tracks the
  initial press pin across viewport movement. Keyboard selection is also
  libghostty-driven: Shift+navigation keys are intercepted by the C++ facade,
  sent through Zig as `GHOSTTY_SELECTION_ADJUST_*`; Zig seeds from the active
  cursor when needed, applies `ghostty_terminal_selection_adjust`, installs the
  selection, and marks `TerminalSession.needs_replan`. Multi-click behavior is a
  Qt-facing component policy (`singleClickSelection`, `doubleClickSelection`,
  `tripleClickSelection`) that maps directly to libghostty's
  cell/word/line/output gesture units. Select-all and semantic
  command-output/input selection are also libghostty derives:
  `TerminalView::selectAll()`, `selectCommandOutputAt(x, y)`, and
  `selectCommandInputAt(x, y)` call through Zig, which derives the
  `GhosttySelection`, installs it with `GHOSTTY_TERMINAL_OPT_SELECTION`, and
  marks `TerminalSession.needs_replan`. Selection runs only in the
  `!mouse_tracking()` branch; if tracking turns on mid-drag, abort the selection
  (clear `OPT_SELECTION`, reset the gesture) and stop the autoscroll timer.
- **Selection copy is length-delimited and owned across the private ABI.** The
  C++ facade must not use fixed stack buffers for clipboard text. Zig formats
  the active selection with libghostty-vt, copies it into a `TakoTerminalBytes`
  allocation, and C++ converts it with explicit UTF-8 length before calling
  `tako_terminal_bytes_free`. Keep this ownership pattern for PRIMARY
  copy-on-select and CLIPBOARD copy.
