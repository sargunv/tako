//! Selection engine: drives the upstream `GhosttySelectionGesture` state
//! machine from a stream of pointer events. The analog of the input encoders
//! in [`tako_term::key`] / [`tako_term::mouse`] — it owns the per-surface
//! gesture handle + reusable event objects and resolves surface pixels to grid
//! refs.
//!
//! Threading: `!Send` — the [`SelectionGesture`](tako_term::gesture::SelectionGesture)
//! handle borrows the `!Send` terminal per event. The owner (typically
//! [`crate::Surface`]) must declare it before the panel so the gesture frees
//! before the terminal on drop.

use tako_term::gesture::{
    GestureBehaviors, GestureGeometry, SelectionGesture, SelectionGestureEvent, SurfacePosition,
};
use tako_term::grid_ref::GridRef;
use tako_term::point::Point;
use tako_term::selection::Selection;
use tako_term::terminal::Terminal;

use crate::Error;

/// xterm-default multi-click window (max interval between clicks to count as a
/// double/triple click).
const REPEAT_INTERVAL_NS: u64 = 500_000_000;

/// xterm-default max pixel distance between repeat clicks (a click outside this
/// radius from the previous resets the click count).
const REPEAT_DISTANCE_PX: f64 = 8.0;

/// Drives the libghostty-vt selection gesture from Qt-style mouse events.
///
/// The engine is created once per surface and reused. It owns the gesture
/// handle (anchored to the terminal passed at construction) plus reusable
/// PRESS/DRAG/RELEASE event objects. Geometry is pushed in via
/// [`Self::set_geometry`] whenever the grid resizes.
pub struct SelectionEngine {
    gesture: SelectionGesture,
    press: SelectionGestureEvent,
    drag: SelectionGestureEvent,
    release: SelectionGestureEvent,
    geometry: Geometry,
    behaviors: GestureBehaviors,
}

#[derive(Clone, Copy)]
struct Geometry {
    cols: u16,
    rows: u16,
    cell_w: u32,
    cell_h: u32,
}

impl Geometry {
    fn to_gesture(self) -> GestureGeometry {
        GestureGeometry {
            columns: self.cols as u32,
            cell_width: self.cell_w,
            padding_left: 0,
            screen_height: self.rows as u32 * self.cell_h,
        }
    }
}

impl SelectionEngine {
    /// Create the engine, anchoring the gesture to `terminal`. The terminal
    /// must outlive the engine (enforced by field declaration order in the
    /// owner).
    pub fn new(terminal: &Terminal) -> Result<Self, Error> {
        let press_type = tako_term::ffi::GhosttySelectionGestureEventType_GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_PRESS;
        let drag_type = tako_term::ffi::GhosttySelectionGestureEventType_GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_DRAG;
        let release_type = tako_term::ffi::GhosttySelectionGestureEventType_GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_RELEASE;
        Ok(Self {
            gesture: SelectionGesture::new(terminal)?,
            press: SelectionGestureEvent::new(press_type)?,
            drag: SelectionGestureEvent::new(drag_type)?,
            release: SelectionGestureEvent::new(release_type)?,
            geometry: Geometry {
                cols: 1,
                rows: 1,
                cell_w: 1,
                cell_h: 1,
            },
            behaviors: GestureBehaviors::DEFAULT,
        })
    }

    /// Update the grid + cell metrics (call on resize / DPR change). The engine
    /// uses these to map surface px → cell coords and to build the gesture
    /// geometry.
    pub fn set_geometry(&mut self, cols: u16, rows: u16, cell_w: u32, cell_h: u32) {
        self.geometry = Geometry {
            cols,
            rows,
            cell_w: cell_w.max(1),
            cell_h: cell_h.max(1),
        };
    }

