//! Key encoder: translates a physical key + modifiers into the bytes a
//! terminal program expects. Wraps `ghostty_key_encoder_*` +
//! `ghostty_key_event_*` (key/encoder.h).
//!
//! ## Lifecycle
//!
//! Create one [`KeyEncoder`] per surface; reuse it across keystrokes (it
//! caches mode state). Build a fresh [`KeyEvent`] per press (cheap — the
//! underlying handle is small). [`KeyEncoder::encode`] syncs mode-aware
//! options from the terminal first, so application cursor mode / keypad
//! mode / Kitty protocol flags are always current.
//!
//! ## Reference
//!
//! `example/c-vt-encode-key/src/main.c`.

use crate::ffi;
use crate::terminal::Terminal;

/// Owned `GhosttyKeyEncoder` handle.
pub struct KeyEncoder {
    raw: ffi::GhosttyKeyEncoder,
}

impl KeyEncoder {
    pub fn new() -> Result<Self, crate::Error> {
        let mut raw: ffi::GhosttyKeyEncoder = core::ptr::null_mut();
        // SAFETY: default allocator (NULL). `raw` is an out-param.
        let result = unsafe { ffi::ghostty_key_encoder_new(core::ptr::null(), &mut raw) };
        crate::Error::from_result(result)?;
        assert!(!raw.is_null(), "ghostty_key_encoder_new returned null");
        Ok(Self { raw })
    }

    /// Encode `event` for the given terminal, returning the bytes to write
    /// to the PTY. Syncs mode-aware options from the terminal first.
    ///
    /// Returns `Vec::new()` when the encoder produces no output (e.g. bare
    /// modifier press); callers should not write an empty buffer.
    pub fn encode(&mut self, terminal: &Terminal, event: &KeyEvent) -> Vec<u8> {
        // SAFETY: both handles are valid; sync options are read-only on the
        // terminal side. Re-entrancy rules disallow calling vt_write from
        // callbacks; this path is independent.
        unsafe { ffi::ghostty_key_encoder_setopt_from_terminal(self.raw, terminal.as_raw()) };

        // Static 128-byte buffer covers all real escape sequences; fall back
        // to a dynamic alloc if the encoder overflows (per encoder.h:212–228).
        let mut buf = [0u8; 128];
        let mut written = 0usize;
        let result = unsafe {
            ffi::ghostty_key_encoder_encode(
                self.raw,
                event.raw,
                buf.as_mut_ptr() as _,
                buf.len(),
                &mut written,
            )
        };

        if result == ffi::GhosttyResult_GHOSTTY_SUCCESS {
            return buf[..written].to_vec();
        }
        if result == ffi::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
            // Retry with the required size (encoder.h:212–228).
            let mut dynamic = vec![0u8; written];
            let mut written2 = 0usize;
            let result2 = unsafe {
                ffi::ghostty_key_encoder_encode(
                    self.raw,
                    event.raw,
                    dynamic.as_mut_ptr() as _,
                    dynamic.len(),
                    &mut written2,
                )
            };
            debug_assert_eq!(result2, ffi::GhosttyResult_GHOSTTY_SUCCESS);
            dynamic.truncate(written2);
            return dynamic;
        }
        // Any other error: produce no output.
        Vec::new()
    }

    /// Set macOS option-as-alt policy. No-op on Linux/X11 but cheap to expose.
    pub fn set_option_as_alt(&mut self, mode: ffi::GhosttyOptionAsAlt) {
        let value = mode;
        unsafe {
            ffi::ghostty_key_encoder_setopt(
                self.raw,
                ffi::GhosttyKeyEncoderOption_GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT,
                &value as *const _ as *const core::ffi::c_void,
            );
        }
    }
}

impl Drop for KeyEncoder {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_key_encoder_free(self.raw) };
    }
}

/// Owned `GhosttyKeyEvent` handle. Reset and reuse across presses to avoid
/// per-keystroke allocation.
pub struct KeyEvent {
    raw: ffi::GhosttyKeyEvent,
}

