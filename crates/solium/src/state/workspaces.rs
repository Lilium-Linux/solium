//! Workspaces and groups: how the selections a pane or a scripted surface is in carry it, and
//! whether a window, a desk or every window is on stage -- headed somewhere the user can see it.

use super::*;

/// How far past the present a question about where a window is *going* looks.
///
/// **Focus judges where a window is settling; the pointer judges where it is
/// drawn.** That is the rule, and this constant is the line between its two
/// halves. A click lands on what is on screen this frame, so the hit test —
/// [`Solium::window_under`], through `Frame::covers` — samples the present
/// and must go on doing so. Focus is a decision about what the user is about
/// to work with, so it belongs to the destination: [`Solium::settle_focus`]
/// and [`Solium::everything_is_off_stage`] both ask [`Solium::drawn_at`] this
/// far ahead, through [`Solium::settling`]. Where the two meet —
/// `settle_focus` asking which window is under the pointer — it is still a
/// focus decision, and it hit-tests the destination.
///
/// The cases, each pinned by the test named:
///
/// * a closing window is headed for `present::close`'s opacity zero, so it is
///   no candidate from the press onwards, even while it is still visibly
///   fading and still takes clicks —
///   `a_window_mid_close_under_the_pointer_is_not_handed_the_keyboard`;
/// * a window being given back is headed for full opacity, so it is a
///   candidate on the frame its restore starts, when it is still drawn at
///   nothing — `a_refused_close_gives_the_keyboard_back_to_the_window_it_brings_back`;
/// * a desk just switched to is headed on stage and a desk just left is headed
///   off, on the switch's first frame, by either arm —
///   `the_first_frame_of_a_workspace_switch_focuses_the_desk_switched_to` and
///   `a_pointer_over_the_desk_being_left_does_not_hand_it_the_keyboard`, which
///   also asserts the click on the same pixel on the same frame still goes to
///   what is drawn.
///
/// A reload *starts* the workspace slide; it does not finish it. Asking where
/// the windows are at that instant asks where they were before it, so the
/// question has to be put to a moment when the transforms have landed.
///
/// Only has to be longer than the longest animation a configuration can name,
/// and costs nothing for being longer than that: `Animation::progress` clamps
/// the elapsed time to the duration, so every extra second is the same
/// division. An hour is not a guess at how long an animation takes — it is far
/// enough that it cannot be one.
pub(super) const SETTLED: Duration = Duration::from_secs(60 * 60);

/// Whether none of these windows is drawn on any screen: each a slot, where
/// the window lives, and the rectangle it is drawn at.
///
/// **A drawn rectangle counts only on a screen its slot is on**, because that
/// is the only kind of screen that draws it: [`crate::render::drawn_on`], the
/// renderer's own cull. Until #134's third review this asked whether the
/// drawn rectangle reached *any* screen, and with two monitors side by side
/// the left one's workspace next door is carried onto the right one -- which
/// never draws it -- so its windows came back on stage.
/// `a_window_is_on_stage_only_on_a_monitor_that_draws_it` pins the rule, and
/// `on_two_monitors_a_window_on_the_left_monitors_hidden_workspace_is_not_on_stage`
/// the shipped workspaces meeting it.
///
/// `None` when there is nothing to ask about — no screens, or no windows. The
/// free function over plain rectangles, for the same reason [`super::monitors::anywhere_on`] is
/// one: `Solium` needs a `Display` and cannot be built in a unit test, so the
/// half that decides is the half kept testable. The instant the rectangles were
/// measured at is the caller's, and is the other half — see
/// [`Solium::everything_is_off_stage`], which got it wrong.
pub(super) fn nothing_on_stage(
    drawn: impl IntoIterator<Item = (Rectangle<i32, Logical>, Rectangle<f64, Logical>)>,
    screens: &[Rectangle<i32, Logical>],
) -> Option<bool> {
    if screens.is_empty() {
        return None;
    }
    let mut any = false;
    for (slot, rect) in drawn {
        any = true;
        if screens
            .iter()
            .any(|screen| crate::render::drawn_on(slot, *screen) && screen.to_f64().overlaps(rect))
        {
            return Some(false);
        }
    }
    any.then_some(true)
}

