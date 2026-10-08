//! Geometry as files (\[16\] decision 6, §2): an effect's `mesh` writes the
//! whole grid once a pass, and Rust checks what it wrote before a vertex of
//! it is drawn (\[16\] §5).
//!
//! A call is told `t`, one table per effect state reused every call, which
//! carries every param beside the engine's fields ([`super::sandbox`]'s
//! `reused`): `tests::every_param_reaches_t`. What it wrote is refused when
//! it is not a whole grid of finite numbers inside a box four monitors wide
//! and high around the pane's
//! (`tests::a_mesh_that_writes_a_nan_or_too_few_points_or_too_far_is_refused`),
//! and a file is held at load to drawing the window where it is at progress
//! 0 (`tests::a_geometry_file_that_moves_the_window_at_progress_zero_is_refused_at_load`).

use std::time::{Duration, Instant};

use solium_effects::spec::{GridSpec, Value};
use solium_effects::{Axis, Rect};

use super::sandbox::{Budget, Sandbox, numbers};
use crate::warp::UnitRect;

/// The most numeric params a geometry or pixels effect may pack:
/// `effects.limits.params`' upper bound. A representation's size, not a
/// behaviour (Ruling 28): a geometry rides `present::Frame`, which is `Copy`,
/// so its params are an inline array, and the key picks how many of it may
/// be used. `settings::tests::the_engines_keys_read_with_their_defaults_and_bounds`.
pub(crate) const PARAMS_MAX: usize = 64;

/// A geometry or pixels effect's numeric params, in their sorted order: a
/// number, an integer as its number, a boolean as 0 or 1, a `vec4` as four.
/// An inline array, so a geometry can ride `present::Frame`, which is `Copy`.
/// `tests::params_pack_up_to_their_limit_and_lerp_only_like_with_like`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Params {
    len: u8,
    values: [f32; PARAMS_MAX],
}

/// Empty (an array past 32 has no derived `Default`).
impl Default for Params {
    fn default() -> Self {
        Self {
            len: 0,
            values: [0.0; PARAMS_MAX],
        }
    }
}

impl Params {
    /// The numbers, at most `limit` of them (`effects.limits.params`); more
    /// is `Err` with how many, for a problem naming the key.
    /// `tests::params_pack_up_to_their_limit_and_lerp_only_like_with_like`.
    pub(crate) fn pack(numbers: &[f32], limit: usize) -> Result<Self, usize> {
        if numbers.len() > limit.min(PARAMS_MAX) {
            return Err(numbers.len());
        }
        let mut params = Self::default();
        let len = u8::try_from(numbers.len()).map_err(|_| numbers.len())?;
        params
            .values
            .get_mut(..numbers.len())
            .ok_or(numbers.len())?
            .copy_from_slice(numbers);
        params.len = len;
        Ok(params)
    }

    pub(crate) fn as_slice(&self) -> &[f32] {
        self.values.get(..usize::from(self.len)).unwrap_or(&[])
    }

    /// Each lerped toward `other`'s when both hold as many; `other`'s
    /// otherwise (another effect's params do not blend with these).
    /// `tests::params_pack_up_to_their_limit_and_lerp_only_like_with_like`.
    pub(crate) fn lerp(self, other: Self, t: f64) -> Self {
        if self.len != other.len {
            return other;
        }
        let mut params = self;
        for (mine, theirs) in params
            .values
            .iter_mut()
            .zip(other.values)
            .take(usize::from(self.len))
        {
            #[expect(clippy::cast_possible_truncation, reason = "a param, as stored")]
            let blended = (f64::from(*mine) + (f64::from(theirs) - f64::from(*mine)) * t) as f32;
            *mine = blended;
        }
        params
    }
}

/// Bound params packed for a geometry: refused, saying why, for a word
/// (a present's params blend between two presents, and a word cannot) or
/// more numbers than `limit`, `effects.limits.params`, never cut.
/// `tests::params_pack_up_to_their_limit_and_lerp_only_like_with_like`.
pub(crate) fn packed(bound: &[(String, Value)], limit: usize) -> Result<Params, String> {
    let mut numbers = Vec::with_capacity(bound.len());
    for (name, value) in bound {
        #[expect(clippy::cast_possible_truncation, reason = "a param, stored as f32")]
        #[expect(clippy::cast_precision_loss, reason = "a param, far below 2^24")]
        match value {
            Value::Number(number) => numbers.push(*number as f32),
            Value::Int(int) => numbers.push(*int as f32),
            Value::Bool(yes) => numbers.push(if *yes { 1.0 } else { 0.0 }),
            Value::Vec4(four) => numbers.extend(four.iter().map(|each| *each as f32)),
            Value::Word(_) => {
                return Err(format!(
                    "its param `{name}` is a word, and a `sol.present` blends its params between two presents, which a word cannot"
                ));
            }
        }
    }
    Params::pack(&numbers, limit).map_err(|count| {
        format!(
            "its params are {count} numbers once packed, more than `effects.limits.params` ({limit}); none is cut"
        )
    })
}

/// Packed params named again, by `defaults`' names and kinds (the effect's
/// params at their defaults, in the same sorted order): what a `mesh`'s `t`
/// is told. A number is the `f32` it was stored as, widened (Ruling 19); a
/// boolean is true from 0.5, so a blend between two crosses half way.
/// `tests::params_pack_up_to_their_limit_and_lerp_only_like_with_like`.
pub(crate) fn unpacked(defaults: &[(String, Value)], params: &Params) -> Vec<(String, Value)> {
    let mut numbers = params.as_slice().iter().map(|each| f64::from(*each));
    let mut named = Vec::with_capacity(defaults.len());
    for (name, default) in defaults {
        let value = match default {
            Value::Number(_) => numbers.next().map(Value::Number),
            #[expect(clippy::cast_possible_truncation, reason = "an int param, rounded")]
            Value::Int(_) => numbers.next().map(|each| Value::Int(each.round() as i64)),
            Value::Bool(_) => numbers.next().map(|each| Value::Bool(each >= 0.5)),
            Value::Vec4(_) => {
                let four: Vec<f64> = numbers.by_ref().take(4).collect();
                match four[..] {
                    [a, b, c, d] => Some(Value::Vec4([a, b, c, d])),
                    _ => None,
                }
            }
            Value::Word(_) => Some(default.clone()),
        };
        named.push((name.clone(), value.unwrap_or_else(|| default.clone())));
    }
    named
}

/// Everything a pane's grid depends on (Ruling 19), each float as its bits,
/// so a key is equal only to the same numbers: the grid is built again when
/// any differs. `tests::a_mesh_is_not_rebuilt_when_neither_progress_nor_an_anchor_changed`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MeshKey {
    effect: super::host::EffectId,
    params: Params,
    part: [u64; 4],
    cols: u32,
    rows: u32,
    axis: Axis,
    /// As it is, not its `signum`, which makes a resize's 0 an open's +1
    /// (`tests::a_mesh_is_not_rebuilt_when_neither_progress_nor_an_anchor_changed`).
    direction: u64,
    seed: u64,
    progress: u64,
    from: [u64; 4],
    to: [u64; 4],
    /// `t.monitor` and `t.scale`, which a `mesh` may read too.
    monitor: [u64; 4],
    scale: u64,
}

impl MeshKey {
    /// The key of `ask` for `effect` and its packed `params`, over a grid of
    /// `cols` by `rows`.
    pub(crate) fn new(
        effect: super::host::EffectId,
        params: Params,
        ask: &Ask<'_>,
        cols: u32,
        rows: u32,
    ) -> Self {
        let rect = |r: Rect| [r.x.to_bits(), r.y.to_bits(), r.w.to_bits(), r.h.to_bits()];
        Self {
            effect,
            params,
            part: [
                ask.part.u0.to_bits(),
                ask.part.v0.to_bits(),
                ask.part.u1.to_bits(),
                ask.part.v1.to_bits(),
            ],
            cols,
            rows,
            axis: ask.axis,
            direction: ask.direction.to_bits(),
            seed: ask.seed.to_bits(),
            progress: ask.progress.to_bits(),
            from: rect(ask.from),
            to: rect(ask.to.unwrap_or(ask.from)),
            monitor: rect(ask.monitor),
            scale: ask.scale.to_bits(),
        }
    }

