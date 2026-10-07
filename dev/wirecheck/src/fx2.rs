//! FX2's GPU cases: what an effect program does on this driver, which no unit
//! test can reach. Run alone with `WIRECHECK_ONLY=fx2`, and after FX0's cases
//! in a full run. Each prints a `=== FX2:` heading.

use std::collections::HashMap;

use anyhow::{Result, anyhow};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture, ffi};
use smithay::backend::renderer::{Bind as _, ExportMem as _, ImportMem as _, Texture as _};
use smithay::utils::Rectangle;
use solium_effects::glsl::{self, Host, ParamKind, Signature};
use solium_effects::spec::Value;
use solium_effects::stage::{self, Binding, Depends, Format, Plan, Stage};

#[path = "../../../crates/solium/src/effect/gl.rs"]
#[allow(
    dead_code,
    reason = "the compositor's effect programs, of which these cases need part"
)]
mod gl;

/// The compositor's executor, with `gl` and `pool` beside it as `super::`
/// (Ruling 2): cases 12d to 12g and 12q run plans through it.
#[path = "../../../crates/solium/src/effect/run.rs"]
#[allow(
    dead_code,
    unfulfilled_lint_expectations,
    reason = "the compositor's executor, of which these cases read what the compositor does not"
)]
mod run;

/// The compositor's effect element (Smithay and `solium_effects` only): case
/// 12h draws it.
#[path = "../../../crates/solium/src/effect/element.rs"]
#[allow(
    dead_code,
    reason = "the compositor's effect element, of which case 12h draws the element"
)]
mod element;

/// The compositor's pool, included once, by FX0's cases: case 12c probes
/// its formats, and the executor draws into its targets.
use crate::fx0::pool;

/// Every FX2 case, in order.
pub(crate) fn all(renderer: &mut GlesRenderer) -> Result<()> {
    a_typo_is_reported_at_its_own_line(renderer)?;
    introspection_agrees_with_the_signature(renderer)?;
    rgba16f_is_renderable_or_reported(renderer)?;
    an_identity_pass_returns_its_input(renderer)?;
    a_kawase_blur_matches_the_cpu(renderer)?;
    a_pass_reading_three_textures_samples_each(renderer)?;
    smithay_draws_as_before_after_a_run(renderer)?;
    a_withheld_program_is_pending(renderer)?;
    an_instance_keeps_its_targets_between_runs(renderer)?;
    a_state_is_rebuilt_only_when_its_depends_changes(renderer)?;
    a_held_instance_dropped_gives_its_targets_back(renderer)?;
    a_chain_feeds_each_links_result_to_the_next(renderer)?;
    nothing_is_allocated_while_a_result_is_drawn(renderer)?;
    a_result_is_cut_by_its_mask(renderer)?;
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

/// A test effect folder of `crates/solium/tests/fixtures/effects/`, as its
/// `effect.lua` binds at the params given: its inputs, its stages and its
/// params, and its frags by file name.
struct Fixture {
    inputs: &'static [&'static str],
    stages: Vec<Stage>,
    params: Vec<(String, Value)>,
    frags: &'static [(&'static str, &'static str)],
}

fn pass(frag: &str, scale: f64, uses: &[&str]) -> Stage {
    Stage::Pass {
        frag: frag.to_owned(),
        scale,
        format: Format::Rgba8,
        uses: uses.iter().map(|name| (*name).to_owned()).collect(),
        input: None,
    }
}

/// `identity/`: one pass returning `sol_tex(uv)`.
fn identity() -> Fixture {
    Fixture {
        inputs: &["self"],
        stages: vec![pass("effect.frag", 1.0, &[])],
        params: Vec::new(),
        frags: &[(
            "effect.frag",
            include_str!("../../../crates/solium/tests/fixtures/effects/identity/effect.frag"),
        )],
    }
}

/// `kawase/` at `passes` and `offset`: \[16\] §3's dual Kawase, `passes`
/// down at half scale and as many up at twice.
fn kawase(passes: u32, offset: f64) -> Fixture {
    let mut stages: Vec<Stage> = (0..passes).map(|_| pass("down.frag", 0.5, &[])).collect();
    stages.extend((0..passes).map(|_| pass("up.frag", 2.0, &[])));
    Fixture {
        inputs: &["self"],
        stages,
        params: vec![
            ("offset".to_owned(), Value::Number(offset)),
            ("passes".to_owned(), Value::Int(i64::from(passes))),
        ],
        frags: &[
            (
                "down.frag",
                include_str!("../../../crates/solium/tests/fixtures/effects/kawase/down.frag"),
            ),
            (
                "up.frag",
                include_str!("../../../crates/solium/tests/fixtures/effects/kawase/up.frag"),
            ),
        ],
    }
}

/// `three/`: a state drawing green, and one pass reading `self`'s red, the
/// state's green and the analytic shape's coverage as blue.
fn three() -> Fixture {
    Fixture {
        inputs: &["self", "shape", "state:a"],
        stages: vec![
            Stage::State {
                name: "a".to_owned(),
                format: Format::Rgba8,
                scale: 1.0,
                depends: Depends::Params,
                body: vec![pass("a.frag", 1.0, &[])],
            },
            pass("three.frag", 1.0, &["a", "shape"]),
        ],
        params: Vec::new(),
        frags: &[
            (
                "a.frag",
                include_str!("../../../crates/solium/tests/fixtures/effects/three/a.frag"),
            ),
            (
                "three.frag",
                include_str!("../../../crates/solium/tests/fixtures/effects/three/three.frag"),
            ),
        ],
    }
}

/// `state-count/`: a state `n`, made again only when the params move, whose
/// pass writes `fract(sol_time)` into red, and a pass returning the state.
fn state_count() -> Fixture {
    Fixture {
        inputs: &["self"],
        stages: vec![
            Stage::State {
                name: "n".to_owned(),
                format: Format::Rgba8,
                scale: 1.0,
                depends: Depends::Params,
                body: vec![pass("count.frag", 1.0, &[])],
            },
            pass("read.frag", 1.0, &["n"]),
        ],
        params: vec![("n".to_owned(), Value::Number(1.0))],
        frags: &[
            (
                "count.frag",
                include_str!("../../../crates/solium/tests/fixtures/effects/state-count/count.frag"),
            ),
            (
                "read.frag",
                include_str!("../../../crates/solium/tests/fixtures/effects/state-count/read.frag"),
            ),
        ],
    }
}

