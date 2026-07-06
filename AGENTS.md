# Tako

A native KDE terminal for AI coding agents. Built in Rust on Qt6/QML/Kirigami,
with libghostty-vt as the terminal core.

## Project map

```
tako/
├── crates/
│   ├── tako-term/      libghostty-vt bindgen wrapper, PTY bridge, OSC dispatch
│   ├── tako-render/    QQuickItem RHI terminal renderer (cxx-qt-exposed)
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
- `cargo build` / `cargo test` — usual Rust workspace commands.
- `cargo run -p tako-app` — launch the Tako window (Phase 0 spike). Requires Qt6
  `-devel` packages on the host and an active Plasma/ graphical session.

`tako-term`'s first build fetches the pinned ghostty tarball (~37 MB) and runs
`zig build -Demit-lib-vt` (several minutes). The result is cached under
`~/.cache/tako/ghostty-vt/<commit>/` so later builds skip it. bindgen needs
`libclang` (Fedora: `clang-devel`, or the runtime `clang-libs` suffices).

Native system libraries (Qt6/KDE Frameworks, freetype, harfbuzz, fontconfig) are
expected from the host for now; pixi/conda-forge packaging is deferred.

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
