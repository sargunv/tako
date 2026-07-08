# Tako

A native KDE terminal for AI coding agents. Built in Rust on Qt6/QML/Kirigami,
with libghostty-vt as the terminal core.

## Project map

```
tako/
├── crates/
│   ├── tako-term/      libghostty-vt bindgen + link (build.rs fetches/builds); PTY bridge, OSC dispatch
│   ├── tako-render/    TerminalPanel + FramePlanner + Surface + QQuickFramebufferObject GL renderer (C ABI in ffi.rs)
│   └── tako-app/       cxx-qt bridge + `tako` binary entry (QML <-> Rust)
├── kcfg/               (future) takorc.kcfg schema + .kcfgc codegen
├── data/               (future) .desktop, metainfo, icons, D-Bus service file
├── ROADMAP.md          the authoritative design document
└── cmux/               (gitignored) product reference checkout
```

Crates for later phases (`tako-model`, `tako-bonsplit`, `tako-dbus`, `tako-cli`,
`tako-git`, `tako-net`, `tako-notify`, `tako-hooks`, `tako-session`,
`tako-config`) are created when their phase starts, not pre-scaffolded.

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

`tako-term`'s first build fetches the pinned ghostty tarball (~37 MB) and runs
`zig build -Demit-lib-vt` (several minutes). The result is cached under
`~/.cache/tako/ghostty-vt/<commit>/` so later builds skip it. bindgen needs
`libclang` (Fedora: `clang-devel`, or the runtime `clang-libs` suffices).

Native system libraries (Qt6/KDE Frameworks, freetype, harfbuzz, fontconfig) are
expected from the host for now; pixi/conda-forge packaging is deferred.
`tako-render` links system **freetype** via the `freetype-rs` crate; shaping
uses **rustybuzz** (pure-Rust HarfBuzz port) instead of `harfbuzz_rs` because
`harfbuzz-sys` pulls its own `freetype-sys`, creating a cargo
`links =
"freetype"` conflict. Revisit if strict system-HarfBuzz linkage is
required.

## Project invariants

<!-- List non-negotiable rules for the project as they emerge. -->