/// `tint/` at `amount`: one pass mixing the colour toward its alpha.
fn tint(amount: f64) -> Fixture {
    Fixture {
        inputs: &["self"],
        stages: vec![pass("effect.frag", 1.0, &[])],
        params: vec![("amount".to_owned(), Value::Number(amount))],
        frags: &[(
            "effect.frag",
            include_str!("../../../crates/solium/tests/fixtures/effects/tint/effect.frag"),
        )],
    }
}

/// A fixture flattened as the host flattens it, each step's key set as the
/// host sets it, and every program compiled through `gl::Program::compile`.
fn build(renderer: &mut GlesRenderer, fixture: Fixture) -> Result<(Plan, HashMap<u64, gl::Program>)> {
    let Fixture {
        inputs,
        stages,
        params,
        frags,
    } = fixture;
    let mut resolve = |_: &str, _: &[(String, Value)]| -> std::result::Result<Binding, String> {
        Ok(Binding {
            stages: stages.clone(),
            inputs: inputs.iter().map(|name| (*name).to_owned()).collect(),
            params: params.clone(),
        })
    };
    let mut plan = stage::flatten("fixture", &[], &mut resolve).map_err(|err| anyhow!("{err}"))?;
    let mut programs = HashMap::new();
    for step in plan
        .steps
        .iter_mut()
        .chain(plan.states.iter_mut().flat_map(|state| state.steps.iter_mut()))
    {
        let text = frags
            .iter()
            .find(|(name, _)| *name == step.frag)
            .map(|(_, text)| *text)
            .ok_or_else(|| anyhow!("no fixture frag {}", step.frag))?;
        let sources = glsl::assemble(&step.signature, text);
        step.key = sources.key(glsl::PASS_VERTEX);
        if !programs.contains_key(&step.key) {
            let program = compile_sources(renderer, &sources)?.map_err(|log| {
                anyhow!("{} did not compile ({}): {}", step.frag, log.stage, log.text)
            })?;
            programs.insert(step.key, program);
        }
    }
    Ok((plan, programs))
}

fn free(renderer: &mut GlesRenderer, programs: HashMap<u64, gl::Program>) -> Result<()> {
    // SAFETY: `with_context` makes the context current; every program was
    // made in it.
    renderer
        .with_context(|context| unsafe {
            for program in programs.values() {
                program.delete(context);
            }
        })
        .map_err(|err| anyhow!("{err}"))
}

/// The compiled programs as the host answers for them: a key not compiled is
/// pending.
fn lookup(programs: &HashMap<u64, gl::Program>, key: u64) -> run::Lookup<'_> {
    programs
        .get(&key)
        .map_or(run::Lookup::Pending, run::Lookup::Ready)
}

/// What a run reads when its one input fills the box: the whole box is the
/// part, square-cornered, at rest.
fn whole<'a>(
    size: (usize, usize),
    textures: &'a [(&'a str, GlesTexture, run::BoxMap)],
) -> run::Inputs<'a> {
    run::Inputs {
        padded: (
            u32::try_from(size.0).unwrap_or(1),
            u32::try_from(size.1).unwrap_or(1),
        ),
        content: [0.0, 0.0, 1.0, 1.0],
        textures,
        radii: [0.0; 4],
        time: 0.0,
        transition: run::Transition::default(),
        clamp_first: false,
    }
}

/// One run on a carrier bound for it, as `prepare` binds one, its result
/// waited for so it can be read back; its states keyed on nothing moving.
fn run_once<'p>(
    renderer: &mut GlesRenderer,
    pool: &mut pool::Pool,
    programs: &dyn Fn(u64) -> run::Lookup<'p>,
    plan: &Plan,
    held: &mut run::Held,
    inputs: &run::Inputs<'_>,
) -> Result<run::Outcome> {
    run_keyed(renderer, pool, programs, plan, held, inputs, &run::Keys::default())
}

/// [`run_once`] with the keys its states are kept on.
fn run_keyed<'p>(
    renderer: &mut GlesRenderer,
    pool: &mut pool::Pool,
    programs: &dyn Fn(u64) -> run::Lookup<'p>,
    plan: &Plan,
    held: &mut run::Held,
    inputs: &run::Inputs<'_>,
    keys: &run::Keys,
) -> Result<run::Outcome> {
    let formats = Some(pool::probe_formats(renderer));
    let mut carrier = pool.carrier(renderer).ok_or_else(|| anyhow!("no carrier"))?;
    let outcome = {
        let mut bound = renderer.bind(&mut carrier).map_err(|err| anyhow!("{err}"))?;
        run::run(renderer, &mut bound, pool, programs, formats, plan, held, inputs, keys)
    };
    if let run::Outcome::Done(_, sync) = &outcome {
        sync.wait().map_err(|err| anyhow!("{err:?}"))?;
    }
    Ok(outcome)
}

/// A picture of `size`, every pixel `pixel(x, y)` as RGBA bytes, top row
/// first, uploaded as a texture: the texture and its bytes.
fn upload(
    renderer: &mut GlesRenderer,
    size: (usize, usize),
    pixel: impl Fn(usize, usize) -> [u8; 4],
) -> Result<(GlesTexture, Vec<u8>)> {
    let mut bytes = Vec::with_capacity(size.0 * size.1 * 4);
    for y in 0..size.1 {
        for x in 0..size.0 {
            bytes.extend(pixel(x, y));
        }
    }
    let side = |n: usize| i32::try_from(n).unwrap_or(1);
    let texture = renderer
        .import_memory(&bytes, Fourcc::Abgr8888, (side(size.0), side(size.1)).into(), false)
        .map_err(|err| anyhow!("uploading: {err}"))?;
    Ok((texture, bytes))
}

/// A texture read back as RGBA bytes, its first row first.
fn read_rgba(renderer: &mut GlesRenderer, texture: &GlesTexture) -> Result<Vec<u8>> {
    let mut texture = texture.clone();
    let region = Rectangle::from_size(texture.size());
    let framebuffer = renderer
        .bind(&mut texture)
        .map_err(|err| anyhow!("binding to read: {err}"))?;
    let mapping = renderer
        .copy_framebuffer(&framebuffer, region, Fourcc::Abgr8888)
        .map_err(|err| anyhow!("copying: {err}"))?;
    drop(framebuffer);
    Ok(renderer
        .map_texture(&mapping)
        .map_err(|err| anyhow!("mapping: {err}"))?
        .to_vec())
}

/// The first pixel where two pictures of `width` differ: where, and each one's.
fn first_difference(got: &[u8], want: &[u8], width: usize) -> Option<((usize, usize), Vec<u8>, Vec<u8>)> {
    let at = got
        .chunks_exact(4)
        .zip(want.chunks_exact(4))
        .position(|(a, b)| a != b)
        .or_else(|| (got.len() != want.len()).then_some(got.len().min(want.len()) / 4))?;
    let pixel = |bytes: &[u8]| bytes.get(at * 4..at * 4 + 4).map(<[u8]>::to_vec).unwrap_or_default();
    Some(((at % width, at / width), pixel(got), pixel(want)))
}

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

