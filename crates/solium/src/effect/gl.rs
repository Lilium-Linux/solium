#![expect(
    unsafe_code,
    reason = "effect programs in raw GL: three source strings and the driver's log, neither of which smithay's program API gives"
)]
#![expect(
    dead_code,
    reason = "a log's stage is read by wirecheck alone, which includes this file"
)]

//! Effect programs in raw GL (\[16\] decision 8's "rewrite its pipeline in raw
//! GL", for the whole engine; Ruling 2).
//!
//! The fragment shader is three source strings (prelude, the user's file,
//! epilogue), so the driver's log names the user's own line, and a failure
//! carries that log, which smithay's `compile_custom_texture_shader` drops
//! (`gles/error.rs:12`). Smithay and std only, so `dev/wirecheck` includes
//! this file and compiles through it (cases 12a, 12b and 12j), with
//! `super::params` and `super::warp` beside it.

use std::rc::Rc;

use smithay::backend::renderer::gles::ffi;

/// A linked effect program, and what it really has. Cheap to clone, so a
/// warp element can carry the one a folder's frag compiled into (Task 29).
#[derive(Clone, Debug)]
pub(crate) struct Program {
    pub(crate) id: ffi::types::GLuint,
    /// The full-target quad's buffer, made once with the program; a warp
    /// program's vertices, uploaded each draw.
    pub(crate) buffer: ffi::types::GLuint,
    /// Every active uniform: name, location, GL type (`glGetActiveUniform`;
    /// wirecheck case 12b).
    uniforms: Rc<[(String, ffi::types::GLint, ffi::types::GLenum)]>,
    /// The vertex attributes a warp program reads, where they are:
    /// `position` and `uvq` (`glGetAttribLocation`; wirecheck case 12j).
    attributes: [(&'static str, Option<ffi::types::GLuint>); 2],
    /// A mesh program's own 1×1 framebuffer and its renderbuffer, which it
    /// draws into (nothing, its rasteriser off) so a run needs no complete
    /// framebuffer bound around it; 0 for any other program.
    target: [ffi::types::GLuint; 2],
    /// A test program's `.vert`, which `host::tests::Refusing` runs as
    /// `host::tests::test_mesh` reads it.
    #[cfg(test)]
    pub(crate) source: Option<Rc<str>>,
}

/// What a pixel program in the warp reads, beside the capture. `Copy` and
/// compared, because it is part of the warp's `Shape` (Task 29).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PixelUniforms {
    pub(crate) progress: f32,
    pub(crate) clamped: f32,
    pub(crate) direction: f32,
    pub(crate) seed: f32,
    pub(crate) time: f32,
    /// The capture's size in pixels: `sol_size`, and `sol_texel` is 1 / it.
    /// For a present's pixels, the size the capture stands for, the pane's
    /// whole box at its monitor's scale, even while the pane is drawn small
    /// enough to be captured at a fraction of it (#307): a frag's on-screen
    /// arithmetic does not move with the resolution.
    pub(crate) size: [f32; 2],
    /// The subject inside the padded capture, in `uv`: corner and size.
    /// For a present's pixels, the pane inside a capture that holds its
    /// layers' bleed around it (#312, `render::content_in`); its popups'
    /// whole capture for theirs.
    pub(crate) content: [f32; 4],
    /// The part of the pane's unit square the capture is (u0, v0, u1, v1).
    pub(crate) part: [f32; 4],
    /// The effect's numeric params in their sorted order, at most
    /// `effects.limits.params` of them (Task 28's `Params`; amended
    /// 2026-10-08, configurable).
    pub(crate) params: super::params::Params,
}

/// What a mesh program reads besides its params: the engine's uniforms, as
/// GL takes them. Rectangles are x, y, w and h in global logical pixels;
/// `part` is u0, v0, u1 and v1; `grid` is the columns and rows; `axis` is
/// `solium_effects::mesh::axis_index`'s.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct MeshUniforms {
    pub(crate) from: [f32; 4],
    pub(crate) to: [f32; 4],
    pub(crate) monitor: [f32; 4],
    pub(crate) part: [f32; 4],
    pub(crate) grid: [f32; 2],
    pub(crate) progress: f32,
    pub(crate) clamped: f32,
    pub(crate) direction: f32,
    pub(crate) seed: f32,
    pub(crate) scale: f32,
    pub(crate) axis: i32,
}

