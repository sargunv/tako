//! Test-only owning wrappers for libghostty-vt handles, plus raw render-state
//! helpers used while Zig owns the production terminal session.
//!
//! Both handles are opaque C pointers that are neither `Send` nor `Sync` —
//! Rust's default rules for raw pointers correctly keep them pinned to the
//! thread that owns them. Production `TerminalView` owns `GhosttyTerminal` and
//! `GhosttyRenderState` in Zig; Rust only borrows those handles while frame
//! planning is still being migrated.

#[cfg(test)]
use std::ffi::c_void;

use crate::Error;
#[cfg(test)]
use crate::effects::TerminalEffects;
use crate::ffi;

/// A complete terminal emulator instance: screen, scrollback, cursor, modes,
/// and VT stream parser. Owns the underlying `GhosttyTerminal` handle and
/// frees it on drop.
///
/// Test helpers can create this with effects registered; production
/// `TerminalView` owns effects in Zig.
#[cfg(test)]
pub struct Terminal {
    raw: ffi::GhosttyTerminal,
    /// Boxed userdata pointer if effects are registered, null otherwise.
    /// Freed in `Drop` after `ghostty_terminal_free`.
    effects: *mut c_void,
}

#[cfg(test)]
impl Terminal {
    /// Create a new terminal of the given cell dimensions and scrollback cap,
    /// with no effects registered. Most programs (vim, tmux, less) require
    /// query responses — use [`Terminal::new_with_effects`] for those.
    pub fn new(cols: u16, rows: u16, max_scrollback: usize) -> Result<Self, Error> {
        assert!(cols > 0 && rows > 0, "terminal cols/rows must be > 0");
        let opts = ffi::GhosttyTerminalOptions {
            cols,
            rows,
            max_scrollback,
        };
        let mut raw: ffi::GhosttyTerminal = core::ptr::null_mut();
        // SAFETY: default allocator (NULL first arg). `raw` is an uninitialized
        // out-handle; on success the library writes a valid handle into it.
        let result = unsafe {
            ffi::ghostty_terminal_new(
                core::ptr::null(),
                &mut raw as *mut ffi::GhosttyTerminal,
                opts,
            )
        };
        Error::from_result(result)?;
        assert!(
            !raw.is_null(),
            "ghostty_terminal_new returned success but null handle"
        );
        Ok(Self {
            raw,
            effects: core::ptr::null_mut(),
        })
    }

    /// Create a terminal and register the given effects for Rust tests.
    pub fn new_with_effects(
        cols: u16,
        rows: u16,
        max_scrollback: usize,
        effects: TerminalEffects,
    ) -> Result<Self, Error> {
        let mut term = Self::new(cols, rows, max_scrollback)?;
        // SAFETY: term.raw is freshly created and valid. The returned pointer
        // is stored and freed in Drop.
        term.effects = unsafe { crate::effects::register(term.raw, effects) };
        Ok(term)
    }

    /// Feed raw PTY/VT bytes into the terminal's stream parser. This never
    /// fails — malformed input is handled internally (per the C API contract).
    pub fn vt_write(&mut self, data: &[u8]) {
        // SAFETY: `data` is a valid borrow for `data.len()` bytes. The library
        // only reads the slice during the call.
        unsafe { ffi::ghostty_terminal_vt_write(self.raw, data.as_ptr(), data.len()) };
    }

    /// Resize the terminal grid and its pixel dimensions (used by image
    /// protocols and size reports).
    pub fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<(), Error> {
        let result = unsafe {
            ffi::ghostty_terminal_resize(self.raw, cols, rows, cell_width_px, cell_height_px)
        };
        Error::from_result(result)
    }

    /// Full terminal reset (RIS, ESC c).
    pub fn reset(&mut self) {
        unsafe { ffi::ghostty_terminal_reset(self.raw) };
    }

    /// Raw libghostty-vt handle for test snapshot walkers. Production terminal
    /// behavior is owned by the Zig core.
    pub fn as_raw(&self) -> ffi::GhosttyTerminal {
        self.raw
    }
}