/// 12d's and 12q's input: 64×32, the top-left quadrant red, the rest blue.
fn quadrant(renderer: &mut GlesRenderer) -> Result<(GlesTexture, Vec<u8>, (usize, usize))> {
    let size = (64, 32);
    let (texture, bytes) = upload(renderer, size, |x, y| {
        if x < size.0 / 2 && y < size.1 / 2 { RED } else { BLUE }
    })?;
    Ok((texture, bytes, size))
}

/// **Case 12d: an identity pass returns its input byte for byte, top row
/// first.** The `identity` fixture over a 64×32 picture whose top-left
/// quadrant is red: the result, read back, is the input, so `uv` (0, 0) is
/// the top-left of a texture that stores its top row first, in what a pass
/// reads and in what it writes (Ruling 6).
fn an_identity_pass_returns_its_input(renderer: &mut GlesRenderer) -> Result<()> {
    println!("\n=== FX2: an identity pass returns its input byte for byte, top row first ===");
    let (input, bytes, size) = quadrant(renderer)?;
    let (plan, programs) = build(renderer, identity())?;
    let mut pool = pool::Pool::new(0);
    let mut held = run::Held::default();
    let textures = [("self", input, run::BoxMap::WHOLE)];
    let outcome = run_once(
        renderer,
        &mut pool,
        &|key| lookup(&programs, key),
        &plan,
        &mut held,
        &whole(size, &textures),
    )?;
    let run::Outcome::Done(result, _) = outcome else {
        return Err(anyhow!("the identity run came to {outcome:?}"));
    };
    let read = read_rgba(renderer, &result)?;
    if let Some(((x, y), got, want)) = first_difference(&read, &bytes, size.0) {
        let red = read
            .chunks_exact(4)
            .position(|pixel| pixel == RED)
            .map(|at| (at % size.0, at / size.0));
        return Err(anyhow!(
            "the identity pass moved or changed its input: {got:?} at ({x}, {y}), not {want:?}; the first red is at {red:?}"
        ));
    }
    drop(result);
    held.release(&mut pool);
    pool.sweep(renderer);
    free(renderer, programs)?;
    println!("  64×32 through one pass: every byte where it was, the red quadrant top-left");
    Ok(())
}

/// One picture on the CPU, each channel 0 to 255, top row first.
struct Cpu {
    size: (usize, usize),
    pixels: Vec<[f64; 4]>,
}

impl Cpu {
    /// `sol_tex(uv)`: clamped half a texel inside the edge, as `run`
    /// clamps it (`run::clamp_rect`), then read as `GL_LINEAR` reads it.
    #[expect(clippy::cast_precision_loss, reason = "a texture side")]
    fn sample(&self, uv: [f64; 2]) -> [f64; 4] {
        let (w, h) = (self.size.0 as f64, self.size.1 as f64);
        let u = uv[0].clamp(0.5 / w, 1.0 - 0.5 / w) * w - 0.5;
        let v = uv[1].clamp(0.5 / h, 1.0 - 0.5 / h) * h - 0.5;
        let (x0, y0) = (u.floor(), v.floor());
        let (fx, fy) = (u - x0, v - y0);
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a texel index, 0 or more")]
        let at = |x: f64, y: f64| {
            let (x, y) = ((x as usize).min(self.size.0 - 1), (y as usize).min(self.size.1 - 1));
            self.pixels[y * self.size.0 + x]
        };
        let (a, b, c, d) = (at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0));
        std::array::from_fn(|k| {
            (a[k] * (1.0 - fx) + b[k] * fx) * (1.0 - fy) + (c[k] * (1.0 - fx) + d[k] * fx) * fy
        })
    }

    /// One Kawase pass of the fixture's `down.frag` or `up.frag` into `size`,
    /// each channel rounded to a byte as an `rgba8` target stores it.
    #[expect(clippy::cast_precision_loss, reason = "a texture side")]
    fn kawase(&self, up: bool, offset: f64, size: (usize, usize)) -> Cpu {
        let texel = [1.0 / self.size.0 as f64, 1.0 / self.size.1 as f64];
        let h = [texel[0] * 0.5 * offset, texel[1] * 0.5 * offset];
        let mut pixels = Vec::with_capacity(size.0 * size.1);
        for y in 0..size.1 {
            for x in 0..size.0 {
                let uv = [(x as f64 + 0.5) / size.0 as f64, (y as f64 + 0.5) / size.1 as f64];
                let s = |dx: f64, dy: f64| self.sample([uv[0] + dx, uv[1] + dy]);
                let taps: Vec<([f64; 4], f64)> = if up {
                    vec![
                        (s(-2.0 * h[0], 0.0), 1.0),
                        (s(2.0 * h[0], 0.0), 1.0),
                        (s(0.0, -2.0 * h[1]), 1.0),
                        (s(0.0, 2.0 * h[1]), 1.0),
                        (s(h[0], h[1]), 2.0),
                        (s(-h[0], -h[1]), 2.0),
                        (s(h[0], -h[1]), 2.0),
                        (s(-h[0], h[1]), 2.0),
                    ]
                } else {
                    vec![
                        (s(0.0, 0.0), 4.0),
                        (s(-h[0], -h[1]), 1.0),
                        (s(h[0], h[1]), 1.0),
                        (s(h[0], -h[1]), 1.0),
                        (s(-h[0], h[1]), 1.0),
                    ]
                };
                let total: f64 = taps.iter().map(|(_, weight)| weight).sum();
                pixels.push(std::array::from_fn(|k| {
                    (taps.iter().map(|(tap, weight)| tap[k] * weight).sum::<f64>() / total)
                        .round()
                        .clamp(0.0, 255.0)
                }));
            }
        }
        Cpu { size, pixels }
    }
}

