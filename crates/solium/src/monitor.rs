//! Where the monitors are, relative to each other.
//!
//! Every rect in the compositor — a pane's slot, a script's `sol.place`, the
//! pointer — lives in one **global logical space**. An output is a window onto
//! a region of it, and that is the whole of what multi-monitor means here:
//! there is no per-screen coordinate system to convert between, so a window
//! moves to the next monitor by being placed at an x that lands there.
//!
//! What this module owns is the one thing that is not derivable: *where each
//! output goes*. The kernel reports connectors in an order that has nothing to
//! do with which one is on the left, so the arrangement has to be either
//! configured or guessed, and this holds both.
//!
//! ## The guess
//!
//! Left to right in the order the connectors were enumerated, top edges
//! aligned. It is wrong about as often as it is right — which is fine, because
//! being wrong looks like "my second screen is on the wrong side" and is fixed
//! by naming the position. Being *unpredictable* would not be fine, so the
//! order is the enumeration order and never anything cleverer.
//!
//! ## The configuration
//!
//! ```lua
//! sol.monitors({
//!     { name = "DP-1", x = 0, y = 0 },
//!     { name = "HDMI-A-1", x = 2560, y = 180 },
//! })
//! ```
//!
//! A name that matches nothing is warned about rather than ignored: a monitor
//! layout that silently does nothing is the hardest kind of configuration to
//! debug, and the usual cause is a connector name that does not exist on this
//! machine.

use smithay::{
    desktop::{Space, Window},
    output::Output,
    utils::{Logical, Physical, Point, Rectangle, Size, Transform},
};

/// Which side of another monitor a screen sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Left,
    Right,
    Above,
    Below,
}

/// How the other axis lines up when one monitor is placed beside another.
///
/// Worth having rather than always aligning the top edges. A 1080p beside a
/// 1440p leaves 360 rows belonging to no screen, and which end of the small
/// monitor that dead strip is at decides whether the pointer catches on it on
/// the way to the taskbar or on the way to the menu bar. `centre` is the
/// default because it halves the strip instead of putting all of it at one end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Align {
    /// Top edges, or left edges.
    Start,
    #[default]
    Centre,
    /// Bottom edges, or right edges.
    End,
}

/// Which mode to drive a monitor at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Wanted {
    /// The highest refresh rate at the monitor's preferred resolution.
    ///
    /// The default, and not the same as "preferred": the EDID's preferred
    /// *flag* names a resolution and usually pairs it with a pedestrian refresh
    /// rate. A 260 Hz panel reports 2560x1440@60 as preferred, and taking that
    /// literally drives a fast display slowly and makes every animation in the
    /// compositor look worse than it is.
    #[default]
    Best,
    /// Exactly what the EDID's preferred flag says, refresh rate included.
    /// For a monitor that misbehaves at its highest rate.
    Preferred,
    /// The largest resolution, at its highest refresh rate.
    Widest,
    /// A particular mode. `None` for the refresh means "the highest available
    /// at this resolution".
    Exact {
        width: i32,
        height: i32,
        refresh: Option<i32>,
    },
}

/// How many device pixels to a logical one.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum Scaling {
    /// Worked out from the panel's physical size and its mode. The default.
    #[default]
    Auto,
    /// What the configuration said.
    Fixed(f64),
}

/// The dots per inch above which a display is treated as needing 2x.
///
/// 192 is what GNOME and KDE both use, and matching them matters more than
/// being right in the abstract: it is the number every monitor's marketing and
/// every forum answer is implicitly calibrated against.
///
/// It puts a 13" 4K laptop panel (~331 dpi) at 2x, which is unreadable
/// otherwise, and a 27" 4K (~163 dpi) at 1x, which is arguable and is exactly
/// why `scale` is settable. A 27" 1440p desktop panel is ~109 and nowhere near.
const HIDPI: f64 = 192.0;

/// The scale a panel of this size and resolution probably wants.
///
/// `physical` is in millimetres, as EDID reports it. A monitor that does not
/// say — and plenty do not, reporting 0x0 — gets 1x, because a guess from no
/// information is worse than the status quo.
pub(crate) fn automatic(
    physical: Size<i32, smithay::utils::Raw>,
    mode: Size<i32, Physical>,
) -> f64 {
    if physical.w <= 0 || mode.w <= 0 {
        return 1.0;
    }
    let inches = f64::from(physical.w) / 25.4;
    let dpi = f64::from(mode.w) / inches;
    if dpi >= HIDPI { 2.0 } else { 1.0 }
}

