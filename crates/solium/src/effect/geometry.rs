//! Geometry as files (\[16\] decision 6, §2): an effect's `.vert` places
//! every point of its grid, run on the GPU once a pass and read back
//! ([`run`]), and Rust checks the grid before a vertex of it is drawn
//! (\[16\] §5): refused when it is not a whole grid of finite numbers inside
//! a box four monitors wide and high around the pane's
//! (`tests::a_grid_of_nan_too_few_points_or_too_far_is_refused`), and held,
//! once its program compiled, to drawing the window where it is at progress
//! 0 (`tests::a_mesh_that_moves_the_window_at_progress_zero_is_refused`).

use solium_effects::spec::{GridSpec, Value};
use solium_effects::{Axis, Rect};

use super::host::Compiler;
use crate::warp::UnitRect;

pub(crate) use super::params::{PARAMS_MAX, Params};

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
                    "its param `{name}` is a word, and a `present` blends its params between two presents, which a word cannot"
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
/// popups), and the version a `present` geometry began with, pinned
/// while `effects.present.on_reload = "keep"` holds it.
/// `tests::a_mesh_is_not_rebuilt_when_neither_progress_nor_an_anchor_changed`.
#[derive(Debug, Default)]
pub(crate) struct Meshes {
    last: Vec<(MeshKey, crate::warp::Grid)>,
    /// `state::tests::real_client::a_reload_mid_present_follows_on_reload`.
    pub(crate) pinned: Option<std::rc::Rc<super::host::Loaded>>,
    /// Per piece ([`crate::render::WARP_ORDER`]'s), whether its refusal has
    /// been said since its grid last built or the present began.
    said: [bool; crate::render::WARP_ORDER.len()],
    /// How many refusals were said, for a test to count.
    #[cfg(test)]
    pub(crate) warned: u32,
}

impl Meshes {
    /// Whether to say that piece `piece`'s grid was refused: the first
    /// time since that piece's grid last built or the present began, so a
    /// refused mesh is said once a pane and not every pass, and its popups'
    /// refusal is not said again because the pane's own grid built.
    /// `state::tests::real_client::a_present_refused_for_its_popups_alone_is_said_once`.
    pub(crate) fn say_refused(&mut self, piece: usize) -> bool {
        let first = self
            .said
            .get_mut(piece)
            .is_some_and(|said| !std::mem::replace(said, true));
        #[cfg(test)]
        if first {
            self.warned += 1;
        }
        first
    }

    /// Piece `piece`'s grid built: a later refusal of it is said again
    /// (`state::tests::real_client::a_present_refused_for_its_popups_alone_is_said_once`).
    pub(crate) fn built(&mut self, piece: usize) {
        if let Some(said) = self.said.get_mut(piece) {
            *said = false;
        }
    }

    /// A new present began: whatever it refuses is said again
    /// (`state::tests::real_client::a_present_refused_for_its_popups_alone_is_said_once`).
    pub(crate) fn begin(&mut self) {
        self.said = Default::default();
    }

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

/// Why a mesh's grid is not drawn.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Refusal {
    /// The GPU could not run it, or it has no program.
    Error {
        message: String,
    },
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
}

/// What one run of a mesh is told: its `sol_` uniforms, and its params.
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
    /// The pane's monitor, global logical: the checks' box and `sol_monitor`.
    pub(crate) monitor: Rect,
    pub(crate) scale: f64,
    pub(crate) params: &'a [(String, Value)],
}

impl Ask<'_> {
    /// Its uniforms as GL takes them, over a grid of `cols` by `rows`.
    pub(crate) fn uniforms(&self, cols: u32, rows: u32) -> super::gl::MeshUniforms {
        #[expect(clippy::cast_possible_truncation, reason = "GL's floats")]
        let rect = |r: Rect| [r.x as f32, r.y as f32, r.w as f32, r.h as f32];
        #[expect(clippy::cast_possible_truncation, reason = "GL's floats")]
        let float = |value: f64| value as f32;
        #[expect(clippy::cast_precision_loss, reason = "a grid's cells, far below 2^24")]
        let grid = [cols as f32, rows as f32];
        super::gl::MeshUniforms {
            from: rect(self.from),
            to: rect(self.to.unwrap_or(self.from)),
            monitor: rect(self.monitor),
            part: [
                float(self.part.u0),
                float(self.part.v0),
                float(self.part.u1),
                float(self.part.v1),
            ],
            grid,
            progress: float(self.progress),
            clamped: float(self.clamped),
            direction: float(self.direction),
            seed: float(self.seed),
            scale: float(self.scale),
            axis: solium_effects::mesh::axis_index(self.axis),
        }
    }

    /// Its params as the numbers a program's uniforms take, each by its
    /// `p_<name>`, a `vec4` four slots of one name; a word has none.
    pub(crate) fn slots(&self) -> (Vec<f32>, Vec<String>) {
        let (mut values, mut names) = (Vec::new(), Vec::new());
        for (name, value) in self.params {
            #[expect(clippy::cast_possible_truncation, reason = "GL's floats")]
            #[expect(clippy::cast_precision_loss, reason = "a param, far below 2^24")]
            let numbers: Vec<f32> = match value {
                Value::Number(number) => vec![*number as f32],
                Value::Int(int) => vec![*int as f32],
                Value::Bool(yes) => vec![if *yes { 1.0 } else { 0.0 }],
                Value::Vec4(four) => four.iter().map(|each| *each as f32).collect(),
                Value::Word(_) => Vec::new(),
            };
            names.extend(std::iter::repeat_n(format!("p_{name}"), numbers.len()));
            values.extend(numbers);
        }
        (values, names)
    }
}

