//! Live terminal surface: owns the [`Terminal`], [`RenderState`], PTY, font, and
//! glyph atlas, and produces a [`FramePlan`] of ready-to-draw colored glyph
//! quads for the C++ QSG renderer.
//!
//! Threading: the [`Terminal`] is not `Send`, so the Surface (and everything it
//! owns) lives on one thread — the GUI thread that hosts the QQuickItem. The
//! [`StreamingPty`] reader is the only piece on a background thread, and it
//! only touches its own mutex buffer, never the terminal.
//
// The workspace denies `unsafe_code`; this module is the C-FFI boundary and
// scopes the relaxation here only. The raw pointers handed out in a FramePlan
// are borrowed from the Surface and valid only until the next tick.
#![allow(unsafe_code)]
#![allow(unsafe_op_in_unsafe_fn)]

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::ffi::CString;
use std::os::raw::c_char;
use std::process::Command;
use std::ptr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tako_term::effects::TerminalEffects;
use tako_term::input as vt_input;
use tako_term::key::{KeyEncoder, KeyEvent};
use tako_term::modes;
use tako_term::mouse::{MouseEncoder, MouseEvent};
use tako_term::pty::StreamingPty;
use tako_term::snapshot::FrameSnapshot;
use tako_term::terminal::{RenderState, Terminal};

use crate::atlas::GlyphAtlas;
use crate::font::{CellMetrics, FontFace, GlyphBitmap, ShapedGlyph};

/// One vertex of a textured quad: pixel-space position, atlas UV, and a
/// modulate color. The renderer uploads these verbatim into a VBO and draws
/// with a single shader that multiplies the atlas coverage by the color.
///
/// Layout (20 bytes, matched in `gl_renderer.rs`'s vertex-attrib setup):
/// `{ x: f32, y: f32, u: f32, v: f32, r/g/b/a: u8 }`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
    pub u: f32,
    pub v: f32,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// The render plan handed to the GL renderer each tick. All pointers are
/// borrowed from the [`Surface`] and valid only until the next
/// [`Surface::tick`] (or the surface's destruction); the renderer deep-copies
/// them in its `synchronize()` step.
///
/// `vertices` is one flat buffer of glyph + cursor quad vertices in draw order
/// (cursor last, so it layers over glyphs). Background-cell quads land in P2.
#[repr(C)]
pub struct FramePlan {
    /// FBO clear color (terminal default background).
    pub clear_color: [u8; 4],
    pub cell_w: f32,
    pub cell_h: f32,
    pub cols: u32,
    pub rows: u32,
    pub vertices: *const Vertex,
    pub vertex_count: usize,
    pub atlas_w: u32,
    pub atlas_h: u32,
    /// Grayscale atlas pixels (`atlas_w * atlas_h` bytes).
    pub atlas_pixels: *const u8,
    /// Bumped whenever the atlas is rebuilt, even if dimensions are unchanged
    /// (shelf-pack reuses space within the same canvas). The renderer
    /// re-uploads the texture whenever this changes.
    pub atlas_generation: u64,
}

impl Default for FramePlan {
    fn default() -> Self {
        Self {
            clear_color: [0; 4],
            cell_w: 0.0,
            cell_h: 0.0,
            cols: 0,
            rows: 0,
            vertices: ptr::null(),
            vertex_count: 0,
            atlas_w: 0,
            atlas_h: 0,
            atlas_pixels: ptr::null(),
            atlas_generation: 0,
        }
    }
}

/// Optional one-shot command injected into the PTY shortly after spawn. Driven
/// by `TAKO_AUTORUN` (command) + `TAKO_AUTORUN_DELAY_MS` env vars for hands-off
/// perf testing (lets the shell's slow zshrc finish before we inject).
struct Autorun {
    start: Instant,
    delay: Duration,
    cmd: Vec<u8>,
    fired: bool,
}

/// State shared between the Surface (GUI thread) and the effects callbacks
/// (invoked synchronously inside `ghostty_terminal_vt_write`, also on the GUI
/// thread — but the closures need to outlive the borrow that creates them).
///
/// `Arc<Mutex<_>>` is overkill in the single-threaded case but cheap and
/// removes a class of soundness footguns if the threading model ever changes.
#[derive(Default)]
struct Shared {
    /// Bytes the `write_pty` effect produced during the most recent
    /// `vt_write`. Drained by the next tick and forwarded to the PTY. Holding
    /// them here (rather than writing inside the callback) keeps the callback
    /// re-entrancy-safe: we never touch the writer mid-`vt_write`.
    pty_response: Vec<u8>,
    /// Set by `title_changed`; tick snapshots `terminal.title()` after.
    title_dirty: bool,
    /// Set by `pwd_changed`; tick snapshots `terminal.pwd()` after.
    pwd_dirty: bool,
    /// Bell rang since the last tick (KNotification bridge pending).
    bell_count: u32,
}

/// Snapshot of the cell metrics + grid size, read by the XTWINOPS size
/// callback. Held in an `Arc<Mutex<_>>` so the closure can query it without
/// borrowing the Surface.
#[derive(Clone, Copy)]
struct MetricsSnapshot {
    cols: u16,
    rows: u16,
    cell_w: u32,
    cell_h: u32,
}

