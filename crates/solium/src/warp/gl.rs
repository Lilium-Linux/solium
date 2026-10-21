#![expect(
    unsafe_code,
    reason = "the warp's own GL: a program and a draw Smithay cannot make"
)]

//! The warp's program and its draw, in raw GL, and the program a backdrop
//! beneath a warp and a capture's hole are drawn with (Ruling F37, amended
//! 2026-10-14, live under motion and warps). Smithay and std only, so
//! `dev/wirecheck` compiles and draws them on a GPU (cases 11e, 11g and
//! 13i; 12j draws a folder's frag through [`triangles`] too, as
//! `effect::gl::Program::draw_warp`). Compiled between frames by `pass::Programs::warp` and
//! `pass::Programs::beneath`, latched there, and carried by each `Warp`,
//! `effect::beneath::Beneath` and `effect::beneath::Hole`.

use smithay::{
    backend::renderer::gles::ffi,
    utils::{Physical, Rectangle},
};

/// The warp's vertex shader: one text, in `solium_effects`, for this program
/// and for a folder's frag compiled into the warp (Task 29).
const VERTEX: &str = solium_effects::glsl::WARP_VERTEX;

const FRAGMENT: &str = r"
precision highp float;
uniform sampler2D tex;
uniform float alpha;
varying highp vec3 v_uvq;
void main() {
    // The divide is the perspective correction: without it the texture is
    // interpolated affinely across each triangle and creases along the
    // diagonal they share.
    vec2 uv = v_uvq.xy / v_uvq.z;
    gl_FragColor = texture2D(tex, uv) * alpha;
}
";

/// The scissor box, in window pixels (`[x, y, w, h]` for `glScissor`), of a
/// rectangle in the frame's physical pixels: its corners through the frame's
/// projection (column-major, as `UniformMatrix3fv(…, FALSE, …)` takes it) to
/// clip space, then through the viewport, and their bounding box. Right for
/// every output transform, because the eight take axis-aligned rectangles to
/// axis-aligned ones. `warp::tests::a_damage_rectangle_scissors_where_the_projection_puts_it`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "window coordinates, rounded"
)]
pub(crate) fn scissor_box(
    projection: &[f32; 9],
    viewport: [i32; 4],
    rect: Rectangle<i32, Physical>,
) -> [i32; 4] {
    let corner = |x: i32, y: i32| {
        let (x, y) = (x as f32, y as f32);
        let clip_x = projection[0] * x + projection[3] * y + projection[6];
        let clip_y = projection[1] * x + projection[4] * y + projection[7];
        (
            viewport[0] as f32 + (clip_x + 1.0) / 2.0 * viewport[2] as f32,
            viewport[1] as f32 + (clip_y + 1.0) / 2.0 * viewport[3] as f32,
        )
    };
    let (x0, y0, x1, y1) = (
        rect.loc.x,
        rect.loc.y,
        rect.loc.x + rect.size.w,
        rect.loc.y + rect.size.h,
    );
    let points = [
        corner(x0, y0),
        corner(x1, y0),
        corner(x0, y1),
        corner(x1, y1),
    ];
    let left = points.iter().map(|p| p.0).fold(f32::MAX, f32::min).round();
    let right = points.iter().map(|p| p.0).fold(f32::MIN, f32::max).round();
    let bottom = points.iter().map(|p| p.1).fold(f32::MAX, f32::min).round();
    let top = points.iter().map(|p| p.1).fold(f32::MIN, f32::max).round();
    [
        left as i32,
        bottom as i32,
        (right - left) as i32,
        (top - bottom) as i32,
    ]
}

/// The compiled warp program: GL names only, so `Copy`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Program {
    id: ffi::types::GLuint,
    projection: ffi::types::GLint,
    tex: ffi::types::GLint,
    alpha: ffi::types::GLint,
    position: ffi::types::GLuint,
    uvq: ffi::types::GLuint,
    buffer: ffi::types::GLuint,
}