impl KeyEvent {
    pub fn new() -> Result<Self, crate::Error> {
        let mut raw: ffi::GhosttyKeyEvent = core::ptr::null_mut();
        let result = unsafe { ffi::ghostty_key_event_new(core::ptr::null(), &mut raw) };
        crate::Error::from_result(result)?;
        assert!(!raw.is_null(), "ghostty_key_event_new returned null");
        Ok(Self { raw })
    }

    pub fn set_action(&mut self, action: ffi::GhosttyKeyAction) {
        unsafe { ffi::ghostty_key_event_set_action(self.raw, action) };
    }
    pub fn set_key(&mut self, key: ffi::GhosttyKey) {
        unsafe { ffi::ghostty_key_event_set_key(self.raw, key) };
    }
    pub fn set_mods(&mut self, mods: ffi::GhosttyMods) {
        unsafe { ffi::ghostty_key_event_set_mods(self.raw, mods) };
    }
    pub fn set_consumed_mods(&mut self, mods: ffi::GhosttyMods) {
        unsafe { ffi::ghostty_key_event_set_consumed_mods(self.raw, mods) };
    }
    pub fn set_composing(&mut self, composing: bool) {
        unsafe { ffi::ghostty_key_event_set_composing(self.raw, composing) };
    }
    /// Set the UTF-8 text generated by the key. Pass `None` to clear (let the
    /// encoder derive text from the logical key). The encoder docs
    /// (key/event.h:430–440) warn against passing C0 controls or macOS PUA
    /// function-key codes; callers should strip those.
    pub fn set_utf8(&mut self, text: Option<&[u8]>) {
        match text {
            Some(bytes) => unsafe {
                ffi::ghostty_key_event_set_utf8(self.raw, bytes.as_ptr() as _, bytes.len())
            },
            None => unsafe { ffi::ghostty_key_event_set_utf8(self.raw, core::ptr::null(), 0) },
        }
    }
    pub fn set_unshifted_codepoint(&mut self, cp: u32) {
        unsafe { ffi::ghostty_key_event_set_unshifted_codepoint(self.raw, cp) };
    }
}

impl Drop for KeyEvent {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_key_event_free(self.raw) };
    }
}

// Re-export the key/mods constants from bindgen for caller convenience.
pub use ffi::{
    GhosttyKey, GhosttyKeyAction, GhosttyKeyEncoderOption, GhosttyKittyKeyFlags, GhosttyMods,
    GhosttyOptionAsAlt,
};

/// Modifier bit constants (GHOSTTY_MODS_* in key/event.h). Bindgen exposes
/// them as `u32`, but `GhosttyMods` is `u16`; cast at the call site.
pub mod mods {
    pub const SHIFT: u16 = 1 << 0;
    pub const CTRL: u16 = 1 << 1;
    pub const ALT: u16 = 1 << 2;
    pub const SUPER: u16 = 1 << 3;
    pub const CAPS_LOCK: u16 = 1 << 4;
    pub const NUM_LOCK: u16 = 1 << 5;
    pub const SHIFT_SIDE: u16 = 1 << 6;
    pub const CTRL_SIDE: u16 = 1 << 7;
    pub const ALT_SIDE: u16 = 1 << 8;
    pub const SUPER_SIDE: u16 = 1 << 9;
}

/// Kitty keyboard protocol flag constants (GHOSTTY_KITTY_KEY_* in
/// key/encoder.h).
pub mod kitty {
    pub const DISABLED: u8 = 0;
    pub const DISAMBIGUATE: u8 = 1 << 0;
    pub const REPORT_EVENTS: u8 = 1 << 1;
    pub const REPORT_ALTERNATES: u8 = 1 << 2;
    pub const REPORT_ALL: u8 = 1 << 3;
    pub const REPORT_ASSOCIATED: u8 = 1 << 4;
    pub const ALL: u8 =
        DISAMBIGUATE | REPORT_EVENTS | REPORT_ALTERNATES | REPORT_ALL | REPORT_ASSOCIATED;
}
