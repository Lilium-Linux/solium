#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]
//! Solium's vertex deformations: the shapes a rectangle cannot hold.
//!
//! A genie, a fold, a curl, a page turn. Each is a **named function with
//! parameters** saying where a point of a window's unit square is drawn, and
//! how finely the window has to be cut for the result to read as a curve
//! rather than as a fan of flat pieces. Everything else -- capture,
//! projection, damage, blending between two of them -- is the same code for
//! all of them and lives in the compositor.
//!
//! ## Why this is a crate and not a shader
//!
//! Two reasons, both hard, both in
//! `docs/superpowers/specs/2026-09-12-panes-and-effects-design.md`:
//!
//! * The damage tracker needs the deformed bounding box **before** anything is
//!   drawn, and a shader cannot tell it one. A wrong damage rect is a corrupt
//!   screen; a wrong fragment shader is only a wrong picture.
//! * An effect you can only judge by launching a compositor is an effect
//!   nobody tunes. Everything here is arithmetic on `f64`, so the same code
//!   that bends real windows is driven by the unit tests below and by the
//!   preview page in `preview/` with live sliders.
//!
//! ## What it deliberately does not know
//!
//! What a pane is. The two rectangles a deformation morphs between arrive
//! already resolved: this crate is handed numbers, and the compositor is what
//! turns "the dock icon" into a rectangle, once per frame. See `Deform::place`.
//!
//! ```
//! use solium_effects::{Axis, Deform, Rect};
//!
//! let window = Rect::new(0.0, 0.0, 800.0, 600.0);
//! let icon = Rect::new(400.0, 1000.0, 64.0, 32.0);
//! let genie = Deform::Genie { progress: 1.0, spread: 1.0, axis: Axis::Down };
//!
//! // All the way in: every corner of the window is inside the icon.
//! let (x, y) = genie.place(window, icon, 0.0, 0.0);
//! assert!((x - icon.x).abs() < 1e-9 && (y - icon.y).abs() < 1e-9);
//! ```

pub mod ffi;

/// A rectangle, in whatever coordinates the caller is using.
///
/// This crate's own rather than the compositor's, because the compositor's is
/// Smithay's and this crate has no dependencies. The conversion happens at one
/// seam, in `present.rs`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    #[must_use]
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    /// The point at `(u, v)` of this rectangle's unit square. `(0, 0)` is its
    /// top left corner.
    #[must_use]
    pub fn at(self, u: f64, v: f64) -> (f64, f64) {
        (self.x + u * self.w, self.y + v * self.h)
    }
}

/// Interpolate between two values.
///
/// `solium_animation::lerp` is the same three characters, and this is not a
/// dependency on it: an effects crate that imports the animation crate is one
/// step from an effects crate that imports the compositor, and the empty
/// `[dependencies]` in `Cargo.toml` is the thing keeping both testable.
fn lerp(from: f64, to: f64, progress: f64) -> f64 {
    from + (to - from) * progress
}

/// One name, in the form a lookup compares.
fn tidy(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace(['_', '-'], "")
}

/// Where an effect's parameters come from.
///
/// A trait rather than a map, because the same parameters arrive from a Lua
/// table in the compositor, from a slider in the preview, and from a literal
/// in a test -- and this crate may not depend on any of the three. Every
/// parameter is optional and every effect has a default for each of its own,
/// which is what makes `{ effect = "genie" }` on its own a legal thing to
/// write.
///
/// What this cannot do is complain. A parameter that is missing, misspelled or
/// the wrong type is indistinguishable here from one nobody wrote, and there
/// is no logger to say so -- so both give the default. The check that catches
/// it is `script::shipped`, which reads the names out of this crate and fails
/// the build when the shipped Lua asks for one that does not exist.
pub trait Params {
    /// A number, if the caller has one under that name.
    fn number(&self, key: &str) -> Option<f64>;

    /// A word: an enumerated choice, such as which way a genie is pulled.
    fn word(&self, key: &str) -> Option<String>;
}

/// No parameters at all: every effect at its own defaults.
impl Params for () {
    fn number(&self, _key: &str) -> Option<f64> {
        None
    }
    fn word(&self, _key: &str) -> Option<String> {
        None
    }
}

