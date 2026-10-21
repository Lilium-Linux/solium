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
    pub(crate) size: [f32; 2],
    /// The subject inside the padded capture, in `uv`.
    pub(crate) content: [f32; 4],
    /// The part of the pane's unit square the capture is (u0, v0, u1, v1).
    pub(crate) part: [f32; 4],
    /// The effect's numeric params in their sorted order, at most
    /// `effects.limits.params` of them (Task 28's `Params`; amended
    /// 2026-10-08, configurable).
    pub(crate) params: super::params::Params,
}

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
            })
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
