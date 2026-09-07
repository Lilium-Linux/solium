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
    utils::{Logical, Point, Rectangle, Size},
};

/// One line of the configured arrangement.
#[derive(Clone, Debug)]
pub(crate) struct Placement {
    /// The connector name, as the kernel reports it: `DP-1`, `HDMI-A-1`,
    /// `eDP-1`. `--probe` prints the ones this machine has.
    pub(crate) name: String,
    /// Where its top-left corner goes in the global space.
    pub(crate) at: Point<i32, Logical>,
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

    /// Where each of `monitors` goes, in the order given.
    ///
    /// Named ones take their configured position. The rest go to the right of
    /// everything placed so far — *including* the configured ones, so adding a
    /// third screen to a configuration that names two does not land it on top
    /// of one of them.
    ///
    /// Always returns one position per monitor. A layout that dropped an output
    /// would be a black screen with no error, so there is no path here that
    /// can leave one out.
    pub(crate) fn place(
        &self,
        monitors: &[(String, Size<i32, Logical>)],
    ) -> Vec<Point<i32, Logical>> {
        let mut placed: Vec<Option<Point<i32, Logical>>> = vec![None; monitors.len()];

        for (index, (name, _)) in monitors.iter().enumerate() {
            if let Some(placement) = self
                .placements
                .iter()
                .find(|placement| placement.name == *name)
            {
                placed[index] = Some(placement.at);
            }
        }

        // The right edge of everything decided so far, so an unconfigured
        // monitor lands beside the others rather than under them.
        let mut edge = placed
            .iter()
            .zip(monitors)
            .filter_map(|(at, (_, size))| at.map(|at| at.x + size.w))
            .max()
            .unwrap_or(0);

        for (index, (_, size)) in monitors.iter().enumerate() {
            if placed[index].is_some() {
                continue;
            }
            placed[index] = Some((edge, 0).into());
            edge += size.w;
        }

        // Every entry was filled by the loop above; the fallback keeps the
        // signature honest rather than describing a reachable case.
        placed
            .into_iter()
            .map(|at| at.unwrap_or_default())
            .collect()
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

    fn monitors(names: &[(&str, i32, i32)]) -> Vec<(String, Size<i32, Logical>)> {
        names
            .iter()
            .map(|(name, w, h)| ((*name).to_owned(), Size::from((*w, *h))))
            .collect()
    }

    #[test]
    fn one_monitor_is_at_the_origin() {
        let placed = Arrangement::default().place(&monitors(&[("eDP-1", 1920, 1080)]));
        assert_eq!(placed, vec![Point::from((0, 0))]);
    }

    #[test]
    fn unconfigured_monitors_go_left_to_right() {
        // The default guess, and the reason it is a guess: enumeration order
        // is the only thing available and it says nothing about the desk.
        let placed = Arrangement::default().place(&monitors(&[
            ("DP-1", 2560, 1440),
            ("HDMI-A-1", 1920, 1080),
            ("DP-2", 1280, 1024),
        ]));
        assert_eq!(
            placed,
            vec![
                Point::from((0, 0)),
                Point::from((2560, 0)),
                Point::from((4480, 0)),
            ]
        );
    }

    #[test]
    fn a_configured_monitor_takes_its_position() {
        let arrangement = Arrangement::new(vec![
            Placement {
                name: "HDMI-A-1".to_owned(),
                at: (0, 0).into(),
            },
            Placement {
                name: "DP-1".to_owned(),
                at: (1920, 200).into(),
            },
        ]);
        // Enumerated in the other order, on purpose: the configuration decides
        // the arrangement, and the kernel's order must not be able to override
        // it.
        let placed =
            arrangement.place(&monitors(&[("DP-1", 2560, 1440), ("HDMI-A-1", 1920, 1080)]));
        assert_eq!(placed, vec![Point::from((1920, 200)), Point::from((0, 0))]);
    }

    #[test]
    fn an_unconfigured_monitor_lands_beside_the_configured_ones() {
        // A third screen plugged into a configuration that names two. Placing
        // it at the origin would put it on top of one of them, which looks
        // like both being broken rather than one being unconfigured.
        let arrangement = Arrangement::new(vec![Placement {
            name: "DP-1".to_owned(),
            at: (0, 0).into(),
        }]);
        let placed =
            arrangement.place(&monitors(&[("DP-1", 2560, 1440), ("HDMI-A-1", 1920, 1080)]));
        assert_eq!(placed, vec![Point::from((0, 0)), Point::from((2560, 0))]);
    }

    #[test]
    fn every_monitor_gets_a_position() {
        // The property that matters more than any particular arrangement: a
        // monitor with no position is a screen that is on and black.
        let arrangement = Arrangement::new(vec![Placement {
            name: "nothing-called-this".to_owned(),
            at: (100, 100).into(),
        }]);
        let all = monitors(&[("DP-1", 800, 600), ("DP-2", 800, 600), ("DP-3", 800, 600)]);
        assert_eq!(arrangement.place(&all).len(), all.len());
    }

    #[test]
    fn a_name_nothing_answers_to_is_reported() {
        let arrangement = Arrangement::new(vec![
            Placement {
                name: "DP-1".to_owned(),
                at: (0, 0).into(),
            },
            Placement {
                name: "DP-9".to_owned(),
                at: (0, 0).into(),
            },
        ]);
        assert_eq!(
            arrangement.unmatched(&monitors(&[("DP-1", 800, 600)])),
            vec!["DP-9"]
        );
    }
}
