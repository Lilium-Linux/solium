//! A transform that names a selection.
//!
//! Everything the compositor draws was addressable one way — a pane, by the id
//! a script holds it as — and that one way is why `workspaces.lua` moved every
//! window in a loop while the wallpaper stayed where it was. A surface was
//! *drawn* and not *addressable*, so there was nothing to put in the same
//! sentence as the windows.
//!
//! ## The model
//!
//! **A transform names a selection, and selections compose.** A group is not a
//! new kind of thing on screen; it is a *name for a selection*, and the
//! transform on it composes with whatever its members are already doing:
//!
//! ```lua
//! sol.group("desk-2", { windows = { 3, 7 }, surfaces = { "wallpaper-2" } })
//! sol.present_group("desk-2", { x = -2560 }, { duration = 300 })
//! ```
//!
//! One call, one animation, one target. The wallpaper travels because it is in
//! the selection, **not because the compositor knows what a wallpaper is** — and
//! it still does not. `sol.surface` is one primitive doing five jobs and that
//! stays true.
//!
//! Composition is the point rather than a detail. A window individually tilted
//! inside a moving desk stays tilted *within* it, which is the difference
//! between a desk and "every window, moved by hand, and hope".
//!
//! ## Where membership lives, and why here
//!
//! **On the group.** A `Group` owns its member list and its own in-flight
//! transform, and both leave when it does.
//!
//! The alternative — a field on `Pane`, or a table on `Solium` keyed by
//! `PaneId` — was rejected twice over. A sixth `HashMap<PaneId, _>` beside the
//! panes is exactly the shape the five changes before this one deleted: a table
//! that has to be reconciled by hand and is kept for ever when it is not. And a
//! field on `Pane` has nowhere to put the other two kinds of member: a
//! `scripted::Surface` would need the same field again, and a monitor is a
//! Smithay `Output` with no room for one at all. Membership on the group is the
//! only home all three kinds share.
//!
//! What a dead member costs here is a `u64` in a `Vec` the script itself
//! declared — not a table entry holding megabytes. A group naming a pane that
//! has closed resolves to nothing, draws nothing, and needs no sweep.
//!
//! ## What stays cheap
//!
//! A node in no group pays one `Vec::is_empty`. [`Shift::apply`] on an identity
//! shift returns the frame it was given, so a desktop with no groups emits
//! exactly the elements it emitted before, through the same path. That rule is
//! load-bearing: a compositor that routes every window through new arithmetic to
//! support a selection nobody has declared has made every frame worse.

use std::time::Duration;

use crate::{
    mat4::Mat4,
    present::{Blend, Curve, Frame, Transform},
    scripted::SurfaceId,
};

/// What a transform on a selection does to each member of it.
///
/// **Relative, where a [`Frame`] is absolute.** A group has no rectangle of its
/// own — "the windows on desk 2" is not a shape — so there is no destination to
/// animate towards, only a displacement to animate *by*. `{ x = -2560 }` is a
/// screen to the left of wherever each member already is, which is what lets one
/// call move a window, a wallpaper and a scrim that are nowhere near each other.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Shift {
    /// Logical pixels, added to where each member is drawn.
    pub(crate) dx: f64,
    pub(crate) dy: f64,
    /// Multiplied into each member's own opacity, so a half-faded window in a
    /// half-faded desk is a quarter there rather than a half.
    pub(crate) opacity: f32,
    /// Composed with each member's own matrix, the member's applied first.
    ///
    /// **About each member's own centre**, which is the honest limit of this
    /// stage: it tilts every window in the selection in place rather than
    /// turning the desk they sit on. A common pivot needs a rectangle for the
    /// group and a `pivot` on the node, and both are `z`, `pivot`, node alpha —
    /// the last item of this phase. What composes correctly *today* is the part
    /// that made this item worth doing first: the translation and the opacity,
    /// which is what a desk is made of.
    pub(crate) matrix: Mat4,
}

impl Shift {
    /// Doing nothing, which is what a node in no group gets.
    pub(crate) const NONE: Self = Self {
        dx: 0.0,
        dy: 0.0,
        opacity: 1.0,
        matrix: Mat4::IDENTITY,
    };

