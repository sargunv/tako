//! Selection gesture state machine: drives press/drag/release/autoscroll/
//! deep-press + multi-click behaviors from a stream of pointer events. Wraps
//! `GhosttySelectionGesture` + `GhosttySelectionGestureEvent` (selection.h).
//!
//! The [`SelectionGesture`] handle is GUI-thread-owned and `!Send` — it
//! borrows the `!Send` terminal per event and anchors tracked refs in it. The
//! terminal must outlive the gesture; [`SelectionGesture::new`] borrows the
//! terminal to capture its raw pointer for drop-time freeing. Enforce this by
//! declaring the gesture owner before the terminal owner in the containing
//! struct (Rust drops fields in declaration order).
//!
//! ## Reference
//!
//! `include/ghostty/vt/selection.h`, `example/c-vt-selection-gesture/src/main.c`.

use crate::ffi;
use crate::terminal::Terminal;
use crate::{Error, selection::Selection};

pub use ffi::GhosttySelectionGestureAutoscroll as GestureAutoscroll;
pub use ffi::GhosttySelectionGestureAutoscroll_GHOSTTY_SELECTION_GESTURE_AUTOSCROLL_DOWN as AUTOSCROLL_DOWN;
pub use ffi::GhosttySelectionGestureAutoscroll_GHOSTTY_SELECTION_GESTURE_AUTOSCROLL_NONE as AUTOSCROLL_NONE;
pub use ffi::GhosttySelectionGestureAutoscroll_GHOSTTY_SELECTION_GESTURE_AUTOSCROLL_UP as AUTOSCROLL_UP;
/// Multi-click behavior for a click count (single/double/triple). Mirrors
/// `GhosttySelectionGestureBehavior`.
pub use ffi::GhosttySelectionGestureBehavior as GestureBehavior;
pub use ffi::GhosttySelectionGestureBehavior_GHOSTTY_SELECTION_GESTURE_BEHAVIOR_CELL as BEHAVIOR_CELL;
pub use ffi::GhosttySelectionGestureBehavior_GHOSTTY_SELECTION_GESTURE_BEHAVIOR_LINE as BEHAVIOR_LINE;
pub use ffi::GhosttySelectionGestureBehavior_GHOSTTY_SELECTION_GESTURE_BEHAVIOR_OUTPUT as BEHAVIOR_OUTPUT;
pub use ffi::GhosttySelectionGestureBehavior_GHOSTTY_SELECTION_GESTURE_BEHAVIOR_WORD as BEHAVIOR_WORD;

/// Behavior table for single/double/triple clicks. Defaults when unset: cell,
/// word, line.
#[derive(Debug, Clone, Copy)]
pub struct GestureBehaviors {
    pub single_click: GestureBehavior,
    pub double_click: GestureBehavior,
    pub triple_click: GestureBehavior,
}

impl GestureBehaviors {
    /// Ghostty defaults: single=cell, double=word, triple=line.
    pub const DEFAULT: Self = Self {
        single_click: BEHAVIOR_CELL,
        double_click: BEHAVIOR_WORD,
        triple_click: BEHAVIOR_LINE,
    };

    fn to_ffi(self) -> ffi::GhosttySelectionGestureBehaviors {
        ffi::GhosttySelectionGestureBehaviors {
            single_click: self.single_click,
            double_click: self.double_click,
            triple_click: self.triple_click,
        }
    }
}

/// Display geometry for drag/autoscroll: maps surface px → cell and detects
/// autoscroll edges. Mirrors `GhosttySelectionGestureGeometry`.
#[derive(Debug, Clone, Copy, Default)]
pub struct GestureGeometry {
    pub columns: u32,
    pub cell_width: u32,
    pub padding_left: u32,
    pub screen_height: u32,
}

impl GestureGeometry {
    fn to_ffi(self) -> ffi::GhosttySelectionGestureGeometry {
        ffi::GhosttySelectionGestureGeometry {
            columns: self.columns,
            cell_width: self.cell_width,
            padding_left: self.padding_left,
            screen_height: self.screen_height,
        }
    }
}

/// Surface-pixel pointer position (top-left origin). Mirrors
/// `GhosttySurfacePosition`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SurfacePosition {
    pub x: f64,
    pub y: f64,
}

/// The reusable gesture-event handle, fixed to one event type at construction.
/// Set options then dispatch via [`SelectionGesture::dispatch`].
pub struct SelectionGestureEvent {
    handle: ffi::GhosttySelectionGestureEvent,
}

