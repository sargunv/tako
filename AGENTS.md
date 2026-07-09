# Tako

A native KDE terminal for AI coding agents. Built in Rust on Qt6/QML/Kirigami,
with libghostty-vt as the terminal core.

## Project map

```
tako/
├── crates/
│   ├── tako-app/       cxx-qt bridge + `tako` binary entry (QML <-> Rust)
│   └── tako-terminal/  embeddable Qt Quick TerminalView package (C++ facade + Zig core)
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
later builds skip it.

Native system libraries (Qt6/KDE Frameworks, freetype, harfbuzz, fontconfig) are
expected from the host for now; pixi/conda-forge packaging is deferred.
`crates/tako-terminal/build.rs` uses pkg-config to link system **freetype2** and
**harfbuzz** for the Zig core. Zig owns font loading, shaping, rasterization,
cell metrics, atlas packing, and render-frame planning in production.

## Project invariants

<!-- List non-negotiable rules for the project as they emerge. -->

- **All UI must confirm to the KDE Human Interface Guidelines.** Research
  https://develop.kde.org/hig/ before adding or changing UI components, copy, or
  icons.
- **`unsafe_code` is denied workspace-wide.** Scope exceptions narrowly with
  module-level `#![allow(unsafe_code)]`; never relax it at workspace level.
  Expected exceptions are FFI/build glue only.
- **Create phase crates only when work starts.** Do not pre-scaffold empty
  crates for later roadmap phases; they make the workspace noisier without
  improving design.
- **Build the remaining app around snapshots plus actions.** Rust owns durable
  model state and exposes immutable snapshots/actions to QML. QML/Kirigami owns
  the reactive shell for workspaces, panes, tabs, sidebars, notifications, and
  settings.
- **Keep the terminal an embeddable component.** App and QML code should consume
  `TerminalView` through its public Qt properties, signals, and invokables. New
  app features should not depend on terminal backend internals.
- **Use libghostty-vt as the source of terminal truth.** Prefer its parsers,
  mode tracking, input encoders, render-state dirty tracking, grid/scrollback,
  OSC state, and selection machinery over host-side parsers or hand-rolled VT
  logic. OSC 52 remains deferred until a public libghostty-vt API exposes the
  clipboard payload.
- **Keep settings ownership split.** App settings belong in KDE settings
  (`KConfig`/`KConfigXT`); project-scoped, agent-editable settings belong in
  `.config/tako/config.toml`.
- **Prefer KDE-native integrations.** Use KDE/Qt facilities for settings,
  notifications, global shortcuts, activation/focus, theme colors/icons, file
  operations, and packaging. Avoid "portable" escape hatches.
