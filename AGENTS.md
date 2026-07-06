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
│   └── tako-app/       cxx-qt bridge: registers Rust model to QML
├── qml/                (future) sidebar, tabs, splits, notification panel
├── kcfg/               (future) takorc.kcfg schema + .kcfgc codegen
├── data/               (future) .desktop, metainfo, icons, D-Bus service file
├── src/main.rs         loads QML, starts D-Bus server, drives the model
├── ROADMAP.md          the authoritative design document
└── cmux/               (gitignored) product reference checkout
```

See ROADMAP.md for the full architecture, data model, and phased roadmap.

## Dev tool commands

The project uses [mise](https://mise.jdx.dev) to bootstrap the toolchain
([hk](https://hk.jdx.dev), [dprint](https://dprint.dev), rust), recorded in
`mise.toml` / `mise.lock`.

- `mise install` — install pinned toolchain.
- `mise run check` — run `hk check --all` (dprint check + cargo clippy).
- `mise run fix` — run `hk fix --all` (dprint fmt + cargo clippy --fix).
- `cargo build` / `cargo test` — usual Rust workspace commands.

Native system libraries (Qt6/KDE Frameworks, freetype, harfbuzz, fontconfig) are
expected from the host for now; pixi/conda-forge packaging is deferred.

## Project invariants

<!-- List non-negotiable rules for the project as they emerge. -->