impl SelectionGestureEvent {
    /// Create a reusable event of the given type.
    pub fn new(event_type: ffi::GhosttySelectionGestureEventType) -> Result<Self, Error> {
        let mut handle: ffi::GhosttySelectionGestureEvent = core::ptr::null_mut();
        let result = unsafe {
            ffi::ghostty_selection_gesture_event_new(core::ptr::null(), &mut handle, event_type)
        };
        Error::from_result(result)?;
        assert!(
            !handle.is_null(),
            "ghostty_selection_gesture_event_new returned null"
        );
        Ok(Self { handle })
    }

    /// Clear a previously-set option (subsequent dispatches won't include it).
    pub fn clear(&mut self, option: ffi::GhosttySelectionGestureEventOption) {
        let _ = unsafe {
            ffi::ghostty_selection_gesture_event_set(self.handle, option, core::ptr::null())
        };
    }

    /// Set the grid ref under the pointer (required for PRESS + DRAG).
    pub fn set_ref(&mut self, ref_: &crate::grid_ref::GridRef) {
        let _ = unsafe {
            ffi::ghostty_selection_gesture_event_set(
                self.handle,
                ffi::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF,
                &ref_.raw as *const ffi::GhosttyGridRef as *const core::ffi::c_void,
            )
        };
    }

    /// Set the surface-pixel pointer position (for autoscroll edge detection).
    pub fn set_position(&mut self, pos: SurfacePosition) {
        let raw = ffi::GhosttySurfacePosition { x: pos.x, y: pos.y };
        let _ = unsafe {
            ffi::ghostty_selection_gesture_event_set(
                self.handle,
                ffi::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_POSITION,
                &raw as *const ffi::GhosttySurfacePosition as *const core::ffi::c_void,
            )
        };
    }

    /// Set the monotonic event timestamp in ns (enables multi-click counting).
    pub fn set_time_ns(&mut self, time_ns: u64) {
        let _ = unsafe {
            ffi::ghostty_selection_gesture_event_set(
                self.handle,
                ffi::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_TIME_NS,
                &time_ns as *const u64 as *const core::ffi::c_void,
            )
        };
    }

    /// Set the max interval between repeat clicks in ns (multi-click window).
    pub fn set_repeat_interval_ns(&mut self, interval_ns: u64) {
        let _ = unsafe {
            ffi::ghostty_selection_gesture_event_set(
                self.handle,
                ffi::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REPEAT_INTERVAL_NS,
                &interval_ns as *const u64 as *const core::ffi::c_void,
            )
        };
    }

    /// Set the max pixel distance between repeat clicks.
    pub fn set_repeat_distance(&mut self, pixels: f64) {
        let _ = unsafe {
            ffi::ghostty_selection_gesture_event_set(
                self.handle,
                ffi::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REPEAT_DISTANCE,
                &pixels as *const f64 as *const core::ffi::c_void,
            )
        };
    }

    /// Set the behavior table (single/double/triple click semantics).
    pub fn set_behaviors(&mut self, behaviors: GestureBehaviors) {
        let raw = behaviors.to_ffi();
        let _ = unsafe {
            ffi::ghostty_selection_gesture_event_set(
                self.handle,
                ffi::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_BEHAVIORS,
                &raw as *const ffi::GhosttySelectionGestureBehaviors as *const core::ffi::c_void,
            )
        };
    }

    /// Set whether drag/autoscroll produces a rectangular selection (Alt-drag).
    pub fn set_rectangle(&mut self, rectangle: bool) {
        let _ = unsafe {
            ffi::ghostty_selection_gesture_event_set(
                self.handle,
                ffi::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_RECTANGLE,
                &rectangle as *const bool as *const core::ffi::c_void,
            )
        };
    }

    /// Set the display geometry (required for DRAG + AUTOSCROLL_TICK).
    pub fn set_geometry(&mut self, geometry: GestureGeometry) {
        let raw = geometry.to_ffi();
        let _ = unsafe {
            ffi::ghostty_selection_gesture_event_set(
                self.handle,
                ffi::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_GEOMETRY,
                &raw as *const ffi::GhosttySelectionGestureGeometry as *const core::ffi::c_void,
            )
        };
    }

    /// Set the viewport cell coordinate for an autoscroll tick.
    pub fn set_viewport(&mut self, coord: crate::point::PointCoordinate) {
        let raw: ffi::GhosttyPointCoordinate = coord.into();
        let _ = unsafe {
            ffi::ghostty_selection_gesture_event_set(
                self.handle,
                ffi::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_VIEWPORT,
                &raw as *const ffi::GhosttyPointCoordinate as *const core::ffi::c_void,
            )
        };
    }
}

impl Drop for SelectionGestureEvent {
    fn drop(&mut self) {
        // free allows NULL; our handle is non-null by construction.
        unsafe { ffi::ghostty_selection_gesture_event_free(self.handle) };
    }
}

