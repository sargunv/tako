//! Phase 0 §3 Step B: prove the glyph atlas pipeline end to end. Shape +
//! rasterize a snapshot's graphemes, pack them, write a PNG, and assert the
//! glyphs landed with non-empty coverage.

use std::path::PathBuf;
use std::process::Command;

use tako_render::{FontFace, GlyphAtlas};
use tako_term::snapshot::FrameSnapshot;
use tako_term::terminal::{RenderState, Terminal};

/// Resolve the system's default monospace font file via fontconfig.
fn default_mono_font() -> PathBuf {
    let out = Command::new("fc-match")
        .args(["-f", "%{file}", "monospace"])
        .output()
        .expect("fc-match failed");
    assert!(out.status.success(), "fc-match returned non-zero");
    let path = String::from_utf8(out.stdout).expect("fc-match output non-UTF8");
    let path = PathBuf::from(path.trim().to_string());
    assert!(
        path.exists(),
        "resolved font does not exist: {}",
        path.display()
    );
    path
}

#[test]
fn shapes_rasterizes_and_packs_hello_world() {
    let (mut t, mut rs) = (
        Terminal::new(40, 5, 10_000).expect("terminal_new"),
        RenderState::new().expect("render_state_new"),
    );
    t.vt_write(b"Hello, world!");
    let snap = FrameSnapshot::capture(&mut t, &mut rs);

    let font = FontFace::from_path(default_mono_font(), 20).expect("load font");
    let atlas = GlyphAtlas::from_snapshot(&font, &snap);

    // Sanity: monospace metrics are positive and cell width/height reasonable.
    assert!(atlas.metrics.width > 0);
    assert!(atlas.metrics.height > 0);

    // Shape the ASCII set we wrote and confirm each maps to a real glyph.
    let expected = ['H', 'e', 'l', 'o', ',', ' ', 'w', 'r', 'd', '!'];
    for ch in expected {
        let shaped = font.shape(&ch.to_string());
        assert_eq!(
            shaped.len(),
            1,
            "ASCII {:?} shaped to {} glyphs",
            ch,
            shaped.len()
        );
        let gid = shaped[0].glyph_id;
        let rect = atlas
            .glyphs
            .get(&gid)
            .unwrap_or_else(|| panic!("glyph {gid} for {ch:?} missing from atlas"));
        // Printable glyphs have a non-empty bitmap; space is the exception.
        if ch == ' ' {
            assert_eq!((rect.w, rect.h), (0, 0), "space should have an empty rect");
        } else {
            assert!(
                rect.w > 0 && rect.h > 0,
                "glyph for {ch:?} has empty bitmap in atlas"
            );
            // Pen advance for a monospace glyph should match the cell width.
            assert!(
                (rect.x_advance - atlas.metrics.width as f32).abs() < 1.5,
                "advance {} for {ch:?} not near cell width {}",
                rect.x_advance,
                atlas.metrics.width
            );
        }
    }

    // The atlas texture must have at least one non-zero pixel.
    assert!(
        atlas.pixels.iter().any(|&p| p > 0),
        "atlas is entirely blank"
    );

    // Write a PNG to /tmp for visual inspection.
    let png = PathBuf::from("/tmp/tako-phase0-atlas.png");
    atlas.write_png(&png).expect("write_png");
    let meta = std::fs::metadata(&png).expect("png metadata");
    assert!(meta.len() > 0, "written PNG is empty");
    eprintln!(
        "atlas PNG written to {} ({}x{}, {} bytes)",
        png.display(),
        atlas.width,
        atlas.height,
        meta.len()
    );
}
