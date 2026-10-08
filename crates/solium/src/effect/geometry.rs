//! Geometry as files (\[16\] decision 6, §2): an effect's `mesh` writes the
//! whole grid once a pass, and Rust checks what it wrote before a vertex of
//! it is drawn (\[16\] §5).
//!
//! A call is told `t`, one table per effect state reused every call, which
//! carries every param beside the engine's fields ([`super::sandbox`]'s
//! `reused`): `tests::every_param_reaches_t`. What it wrote is refused when
//! it is not a whole grid of finite numbers within four monitors of the
//! pane's (`tests::a_mesh_that_writes_a_nan_or_too_few_points_or_too_far_is_refused`),
//! and a file is held at load to drawing the window where it is at progress
//! 0 (`tests::a_geometry_file_that_moves_the_window_at_progress_zero_is_refused_at_load`).

use std::time::{Duration, Instant};

use solium_effects::spec::{GridSpec, Value};
use solium_effects::{Axis, Rect};

use super::sandbox::{Budget, Sandbox, numbers};
use crate::warp::UnitRect;

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
    /// A point past four monitors' width or height around the pane's.
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
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "Task 28's grids call it once a pass")
)]
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
    // Four monitors' width and height around the pane's monitor.
    let (mx, my, mw, mh) = (ask.monitor.x, ask.monitor.y, ask.monitor.w, ask.monitor.h);
    let inside = points.as_chunks::<2>().0.iter().all(|&[x, y]| {
        x >= mx - 1.5 * mw && x <= mx + 2.5 * mw && y >= my - 1.5 * mh && y <= my + 2.5 * mh
    });
    if !inside {
        return Err(Refusal::TooBig);
    }
    Ok(points)
}

/// The load-time contract: at progress 0 the file draws the window exactly
/// where it is, on all four axes and in every direction (+1 arriving, −1
/// leaving, 0 resizing), within 1e-9; at progress 1 it writes a valid grid.
/// `tests::a_geometry_file_that_moves_the_window_at_progress_zero_is_refused_at_load`,
/// `tests::a_file_that_moves_the_window_arriving_is_refused_at_load`,
/// `tests::a_file_that_moves_the_window_resizing_is_refused_at_load`,
/// `tests::a_file_that_moves_the_window_on_one_axis_is_refused_at_load`,
/// `tests::a_file_whose_grid_at_progress_one_is_not_finite_is_refused_at_load`.
///
/// Its 24 calls share one load budget (Ruling 4's 100 ms for a load-time
/// call), so a load holds the thread no longer than one such call
/// (`tests::the_checks_at_load_share_one_budget`); and a version whose
/// fastest call is longer than a frame gives a `mesh` is refused, since
/// every frame would stop it. The fastest, because one call is slow when
/// the machine is busy, and 24 are slow only when the file is
/// (`tests::a_mesh_too_slow_for_a_frame_is_refused_at_load`).
pub(crate) fn at_rest(
    sandbox: &Sandbox,
    grid: GridSpec,
    params: &[(String, Value)],
) -> Result<(), Refusal> {
    let from = Rect::new(100.0, 100.0, 400.0, 300.0);
    let mut clock = Checks {
        deadline: Instant::now() + Budget::LOAD.time,
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
            let points = clock.call(sandbox, &ask, cols, rows)?;
            let mut at = points.as_chunks::<2>().0.iter();
            for r in 0..=rows {
                for c in 0..=cols {
                    let (x, y) = from.at(
                        f64::from(c) / f64::from(cols),
                        f64::from(r) / f64::from(rows),
                    );
                    let Some(&[px, py]) = at.next() else {
                        return Err(Refusal::MovesAtRest);
                    };
                    if (px - x).abs() > 1e-9 || (py - y).abs() > 1e-9 {
                        return Err(Refusal::MovesAtRest);
                    }
                }
            }
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
/// sweep, as `Deform::segments` does. `tests::the_grid_turns_with_the_axis` (Task 27).
pub(crate) fn turned(grid: GridSpec, axis: Axis) -> (u32, u32) {
    match grid {
        GridSpec::Fixed { cols, rows } => (cols.max(1), rows.max(1)),
        GridSpec::Turning { along, across } if axis.horizontal() => (along.max(1), across.max(1)),
        GridSpec::Turning { along, across } => (across.max(1), along.max(1)),
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

    /// **The checks at load share one budget**: 24 calls of 20 ms each are
    /// stopped at the load budget's 100 ms, not run for 480, so a load holds
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

}
