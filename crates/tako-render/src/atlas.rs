//! Glyph atlas: shapes a snapshot's graphemes, rasterizes each unique glyph
//! once, and packs the bitmaps into a single texture via shelf packing. The
//! output is a grayscale pixel buffer plus a per-glyph rect map the renderer
//! samples from.

use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::{self, BufWriter};
use std::path::Path;

use png::{BitDepth, ColorType, Encoder};

use crate::font::{CellMetrics, FontFace, GlyphBitmap};
use tako_term::snapshot::FrameSnapshot;

/// One glyph's placement in the atlas, with the metrics the renderer needs to
/// composite it onto the grid.
#[derive(Debug, Clone, Copy)]
pub struct GlyphRect {
    pub glyph_id: u32,
    /// Top-left position in atlas pixels.
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// Freetype bitmap_left / bitmap_top bearings (pixels).
    pub left_bearing: i32,
    pub top_bearing: i32,
    /// Horizontal pen advance from shaping (pixels).
    pub x_advance: f32,
}

/// A packed grayscale glyph atlas.
#[derive(Debug)]
pub struct GlyphAtlas {
    pub width: u32,
    pub height: u32,
    /// `width * height` grayscale bytes, row-major.
    pub pixels: Vec<u8>,
    pub glyphs: HashMap<u32, GlyphRect>,
    pub metrics: CellMetrics,
}

const ATLAS_WIDTH: u32 = 512;

impl GlyphAtlas {
    /// Shape every unique non-empty grapheme in `snap`, rasterize the unique
    /// glyph IDs, and pack the result.
    pub fn from_snapshot(face: &FontFace, snap: &FrameSnapshot) -> Self {
        let unique: BTreeSet<String> = snap
            .rows_data
            .iter()
            .flat_map(|r| r.cells.iter())
            .filter(|c| !c.grapheme.is_empty())
            .map(|c| c.grapheme.clone())
            .collect();
        Self::from_graphemes(face, unique.iter().map(String::as_str))
    }

    /// Build an atlas from an explicit grapheme set.
    pub fn from_graphemes<'a>(
        face: &FontFace,
        graphemes: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        // Shape → collect unique glyph ids + a representative advance.
        let mut advance: HashMap<u32, f32> = HashMap::new();
        for g in graphemes {
            for sg in face.shape(g) {
                advance.entry(sg.glyph_id).or_insert(sg.x_advance);
            }
        }

        // Rasterize each unique glyph id once.
        let mut bitmaps: HashMap<u32, GlyphBitmap> = HashMap::new();
        for &gid in advance.keys() {
            if let Ok(bm) = face.rasterize(gid) {
                bitmaps.insert(gid, bm);
            }
        }

        Self::pack(face, bitmaps, advance)
    }

    /// Build an atlas from a pre-shaped glyph-id → advance map, using a
    /// caller-owned rasterization cache so each glyph id is rasterized at most
    /// once across rebuilds. Lets the caller cache shaping across frames
    /// (rustybuzz re-parses the font per `shape` — also now cached in
    /// [`FontFace`]).
    pub fn from_glyph_advances(
        face: &FontFace,
        advance: &HashMap<u32, f32>,
        raster_cache: &mut HashMap<u32, GlyphBitmap>,
    ) -> Self {
        let mut bitmaps: HashMap<u32, GlyphBitmap> = HashMap::new();
        for &gid in advance.keys() {
            // Rasterize-and-cache: subsequent atlas rebuilds (which re-pack the
            // whole map when new glyphs arrive) reuse the cached bitmap rather
            // than re-calling FreeType for every glyph each time.
            let bm = raster_cache
                .entry(gid)
                .or_insert_with(|| face.rasterize(gid).unwrap_or_default());
            if bm.width > 0 && bm.height > 0 {
                bitmaps.insert(gid, bm.clone());
            }
        }
        Self::pack(face, bitmaps, advance.clone())
    }

    fn pack(
        face: &FontFace,
        bitmaps: HashMap<u32, GlyphBitmap>,
        advance: HashMap<u32, f32>,
    ) -> Self {
        let mut glyphs: HashMap<u32, GlyphRect> = HashMap::new();
        let mut placements: Vec<(u32, u32, u32)> = Vec::new(); // gid, x, y

        // Shelf-pack tallest-first for density.
        let mut ids: Vec<u32> = bitmaps.keys().copied().collect();
        ids.sort_by_key(|g| std::cmp::Reverse(bitmaps[g].height));

        // Pixel (0, 0) is reserved as a permanent white texel (coverage=1.0)
        // so the renderer can draw flat-color background and cursor quads
        // using the same shader/texture as glyphs — no second texture needed.
        // Glyph packing therefore starts at column 1 on the first row; later
        // rows wrap back to column 0 (the white texel only lives at y=0).
        let mut x = 1u32;
        let mut y = 0u32;
        let mut row_h = 0u32;

        for gid in ids {
            let bm = &bitmaps[&gid];
            let (w, h) = (bm.width, bm.height);
            let adv = advance.get(&gid).copied().unwrap_or(0.0);

            if w == 0 || h == 0 {
                // Blank (e.g. space): record a zero-size rect with its advance.
                glyphs.insert(
                    gid,
                    GlyphRect {
                        glyph_id: gid,
                        x: 0,
                        y: 0,
                        w: 0,
                        h: 0,
                        left_bearing: bm.left_bearing,
                        top_bearing: bm.top_bearing,
                        x_advance: adv,
                    },
                );
                continue;
            }

            if x + w > ATLAS_WIDTH {
                y += row_h;
                x = 0;
                row_h = 0;
            }
            glyphs.insert(
                gid,
                GlyphRect {
                    glyph_id: gid,
                    x,
                    y,
                    w,
                    h,
                    left_bearing: bm.left_bearing,
                    top_bearing: bm.top_bearing,
                    x_advance: adv,
                },
            );
            placements.push((gid, x, y));
            x += w;
            row_h = row_h.max(h);
        }

        let height = (y + row_h).max(1);
        let mut pixels = vec![0u8; (ATLAS_WIDTH * height) as usize];
        // White texel at (0, 0) — sampled by flat-color quads (bg, cursor) for
        // full coverage. See the pack() doc comment above.
        pixels[0] = 255;
        for (gid, px, py) in placements {
            let bm = &bitmaps[&gid];
            for r in 0..bm.height {
                for c in 0..bm.width {
                    let dst = ((py + r) * ATLAS_WIDTH + (px + c)) as usize;
                    let src = (r * bm.width + c) as usize;
                    pixels[dst] = bm.pixels[src];
                }
            }
        }

        GlyphAtlas {
            width: ATLAS_WIDTH,
            height,
            pixels,
            glyphs,
            metrics: face.cell_metrics(),
        }
    }

    /// Write the atlas as an 8-bit grayscale PNG (for inspection / testing).
    pub fn write_png(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let file = File::create(path)?;
        let w = &mut BufWriter::new(file);
        let mut enc = Encoder::new(w, self.width, self.height);
        enc.set_color(ColorType::Grayscale);
        enc.set_depth(BitDepth::Eight);
        let mut writer = enc
            .write_header()
            .map_err(|e| io::Error::other(e.to_string()))?;
        writer
            .write_image_data(&self.pixels)
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(())
    }
}
