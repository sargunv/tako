//! Misc input helpers: focus encoding, paste safety/encoding, scrollback
//! navigation. Wraps `ghostty_focus_encode`, `ghostty_paste_*`, and
//! `ghostty_terminal_scroll_viewport`.
//!
//! ## Reference
//!
//! `example/c-vt-encode-focus/src/main.c`, `example/c-vt-paste/src/main.c`.

use crate::ffi;
use crate::terminal::Terminal;

/// Encode a focus gained/lost event into the bytes to write to the PTY
/// (CSI I / CSI O). Caller is responsible for checking that focus reporting
/// (DEC mode 1004) is enabled before writing — see [`crate::modes`].
///
/// Returns `Vec::new()` on encoding failure (shouldn't happen with an
/// 8-byte buffer — the sequences are 3 bytes each).
pub fn encode_focus(gained: bool) -> Vec<u8> {
    let event = if gained {
        ffi::GhosttyFocusEvent_GHOSTTY_FOCUS_GAINED
    } else {
        ffi::GhosttyFocusEvent_GHOSTTY_FOCUS_LOST
    };
    let mut buf = [0u8; 8];
    let mut written = 0usize;
    let result =
        unsafe { ffi::ghostty_focus_encode(event, buf.as_mut_ptr() as _, buf.len(), &mut written) };
    if result == ffi::GhosttyResult_GHOSTTY_SUCCESS {
        buf[..written].to_vec()
    } else {
        Vec::new()
    }
}

/// `true` if `data` is safe to paste without bracketed-paste wrapping:
/// no newlines and no embedded `ESC[201~` (paste-end injection).
pub fn paste_is_safe(data: &[u8]) -> bool {
    if data.is_empty() {
        return true;
    }
    // SAFETY: pointer + len describe a valid borrow.
    unsafe { ffi::ghostty_paste_is_safe(data.as_ptr() as *const core::ffi::c_char, data.len()) }
}

/// Encode `data` for pasting into the terminal. When `bracketed` is true,
/// wraps in `ESC[200~` / `ESC[201~`; in both cases strips unsafe controls
/// (NUL/ESC/DEL→space) and, when not bracketed, replaces newlines with CR.
///
/// Returns the encoded bytes ready to write to the PTY.
pub fn paste_encode(data: &[u8], bracketed: bool) -> Vec<u8> {
    if data.is_empty() {
        // Emit just the bracket wrappers if requested, so a "paste nothing"
        // still toggles bracketed state cleanly.
        if bracketed {
            return b"\x1b[200~\x1b[201~".to_vec();
        }
        return Vec::new();
    }
    // ghostty_paste_encode mutates `data` in place (paste.h:67–70). Copy first.
    let mut scratch = data.to_vec();
    // Probe with a 0-cap buffer to learn the required size.
    let mut written = 0usize;
    let result = unsafe {
        ffi::ghostty_paste_encode(
            scratch.as_mut_ptr() as _,
            scratch.len(),
            bracketed,
            core::ptr::null_mut(),
            0,
            &mut written,
        )
    };
    if result != ffi::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
        // Either succeeded with no extra space needed (only happens with
        // empty input, handled above) or hit a hard error.
        return Vec::new();
    }
    let mut out = vec![0u8; written];
    let mut written2 = 0usize;
    let result2 = unsafe {
        ffi::ghostty_paste_encode(
            scratch.as_mut_ptr() as _,
            scratch.len(),
            bracketed,
            out.as_mut_ptr() as _,
            out.len(),
            &mut written2,
        )
    };
    debug_assert_eq!(result2, ffi::GhosttyResult_GHOSTTY_SUCCESS);
    out.truncate(written2);
    out
}

/// Scroll viewport behaviors for [`Terminal::scroll_viewport`].
pub enum Scroll {
    /// Jump to the top of the scrollback.
    Top,
    /// Jump to the active area (newest output).
    Bottom,
    /// Scroll by `delta` rows; negative = up (older), positive = down (newer).
    Delta(i64),
}

