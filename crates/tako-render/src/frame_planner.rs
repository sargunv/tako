//! Pure terminal renderer: owns the font, glyph caches, and glyph atlas, and
//! turns a [`FrameSnapshot`] into a [`FramePlan`] of ready-to-draw colored
//! glyph quads. Has no terminal or PTY — it only reads snapshots, so it can be
//! unit-tested with synthetic input and no shell.
//!
//! The [`FramePlanner`] is single-threaded (it isn't `Send`); the snapshot it
//! consumes is plain owned data safe to produce anywhere.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::process::Command;

use lru::LruCache;
use tako_term::snapshot::FrameSnapshot;

use crate::Error;
use crate::atlas::GlyphAtlas;
use crate::font::{CellMetrics, FontError, FontFace, GlyphBitmap, ShapedGlyph};

/// Cap on the number of distinct graphemes whose shaped output we keep. A
/// full screen rarely exceeds a few thousand unique graphemes; bounding the
/// cache prevents bursts of unique text (base64, binary dumps) from growing it
/// without limit. Eviction reshapes on next use (cheap — rustybuzz tables are
/// cached in [`FontFace`]); the raster cache survives, so FreeType never
/// re-rasterizes an evicted-and-re-seen glyph.
const SHAPE_CACHE_CAP: usize = 4096;

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
/// borrowed from the [`FramePlanner`] and valid only until the next
/// [`FramePlanner::build_plan`] (or the planner's destruction); the renderer
/// deep-copies them in its `synchronize()` step.
///
/// `vertices` is one flat buffer of glyph + cursor quad vertices in draw order
/// (cursor last, so it layers over glyphs). Background-cell quads use the
/// sentinel [`FLAT_UV`].
#[repr(C)]
#[derive(Clone, Copy, Default)]
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

/// A font + glyph-cache owner that produces [`FramePlan`]s from snapshots.
pub struct FramePlanner {
    font: FontFace,
    cell: CellMetrics,
    /// Resolved font file path (kept so the font can be reloaded on DPR
    /// change).
    font_path: String,
    /// Logical (DIP) font size the user requested. The actual rasterized size
    /// is `logical_pixel_height × dpr` so glyphs stay crisp on hidpi displays.
    logical_pixel_height: u32,
    /// Device pixel ratio the font was rasterized for.
    dpr: f32,
    atlas: GlyphAtlas,
    /// Bumped every time `atlas` is reassigned, so the renderer can detect
    /// content changes that don't alter dimensions (shelf-pack repacking).
    atlas_generation: u64,
    glyph_advance: HashMap<u32, f32>,
    /// Rasterize-once cache keyed by glyph id, shared across atlas rebuilds so
    /// FreeType never rasterizes the same glyph twice.
    raster_cache: HashMap<u32, GlyphBitmap>,
    shape_cache: LruCache<String, Vec<ShapedGlyph>>,
    vertex_buf: Vec<Vertex>,
}

impl FramePlanner {
    /// Resolve `font_path` (or the system default monospace if `None`) and
    /// load it at `logical_pixel_height`, rasterized at `dpr × pixel_height`
    /// physical pixels for hidpi sharpness.
    pub fn new(
        font_path: Option<&str>,
        logical_pixel_height: u32,
        dpr: f32,
    ) -> Result<Self, Error> {
        let path = match font_path {
            Some(p) => p.to_string(),
            None => resolve_default_font()?,
        };
        let font = FontFace::from_path(&path, physical_font_size(logical_pixel_height, dpr))?;
        Self::with_font(font, path, logical_pixel_height, dpr)
    }