/// **Case 12e: a dual Kawase built from `pass` matches a CPU reference
/// within ±1.** The `kawase` fixture at `passes = 2, offset = 2` over a
/// 257×129 checkerboard of 8-pixel squares, 0 and 255 at a different phase
/// in each channel: sizes from `Plan::sizes` (129×65, 65×33, back to 129×65
/// and 257×129), `sol_texel` one texel of each pass's input, bilinear reads
/// clamped at the edge, every channel within 1 of the CPU's.
fn a_kawase_blur_matches_the_cpu(renderer: &mut GlesRenderer) -> Result<()> {
    println!("\n=== FX2: a dual Kawase built from pass matches a CPU reference within one ===");
    let size = (257, 129);
    let phases: [(usize, usize); 4] = [(0, 0), (3, 5), (5, 2), (2, 7)];
    let checker = |x: usize, y: usize| -> [u8; 4] {
        std::array::from_fn(|k| {
            let (px, py) = phases[k];
            if ((x + px) / 8 + (y + py) / 8) % 2 == 0 { 255 } else { 0 }
        })
    };
    let (input, bytes) = upload(renderer, size, checker)?;
    let offset = 2.0;
    let (plan, programs) = build(renderer, kawase(2, offset))?;
    let padded = (257, 129);
    let (sizes, _) = plan.sizes(padded);
    let mut cpu = Cpu {
        size,
        pixels: bytes
            .chunks_exact(4)
            .map(|pixel| std::array::from_fn(|k| f64::from(pixel[k])))
            .collect(),
    };
    for (step, size) in plan.steps.iter().zip(&sizes) {
        let size = (
            usize::try_from(size.0).unwrap_or(1),
            usize::try_from(size.1).unwrap_or(1),
        );
        cpu = cpu.kawase(step.frag == "up.frag", offset, size);
    }
    if cpu.size != size {
        return Err(anyhow!("the plan came back to {:?}, not {size:?}", cpu.size));
    }
    let mut pool = pool::Pool::new(0);
    let mut held = run::Held::default();
    let textures = [("self", input, run::BoxMap::WHOLE)];
    let outcome = run_once(
        renderer,
        &mut pool,
        &|key| lookup(&programs, key),
        &plan,
        &mut held,
        &whole(size, &textures),
    )?;
    let run::Outcome::Done(result, _) = outcome else {
        return Err(anyhow!("the kawase run came to {outcome:?}"));
    };
    if result.size() != (257, 129).into() {
        return Err(anyhow!("the kawase result is {:?}, not 257×129", result.size()));
    }
    let read = read_rgba(renderer, &result)?;
    let mut worst = (0.0_f64, (0, 0));
    for (at, (got, want)) in read.chunks_exact(4).zip(&cpu.pixels).enumerate() {
        for k in 0..4 {
            let off = (f64::from(got[k]) - want[k]).abs();
            if off > worst.0 {
                worst = (off, (at % size.0, at / size.0));
            }
        }
    }
    drop(result);
    held.release(&mut pool);
    pool.sweep(renderer);
    free(renderer, programs)?;
    let (off, (x, y)) = worst;
    if off > 1.0 {
        return Err(anyhow!(
            "the kawase blur differs from the CPU reference by {off} at ({x}, {y})"
        ));
    }
    println!("  two down and two up over 257×129: at most {off} from the CPU's, at ({x}, {y})");
    Ok(())
}

/// **Case 12f: a pass reading three textures samples each.** The `three`
/// fixture over a solid red 16×16 `self`: a state draws green, and one pass
/// reads `self` on unit 0, the state on unit 1 and the analytic shape, which
/// covers the whole box; its centre is white.
fn a_pass_reading_three_textures_samples_each(renderer: &mut GlesRenderer) -> Result<()> {
    println!("\n=== FX2: a pass reading three textures samples each ===");
    let size = (16, 16);
    let (input, _) = upload(renderer, size, |_, _| RED)?;
    let (plan, programs) = build(renderer, three())?;
    let mut pool = pool::Pool::new(0);
    let mut held = run::Held::default();
    let textures = [("self", input, run::BoxMap::WHOLE)];
    let outcome = run_once(
        renderer,
        &mut pool,
        &|key| lookup(&programs, key),
        &plan,
        &mut held,
        &whole(size, &textures),
    )?;
    let run::Outcome::Done(result, _) = outcome else {
        return Err(anyhow!("the three-texture run came to {outcome:?}"));
    };
    let read = read_rgba(renderer, &result)?;
    drop(result);
    held.release(&mut pool);
    pool.sweep(renderer);
    free(renderer, programs)?;
    let centre = (8 * size.0 + 8) * 4;
    let got = read.get(centre..centre + 3).map(<[u8]>::to_vec).unwrap_or_default();
    if got != [255, 255, 255] {
        return Err(anyhow!("a pass reading three textures got {got:?}"));
    }
    println!("  self's red, the state's green and the shape's blue, all at the centre");
    Ok(())
}

/// Smithay's own draw of a solid colour into a 32×32 pooled target, through
/// `pool::paint`, as captures draw: the bytes it leaves.
fn smithay_solid(
    renderer: &mut GlesRenderer,
    pool: &mut pool::Pool,
    colour: smithay::backend::renderer::Color32F,
) -> Result<Vec<u8>> {
    use smithay::backend::renderer::element::{Id, Kind, solid::SolidColorRenderElement};
    use smithay::backend::renderer::utils::CommitCounter;
    use smithay::backend::renderer::Frame as _;
    let side = 32;
    let target = pool
        .target(&mut pool::Gl(renderer), (side, side).into(), pool::Format::Rgba8)
        .ok_or_else(|| anyhow!("no pooled target"))?;
    let mut carrier = pool.carrier(renderer).ok_or_else(|| anyhow!("no carrier"))?;
    {
        let mut bound = renderer.bind(&mut carrier).map_err(|err| anyhow!("{err}"))?;
        let mut frame =
            pool::frame_for(renderer, &mut bound, &target).map_err(|err| anyhow!("{err}"))?;
        let solid = SolidColorRenderElement::new(
            Id::new(),
            Rectangle::from_size((side, side).into()),
            CommitCounter::default(),
            colour,
            Kind::Unspecified,
        );
        pool::paint(&mut frame, (side, side).into(), &[solid], 1.0)
            .map_err(|err| anyhow!("{err}"))?;
        frame
            .finish()
            .map_err(|err| anyhow!("{err}"))?
            .wait()
            .map_err(|err| anyhow!("{err:?}"))?;
    }
    let read = read_rgba(renderer, target.texture())?;
    drop(target);
    pool.sweep(renderer);
    Ok(read)
}

/// What smithay leaves for whatever draws next, at its worst for a pass: a
/// divisor of 1 on attribute 0, as smithay's own per-instance attribute
/// carries wherever the driver put it.
fn leave_a_divisor_on_attribute_0(renderer: &mut GlesRenderer) -> Result<()> {
    // SAFETY: `with_context` makes the context current.
    renderer
        .with_context(|context| unsafe { context.VertexAttribDivisor(0, 1) })
        .map_err(|err| anyhow!("{err}"))
}

