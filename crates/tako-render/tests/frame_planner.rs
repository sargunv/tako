//! FramePlanner: prove a snapshot → FramePlan pipeline works without a PTY
//! or a shell. The planner is a pure view over a snapshot, so this drives it
//! with a real [`Terminal`] (no PTY) and asserts the vertex output is sane.
//! This is the layer-appropriate replacement for the old Surface integration
//! test that spawned a real shell.

#![allow(unsafe_code)]

use std::ptr;

use tako_render::FramePlanner;
use tako_render::frame_planner::{FramePlan, Vertex};
use tako_term::snapshot::FrameSnapshot;
use tako_term::terminal::{RenderState, Terminal};

#[test]
fn builds_vertices_from_a_snapshot() {
    // Real terminal + render state, no PTY. Feed it literal bytes so the
    // snapshot has known content + a visible cursor.
    let (mut term, mut state) = (
        Terminal::new(40, 5, 10_000).expect("terminal_new"),
        RenderState::new().expect("render_state_new"),
    );
    term.vt_write(b"Hello!");

    let mut planner =
        FramePlanner::new(None, 18, 1.0).expect("planner (resolves default mono font)");

    // Snapshot is the planner's input contract; capture it fresh.
    let snap = FrameSnapshot::capture(&mut term, &mut state);
    let plan: FramePlan = planner.build_plan(&snap);

    // "Hello!" + a cursor quad → at least one vertex, and the planner reports
    // the snapshot's grid size.
    assert_eq!(plan.cols, 40);
    assert_eq!(plan.rows, 5);
    assert!(plan.cell_w > 0.0 && plan.cell_h > 0.0);
    assert!(plan.clear_color[3] == 255, "clear color must be opaque");
    assert!(
        plan.vertex_count > 0,
        "expected glyphs + cursor vertices after 'Hello!'"
    );
    assert!(
        !plan.vertices.is_null(),
        "non-empty plan has a null vertex ptr"
    );

    // Atlas pixels pointer must agree with non-zero size.
    let has_atlas = plan.atlas_w > 0 && plan.atlas_h > 0;
    assert_eq!(has_atlas, !plan.atlas_pixels.is_null());

    // Validate a sample vertex: UVs in [0,1] for a glyph quad, opaque alpha.
    let v: Vertex = unsafe { ptr::read(plan.vertices) };
    assert!(v.u >= 0.0, "first vertex u is negative: {}", v.u);
    assert!(v.a == 255, "vertex alpha should be 255");

    // A second build over the same snapshot must stay self-consistent (the
    // shape cache is now bounded; a re-build should hit it, not regress).
    let plan2 = planner.build_plan(&snap);
    assert_eq!(plan2.vertex_count, plan.vertex_count);
}
