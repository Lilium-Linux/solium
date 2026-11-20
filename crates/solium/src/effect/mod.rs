#![expect(unsafe_code, reason = "the calls into effect::gl")]

//! Effects as user folders: the host, the descriptions, the programs, the
//! executor, the rules and the transitions. \[16\] §2; the spec §6.
//!
//! Singular, apart from the `solium_effects` crate, which holds what is data
//! and text; this module holds GL and everything per frame (Ruling 2). No
//! effect runs code of its own on the CPU: an effect is data
//! (`effect.toml`) and shaders.

pub(crate) mod beneath;
pub(crate) mod crossfade;
pub(crate) mod described;
pub(crate) mod element;
pub(crate) mod fade;
pub(crate) mod geometry;
pub(crate) mod gl;
pub(crate) mod host;
/// Lua tables as a [`crate::script::Doc`], for the tests that write rules
/// and settings as the Lua window manager does.
#[cfg(test)]
pub(crate) mod lua_doc;
pub(crate) mod mask;
pub(crate) mod memo;
pub(crate) mod param;
pub(crate) mod params;
pub(crate) mod plan;
pub(crate) mod rules;
pub(crate) mod run;
pub(crate) mod settings;
pub(crate) mod store;
pub(crate) use param::param_value;
pub(crate) mod xray;

/// So `super::pool` resolves the same in `effect/run.rs` here and in
/// `dev/wirecheck`, which includes `pool.rs` beside it (Ruling 2).
pub(crate) use crate::pool;
/// So `super::warp`'s draw resolves the same in `effect/gl.rs` here and in
/// `dev/wirecheck`, which includes `warp/gl.rs` (Task 29).
pub(crate) use crate::warp;

use smithay::backend::renderer::gles::GlesRenderer;

/// [`host::Compiler`] on the renderer, with the context made current: what
/// `render::prepare` compiles effects through (wirecheck cases 12a and 12b
/// compile through the same [`gl::Program::compile`]).
#[derive(Debug)]
pub(crate) struct GlCompiler<'a>(pub(crate) &'a mut GlesRenderer);

impl host::Compiler for GlCompiler<'_> {
    type Program = gl::Program;

    fn compile(
        &mut self,
        vertex: &str,
        sources: &solium_effects::glsl::Sources,
    ) -> Result<gl::Program, String> {
        // SAFETY: `with_context` makes the renderer's context current.
        let compiled = self.0.with_context(|context| unsafe {
            gl::Program::compile(context, vertex, sources.strings())
                .map(|program| (program, gl::max_texture_units(context)))
        });
        let (program, units) = match compiled {
            Err(err) => return Err(format!("{err}")),
            Ok(Err(log)) => return Err(log.text),
            Ok(Ok(compiled)) => compiled,
        };
        // A pass the executor could not bind every texture of is refused
        // here, not failed at its first run (Ruling 10).
        // `gl::tests::a_program_reading_more_textures_than_the_gpu_samples_is_refused`.
        match gl::too_many_textures(program.uniforms(), units) {
            Some(refused) => {
                self.delete(program);
                Err(refused)
            }
            None => Ok(program),
        }
    }

    fn compile_mesh(
        &mut self,
        sources: &solium_effects::glsl::Sources,
    ) -> Result<gl::Program, String> {
        // SAFETY: `with_context` makes the renderer's context current.
        let compiled = self.0.with_context(|context| unsafe {
            gl::Program::compile_mesh(context, sources.strings())
        });
        match compiled {
            Err(err) => Err(format!("{err}")),
            Ok(Err(log)) => Err(log.text),
            Ok(Ok(program)) => Ok(program),
        }
    }

    fn run_mesh(
        &mut self,
        program: &gl::Program,
        uniforms: &gl::MeshUniforms,
        params: (&[f32], &[String]),
        grid: (u32, u32),
    ) -> Result<Vec<f32>, String> {
        // SAFETY: `with_context` makes the renderer's context current, the
        // one the program was made in.
        self.0
            .with_context(|context| unsafe { program.run_mesh(context, uniforms, params, grid) })
            .map_err(|err| format!("{err}"))?
    }

    fn delete(&mut self, program: gl::Program) {
        // SAFETY: the program was made in this renderer's context.
        let _ = self
            .0
            .with_context(|context| unsafe { program.delete(context) });
    }

    /// The probe's log, read: NVIDIA numbers the user's file from 0 after
    /// `#line 0 1`, where GLSL ES 1.00 says 1 (wirecheck case 12a).
    fn line_shift(&mut self) -> u32 {
        let probe = solium_effects::glsl::line_probe();
        // SAFETY: `with_context` makes the renderer's context current, and
        // the probe, if it links, is deleted in it.
        let log = self.0.with_context(|context| unsafe {
            match gl::Program::compile(context, solium_effects::glsl::PASS_VERTEX, probe.strings())
            {
                Ok(program) => {
                    program.delete(context);
                    None
                }
                Err(log) => Some(log.text),
            }
        });
        log.ok()
            .flatten()
            .map_or(0, |log| solium_effects::glsl::line_shift(&log))
    }
}