/// The most points one mesh run places: far past any effect's grid (the
/// genie's is 441), and short of a buffer a mistyped grid could make huge.
pub(crate) const MOST_POINTS: usize = 1 << 16;

/// Why a program did not build: which stage, and the driver's own words.
#[derive(Debug)]
pub(crate) struct Log {
    pub(crate) stage: &'static str,
    pub(crate) text: String,
}

impl Program {
    /// Compile `vertex` and the three `fragment` strings, and link, with the
    /// vertex attribute `position` at location 0. Wirecheck cases 12a and 12b.
    ///
    /// # Safety
    /// A GL context is current: inside `with_context`.
    pub(crate) unsafe fn compile(
        gl: &ffi::Gles2,
        vertex: &str,
        fragment: [&str; 3],
    ) -> Result<Self, Log> {
        // SAFETY: the caller's contract.
        unsafe {
            let vs = stage(gl, ffi::VERTEX_SHADER, &[vertex]).map_err(|text| Log {
                stage: "vertex",
                text,
            })?;
            let fs = match stage(gl, ffi::FRAGMENT_SHADER, &fragment) {
                Ok(fs) => fs,
                Err(text) => {
                    gl.DeleteShader(vs);
                    return Err(Log {
                        stage: "fragment",
                        text,
                    });
                }
            };
            let id = gl.CreateProgram();
            gl.AttachShader(id, vs);
            gl.AttachShader(id, fs);
            gl.BindAttribLocation(id, 0, c"position".as_ptr().cast());
            gl.LinkProgram(id);
            gl.DeleteShader(vs);
            gl.DeleteShader(fs);
            let mut linked = 0;
            gl.GetProgramiv(id, ffi::LINK_STATUS, &raw mut linked);
            if linked == 0 {
                let text = program_log(gl, id);
                gl.DeleteProgram(id);
                return Err(Log {
                    stage: "link",
                    text,
                });
            }
            let uniforms = active_uniforms(gl, id).into();
            let attribute = |name: &std::ffi::CStr| {
                u32::try_from(gl.GetAttribLocation(id, name.as_ptr().cast())).ok()
            };
            let attributes = [
                ("position", attribute(c"position")),
                ("uvq", attribute(c"uvq")),
            ];
            let mut buffer = 0;
            gl.GenBuffers(1, &raw mut buffer);
            Ok(Self {
                id,
                buffer,
                uniforms,
                attributes,
                target: [0, 0],
                #[cfg(test)]
                source: None,
            })
        }
    }

