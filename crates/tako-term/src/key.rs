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

/// Map a [`GhosttyKey`] to its unshifted Unicode codepoint (the character
/// produced by pressing the key with no modifiers on a US layout). Returns 0
/// for keys that have no direct codepoint (function keys, arrows, etc.).
///
/// This mirrors ghostty's internal `key.Key.codepoint()` table
/// (`src/input/key.zig:782`). The embedder should set this on every key event
/// so the encoder can correctly handle Caps Lock + Ctrl combos and derive the
/// right C0 byte when no UTF-8 text is supplied.
pub fn unshifted_codepoint(key: ffi::GhosttyKey) -> u32 {
    // GhosttyKey constants are sequential starting at 0 (UNIDENTIFIED). We
    // match on the named constants to stay robust against enum reordering.
    match key {
        ffi::GhosttyKey_GHOSTTY_KEY_A => b'a' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_B => b'b' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_C => b'c' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_D => b'd' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_E => b'e' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_F => b'f' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_G => b'g' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_H => b'h' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_I => b'i' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_J => b'j' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_K => b'k' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_L => b'l' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_M => b'm' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_N => b'n' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_O => b'o' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_P => b'p' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_Q => b'q' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_R => b'r' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_S => b's' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_T => b't' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_U => b'u' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_V => b'v' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_W => b'w' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_X => b'x' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_Y => b'y' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_Z => b'z' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_DIGIT_0 => b'0' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_DIGIT_1 => b'1' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_DIGIT_2 => b'2' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_DIGIT_3 => b'3' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_DIGIT_4 => b'4' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_DIGIT_5 => b'5' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_DIGIT_6 => b'6' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_DIGIT_7 => b'7' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_DIGIT_8 => b'8' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_DIGIT_9 => b'9' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_SEMICOLON => b';' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_SPACE => b' ' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_QUOTE => b'\'' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_COMMA => b',' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_BACKQUOTE => b'`' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_PERIOD => b'.' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_SLASH => b'/' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_MINUS => b'-' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_EQUAL => b'=' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_BRACKET_LEFT => b'[' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_BRACKET_RIGHT => b']' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_BACKSLASH => b'\\' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_TAB => b'\t' as u32,
        // Numpad keys also map to their face-value codepoint.
        ffi::GhosttyKey_GHOSTTY_KEY_NUMPAD_0 => b'0' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_NUMPAD_1 => b'1' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_NUMPAD_2 => b'2' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_NUMPAD_3 => b'3' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_NUMPAD_4 => b'4' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_NUMPAD_5 => b'5' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_NUMPAD_6 => b'6' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_NUMPAD_7 => b'7' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_NUMPAD_8 => b'8' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_NUMPAD_9 => b'9' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_NUMPAD_DECIMAL => b'.' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_NUMPAD_DIVIDE => b'/' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_NUMPAD_MULTIPLY => b'*' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_NUMPAD_SUBTRACT => b'-' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_NUMPAD_ADD => b'+' as u32,
        ffi::GhosttyKey_GHOSTTY_KEY_NUMPAD_EQUAL => b'=' as u32,
        _ => 0,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Cross-check `mods::*` (hand-mirrored as `u16`) against the bindgen
    /// `#define` constants (exposed as `u32`). Catches a desync if the table
    /// is hand-edited or the ghostty pin is bumped and reorders the bits.
    /// `modes.rs` is not cross-checkable here — its constants come from an
    /// inline-fn macro bindgen can't evaluate, so there is no bindgen
    /// source of truth to compare against.
    #[test]
    fn mods_match_bindgen_constants() {
        assert_eq!(mods::SHIFT as u32, ffi::GHOSTTY_MODS_SHIFT);
        assert_eq!(mods::CTRL as u32, ffi::GHOSTTY_MODS_CTRL);
        assert_eq!(mods::ALT as u32, ffi::GHOSTTY_MODS_ALT);
        assert_eq!(mods::SUPER as u32, ffi::GHOSTTY_MODS_SUPER);
        assert_eq!(mods::CAPS_LOCK as u32, ffi::GHOSTTY_MODS_CAPS_LOCK);
        assert_eq!(mods::NUM_LOCK as u32, ffi::GHOSTTY_MODS_NUM_LOCK);
        assert_eq!(mods::SHIFT_SIDE as u32, ffi::GHOSTTY_MODS_SHIFT_SIDE);
        assert_eq!(mods::CTRL_SIDE as u32, ffi::GHOSTTY_MODS_CTRL_SIDE);
        assert_eq!(mods::ALT_SIDE as u32, ffi::GHOSTTY_MODS_ALT_SIDE);
        assert_eq!(mods::SUPER_SIDE as u32, ffi::GHOSTTY_MODS_SUPER_SIDE);
    }

    /// Verify `unshifted_codepoint` returns lowercase ASCII for letter keys
    /// (the encoder lowercases via this value when Caps Lock is active).
    #[test]
    fn unshifted_codepoint_letters_are_lowercase() {
        assert_eq!(
            unshifted_codepoint(ffi::GhosttyKey_GHOSTTY_KEY_A),
            b'a' as u32
        );
        assert_eq!(
            unshifted_codepoint(ffi::GhosttyKey_GHOSTTY_KEY_C),
            b'c' as u32
        );
        assert_eq!(
            unshifted_codepoint(ffi::GhosttyKey_GHOSTTY_KEY_Z),
            b'z' as u32
        );
    }

    /// Non-printable keys (arrows, function keys, modifiers) return 0 — the
    /// encoder treats 0 as "unset" and falls back to the logical key.
    #[test]
    fn unshifted_codepoint_nonprintable_is_zero() {
        assert_eq!(
            unshifted_codepoint(ffi::GhosttyKey_GHOSTTY_KEY_ARROW_LEFT),
            0
        );
        assert_eq!(unshifted_codepoint(ffi::GhosttyKey_GHOSTTY_KEY_F1), 0);
        assert_eq!(
            unshifted_codepoint(ffi::GhosttyKey_GHOSTTY_KEY_SHIFT_LEFT),
            0
        );
        assert_eq!(
            unshifted_codepoint(ffi::GhosttyKey_GHOSTTY_KEY_UNIDENTIFIED),
            0
        );
    }

    /// Ctrl+C must produce the single C0 byte `\x03`, NOT a CSI u sequence
    /// (`\x1b[3;5u`). This is the core regression test for the control-
    /// sequence-on-input bug: when Qt's `QKeyEvent::text()` returns "\x03"
    /// for Ctrl+C, the embedder strips it (see `Surface::key_event`) and
    /// passes `None` + the unshifted codepoint instead. The encoder then
    /// derives the C0 byte from the logical key.
    #[test]
    fn ctrl_c_produces_c0_byte_not_csi_u() {
        let mut encoder = KeyEncoder::new().expect("encoder");
        let mut event = KeyEvent::new().expect("event");
        let terminal = crate::terminal::Terminal::new(80, 24, 0).expect("terminal");

        event.set_action(ffi::GhosttyKeyAction_GHOSTTY_KEY_ACTION_PRESS);
        event.set_key(ffi::GhosttyKey_GHOSTTY_KEY_C);
        event.set_mods(mods::CTRL);
        event.set_consumed_mods(mods::CTRL);
        // Simulate the stripped text: Qt returns "\x03" but we pass None.
        event.set_utf8(None);
        event.set_unshifted_codepoint(unshifted_codepoint(ffi::GhosttyKey_GHOSTTY_KEY_C));

        let bytes = encoder.encode(&terminal, &event);
        assert_eq!(bytes, &[0x03], "Ctrl+C should produce ETX (\\x03)");
    }

    /// Same regression check for Ctrl+D (EOT = \x04).
    #[test]
    fn ctrl_d_produces_c0_byte_not_csi_u() {
        let mut encoder = KeyEncoder::new().expect("encoder");
        let mut event = KeyEvent::new().expect("event");
        let terminal = crate::terminal::Terminal::new(80, 24, 0).expect("terminal");

        event.set_action(ffi::GhosttyKeyAction_GHOSTTY_KEY_ACTION_PRESS);
        event.set_key(ffi::GhosttyKey_GHOSTTY_KEY_D);
        event.set_mods(mods::CTRL);
        event.set_consumed_mods(mods::CTRL);
        event.set_utf8(None);
        event.set_unshifted_codepoint(unshifted_codepoint(ffi::GhosttyKey_GHOSTTY_KEY_D));

        let bytes = encoder.encode(&terminal, &event);
        assert_eq!(bytes, &[0x04], "Ctrl+D should produce EOT (\\x04)");
    }

    #[test]
    fn arrow_left_defaults_to_normal_cursor_sequence() {
        let mut encoder = KeyEncoder::new().expect("encoder");
        let mut event = KeyEvent::new().expect("event");
        let terminal = crate::terminal::Terminal::new(80, 24, 0).expect("terminal");

        event.set_action(ffi::GhosttyKeyAction_GHOSTTY_KEY_ACTION_PRESS);
        event.set_key(ffi::GhosttyKey_GHOSTTY_KEY_ARROW_LEFT);
        event.set_mods(0);
        event.set_consumed_mods(0);
        event.set_utf8(None);
        event.set_unshifted_codepoint(0);

        let bytes = encoder.encode(&terminal, &event);
        assert_eq!(bytes, b"\x1b[D");
    }

    #[test]
    fn arrow_left_uses_application_cursor_sequence_after_smkx() {
        let mut encoder = KeyEncoder::new().expect("encoder");
        let mut event = KeyEvent::new().expect("event");
        let mut terminal = crate::terminal::Terminal::new(80, 24, 0).expect("terminal");
        terminal.vt_write(b"\x1b[?1h\x1b=");

        event.set_action(ffi::GhosttyKeyAction_GHOSTTY_KEY_ACTION_PRESS);
        event.set_key(ffi::GhosttyKey_GHOSTTY_KEY_ARROW_LEFT);
        event.set_mods(0);
        event.set_consumed_mods(0);
        event.set_utf8(None);
        event.set_unshifted_codepoint(0);

        let bytes = encoder.encode(&terminal, &event);
        assert_eq!(bytes, b"\x1bOD");
    }

    #[test]
    fn ctrl_arrow_left_keeps_ctrl_as_terminal_modifier() {
        let mut encoder = KeyEncoder::new().expect("encoder");
        let mut event = KeyEvent::new().expect("event");
        let terminal = crate::terminal::Terminal::new(80, 24, 0).expect("terminal");

        event.set_action(ffi::GhosttyKeyAction_GHOSTTY_KEY_ACTION_PRESS);
        event.set_key(ffi::GhosttyKey_GHOSTTY_KEY_ARROW_LEFT);
        event.set_mods(mods::CTRL);
        event.set_consumed_mods(0);
        event.set_utf8(None);
        event.set_unshifted_codepoint(0);

        let bytes = encoder.encode(&terminal, &event);
        assert_eq!(bytes, b"\x1b[1;5D");
    }

    #[test]
    fn arrow_left_ignores_num_lock_modifier_for_navigation_sequence() {
        let mut encoder = KeyEncoder::new().expect("encoder");
        let mut event = KeyEvent::new().expect("event");
        let terminal = crate::terminal::Terminal::new(80, 24, 0).expect("terminal");

        event.set_action(ffi::GhosttyKeyAction_GHOSTTY_KEY_ACTION_PRESS);
        event.set_key(ffi::GhosttyKey_GHOSTTY_KEY_ARROW_LEFT);
        event.set_mods(mods::NUM_LOCK);
        event.set_consumed_mods(0);
        event.set_utf8(None);
        event.set_unshifted_codepoint(0);

        let bytes = encoder.encode(&terminal, &event);
        assert_eq!(bytes, b"\x1b[D");
    }

    /// Verify the bug: if we DON'T strip the control char and pass "\x03" as
    /// UTF-8 text, the encoder produces the wrong CSI u sequence. This test
    /// documents the buggy behavior so the fix's rationale is clear.
    #[test]
    fn ctrl_c_with_raw_control_char_produces_csi_u() {
        let mut encoder = KeyEncoder::new().expect("encoder");
        let mut event = KeyEvent::new().expect("event");
        let terminal = crate::terminal::Terminal::new(80, 24, 0).expect("terminal");

        event.set_action(ffi::GhosttyKeyAction_GHOSTTY_KEY_ACTION_PRESS);
        event.set_key(ffi::GhosttyKey_GHOSTTY_KEY_C);
        event.set_mods(mods::CTRL);
        event.set_consumed_mods(mods::CTRL);
        // Pass the raw control char — this is what Qt gives us and what the
        // encoder docs say NOT to do. The output is CSI 3;5u (wrong).
        event.set_utf8(Some(b"\x03"));

        let bytes = encoder.encode(&terminal, &event);
        assert_eq!(
            bytes, b"\x1b[3;5u",
            "raw control char produces CSI u (the bug)"
        );
    }
}