/// Per-pointer selection gesture state. Owns the opaque gesture handle and a
/// borrowed terminal pointer (for drop-time freeing).
///
/// **Lifetime:** the `Terminal` passed to [`Self::new`] must outlive this
/// gesture. The gesture is `!Send` (it holds a raw terminal pointer and
/// borrows it per event).
pub struct SelectionGesture {
    handle: ffi::GhosttySelectionGesture,
    /// Borrowed terminal pointer, captured at construction for drop-time
    /// freeing. Valid because the terminal must outlive this gesture.
    terminal: ffi::GhosttyTerminal,
}

impl SelectionGesture {
    /// Create a gesture. `terminal` is borrowed only for its raw pointer; the
    /// caller must keep it alive for the gesture's lifetime.
    pub fn new(terminal: &Terminal) -> Result<Self, Error> {
        let mut handle: ffi::GhosttySelectionGesture = core::ptr::null_mut();
        let result = unsafe { ffi::ghostty_selection_gesture_new(core::ptr::null(), &mut handle) };
        Error::from_result(result)?;
        assert!(
            !handle.is_null(),
            "ghostty_selection_gesture_new returned null"
        );
        Ok(Self {
            handle,
            terminal: terminal.as_raw(),
        })
    }

    /// Reset gesture state (click count, anchor, dragged flag). Call to clear
    /// an in-progress gesture without producing a selection.
    pub fn reset(&mut self, terminal: &Terminal) {
        // SAFETY: terminal is the same one passed to new (invariant).
        unsafe { ffi::ghostty_selection_gesture_reset(self.handle, terminal.as_raw()) };
    }

    /// Apply `event` to this gesture against `terminal`. Returns `Ok(Some(sel))`
    /// when the event produced a selection snapshot (DRAG, AUTOSCROLL_TICK,
    /// DEEP_PRESS), `Ok(None)` when it didn't (PRESS, RELEASE), or an error.
    ///
    /// The returned [`Selection`] is an untracked snapshot; install it via
    /// [`Terminal::set_selection`](crate::terminal::Terminal::set_selection) to
    /// make it the active (tracked, rendered) selection.
    pub fn dispatch(
        &self,
        terminal: &Terminal,
        event: &SelectionGestureEvent,
    ) -> Result<Option<Selection>, Error> {
        let mut out = Selection::empty();
        let result = unsafe {
            ffi::ghostty_selection_gesture_event(
                self.handle,
                terminal.as_raw(),
                event.handle,
                &mut out.raw,
            )
        };
        if result == ffi::GhosttyResult_GHOSTTY_NO_VALUE {
            return Ok(None);
        }
        Error::from_result(result)?;
        Ok(Some(out))
    }

    /// Current click count (0 = inactive/no recent click).
    pub fn click_count(&self, terminal: &Terminal) -> u8 {
        let mut v: u8 = 0;
        let _ = unsafe {
            ffi::ghostty_selection_gesture_get(
                self.handle,
                terminal.as_raw(),
                ffi::GhosttySelectionGestureData_GHOSTTY_SELECTION_GESTURE_DATA_CLICK_COUNT,
                &mut v as *mut u8 as *mut core::ffi::c_void,
            )
        };
        v
    }

    /// `true` if the current gesture involved a drag (vs a pure click).
    pub fn dragged(&self, terminal: &Terminal) -> bool {
        let mut v: bool = false;
        let _ = unsafe {
            ffi::ghostty_selection_gesture_get(
                self.handle,
                terminal.as_raw(),
                ffi::GhosttySelectionGestureData_GHOSTTY_SELECTION_GESTURE_DATA_DRAGGED,
                &mut v as *mut bool as *mut core::ffi::c_void,
            )
        };
        v
    }

    /// Current autoscroll request for an active drag gesture.
    pub fn autoscroll(&self, terminal: &Terminal) -> GestureAutoscroll {
        let mut v = AUTOSCROLL_NONE;
        let _ = unsafe {
            ffi::ghostty_selection_gesture_get(
                self.handle,
                terminal.as_raw(),
                ffi::GhosttySelectionGestureData_GHOSTTY_SELECTION_GESTURE_DATA_AUTOSCROLL,
                &mut v as *mut GestureAutoscroll as *mut core::ffi::c_void,
            )
        };
        v
    }
}

impl Drop for SelectionGesture {
    fn drop(&mut self) {
        // SAFETY: the terminal captured at construction outlives this gesture
        // (documented lifetime invariant). free allows NULL too, but passing
        // the live terminal lets it release tracked refs cleanly.
        unsafe { ffi::ghostty_selection_gesture_free(self.handle, self.terminal) };
    }
}