/// Which way an effect sweeps across the window.
///
/// The axis a genie was hardcoded to before this crate existed, which is why a
/// dock down the side of the screen was inexpressible: `(1.0 - v)` is *down*
/// and nothing else. Named for the direction the window is pulled, so the
/// edge nearest the dock is the edge that leads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Axis {
    /// Pulled downwards: the bottom edge leads. A dock along the bottom, and
    /// the default because that is the dock everybody has seen.
    #[default]
    Down,
    /// Pulled upwards: the top edge leads. A bar along the top.
    Up,
    /// Pulled to the left: the left edge leads. A dock down the left side.
    Left,
    /// Pulled to the right: the right edge leads.
    Right,
}

impl Axis {
    /// How far through the sweep the point at `(u, v)` is.
    ///
    /// 0 leads and 1 follows, so a point at phase 0 has finished its journey
    /// before a point at phase 1 has started one. That lag is the whole
    /// difference between a genie and a shrink.
    #[must_use]
    pub fn phase(self, u: f64, v: f64) -> f64 {
        match self {
            Self::Down => 1.0 - v,
            Self::Up => v,
            Self::Left => u,
            Self::Right => 1.0 - u,
        }
    }

    /// Whether the sweep runs across the window rather than down it.
    ///
    /// What [`Deform::segments`] turns the grid by. A window cut into 48 rows
    /// and 8 columns has 48 steps of phase for a dock at the bottom and eight
    /// for one at the side -- which is not a subtler genie, it is a flipbook.
    #[must_use]
    pub fn horizontal(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }

    /// Every axis, for tools that want to show them all and for the check that
    /// the shipped Lua only names ones that exist.
    #[must_use]
    pub fn all() -> [(&'static str, Self); 4] {
        [
            ("down", Self::Down),
            ("up", Self::Up),
            ("left", Self::Left),
            ("right", Self::Right),
        ]
    }

    /// Look an axis up by the name a script or a settings file would use.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let wanted = tidy(name);
        Self::all()
            .into_iter()
            .find(|(known, _)| tidy(known) == wanted)
            .map(|(_, axis)| axis)
    }

    /// What to call this axis.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Down => "down",
            Self::Up => "up",
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

/// A deformation a rectangle cannot express.
///
/// A small enum rather than a callback: a deform has to be blended between two
/// frames, compared for equality, and named by a script, and a closure does
/// none of those.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Deform {
    /// Pulled from one rectangle into another like a sheet through a
    /// letterbox: the minimise, and the open that undoes it.
    ///
    /// `progress` 0 draws the window where it is, 1 has all of it inside the
    /// far rectangle. The rows nearest that rectangle go first, and that lag
    /// is the whole effect: it bends the sheet instead of shrinking it.
    /// `spread` is how much of the window is in motion at once -- 0 pulls it
    /// in rigidly, larger values draw the tail out behind it. `axis` is which
    /// edge leads.
    Genie {
        progress: f32,
        spread: f32,
        axis: Axis,
    },
}

impl Deform {
    /// Where the point at `(u, v)` of a window's unit square is drawn.
    ///
    /// A morph between two rectangles, and **both of them are arguments**.
    /// `from` is where the window is being drawn and `to` is what it is being
    /// pulled into -- a dock icon, a tab, another window. Neither is stored:
    /// the compositor resolves `to` from an identity once per frame, because a
    /// rectangle snapshotted when the script ran aims at where the dock icon
    /// was half a second ago.
    ///
    /// Which of the two is the window is the caller's choice, so there is no
    /// separate effect for the way back: swap them, or animate `progress`
    /// down instead of up, and the same function opens a window out of an icon
    /// that it minimised into one.
    #[must_use]
    pub fn place(self, from: Rect, to: Rect, u: f64, v: f64) -> (f64, f64) {
        match self {
            Self::Genie {
                progress,
                spread,
                axis,
            } => {
                let spread = f64::from(spread).max(0.0);
                // Each step across the axis runs its own copy of the
                // animation, the ones furthest from the target starting last.
                // Smoothstepped per step, so the sheet arrives without a
                // crease.
                let step = (f64::from(progress) * (1.0 + spread) - axis.phase(u, v) * spread)
                    .clamp(0.0, 1.0);
                let eased = step * step * (3.0 - 2.0 * step);
                let (from_x, from_y) = from.at(u, v);
                let (to_x, to_y) = to.at(u, v);
                (lerp(from_x, to_x, eased), lerp(from_y, to_y, eased))
            }
        }
    }

