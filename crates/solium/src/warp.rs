#![expect(
    unsafe_code,
    reason = "the one file that talks to GLES directly; every other file \
              uses Smithay's traits, which cannot place four corners \
              independently -- see docs/spikes/2026-09-06-3d-presentation.md"
)]

//! Drawing a texture through arbitrary corners: the one place with GLES in it.
//!
//! Everything else in the renderer talks to Smithay's traits. Nothing in those
//! traits can place a texture's four corners independently, and without that
//! there is no perspective, no genie and no fold — so this file crosses the
//! line and the rest does not. A Vulkan backend has one file to reimplement
//! rather than a habit to unpick. See
//! `docs/spikes/2026-09-06-3d-presentation.md`.
//!
//! Deliberately knows nothing about windows. It takes a texture and four
//! corners, which is what a tilted window, a Stage Manager card, an overview
//! thumbnail and a folding panel all reduce to.
//!
//! The interpolation is projective, not affine. Two triangles with plain UVs
//! would sample correctly at the corners and wrongly everywhere else, with a
//! visible crease along the shared diagonal — the classic wrong-looking
//! perspective. Each corner carries `q = 1/w` and the fragment shader divides,
//! which is what makes a receding edge compress its texture the way it should.

use std::cell::RefCell;

use smithay::{
    backend::renderer::{
        Texture,
        element::{Element, Id, Kind, RenderElement, UnderlyingStorage},
        gles::{GlesError, GlesFrame, GlesRenderer, GlesTexture, ffi},
        utils::CommitCounter,
        utils::OpaqueRegions,
    },
    utils::{Buffer as BufferCoords, Physical, Rectangle, Scale, Size, Transform},
};

use crate::mat4::Mat4;

/// One corner: where it lands, and its projective weight.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Corner {
    /// Position in physical output pixels.
    pub(crate) x: f32,
    pub(crate) y: f32,
    /// Texture coordinate, 0..1.
    pub(crate) u: f32,
    pub(crate) v: f32,
    /// `1/w` from the projection, for perspective-correct sampling.
    pub(crate) q: f32,
}

/// A texture drawn through four corners.
#[derive(Clone, Debug)]
pub(crate) struct Warp {
    id: Id,
    commit: CommitCounter,
    texture: GlesTexture,
    mesh: Mesh,
    bounds: Rectangle<i32, Physical>,
    alpha: f32,
}

impl Warp {
    /// Draw `texture` through `mesh`.
    pub(crate) fn new(
        id: Id,
        commit: CommitCounter,
        texture: GlesTexture,
        mesh: Mesh,
        alpha: f32,
    ) -> Self {
        // The bounding box is what the damage tracker reasons about: a warped
        // texture can land anywhere, and claiming a smaller area than it
        // covers leaves the difference undrawn until something else damages it.
        let mut left = f32::MAX;
        let mut right = f32::MIN;
        let mut top = f32::MAX;
        let mut bottom = f32::MIN;
        for corner in &mesh.vertices {
            left = left.min(corner.x);
            right = right.max(corner.x);
            top = top.min(corner.y);
            bottom = bottom.max(corner.y);
        }

        #[expect(
            clippy::cast_possible_truncation,
            reason = "output coordinates are small integers"
        )]
        let bounds = Rectangle::new(
            (left.floor() as i32, top.floor() as i32).into(),
            (
                ((right.ceil() - left.floor()) as i32).max(1),
                ((bottom.ceil() - top.floor()) as i32).max(1),
            )
                .into(),
        );

        Self {
            id,
            commit,
            texture,
            mesh,
            bounds,
            alpha,
        }
    }
}

/// A deformed window as triangles, in physical pixels.
///
/// Built per frame: the whole point is that it moves. A flat window never
/// reaches here -- it stays a rectangle and costs a rectangle.
#[derive(Clone, Debug)]
pub(crate) struct Mesh {
    /// A triangle list, three vertices per triangle.
    vertices: Vec<Corner>,
}

