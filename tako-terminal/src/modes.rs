//! Terminal mode packing helpers.
//!
//! `GhosttyMode` is a packed `uint16_t`: bits 0–14 carry the numeric mode
//! value, bit 15 marks ANSI (vs DEC private). The header defines the
//! `GHOSTTY_MODE_*` constants via `#define GHOSTTY_MODE_X (ghostty_mode_new(N, bool))`,
//! which bindgen can't evaluate (it's an inline-fn call), so we mirror them
//! here.
//!
//! Reference: `include/ghostty/vt/modes.h`.

use crate::ffi::GhosttyMode;

/// Pack a DEC private mode value (ANSI bit = 0).
const fn dec(value: u16) -> GhosttyMode {
    value & 0x7FFF
}

/// Pack an ANSI mode value (ANSI bit = 1).
const fn ansi(value: u16) -> GhosttyMode {
    (value & 0x7FFF) | 0x8000
}

// ANSI modes (DEC private bit clear).
pub const KAM: GhosttyMode = ansi(2);
pub const INSERT: GhosttyMode = ansi(4);
pub const SRM: GhosttyMode = ansi(12);
pub const LINEFEED: GhosttyMode = ansi(20);

// DEC private modes.
pub const DECCKM: GhosttyMode = dec(1); // Cursor keys (application cursor mode)
pub const COLUMN_132: GhosttyMode = dec(3);
pub const SLOW_SCROLL: GhosttyMode = dec(4);
pub const REVERSE_COLORS: GhosttyMode = dec(5); // Reverse video
pub const ORIGIN: GhosttyMode = dec(6);
pub const WRAPAROUND: GhosttyMode = dec(7);
pub const AUTOREPEAT: GhosttyMode = dec(8);
pub const X10_MOUSE: GhosttyMode = dec(9);
pub const CURSOR_BLINKING: GhosttyMode = dec(12);
pub const CURSOR_VISIBLE: GhosttyMode = dec(25); // DECTCEM
pub const ENABLE_MODE_3: GhosttyMode = dec(40);
pub const REVERSE_WRAP: GhosttyMode = dec(45);
pub const ALT_SCREEN_LEGACY: GhosttyMode = dec(47);
pub const KEYPAD_KEYS: GhosttyMode = dec(66); // Application keypad
pub const BACKARROW_KEY_MODE: GhosttyMode = dec(67);
pub const LEFT_RIGHT_MARGIN: GhosttyMode = dec(69);
pub const NORMAL_MOUSE: GhosttyMode = dec(1000);
pub const BUTTON_MOUSE: GhosttyMode = dec(1002);
pub const ANY_MOUSE: GhosttyMode = dec(1003);
pub const FOCUS_EVENT: GhosttyMode = dec(1004);
pub const UTF8_MOUSE: GhosttyMode = dec(1005);
pub const SGR_MOUSE: GhosttyMode = dec(1006);
pub const ALT_SCROLL: GhosttyMode = dec(1007); // Wheel in alternate screen
pub const URXVT_MOUSE: GhosttyMode = dec(1015);
pub const SGR_PIXELS_MOUSE: GhosttyMode = dec(1016);
pub const NUMLOCK_KEYPAD: GhosttyMode = dec(1035);
pub const ALT_ESC_PREFIX: GhosttyMode = dec(1036);
pub const ALT_SENDS_ESC: GhosttyMode = dec(1039);
pub const REVERSE_WRAP_EXT: GhosttyMode = dec(1045);
pub const ALT_SCREEN: GhosttyMode = dec(1047);
pub const SAVE_CURSOR: GhosttyMode = dec(1048);
pub const ALT_SCREEN_SAVE: GhosttyMode = dec(1049);
pub const BRACKETED_PASTE: GhosttyMode = dec(2004);
pub const SYNC_OUTPUT: GhosttyMode = dec(2026);
pub const GRAPHEME_CLUSTER: GhosttyMode = dec(2027);
pub const COLOR_SCHEME_REPORT: GhosttyMode = dec(2031);
pub const IN_BAND_RESIZE: GhosttyMode = dec(2048);