    /// Resolve a surface-pixel position to a viewport-space grid ref, clamping
    /// to the grid bounds. Returns `None` only if the terminal rejects the
    /// point (shouldn't happen for an in-bounds point).
    fn px_to_grid_ref(&self, terminal: &Terminal, x_px: f32, y_px: f32) -> Option<GridRef> {
        let g = self.geometry;
        let col = ((x_px / g.cell_w as f32) as u32).min(g.cols.saturating_sub(1) as u32) as u16;
        let row = ((y_px / g.cell_h as f32) as u32).min(g.rows.saturating_sub(1) as u32);
        let point = Point::viewport(col, row);
        terminal.grid_ref(point).ok()
    }

    /// Pointer press — begins (or extends, for multi-click) a gesture. Returns
    /// the selection the press produced:
    /// - `None` for a single-click (cell behavior) — the caller should clear any
    ///   existing selection;
    /// - `Some(word)` for a double-click;
    /// - `Some(line)` for a triple-click.
    ///
    /// Follow with [`Self::extend`] / [`Self::end`]. `time_ns` is a monotonic
    /// timestamp (enables multi-click counting). `rectangle` requests block-select
    /// mode for the subsequent drag.
    pub fn begin(
        &mut self,
        terminal: &Terminal,
        x_px: f32,
        y_px: f32,
        time_ns: u64,
        rectangle: bool,
    ) -> Option<Selection> {
        let ref_ = self.px_to_grid_ref(terminal, x_px, y_px)?;
        self.press.set_ref(&ref_);
        self.press.set_position(SurfacePosition {
            x: x_px as f64,
            y: y_px as f64,
        });
        self.press.set_time_ns(time_ns);
        self.press.set_repeat_interval_ns(REPEAT_INTERVAL_NS);
        self.press.set_repeat_distance(REPEAT_DISTANCE_PX);
        self.press.set_behaviors(self.behaviors);
        // Rectangle is a drag-time option, but set it on press too so the
        // gesture records the intent from the start.
        self.press.set_rectangle(rectangle);
        self.gesture.dispatch(terminal, &self.press).ok().flatten()
    }

    /// Pointer drag — extend the selection to the current cell. Returns the new
    /// selection snapshot to install, or `None` if the gesture produced nothing
    /// (e.g. the drag landed outside the grid).
    pub fn extend(
        &mut self,
        terminal: &Terminal,
        x_px: f32,
        y_px: f32,
        rectangle: bool,
    ) -> Option<Selection> {
        let ref_ = self.px_to_grid_ref(terminal, x_px, y_px)?;
        self.drag.set_ref(&ref_);
        self.drag.set_position(SurfacePosition {
            x: x_px as f64,
            y: y_px as f64,
        });
        self.drag.set_geometry(self.geometry.to_gesture());
        self.drag.set_rectangle(rectangle);
        self.gesture.dispatch(terminal, &self.drag).ok().flatten()
    }

    /// Pointer release — finalize the gesture. Returns `true` if the gesture
    /// involved a drag (the caller may copy-on-select); `false` for a pure
    /// click (which selected a single cell / word / line via multi-click).
    pub fn end(&mut self, terminal: &Terminal, x_px: f32, y_px: f32) -> bool {
        if let Some(ref_) = self.px_to_grid_ref(terminal, x_px, y_px) {
            self.release.set_ref(&ref_);
        } else {
            self.release
                .clear(tako_term::ffi::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF);
        }
        let _ = self.gesture.dispatch(terminal, &self.release);
        self.gesture.dragged(terminal)
    }

    /// Current click count (for diagnostics / deciding word vs line semantics).
    pub fn click_count(&self, terminal: &Terminal) -> u8 {
        self.gesture.click_count(terminal)
    }

    /// Abort the in-progress gesture: reset gesture state. The caller should
    /// also clear the installed selection (the engine doesn't touch the
    /// terminal's installed state).
    pub fn abort(&mut self, terminal: &Terminal) {
        self.gesture.reset(terminal);
    }

    /// Borrow the underlying gesture handle (rarely needed externally).
    pub fn gesture(&self) -> &SelectionGesture {
        &self.gesture
    }
}
