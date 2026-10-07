//! FX2's GPU cases: what an effect program does on this driver, which no unit
//! test can reach. Run alone with `WIRECHECK_ONLY=fx2`, and after FX0's cases
//! in a full run. Each prints a `=== FX2:` heading.

use anyhow::{Result, anyhow};
use smithay::backend::renderer::gles::{GlesRenderer, ffi};
use solium_effects::glsl::{self, Host, ParamKind, Signature};

#[path = "../../../crates/solium/src/effect/gl.rs"]
#[allow(
    dead_code,
    reason = "the compositor's effect programs, of which these cases need part"
)]
mod gl;

/// The compositor's pool, included once, by FX0's cases: case 12c probes
/// its formats.
use crate::fx0::pool;

/// Every FX2 case, in order.
pub(crate) fn all(renderer: &mut GlesRenderer) -> Result<()> {
    a_typo_is_reported_at_its_own_line(renderer)?;
    introspection_agrees_with_the_signature(renderer)?;
    rgba16f_is_renderable_or_reported(renderer)?;
    Ok(())
}

fn signature() -> Signature {
    Signature {
        host: Host::Pass,
        params: vec![
            ("offset".to_owned(), ParamKind::Float),
            ("passes".to_owned(), ParamKind::Int),
            ("tint".to_owned(), ParamKind::Vec4),
        ],
        uses: vec!["sharp".to_owned()],
        known: vec!["sharp".to_owned(), "soft".to_owned()],
    }
}

fn compile(
    renderer: &mut GlesRenderer,
    user: &str,
) -> Result<std::result::Result<gl::Program, gl::Log>> {
    compile_sources(renderer, &glsl::assemble(&signature(), user))
}

fn compile_sources(
    renderer: &mut GlesRenderer,
    sources: &glsl::Sources,
) -> Result<std::result::Result<gl::Program, gl::Log>> {
    // SAFETY: `with_context` makes the context current for the compile.
    renderer
        .with_context(|context| unsafe {
            gl::Program::compile(context, glsl::PASS_VERTEX, sources.strings())
        })
        .map_err(|err| anyhow!("{err}"))
}

/// **Case 12a: a `.frag` with a typo is reported at its own line, on this
/// driver.** Line 3 of the user's string reads an undeclared name; the
/// driver's log, through `glsl::compile_log` and the driver's `#line` rule
/// read from its log of `glsl::line_probe` (as `GlCompiler::line_shift` reads
/// it), must say string 1, line 3. The case prints which rule the driver
/// follows: GLSL ES 1.00's, where the line after `#line 0 1` is 1, or GLSL ES
/// 3.00's, where it is 0 (NVIDIA's).
fn a_typo_is_reported_at_its_own_line(renderer: &mut GlesRenderer) -> Result<()> {
    println!("\n=== FX2: a .frag's error is reported at its own line ===");
    let probe = match compile_sources(renderer, &glsl::line_probe())? {
        Ok(_) => return Err(anyhow!("the line probe, reading an undeclared name, compiled")),
        Err(log) => log,
    };
    let shift = glsl::line_shift(&probe.text);
    let user =
        "vec4 sol_effect(vec2 uv) {\n    vec4 c = sol_tex(uv);\n    return c * not_declared_here;\n}\n";
    let log = match compile(renderer, user)? {
        Ok(_) => return Err(anyhow!("a .frag reading an undeclared name compiled")),
        Err(log) => log,
    };
    let found = glsl::compile_log(&log.text);
    if !found
        .iter()
        .any(|each| each.string == Some(1) && each.line.map(|line| line + shift) == Some(3))
    {
        return Err(anyhow!(
            "the log did not map to string 1, line 3 (line rule shift {shift}, probe log {:?}): {:?}\n{}",
            probe.text,
            found,
            log.text
        ));
    }
    println!(
        "  {}: string 1, line 3, by GLSL ES {}'s #line rule",
        log.text.lines().next().unwrap_or(""),
        if shift == 1 { "3.00" } else { "1.00" }
    );
    Ok(())
}