    /// Build a planner around an already-loaded font. Kept `pub` as a test
    /// seam (lets a synthetic [`FontFace`] drive [`Self::build_plan`] without
    /// touching the filesystem or spawning a shell).
    pub fn with_font(
        font: FontFace,
        font_path: String,
        logical_pixel_height: u32,
        dpr: f32,
    ) -> Result<Self, Error> {
        let cell = font.cell_metrics();
        let mut raster_cache = HashMap::new();
        let atlas = GlyphAtlas::from_glyph_advances(&font, &HashMap::new(), &mut raster_cache);
        Ok(Self {
            font,
            cell,
            font_path,
            logical_pixel_height,
            dpr,
            atlas,
            atlas_generation: 0,
            glyph_advance: HashMap::new(),
            raster_cache,
            shape_cache: LruCache::new(NonZeroUsize::new(SHAPE_CACHE_CAP).unwrap()),
            vertex_buf: Vec::new(),
        })
    }

    pub fn cell(&self) -> CellMetrics {
        self.cell
    }

    /// Reload the font at a new device-pixel ratio, invalidating all
    /// size-dependent caches. Returns the new cell metrics so the caller can
    /// reflow the terminal grid. No-op when `dpr` is within 0.01 of the
    /// current value (returns the unchanged metrics).
    pub fn set_dpr(&mut self, dpr: f32) -> CellMetrics {
        if (dpr - self.dpr).abs() < 0.01 {
            return self.cell;
        }
        self.dpr = dpr;
        let physical_px = physical_font_size(self.logical_pixel_height, dpr);
        match FontFace::from_path(&self.font_path, physical_px) {
            Ok(font) => {
                self.font = font;
                self.cell = self.font.cell_metrics();
            }
            Err(e) => {
                log::warn!("font reload at dpr {dpr} (phys {physical_px}) failed: {e}");
                return self.cell;
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
        self.cell
    }

    /// Turn a snapshot into a flat buffer of glyph + cursor quads.
    pub fn build_plan(&mut self, snap: &FrameSnapshot) -> FramePlan {
        let t0 = std::time::Instant::now();
        let CellMetrics {
            width: cw,
            height: ch,
            ascent,
            ..
        } = self.cell;
        let (cw, ch) = (cw as f32, ch as f32);

        // Collect unique graphemes; shape via cache; refresh the atlas if the
        // glyph-id set grew.
        let mut unique: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for row in &snap.rows_data {
            for cell in &row.cells {
                if !cell.grapheme.is_empty() {
                    unique.insert(cell.grapheme.clone());
                }
            }
        }
        let unique_count = unique.len();
        let t_unique = std::time::Instant::now();

        // Rebuild the atlas only when new glyph ids appear this frame. Compare
        // against the previous advance-map length, NOT atlas.glyphs.len(): the
        // atlas excludes blank glyphs (e.g. space) while advance includes them,
        // so a count mismatch is permanent and would force a rebuild every frame.
        let prev_advance_len = self.glyph_advance.len();
        let mut advance: HashMap<u32, f32> = std::mem::take(&mut self.glyph_advance);
        for g in &unique {
            if !self.shape_cache.contains(g) {
                let shaped = self.font.shape(g);
                self.shape_cache.put(g.clone(), shaped);
            }
            let shaped = self
                .shape_cache
                .peek(g)
                .expect("shape_cache just populated")
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
        let t_shape = std::time::Instant::now();

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
            // The selected cell range for this row (inclusive), if any. Cells
            // inside it get inverted fg/bg — the xterm/ghostty selection
            // rendering, which works with the opaque-quad pipeline (no
            // blending needed) and stays legible on any background.
            let sel = row.selection;
            for (col_i, cell) in row.cells.iter().enumerate() {
                let col_x = col_i as f32 * cw;
                let selected =
                    sel.is_some_and(|(sx, ex)| col_i >= sx as usize && col_i <= ex as usize);

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
                // Selection inverts fg/bg for cells in the range (applied after
                // SGR-7 inverse so both compose correctly).
                let (mut eff_fg, mut eff_bg) = if cell.style.inverse {
                    (raw_bg, raw_fg)
                } else {
                    (raw_fg, raw_bg)
                };
                if selected {
                    core::mem::swap(&mut eff_fg, &mut eff_bg);
                }
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
        // The cursor is sized/positioned by its visual style (DECSCUSR):
        //   Block       — full cell (default).
        //   Bar         — left ~1/8 of cell.
        //   Underline   — bottom ~1/8 of cell.
        //   BlockHollow — full cell with inverted fg/bg; we draw a thin border
        //                 approximation (4 quads) since our shader has no
        //                 outline primitive.
        if let Some((cx, cy)) = snap.cursor.viewport
            && snap.cursor.visible
        {
            let color = snap.colors.cursor.unwrap_or(snap.colors.foreground);
            let px = cx as f32 * cw;
            let py = cy as f32 * ch;
            match snap.cursor.style {
                tako_term::snapshot::CursorStyle::BlockHollow => {
                    // Hollow block: draw a frame (4 thin quads) using the
                    // foreground color so the cursor stands out against any bg.
                    let bg = snap.colors.foreground;
                    let thickness = (cw.min(ch) * 0.1).max(1.0);
                    for &(y, h) in &[(py, thickness), (py + ch - thickness, thickness)] {
                        push_quad(
                            px,
                            y,
                            cw,
                            h,
                            FLAT_UV,
                            FLAT_UV,
                            FLAT_UV,
                            FLAT_UV,
                            (bg.r, bg.g, bg.b),
                        );
                    }
                    for &(x, w) in &[(px, thickness), (px + cw - thickness, thickness)] {
                        push_quad(
                            x,
                            py + thickness,
                            w,
                            ch - 2.0 * thickness,
                            FLAT_UV,
                            FLAT_UV,
                            FLAT_UV,
                            FLAT_UV,
                            (bg.r, bg.g, bg.b),
                        );
                    }
                }
                tako_term::snapshot::CursorStyle::Bar => {
                    push_quad(
                        px,
                        py,
                        (cw * 0.125).max(1.0),
                        ch,
                        FLAT_UV,
                        FLAT_UV,
                        FLAT_UV,
                        FLAT_UV,
                        (color.r, color.g, color.b),
                    );
                }
                tako_term::snapshot::CursorStyle::Underline => {
                    let h = (ch * 0.125).max(1.0);
                    push_quad(
                        px,
                        py + ch - h,
                        cw,
                        h,
                        FLAT_UV,
                        FLAT_UV,
                        FLAT_UV,
                        FLAT_UV,
                        (color.r, color.g, color.b),
                    );
                }
                tako_term::snapshot::CursorStyle::Block => {
                    push_quad(
                        px,
                        py,
                        cw,
                        ch,
                        FLAT_UV,
                        FLAT_UV,
                        FLAT_UV,
                        FLAT_UV,
                        (color.r, color.g, color.b),
                    );
                }
            }
            // TODO: blink phase (Cursor.blinking + Cursor.password_input).
        }

        self.vertex_buf = verts;
        let t_quads = std::time::Instant::now();

        let build_total_us = t_quads.duration_since(t0).as_micros();
        if build_total_us > 5_000 {
            log::debug!(
                "build_plan total={build_total_us}µs unique={}µs (n={unique_count}) shape={}µs \
                 (atlas_rebuilt={atlas_rebuilt}, advance_n={}, raster_cache_n={}) verts={}µs (n={})",
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
            cols: snap.cols as u32,
            rows: snap.rows as u32,
            vertices: if self.vertex_buf.is_empty() {
                std::ptr::null()
            } else {
                self.vertex_buf.as_ptr()
            },
            vertex_count: self.vertex_buf.len(),
            atlas_w,
            atlas_h,
            atlas_pixels: if atlas_w * atlas_h > 0 {
                self.atlas.pixels.as_ptr()
            } else {
                std::ptr::null()
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
pub(crate) fn resolve_default_font() -> Result<String, FontError> {
    let out = Command::new("fc-match")
        .args(["-f", "%{file}", "monospace"])
        .output()
        .map_err(|e| FontError(format!("fc-match failed: {e}")))?;
    if !out.status.success() {
        return Err(FontError("fc-match returned non-zero".to_string()));
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        return Err(FontError("fc-match returned empty path".to_string()));
    }
    Ok(path)
}
