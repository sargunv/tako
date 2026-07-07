//! GL renderer for the [`QQuickFramebufferObject`](crate) terminal surface.
//!
//! [`GlRenderer`] owns the GL resources (shader program, VBO/IBO/VAO, atlas
//! texture) used to draw the terminal each frame. It is constructed without a
//! GL context (on the GUI thread), then lazily attaches to one on the render
//! thread via [`GlRenderer::ensure_gl`] (which needs Qt's `QOpenGLContext`
//! current so glow can resolve function pointers through it).
//!
//! Threading: `ingest_plan` is called from
//! `QQuickFramebufferObject::Renderer::synchronize` on the GUI thread; `render`
//! is called from `Renderer::render` on the render thread. The Qt framework
//! guarantees these never overlap, so the renderer's staging buffers are
//! written-then-read sequentially across the thread boundary.
//!
//! Vertex layout (20 bytes; must match `Surface::Vertex` in
//! [`crate::surface`]): `{ pos: [f32;2], uv: [f32;2], color: [u8;4] }`. Indices
//! use the standard quad pattern `0,1,2,0,2,3` repeated per quad, generated
//! once into a static IBO.

#![allow(unsafe_code)]
#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::c_void;
use std::os::raw::c_char;

use glow::HasContext;

use crate::surface::{FramePlan, Vertex};

const VERTEX_SHADER_SRC: &str = r#"#version 110
attribute vec2 a_pos;
attribute vec2 a_uv;
attribute vec4 a_color;
uniform vec2 u_viewport;
varying vec2 v_uv;
varying vec4 v_color;
void main() {
    // Pixel coords (origin top-left) → NDC. We do NOT flip Y here:
    // QQuickFramebufferObject's default compositing already inverts the FBO
    // vertically for display, so our top-left-origin vertex coords map
    // directly to the item's top-left.
    vec2 ndc = (a_pos / u_viewport) * 2.0 - 1.0;
    gl_Position = vec4(ndc.x, ndc.y, 0.0, 1.0);
    v_uv = a_uv;
    v_color = a_color;
}
"#;

const FRAGMENT_SHADER_SRC: &str = r#"#version 110
uniform sampler2D u_atlas;
varying vec2 v_uv;
varying vec4 v_color;
void main() {
    // Atlas is single-channel grayscale; sample red as glyph coverage and
    // modulate the per-vertex color. Background/cursor quads sample the white
    // texel (coverage = 1.0) and so render as flat opaque color.
    float coverage = texture2D(u_atlas, v_uv).r;
    gl_FragColor = vec4(v_color.rgb, v_color.a * coverage);
}
"#;

/// Pre-allocated capacity: 16384 quads = 65536 vertices. A terminal of
/// 200×50 cells (~10000 cells) with one glyph each stays well under this; we
/// clamp draws to the cap rather than resizing GL storage mid-frame.
const MAX_QUADS: usize = 1 << 14;

/// One renderer-side terminal GL pipeline. Created without a context; attach
/// to a GL context via [`Self::ensure_gl`] on the render thread.
pub struct GlRenderer {
    gl: Option<glow::Context>,
    program: Option<glow::Program>,
    vao: Option<glow::VertexArray>,
    vbo: Option<glow::Buffer>,
    ibo: Option<glow::Buffer>,
    atlas_texture: Option<glow::Texture>,
    u_viewport: Option<glow::UniformLocation>,
    /// Generation last uploaded to `atlas_texture`. Re-upload on change.
    atlas_generation: u64,

    // ---- staging (written by ingest_plan on GUI thread) ----
    vertex_buf: Vec<Vertex>,
    atlas_buf: Vec<u8>,
    atlas_w: u32,
    atlas_h: u32,
    clear_color: [u8; 4],
    viewport: (i32, i32),
}

impl Default for GlRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl GlRenderer {
    pub fn new() -> Self {
        Self {
            gl: None,
            program: None,
            vao: None,
            vbo: None,
            ibo: None,
            atlas_texture: None,
            u_viewport: None,
            atlas_generation: 0,
            vertex_buf: Vec::new(),
            atlas_buf: Vec::new(),
            atlas_w: 0,
            atlas_h: 0,
            clear_color: [0; 4],
            viewport: (1, 1),
        }
    }

