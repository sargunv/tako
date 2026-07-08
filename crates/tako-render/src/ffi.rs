//! The C ABI surface for the C++ `TakoTerminalView` / `TakoTerminalRenderer`.
//!
//! Every `extern "C"` entry point lives here so the cross-boundary contract has
//! a single home and the `unsafe` is scoped to this one module. The hand-
//! written C++ `QQuickFramebufferObject` subclass (see `cpp/`) calls these; the
//! safe-ish Rust API on [`Surface`](crate::Surface) / [`GlRenderer`](crate::GlRenderer)
//! stays free of null-checking boilerplate.
//!
//! Pointers handed back inside a [`FramePlan`] are borrowed from the planner
//! and valid only until the next tick (C++ copies into QSG geometry before the
//! next call).

// The workspace denies `unsafe_code`; this module IS the C-FFI boundary.
#![allow(unsafe_code)]
#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::c_void;
use std::os::raw::c_char;

use crate::frame_planner::FramePlan;
use crate::gl_renderer::{GlRenderer, LoaderFn};
use crate::surface::Surface;

/// Dereference a borrowed raw pointer, or `None` if null. The lifetime is
/// unbounded so callers can hand the reference to a method that borrows
/// `&mut self` next; correctness rests on the C++ caller not retaining the
/// reference across a `*_tick`/`*_destroy` call.
///
/// # Safety
/// `p` must be null or a valid, properly-aligned pointer to a live `T` that
/// the caller owns for the duration of the returned borrow.
unsafe fn deref<'a, T>(p: *mut T) -> Option<&'a mut T> {
    if p.is_null() {
        None
    } else {
        Some(unsafe { &mut *p })
    }
}

// ---- Surface lifecycle ----

/// Spawn a surface. `font_path` may be null to use the system default mono
/// font. `pixel_height` is the logical (DIP) cell height; the font is
/// rasterized at `dpr × pixel_height` physical pixels for hidpi sharpness.
///
/// Returns an opaque heap pointer on success, or null on failure (logged).
///
/// # Safety
/// `font_path` must be a valid NUL-terminated C string if non-null. The caller
/// owns the returned pointer and must free it with [`tako_surface_destroy`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_new(
    cols: u16,
    rows: u16,
    font_path: *const c_char,
    pixel_height: u32,
    dpr: f32,
) -> *mut Surface {
    let font = if font_path.is_null() {
        None
    } else {
        Some(
            unsafe { std::ffi::CStr::from_ptr(font_path) }
                .to_string_lossy()
                .into_owned(),
        )
    };
    match Surface::new(cols, rows, font.as_deref(), pixel_height, dpr) {
        Ok(s) => Box::into_raw(Box::new(s)),
        Err(e) => {
            log::error!("tako_surface_new failed: {e}");
            std::ptr::null_mut()
        }
    }
}

/// Free a surface returned by [`tako_surface_new`]. No-op on null.
///
/// # Safety
/// `s` must be either null or a pointer previously returned by
/// [`tako_surface_new`] that has not already been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_destroy(s: *mut Surface) {
    if !s.is_null() {
        drop(unsafe { Box::from_raw(s) });
    }
}

/// Rebuild the frame plan and write it into `*out`. The pointers inside `out`
/// are valid until the next `tako_surface_tick` or `tako_surface_destroy`.
///
/// # Safety
/// `s` must be a valid [`Surface`] pointer. `out` must point to writable memory
/// the caller owns; the borrowed pointers inside `*out` are invalid after the
/// next `tako_surface_tick` or [`tako_surface_destroy`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_tick(s: *mut Surface, out: *mut FramePlan) {
    let Some(surface) = (unsafe { deref(s) }) else {
        return;
    };
    if out.is_null() {
        return;
    }
    let plan = surface.tick();
    unsafe { *out = plan };
}

// ---- Surface: sizing & DPR ----

/// Resize the terminal grid to fit `width_px × height_px` physical pixels.
/// Safe to call with the current size (no-op).
///
/// # Safety
/// `s` must be a valid [`Surface`] pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_resize_pixels(
    s: *mut Surface,
    width_px: u32,
    height_px: u32,
) {
    let Some(surface) = (unsafe { deref(s) }) else {
        return;
    };
    surface.resize_to_pixels(width_px, height_px);
}