/// [`Solium::on_stage`]'s rule, for a window living at `slot` and drawn as
/// `frame`: it paints something, on a screen that draws it.
pub(super) fn staged(
    slot: Rectangle<i32, Logical>,
    frame: Frame,
    screens: &[Rectangle<i32, Logical>],
) -> bool {
    frame.shows() && nothing_on_stage([(slot, frame.rect)], screens) != Some(true)
}

/// Whether a selection headed for `bound` puts away the desk it carries:
/// carries it anywhere at all, or fades it to nothing as [`Frame::shows`]
/// counts nothing.
///
/// **Measured against no screen.** `workspaces.lua` parks a desk by its
/// monitor's work area times `spread`, not by the screen, and until #134's
/// sixth review this asked whether the screen, carried as the desk is headed,
/// was still on itself: at `spread = 1.0` a panel across the slide left every
/// parked desk overlapping its screen by the panel's thickness, and on stage.
/// `with_a_panel_across_a_horizontal_slide_a_genuine_activation_on_a_hidden_workspace_leaves_the_keyboard_exactly_where_it_was`,
/// `with_a_bar_across_a_vertical_slide_a_genuine_activation_on_a_hidden_workspace_leaves_the_keyboard_exactly_where_it_was`.
/// A selection that nudges a desk and leaves it on screen puts it away too.
///
/// [`Solium::carried_by_a_selection`]'s rule, over plain values for the reason
/// [`nothing_on_stage`] is one: `Solium` cannot be built in a unit test.
/// `a_desk_is_put_away_when_its_selection_is_headed_anywhere_or_to_nothing`.
pub(super) fn put_away(bound: crate::group::Shift) -> bool {
    let (dx, dy) = bound.offset();
    dx != 0.0 || dy != 0.0 || !bound.apply(Frame::real(Rectangle::default())).shows()
}

/// Whether a window is fullscreen, as the compositor last told it: the
/// state its next configure carries, which is set and unset in
/// `fullscreen_request` and `unfullscreen_request`.
/// `leaving_fullscreen_puts_the_bar_back_on_top`.
fn fullscreen(window: &Window) -> bool {
    window.toplevel().is_some_and(|toplevel| {
        toplevel.with_pending_state(|state| state.states.contains(xdg_toplevel::State::Fullscreen))
    })
}

impl Solium {
    /// How the selections a pane is in carry it at `now`: the half of
    /// [`Self::drawn_at`] that is the groups', before it is composed onto the
    /// pane's own frame -- which `Shift::apply` returns untouched when there
    /// is nothing to carry.
    ///
    /// [`Self::carried_by_a_selection`] asks the same selections where they
    /// are *headed*, `Groups::bound_for_window`, with the same monitor; see it
    /// for why that is not this asked late.
    pub(super) fn carried_at(
        &self,
        pane: &Pane,
        real: Rectangle<i32, Logical>,
        now: std::time::Duration,
    ) -> crate::group::Shift {
        // Carried by the selections it was in when its client went, as they
        // move now, and whoever they hold. See `crate::pane::Left::groups`.
        if let Some(left) = pane.left() {
            return self.groups.on_named(&left.groups, now);
        }
        if self.groups.is_empty() {
            return crate::group::Shift::NONE;
        }
        let monitor = self.named_monitor_of(real);
        self.groups
            .on_window(pane.id().get(), monitor.as_deref(), now)
    }

    /// The connector a pane is on, when a selection names a monitor.
    ///
    /// Only worked out when a selection has actually named a screen: this is
    /// a geometric search over the outputs, per pane, per frame.
    pub(super) fn named_monitor_of(&self, real: Rectangle<i32, Logical>) -> Option<String> {
        self.groups
            .names_monitors()
            .then(|| self.output_of(real).map(|output| output.name()))
            .flatten()
    }