    /// Whether this changes anything at all.
    ///
    /// The gate on the cheap path, and it is checked rather than assumed: a
    /// group declared and never presented has an identity shift, and a member of
    /// one must cost exactly what a member of none costs.
    pub(crate) fn is_identity(&self) -> bool {
        self.dx == 0.0
            && self.dy == 0.0
            && (self.opacity - 1.0).abs() < f32::EPSILON
            && self.matrix.is_identity()
    }

    /// Both, for a node that is in two selections at once.
    ///
    /// Offsets add, opacities multiply, matrices compose with `self` first.
    /// Order matters only for the matrices, and the order is declaration order —
    /// stated rather than left to be discovered, because two groups with
    /// rotations in them is the one case where it is visible.
    fn and(self, then: Self) -> Self {
        Self {
            dx: self.dx + then.dx,
            dy: self.dy + then.dy,
            opacity: self.opacity * then.opacity,
            matrix: self.matrix.then(then.matrix),
        }
    }

    /// One member's frame, as this selection has it drawn.
    ///
    /// Returns its argument untouched when there is nothing to do — see the
    /// module header for why that is a rule and not an optimisation.
    pub(crate) fn apply(self, frame: Frame) -> Frame {
        if self.is_identity() {
            return frame;
        }
        Frame {
            opacity: frame.opacity * self.opacity,
            matrix: frame.matrix.then(self.matrix),
            ..frame.shifted(self.dx, self.dy)
        }
    }

    /// How far this displaces something, which is all a rebase needs.
    pub(crate) const fn offset(self) -> (f64, f64) {
        (self.dx, self.dy)
    }
}

impl Blend for Shift {
    fn blend(self, other: Self, progress: f64) -> Self {
        Self {
            dx: solium_animation::lerp(self.dx, other.dx, progress),
            dy: solium_animation::lerp(self.dy, other.dy, progress),
            #[expect(
                clippy::cast_possible_truncation,
                reason = "an opacity is a small float either way"
            )]
            opacity: solium_animation::lerp(
                f64::from(self.opacity),
                f64::from(other.opacity),
                progress,
            ) as f32,
            matrix: self.matrix.blend(other.matrix, progress),
        }
    }
}

/// One thing a selection can name.
///
/// The vocabulary from the design's table, less the two that are not addressable
/// yet: a *layer* within a pane's style has no layers to name until Phase 3, and
/// a group naming another group is recursion nobody has asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Member {
    /// A window, by the id scripts hold it as.
    Window(u64),
    /// A surface a script declared. See [`SurfaceId`] for why this is a number
    /// and not the name the script wrote.
    Surface(SurfaceId),
    /// Every node drawn on one monitor, by connector name.
    ///
    /// The selection that *is* an output — what a whole-screen transform or a
    /// global shader names. Live rather than expanded at declaration, so a
    /// window opened on that screen afterwards is in it.
    ///
    /// **Windows and scripted surfaces, not layer-shell clients.** A bar that
    /// is somebody else's client is drawn from its own layer map and reaches the
    /// frame without passing through a pane's transform at all; giving it one is
    /// its own change and the design defers it.
    Monitor(Box<str>),
}

/// Who is in a group.
///
/// `on` is here rather than on each surface because it is a property of the
/// *selection*: "desk 2 on DP-1" is one idea, and spelling the monitor out
/// against every surface in it is three chances to spell it differently. A
/// surface declared `on = "every-monitor"` is several things wearing one name,
/// and a desk wants one of them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Selection {
    pub(crate) members: Vec<Member>,
    /// Narrows the surfaces in this selection to one monitor's instance.
    /// `None` takes every instance, which is what a group that is not about a
    /// particular screen means.
    pub(crate) on: Option<Box<str>>,
}

impl Selection {
    fn holds_window(&self, id: u64, monitor: Option<&str>) -> bool {
        self.members.iter().any(|member| match member {
            Member::Window(each) => *each == id,
            Member::Monitor(name) => monitor == Some(&**name),
            Member::Surface(_) => false,
        })
    }

    fn holds_surface(&self, id: SurfaceId, monitor: &str) -> bool {
        if self.on.as_deref().is_some_and(|only| only != monitor) {
            return false;
        }
        self.members.iter().any(|member| match member {
            Member::Surface(each) => *each == id,
            Member::Monitor(name) => &**name == monitor,
            Member::Window(_) => false,
        })
    }