/// One monitor, as the configuration describes it.
#[derive(Clone, Debug)]
pub(crate) struct Placement {
    /// The connector name, as the kernel reports it: `DP-1`, `HDMI-A-1`,
    /// `eDP-1`. `--probe` prints the ones this machine has.
    pub(crate) name: String,
    /// Where its top-left corner goes in the global space, when a position was
    /// given outright.
    pub(crate) at: Option<Point<i32, Logical>>,
    /// Beside another monitor, when that was given instead — which is what
    /// people actually want to write, because it does not go stale when a
    /// monitor's resolution changes.
    pub(crate) beside: Option<(Side, String, Align)>,
    /// Which mode to drive it at.
    pub(crate) mode: Wanted,
    /// Whether to ask for variable refresh rate.
    ///
    /// `None` leaves it alone, which on every driver means off. Worth being a
    /// three-state rather than a bool so that a monitor mentioned for its
    /// position does not silently have VRR turned off for it.
    pub(crate) vrr: Option<bool>,
    /// Rotation and flipping. A monitor stood on its end is `"90"`.
    pub(crate) transform: Option<Transform>,
    /// Whether to drive it at all. A connected monitor that is switched off
    /// here is not given a CRTC, so it costs nothing and frees one.
    pub(crate) enabled: bool,
    /// Whether this is the monitor things belonging to one screen go on — a
    /// dock, a bar, a layer surface that named no output.
    pub(crate) primary: bool,
    /// How many device pixels to a logical one.
    pub(crate) scale: Scaling,
}

/// Where the monitors went, and what could not be worked out.
#[derive(Clone, Debug, Default)]
pub(crate) struct Layout {
    /// One position per monitor given, in the same order.
    pub(crate) at: Vec<Point<i32, Logical>>,
    /// Monitors whose `right_of` (or `below`, …) named something that is not
    /// there, or that name each other in a circle. Placed anyway, to the right
    /// of everything, and reported so the caller can say so.
    pub(crate) unresolved: Vec<String>,
}

/// The arrangement a script asked for.
///
/// Empty means "arrange them yourself", which is the default and is what a
/// machine with one monitor wants.
#[derive(Clone, Debug, Default)]
pub(crate) struct Arrangement {
    placements: Vec<Placement>,
}

impl Arrangement {
    pub(crate) fn new(placements: Vec<Placement>) -> Self {
        Self { placements }
    }

    fn find(&self, name: &str) -> Option<&Placement> {
        self.placements
            .iter()
            .find(|placement| placement.name == name)
    }

    /// Whether this connector should be driven. Unmentioned monitors are on:
    /// a configuration that names two screens must not switch off a third.
    pub(crate) fn enabled(&self, name: &str) -> bool {
        self.find(name).is_none_or(|placement| placement.enabled)
    }

    /// Which mode this connector was asked for.
    pub(crate) fn mode(&self, name: &str) -> Wanted {
        self.find(name)
            .map(|placement| placement.mode)
            .unwrap_or_default()
    }

    /// Whether this connector was asked for variable refresh rate.
    pub(crate) fn vrr(&self, name: &str) -> Option<bool> {
        self.find(name)?.vrr
    }

    /// The transform this connector was asked for, if any.
    pub(crate) fn transform(&self, name: &str) -> Option<Transform> {
        self.find(name)?.transform
    }

    /// The monitor things belonging to one screen go on.
    ///
    /// The first one marked `primary` that is actually here, and otherwise
    /// nothing — the caller falls back to the first monitor, which it has and
    /// this does not.
    pub(crate) fn primary(&self) -> Option<&str> {
        self.placements
            .iter()
            .find(|placement| placement.primary && placement.enabled)
            .map(|placement| placement.name.as_str())
    }

    /// The scale this connector was asked for.
    pub(crate) fn scale(&self, name: &str) -> Scaling {
        self.find(name)
            .map(|placement| placement.scale)
            .unwrap_or_default()
    }

