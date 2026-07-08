//! Live terminal surface: orchestrates a [`TerminalPanel`] (terminal + PTY +
//! OSC state) with a [`FramePlanner`] (font + atlas + vertices) and the input
//! encoders. This is the glue the C++ `QQuickFramebufferObject` item drives.
//!
//! Threading: not `Send` — both the panel (terminal) and the planner (font
//! caches) live on the GUI thread. The only piece on a background thread is
//! the PTY reader inside [`StreamingPty`], and it only touches its own mutex
//! buffer, never the terminal.

use std::time::{Duration, Instant};

use tako_term::input as vt_input;
use tako_term::key;
use tako_term::key::{KeyEncoder, KeyEvent};
use tako_term::modes;
use tako_term::mouse::{MouseEncoder, MouseEvent};
use tako_term::snapshot::{Cursor, Dirty};

use crate::Error;
use crate::font::CellMetrics;
use crate::frame_planner::FramePlan;
use crate::frame_planner::FramePlanner;
use crate::panel::TerminalPanel;

/// A live terminal surface. Drop closes the PTY and frees everything.
pub struct Surface {
    panel: TerminalPanel,
    planner: FramePlanner,
    key_encoder: KeyEncoder,
    mouse_encoder: MouseEncoder,
    /// Scratch event handles reused per keystroke / per mouse event (cheap
    /// allocation amortized across thousands of events).
    key_event: KeyEvent,
    mouse_event: MouseEvent,
    autorun: Option<Autorun>,
    /// Device pixel ratio the font was rasterized for.
    dpr: f32,
    /// Last frame plan, returned unchanged on idle ticks so the C++ side can
    /// skip `update()` (no GPU work) when nothing changed. Its borrowed
    /// pointers stay valid as long as the planner doesn't rebuild — which only
    /// happens on a non-idle tick.
    last_plan: FramePlan,
    /// Set whenever *view* state changes in a way `snap.dirty` can't observe —
    /// currently just a DPR/font reload (`set_dpr`): the terminal content is
    /// unchanged (so the render-state dirty flag stays false and cols/rows are
    /// unchanged, making `resize_to_pixels` a no-op) but the glyphs, atlas, and
    /// the GL viewport all need a rebuild + `update()` or the host renders the
    /// new big glyphs into the stale small viewport. Cleared after `build_plan`.
    needs_replan: bool,
    /// Last rendered cursor state. libghostty-vt can report `Dirty::False` for
    /// cursor-only movement, so cursor changes are tracked separately.
    last_cursor: Option<Cursor>,
}

/// Optional one-shot command injected into the PTY shortly after spawn. Driven
/// by `TAKO_AUTORUN` (command) + `TAKO_AUTORUN_DELAY_MS` env vars for hands-off
/// perf testing (lets the shell's slow zshrc finish before we inject).
struct Autorun {
    start: Instant,
    delay: Duration,
    cmd: Vec<u8>,
    fired: bool,
}

impl Surface {
    /// Spawn a shell on a PTY, load `font_path` (or the system default
    /// monospace if `None`) at `pixel_height`, and rasterize at
    /// `dpr × pixel_height` physical pixels for hidpi sharpness.
    pub fn new(
        cols: u16,
        rows: u16,
        font_path: Option<&str>,
        pixel_height: u32,
        dpr: f32,
    ) -> Result<Self, Error> {
        let panel = TerminalPanel::new(cols, rows)?;
        let planner = FramePlanner::new(font_path, pixel_height, dpr)?;
        Self::assemble(panel, planner, dpr)
    }

    fn assemble(panel: TerminalPanel, planner: FramePlanner, dpr: f32) -> Result<Self, Error> {
        let mouse_encoder = MouseEncoder::new()?;
        Ok(Self {
            panel,
            planner,
            key_encoder: KeyEncoder::new()?,
            mouse_encoder,
            key_event: KeyEvent::new()?,
            mouse_event: MouseEvent::new()?,
            autorun: build_autorun(),
            dpr,
            last_plan: FramePlan::default(),
            needs_replan: true,
            last_cursor: None,
        })
    }

    pub fn cols(&self) -> u16 {
        self.panel.cols()
    }
    pub fn rows(&self) -> u16 {
        self.panel.rows()
    }
    pub fn cell(&self) -> CellMetrics {
        self.planner.cell()
    }

    /// Reload the font at a new device-pixel ratio, invalidating all
    /// size-dependent caches. The caller should follow this with
    /// [`Surface::resize_to_pixels`] (passing the current physical item size)
    /// so the grid reflows to the new cell metrics.
    ///
    /// No-op when `dpr` is within 0.01 of the current value.
    pub fn set_dpr(&mut self, dpr: f32) {
        if (dpr - self.dpr).abs() < 0.01 {
            return;
        }
        self.dpr = dpr;
        let cell = self.planner.set_dpr(dpr);
        // Refresh the size-query snapshot with the new cell metrics.
        self.panel.set_cell_metrics(cell);
        // Force a replan even though terminal content (and thus cols/rows)
        // is unchanged: the font, atlas, and GL viewport all changed, and
        // without this the idle-skip would suppress `update()` and the host
        // would render new big glyphs into the stale viewport.
        self.needs_replan = true;
    }