/// The executor's draw of one `sol_tex` pass with its divisor line left out:
/// 12g's control. A scratch copy of `run::Draw::draw`, cut to what the
/// `identity` fixture reads.
///
/// # Safety
/// Inside a frame's own context, with the program and texture its.
unsafe fn draw_without_divisor_reset(
    context: &ffi::Gles2,
    program: &gl::Program,
    texture: ffi::types::GLuint,
) {
    const QUAD: [f32; 8] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
    // SAFETY: the caller's contract.
    unsafe {
        context.Disable(ffi::BLEND);
        context.UseProgram(program.id);
        context.ActiveTexture(ffi::TEXTURE0);
        context.BindTexture(ffi::TEXTURE_2D, texture);
        let linear = i32::try_from(ffi::LINEAR).unwrap_or_default();
        let edge = i32::try_from(ffi::CLAMP_TO_EDGE).unwrap_or_default();
        context.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MIN_FILTER, linear);
        context.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MAG_FILTER, linear);
        context.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_S, edge);
        context.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_T, edge);
        if let Some(location) = program.location("sol_tex_sampler") {
            context.Uniform1i(location, 0);
        }
        for name in ["sol_tex_box", "sol_tex_clamp"] {
            if let Some(location) = program.location(name) {
                context.Uniform4f(location, 0.0, 0.0, 1.0, 1.0);
            }
        }
        context.BindBuffer(ffi::ARRAY_BUFFER, program.buffer);
        context.BufferData(
            ffi::ARRAY_BUFFER,
            isize::try_from(std::mem::size_of_val(&QUAD)).unwrap_or_default(),
            QUAD.as_ptr().cast(),
            ffi::STREAM_DRAW,
        );
        context.EnableVertexAttribArray(0);
        context.VertexAttribPointer(0, 2, ffi::FLOAT, ffi::FALSE, 0, std::ptr::null());
        // `run::Draw::draw` resets attribute 0's divisor here.
        context.DrawArrays(ffi::TRIANGLE_STRIP, 0, 4);
        context.DisableVertexAttribArray(0);
        context.BindBuffer(ffi::ARRAY_BUFFER, 0);
        context.BindTexture(ffi::TEXTURE_2D, 0);
        context.UseProgram(0);
        context.Enable(ffi::BLEND);
    }
}

/// **Case 12g: after a run, smithay draws as before; and a run after smithay
/// draws right.** A solid drawn by smithay through `pool::paint` after a run
/// of the `kawase` plan is byte for byte the solid it drew before any run.
/// Then smithay draws, attribute 0 is left with a divisor of 1 (what smithay's
/// per-instance attribute carries, wherever the driver put it), and the
/// `identity` plan still returns its input; its control, the same pass drawn
/// with the divisor line left out, must come out wrong, or the case proves
/// nothing.
fn smithay_draws_as_before_after_a_run(renderer: &mut GlesRenderer) -> Result<()> {
    use smithay::backend::renderer::{Color32F, Frame as _};
    println!("\n=== FX2: after a run smithay draws as before, and a run after smithay draws right ===");
    let colour = Color32F::new(0.25, 0.5, 0.75, 1.0);
    let mut pool = pool::Pool::new(0);
    let before = smithay_solid(renderer, &mut pool, colour)?;
    if before.chunks_exact(4).any(|pixel| pixel != before.get(..4).unwrap_or_default()) {
        return Err(anyhow!("smithay's own solid is not one colour: {:?}", before.get(..8)));
    }
    let (plan, programs) = build(renderer, kawase(2, 2.0))?;
    let (checker, _) = upload(renderer, (257, 129), |x, y| if (x / 8 + y / 8) % 2 == 0 { RED } else { BLUE })?;
    let mut held = run::Held::default();
    let textures = [("self", checker, run::BoxMap::WHOLE)];
    let outcome = run_once(
        renderer,
        &mut pool,
        &|key| lookup(&programs, key),
        &plan,
        &mut held,
        &whole((257, 129), &textures),
    )?;
    if !matches!(outcome, run::Outcome::Done(..)) {
        return Err(anyhow!("the kawase run came to {outcome:?}"));
    }
    drop(outcome);
    held.release(&mut pool);
    free(renderer, programs)?;
    let after = smithay_solid(renderer, &mut pool, colour)?;
    if let Some(((x, y), got, want)) = first_difference(&after, &before, 32) {
        return Err(anyhow!(
            "smithay's solid after a run is {got:?} at ({x}, {y}), not {want:?}"
        ));
    }
    // A run after smithay, attribute 0 left per instance.
    let (input, bytes, size) = quadrant(renderer)?;
    let (plan, programs) = build(renderer, identity())?;
    smithay_solid(renderer, &mut pool, colour)?;
    leave_a_divisor_on_attribute_0(renderer)?;
    let mut held = run::Held::default();
    let textures = [("self", input.clone(), run::BoxMap::WHOLE)];
    let outcome = run_once(
        renderer,
        &mut pool,
        &|key| lookup(&programs, key),
        &plan,
        &mut held,
        &whole(size, &textures),
    )?;
    let run::Outcome::Done(result, _) = outcome else {
        return Err(anyhow!("the identity run after smithay came to {outcome:?}"));
    };
    if let Some(((x, y), got, want)) = first_difference(&read_rgba(renderer, &result)?, &bytes, size.0) {
        return Err(anyhow!(
            "a run after smithay drew {got:?} at ({x}, {y}), not {want:?}"
        ));
    }
    drop(result);
    held.release(&mut pool);
    // The control: the same pass, its divisor line left out, attribute 0
    // left per instance inside the frame, after the clear, since smithay's
    // clear is a solid draw that sets its own divisors.
    let program = plan
        .steps
        .first()
        .and_then(|step| programs.get(&step.key))
        .ok_or_else(|| anyhow!("no identity program"))?;
    let target = pool
        .target(&mut pool::Gl(renderer), (64, 32).into(), pool::Format::Rgba8)
        .ok_or_else(|| anyhow!("no pooled target"))?;
    let mut carrier = pool.carrier(renderer).ok_or_else(|| anyhow!("no carrier"))?;
    {
        let mut bound = renderer.bind(&mut carrier).map_err(|err| anyhow!("{err}"))?;
        let mut frame =
            pool::frame_for(renderer, &mut bound, &target).map_err(|err| anyhow!("{err}"))?;
        frame
            .clear(Color32F::TRANSPARENT, &[Rectangle::from_size((64, 32).into())])
            .map_err(|err| anyhow!("{err}"))?;
        let name = input.tex_id();
        // SAFETY: inside the frame's own context; the program and texture
        // are its.
        frame
            .with_context(|context| unsafe {
                context.VertexAttribDivisor(0, 1);
                draw_without_divisor_reset(context, program, name);
            })
            .map_err(|err| anyhow!("{err}"))?;
        frame
            .finish()
            .map_err(|err| anyhow!("{err}"))?
            .wait()
            .map_err(|err| anyhow!("{err:?}"))?;
    }
    let control = read_rgba(renderer, target.texture())?;
    // SAFETY: `with_context` makes the context current; attribute 0 goes
    // back to per vertex, as the run left it.
    renderer
        .with_context(|context| unsafe { context.VertexAttribDivisor(0, 0) })
        .map_err(|err| anyhow!("{err}"))?;
    drop(target);
    pool.sweep(renderer);
    free(renderer, programs)?;
    if first_difference(&control, &bytes, size.0).is_none() {
        return Err(anyhow!("the control saw no corruption, so 12g sees nothing"));
    }
    println!(
        "  smithay's solid unchanged by a run; a run after smithay exact, and without its divisor reset not"
    );
    Ok(())
}