    /// Compile a geometry's mesh program: `vertex`, its three strings (the
    /// mesh prelude, its `.vert`, the epilogue), linked with
    /// `solium_effects::mesh::MESH_FRAGMENT` and its `sol_point` captured by
    /// transform feedback; the grid point's `position` at location 0. A
    /// context without transform feedback (GLES 2) is refused as a link
    /// failure, with why.
    ///
    /// # Safety
    /// A GL context is current: inside `with_context`.
    pub(crate) unsafe fn compile_mesh(gl: &ffi::Gles2, vertex: [&str; 3]) -> Result<Self, Log> {
        if !(gl.TransformFeedbackVaryings.is_loaded()
            && gl.BeginTransformFeedback.is_loaded()
            && gl.MapBufferRange.is_loaded())
        {
            return Err(Log {
                stage: "link",
                text: "a mesh is read back through transform feedback, which this GL context does not have (it needs GLES 3)".to_owned(),
            });
        }
        // SAFETY: the caller's contract.
        unsafe {
            let vs = stage(gl, ffi::VERTEX_SHADER, &vertex).map_err(|text| Log {
                stage: "vertex",
                text,
            })?;
            let fs = match stage(
                gl,
                ffi::FRAGMENT_SHADER,
                &[solium_effects::mesh::MESH_FRAGMENT],
            ) {
                Ok(fs) => fs,
                Err(text) => {
                    gl.DeleteShader(vs);
                    return Err(Log {
                        stage: "fragment",
                        text,
                    });
                }
            };
            let id = gl.CreateProgram();
            gl.AttachShader(id, vs);
            gl.AttachShader(id, fs);
            gl.BindAttribLocation(id, 0, c"position".as_ptr().cast());
            let varyings = [c"sol_point".as_ptr()];
            gl.TransformFeedbackVaryings(id, 1, varyings.as_ptr().cast(), ffi::INTERLEAVED_ATTRIBS);
            gl.LinkProgram(id);
            gl.DeleteShader(vs);
            gl.DeleteShader(fs);
            let mut linked = 0;
            gl.GetProgramiv(id, ffi::LINK_STATUS, &raw mut linked);
            if linked == 0 {
                let text = program_log(gl, id);
                gl.DeleteProgram(id);
                return Err(Log {
                    stage: "link",
                    text,
                });
            }
            let uniforms = active_uniforms(gl, id).into();
            let position =
                u32::try_from(gl.GetAttribLocation(id, c"position".as_ptr().cast())).ok();
            let mut buffer = 0;
            gl.GenBuffers(1, &raw mut buffer);
            // A complete framebuffer of its own: a draw with no complete one
            // bound is refused, rasteriser off or not, and a compile or a
            // `--check` may have none.
            let (mut framebuffer, mut renderbuffer) = (0, 0);
            let (mut was_draw, mut was_read) = (0, 0);
            gl.GetIntegerv(ffi::DRAW_FRAMEBUFFER_BINDING, &raw mut was_draw);
            gl.GetIntegerv(ffi::READ_FRAMEBUFFER_BINDING, &raw mut was_read);
            gl.GenRenderbuffers(1, &raw mut renderbuffer);
            gl.BindRenderbuffer(ffi::RENDERBUFFER, renderbuffer);
            gl.RenderbufferStorage(ffi::RENDERBUFFER, ffi::RGBA8, 1, 1);
            gl.BindRenderbuffer(ffi::RENDERBUFFER, 0);
            gl.GenFramebuffers(1, &raw mut framebuffer);
            gl.BindFramebuffer(ffi::FRAMEBUFFER, framebuffer);
            gl.FramebufferRenderbuffer(
                ffi::FRAMEBUFFER,
                ffi::COLOR_ATTACHMENT0,
                ffi::RENDERBUFFER,
                renderbuffer,
            );
            gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, u32::try_from(was_draw).unwrap_or(0));
            gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, u32::try_from(was_read).unwrap_or(0));
            Ok(Self {
                id,
                buffer,
                uniforms,
                attributes: [("position", position), ("uvq", None)],
                target: [framebuffer, renderbuffer],
                #[cfg(test)]
                source: None,
            })
        }
    }

    /// Run this mesh program over a grid of `cols` by `rows` cells: the
    /// engine's `uniforms`, the params `values` by `names` (as
    /// [`Self::draw_warp`] sets them), one point for each of the grid's
    /// `(cols + 1) × (rows + 1)` corners, row by row from the top-left, with
    /// nothing drawn; and read back, x and y for each. What smithay expects
    /// is put back.
    ///
    /// # Safety
    /// The context the program was made in is current.
    pub(crate) unsafe fn run_mesh(
        &self,
        gl: &ffi::Gles2,
        uniforms: &MeshUniforms,
        (values, names): (&[f32], &[String]),
        (cols, rows): (u32, u32),
    ) -> Result<Vec<f32>, String> {
        let Some(position) = self.attribute("position") else {
            return Err("the mesh program reads no grid point".to_owned());
        };
        let points = (cols as usize + 1).saturating_mul(rows as usize + 1);
        if points > MOST_POINTS {
            return Err(format!(
                "its grid has {points} points, more than the {MOST_POINTS} a mesh may place"
            ));
        }
        let mut cells = Vec::with_capacity(points * 2);
        for r in 0..=rows {
            for c in 0..=cols {
                #[expect(clippy::cast_precision_loss, reason = "a grid of at most 2^16 points")]
                cells.extend([c as f32, r as f32]);
            }
        }
        let bytes = isize::try_from(points * 2 * std::mem::size_of::<f32>()).unwrap_or(0);
        let count = i32::try_from(points).unwrap_or(0);
        // SAFETY: the caller's contract; every name is made and given back
        // here, but the program's own.
        unsafe {
            gl.UseProgram(self.id);
            let at = |name: &str| self.location(name);
            let vec4 = |name: &str, v: [f32; 4]| {
                if let Some(location) = at(name) {
                    gl.Uniform4f(location, v[0], v[1], v[2], v[3]);
                }
            };
            let float = |name: &str, v: f32| {
                if let Some(location) = at(name) {
                    gl.Uniform1f(location, v);
                }
            };
            vec4("sol_from", uniforms.from);
            vec4("sol_to", uniforms.to);
            vec4("sol_monitor", uniforms.monitor);
            vec4("sol_part", uniforms.part);
            if let Some(location) = at("sol_grid") {
                gl.Uniform2f(location, uniforms.grid[0], uniforms.grid[1]);
            }
            float("sol_progress", uniforms.progress);
            float("sol_clamped", uniforms.clamped);
            float("sol_direction", uniforms.direction);
            float("sol_seed", uniforms.seed);
            float("sol_scale", uniforms.scale);
            if let Some(location) = at("sol_axis") {
                gl.Uniform1i(location, uniforms.axis);
            }
            set_params(gl, self, values, names);

            gl.BindBuffer(ffi::ARRAY_BUFFER, self.buffer);
            gl.BufferData(
                ffi::ARRAY_BUFFER,
                bytes,
                cells.as_ptr().cast(),
                ffi::STREAM_DRAW,
            );
            gl.EnableVertexAttribArray(position);
            gl.VertexAttribPointer(position, 2, ffi::FLOAT, ffi::FALSE, 0, std::ptr::null());
            gl.VertexAttribDivisor(position, 0);

            // Whatever error was flagged before is not this run's.
            for _ in 0..16 {
                if gl.GetError() == ffi::NO_ERROR {
                    break;
                }
            }
            let (mut was_draw, mut was_read) = (0, 0);
            gl.GetIntegerv(ffi::DRAW_FRAMEBUFFER_BINDING, &raw mut was_draw);
            gl.GetIntegerv(ffi::READ_FRAMEBUFFER_BINDING, &raw mut was_read);
            gl.BindFramebuffer(ffi::FRAMEBUFFER, self.target[0]);
            let mut feedback = 0;
            gl.GenBuffers(1, &raw mut feedback);
            gl.BindBuffer(ffi::TRANSFORM_FEEDBACK_BUFFER, feedback);
            gl.BufferData(
                ffi::TRANSFORM_FEEDBACK_BUFFER,
                bytes,
                std::ptr::null(),
                ffi::STREAM_READ,
            );
            gl.BindBufferBase(ffi::TRANSFORM_FEEDBACK_BUFFER, 0, feedback);
            gl.Enable(ffi::RASTERIZER_DISCARD);
            gl.BeginTransformFeedback(ffi::POINTS);
            gl.DrawArrays(ffi::POINTS, 0, count);
            gl.EndTransformFeedback();
            gl.Disable(ffi::RASTERIZER_DISCARD);
            let error = gl.GetError();
            let mapped = if error == ffi::NO_ERROR {
                gl.MapBufferRange(ffi::TRANSFORM_FEEDBACK_BUFFER, 0, bytes, ffi::MAP_READ_BIT)
            } else {
                std::ptr::null_mut()
            };
            let read = if error != ffi::NO_ERROR {
                Err(format!(
                    "the GPU would not run the mesh (GL error {error:#06x})"
                ))
            } else if mapped.is_null() {
                Err("the GPU gave its mesh's points no buffer to read".to_owned())
            } else {
                let floats: &[f32] = std::slice::from_raw_parts(mapped.cast::<f32>(), points * 2);
                let read = floats.to_vec();
                gl.UnmapBuffer(ffi::TRANSFORM_FEEDBACK_BUFFER);
                Ok(read)
            };
            gl.BindBufferBase(ffi::TRANSFORM_FEEDBACK_BUFFER, 0, 0);
            gl.BindBuffer(ffi::TRANSFORM_FEEDBACK_BUFFER, 0);
            gl.DeleteBuffers(1, &raw const feedback);
            gl.DisableVertexAttribArray(position);
            gl.BindBuffer(ffi::ARRAY_BUFFER, 0);
            gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, u32::try_from(was_draw).unwrap_or(0));
            gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, u32::try_from(was_read).unwrap_or(0));
            gl.UseProgram(0);
            read
        }
    }

    /// A program with no GL name, no buffer and no uniforms, which no test
    /// ever draws: what `host::tests::Refusing` answers for a source it does
    /// not refuse, so a `Solium`'s own host compiles with no GPU.
    /// `state::tests::real_client::every_failure_leaves_the_part_drawn`.
    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self {
            id: 0,
            buffer: 0,
            uniforms: Rc::from([]),
            attributes: [("position", None), ("uvq", None)],
            target: [0, 0],
            source: None,
        }
    }

    /// Every active uniform, as the driver reports it: wirecheck case 12b.
    pub(crate) fn uniforms(&self) -> &[(String, i32, u32)] {
        &self.uniforms
    }

    pub(crate) fn location(&self, name: &str) -> Option<i32> {
        self.uniform(name).map(|(location, _)| location)
    }

    /// An active uniform's location and GL type.
    fn uniform(&self, name: &str) -> Option<(i32, u32)> {
        self.uniforms
            .iter()
            .find(|(each, _, _)| each == name)
            .map(|(_, location, kind)| (*location, *kind))
    }

    /// Whether the program reads `name`: a uniform the driver did not
    /// optimise away.
    pub(crate) fn reads(&self, name: &str) -> bool {
        self.uniform(name).is_some()
    }

    /// Where the vertex attribute `name` is, if the program has it: a warp
    /// program's `position` and `uvq` (wirecheck case 12j).
    pub(crate) fn attribute(&self, name: &str) -> Option<u32> {
        self.attributes
            .iter()
            .find(|(each, _)| *each == name)
            .and_then(|(_, location)| *location)
    }

    /// Draw `vertices` (x, y, u·q, v·q, q per vertex, physical pixels)
    /// through `projection` as the warp draws them, with this program, a
    /// folder's frag compiled for the warp (`glsl::WARP_VERTEX`, the prelude
    /// for `Host::Warp`): `textures[0]` is the capture, `sol_tex`, on unit 0,
    /// each further one on the next unit; `uniforms` and `alpha` set; once,
    /// or once under each of `scissors`, then what smithay expects put back.
    /// `params` names `uniforms.params`' slots in order (`p_<name>`, a
    /// `vec4` four slots of one name), each set as the program declares it:
    /// `Uniform1f`, `Uniform1i`, or `Uniform4f` from four slots. A uniform
    /// the program does not have (optimised away) is skipped, and so is
    /// `uvq` for a frag that reads no `uv`. Wirecheck case 12j.
    ///
    /// # Safety
    /// A GL context is current, and every name is this context's.
    #[expect(
        clippy::too_many_arguments,
        reason = "the warp's draw, and what a pixel program reads"
    )]
    pub(crate) unsafe fn draw_warp(
        &self,
        gl: &ffi::Gles2,
        projection: &[f32; 9],
        textures: &[u32],
        vertices: &[f32],
        alpha: f32,
        uniforms: &PixelUniforms,
        params: &[String],
        scissors: &[[i32; 4]],
    ) {
        // `uvq` is dropped by the driver from a frag that reads no `uv`.
        let Some(position) = self.attribute("position") else {
            return;
        };
        let uvq = self.attribute("uvq");
        // SAFETY: the caller's contract.
        unsafe {
            gl.UseProgram(self.id);
            let at = |name: &str| self.location(name);
            if let Some(location) = at("projection") {
                gl.UniformMatrix3fv(location, 1, ffi::FALSE, projection.as_ptr());
            }
            let float = |name: &str, v: f32| {
                if let Some(location) = at(name) {
                    gl.Uniform1f(location, v);
                }
            };
            let vec2 = |name: &str, v: [f32; 2]| {
                if let Some(location) = at(name) {
                    gl.Uniform2f(location, v[0], v[1]);
                }
            };
            let vec4 = |name: &str, v: [f32; 4]| {
                if let Some(location) = at(name) {
                    gl.Uniform4f(location, v[0], v[1], v[2], v[3]);
                }
            };
            float("alpha", alpha);
            for (unit, texture) in textures.iter().enumerate() {
                super::warp::bind_texture(gl, u32::try_from(unit).unwrap_or(0), *texture);
            }
            gl.ActiveTexture(ffi::TEXTURE0);
            if let Some(location) = at("sol_tex_sampler") {
                gl.Uniform1i(location, 0);
            }
            vec4("sol_tex_box", [0.0, 0.0, 1.0, 1.0]);
            vec4("sol_tex_clamp", [0.0, 0.0, 1.0, 1.0]);
            let [w, h] = uniforms.size;
            vec2("sol_texel", [1.0 / w.max(1.0), 1.0 / h.max(1.0)]);
            vec2("sol_size", uniforms.size);
            vec2("sol_box_px", uniforms.size);
            vec4("sol_content", uniforms.content);
            vec4("sol_part", uniforms.part);
            float("sol_progress", uniforms.progress);
            float("sol_clamped", uniforms.clamped);
            float("sol_direction", uniforms.direction);
            float("sol_seed", uniforms.seed);
            float("sol_time", uniforms.time);
            set_params(gl, self, uniforms.params.as_slice(), params);

            gl.Enable(ffi::BLEND);
            gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);
            super::warp::triangles(gl, (self.buffer, position, uvq), vertices, scissors);
            for unit in (0..textures.len()).rev() {
                gl.ActiveTexture(ffi::TEXTURE0 + u32::try_from(unit).unwrap_or(0));
                gl.BindTexture(ffi::TEXTURE_2D, 0);
            }
            gl.UseProgram(0);
        }
    }

    /// # Safety
    /// The context the program was made in is current.
    pub(crate) unsafe fn delete(&self, gl: &ffi::Gles2) {
        // SAFETY: the caller's contract; both names are this context's.
        unsafe {
            gl.DeleteBuffers(1, &raw const self.buffer);
            if self.target[0] != 0 {
                gl.DeleteFramebuffers(1, &raw const self.target[0]);
                gl.DeleteRenderbuffers(1, &raw const self.target[1]);
            }
            gl.DeleteProgram(self.id);
        }
    }
}

