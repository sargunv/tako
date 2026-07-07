# Tako

A native KDE terminal for AI coding agents. Built in Rust on Qt6/QML/Kirigami,
with libghostty-vt as the terminal core.

## Project map

```
tako/
├── crates/
│   ├── tako-term/      libghostty-vt bindgen wrapper, PTY bridge, OSC dispatch
│   ├── tako-render/    glyph atlas (freetype+rustybuzz) + QQuickItem RHI renderer
│   ├── tako-model/     Window/Workspace/Split/Pane/Surface/Panel tree
│   ├── tako-bonsplit/  binary split tree
│   ├── tako-dbus/      D-Bus server + client (zbus)
│   ├── tako-cli/       `tako` CLI -> D-Bus
│   ├── tako-git/       gix branch/dirty/index + inotify; reqwest PR polling
│   ├── tako-net/       procfs port scanner + per-workspace attribution
│   ├── tako-notify/    OSC ingest + notification store + KNotification bridge
│   ├── tako-hooks/     agent hook installers + state machine
│   ├── tako-session/   serde snapshot to ~/.local/state/tako/ (XDG paths)
│   ├── tako-config/    KConfig bridge + ghostty config reader + project JSON
│   ├── tako-term/      libghostty-vt bindgen + link (build.rs fetches/builds)
│   └── tako-app/       cxx-qt bridge + `tako` binary entry (QML <-> Rust)
├── kcfg/               (future) takorc.kcfg schema + .kcfgc codegen
├── data/               (future) .desktop, metainfo, icons, D-Bus service file
├── ROADMAP.md          the authoritative design document
└── cmux/               (gitignored) product reference checkout
```

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

- **`unsafe_code` is denied workspace-wide.** The only permitted exception is a
  cxx-qt `#[cxx_qt::bridge]` module, which needs `unsafe extern` blocks (edition
  2024 FFI syntax). Scope the relaxation with a module-level
  `#![allow(unsafe_code)]` inside the bridge file only — never relax at crate or
  workspace level.
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
