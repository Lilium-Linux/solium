//! Who has the keyboard, on this side of the gate in `crate::focus`: the focused window and
//! whether a pane is drawn focused, focusing a window, moving the keyboard on when a window goes
//! or leaves the screen, and the scripts' `focus` event.

use super::*;

impl Solium {
    /// Whether a pane's frame is drawn focused.
    ///
    /// The keyboard's answer for a window, and the answer a window had when
    /// its client went for one fading out: the keyboard has moved on by then,
    /// and a titlebar that greyed out while it faded would be drawing the
    /// focus change on a window nobody can focus.
    pub(crate) fn looks_focused(&self, id: crate::pane::PaneId) -> bool {
        if let Some(left) = self.panes.get(id).and_then(Pane::left) {
            return left.focused;
        }
        self.focused_window()
            .is_some_and(|window| self.panes.id_of(&window) == Some(id))
    }

    /// The window with keyboard focus, if any.
    ///
    /// A focused *popup* answers with the window it belongs to, because that is
    /// what everything asking this question means. A popup grab moves keyboard
    /// focus onto the menu's own surface -- that is how a menu receives Escape
    /// and arrow keys -- but `window_for` matches toplevels only, so without
    /// this the answer would be `None` for as long as a menu is open.
    ///
    /// Three things read it and all three were wrong that way: a titlebar drew
    /// unfocused on every right-click, `snapshot`'s `focused` went null so a
    /// Lua layout saw no focused window, and `settle_focus` treated the gap as
    /// "nothing is focused" and moved the selection. The menu is a part of its
    /// window, not a rival to it.
    pub(crate) fn focused_window(&self) -> Option<Window> {
        let surface = self.seat.get_keyboard()?.current_focus()?;
        if let Some(window) = self.window_for(&surface) {
            return Some(window);
        }
        // Only a popup has a root to find; for anything else this is the same
        // `None` the line above already produced.
        let popup = self.popups.find_popup(&surface)?;
        let root = find_popup_root_surface(&popup).ok()?;
        self.window_for(&root)
    }

    pub(crate) fn is_focused(&self, window: &Window) -> bool {
        self.focused_window().as_ref() == Some(window)
    }

