#![expect(
    unsafe_code,
    reason = "the warp's own GL: a program and a draw Smithay cannot make"
)]

//! The warp's program and its draw, in raw GL. Smithay and std only, so
//! `dev/wirecheck` compiles and draws it on a GPU (cases 11e and 11g).
//! Compiled between frames by `pass::Programs::warp`, latched there, and
//! carried by each `Warp`.

use smithay::{
    backend::renderer::gles::ffi,
    utils::{Physical, Rectangle},
};

const VERTEX: &str = r"
precision highp float;
uniform mat3 projection;
attribute vec2 position;
attribute vec3 uvq;
// Explicitly highp on both sides: a varying whose precision differs between
// the two stages is a link error the driver is free to resolve by handing the
// fragment stage zeroes, which looks exactly like an attribute that was never
// uploaded.
varying highp vec3 v_uvq;
void main() {
    vec3 clip = projection * vec3(position, 1.0);
    gl_Position = vec4(clip.xy, 0.0, 1.0);
    v_uvq = uvq;
}
";

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
            let vertex = compile_stage(gl, ffi::VERTEX_SHADER, VERTEX)
                .ok_or("the warp's vertex shader did not compile")?;
            let fragment = compile_stage(gl, ffi::FRAGMENT_SHADER, FRAGMENT)
                .ok_or("the warp's fragment shader did not compile")?;
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
                return Err("the warp program did not link");
            }

            let mut buffer = 0;
            gl.GenBuffers(1, &raw mut buffer);

            Ok(Self {
                id,
                projection: gl.GetUniformLocation(id, c"projection".as_ptr().cast()),
                tex: gl.GetUniformLocation(id, c"tex".as_ptr().cast()),
                alpha: gl.GetUniformLocation(id, c"alpha".as_ptr().cast()),
                #[expect(
                    clippy::cast_sign_loss,
                    reason = "a located attribute is never negative"
                )]
                position: gl.GetAttribLocation(id, c"position".as_ptr().cast()) as u32,
                #[expect(clippy::cast_sign_loss, reason = "as above")]
                uvq: gl.GetAttribLocation(id, c"uvq".as_ptr().cast()) as u32,
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

            gl.ActiveTexture(ffi::TEXTURE0);
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
            gl.Uniform1i(self.tex, 0);

            gl.Enable(ffi::BLEND);
            gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);

            gl.BindBuffer(ffi::ARRAY_BUFFER, self.buffer);
            gl.BufferData(
                ffi::ARRAY_BUFFER,
                isize::try_from(std::mem::size_of_val(vertices)).unwrap_or_default(),
                vertices.as_ptr().cast(),
                ffi::STREAM_DRAW,
            );

            let stride = i32::try_from(5 * std::mem::size_of::<f32>()).unwrap_or_default();
            gl.EnableVertexAttribArray(self.position);
            gl.VertexAttribPointer(
                self.position,
                2,
                ffi::FLOAT,
                ffi::FALSE,
                stride,
                std::ptr::null(),
            );
            gl.EnableVertexAttribArray(self.uvq);
            gl.VertexAttribPointer(
                self.uvq,
                3,
                ffi::FLOAT,
                ffi::FALSE,
                stride,
                (2 * std::mem::size_of::<f32>()) as *const _,
            );

            // Per vertex, not per instance. The divisor is state on the
            // attribute *index*, not on the program, and Smithay draws its own
            // elements instanced with a divisor of 1 on index 1 -- which is
            // where `uvq` happens to land. Inherit that and every vertex reads
            // corner 0's texture coordinate, so the whole quad samples one
            // texel: the window renders as a single flat colour, geometry
            // perfectly correct, which is a memorably confusing way to fail.
            gl.VertexAttribDivisor(self.position, 0);
            gl.VertexAttribDivisor(self.uvq, 0);

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
            gl.DisableVertexAttribArray(self.position);
            gl.DisableVertexAttribArray(self.uvq);
            gl.BindBuffer(ffi::ARRAY_BUFFER, 0);
            gl.BindTexture(ffi::TEXTURE_2D, 0);
            gl.UseProgram(0);
        }
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
