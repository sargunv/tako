# tako-terminal

Embeddable Qt Quick terminal component for Tako.

`tako-terminal` exposes a `TerminalView` QML item (under `org.tako.terminal`)
that any Qt Quick application can drop in. The embedding app treats it like any
other Qt component: it binds to properties, connects signals, and calls
invokables, without reaching into backend internals.

## How it works

The component splits cleanly down an private ABI:

- **Qt/QML facade** (C++): the `TerminalView` item and its render-thread
  renderer. Owns Qt concerns — input events, focus, clipboard, timers, window
  and DPR hooks, QML registration.
- **Implementation core** (Zig): owns the live terminal. It drives the
  libghostty-vt terminal and render state, the PTY session and shell
  integration, input encoding, selection, font shaping/rasterization, glyph
  atlas, and frame planning. It produces a `FramePlan` the C++ renderer draws.

The boundary between the two sides stays private and mechanical: the facade
hands host events and state in, and the core hands frame plans and queries out.

## Library self-sufficiency

This package fetches, verifies, builds, and links the pinned libghostty-vt
static library itself, so dependents don't need ghostty on their build path. It
also links system FreeType/HarfBuzz for the Zig font service.

## Configuration

`tako-terminal` exposes its knobs as regular Qt properties on `TerminalView` —
it has no internal settings system of its own. The embedding app owns
configuration policy; for Tako that will be KDE settings. Font, default colors,
and default cursor properties apply live without restarting the PTY;
session-spawn properties (`program`, `initialWorkingDirectory`,
`scrollbackLimit`, `shellIntegration`) apply at session start and on
`restart()`.