    /// Where one monitor's instance of a scripted surface is actually drawn.
    ///
    /// The surface half of [`Self::drawn_at`], and deliberately a rectangle
    /// rather than a `Frame`: a selection reaches a surface as a displacement
    /// and an opacity, and no further. A matrix or a deformation on a group
    /// reaches its *panes* — bending a surface means capturing it into a texture
    /// first, and a scripted surface is a memory buffer on the software path,
    /// where there is no texture to bend. That is `offscreen::capture` for
    /// surfaces, which is a change of its own and not a line of this one.
    pub(crate) fn carried(
        &self,
        id: crate::scripted::SurfaceId,
        output: &Output,
        area: Rectangle<i32, Logical>,
    ) -> Rectangle<i32, Logical> {
        if self.groups.is_empty() {
            return area;
        }
        let (dx, dy) = self
            .groups
            .on_surface(id, &output.name(), self.clock.now())
            .offset();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a displacement on this desktop, in logical pixels"
        )]
        let moved = Rectangle::new(
            (
                area.loc.x + dx.round() as i32,
                area.loc.y + dy.round() as i32,
            )
                .into(),
            area.size,
        );
        moved
    }

    /// How much of a scripted surface a selection is showing.
    pub(crate) fn carried_alpha(&self, id: crate::scripted::SurfaceId, output: &Output) -> f32 {
        if self.groups.is_empty() {
            return 1.0;
        }
        self.groups
            .on_surface(id, &output.name(), self.clock.now())
            .opacity
    }

    /// The instant a question about where windows are *going* is put to.
    ///
    /// Far enough ahead that every transform running now has landed. One
    /// function, so the two focus readers cannot drift apart on it again: see
    /// [`SETTLED`] for the rule and the tests that pin it.
    pub(super) fn settling(&self) -> Duration {
        self.clock.now().saturating_add(SETTLED)
    }

    /// Whether a pane is headed somewhere the user can see it.
    ///
    /// **The two halves `Frame::covers` asks of a point — `shows()` and the
    /// rectangle — asked of the screens instead, and of the destination rather
    /// than the frame being drawn.** `landed` is [`Self::settling`]; [`SETTLED`]
    /// says why and names the tests. It was the present until #127's fourth
    /// review, and that put the question to a restore at its progress zero —
    /// which answers `present::close`'s opacity-zero end, so the window being
    /// given back declined itself — and to a workspace switch before it had
    /// moved anything.
    ///
    /// Through [`Self::drawn_at`] and not `pane_outer`, for one reason: a
    /// hidden workspace is **parked a screen away, not unmapped**. Its windows
    /// keep the rectangle their layout gave them and a selection carries them
    /// off-stage, so the real rectangle says they are on screen and only the
    /// drawn one knows better. See `workspaces.lua`, and
    /// `a_close_does_not_hand_the_keyboard_to_a_workspace_nobody_can_see`.
    ///
    /// **On a screen that draws it, and not merely on a screen.** A pane is
    /// drawn only on the monitors its slot is on -- the renderer's own cull,
    /// [`crate::render::drawn_on`] -- so its drawn rectangle is measured
    /// against those. Measured against every screen, as it was until #134's
    /// third review, the left monitor's hidden workspace -- carried a screen
    /// and a bit to the right by the shipped `spread` -- was on stage on the
    /// right monitor, and `offer_keyboard`, `settle_focus` and an activation
    /// all handed it the keyboard. `nothing_on_stage` has the rule and the
    /// `on_two_monitors_` tests in `keyboard_at_open` have each caller.
    ///
    /// **No screens is not "invisible".** `nothing_on_stage` answers `None`
    /// when there is nothing to measure against, and a compositor with no
    /// output bound yet must not decide that every window is unreachable — the
    /// caller would then refuse to focus anything at all. "Not known to be off
    /// stage" is the honest reading and the safe one.
    pub(super) fn on_stage(
        &self,
        pane: &Pane,
        screens: &[Rectangle<i32, Logical>],
        landed: Duration,
    ) -> bool {
        let slot = self.pane_outer(pane);
        staged(slot, self.drawn_at(pane, slot, landed), screens)
    }

    /// Whether the selections this pane is in are putting its desk away: on a
    /// workspace other than the one its monitor is showing.
    ///
    /// **Asked of the selections directly**, because that is what a hidden
    /// workspace is. `workspaces.lua` parks a desk with `sol.present_group`
    /// and clears the shift of the desk in view, so the compositor -- which
    /// knows no workspaces -- can still tell a window on a desk nobody is
    /// looking at from one on the desk in front of them, wherever either one's
    /// own frame is. A window on the desk in view is carried by nothing, and a
    /// column scrolled off it is still focused and brought back by the strip.
    ///
    /// **Where the selections are headed, and not where they have got to.**
    /// Until #134's fifth review this was the shift at [`Self::settling`]. A
    /// spring never lands exactly on nothing before the travel is retired
    /// (`Groups::bound_for_window` says why), so with
    /// `workspaces.motion.easing = "spring"` every window on the desk being
    /// switched to was refused for the whole slide
    /// (`on_a_spring_a_genuine_activation_mid_slide_on_the_desk_being_switched_to_takes_the_keyboard`).
    ///
    /// **Put away is carried anywhere, or faded to nothing**, and measured
    /// against no screen: [`put_away`] is the rule, and says why. A selection
    /// that dims a desk or turns it leaves it where the user can see it. One
    /// that fades it to nothing is refused here, before it is focused, where
    /// until #134's fifth review it was focused and then handed off to
    /// whatever `settle_focus` picked
    /// (`a_genuine_activation_of_a_window_a_selection_fades_to_nothing_leaves_the_keyboard_exactly_where_it_was`).
    ///
    /// Until #134's fourth review this was inferred from where the frame
    /// lands -- the pane's own frame on stage and its carried one off -- which
    /// missed every window on a hidden desk whose own frame was already off
    /// stage: a column scrolled off it, and a window being closed there.
    /// `a_genuine_activation_of_a_column_scrolled_off_a_hidden_workspace_leaves_the_keyboard_exactly_where_it_was`,
    /// `a_genuine_activation_of_a_window_being_closed_on_a_hidden_workspace_leaves_the_keyboard_exactly_where_it_was`,
    /// and on the desk in view
    /// `a_genuine_activation_of_a_column_scrolled_off_screen_brings_it_back_with_the_keyboard`.
    pub(super) fn carried_by_a_selection(&self, pane: crate::pane::PaneId) -> bool {
        if self.groups.is_empty() {
            return false;
        }
        let Some(pane) = self.panes.get(pane) else {
            return false;
        };
        let Some(output) = self.output_of(self.pane_outer(pane)) else {
            return false;
        };
        // The monitor `carried_at` would name, so a selection of a monitor
        // holds the pane here exactly when it carries it there.
        let monitor = self.groups.names_monitors().then(|| output.name());
        let bound = self
            .groups
            .bound_for_window(pane.id().get(), monitor.as_deref());
        put_away(bound)
    }

    /// The pane lifted over the bars on the monitor at `screen`, if any:
    /// [`crate::stack::lifted`], asked of the panes front first.
    ///
    /// **On the monitor's shown workspace** is drawn on that monitor, by its
    /// slot ([`crate::render::drawn_on`]), and not on a desk a selection is
    /// putting away ([`Self::carried_by_a_selection`]). So a fullscreen window
    /// on a workspace that is not shown changes nothing, and neither does one
    /// behind another window on its own.
    /// `a_fullscreen_window_on_a_workspace_not_shown_leaves_the_bar_on_top`,
    /// and `crate::stack`'s `only_the_front_pane_of_the_shown_workspace_is_lifted`.
    ///
    /// **What is left of a window whose client has gone counts**, for as
    /// long as it is drawn. It has no client to be fullscreen, so a
    /// fullscreen window behind it is lifted once it is gone rather than the
    /// moment its client leaves, and it fades out over the fullscreen window,
    /// as it stood.
    /// `a_window_closing_in_front_of_a_fullscreen_one_fades_out_over_it`.
    ///
    /// Read by the renderer (`render::stacked`) and by the hit tests
    /// ([`Self::topmost_above`], `panes_front_first`), which is what makes the
    /// window drawn over the bars the one that is clicked there.
    pub(crate) fn lifted_on(&self, screen: Rectangle<i32, Logical>) -> Option<crate::pane::PaneId> {
        crate::stack::lifted(
            self.panes.iter().rev().map(|pane| {
                let shown = crate::render::drawn_on(self.pane_outer(pane), screen)
                    && !self.carried_by_a_selection(pane.id());
                let candidate = crate::stack::Candidate {
                    shown,
                    fullscreen: pane.client().is_some_and(fullscreen),
                };
                (pane.id(), candidate)
            }),
            self.fullscreen_covers,
        )
    }

    /// [`Self::on_stage`] for one pane, asked by id at [`Self::settling`]: for
    /// a caller with a single question rather than a walk to hoist the
    /// screens out of. A pane that is not there is not on stage.
    pub(super) fn pane_on_stage(&self, pane: crate::pane::PaneId) -> bool {
        let landed = self.settling();
        let screens = self.screens();
        self.panes
            .get(pane)
            .is_some_and(|held| self.on_stage(held, &screens, landed))
    }

    /// Whether a selection is carrying every window off every screen.
    ///
    /// **The recovery path is part of issue #116.** The session that found it
    /// had every window drawn two screen-widths to the left, nothing on any
    /// monitor but a wallpaper, and no key that brought it back — `super+1`
    /// early-returned because the scripts believed workspace 1 was already in
    /// view. What would have saved it was one line saying where everything had
    /// gone. The compositor knew; nothing asked it.
    ///
    /// **The compositor cannot tell a lost desktop from an empty workspace,
    /// and does not pretend to.** They are the same picture: in both, every
    /// window is in a selection carried off screen and the desk in view has
    /// none. The difference is intent, and intent lives in the scripts. So
    /// this is asked at the one moment where the answer is worth having
    /// either way — the end of [`Self::reload`], which is both the keypress
    /// that lost the desktop and the keypress anyone reaches for when
    /// something on screen has gone wrong. Asking it on every script event
    /// instead would warn about every empty workspace anybody switched to,
    /// and a warning that is usually wrong is one nobody reads.
    ///
    /// `None` when nothing is grouped, when there are no screens, or when
    /// there are no windows — none of which is a question with an answer.
    ///
    /// Measured through [`Self::drawn_at`], which is the same function the
    /// renderer uses to place a pane: asking `Groups` for the offset
    /// separately would be a second answer to "where is this window", and two
    /// answers drift.
    ///
    /// **Sampled [`SETTLED`] ahead, and the first version was not.** It asked
    /// `self.clock.now()` the instant after the three dispatches, which is the
    /// instant every group transform they started is at *progress zero* — so it
    /// measured where the previous session had left the desks rather than what
    /// this reload was doing with them. Wrong both ways round: a reload that
    /// rescued an off-stage desktop printed the warning anyway, and one that
    /// carried the desktop off went quiet. Only a displacement from a
    /// membership change, which is instantaneous, came out right, and that is
    /// the case the fault hid behind (#116 review).
    pub(super) fn everything_is_off_stage(&self) -> Option<bool> {
        if self.groups.is_empty() {
            return None;
        }
        let screens: Vec<Rectangle<i32, Logical>> = self
            .space
            .outputs()
            .filter_map(|output| self.space.output_geometry(output))
            .collect();
        // Having no screens is `nothing_on_stage`'s answer to give, and it does
        // -- a second check here would be a second place that decides what an
        // unanswerable question comes back as.
        let landed = self.settling();
        nothing_on_stage(
            self.panes.iter().filter(|pane| pane.managed()).map(|pane| {
                let slot = self.pane_outer(pane);
                (slot, self.drawn_at(pane, slot, landed).rect)
            }),
            &screens,
        )
    }
}