    /// Where each of `monitors` goes, in the order given.
    ///
    /// Monitors given a position outright take it. The rest are resolved
    /// against whatever is already placed, and when nothing more can be
    /// resolved one unplaced monitor is *seeded* — given the next free space to
    /// the right — and resolving continues.
    ///
    /// The seeding is the part that matters, and the first version of this did
    /// not have it. Almost nobody writes coordinates: they write
    ///
    /// ```lua
    /// { name = "DP-1", primary = true },
    /// { name = "DP-2", right_of = "DP-1" },
    /// ```
    ///
    /// where the anchor has no position either, because which screen is at the
    /// origin is not a thing anyone cares about. Resolving relatives only
    /// against *positioned* monitors leaves that configuration entirely
    /// unresolved — which is the way it will almost always be written.
    ///
    /// Seeding prefers a monitor something else is anchored to, so the chain
    /// gets its foot on the ground rather than being seeded from the middle.
    ///
    /// Always returns one position per monitor. A monitor left without one
    /// would be a screen that is on and black with nothing said about it, so
    /// there is no path here that can leave one out.
    pub(crate) fn place(&self, monitors: &[(String, Size<i32, Logical>)]) -> Layout {
        let mut placed: Vec<Option<Point<i32, Logical>>> = vec![None; monitors.len()];
        let mut unresolved = Vec::new();
        let index_of = |name: &str| monitors.iter().position(|(each, _)| each == name);
        let beside_of = |name: &str| {
            self.find(name)
                .and_then(|placement| placement.beside.clone())
        };

        // Positions given outright.
        for (index, (name, _)) in monitors.iter().enumerate() {
            if let Some(at) = self.find(name).and_then(|placement| placement.at) {
                placed[index] = Some(at);
            }
        }

        // One extra round beyond the number of monitors: each round either
        // resolves something, seeds something, or finishes, so this cannot
        // spin on a configuration where two screens name each other.
        for _ in 0..=monitors.len() {
            // Everything that can be worked out from what is already placed.
            loop {
                let mut progress = false;
                for (index, (name, size)) in monitors.iter().enumerate() {
                    if placed[index].is_some() {
                        continue;
                    }
                    let Some((side, anchor, align)) = beside_of(name) else {
                        continue;
                    };
                    let Some(anchor_index) = index_of(&anchor) else {
                        continue;
                    };
                    let Some(anchor_at) = placed[anchor_index] else {
                        continue;
                    };
                    let anchor_size = monitors[anchor_index].1;
                    placed[index] = Some(beside(anchor_at, anchor_size, side, align, *size));
                    progress = true;
                }
                if !progress {
                    break;
                }
            }

            // Stuck. Seed one monitor and go round again — preferring one that
            // something else is waiting on, so a chain is seeded from its end
            // rather than its middle.
            let anchors: Vec<String> = monitors
                .iter()
                .filter_map(|(name, _)| beside_of(name).map(|(_, anchor, _)| anchor))
                .collect();
            let seed = monitors
                .iter()
                .enumerate()
                .filter(|(index, _)| placed[*index].is_none())
                .min_by_key(|(_, (name, _))| {
                    // An anchor first, then enumeration order.
                    u8::from(!anchors.contains(name))
                })
                .map(|(index, _)| index);
            let Some(seed) = seed else {
                break;
            };

            // The right edge of everything decided so far, so a seeded monitor
            // lands beside the others rather than on top of them.
            let edge = placed
                .iter()
                .zip(monitors)
                .filter_map(|(at, (_, size))| at.map(|at| at.x + size.w))
                .max()
                .unwrap_or(0);
            placed[seed] = Some((edge, 0).into());

            // A monitor that asked to be beside something and had to be seeded
            // instead named something that is not here, or is in a circle with
            // it. Placed anyway, and said out loud.
            if beside_of(&monitors[seed].0).is_some() {
                unresolved.push(monitors[seed].0.clone());
            }
        }

        Layout {
            // Every entry was filled by the loop above; the fallback keeps the
            // signature honest rather than describing a reachable case.
            at: placed
                .into_iter()
                .map(|at| at.unwrap_or_default())
                .collect(),
            unresolved,
        }
    }

