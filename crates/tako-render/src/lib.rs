//! Tako's terminal renderer: a Qt `QQuickFramebufferObject` hosting a
//! libghostty-vt terminal, driven from Rust.
//!
//! - [`font`] — glyph shaping (rustybuzz/HarfBuzz) + rasterization (freetype).
//! - [`atlas`] — shelf-packed grayscale glyph atlas built from a
//!   [`tako_term::snapshot::FrameSnapshot`]; reserves a white texel at (0, 0).
//! - [`panel`] — model-owned terminal core: [`TerminalPanel`] owns the
//!   libghostty-vt terminal + render state + PTY + OSC-derived side state.
//! - [`frame_planner`] — pure view: [`FramePlanner`] owns the font + glyph
//!   caches and turns a snapshot into a [`FramePlan`] of draw vertices.
//! - [`surface`] — [`Surface`] orchestrates a panel + planner + input
//!   encoders; the glue the C++ QQuickItem drives.
//! - [`gl_renderer`] — glow-based GL renderer that consumes a [`FramePlan`]
//!   inside a `QQuickFramebufferObject::Renderer`.
//! - [`ffi`] — the `extern "C"` ABI the C++ view calls.

pub mod atlas;
pub mod ffi;
pub mod font;
pub mod frame_planner;
pub mod gl_renderer;
pub mod panel;
pub mod qml_init;
pub mod surface;

pub use atlas::{GlyphAtlas, GlyphRect};
pub use font::{CellMetrics, FontError, FontFace, GlyphBitmap, ShapedGlyph};
pub use frame_planner::{FramePlan, FramePlanner, Vertex};
pub use gl_renderer::GlRenderer;
pub use panel::TerminalPanel;
pub use surface::Surface;

/// A surface setup or runtime failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("terminal: {0}")]
    Terminal(#[from] tako_term::Error),
    #[error("font: {0}")]
    Font(#[from] FontError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Self::Other(s)
    }
}
