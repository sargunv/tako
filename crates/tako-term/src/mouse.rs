//! Mouse encoder: translates mouse button/motion events into terminal
//! escape sequences. Wraps `ghostty_mouse_encoder_*` +
//! `ghostty_mouse_event_*` (mouse/encoder.h, mouse/event.h).
//!
//! ## Reference
//!
//! `example/c-vt-encode-mouse/src/main.c`.

use crate::ffi;
use crate::terminal::Terminal;

/// Owned `GhosttyMouseEncoder` handle.
pub struct MouseEncoder {
    raw: ffi::GhosttyMouseEncoder,
}

impl MouseEncoder {
    pub fn new() -> Result<Self, crate::Error> {
        let mut raw: ffi::GhosttyMouseEncoder = core::ptr::null_mut();
        let result = unsafe { ffi::ghostty_mouse_encoder_new(core::ptr::null(), &mut raw) };
        crate::Error::from_result(result)?;
        assert!(!raw.is_null(), "ghostty_mouse_encoder_new returned null");
        Ok(Self { raw })
    }

    /// Encode `event` for the given terminal, returning bytes to write to
    /// the PTY. Syncs tracking mode + format from the terminal first.
    /// Returns an empty vec when the encoder produced no output (mouse
    /// reporting off, motion dedup, etc.).
    pub fn encode(&mut self, terminal: &Terminal, event: &MouseEvent) -> Vec<u8> {
        unsafe { ffi::ghostty_mouse_encoder_setopt_from_terminal(self.raw, terminal.as_raw()) };

        let mut buf = [0u8; 64];
        let mut written = 0usize;
        let result = unsafe {
            ffi::ghostty_mouse_encoder_encode(
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
            let mut dynamic = vec![0u8; written];
            let mut written2 = 0usize;
            let result2 = unsafe {
                ffi::ghostty_mouse_encoder_encode(
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
        Vec::new()
    }

    /// Clear motion-dedup state. Call when the surface loses focus or the
    /// mouse leaves it, so the next entry is reported even if it lands on the
    /// same cell as the previous event.
    pub fn reset(&mut self) {
        unsafe { ffi::ghostty_mouse_encoder_reset(self.raw) };
    }

    /// Set the renderer geometry context. Call when cell metrics or surface
    /// pixel size change (resize, DPR change). All units are physical pixels.
    pub fn set_size(
        &mut self,
        screen_w: u32,
        screen_h: u32,
        cell_w: u32,
        cell_h: u32,
        padding: u32,
    ) {
        let mut size: ffi::GhosttyMouseEncoderSize = unsafe { core::mem::zeroed() };
        size.size = core::mem::size_of::<ffi::GhosttyMouseEncoderSize>();
        size.screen_width = screen_w;
        size.screen_height = screen_h;
        size.cell_width = cell_w;
        size.cell_height = cell_h;
        size.padding_top = padding;
        size.padding_bottom = 0;
        size.padding_left = 0;
        size.padding_right = 0;
        unsafe {
            ffi::ghostty_mouse_encoder_setopt(
                self.raw,
                ffi::GhosttyMouseEncoderOption_GHOSTTY_MOUSE_ENCODER_OPT_SIZE,
                &size as *const _ as *const core::ffi::c_void,
            );
        }
    }

    /// Set whether any button is currently held (drives any-event tracking
    /// motion emission policy).
    pub fn set_any_button_pressed(&mut self, pressed: bool) {
        unsafe {
            ffi::ghostty_mouse_encoder_setopt(
                self.raw,
                ffi::GhosttyMouseEncoderOption_GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED,
                &pressed as *const _ as *const core::ffi::c_void,
            );
        }
    }
}

impl Drop for MouseEncoder {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_mouse_encoder_free(self.raw) };
    }
}

/// Owned `GhosttyMouseEvent` handle. Reset and reuse across events.
pub struct MouseEvent {
    raw: ffi::GhosttyMouseEvent,
}

impl MouseEvent {
    pub fn new() -> Result<Self, crate::Error> {
        let mut raw: ffi::GhosttyMouseEvent = core::ptr::null_mut();
        let result = unsafe { ffi::ghostty_mouse_event_new(core::ptr::null(), &mut raw) };
        crate::Error::from_result(result)?;
        assert!(!raw.is_null(), "ghostty_mouse_event_new returned null");
        Ok(Self { raw })
    }

    pub fn set_action(&mut self, action: ffi::GhosttyMouseAction) {
        unsafe { ffi::ghostty_mouse_event_set_action(self.raw, action) };
    }
    pub fn set_button(&mut self, button: ffi::GhosttyMouseButton) {
        unsafe { ffi::ghostty_mouse_event_set_button(self.raw, button) };
    }
    pub fn clear_button(&mut self) {
        unsafe { ffi::ghostty_mouse_event_clear_button(self.raw) };
    }
    pub fn set_mods(&mut self, mods: ffi::GhosttyMods) {
        unsafe { ffi::ghostty_mouse_event_set_mods(self.raw, mods) };
    }
    /// Position in surface-space pixels. Note: callers must Y-flip from Qt's
    /// top-down convention to the bottom-up convention ghostty expects if
    /// needed (ghostty's mouse encoder treats (0,0) as the top-left of the
    /// rendered area, matching Qt, so no flip is required).
    pub fn set_position(&mut self, x: f32, y: f32) {
        unsafe {
            ffi::ghostty_mouse_event_set_position(self.raw, ffi::GhosttyMousePosition { x, y });
        }
    }
}

impl Drop for MouseEvent {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_mouse_event_free(self.raw) };
    }
}

pub use ffi::{
    GhosttyMouseAction, GhosttyMouseButton, GhosttyMouseFormat, GhosttyMouseTrackingMode,
};