    /// Resize the terminal grid to fit `width_px × height_px` physical pixels.
    /// Computes cols/rows from the cell metrics, resizes the panel (terminal +
    /// PTY) and the mouse-encoder size context. No-op if the computed size
    /// matches the current one (avoids resize storms on sub-cell window motion
    /// during drag).
    pub fn resize_to_pixels(&mut self, width_px: u32, height_px: u32) {
        let cell = self.planner.cell();
        let cw = cell.width.max(1);
        let ch = cell.height.max(1);
        let cols = ((width_px / cw).max(1)).min(u16::MAX as u32) as u16;
        let rows = ((height_px / ch).max(1)).min(u16::MAX as u32) as u16;
        if cols == self.panel.cols() && rows == self.panel.rows() {
            return;
        }
        self.panel.resize(cols, rows, cell);
        // Keep the mouse encoder's size context in sync so coordinate mapping
        // (surface px → cell coords) stays correct.
        self.mouse_encoder
            .set_size(cols as u32 * cw, rows as u32 * ch, cw, ch, 0);
    }

    /// Send typed input (keyboard) to the shell.
    pub fn write_input(&mut self, bytes: &[u8]) {
        self.panel.write_input(bytes);
    }

    /// Encode and send a key event. `key` is a `GhosttyKey` enum value;
    /// `mods`/`consumed_mods` are `GhosttyMods` bitmasks; `text` is the
    /// UTF-8 text the key produced (or `None` to let the encoder derive it).
    pub fn key_event(
        &mut self,
        action: tako_term::ffi::GhosttyKeyAction,
        key: tako_term::ffi::GhosttyKey,
        mods: u16,
        consumed_mods: u16,
        text: Option<&[u8]>,
    ) {
        self.key_event.set_action(action);
        self.key_event.set_key(key);
        self.key_event.set_mods(mods);
        self.key_event.set_consumed_mods(consumed_mods);

        // Set the unshifted codepoint from the logical key. This lets the
        // encoder correctly handle Caps Lock + Ctrl combos and derive the
        // right C0 byte when no UTF-8 text is supplied (e.g. Ctrl+C → \x03).
        self.key_event
            .set_unshifted_codepoint(key::unshifted_codepoint(key));

        // Strip C0 control characters (U+0000–U+001F, U+007F) from the UTF-8
        // text. The encoder contract (key/event.h:430–440) says: "Do not pass
        // C0 control characters … pass NULL instead and let the encoder use
        // the logical key." Qt's QKeyEvent::text() returns these control
        // characters for Ctrl+letter combos (e.g. "\x03" for Ctrl+C), which
        // would make the encoder emit CSI u sequences (CSI 3;5u) instead of
        // the expected single-byte C0 controls (\x03).
        //
        // In UTF-8, C0 controls and DEL are always single bytes (< 0x20 or
        // 0x7F), so checking each byte is sufficient — multi-byte sequences
        // never contain bytes below 0x80.
        let text = text.and_then(|bytes| {
            if bytes.iter().any(|&b| b < 0x20 || b == 0x7F) {
                None
            } else {
                Some(bytes)
            }
        });
        self.key_event.set_utf8(text);

        let bytes = self
            .key_encoder
            .encode(self.panel.terminal(), &self.key_event);
        if !bytes.is_empty() {
            self.panel.write_input(&bytes);
        }
    }

    /// Encode and send a mouse event. Position is in surface-space pixels.
    /// `button` is `None` for motion events; pass the actual button for
    /// press/release. `mods` is a `GhosttyMods` bitmask.
    pub fn mouse_event(
        &mut self,
        action: tako_term::ffi::GhosttyMouseAction,
        button: Option<tako_term::ffi::GhosttyMouseButton>,
        x_px: f32,
        y_px: f32,
        mods: u16,
    ) {
        self.mouse_event.set_action(action);
        match button {
            Some(b) => self.mouse_event.set_button(b),
            None => self.mouse_event.clear_button(),
        }
        self.mouse_event.set_mods(mods);
        self.mouse_event.set_position(x_px, y_px);
        let bytes = self
            .mouse_encoder
            .encode(self.panel.terminal(), &self.mouse_event);
        if !bytes.is_empty() {
            self.panel.write_input(&bytes);
        }
    }