/// The numbers a grid of `cols` by `rows` cells holds: x and y for each of
/// its corners, or `usize::MAX` for a grid no buffer could hold.
pub(crate) fn numbers(cols: u32, rows: u32) -> usize {
    (cols as usize)
        .checked_add(1)
        .and_then(|across| across.checked_mul(rows as usize + 1))
        .and_then(|points| points.checked_mul(2))
        .unwrap_or(usize::MAX)
}

/// A mesh run once a pass: `program`, its geometry's compiled `.vert`, run
/// by `compiler` with `ask`, and its grid checked.
/// `tests::a_grid_of_nan_too_few_points_or_too_far_is_refused`.
pub(crate) fn run<C: Compiler>(
    compiler: &mut C,
    program: &C::Program,
    ask: &Ask<'_>,
    cols: u32,
    rows: u32,
) -> Result<Vec<f64>, Refusal> {
    let (values, names) = ask.slots();
    let points = compiler
        .run_mesh(
            program,
            &ask.uniforms(cols, rows),
            (&values, &names),
            (cols, rows),
        )
        .map_err(|message| Refusal::Error { message })?;
    checked(points.into_iter().map(f64::from).collect(), ask, cols, rows)
}

/// A grid as a mesh placed it, checked: x and y for every point, every one
/// finite, and every point inside a box four monitors wide and high centred
/// on the pane's monitor.
/// `tests::a_grid_of_nan_too_few_points_or_too_far_is_refused`,
/// `tests::the_box_is_four_monitors_wide_and_high_around_the_panes_monitor`.
pub(crate) fn checked(
    points: Vec<f64>,
    ask: &Ask<'_>,
    cols: u32,
    rows: u32,
) -> Result<Vec<f64>, Refusal> {
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
    // One and a half monitors beyond each edge of the pane's.
    let (mx, my, mw, mh) = (ask.monitor.x, ask.monitor.y, ask.monitor.w, ask.monitor.h);
    let inside = points.as_chunks::<2>().0.iter().all(|&[x, y]| {
        x >= mx - 1.5 * mw && x <= mx + 2.5 * mw && y >= my - 1.5 * mh && y <= my + 2.5 * mh
    });
    if !inside {
        return Err(Refusal::TooBig);
    }
    Ok(points)
}

/// A part of the window the checks draw at rest beside the whole of it: in
/// from each edge by a different amount, so a mesh that places points from
/// `c / cols` instead of the part, or reads one side's bound for another's,
/// moves them, and would squeeze a part reaching past the window (its
/// popups) onto it (`tests::a_mesh_that_ignores_its_part_is_refused`).
/// Inside the window and not past it, because at progress 0 a geometry
/// rests only there: the genie has begun to pull what lies beyond its
/// leading edge (as the Rust genie it is held to has). A sample, as the
/// rectangles beside it are, not a behaviour.
const INNER: UnitRect = UnitRect {
    u0: 0.25,
    v0: 0.125,
    u1: 0.875,
    v1: 0.5,
};

/// How far from where the window is a point at rest may be: the GPU places
/// it in 32-bit floats, a thousandth of a pixel off at these sizes.
const AT_REST: f64 = 0.01;

/// One run of a mesh over a grid of so many columns and rows: what
/// [`at_rest`] holds to its contract.
pub(crate) type Run<'a> = dyn FnMut(&Ask<'_>, u32, u32) -> Result<Vec<f64>, Refusal> + 'a;

