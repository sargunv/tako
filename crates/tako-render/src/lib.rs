//! QQuickItem RHI terminal renderer (cxx-qt-exposed).
//!
//! Phase 0 §3:
//! - [`font`] — glyph shaping (rustybuzz/HarfBuzz) + rasterization (freetype).
//! - [`atlas`] — shelf-packed grayscale glyph atlas built from a
//!   [`tako_term::snapshot::FrameSnapshot`].
//!
//! Later phases add the QQuickItem/QSG render node and the cxx-qt bridge.

pub mod atlas;
pub mod font;
pub mod qml_init;
pub mod surface;

pub use atlas::{GlyphAtlas, GlyphRect};
pub use font::{CellMetrics, FontError, FontFace, GlyphBitmap, ShapedGlyph};
pub use surface::{CQuad, CRect, FramePlan, Surface};
