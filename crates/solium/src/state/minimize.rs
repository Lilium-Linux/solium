//! A window minimized (#30) and shown again: the state, what the client is
//! told, the keyboard leaving it, and the scripts' events.
//!
//! The engine keeps the state and nothing else. Where a minimized window goes
//! on screen -- a genie into a dock, a flight home on a phone -- is the
//! configuration's present, started from the `minimize` event; the window is
//! drawn while that present plays out, and not after. Layouts leave a
//! minimized window out of their arrangement; the compositor only refuses to
//! move it (`move_pane`).

use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;

use super::*;

impl Solium {
    /// Whether `pane` is minimized and put away at `now`: drawn nowhere, on no
    /// stage, under no point. A minimized window is still drawn while a
    /// present on it plays out (`present::playing`), so a configuration's
    /// genie finishes before the window goes, and while a configuration shows
    /// it anyway (`Pane::shown_minimized`) -- but never while it is being
    /// closed, where the close's own fade would draw it back.
    /// `tests::real_client::reflow_on_close::minimize::a_minimized_window_is_drawn_until_its_present_lands`.
    pub(crate) fn minimized_away(&self, pane: &Pane, now: Duration) -> bool {
        pane.minimized()
            && (pane.leaving() || !(present::playing(pane, now) || pane.shown_minimized()))
    }

    /// Minimize `pane`'s window: `sol.minimize`, `super+m` through the
    /// shipped configuration, or its own client asking.
    ///
    /// **A modal dialog goes with the window it belongs to**: minimizing a
    /// window minimizes its modal dialogs, and minimizing a dialog minimizes
    /// the window it is for, and so the whole family ([`Self::family_of`]).
    /// Within it, nothing is done to a window already minimized -- nothing
    /// is told twice -- nor to one being closed, one whose application has
    /// not arrived, or a menu or tooltip that places itself.
    ///
    /// In this order, each for a reason. The client is told first, so the
    /// configure that says `suspended` goes out with the frame that starts
    /// the animation. Then the scripts hear `minimize`, before anything else
    /// reacts, with the window already `minimized` in their snapshot: a
    /// configuration starts its present in that same dispatch, so the window
    /// is never drawn put away without one, and a layout takes it out. Then
    /// the pointer: a lock or a confinement the window held is let go of, and
    /// the window under the pointer has it, so a press with no motion before
    /// it reaches what is drawn there. And last, if one of them had the
    /// keyboard, the keyboard leaves it as it leaves a window whose close has
    /// landed (`config.focus.after_close`), for the window the layout has
    /// just grown into its space -- once, with the whole family put away.
    /// `tests::real_client::reflow_on_close::minimize::sol_minimize_marks_the_row_fires_once_and_leaves_the_client_mapped`,
    /// `tests::real_client::reflow_on_close::minimize::the_keyboard_leaves_a_minimized_window_for_the_next_and_never_comes_back_to_it`,
    /// `tests::real_client::reflow_on_close::minimize::a_press_with_no_motion_after_a_minimize_reaches_the_window_under_it`,
    /// `tests::real_client::reflow_on_close::minimize::a_minimized_window_lets_go_of_the_pointer_it_locked`,
    /// `tests::real_client::reflow_on_close::minimize::a_modal_dialog_is_minimized_and_shown_with_its_window`.
    pub(crate) fn minimize(&mut self, pane: crate::pane::PaneId) {
        let members: Vec<(crate::pane::PaneId, Window)> = self
            .family_of(pane)
            .into_iter()
            .filter_map(|member| {
                let held = self.panes.get(member)?;
                if held.minimized() || held.leaving() || !held.managed() {
                    return None;
                }
                Some((member, held.client()?.clone()))
            })
            .collect();
        if members.is_empty() {
            return;
        }
        for (member, window) in &members {
            if let Some(held) = self.panes.get_mut(*member) {
                held.set_minimized(true);
            }
            tell_minimized(window, true);
        }
        self.redraw = true;
        for (member, _) in &members {
            self.trigger_minimized(*member);
        }
        for (_, window) in &members {
            self.let_go_of_the_pointer(window);
        }
        self.repoint_clients();
        if let Some((member, window)) = members
            .iter()
            .find(|(_, window)| self.is_focused(window))
            .cloned()
        {
            self.note_keyboard_left(member);
            self.hand_off_keyboard(&window);
        }
    }