/// A live terminal surface. Drop closes the PTY and frees everything.
pub struct Surface {
    terminal: Terminal,
    state: RenderState,
    pty: StreamingPty,
    font: FontFace,
    cell: CellMetrics,
    cols: u16,
    rows: u16,
    atlas: GlyphAtlas,
    /// Bumped every time `atlas` is reassigned, so the renderer can detect
    /// content changes that don't alter dimensions (shelf-pack repacking).
    atlas_generation: u64,
    glyph_advance: HashMap<u32, f32>,
    /// Rasterize-once cache keyed by glyph id, shared across atlas rebuilds so
    /// FreeType never rasterizes the same glyph twice.
    raster_cache: HashMap<u32, GlyphBitmap>,
    shape_cache: HashMap<String, Vec<ShapedGlyph>>,
    vertex_buf: Vec<Vertex>,
    autorun: Option<Autorun>,
    /// Resolved font file path (kept so the font can be reloaded on DPR change).
    font_path: String,
    /// Logical (DIP) font size the user requested. The actual rasterized size
    /// is `logical_pixel_height × dpr` so glyphs stay crisp on hidpi displays.
    logical_pixel_height: u32,
    /// Device pixel ratio of the screen hosting this surface. Changing it (via
    /// [`Surface::set_dpr`]) reloads the font at the new physical size and
    /// invalidates size-dependent caches.
    dpr: f32,

    /// Shared state between the Surface and the effects callbacks.
    shared: Arc<Mutex<Shared>>,
    /// Cell metrics snapshot, kept in sync with `cell`/`cols`/`rows` for the
    /// size query callback.
    metrics: Arc<Mutex<MetricsSnapshot>>,
    /// Latest window title snapshotted from the terminal (OSC 0/2).
    title: String,
    /// Latest pwd snapshotted from the terminal (OSC 7/9/1337).
    pwd: String,

    /// Key/mouse encoders reused across events (they own mode-state caches).
    key_encoder: KeyEncoder,
    mouse_encoder: MouseEncoder,
    /// Scratch event handles reused per keystroke / per mouse event (cheap
    /// allocation amortized across thousands of events).
    key_event: KeyEvent,
    mouse_event: MouseEvent,

    /// Latest window title as observed by the host (C++ QML window title).
    /// Read via [`Surface::take_host_events`] each tick.
    host_title: Option<String>,
}

impl Surface {
    /// Spawn a shell on a PTY and load `font_path` (or the system default
    /// monospace if `font_path` is `None`) at the logical `pixel_height`,
    /// rasterized at `dpr × pixel_height` physical pixels for hidpi sharpness.
    /// `dpr` is the device pixel ratio of the screen hosting this surface;
    /// change it later via [`Surface::set_dpr`].
    pub fn new(
        cols: u16,
        rows: u16,
        font_path: Option<&str>,
        logical_pixel_height: u32,
        dpr: f32,
    ) -> Result<Self, String> {
        // Resolve the font + cell metrics first so the size callback can read
        // them. (The other branches of this constructor only use them later.)
        let path = match font_path {
            Some(p) => p.to_string(),
            None => resolve_default_font()?,
        };
        let physical_px = physical_font_size(logical_pixel_height, dpr);
        let font =
            FontFace::from_path(&path, physical_px).map_err(|e| format!("font load: {e}"))?;
        let cell = font.cell_metrics();

        // Build the shared state + effects BEFORE the terminal so the
        // write_pty closure can clone the Arc and append response bytes.
        let shared = Arc::new(Mutex::new(Shared::default()));
        let shared_for_write = Arc::clone(&shared);
        let shared_for_bell = Arc::clone(&shared);
        let shared_for_title = Arc::clone(&shared);
        let shared_for_pwd = Arc::clone(&shared);

        // Size query is read-only state. Stash the cell metrics + grid size in
        // an Arc so the closure can read them without borrowing the Surface.
        // Updated by `resize_to_pixels` and `set_dpr` after construction.
        let metrics = Arc::new(Mutex::new(MetricsSnapshot {
            cols,
            rows,
            cell_w: cell.width,
            cell_h: cell.height,
        }));
        let metrics_for_size = Arc::clone(&metrics);

        let effects = TerminalEffects::new()
            .with_write_pty(move |bytes: &[u8]| {
                if let Ok(mut g) = shared_for_write.lock() {
                    g.pty_response.extend_from_slice(bytes);
                }
            })
            .with_bell(move || {
                if let Ok(mut g) = shared_for_bell.lock() {
                    g.bell_count = g.bell_count.saturating_add(1);
                }
            })
            .with_title_changed(move || {
                if let Ok(mut g) = shared_for_title.lock() {
                    g.title_dirty = true;
                }
            })
            .with_pwd_changed(move || {
                if let Ok(mut g) = shared_for_pwd.lock() {
                    g.pwd_dirty = true;
                }
            })
            .with_size(move || {
                let m = metrics_for_size.lock().expect("metrics poisoned");
                tako_term::effects::SizeInfo {
                    cols: m.cols,
                    rows: m.rows,
                    cell_w_px: m.cell_w,
                    cell_h_px: m.cell_h,
                }
            });

        let terminal = Terminal::new_with_effects(cols, rows, 10_000, effects)
            .map_err(|e| format!("terminal_new: {e}"))?;
        let state = RenderState::new().map_err(|e| format!("render_state_new: {e}"))?;
        let pty = StreamingPty::spawn_shell(cols, rows).map_err(|e| format!("spawn shell: {e}"))?;

        // Empty atlas to start; filled on the first tick. raster_cache is
        // declared alongside so we can move it into Self below.
        let mut raster_cache: HashMap<u32, GlyphBitmap> = HashMap::new();
        let atlas = GlyphAtlas::from_glyph_advances(&font, &HashMap::new(), &mut raster_cache);

        let autorun = std::env::var("TAKO_AUTORUN").ok().map(|cmd| Autorun {
            start: Instant::now(),
            delay: Duration::from_millis(
                std::env::var("TAKO_AUTORUN_DELAY_MS")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(2000),
            ),
            cmd: format!("{cmd}\n").into_bytes(),
            fired: false,
        });

        let key_encoder = KeyEncoder::new().map_err(|e| format!("key_encoder_new: {e}"))?;
        let mouse_encoder = MouseEncoder::new().map_err(|e| format!("mouse_encoder_new: {e}"))?;
        let key_event = KeyEvent::new().map_err(|e| format!("key_event_new: {e}"))?;
        let mouse_event = MouseEvent::new().map_err(|e| format!("mouse_event_new: {e}"))?;

        Ok(Self {
            terminal,
            state,
            pty,
            font,
            cell,
            cols,
            rows,
            atlas,
            atlas_generation: 0,
            glyph_advance: HashMap::new(),
            raster_cache,
            shape_cache: HashMap::new(),
            vertex_buf: Vec::new(),
            autorun,
            font_path: path,
            logical_pixel_height,
            dpr,
            shared,
            metrics,
            title: String::new(),
            pwd: String::new(),
            key_encoder,
            mouse_encoder,
            key_event,
            mouse_event,
            host_title: None,
        })
    }

