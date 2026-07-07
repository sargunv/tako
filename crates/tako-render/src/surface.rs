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
use std::time::{Duration, Instant};

use tako_term::pty::StreamingPty;
use tako_term::snapshot::FrameSnapshot;
use tako_term::terminal::{RenderState, Terminal};

use crate::atlas::GlyphAtlas;
use crate::font::{CellMetrics, FontFace, GlyphBitmap, ShapedGlyph};

/// One vertex of a textured quad: pixel-space position, atlas UV, and a
/// modulate color. The renderer uploads these verbatim into a VBO and draws
/// with a single shader that multiplies the atlas coverage by the color.
///
/// Layout (20 bytes, matched in `gl_renderer.rs`'s vertex-attrib setup):
/// `{ x: f32, y: f32, u: f32, v: f32, r/g/b/a: u8 }`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
    pub u: f32,
    pub v: f32,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// The render plan handed to the GL renderer each tick. All pointers are
/// borrowed from the [`Surface`] and valid only until the next
/// [`Surface::tick`] (or the surface's destruction); the renderer deep-copies
/// them in its `synchronize()` step.
///
/// `vertices` is one flat buffer of glyph + cursor quad vertices in draw order
/// (cursor last, so it layers over glyphs). Background-cell quads land in P2.
#[repr(C)]
pub struct FramePlan {
    /// FBO clear color (terminal default background).
    pub clear_color: [u8; 4],
    pub cell_w: f32,
    pub cell_h: f32,
    pub cols: u32,
    pub rows: u32,
    pub vertices: *const Vertex,
    pub vertex_count: usize,
    pub atlas_w: u32,
    pub atlas_h: u32,
    /// Grayscale atlas pixels (`atlas_w * atlas_h` bytes).
    pub atlas_pixels: *const u8,
    /// Bumped whenever the atlas is rebuilt, even if dimensions are unchanged
    /// (shelf-pack reuses space within the same canvas). The renderer
    /// re-uploads the texture whenever this changes.
    pub atlas_generation: u64,
}

impl Default for FramePlan {
    fn default() -> Self {
        Self {
            clear_color: [0; 4],
            cell_w: 0.0,
            cell_h: 0.0,
            cols: 0,
            rows: 0,
            vertices: ptr::null(),
            vertex_count: 0,
            atlas_w: 0,
            atlas_h: 0,
            atlas_pixels: ptr::null(),
            atlas_generation: 0,
        }
    }
}

/// Optional one-shot command injected into the PTY shortly after spawn. Driven
/// by `TAKO_AUTORUN` (command) + `TAKO_AUTORUN_DELAY_MS` env vars for hands-off
/// perf testing (lets the shell's slow zshrc finish before we inject).
struct Autorun {
    start: Instant,
    delay: Duration,
    cmd: Vec<u8>,
    fired: bool,
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
    /// Bumped every time `atlas` is reassigned, so the renderer can detect
    /// content changes that don't alter dimensions (shelf-pack repacking).
    atlas_generation: u64,
    glyph_advance: HashMap<u32, f32>,
    /// Rasterize-once cache keyed by glyph id, shared across atlas rebuilds so
    /// FreeType never rasterizes the same glyph twice.
    raster_cache: HashMap<u32, GlyphBitmap>,
    shape_cache: HashMap<String, Vec<ShapedGlyph>>,
    vertex_buf: Vec<Vertex>,
    autorun: Option<Autorun>,
    /// Resolved font file path (kept so the font can be reloaded on DPR change).
    font_path: String,
    /// Logical (DIP) font size the user requested. The actual rasterized size
    /// is `logical_pixel_height × dpr` so glyphs stay crisp on hidpi displays.
    logical_pixel_height: u32,
    /// Device pixel ratio of the screen hosting this surface. Changing it (via
    /// [`Surface::set_dpr`]) reloads the font at the new physical size and
    /// invalidates size-dependent caches.
    dpr: f32,
}