/// **Case 12q: a program withheld is pending, and latches nothing.** The
/// `identity` plan run with its program answered as not compiled yet: the
/// run is pending, nothing is drawn and the chain has not failed; with the
/// program, the same `Held` runs, twice, and draws 12d's picture each time.
/// A failed program fails the chain, and the failure stays when the program
/// is there: the real-GPU half of `run`'s unit test, whose latch is inside
/// `run`.
fn a_withheld_program_is_pending(renderer: &mut GlesRenderer) -> Result<()> {
    println!("\n=== FX2: a withheld program is pending and latches nothing ===");
    let (input, bytes, size) = quadrant(renderer)?;
    let (plan, programs) = build(renderer, identity())?;
    let mut pool = pool::Pool::new(0);
    let mut held = run::Held::default();
    let textures = [("self", input, run::BoxMap::WHOLE)];
    let inputs = whole(size, &textures);
    let outcome = run_once(renderer, &mut pool, &|_| run::Lookup::Pending, &plan, &mut held, &inputs)?;
    if !matches!(outcome, run::Outcome::Pending) || held.failed() || held.output().is_some() {
        return Err(anyhow!(
            "a pending program latched the chain: {outcome:?}, failed {}, a result {}",
            held.failed(),
            held.output().is_some()
        ));
    }
    for round in 1..=2 {
        let outcome = run_once(
            renderer,
            &mut pool,
            &|key| lookup(&programs, key),
            &plan,
            &mut held,
            &inputs,
        )?;
        let run::Outcome::Done(result, _) = outcome else {
            return Err(anyhow!("the run after the compile did not draw (round {round}): {outcome:?}"));
        };
        if let Some(((x, y), got, want)) = first_difference(&read_rgba(renderer, &result)?, &bytes, size.0) {
            return Err(anyhow!(
                "the run after the compile did not draw (round {round}): {got:?} at ({x}, {y}), not {want:?}"
            ));
        }
    }
    let mut broken = run::Held::default();
    let failed = run_once(renderer, &mut pool, &|_| run::Lookup::Failed, &plan, &mut broken, &inputs)?;
    let again = run_once(
        renderer,
        &mut pool,
        &|key| lookup(&programs, key),
        &plan,
        &mut broken,
        &inputs,
    )?;
    if !matches!(failed, run::Outcome::Failed) || !matches!(again, run::Outcome::Failed) || !broken.failed() {
        return Err(anyhow!(
            "a failed program did not latch the chain: {failed:?}, then {again:?}"
        ));
    }
    held.release(&mut pool);
    pool.sweep(renderer);
    free(renderer, programs)?;
    println!("  pending: nothing drawn, nothing latched; then drawn twice; a failure latched");
    Ok(())
}

/// 12l's to 12p's input: a 257×129 checkerboard of 8-pixel squares, red and
/// blue, the size 12e blurs.
fn checker(renderer: &mut GlesRenderer) -> Result<(GlesTexture, Vec<u8>, (usize, usize))> {
    let size = (257, 129);
    let (texture, bytes) =
        upload(renderer, size, |x, y| if (x / 8 + y / 8) % 2 == 0 { RED } else { BLUE })?;
    Ok((texture, bytes, size))
}

/// **Case 12l: an instance keeps its targets between runs.** The `kawase`
/// plan at `passes = 2` over 257×129 draws 129×65, 65×33, 129×65 and
/// 257×129: its first run makes three targets, not four, since the second
/// 129×65 step draws into the first's once nothing reads it, and a second
/// run with the same keys makes none.
fn an_instance_keeps_its_targets_between_runs(renderer: &mut GlesRenderer) -> Result<()> {
    println!("\n=== FX2: an instance keeps its targets between runs ===");
    let (input, _, size) = checker(renderer)?;
    let (plan, programs) = build(renderer, kawase(2, 2.0))?;
    let (sizes, _) = plan.sizes((257, 129));
    let (_, slots) = Plan::slots(&plan.steps, &sizes);
    let mut pool = pool::Pool::new(0);
    let mut held = run::Held::default();
    let textures = [("self", input, run::BoxMap::WHOLE)];
    let inputs = whole(size, &textures);
    let mut made = Vec::new();
    for round in 1..=2 {
        let outcome = run_once(
            renderer,
            &mut pool,
            &|key| lookup(&programs, key),
            &plan,
            &mut held,
            &inputs,
        )?;
        if !matches!(outcome, run::Outcome::Done(..)) {
            return Err(anyhow!("run {round} of the kawase plan came to {outcome:?}"));
        }
        made.push(pool.made());
    }
    held.release(&mut pool);
    pool.sweep(renderer);
    free(renderer, programs)?;
    if made != [slots, slots] || slots != 3 {
        return Err(anyhow!(
            "the pool had made {made:?} targets after each run, not {slots} and none more ({} steps)",
            plan.steps.len()
        ));
    }
    println!("  four steps drawn into three targets, and a second run made none");
    Ok(())
}

/// The red of a 16×16 result's centre, 0 to 1.
fn centre_red(renderer: &mut GlesRenderer, texture: &GlesTexture) -> Result<f64> {
    let read = read_rgba(renderer, texture)?;
    let size = texture.size();
    let width = usize::try_from(size.w).unwrap_or(1);
    let height = usize::try_from(size.h).unwrap_or(1);
    let at = ((height / 2) * width + width / 2) * 4;
    read.get(at)
        .map(|red| f64::from(*red) / 255.0)
        .ok_or_else(|| anyhow!("no centre in {} bytes", read.len()))
}