impl Program {
    /// Compile and link, or say which stage failed.
    ///
    /// # Safety
    /// A GL context is current: inside `with_context`.
    pub(crate) unsafe fn compile(gl: &ffi::Gles2) -> Result<Self, &'static str> {
        // SAFETY: the caller's contract.
        unsafe {
            let id = link(
                gl,
                (VERTEX, FRAGMENT),
                (
                    "the warp's vertex shader did not compile",
                    "the warp's fragment shader did not compile",
                    "the warp program did not link",
                ),
            )?;
            let (position, uvq, buffer) = attributes(gl, id);
            Ok(Self {
                id,
                projection: gl.GetUniformLocation(id, c"projection".as_ptr().cast()),
                tex: gl.GetUniformLocation(id, c"tex".as_ptr().cast()),
                alpha: gl.GetUniformLocation(id, c"alpha".as_ptr().cast()),
                position,
                uvq,
                buffer,
            })
        }
    }

    /// Draw `vertices` (x, y, u·q, v·q, q per vertex, physical pixels) with
    /// `texture`, through `projection`: once, or with `scissors` (window
    /// boxes, from [`scissor_box`]) once under each.
    ///
    /// # Safety
    /// A GL context is current, and every name is this context's.
    pub(crate) unsafe fn draw(
        &self,
        gl: &ffi::Gles2,
        projection: &[f32; 9],
        texture: ffi::types::GLuint,
        vertices: &[f32],
        alpha: f32,
        scissors: &[[i32; 4]],
    ) {
        // SAFETY: the caller's contract.
        unsafe {
            gl.UseProgram(self.id);
            gl.UniformMatrix3fv(self.projection, 1, ffi::FALSE, projection.as_ptr());
            gl.Uniform1f(self.alpha, alpha);
            bind_texture(gl, 0, texture);
            gl.Uniform1i(self.tex, 0);

            gl.Enable(ffi::BLEND);
            gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);

            triangles(
                gl,
                (self.buffer, self.position, Some(self.uvq)),
                vertices,
                scissors,
            );
            gl.BindTexture(ffi::TEXTURE_2D, 0);
            gl.UseProgram(0);
        }
    }
}

const BENEATH_VERTEX: &str = r"
precision highp float;
uniform mat3 projection;
attribute vec2 position;
attribute vec3 uvq;
varying highp vec3 v_uvq;
// Where the vertex is in the frame's own pixels: what the backdrop is
// sampled at, linear on the screen as it should be.
varying highp vec2 v_screen;
void main() {
    vec3 clip = projection * vec3(position, 1.0);
    gl_Position = vec4(clip.xy, 0.0, 1.0);
    v_uvq = uvq;
    v_screen = position;
}
";

const BENEATH_FRAGMENT: &str = r"
precision highp float;
uniform sampler2D tex;
uniform float alpha;
uniform vec4 screen_map;
uniform vec4 mask_rect;
uniform vec4 mask_radii;
uniform vec2 mask_size;
uniform float erase;
varying highp vec3 v_uvq;
varying highp vec2 v_screen;
void main() {
    // Where the fragment is in the pane's own capture: the mesh's texture
    // coordinate, divided for perspective as the warp divides it, then
    // MASKED_TEXTURE's rounded rectangle.
    vec2 px = (v_uvq.xy / v_uvq.z) * mask_size - mask_rect.xy;
    vec2 half_size = mask_rect.zw * 0.5;
    vec2 at = px - half_size;
    float picked = at.x < 0.0 ? (at.y < 0.0 ? mask_radii.x : mask_radii.z) : (at.y < 0.0 ? mask_radii.y : mask_radii.w);
    float r = min(picked, min(half_size.x, half_size.y));
    vec2 p = abs(at) - (half_size - vec2(r));
    float away = min(max(p.x, p.y), 0.0) + length(max(p, 0.0)) - r;
    float coverage = clamp(0.5 - away, 0.0, 1.0);
    if (erase > 0.5) {
        // Blended ZERO, ONE_MINUS_SRC_ALPHA: what is below is taken out.
        gl_FragColor = vec4(coverage);
        return;
    }
    // The backdrop where the fragment itself is: a memo spans the monitor,
    // a T2 result its place's box (ScreenMap).
    gl_FragColor = texture2D(tex, v_screen * screen_map.xy + screen_map.zw) * (alpha * coverage);
}
";

/// What a [`BeneathProgram`] draws: a backdrop sampled where each fragment
/// is, `map` a `ScreenMap`'s `(mul, add)` and `alpha` the pane's opacity;
/// or a hole, what is below taken out in the mask's shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Draw {
    Backdrop { map: [f32; 4], alpha: f32 },
    Erase,
}

