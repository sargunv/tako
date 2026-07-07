//! Phase 0 §3 Step C: prove the Surface produces a renderable FramePlan
//! (quads with atlas UVs + colors + cursor) without needing Qt.
#![allow(unsafe_code)]

use std::ptr;
use std::time::Duration;

use tako_render::Surface;
use tako_render::surface::FramePlan;

#[test]
fn surface_ticks_and_emits_quads() {
    // Spawn the surface at a small cell size; let the shell emit its prompt.
    let mut surface = Surface::new(80, 24, None, 18).expect("surface_new");

    // Give the shell a moment to emit its prompt, then tick a few times.
    for i in 0..10 {
        std::thread::sleep(Duration::from_millis(40));
        let plan: FramePlan = surface.tick();
        eprintln!("[tick {i}] quad_count={}", plan.quad_count);
        // The plan must be self-consistent every tick.
        assert_eq!(plan.cols, 80);
        assert_eq!(plan.rows, 24);
        assert!(plan.cell_w > 0.0 && plan.cell_h > 0.0);
        assert!(plan.bg.a == 255, "background rect must be opaque");
        // quads pointer is dereferenceable when there are quads.
        if plan.quad_count > 0 {
            assert!(plan.quads != ptr::null());
        }
        // Atlas pixels pointer must agree with non-zero size.
        let has_atlas = plan.atlas_w > 0 && plan.atlas_h > 0;
        assert_eq!(has_atlas, plan.atlas_pixels != ptr::null());
    }

    // Drive the shell explicitly to guarantee output (in case $SHELL emits no
    // prompt in this environment).
    surface.write_input(b"echo tako-surface-marker-1\n");
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(40));
        let p = surface.tick();
        if p.quad_count > 0 {
            break;
        }
    }

    // After the shell has had time, there should be SOME glyph quads (a prompt
    // and a visible cursor). We don't assert exact content (shell-dependent).
    let final_plan = surface.tick();
    assert!(
        final_plan.quad_count > 0,
        "expected non-zero glyph quads after shell startup"
    );
    assert_eq!(
        final_plan.cursor.a, 255,
        "cursor should be visible after shell startup"
    );

    // Validate a sample quad: UVs inside the atlas, opaque color.
    let q = unsafe { *final_plan.quads };
    assert!(q.u0 >= 0.0 && q.u1 <= 1.0 + 1e-3);
    assert!(q.v0 >= 0.0 && q.v1 <= 1.0 + 1e-3);
    assert_eq!(q.a, 255);
    assert!(q.w > 0.0 && q.h > 0.0);
}
