#![expect(
    unsafe_code,
    reason = "effect programs in raw GL: three source strings and the driver's log, neither of which smithay's program API gives"
)]
#![expect(
    dead_code,
    reason = "Task 21's runner draws through the executor, which reads a program's uniforms; wirecheck reads part"
)]

//! Effect programs in raw GL (\[16\] decision 8's "rewrite its pipeline in raw
//! GL", for the whole engine; Ruling 2).
//!
//! The fragment shader is three source strings (prelude, the user's file,
//! epilogue), so the driver's log names the user's own line, and a failure
//! carries that log, which smithay's `compile_custom_texture_shader` drops
//! (`gles/error.rs:12`). Smithay and std only, so `dev/wirecheck` includes
//! this file and compiles through it (cases 12a and 12b).

use smithay::backend::renderer::gles::ffi;

/// A linked effect program, and what it really has.
#[derive(Clone, Debug)]
pub(crate) struct Program {
    pub(crate) id: ffi::types::GLuint,
    /// The full-target quad's buffer, made once with the program.
    pub(crate) buffer: ffi::types::GLuint,
    /// Every active uniform: name, location, GL type (`glGetActiveUniform`;
    /// wirecheck case 12b).
    uniforms: Vec<(String, ffi::types::GLint, ffi::types::GLenum)>,
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
            let uniforms = active_uniforms(gl, id);
            let mut buffer = 0;
            gl.GenBuffers(1, &raw mut buffer);
            Ok(Self {
                id,
                buffer,
                uniforms,
            })
        }
    }

    /// Every active uniform, as the driver reports it: wirecheck case 12b.
    pub(crate) fn uniforms(&self) -> &[(String, i32, u32)] {
        &self.uniforms
    }

    pub(crate) fn location(&self, name: &str) -> Option<i32> {
        self.uniforms
            .iter()
            .find(|(each, _, _)| each == name)
            .map(|(_, location, _)| *location)
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