/// One stage from `strings`, or the driver's log.
unsafe fn stage(
    gl: &ffi::Gles2,
    kind: ffi::types::GLenum,
    strings: &[&str],
) -> Result<ffi::types::GLuint, String> {
    // SAFETY: the caller's contract; the pointers and lengths live for the call.
    unsafe {
        let shader = gl.CreateShader(kind);
        let pointers: Vec<*const ffi::types::GLchar> =
            strings.iter().map(|each| each.as_ptr().cast()).collect();
        let lengths: Vec<ffi::types::GLint> = strings
            .iter()
            .map(|each| ffi::types::GLint::try_from(each.len()).unwrap_or(ffi::types::GLint::MAX))
            .collect();
        let count = ffi::types::GLsizei::try_from(strings.len()).unwrap_or(0);
        gl.ShaderSource(shader, count, pointers.as_ptr(), lengths.as_ptr());
        gl.CompileShader(shader);
        let mut compiled = 0;
        gl.GetShaderiv(shader, ffi::COMPILE_STATUS, &raw mut compiled);
        if compiled == 0 {
            let mut length = 0;
            gl.GetShaderiv(shader, ffi::INFO_LOG_LENGTH, &raw mut length);
            let mut text = vec![0_u8; usize::try_from(length.max(1)).unwrap_or(1)];
            gl.GetShaderInfoLog(
                shader,
                length.max(1),
                std::ptr::null_mut(),
                text.as_mut_ptr().cast(),
            );
            gl.DeleteShader(shader);
            return Err(String::from_utf8_lossy(&text)
                .trim_end_matches('\0')
                .trim()
                .to_owned());
        }
        Ok(shader)
    }
}

