//! Selection value type, semantic derives, mutation/query helpers, and
//! one-shot clipboard formatting. Wraps `GhosttySelection` (selection.h) and
//! the `ghostty_terminal_select_*` / `ghostty_terminal_selection_*` families.
//!
//! A [`Selection`] is a **snapshot** carrying two untracked [`GridRef`](crate::grid_ref::GridRef)s
//! — valid only until the next mutating terminal op. The terminal owns a
//! tracked active selection (installed via [`Terminal::set_selection`]); the
//! render-state machinery reads that one and exposes per-row highlight ranges.
//!
//! ## Reference
//!
//! `include/ghostty/vt/selection.h`, `example/c-vt-selection/src/main.c`,
//! `example/c-vt-render/src/main.c`.

use crate::ffi;
use crate::terminal::Terminal;
use crate::{Error, grid_ref::GridRef, point::Point};

/// Selection endpoint ordering, for [`Terminal::selection_order`] /
/// [`Terminal::selection_ordered`]. Mirrors `GhosttySelectionOrder`.
pub use ffi::GhosttySelectionOrder as SelectionOrder;
pub use ffi::GhosttySelectionOrder_GHOSTTY_SELECTION_ORDER_FORWARD as ORDER_FORWARD;
pub use ffi::GhosttySelectionOrder_GHOSTTY_SELECTION_ORDER_MIRRORED_FORWARD as ORDER_MIRRORED_FORWARD;
pub use ffi::GhosttySelectionOrder_GHOSTTY_SELECTION_ORDER_MIRRORED_REVERSE as ORDER_MIRRORED_REVERSE;
pub use ffi::GhosttySelectionOrder_GHOSTTY_SELECTION_ORDER_REVERSE as ORDER_REVERSE;

/// Output format for [`Terminal::selection_format`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Plain,
    Vt,
    Html,
}

/// A selection snapshot: two inclusive [`GridRef`] endpoints plus a
/// rectangle flag. Endpoints may be in either order (start may be after end
/// visually); the direction is preserved and matters for keyboard adjustment.
#[derive(Clone, Copy)]
pub struct Selection {
    pub(crate) raw: ffi::GhosttySelection,
}

impl Selection {
    /// Construct a zero/empty selection (null endpoints) usable as an
    /// out-parameter for the derive functions.
    pub fn empty() -> Self {
        Self {
            raw: ffi::GhosttySelection {
                size: core::mem::size_of::<ffi::GhosttySelection>(),
                start: ffi::GhosttyGridRef {
                    size: core::mem::size_of::<ffi::GhosttyGridRef>(),
                    node: core::ptr::null_mut(),
                    x: 0,
                    y: 0,
                },
                end: ffi::GhosttyGridRef {
                    size: core::mem::size_of::<ffi::GhosttyGridRef>(),
                    node: core::ptr::null_mut(),
                    x: 0,
                    y: 0,
                },
                rectangle: false,
            },
        }
    }

    /// Start endpoint (inclusive; may be visually after `end`).
    pub fn start(&self) -> GridRef {
        GridRef::from(self.raw.start)
    }
    /// End endpoint (inclusive).
    pub fn end(&self) -> GridRef {
        GridRef::from(self.raw.end)
    }
    /// Rectangle / block mode (vs linear).
    pub fn rectangle(&self) -> bool {
        self.raw.rectangle
    }

    /// Construct a selection snapshot from two untracked endpoint refs.
    pub fn from_refs(start: GridRef, end: GridRef, rectangle: bool) -> Self {
        Self {
            raw: ffi::GhosttySelection {
                size: core::mem::size_of::<ffi::GhosttySelection>(),
                start: start.raw,
                end: end.raw,
                rectangle,
            },
        }
    }
}

impl Terminal {
    // ---- installing / reading the active selection ----

    /// Install `selection` as the terminal's tracked active selection. The
    /// terminal copies it into tracked state; the passed refs need not outlive
    /// the call. Render-state row iteration then exposes per-row highlight
    /// ranges. Pass `None` to clear.
    pub fn set_selection(&mut self, selection: Option<&Selection>) -> Result<(), Error> {
        let result = match selection {
            Some(s) => {
                let ptr = &s.raw as *const ffi::GhosttySelection as *const core::ffi::c_void;
                unsafe {
                    ffi::ghostty_terminal_set(
                        self.as_raw(),
                        ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_SELECTION,
                        ptr,
                    )
                }
            }
            None => unsafe {
                ffi::ghostty_terminal_set(
                    self.as_raw(),
                    ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_SELECTION,
                    core::ptr::null(),
                )
            },
        };
        Error::from_result(result)
    }

    /// Read the terminal's active selection as an untracked snapshot. `None`
    /// when there is no active selection. The snapshot is valid only until the
    /// next mutating terminal op.
    pub fn selection(&self) -> Option<Selection> {
        let mut out = ffi::GhosttySelection {
            size: core::mem::size_of::<ffi::GhosttySelection>(),
            start: ffi::GhosttyGridRef {
                size: core::mem::size_of::<ffi::GhosttyGridRef>(),
                node: core::ptr::null_mut(),
                x: 0,
                y: 0,
            },
            end: ffi::GhosttyGridRef {
                size: core::mem::size_of::<ffi::GhosttyGridRef>(),
                node: core::ptr::null_mut(),
                x: 0,
                y: 0,
            },
            rectangle: false,
        };
        let result = unsafe {
            ffi::ghostty_terminal_get(
                self.as_raw(),
                ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_SELECTION,
                &mut out as *mut ffi::GhosttySelection as *mut core::ffi::c_void,
            )
        };
        if result == ffi::GhosttyResult_GHOSTTY_SUCCESS {
            Some(Selection { raw: out })
        } else {
            None
        }
    }