    /// Columns and rows the mesh needs to look like a curve rather than a fan
    /// of flat pieces. Columns subdivide `u`, rows subdivide `v`.
    ///
    /// The grid follows the axis, which is the whole reason the axis is a
    /// parameter and not two effects. Across the sweep the taper is linear, so
    /// a handful of cells only matters when a matrix is in play too; along it
    /// is where the bend lives and where every cell is one more step of phase.
    #[must_use]
    pub fn segments(self) -> (u32, u32) {
        /// Cells along the sweep: enough that the bend has no visible facets.
        const ALONG: u32 = 48;
        /// Cells across it.
        const ACROSS: u32 = 8;

        match self {
            Self::Genie { axis, .. } => {
                if axis.horizontal() {
                    (ALONG, ACROSS)
                } else {
                    (ACROSS, ALONG)
                }
            }
        }
    }

    /// The same deform, doing nothing.
    #[must_use]
    pub fn at_rest(self) -> Self {
        match self {
            Self::Genie { spread, axis, .. } => Self::Genie {
                progress: 0.0,
                spread,
                axis,
            },
        }
    }

    /// Blend two deforms, either of which may be absent.
    ///
    /// Absent means "not deformed", which for a genie is progress 0 -- so a
    /// script animating into one does not have to name the starting state, and
    /// clearing one animates back out of it.
    #[must_use]
    pub fn blend(from: Option<Self>, to: Option<Self>, progress: f64) -> Option<Self> {
        match (from, to) {
            (None, None) => None,
            (Some(one), None) => Some(one.mix(one.at_rest(), progress)),
            (None, Some(other)) => Some(other.at_rest().mix(other, progress)),
            (Some(one), Some(other)) => Some(one.mix(other, progress)),
        }
    }

    /// Blend towards another deform of the same kind.
    ///
    /// Between different kinds there is no meaningful halfway, so the
    /// destination wins outright; a script wanting a hand-off animates one out
    /// and the next one in. The axis is the same: there is no rectangle
    /// half way between *down* and *left*, and a grid cannot be turned a
    /// quarter of the way round.
    #[must_use]
    pub fn mix(self, other: Self, progress: f64) -> Self {
        match (self, other) {
            (
                Self::Genie {
                    progress: from_progress,
                    spread: from_spread,
                    ..
                },
                Self::Genie {
                    progress: to_progress,
                    spread: to_spread,
                    axis,
                },
            ) => Self::Genie {
                progress: number(lerp(
                    f64::from(from_progress),
                    f64::from(to_progress),
                    progress,
                )),
                spread: number(lerp(f64::from(from_spread), f64::from(to_spread), progress)),
                axis,
            },
        }
    }

    /// Every effect, at its defaults, for tools that want to show them all.
    ///
    /// This is the whole vocabulary [`Self::from_name`] knows, so it is also
    /// what a script may write -- and what the shipped `lua/*.lua` is checked
    /// against by `script::shipped`. An effect added here is available to
    /// every script and to the preview the moment it compiles, with nothing to
    /// add in `script.rs`; that is the point of the crate.
    #[must_use]
    pub fn all() -> [(&'static str, Self); 1] {
        [(
            "genie",
            Self::Genie {
                // All the way in, because that is what one animates *towards*.
                progress: 1.0,
                spread: 1.0,
                axis: Axis::Down,
            },
        )]
    }

    /// Look an effect up by the name a script used, and read its parameters.
    ///
    /// Mirrors `Curve::from_name`, including the part that matters: the names
    /// come from the engine rather than from a list kept in `script.rs`.
    #[must_use]
    pub fn from_name(name: &str, params: &dyn Params) -> Option<Self> {
        let wanted = tidy(name);
        Self::all()
            .into_iter()
            .find(|(known, _)| tidy(known) == wanted)
            .map(|(_, effect)| effect.with(params))
    }

    /// What to call this effect.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Genie { .. } => "genie",
        }
    }

    /// This effect with whatever the caller named read into it. Anything it
    /// does not name keeps the default it already has.
    #[must_use]
    fn with(self, params: &dyn Params) -> Self {
        match self {
            Self::Genie {
                progress,
                spread,
                axis,
            } => Self::Genie {
                progress: params.number("progress").map_or(progress, number),
                spread: params.number("spread").map_or(spread, number),
                axis: params
                    .word("axis")
                    .and_then(|word| Axis::from_name(&word))
                    .unwrap_or(axis),
            },
        }
    }
}

