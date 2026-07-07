//! Phase 0 §3 Step C: prove the Surface produces a renderable FramePlan
//! (vertices with atlas UVs + colors + cursor) without needing Qt.
#![allow(unsafe_code)]

use std::ptr;
use std::time::Duration;

use tako_render::Surface;
use tako_render::surface::{FramePlan, Vertex};

#[test]
fn surface_ticks_and_emits_vertices() {
    // Spawn the surface at a small cell size; let the shell emit its prompt.
    let mut surface = Surface::new(80, 24, None, 18).expect("surface_new");

    // Give the shell a moment to emit its prompt, then tick a few times.
    for i in 0..10 {
        std::thread::sleep(Duration::from_millis(40));
        let plan: FramePlan = surface.tick();
        eprintln!("[tick {i}] vertex_count={}", plan.vertex_count);
        // The plan must be self-consistent every tick.
        assert_eq!(plan.cols, 80);
        assert_eq!(plan.rows, 24);
        assert!(plan.cell_w > 0.0 && plan.cell_h > 0.0);
        // clear_color alpha must be opaque (background is drawn via glClear).
        assert_eq!(plan.clear_color[3], 255, "clear color must be opaque");
        // vertices pointer is dereferenceable when there are vertices.
        if plan.vertex_count > 0 {
            assert!(!plan.vertices.is_null());
        }
        // Atlas pixels pointer must agree with non-zero size.
        let has_atlas = plan.atlas_w > 0 && plan.atlas_h > 0;
        assert_eq!(has_atlas, !plan.atlas_pixels.is_null());
    }

    // Drive the shell explicitly to guarantee output (in case $SHELL emits no
    // prompt in this environment).
    surface.write_input(b"echo tako-surface-marker-1\n");
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(40));
        let p = surface.tick();
        if p.vertex_count > 0 {
            break;
        }
    }

    // After the shell has had time, there should be SOME vertices (a prompt
    // glyph plus a cursor quad). We don't assert exact content (shell-dependent).
    let final_plan = surface.tick();
    assert!(
        final_plan.vertex_count > 0,
        "expected non-zero vertices after shell startup"
    );

    // Validate a sample vertex: UVs in [0,1], opaque alpha, in-bounds position.
    let v: Vertex = unsafe { ptr::read(final_plan.vertices) };
    assert!(v.u >= 0.0 && v.u <= 1.0 + 1e-3, "u out of range: {}", v.u);
    assert!(v.v >= 0.0 && v.v <= 1.0 + 1e-3, "v out of range: {}", v.v);
    assert_eq!(v.a, 255, "vertex alpha should be 255");
}