    // ---- semantic derives (double/triple-click, select-all, output) ----

    /// Word selection around `ref_` (double-click). `boundaries` overrides the
    /// default word-delimiter codepoints when non-empty.
    pub fn select_word(&self, ref_: &GridRef, boundaries: &[u32]) -> Result<Selection, Error> {
        let opts = ffi::GhosttyTerminalSelectWordOptions {
            size: core::mem::size_of::<ffi::GhosttyTerminalSelectWordOptions>(),
            ref_: ref_.raw,
            boundary_codepoints: if boundaries.is_empty() {
                core::ptr::null()
            } else {
                boundaries.as_ptr()
            },
            boundary_codepoints_len: boundaries.len(),
        };
        let mut out = Selection::empty();
        let result =
            unsafe { ffi::ghostty_terminal_select_word(self.as_raw(), &opts, &mut out.raw) };
        Error::from_result(result)?;
        Ok(out)
    }

    /// Triple-click line selection around `ref_`. `semantic_prompt_boundary`
    /// bounds the line by OSC 133 prompt markers (selects input, not the
    /// leading prompt).
    pub fn select_line(
        &self,
        ref_: &GridRef,
        whitespace: &[u32],
        semantic_prompt_boundary: bool,
    ) -> Result<Selection, Error> {
        let opts = ffi::GhosttyTerminalSelectLineOptions {
            size: core::mem::size_of::<ffi::GhosttyTerminalSelectLineOptions>(),
            ref_: ref_.raw,
            whitespace: if whitespace.is_empty() {
                core::ptr::null()
            } else {
                whitespace.as_ptr()
            },
            whitespace_len: whitespace.len(),
            semantic_prompt_boundary,
        };
        let mut out = Selection::empty();
        let result =
            unsafe { ffi::ghostty_terminal_select_line(self.as_raw(), &opts, &mut out.raw) };
        Error::from_result(result)?;
        Ok(out)
    }

    /// Semantic command-output selection containing `ref_` (OSC 133-bounded).
    pub fn select_output(&self, ref_: &GridRef) -> Result<Selection, Error> {
        let mut out = Selection::empty();
        let result =
            unsafe { ffi::ghostty_terminal_select_output(self.as_raw(), ref_.raw, &mut out.raw) };
        Error::from_result(result)?;
        Ok(out)
    }

    // ---- mutation / query ----

    /// Query whether `point` falls within `selection` (handles rect/block and
    /// multi-row linear). Used for ad-hoc hit-testing; the per-row highlight
    /// path goes through the render state instead.
    pub fn selection_contains(&self, selection: &Selection, point: Point) -> bool {
        let mut contains = false;
        let result = unsafe {
            ffi::ghostty_terminal_selection_contains(
                self.as_raw(),
                &selection.raw,
                point.to_ffi(),
                &mut contains,
            )
        };
        result == ffi::GhosttyResult_GHOSTTY_SUCCESS && contains
    }

    /// Read the endpoint ordering of `selection`.
    pub fn selection_order(&self, selection: &Selection) -> SelectionOrder {
        let mut order = ffi::GhosttySelectionOrder_GHOSTTY_SELECTION_ORDER_FORWARD;
        let _ = unsafe {
            ffi::ghostty_terminal_selection_order(self.as_raw(), &selection.raw, &mut order)
        };
        order
    }

    /// Format the active selection (or the terminal's installed one when
    /// `selection` is `None`) into owned bytes. The copy/clipboard path:
    /// pass `Format::Plain` with unwrap+trim for ghostty-equivalent clipboard
    /// text. Returns `None` when there's no selection to format.
    pub fn selection_format(
        &self,
        selection: Option<&Selection>,
        format: Format,
        unwrap: bool,
        trim: bool,
    ) -> Option<Vec<u8>> {
        let emit = match format {
            Format::Plain => ffi::GhosttyFormatterFormat_GHOSTTY_FORMATTER_FORMAT_PLAIN,
            Format::Vt => ffi::GhosttyFormatterFormat_GHOSTTY_FORMATTER_FORMAT_VT,
            Format::Html => ffi::GhosttyFormatterFormat_GHOSTTY_FORMATTER_FORMAT_HTML,
        };
        let opts = ffi::GhosttyTerminalSelectionFormatOptions {
            size: core::mem::size_of::<ffi::GhosttyTerminalSelectionFormatOptions>(),
            emit,
            unwrap,
            trim,
            selection: match selection {
                Some(s) => &s.raw as *const ffi::GhosttySelection,
                None => core::ptr::null(),
            },
        };
        let mut ptr: *mut u8 = core::ptr::null_mut();
        let mut len = 0usize;
        let result = unsafe {
            ffi::ghostty_terminal_selection_format_alloc(
                self.as_raw(),
                core::ptr::null(),
                opts,
                &mut ptr,
                &mut len,
            )
        };
        if result != ffi::GhosttyResult_GHOSTTY_SUCCESS || ptr.is_null() {
            return None;
        }
        // SAFETY: on success the library returns a buffer of `len` bytes owned
        // by us; free with ghostty_free(default-allocator, ptr, len).
        let bytes = unsafe { core::slice::from_raw_parts(ptr, len) }.to_vec();
        unsafe { ffi::ghostty_free(core::ptr::null(), ptr, len) };
        Some(bytes)
    }
}