/// Cut a rectangle into a mesh and project it, about its own centre.
///
/// The deform moves points around inside the window's own space; the matrix
/// then places that in 3D. Both are optional and they compose, which is what
/// lets a genie happen to a window that is also tilted.
///
/// The deform arrives with its anchor already resolved to a rectangle — see
/// `present::Anchor` — because the thing it is aimed at moves, and the frame
/// being drawn is the only moment its position is known.
///
/// Returns `None` when any vertex lands at or behind the viewer: a shape with
/// one vertex projected from behind is not that shape any more, and drawing it
/// anyway folds the texture across the screen.
pub(crate) fn mesh(
    rect: Rectangle<f64, smithay::utils::Logical>,
    matrix: Mat4,
    deform: Option<crate::present::Aimed>,
    scale: f64,
) -> Option<Mesh> {
    // The two ends of the morph, in the plain numbers the effects crate takes.
    let from = crate::present::for_effects(rect);
    let morph = deform.map(|deform| (deform.effect, crate::present::for_effects(deform.to)));
    // One cell unless a deform asks for more: a matrix alone is exact at the
    // corners, because a projective map takes straight edges to straight
    // edges and the per-vertex `q` carries the rest.
    let (columns, rows) = morph.map_or((1, 1), |(effect, _)| effect.segments());
    let (centre_x, centre_y) = (
        rect.loc.x + rect.size.w / 2.0,
        rect.loc.y + rect.size.h / 2.0,
    );
    #[expect(clippy::cast_possible_truncation, reason = "screen-sized floats")]
    let scale32 = scale as f32;

    let mut grid = Vec::with_capacity(((columns + 1) * (rows + 1)) as usize);
    for row in 0..=rows {
        let v = f64::from(row) / f64::from(rows);
        for column in 0..=columns {
            let u = f64::from(column) / f64::from(columns);
            let (x, y) = match morph {
                Some((effect, to)) => effect.place(from, to, u, v),
                None => from.at(u, v),
            };
            #[expect(clippy::cast_possible_truncation, reason = "screen-sized floats")]
            let offset = (((x - centre_x) as f32), ((y - centre_y) as f32));
            let (projected_x, projected_y, w) = matrix.project_with_w(offset.0, offset.1, 0.0)?;
            #[expect(clippy::cast_possible_truncation, reason = "screen-sized floats")]
            let (origin_x, origin_y) = ((centre_x * scale) as f32, (centre_y * scale) as f32);
            #[expect(clippy::cast_possible_truncation, reason = "unit square floats")]
            grid.push(Corner {
                x: origin_x + projected_x * scale32,
                y: origin_y + projected_y * scale32,
                u: u as f32,
                v: v as f32,
                q: 1.0 / w,
            });
        }
    }

    // Two triangles per cell. Indices would save a third of the upload, but
    // this is a few thousand floats a frame at most and a flat list is one
    // less thing to get wrong.
    let at = |column: u32, row: u32| grid.get((row * (columns + 1) + column) as usize).copied();
    let mut vertices = Vec::with_capacity((columns * rows * 6) as usize);
    for row in 0..rows {
        for column in 0..columns {
            let top_left = at(column, row)?;
            let top_right = at(column + 1, row)?;
            let bottom_right = at(column + 1, row + 1)?;
            let bottom_left = at(column, row + 1)?;
            vertices.extend_from_slice(&[
                top_left,
                top_right,
                bottom_right,
                top_left,
                bottom_right,
                bottom_left,
            ]);
        }
    }
    Some(Mesh { vertices })
}

impl Element for Warp {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, BufferCoords> {
        Rectangle::from_size(self.texture.size().to_f64())
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.bounds
    }

    /// None. A warped texture's edges are antialiased against whatever is
    /// behind them, and claiming any of it opaque would leave the background
    /// undrawn under a rotated edge.
    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::default()
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for Warp {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        _src: Rectangle<f64, BufferCoords>,
        _dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        if damage.is_empty() {
            return Ok(());
        }
        let projection = *frame.projection();
        let texture = self.texture.tex_id();
        let mesh = &self.mesh;
        let alpha = self.alpha;

        frame.with_context(|gl| {
            PROGRAM.with_borrow_mut(|slot| {
                let program = match slot {
                    Some(program) => program,
                    None => {
                        // SAFETY: a GL context is current for the duration of
                        // `with_context`, which is the whole contract of it.
                        match unsafe { Program::compile(gl) } {
                            Some(program) => slot.insert(program),
                            None => return,
                        }
                    }
                };
                // SAFETY: as above; every name used was created by `compile`
                // against this same context.
                unsafe { program.draw(gl, &projection, texture, mesh, alpha) }
            });
        })
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        // Never a scanout candidate: the point of this element is that it is
        // not a rectangle, and a plane can only show a rectangle.
        None
    }
}

