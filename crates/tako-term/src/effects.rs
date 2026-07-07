//! Effects callbacks: outbound side of the libghostty-vt terminal API.
//!
//! `ghostty_terminal_vt_write` invokes registered callbacks synchronously
//! during parsing (terminal.h:1093–1104): the most important is
//! `write_pty`, which receives response bytes for queries like DECRQM, DA1,
//! XTVERSION, focus events, mouse reports, etc. Without it, every query
//! silently drops and interactive programs (vim, tmux, less) malfunction.
//!
//! ## Threading
//!
//! Callbacks fire on whatever thread calls `ghostty_terminal_vt_write`. The
//! `Terminal` itself is single-threaded (not `Send`), so effects and the
//! terminal share one thread by construction.
//!
//! ## Safety
//!
//! Each callback registration stores a `*mut c_void` userdata pointer that
//! libghostty passes back to the trampoline. We use a `Box<TerminalEffects>`
//! leaked via `Box::into_raw` whose pointer is registered as
//! `GHOSTTY_TERMINAL_OPT_USERDATA`. The box is freed when the `Terminal`
//! drops, after `ghostty_terminal_free` makes further callbacks impossible.
//!
//! Re-entrancy: callbacks MUST NOT call `ghostty_terminal_vt_write` on the
//! same terminal (per terminal.h:1103). The trampolines therefore only touch
//! the boxed state, never the terminal handle.

use std::ffi::c_void;

use crate::ffi;

/// Outbound effects from a [`crate::terminal::Terminal`]. Each callback is
/// optional; `None` means libghostty silently ignores the corresponding
/// sequence. The `write_pty` callback is by far the most important — without
/// it, no query responses reach the PTY.
/// Pixel + cell geometry reported to libghostty-vt in response to XTWINOPS
/// size queries (CSI 14/16/18 t).
#[derive(Debug, Clone, Copy, Default)]
pub struct SizeInfo {
    pub cols: u16,
    pub rows: u16,
    pub cell_w_px: u32,
    pub cell_h_px: u32,
}

/// Closure type that returns the current size info. Queried on demand by
/// the `size` effect.
pub type SizeCb = Box<dyn Fn() -> SizeInfo + Send>;

pub struct TerminalEffects {
    pub write_pty: Option<WritePtyCb>,
    pub bell: Option<BellCb>,
    pub title_changed: Option<TitleChangedCb>,
    pub pwd_changed: Option<PwdChangedCb>,
    pub size: Option<SizeCb>,
}

/// Boxed `write_pty` callback.
pub type WritePtyCb = Box<dyn FnMut(&[u8]) + Send>;
/// Boxed `bell` callback.
pub type BellCb = Box<dyn FnMut() + Send>;
/// Boxed `title_changed` callback.
pub type TitleChangedCb = Box<dyn FnMut() + Send>;
/// Boxed `pwd_changed` callback.
pub type PwdChangedCb = Box<dyn FnMut() + Send>;

impl TerminalEffects {
    /// Build an empty effects set; register callbacks via the builder.
    pub fn new() -> Self {
        Self {
            write_pty: None,
            bell: None,
            title_changed: None,
            pwd_changed: None,
            size: None,
        }
    }

    pub fn with_write_pty<F: FnMut(&[u8]) + Send + 'static>(mut self, f: F) -> Self {
        self.write_pty = Some(Box::new(f));
        self
    }
    pub fn with_bell<F: FnMut() + Send + 'static>(mut self, f: F) -> Self {
        self.bell = Some(Box::new(f));
        self
    }
    pub fn with_title_changed<F: FnMut() + Send + 'static>(mut self, f: F) -> Self {
        self.title_changed = Some(Box::new(f));
        self
    }
    pub fn with_pwd_changed<F: FnMut() + Send + 'static>(mut self, f: F) -> Self {
        self.pwd_changed = Some(Box::new(f));
        self
    }
    pub fn with_size<F: Fn() -> SizeInfo + Send + Sync + 'static>(mut self, f: F) -> Self {
        self.size = Some(Box::new(f));
        self
    }
}

impl Default for TerminalEffects {
    fn default() -> Self {
        Self::new()
    }
}

// ---- trampolines ----
//
// `extern "C"` shims that libghostty calls. Each recovers the boxed
// `TerminalEffects` from userdata and dispatches to the matching closure.
// Send-safety: the box is only touched from the terminal's owning thread
// (single-thread ownership of `Terminal`).