    /// Give the keyboard to something, if a window went and left it nowhere.
    ///
    /// Focus is a Wayland concept and belongs to a surface, so when the focused
    /// window's surface dies the seat is simply left holding nothing. Nothing
    /// takes it back: focus was only ever set when a window opened or the
    /// pointer moved. So closing a window meant the keyboard went dead, every
    /// binding that acts on "the focused window" stopped working, and the only
    /// way out was to move the mouse over something. From the other side of the
    /// screen that is a session that broke when you closed a window.
    ///
    /// The window under the pointer first, because with focus-follows-mouse
    /// that is where focus would land the moment you moved; the topmost
    /// otherwise. Called where a window went, and only when nothing has focus,
    /// so it cannot argue with a script that has just chosen one.
    ///
    /// **While locked, the only place the keyboard can go is a lock surface**,
    /// so that is where this sends it: to the lock screen on the monitor the
    /// pointer is on, or any surviving one, unless it is already on one. A
    /// window is never considered, and the gate in `focus.rs` would refuse one
    /// anyway. What this adds over the gate is the other half: when the lock
    /// surface the keyboard was on dies -- its monitor unplugged while locked --
    /// the keyboard is handed to one that is still there, rather than left on
    /// nothing until another maps (see `Solium::lock_surface_destroyed`).
    ///
    /// **Nor, just after an unlock, while the key that unlocked is still
    /// down.** `unlock` leaves the keyboard on nothing until every key is up,
    /// so that no window is told in `wl_keyboard.enter` that Enter is held, and
    /// `input::key` calls this once they are (`refocus_on_release`). An empty
    /// seat is exactly what the rest of this function hands a window, so
    /// without the early return any other caller in that tenth of a second
    /// hands the held key over after all: `sync_panes`, and [`Self::give_back`]
    /// wherever a close in flight is given back -- a refusal's deadline, a
    /// dialog answering it, or `settle_closing`'s retry.
    /// `a_close_in_flight_across_a_lock_never_takes_the_keyboard` drives the
    /// first two.
    ///
    /// **Both arms judge where windows are settling, not where they are
    /// drawn.** That is [`super::workspaces::SETTLED`]'s rule, and the tests for each case are
    /// named there. Asking at the present was #127's fourth review, twice
    /// over: `give_back`'s call here found the window it was giving back still
    /// at its restore's progress zero — `present::close`'s opacity zero — and
    /// declined it, and a workspace switch's first frame had the desk being
    /// left on stage and the desk arriving off it.
    ///
    /// **Neither candidate may be a window on its way out.** The pointer arm
    /// gets that from hit-testing the destination, where a closing pane is at
    /// opacity zero from the press onwards; hit-testing the present, as it did
    /// before, only got it once the fade had landed, and a pane still fading
    /// under the pointer was handed the keyboard. The topmost arm had to be
    /// told as well: it reads the pane list directly, and the pane a close is
    /// playing on is usually the topmost one there is, so without the filter
    /// [`Self::hand_off_keyboard`] would take the keyboard off a closing window
    /// and give it straight back.
    ///
    /// **And neither may be a window that is not on screen**, which is the
    /// other half of the same sentence and was missing from the topmost arm
    /// for exactly as long. `window_under` gates on `Frame::covers` and so
    /// asks about pixels twice over — opacity and the rectangle; the topmost
    /// arm asked about neither. A hidden workspace is parked a screen away
    /// rather than unmapped, so closing the only window on the workspace in
    /// view handed the keyboard to a window on a desk the user cannot see, and
    /// with it [`Self::focus_window`]'s `trigger_focus` — which is what a
    /// workspace script acts on. `hand_off_keyboard`'s "nothing is focused when
    /// there is nothing else open" was false in that case, and on a refusal
    /// `give_back`'s call here then *declined*, because something was focused —
    /// so it was permanent. That is #127's own symptom reached by a second
    /// route, and it is [`Self::on_stage`] that closes it: the same visibility
    /// question the pointer path already asks, put to the screens rather than
    /// to a point.
    pub(crate) fn settle_focus(&mut self) {
        if self.lock.is_some() {
            self.settle_lock_focus();
            return;
        }
        if self.refocus_on_release {
            return;
        }
        if self.focused_window().is_some() {
            return;
        }
        let at = self
            .seat
            .get_pointer()
            .map(|pointer| pointer.current_location());
        // Once for the whole walk, and at the destination rather than the
        // frame being drawn: focus is about what the user is going to work
        // with. See `SETTLED`.
        let landed = self.settling();
        let screens = self.screens();
        let next = at
            .and_then(|at| self.window_under_at(at, landed))
            .map(|(window, _)| window)
            .or_else(|| {
                self.panes
                    .iter()
                    .rev()
                    .filter(|pane| !pane.leaving())
                    .filter(|pane| self.on_stage(pane, &screens, landed))
                    .find_map(|pane| pane.client().cloned())
            });
        if let Some(window) = next {
            tracing::debug!("a window went and the keyboard had nowhere to be");
            self.focus_window(&window, SERIAL_COUNTER.next_serial());
        }
    }