/// Each of `names` (`p_<name>`), the slots of `values` in order: a
/// `vec4` takes four slots of one name and `Uniform4f`; an `int` (or a
/// `bool`) `Uniform1i`, rounded; anything else `Uniform1f`. A name the
/// program does not have takes its slots and sets nothing.
///
/// # Safety
/// A GL context is current, and `program` is in use in it.
unsafe fn set_params(gl: &ffi::Gles2, program: &Program, values: &[f32], names: &[String]) {
    let mut at = 0;
    while let Some(name) = names.get(at) {
        // Every slot this name has, so one optimised away is skipped whole.
        let run = names
            .get(at..)
            .unwrap_or_default()
            .iter()
            .take_while(|each| *each == name)
            .count()
            .max(1);
        let slot = |offset: usize| values.get(at + offset).copied().unwrap_or(0.0);
        // SAFETY: the caller's contract.
        unsafe {
            match program.uniform(name) {
                Some((location, ffi::FLOAT_VEC4)) => {
                    gl.Uniform4f(location, slot(0), slot(1), slot(2), slot(3));
                }
                Some((location, ffi::INT | ffi::BOOL)) => {
                    #[expect(clippy::cast_possible_truncation, reason = "an int param, rounded")]
                    gl.Uniform1i(location, slot(0).round() as i32);
                }
                Some((location, _)) => gl.Uniform1f(location, slot(0)),
                None => {}
            }
        }
        at += run;
    }
}

