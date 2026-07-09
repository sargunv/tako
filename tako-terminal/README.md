# tako-terminal

Embeddable Qt Quick terminal component for Tako.

`tako-terminal` is the boundary we want to be able to carve out as a small
library for any Qt application:

- public API: Qt/QML `TerminalView` under `org.tako.terminal`;
- facade language: C++, because Qt Quick item/render-thread vtables live here;
- implementation boundary: Zig behind a private C ABI;
- libghostty-vt build ownership: this package fetches, verifies, builds, and
  links the pinned `libghostty-vt.a`;
- current backend: Zig owns the live libghostty-vt terminal, render-state
  handle, effects callbacks, PTY/session lifecycle, input encoding, selection,
  shell integration, and terminal option/query plumbing, plus presentation state
  such as focus, cursor blink phase, IME preedit bytes, previous cursor state,
  font family/default resolution, render-state row/cell snapshot capture, and
  the frame rebuild decision. Zig also owns font shaping/rasterization, cell
  metrics, glyph atlas work, terminal glyph foreground/visibility resolution,
  terminal/preedit text positioning, terminal cell backgrounds, text
  decorations, preedit underline/cursor flats, terminal cursor quads, final
  `Vertex` buffers, and final `FramePlan` metadata.

The app should treat this like any other Qt component. `tako-app` may bind to Qt
properties, connect signals, and call invokables, but it should not reach
through to implementation-core details.

Configuration loading belongs to the embedding application. For Tako, app
settings will eventually bind to `TerminalView` properties; for now the demo QML
hardcodes a small set of defaults. The terminal component itself does not read
dotfiles.

## TerminalView API

Current QML-facing surface:

- `program`: optional executable. Empty means the user's shell.
- `initialWorkingDirectory`: optional cwd for session spawn.
- `scrollbackLimit`: maximum scrollback rows for the next session. Defaults to
  10000.
- `shellIntegration`: auto-source Tako shell integration for supported spawned
  shells (`bash`, `zsh`, `fish`) so libghostty-vt receives OSC 133 semantic
  prompt markers. Defaults to `true`.
- `autoStart`: create the PTY session automatically at `componentComplete`.
  Defaults to `true`; set to `false` for deferred/lazy session creation. Setting
  it back to `true` after completion starts the session.
- `running`: read-only `true` while a PTY session exists and has not exited.
- `title`: read-only OSC-derived title.
- `currentWorkingDirectory`: read-only OSC-derived cwd.
- `hoveredHyperlink`: read-only OSC 8 URI under the pointer while Ctrl/Meta is
  held.
- `exited`: read-only session exit state.
- `scrollbarTotal`, `scrollbarOffset`, `scrollbarLength`: read-only scrollback
  viewport geometry for embedder-owned scrollbars.
- `viewportAtBottom`: read-only `true` when the viewport follows the active
  screen instead of looking back through scrollback.
- `fontFamily`: optional fontconfig family. Empty means system monospace.
- `fontPixelSize`: logical terminal cell height; live-reloads the font without
  restarting the PTY.
- `fontPointSize`: Qt-style point-size twin for `fontPixelSize`; live-reloads
  the font without restarting the PTY.
- `foregroundColor`, `backgroundColor`, `cursorColor`: optional terminal default
  colors. Invalid/unset colors use libghostty-vt defaults; live changes preserve
  OSC color overrides owned by libghostty state.
- `colorPalette`: optional full 256-entry default palette as a `QVariantList` of
  `QColor`s. Empty/unset resets to libghostty-vt's built-in palette.
- `cursorStyle`: default DECSCUSR reset cursor style (`BarCursor`,
  `BlockCursor`, `UnderlineCursor`, `HollowBlockCursor`).
- `cursorBlink`: default DECSCUSR reset blink preference.
- `singleClickSelection`, `doubleClickSelection`, `tripleClickSelection`: choose
  libghostty-vt gesture units (`CellSelection`, `WordSelection`,
  `LineSelection`, `CommandOutputSelection`) for multi-click selection.
- `engineVersion`: read-only libghostty-vt version string, queried by Zig.
- `copySelection()`, `pasteClipboard()`, `clearSelection()`, `selectAll()`,
  `selectCommandOutputAt(x, y)`, `selectCommandInputAt(x, y)`,
  `writeText(text)`, `scrollLines(lines)`, `scrollToTop()`, `scrollToBottom()`,
  `scrollToRow(row)`, `start()`, `stop()`, `restart()`.
- `bell(count)`: emitted after BEL, with coalesced count since the previous
  pump. Embedders decide whether this becomes sound, visual flash, sidebar
  attention, or desktop notification.

Session-spawn properties (`program`, `initialWorkingDirectory`,
`scrollbackLimit`, `shellIntegration`) are applied before `componentComplete()`
starts the PTY, or before an explicit `start()` when `autoStart` is false.
Changing them after a session starts takes effect on `restart()`. `stop()` kills
the current session and clears host-visible session state; `restart()` is
`stop()` followed by `start()`. Font, default color/palette, and default cursor
properties apply live: the component reloads glyph caches, updates libghostty-vt
terminal defaults, and reflows/replans while keeping the PTY running.