    /// Reload the font at a new device-pixel ratio, invalidating all
    /// size-dependent caches. The caller should follow this with
    /// [`Surface::resize_to_pixels`] (passing the current physical item size)
    /// so the grid reflows to the new cell metrics.
    ///
    /// No-op when `dpr` is within 0.01 of the current value.
    pub fn set_dpr(&mut self, dpr: f32) {
        if (dpr - self.dpr).abs() < 0.01 {
            return;
        }
        self.dpr = dpr;
        let physical_px = physical_font_size(self.logical_pixel_height, dpr);
        match FontFace::from_path(&self.font_path, physical_px) {
            Ok(font) => {
                self.font = font;
                self.cell = self.font.cell_metrics();
            }
            Err(e) => {
                eprintln!("[surface] font reload at dpr {dpr} (phys {physical_px}) failed: {e}");
                return;
            }
        }
        // Invalidate all size-dependent caches: glyph x_advance (scaled by px),
        // rasterized bitmaps, shaped advances, the packed atlas.
        self.glyph_advance.clear();
        self.raster_cache.clear();
        self.shape_cache.clear();
        self.vertex_buf.clear();
        self.atlas =
            GlyphAtlas::from_glyph_advances(&self.font, &HashMap::new(), &mut self.raster_cache);
        self.atlas_generation = self.atlas_generation.wrapping_add(1);
        // Refresh the size-query snapshot with the new cell metrics.
        if let Ok(mut m) = self.metrics.lock() {
            *m = MetricsSnapshot {
                cols: self.cols,
                rows: self.rows,
                cell_w: self.cell.width,
                cell_h: self.cell.height,
            };
        }
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }
    pub fn rows(&self) -> u16 {
        self.rows
    }
    pub fn cell(&self) -> CellMetrics {
        self.cell
    }

    /// Resize the terminal grid to fit `width_px × height_px` (in current
    /// cell-metric units; DIPs pre-P4, physical px post-P4). Computes cols/rows
    /// from the cell metrics, resizes both the libghostty-vt terminal and the
    /// PTY (so the child sees `SIGWINCH`), and updates internal fields. No-op
    /// if the computed size matches the current one (avoids resize storms on
    /// sub-cell window motion during drag).
    pub fn resize_to_pixels(&mut self, width_px: u32, height_px: u32) {
        let cw = self.cell.width.max(1);
        let ch = self.cell.height.max(1);
        let cols = ((width_px / cw).max(1)).min(u16::MAX as u32) as u16;
        let rows = ((height_px / ch).max(1)).min(u16::MAX as u32) as u16;
        if cols == self.cols && rows == self.rows {
            return;
        }
        let old_cols = self.cols;
        let old_rows = self.rows;
        self.cols = cols;
        self.rows = rows;
        if let Err(e) = self.terminal.resize(cols, rows, cw, ch) {
            eprintln!(
                "[surface] terminal resize {old_cols}x{old_rows} → {cols}x{rows} failed: {e}"
            );
        }
        if let Err(e) = self.pty.resize(cols, rows) {
            eprintln!("[surface] pty resize {old_cols}x{old_rows} → {cols}x{rows} failed: {e}");
        }
        // Keep the mouse encoder's size context in sync so coordinate mapping
        // (surface px → cell coords) stays correct.
        self.mouse_encoder
            .set_size(cols as u32 * cw, rows as u32 * ch, cw, ch, 0);
        // Keep the XTWINOPS size-query snapshot in sync.
        if let Ok(mut m) = self.metrics.lock() {
            *m = MetricsSnapshot {
                cols,
                rows,
                cell_w: cw,
                cell_h: ch,
            };
        }
    }

    /// Send typed input (keyboard) to the shell.
    pub fn write_input(&mut self, bytes: &[u8]) {
        let _ = self.pty.write(bytes);
    }