    /// A key at `progress` toward `to`, everything else fixed.
    #[cfg(test)]
    pub(crate) fn for_test(progress: f64, to: Rect) -> Self {
        let ask = Ask {
            progress,
            clamped: progress,
            to: Some(to),
            ..tests::ask()
        };
        Self::new(
            super::host::EffectId::for_test(1),
            Params::default(),
            &ask,
            1,
            1,
        )
    }
}

/// A pane's last grids and their keys, one per warp piece (the pane and its
/// popups), and the version a `sol.present` geometry began with, pinned
/// while `effects.present.on_reload = "keep"` holds it.
/// `tests::a_mesh_is_not_rebuilt_when_neither_progress_nor_an_anchor_changed`.
#[derive(Debug, Default)]
pub(crate) struct Meshes {
    last: Vec<(MeshKey, crate::warp::Grid)>,
    /// `state::tests::real_client::a_reload_mid_present_follows_on_reload`.
    pub(crate) pinned: Option<std::rc::Rc<super::host::Loaded>>,
    /// Whether a refusal has been logged since the last grid built, so a
    /// pane's refused mesh is said once and not every pass.
    pub(crate) said: bool,
}

impl Meshes {
    /// The grid for `key`: the one built for it last, or `build`'s, kept,
    /// the oldest of the pane's pieces' grids given up for it.
    /// `tests::a_mesh_is_not_rebuilt_when_neither_progress_nor_an_anchor_changed`.
    pub(crate) fn get_or_build(
        &mut self,
        key: MeshKey,
        build: impl FnOnce() -> Result<crate::warp::Grid, Refusal>,
    ) -> Result<&crate::warp::Grid, Refusal> {
        let at = match self.last.iter().position(|(held, _)| *held == key) {
            Some(at) => at,
            None => {
                let grid = build()?;
                // One grid per piece a warped pane draws, its own and its
                // popups' (`render::WARP_ORDER`): kept most recent first.
                self.last.truncate(crate::render::WARP_ORDER.len() - 1);
                self.last.insert(0, (key, grid));
                0
            }
        };
        self.last
            .get(at)
            .map(|(_, grid)| grid)
            .ok_or(Refusal::Count { wanted: 0, got: 0 })
    }
}

/// Why a `mesh` call's grid is not drawn.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Refusal {
    /// A Lua error, at its line.
    Error {
        line: Option<u32>,
        message: String,
    },
    /// Stopped by its budget, or its state was stopped before.
    Budget,
    /// Not `wanted` numbers: x and y for every grid point.
    Count {
        wanted: usize,
        got: usize,
    },
    NotFinite,
    /// A point outside the box four monitors wide and high centred on the
    /// pane's monitor.
    TooBig,
    /// At progress 0 the window is not where it is.
    MovesAtRest,
    /// At load, the fastest of the checks' calls took `took`, more than a
    /// frame gives a `mesh` ([`Budget::MESH`]).
    Slow {
        took: Duration,
    },
}

/// What one `mesh` call is told.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Ask<'a> {
    pub(crate) progress: f64,
    pub(crate) clamped: f64,
    pub(crate) direction: f64,
    pub(crate) axis: Axis,
    /// The subject's rectangle, global logical.
    pub(crate) from: Rect,
    /// The target's, resolved this pass; `from` when there is none.
    pub(crate) to: Option<Rect>,
    pub(crate) part: UnitRect,
    pub(crate) seed: f64,
    /// The pane's monitor, global logical: the checks' box and `t.monitor`.
    pub(crate) monitor: Rect,
    pub(crate) scale: f64,
    pub(crate) params: &'a [(String, Value)],
}

/// One `mesh` call once a pass, under the per-frame budget, checked.
/// `tests::a_mesh_that_writes_a_nan_or_too_few_points_or_too_far_is_refused`,
/// `tests::a_mesh_that_runs_forever_is_stopped_by_its_budget`.
pub(crate) fn mesh(
    sandbox: &Sandbox,
    ask: &Ask<'_>,
    cols: u32,
    rows: u32,
) -> Result<Vec<f64>, Refusal> {
    checked(sandbox, ask, cols, rows, Budget::MESH)
}

/// `t`'s table `name` (a rectangle, `part`, or a `vec4` param), made again
/// if the effect replaced it with something else
/// (`tests::t_s_tables_are_made_again_when_an_effect_replaces_them`).
fn rect_in(lua: &mlua::Lua, t: &mlua::Table, name: &str) -> mlua::Result<mlua::Table> {
    if let Some(table) = t.raw_get::<Option<mlua::Table>>(name).ok().flatten() {
        return Ok(table);
    }
    let table = lua.create_table_with_capacity(0, 4)?;
    t.raw_set(name, &table)?;
    Ok(table)
}

/// A `mesh` call under `limit`, and what it wrote checked.
fn checked(
    sandbox: &Sandbox,
    ask: &Ask<'_>,
    cols: u32,
    rows: u32,
    limit: Duration,
) -> Result<Vec<f64>, Refusal> {
    let points = sandbox
        .call_mesh(
            limit,
            |lua, t, _out| {
                let rect = |name: &str, r: Rect| -> mlua::Result<()> {
                    let table = rect_in(lua, t, name)?;
                    table.raw_set("x", r.x)?;
                    table.raw_set("y", r.y)?;
                    table.raw_set("w", r.w)?;
                    table.raw_set("h", r.h)
                };
                t.raw_set("progress", ask.progress)?;
                t.raw_set("clamped", ask.clamped)?;
                t.raw_set("direction", ask.direction)?;
                t.raw_set("axis", ask.axis.name())?;
                t.raw_set("seed", ask.seed)?;
                t.raw_set("scale", ask.scale)?;
                rect("from", ask.from)?;
                rect("to", ask.to.unwrap_or(ask.from))?;
                rect("monitor", ask.monitor)?;
                let part = rect_in(lua, t, "part")?;
                part.raw_set("u0", ask.part.u0)?;
                part.raw_set("v0", ask.part.v0)?;
                part.raw_set("u1", ask.part.u1)?;
                part.raw_set("v1", ask.part.v1)?;
                // Every param (\[16\] §2: "`t` carries every param"):
                // `tests::every_param_reaches_t`. A `vec4` fills a table made
                // once and kept in `t`, so a pass allocates nothing.
                for (name, value) in ask.params {
                    match value {
                        Value::Number(number) => t.raw_set(name.as_str(), *number)?,
                        Value::Int(int) => t.raw_set(name.as_str(), *int)?,
                        Value::Bool(yes) => t.raw_set(name.as_str(), *yes)?,
                        Value::Word(word) => t.raw_set(name.as_str(), word.as_str())?,
                        Value::Vec4(four) => {
                            let table = rect_in(lua, t, name)?;
                            for (index, each) in four.iter().enumerate() {
                                table.raw_set(index + 1, *each)?;
                            }
                        }
                    }
                }
                Ok(())
            },
            cols,
            rows,
        )
        .map_err(|problem| {
            if sandbox.poisoned() {
                Refusal::Budget
            } else {
                Refusal::Error {
                    line: problem.line,
                    message: problem.message,
                }
            }
        })?;
    let wanted = numbers(cols, rows);
    if points.len() != wanted {
        return Err(Refusal::Count {
            wanted,
            got: points.len(),
        });
    }
    if points.iter().any(|point| !point.is_finite()) {
        return Err(Refusal::NotFinite);
    }
    // A box four monitors wide and high centred on the pane's monitor, one
    // and a half of it beyond each edge:
    // `tests::the_box_is_four_monitors_wide_and_high_around_the_panes_monitor`.
    let (mx, my, mw, mh) = (ask.monitor.x, ask.monitor.y, ask.monitor.w, ask.monitor.h);
    let inside = points.as_chunks::<2>().0.iter().all(|&[x, y]| {
        x >= mx - 1.5 * mw && x <= mx + 2.5 * mw && y >= my - 1.5 * mh && y <= my + 2.5 * mh
    });
    if !inside {
        return Err(Refusal::TooBig);
    }
    Ok(points)
}