/// **Case 12m: a state is rebuilt only when its depends changes.** The
/// `state-count` fixture, whose state `n` (`depends = "params"`) writes
/// `fract(sol_time)` into red: run at `time` 0.1 and 0.2 with `params` 1,
/// then 0.3 with `params` 2, its red is 0.1, 0.1 and 0.3; then at 0.4 with
/// `params` 2 over a box of another size, where the state it held no longer
/// fits, 0.4.
fn a_state_is_rebuilt_only_when_its_depends_changes(renderer: &mut GlesRenderer) -> Result<()> {
    println!("\n=== FX2: a state is made again only when what it depends on changes ===");
    let (plan, programs) = build(renderer, state_count())?;
    let mut pool = pool::Pool::new(0);
    let mut held = run::Held::default();
    let mut reds = Vec::new();
    for (time, params, side) in [(0.1, 1, 16), (0.2, 1, 16), (0.3, 2, 16), (0.4, 2, 24)] {
        let (input, _) = upload(renderer, (side, side), |_, _| RED)?;
        let textures = [("self", input, run::BoxMap::WHOLE)];
        let mut inputs = whole((side, side), &textures);
        inputs.time = time;
        let keys = run::Keys {
            params,
            ..run::Keys::default()
        };
        let outcome = run_keyed(
            renderer,
            &mut pool,
            &|key| lookup(&programs, key),
            &plan,
            &mut held,
            &inputs,
            &keys,
        )?;
        let run::Outcome::Done(result, _) = outcome else {
            return Err(anyhow!("the state-count run at {time} came to {outcome:?}"));
        };
        reds.push(centre_red(renderer, &result)?);
    }
    held.release(&mut pool);
    pool.sweep(renderer);
    free(renderer, programs)?;
    let want = [0.1, 0.1, 0.3, 0.4];
    if reds.iter().zip(want).any(|(got, want)| (got - want).abs() > 1.0 / 255.0) {
        return Err(anyhow!("the state's red over four runs was {reds:?}, not {want:?}"));
    }
    println!("  red {reds:.3?}: kept while the params held, made when they moved and when the box did");
    Ok(())
}

/// **Case 12n: a held instance dropped gives its targets back.** After a
/// run of the `kawase` plan, `Held::release` leaves the pool's free list
/// holding exactly the plan's slots: asked for each slot's size and format
/// the pool makes nothing, and asked for one more, it makes one.
fn a_held_instance_dropped_gives_its_targets_back(renderer: &mut GlesRenderer) -> Result<()> {
    println!("\n=== FX2: a held instance dropped gives its targets back ===");
    let (input, _, size) = checker(renderer)?;
    let (plan, programs) = build(renderer, kawase(2, 2.0))?;
    let (sizes, _) = plan.sizes((257, 129));
    let (slots, count) = Plan::slots(&plan.steps, &sizes);
    let mut pool = pool::Pool::new(64 << 20);
    let mut held = run::Held::default();
    let textures = [("self", input, run::BoxMap::WHOLE)];
    let outcome = run_once(
        renderer,
        &mut pool,
        &|key| lookup(&programs, key),
        &plan,
        &mut held,
        &whole(size, &textures),
    )?;
    if !matches!(outcome, run::Outcome::Done(..)) {
        return Err(anyhow!("the kawase run came to {outcome:?}"));
    }
    drop(outcome);
    held.release(&mut pool);
    let before = pool.made();
    let mut taken = Vec::new();
    for slot in 0..count {
        let step = slots
            .iter()
            .position(|each| *each == slot)
            .ok_or_else(|| anyhow!("slot {slot} has no step"))?;
        let (w, h) = sizes[step];
        let size = (i32::try_from(w)?, i32::try_from(h)?).into();
        taken.push(
            pool.target(&mut pool::Gl(renderer), size, plan.steps[step].format)
                .ok_or_else(|| anyhow!("no target for slot {slot}"))?,
        );
    }
    let from_the_free_list = pool.made() - before;
    taken.push(
        pool.target(&mut pool::Gl(renderer), (129, 65).into(), pool::Format::Rgba8)
            .ok_or_else(|| anyhow!("no target past the slots"))?,
    );
    let past = pool.made() - before;
    drop(taken);
    pool.sweep(renderer);
    free(renderer, programs)?;
    if from_the_free_list != 0 || past != 1 {
        return Err(anyhow!(
            "after the release the pool made {from_the_free_list} of the {count} slots' targets and {past} with one more"
        ));
    }
    println!("  {count} targets given back for {count} slots, and none more");
    Ok(())
}

/// **Case 12o: a chain feeds each link's result to the next one's first
/// input, on the GPU.** `chain([identity, tint])` over 12d's picture is
/// byte for byte `tint` run on it alone, and is not the picture.
fn a_chain_feeds_each_links_result_to_the_next(renderer: &mut GlesRenderer) -> Result<()> {
    println!("\n=== FX2: a chain feeds each link's result to the next, on the GPU ===");
    let (input, bytes, size) = quadrant(renderer)?;
    let (first, mut programs) = build(renderer, identity())?;
    let (second, tint_programs) = build(renderer, tint(0.5))?;
    programs.extend(tint_programs);
    let alone = second.clone();
    let chained = stage::chain(vec![first, second]);
    let mut pool = pool::Pool::new(0);
    let textures = [("self", input, run::BoxMap::WHOLE)];
    let inputs = whole(size, &textures);
    let mut reads = Vec::new();
    for plan in [&chained, &alone] {
        let mut held = run::Held::default();
        let outcome = run_once(
            renderer,
            &mut pool,
            &|key| lookup(&programs, key),
            plan,
            &mut held,
            &inputs,
        )?;
        let run::Outcome::Done(result, _) = outcome else {
            return Err(anyhow!("a run of {} steps came to {outcome:?}", plan.steps.len()));
        };
        reads.push(read_rgba(renderer, &result)?);
        drop(result);
        held.release(&mut pool);
    }
    pool.sweep(renderer);
    free(renderer, programs)?;
    let [chain, tint] = reads.as_slice() else {
        return Err(anyhow!("two runs, not {}", reads.len()));
    };
    if let Some(((x, y), got, want)) = first_difference(chain, tint, size.0) {
        return Err(anyhow!(
            "the chain drew {got:?} at ({x}, {y}) where tint alone drew {want:?}"
        ));
    }
    if first_difference(chain, &bytes, size.0).is_none() {
        return Err(anyhow!("the chain returned its input: the tint did not run"));
    }
    println!("  identity then tint, as one plan, is tint alone");
    Ok(())
}