    /// The windows this names outright, for working out what has to be rebased.
    fn named_windows(&self) -> impl Iterator<Item = u64> + '_ {
        self.members.iter().filter_map(|member| match member {
            Member::Window(id) => Some(*id),
            Member::Surface(_) | Member::Monitor(_) => None,
        })
    }

    fn names_a_monitor(&self) -> bool {
        self.members
            .iter()
            .any(|member| matches!(member, Member::Monitor(_)))
    }
}

/// A named selection, and where it is being carried.
#[derive(Debug)]
struct Group {
    name: Box<str>,
    selection: Selection,
    /// `None` means "not moved", which is not the same as "moved by nothing":
    /// an identity shift still costs the fold below a visit, and a group that
    /// has landed and been released costs nothing at all.
    travel: Option<Transform<Shift>>,
}

impl Group {
    fn shift(&self, now: Duration) -> Shift {
        self.travel
            .map_or(Shift::NONE, |transform| transform.at(now))
    }
}

/// Every selection a script has named.
///
/// A `Vec` and not a map: there are as many of these as a configuration writes
/// down — a handful — and every question asked of them walks all of them anyway,
/// because a node can be in more than one.
#[derive(Debug, Default)]
pub(crate) struct Groups {
    groups: Vec<Group>,
    /// Whether any selection names a monitor, so the compositor can skip working
    /// out which screen a window is on when nothing has asked.
    monitors_named: bool,
}

/// What a membership change displaces, and by how far.
///
/// Handed back rather than applied here, because putting a window back where it
/// was is `present::rebase`'s job and this module holds no panes.
pub(crate) type Displaced = Vec<(u64, (f64, f64))>;

impl Groups {
    /// Whether anything at all is grouped. The first gate on the cheap path.
    pub(crate) fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// Whether any selection names a monitor rather than its contents.
    ///
    /// Read before asking the compositor which screen a pane is on: that is a
    /// geometric search per pane per frame, and no shipped configuration pays
    /// for it.
    pub(crate) const fn names_monitors(&self) -> bool {
        self.monitors_named
    }

    fn find(&self, name: &str) -> Option<&Group> {
        self.groups.iter().find(|group| &*group.name == name)
    }

    /// Name a selection, or say who is in one that already has a name.
    ///
    /// Re-declaring replaces the membership and **keeps the transform**, which
    /// is what makes a mode that rebuilds its groups every time it runs — which
    /// is every mode — not restart its own animation.
    ///
    /// Returns the windows whose total displacement changed as a result, so the
    /// caller can keep them looking where they are. See [`present::rebase`] for
    /// why that matters: without it, sending a window to another workspace stops
    /// sliding and starts teleporting.
    pub(crate) fn declare(&mut self, name: &str, selection: Selection, now: Duration) -> Displaced {
        if self
            .find(name)
            .is_some_and(|group| group.selection == selection)
        {
            return Displaced::new();
        }
        let watched = self.watched(name, &selection);
        let before = self.offsets(&watched, now);
        match self.groups.iter_mut().find(|group| &*group.name == name) {
            Some(group) => group.selection = selection,
            None => self.groups.push(Group {
                name: name.into(),
                selection,
                travel: None,
            }),
        }
        self.refresh_monitors();
        self.displaced(&watched, &before, now)
    }

    /// Forget a selection. Its members go back to being drawn where they live.
    pub(crate) fn forget(&mut self, name: &str, now: Duration) -> Displaced {
        let Some(group) = self.find(name) else {
            return Displaced::new();
        };
        let watched: Vec<u64> = group.selection.named_windows().collect();
        let before = self.offsets(&watched, now);
        self.groups.retain(|group| &*group.name != name);
        self.refresh_monitors();
        self.displaced(&watched, &before, now)
    }

    /// Carry a named selection to `to`, over `duration`.
    ///
    /// Nothing happens to a name nobody has declared: a transform on a selection
    /// that does not exist has no members to apply it to, and inventing an empty
    /// group to hold it would mean a typo silently becoming a thing.
    pub(crate) fn present(
        &mut self,
        name: &str,
        to: Shift,
        now: Duration,
        duration: Duration,
        easing: Curve,
    ) -> bool {
        let Some(group) = self.groups.iter_mut().find(|group| &*group.name == name) else {
            return false;
        };
        let from = group
            .travel
            .map_or(Shift::NONE, |transform| transform.at(now));
        // Never released: a selection asked to sit a screen to the left has to
        // stay there. What releases is `clear`, which is the call that means
        // "back to nothing".
        group.travel = Some(Transform::new(from, to, now, duration, easing, false));
        true
    }