    /// Show a minimized window again: its own client, or
    /// [`Self::focus_window`], which shows a window before it focuses it.
    /// With its family, as [`Self::minimize`] puts it away; nothing for a
    /// window that is not minimized.
    ///
    /// A window that is on no monitor -- one whose monitor went while it was
    /// put away, and that nothing has rescued -- is first brought onto one,
    /// as `rescue_offscreen` brings back a window that is shown.
    ///
    /// The scripts hear `unminimize` with the window no longer `minimized`:
    /// a layout takes it back where it was, and a configuration presents it
    /// back from wherever it put it. Then the pointer is given to whatever is
    /// under it now. The keyboard is not moved here; a focus that asked for
    /// this moves it next, and [`Self::show_again`] settles it otherwise.
    /// `tests::real_client::reflow_on_close::minimize::sol_unminimize_shows_the_window_again`,
    /// `tests::real_client::reflow_on_close::minimize::focusing_a_minimized_window_shows_it_first`,
    /// `tests::real_client::reflow_on_close::minimize::unminimizing_a_window_on_no_monitor_brings_it_onto_one`.
    pub(crate) fn unminimize(&mut self, pane: crate::pane::PaneId) {
        let members: Vec<crate::pane::PaneId> = self
            .family_of(pane)
            .into_iter()
            .filter(|member| self.panes.get(*member).is_some_and(Pane::minimized))
            .collect();
        if members.is_empty() {
            return;
        }
        for member in &members {
            let Some(held) = self.panes.get(*member) else {
                continue;
            };
            let outer = self.pane_outer(held);
            if let Some(moved) = self.rescued(outer) {
                let now = self.clock.now();
                self.move_pane(
                    *member,
                    moved,
                    outer,
                    AnimationSpec::default(),
                    now,
                    Standing::Kept,
                );
            }
            if let Some(held) = self.panes.get_mut(*member) {
                held.set_minimized(false);
                if let Some(window) = held.client().cloned() {
                    tell_minimized(&window, false);
                }
            }
        }
        self.redraw = true;
        for member in &members {
            self.trigger_unminimized(*member);
        }
        self.repoint_clients();
    }

    /// [`Self::unminimize`], asked for by a script -- `sol.unminimize`,
    /// `sol.act("windows.unminimize")` -- or by an X11 client, and not by a
    /// focus: with nothing holding the keyboard afterwards, it settles where
    /// it settles after any window goes or comes, so it is not left on
    /// nothing.
    /// `tests::real_client::reflow_on_close::minimize::a_window_shown_again_with_nothing_focused_takes_the_keyboard`.
    pub(crate) fn show_again(&mut self, pane: crate::pane::PaneId) {
        self.unminimize(pane);
        if self.focused_window().is_none() {
            self.settle_focus();
        }
    }