- **`unsafe_code` is denied workspace-wide.** The permitted exceptions are: a
  cxx-qt `#[cxx_qt::bridge]` module (`unsafe extern` blocks, edition 2024 FFI
  syntax); `crates/tako-app/build.rs` (an `unsafe` `cc_builder` closure —
  cxx-qt-build 0.9's only flag-passing API); and the `libghostty-vt` boundary in
  `crates/tako-term/src/*` + the C ABI in `crates/tako-render/src/ffi.rs` +
  `crates/tako-render/src/gl_renderer.rs` (raw GL/handle pointers). Scope every
  relaxation with a module-level `#![allow(unsafe_code)]` — never relax at crate
  or workspace level.
- **The terminal backend is single-threaded by construction.** `Terminal` owns
  raw pointers and is `!Send`; effects fire synchronously inside
  `ghostty_terminal_vt_write` on the owning thread. Accordingly the
  `tako-term::effects` callback types are intentionally **not** `Send`, and
  `TerminalPanel` shares effect side-state via `Rc<RefCell<_>>` (not
  `Arc<Mutex<_>>`). Don't add `Send` bounds or thread-spawning here without
  reworking the ownership model.
- **The Rust `Surface` is GUI-thread-owned; `createRenderer` is not.**
  `QQuickFramebufferObject::createRenderer()` runs on the QSG render thread. The
  `Surface` (and its `QSocketNotifier`) must be created on the GUI thread — in
  the `TakoTerminalView` constructor — never in `createRenderer`, or the
  notifier would parent cross-thread _and_ the GUI-thread timer would touch a
  `!Send` `Surface` from the wrong thread. `createRenderer` only spawns the
  render-thread `TakoTerminalRenderer`, which reads `view->plan()` during
  `synchronize()` (called with the GUI thread blocked — the only safe
  render-thread→item touch).
- **`tako-render` is split along the model/view seam.** `TerminalPanel`
  (panel.rs) owns the terminal + PTY + OSC state — no font, no atlas. This is
  what Phase 2's `tako-model` tree will own per-surface. `FramePlanner`
  (frame_planner.rs) is a pure view: snapshot in, `FramePlan` out, no terminal
  access — unit-testable without a shell. `Surface` (surface.rs) just
  orchestrates the two plus the input encoders. The `extern "C"` ABI the C++
  `QQuickFramebufferObject` calls lives entirely in `ffi.rs`. Keep new
  terminal-core state in `TerminalPanel`, new rendering state in `FramePlanner`,
  and new C entry points in `ffi.rs` — don't re-bloat `Surface` or scatter FFI
  across modules.
- **Qt is discovered via `qmake` on PATH.** `mise.toml` sets
  `QT_VERSION_MAJOR=6` so cxx-qt-build picks the Qt6 `qmake6`. Keep that env var
  when adding any Qt-linking crate.
- **The ghostty pin must carry the full libghostty-vt C API** (`render.h`,
  `terminal.h`, `build_info.h`, static-lib build). The latest stable tag
  (v1.3.1) lacks these — they landed upstream on `main` after v1.3.1. Tako pins
  an upstream `main` commit in `crates/tako-term/build.rs`; bump it deliberately
  and re-verify the bindgen surface.
- **cxx-qt-build does not register hand-written C++ `QML_ELEMENT` classes.** Its
  compiled `org.tako` QML module only registers types generated from the cxx-qt
  bridge (`#[qml_element]`). A C++ `QQuickItem` subclass added via
  `CxxQtBuilder::cpp_file` must register itself imperatively with
  `qmlRegisterType<T>("org.tako.<sub>", ...)` under a **separate** URI (the
  compiled module's qmldir takes precedence over same-URI imperative
  registrations). The registration C ABI lives in the C++ file; the safe Rust
  wrapper is `tako_render::qml_init::register_qml_types()`, called from
  `tako_app::run()` before `engine.load`.
- **bindgen needs the clang resource include path.** On systems with
  `clang-libs` but not `clang-devel` (e.g. Fedora default), libclang can't
  locate `<limits.h>` and bindgen fails with
  `fatal error: 'limits.h' file not
  found`. `tako-term/build.rs` probes
  `/usr/lib/clang/<major>/include/` and passes `-resource-dir=<parent>` so the
  built-in headers are found. Adding new libghostty-vt headers that pull in more
  stdarg/stdint types can re-trip this on a fresh toolchain.
- **C++ view code includes libghostty-vt enum headers.** The C++
  `TakoTerminalView` pulls in `<ghostty/vt/key/event.h>` and
  `<ghostty/vt/mouse/event.h>` for the enum constants (`GHOSTTY_KEY_*`,
  `GHOSTTY_MODS_*`, `GHOSTTY_MOUSE_*`). `tako-app/build.rs` resolves the include
  path from the tako-term cache and passes it via `CxxQtBuilder::include_dir`.
  If you add new enum usage to C++, rebuild `tako-term` first (its build script
  fetches the headers).
- **The C ABI contract is generated, not hand-mirrored.** `tako-render/build.rs`
  runs `cbindgen` over the crate and writes `cpp/tako_render.h` (gitignored) —
  `FramePlan`, `Vertex`, the opaque `Surface`/`GlRenderer`, `LoaderFn`, and
  every `tako_surface_*` / `tako_gl_renderer_*` declaration. The C++
  `TakoTerminalView` includes it and stores typed `Surface*`/`GlRenderer*`
  members; there is no second hand-rolled copy of the contract. Adding a field
  to `FramePlan` or a new `tako_surface_*` fn needs no manual C++ sync — the
  next build regenerates the header. (Don't edit `tako_render.h` by hand; it's
  overwritten on every build where the source changed.)
- **PTY output is event-driven, not polled.** `StreamingPty` owns a readiness
  pipe (`nix`): the reader thread writes one byte after each successful PTY
  read, the C++ side watches the read end with `QSocketNotifier`, and on wake
  calls `tako_surface_drain_notify` + `tako_surface_tick`. A 100 ms safety
  `QTimer` backstops it (and drives the env-test `TAKO_AUTORUN` harness).
  `tako_surface_tick` returns `bool` — `true` only when a new frame was built —
  so the C++ skips `update()` (no GPU work) on idle ticks. The write end is held
  by `StreamingPty` for its whole lifetime so the read end never sees EOF (which
  would busy-loop the level-triggered notifier) until teardown. If the pipe
  can't be created, `notify_fd` returns -1 and the timer alone drives ticks.
- **Wayland fractional DPR is delivered late; react to
  `ItemDevicePixelRatioHasChanged`.** A window is created with the integer DPR
  (e.g. 2) and the compositor's preferred fractional scale (e.g. 1.7) arrives
  asynchronously as a `wp_fractional_scale` preferred_scale event. Qt surfaces
  it as `QQuickItem::itemChange(ItemDevicePixelRatioHasChanged,
  {realValue})`
  — _not_ via `screenChanged` (which only fires on a monitor switch) and _not_
  via `activeFocusItemChanged` (which catches it incidentally and races
  per-monitor, leaving the terminal rendered at the wrong size).
  `Surface::set_dpr` sets a `needs_replan` flag because a DPR change reloads the
  font + GL viewport but doesn't dirty the terminal content nor change cols/rows
  — without it the idle-skip would suppress `update()` and the host would draw
  new big glyphs into the stale viewport. (General rule: the idle-skip's "did
  anything change" signal must cover every state the plan/viewport depend on,
  not just terminal content.)