/// **Case 12b: the prelude compiles, and introspection agrees with the
/// signature**: every param the `.frag` reads is active with the type the
/// prelude declared it as.
fn introspection_agrees_with_the_signature(renderer: &mut GlesRenderer) -> Result<()> {
    println!("\n=== FX2: the prelude compiles, and its uniforms are what it declared ===");
    let user = "vec4 sol_effect(vec2 uv) {\n    return sol_sharp(uv) * p_offset * float(p_passes) + p_tint * sol_shape(uv) + sol_tex(uv);\n}\n";
    let program = compile(renderer, user)?
        .map_err(|log| anyhow!("the prelude did not compile ({}): {}", log.stage, log.text))?;
    for (name, kind) in [
        ("p_offset", ffi::FLOAT),
        ("p_passes", ffi::INT),
        ("p_tint", ffi::FLOAT_VEC4),
        ("sol_sharp_sampler", ffi::SAMPLER_2D),
        ("sol_tex_sampler", ffi::SAMPLER_2D),
    ] {
        let found = program.uniforms().iter().find(|(each, _, _)| each == name);
        if found.map(|(_, _, ty)| *ty) != Some(kind) {
            return Err(anyhow!("{name} is {found:?}, not GL type {kind:#x}"));
        }
    }
    // SAFETY: the context is current inside `with_context`.
    renderer
        .with_context(|context| unsafe { program.delete(context) })
        .map_err(|err| anyhow!("{err}"))?;
    println!("  float, int, vec4 and two samplers, as declared");
    Ok(())
}

/// **Case 12c: an `rgba16f` target is renderable here, or reported
/// unsupported**, and the probe agrees with a real draw: 0.5 cleared into a
/// 1×1 half-float target through the pool's own framebuffer reads back as
/// 0.5, which an 8-bit target could not hold exactly.
fn rgba16f_is_renderable_or_reported(renderer: &mut GlesRenderer) -> Result<()> {
    use smithay::backend::renderer::{Bind as _, Color32F, Frame as _};
    use smithay::utils::Rectangle;
    println!("\n=== FX2: rgba16f renders here, or is reported missing ===");
    let mut pool = pool::Pool::new(1 << 20);
    let formats = pool::formats(renderer, &mut pool);
    pool.sweep(renderer);
    if !formats.rgba16f {
        println!("  rgba16f: not renderable on this GPU; effects start at a fallback (Ruling 11)");
        return Ok(());
    }
    let target = pool
        .target(&mut pool::Gl(renderer), (1, 1).into(), pool::Format::Rgba16f)
        .ok_or_else(|| anyhow!("the probe said yes and no target came"))?;
    let mut carrier = pool.carrier(renderer).ok_or_else(|| anyhow!("no carrier"))?;
    {
        let mut bound = renderer.bind(&mut carrier).map_err(|err| anyhow!("{err}"))?;
        let mut frame =
            pool::frame_for(renderer, &mut bound, &target).map_err(|err| anyhow!("{err}"))?;
        frame
            .clear(Color32F::new(0.5, 0.5, 0.5, 0.5), &[Rectangle::from_size((1, 1).into())])
            .map_err(|err| anyhow!("{err}"))?;
        frame
            .finish()
            .map_err(|err| anyhow!("{err}"))?
            .wait()
            .map_err(|err| anyhow!("{err:?}"))?;
    }
    let read = read_float(renderer, target.fbo())?;
    drop(target);
    pool.sweep(renderer);
    if read.iter().any(|channel| (channel - 0.5).abs() > 1e-3) {
        return Err(anyhow!("an rgba16f target cleared to 0.5 read back {read:?}"));
    }
    println!("  rgba16f: renderable, 0.5 reads back as {}", read[0]);
    Ok(())
}

/// One pixel of a float colour buffer, read as `RGBA`/`FLOAT`, which GLES 3
/// accepts for any floating-point framebuffer, through the target's own
/// framebuffer object.
fn read_float(renderer: &mut GlesRenderer, fbo: u32) -> Result<[f32; 4]> {
    let mut pixel = [0.0_f32; 4];
    // SAFETY: `with_context` makes the context current; the framebuffer was
    // made in it, and `pixel` holds the four floats one pixel reads as.
    let error = renderer
        .with_context(|gl| unsafe {
            gl.BindFramebuffer(ffi::FRAMEBUFFER, fbo);
            gl.ReadPixels(0, 0, 1, 1, ffi::RGBA, ffi::FLOAT, pixel.as_mut_ptr().cast());
            gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
            gl.GetError()
        })
        .map_err(|err| anyhow!("{err}"))?;
    if error != ffi::NO_ERROR {
        return Err(anyhow!("reading the rgba16f target back failed: GL error {error:#x}"));
    }
    Ok(pixel)
}