/// The compiled beneath program (Ruling F37): GL names only, so `Copy`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BeneathProgram {
    id: ffi::types::GLuint,
    projection: ffi::types::GLint,
    tex: ffi::types::GLint,
    alpha: ffi::types::GLint,
    screen_map: ffi::types::GLint,
    mask_rect: ffi::types::GLint,
    mask_radii: ffi::types::GLint,
    mask_size: ffi::types::GLint,
    erase: ffi::types::GLint,
    position: ffi::types::GLuint,
    uvq: ffi::types::GLuint,
    buffer: ffi::types::GLuint,
}

impl BeneathProgram {
    /// Compile and link, or say which stage failed.
    ///
    /// # Safety
    /// A GL context is current: inside `with_context`.
    pub(crate) unsafe fn compile(gl: &ffi::Gles2) -> Result<Self, &'static str> {
        // SAFETY: the caller's contract.
        unsafe {
            let id = link(
                gl,
                (BENEATH_VERTEX, BENEATH_FRAGMENT),
                (
                    "the beneath program's vertex shader did not compile",
                    "the beneath program's fragment shader did not compile",
                    "the beneath program did not link",
                ),
            )?;
            let (position, uvq, buffer) = attributes(gl, id);
            let at = |name: &std::ffi::CStr| gl.GetUniformLocation(id, name.as_ptr().cast());
            Ok(Self {
                id,
                projection: at(c"projection"),
                tex: at(c"tex"),
                alpha: at(c"alpha"),
                screen_map: at(c"screen_map"),
                mask_rect: at(c"mask_rect"),
                mask_radii: at(c"mask_radii"),
                mask_size: at(c"mask_size"),
                erase: at(c"erase"),
                position,
                uvq,
                buffer,
            })
        }
    }

    /// Draw `vertices` (x, y, u·q, v·q, q per vertex, physical pixels)
    /// through `projection`, cut by `mask` (the rect, radii and size of
    /// `fragment::MASKED_TEXTURE`, in the texture coordinates' pixels):
    /// `texture` sampled at each fragment's own position for a backdrop,
    /// blended `ONE, ONE_MINUS_SRC_ALPHA`; for a hole no texture, blended
    /// `ZERO, ONE_MINUS_SRC_ALPHA`, then put back to what smithay expects.
    /// Once, or once under each of `scissors`.
    ///
    /// # Safety
    /// A GL context is current, and every name is this context's.
    #[expect(
        clippy::too_many_arguments,
        reason = "the warp's draw, and the mask and what is drawn"
    )]
    pub(crate) unsafe fn draw(
        &self,
        gl: &ffi::Gles2,
        projection: &[f32; 9],
        texture: ffi::types::GLuint,
        vertices: &[f32],
        draw: Draw,
        (rect, radii, size): ([f32; 4], [f32; 4], [f32; 2]),
        scissors: &[[i32; 4]],
    ) {
        // SAFETY: the caller's contract.
        unsafe {
            gl.UseProgram(self.id);
            gl.UniformMatrix3fv(self.projection, 1, ffi::FALSE, projection.as_ptr());
            gl.Uniform4f(self.mask_rect, rect[0], rect[1], rect[2], rect[3]);
            gl.Uniform4f(self.mask_radii, radii[0], radii[1], radii[2], radii[3]);
            gl.Uniform2f(self.mask_size, size[0], size[1]);
            gl.Enable(ffi::BLEND);
            match draw {
                Draw::Backdrop { map, alpha } => {
                    gl.Uniform1f(self.erase, 0.0);
                    gl.Uniform1f(self.alpha, alpha);
                    gl.Uniform4f(self.screen_map, map[0], map[1], map[2], map[3]);
                    bind_texture(gl, 0, texture);
                    gl.Uniform1i(self.tex, 0);
                    gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);
                }
                Draw::Erase => {
                    gl.Uniform1f(self.erase, 1.0);
                    gl.BlendFunc(ffi::ZERO, ffi::ONE_MINUS_SRC_ALPHA);
                }
            }
            triangles(
                gl,
                (self.buffer, self.position, Some(self.uvq)),
                vertices,
                scissors,
            );
            gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);
            gl.BindTexture(ffi::TEXTURE_2D, 0);
            gl.UseProgram(0);
        }
    }
}

