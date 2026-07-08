//! Terminal panel: the model-owned terminal core. Holds the libghostty-vt
//! [`Terminal`] + [`RenderState`], the PTY, and the OSC-derived side state
//! (title, cwd). No font, no atlas, no vertices — rendering is [`crate::frame_planner`].
//!
//! Threading: not `Send`. The terminal is single-threaded and effects fire
//! synchronously inside `vt_write` on this thread, so the side-channel state
//! uses `Rc<RefCell<_>>` (not `Arc<Mutex<_>>`).

use std::cell::RefCell;
use std::rc::Rc;

use tako_term::effects::{SizeInfo, TerminalEffects, TerminalIdentity};
use tako_term::pty::StreamingPty;
use tako_term::snapshot::FrameSnapshot;
use tako_term::terminal::{RenderState, Terminal};

use crate::Error;
use crate::font::CellMetrics;

/// Default scrollback cap in rows. ~400 KB for a typical 80-col grid (matches
/// cmux's per-terminal budget). Exposed as a named constant rather than a magic
/// literal at the call site; a future `tako-config` will make it configurable.
pub const DEFAULT_SCROLLBACK: usize = 10_000;

/// State shared between the [`TerminalPanel`] and the effects callbacks
/// (invoked synchronously inside `ghostty_terminal_vt_write`). Each callback
/// only flips a flag or appends to a buffer; the panel drains it on the next
/// [`TerminalPanel::pump`] so the callbacks stay re-entrancy-safe (they never
/// touch the PTY writer mid-`vt_write`).
#[derive(Default)]
struct Shared {
    /// Bytes the `write_pty` effect produced during the most recent
    /// `vt_write` (DA1, XTVERSION, focus, mouse reports, …). Drained by the
    /// next `pump` and forwarded to the PTY.
    pty_response: Vec<u8>,
    /// Set by `title_changed`; `pump` snapshots `terminal.title()` after.
    title_dirty: bool,
    /// Set by `pwd_changed`; `pump` snapshots `terminal.pwd()` after.
    pwd_dirty: bool,
    /// Bell rang since the last `pump` (KNotification bridge pending).
    bell_count: u32,
}

/// Snapshot of the cell metrics + grid size, read by the XTWINOPS size
/// callback (CSI 14/16/18 t). Held in an `Rc<RefCell<_>>` so the callback can
/// query it without borrowing the panel.
#[derive(Clone, Copy)]
struct MetricsSnapshot {
    cols: u16,
    rows: u16,
    cell_w: u32,
    cell_h: u32,
}

/// A live terminal panel: terminal + render state + PTY + OSC state. Drop
/// closes the PTY and frees everything.
pub struct TerminalPanel {
    terminal: Terminal,
    state: RenderState,
    pty: StreamingPty,
    cols: u16,
    rows: u16,
    #[allow(dead_code)]
    scrollback: usize,
    /// Shared with the effects closures registered on the terminal.
    shared: Rc<RefCell<Shared>>,
    /// Read by the XTWINOPS size callback registered on the terminal.
    metrics: Rc<RefCell<MetricsSnapshot>>,
    title: String,
    pwd: String,
    /// Latest window title waiting to be pulled by the host. Set by `pump`
    /// when OSC 0/2 changes the title; drained via [`Self::take_host_title`].
    host_title: Option<String>,
}

impl TerminalPanel {
    /// Spawn a shell on a PTY and wire up OSC effects with the default
    /// scrollback cap ([`DEFAULT_SCROLLBACK`]) and identity.
    pub fn new(cols: u16, rows: u16) -> Result<Self, Error> {
        Self::with_pty_and_scrollback(
            cols,
            rows,
            DEFAULT_SCROLLBACK,
            TerminalIdentity::default(),
            StreamingPty::spawn_shell(cols, rows)?,
        )
    }

