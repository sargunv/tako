//! Grid reference helpers: stable cell handles that survive grid mutation,
//! plus the cell-content readers. Wraps `GhosttyGridRef` (grid_ref.h) and the
//! `ghostty_terminal_grid_ref` / `ghostty_grid_ref_*` accessors.
//!
//! A [`GridRef`] is an **untracked** snapshot — valid only until the next
//! mutating terminal op (`vt_write`, `set`, `resize`, `reset`). For long-lived
//! references (e.g. a selection that must survive scroll/reflow), the terminal
//! owns tracked state itself via `OPT_SELECTION`; raw refs are for one-shot
//! derive/format/contains calls.
//!
//! ## Reference
//!
//! `include/ghostty/vt/grid_ref.h`, `example/c-vt-selection/src/main.c`.

use crate::ffi;
use crate::terminal::Terminal;
use crate::{Error, point::Point};

/// An untracked grid reference: a stable-while-unmutated handle to a single
/// cell. Sized-struct ABI; `size` must be set to `sizeof(GhosttyGridRef)` at
/// construction. Use [`Terminal::grid_ref`] to produce one from a [`Point`].
#[derive(Clone, Copy)]
pub struct GridRef {
    pub(crate) raw: ffi::GhosttyGridRef,
}

impl GridRef {
    /// Zero-initialized (null node) grid ref. Only useful as an out-parameter
    /// for the derive functions; passing a null one to cell readers fails.
    pub fn empty() -> Self {
        Self {
            raw: ffi::GhosttyGridRef {
                size: core::mem::size_of::<ffi::GhosttyGridRef>(),
                node: core::ptr::null_mut(),
                x: 0,
                y: 0,
            },
        }
    }

    /// Column of this ref (set by `grid_ref`). Stable for the ref's lifetime.
    pub fn x(&self) -> u16 {
        self.raw.x
    }
    /// Row of this ref (set by `grid_ref`).
    pub fn y(&self) -> u16 {
        self.raw.y
    }

    /// Read this cell's full grapheme cluster as UTF-8.
    ///
    /// Returns `None` for a blank cell or an invalid ref.
    pub fn graphemes(&self) -> Option<String> {
        // Two-pass: probe with cap 0 to learn the required byte length.
        let mut probe_len = 0usize;
        let probe_result = unsafe {
            ffi::ghostty_grid_ref_graphemes(&self.raw, core::ptr::null_mut(), 0, &mut probe_len)
        };
        if probe_result != ffi::GhosttyResult_GHOSTTY_SUCCESS || probe_len == 0 {
            return None;
        }
        let mut buf = vec![0u32; probe_len];
        let mut written = 0usize;
        let result = unsafe {
            ffi::ghostty_grid_ref_graphemes(&self.raw, buf.as_mut_ptr(), buf.len(), &mut written)
        };
        if result != ffi::GhosttyResult_GHOSTTY_SUCCESS || written == 0 {
            return None;
        }
        buf.truncate(written);
        // Build a String from the codepoint vec. char::from_u32 filters
        // surrogates/invalid; lossy-insert keeps the count meaningful.
        let mut s = String::with_capacity(written);
        for cp in buf {
            if let Some(c) = char::from_u32(cp) {
                s.push(c);
            }
        }
        if s.is_empty() { None } else { Some(s) }
    }
}

impl From<ffi::GhosttyGridRef> for GridRef {
    fn from(raw: ffi::GhosttyGridRef) -> Self {
        Self { raw }
    }
}

impl Terminal {
    /// Resolve a [`Point`] to an untracked [`GridRef`] for the current screen.
    /// The returned ref is valid only until the next mutating terminal op.
    pub fn grid_ref(&self, point: Point) -> Result<GridRef, Error> {
        let mut raw = ffi::GhosttyGridRef {
            size: core::mem::size_of::<ffi::GhosttyGridRef>(),
            node: core::ptr::null_mut(),
            x: 0,
            y: 0,
        };
        let result =
            unsafe { ffi::ghostty_terminal_grid_ref(self.as_raw(), point.to_ffi(), &mut raw) };
        Error::from_result(result)?;
        Ok(GridRef { raw })
    }

    /// Invert a [`GridRef`] back to a coordinate in the requested tag's space.
    /// Returns `None` when the ref can't be expressed in that tag (e.g. a
    /// scrollback cell in ACTIVE coordinates), or when the ref is stale.
    pub fn point_from_grid_ref(
        &self,
        ref_: &GridRef,
        tag: crate::point::PointTag,
    ) -> Option<crate::point::PointCoordinate> {
        let mut out: ffi::GhosttyPointCoordinate = ffi::GhosttyPointCoordinate { x: 0, y: 0 };
        let result = unsafe {
            ffi::ghostty_terminal_point_from_grid_ref(self.as_raw(), &ref_.raw, tag, &mut out)
        };
        if result == ffi::GhosttyResult_GHOSTTY_SUCCESS {
            Some(crate::point::PointCoordinate::from(out))
        } else {
            None
        }
    }
}