/// Bind `texture` on `unit`, clamped and linear, as every warp program
/// samples it, leaving `unit` active.
///
/// # Safety
/// A GL context is current, and `texture` is its.
pub(crate) unsafe fn bind_texture(gl: &ffi::Gles2, unit: u32, texture: ffi::types::GLuint) {
    // SAFETY: the caller's contract.
    unsafe {
        gl.ActiveTexture(ffi::TEXTURE0 + unit);
        gl.BindTexture(ffi::TEXTURE_2D, texture);
        // Clamped, so the divide landing a hair outside 0..1 at an edge
        // samples the edge rather than wrapping to the far side.
        gl.TexParameteri(
            ffi::TEXTURE_2D,
            ffi::TEXTURE_WRAP_S,
            i32::try_from(ffi::CLAMP_TO_EDGE).unwrap_or_default(),
        );
        gl.TexParameteri(
            ffi::TEXTURE_2D,
            ffi::TEXTURE_WRAP_T,
            i32::try_from(ffi::CLAMP_TO_EDGE).unwrap_or_default(),
        );
        // Linear, and explicitly: a texture whose min filter still wants
        // mipmaps -- the GL default, and what `create_buffer` hands back --
        // is incomplete, and an incomplete texture samples as opaque
        // black. That reads as "the capture drew nothing" and sends you
        // looking in entirely the wrong place.
        gl.TexParameteri(
            ffi::TEXTURE_2D,
            ffi::TEXTURE_MIN_FILTER,
            i32::try_from(ffi::LINEAR).unwrap_or_default(),
        );
        gl.TexParameteri(
            ffi::TEXTURE_2D,
            ffi::TEXTURE_MAG_FILTER,
            i32::try_from(ffi::LINEAR).unwrap_or_default(),
        );
    }
}

/// Draw `vertices` (x, y, u·q, v·q, q per vertex) as triangles through the
/// bound program, from `buffer` into its `position` and `uvq` attributes,
/// `uvq` none for a program that reads no texture coordinate (a pixel
/// program whose frag reads no `uv`, which the driver drops it from,
/// wirecheck case 12j): once, or once under each of `scissors`; then put
/// back what smithay expects to find.
///
/// # Safety
/// A GL context is current, a program reading `position` (and `uvq`, when
/// given) is in use, and every name is this context's.
pub(crate) unsafe fn triangles(
    gl: &ffi::Gles2,
    (buffer, position, uvq): (
        ffi::types::GLuint,
        ffi::types::GLuint,
        Option<ffi::types::GLuint>,
    ),
    vertices: &[f32],
    scissors: &[[i32; 4]],
) {
    // SAFETY: the caller's contract.
    unsafe {
        gl.BindBuffer(ffi::ARRAY_BUFFER, buffer);
        gl.BufferData(
            ffi::ARRAY_BUFFER,
            isize::try_from(std::mem::size_of_val(vertices)).unwrap_or_default(),
            vertices.as_ptr().cast(),
            ffi::STREAM_DRAW,
        );

        let stride = i32::try_from(5 * std::mem::size_of::<f32>()).unwrap_or_default();
        gl.EnableVertexAttribArray(position);
        gl.VertexAttribPointer(
            position,
            2,
            ffi::FLOAT,
            ffi::FALSE,
            stride,
            std::ptr::null(),
        );
        if let Some(uvq) = uvq {
            gl.EnableVertexAttribArray(uvq);
            gl.VertexAttribPointer(
                uvq,
                3,
                ffi::FLOAT,
                ffi::FALSE,
                stride,
                (2 * std::mem::size_of::<f32>()) as *const _,
            );
        }

        // Per vertex, not per instance. The divisor is state on the
        // attribute *index*, not on the program, and Smithay draws its own
        // elements instanced with a divisor of 1 on index 1 -- which is
        // where `uvq` happens to land. Inherit that and every vertex reads
        // corner 0's texture coordinate, so the whole quad samples one
        // texel: the window renders as a single flat colour, geometry
        // perfectly correct, which is a memorably confusing way to fail.
        gl.VertexAttribDivisor(position, 0);
        if let Some(uvq) = uvq {
            gl.VertexAttribDivisor(uvq, 0);
        }

        #[expect(
            clippy::cast_possible_truncation,
            reason = "a mesh is thousands of vertices, not billions"
        )]
        let count = (vertices.len() / 5) as i32;
        if scissors.is_empty() {
            gl.DrawArrays(ffi::TRIANGLES, 0, count);
        } else {
            // Once per damage rectangle: drawing the whole mesh under
            // partial damage blends its translucent pixels a second time
            // over the copy already there (wirecheck case 11g). The box
            // smithay left is put back: it leaves the scissor on at the
            // whole output (`gles/mod.rs:2058-2059`).
            let mut saved = [0_i32; 4];
            gl.GetIntegerv(ffi::SCISSOR_BOX, saved.as_mut_ptr());
            gl.Enable(ffi::SCISSOR_TEST);
            for [x, y, w, h] in scissors {
                gl.Scissor(*x, *y, *w, *h);
                gl.DrawArrays(ffi::TRIANGLES, 0, count);
            }
            gl.Scissor(saved[0], saved[1], saved[2], saved[3]);
        }

        // Put back what Smithay expects to find: it does not re-bind
        // everything per element, so leaving our buffer and attributes
        // enabled corrupts whatever draws next.
        gl.DisableVertexAttribArray(position);
        if let Some(uvq) = uvq {
            gl.DisableVertexAttribArray(uvq);
        }
        gl.BindBuffer(ffi::ARRAY_BUFFER, 0);
    }
}