unsafe extern "C" fn trampoline_write_pty(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    if userdata.is_null() {
        return;
    }
    let effects = unsafe { &mut *(userdata as *mut TerminalEffects) };
    if let Some(f) = effects.write_pty.as_deref_mut() {
        let slice = if data.is_null() || len == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(data, len) }
        };
        f(slice);
    }
}

unsafe extern "C" fn trampoline_bell(_terminal: ffi::GhosttyTerminal, userdata: *mut c_void) {
    if userdata.is_null() {
        return;
    }
    let effects = unsafe { &mut *(userdata as *mut TerminalEffects) };
    if let Some(f) = effects.bell.as_deref_mut() {
        f();
    }
}

unsafe extern "C" fn trampoline_title_changed(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
) {
    if userdata.is_null() {
        return;
    }
    let effects = unsafe { &mut *(userdata as *mut TerminalEffects) };
    if let Some(f) = effects.title_changed.as_deref_mut() {
        f();
    }
}

unsafe extern "C" fn trampoline_pwd_changed(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
) {
    if userdata.is_null() {
        return;
    }
    let effects = unsafe { &mut *(userdata as *mut TerminalEffects) };
    if let Some(f) = effects.pwd_changed.as_deref_mut() {
        f();
    }
}

/// XTVERSION responder: claims identity `tako <version>`. libghostty wraps
/// the response in `DCS > | ... ST` itself; we only supply the version string.
unsafe extern "C" fn trampoline_xtversion(
    _terminal: ffi::GhosttyTerminal,
    _userdata: *mut c_void,
) -> ffi::GhosttyString {
    // Leak-on-each-call: libghostty copies the bytes immediately (terminal.h
    // docs say "memory must remain valid until the callback returns"). A
    // 'static slice avoids any lifetime plumbing.
    const VERSION: &[u8] = b"tako 0.1.0";
    ffi::GhosttyString {
        ptr: VERSION.as_ptr(),
        len: VERSION.len(),
    }
}

/// ENQ responder: returns the legacy "I am a VT102" reply that xterm sends
/// (per DEC STD 070 § 4.5). Anything else makes some TUIs assume features
/// we don't implement.
unsafe extern "C" fn trampoline_enquiry(
    _terminal: ffi::GhosttyTerminal,
    _userdata: *mut c_void,
) -> ffi::GhosttyString {
    // xterm's response to ENQ.
    const ENQ_RESPONSE: &[u8] = b"\x1b[?1;2c";
    ffi::GhosttyString {
        ptr: ENQ_RESPONSE.as_ptr(),
        len: ENQ_RESPONSE.len(),
    }
}

/// DA1/DA2/DA3 responder: identifies tako as a modern xterm-256color-emulator
/// with VT220 conformance + ANSI color. libghostty builds the response bytes
/// itself and delivers them via write_pty; we just fill in the struct.
unsafe extern "C" fn trampoline_device_attributes(
    _terminal: ffi::GhosttyTerminal,
    _userdata: *mut c_void,
    out: *mut ffi::GhosttyDeviceAttributes,
) -> bool {
    if out.is_null() {
        return false;
    }
    // SAFETY: libghostty allocates `out` and documents it as caller-owned; we
    // fill in the primary DA1 response and secondary DA2 identity.
    let attrs = unsafe { &mut *out };
    // DA1: VT220 conformance + ANSI color (matches what xterm reports).
    attrs.primary.conformance_level = 62; // VT220
    // Set num_features explicitly because bindgen zeroed the struct.
    attrs.primary.num_features = 1;
    attrs.primary.features[0] = 22; // GHOSTTY_DA_FEATURE_ANSI_COLOR
    // DA2: device_type VT220 (1), firmware 1, no ROM cartridge.
    attrs.secondary.device_type = 1;
    attrs.secondary.firmware_version = 1;
    attrs.secondary.rom_cartridge = 0;
    // DA3: arbitrary unit ID (xterm reports 0; we do the same).
    attrs.tertiary.unit_id = 0;
    true
}

