//! QQuickFramebufferObject terminal renderer (cxx-qt-exposed).
//!
//! - [`font`] — glyph shaping (rustybuzz/HarfBuzz) + rasterization (freetype).
//! - [`atlas`] — shelf-packed grayscale glyph atlas built from a
//!   [`tako_term::snapshot::FrameSnapshot`]; reserves a white texel at (0, 0).
//! - [`surface`] — owns the [`tako_term::terminal::Terminal`] + PTY + font +
//!   atlas and produces a [`surface::FramePlan`] of ready-to-draw vertices.
//! - [`gl_renderer`] — glow-based GL renderer that consumes a [`FramePlan`]
//!   inside a `QQuickFramebufferObject::Renderer`.

pub mod atlas;
pub mod font;
pub mod gl_renderer;
pub mod qml_init;
pub mod surface;

pub use atlas::{GlyphAtlas, GlyphRect};
pub use font::{CellMetrics, FontError, FontFace, GlyphBitmap, ShapedGlyph};
pub use gl_renderer::GlRenderer;
pub use surface::{FramePlan, Surface, Vertex};
