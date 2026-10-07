#![expect(
    unsafe_code,
    reason = "effect passes in raw GL: several textures per program, which smithay's texture program cannot sample"
)]
#![expect(
    dead_code,
    reason = "Task 21's runner runs plans; until then wirecheck runs them (cases 12d to 12g and 12q)"
)]

//! The executor: a plan's steps, each drawn into a pooled target in a frame
//! opened on the bound carrier (\[fx0\] Task 15's `frame_for`), each reading
//! `sol_tex` on unit 0 and its `uses` on the next units (Ruling 10).
//!
//! No CPU wait between steps: GL orders one context's reads after its writes
//! (\[fx0\] Task 9's cases 11c and 11d), so each step's sync point is dropped
//! and only the run's last comes back for the caller to settle.
//!
//! Smithay, std, `solium_effects` and `super::{gl, pool}` only, so
//! `dev/wirecheck` includes it (cases 12d to 12g and 12q).

use smithay::backend::renderer::{
    Frame as _, Texture as _,
    gles::{GlesRenderer, GlesTarget, GlesTexture, ffi},
    sync::SyncPoint,
};
use solium_effects::spec::Value;
use solium_effects::stage::{Feed, Plan, Step};

use super::{gl, pool};

/// Where a texture sits in the padded box: `uv` in the box maps to
/// `offset + uv * scale` in the texture.
/// `tests::a_texel_is_one_of_the_first_inputs_own_texels_in_uv`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BoxMap {
    pub(crate) offset: [f32; 2],
    pub(crate) scale: [f32; 2],
}

impl BoxMap {
    pub(crate) const WHOLE: Self = Self {
        offset: [0.0, 0.0],
        scale: [1.0, 1.0],
    };
}

/// The whole padded box, in `uv`: x, y, w, h.
/// `tests::the_first_input_is_clamped_to_its_edge_texels`.
const WHOLE_BOX: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// A transition's values for the prelude's `sol_progress` and the rest,
/// zero outside one: wirecheck 12d's run.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Transition {
    pub(crate) progress: f32,
    pub(crate) clamped: f32,
    pub(crate) direction: f32,
    pub(crate) seed: f32,
}

/// What one run reads: wirecheck 12d to 12g.
#[derive(Debug)]
pub(crate) struct Inputs<'a> {
    /// The padded box: the part plus the effect's reach, in pixels.
    pub(crate) padded: (u32, u32),
    /// The part inside the padded box, in `uv`: x, y, w, h.
    pub(crate) content: [f32; 4],
    /// Each input name's texture and where it sits in the box.
    pub(crate) textures: &'a [(&'a str, GlesTexture, BoxMap)],
    /// The mask's radii in pixels: top-left, top-right, bottom-left,
    /// bottom-right.
    pub(crate) radii: [f32; 4],
    /// Seconds, from the one clock (#164).
    pub(crate) time: f32,
    pub(crate) transition: Transition,
    /// Clamp the first step's `sol_tex` into the part (Ruling 6's edge rule):
    /// the caller sets it for a T1 chain in a `replace` slot.
    pub(crate) clamp_first: bool,
}

/// One chain's targets, held across runs: one a step, one a state's step,
/// the last output, and whether the chain failed, which is latched
/// (wirecheck 12q).
#[derive(Debug, Default)]
pub(crate) struct Held {
    steps: Vec<Option<pool::Target>>,
    states: Vec<Vec<Option<pool::Target>>>,
    output: Option<GlesTexture>,
    failed: bool,
}

impl Held {
    /// Give every target back to the pool: wirecheck 12q.
    pub(crate) fn release(&mut self, pool: &mut pool::Pool) {
        for target in self
            .steps
            .drain(..)
            .chain(self.states.drain(..).flatten())
            .flatten()
        {
            pool.give_back(target);
        }
        self.output = None;
    }

    /// The last run's result, while it stands: wirecheck 12q.
    pub(crate) fn output(&self) -> Option<&GlesTexture> {
        self.output.as_ref()
    }

    pub(crate) fn failed(&self) -> bool {
        self.failed
    }
}

/// A texture side, as the prelude's floats take it.
/// `tests::a_texel_is_one_of_the_first_inputs_own_texels_in_uv`.
#[expect(clippy::cast_precision_loss, reason = "a texture side, far below 2^24")]
fn side(n: u32) -> f32 {
    n as f32
}

