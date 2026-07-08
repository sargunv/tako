//! Point coordinate types: the `(tag, x, y)` triple used to address a cell in
//! the terminal's grid. Wraps `GhosttyPoint` / `GhosttyPointCoordinate` /
//! `GhosttyPointTag` (point.h).
//!
//! ## Reference
//!
//! `include/ghostty/vt/point.h`, `example/c-vt-selection/src/main.c`.

use crate::ffi;

/// Which grid space a [`Point`] is expressed in.
///
/// The same logical cell has different coordinates depending on the tag: the
/// viewport shifts as you scroll, the screen includes scrollback, and the
/// active area is where the cursor can move. Convert with
/// [`crate::grid_ref`] round-trips when you need to cross spaces.
pub use ffi::GhosttyPointTag as PointTag;
pub use ffi::GhosttyPointTag_GHOSTTY_POINT_TAG_ACTIVE as TAG_ACTIVE;
pub use ffi::GhosttyPointTag_GHOSTTY_POINT_TAG_HISTORY as TAG_HISTORY;
pub use ffi::GhosttyPointTag_GHOSTTY_POINT_TAG_SCREEN as TAG_SCREEN;
pub use ffi::GhosttyPointTag_GHOSTTY_POINT_TAG_VIEWPORT as TAG_VIEWPORT;

/// A `(x, y)` cell coordinate. `x` is the column (0-based, left→right); `y` is
/// the row. For the `VIEWPORT`/`SCREEN`/`HISTORY` tags, `y` may exceed the
/// visible row count to address scrollback.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PointCoordinate {
    pub x: u16,
    pub y: u32,
}

impl PointCoordinate {
    pub fn new(x: u16, y: u32) -> Self {
        Self { x, y }
    }
}

impl From<ffi::GhosttyPointCoordinate> for PointCoordinate {
    fn from(c: ffi::GhosttyPointCoordinate) -> Self {
        Self { x: c.x, y: c.y }
    }
}

impl From<PointCoordinate> for ffi::GhosttyPointCoordinate {
    fn from(c: PointCoordinate) -> Self {
        Self { x: c.x, y: c.y }
    }
}

/// A tagged cell address — the (tag, coordinate) pair libghostty-vt uses to
/// resolve a [`crate::grid_ref::GridRef`] or test
/// [`crate::selection::contains`].
#[derive(Debug, Clone, Copy)]
pub struct Point {
    pub tag: PointTag,
    pub coord: PointCoordinate,
}

impl Point {
    /// Build a viewport-space point: column `x`, row `y` within the visible
    /// window (row 0 = top of the viewport). This is what mouse events resolve
    /// to.
    pub fn viewport(x: u16, y: u32) -> Self {
        Self {
            tag: TAG_VIEWPORT,
            coord: PointCoordinate { x, y },
        }
    }

    /// Build an active-area point: the region where the cursor can move
    /// (excludes scrollback).
    pub fn active(x: u16, y: u32) -> Self {
        Self {
            tag: TAG_ACTIVE,
            coord: PointCoordinate { x, y },
        }
    }

    /// Convert to the raw FFI value (owned; safe to pass across the boundary).
    pub fn to_ffi(self) -> ffi::GhosttyPoint {
        ffi::GhosttyPoint {
            tag: self.tag,
            value: ffi::GhosttyPointValue {
                coordinate: self.coord.into(),
            },
        }
    }
}