/// **Case 12p: nothing is allocated while a result is drawn.** After a run
/// of the `kawase` plan, its result, `Held::output`, drawn by smithay as a
/// `TextureRenderElement` through `pool::paint` into a target taken before,
/// makes no target, and what is drawn is the result.
fn nothing_is_allocated_while_a_result_is_drawn(renderer: &mut GlesRenderer) -> Result<()> {
    println!("\n=== FX2: nothing is allocated while a result is drawn ===");
    let (input, _, size) = checker(renderer)?;
    let (plan, programs) = build(renderer, kawase(2, 2.0))?;
    let mut pool = pool::Pool::new(0);
    let mut held = run::Held::default();
    let textures = [("self", input, run::BoxMap::WHOLE)];
    let outcome = run_once(
        renderer,
        &mut pool,
        &|key| lookup(&programs, key),
        &plan,
        &mut held,
        &whole(size, &textures),
    )?;
    if !matches!(outcome, run::Outcome::Done(..)) {
        return Err(anyhow!("the kawase run came to {outcome:?}"));
    }
    drop(outcome);
    let result = held
        .output()
        .cloned()
        .ok_or_else(|| anyhow!("a run that drew holds no output"))?;
    let side = (i32::try_from(size.0)?, i32::try_from(size.1)?);
    let target = pool
        .target(&mut pool::Gl(renderer), side.into(), pool::Format::Rgba8)
        .ok_or_else(|| anyhow!("no pooled target"))?;
    let before = pool.made();
    let mut carrier = pool.carrier(renderer).ok_or_else(|| anyhow!("no carrier"))?;
    {
        use smithay::backend::renderer::Frame as _;
        let element = crate::element_for(renderer, result.clone(), side, side, 1.0);
        let mut bound = renderer.bind(&mut carrier).map_err(|err| anyhow!("{err}"))?;
        let mut frame =
            pool::frame_for(renderer, &mut bound, &target).map_err(|err| anyhow!("{err}"))?;
        pool::paint(&mut frame, side.into(), &[element], 1.0).map_err(|err| anyhow!("{err}"))?;
        frame
            .finish()
            .map_err(|err| anyhow!("{err}"))?
            .wait()
            .map_err(|err| anyhow!("{err:?}"))?;
    }
    let made = pool.made() - before;
    let drawn = read_rgba(renderer, target.texture())?;
    let want = read_rgba(renderer, &result)?;
    drop(target);
    drop(result);
    held.release(&mut pool);
    pool.sweep(renderer);
    free(renderer, programs)?;
    if made != 0 {
        return Err(anyhow!("drawing a result made {made} targets"));
    }
    let off = drawn
        .iter()
        .zip(&want)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    if drawn.len() != want.len() || off > 1 {
        return Err(anyhow!("the result drawn differs from the result by {off}"));
    }
    println!("  the result drawn by smithay, with no target made");
    Ok(())
}

const GREEN: [u8; 4] = [0, 255, 0, 255];
const CLEAR: [u8; 4] = [0, 0, 0, 0];

/// **Case 12h: a result drawn through its mask: outside cut, inside exact.**
/// The compositor's own `EffectElement`, over a 148×98 box through
/// `MASKED_TEXTURE` compiled as `Programs::masked` compiles it, drawn by
/// `pool::paint` into a pooled target as captures draw: a solid green result
/// cut by the mask `(24, 24, 100, 50)` at radius 10 is green at (74, 49) and
/// (30, 49), and transparent at (10, 10) and at (25, 25), inside the rect past
/// a corner's arc. The same from a 74×49 result drawn over the 148×98 box,
/// because the mask is measured in the placement's pixels; and with no mask,
/// the result whole.
fn a_result_is_cut_by_its_mask(renderer: &mut GlesRenderer) -> Result<()> {
    use smithay::backend::renderer::Frame as _;
    use smithay::backend::renderer::element::Id;
    use smithay::backend::renderer::utils::CommitCounter;
    use smithay::utils::Physical;
    use solium_effects::fragment::{Corners, MASKED_TEXTURE};
    println!("\n=== FX2: a result drawn through its mask: outside cut, inside exact ===");
    let program = renderer
        .compile_custom_texture_shader(
            MASKED_TEXTURE,
            &crate::uniforms::registration(MASKED_TEXTURE),
        )
        .map_err(|err| anyhow!("MASKED_TEXTURE did not compile in every variant: {err}"))?;
    let side = (148, 98);
    let mask = (
        Rectangle::<f64, Physical>::new((24.0, 24.0).into(), (100.0, 50.0).into()),
        Corners::all(10.0),
    );
    let cut = [
        ((74, 49), GREEN),
        ((10, 10), CLEAR),
        ((25, 25), CLEAR),
        ((30, 49), GREEN),
    ];
    let whole = [((0, 0), GREEN), ((10, 10), GREEN), ((147, 97), GREEN)];
    let mut pool = pool::Pool::new(0);
    let mut carrier = pool.carrier(renderer).ok_or_else(|| anyhow!("no carrier"))?;
    for (what, result, mask, points) in [
        ("a 148×98 result", (148, 98), Some(mask), &cut[..]),
        ("a 74×49 result over the 148×98 box", (74, 49), Some(mask), &cut[..]),
        ("a result with no mask", (148, 98), None, &whole[..]),
    ] {
        let (texture, _) = upload(renderer, result, |_, _| GREEN)?;
        let placed =
            element::EffectElement::new(Id::new(), CommitCounter::default(), texture, program.clone())
                .at(Rectangle::from_size(side.into()), mask, 1.0);
        let target = pool
            .target(&mut pool::Gl(renderer), side.into(), pool::Format::Rgba8)
            .ok_or_else(|| anyhow!("no pooled target"))?;
        {
            let mut bound = renderer.bind(&mut carrier).map_err(|err| anyhow!("{err}"))?;
            let mut frame =
                pool::frame_for(renderer, &mut bound, &target).map_err(|err| anyhow!("{err}"))?;
            pool::paint(&mut frame, side.into(), &[placed], 1.0)
                .map_err(|err| anyhow!("{err}"))?;
            frame
                .finish()
                .map_err(|err| anyhow!("{err}"))?
                .wait()
                .map_err(|err| anyhow!("{err:?}"))?;
        }
        let drawn = read_rgba(renderer, target.texture())?;
        drop(target);
        for &((x, y), want) in points {
            let at = (y * usize::try_from(side.0)? + x) * 4;
            let got = drawn.get(at..at + 4).unwrap_or_default();
            if got != want {
                return Err(anyhow!("{what}: ({x}, {y}) is {got:?}, not {want:?}"));
            }
        }
    }
    pool.sweep(renderer);
    println!(
        "  inside exact, outside and past a corner's arc transparent, drawn 1:1 and at twice \
         its size; with no mask, whole"
    );
    Ok(())
}