impl Surface {
    /// Spawn a shell on a PTY and load `font_path` (or the system default
    /// monospace if `font_path` is `None`) at the logical `pixel_height`,
    /// rasterized at `dpr × pixel_height` physical pixels for hidpi sharpness.
    /// `dpr` is the device pixel ratio of the screen hosting this surface;
    /// change it later via [`Surface::set_dpr`].
    pub fn new(
        cols: u16,
        rows: u16,
        font_path: Option<&str>,
        logical_pixel_height: u32,
        dpr: f32,
    ) -> Result<Self, String> {
        let terminal =
            Terminal::new(cols, rows, 10_000).map_err(|e| format!("terminal_new: {e}"))?;
        let state = RenderState::new().map_err(|e| format!("render_state_new: {e}"))?;
        let pty = StreamingPty::spawn_shell(cols, rows).map_err(|e| format!("spawn shell: {e}"))?;

        let path = match font_path {
            Some(p) => p.to_string(),
            None => resolve_default_font()?,
        };
        let physical_px = physical_font_size(logical_pixel_height, dpr);
        let font =
            FontFace::from_path(&path, physical_px).map_err(|e| format!("font load: {e}"))?;
        let cell = font.cell_metrics();

        // Empty atlas to start; filled on the first tick. raster_cache is
        // declared alongside so we can move it into Self below.
        let mut raster_cache: HashMap<u32, GlyphBitmap> = HashMap::new();
        let atlas = GlyphAtlas::from_glyph_advances(&font, &HashMap::new(), &mut raster_cache);

        let autorun = std::env::var("TAKO_AUTORUN").ok().map(|cmd| Autorun {
            start: Instant::now(),
            delay: Duration::from_millis(
                std::env::var("TAKO_AUTORUN_DELAY_MS")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(2000),
            ),
            cmd: format!("{cmd}\n").into_bytes(),
            fired: false,
        });

        Ok(Self {
            terminal,
            state,
            pty,
            font,
            cell,
            cols,
            rows,
            atlas,
            atlas_generation: 0,
            glyph_advance: HashMap::new(),
            raster_cache,
            shape_cache: HashMap::new(),
            vertex_buf: Vec::new(),
            autorun,
            font_path: path,
            logical_pixel_height,
            dpr,
        })
    }

