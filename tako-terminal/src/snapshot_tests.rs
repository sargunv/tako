//! Phase 0 §3 Step A: snapshot walker roundtrip with literal VT bytes.
//!
//! Mirrors the `c-vt-render` embedding contract: feed styled content, capture a
//! frame, and assert the cells, styles, and resolved colors come back correctly.

use crate::snapshot::{Dirty, FrameSnapshot};
use crate::terminal::{RenderState, Terminal};

/// Concatenate all graphemes in a row.
fn row_text(row: &crate::snapshot::Row) -> String {
    row.cells.iter().map(|c| c.grapheme.as_str()).collect()
}

#[test]
fn dec_2026_synchronized_output_mode_is_tracked() {
    let (mut t, _rs) = fresh_terminal();
    assert!(!t.mode_get(crate::modes::SYNC_OUTPUT));

    t.vt_write(b"\x1b[?2026h");
    assert!(t.mode_get(crate::modes::SYNC_OUTPUT));

    t.vt_write(b"\x1b[?2026l");
    assert!(!t.mode_get(crate::modes::SYNC_OUTPUT));
}

#[test]
fn osc_8_hyperlink_uri_is_read_from_grid_ref() {
    let (mut t, _rs) = fresh_terminal();
    t.vt_write(b"\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\");

    let linked = t
        .grid_ref(crate::point::Point::active(0, 0))
        .expect("linked grid ref");
    assert_eq!(
        linked.hyperlink_uri().as_deref(),
        Some("https://example.com")
    );

    let blank = t
        .grid_ref(crate::point::Point::active(4, 0))
        .expect("blank grid ref");
    assert_eq!(blank.hyperlink_uri(), None);
}

fn fresh_terminal() -> (Terminal, RenderState) {
    let t = Terminal::new(40, 5, 10_000).expect("terminal_new");
    let r = RenderState::new().expect("render_state_new");
    (t, r)
}

#[test]
fn selection_gesture_can_select_visible_scrollback_viewport_rows() {
    use crate::gesture::{
        GestureBehaviors, GestureGeometry, SelectionGesture, SelectionGestureEvent, SurfacePosition,
    };
    use crate::selection::Format;

    let (mut t, _rs) = fresh_terminal();
    t.vt_write(b"alpha\r\nbravo\r\ncharlie\r\ndelta\r\necho\r\nfoxtrot");
    t.scroll_viewport_delta(-2);

    let gesture = SelectionGesture::new(&t).expect("selection gesture");
    let press_type =
        crate::ffi::GhosttySelectionGestureEventType_GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_PRESS;
    let drag_type =
        crate::ffi::GhosttySelectionGestureEventType_GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_DRAG;

    let start = t
        .grid_ref(crate::point::Point::viewport(0, 0))
        .expect("scrolled-back viewport start ref");
    let end = t
        .grid_ref(crate::point::Point::viewport(5, 0))
        .expect("scrolled-back viewport end ref");

    let mut press = SelectionGestureEvent::new(press_type).expect("press event");
    press.set_ref(&start);
    press.set_position(SurfacePosition { x: 0.0, y: 0.0 });
    press.set_time_ns(1);
    press.set_repeat_interval_ns(500_000_000);
    press.set_repeat_distance(8.0);
    press.set_behaviors(GestureBehaviors::DEFAULT);
    assert!(gesture.dispatch(&t, &press).expect("press").is_none());

    let mut drag = SelectionGestureEvent::new(drag_type).expect("drag event");
    drag.set_ref(&end);
    drag.set_position(SurfacePosition { x: 5.0, y: 0.0 });
    drag.set_geometry(GestureGeometry {
        columns: 40,
        cell_width: 1,
        padding_left: 0,
        screen_height: 5,
    });
    drag.set_rectangle(false);
    let selection = gesture
        .dispatch(&t, &drag)
        .expect("drag")
        .expect("drag selection");
    t.set_selection(Some(&selection))
        .expect("install selection");

    let selected = t
        .selection_format(None, Format::Plain, true, true)
        .expect("formatted selection");
    assert_eq!(String::from_utf8(selected).unwrap(), "alpha");
}

#[test]
fn osc_133_command_output_can_be_selected_semantically() {
    use crate::selection::Format;

    let (mut t, _rs) = fresh_terminal();
    t.vt_write(
        b"\x1b]133;A\x07$ \x1b]133;B\x07echo hi\r\n\x1b]133;C\x07hello\r\nworld\x1b]133;D\x07",
    );

    let output_ref = t
        .grid_ref(crate::point::Point::active(0, 1))
        .expect("output grid ref");
    let selection = t
        .select_output(&output_ref)
        .expect("semantic output selection");
    t.set_selection(Some(&selection))
        .expect("install output selection");

    let selected = t
        .selection_format(None, Format::Plain, true, true)
        .expect("formatted output selection");
    assert_eq!(String::from_utf8(selected).unwrap(), "hello\nworld");
}

#[test]
fn osc_133_command_input_can_be_selected_without_the_prompt() {
    use crate::selection::Format;

    let (mut t, _rs) = fresh_terminal();
    t.vt_write(
        b"\x1b]133;A\x07$ \x1b]133;B\x07echo hi\r\n\x1b]133;C\x07hello\r\nworld\x1b]133;D\x07",
    );

    let input_ref = t
        .grid_ref(crate::point::Point::active(2, 0))
        .expect("input grid ref");
    let selection = t
        .select_line(&input_ref, &[], true)
        .expect("semantic input selection");
    t.set_selection(Some(&selection))
        .expect("install input selection");

    let selected = t
        .selection_format(None, Format::Plain, true, true)
        .expect("formatted input selection");
    assert_eq!(String::from_utf8(selected).unwrap(), "echo hi");
}

#[test]
fn selection_gesture_can_use_command_output_as_a_multiclick_behavior() {
    use crate::gesture::{
        BEHAVIOR_CELL, BEHAVIOR_OUTPUT, BEHAVIOR_WORD, GestureBehaviors, SelectionGesture,
        SelectionGestureEvent, SurfacePosition,
    };
    use crate::selection::Format;

    let (mut t, _rs) = fresh_terminal();
    t.vt_write(
        b"\x1b]133;A\x07$ \x1b]133;B\x07echo hi\r\n\x1b]133;C\x07hello\r\nworld\x1b]133;D\x07",
    );

    let gesture = SelectionGesture::new(&t).expect("selection gesture");
    let press_type =
        crate::ffi::GhosttySelectionGestureEventType_GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_PRESS;
    let output_ref = t
        .grid_ref(crate::point::Point::active(0, 1))
        .expect("output grid ref");
    let mut press = SelectionGestureEvent::new(press_type).expect("press event");
    press.set_ref(&output_ref);
    press.set_position(SurfacePosition { x: 0.0, y: 1.0 });
    press.set_repeat_interval_ns(500_000_000);
    press.set_repeat_distance(8.0);
    press.set_behaviors(GestureBehaviors {
        single_click: BEHAVIOR_CELL,
        double_click: BEHAVIOR_WORD,
        triple_click: BEHAVIOR_OUTPUT,
    });

    press.set_time_ns(1);
    assert!(gesture.dispatch(&t, &press).expect("first press").is_none());
    press.set_time_ns(2);
    assert!(
        gesture
            .dispatch(&t, &press)
            .expect("second press")
            .is_some()
    );
    press.set_time_ns(3);
    let selection = gesture
        .dispatch(&t, &press)
        .expect("third press")
        .expect("output selection");
    t.set_selection(Some(&selection))
        .expect("install output selection");

    let selected = t
        .selection_format(None, Format::Plain, true, true)
        .expect("formatted output selection");
    assert_eq!(String::from_utf8(selected).unwrap(), "hello\nworld");
}

#[test]
fn captures_styled_content_and_resolves_colors() {
    let (mut t, mut rs) = fresh_terminal();
    // bold green "world", then reset
    t.vt_write(b"Hello, \x1b[1;32mworld\x1b[0m!");

    let snap = FrameSnapshot::capture(&mut t, &mut rs);
    assert_eq!(snap.cols, 40);
    assert_eq!(snap.rows, 5);
    assert_eq!(
        snap.dirty,
        Dirty::Full,
        "first frame after writes should be full-dirty"
    );

    let row0 = &snap.rows_data[0];
    assert!(row0.dirty, "row 0 should be marked dirty");
    assert_eq!(row_text(row0), "Hello, world!");

    // Columns 7..12 are "world" — they must be bold with fg = palette[2] (green).
    let green = snap.colors.palette[2];
    for cell in &row0.cells[7..12] {
        assert!(
            cell.style.bold,
            "world cell {:?} should be bold",
            cell.grapheme
        );
        assert_eq!(
            cell.fg,
            Some(green),
            "world cell {:?} should resolve to palette[2]",
            cell.grapheme
        );
    }
    // Non-world cells are not bold.
    assert!(!row0.cells[0].style.bold, "'H' should not be bold");
}

#[test]
fn dirty_resets_after_clear_and_no_changes() {
    let (mut t, mut rs) = fresh_terminal();
    t.vt_write(b"something");

    let snap1 = FrameSnapshot::capture(&mut t, &mut rs);
    assert_ne!(snap1.dirty, Dirty::False, "must be dirty after writes");

    rs.clear_dirty().expect("clear_dirty");

    // No new writes between captures → render_state_update finds nothing.
    let snap2 = FrameSnapshot::capture(&mut t, &mut rs);
    assert_eq!(
        snap2.dirty,
        Dirty::False,
        "dirty should be False after clear with no new writes"
    );
}

#[test]
fn cursor_is_visible_after_writes() {
    let (mut t, mut rs) = fresh_terminal();
    t.vt_write(b"ab");
    let snap = FrameSnapshot::capture(&mut t, &mut rs);
    assert!(snap.cursor.visible, "cursor should be visible");
    let (cx, cy) = snap
        .cursor
        .viewport
        .expect("cursor should be in viewport after writes");
    assert_eq!(cx, 2, "cursor column after 'ab' should be 2");
    assert_eq!(cy, 0, "cursor row should be 0");
}

#[test]
fn csi_cursor_left_moves_visible_cursor() {
    let (mut t, mut rs) = fresh_terminal();
    t.vt_write(b"abc\x1b[D");

    let snap = FrameSnapshot::capture(&mut t, &mut rs);
    let (cx, cy) = snap
        .cursor
        .viewport
        .expect("cursor should be in viewport after CUB");
    assert_eq!(cx, 2, "cursor column after CUB should move left");
    assert_eq!(cy, 0, "cursor row should not change");
    assert_eq!(row_text(&snap.rows_data[0]), "abc");
}

#[test]
fn cursor_only_move_updates_snapshot_even_when_not_dirty() {
    let (mut t, mut rs) = fresh_terminal();
    t.vt_write(b"abc");
    let _ = FrameSnapshot::capture(&mut t, &mut rs);
    rs.clear_dirty().expect("clear_dirty");

    t.vt_write(b"\x1b[D");
    let snap = FrameSnapshot::capture(&mut t, &mut rs);

    assert_eq!(
        snap.dirty,
        Dirty::False,
        "libghostty-vt does not mark cursor-only moves dirty"
    );
    assert_eq!(snap.cursor.viewport, Some((2, 0)));
}

#[test]
fn installed_selection_is_captured_as_per_row_ranges() {
    let (mut t, mut rs) = fresh_terminal();
    t.vt_write(b"hello world");

    // No selection yet → no row carries a range.
    let snap = FrameSnapshot::capture(&mut t, &mut rs);
    assert!(
        snap.rows_data[0].selection.is_none(),
        "no selection should be installed initially"
    );
    rs.clear_dirty().expect("clear_dirty");

    // select_word at column 0 → "hello" (cols 0..4), install it.
    let point = crate::point::Point::active(0, 0);
    let ref_ = t.grid_ref(point).expect("grid_ref");
    let sel = t.select_word(&ref_, &[]).expect("select_word");
    t.set_selection(Some(&sel)).expect("set_selection");

    let snap2 = FrameSnapshot::capture(&mut t, &mut rs);
    let range = snap2.rows_data[0]
        .selection
        .expect("row 0 should be selected");
    assert!(
        range.0 <= 4 && range.1 >= 4,
        "word \"hello\" (cols 0..4) should be selected; got {range:?}"
    );

    // Clearing the selection removes the range.
    t.set_selection(None).expect("clear selection");
    let snap3 = FrameSnapshot::capture(&mut t, &mut rs);
    assert!(
        snap3.rows_data[0].selection.is_none(),
        "selection range should clear after clearing the installed selection"
    );
}
