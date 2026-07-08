//! Glyph shaping (rustybuzz / HarfBuzz) + rasterization (freetype).
//!
//! A [`FontFace`] owns an [`freetype::Library`] + [`freetype::Face`] loaded
//! from a file's bytes (kept alive in a shared [`Rc`]) plus the same bytes for
//! [`rustybuzz`] shaping. It shapes UTF-8 text into positioned glyph IDs and
//! rasterizes glyph IDs into 8-bit grayscale bitmaps for the atlas.
//!
//! Field order matters: `face` is declared before `library` so it is dropped
//! first (FreeType requires faces be freed before their library).

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};

use freetype::Library;
use freetype::face::LoadFlag;

/// A font-loading or rasterization failure.
#[derive(Debug)]
pub struct FontError(pub String);

impl fmt::Display for FontError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for FontError {}

impl From<freetype::Error> for FontError {
    fn from(e: freetype::Error) -> Self {
        Self(format!("freetype error: {e:?}"))
    }
}

/// One shaped glyph: its font-specific glyph ID and pixel-space positioning.
#[derive(Debug, Clone, Copy)]
pub struct ShapedGlyph {
    pub glyph_id: u32,
    pub cluster: u32,
    /// Horizontal pen advance after this glyph, in pixels.
    pub x_advance: f32,
    /// Pixel-space offsets to apply to the glyph's origin.
    pub x_offset: f32,
    pub y_offset: f32,
}

/// A rasterized 8-bit grayscale glyph bitmap, plus its bearing.
#[derive(Debug, Clone, Default)]
pub struct GlyphBitmap {
    pub width: u32,
    pub height: u32,
    /// Horizontal bearing (bitmap_left), in pixels.
    pub left_bearing: i32,
    /// Vertical bearing from the baseline up (bitmap_top), in pixels.
    pub top_bearing: i32,
    /// `width * height` grayscale bytes, row-major.
    pub pixels: Vec<u8>,
}

/// Monospace cell metrics derived from a sized face, in pixels.
#[derive(Debug, Clone, Copy)]
pub struct CellMetrics {
    pub width: u32,
    pub height: u32,
    pub ascent: i32,
    pub descent: i32,
}

/// A loaded, sized font face ready for shaping and rasterization.
pub struct FontFace {
    face: freetype::Face,
    /// Held only so its Drop (`FT_Done_FreeType`) runs after `face`'s Drop.
    /// Declared after `face` so the field drop order frees the face first.
    _library: Library,
    /// Parsed font tables for rustybuzz shaping, built once at construction.
    /// Borrows a process-lifetime `&'static [u8]` from the path-keyed interner
    /// ([`intern_font_bytes`]): the file is read + leaked once per unique font
    /// path, then reused across every [`FontFace`] (notably across DPR-change
    /// reloads, which previously re-leaked the whole file each time).
    rb_face: rustybuzz::Face<'static>,
    pixel_height: u32,
    /// Font units-per-em, cached for scaling rustybuzz design-unit advances.
    units_per_em: u32,
}

/// Process-lifetime interner of font file bytes, keyed by canonical path.
/// Each unique font file is read once and leaked; subsequent loads (including
/// every [`FontFace::set_pixel`] / DPR-driven reload via [`FontFace::from_path`])
/// reuse the same `&'static` slice instead of re-leaking. Bounded to the number
/// of distinct font paths ever loaded (typically one). FreeType still keeps its
/// own `Rc<Vec<u8>>` copy per face (its API requires it); the win is that the
/// rustybuzz side no longer accumulates.
static FONT_BYTES: OnceLock<Mutex<HashMap<PathBuf, &'static [u8]>>> = OnceLock::new();

fn intern_font_bytes(path: &Path) -> Result<&'static [u8], FontError> {
    let map = FONT_BYTES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().expect("font bytes interner poisoned");
    if let Some(bytes) = guard.get(path) {
        return Ok(*bytes);
    }
    let bytes = std::fs::read(path).map_err(|e| FontError(format!("read font failed: {e}")))?;
    let leaked: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    guard.insert(path.to_path_buf(), leaked);
    Ok(leaked)
}