    /// Lazily create the glow context + compile the shader + allocate GL
    /// buffers. Idempotent. Must run on the render thread with Qt's
    /// `QOpenGLContext` current.
    ///
    /// # Safety
    ///
    /// `loader` must resolve GL function names against the *currently-current*
    /// GL context on this thread. The returned pointers must remain valid for
    /// the lifetime of the renderer.
    pub unsafe fn ensure_gl(&mut self, loader: LoaderFn, loader_userdata: *mut c_void) {
        if self.gl.is_some() {
            return;
        }
        let gl = unsafe {
            glow::Context::from_loader_function(|sym: &str| -> *const c_void {
                let cs = std::ffi::CString::new(sym).unwrap();
                loader(cs.as_ptr(), loader_userdata)
            })
        };

        let program = unsafe { Self::compile_program(&gl) };

        let vao = unsafe { gl.create_vertex_array() }.expect("glCreateVertexArrays");
        unsafe {
            gl.bind_vertex_array(Some(vao));

            // VBO: pre-allocate enough for MAX_QUADS quads; we re-stream its
            // contents every frame via buffer_sub_data (orphan-free, no
            // reallocation).
            let vbo = gl.create_buffer().expect("glCreateBuffers (vbo)");
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_size(
                glow::ARRAY_BUFFER,
                (MAX_QUADS * 4 * core::mem::size_of::<Vertex>()) as i32,
                glow::DYNAMIC_DRAW,
            );

            // IBO: standard quad pattern, generated once for MAX_QUADS.
            let ibo = gl.create_buffer().expect("glCreateBuffers (ibo)");
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ibo));
            let mut indices = Vec::<u32>::with_capacity(MAX_QUADS * 6);
            for i in 0..(MAX_QUADS as u32) {
                let b = i * 4;
                indices.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
            }
            // glow 0.17 dropped the typed u32 variant; upload as bytes
            // (u32 is host-endian, which matches the GL driver's expectation).
            let index_bytes = core::slice::from_raw_parts(
                indices.as_ptr() as *const u8,
                indices.len() * core::mem::size_of::<u32>(),
            );
            gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, index_bytes, glow::STATIC_DRAW);

            // Vertex layout (20-byte stride): pos(2f)@0, uv(2f)@8, color(4ub,norm)@16.
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 20, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, 20, 8);
            gl.enable_vertex_attrib_array(2);
            gl.vertex_attrib_pointer_f32(2, 4, glow::UNSIGNED_BYTE, true, 20, 16);

            gl.bind_vertex_array(None);
            self.vbo = Some(vbo);
            self.ibo = Some(ibo);
        }

        // Atlas texture: single-channel, bilinear, clamped. Initial storage is
        // allocated on the first ingest (tex_image_2d) call.
        let atlas_texture = unsafe { gl.create_texture() }.expect("glCreateTextures");
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(atlas_texture));
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_S,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::CLAMP_TO_EDGE as i32,
            );
        }

        let u_viewport = unsafe { gl.get_uniform_location(program, "u_viewport") };
        // Pin u_atlas sampler to texture unit 0 once; the renderer binds the
        // atlas there every frame.
        let u_atlas = unsafe { gl.get_uniform_location(program, "u_atlas") };
        unsafe {
            gl.use_program(self.program);
            gl.uniform_1_i32(u_atlas.as_ref(), 0);
            gl.use_program(None);
        }

        self.gl = Some(gl);
        self.program = Some(program);
        self.vao = Some(vao);
        self.atlas_texture = Some(atlas_texture);
        self.u_viewport = u_viewport;
    }

    unsafe fn compile_program(gl: &glow::Context) -> glow::Program {
        let vs = unsafe { gl.create_shader(glow::VERTEX_SHADER) }.expect("create_shader vs");
        unsafe {
            gl.shader_source(vs, VERTEX_SHADER_SRC);
            gl.compile_shader(vs);
            assert!(
                gl.get_shader_compile_status(vs),
                "vertex shader compile: {}",
                gl.get_shader_info_log(vs)
            );
        }
        let fs = unsafe { gl.create_shader(glow::FRAGMENT_SHADER) }.expect("create_shader fs");
        unsafe {
            gl.shader_source(fs, FRAGMENT_SHADER_SRC);
            gl.compile_shader(fs);
            assert!(
                gl.get_shader_compile_status(fs),
                "fragment shader compile: {}",
                gl.get_shader_info_log(fs)
            );
        }
        let program = unsafe { gl.create_program() }.expect("create_program");
        unsafe {
            gl.attach_shader(program, vs);
            gl.attach_shader(program, fs);
            // Bind attrib locations before linking so the layout matches the
            // VAO setup above (0=pos, 1=uv, 2=color).
            gl.bind_attrib_location(program, 0, "a_pos");
            gl.bind_attrib_location(program, 1, "a_uv");
            gl.bind_attrib_location(program, 2, "a_color");
            gl.link_program(program);
            assert!(
                gl.get_program_link_status(program),
                "program link: {}",
                gl.get_program_info_log(program)
            );
            gl.detach_shader(program, vs);
            gl.detach_shader(program, fs);
            gl.delete_shader(vs);
            gl.delete_shader(fs);
        }
        program
    }

    /// Deep-copy a [`FramePlan`]'s borrowed data into renderer-owned staging.
    /// GUI thread (called from `Renderer::synchronize`).
    ///
    /// # Safety
    ///
    /// `plan.vertices` and `plan.atlas_pixels` must be valid for their stated
    /// lengths, or null (in which case the corresponding copy is skipped).
    pub unsafe fn ingest_plan(&mut self, plan: &FramePlan, viewport_w: i32, viewport_h: i32) {
        self.viewport = (viewport_w.max(1), viewport_h.max(1));
        self.clear_color = plan.clear_color;

        self.vertex_buf.clear();
        let n = plan.vertex_count;
        if n > 0 && !plan.vertices.is_null() {
            // Cap at MAX_QUADS quads to stay inside the pre-sized IBO/VBO.
            let max_verts = MAX_QUADS * 4;
            let take = n.min(max_verts);
            let slice = unsafe { core::slice::from_raw_parts(plan.vertices, take) };
            self.vertex_buf.extend_from_slice(slice);
        }

        if plan.atlas_generation != self.atlas_generation
            && plan.atlas_w > 0
            && plan.atlas_h > 0
            && !plan.atlas_pixels.is_null()
        {
            let size = (plan.atlas_w * plan.atlas_h) as usize;
            let pixels = unsafe { core::slice::from_raw_parts(plan.atlas_pixels, size) };
            self.atlas_buf.clear();
            self.atlas_buf.extend_from_slice(pixels);
            self.atlas_w = plan.atlas_w;
            self.atlas_h = plan.atlas_h;
            self.atlas_generation = plan.atlas_generation;
        }
    }

    /// Draw the current staging data. Render thread, GL context current.
    pub fn render(&mut self) {
        let Some(gl) = self.gl.as_ref() else {
            return;
        };
        unsafe {
            // Re-upload atlas texture if its generation changed since last frame.
            if !self.atlas_buf.is_empty() && self.atlas_w > 0 && self.atlas_h > 0 {
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
                gl.bind_texture(glow::TEXTURE_2D, self.atlas_texture);
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::R8 as i32,
                    self.atlas_w as i32,
                    self.atlas_h as i32,
                    0,
                    glow::RED,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&self.atlas_buf)),
                );
            }

            gl.viewport(0, 0, self.viewport.0, self.viewport.1);
            gl.clear_color(
                self.clear_color[0] as f32 / 255.0,
                self.clear_color[1] as f32 / 255.0,
                self.clear_color[2] as f32 / 255.0,
                self.clear_color[3] as f32 / 255.0,
            );
            gl.clear(glow::COLOR_BUFFER_BIT);

            if self.vertex_buf.is_empty() {
                return;
            }

            gl.enable(glow::BLEND);
            gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);

            // Stream vertex data into the pre-allocated VBO.
            let bytes = self.vertex_buf.len() * core::mem::size_of::<Vertex>();
            let slice = core::slice::from_raw_parts(self.vertex_buf.as_ptr() as *const u8, bytes);
            gl.bind_buffer(glow::ARRAY_BUFFER, self.vbo);
            gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, 0, slice);

            gl.use_program(self.program);
            gl.uniform_2_f32(
                self.u_viewport.as_ref(),
                self.viewport.0 as f32,
                self.viewport.1 as f32,
            );

            gl.bind_vertex_array(self.vao);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, self.atlas_texture);

            let index_count = ((self.vertex_buf.len() / 4) * 6) as i32;
            gl.draw_elements(glow::TRIANGLES, index_count, glow::UNSIGNED_INT, 0);

            gl.bind_vertex_array(None);
            gl.use_program(None);
        }
    }
}

