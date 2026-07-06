//! Safe owning wrappers over the libghostty-vt `Terminal` and `RenderState`
//! handles.
//!
//! Both handles are opaque C pointers that are neither `Send` nor `Sync` —
//! Rust's default rules for raw pointers correctly keep them pinned to the
//! thread that owns them. This is the single-thread ownership model the
//! render-state API documents: only the [`RenderState::update`] call needs
//! exclusive access to the [`Terminal`], and it is short.

use crate::Error;
use crate::ffi;

/// A complete terminal emulator instance: screen, scrollback, cursor, modes,
/// and VT stream parser. Owns the underlying `GhosttyTerminal` handle and
/// frees it on drop.
pub struct Terminal {
    raw: ffi::GhosttyTerminal,
}

/// A render state snapshot of a [`Terminal`]: viewport cells, cursor, colors,
/// and dirty tracking. Owns the underlying `GhosttyRenderState` handle.
pub struct RenderState {
    raw: ffi::GhosttyRenderState,
}

impl Terminal {
    /// Create a new terminal of the given cell dimensions and scrollback cap.
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
        Ok(Self { raw })
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

    /// Crate-private raw handle, for the snapshot walker in [`crate::snapshot`].
    pub(crate) fn as_raw(&self) -> ffi::GhosttyTerminal {
        self.raw
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // SAFETY: we own the handle uniquely and never touch it after drop.
        unsafe { ffi::ghostty_terminal_free(self.raw) };
    }
}

impl RenderState {
    /// Create an empty render state. Populate it via [`Self::update`].
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

    /// Pull terminal changes into the render state and recompute dirty flags.
    /// This is the only call that needs exclusive access to `terminal`.
    pub fn update(&mut self, terminal: &mut Terminal) -> Result<(), Error> {
        let result = unsafe { ffi::ghostty_render_state_update(self.raw, terminal.as_raw()) };
        Error::from_result(result)
    }

    /// Clear the global dirty flag after a frame has been drawn. Per-row dirty
    /// flags are cleared individually by [`crate::snapshot::FrameSnapshot`]
    /// while walking rows.
    pub fn clear_dirty(&mut self) -> Result<(), Error> {
        let value = ffi::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_FALSE;
        let result = unsafe {
            ffi::ghostty_render_state_set(
                self.raw,
                ffi::GhosttyRenderStateOption_GHOSTTY_RENDER_STATE_OPTION_DIRTY,
                &value as *const _ as *const core::ffi::c_void,
            )
        };
        Error::from_result(result)
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