/// A part of the window the checks at load draw at rest beside the whole of
/// it: in from each edge by a different amount, so a file that places
/// points from `c / cols` instead of the part, or reads one side's bound for
/// another's, moves them, and would squeeze a part reaching past the window
/// (its popups) onto it
/// (`tests::a_file_that_ignores_its_part_is_refused_at_load`). Inside the
/// window and not past it, because at progress 0 a geometry rests only
/// there: the genie has begun to pull what lies beyond its leading edge (as
/// the Rust genie it is held to has). A sample, as the rectangles beside it
/// are, not a behaviour.
const INNER: UnitRect = UnitRect {
    u0: 0.25,
    v0: 0.125,
    u1: 0.875,
    v1: 0.5,
};

/// The load-time contract: at progress 0 the file draws the window exactly
/// where it is, on all four axes and in every direction (+1 arriving, −1
/// leaving, 0 resizing), within 1e-9, over the window and over a part of it
/// ([`INNER`]); at progress 1 it writes a valid grid.
/// `tests::a_geometry_file_that_moves_the_window_at_progress_zero_is_refused_at_load`,
/// `tests::a_file_that_moves_the_window_arriving_is_refused_at_load`,
/// `tests::a_file_that_moves_the_window_resizing_is_refused_at_load`,
/// `tests::a_file_that_moves_the_window_on_one_axis_is_refused_at_load`,
/// `tests::a_file_whose_grid_at_progress_one_is_not_finite_is_refused_at_load`,
/// `tests::a_file_that_ignores_its_part_is_refused_at_load`.
///
/// Its 36 calls share one load budget, the sandbox's
/// `effects.sandbox.load_ms` (100 ms by default), so a load holds the thread
/// no longer than one load-time call
/// (`tests::the_checks_at_load_share_one_budget`,
/// `tests::the_checks_at_load_read_the_configured_load_budget`); and a version whose
/// fastest call is longer than a frame gives a `mesh` is refused, since
/// every frame would stop it. The fastest, because one call is slow when
/// the machine is busy, and 36 are slow only when the file is
/// (`tests::a_mesh_too_slow_for_a_frame_is_refused_at_load`).
pub(crate) fn at_rest(
    sandbox: &Sandbox,
    grid: GridSpec,
    params: &[(String, Value)],
) -> Result<(), Refusal> {
    let from = Rect::new(100.0, 100.0, 400.0, 300.0);
    let mut clock = Checks {
        deadline: Instant::now() + sandbox.caps().load,
        fastest: None,
    };
    for (_, axis) in Axis::all() {
        let (cols, rows) = turned(grid, axis);
        for direction in [1.0, -1.0, 0.0] {
            let mut ask = Ask {
                progress: 0.0,
                clamped: 0.0,
                direction,
                axis,
                from,
                to: Some(Rect::new(900.0, 700.0, 40.0, 40.0)),
                part: UnitRect::WHOLE,
                seed: 0.0,
                monitor: Rect::new(0.0, 0.0, 1920.0, 1080.0),
                scale: 1.0,
                params,
            };
            for part in [UnitRect::WHOLE, INNER] {
                ask.part = part;
                let points = clock.call(sandbox, &ask, cols, rows)?;
                let mut at = points.as_chunks::<2>().0.iter();
                for r in 0..=rows {
                    for c in 0..=cols {
                        let (x, y) = from.at(
                            part.u0 + (part.u1 - part.u0) * (f64::from(c) / f64::from(cols)),
                            part.v0 + (part.v1 - part.v0) * (f64::from(r) / f64::from(rows)),
                        );
                        let Some(&[px, py]) = at.next() else {
                            return Err(Refusal::MovesAtRest);
                        };
                        if (px - x).abs() > 1e-9 || (py - y).abs() > 1e-9 {
                            return Err(Refusal::MovesAtRest);
                        }
                    }
                }
            }
            ask.part = UnitRect::WHOLE;
            ask.progress = 1.0;
            ask.clamped = 1.0;
            clock.call(sandbox, &ask, cols, rows)?;
        }
    }
    clock.slow().map_or(Ok(()), Err)
}

/// The clock of [`at_rest`]'s calls: the one deadline they share, and the
/// fastest of them so far.
#[derive(Debug)]
struct Checks {
    deadline: Instant,
    fastest: Option<Duration>,
}

impl Checks {
    /// One call under what is left of the deadline, timed. A call the
    /// deadline stops is refused as too slow for a frame when every call
    /// before it was, and as stopped otherwise
    /// (`tests::the_checks_at_load_share_one_budget`).
    fn call(
        &mut self,
        sandbox: &Sandbox,
        ask: &Ask<'_>,
        cols: u32,
        rows: u32,
    ) -> Result<Vec<f64>, Refusal> {
        let started = Instant::now();
        let left = self.deadline.saturating_duration_since(started);
        if left.is_zero() {
            return Err(self.slow().unwrap_or(Refusal::Budget));
        }
        let result = checked(sandbox, ask, cols, rows, left);
        if matches!(result, Err(Refusal::Budget)) {
            return Err(self.slow().unwrap_or(Refusal::Budget));
        }
        let took = started.elapsed();
        self.fastest = Some(self.fastest.map_or(took, |fastest| fastest.min(took)));
        result
    }

    /// Refused as too slow for a frame, if the fastest call was.
    fn slow(&self) -> Option<Refusal> {
        self.fastest
            .filter(|&took| took > Budget::MESH)
            .map(|took| Refusal::Slow { took })
    }
}

/// A grid's columns and rows for an axis: a turning grid runs `along` the
/// sweep, as `Deform::segments` does. `tests::the_grid_turns_with_the_axis`.
pub(crate) fn turned(grid: GridSpec, axis: Axis) -> (u32, u32) {
    match grid {
        GridSpec::Fixed { cols, rows } => (cols.max(1), rows.max(1)),
        GridSpec::Turning { along, across } if axis.horizontal() => (along.max(1), across.max(1)),
        GridSpec::Turning { along, across } => (across.max(1), along.max(1)),
    }
}