impl FontFace {
    /// Load `path` at `pixel_height` (the cell height in pixels).
    pub fn from_path(path: impl AsRef<Path>, pixel_height: u32) -> Result<Self, FontError> {
        let path = path.as_ref();
        let rb_bytes = intern_font_bytes(path)?;
        let library = Library::init()?;
        // FreeType keeps its own Rc<Vec<u8>> copy (its memory-face API requires
        // it); we can't share the interned &'static through it.
        let face = library.new_memory_face(Rc::new(rb_bytes.to_vec()), 0)?;
        face.set_pixel_sizes(pixel_height, pixel_height)?;
        let units_per_em = face.raw().units_per_EM.max(1) as u32;

        let rb_face = rustybuzz::Face::from_slice(rb_bytes, 0)
            .ok_or_else(|| FontError("rustybuzz parse failed".to_string()))?;

        Ok(Self {
            face,
            _library: library,
            rb_face,
            pixel_height,
            units_per_em,
        })
    }

    pub fn pixel_height(&self) -> u32 {
        self.pixel_height
    }

    /// Monospace cell metrics for the current pixel size.
    pub fn cell_metrics(&self) -> CellMetrics {
        // Height / ascent / descent come from the sized face (26.6 fixed).
        let m = self.face.size_metrics();
        let (ascent, descent, height) = match &m {
            Some(m) => (
                (m.ascender >> 6) as i32,
                (m.descender >> 6) as i32,
                ((m.height + 32) >> 6) as i32, // round
            ),
            None => (
                self.face.ascender() as i32,
                self.face.descender() as i32,
                self.face.height() as i32,
            ),
        };
        let height = height.max(1) as u32;
        // Width: use a representative monospace glyph's advance rather than
        // `max_advance` (which reflects the widest glyph in the font, e.g. an
        // emoji or CJK cell, not the monospace cell width).
        let width = self.glyph_advance_px(b'M' as usize).max(1) as u32;
        CellMetrics {
            width,
            height,
            ascent,
            descent,
        }
    }

    /// Horizontal pen advance in pixels for `char_code`, without rendering.
    fn glyph_advance_px(&self, char_code: usize) -> i32 {
        let gid = match self.face.get_char_index(char_code) {
            Some(gid) => gid,
            None => return 0,
        };
        if self.face.load_glyph(gid, LoadFlag::empty()).is_err() {
            return 0;
        }
        // advance.x is 26.6 fixed (i64 in freetype-rs).
        (self.face.glyph().advance().x >> 6) as i32
    }

    /// Shape UTF-8 `text` into positioned glyphs. The font tables are parsed
    /// once at construction (see [`Self::rb_face`]); this only allocates the
    /// shaping buffer per call.
    pub fn shape(&self, text: &str) -> Vec<ShapedGlyph> {
        let mut buf = rustybuzz::UnicodeBuffer::new();
        buf.push_str(text);
        let glyphs = rustybuzz::shape(&self.rb_face, &[], buf);

        // rustybuzz returns advances/offsets in font design units; scale to
        // pixels via the requested pixel height over units-per-em.
        let scale = self.pixel_height as f32 / self.units_per_em as f32;
        glyphs
            .glyph_infos()
            .iter()
            .zip(glyphs.glyph_positions())
            .map(|(info, pos)| ShapedGlyph {
                glyph_id: info.glyph_id,
                cluster: info.cluster,
                x_advance: pos.x_advance as f32 * scale,
                x_offset: pos.x_offset as f32 * scale,
                y_offset: pos.y_offset as f32 * scale,
            })
            .collect()
    }

    /// Rasterize `glyph_id` into an 8-bit grayscale bitmap. The glyph is
    /// rendered at the face's current pixel size.
    pub fn rasterize(&self, glyph_id: u32) -> Result<GlyphBitmap, FontError> {
        self.face.load_glyph(glyph_id, LoadFlag::RENDER)?;
        let slot = self.face.glyph();
        let bm = slot.bitmap();
        let width = bm.width().max(0) as u32;
        let height = bm.rows().max(0) as u32;
        let pitch = bm.pitch() as usize;
        let buffer = bm.buffer();

        // Unpack pitch→width into a tight row-major buffer.
        let mut pixels = Vec::with_capacity((width as usize) * (height as usize));
        for row in 0..height as usize {
            let start = row * pitch;
            pixels.extend_from_slice(&buffer[start..start + width as usize]);
        }

        Ok(GlyphBitmap {
            width,
            height,
            left_bearing: slot.bitmap_left(),
            top_bearing: slot.bitmap_top(),
            pixels,
        })
    }
}