/// XTWINOPS size responder: report the terminal's pixel dimensions for
/// `CSI 14 t` (text area px), `CSI 16 t` (cell px), `CSI 18 t` (text area
/// cells). libghostty builds the response bytes itself; we supply geometry.
unsafe extern "C" fn trampoline_size(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    out: *mut ffi::GhosttySizeReportSize,
) -> bool {
    if out.is_null() || userdata.is_null() {
        return false;
    }
    let effects = unsafe { &*(userdata as *const TerminalEffects) };
    let Some(s) = effects.size.as_ref() else {
        return false;
    };
    let info = s();
    // SAFETY: libghostty allocates `out` and documents it as caller-owned.
    let out = unsafe { &mut *out };
    out.columns = info.cols;
    out.rows = info.rows;
    out.cell_width = info.cell_w_px;
    out.cell_height = info.cell_h_px;
    true
}

/// CSI ? 996 n (color scheme report). libghostty builds the response; we
/// just report light/dark. Currently hardcoded dark; TODO: hook to KDE
/// color scheme.
unsafe extern "C" fn trampoline_color_scheme(
    _terminal: ffi::GhosttyTerminal,
    _userdata: *mut c_void,
    out: *mut ffi::GhosttyColorScheme,
) -> bool {
    if out.is_null() {
        return false;
    }
    // SAFETY: writing an enum value to a valid out-pointer.
    unsafe { *out = ffi::GhosttyColorScheme_GHOSTTY_COLOR_SCHEME_DARK };
    true
}

// Keep an FFI symbol reference so dead-code linting doesn't flag the import
// when only a subset of effects is registered.
#[allow(dead_code)]
fn _reference_fns() {
    let _ = ffi::ghostty_terminal_set
        as unsafe extern "C" fn(
            ffi::GhosttyTerminal,
            ffi::GhosttyTerminalOption,
            *const c_void,
        ) -> ffi::GhosttyResult;
}

/// Register callbacks on an already-created terminal. The caller MUST follow
/// this with [`free_effects`] before [`ffi::ghostty_terminal_free`] — typically
/// by stashing the returned pointer inside the owning `Terminal` so its
/// `Drop` can free it.
///
/// # Safety
///
/// `terminal` must be a valid `GhosttyTerminal`. The returned pointer is
/// owned by the caller and must be freed with [`free_effects`] after the
/// terminal itself is destroyed.
pub unsafe fn register(terminal: ffi::GhosttyTerminal, effects: TerminalEffects) -> *mut c_void {
    let boxed = Box::into_raw(Box::new(effects));
    let userdata = boxed as *mut c_void;

    // USERDATA's `InType` is `?*const anyopaque` — the value passed is stored
    // verbatim (NOT dereferenced). So we pass the box pointer directly.
    // For the callback options, InType is the function-pointer type itself;
    // libghostty stores our trampoline function pointer directly.
    unsafe {
        ffi::ghostty_terminal_set(
            terminal,
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_USERDATA,
            userdata,
        );
        ffi::ghostty_terminal_set(
            terminal,
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_WRITE_PTY,
            trampoline_write_pty as *const c_void,
        );
        ffi::ghostty_terminal_set(
            terminal,
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_BELL,
            trampoline_bell as *const c_void,
        );
        ffi::ghostty_terminal_set(
            terminal,
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_TITLE_CHANGED,
            trampoline_title_changed as *const c_void,
        );
        ffi::ghostty_terminal_set(
            terminal,
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_PWD_CHANGED,
            trampoline_pwd_changed as *const c_void,
        );
        ffi::ghostty_terminal_set(
            terminal,
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_XTVERSION,
            trampoline_xtversion as *const c_void,
        );
        ffi::ghostty_terminal_set(
            terminal,
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_ENQUIRY,
            trampoline_enquiry as *const c_void,
        );
        ffi::ghostty_terminal_set(
            terminal,
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES,
            trampoline_device_attributes as *const c_void,
        );
        ffi::ghostty_terminal_set(
            terminal,
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_SIZE,
            trampoline_size as *const c_void,
        );
        ffi::ghostty_terminal_set(
            terminal,
            ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_SCHEME,
            trampoline_color_scheme as *const c_void,
        );
    }
    boxed as *mut c_void
}

/// Free a `TerminalEffects` box returned by [`register`]. Safe on null.
///
/// # Safety
///
/// `ptr` must be either null or a pointer previously returned by [`register`]
/// for a terminal that has already been freed (or is being freed in the same
/// `Drop`). Calling this while the terminal still exists is undefined —
/// libghostty would dereference freed memory on the next `vt_write`.
pub unsafe fn free_effects(ptr: *mut c_void) {
    if !ptr.is_null() {
        unsafe {
            drop(Box::from_raw(ptr as *mut TerminalEffects));
        }
    }
}