unsafe fn program_log(gl: &ffi::Gles2, id: ffi::types::GLuint) -> String {
    // SAFETY: the caller's contract.
    unsafe {
        let mut length = 0;
        gl.GetProgramiv(id, ffi::INFO_LOG_LENGTH, &raw mut length);
        let mut text = vec![0_u8; usize::try_from(length.max(1)).unwrap_or(1)];
        gl.GetProgramInfoLog(
            id,
            length.max(1),
            std::ptr::null_mut(),
            text.as_mut_ptr().cast(),
        );
        String::from_utf8_lossy(&text)
            .trim_end_matches('\0')
            .trim()
            .to_owned()
    }
}

unsafe fn active_uniforms(gl: &ffi::Gles2, id: ffi::types::GLuint) -> Vec<(String, i32, u32)> {
    // SAFETY: the caller's contract; `name` outlives both calls, and the
    // driver writes at most its length, a NUL included.
    unsafe {
        let mut count = 0;
        gl.GetProgramiv(id, ffi::ACTIVE_UNIFORMS, &raw mut count);
        let mut found = Vec::new();
        for index in 0..u32::try_from(count).unwrap_or(0) {
            let mut name = [0_u8; 128];
            let (mut length, mut size, mut kind) = (0, 0, 0);
            gl.GetActiveUniform(
                id,
                index,
                128,
                &raw mut length,
                &raw mut size,
                &raw mut kind,
                name.as_mut_ptr().cast(),
            );
            let written = name
                .get(..usize::try_from(length).unwrap_or(0))
                .unwrap_or_default();
            let text = String::from_utf8_lossy(written).into_owned();
            let location = gl.GetUniformLocation(id, name.as_ptr().cast());
            found.push((text, location, kind));
        }
        found
    }
}