#[cfg(test)]
impl Drop for Terminal {
    fn drop(&mut self) {
        // Free the terminal first so no in-flight callbacks can fire after
        // the userdata is freed.
        unsafe { ffi::ghostty_terminal_free(self.raw) };
        if !self.effects.is_null() {
            // SAFETY: after ghostty_terminal_free, no further callbacks fire;
            // the box is uniquely ours.
            unsafe { crate::effects::free_effects(self.effects) };
        }
    }
}

/// A render state snapshot of a libghostty-vt terminal: viewport cells, cursor,
/// colors, and dirty tracking. Owns the underlying `GhosttyRenderState` handle
/// in Rust tests only; production ownership lives in Zig.
pub struct RenderState {
    raw: ffi::GhosttyRenderState,
}

impl RenderState {
    /// Create an empty render state. Populate it via [`Self::update_raw`] in
    /// production, or the test-only [`Self::update`] helper in Rust tests.
    pub fn new() -> Result<Self, Error> {
        let mut raw: ffi::GhosttyRenderState = core::ptr::null_mut();
        let result = unsafe {
            ffi::ghostty_render_state_new(
                core::ptr::null(),
                &mut raw as *mut ffi::GhosttyRenderState,
            )
        };
        Error::from_result(result)?;
        assert!(
            !raw.is_null(),
            "ghostty_render_state_new returned success but null handle"
        );
        Ok(Self { raw })
    }

    /// Pull terminal changes into the render state and recompute dirty flags
    /// from the Rust test terminal helper.
    #[cfg(test)]
    pub fn update(&mut self, terminal: &mut Terminal) -> Result<(), Error> {
        self.update_raw(terminal.as_raw())
    }

    /// Pull terminal changes from a non-Rust-owned terminal handle. This is the
    /// bridge used while Zig owns `GhosttyTerminal` and Rust still owns frame
    /// planning.
    pub fn update_raw(&mut self, terminal: ffi::GhosttyTerminal) -> Result<(), Error> {
        render_state_update_raw(self.raw, terminal)
    }

    /// Test helper: clear the global dirty flag after a frame has been drawn.
    /// Per-row dirty flags are cleared individually by
    /// [`crate::snapshot::FrameSnapshot`] while walking rows.
    pub fn clear_dirty(&mut self) -> Result<(), Error> {
        render_state_clear_dirty_raw(self.raw)
    }

    /// Crate-private raw handle, for the snapshot walker.
    pub(crate) fn as_raw(&self) -> ffi::GhosttyRenderState {
        self.raw
    }
}

impl Default for RenderState {
    fn default() -> Self {
        Self::new().expect("ghostty_render_state_new failed")
    }
}

impl Drop for RenderState {
    fn drop(&mut self) {
        // SAFETY: we own the handle uniquely.
        unsafe { ffi::ghostty_render_state_free(self.raw) };
    }
}

/// Pull terminal changes into a render state owned by the Zig terminal session.
pub(crate) fn render_state_update_raw(
    state: ffi::GhosttyRenderState,
    terminal: ffi::GhosttyTerminal,
) -> Result<(), Error> {
    let result = unsafe { ffi::ghostty_render_state_update(state, terminal) };
    Error::from_result(result)
}

/// Clear the global dirty flag on a Rust test render state. Production dirty
/// clearing lives in Zig, next to the owned `GhosttyRenderState`.
pub(crate) fn render_state_clear_dirty_raw(state: ffi::GhosttyRenderState) -> Result<(), Error> {
    let value = ffi::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_FALSE;
    let result = unsafe {
        ffi::ghostty_render_state_set(
            state,
            ffi::GhosttyRenderStateOption_GHOSTTY_RENDER_STATE_OPTION_DIRTY,
            &value as *const _ as *const core::ffi::c_void,
        )
    };
    Error::from_result(result)
}
