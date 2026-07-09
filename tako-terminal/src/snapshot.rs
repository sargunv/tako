//! Pure-Rust snapshot of a libghostty-vt render state, produced by walking the
//! libghostty-vt row/cell iterators.
//!
//! This mirrors the C `example/c-vt-render/src/main.c` embedding contract:
//! update the render state from the terminal, then read dirty state, colors,
//! cursor, and per-row cell data into renderer-consumable values. The snapshot
//! is plain owned data (`Vec`s, `String`s) safe to pass across threads and up
//! to a renderer.
//!
//! The capture helpers clear per-row dirty flags as they walk. The global dirty
//! flag is left untouched; the owner clears it after accepting the rendered
//! frame. Production does that in Zig because Zig owns the live render-state
//! handle.

#[cfg(test)]
use crate::ffi;
#[cfg(test)]
use crate::terminal::{RenderState, Terminal};

/// Frame-level dirty classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dirty {
    /// Nothing changed; the frame can be skipped entirely.
    False,
    /// Some rows changed; redraw only dirty rows.
    Partial,
    /// Global state changed; redraw everything.
    Full,
}

/// An RGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[cfg(test)]
impl From<ffi::GhosttyColorRgb> for Rgb {
    fn from(c: ffi::GhosttyColorRgb) -> Self {
        Self {
            r: c.r,
            g: c.g,
            b: c.b,
        }
    }
}

/// Terminal-wide colors resolved from the render state.
#[derive(Debug, Clone)]
pub struct Colors {
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Option<Rgb>,
    pub palette: [Rgb; 256],
}

/// Visual style of the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorStyle {
    Bar,
    Block,
    Underline,
    BlockHollow,
}

/// Cursor state from the render state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub visible: bool,
    /// `Some((x, y))` when the cursor is within the viewport, in cell coords.
    pub viewport: Option<(u16, u16)>,
    pub wide_tail: bool,
    pub style: CursorStyle,
    pub blinking: bool,
    pub password_input: bool,
}

/// A flattened cell style. Underline is a bool (present/absent); richer
/// underline kinds can be added when a renderer needs them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub blink: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub overline: bool,
    pub underline: bool,
}

/// A single cell's render data.
#[derive(Debug, Clone)]
pub struct Cell {
    /// The cell's grapheme cluster, UTF-8. Empty for a blank cell.
    pub grapheme: String,
    pub style: Style,
    /// Whether the cell should emit text glyphs after conceal/invisible and
    /// blank-cell handling.
    pub text_visible: bool,
    /// Final foreground color for glyph rendering after inverse/selection/faint
    /// resolution. Production computes this in Zig before crossing the planner
    /// ABI.
    pub text_fg: Rgb,
    /// Resolved foreground color, or `None` if the cell has no explicit fg
    /// (use the terminal foreground).
    pub fg: Option<Rgb>,
    /// Resolved background color, or `None` if the cell has no explicit bg.
    pub bg: Option<Rgb>,
}

/// One row of cells.
#[derive(Debug, Clone)]
pub struct Row {
    pub dirty: bool,
    /// Selected cell range for this row, inclusive, when the terminal has an
    /// installed selection intersecting this row. `None` when the row has no
    /// selection highlight (no active selection, or the row doesn't intersect
    /// it). The render state computes this from the installed selection,
    /// handling linear vs rectangle splitting.
    pub selection: Option<(u16, u16)>,
    pub cells: Vec<Cell>,
}

/// A complete snapshot of one viewport frame.
#[derive(Debug, Clone)]
pub struct FrameSnapshot {
    /// Viewport width in cells.
    pub cols: u16,
    /// Viewport height in cells.
    pub rows: u16,
    pub dirty: Dirty,
    pub colors: Colors,
    pub cursor: Cursor,
    /// One [`Row`] per viewport line, top to bottom.
    pub rows_data: Vec<Row>,
}