impl Terminal {
    /// Scroll the viewport. Mouse-wheel and keyboard page-up/down should
    /// route here.
    pub fn scroll_viewport(&mut self, behavior: Scroll) {
        let bv = match behavior {
            Scroll::Top => ffi::GhosttyTerminalScrollViewport {
                tag: ffi::GhosttyTerminalScrollViewportTag_GHOSTTY_SCROLL_VIEWPORT_TOP,
                value: ffi::GhosttyTerminalScrollViewportValue { delta: 0 },
            },
            Scroll::Bottom => ffi::GhosttyTerminalScrollViewport {
                tag: ffi::GhosttyTerminalScrollViewportTag_GHOSTTY_SCROLL_VIEWPORT_BOTTOM,
                value: ffi::GhosttyTerminalScrollViewportValue { delta: 0 },
            },
            Scroll::Delta(d) => ffi::GhosttyTerminalScrollViewport {
                tag: ffi::GhosttyTerminalScrollViewportTag_GHOSTTY_SCROLL_VIEWPORT_DELTA,
                value: ffi::GhosttyTerminalScrollViewportValue { delta: d as isize },
            },
        };
        // SAFETY: handle is valid; the value union is read by tag.
        unsafe { ffi::ghostty_terminal_scroll_viewport(self.as_raw(), bv) };
    }

    /// Query whether any mouse tracking mode is on. Drives the
    /// report-vs-select policy in the embedder.
    pub fn mouse_tracking(&self) -> bool {
        let mut v: bool = false;
        let result = unsafe {
            ffi::ghostty_terminal_get(
                self.as_raw(),
                ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING,
                &mut v as *mut bool as *mut core::ffi::c_void,
            )
        };
        result == ffi::GhosttyResult_GHOSTTY_SUCCESS && v
    }

    /// `true` iff the named mode is currently set. See [`crate::modes`] for
    /// the mode constants.
    pub fn mode_get(&self, mode: ffi::GhosttyMode) -> bool {
        let mut v: bool = false;
        let result =
            unsafe { ffi::ghostty_terminal_mode_get(self.as_raw(), mode, &mut v as *mut bool) };
        result == ffi::GhosttyResult_GHOSTTY_SUCCESS && v
    }

    /// Read the terminal's current title (OSC 0/2). Empty when unset. Borrowed
    /// until the next `vt_write` or `reset`.
    pub fn title(&self) -> &[u8] {
        let mut s = ffi::GhosttyString {
            ptr: core::ptr::null_mut(),
            len: 0,
        };
        let result = unsafe {
            ffi::ghostty_terminal_get(
                self.as_raw(),
                ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_TITLE,
                &mut s as *mut _ as *mut core::ffi::c_void,
            )
        };
        if result != ffi::GhosttyResult_GHOSTTY_SUCCESS || s.ptr.is_null() {
            return &[];
        }
        // SAFETY: libghostty documents the borrow as valid until the next
        // mutating terminal call (terminal.h:825–827). We don't mutate here.
        unsafe { std::slice::from_raw_parts(s.ptr, s.len) }
    }

    /// Read the terminal's current pwd (OSC 7/9/1337). Empty when unset.
    /// Borrowed until the next `vt_write` or `reset`.
    pub fn pwd(&self) -> &[u8] {
        let mut s = ffi::GhosttyString {
            ptr: core::ptr::null_mut(),
            len: 0,
        };
        let result = unsafe {
            ffi::ghostty_terminal_get(
                self.as_raw(),
                ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_PWD,
                &mut s as *mut _ as *mut core::ffi::c_void,
            )
        };
        if result != ffi::GhosttyResult_GHOSTTY_SUCCESS || s.ptr.is_null() {
            return &[];
        }
        unsafe { std::slice::from_raw_parts(s.ptr, s.len) }
    }

    /// Viewport-scrollback position info for rendering a scrollbar. Polled
    /// each frame; libghostty has no change notification (terminal.h:793–800).
    pub fn scrollbar(&self) -> Option<Scrollbar> {
        let mut raw: ffi::GhosttyTerminalScrollbar = unsafe { core::mem::zeroed() };
        let result = unsafe {
            ffi::ghostty_terminal_get(
                self.as_raw(),
                ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_SCROLLBAR,
                &mut raw as *mut _ as *mut core::ffi::c_void,
            )
        };
        if result != ffi::GhosttyResult_GHOSTTY_SUCCESS {
            return None;
        }
        Some(Scrollbar {
            total: raw.total,
            offset: raw.offset,
            len: raw.len,
        })
    }

    /// `true` iff the viewport is following the active area (not scrolled
    /// into history).
    pub fn viewport_active(&self) -> bool {
        let mut v: bool = false;
        let result = unsafe {
            ffi::ghostty_terminal_get(
                self.as_raw(),
                ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_VIEWPORT_ACTIVE,
                &mut v as *mut bool as *mut core::ffi::c_void,
            )
        };
        result == ffi::GhosttyResult_GHOSTTY_SUCCESS && v
    }
}

/// Scrollbar geometry, polled from the terminal each frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct Scrollbar {
    pub total: u64,
    pub offset: u64,
    pub len: u64,
}
