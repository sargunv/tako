//! Live terminal surface: owns the [`Terminal`], [`RenderState`], PTY, font, and
//! glyph atlas, and produces a [`FramePlan`] of ready-to-draw colored glyph
//! quads for the C++ QSG renderer.
//!
//! Threading: the [`Terminal`] is not `Send`, so the Surface (and everything it
//! owns) lives on one thread — the GUI thread that hosts the QQuickItem. The
//! [`StreamingPty`] reader is the only piece on a background thread, and it
//! only touches its own mutex buffer, never the terminal.
//
// The workspace denies `unsafe_code`; this module is the C-FFI boundary and
// scopes the relaxation here only. The raw pointers handed out in a FramePlan
// are borrowed from the Surface and valid only until the next tick.
#![allow(unsafe_code)]
#![allow(unsafe_op_in_unsafe_fn)]

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::ffi::CString;
use std::os::raw::c_char;
use std::process::Command;
use std::ptr;

use tako_term::pty::StreamingPty;
use tako_term::snapshot::FrameSnapshot;
use tako_term::terminal::{RenderState, Terminal};

use crate::atlas::GlyphAtlas;
use crate::font::{CellMetrics, FontFace, ShapedGlyph};

/// One textured quad: destination rect + atlas UV rect + modulate color.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CQuad {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// A flat-colored rect (background or cursor). `a == 0` means "skip".
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// The frame plan handed to the C++ renderer each tick. All pointers are
/// borrowed from the [`Surface`] and valid only until the next
/// [`Surface::tick`] (or the surface's destruction).
#[repr(C)]
pub struct FramePlan {
    /// Full-area background rect (`a == 0` to skip).
    pub bg: CRect,
    /// Cursor rect (`a == 0` if invisible).
    pub cursor: CRect,
    /// Default foreground color (used by the renderer to tint monochrome text).
    pub fg_default: [u8; 4],
    pub cell_w: f32,
    pub cell_h: f32,
    pub cols: u32,
    pub rows: u32,
    pub quads: *const CQuad,
    pub quad_count: usize,
    pub atlas_w: u32,
    pub atlas_h: u32,
    /// Grayscale atlas pixels (`atlas_w * atlas_h` bytes).
    pub atlas_pixels: *const u8,
}

impl Default for FramePlan {
    fn default() -> Self {
        Self {
            bg: CRect::default(),
            cursor: CRect::default(),
            fg_default: [0; 4],
            cell_w: 0.0,
            cell_h: 0.0,
            cols: 0,
            rows: 0,
            quads: ptr::null(),
            quad_count: 0,
            atlas_w: 0,
            atlas_h: 0,
            atlas_pixels: ptr::null(),
        }
    }
}

/// A live terminal surface. Drop closes the PTY and frees everything.
pub struct Surface {
    terminal: Terminal,
    state: RenderState,
    pty: StreamingPty,
    font: FontFace,
    cell: CellMetrics,
    cols: u16,
    rows: u16,
    atlas: GlyphAtlas,
    glyph_advance: HashMap<u32, f32>,
    shape_cache: HashMap<String, Vec<ShapedGlyph>>,
    quads_buf: Vec<CQuad>,
}

impl Surface {
    /// Spawn a shell on a PTY and load `font_path` (or the system default
    /// monospace if `font_path` is `None`) at `pixel_height`.
    pub fn new(
        cols: u16,
        rows: u16,
        font_path: Option<&str>,
        pixel_height: u32,
    ) -> Result<Self, String> {
        let terminal =
            Terminal::new(cols, rows, 10_000).map_err(|e| format!("terminal_new: {e}"))?;
        let state = RenderState::new().map_err(|e| format!("render_state_new: {e}"))?;
        let pty = StreamingPty::spawn_shell(cols, rows).map_err(|e| format!("spawn shell: {e}"))?;

        let path = match font_path {
            Some(p) => p.to_string(),
            None => resolve_default_font()?,
        };
        let font =
            FontFace::from_path(path, pixel_height).map_err(|e| format!("font load: {e}"))?;
        let cell = font.cell_metrics();

        // Empty atlas to start; filled on the first tick.
        let atlas = GlyphAtlas::from_glyph_advances(&font, &HashMap::new());

        Ok(Self {
            terminal,
            state,
            pty,
            font,
            cell,
            cols,
            rows,
            atlas,
            glyph_advance: HashMap::new(),
            shape_cache: HashMap::new(),
            quads_buf: Vec::new(),
        })
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }
    pub fn rows(&self) -> u16 {
        self.rows
    }
    pub fn cell(&self) -> CellMetrics {
        self.cell
    }