    /// Encode and send a key event. `key` is a `GhosttyKey` enum value;
    /// `mods`/`consumed_mods` are `GhosttyMods` bitmasks; `text` is the
    /// UTF-8 text the key produced (or `None` to let the encoder derive it).
    pub fn key_event(
        &mut self,
        action: tako_term::ffi::GhosttyKeyAction,
        key: tako_term::ffi::GhosttyKey,
        mods: u16,
        consumed_mods: u16,
        text: Option<&[u8]>,
    ) {
        self.key_event.set_action(action);
        self.key_event.set_key(key);
        self.key_event.set_mods(mods);
        self.key_event.set_consumed_mods(consumed_mods);
        self.key_event.set_utf8(text);
        let bytes = self.key_encoder.encode(&self.terminal, &self.key_event);
        if !bytes.is_empty() {
            let _ = self.pty.write(&bytes);
        }
    }

    /// Encode and send a mouse event. Position is in surface-space pixels.
    /// `button` is `None` for motion events; pass the actual button for
    /// press/release. `mods` is a `GhosttyMods` bitmask.
    pub fn mouse_event(
        &mut self,
        action: tako_term::ffi::GhosttyMouseAction,
        button: Option<tako_term::ffi::GhosttyMouseButton>,
        x_px: f32,
        y_px: f32,
        mods: u16,
    ) {
        self.mouse_event.set_action(action);
        match button {
            Some(b) => self.mouse_event.set_button(b),
            None => self.mouse_event.clear_button(),
        }
        self.mouse_event.set_mods(mods);
        self.mouse_event.set_position(x_px, y_px);
        let bytes = self.mouse_encoder.encode(&self.terminal, &self.mouse_event);
        if !bytes.is_empty() {
            let _ = self.pty.write(&bytes);
        }
    }

    /// Tell the encoder whether any button is held (drives any-event motion
    /// dedup). Call on press/release; the encoder does not query this itself.
    pub fn mouse_set_any_button(&mut self, pressed: bool) {
        self.mouse_encoder.set_any_button_pressed(pressed);
    }

    /// Focus gained/lost. No-op when focus reporting (DEC mode 1004) is off.
    pub fn focus_event(&mut self, gained: bool) {
        if self.terminal.mode_get(modes::FOCUS_EVENT) {
            let bytes = vt_input::encode_focus(gained);
            if !bytes.is_empty() {
                let _ = self.pty.write(&bytes);
            }
        }
    }

    /// Paste bytes into the terminal, wrapping in bracketed paste sequences
    /// when DEC mode 2004 is set.
    pub fn paste(&mut self, data: &[u8]) {
        let bracketed = self.terminal.mode_get(modes::BRACKETED_PASTE);
        let bytes = vt_input::paste_encode(data, bracketed);
        if !bytes.is_empty() {
            let _ = self.pty.write(&bytes);
        }
    }

    /// Scroll the viewport by `delta_rows` (negative = up into history,
    /// positive = down toward active area).
    pub fn scroll(&mut self, delta_rows: i64) {
        self.terminal
            .scroll_viewport(vt_input::Scroll::Delta(delta_rows));
    }

    /// `true` if any mouse tracking mode is on — drives the report-vs-select
    /// policy in the embedder.
    pub fn mouse_tracking(&self) -> bool {
        self.terminal.mouse_tracking()
    }

    /// Take the latest host-bound window title, if it changed since the last
    /// call. C++ uses this to update the QML window title.
    pub fn take_host_title(&mut self) -> Option<String> {
        self.host_title.take()
    }

    /// Drain PTY output, advance the terminal, and rebuild the frame plan.
    pub fn tick(&mut self) -> FramePlan {
        let t0 = Instant::now();

        // Fire autorun once after the configured delay (env-driven perf harness).
        if let Some(a) = &mut self.autorun
            && !a.fired
            && a.start.elapsed() >= a.delay
        {
            a.fired = true;
            eprintln!(
                "[tick] autorun firing: {:?}",
                String::from_utf8_lossy(&a.cmd).trim()
            );
            let _ = self.pty.write(&a.cmd);
        }

        let bytes = self.pty.drain();
        let byte_count = bytes.len();
        let t_drain = Instant::now();
        if !bytes.is_empty() {
            self.terminal.vt_write(&bytes);
        }
        // Drain any PTY-bound response bytes the effects callbacks produced
        // during vt_write (DA1/XTVERSION/focus/mouse/etc.). Doing it here
        // keeps the callback re-entrancy-safe — it only appends to a buffer.
        let pty_response: Vec<u8> = {
            let mut g = self.shared.lock().expect("effects shared state poisoned");
            std::mem::take(&mut g.pty_response)
        };
        if !pty_response.is_empty() {
            let _ = self.pty.write(&pty_response);
        }
        // Snapshot title/pwd if the callbacks flagged them. We must read
        // these now, before the next vt_write invalidates the borrow.
        {
            let g = self.shared.lock().expect("effects shared state poisoned");
            let title_dirty = g.title_dirty;
            let pwd_dirty = g.pwd_dirty;
            let bell_count = g.bell_count;
            drop(g);
            if title_dirty {
                let t = self.terminal.title().to_vec();
                let new_title = String::from_utf8_lossy(&t).into_owned();
                if new_title != self.title {
                    self.title = new_title.clone();
                    self.host_title = Some(new_title);
                }
                if let Ok(mut g) = self.shared.lock() {
                    g.title_dirty = false;
                }
            }
            if pwd_dirty {
                let p = self.terminal.pwd().to_vec();
                self.pwd = String::from_utf8_lossy(&p).into_owned();
                if let Ok(mut g) = self.shared.lock() {
                    g.pwd_dirty = false;
                }
            }
            if bell_count > 0 {
                // TODO: KNotification bridge.
                if let Ok(mut g) = self.shared.lock() {
                    g.bell_count = 0;
                }
            }
        }
        let t_vt = Instant::now();
        let snap = FrameSnapshot::capture(&mut self.terminal, &mut self.state);
        let t_snap = Instant::now();
        let plan = self.build_plan(&snap);
        let t_plan = Instant::now();
        self.state.clear_dirty().ok();

        let total_us = t_plan.duration_since(t0).as_micros();
        // Log slow frames (>5ms) or frames that ingested a lot of PTY bytes.
        // Both are signals that something is bound to output volume.
        if total_us > 5_000 || byte_count > 4_096 {
            eprintln!(
                "[tick] total={total_us}µs bytes={byte_count} drain={}µs vt={}µs snap={}µs plan={}µs verts={}",
                t_drain.duration_since(t0).as_micros(),
                t_vt.duration_since(t_drain).as_micros(),
                t_snap.duration_since(t_vt).as_micros(),
                t_plan.duration_since(t_snap).as_micros(),
                plan.vertex_count,
            );
        }

        plan
    }

