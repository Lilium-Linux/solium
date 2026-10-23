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
    /// Nothing for a window already minimized -- nothing is told twice --
    /// nor for one being closed, one whose application has not arrived, or a
    /// menu or tooltip that places itself.
    ///
    /// In this order, each for a reason. The client is told first, so the
    /// configure that says `suspended` goes out with the frame that starts
    /// the animation. Then the scripts hear `minimize`, before anything else
    /// reacts, with the window already `minimized` in their snapshot: a
    /// configuration starts its present in that same dispatch, so the window
    /// is never drawn put away without one, and a layout takes it out. And
    /// last, if it had the keyboard, the keyboard leaves it as it leaves a
    /// window whose close has landed (`config.focus.after_close`), for the
    /// window the layout has just grown into its space.
    /// `tests::real_client::reflow_on_close::minimize::sol_minimize_marks_the_row_fires_once_and_leaves_the_client_mapped`,
    /// `tests::real_client::reflow_on_close::minimize::the_keyboard_leaves_a_minimized_window_for_the_next_and_never_comes_back_to_it`.
    pub(crate) fn minimize(&mut self, pane: crate::pane::PaneId) {
        let Some(held) = self.panes.get_mut(pane) else {
            return;
        };
        if held.minimized() || held.leaving() || !held.managed() {
            return;
        }
        let Some(window) = held.client().cloned() else {
            return;
        };
        held.set_minimized(true);
        tell_minimized(&window, true);
        self.redraw = true;
        self.trigger_minimized(pane);
        if self.is_focused(&window) {
            self.note_keyboard_left(pane);
            self.hand_off_keyboard(&window);
        }
    }

    /// Show a minimized window again: `sol.unminimize`, its own client, or
    /// [`Self::focus_window`], which shows a window before it focuses it.
    /// Nothing for a window that is not minimized.
    ///
    /// The scripts hear `unminimize` with the window no longer `minimized`:
    /// a layout takes it back where it was, and a configuration presents it
    /// back from wherever it put it. The keyboard is not moved here; a focus
    /// that asked for this moves it next.
    /// `tests::real_client::reflow_on_close::minimize::sol_unminimize_shows_the_window_again`,
    /// `tests::real_client::reflow_on_close::minimize::focusing_a_minimized_window_shows_it_first`.
    pub(crate) fn unminimize(&mut self, pane: crate::pane::PaneId) {
        let Some(held) = self.panes.get_mut(pane) else {
            return;
        };
        if !held.minimized() {
            return;
        }
        held.set_minimized(false);
        if let Some(window) = held.client().cloned() {
            tell_minimized(&window, false);
        }
        self.redraw = true;
        self.trigger_unminimized(pane);
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
