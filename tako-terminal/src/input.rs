//! Misc terminal input/query helpers not covered by the dedicated key/mouse
//! modules.
//!
//! ## Reference
//!
//! `ghostty/vt/terminal.h`.

use crate::ffi;
use crate::terminal::Terminal;

impl Terminal {
    /// `true` iff the named mode is currently set. See [`crate::modes`] for
    /// the mode constants.
    pub fn mode_get(&self, mode: ffi::GhosttyMode) -> bool {
        let mut v: bool = false;
        let result =
            unsafe { ffi::ghostty_terminal_mode_get(self.as_raw(), mode, &mut v as *mut bool) };
        result == ffi::GhosttyResult_GHOSTTY_SUCCESS && v
    }

    /// Move the scrollback viewport by `delta_rows`. Negative values scroll
    /// back into history; positive values scroll toward the active area.
    ///
    /// Production `TerminalView` scrolling is owned by the Zig core, but this
    /// safe wrapper keeps tests and non-Qt terminal consumers from touching the
    /// raw C ABI directly.
    pub fn scroll_viewport_delta(&mut self, delta_rows: isize) {
        let behavior = ffi::GhosttyTerminalScrollViewport {
            tag: ffi::GhosttyTerminalScrollViewportTag_GHOSTTY_SCROLL_VIEWPORT_DELTA,
            value: ffi::GhosttyTerminalScrollViewportValue { delta: delta_rows },
        };
        unsafe { ffi::ghostty_terminal_scroll_viewport(self.as_raw(), behavior) };
    }
}