    /// Configured names that no connector answered to.
    ///
    /// Reported by the caller: a typo here is the difference between a
    /// configuration that works and one that appears to be ignored.
    pub(crate) fn unmatched(&self, monitors: &[(String, Size<i32, Logical>)]) -> Vec<&str> {
        self.placements
            .iter()
            .map(|placement| placement.name.as_str())
            .filter(|name| !monitors.iter().any(|(each, _)| each == name))
            .collect()
    }
}

/// Where a monitor of `size` goes when placed on one side of another.
fn beside(
    anchor_at: Point<i32, Logical>,
    anchor_size: Size<i32, Logical>,
    side: Side,
    align: Align,
    size: Size<i32, Logical>,
) -> Point<i32, Logical> {
    // How far along the *other* axis it slides, so the two line up as asked.
    let offset = |anchor: i32, own: i32| match align {
        Align::Start => 0,
        Align::Centre => (anchor - own) / 2,
        Align::End => anchor - own,
    };
    match side {
        Side::Right => (
            anchor_at.x + anchor_size.w,
            anchor_at.y + offset(anchor_size.h, size.h),
        ),
        Side::Left => (
            anchor_at.x - size.w,
            anchor_at.y + offset(anchor_size.h, size.h),
        ),
        Side::Below => (
            anchor_at.x + offset(anchor_size.w, size.w),
            anchor_at.y + anchor_size.h,
        ),
        Side::Above => (
            anchor_at.x + offset(anchor_size.w, size.w),
            anchor_at.y - size.h,
        ),
    }
    .into()
}

/// A scale by the way a configuration writes one.
///
/// A number, or the word `auto`. Refuses anything outside a sane band rather
/// than believing it: a scale of 0 divides the desktop by zero and a scale of
/// 40 makes one window fill a wall, and both are far more likely to be a typo
/// than a request.
pub(crate) fn scaling(value: &f64) -> Option<Scaling> {
    if *value >= 0.5 && *value <= 8.0 {
        Some(Scaling::Fixed(*value))
    } else {
        None
    }
}

/// A mode by the way a configuration writes one.
///
/// `"2560x1440@165"`, `"2560x1440"`, or one of the words `best`, `preferred`
/// and `widest`. The `WxH@R` form is what every display tool on Linux uses and
/// what anyone will reach for first, so it is the form the documentation shows;
/// a table with `w`, `h` and `refresh` keys does the same thing for anyone
/// generating a configuration rather than writing one.
///
/// Returns `None` for anything it cannot read, so the caller can say which
/// monitor and fall back rather than guessing.
pub(crate) fn mode(text: &str) -> Option<Wanted> {
    let text = text.trim().to_ascii_lowercase();
    match text.as_str() {
        "best" | "auto" | "" => return Some(Wanted::Best),
        "preferred" => return Some(Wanted::Preferred),
        "widest" | "highres" => return Some(Wanted::Widest),
        _ => {}
    }

    // `2560x1440@165`, and the refresh is optional. Split on the `@` first so
    // a malformed rate cannot be read as part of the height.
    let (size, refresh) = match text.split_once('@') {
        Some((size, refresh)) => {
            // Trailing `hz` is what people write, and a fractional rate is
            // what a mode list prints; both mean the same integer here.
            let refresh = refresh.trim_end_matches("hz").trim();
            let refresh: f64 = refresh.parse().ok()?;
            if refresh <= 0.0 {
                return None;
            }
            #[expect(clippy::cast_possible_truncation, reason = "a refresh rate in hertz")]
            (size, Some(refresh.round() as i32))
        }
        None => (text.as_str(), None),
    };
    let (width, height) = size.split_once('x')?;
    let width: i32 = width.trim().parse().ok()?;
    let height: i32 = height.trim().parse().ok()?;
    if width <= 0 || height <= 0 {
        return None;
    }
    Some(Wanted::Exact {
        width,
        height,
        refresh,
    })
}

/// A transform by the name a configuration writes.
///
/// The numbers are degrees anticlockwise, which is what every other display
/// tool calls them, and `flipped` is mirrored horizontally first.
pub(crate) fn transform(name: &str) -> Option<Transform> {
    match name.trim().to_ascii_lowercase().as_str() {
        "normal" | "0" => Some(Transform::Normal),
        "90" => Some(Transform::_90),
        "180" => Some(Transform::_180),
        "270" => Some(Transform::_270),
        "flipped" | "flipped-0" => Some(Transform::Flipped),
        "flipped-90" => Some(Transform::Flipped90),
        "flipped-180" => Some(Transform::Flipped180),
        "flipped-270" => Some(Transform::Flipped270),
        _ => None,
    }
}

