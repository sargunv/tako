//! Phase 0 §3 Step A: real-PTY roundtrip. Spawn a shell, drive it through a
//! PTY, feed its output into a [`Terminal`], and confirm the rendered snapshot
//! contains the echoed content.

use std::time::Duration;

use tako_term::pty::spawn_shell;
use tako_term::snapshot::FrameSnapshot;
use tako_term::terminal::{RenderState, Terminal};

#[test]
fn spawns_shell_and_renders_output() {
    let (mut t, mut rs) = (
        Terminal::new(80, 24, 10_000).expect("terminal_new"),
        RenderState::new().expect("render_state_new"),
    );

    let mut session = spawn_shell(80, 24).expect("spawn_shell");
    // printf writes a unique marker the shell will echo back through the PTY.
    session
        .write(b"printf 'tako-marker-42\\n'\n")
        .expect("write");
    let output = session.read_for(Duration::from_millis(900));

    assert!(
        !output.is_empty(),
        "PTY produced no bytes — did the shell fail to spawn?"
    );
    t.vt_write(&output);

    let snap = FrameSnapshot::capture(&mut t, &mut rs);
    let screen: String = snap
        .rows_data
        .iter()
        .flat_map(|r| r.cells.iter().map(|c| c.grapheme.as_str()))
        .collect::<String>();

    assert!(
        screen.contains("tako-marker-42"),
        "expected marker in rendered screen; got first 200 chars: {:?}",
        screen.chars().take(200).collect::<String>()
    );
}