/// The contract a geometry is held to once its program compiled, before its
/// version runs: at progress 0 it draws the window exactly where it is, on
/// all four axes and in every direction (+1 arriving, −1 leaving, 0
/// resizing), within [`AT_REST`], over the window and over a part of it
/// ([`INNER`]); at progress 1 it gives a valid grid. `run` runs the mesh.
/// `tests::a_mesh_that_moves_the_window_at_progress_zero_is_refused`,
/// `tests::a_mesh_that_moves_the_window_arriving_is_refused`,
/// `tests::a_mesh_that_moves_the_window_resizing_is_refused`,
/// `tests::a_mesh_that_moves_the_window_on_one_axis_is_refused`,
/// `tests::a_mesh_whose_grid_at_progress_one_is_not_finite_is_refused`,
/// `tests::a_mesh_that_ignores_its_part_is_refused`.
pub(crate) fn at_rest(
    run: &mut Run<'_>,
    grid: GridSpec,
    params: &[(String, Value)],
) -> Result<(), Refusal> {
    let from = Rect::new(100.0, 100.0, 400.0, 300.0);
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
                let points = run(&ask, cols, rows)?;
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
                        if (px - x).abs() > AT_REST || (py - y).abs() > AT_REST {
                            return Err(Refusal::MovesAtRest);
                        }
                    }
                }
            }
            ask.part = UnitRect::WHOLE;
            ask.progress = 1.0;
            ask.clamped = 1.0;
            run(&ask, cols, rows)?;
        }
    }
    Ok(())
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

    use super::{Ask, Refusal, at_rest, checked};
    use crate::warp::UnitRect;

    /// One run's question, a closing window halfway to a corner.
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

    /// The window where it is, over `ask`'s part, as a mesh that draws it
    /// flat places it: what the test compilers run for a `.vert`.
    pub(crate) fn rest_grid(ask: &Ask<'_>, cols: u32, rows: u32) -> Vec<f64> {
        let part = ask.part;
        let mut points = Vec::new();
        for r in 0..=rows {
            for c in 0..=cols {
                let (x, y) = ask.from.at(
                    part.u0 + (part.u1 - part.u0) * (f64::from(c) / f64::from(cols)),
                    part.v0 + (part.v1 - part.v0) * (f64::from(r) / f64::from(rows)),
                );
                points.extend([x, y]);
            }
        }
        points
    }

    /// **A grid of NaN, of too few points, or too far is refused**, by
    /// which, and a whole grid inside the box is taken as it is.
    #[test]
    fn a_grid_of_nan_too_few_points_or_too_far_is_refused() {
        let asked = ask();
        let flat = rest_grid(&asked, 2, 1);
        assert_eq!(checked(flat.clone(), &asked, 2, 1), Ok(flat.clone()));
        assert_eq!(
            checked(flat[..10].to_vec(), &asked, 2, 1),
            Err(Refusal::Count {
                wanted: 12,
                got: 10
            })
        );
        let mut nan = flat.clone();
        nan[3] = f64::NAN;
        assert_eq!(checked(nan, &asked, 2, 1), Err(Refusal::NotFinite));
        let mut far = flat;
        far[0] = 1e6;
        assert_eq!(checked(far, &asked, 2, 1), Err(Refusal::TooBig));
    }

    /// The box a grid must keep inside: one and a half monitors past each
    /// edge of the pane's, the edges themselves inside.
    #[test]
    fn the_box_is_four_monitors_wide_and_high_around_the_panes_monitor() {
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
            let got = checked(vec![px, py, px, py, px, py, px, py], &asked, 1, 1);
            if inside {
                assert!(got.is_ok(), "({px}, {py}) is inside: {got:?}");
            } else {
                assert_eq!(got, Err(Refusal::TooBig), "({px}, {py}) is outside");
            }
        }
    }

    /// `at_rest` with a mesh that is `rest_grid` but where `moved` says
    /// otherwise, there one pixel to the right.
    fn checked_at_rest(moved: impl Fn(&Ask<'_>) -> bool) -> Result<(), Refusal> {
        let mut run = |asked: &Ask<'_>, cols: u32, rows: u32| {
            let mut points = rest_grid(asked, cols, rows);
            if moved(asked) {
                for x in points.iter_mut().step_by(2) {
                    *x += 1.0;
                }
            }
            checked(points, asked, cols, rows)
        };
        at_rest(
            &mut run,
            solium_effects::spec::GridSpec::Turning {
                along: 4,
                across: 2,
            },
            &[],
        )
    }

    #[test]
    fn a_mesh_that_draws_the_window_where_it_is_passes() {
        assert_eq!(checked_at_rest(|_| false), Ok(()));
        assert_eq!(
            checked_at_rest(|asked| asked.progress > 0.0),
            Ok(()),
            "moving once it has begun is what a geometry is for"
        );
    }

    /// **A mesh that moves the window at progress 0 is refused**, held
    /// to it once its program compiled, before its version runs.
    #[test]
    fn a_mesh_that_moves_the_window_at_progress_zero_is_refused() {
        assert_eq!(checked_at_rest(|_| true), Err(Refusal::MovesAtRest));
    }

    #[test]
    fn a_mesh_that_moves_the_window_arriving_is_refused() {
        assert_eq!(
            checked_at_rest(|asked| asked.direction > 0.0),
            Err(Refusal::MovesAtRest)
        );
    }

    #[test]
    fn a_mesh_that_moves_the_window_resizing_is_refused() {
        assert_eq!(
            checked_at_rest(|asked| asked.direction == 0.0),
            Err(Refusal::MovesAtRest)
        );
    }

    #[test]
    fn a_mesh_that_moves_the_window_on_one_axis_is_refused() {
        assert_eq!(
            checked_at_rest(|asked| asked.axis == Axis::Left),
            Err(Refusal::MovesAtRest)
        );
    }

    /// A mesh that places its points from `c / cols` over the whole window,
    /// ignoring its part, draws the window where it is for the whole of it
    /// and squeezes a part onto it.
    #[test]
    fn a_mesh_that_ignores_its_part_is_refused() {
        let mut run = |asked: &Ask<'_>, cols: u32, rows: u32| {
            let whole = Ask {
                part: UnitRect::WHOLE,
                ..*asked
            };
            checked(rest_grid(&whole, cols, rows), asked, cols, rows)
        };
        assert_eq!(
            at_rest(
                &mut run,
                solium_effects::spec::GridSpec::Fixed { cols: 2, rows: 2 },
                &[]
            ),
            Err(Refusal::MovesAtRest)
        );
    }

    #[test]
    fn a_mesh_whose_grid_at_progress_one_is_not_finite_is_refused() {
        let mut run = |asked: &Ask<'_>, cols: u32, rows: u32| {
            let mut points = rest_grid(asked, cols, rows);
            if asked.progress >= 1.0 {
                points[0] = f64::NAN;
            }
            checked(points, asked, cols, rows)
        };
        assert_eq!(
            at_rest(
                &mut run,
                solium_effects::spec::GridSpec::Fixed { cols: 1, rows: 1 },
                &[]
            ),
            Err(Refusal::NotFinite)
        );
    }

    /// What a mesh program is told: its rectangles and the axis as GL takes
    /// them, and each param by its `p_` name, a `vec4` four slots of one.
    #[test]
    fn a_mesh_is_told_its_uniforms_and_its_params_by_name() {
        use solium_effects::spec::Value;
        let params = [
            ("spread".to_owned(), Value::Number(1.5)),
            ("tint".to_owned(), Value::Vec4([0.0, 0.25, 0.5, 1.0])),
            ("on".to_owned(), Value::Bool(true)),
        ];
        let asked = Ask {
            axis: Axis::Left,
            params: &params,
            ..ask()
        };
        let uniforms = asked.uniforms(4, 2);
        assert_eq!(uniforms.from, [100.0, 100.0, 400.0, 300.0]);
        assert_eq!(uniforms.to, [900.0, 700.0, 40.0, 40.0]);
        assert_eq!((uniforms.grid, uniforms.axis), ([4.0, 2.0], 2));
        let (values, names) = asked.slots();
        assert_eq!(values, [1.5, 0.0, 0.25, 0.5, 1.0, 1.0]);
        assert_eq!(
            names,
            ["p_spread", "p_tint", "p_tint", "p_tint", "p_tint", "p_on"]
        );
    }

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

    /// The genie's grid from `rect` toward `to` at `progress`, with
    /// `spread` as the Rust genie widens its `f32`, which the shipped
    /// `genie.vert` is held to within 0.01 px on a GPU (wirecheck): what
    /// `warp::tests::a_genie_grid_lands_where_the_rust_genie_put_it`
    /// projects.
    pub(crate) fn genie_grid(
        rect: smithay::utils::Rectangle<f64, smithay::utils::Logical>,
        to: smithay::utils::Rectangle<f64, smithay::utils::Logical>,
        progress: f32,
        spread: f32,
    ) -> crate::warp::Grid {
        let (cols, rows) = super::turned(
            solium_effects::spec::GridSpec::Turning {
                along: 48,
                across: 8,
            },
            Axis::Down,
        );
        let genie = solium_effects::Deform::Genie {
            progress,
            spread,
            axis: Axis::Down,
        };
        let (from, to) = (
            crate::present::for_effects(rect),
            crate::present::for_effects(to),
        );
        let mut points = Vec::new();
        for r in 0..=rows {
            for c in 0..=cols {
                let (x, y) = genie.place(
                    from,
                    to,
                    f64::from(c) / f64::from(cols),
                    f64::from(r) / f64::from(rows),
                );
                points.extend([x, y]);
            }
        }
        crate::warp::Grid {
            cols,
            rows,
            part: UnitRect::WHOLE,
            points,
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