/// Reload the font at a new device-pixel ratio and invalidate size-dependent
/// caches. The caller should follow with [`tako_surface_resize_pixels`] using
/// the current physical item size so the grid reflows. No-op when `dpr` is
/// within 0.01 of the current value.
///
/// # Safety
/// `s` must be a valid [`Surface`] pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_set_dpr(s: *mut Surface, dpr: f32) {
    let Some(surface) = (unsafe { deref(s) }) else {
        return;
    };
    surface.set_dpr(dpr);
}

// ---- Surface: I/O ----

/// Send typed input bytes to the shell.
///
/// # Safety
/// `s` must be a valid [`Surface`] pointer. `data` must point to `len` readable
/// bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_write(s: *mut Surface, data: *const u8, len: usize) {
    let Some(surface) = (unsafe { deref(s) }) else {
        return;
    };
    if data.is_null() {
        return;
    }
    let slice = unsafe { std::slice::from_raw_parts(data, len) };
    surface.write_input(slice);
}

/// Paste bytes into the terminal (with bracketed paste wrapping if DEC mode
/// 2004 is set).
///
/// # Safety
/// `s` must be a valid [`Surface`] pointer. `data` must point to `len`
/// readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_paste(s: *mut Surface, data: *const u8, len: usize) {
    let Some(surface) = (unsafe { deref(s) }) else {
        return;
    };
    if data.is_null() {
        return;
    }
    let slice = unsafe { std::slice::from_raw_parts(data, len) };
    surface.paste(slice);
}

/// Scroll the viewport by `delta_rows` (negative = up into history).
///
/// # Safety
/// `s` must be a valid [`Surface`] pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_scroll(s: *mut Surface, delta_rows: i64) {
    let Some(surface) = (unsafe { deref(s) }) else {
        return;
    };
    surface.scroll(delta_rows);
}

/// Returns 1 if any mouse tracking mode is on (embedder should forward mouse
/// events to the PTY rather than do selection), 0 otherwise.
///
/// # Safety
/// `s` must be a valid [`Surface`] pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_mouse_tracking(s: *mut Surface) -> i32 {
    if s.is_null() {
        return 0;
    }
    // SAFETY: checked non-null above; shared borrow only, no mutation.
    let surface = unsafe { &*s };
    i32::from(surface.mouse_tracking())
}

/// Take the latest window title, if it changed. Returns the length written
/// into `out_buf` (excluding NUL). Returns 0 when there is no new title or
/// `out_buf` is too small; the caller should pass a buffer of at least 256 B.
/// A NUL terminator is written after the title bytes.
///
/// # Safety
/// `s` must be a valid [`Surface`] pointer. `out_buf` must point to `cap`
/// writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_take_title(
    s: *mut Surface,
    out_buf: *mut u8,
    cap: usize,
) -> usize {
    let Some(surface) = (unsafe { deref(s) }) else {
        return 0;
    };
    if out_buf.is_null() || cap == 0 {
        return 0;
    }
    let Some(title) = surface.take_host_title() else {
        return 0;
    };
    let bytes = title.as_bytes();
    if bytes.len() + 1 > cap {
        return 0;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), out_buf, bytes.len());
        *out_buf.add(bytes.len()) = 0;
    }
    bytes.len()
}

// ---- Surface: input encoders ----
//
// The C ABI passes libghostty-vt enum values directly (defined in
// `tako_term::ffi`). C++ includes the matching C headers via
// `<ghostty/vt/key/event.h>` and `<ghostty/vt/mouse/event.h>`.

/// Encode and forward a key event to the PTY.
///
/// # Safety
/// `s` must be a valid [`Surface`] pointer. If `text_len > 0`, `text` must
/// point to `text_len` valid UTF-8 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_key_event(
    s: *mut Surface,
    action: u32,
    key: u32,
    mods: u16,
    consumed_mods: u16,
    text: *const u8,
    text_len: usize,
) {
    let Some(surface) = (unsafe { deref(s) }) else {
        return;
    };
    let text = if text.is_null() || text_len == 0 {
        None
    } else {
        Some(unsafe { std::slice::from_raw_parts(text, text_len) })
    };
    surface.key_event(action, key, mods, consumed_mods, text);
}