/// An alignment by the name a configuration writes.
pub(crate) fn align(name: &str) -> Option<Align> {
    match name.trim().to_ascii_lowercase().as_str() {
        "start" | "top" | "left" => Some(Align::Start),
        "centre" | "center" | "middle" => Some(Align::Centre),
        "end" | "bottom" | "right" => Some(Align::End),
        _ => None,
    }
}

/// The output covering a point, if any.
pub(crate) fn at(space: &Space<Window>, point: Point<f64, Logical>) -> Option<Output> {
    space
        .outputs()
        .find(|output| {
            space
                .output_geometry(output)
                .is_some_and(|geometry| geometry.to_f64().contains(point))
        })
        .cloned()
}

/// The output nearest a point, measured from its centre.
///
/// What `at` falls back to. The point can be outside every output — the gap in
/// an L-shaped arrangement, or a window placed off the edge — and the honest
/// answer there is the closest screen rather than none, because the callers
/// are asking "which monitor is this on" in order to *do* something.
pub(crate) fn nearest(space: &Space<Window>, point: Point<f64, Logical>) -> Option<Output> {
    space
        .outputs()
        .filter_map(|output| {
            let geometry = space.output_geometry(output)?;
            let centre = geometry.loc.to_f64()
                + Point::from((
                    f64::from(geometry.size.w) / 2.0,
                    f64::from(geometry.size.h) / 2.0,
                ));
            let (dx, dy) = (centre.x - point.x, centre.y - point.y);
            // Squared: the ordering is the same and there is no square root.
            #[expect(clippy::cast_possible_truncation, reason = "compared, not measured")]
            Some((output.clone(), (dx * dx + dy * dy) as i64))
        })
        .min_by_key(|(_, distance)| *distance)
        .map(|(output, _)| output)
}