/// A param, as GL takes it: wirecheck 12e's `p_offset`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a param, given to GL as a float"
)]
fn narrow(value: f64) -> f32 {
    value as f32
}

/// One texel of a `size` texture, in the box's `uv`. Ruling 6.
/// `tests::a_texel_is_one_of_the_first_inputs_own_texels_in_uv`.
pub(crate) fn texel(size: (u32, u32), map: BoxMap) -> [f32; 2] {
    let (w, h) = (side(size.0.max(1)), side(size.1.max(1)));
    [1.0 / w / map.scale[0], 1.0 / h / map.scale[1]]
}

/// Where `sol_tex` is clamped, in its texture's own coordinates: `clamp` (the
/// part, or the whole box) through `map`, half a texel inside, so a linear
/// read at the edge reads the edge texel alone and never half of the one
/// beyond it (Ruling 6's edge rule). A side under one texel is its centre.
/// `tests::the_first_input_is_clamped_to_its_edge_texels`.
pub(crate) fn clamp_rect(map: BoxMap, clamp: [f32; 4], size: (u32, u32)) -> [f32; 4] {
    let half = [0.5 / side(size.0.max(1)), 0.5 / side(size.1.max(1))];
    let mut out = [0.0; 4];
    for axis in 0..2 {
        let low = map.offset[axis] + clamp[axis] * map.scale[axis] + half[axis];
        let high =
            map.offset[axis] + (clamp[axis] + clamp[axis + 2]) * map.scale[axis] - half[axis];
        let (low, high) = if low <= high {
            (low, high)
        } else {
            let middle = (low + high) / 2.0;
            (middle, middle)
        };
        out[axis] = low;
        out[axis + 2] = high;
    }
    out
}

/// Keep `slots` at `len`, giving back any target past it. Wirecheck 12q
/// runs one `Held` twice.
fn fit(slots: &mut Vec<Option<pool::Target>>, len: usize, pool: &mut pool::Pool) {
    let keep = len.min(slots.len());
    for target in slots.drain(keep..).flatten() {
        pool.give_back(target);
    }
    slots.resize_with(len, || None);
}

/// The target a step draws into: the one it held, if it is still the size
/// and format the step needs, else one from the pool. Wirecheck 12e's
/// passes, each of its own size.
fn take(
    slot: &mut Option<pool::Target>,
    pool: &mut pool::Pool,
    renderer: &mut GlesRenderer,
    size: (u32, u32),
    format: pool::Format,
) -> Option<pool::Target> {
    let wanted =
        smithay::utils::Size::from((i32::try_from(size.0).ok()?, i32::try_from(size.1).ok()?));
    if let Some(held) = slot.as_ref()
        && held.size() == wanted
        && held.format() == format
    {
        return Some(held.clone());
    }
    if let Some(old) = slot.take() {
        pool.give_back(old);
    }
    let target = pool.target(&mut pool::Gl(renderer), wanted, format)?;
    *slot = Some(target.clone());
    Some(target)
}

/// What one run came to. `tests::a_program_not_compiled_yet_is_pending_not_failed`,
/// wirecheck 12q.
#[derive(Debug)]
pub(crate) enum Outcome {
    /// Drawn: the result, and the last step's sync point to settle.
    Done(GlesTexture, SyncPoint),
    /// Not run, and nothing latched: a program not compiled yet, or a format
    /// the probe refused before the rebind (Rulings 10, 11).
    Pending,
    /// Failed, latched in `Held`: a program that would not compile, a GL
    /// error, a missing target.
    Failed,
}

/// A program by key, as the host has it.
/// `effect::host::tests::a_programs_lookup_tells_pending_from_failed`.
#[derive(Debug)]
pub(crate) enum Lookup<'p, P = gl::Program> {
    Ready(&'p P),
    Pending,
    Failed,
}