    /// Take the keyboard off a window that is no longer on screen, and give it
    /// to whatever is.
    ///
    /// **The half of issue #127's hit-test fix that a mouse cannot reach.** The
    /// review before this one stopped an invisible closing pane from winning
    /// `window_under`, which fixed the keystrokes that follow a *click*: the
    /// press lands in the window that is drawn and focus goes there with it.
    /// It fixed nothing for the user who never touches the mouse. `close_pane`
    /// does not move focus, and `settle_focus` runs only from `sync_panes`,
    /// which calls it when the *pane set* changes -- and a close that has been
    /// asked and not yet answered changes nothing. So `super+q` followed by
    /// carrying on typing put every keystroke into an invisible window for the
    /// whole 190 ms animation plus the 1000 ms grace period, with the window
    /// that reflowed into its place on screen taking the blame. Close, keep
    /// typing is the ordinary way to meet this; clicking first is the rare one.
    ///
    /// **Here, at the request, and not at the press.** The boundary is the same
    /// one `Frame::covers` draws for the pointer, deliberately: while the
    /// leaving animation is playing the window is still *there* -- shrinking
    /// and fading, but drawn, and still alive -- and a half-faded window that
    /// keeps its clicks must keep its keystrokes too, or the two halves of a
    /// press disagree about which window the user is looking at. The animation
    /// landing is the instant the window stops covering anything, and it is the
    /// same instant `settle_closing` sends the request. So the exposure is the
    /// 190 ms in which the window is visible, rather than the 1190 ms in which
    /// it is not.
    ///
    /// **What it is not: a decision about where focus should end up.** That is
    /// [`Self::settle_focus`]'s, unchanged and already the answer everywhere
    /// else a window goes -- the window under the pointer, else the topmost.
    /// This only clears the seat first, because `settle_focus` declines to
    /// argue with a focus that is already set, and the focus that is set is the
    /// one being taken away.
    ///
    /// Nothing is focused when there is nothing else open **on screen** — a
    /// window parked on a hidden workspace is not a candidate, which is
    /// [`Self::settle_focus`]'s own note and was not true until this round.
    /// That is the right answer rather than a gap: typing into a window that is
    /// not on screen is the fault, and typing into nothing at least loses no
    /// keystrokes to the wrong application. [`Self::give_back`] calls
    /// `settle_focus` again, and the window that comes back takes the keyboard
    /// on the frame its restore starts —
    /// `a_refused_close_gives_the_keyboard_back_to_the_window_it_brings_back`
    /// asserts both the empty seat and the return. The sentence that stood here
    /// before it claimed the second half and it did not hold: focus was judged
    /// at the restore's progress zero, where the window is still invisible.
    pub(super) fn hand_off_keyboard(&mut self, leaving: &Window) {
        if !self.is_focused(leaving) {
            return;
        }
        // The window's own menu first: its grab would refuse the line below.
        // See `release_grabs_of`.
        self.release_grabs_of(leaving);
        self.give_keyboard(None, SERIAL_COUNTER.next_serial());
        crate::xwayland::activate(self, None);
        self.settle_focus();
    }

    pub(crate) fn focus_window(&mut self, window: &Window, serial: Serial) {
        let Some(location) = self.space.element_location(window) else {
            return;
        };
        // The rule in `focus.rs`, asked before anything happens rather than
        // left to `give_keyboard` to refuse halfway through. A focus that is
        // refused must not restack a window behind the lock, activate an X11
        // window, or tell a layout that something is focused when nothing is.
        // Scripts (`sol.focus`), layouts' event handlers, xdg-activation and
        // `settle_focus` all arrive here, and all of them can while locked.
        if !self.may_focus(window) {
            tracing::debug!("refused to focus a window: the session is locked");
            return;
        }
        // Frames are drawn differently focused and unfocused, and restacking
        // changes what covers what. Both are the screen changing.
        self.redraw = true;
        // `true` restacks: a clicked window comes to the front.
        self.map_stacked(window.clone(), location, true);

        if self.seat.get_keyboard().is_some() {
            // The window's own surface, so this works for an X11 window as
            // well as an xdg one.
            let surface = window.wl_surface().map(|surface| surface.into_owned());
            self.give_keyboard(surface.clone(), serial);
            // X11 wants telling separately, in its own terms: a window that
            // has keyboard focus but was never activated draws itself
            // unfocused however much typing goes into it.
            crate::xwayland::activate(self, surface.as_ref());
        }

        // A layout may want to follow: a scroller brings the focused column
        // fully into view, which is the difference between clicking a window
        // half off the edge and being able to use it.
        if !self.focusing {
            self.focusing = true;
            self.trigger_focus(window);
            self.focusing = false;
        }
    }

    /// Tell scripts focus moved.
    fn trigger_focus(&mut self, window: &Window) {
        let id = self.window_id(window);
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return;
        };
        let outcome = scripts.focused(id, snapshot);
        self.scripts = Some(scripts);
        self.apply(outcome);
    }
}