thread_local! {
    /// The compiled program, kept for the life of the context.
    ///
    /// Thread-local rather than global because a GL context belongs to the
    /// thread that made it current, and this is only ever reached from inside
    /// `with_context` on the render thread.
    static PROGRAM: RefCell<Option<Program>> = const { RefCell::new(None) };
}

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

#[derive(Debug)]
struct Program {
    id: ffi::types::GLuint,
    projection: ffi::types::GLint,
    tex: ffi::types::GLint,
    alpha: ffi::types::GLint,
    position: ffi::types::GLuint,
    uvq: ffi::types::GLuint,
    buffer: ffi::types::GLuint,
}

impl Program {
    unsafe fn compile(gl: &ffi::Gles2) -> Option<Self> {
        unsafe {
            let vertex = compile_stage(gl, ffi::VERTEX_SHADER, VERTEX)?;
            let fragment = compile_stage(gl, ffi::FRAGMENT_SHADER, FRAGMENT)?;
            let id = gl.CreateProgram();
            gl.AttachShader(id, vertex);
            gl.AttachShader(id, fragment);
            gl.LinkProgram(id);
            gl.DeleteShader(vertex);
            gl.DeleteShader(fragment);

            let mut linked = 0;
            gl.GetProgramiv(id, ffi::LINK_STATUS, &raw mut linked);
            if linked == 0 {
                tracing::error!("the warp program did not link");
                gl.DeleteProgram(id);
                return None;
            }

            let mut buffer = 0;
            gl.GenBuffers(1, &raw mut buffer);

            Some(Self {
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
            .inspect(|program| {
                tracing::debug!(
                    position = program.position,
                    uvq = program.uvq,
                    projection = program.projection,
                    tex = program.tex,
                    alpha = program.alpha,
                    "warp program linked"
                );
            })
        }
    }

    unsafe fn draw(
        &self,
        gl: &ffi::Gles2,
        projection: &[f32; 9],
        texture: ffi::types::GLuint,
        mesh: &Mesh,
        alpha: f32,
    ) {
        let mut vertices = Vec::with_capacity(mesh.vertices.len() * 5);
        for corner in &mesh.vertices {
            vertices.extend_from_slice(&[
                corner.x,
                corner.y,
                corner.u * corner.q,
                corner.v * corner.q,
                corner.q,
            ]);
        }

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
                isize::try_from(std::mem::size_of_val(vertices.as_slice())).unwrap_or_default(),
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
            let count = mesh.vertices.len() as i32;
            gl.DrawArrays(ffi::TRIANGLES, 0, count);

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

unsafe fn compile_stage(
    gl: &ffi::Gles2,
    kind: ffi::types::GLenum,
    source: &str,
) -> Option<ffi::types::GLuint> {
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
            tracing::error!(kind, "a warp shader did not compile");
            gl.DeleteShader(shader);
            return None;
        }
        Some(shader)
    }
}

/// Unused, but part of the element contract for a texture-backed element.
#[expect(dead_code, reason = "kept beside src() for whoever needs the size")]
pub(crate) fn texture_size(texture: &GlesTexture) -> Size<i32, BufferCoords> {
    let _ = Transform::Normal;
    texture.size()
}

/// Point GL back at the window system's framebuffer.
///
/// An offscreen pass binds a framebuffer object of its own. On a backend that
/// renders into an EGL surface rather than an FBO -- winit is one -- nothing
/// ever binds FBO 0 again, so that object stays current and every later frame
/// is drawn into a texture nobody shows. The compositor looks frozen while
/// happily reporting successful frames. So whoever binds an FBO puts this
/// back.
pub(crate) fn release_framebuffer(renderer: &mut GlesRenderer) {
    // SAFETY: `with_context` makes the renderer's context current for the
    // call, and 0 is always a valid framebuffer name.
    let restored = unsafe { renderer.with_context(|gl| gl.BindFramebuffer(ffi::FRAMEBUFFER, 0)) };
    if let Err(err) = restored {
        tracing::warn!(?err, "could not release the offscreen framebuffer");
    }
}