/// Whether `plan` can run now: `None` to run it, else why not, read before
/// anything is drawn. A failed program wins over a pending one.
/// `tests::a_program_not_compiled_yet_is_pending_not_failed`.
pub(crate) fn preflight<'p, P>(
    plan: &Plan,
    programs: &dyn Fn(u64) -> Lookup<'p, P>,
    formats: Option<pool::Formats>,
) -> Option<Outcome> {
    let mut pending = formats.is_some_and(|formats| !formats.supports(plan));
    for step in plan
        .steps
        .iter()
        .chain(plan.states.iter().flat_map(|state| state.steps.iter()))
    {
        match programs(step.key) {
            Lookup::Ready(_) => {}
            Lookup::Pending => pending = true,
            Lookup::Failed => return Some(Outcome::Failed),
        }
    }
    pending.then_some(Outcome::Pending)
}

/// Run `plan` once. `Pending` before anything is drawn when a program is not
/// compiled yet; `Failed` when a program failed, a target is missing or GL
/// reported an error, which is checked once a run rather than a step, since
/// `glGetError` can stall; a failure is latched in `held`. Either way the
/// caller draws the part as if no effect were configured (\[16\] §5).
/// Wirecheck cases 12d to 12g and 12q.
#[expect(
    clippy::too_many_arguments,
    reason = "one run's whole context, passed down once"
)]
pub(crate) fn run<'p>(
    renderer: &mut GlesRenderer,
    carrier: &mut GlesTarget<'_>,
    pool: &mut pool::Pool,
    programs: &dyn Fn(u64) -> Lookup<'p>,
    formats: Option<pool::Formats>,
    plan: &Plan,
    held: &mut Held,
    inputs: &Inputs<'_>,
) -> Outcome {
    if held.failed {
        return Outcome::Failed;
    }
    // Nothing is drawn, and nothing latched, until every program is there
    // (wirecheck 12q).
    if let Some(outcome) = preflight(plan, programs, formats) {
        if matches!(outcome, Outcome::Failed) {
            held.failed = true;
            held.output = None;
        }
        return outcome;
    }
    let ready = |key: u64| match programs(key) {
        Lookup::Ready(program) => Some(program),
        Lookup::Pending | Lookup::Failed => None,
    };
    let drawn = draw_plan(renderer, carrier, pool, &ready, plan, held, inputs);
    // SAFETY: `with_context` makes the renderer's context current.
    let error = renderer
        .with_context(|context| unsafe { context.GetError() })
        .ok();
    match (drawn, error) {
        (Some((texture, sync)), Some(ffi::NO_ERROR)) => {
            held.output = Some(texture.clone());
            Outcome::Done(texture, sync)
        }
        _ => {
            held.failed = true;
            held.output = None;
            Outcome::Failed
        }
    }
}

/// Every state, then the steps: the last step's texture and sync point, or
/// `None` at the first thing missing. Wirecheck 12f.
fn draw_plan<'p>(
    renderer: &mut GlesRenderer,
    carrier: &mut GlesTarget<'_>,
    pool: &mut pool::Pool,
    programs: &dyn Fn(u64) -> Option<&'p gl::Program>,
    plan: &Plan,
    held: &mut Held,
    inputs: &Inputs<'_>,
) -> Option<(GlesTexture, SyncPoint)> {
    let (sizes, state_sizes) = plan.sizes(inputs.padded);
    fit(&mut held.steps, plan.steps.len(), pool);
    for extra in held
        .states
        .drain(plan.states.len().min(held.states.len())..)
    {
        for target in extra.into_iter().flatten() {
            pool.give_back(target);
        }
    }
    held.states.resize_with(plan.states.len(), Vec::new);
    // States first, because a step may read one (wirecheck 12f).
    let mut states: Vec<GlesTexture> = Vec::with_capacity(plan.states.len());
    for (index, state) in plan.states.iter().enumerate() {
        let slots = held.states.get_mut(index)?;
        fit(slots, state.steps.len(), pool);
        let sizes = state_sizes.get(index)?;
        let (texture, _) = steps(
            renderer,
            carrier,
            pool,
            programs,
            &state.steps,
            sizes,
            slots,
            &states,
            inputs,
            false,
        )?;
        states.push(texture);
    }
    steps(
        renderer,
        carrier,
        pool,
        programs,
        &plan.steps,
        &sizes,
        &mut held.steps,
        &states,
        inputs,
        inputs.clamp_first,
    )
}