    /// Tell the encoder whether any button is held (drives any-event motion
    /// dedup). Call on press/release; the encoder does not query this itself.
    pub fn mouse_set_any_button(&mut self, pressed: bool) {
        self.mouse_encoder.set_any_button_pressed(pressed);
    }

    /// Focus gained/lost. No-op when focus reporting (DEC mode 1004) is off.
    pub fn focus_event(&mut self, gained: bool) {
        if self.panel.terminal().mode_get(modes::FOCUS_EVENT) {
            let bytes = vt_input::encode_focus(gained);
            if !bytes.is_empty() {
                self.panel.write_input(&bytes);
            }
        }
    }

    /// Paste bytes into the terminal, wrapping in bracketed paste sequences
    /// when DEC mode 2004 is set.
    pub fn paste(&mut self, data: &[u8]) {
        let bracketed = self.panel.terminal().mode_get(modes::BRACKETED_PASTE);
        let bytes = vt_input::paste_encode(data, bracketed);
        if !bytes.is_empty() {
            self.panel.write_input(&bytes);
        }
    }

    /// Scroll the viewport by `delta_rows` (negative = up into history,
    /// positive = down toward active area).
    pub fn scroll(&mut self, delta_rows: i64) {
        self.panel.scroll(delta_rows);
    }

    /// `true` if any mouse tracking mode is on — drives the report-vs-select
    /// policy in the embedder.
    pub fn mouse_tracking(&self) -> bool {
        self.panel.terminal().mouse_tracking()
    }

    /// Take the latest host-bound window title, if it changed since the last
    /// call. C++ uses this to update the QML window title.
    pub fn take_host_title(&mut self) -> Option<String> {
        self.panel.take_host_title()
    }

    /// Raw fd that becomes readable when PTY output is pending. `None` (as
    /// `-1` via the C ABI) if the readiness pipe couldn't be created.
    pub fn notify_fd(&self) -> Option<std::os::fd::RawFd> {
        self.panel.notify_fd()
    }

    /// Clear pending readiness-wake bytes (call when the embedder's notifier
    /// fires, before [`Self::tick`]).
    pub fn drain_notify(&self) {
        self.panel.drain_notify();
    }

    /// Drain PTY output, advance the terminal, and rebuild the frame plan if
    /// anything changed. Returns the plan and a flag that's `true` when the
    /// plan was actually rebuilt (the embedder should `update()` / re-render)
    /// and `false` when nothing changed (the returned plan is the cached
    /// previous one; the embedder can skip rendering).
    pub fn tick(&mut self) -> (FramePlan, bool) {
        let t0 = Instant::now();

        // Fire autorun once after the configured delay (env-driven perf harness).
        if let Some(a) = &mut self.autorun
            && !a.fired
            && a.start.elapsed() >= a.delay
        {
            a.fired = true;
            log::debug!(
                "autorun firing: {:?}",
                String::from_utf8_lossy(&a.cmd).trim()
            );
            self.panel.write_input(&a.cmd);
        }

        let byte_count = self.panel.pump();
        let t_pump = Instant::now();
        let snap = self.panel.capture_frame();

        let cursor_changed = self.last_cursor.as_ref() != Some(&snap.cursor);

        // Rebuild iff terminal content changed (`snap.dirty`), cursor state
        // changed, or view state changed (`needs_replan` — a DPR/font reload).
        // libghostty-vt can report Dirty::False for cursor-only movement, so
        // cursor is tracked outside the dirty bit to keep shell line editing
        // visually responsive without giving up idle GPU skips.
        if snap.dirty == Dirty::False && !cursor_changed && !self.needs_replan {
            return (self.last_plan, false);
        }

        let t_snap = Instant::now();
        let plan = self.planner.build_plan(&snap);
        let t_plan = Instant::now();
        self.panel.clear_dirty();
        self.needs_replan = false;
        self.last_cursor = Some(snap.cursor.clone());
        self.last_plan = plan;

        let total_us = t_plan.duration_since(t0).as_micros();
        // Log slow frames (>5ms) or frames that ingested a lot of PTY bytes.
        // Both are signals that something is bound to output volume.
        if total_us > 5_000 || byte_count > 4_096 {
            log::debug!(
                "tick total={total_us}µs bytes={byte_count} pump={}µs snap={}µs plan={}µs \
                 verts={}",
                t_pump.duration_since(t0).as_micros(),
                t_snap.duration_since(t_pump).as_micros(),
                t_plan.duration_since(t_snap).as_micros(),
                plan.vertex_count,
            );
        }

        (plan, true)
    }
}

fn build_autorun() -> Option<Autorun> {
    std::env::var("TAKO_AUTORUN").ok().map(|cmd| Autorun {
        start: Instant::now(),
        delay: Duration::from_millis(
            std::env::var("TAKO_AUTORUN_DELAY_MS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(2000),
        ),
        cmd: format!("{cmd}\n").into_bytes(),
        fired: false,
    })
}