    fn build_plan(&mut self, snap: &FrameSnapshot) -> FramePlan {
        let t0 = Instant::now();
        let CellMetrics {
            width: cw,
            height: ch,
            ascent,
            ..
        } = self.cell;
        let (cw, ch) = (cw as f32, ch as f32);

        // Collect unique graphemes; shape via cache; refresh the atlas if the
        // glyph-id set grew.
        let unique: BTreeSet<String> = snap
            .rows_data
            .iter()
            .flat_map(|r| r.cells.iter())
            .filter(|c| !c.grapheme.is_empty())
            .map(|c| c.grapheme.clone())
            .collect();
        let unique_count = unique.len();
        let t_unique = Instant::now();

        // Rebuild the atlas only when new glyph ids appear this frame. Compare
        // against the previous advance-map length, NOT atlas.glyphs.len(): the
        // atlas excludes blank glyphs (e.g. space) while advance includes them,
        // so a count mismatch is permanent and would force a rebuild every frame.
        let prev_advance_len = self.glyph_advance.len();
        let mut advance: HashMap<u32, f32> = std::mem::take(&mut self.glyph_advance);
        for g in &unique {
            let shaped = self
                .shape_cache
                .entry(g.clone())
                .or_insert_with(|| self.font.shape(g))
                .clone();
            for sg in shaped {
                advance.entry(sg.glyph_id).or_insert(sg.x_advance);
            }
        }
        let atlas_rebuilt = advance.len() > prev_advance_len;
        if atlas_rebuilt {
            self.atlas =
                GlyphAtlas::from_glyph_advances(&self.font, &advance, &mut self.raster_cache);
            self.atlas_generation = self.atlas_generation.wrapping_add(1);
        }
        self.glyph_advance = advance;
        let t_shape = Instant::now();

        let atlas_w = self.atlas.width;
        let atlas_h = self.atlas.height;
        let inv_w = if atlas_w > 0 {
            1.0 / atlas_w as f32
        } else {
            0.0
        };
        let inv_h = if atlas_h > 0 {
            1.0 / atlas_h as f32
        } else {
            0.0
        };

        let fg_default = snap.colors.foreground;
        let default_fg = (fg_default.r, fg_default.g, fg_default.b);
        let default_bg = (
            snap.colors.background.r,
            snap.colors.background.g,
            snap.colors.background.b,
        );
        let mut verts: Vec<Vertex> = Vec::new();

        // Push a textured quad (two triangles, 4 vertices) in pixel space.
        // `push_quad` is inlined-hot; keep attribute order identical to the
        // GL vertex-attrib layout in `gl_renderer.rs`.
        let mut push_quad = |x: f32,
                             y: f32,
                             w: f32,
                             h: f32,
                             u0: f32,
                             v0: f32,
                             u1: f32,
                             v1: f32,
                             (r, g, b): (u8, u8, u8)| {
            let a = 255u8;
            // Counter-clockwise wound (matches the index pattern in the GL
            // renderer: 0,1,2, 0,2,3).
            verts.push(Vertex {
                x,
                y,
                u: u0,
                v: v0,
                r,
                g,
                b,
                a,
            }); // top-left
            verts.push(Vertex {
                x: x + w,
                y,
                u: u1,
                v: v0,
                r,
                g,
                b,
                a,
            }); // top-right
            verts.push(Vertex {
                x: x + w,
                y: y + h,
                u: u1,
                v: v1,
                r,
                g,
                b,
                a,
            }); // br
            verts.push(Vertex {
                x,
                y: y + h,
                u: u0,
                v: v1,
                r,
                g,
                b,
                a,
            }); // bl
        };

        for (row_i, row) in snap.rows_data.iter().enumerate() {
            let row_y = row_i as f32 * ch;
            let baseline = row_y + ascent as f32;
            for (col_i, cell) in row.cells.iter().enumerate() {
                let col_x = col_i as f32 * cw;

                // Resolve effective fg/bg. `cell.fg`/`cell.bg` are None when
                // the cell has no explicit color → fall back to the terminal
                // defaults. Inverse (SGR 7) swaps the two; applied here rather
                // than in ghostty-vt so the rest of the pipeline sees final
                // colors. Reference: ghostling main.c `render_terminal`.
                let raw_fg = match cell.fg {
                    Some(c) => (c.r, c.g, c.b),
                    None => default_fg,
                };
                let raw_bg = match cell.bg {
                    Some(c) => (c.r, c.g, c.b),
                    None => default_bg,
                };
                let (eff_fg, eff_bg) = if cell.style.inverse {
                    (raw_bg, raw_fg)
                } else {
                    (raw_fg, raw_bg)
                };
                // Faint (SGR 2): halve foreground intensity.
                // TODO(bold/italic/underline): style.bold needs a bold face
                // variant; italic needs an italic face; underline/strikethrough
                // need line primitives. All deferred.
                let eff_fg = if cell.style.faint {
                    (eff_fg.0 / 2, eff_fg.1 / 2, eff_fg.2 / 2)
                } else {
                    eff_fg
                };

                // Background quad for cells whose effective bg differs from
                // the terminal default (the FBO was cleared to default_bg, so
                // default-bg cells need no quad). Drawn before the glyph so the
                // glyph composites over it within the cell. Sentinel UV
                // (-1,-1) signals the shader to synthesize coverage 1.0.
                if eff_bg != default_bg {
                    push_quad(
                        col_x, row_y, cw, ch, FLAT_UV, FLAT_UV, FLAT_UV, FLAT_UV, eff_bg,
                    );
                }

                if cell.grapheme.is_empty() {
                    continue;
                }

                let shaped = self
                    .shape_cache
                    .get(&cell.grapheme)
                    .cloned()
                    .unwrap_or_default();
                let mut pen_x = col_x;
                for sg in shaped {
                    if let Some(rect) = self.atlas.glyphs.get(&sg.glyph_id)
                        && rect.w > 0
                        && rect.h > 0
                    {
                        let qx = pen_x + rect.left_bearing as f32;
                        let qy = baseline - rect.top_bearing as f32;
                        push_quad(
                            qx,
                            qy,
                            rect.w as f32,
                            rect.h as f32,
                            rect.x as f32 * inv_w,
                            rect.y as f32 * inv_h,
                            (rect.x + rect.w) as f32 * inv_w,
                            (rect.y + rect.h) as f32 * inv_h,
                            eff_fg,
                        );
                    }
                    pen_x += sg.x_advance;
                }
            }
        }

        // Cursor: drawn last so it layers on top of any glyph beneath it.
        // Sentinel UV (-1,-1) signals the shader to synthesize coverage 1.0.
        // The cursor is sized/positioned by its visual style (DECSCUSR):
        //   Block       — full cell (default).
        //   Bar         — left ~1/8 of cell.
        //   Underline   — bottom ~1/8 of cell.
        //   BlockHollow — full cell with inverted fg/bg; we draw a thin border
        //                 approximation (4 quads) since our shader has no
        //                 outline primitive.
        if let Some((cx, cy)) = snap.cursor.viewport
            && snap.cursor.visible
        {
            let color = snap.colors.cursor.unwrap_or(snap.colors.foreground);
            let px = cx as f32 * cw;
            let py = cy as f32 * ch;
            match snap.cursor.style {
                tako_term::snapshot::CursorStyle::BlockHollow => {
                    // Hollow block: draw a frame (4 thin quads) using the
                    // foreground color so the cursor stands out against any bg.
                    let bg = snap.colors.foreground;
                    let thickness = (cw.min(ch) * 0.1).max(1.0);
                    for &(y, h) in &[(py, thickness), (py + ch - thickness, thickness)] {
                        push_quad(
                            px,
                            y,
                            cw,
                            h,
                            FLAT_UV,
                            FLAT_UV,
                            FLAT_UV,
                            FLAT_UV,
                            (bg.r, bg.g, bg.b),
                        );
                    }
                    for &(x, w) in &[(px, thickness), (px + cw - thickness, thickness)] {
                        push_quad(
                            x,
                            py + thickness,
                            w,
                            ch - 2.0 * thickness,
                            FLAT_UV,
                            FLAT_UV,
                            FLAT_UV,
                            FLAT_UV,
                            (bg.r, bg.g, bg.b),
                        );
                    }
                }
                tako_term::snapshot::CursorStyle::Bar => {
                    push_quad(
                        px,
                        py,
                        (cw * 0.125).max(1.0),
                        ch,
                        FLAT_UV,
                        FLAT_UV,
                        FLAT_UV,
                        FLAT_UV,
                        (color.r, color.g, color.b),
                    );
                }
                tako_term::snapshot::CursorStyle::Underline => {
                    let h = (ch * 0.125).max(1.0);
                    push_quad(
                        px,
                        py + ch - h,
                        cw,
                        h,
                        FLAT_UV,
                        FLAT_UV,
                        FLAT_UV,
                        FLAT_UV,
                        (color.r, color.g, color.b),
                    );
                }
                tako_term::snapshot::CursorStyle::Block => {
                    push_quad(
                        px,
                        py,
                        cw,
                        ch,
                        FLAT_UV,
                        FLAT_UV,
                        FLAT_UV,
                        FLAT_UV,
                        (color.r, color.g, color.b),
                    );
                }
            }
            // TODO: blink phase (Cursor.blinking + Cursor.password_input).
        }

        self.vertex_buf = verts;
        let t_quads = Instant::now();

        let build_total_us = t_quads.duration_since(t0).as_micros();
        if build_total_us > 5_000 {
            eprintln!(
                "[build_plan] total={build_total_us}µs unique={}µs (n={unique_count}) shape={}µs (atlas_rebuilt={atlas_rebuilt}, advance_n={}, raster_cache_n={}) verts={}µs (n={})",
                t_unique.duration_since(t0).as_micros(),
                t_shape.duration_since(t_unique).as_micros(),
                self.glyph_advance.len(),
                self.raster_cache.len(),
                t_quads.duration_since(t_shape).as_micros(),
                self.vertex_buf.len(),
            );
        }

        FramePlan {
            clear_color: [
                snap.colors.background.r,
                snap.colors.background.g,
                snap.colors.background.b,
                255,
            ],
            cell_w: cw,
            cell_h: ch,
            cols: self.cols as u32,
            rows: self.rows as u32,
            vertices: if self.vertex_buf.is_empty() {
                ptr::null()
            } else {
                self.vertex_buf.as_ptr()
            },
            vertex_count: self.vertex_buf.len(),
            atlas_w,
            atlas_h,
            atlas_pixels: if atlas_w * atlas_h > 0 {
                self.atlas.pixels.as_ptr()
            } else {
                ptr::null()
            },
            atlas_generation: self.atlas_generation,
        }
    }
}