/// The smallest rectangle covering every output.
///
/// What the pointer may be moved within. Not the same as any one output's
/// rect, and not the same as the sum of their areas either: an L-shaped
/// arrangement has a hole in it, which is why the pointer is clamped per
/// output rather than to this. See `state::clamp_pointer`.
pub(crate) fn union(space: &Space<Window>) -> Option<Rectangle<i32, Logical>> {
    space
        .outputs()
        .filter_map(|output| space.output_geometry(output))
        .reduce(|whole, each| whole.merge(each))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A monitor named with nothing said about it: on, not primary, wherever
    /// the arrangement puts it. The tests vary one field at a time from here.
    fn named(name: &str) -> Placement {
        Placement {
            name: name.to_owned(),
            at: None,
            beside: None,
            mode: Wanted::default(),
            vrr: None,
            transform: None,
            enabled: true,
            primary: false,
            scale: Scaling::default(),
        }
    }

    fn monitors(names: &[(&str, i32, i32)]) -> Vec<(String, Size<i32, Logical>)> {
        names
            .iter()
            .map(|(name, w, h)| ((*name).to_owned(), Size::from((*w, *h))))
            .collect()
    }

    fn at(name: &str, x: i32, y: i32) -> Placement {
        Placement {
            at: Some((x, y).into()),
            ..named(name)
        }
    }

    fn beside_of(name: &str, side: Side, anchor: &str, align: Align) -> Placement {
        Placement {
            beside: Some((side, anchor.to_owned(), align)),
            ..named(name)
        }
    }

    #[test]
    fn one_monitor_is_at_the_origin() {
        let layout = Arrangement::default().place(&monitors(&[("eDP-1", 1920, 1080)]));
        assert_eq!(layout.at, vec![Point::from((0, 0))]);
    }

    #[test]
    fn unconfigured_monitors_go_left_to_right() {
        // The default guess, and the reason it is a guess: enumeration order
        // is the only thing available and it says nothing about the desk.
        let layout = Arrangement::default().place(&monitors(&[
            ("DP-1", 2560, 1440),
            ("HDMI-A-1", 1920, 1080),
            ("DP-2", 1280, 1024),
        ]));
        assert_eq!(
            layout.at,
            vec![
                Point::from((0, 0)),
                Point::from((2560, 0)),
                Point::from((4480, 0)),
            ]
        );
    }

    #[test]
    fn a_configured_monitor_takes_its_position() {
        let arrangement = Arrangement::new(vec![at("HDMI-A-1", 0, 0), at("DP-1", 1920, 200)]);
        // Enumerated in the other order, on purpose: the configuration decides
        // the arrangement, and the kernel's order must not be able to override
        // it.
        let layout =
            arrangement.place(&monitors(&[("DP-1", 2560, 1440), ("HDMI-A-1", 1920, 1080)]));
        assert_eq!(
            layout.at,
            vec![Point::from((1920, 200)), Point::from((0, 0))]
        );
    }

    #[test]
    fn an_unconfigured_monitor_lands_beside_the_configured_ones() {
        // A third screen plugged into a configuration that names two. Placing
        // it at the origin would put it on top of one of them, which looks
        // like both being broken rather than one being unconfigured.
        let arrangement = Arrangement::new(vec![at("DP-1", 0, 0)]);
        let layout =
            arrangement.place(&monitors(&[("DP-1", 2560, 1440), ("HDMI-A-1", 1920, 1080)]));
        assert_eq!(layout.at, vec![Point::from((0, 0)), Point::from((2560, 0))]);
    }

    #[test]
    fn every_monitor_gets_a_position() {
        // The property that matters more than any particular arrangement: a
        // monitor with no position is a screen that is on and black.
        let arrangement = Arrangement::new(vec![at("nothing-called-this", 100, 100)]);
        let all = monitors(&[("DP-1", 800, 600), ("DP-2", 800, 600), ("DP-3", 800, 600)]);
        assert_eq!(arrangement.place(&all).at.len(), all.len());
    }

    #[test]
    fn a_name_nothing_answers_to_is_reported() {
        let arrangement = Arrangement::new(vec![at("DP-1", 0, 0), at("DP-9", 0, 0)]);
        assert_eq!(
            arrangement.unmatched(&monitors(&[("DP-1", 800, 600)])),
            vec!["DP-9"]
        );
    }

    /// The form people actually want to write: no arithmetic, and it does not
    /// go stale when a monitor's resolution changes.
    #[test]
    fn a_monitor_can_be_placed_beside_another() {
        let arrangement = Arrangement::new(vec![
            at("DP-1", 0, 0),
            beside_of("DP-2", Side::Right, "DP-1", Align::Start),
        ]);
        let layout = arrangement.place(&monitors(&[("DP-1", 2560, 1440), ("DP-2", 1920, 1080)]));
        assert_eq!(layout.at, vec![Point::from((0, 0)), Point::from((2560, 0))]);
        assert!(layout.unresolved.is_empty());
    }

    #[test]
    fn beside_works_on_all_four_sides() {
        let anchor = Point::from((1000, 1000));
        let anchor_size = Size::from((2560, 1440));
        let size = Size::from((1920, 1080));
        let start = Align::Start;
        assert_eq!(
            beside(anchor, anchor_size, Side::Right, start, size),
            Point::from((3560, 1000))
        );
        assert_eq!(
            beside(anchor, anchor_size, Side::Left, start, size),
            Point::from((-920, 1000))
        );
        assert_eq!(
            beside(anchor, anchor_size, Side::Below, start, size),
            Point::from((1000, 2440))
        );
        assert_eq!(
            beside(anchor, anchor_size, Side::Above, start, size),
            Point::from((1000, -80))
        );
    }

    /// A 1080p beside a 1440p leaves 360 rows belonging to no screen, and
    /// which end of the small monitor they sit at is what the alignment
    /// chooses. Centring halves the strip rather than putting it all at one
    /// end, which is why it is the default.
    #[test]
    fn alignment_decides_where_the_dead_strip_goes() {
        let anchor = Point::from((0, 0));
        let anchor_size = Size::from((2560, 1440));
        let size = Size::from((1920, 1080));
        assert_eq!(
            beside(anchor, anchor_size, Side::Right, Align::Start, size).y,
            0
        );
        assert_eq!(
            beside(anchor, anchor_size, Side::Right, Align::Centre, size).y,
            180
        );
        assert_eq!(
            beside(anchor, anchor_size, Side::Right, Align::End, size).y,
            360
        );
    }

    /// Written in the wrong order on purpose. A configuration is a list a
    /// person edits, and requiring them to sort it by dependency would be a
    /// rule nobody is told about until it silently does the wrong thing.
    #[test]
    fn a_chain_resolves_whatever_order_it_is_written_in() {
        let arrangement = Arrangement::new(vec![
            beside_of("DP-3", Side::Right, "DP-2", Align::Start),
            beside_of("DP-2", Side::Right, "DP-1", Align::Start),
            at("DP-1", 0, 0),
        ]);
        let layout = arrangement.place(&monitors(&[
            ("DP-1", 1000, 1000),
            ("DP-2", 1000, 1000),
            ("DP-3", 1000, 1000),
        ]));
        assert_eq!(
            layout.at,
            vec![
                Point::from((0, 0)),
                Point::from((1000, 0)),
                Point::from((2000, 0)),
            ]
        );
    }

    /// The way anyone will actually write it: the anchor has no position
    /// either, because which screen is at the origin is not a thing people
    /// care about. The first version of this resolved relatives only against
    /// *positioned* monitors, so this configuration — the likely one — came out
    /// entirely unresolved and in enumeration order.
    #[test]
    fn an_anchor_with_no_position_of_its_own_still_anchors() {
        let arrangement = Arrangement::new(vec![
            named("DP-1"),
            beside_of("DP-2", Side::Right, "DP-1", Align::Start),
        ]);
        // Enumerated the other way round, so nothing about the answer can be
        // coming from the kernel's order.
        let layout = arrangement.place(&monitors(&[("DP-2", 1920, 1080), ("DP-1", 2560, 1440)]));
        assert_eq!(layout.at, vec![Point::from((2560, 0)), Point::from((0, 0))]);
        assert!(layout.unresolved.is_empty());
    }

    /// A chain with nothing positioned anywhere in it. Seeding prefers a
    /// monitor something else is anchored to, so the chain gets its foot on the
    /// ground rather than being seeded from the middle.
    #[test]
    fn a_chain_needs_no_coordinates_at_all() {
        let arrangement = Arrangement::new(vec![
            beside_of("DP-3", Side::Right, "DP-2", Align::Start),
            beside_of("DP-2", Side::Right, "DP-1", Align::Start),
            named("DP-1"),
        ]);
        let layout = arrangement.place(&monitors(&[
            ("DP-1", 1000, 1000),
            ("DP-2", 1000, 1000),
            ("DP-3", 1000, 1000),
        ]));
        assert_eq!(
            layout.at,
            vec![
                Point::from((0, 0)),
                Point::from((1000, 0)),
                Point::from((2000, 0)),
            ]
        );
        assert!(layout.unresolved.is_empty());
    }

    /// Two monitors each to the right of the other. Nothing can satisfy that.
    /// One of them is seeded and reported, and the other then resolves against
    /// it — which is a better desk than dumping both in a row, and the only
    /// unacceptable answer is one that never finishes.
    #[test]
    fn monitors_that_name_each_other_still_get_placed() {
        let arrangement = Arrangement::new(vec![
            beside_of("DP-1", Side::Right, "DP-2", Align::Start),
            beside_of("DP-2", Side::Right, "DP-1", Align::Start),
        ]);
        let layout = arrangement.place(&monitors(&[("DP-1", 800, 600), ("DP-2", 800, 600)]));
        assert_eq!(layout.at, vec![Point::from((0, 0)), Point::from((800, 0))]);
        assert_eq!(layout.unresolved, vec!["DP-1"]);
    }

    #[test]
    fn an_anchor_that_is_not_there_is_reported() {
        let arrangement = Arrangement::new(vec![beside_of(
            "DP-2",
            Side::Right,
            "the-one-i-unplugged",
            Align::Start,
        )]);
        let layout = arrangement.place(&monitors(&[("DP-2", 800, 600)]));
        assert_eq!(layout.at, vec![Point::from((0, 0))]);
        assert_eq!(layout.unresolved, vec!["DP-2"]);
    }

    /// A configuration that names two screens must not switch off a third.
    #[test]
    fn a_monitor_nobody_mentioned_is_on() {
        let arrangement = Arrangement::new(vec![Placement {
            enabled: false,
            ..named("DP-1")
        }]);
        assert!(!arrangement.enabled("DP-1"));
        assert!(arrangement.enabled("DP-2"));
    }

    #[test]
    fn a_monitor_switched_off_cannot_be_the_primary_one() {
        // Otherwise a dock goes on a screen that is not being driven, which
        // is a dock nobody can see and no error anywhere.
        let arrangement = Arrangement::new(vec![
            Placement {
                primary: true,
                enabled: false,
                ..named("DP-1")
            },
            Placement {
                primary: true,
                ..named("DP-2")
            },
        ]);
        assert_eq!(arrangement.primary(), Some("DP-2"));
    }

    /// The heuristic, on real panels. Matching what GNOME and KDE do matters
    /// more than being right in the abstract, because that is the number
    /// everybody's expectations are calibrated against.
    #[test]
    fn the_automatic_scale_reads_laptops_and_desktops_apart() {
        // A 13.3" 4K laptop panel: unreadable at 1x.
        assert_eq!(automatic((294, 165).into(), (3840, 2160).into()), 2.0);
        // A 27" 1440p desktop panel, which is what this is developed on.
        assert_eq!(automatic((596, 336).into(), (2560, 1440).into()), 1.0);
        // A 27" 4K: 163 dpi, under the line, and exactly the case the issue
        // says only a person can decide. It gets 1x and `scale` is settable.
        assert_eq!(automatic((596, 336).into(), (3840, 2160).into()), 1.0);
        // A monitor that reports no physical size at all -- and plenty do.
        // A guess from no information is worse than the status quo.
        assert_eq!(automatic((0, 0).into(), (3840, 2160).into()), 1.0);
    }

    #[test]
    fn a_scale_outside_reason_is_refused() {
        assert_eq!(scaling(&1.5), Some(Scaling::Fixed(1.5)));
        assert_eq!(scaling(&2.0), Some(Scaling::Fixed(2.0)));
        // Far likelier a typo than a request, and both ends are ruinous: zero
        // divides the desktop by zero, forty makes one window fill a wall.
        assert_eq!(scaling(&0.0), None);
        assert_eq!(scaling(&40.0), None);
        assert_eq!(scaling(&-2.0), None);
    }

    #[test]
    fn modes_parse_the_way_people_write_them() {
        // The form every display tool uses, and the one anyone reaches for.
        assert_eq!(
            mode("2560x1440@165"),
            Some(Wanted::Exact {
                width: 2560,
                height: 1440,
                refresh: Some(165)
            })
        );
        // A resolution alone means its highest refresh.
        assert_eq!(
            mode("1920x1080"),
            Some(Wanted::Exact {
                width: 1920,
                height: 1080,
                refresh: None
            })
        );
        // What a mode list prints, and what people type.
        assert_eq!(
            mode("2560x1440@59.94"),
            Some(Wanted::Exact {
                width: 2560,
                height: 1440,
                refresh: Some(60)
            })
        );
        assert_eq!(
            mode(" 2560 x 1440 @ 165 Hz "),
            Some(Wanted::Exact {
                width: 2560,
                height: 1440,
                refresh: Some(165)
            })
        );
        assert_eq!(mode("preferred"), Some(Wanted::Preferred));
        assert_eq!(mode("best"), Some(Wanted::Best));
        assert_eq!(mode("widest"), Some(Wanted::Widest));
        // Unreadable rather than guessed at: the caller says which monitor and
        // falls back, which is a line in the log instead of a wrong mode.
        assert_eq!(mode("2560x"), None);
        assert_eq!(mode("2560x1440@"), None);
        assert_eq!(mode("2560x1440@0"), None);
        assert_eq!(mode("as big as it goes"), None);
    }

    #[test]
    fn transforms_and_alignments_parse_the_names_people_write() {
        assert_eq!(transform("90"), Some(Transform::_90));
        assert_eq!(transform("Flipped-180"), Some(Transform::Flipped180));
        assert_eq!(transform("normal"), Some(Transform::Normal));
        assert_eq!(transform("sideways"), None);
        assert_eq!(align("center"), Some(Align::Centre));
        assert_eq!(align("centre"), Some(Align::Centre));
        assert_eq!(align("bottom"), Some(Align::End));
        assert_eq!(align("askew"), None);
    }
}