impl FrameSnapshot {
    /// Update `state` from `terminal`, then walk the render state into owned
    /// Rust data. Clears per-row dirty flags as it goes; the caller clears the
    /// global dirty flag after rendering.
    #[cfg(test)]
    pub fn capture(terminal: &mut Terminal, state: &mut RenderState) -> Self {
        state
            .update(terminal)
            .expect("ghostty_render_state_update failed");
        Self::capture_updated(state.as_raw())
    }

    #[cfg(test)]
    pub(crate) fn capture_updated(state: ffi::GhosttyRenderState) -> Self {
        Self::capture_updated_with_cursor(state, Cursor::capture(state))
    }

    #[cfg(test)]
    pub(crate) fn capture_updated_with_cursor(
        state: ffi::GhosttyRenderState,
        cursor: Cursor,
    ) -> Self {
        let cols = get_u16(
            state,
            ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_COLS,
        );
        let rows = get_u16(
            state,
            ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_ROWS,
        );
        let dirty = Dirty::from_raw(get_int(
            state,
            ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_DIRTY,
        ));
        let colors = Colors::capture(state);
        let rows_data = walk_rows(state, &colors);

        FrameSnapshot {
            cols,
            rows,
            dirty,
            colors,
            cursor,
            rows_data,
        }
    }
}

impl Dirty {
    #[cfg(test)]
    fn from_raw(v: ffi::GhosttyRenderStateDirty) -> Self {
        match v {
            ffi::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_FALSE => Self::False,
            ffi::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_PARTIAL => Self::Partial,
            ffi::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_FULL => Self::Full,
            _ => Self::Full,
        }
    }
}

impl Colors {
    #[cfg(test)]
    fn capture(state: ffi::GhosttyRenderState) -> Self {
        // SAFETY: zero-init is valid for this repr(C) POD struct; the library
        // reads `size` to decide how many fields to write.
        let mut raw: ffi::GhosttyRenderStateColors = unsafe { core::mem::zeroed() };
        raw.size = core::mem::size_of::<ffi::GhosttyRenderStateColors>();
        let result = unsafe { ffi::ghostty_render_state_colors_get(state, &mut raw as *mut _) };
        assert!(
            result == ffi::GhosttyResult_GHOSTTY_SUCCESS,
            "ghostty_render_state_colors_get failed"
        );

        let cursor = raw.cursor_has_value.then(|| Rgb::from(raw.cursor));
        let mut palette = [Rgb::default(); 256];
        palette.copy_from_slice(
            &raw.palette
                .iter()
                .copied()
                .map(Rgb::from)
                .collect::<Vec<_>>(),
        );
        Self {
            foreground: Rgb::from(raw.foreground),
            background: Rgb::from(raw.background),
            cursor,
            palette,
        }
    }
}

impl Cursor {
    #[cfg(test)]
    fn capture(state: ffi::GhosttyRenderState) -> Self {
        let visible = get_bool(
            state,
            ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VISIBLE,
        );
        let in_viewport = get_bool(
            state,
            ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_HAS_VALUE,
        );
        let viewport = if in_viewport {
            let x = get_u16(
                state,
                ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_X,
            );
            let y = get_u16(
                state,
                ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_Y,
            );
            Some((x, y))
        } else {
            None
        };
        let wide_tail = in_viewport
            && get_bool(
                state,
                ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_WIDE_TAIL,
            );
        let style = CursorStyle::from_raw(get_int(
            state,
            ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VISUAL_STYLE,
        ));
        let blinking = get_bool(
            state,
            ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_BLINKING,
        );
        let password_input = get_bool(
            state,
            ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_PASSWORD_INPUT,
        );
        Self {
            visible,
            viewport,
            wide_tail,
            style,
            blinking,
            password_input,
        }
    }
}

impl CursorStyle {
    #[cfg(test)]
    fn from_raw(v: ffi::GhosttyRenderStateCursorVisualStyle) -> Self {
        match v {
            ffi::GhosttyRenderStateCursorVisualStyle_GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BAR => {
                Self::Bar
            }
            ffi::GhosttyRenderStateCursorVisualStyle_GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK => {
                Self::Block
            }
            ffi::GhosttyRenderStateCursorVisualStyle_GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE => {
                Self::Underline
            }
            ffi::GhosttyRenderStateCursorVisualStyle_GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK_HOLLOW => {
                Self::BlockHollow
            }
            _ => Self::Block,
        }
    }
}