impl Drop for GlRenderer {
    fn drop(&mut self) {
        // GL resources are leaked here: deleting them requires the GL context
        // to be current on this thread, which isn't guaranteed at drop. The
        // context itself frees its resources when Qt tears it down, so the
        // leak is bounded to context lifetime. Revisit if a context is shared
        // across many surfaces (would accumulate).
    }
}

/// C function-pointer type for resolving GL entry points. Matches
/// `QOpenGLContext::getProcAddress`-style loaders.
pub type LoaderFn = unsafe extern "C" fn(*const c_char, *mut c_void) -> *const c_void;

// ---- C ABI for the C++ QQuickFramebufferObject::Renderer ----

/// Construct a renderer without a GL context. Safe to call from the GUI
/// thread. Use [`tako_gl_renderer_ensure_gl`] on the render thread to attach
/// it to Qt's GL context.
///
/// # Safety
///
/// The caller owns the returned pointer and must free it with
/// [`tako_gl_renderer_destroy`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_gl_renderer_new() -> *mut GlRenderer {
    Box::into_raw(Box::new(GlRenderer::new()))
}

/// Free a renderer returned by [`tako_gl_renderer_new`]. No-op on null.
///
/// # Safety
///
/// `r` must be null or a pointer previously returned by
/// [`tako_gl_renderer_new`], not already freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_gl_renderer_destroy(r: *mut GlRenderer) {
    if !r.is_null() {
        drop(unsafe { Box::from_raw(r) });
    }
}