/// One parameter, narrowed to what a vertex function needs.
///
/// `f32` throughout because these are counts of nothing -- a fraction, a
/// spread -- and a deform is compared for equality every frame.
#[expect(
    clippy::cast_possible_truncation,
    reason = "an effect parameter is a small float either way"
)]
fn number(value: f64) -> f32 {
    value as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Rect = Rect::new(100.0, 50.0, 800.0, 600.0);
    const ICON: Rect = Rect::new(600.0, 1000.0, 64.0, 32.0);

    fn genie(progress: f32, axis: Axis) -> Deform {
        Deform::Genie {
            progress,
            spread: 1.0,
            axis,
        }
    }

    /// The four corners of the unit square at some progress.
    fn corners(deform: Deform, from: Rect, to: Rect) -> [(f64, f64); 4] {
        [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)].map(|(u, v)| deform.place(from, to, u, v))
    }

    #[test]
    fn at_rest_a_deform_draws_the_window_exactly_where_it_is() {
        // The property the whole cheap path rests on: progress 0 is not
        // "nearly the window", it is the window, to the last bit.
        for axis in Axis::all().map(|(_, axis)| axis) {
            for step in 0..=10 {
                let (u, v) = (f64::from(step) / 10.0, f64::from(step % 7) / 7.0);
                assert_eq!(genie(0.0, axis).place(WINDOW, ICON, u, v), WINDOW.at(u, v));
            }
        }
    }

    #[test]
    fn all_the_way_in_every_corner_is_inside_the_target() {
        for axis in Axis::all().map(|(_, axis)| axis) {
            for corner in corners(genie(1.0, axis), WINDOW, ICON) {
                assert!(
                    corner.0 >= ICON.x - 1e-9
                        && corner.0 <= ICON.x + ICON.w + 1e-9
                        && corner.1 >= ICON.y - 1e-9
                        && corner.1 <= ICON.y + ICON.h + 1e-9,
                    "{} left {corner:?} outside {ICON:?}",
                    axis.name()
                );
            }
        }
    }

    /// **The lag is what makes it a genie rather than a shrink.**
    ///
    /// Half way through, the edge nearest the target has travelled further
    /// than the edge furthest from it. Without that the window is a rectangle
    /// changing size, which is a different animation with the same endpoints
    /// -- and the endpoints are all the two tests above look at.
    #[test]
    fn the_leading_edge_goes_first() {
        let travelled = |deform: Deform, u: f64, v: f64| {
            let (x, y) = deform.place(WINDOW, ICON, u, v);
            let (rest_x, rest_y) = WINDOW.at(u, v);
            (x - rest_x).hypot(y - rest_y)
        };

        for (axis, (lead_u, lead_v), (trail_u, trail_v)) in [
            (Axis::Down, (0.5, 1.0), (0.5, 0.0)),
            (Axis::Up, (0.5, 0.0), (0.5, 1.0)),
            (Axis::Left, (0.0, 0.5), (1.0, 0.5)),
            (Axis::Right, (1.0, 0.5), (0.0, 0.5)),
        ] {
            let deform = genie(0.5, axis);
            let (lead, trail) = (
                travelled(deform, lead_u, lead_v),
                travelled(deform, trail_u, trail_v),
            );
            assert!(
                lead > trail + 1.0,
                "{}: the leading edge moved {lead:.1} and the trailing one {trail:.1}, \
                 which is a shrink and not a genie",
                axis.name()
            );
        }
    }

    /// **No part of the sheet overtakes the part ahead of it.**
    ///
    /// The sheet *stretches* -- the neck is the whole look -- so neighbouring
    /// rows do not stay a fixed distance apart and a test asserting they do is
    /// asserting a shrink. What must hold instead is the ordering: a row
    /// closer to the target is never less far along than one behind it, at any
    /// progress and at any spread. Break that and the sheet folds through
    /// itself, which draws as a crease that flickers rather than as anything a
    /// reader would call a bug in a genie.
    ///
    /// Measured against a target the same size as the window, so every point
    /// has the same distance to cover and how far it has gone is comparable
    /// between two of them. Against a shrinking target it is not: a row near
    /// the top has further to travel than a row near the bottom, whichever is
    /// further along.
    #[test]
    fn no_part_of_the_sheet_overtakes_the_one_ahead_of_it() {
        let shifted = Rect::new(WINDOW.x, WINDOW.y + 900.0, WINDOW.w, WINDOW.h);
        let travelled = |deform: Deform, u: f64, v: f64| {
            let (x, y) = deform.place(WINDOW, shifted, u, v);
            let (rest_x, rest_y) = WINDOW.at(u, v);
            (x - rest_x).hypot(y - rest_y)
        };

        for spread in [0.0, 0.5, 1.4, 4.0] {
            for step in 0_u8..=10 {
                let progress = f32::from(step) / 10.0;
                let deform = Deform::Genie {
                    progress,
                    spread,
                    axis: Axis::Down,
                };
                let (columns, rows) = deform.segments();
                // Down the phase axis, from the leading edge backwards.
                let mut ahead = f64::MAX;
                for row in (0..=rows).rev() {
                    let v = f64::from(row) / f64::from(rows);
                    for column in 0..=columns {
                        let behind = travelled(deform, f64::from(column) / f64::from(columns), v);
                        assert!(
                            behind <= ahead + 1e-9,
                            "spread {spread} at {progress}: row {row} has gone {behind:.3} \
                             and the row ahead of it only {ahead:.3}"
                        );
                    }
                    ahead = travelled(deform, 0.0, v);
                }
            }
        }

        // And the instrument: at a spread this large the rows really are at
        // different points, or the assertion above is comparing a rigid slab
        // with itself and holds for the wrong reason.
        let wide = Deform::Genie {
            progress: 0.5,
            spread: 4.0,
            axis: Axis::Down,
        };
        assert!(
            travelled(wide, 0.5, 1.0) > travelled(wide, 0.5, 0.0) + 1.0,
            "the sheet is not spread out at all"
        );
    }

    /// **The grid follows the axis.**
    ///
    /// The reason the axis could not simply be a parameter over a fixed grid:
    /// the fine subdivision has to be the one the phase varies along. Eight
    /// steps of phase is a flipbook, whichever way the dock faces.
    #[test]
    fn the_fine_subdivision_is_the_one_the_phase_runs_along() {
        for (_, axis) in Axis::all() {
            let (columns, rows) = genie(0.5, axis).segments();
            let (along, across) = if axis.horizontal() {
                (columns, rows)
            } else {
                (rows, columns)
            };
            assert!(
                along >= across * 4,
                "{}: {along} cells along the sweep and {across} across it",
                axis.name()
            );
            // And the phase really does vary along that one and not the other.
            let (a, b) = if axis.horizontal() {
                (axis.phase(0.0, 0.5), axis.phase(1.0, 0.5))
            } else {
                (axis.phase(0.5, 0.0), axis.phase(0.5, 1.0))
            };
            assert!((a - b).abs() > 0.9, "{}: phase barely moved", axis.name());
        }
    }

    /// **Opening out of an icon is the same effect as minimising into one.**
    ///
    /// Which is why there is no `Ungenie`: the direction is which rectangle is
    /// handed in as the window, and the compositor's `from` is whichever one
    /// it is drawing.
    #[test]
    fn the_morph_runs_both_ways() {
        let deform = genie(1.0, Axis::Down);
        for corner in corners(deform, ICON, WINDOW) {
            assert!(
                corner.0 >= WINDOW.x - 1e-9 && corner.0 <= WINDOW.x + WINDOW.w + 1e-9,
                "{corner:?} is not inside {WINDOW:?}"
            );
        }
    }

    /// **A target that is where the window already is deforms nothing.**
    ///
    /// The compositor's failure mode when an anchor cannot be resolved -- the
    /// pane it named has closed -- is to aim the deform at the window's own
    /// rectangle. It has to come out as the undeformed window rather than as
    /// anything interesting.
    #[test]
    fn aiming_at_itself_is_a_no_op() {
        for progress in [0.0, 0.25, 0.5, 1.0] {
            for (u, v) in [(0.0, 0.0), (0.5, 0.5), (1.0, 1.0), (0.25, 0.75)] {
                assert_eq!(
                    genie(progress, Axis::Down).place(WINDOW, WINDOW, u, v),
                    WINDOW.at(u, v)
                );
            }
        }
    }

    #[test]
    fn blending_from_nothing_starts_at_rest_and_lands_on_the_deform() {
        let target = genie(1.0, Axis::Down);
        assert_eq!(Deform::blend(None, None, 0.5), None);
        assert_eq!(
            Deform::blend(None, Some(target), 0.0),
            Some(target.at_rest())
        );
        assert_eq!(Deform::blend(None, Some(target), 1.0), Some(target));
        // And clearing one animates back out of it rather than snapping.
        assert_eq!(
            Deform::blend(Some(target), None, 1.0),
            Some(target.at_rest())
        );
        let halfway = Deform::blend(None, Some(target), 0.5);
        assert_eq!(halfway, Some(genie(0.5, Axis::Down)));
    }

    /// **An axis does not blend; the destination's wins.**
    ///
    /// There is no grid a quarter of the way between 48 rows and 48 columns,
    /// so a halfway axis would be a lie the mesh could not draw.
    #[test]
    fn the_destination_axis_wins_outright() {
        let down = genie(1.0, Axis::Down);
        let left = genie(1.0, Axis::Left);
        for progress in [0.0, 0.5, 1.0] {
            assert_eq!(
                down.mix(left, progress),
                genie(1.0, Axis::Left),
                "at {progress} the axis was not the destination's"
            );
        }
    }

    #[test]
    fn effects_are_found_by_the_names_scripts_use() {
        assert_eq!(
            Deform::from_name("genie", &()),
            Some(Deform::Genie {
                progress: 1.0,
                spread: 1.0,
                axis: Axis::Down,
            })
        );
        assert_eq!(
            Deform::from_name("GENIE", &()),
            Deform::from_name("genie", &())
        );
        assert_eq!(Deform::from_name("nonsense", &()), None);
    }

    /// Every name in [`Deform::all`] is one a script can write, and every
    /// effect answers to the name it is listed under.
    ///
    /// The round trip rather than either half, for the reason
    /// `solium_animation`'s equivalent gives: the halves are two lists that
    /// have to agree and there is nothing but this holding them together.
    #[test]
    fn every_name_resolves_and_every_effect_names_itself() {
        for (name, effect) in Deform::all() {
            assert_eq!(
                Deform::from_name(name, &()),
                Some(effect),
                "{name} is unreadable"
            );
            assert_eq!(
                effect.name(),
                name,
                "{name} does not answer to its own name"
            );
        }
        for (name, axis) in Axis::all() {
            assert_eq!(Axis::from_name(name), Some(axis), "{name} is unreadable");
            assert_eq!(axis.name(), name, "{name} does not answer to its own name");
        }
    }

    /// Parameters are read through the caller's own table, and anything it
    /// does not carry keeps the effect's default.
    #[test]
    fn parameters_are_read_and_the_rest_default() {
        struct Given;
        impl Params for Given {
            fn number(&self, key: &str) -> Option<f64> {
                (key == "spread").then_some(2.5)
            }
            fn word(&self, key: &str) -> Option<String> {
                (key == "axis").then(|| "LEFT".to_owned())
            }
        }

        assert_eq!(
            Deform::from_name("genie", &Given),
            Some(Deform::Genie {
                // Untouched: nothing was given for it.
                progress: 1.0,
                spread: 2.5,
                axis: Axis::Left,
            })
        );

        // A word that names no axis is indistinguishable from one nobody
        // wrote, and both keep the default. `script::shipped` is what catches
        // it in the Lua that ships.
        struct Nonsense;
        impl Params for Nonsense {
            fn number(&self, _key: &str) -> Option<f64> {
                None
            }
            fn word(&self, _key: &str) -> Option<String> {
                Some("sideways".to_owned())
            }
        }
        assert_eq!(
            Deform::from_name("genie", &Nonsense),
            Deform::from_name("genie", &())
        );
    }
}