impl Style {
    #[cfg(test)]
    fn from_raw(s: ffi::GhosttyStyle) -> Self {
        Self {
            bold: s.bold,
            italic: s.italic,
            faint: s.faint,
            blink: s.blink,
            inverse: s.inverse,
            invisible: s.invisible,
            strikethrough: s.strikethrough,
            overline: s.overline,
            underline: s.underline != 0,
        }
    }
}

// ---- row/cell iteration ----

/// RAII handle over the row iterator.
#[cfg(test)]
struct RowIter {
    raw: ffi::GhosttyRenderStateRowIterator,
}

#[cfg(test)]
impl RowIter {
    fn new() -> Self {
        let mut raw: ffi::GhosttyRenderStateRowIterator = core::ptr::null_mut();
        let result =
            unsafe { ffi::ghostty_render_state_row_iterator_new(core::ptr::null(), &mut raw) };
        assert!(
            result == ffi::GhosttyResult_GHOSTTY_SUCCESS,
            "row_iterator_new failed"
        );
        Self { raw }
    }

    /// Populate from the render state. Reuses the existing handle.
    fn populate(&mut self, state: ffi::GhosttyRenderState) {
        let result = unsafe {
            ffi::ghostty_render_state_get(
                state,
                ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                &mut self.raw as *mut _ as *mut core::ffi::c_void,
            )
        };
        assert!(
            result == ffi::GhosttyResult_GHOSTTY_SUCCESS,
            "render_state_get(ROW_ITERATOR) failed"
        );
    }
}

#[cfg(test)]
impl Drop for RowIter {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_render_state_row_iterator_free(self.raw) };
    }
}

/// RAII handle over the per-row cells container.
#[cfg(test)]
struct RowCells {
    raw: ffi::GhosttyRenderStateRowCells,
}

#[cfg(test)]
impl RowCells {
    fn new() -> Self {
        let mut raw: ffi::GhosttyRenderStateRowCells = core::ptr::null_mut();
        let result =
            unsafe { ffi::ghostty_render_state_row_cells_new(core::ptr::null(), &mut raw) };
        assert!(
            result == ffi::GhosttyResult_GHOSTTY_SUCCESS,
            "row_cells_new failed"
        );
        Self { raw }
    }
}

#[cfg(test)]
impl Drop for RowCells {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_render_state_row_cells_free(self.raw) };
    }
}

#[cfg(test)]
fn walk_rows(state: ffi::GhosttyRenderState, colors: &Colors) -> Vec<Row> {
    let mut iter = RowIter::new();
    iter.populate(state);
    let mut cells = RowCells::new();
    let mut out = Vec::new();

    while unsafe { ffi::ghostty_render_state_row_iterator_next(iter.raw) } {
        let dirty = row_get_bool(
            iter.raw,
            ffi::GhosttyRenderStateRowData_GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY,
        );

        // Populate the reusable cells handle with this row's cells.
        let result = unsafe {
            ffi::ghostty_render_state_row_get(
                iter.raw,
                ffi::GhosttyRenderStateRowData_GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                &mut cells.raw as *mut _ as *mut core::ffi::c_void,
            )
        };
        assert!(
            result == ffi::GhosttyResult_GHOSTTY_SUCCESS,
            "row_get(CELLS) failed"
        );

        let selection = row_get_selection(iter.raw);
        let mut row_cells = Vec::new();
        while unsafe { ffi::ghostty_render_state_row_cells_next(cells.raw) } {
            let col = row_cells.len();
            let selected =
                selection.is_some_and(|(sx, ex)| col >= sx as usize && col <= ex as usize);
            row_cells.push(read_cell(cells.raw, colors, selected));
        }

        // Clear this row's dirty flag now that we've captured it.
        let clean = false;
        let _ = unsafe {
            ffi::ghostty_render_state_row_set(
                iter.raw,
                ffi::GhosttyRenderStateRowOption_GHOSTTY_RENDER_STATE_ROW_OPTION_DIRTY,
                &clean as *const _ as *const core::ffi::c_void,
            )
        };

        out.push(Row {
            dirty,
            selection,
            cells: row_cells,
        });
    }

    out
}