    /// Reload the font at a new device-pixel ratio, invalidating all
    /// size-dependent caches. The caller should follow this with
    /// [`Surface::resize_to_pixels`] (passing the current physical item size)
    /// so the grid reflows to the new cell metrics.
    ///
    /// No-op when `dpr` is within 0.01 of the current value.
    pub fn set_dpr(&mut self, dpr: f32) {
        if (dpr - self.dpr).abs() < 0.01 {
            return;
        }
        self.dpr = dpr;
        let physical_px = physical_font_size(self.logical_pixel_height, dpr);
        match FontFace::from_path(&self.font_path, physical_px) {
            Ok(font) => {
                self.font = font;
                self.cell = self.font.cell_metrics();
            }
            Err(e) => {
                eprintln!("[surface] font reload at dpr {dpr} (phys {physical_px}) failed: {e}");
                return;
            }
        }
        // Invalidate all size-dependent caches: glyph x_advance (scaled by px),
        // rasterized bitmaps, shaped advances, the packed atlas.
        self.glyph_advance.clear();
        self.raster_cache.clear();
        self.shape_cache.clear();
        self.vertex_buf.clear();
        self.atlas =
            GlyphAtlas::from_glyph_advances(&self.font, &HashMap::new(), &mut self.raster_cache);
        self.atlas_generation = self.atlas_generation.wrapping_add(1);
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

    /// Resize the terminal grid to fit `width_px × height_px` (in current
    /// cell-metric units; DIPs pre-P4, physical px post-P4). Computes cols/rows
    /// from the cell metrics, resizes both the libghostty-vt terminal and the
    /// PTY (so the child sees `SIGWINCH`), and updates internal fields. No-op
    /// if the computed size matches the current one (avoids resize storms on
    /// sub-cell window motion during drag).
    pub fn resize_to_pixels(&mut self, width_px: u32, height_px: u32) {
        let cw = self.cell.width.max(1);
        let ch = self.cell.height.max(1);
        let cols = ((width_px / cw).max(1)).min(u16::MAX as u32) as u16;
        let rows = ((height_px / ch).max(1)).min(u16::MAX as u32) as u16;
        if cols == self.cols && rows == self.rows {
            return;
        }
        let old_cols = self.cols;
        let old_rows = self.rows;
        self.cols = cols;
        self.rows = rows;
        if let Err(e) = self.terminal.resize(cols, rows, cw, ch) {
            eprintln!(
                "[surface] terminal resize {old_cols}x{old_rows} → {cols}x{rows} failed: {e}"
            );
        }
        if let Err(e) = self.pty.resize(cols, rows) {
            eprintln!("[surface] pty resize {old_cols}x{old_rows} → {cols}x{rows} failed: {e}");
        }
    }

    /// Send typed input (keyboard) to the shell.
    pub fn write_input(&mut self, bytes: &[u8]) {
        let _ = self.pty.write(bytes);
    }

    /// Drain PTY output, advance the terminal, and rebuild the frame plan.
    pub fn tick(&mut self) -> FramePlan {
        let t0 = Instant::now();

        // Fire autorun once after the configured delay (env-driven perf harness).
        if let Some(a) = &mut self.autorun
            && !a.fired
            && a.start.elapsed() >= a.delay
        {
            a.fired = true;
            eprintln!(
                "[tick] autorun firing: {:?}",
                String::from_utf8_lossy(&a.cmd).trim()
            );
            let _ = self.pty.write(&a.cmd);
        }

        let bytes = self.pty.drain();
        let byte_count = bytes.len();
        let t_drain = Instant::now();
        if !bytes.is_empty() {
            self.terminal.vt_write(&bytes);
        }
        let t_vt = Instant::now();
        let snap = FrameSnapshot::capture(&mut self.terminal, &mut self.state);
        let t_snap = Instant::now();
        let plan = self.build_plan(&snap);
        let t_plan = Instant::now();
        self.state.clear_dirty().ok();

        let total_us = t_plan.duration_since(t0).as_micros();
        // Log slow frames (>5ms) or frames that ingested a lot of PTY bytes.
        // Both are signals that something is bound to output volume.
        if total_us > 5_000 || byte_count > 4_096 {
            eprintln!(
                "[tick] total={total_us}µs bytes={byte_count} drain={}µs vt={}µs snap={}µs plan={}µs verts={}",
                t_drain.duration_since(t0).as_micros(),
                t_vt.duration_since(t_drain).as_micros(),
                t_snap.duration_since(t_vt).as_micros(),
                t_plan.duration_since(t_snap).as_micros(),
                plan.vertex_count,
            );
        }

        plan
    }

    fn build_plan(&mut self, snap: &FrameSnapshot) -> FramePlan {
        let t0 = Instant::now();
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
        let unique_count = unique.len();
        let t_unique = Instant::now();

        // Rebuild the atlas only when new glyph ids appear this frame. Compare
        // against the previous advance-map length, NOT atlas.glyphs.len(): the
        // atlas excludes blank glyphs (e.g. space) while advance includes them,
        // so a count mismatch is permanent and would force a rebuild every frame.
        let prev_advance_len = self.glyph_advance.len();
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
        let atlas_rebuilt = advance.len() > prev_advance_len;
        if atlas_rebuilt {
            self.atlas =
                GlyphAtlas::from_glyph_advances(&self.font, &advance, &mut self.raster_cache);
            self.atlas_generation = self.atlas_generation.wrapping_add(1);
        }
        self.glyph_advance = advance;
        let t_shape = Instant::now();

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
        let default_fg = (fg_default.r, fg_default.g, fg_default.b);
        let default_bg = (
            snap.colors.background.r,
            snap.colors.background.g,
            snap.colors.background.b,
        );
        let mut verts: Vec<Vertex> = Vec::new();

        // Push a textured quad (two triangles, 4 vertices) in pixel space.
        // `push_quad` is inlined-hot; keep attribute order identical to the
        // GL vertex-attrib layout in `gl_renderer.rs`.
        let mut push_quad = |x: f32,
                             y: f32,
                             w: f32,
                             h: f32,
                             u0: f32,
                             v0: f32,
                             u1: f32,
                             v1: f32,
                             (r, g, b): (u8, u8, u8)| {
            let a = 255u8;
            // Counter-clockwise wound (matches the index pattern in the GL
            // renderer: 0,1,2, 0,2,3).
            verts.push(Vertex {
                x,
                y,
                u: u0,
                v: v0,
                r,
                g,
                b,
                a,
            }); // top-left
            verts.push(Vertex {
                x: x + w,
                y,
                u: u1,
                v: v0,
                r,
                g,
                b,
                a,
            }); // top-right
            verts.push(Vertex {
                x: x + w,
                y: y + h,
                u: u1,
                v: v1,
                r,
                g,
                b,
                a,
            }); // br
            verts.push(Vertex {
                x,
                y: y + h,
                u: u0,
                v: v1,
                r,
                g,
                b,
                a,
            }); // bl
        };

        for (row_i, row) in snap.rows_data.iter().enumerate() {
            let row_y = row_i as f32 * ch;
            let baseline = row_y + ascent as f32;
            for (col_i, cell) in row.cells.iter().enumerate() {
                let col_x = col_i as f32 * cw;

                // Resolve effective fg/bg. `cell.fg`/`cell.bg` are None when
                // the cell has no explicit color → fall back to the terminal
                // defaults. Inverse (SGR 7) swaps the two; applied here rather
                // than in ghostty-vt so the rest of the pipeline sees final
                // colors. Reference: ghostling main.c `render_terminal`.
                let raw_fg = match cell.fg {
                    Some(c) => (c.r, c.g, c.b),
                    None => default_fg,
                };
                let raw_bg = match cell.bg {
                    Some(c) => (c.r, c.g, c.b),
                    None => default_bg,
                };
                let (eff_fg, eff_bg) = if cell.style.inverse {
                    (raw_bg, raw_fg)
                } else {
                    (raw_fg, raw_bg)
                };
                // Faint (SGR 2): halve foreground intensity.
                // TODO(bold/italic/underline): style.bold needs a bold face
                // variant; italic needs an italic face; underline/strikethrough
                // need line primitives. All deferred.
                let eff_fg = if cell.style.faint {
                    (eff_fg.0 / 2, eff_fg.1 / 2, eff_fg.2 / 2)
                } else {
                    eff_fg
                };

                // Background quad for cells whose effective bg differs from
                // the terminal default (the FBO was cleared to default_bg, so
                // default-bg cells need no quad). Drawn before the glyph so the
                // glyph composites over it within the cell. Sentinel UV
                // (-1,-1) signals the shader to synthesize coverage 1.0.
                if eff_bg != default_bg {
                    push_quad(
                        col_x, row_y, cw, ch, FLAT_UV, FLAT_UV, FLAT_UV, FLAT_UV, eff_bg,
                    );
                }

                if cell.grapheme.is_empty() {
                    continue;
                }

                let shaped = self
                    .shape_cache
                    .get(&cell.grapheme)
                    .cloned()
                    .unwrap_or_default();
                let mut pen_x = col_x;
                for sg in shaped {
                    if let Some(rect) = self.atlas.glyphs.get(&sg.glyph_id)
                        && rect.w > 0
                        && rect.h > 0
                    {
                        let qx = pen_x + rect.left_bearing as f32;
                        let qy = baseline - rect.top_bearing as f32;
                        push_quad(
                            qx,
                            qy,
                            rect.w as f32,
                            rect.h as f32,
                            rect.x as f32 * inv_w,
                            rect.y as f32 * inv_h,
                            (rect.x + rect.w) as f32 * inv_w,
                            (rect.y + rect.h) as f32 * inv_h,
                            eff_fg,
                        );
                    }
                    pen_x += sg.x_advance;
                }
            }
        }

        // Cursor: drawn last so it layers on top of any glyph beneath it.
        // Sentinel UV (-1,-1) signals the shader to synthesize coverage 1.0.
        if let Some((cx, cy)) = snap.cursor.viewport
            && snap.cursor.visible
        {
            let color = snap.colors.cursor.unwrap_or(snap.colors.foreground);
            push_quad(
                cx as f32 * cw,
                cy as f32 * ch,
                cw,
                ch,
                FLAT_UV,
                FLAT_UV,
                FLAT_UV,
                FLAT_UV,
                (color.r, color.g, color.b),
            );
        }

        self.vertex_buf = verts;
        let t_quads = Instant::now();

        let build_total_us = t_quads.duration_since(t0).as_micros();
        if build_total_us > 5_000 {
            eprintln!(
                "[build_plan] total={build_total_us}µs unique={}µs (n={unique_count}) shape={}µs (atlas_rebuilt={atlas_rebuilt}, advance_n={}, raster_cache_n={}) verts={}µs (n={})",
                t_unique.duration_since(t0).as_micros(),
                t_shape.duration_since(t_unique).as_micros(),
                self.glyph_advance.len(),
                self.raster_cache.len(),
                t_quads.duration_since(t_shape).as_micros(),
                self.vertex_buf.len(),
            );
        }

        FramePlan {
            clear_color: [
                snap.colors.background.r,
                snap.colors.background.g,
                snap.colors.background.b,
                255,
            ],
            cell_w: cw,
            cell_h: ch,
            cols: self.cols as u32,
            rows: self.rows as u32,
            vertices: if self.vertex_buf.is_empty() {
                ptr::null()
            } else {
                self.vertex_buf.as_ptr()
            },
            vertex_count: self.vertex_buf.len(),
            atlas_w,
            atlas_h,
            atlas_pixels: if atlas_w * atlas_h > 0 {
                self.atlas.pixels.as_ptr()
            } else {
                ptr::null()
            },
            atlas_generation: self.atlas_generation,
        }
    }
}

/// Compute the physical pixel size to rasterize the font at, given a logical
/// (DIP) height and the device-pixel ratio. Rounds to the nearest integer so
/// FreeType's `set_pixel_sizes` (which takes integers) gets a stable target.
/// `max(1)` guards the degenerate `dpr=0` / zero-height case.
fn physical_font_size(logical_pixel_height: u32, dpr: f32) -> u32 {
    let p = (dpr * logical_pixel_height as f32).round() as u32;
    p.max(1)
}

/// Sentinel UV emitted for flat-color quads (cell backgrounds, cursor). The
/// shader checks `v_uv.x < 0.0` and synthesizes coverage 1.0 (no texture
/// fetch), so these quads render as opaque color without needing a dedicated
/// texel in the glyph atlas.
const FLAT_UV: f32 = -1.0;

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

/// Spawn a surface. `font_path` may be null to use the system default mono
/// font. `pixel_height` is the logical (DIP) cell height; the font is
/// rasterized at `dpr × pixel_height` physical pixels for hidpi sharpness.
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
    dpr: f32,
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
    match Surface::new(cols, rows, font.as_deref(), pixel_height, dpr) {
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

/// Resize the terminal grid to fit `width_px × height_px` device-independent
/// pixels (pre-P4; physical px post-P4). The surface computes cols/rows from
/// its cell metrics and resizes both the terminal and the PTY. Safe to call
/// with the current size (no-op).
///
/// # Safety
///
/// `s` must be a valid [`Surface`] pointer from [`tako_surface_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_resize_pixels(
    s: *mut Surface,
    width_px: u32,
    height_px: u32,
) {
    if s.is_null() {
        return;
    }
    let surface = unsafe { &mut *s };
    surface.resize_to_pixels(width_px, height_px);
}

/// Reload the font at a new device-pixel ratio and invalidate size-dependent
/// caches. The caller should follow this with [`tako_surface_resize_pixels`]
/// using the current physical item size so the grid reflows to the new cell
/// metrics. No-op when `dpr` is within 0.01 of the current value.
///
/// # Safety
///
/// `s` must be a valid [`Surface`] pointer from [`tako_surface_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_set_dpr(s: *mut Surface, dpr: f32) {
    if s.is_null() {
        return;
    }
    let surface = unsafe { &mut *s };
    surface.set_dpr(dpr);
}

// Keep CString reachable for the FFI doc; avoids dead-code churn if unused.
#[allow(dead_code)]
fn _cstring_marker(s: &str) -> CString {
    CString::new(s).unwrap()
}