/// Attach the renderer to the current thread's GL context. Idempotent. Must
/// run on the render thread with Qt's `QOpenGLContext` current.
///
/// # Safety
///
/// `r` must be a valid [`GlRenderer`] pointer. `loader` must resolve symbols
/// against the current GL context; `loader_userdata` is passed through
/// verbatim to each `loader` call (typically the `QOpenGLContext*`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_gl_renderer_ensure_gl(
    r: *mut GlRenderer,
    loader: LoaderFn,
    loader_userdata: *mut c_void,
) {
    if r.is_null() {
        return;
    }
    let renderer = unsafe { &mut *r };
    unsafe { renderer.ensure_gl(loader, loader_userdata) };
}

/// Copy a [`FramePlan`]'s borrowed data into the renderer's staging buffers.
/// GUI thread. Must run before [`tako_gl_renderer_render`] each frame.
///
/// # Safety
///
/// `r` must be a valid [`GlRenderer`] pointer. `plan` must point to a valid
/// [`FramePlan`] whose borrowed pointers are still live (i.e. before the next
/// `tako_surface_tick`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_gl_renderer_ingest_plan(
    r: *mut GlRenderer,
    plan: *const FramePlan,
    viewport_w: i32,
    viewport_h: i32,
) {
    if r.is_null() || plan.is_null() {
        return;
    }
    let renderer = unsafe { &mut *r };
    let plan = unsafe { &*plan };
    unsafe { renderer.ingest_plan(plan, viewport_w, viewport_h) };
}

/// Draw the latest staging data. Render thread, GL context current.
///
/// # Safety
///
/// `r` must be a valid [`GlRenderer`] pointer that has been attached to a GL
/// context via [`tako_gl_renderer_ensure_gl`]. The GL context must be current
/// on the calling thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tako_gl_renderer_render(r: *mut GlRenderer) {
    if r.is_null() {
        return;
    }
    let renderer = unsafe { &mut *r };
    renderer.render();
}