    /// Carry a selection back to doing nothing, and then stop carrying it.
    pub(crate) fn clear(
        &mut self,
        name: &str,
        now: Duration,
        duration: Duration,
        easing: Curve,
    ) -> bool {
        let Some(group) = self.groups.iter_mut().find(|group| &*group.name == name) else {
            return false;
        };
        let Some(current) = group.travel else {
            return false;
        };
        group.travel = Some(Transform::new(
            current.at(now),
            Shift::NONE,
            now,
            duration,
            easing,
            true,
        ));
        true
    }

    /// How a window is carried this instant, across every selection it is in.
    ///
    /// `monitor` is its screen's connector name, and only has to be worked out
    /// when [`Self::names_monitors`] says something asked.
    pub(crate) fn on_window(&self, id: u64, monitor: Option<&str>, now: Duration) -> Shift {
        self.fold(now, |group| group.selection.holds_window(id, monitor))
    }

    /// The same, for one monitor's instance of a scripted surface.
    pub(crate) fn on_surface(&self, id: SurfaceId, monitor: &str, now: Duration) -> Shift {
        self.fold(now, |group| group.selection.holds_surface(id, monitor))
    }

    fn fold(&self, now: Duration, wanted: impl Fn(&Group) -> bool) -> Shift {
        if self.groups.is_empty() {
            return Shift::NONE;
        }
        self.groups
            .iter()
            .filter(|group| wanted(group))
            .fold(Shift::NONE, |so_far, group| so_far.and(group.shift(now)))
    }

    /// Retire transforms that have landed. Returns whether any is still moving.
    ///
    /// Every group is visited deliberately, exactly as `Solium::settle` visits
    /// every pane: a short-circuit would leave later selections carried for ever.
    pub(crate) fn settle(&mut self, now: Duration) -> bool {
        let mut animating = false;
        for group in &mut self.groups {
            let Some(travel) = group.travel else {
                continue;
            };
            if travel.finished(now) {
                if travel.releases() {
                    group.travel = None;
                }
            } else {
                animating = true;
            }
        }
        animating
    }

    /// The windows a declaration could move: the ones it names now, and the ones
    /// the selection of that name used to hold.
    ///
    /// Only windows named outright. A `Member::Monitor` selection changes what
    /// is on a screen when the *user* drags a window across a bezel, which no
    /// declaration observes — so there is nothing here a rebase could be honest
    /// about, and pretending otherwise would move windows nobody moved.
    fn watched(&self, name: &str, incoming: &Selection) -> Vec<u64> {
        let mut watched: Vec<u64> = incoming.named_windows().collect();
        if let Some(group) = self.find(name) {
            for id in group.selection.named_windows() {
                if !watched.contains(&id) {
                    watched.push(id);
                }
            }
        }
        watched
    }

    fn offsets(&self, windows: &[u64], now: Duration) -> Vec<(f64, f64)> {
        windows
            .iter()
            .map(|id| self.on_window(*id, None, now).offset())
            .collect()
    }

    /// What each watched window has to be put back by to stay where it looks.
    fn displaced(&self, windows: &[u64], before: &[(f64, f64)], now: Duration) -> Displaced {
        windows
            .iter()
            .zip(before)
            .filter_map(|(id, was)| {
                let now = self.on_window(*id, None, now).offset();
                let by = (was.0 - now.0, was.1 - now.1);
                (by.0 != 0.0 || by.1 != 0.0).then_some((*id, by))
            })
            .collect()
    }

    fn refresh_monitors(&mut self) {
        self.monitors_named = self
            .groups
            .iter()
            .any(|group| group.selection.names_a_monitor());
    }
}

#[cfg(test)]
mod tests {
    use super::{Group, Groups, Member, Selection, Shift};
    use crate::present::{Blend, Curve, Frame};
    use smithay::utils::{Logical, Rectangle};
    use std::time::Duration;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    fn by(dx: f64, dy: f64) -> Shift {
        Shift {
            dx,
            dy,
            ..Shift::NONE
        }
    }