/// Compute the physical pixel size to rasterize the font at, given a logical
/// (DIP) height and the device-pixel ratio. Rounds to the nearest integer so
/// FreeType's `set_pixel_sizes` (which takes integers) gets a stable target.
/// `max(1)` guards the degenerate `dpr=0` / zero-height case.
fn physical_font_size(logical_pixel_height: u32, dpr: f32) -> u32 {
    let p = (dpr * logical_pixel_height as f32).round() as u32;
    p.max(1)
}

/// Sentinel UV emitted for flat-color quads (cell backgrounds, cursor). The
/// shader checks `v_uv.x < 0.0` and synthesizes coverage 1.0 (no texture
/// fetch), so these quads render as opaque color without needing a dedicated
/// texel in the glyph atlas.
const FLAT_UV: f32 = -1.0;

/// Resolve the system default monospace font path via fontconfig (`fc-match`).
fn resolve_default_font() -> Result<String, String> {
    let out = Command::new("fc-match")
        .args(["-f", "%{file}", "monospace"])
        .output()
        .map_err(|e| format!("fc-match failed: {e}"))?;
    if !out.status.success() {
        return Err("fc-match returned non-zero".to_string());
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        return Err("fc-match returned empty path".to_string());
    }
    Ok(path)
}

// ---- C ABI surface for the C++ QQuickItem renderer ----
// Pointers are borrowed from the Surface and valid only across a single tick
// (C++ copies into QSG geometry before the next call).