/// The side of `from` that `to`'s centre lies on, for `axis = "auto"`,
/// picked once when a flight starts: the larger of the two offsets, each
/// over the window's half-size; vertical on a tie; `down` when the target's
/// centre is inside, the window's edge included.
/// `tests::auto_picks_the_side_the_target_lies_on`.
pub(crate) fn auto_axis(from: Rect, to: Rect) -> Axis {
    let (cx, cy) = (from.x + from.w / 2.0, from.y + from.h / 2.0);
    let (tx, ty) = (to.x + to.w / 2.0, to.y + to.h / 2.0);
    let (dx, dy) = (
        (tx - cx) / (from.w / 2.0).max(1.0),
        (ty - cy) / (from.h / 2.0).max(1.0),
    );
    if dx.abs() <= 1.0 && dy.abs() <= 1.0 {
        return Axis::Down;
    }
    if dy.abs() >= dx.abs() {
        if dy >= 0.0 { Axis::Down } else { Axis::Up }
    } else if dx > 0.0 {
        Axis::Right
    } else {
        Axis::Left
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use solium_effects::{Axis, Rect};

    use super::{Ask, Refusal, at_rest, mesh};
    use crate::effect::host::tests::{folder, scratch};
    use crate::effect::sandbox::Sandbox;
    use crate::warp::UnitRect;

    fn loaded(name: &str, lua: &str) -> (Sandbox, std::path::PathBuf) {
        let place = scratch(&format!("geometry-{name}"));
        let dir = folder(&place, name, lua, &[]);
        let mut sandbox = Sandbox::new(name, &dir.join("effect.lua")).expect("a sandbox");
        sandbox.load_effect().expect("loads");
        (sandbox, place)
    }

    /// One call's question, a closing window halfway to a corner.
    pub(crate) fn ask() -> Ask<'static> {
        Ask {
            progress: 0.5,
            clamped: 0.5,
            direction: -1.0,
            axis: Axis::Down,
            from: Rect::new(100.0, 100.0, 400.0, 300.0),
            to: Some(Rect::new(900.0, 700.0, 40.0, 40.0)),
            part: UnitRect::WHOLE,
            seed: 0.25,
            monitor: Rect::new(0.0, 0.0, 1920.0, 1080.0),
            scale: 1.0,
            params: &[],
        }
    }

    /// A geometry file that draws the window where it is, at any progress.
    pub(crate) const FLAT: &str = "return { api = 1, grid = { 1, 1 }, mesh = function(t, cols, rows, out)
        local n = 0
        for r = 0, rows do for c = 0, cols do local u, v = sol_grid(t, c, r)
            out[n + 1], out[n + 2], n = t.from.x + u * t.from.w, t.from.y + v * t.from.h, n + 2 end end end }";

    /// **An effect's Lua has `math` and the prelude, and no `sol`.**
    #[test]
    fn an_effects_lua_has_math_and_the_prelude_and_no_sol() {
        let (sandbox, place) = loaded(
            "prelude",
            "assert(math.sin and sol_phase and sol_grid and sol == nil) return { api = 1, grid = { 1, 1 }, mesh = function() end }",
        );
        let phase: f64 = sandbox
            .lua()
            .globals()
            .get::<mlua::Function>("sol_phase")
            .expect("sol_phase")
            .call((0.2, 0.7, "down"))
            .expect("a call");
        assert!((phase - 0.3).abs() < 1e-12, "{phase}");
        let _ = std::fs::remove_dir_all(place);
    }

    /// A geometry file that spins while the global `spin` is set, which a
    /// test sets after load so the load itself returns.
    pub(crate) const SPIN: &str = "spin = false return { api = 1, grid = { 1, 1 }, mesh = function(t, cols, rows, out)
            while spin do end
            local n = 0 for r = 0, rows do for c = 0, cols do local u, v = sol_grid(t, c, r)
            out[n + 1], out[n + 2], n = t.from.x + u * t.from.w, t.from.y + v * t.from.h, n + 2 end end end }";

    /// **A mesh that runs forever is stopped by its budget**, and its state
    /// runs nothing more until it is rebuilt (Ruling 4: mlua 0.12.1 leaves
    /// the error in the stopped frame's locals;
    /// `a_stopped_state_is_rebuilt_after_the_frame`).
    #[test]
    fn a_mesh_that_runs_forever_is_stopped_by_its_budget() {
        let (sandbox, place) = loaded("forever", SPIN);
        sandbox.lua().globals().set("spin", true).expect("set");
        let started = std::time::Instant::now();
        let refused = mesh(&sandbox, &ask(), 1, 1);
        assert!(
            started.elapsed() < std::time::Duration::from_millis(50),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(refused, Err(Refusal::Budget));
        sandbox.lua().globals().set("spin", false).expect("set");
        assert_eq!(
            mesh(&sandbox, &ask(), 1, 1),
            Err(Refusal::Budget),
            "a stopped state ran again"
        );
        assert!(sandbox.poisoned());
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A state stopped by its budget is rebuilt after the frame** (Ruling
    /// 4): a 2 ms overrun can be preemption or a GC step, not a bad file, so
    /// the next call on the rebuilt state draws and the effect keeps its id;
    /// the pane that overran keeps the poisoned state it holds, and fades.
    #[test]
    fn a_stopped_state_is_rebuilt_after_the_frame() {
        use crate::effect::host::tests::Counting;
        let place = scratch("revive");
        folder(&place, "spin", SPIN, &[]);
        let mut host: crate::effect::host::Host<u32> = crate::effect::host::Host::new(
            crate::effect::host::Library::with(Some(place.clone()), place.join("none")),
        );
        host.want("present", ["spin".to_owned()]);
        host.compile_pending(&mut Counting::default());
        let id = host.id("spin").expect("current");
        let stopped = host.effect("spin").expect("loaded");
        stopped
            .sandbox()
            .lua()
            .globals()
            .set("spin", true)
            .expect("set");
        assert_eq!(mesh(stopped.sandbox(), &ask(), 1, 1), Err(Refusal::Budget));
        host.revive();
        let fresh = host.effect("spin").expect("still loaded");
        assert!(!fresh.sandbox().poisoned());
        assert!(
            mesh(fresh.sandbox(), &ask(), 1, 1).is_ok(),
            "the next call after the rebuild did not draw"
        );
        assert_eq!(host.id("spin"), Some(id), "a rebuild is not a new version");
        assert!(
            mesh(stopped.sandbox(), &ask(), 1, 1).is_err(),
            "the pane that overran holds its own state"
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// **`t` carries every param** (\[16\] §2): a number, an integer, a
    /// boolean, four numbers as a table and a word as a string, so a file can
    /// branch on a word param; and `t.monitor` is the pane's monitor.
    #[test]
    fn every_param_reaches_t() {
        use solium_effects::spec::Value;
        let lua = "return { api = 1, grid = { 1, 1 },
            params = { k = { 1 }, n = { 2, int = true }, on = { true }, c = { { 1, 0, 0, 1 } }, mode = { 'fold' } },
            mesh = function(t, cols, rows, out)
                assert(t.k == 1 and t.n == 2 and t.on == true and t.c[1] == 1 and t.c[4] == 1 and t.mode == 'fold', 'a param did not reach t')
                assert(t.monitor.w == 1920 and t.monitor.h == 1080, 't.monitor is not the monitor')
                local n = 0 for r = 0, rows do for c = 0, cols do local u, v = sol_grid(t, c, r)
                out[n + 1], out[n + 2], n = t.from.x + u * t.from.w, t.from.y + v * t.from.h, n + 2 end end end }";
        let (sandbox, place) = loaded("params", lua);
        let params = [
            ("c".to_owned(), Value::Vec4([1.0, 0.0, 0.0, 1.0])),
            ("k".to_owned(), Value::Number(1.0)),
            ("mode".to_owned(), Value::Word("fold".to_owned())),
            ("n".to_owned(), Value::Int(2)),
            ("on".to_owned(), Value::Bool(true)),
        ];
        let asked = Ask {
            params: &params,
            ..ask()
        };
        assert!(
            mesh(&sandbox, &asked, 1, 1).is_ok(),
            "{:?}",
            mesh(&sandbox, &asked, 1, 1)
        );
        assert!(
            mesh(&sandbox, &asked, 1, 1).is_ok(),
            "the second call, on the reused tables"
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// **`t`'s tables are made again when an effect replaces them**: a file
    /// that writes over `t.from` or a `vec4` param's table is told them as
    /// tables on its next call, not refused for ever.
    #[test]
    fn t_s_tables_are_made_again_when_an_effect_replaces_them() {
        use solium_effects::spec::Value;
        let lua = "return { api = 1, grid = { 1, 1 }, params = { c = { { 1, 0, 0, 1 } } },
            mesh = function(t, cols, rows, out)
                local n = 0 for r = 0, rows do for c = 0, cols do local u, v = sol_grid(t, c, r)
                out[n + 1], out[n + 2], n = t.from.x + u * t.from.w + t.c[4] - 1, t.from.y + v * t.from.h, n + 2 end end
                t.from, t.c = 1, 'gone' end }";
        let (sandbox, place) = loaded("replaces", lua);
        let params = [("c".to_owned(), Value::Vec4([1.0, 0.0, 0.0, 1.0]))];
        let asked = Ask {
            params: &params,
            ..ask()
        };
        assert!(mesh(&sandbox, &asked, 1, 1).is_ok());
        assert!(
            mesh(&sandbox, &asked, 1, 1).is_ok(),
            "{:?}",
            mesh(&sandbox, &asked, 1, 1)
        );
        let _ = std::fs::remove_dir_all(place);
    }

    #[test]
    fn a_mesh_that_errors_is_refused_with_its_line() {
        let (sandbox, place) = loaded(
            "errors",
            "return { api = 1, grid = { 1, 1 },\n mesh = function(t, cols, rows, out)\n  local x = t.nothing.here\n end }",
        );
        assert!(matches!(
            mesh(&sandbox, &ask(), 1, 1),
            Err(Refusal::Error { line: Some(3), .. })
        ));
        let _ = std::fs::remove_dir_all(place);
    }

    #[test]
    fn a_mesh_that_writes_a_nan_or_too_few_points_or_too_far_is_refused() {
        let (nan, a) = loaded(
            "nan",
            "return { api = 1, grid = { 1, 1 }, mesh = function(t, cols, rows, out) for i = 1, 8 do out[i] = 0/0 end end }",
        );
        assert_eq!(mesh(&nan, &ask(), 1, 1), Err(Refusal::NotFinite));
        let (few, b) = loaded(
            "few",
            "return { api = 1, grid = { 1, 1 }, mesh = function(t, cols, rows, out) out[1], out[2] = 0, 0 end }",
        );
        assert_eq!(
            mesh(&few, &ask(), 1, 1),
            Err(Refusal::Count { wanted: 8, got: 2 })
        );
        let (far, c) = loaded(
            "far",
            "return { api = 1, grid = { 1, 1 }, mesh = function(t, cols, rows, out) for i = 1, 8 do out[i] = 1e7 end end }",
        );
        assert_eq!(mesh(&far, &ask(), 1, 1), Err(Refusal::TooBig));
        for place in [a, b, c] {
            let _ = std::fs::remove_dir_all(place);
        }
    }

    /// The `out` table is reused and cleared: a short write after a full one
    /// is still refused.
    #[test]
    fn the_out_table_is_cleared_between_calls() {
        let (sandbox, place) = loaded(
            "short-after",
            "full = true return { api = 1, grid = { 1, 1 }, mesh = function(t, cols, rows, out)
            if full then for i = 1, 8 do out[i] = 100 end else out[1], out[2] = 100, 100 end end }",
        );
        assert!(mesh(&sandbox, &ask(), 1, 1).is_ok());
        sandbox.lua().globals().set("full", false).expect("set");
        assert!(matches!(
            mesh(&sandbox, &ask(), 1, 1),
            Err(Refusal::Count { .. })
        ));
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A grid no Lua could fill is refused, not allocated**: the count is
    /// worked out without overflowing and nothing is sized by it, so a
    /// `grid` of four billion columns is a refusal, not a crash or a hang.
    #[test]
    fn a_grid_no_lua_could_fill_is_refused_not_allocated() {
        let (sandbox, place) = loaded(
            "huge",
            "return { api = 1, grid = { 1, 1 }, mesh = function(t, cols, rows, out) end }",
        );
        let started = std::time::Instant::now();
        assert!(matches!(
            mesh(&sandbox, &ask(), u32::MAX, u32::MAX),
            Err(Refusal::Count { got: 0, .. })
        ));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A geometry file that moves the window at progress 0 is refused at
    /// load** (the rule Phase 0 Task 13's at-rest release depends on); a file
    /// that draws it where it is passes.
    #[test]
    fn a_geometry_file_that_moves_the_window_at_progress_zero_is_refused_at_load() {
        let fixtures = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/effects/mover/effect.lua"
        ));
        let mut mover = Sandbox::new("mover", fixtures).expect("a sandbox");
        let spec = mover.load_effect().expect("loads");
        assert_eq!(
            at_rest(&mover, spec.grid.expect("a grid"), &[]),
            Err(Refusal::MovesAtRest)
        );
        let (flat, place) = loaded("flat", FLAT);
        assert_eq!(
            at_rest(
                &flat,
                solium_effects::spec::GridSpec::Fixed { cols: 1, rows: 1 },
                &[]
            ),
            Ok(())
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A file that moves the window arriving is refused too**: an open
    /// ends at progress 0 with direction +1, so a file that branches on
    /// `t.direction` is held to its rest in every direction, or the window
    /// jumps when \[fx0\] Task 13 releases it at rest.
    #[test]
    fn a_file_that_moves_the_window_arriving_is_refused_at_load() {
        let lua = "return { api = 1, grid = { 1, 1 }, mesh = function(t, cols, rows, out)
            local n, shift = 0, (t.direction > 0) and 1 or 0
            for r = 0, rows do for c = 0, cols do local u, v = sol_grid(t, c, r)
            out[n + 1], out[n + 2], n = t.from.x + u * t.from.w + shift, t.from.y + v * t.from.h, n + 2 end end end }";
        let (arriving, place) = loaded("arriving", lua);
        assert_eq!(
            at_rest(
                &arriving,
                solium_effects::spec::GridSpec::Fixed { cols: 1, rows: 1 },
                &[]
            ),
            Err(Refusal::MovesAtRest)
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A file that ignores its part is refused at load**: one that places
    /// point `(c, r)` at the window's `(c / cols, r / rows)`, not at
    /// `sol_grid`'s, draws the whole window right and would squeeze a part
    /// that reaches past it (its popups) onto the window; so the checks at
    /// load draw a part of the window at rest too.
    #[test]
    fn a_file_that_ignores_its_part_is_refused_at_load() {
        let lua = "return { api = 1, grid = { 2, 2 }, mesh = function(t, cols, rows, out)
            local n = 0
            for r = 0, rows do for c = 0, cols do local u, v = c / cols, r / rows
            out[n + 1], out[n + 2], n = t.from.x + u * t.from.w, t.from.y + v * t.from.h, n + 2 end end end }";
        let (sandbox, place) = loaded("ignores-part", lua);
        let whole = Ask {
            progress: 0.0,
            clamped: 0.0,
            ..ask()
        };
        let drawn = mesh(&sandbox, &whole, 2, 2).expect("a grid");
        assert!(
            (drawn[2] - 300.0).abs() < 1e-9,
            "the premise: it draws the whole window where it is"
        );
        assert_eq!(
            at_rest(
                &sandbox,
                solium_effects::spec::GridSpec::Fixed { cols: 2, rows: 2 },
                &[]
            ),
            Err(Refusal::MovesAtRest)
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// A grid of one cell, as the checks at load are given it.
    const ONE: solium_effects::spec::GridSpec =
        solium_effects::spec::GridSpec::Fixed { cols: 1, rows: 1 };

    /// **A file that moves the window resizing is refused too**: a size
    /// change runs with direction 0, and lands at rest as an open does.
    #[test]
    fn a_file_that_moves_the_window_resizing_is_refused_at_load() {
        let lua = "return { api = 1, grid = { 1, 1 }, mesh = function(t, cols, rows, out)
            local n, shift = 0, (t.direction == 0) and 1 or 0
            for r = 0, rows do for c = 0, cols do local u, v = sol_grid(t, c, r)
            out[n + 1], out[n + 2], n = t.from.x + u * t.from.w + shift, t.from.y + v * t.from.h, n + 2 end end end }";
        let (resizing, place) = loaded("resizing", lua);
        assert_eq!(at_rest(&resizing, ONE, &[]), Err(Refusal::MovesAtRest));
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A file that moves the window on one axis alone is refused**, on
    /// each of the four: a genie pulled left must rest where one pulled
    /// down does.
    #[test]
    fn a_file_that_moves_the_window_on_one_axis_is_refused_at_load() {
        let lua = "moves_on = nil return { api = 1, grid = { 1, 1 }, mesh = function(t, cols, rows, out)
            local n, shift = 0, (t.axis == moves_on) and 1 or 0
            for r = 0, rows do for c = 0, cols do local u, v = sol_grid(t, c, r)
            out[n + 1], out[n + 2], n = t.from.x + u * t.from.w, t.from.y + v * t.from.h + shift, n + 2 end end end }";
        let (sandbox, place) = loaded("one-axis", lua);
        assert_eq!(at_rest(&sandbox, ONE, &[]), Ok(()), "it moves on no axis");
        for (name, _) in Axis::all() {
            sandbox.lua().globals().set("moves_on", name).expect("set");
            assert_eq!(
                at_rest(&sandbox, ONE, &[]),
                Err(Refusal::MovesAtRest),
                "moving on {name} alone passed"
            );
        }
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A file whose grid at progress 1 is not finite is refused at
    /// load**: progress 1 is called as well as 0, and checked as every
    /// frame's call is.
    #[test]
    fn a_file_whose_grid_at_progress_one_is_not_finite_is_refused_at_load() {
        let lua = "return { api = 1, grid = { 1, 1 }, mesh = function(t, cols, rows, out)
            local n, off = 0, (t.progress == 1) and 0/0 or 0
            for r = 0, rows do for c = 0, cols do local u, v = sol_grid(t, c, r)
            out[n + 1], out[n + 2], n = t.from.x + u * t.from.w + off, t.from.y + v * t.from.h, n + 2 end end end }";
        let (sandbox, place) = loaded("far-end", lua);
        assert_eq!(at_rest(&sandbox, ONE, &[]), Err(Refusal::NotFinite));
        let _ = std::fs::remove_dir_all(place);
    }

    /// A flat file whose `mesh` first calls `wait(ms)`, a Rust function
    /// that sleeps: a call that takes as long as a test wants, on any
    /// machine.
    fn waiting(name: &str, ms: u64) -> (Sandbox, std::path::PathBuf) {
        let lua = format!(
            "return {{ api = 1, grid = {{ 1, 1 }}, mesh = function(t, cols, rows, out)
            wait({ms})
            local n = 0 for r = 0, rows do for c = 0, cols do local u, v = sol_grid(t, c, r)
            out[n + 1], out[n + 2], n = t.from.x + u * t.from.w, t.from.y + v * t.from.h, n + 2 end end end }}"
        );
        let place = scratch(&format!("geometry-{name}"));
        let dir = folder(&place, name, &lua, &[]);
        let mut sandbox = Sandbox::new(name, &dir.join("effect.lua")).expect("a sandbox");
        let wait = sandbox
            .lua()
            .create_function(|_, ms: u64| {
                std::thread::sleep(std::time::Duration::from_millis(ms));
                Ok(())
            })
            .expect("a function");
        sandbox.lua().globals().set("wait", wait).expect("set");
        sandbox.load_effect().expect("loads");
        (sandbox, place)
    }

    /// [`waiting`]'s file made with `caps`, whose `mesh` waits `ms` on its
    /// first call only: the checks at load share one deadline, so one slow
    /// call is what a slow check costs, and the fast calls after it keep the
    /// fastest under a frame's budget.
    fn waiting_once(
        name: &str,
        ms: u64,
        caps: crate::effect::settings::Caps,
    ) -> (Sandbox, std::path::PathBuf) {
        let lua = format!(
            "local waited = false
            return {{ api = 1, grid = {{ 1, 1 }}, mesh = function(t, cols, rows, out)
            if not waited then waited = true wait({ms}) end
            local n = 0 for r = 0, rows do for c = 0, cols do local u, v = sol_grid(t, c, r)
            out[n + 1], out[n + 2], n = t.from.x + u * t.from.w, t.from.y + v * t.from.h, n + 2 end end end }}"
        );
        let place = scratch(&format!("geometry-{name}"));
        let dir = folder(&place, name, &lua, &[]);
        let mut sandbox =
            Sandbox::with_caps(name, &dir.join("effect.lua"), caps).expect("a sandbox");
        let wait = sandbox
            .lua()
            .create_function(|_, ms: u64| {
                std::thread::sleep(std::time::Duration::from_millis(ms));
                Ok(())
            })
            .expect("a function");
        sandbox.lua().globals().set("wait", wait).expect("set");
        sandbox.load_effect().expect("loads");
        (sandbox, place)
    }

    /// **The checks at load share `effects.sandbox.load_ms`**: a geometry
    /// file whose first check at load takes 150 ms is stopped under the
    /// default 100, and passes under `load_ms = 500`.
    #[test]
    fn the_checks_at_load_read_the_configured_load_budget() {
        let (sandbox, place) =
            waiting_once("slow-once", 150, crate::effect::settings::Caps::default());
        assert_eq!(
            at_rest(&sandbox, ONE, &[]),
            Err(Refusal::Budget),
            "150 ms passed under the default 100"
        );
        let _ = std::fs::remove_dir_all(place);
        let caps = crate::effect::settings::Caps {
            load: std::time::Duration::from_millis(500),
            ..crate::effect::settings::Caps::default()
        };
        let (sandbox, place) = waiting_once("slow-once-raised", 150, caps);
        assert_eq!(
            at_rest(&sandbox, ONE, &[]),
            Ok(()),
            "refused under load_ms = 500"
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A mesh too slow for a frame is refused at load**: every frame would
    /// stop it, and Ruling 4's rebuild would then load the folder again
    /// after each one, with nothing on the overlay. A call of 3 ms is past
    /// the 2 ms a frame gives.
    #[test]
    fn a_mesh_too_slow_for_a_frame_is_refused_at_load() {
        let (sandbox, place) = waiting("slow", 3);
        match at_rest(&sandbox, ONE, &[]) {
            Err(Refusal::Slow { took }) => {
                assert!(took >= std::time::Duration::from_millis(3), "{took:?}");
            }
            other => panic!("a 3 ms mesh was not refused as slow: {other:?}"),
        }
        let _ = std::fs::remove_dir_all(place);
    }

    /// **The checks at load share one budget**: 36 calls of 20 ms each are
    /// stopped at the load budget's 100 ms, not run for 720, so a load holds
    /// the compositor's thread no longer than one load-time call; and since
    /// every call before the stop was too slow for a frame, that is the
    /// refusal.
    #[test]
    fn the_checks_at_load_share_one_budget() {
        let (sandbox, place) = waiting("slower", 20);
        let started = std::time::Instant::now();
        let refused = at_rest(&sandbox, ONE, &[]);
        assert!(
            started.elapsed() < std::time::Duration::from_millis(200),
            "{:?}",
            started.elapsed()
        );
        assert!(
            matches!(refused, Err(Refusal::Slow { took }) if took >= std::time::Duration::from_millis(20)),
            "{refused:?}"
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// **The box is four monitors wide and four high, centred on the pane's
    /// monitor**: one and a half of it beyond each of its edges, on a
    /// monitor away from the origin.
    #[test]
    fn the_box_is_four_monitors_wide_and_high_around_the_panes_monitor() {
        let (sandbox, place) = loaded(
            "edges",
            "px, py = 0, 0 return { api = 1, grid = { 1, 1 }, mesh = function(t, cols, rows, out)
                for i = 1, 8, 2 do out[i], out[i + 1] = px, py end end }",
        );
        let asked = Ask {
            monitor: Rect::new(1920.0, 100.0, 2560.0, 1440.0),
            ..ask()
        };
        let (left, right, top, bottom) = (-1920.0, 8320.0, -2060.0, 3700.0);
        let (x, y) = (3200.0, 820.0);
        for (px, py, inside) in [
            (left, y, true),
            (left - 1.0, y, false),
            (right, y, true),
            (right + 1.0, y, false),
            (x, top, true),
            (x, top - 1.0, false),
            (x, bottom, true),
            (x, bottom + 1.0, false),
        ] {
            let globals = sandbox.lua().globals();
            globals.set("px", px).expect("set");
            globals.set("py", py).expect("set");
            let got = mesh(&sandbox, &asked, 1, 1);
            if inside {
                assert!(got.is_ok(), "({px}, {py}) is inside: {got:?}");
            } else {
                assert_eq!(got, Err(Refusal::TooBig), "({px}, {py}) is outside");
            }
        }
        let _ = std::fs::remove_dir_all(place);
    }

    /// The shipped `effects/genie/`, loaded as the compositor loads it.
    fn shipped_genie() -> Sandbox {
        let file = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/effects/genie/effect.lua"
        ));
        let mut sandbox = Sandbox::new("genie", file).expect("a sandbox");
        sandbox.load_effect().expect("the shipped genie loads");
        sandbox
    }

    /// **The genie folder matches `Deform::Genie` on all four axes**, grid
    /// included, within 1e-9, the late start of the rows furthest from the
    /// target included. Every number handed to Lua is the `f32` Rust
    /// computes from, widened (Ruling 19).
    #[test]
    fn the_genie_folder_matches_deform_genie_on_all_four_axes() {
        let genie = shipped_genie();
        let (from, to) = (
            Rect::new(100.0, 50.0, 800.0, 600.0),
            Rect::new(600.0, 1000.0, 64.0, 32.0),
        );
        for (_, axis) in Axis::all() {
            for spread in [0.0_f32, 0.5, 1.4, 4.0] {
                for progress in [0.0_f32, 0.13, 0.37, 0.5, 0.99, 1.0, 1.1, -0.2] {
                    let rust = solium_effects::Deform::Genie {
                        progress,
                        spread,
                        axis,
                    };
                    let (cols, rows) = super::turned(
                        solium_effects::spec::GridSpec::Turning {
                            along: 48,
                            across: 8,
                        },
                        axis,
                    );
                    assert_eq!(
                        (cols, rows),
                        rust.segments(),
                        "the turned grid is Deform::segments"
                    );
                    let params = [(
                        "spread".to_owned(),
                        solium_effects::spec::Value::Number(f64::from(spread)),
                    )];
                    let ask = Ask {
                        progress: f64::from(progress),
                        clamped: f64::from(progress).clamp(0.0, 1.0),
                        direction: -1.0,
                        axis,
                        from,
                        to: Some(to),
                        part: UnitRect::WHOLE,
                        seed: 0.0,
                        monitor: Rect::new(0.0, 0.0, 2560.0, 1440.0),
                        scale: 1.0,
                        params: &params,
                    };
                    let lua = mesh(&genie, &ask, cols, rows).expect("a grid");
                    let mut index = 0;
                    for r in 0..=rows {
                        for c in 0..=cols {
                            let (x, y) = rust.place(
                                from,
                                to,
                                f64::from(c) / f64::from(cols),
                                f64::from(r) / f64::from(rows),
                            );
                            assert!(
                                (lua[index] - x).abs() <= 1e-9
                                    && (lua[index + 1] - y).abs() <= 1e-9,
                                "{axis:?} spread {spread} progress {progress} at ({c}, {r}): lua ({}, {}) rust ({x}, {y})",
                                lua[index],
                                lua[index + 1]
                            );
                            index += 2;
                        }
                    }
                }
            }
        }
    }

    /// **Past the window**, on all four axes: parts of (−0.1, −0.1)–(1.1, 1.1),
    /// (0.8, 0.8)–(1.4, 1.3) and (0.8, −0.1)–(1.4, 1.1) land where
    /// `mesh_part`'s Rust genie puts them, over the turned grid. The last two
    /// reach past the window differently across and along, so a sideways
    /// sweep that read the part's rows for its columns, or columns placed
    /// without the part, would bend the wrong points (a side dock's shadow or
    /// popup).
    #[test]
    fn the_genie_folder_matches_past_the_window() {
        let genie = shipped_genie();
        let (from, to) = (
            Rect::new(100.0, 50.0, 800.0, 600.0),
            Rect::new(600.0, 1000.0, 64.0, 32.0),
        );
        let parts = [
            UnitRect {
                u0: -0.1,
                v0: -0.1,
                u1: 1.1,
                v1: 1.1,
            },
            UnitRect {
                u0: 0.8,
                v0: 0.8,
                u1: 1.4,
                v1: 1.3,
            },
            UnitRect {
                u0: 0.8,
                v0: -0.1,
                u1: 1.4,
                v1: 1.1,
            },
        ];
        for (_, axis) in Axis::all() {
            let (cols, rows) = super::turned(
                solium_effects::spec::GridSpec::Turning {
                    along: 48,
                    across: 8,
                },
                axis,
            );
            for spread in [0.5_f32, 1.4, 4.0] {
                for progress in [0.37_f32, 0.99] {
                    let rust = solium_effects::Deform::Genie {
                        progress,
                        spread,
                        axis,
                    };
                    let params = [(
                        "spread".to_owned(),
                        solium_effects::spec::Value::Number(f64::from(spread)),
                    )];
                    for part in parts {
                        let ask = Ask {
                            progress: f64::from(progress),
                            clamped: f64::from(progress),
                            direction: -1.0,
                            axis,
                            from,
                            to: Some(to),
                            part,
                            seed: 0.0,
                            monitor: Rect::new(0.0, 0.0, 2560.0, 1440.0),
                            scale: 1.0,
                            params: &params,
                        };
                        let lua = mesh(&genie, &ask, cols, rows).expect("a grid");
                        let mut index = 0;
                        for r in 0..=rows {
                            for c in 0..=cols {
                                let u = part.u0
                                    + (part.u1 - part.u0) * (f64::from(c) / f64::from(cols));
                                let v = part.v0
                                    + (part.v1 - part.v0) * (f64::from(r) / f64::from(rows));
                                let (x, y) = rust.place(from, to, u, v);
                                assert!(
                                    (lua[index] - x).abs() <= 1e-9
                                        && (lua[index + 1] - y).abs() <= 1e-9,
                                    "{axis:?} spread {spread} progress {progress} part {part:?} at ({c}, {r}): lua ({}, {}) rust ({x}, {y})",
                                    lua[index],
                                    lua[index + 1]
                                );
                                index += 2;
                            }
                        }
                    }
                }
            }
        }
    }

    /// **`auto` picks the side the target lies on**: below is down, above is
    /// up, right is right, left is left; a target inside, down; a tie, the
    /// vertical side.
    #[test]
    fn auto_picks_the_side_the_target_lies_on() {
        let window = Rect::new(100.0, 100.0, 400.0, 200.0);
        let at = |x: f64, y: f64| super::auto_axis(window, Rect::new(x, y, 10.0, 10.0));
        assert_eq!(at(295.0, 900.0), Axis::Down);
        assert_eq!(at(295.0, -500.0), Axis::Up);
        assert_eq!(at(1500.0, 195.0), Axis::Right);
        assert_eq!(at(-900.0, 195.0), Axis::Left);
        assert_eq!(at(295.0, 195.0), Axis::Down, "inside");
        // Inside but off the centre: the target's centre (300, 120) is 0.8
        // of the half-height above, up by the offsets alone.
        assert_eq!(at(295.0, 115.0), Axis::Down, "inside, above the centre");
        // On the window's top-left corner (dx = dy = −1): the edge counts as
        // inside, or the tie would make it up.
        assert_eq!(at(95.0, 95.0), Axis::Down, "on the top-left corner, inside");
        assert_eq!(
            at(495.0, 295.0),
            Axis::Down,
            "on the diagonal, the vertical side"
        );
        // The corner above is still inside; past it, on the same diagonal,
        // the tie itself: below and right is down, above and left is up.
        assert_eq!(
            at(695.0, 395.0),
            Axis::Down,
            "past the corner on the diagonal, below"
        );
        assert_eq!(
            at(-105.0, -5.0),
            Axis::Up,
            "past the corner on the diagonal, above"
        );
    }

    /// The shipped genie's grid from `rect` toward `to` at `progress`, with
    /// `spread` as the Rust genie widens its `f32`: what
    /// `warp::tests::a_lua_grid_lands_where_the_rust_genie_put_it` projects.
    pub(crate) fn genie_grid(
        rect: smithay::utils::Rectangle<f64, smithay::utils::Logical>,
        to: smithay::utils::Rectangle<f64, smithay::utils::Logical>,
        progress: f32,
        spread: f32,
    ) -> crate::warp::Grid {
        let genie = shipped_genie();
        let (cols, rows) = super::turned(
            solium_effects::spec::GridSpec::Turning {
                along: 48,
                across: 8,
            },
            Axis::Down,
        );
        let params = [(
            "spread".to_owned(),
            solium_effects::spec::Value::Number(f64::from(spread)),
        )];
        let ask = Ask {
            progress: f64::from(progress),
            clamped: f64::from(progress),
            direction: -1.0,
            axis: Axis::Down,
            from: crate::present::for_effects(rect),
            to: Some(crate::present::for_effects(to)),
            part: UnitRect::WHOLE,
            seed: 0.0,
            monitor: Rect::new(0.0, 0.0, 3840.0, 1080.0),
            scale: 1.0,
            params: &params,
        };
        crate::warp::Grid {
            cols,
            rows,
            part: UnitRect::WHOLE,
            points: mesh(&genie, &ask, cols, rows).expect("a grid"),
        }
    }

    /// **A mesh is not rebuilt when neither progress nor an anchor changed**,
    /// and is when either does; a second piece's grid (a pane's popups) is
    /// kept beside the first, so the two do not rebuild each other every
    /// pass.
    #[test]
    fn a_mesh_is_not_rebuilt_when_neither_progress_nor_an_anchor_changed() {
        let mut meshes = super::Meshes::default();
        let mut built = 0;
        let key = super::MeshKey::for_test(0.5, Rect::new(0.0, 0.0, 10.0, 10.0));
        let grid = || crate::warp::Grid {
            cols: 1,
            rows: 1,
            part: UnitRect::WHOLE,
            points: vec![0.0; 8],
        };
        for _ in 0..2 {
            let _ = meshes.get_or_build(key, || {
                built += 1;
                Ok(grid())
            });
        }
        assert_eq!(built, 1);
        let _ = meshes.get_or_build(
            super::MeshKey::for_test(0.6, Rect::new(0.0, 0.0, 10.0, 10.0)),
            || {
                built += 1;
                Ok(grid())
            },
        );
        let _ = meshes.get_or_build(
            super::MeshKey::for_test(0.6, Rect::new(1.0, 0.0, 10.0, 10.0)),
            || {
                built += 1;
                Ok(grid())
            },
        );
        assert_eq!(built, 3);
        // Two pieces in turn, each pass: built once each.
        let over = super::MeshKey::for_test(0.6, Rect::new(2.0, 0.0, 10.0, 10.0));
        let pane = super::MeshKey::for_test(0.6, Rect::new(1.0, 0.0, 10.0, 10.0));
        for _ in 0..3 {
            for key in [pane, over] {
                let _ = meshes.get_or_build(key, || {
                    built += 1;
                    Ok(grid())
                });
            }
        }
        assert_eq!(built, 4, "the pane and its popups rebuilt each other");
        // A refusal keeps nothing, and is asked again.
        let refused = super::MeshKey::for_test(0.7, Rect::new(1.0, 0.0, 10.0, 10.0));
        for _ in 0..2 {
            assert_eq!(
                meshes.get_or_build(refused, || {
                    built += 1;
                    Err(Refusal::NotFinite)
                }),
                Err(Refusal::NotFinite)
            );
        }
        assert_eq!(built, 6);
        // A direction of 0 (a resize) is not +1 (an open), though `signum`
        // makes them one: the key holds the direction as it is.
        for direction in [0.0, 1.0] {
            let asked = Ask { direction, ..ask() };
            let key = super::MeshKey::new(
                crate::effect::host::EffectId::for_test(1),
                super::Params::default(),
                &asked,
                1,
                1,
            );
            let _ = meshes.get_or_build(key, || {
                built += 1;
                Ok(grid())
            });
        }
        assert_eq!(built, 8, "a resize's key was an open's");
    }

    /// **Params pack up to their limit and lerp only like with like**: a
    /// boolean is 0 or 1 and a `vec4` four numbers; past the limit, or a
    /// word, is refused naming why; two sets of as many lerp and of another
    /// count take the destination's; named again by the defaults' kinds.
    #[test]
    fn params_pack_up_to_their_limit_and_lerp_only_like_with_like() {
        use solium_effects::spec::Value;
        let bound = [
            ("a".to_owned(), Value::Number(1.5)),
            ("b".to_owned(), Value::Bool(true)),
            ("c".to_owned(), Value::Vec4([0.0, 0.25, 0.5, 1.0])),
            ("d".to_owned(), Value::Int(3)),
        ];
        let params = super::packed(&bound, 8).expect("seven numbers under eight");
        assert_eq!(params.as_slice(), [1.5, 1.0, 0.0, 0.25, 0.5, 1.0, 3.0]);
        assert_eq!(super::unpacked(&bound, &params), bound);
        let refused = super::packed(&bound, 6).expect_err("seven over six");
        assert!(
            refused.contains("7 numbers") && refused.contains("effects.limits.params"),
            "{refused}"
        );
        let word = [("mode".to_owned(), Value::Word("fold".to_owned()))];
        assert!(
            super::packed(&word, 8).is_err_and(|why| why.contains("`mode` is a word")),
            "a word was packed"
        );
        let other = super::Params::pack(&[3.5, 0.0, 1.0, 1.25, 1.5, 2.0, 5.0], 8).expect("packs");
        let half = params.lerp(other, 0.5);
        assert_eq!(half.as_slice(), [2.5, 0.5, 0.5, 0.75, 1.0, 1.5, 4.0]);
        assert_eq!(
            super::unpacked(&bound, &half)[1],
            ("b".to_owned(), Value::Bool(true)),
            "a boolean half way is true"
        );
        let fewer = super::Params::pack(&[9.0], 8).expect("packs");
        assert_eq!(
            params.lerp(fewer, 0.25),
            fewer,
            "another count blends nothing"
        );
        assert_eq!(
            super::Params::pack(&[0.0; 65], 64),
            Err(65),
            "never past PARAMS_MAX"
        );
    }

    /// **The grid turns with the axis** as `Deform::segments` does; a fixed
    /// grid does not.
    #[test]
    fn the_grid_turns_with_the_axis() {
        use solium_effects::spec::GridSpec;
        for (_, axis) in Axis::all() {
            let rust = solium_effects::Deform::Genie {
                progress: 1.0,
                spread: 1.0,
                axis,
            };
            assert_eq!(
                super::turned(
                    GridSpec::Turning {
                        along: 48,
                        across: 8
                    },
                    axis
                ),
                rust.segments()
            );
            assert_eq!(
                super::turned(GridSpec::Fixed { cols: 1, rows: 1 }, axis),
                (1, 1)
            );
        }
    }
}