/// Encode and forward a mouse event to the PTY. `button` 0 = UNKNOWN (use for
/// motion); mouse tracking is checked inside the encoder.
///
/// # Safety
/// `s` must be a valid [`Surface`] pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_mouse_event(
    s: *mut Surface,
    action: u32,
    button: u32,
    x_px: f32,
    y_px: f32,
    mods: u16,
) {
    let Some(surface) = (unsafe { deref(s) }) else {
        return;
    };
    let button = if button == 0 { None } else { Some(button) };
    surface.mouse_event(action, button, x_px, y_px, mods);
}

/// Tell the surface whether any mouse button is currently held (drives
/// any-event motion reporting).
///
/// # Safety
/// `s` must be a valid [`Surface`] pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_mouse_set_any_button(s: *mut Surface, pressed: bool) {
    let Some(surface) = (unsafe { deref(s) }) else {
        return;
    };
    surface.mouse_set_any_button(pressed);
}

/// Focus gained/lost. Forwards focus-reporting bytes to the PTY iff DEC mode
/// 1004 is set.
///
/// # Safety
/// `s` must be a valid [`Surface`] pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_focus_event(s: *mut Surface, gained: bool) {
    let Some(surface) = (unsafe { deref(s) }) else {
        return;
    };
    surface.focus_event(gained);
}

// ---- GlRenderer lifecycle + render ----

/// Construct a renderer without a GL context. Safe to call from the GUI
/// thread. Use [`tako_gl_renderer_ensure_gl`] on the render thread to attach
/// it to Qt's GL context.
///
/// # Safety
/// The caller owns the returned pointer and must free it with
/// [`tako_gl_renderer_destroy`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_gl_renderer_new() -> *mut GlRenderer {
    Box::into_raw(Box::new(GlRenderer::new()))
}

/// Free a renderer returned by [`tako_gl_renderer_new`]. No-op on null.
///
/// # Safety
/// `r` must be null or a pointer previously returned by
/// [`tako_gl_renderer_new`], not already freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_gl_renderer_destroy(r: *mut GlRenderer) {
    if !r.is_null() {
        drop(unsafe { Box::from_raw(r) });
    }
}

/// Attach the renderer to the current thread's GL context. Idempotent. Must
/// run on the render thread with Qt's `QOpenGLContext` current.
///
/// # Safety
/// `r` must be a valid [`GlRenderer`] pointer. `loader` must resolve symbols
/// against the current GL context; `loader_userdata` is passed through
/// verbatim (typically the `QOpenGLContext*`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_gl_renderer_ensure_gl(
    r: *mut GlRenderer,
    loader: LoaderFn,
    loader_userdata: *mut c_void,
) {
    let Some(renderer) = (unsafe { deref(r) }) else {
        return;
    };
    unsafe { renderer.ensure_gl(loader, loader_userdata) };
}

/// Copy a [`FramePlan`]'s borrowed data into the renderer's staging buffers.
/// GUI thread. Must run before [`tako_gl_renderer_render`] each frame.
///
/// # Safety
/// `r` must be a valid [`GlRenderer`] pointer. `plan` must point to a valid
/// [`FramePlan`] whose borrowed pointers are still live (i.e. before the next
/// `tako_surface_tick`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_gl_renderer_ingest_plan(
    r: *mut GlRenderer,
    plan: *const FramePlan,
    viewport_w: i32,
    viewport_h: i32,
) {
    let Some(renderer) = (unsafe { deref(r) }) else {
        return;
    };
    if plan.is_null() {
        return;
    }
    let plan = unsafe { &*plan };
    unsafe { renderer.ingest_plan(plan, viewport_w, viewport_h) };
}

/// Draw the latest staging data. Render thread, GL context current.
///
/// # Safety
/// `r` must be a valid [`GlRenderer`] pointer that has been attached to a GL
/// context via [`tako_gl_renderer_ensure_gl`]. The GL context must be current
/// on the calling thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_gl_renderer_render(r: *mut GlRenderer) {
    let Some(renderer) = (unsafe { deref(r) }) else {
        return;
    };
    renderer.render();
}