/// Why a program reads more textures than this GPU samples at once, if it
/// does, counting its active samplers: what `GlCompiler::compile` refuses
/// (Ruling 10). A GPU samples at least 8, the most a pass may read
/// (`stage::MOST_TEXTURES`), so this guards a driver below GLES 2's minimum.
/// `tests::a_program_reading_more_textures_than_the_gpu_samples_is_refused`.
pub(crate) fn too_many_textures(uniforms: &[(String, i32, u32)], units: i32) -> Option<String> {
    let samplers = uniforms
        .iter()
        .filter(|(_, _, kind)| *kind == ffi::SAMPLER_2D)
        .count();
    (i32::try_from(samplers).unwrap_or(i32::MAX) > units).then(|| {
        format!("the pass reads {samplers} textures, and this GPU samples at most {units} at once")
    })
}

/// `GL_MAX_TEXTURE_IMAGE_UNITS`: how many textures one pass may read.
///
/// # Safety
/// A GL context is current.
pub(crate) unsafe fn max_texture_units(gl: &ffi::Gles2) -> i32 {
    // SAFETY: the caller's contract.
    unsafe {
        let mut units = 0;
        gl.GetIntegerv(ffi::MAX_TEXTURE_IMAGE_UNITS, &raw mut units);
        units
    }
}

#[cfg(test)]
mod tests {
    use super::too_many_textures;
    use smithay::backend::renderer::gles::ffi;

    /// **A program reading more textures than this GPU samples is refused**
    /// at compile (Ruling 10), counting its active samplers and nothing
    /// else; at the GLES 2 minimum of 8, a pass at the load-time limit runs.
    #[test]
    fn a_program_reading_more_textures_than_the_gpu_samples_is_refused() {
        let program = |samplers: usize| -> Vec<(String, i32, u32)> {
            let mut uniforms: Vec<(String, i32, u32)> = (0..samplers)
                .map(|at| (format!("s{at}"), 0, ffi::SAMPLER_2D))
                .collect();
            uniforms.push(("p_amount".to_owned(), 0, ffi::FLOAT));
            uniforms
        };
        assert_eq!(too_many_textures(&program(8), 8), None);
        let refused = too_many_textures(&program(9), 8).expect("refused");
        assert!(refused.contains('9') && refused.contains('8'), "{refused}");
    }
}