    fn desk(windows: &[u64], surfaces: &[u32]) -> Selection {
        Selection {
            members: windows
                .iter()
                .map(|id| Member::Window(*id))
                .chain(
                    surfaces
                        .iter()
                        .map(|id| Member::Surface(crate::scripted::SurfaceId::from_raw(*id))),
                )
                .collect(),
            on: None,
        }
    }

    fn at(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    /// **A node in no group is drawn exactly as it was.**
    ///
    /// The rule the whole design rests on, asserted on the value rather than
    /// argued: an identity shift returns the frame it was handed, bit for bit,
    /// so a desktop that has declared nothing emits what it emitted before.
    #[test]
    fn nothing_grouped_costs_nothing() {
        let groups = Groups::default();
        assert!(groups.is_empty());
        assert!(!groups.names_monitors());

        let frame = Frame::real(rect(100, 200, 800, 600));
        let shift = groups.on_window(1, None, at(0));
        assert_eq!(shift, Shift::NONE);
        assert_eq!(shift.apply(frame), frame);

        // And a group that exists but has never been carried is the same.
        let mut declared = Groups::default();
        declared.declare("desk", desk(&[1], &[]), at(0));
        assert!(declared.on_window(1, None, at(0)).is_identity());
        assert_eq!(declared.on_window(1, None, at(0)).apply(frame), frame);
    }

    /// **A window and a surface in one selection move by the same amount, at
    /// every instant of the animation and not merely at the end.**
    ///
    /// This is the whole of "the workspace slide stops leaving the wallpaper
    /// behind", reduced to the arithmetic that makes it true. Checking only the
    /// destination would pass for two things animated separately on two clocks,
    /// which is exactly the bug — they arrive together and travel apart.
    #[test]
    fn a_window_and_a_surface_travel_as_one() {
        let mut groups = Groups::default();
        let wallpaper = crate::scripted::SurfaceId::from_raw(7);
        groups.declare("desk-2", desk(&[3, 9], &[7]), at(0));
        groups.present("desk-2", by(-2560.0, 0.0), at(0), at(300), Curve::OutCubic);

        let mut moved = false;
        for step in 0..=10 {
            let now = at(step * 30);
            let window = groups.on_window(3, None, now).offset();
            let other = groups.on_window(9, None, now).offset();
            let surface = groups.on_surface(wallpaper, "DP-1", now).offset();
            assert_eq!(
                window, surface,
                "at {now:?} the wallpaper was {surface:?} and the window {window:?}: \
                 one call, one animation, and they are already apart"
            );
            assert_eq!(window, other, "two windows in one selection disagreed");
            moved |= window.0 != 0.0;
        }
        assert!(moved, "nothing moved at all; the test proves nothing");
        assert_eq!(
            groups.on_surface(wallpaper, "DP-1", at(300)).offset(),
            (-2560.0, 0.0)
        );
    }

    /// **A surface a group narrows to one monitor moves only there.**
    ///
    /// `on = "every-monitor"` makes one name into several things, and a desk is
    /// about one screen. Without the narrowing, switching the left monitor's
    /// workspace slides the right monitor's wallpaper off too.
    #[test]
    fn a_monitor_scoped_selection_leaves_the_other_screen_alone() {
        let mut groups = Groups::default();
        let wallpaper = crate::scripted::SurfaceId::from_raw(7);
        let mut selection = desk(&[], &[7]);
        selection.on = Some("DP-1".into());
        groups.declare("desk-2@DP-1", selection, at(0));
        groups.present("desk-2@DP-1", by(-2560.0, 0.0), at(0), at(0), Curve::Linear);

        assert_eq!(
            groups.on_surface(wallpaper, "DP-1", at(0)).offset(),
            (-2560.0, 0.0)
        );
        assert_eq!(
            groups.on_surface(wallpaper, "HDMI-A-1", at(0)).offset(),
            (0.0, 0.0)
        );
    }

    /// **A member's own transform survives the group's, and composes with it.**
    ///
    /// The claim that makes a group different from a loop: a window tilted
    /// inside a moving desk stays tilted, and moves.
    #[test]
    fn a_members_own_transform_composes_rather_than_being_replaced() {
        let mut groups = Groups::default();
        groups.declare("desk", desk(&[1], &[]), at(0));
        groups.present("desk", by(-100.0, 25.0), at(0), at(0), Curve::Linear);

        let tilted = Frame {
            matrix: crate::mat4::Mat4::rotate_y(0.4),
            ..Frame::real(rect(10, 20, 300, 200))
        }
        .with_opacity(0.5);
        let drawn = groups.on_window(1, None, at(0)).apply(tilted);

        assert_eq!(drawn.rect.loc.x, -90.0);
        assert_eq!(drawn.rect.loc.y, 45.0);
        assert_eq!(
            drawn.rect.size, tilted.rect.size,
            "a translation must not resize anything"
        );
        assert_eq!(
            drawn.matrix, tilted.matrix,
            "the member's own tilt is still exactly its own tilt"
        );
        assert!((drawn.opacity - 0.5).abs() < f32::EPSILON);
    }

    /// **Two selections over one window both apply.**
    #[test]
    fn selections_compose() {
        let mut groups = Groups::default();
        groups.declare("desk", desk(&[1], &[]), at(0));
        groups.declare("screen", desk(&[1], &[]), at(0));
        groups.present("desk", by(-100.0, 0.0), at(0), at(0), Curve::Linear);
        groups.present(
            "screen",
            Shift {
                dx: 0.0,
                dy: -40.0,
                opacity: 0.5,
                ..Shift::NONE
            },
            at(0),
            at(0),
            Curve::Linear,
        );
        let shift = groups.on_window(1, None, at(0));
        assert_eq!(shift.offset(), (-100.0, -40.0));
        assert!((shift.opacity - 0.5).abs() < f32::EPSILON);
    }

    /// **A window that changes selection is put back where it was.**
    ///
    /// `super+shift+2` sends a window to another workspace, and with a group per
    /// desk that is a membership change rather than a transform. The difference
    /// between the two desks' offsets lands on the window in one frame, so
    /// without the displacement reported here it teleports — a shipped binding
    /// that used to slide.
    #[test]
    fn moving_between_selections_reports_what_has_to_be_put_back() {
        let mut groups = Groups::default();
        groups.declare("desk-1", desk(&[1], &[]), at(0));
        groups.declare("desk-2", desk(&[], &[]), at(0));
        groups.present("desk-2", by(-2560.0, 0.0), at(0), at(0), Curve::Linear);

        // Window 1 leaves desk 1 for desk 2, which is a screen to the left.
        let out = groups.declare("desk-1", desk(&[], &[]), at(0));
        assert!(out.is_empty(), "leaving a desk that is not carried is free");
        let displaced = groups.declare("desk-2", desk(&[1], &[]), at(0));
        assert_eq!(
            displaced,
            vec![(1, (2560.0, 0.0))],
            "the window has to be put a screen back to the right to stay \
             where it is looking"
        );
    }

    /// And nothing is reported when nothing actually moved.
    #[test]
    fn joining_a_selection_that_is_going_nowhere_displaces_nothing() {
        let mut groups = Groups::default();
        groups.declare("desk-1", desk(&[], &[]), at(0));
        assert!(
            groups
                .declare("desk-1", desk(&[1, 2], &[]), at(0))
                .is_empty()
        );
        // Re-declaring the same membership is not a change at all.
        assert!(
            groups
                .declare("desk-1", desk(&[1, 2], &[]), at(0))
                .is_empty()
        );
    }

    /// **Forgetting a selection puts its members back.**
    #[test]
    fn forgetting_a_selection_reports_the_way_back() {
        let mut groups = Groups::default();
        groups.declare("desk-2", desk(&[4], &[]), at(0));
        groups.present("desk-2", by(0.0, -1440.0), at(0), at(0), Curve::Linear);
        assert_eq!(
            groups.forget("desk-2", at(0)),
            vec![(4, (0.0, -1440.0))],
            "a window whose desk has gone is drawn where the desk left it, \
             and animates home from there"
        );
        assert!(groups.is_empty());
        assert!(groups.forget("desk-2", at(0)).is_empty());
    }

    /// **A selection that is a monitor takes what is on it, live.**
    ///
    /// "The whole screen", nameable — which is the other half of what this item
    /// exists for, and what a global shader will select with.
    #[test]
    fn a_monitor_is_a_selection() {
        let mut groups = Groups::default();
        groups.declare(
            "screen",
            Selection {
                members: vec![Member::Monitor("DP-1".into())],
                on: None,
            },
            at(0),
        );
        assert!(groups.names_monitors());
        groups.present("screen", by(0.0, 30.0), at(0), at(0), Curve::Linear);

        // A window nobody named, on that screen, is in it.
        assert_eq!(
            groups.on_window(99, Some("DP-1"), at(0)).offset(),
            (0.0, 30.0)
        );
        assert_eq!(
            groups.on_window(99, Some("HDMI-A-1"), at(0)).offset(),
            (0.0, 0.0)
        );
        // And so is a surface drawn there, without being named either.
        let bar = crate::scripted::SurfaceId::from_raw(2);
        assert_eq!(groups.on_surface(bar, "DP-1", at(0)).offset(), (0.0, 30.0));
        assert_eq!(
            groups.on_surface(bar, "HDMI-A-1", at(0)).offset(),
            (0.0, 0.0)
        );

        groups.forget("screen", at(0));
        assert!(
            !groups.names_monitors(),
            "the compositor must stop paying for a question nobody is asking"
        );
    }

    /// **A carried selection stops asking for frames when it lands.**
    ///
    /// The same shape `present::settle` has for a pane, and for the same reason:
    /// a group animates on the compositor's clock and damages nothing, so the
    /// next frame only comes because this said so — and a `true` that never
    /// becomes `false` is a compositor that never sleeps.
    #[test]
    fn a_selection_that_lands_lets_the_loop_go_idle() {
        let mut groups = Groups::default();
        groups.declare("desk", desk(&[1], &[]), at(0));
        groups.present("desk", by(-2560.0, 0.0), at(0), at(300), Curve::OutCubic);

        assert!(groups.settle(at(150)), "still travelling");
        assert!(!groups.settle(at(300)), "it has arrived");
        assert_eq!(
            groups.on_window(1, None, at(600)).offset(),
            (-2560.0, 0.0),
            "and it stays where it was carried; only `clear` brings it back"
        );

        groups.clear("desk", at(600), at(300), Curve::OutCubic);
        assert!(groups.settle(at(750)));
        assert!(!groups.settle(at(900)));
        assert!(
            groups.on_window(1, None, at(900)).is_identity(),
            "a cleared selection is back to costing nothing"
        );
    }

    /// A transform on a name nobody declared does nothing, rather than inventing
    /// an empty selection for a typo to live in.
    #[test]
    fn carrying_a_name_nobody_declared_does_nothing() {
        let mut groups = Groups::default();
        assert!(!groups.present("desk-9", by(10.0, 0.0), at(0), at(0), Curve::Linear));
        assert!(!groups.clear("desk-9", at(0), at(0), Curve::Linear));
        assert!(groups.is_empty());
    }

    /// Re-declaring a selection keeps it where it is rather than restarting it.
    ///
    /// Every mode rebuilds its groups each time it runs — `workspaces.apply`
    /// does exactly that — so a declaration that reset the transform would make
    /// a slide start over each time a window opened.
    #[test]
    fn redeclaring_a_selection_does_not_restart_its_animation() {
        let mut groups = Groups::default();
        groups.declare("desk", desk(&[1], &[]), at(0));
        groups.present("desk", by(-1000.0, 0.0), at(0), at(200), Curve::Linear);
        let midway = groups.on_window(1, None, at(100)).offset();
        assert_eq!(midway, (-500.0, 0.0));

        groups.declare("desk", desk(&[1, 2], &[]), at(100));
        assert_eq!(
            groups.on_window(1, None, at(100)).offset(),
            midway,
            "the slide carried on from where it was"
        );
        assert_eq!(groups.on_window(1, None, at(200)).offset(), (-1000.0, 0.0));
    }

    /// A blend of two shifts is what the holder in `present.rs` asks of it.
    #[test]
    fn a_shift_blends_every_part_of_itself() {
        let from = Shift::NONE;
        let to = Shift {
            dx: 100.0,
            dy: -50.0,
            opacity: 0.0,
            matrix: crate::mat4::Mat4::IDENTITY,
        };
        let half = from.blend(to, 0.5);
        assert_eq!(half.dx, 50.0);
        assert_eq!(half.dy, -25.0);
        assert!((half.opacity - 0.5).abs() < f32::EPSILON);
    }

    /// The `Group` type is private; this keeps the field in the test module's
    /// view so a future reader can see there is exactly one transform per group.
    #[test]
    fn a_group_holds_one_transform() {
        let group = Group {
            name: "desk".into(),
            selection: Selection::default(),
            travel: None,
        };
        assert!(group.shift(at(0)).is_identity());
    }
}