/// A texture's size, as the prelude's sides take it: `sol_texel`'s
/// (wirecheck 12e).
fn size_of(texture: &GlesTexture) -> (u32, u32) {
    let size = texture.size();
    (
        u32::try_from(size.w).unwrap_or(1),
        u32::try_from(size.h).unwrap_or(1),
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "one run's whole context, passed down once"
)]
fn steps<'p>(
    renderer: &mut GlesRenderer,
    carrier: &mut GlesTarget<'_>,
    pool: &mut pool::Pool,
    programs: &dyn Fn(u64) -> Option<&'p gl::Program>,
    steps: &[Step],
    sizes: &[(u32, u32)],
    slots: &mut [Option<pool::Target>],
    states: &[GlesTexture],
    inputs: &Inputs<'_>,
    clamp_first: bool,
) -> Option<(GlesTexture, SyncPoint)> {
    let mut drawn: Vec<GlesTexture> = Vec::with_capacity(steps.len());
    let mut last = None;
    for (index, step) in steps.iter().enumerate() {
        let size = *sizes.get(index)?;
        let target = take(slots.get_mut(index)?, pool, renderer, size, step.format)?;
        let program = programs(step.key)?;
        let source = |feed: &Feed| -> Option<(GlesTexture, BoxMap)> {
            match feed {
                Feed::Step(k) => drawn
                    .get(*k)
                    .cloned()
                    .map(|texture| (texture, BoxMap::WHOLE)),
                Feed::State(k) => states
                    .get(*k)
                    .cloned()
                    .map(|texture| (texture, BoxMap::WHOLE)),
                Feed::Input(name) => inputs
                    .textures
                    .iter()
                    .find(|(each, _, _)| *each == name.as_str())
                    .map(|(_, texture, map)| (texture.clone(), *map)),
            }
        };
        let (first, first_map) = source(&step.first)?;
        let mut more = Vec::with_capacity(step.uses.len());
        for (name, feed) in &step.uses {
            let (texture, map) = source(feed)?;
            more.push((name.as_str(), texture, map));
        }
        let part = if clamp_first && index == 0 {
            inputs.content
        } else {
            WHOLE_BOX
        };
        let draw = Draw {
            program,
            step,
            size,
            first: (&first, first_map),
            more: &more,
            clamp: clamp_rect(first_map, part, size_of(&first)),
            inputs,
        };
        let mut frame = pool::frame_for(renderer, carrier, &target).ok()?;
        // SAFETY: inside the frame's own context, with every name this
        // context's; `draw` puts back what smithay expects.
        frame
            .with_context(|context| unsafe { draw.draw(context) })
            .ok()?;
        let sync = frame.finish().ok()?;
        drawn.push(target.texture().clone());
        last = Some((target.texture().clone(), sync));
    }
    last
}

/// One step's draw: wirecheck 12d to 12g.
struct Draw<'a> {
    program: &'a gl::Program,
    step: &'a Step,
    size: (u32, u32),
    first: (&'a GlesTexture, BoxMap),
    more: &'a [(&'a str, GlesTexture, BoxMap)],
    /// `sol_tex_clamp`: [`clamp_rect`]'s.
    clamp: [f32; 4],
    inputs: &'a Inputs<'a>,
}

impl std::fmt::Debug for Draw<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Draw")
            .field("frag", &self.step.frag)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

/// A full-target quad, `position` 0..1, as `glsl::PASS_VERTEX` reads it:
/// wirecheck 12d's byte-for-byte identity.
const QUAD: [f32; 8] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];

/// A texture unit's name, as GL takes it: wirecheck 12f's two.
fn unit(index: usize) -> u32 {
    ffi::TEXTURE0 + u32::try_from(index).unwrap_or(0)
}