    /// Construct a panel around an already-spawned PTY and explicit scrollback
    /// cap + terminal identity. Kept `pub` as a test seam (lets a future test
    /// inject a fake PTY without spawning a shell).
    pub fn with_pty_and_scrollback(
        cols: u16,
        rows: u16,
        scrollback: usize,
        identity: TerminalIdentity,
        pty: StreamingPty,
    ) -> Result<Self, Error> {
        let shared = Rc::new(RefCell::new(Shared::default()));
        let metrics = Rc::new(RefCell::new(MetricsSnapshot {
            cols,
            rows,
            cell_w: 0,
            cell_h: 0,
        }));

        let shared_write = Rc::clone(&shared);
        let shared_bell = Rc::clone(&shared);
        let shared_title = Rc::clone(&shared);
        let shared_pwd = Rc::clone(&shared);
        let metrics_for_size = Rc::clone(&metrics);

        let effects = TerminalEffects::new()
            .with_write_pty(move |bytes: &[u8]| {
                shared_write
                    .borrow_mut()
                    .pty_response
                    .extend_from_slice(bytes);
            })
            .with_bell(move || {
                shared_bell.borrow_mut().bell_count =
                    shared_bell.borrow().bell_count.saturating_add(1);
            })
            .with_title_changed(move || {
                shared_title.borrow_mut().title_dirty = true;
            })
            .with_pwd_changed(move || {
                shared_pwd.borrow_mut().pwd_dirty = true;
            })
            .with_size(move || {
                let m = *metrics_for_size.borrow();
                SizeInfo {
                    cols: m.cols,
                    rows: m.rows,
                    cell_w_px: m.cell_w,
                    cell_h_px: m.cell_h,
                }
            })
            .with_identity(identity);

        let terminal = Terminal::new_with_effects(cols, rows, scrollback, effects)?;
        let state = RenderState::new()?;

        Ok(Self {
            terminal,
            state,
            pty,
            cols,
            rows,
            scrollback,
            shared,
            metrics,
            title: String::new(),
            pwd: String::new(),
            host_title: None,
        })
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }
    pub fn rows(&self) -> u16 {
        self.rows
    }

    /// Borrow the terminal (for input encoders + mode queries).
    pub fn terminal(&self) -> &Terminal {
        &self.terminal
    }

    /// Send bytes (typed input) to the child shell. Logs IO errors instead of
    /// surfacing them: a failing write usually means the child died, which the
    /// next pump observes as EOF — there's nothing actionable for the caller.
    pub fn write_input(&mut self, bytes: &[u8]) {
        if let Err(e) = self.pty.write(bytes) {
            log::warn!("pty write failed: {e}");
        }
    }

    /// Resize the terminal grid + PTY to `cols × rows` with the given cell
    /// metrics (for image-protocol and size-report consumers). Updates the
    /// XTWINOPS size-query snapshot. Logs and continues on partial failure
    /// (matches the prior behavior).
    pub fn resize(&mut self, cols: u16, rows: u16, cell: CellMetrics) {
        let old_cols = self.cols;
        let old_rows = self.rows;
        self.cols = cols;
        self.rows = rows;
        if let Err(e) = self.terminal.resize(cols, rows, cell.width, cell.height) {
            log::warn!("terminal resize {old_cols}x{old_rows} → {cols}x{rows} failed: {e}");
        }
        if let Err(e) = self.pty.resize(cols, rows) {
            log::warn!("pty resize {old_cols}x{old_rows} → {cols}x{rows} failed: {e}");
        }
        *self.metrics.borrow_mut() = MetricsSnapshot {
            cols,
            rows,
            cell_w: cell.width,
            cell_h: cell.height,
        };
    }

    /// Refresh only the cell-metrics side of the XTWINOPS snapshot (after a
    /// DPR change reloads the font but before the grid reflows). The grid
    /// size is carried over unchanged.
    pub fn set_cell_metrics(&mut self, cell: CellMetrics) {
        let mut m = self.metrics.borrow_mut();
        m.cell_w = cell.width;
        m.cell_h = cell.height;
    }