/// Spawn a surface. `font_path` may be null to use the system default mono
/// font. `pixel_height` is the logical (DIP) cell height; the font is
/// rasterized at `dpr × pixel_height` physical pixels for hidpi sharpness.
///
/// Returns an opaque heap pointer on success, or null on failure (logged).
///
/// # Safety
///
/// `font_path` must be a valid NUL-terminated C string if non-null. The caller
/// owns the returned pointer and must free it with [`tako_surface_destroy`].
/// The surface is not thread-safe and must be used from one thread only.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_new(
    cols: u16,
    rows: u16,
    font_path: *const c_char,
    pixel_height: u32,
    dpr: f32,
) -> *mut Surface {
    let font = if font_path.is_null() {
        None
    } else {
        Some(
            std::ffi::CStr::from_ptr(font_path)
                .to_string_lossy()
                .into_owned(),
        )
    };
    match Surface::new(cols, rows, font.as_deref(), pixel_height, dpr) {
        Ok(s) => Box::into_raw(Box::new(s)),
        Err(e) => {
            eprintln!("tako_surface_new failed: {e}");
            ptr::null_mut()
        }
    }
}

/// Free a surface returned by [`tako_surface_new`]. No-op on null.
///
/// # Safety
///
/// `s` must be either null or a pointer previously returned by
/// [`tako_surface_new`] that has not already been freed. After this call the
/// pointer is invalid and must not be used.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_destroy(s: *mut Surface) {
    if !s.is_null() {
        drop(unsafe { Box::from_raw(s) });
    }
}

/// Rebuild the frame plan and write it into `*out`. The pointers inside `out`
/// are valid until the next `tako_surface_tick` or `tako_surface_destroy`.
///
/// # Safety
///
/// `s` must be a valid [`Surface`] pointer from [`tako_surface_new`]. `out`
/// must point to writable memory the caller owns; it is overwritten. The
/// caller must not read the borrowed pointers in `*out` after the next call
/// to `tako_surface_tick` or after [`tako_surface_destroy`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_tick(s: *mut Surface, out: *mut FramePlan) {
    if s.is_null() || out.is_null() {
        return;
    }
    let surface = unsafe { &mut *s };
    let plan = surface.tick();
    unsafe { *out = plan };
}

/// Send typed input bytes to the shell.
///
/// # Safety
///
/// `s` must be a valid [`Surface`] pointer from [`tako_surface_new`]. `data`
/// must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_write(s: *mut Surface, data: *const u8, len: usize) {
    if s.is_null() || data.is_null() {
        return;
    }
    let surface = unsafe { &mut *s };
    let slice = unsafe { std::slice::from_raw_parts(data, len) };
    surface.write_input(slice);
}

/// Resize the terminal grid to fit `width_px × height_px` device-independent
/// pixels (pre-P4; physical px post-P4). The surface computes cols/rows from
/// its cell metrics and resizes both the terminal and the PTY. Safe to call
/// with the current size (no-op).
///
/// # Safety
///
/// `s` must be a valid [`Surface`] pointer from [`tako_surface_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_resize_pixels(
    s: *mut Surface,
    width_px: u32,
    height_px: u32,
) {
    if s.is_null() {
        return;
    }
    let surface = unsafe { &mut *s };
    surface.resize_to_pixels(width_px, height_px);
}