## Ownership Rules

The Qt facade owns Qt concerns: event extraction, focus, clipboard, timers,
socket notifier, QML registration, window/DPR hooks, and the render-thread
`QQuickFramebufferObject::Renderer`. It also owns Qt input-method integration:
committed IME text is sent as terminal input, and `ImCursorRectangle` is derived
from the latest frame plan's cursor metadata.

Selection autoscroll deliberately straddles the facade/core line: libghostty-vt
owns the gesture state and autoscroll decision, while the Qt facade owns the
timer that periodically feeds the latest pointer position back into the core.
Keyboard selection follows the same rule: the Qt facade captures
Shift+navigation shortcuts, while the core applies libghostty-vt selection
adjustments and owns the active selection state. Multi-click selection is also
libghostty-owned: the facade only chooses the behavior table exposed through Qt
properties. Select-all and semantic command-output/input selection also use
libghostty-vt derives. The facade exposes them as QML invokables, and the
implementation core installs the resulting terminal-owned active selection for
rendering and copying.

Shell integration is spawn configuration, not terminal parsing. The Zig PTY core
creates per-session temporary startup files for supported shells and removes
them when the session ends. The scripts emit OSC 133 markers that libghostty-vt
already understands; host code must not parse those sequences itself. For zsh,
the temporary startup path restores the user's real `ZDOTDIR` before `.zshrc`
loads so plugin managers and prompt themes see their normal config directory.

Cursor blink also straddles the boundary: the Qt facade owns the 530 ms timer
and focus transitions, while the implementation core applies the phase as
render-only state. Zig computes the presented cursor and keeps cursor
presentation inside implementation-owned frame planning. Blink is suppressed
when the item is unfocused or libghostty-vt marks the cursor as password input.

Synchronized output (DEC 2026) is owned by libghostty-vt mode state. Zig checks
that mode after each PTY drain and before capturing a terminal frame, deferring
new frame plans while the mode is set and flushing the accumulated terminal
state after the mode resets.

Scrollback navigation is libghostty-owned state. Zig moves the viewport with
`ghostty_terminal_scroll_viewport`, queries scrollbar geometry directly from
libghostty-vt, and the facade exposes cached Qt properties for embedders.
Embedders should build scrollbar UI from those properties instead of querying
the terminal backend.

Hyperlinks are also libghostty-owned state. The facade sends physical pointer
coordinates through the private ABI, the backend resolves them to a grid ref,
and libghostty returns the OSC 8 URI. The facade owns user policy: hover cursor,
`hoveredHyperlink`, and Ctrl/Meta-click opening through Qt.

BEL is emitted as a Qt signal. The implementation core counts libghostty's bell
effect during each PTY pump, the Zig `TerminalSession` coalesces pending rings,
and the facade emits `bell(count)`. Higher-level notification policy belongs to
the embedding app.

IME preedit rendering is host-owned view state. The facade forwards Qt preedit
text and cursor position through the private ABI; the implementation core owns
the live preedit bytes/cursor byte, shapes the preedit text with its Zig-owned
font service, and draws the preedit underline/cursor flats itself while
committed text still goes to the PTY.

The implementation core owns terminal concerns: libghostty-vt handles, PTY
session, input encoding, selection, frame planning, glyph atlas, and renderer
state. Keep the ABI between these two sides private and mechanical.

Current files:

- `Cargo.toml` + `build.rs`: app-facing package/build boundary. Cargo fetches
  and links the pinned libghostty-vt static library, builds the Qt facade, and
  builds the Zig core.
- `lib.rs`: Rust-side QML registration shim used by `tako-app`.
- `tako_terminal_view.*`: Qt/QML facade.
- `tako_terminal_core.h`: private C++↔Zig ABI consumed by the facade and
  imported directly by the Zig core, including terminal construction options
  (`TakoTerminalOptions`), scrollbar state, and owned byte buffers.
- `tako_terminal_frame.h`: private frame-plan ABI (`FramePlan`/`Vertex`) shared
  by the facade and implementation core. Zig produces this public render-thread
  frame shape.
- `tako_terminal_backend.h`: private Zig implementation structs for resolved
  font creation options, cell metrics, frame snapshots, shaped text runs, and
  rasterized glyph bitmaps. The C++ facade must not include it.
- `core.zig`: implementation-core shim. It owns the live libghostty-vt terminal,
  effects callbacks, PTY/session lifecycle, keyboard/focus/paste/mouse-event
  encoding, scrollback viewport navigation, resolved font-path selection,
  scrollbar state, hyperlink hit-testing, BEL coalescing, and direct
  libghostty-vt mode/data queries, including OSC title/current-directory
  readout, viewport geometry, pointer selection gestures/autoscroll, keyboard
  selection adjustment, select-all/semantic selection derives, DEC 2026
  synchronized-output gating, and selection formatting/copy-out. Move real
  terminal responsibilities here without changing the facade API.
- `terminal.rs` and the Rust libghostty helper wrappers are test-only
  scaffolding. Production terminal behavior, font shaping/rasterization, atlas
  work, text positioning, cursor presentation, and preedit flat geometry live in
  Zig.