/// Read this row's selected cell range. Returns `None` when the row doesn't
/// intersect the active selection (NO_VALUE) — i.e. no selection is installed
/// or this row is outside it.
#[cfg(test)]
fn row_get_selection(iter: ffi::GhosttyRenderStateRowIterator) -> Option<(u16, u16)> {
    // SAFETY: zero-init valid for this sized-struct POD; the library reads
    // `size` to decide how many fields to write.
    let mut raw: ffi::GhosttyRenderStateRowSelection = unsafe { core::mem::zeroed() };
    raw.size = core::mem::size_of::<ffi::GhosttyRenderStateRowSelection>();
    let result = unsafe {
        ffi::ghostty_render_state_row_get(
            iter,
            ffi::GhosttyRenderStateRowData_GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION,
            &mut raw as *mut _ as *mut core::ffi::c_void,
        )
    };
    if result == ffi::GhosttyResult_GHOSTTY_SUCCESS {
        Some((raw.start_x, raw.end_x))
    } else {
        None
    }
}

#[cfg(test)]
fn read_cell(cells: ffi::GhosttyRenderStateRowCells, colors: &Colors, selected: bool) -> Cell {
    let grapheme = read_grapheme_utf8(cells);

    let style = {
        // SAFETY: zero-init valid for this repr(C) POD struct; library reads
        // `size` to decide how much to write.
        let mut raw: ffi::GhosttyStyle = unsafe { core::mem::zeroed() };
        raw.size = core::mem::size_of::<ffi::GhosttyStyle>();
        let result = unsafe {
            ffi::ghostty_render_state_row_cells_get(
                cells,
                ffi::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
                &mut raw as *mut _ as *mut core::ffi::c_void,
            )
        };
        assert!(
            result == ffi::GhosttyResult_GHOSTTY_SUCCESS,
            "row_cells_get(STYLE) failed"
        );
        Style::from_raw(raw)
    };

    let fg = read_cell_color(
        cells,
        ffi::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
    );
    let bg = read_cell_color(
        cells,
        ffi::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
    );
    let text_fg = effective_text_fg(style, fg, bg, colors, selected);
    let text_visible = !style.invisible && !grapheme.is_empty();

    Cell {
        grapheme,
        style,
        text_visible,
        text_fg,
        fg,
        bg,
    }
}

#[cfg(test)]
fn effective_text_fg(
    style: Style,
    fg: Option<Rgb>,
    bg: Option<Rgb>,
    colors: &Colors,
    selected: bool,
) -> Rgb {
    let mut effective_fg = fg.unwrap_or(colors.foreground);
    let mut effective_bg = bg.unwrap_or(colors.background);
    if style.inverse {
        core::mem::swap(&mut effective_fg, &mut effective_bg);
    }
    if selected {
        core::mem::swap(&mut effective_fg, &mut effective_bg);
    }
    if style.faint {
        effective_fg = Rgb {
            r: effective_fg.r / 2,
            g: effective_fg.g / 2,
            b: effective_fg.b / 2,
        };
    }
    effective_fg
}

/// Read a cell's resolved color. Returns `None` when the cell has no explicit
/// color of that kind (`GHOSTTY_INVALID_VALUE`).
#[cfg(test)]
fn read_cell_color(
    cells: ffi::GhosttyRenderStateRowCells,
    kind: ffi::GhosttyRenderStateRowCellsData,
) -> Option<Rgb> {
    let mut raw: ffi::GhosttyColorRgb = Rgb::default().into();
    let result = unsafe {
        ffi::ghostty_render_state_row_cells_get(
            cells,
            kind,
            &mut raw as *mut _ as *mut core::ffi::c_void,
        )
    };
    if result == ffi::GhosttyResult_GHOSTTY_SUCCESS {
        Some(Rgb::from(raw))
    } else {
        None
    }
}