    /// Send typed input (keyboard) to the shell.
    pub fn write_input(&mut self, bytes: &[u8]) {
        let _ = self.pty.write(bytes);
    }

    /// Drain PTY output, advance the terminal, and rebuild the frame plan.
    pub fn tick(&mut self) -> FramePlan {
        let bytes = self.pty.drain();
        if !bytes.is_empty() {
            self.terminal.vt_write(&bytes);
        }

        let snap = FrameSnapshot::capture(&mut self.terminal, &mut self.state);
        let plan = self.build_plan(&snap);
        self.state.clear_dirty().ok();
        plan
    }

    fn build_plan(&mut self, snap: &FrameSnapshot) -> FramePlan {
        let CellMetrics {
            width: cw,
            height: ch,
            ascent,
            ..
        } = self.cell;
        let (cw, ch) = (cw as f32, ch as f32);

        // Collect unique graphemes; shape via cache; refresh the atlas if the
        // glyph-id set grew.
        let unique: BTreeSet<String> = snap
            .rows_data
            .iter()
            .flat_map(|r| r.cells.iter())
            .filter(|c| !c.grapheme.is_empty())
            .map(|c| c.grapheme.clone())
            .collect();

        let mut advance: HashMap<u32, f32> = std::mem::take(&mut self.glyph_advance);
        for g in &unique {
            let shaped = self
                .shape_cache
                .entry(g.clone())
                .or_insert_with(|| self.font.shape(g))
                .clone();
            for sg in shaped {
                advance.entry(sg.glyph_id).or_insert(sg.x_advance);
            }
        }
        if advance.len() > self.atlas.glyphs.len() {
            self.atlas = GlyphAtlas::from_glyph_advances(&self.font, &advance);
        }
        self.glyph_advance = advance;

        let atlas_w = self.atlas.width;
        let atlas_h = self.atlas.height;
        let inv_w = if atlas_w > 0 {
            1.0 / atlas_w as f32
        } else {
            0.0
        };
        let inv_h = if atlas_h > 0 {
            1.0 / atlas_h as f32
        } else {
            0.0
        };

        let fg_default = snap.colors.foreground;
        let mut quads: Vec<CQuad> = Vec::new();

        for (row_i, row) in snap.rows_data.iter().enumerate() {
            let row_y = row_i as f32 * ch;
            let baseline = row_y + ascent as f32;
            for (col_i, cell) in row.cells.iter().enumerate() {
                if cell.grapheme.is_empty() {
                    continue;
                }
                let (cr, cg, cb) = match cell.fg {
                    Some(c) => (c.r, c.g, c.b),
                    None => (fg_default.r, fg_default.g, fg_default.b),
                };
                let shaped = self
                    .shape_cache
                    .get(&cell.grapheme)
                    .cloned()
                    .unwrap_or_default();
                let mut pen_x = col_i as f32 * cw;
                for sg in shaped {
                    if let Some(rect) = self.atlas.glyphs.get(&sg.glyph_id) {
                        let qx = pen_x + rect.left_bearing as f32;
                        let qy = baseline - rect.top_bearing as f32;
                        if rect.w > 0 && rect.h > 0 {
                            quads.push(CQuad {
                                x: qx,
                                y: qy,
                                w: rect.w as f32,
                                h: rect.h as f32,
                                u0: rect.x as f32 * inv_w,
                                v0: rect.y as f32 * inv_h,
                                u1: (rect.x + rect.w) as f32 * inv_w,
                                v1: (rect.y + rect.h) as f32 * inv_h,
                                r: cr,
                                g: cg,
                                b: cb,
                                a: 255,
                            });
                        }
                        pen_x += sg.x_advance;
                    }
                }
            }
        }

        self.quads_buf = quads;

        // Background: full area, terminal bg color.
        let bg = CRect {
            x: 0.0,
            y: 0.0,
            w: self.cols as f32 * cw,
            h: self.rows as f32 * ch,
            r: snap.colors.background.r,
            g: snap.colors.background.g,
            b: snap.colors.background.b,
            a: 255,
        };

        // Cursor.
        let cursor = self.build_cursor(snap, cw, ch);

        FramePlan {
            bg,
            cursor,
            fg_default: [fg_default.r, fg_default.g, fg_default.b, 255],
            cell_w: cw,
            cell_h: ch,
            cols: self.cols as u32,
            rows: self.rows as u32,
            quads: self.quads_buf.as_ptr(),
            quad_count: self.quads_buf.len(),
            atlas_w,
            atlas_h,
            atlas_pixels: if atlas_w * atlas_h > 0 {
                self.atlas.pixels.as_ptr()
            } else {
                ptr::null()
            },
        }
    }