    /// The window a modal dialog belongs to, followed up through every modal
    /// dialog to the first window that is not one, and every modal dialog
    /// that belongs to that window at any depth: the family a minimize puts
    /// away together. The window itself first. A window that is neither a
    /// dialog nor has one is a family of one.
    fn family_of(&self, pane: crate::pane::PaneId) -> Vec<crate::pane::PaneId> {
        let dialog_of = |member: &Pane| {
            let window = member.client()?;
            if !self.is_modal(window) {
                return None;
            }
            match self.parent_of(window) {
                crate::script::Parentage::Window(parent) => Some(parent),
                _ => None,
            }
        };
        let mut head = pane;
        let mut seen = vec![pane];
        while let Some(parent) = self
            .panes
            .get(head)
            .and_then(dialog_of)
            .and_then(|parent| self.panes.by_script_id(parent))
            .map(Pane::id)
        {
            // A client may name a cycle of parents, which neither protocol
            // forbids: each is visited once.
            if seen.contains(&parent) {
                break;
            }
            seen.push(parent);
            head = parent;
        }
        let mut family = vec![head];
        let mut at = 0;
        while let Some(&parent) = family.get(at) {
            let children: Vec<crate::pane::PaneId> = self
                .panes
                .iter()
                .filter(|member| dialog_of(member) == Some(parent.get()))
                .map(Pane::id)
                .filter(|child| !family.contains(child))
                .collect();
            family.extend(children);
            at += 1;
        }
        family
    }

    /// Let go of a pointer lock or confinement `window` holds: a minimized
    /// window is not the one being used, and a game put away must not keep
    /// the cursor. Granted again by the first motion over it once it is
    /// shown, as every constraint is.
    fn let_go_of_the_pointer(&self, window: &Window) {
        let (Some(pointer), Some(surface)) = (self.seat.get_pointer(), window.wl_surface()) else {
            return;
        };
        smithay::wayland::pointer_constraints::with_pointer_constraint(
            &surface,
            &pointer,
            |constraint| {
                if let Some(constraint) = constraint
                    && constraint.is_active()
                {
                    constraint.deactivate();
                }
            },
        );
    }

    /// Show a minimized window anyway, or stop: `sol.show_minimized`. See
    /// `Pane::shown_minimized`.
    pub(crate) fn show_minimized(&mut self, pane: crate::pane::PaneId, shown: bool) {
        if let Some(held) = self.panes.get_mut(pane)
            && held.minimized()
            && held.shown_minimized() != shown
        {
            held.set_shown_minimized(shown);
            self.redraw = true;
        }
    }

    /// Tell scripts a window was minimized. See [`Scripts::minimized`].
    fn trigger_minimized(&mut self, pane: crate::pane::PaneId) {
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return;
        };
        let outcome = scripts.minimized(pane.get(), snapshot);
        self.scripts = Some(scripts);
        self.apply(outcome);
    }

    /// Tell scripts a minimized window is shown again. See
    /// [`Scripts::unminimized`].
    fn trigger_unminimized(&mut self, pane: crate::pane::PaneId) {
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return;
        };
        let outcome = scripts.unminimized(pane.get(), snapshot);
        self.scripts = Some(scripts);
        self.apply(outcome);
    }
}

/// Tell a window's client it is minimized, or no longer, where its protocol
/// can say so: an xdg toplevel's `suspended` state (smithay leaves it out for
/// a client older than version 6, which has no word for it), and an X11
/// window's `_NET_WM_STATE_HIDDEN`. Never an unmap: the client stays mapped
/// and alive, and an X11 window unmapped by its window manager is one its
/// client believes was withdrawn.
///
/// The xdg half is `tests::real_client::reflow_on_close::minimize::sol_minimize_marks_the_row_fires_once_and_leaves_the_client_mapped`.
/// The X11 half is checked by reading it: no test can make an X11 window,
/// since an `X11Surface` needs a live Xwayland (see `state/hosted.rs`'s
/// `act`).
fn tell_minimized(window: &Window, minimized: bool) {
    if let Some(toplevel) = window.toplevel() {
        toplevel.with_pending_state(|state| {
            if minimized {
                state.states.set(xdg_toplevel::State::Suspended);
            } else {
                state.states.unset(xdg_toplevel::State::Suspended);
            }
        });
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    } else if let Some(x11) = window.x11_surface()
        && let Err(err) = x11.set_suspended(minimized)
    {
        tracing::warn!(?err, "could not tell an X11 window it is minimized");
    }
}