/// Reload the font at a new device-pixel ratio and invalidate size-dependent
/// caches. The caller should follow this with [`tako_surface_resize_pixels`]
/// using the current physical item size so the grid reflows to the new cell
/// metrics. No-op when `dpr` is within 0.01 of the current value.
///
/// # Safety
///
/// `s` must be a valid [`Surface`] pointer from [`tako_surface_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_set_dpr(s: *mut Surface, dpr: f32) {
    if s.is_null() {
        return;
    }
    let surface = unsafe { &mut *s };
    surface.set_dpr(dpr);
}

// ---- input ----
//
// The C ABI passes libghostty-vt enum values directly (defined in
// `tako_term::ffi`). C++ includes the matching C headers via
// `<ghostty/vt/key/event.h>` and `<ghostty/vt/mouse/event.h>`.

/// Encode and forward a key event to the PTY.
///
/// # Safety
///
/// `s` must be a valid [`Surface`] pointer from [`tako_surface_new`]. If
/// `text_len > 0`, `text` must point to `text_len` valid UTF-8 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_key_event(
    s: *mut Surface,
    action: u32,
    key: u32,
    mods: u16,
    consumed_mods: u16,
    text: *const u8,
    text_len: usize,
) {
    if s.is_null() {
        return;
    }
    let surface = unsafe { &mut *s };
    let text = if text.is_null() || text_len == 0 {
        None
    } else {
        Some(unsafe { std::slice::from_raw_parts(text, text_len) })
    };
    surface.key_event(action, key, mods, consumed_mods, text);
}

/// Encode and forward a mouse event to the PTY. `button` 0 = UNKNOWN (use
/// for motion); mouse tracking is checked inside the encoder.
///
/// # Safety
///
/// `s` must be a valid [`Surface`] pointer from [`tako_surface_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_mouse_event(
    s: *mut Surface,
    action: u32,
    button: u32,
    x_px: f32,
    y_px: f32,
    mods: u16,
) {
    if s.is_null() {
        return;
    }
    let surface = unsafe { &mut *s };
    let button = if button == 0 { None } else { Some(button) };
    surface.mouse_event(action, button, x_px, y_px, mods);
}

/// Tell the surface whether any mouse button is currently held (drives
/// any-event motion reporting).
///
/// # Safety
///
/// `s` must be a valid [`Surface`] pointer from [`tako_surface_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_mouse_set_any_button(s: *mut Surface, pressed: bool) {
    if s.is_null() {
        return;
    }
    let surface = unsafe { &mut *s };
    surface.mouse_set_any_button(pressed);
}

/// Focus gained/lost. Forwards focus-reporting bytes to the PTY iff DEC mode
/// 1004 is set.
///
/// # Safety
///
/// `s` must be a valid [`Surface`] pointer from [`tako_surface_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_focus_event(s: *mut Surface, gained: bool) {
    if s.is_null() {
        return;
    }
    let surface = unsafe { &mut *s };
    surface.focus_event(gained);
}

/// Paste bytes into the terminal (with bracketed paste wrapping if mode 2004
/// is set).
///
/// # Safety
///
/// `s` must be a valid [`Surface`] pointer from [`tako_surface_new`]. `data`
/// must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_paste(s: *mut Surface, data: *const u8, len: usize) {
    if s.is_null() || data.is_null() {
        return;
    }
    let surface = unsafe { &mut *s };
    let slice = unsafe { std::slice::from_raw_parts(data, len) };
    surface.paste(slice);
}

/// Scroll the viewport by `delta_rows` (negative = up into history).
///
/// # Safety
///
/// `s` must be a valid [`Surface`] pointer from [`tako_surface_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_scroll(s: *mut Surface, delta_rows: i64) {
    if s.is_null() {
        return;
    }
    let surface = unsafe { &mut *s };
    surface.scroll(delta_rows);
}

/// Returns 1 if any mouse tracking mode is on (embedder should forward mouse
/// events to the PTY rather than do selection), 0 otherwise.
///
/// # Safety
///
/// `s` must be a valid [`Surface`] pointer from [`tako_surface_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_mouse_tracking(s: *mut Surface) -> i32 {
    if s.is_null() {
        return 0;
    }
    let surface = unsafe { &*s };
    i32::from(surface.mouse_tracking())
}

/// Take the latest window title, if it changed. Returns the length written
/// into `out_buf` (excluding NUL). Returns 0 when there is no new title or
/// `out_buf` is too small; the caller should pass a buffer of at least 256 B.
/// A NUL terminator is written after the title bytes.
///
/// # Safety
///
/// `s` must be a valid [`Surface`] pointer from [`tako_surface_new`].
/// `out_buf` must point to `cap` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_surface_take_title(
    s: *mut Surface,
    out_buf: *mut u8,
    cap: usize,
) -> usize {
    if s.is_null() || out_buf.is_null() || cap == 0 {
        return 0;
    }
    let surface = unsafe { &mut *s };
    let Some(title) = surface.take_host_title() else {
        return 0;
    };
    let bytes = title.as_bytes();
    // Need 1 byte for NUL terminator.
    if bytes.len() + 1 > cap {
        return 0;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), out_buf, bytes.len());
        *out_buf.add(bytes.len()) = 0;
    }
    bytes.len()
}

// Keep CString reachable for the FFI doc; avoids dead-code churn if unused.
#[allow(dead_code)]
fn _cstring_marker(s: &str) -> CString {
    CString::new(s).unwrap()
}