    /// Scroll the viewport by `delta_rows` (negative = up into history).
    pub fn scroll(&mut self, delta_rows: i64) {
        self.terminal
            .scroll_viewport(tako_term::input::Scroll::Delta(delta_rows));
    }

    /// Drain PTY output, advance the terminal, flush PTY-bound response bytes
    /// the effects produced, and snapshot OSC title/pwd. This is the only
    /// method that touches both the PTY buffer and the terminal in one step;
    /// call it once per frame before [`Self::capture_frame`].
    ///
    /// Returns the number of PTY bytes ingested this pump (a useful
    /// diagnostic — bursts of output are the usual slow-frame cause).
    pub fn pump(&mut self) -> usize {
        let bytes = self.pty.drain();
        let byte_count = bytes.len();
        if !bytes.is_empty() {
            self.terminal.vt_write(&bytes);
        }

        // Drain any PTY-bound response bytes the effects callbacks produced
        // during vt_write. Doing it here keeps the callback re-entrancy-safe —
        // it only appends to a buffer.
        let pty_response: Vec<u8> = {
            let mut g = self.shared.borrow_mut();
            std::mem::take(&mut g.pty_response)
        };
        if !pty_response.is_empty()
            && let Err(e) = self.pty.write(&pty_response)
        {
            log::warn!("pty response write failed: {e}");
        }

        // Snapshot title/pwd if the callbacks flagged them. We must read these
        // now, before the next vt_write invalidates the borrow the terminal
        // hands out.
        let (title_dirty, pwd_dirty, bell_count) = {
            let mut g = self.shared.borrow_mut();
            let t = g.title_dirty;
            let p = g.pwd_dirty;
            let b = g.bell_count;
            g.title_dirty = false;
            g.pwd_dirty = false;
            g.bell_count = 0;
            (t, p, b)
        };
        if title_dirty {
            let new = String::from_utf8_lossy(self.terminal.title()).into_owned();
            if new != self.title {
                self.title = new.clone();
                self.host_title = Some(new);
            }
        }
        if pwd_dirty {
            self.pwd = String::from_utf8_lossy(self.terminal.pwd()).into_owned();
        }
        if bell_count > 0 {
            // TODO: KNotification bridge.
        }

        byte_count
    }

    /// Walk the render state into owned Rust data. Clears per-row dirty flags
    /// as it walks; the caller clears the global dirty flag via
    /// [`Self::clear_dirty`] after rendering.
    pub fn capture_frame(&mut self) -> FrameSnapshot {
        FrameSnapshot::capture(&mut self.terminal, &mut self.state)
    }

    /// Clear the global dirty flag after a frame has been drawn.
    pub fn clear_dirty(&mut self) {
        if let Err(e) = self.state.clear_dirty() {
            log::warn!("clear_dirty failed: {e}");
        }
    }

    /// Raw fd that becomes readable when PTY output is pending, for an
    /// event-loop notifier (e.g. Qt `QSocketNotifier`). `None` if the
    /// readiness pipe couldn't be created — fall back to a timer wake.
    pub fn notify_fd(&self) -> Option<std::os::fd::RawFd> {
        self.pty.notify_fd()
    }

    /// `true` once the PTY session has exited. The embedder owns the policy for
    /// what to do next (close tab, quit app, show restart UI, etc.).
    pub fn is_exited(&self) -> bool {
        self.pty.is_exited()
    }

    /// Clear pending readiness-wake bytes. Non-blocking; call when the
    /// notifier fires, before [`Self::pump`].
    pub fn drain_notify(&self) {
        self.pty.drain_notify();
    }

    /// Take the latest host-bound window title, if it changed since the last
    /// call. The host uses this to update the QML window title.
    pub fn take_host_title(&mut self) -> Option<String> {
        self.host_title.take()
    }

    /// Latest OSC title (empty when unset).
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Latest OSC pwd (empty when unset).
    pub fn pwd(&self) -> &str {
        &self.pwd
    }
}