/// Read a cell's grapheme cluster as UTF-8 via the two-pass `GhosttyBuffer`
/// protocol (query required length, then fill).
#[cfg(test)]
fn read_grapheme_utf8(cells: ffi::GhosttyRenderStateRowCells) -> String {
    // First pass: cap 0 → OUT_OF_SPACE with `len` set to required bytes.
    let mut probe = ffi::GhosttyBuffer {
        ptr: core::ptr::null_mut(),
        cap: 0,
        len: 0,
    };
    let _ = unsafe {
        ffi::ghostty_render_state_row_cells_get(
            cells,
            ffi::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
            &mut probe as *mut _ as *mut core::ffi::c_void,
        )
    };
    if probe.len == 0 {
        return String::new();
    }

    let mut bytes = vec![0u8; probe.len];
    let mut buf = ffi::GhosttyBuffer {
        ptr: bytes.as_mut_ptr(),
        cap: bytes.len(),
        len: 0,
    };
    let result = unsafe {
        ffi::ghostty_render_state_row_cells_get(
            cells,
            ffi::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
            &mut buf as *mut _ as *mut core::ffi::c_void,
        )
    };
    assert!(
        result == ffi::GhosttyResult_GHOSTTY_SUCCESS,
        "row_cells_get(GRAPHEMES_UTF8) fill failed"
    );
    String::from_utf8_lossy(&bytes[..buf.len]).into_owned()
}

#[cfg(test)]
impl From<Rgb> for ffi::GhosttyColorRgb {
    fn from(c: Rgb) -> Self {
        ffi::GhosttyColorRgb {
            r: c.r,
            g: c.g,
            b: c.b,
        }
    }
}

// ---- small typed render_state_get helpers ----

#[cfg(test)]
fn get_u16(state: ffi::GhosttyRenderState, kind: ffi::GhosttyRenderStateData) -> u16 {
    let mut v: u16 = 0;
    let result = unsafe {
        ffi::ghostty_render_state_get(state, kind, &mut v as *mut u16 as *mut core::ffi::c_void)
    };
    assert!(
        result == ffi::GhosttyResult_GHOSTTY_SUCCESS,
        "render_state_get failed"
    );
    v
}

#[cfg(test)]
fn get_int<T: Copy>(state: ffi::GhosttyRenderState, kind: ffi::GhosttyRenderStateData) -> T {
    let mut v = unsafe { core::mem::zeroed::<T>() };
    let result = unsafe {
        ffi::ghostty_render_state_get(state, kind, &mut v as *mut T as *mut core::ffi::c_void)
    };
    assert!(
        result == ffi::GhosttyResult_GHOSTTY_SUCCESS,
        "render_state_get failed"
    );
    v
}

#[cfg(test)]
fn get_bool(state: ffi::GhosttyRenderState, kind: ffi::GhosttyRenderStateData) -> bool {
    let mut v: bool = false;
    let result = unsafe {
        ffi::ghostty_render_state_get(state, kind, &mut v as *mut bool as *mut core::ffi::c_void)
    };
    assert!(
        result == ffi::GhosttyResult_GHOSTTY_SUCCESS,
        "render_state_get(bool) failed"
    );
    v
}

#[cfg(test)]
fn row_get_bool(
    iter: ffi::GhosttyRenderStateRowIterator,
    kind: ffi::GhosttyRenderStateRowData,
) -> bool {
    let mut v: bool = false;
    let result = unsafe {
        ffi::ghostty_render_state_row_get(iter, kind, &mut v as *mut bool as *mut core::ffi::c_void)
    };
    assert!(
        result == ffi::GhosttyResult_GHOSTTY_SUCCESS,
        "row_get(bool) failed"
    );
    v
}