/// Compile both stages and link them, or say which failed, in `what`'s
/// words: (vertex, fragment, link).
///
/// # Safety
/// A GL context is current: inside `with_context`.
unsafe fn link(
    gl: &ffi::Gles2,
    (vertex, fragment): (&str, &str),
    what: (&'static str, &'static str, &'static str),
) -> Result<ffi::types::GLuint, &'static str> {
    // SAFETY: the caller's contract.
    unsafe {
        let vertex = compile_stage(gl, ffi::VERTEX_SHADER, vertex).ok_or(what.0)?;
        let fragment = compile_stage(gl, ffi::FRAGMENT_SHADER, fragment).ok_or(what.1)?;
        let id = gl.CreateProgram();
        gl.AttachShader(id, vertex);
        gl.AttachShader(id, fragment);
        gl.LinkProgram(id);
        gl.DeleteShader(vertex);
        gl.DeleteShader(fragment);

        let mut linked = 0;
        gl.GetProgramiv(id, ffi::LINK_STATUS, &raw mut linked);
        if linked == 0 {
            gl.DeleteProgram(id);
            return Err(what.2);
        }
        Ok(id)
    }
}

/// A linked program's `position` and `uvq` attributes, and a buffer of its
/// own for its vertices.
///
/// # Safety
/// A GL context is current, and `id` is a program linked in it.
unsafe fn attributes(
    gl: &ffi::Gles2,
    id: ffi::types::GLuint,
) -> (ffi::types::GLuint, ffi::types::GLuint, ffi::types::GLuint) {
    // SAFETY: the caller's contract.
    unsafe {
        let mut buffer = 0;
        gl.GenBuffers(1, &raw mut buffer);
        #[expect(
            clippy::cast_sign_loss,
            reason = "a located attribute is never negative"
        )]
        let position = gl.GetAttribLocation(id, c"position".as_ptr().cast()) as u32;
        #[expect(clippy::cast_sign_loss, reason = "as above")]
        let uvq = gl.GetAttribLocation(id, c"uvq".as_ptr().cast()) as u32;
        (position, uvq, buffer)
    }
}

/// One shader stage, or `None` if it did not compile.
///
/// # Safety
/// A GL context is current: inside `with_context`.
unsafe fn compile_stage(
    gl: &ffi::Gles2,
    kind: ffi::types::GLenum,
    source: &str,
) -> Option<ffi::types::GLuint> {
    // SAFETY: the caller's contract.
    unsafe {
        let shader = gl.CreateShader(kind);
        let length = i32::try_from(source.len()).unwrap_or_default();
        gl.ShaderSource(
            shader,
            1,
            [source.as_ptr().cast()].as_ptr(),
            &raw const length,
        );
        gl.CompileShader(shader);

        let mut compiled = 0;
        gl.GetShaderiv(shader, ffi::COMPILE_STATUS, &raw mut compiled);
        if compiled == 0 {
            gl.DeleteShader(shader);
            return None;
        }
        Some(shader)
    }
}