impl Draw<'_> {
    /// # Safety
    /// Inside the frame's own context, with the program and every texture
    /// that context's.
    unsafe fn draw(&self, gl: &ffi::Gles2) {
        // SAFETY: the caller's contract.
        unsafe {
            let at = |name: &str| self.program.location(name);
            gl.Disable(ffi::BLEND);
            gl.UseProgram(self.program.id);
            let mut units = vec![(self.first.0.tex_id(), "sol_tex_sampler".to_owned())];
            units.extend(
                self.more.iter().map(|(name, texture, _)| {
                    (texture.tex_id(), solium_effects::glsl::sampler(name))
                }),
            );
            let linear = i32::try_from(ffi::LINEAR).unwrap_or_default();
            let edge = i32::try_from(ffi::CLAMP_TO_EDGE).unwrap_or_default();
            for (index, (texture, sampler)) in units.iter().enumerate() {
                gl.ActiveTexture(unit(index));
                gl.BindTexture(ffi::TEXTURE_2D, *texture);
                // Linear and clamped, on every texture bound: one made by
                // `create_buffer` still wants mipmaps and samples as black
                // (`warp/gl.rs`), and a blur's taps read between texels
                // (wirecheck 12e).
                gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MIN_FILTER, linear);
                gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MAG_FILTER, linear);
                gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_S, edge);
                gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_T, edge);
                if let Some(location) = at(sampler) {
                    gl.Uniform1i(location, i32::try_from(index).unwrap_or(0));
                }
            }
            let vec4 = |name: &str, v: [f32; 4]| {
                if let Some(location) = at(name) {
                    gl.Uniform4f(location, v[0], v[1], v[2], v[3]);
                }
            };
            let vec2 = |name: &str, v: [f32; 2]| {
                if let Some(location) = at(name) {
                    gl.Uniform2f(location, v[0], v[1]);
                }
            };
            let float = |name: &str, v: f32| {
                if let Some(location) = at(name) {
                    gl.Uniform1f(location, v);
                }
            };
            let map = self.first.1;
            vec4(
                "sol_tex_box",
                [map.offset[0], map.offset[1], map.scale[0], map.scale[1]],
            );
            vec4("sol_tex_clamp", self.clamp);
            vec2("sol_texel", texel(size_of(self.first.0), map));
            vec2("sol_size", [side(self.size.0), side(self.size.1)]);
            vec4("sol_content", self.inputs.content);
            vec2(
                "sol_box_px",
                [side(self.inputs.padded.0), side(self.inputs.padded.1)],
            );
            vec4("sol_radii", self.inputs.radii);
            float("sol_progress", self.inputs.transition.progress);
            float("sol_clamped", self.inputs.transition.clamped);
            float("sol_direction", self.inputs.transition.direction);
            float("sol_seed", self.inputs.transition.seed);
            float("sol_time", self.inputs.time);
            for (name, _, map) in self.more {
                vec4(
                    &solium_effects::glsl::box_of(name),
                    [map.offset[0], map.offset[1], map.scale[0], map.scale[1]],
                );
            }
            for (name, value) in &self.step.uniforms {
                let Some(location) = at(&format!("p_{name}")) else {
                    continue;
                };
                match value {
                    Value::Number(number) => gl.Uniform1f(location, narrow(*number)),
                    Value::Int(int) => {
                        gl.Uniform1i(location, i32::try_from(*int).unwrap_or(i32::MAX));
                    }
                    Value::Bool(yes) => gl.Uniform1i(location, i32::from(*yes)),
                    Value::Vec4(four) => gl.Uniform4f(
                        location,
                        narrow(four[0]),
                        narrow(four[1]),
                        narrow(four[2]),
                        narrow(four[3]),
                    ),
                    Value::Word(_) => {}
                }
            }
            gl.BindBuffer(ffi::ARRAY_BUFFER, self.program.buffer);
            gl.BufferData(
                ffi::ARRAY_BUFFER,
                isize::try_from(std::mem::size_of_val(&QUAD)).unwrap_or_default(),
                QUAD.as_ptr().cast(),
                ffi::STREAM_DRAW,
            );
            gl.EnableVertexAttribArray(0);
            gl.VertexAttribPointer(0, 2, ffi::FLOAT, ffi::FALSE, 0, std::ptr::null());
            // Per vertex: smithay draws instanced with a divisor of 1 on its
            // own attribute, and the divisor is state on the index, not the
            // program (`warp/gl.rs`). Wirecheck 12g's control leaves this
            // line out and sees the quad collapse.
            gl.VertexAttribDivisor(0, 0);
            gl.DrawArrays(ffi::TRIANGLE_STRIP, 0, 4);
            // What smithay expects to find (`warp/gl.rs`'s list): every unit
            // used unbound, unit 0 active, and blending as it draws with it.
            // Wirecheck 12g.
            gl.DisableVertexAttribArray(0);
            gl.BindBuffer(ffi::ARRAY_BUFFER, 0);
            for index in (0..units.len()).rev() {
                gl.ActiveTexture(unit(index));
                gl.BindTexture(ffi::TEXTURE_2D, 0);
            }
            gl.UseProgram(0);
            gl.Enable(ffi::BLEND);
            gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BoxMap, clamp_rect, texel};

    /// `sol_texel` is one texel of `sol_tex`'s own texture, in the padded
    /// box's `uv` (Ruling 6): a 100-wide texture seen through a box map that
    /// shows half of it is 1/50 of the box per texel.
    #[test]
    fn a_texel_is_one_of_the_first_inputs_own_texels_in_uv() {
        assert_eq!(texel((100, 50), BoxMap::WHOLE), [0.01, 0.02]);
        assert_eq!(
            texel(
                (100, 50),
                BoxMap {
                    offset: [0.0, 0.0],
                    scale: [0.5, 1.0]
                }
            ),
            [0.02, 0.02]
        );
    }

    /// **The first input is clamped to its edge texels** (Ruling 6's edge
    /// rule): half a texel inside the part, so a linear read at the part's
    /// edge reads the part's own edge texel, never half of the padding
    /// beyond it; through a box map, in the texture's own coordinates; and a
    /// part one texel wide collapses to that texel's centre.
    #[test]
    fn the_first_input_is_clamped_to_its_edge_texels() {
        let near = |got: [f32; 4], want: [f32; 4]| {
            assert!(
                got.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-6),
                "{got:?} is not {want:?}"
            );
        };
        near(
            clamp_rect(BoxMap::WHOLE, [0.1, 0.2, 0.5, 0.4], (100, 50)),
            [0.105, 0.21, 0.595, 0.59],
        );
        near(
            clamp_rect(BoxMap::WHOLE, [0.0, 0.0, 1.0, 1.0], (100, 50)),
            [0.005, 0.01, 0.995, 0.99],
        );
        let right_half = BoxMap {
            offset: [0.5, 0.0],
            scale: [0.5, 1.0],
        };
        near(
            clamp_rect(right_half, [0.0, 0.0, 1.0, 1.0], (100, 50)),
            [0.505, 0.01, 0.995, 0.99],
        );
        near(
            clamp_rect(BoxMap::WHOLE, [0.5, 0.0, 0.01, 1.0], (100, 50)),
            [0.505, 0.01, 0.505, 0.99],
        );
    }

    /// **A program not compiled yet is pending, not failed** (Ruling 10):
    /// nothing is latched and the chain runs once it compiles; a program
    /// that failed is a failure; a format the probe refused waits for the
    /// rebind; unknown formats are not missing (Ruling 11).
    #[test]
    fn a_program_not_compiled_yet_is_pending_not_failed() {
        use super::{Lookup, Outcome, preflight};
        use crate::pool::Formats;
        use solium_effects::spec::Value;
        use solium_effects::stage::{Binding, Format, Stage, flatten};
        let mut lib = |_: &str, _: &[(String, Value)]| -> Result<Binding, String> {
            Ok(Binding {
                stages: vec![Stage::Pass {
                    frag: "a.frag".to_owned(),
                    scale: 1.0,
                    format: Format::Rgba16f,
                    uses: Vec::new(),
                    input: None,
                }],
                inputs: vec!["self".to_owned()],
                params: Vec::new(),
            })
        };
        let plan = flatten("x", &[], &mut lib).expect("flattens");
        let program = 1_u32;
        let half = Some(Formats { rgba16f: true });
        assert!(
            preflight(&plan, &|_| Lookup::Ready(&program), half).is_none(),
            "everything there: run it"
        );
        assert!(matches!(
            preflight(&plan, &|_| Lookup::<u32>::Pending, half),
            Some(Outcome::Pending)
        ));
        assert!(matches!(
            preflight(&plan, &|_| Lookup::<u32>::Failed, half),
            Some(Outcome::Failed)
        ));
        assert!(
            matches!(
                preflight(
                    &plan,
                    &|_| Lookup::Ready(&program),
                    Some(Formats { rgba16f: false })
                ),
                Some(Outcome::Pending)
            ),
            "a refused format waits for the rebind"
        );
        assert!(
            preflight(&plan, &|_| Lookup::Ready(&program), None).is_none(),
            "unknown formats are not missing"
        );
    }
}