    fn build_cursor(&self, snap: &FrameSnapshot, cw: f32, ch: f32) -> CRect {
        let Some((cx, cy)) = snap.cursor.viewport else {
            return CRect::default();
        };
        if !snap.cursor.visible {
            return CRect::default();
        }
        let color = snap.colors.cursor.unwrap_or(snap.colors.foreground);
        CRect {
            x: cx as f32 * cw,
            y: cy as f32 * ch,
            w: cw,
            h: ch,
            r: color.r,
            g: color.g,
            b: color.b,
            a: 255,
        }
    }
}

/// Resolve the system default monospace font path via fontconfig (`fc-match`).
fn resolve_default_font() -> Result<String, String> {
    let out = Command::new("fc-match")
        .args(["-f", "%{file}", "monospace"])
        .output()
        .map_err(|e| format!("fc-match failed: {e}"))?;
    if !out.status.success() {
        return Err("fc-match returned non-zero".to_string());
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        return Err("fc-match returned empty path".to_string());
    }
    Ok(path)
}

// ---- C ABI surface for the C++ QQuickItem renderer ----
// Pointers are borrowed from the Surface and valid only across a single tick
// (C++ copies into QSG geometry before the next call).

/// Spawn a surface. `font_path` may be null to use the system default mono font.
///
/// Returns an opaque heap pointer on success, or null on failure (logged).
///
/// # Safety
///
/// `font_path` must be a valid NUL-terminated C string if non-null. The caller
/// owns the returned pointer and must free it with [`tako_surface_destroy`].
/// The surface is not thread-safe and must be used from one thread only.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_new(
    cols: u16,
    rows: u16,
    font_path: *const c_char,
    pixel_height: u32,
) -> *mut Surface {
    let font = if font_path.is_null() {
        None
    } else {
        Some(
            std::ffi::CStr::from_ptr(font_path)
                .to_string_lossy()
                .into_owned(),
        )
    };
    match Surface::new(cols, rows, font.as_deref(), pixel_height) {
        Ok(s) => Box::into_raw(Box::new(s)),
        Err(e) => {
            eprintln!("tako_surface_new failed: {e}");
            ptr::null_mut()
        }
    }
}

/// Free a surface returned by [`tako_surface_new`]. No-op on null.
///
/// # Safety
///
/// `s` must be either null or a pointer previously returned by
/// [`tako_surface_new`] that has not already been freed. After this call the
/// pointer is invalid and must not be used.
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
///
/// `s` must be a valid [`Surface`] pointer from [`tako_surface_new`]. `out`
/// must point to writable memory the caller owns; it is overwritten. The
/// caller must not read the borrowed pointers in `*out` after the next call
/// to `tako_surface_tick` or after [`tako_surface_destroy`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_tick(s: *mut Surface, out: *mut FramePlan) {
    if s.is_null() || out.is_null() {
        return;
    }
    let surface = unsafe { &mut *s };
    let plan = surface.tick();
    unsafe { *out = plan };
}

/// Send typed input bytes to the shell.
///
/// # Safety
///
/// `s` must be a valid [`Surface`] pointer from [`tako_surface_new`]. `data`
/// must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_write(s: *mut Surface, data: *const u8, len: usize) {
    if s.is_null() || data.is_null() {
        return;
    }
    let surface = unsafe { &mut *s };
    let slice = unsafe { std::slice::from_raw_parts(data, len) };
    surface.write_input(slice);
}

// Keep CString reachable for the FFI doc; avoids dead-code churn if unused.
#[allow(dead_code)]
fn _cstring_marker(s: &str) -> CString {
    CString::new(s).unwrap()
}
