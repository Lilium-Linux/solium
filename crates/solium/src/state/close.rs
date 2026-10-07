//! A window closing: asking it to close and sending the close once its leaving animation lands,
//! bringing it back if it refuses or answers with a dialog, a window going on its own (`depart`,
//! which turns it into `Content::Leaving` when it left something to fade), ending those fades,
//! and the scripts' events for all of it.

use super::*;

impl Solium {
    /// The panes stacked under this one this instant, bottom to top: what a
    /// pane whose client is going stays over while it fades. See
    /// `crate::pane::Left::over`.
    ///
    /// From the space for a window in it, which is the authority on how
    /// clients are stacked, and from the panes for one that is not -- a window
    /// whose application never arrived, which the space has never held.
    fn stacked_under(&self, pane: &Pane) -> Vec<crate::pane::PaneId> {
        let in_space = pane.client().and_then(|window| {
            self.space
                .elements()
                .position(|each| each == window)
                .map(|at| {
                    self.space
                        .elements()
                        .take(at)
                        .filter_map(|each| self.panes.id_of(each))
                        .collect()
                })
        });
        in_space.unwrap_or_else(|| {
            self.panes
                .iter()
                .take_while(|each| each.id() != pane.id())
                .map(Pane::id)
                .collect()
        })
    }

    /// Where a layout has a pane: the tile it asked for, or where the pane is
    /// when it is in none. What `Self::depart` compares across a `close` to
    /// find the windows the layout grew into the space of one that went.
    fn laid_out_at(&self, pane: &Pane) -> Rectangle<i32, Logical> {
        pane.placed().unwrap_or_else(|| self.pane_outer(pane))
    }

    /// Ask a window to close, once it has finished leaving.
    ///
    /// A close is a request the client may refuse, so the compositor cannot
    /// simply animate the window away and drop it. What it can do is animate
    /// first and ask afterwards: the window shrinks and fades while it is
    /// still alive, and the request goes out when that lands. A client that
    /// refuses is left with a window that is drawn away -- so the transform is
    /// cleared in that case too, and the window comes back.
    ///
    /// This covers closes the compositor asks for -- a frame button, a
    /// binding, a script. A client that exits on its own plays the same fade,
    /// from the textures the renderer had already imported for its surfaces,
    /// taken at the moment it goes rather than held for every window on the
    /// chance: see [`Self::depart`] and `crate::remains` (#126).
    ///
    /// **Asked at most once per window.** See [`Pane::leaving`] for the four
    /// states that answer it, and why the narrower question this used to ask
    /// was wrong for most of the time a window spends leaving.
    pub(crate) fn close_pane(&mut self, id: crate::pane::PaneId) {
        // The pane is looked up before the "already leaving" guard rather than
        // after it, which the `closing` map could not do. Same answer either
        // way: an id with no pane returned early on the second check before
        // and returns early on the first one now, and a pane already on its
        // way out must not have its animation restarted.
        let Some(pane) = self.panes.get(id) else {
            return;
        };
        // **The guard was widened rather than the write order changed**, and
        // the two are not alternatives to each other. `settle_closing` clears
        // `closing_at` before it stamps `asked_at`, so swapping those two lines
        // closes a gap of a few statements that nothing can press a key inside
        // -- while leaving the whole of the grace period after them, which is
        // *hundreds of milliseconds of an invisible window*, answering "not
        // closing". That is the window a second `super+q` actually lands in,
        // and only the predicate closes it. Ordering is now irrelevant here
        // either way, which is worth more than picking one: `leaving` is true
        // across the whole transition however those two writes are arranged.
        //
        // **What this leaves the user with for a wedged client, written down
        // because it was a gap and not a decision -- and is now only half a
        // gap.** There is still no force-kill for a Wayland client: no
        // `xkill`, no "application is not responding", no binding that
        // destroys one rather than asking it, because `xdg_toplevel` carries
        // no pid a close could act on without racing the client's own
        // connection for it. Against a hung Wayland client, `super+q` still
        // does one thing per cycle: animate out, ask, wait `GRACE`, come
        // back, roughly 1.34s from press to the window standing there again,
        // for ever.
        //
        // **An X11 client is not for ever any more (#221).** XWayland already
        // hands over a pid the moment a window maps
        // (`crate::xwayland::client_pid`), for free, which is the one thing
        // that was missing here -- so `settle_closing` kills one instead of
        // asking again, the second time a close of it reaches that function
        // with nothing to show for the first `GRACE` (`Pane::x11_refused_once`,
        // set by `settle_refused`'s own silent route and nowhere a dialog
        // answered). The first `super+q` is still only ever a request.
        if pane.leaving() {
            return;
        }
        // No `let else`. `pane_outer` answers for every pane there is -- see
        // its own documentation -- and the bail that used to be here was dead
        // code that read as a real case: a window the compositor cannot
        // *locate* and therefore declines to close.
        let outer = self.pane_outer(pane);
        let now = self.clock.now();
        present::close(pane, outer, now);
        if let Some(pane) = self.panes.get_mut(id) {
            pane.begin_closing(now + present::CLOSING);
        }
        // **In front for its fade** (#128's review, findings 2 and 6). The
        // layout is about to move a neighbour into this window's space, and the
        // window closed is not always the one on top: a strip's sweeps stack
        // its columns left to right, whichever has the keyboard. Raised here,
        // before `closing`, so the sweep below meets a leaving window above
        // whatever it moves -- and `map_laid_out` keeps it there. Its modals
        // go above it as on any raise. See
        // `a_closing_window_fades_in_front_of_the_neighbour_moving_into_its_space`.
        if let Some(window) = self.panes.get(id).and_then(Pane::client).cloned() {
            self.space.raise_element(&window, false);
            self.lift_modals_over(&window);
        }
        self.redraw = true;
        // **And the layout is told now, not when the client has gone** (#128).
        // After `begin_closing`, so the snapshot already lists the window as
        // `leaving`. The animation above is already pinned to the rectangle the
        // window is closing at -- `move_pane` declines to move a leaving pane's
        // transform -- so a layout that reflows here grows the neighbours into
        // the space while the window fades where it stood. See
        // `a_closing_window_hands_its_space_over_before_its_client_is_gone`.
        //
        // Every route here arrives with the scripts back in their slot, which
        // is what `trigger_closing` needs to deliver this at all: the frame
        // button is `frame_action` off a pointer press, and `sol.close` --
        // `super+q` included, which is a binding in `init.lua` -- is a
        // `Command::Close`, applied by `apply` after the dispatch that asked for
        // it has returned the scripts. See
        // `every_close_route_tells_the_layout_the_close_has_begun`.
        self.trigger_closing(id);
    }

    /// Send the close to every window whose leaving animation has landed.
    ///
    /// Returns whether any window is still on its way out, so the backend
    /// keeps drawing until they are gone.
    ///
    /// **Except the ones that have already been answered.** A pane marked
    /// `Pane::answered` is one [`Self::refused_with_a_dialog`] tried and failed
    /// to give back; this is where that retry lives, because this is the only
    /// deadline such a pane is on. See the loop.
    pub(crate) fn settle_closing(&mut self, now: std::time::Duration) -> bool {
        // Over the panes rather than over a map of timers, so a pane that has
        // gone cannot be visited at all. It could be before, between a
        // `Panes::remove` and the `sync_panes` that swept the map after it --
        // and every reader that found such an entry did nothing with it but
        // remove it, so nothing observable turned on that window.
        let due: Vec<crate::pane::PaneId> = self
            .panes
            .iter()
            .filter(|pane| pane.closing_at().is_some_and(|at| now >= at))
            .map(Pane::id)
            .collect();
        for id in due {
            // **A close that has already been answered is retried, not sent.**
            // `Pane::answered` is set by `refused_with_a_dialog` on a pane whose
            // `give_back` was declined -- `present::clear` goes through
            // `with_slot`, which hands back `None` rather than panicking when
            // the transform slot is busy. Nothing else can pick that up: inside
            // `CLOSING` the pane has no `asked_at`, so `settle_refused` is not
            // looking at it, and reaching this line would send the request and
            // close the parent out from under the very dialog that answered it.
            //
            // `continue` rather than `stop_closing`: leaving `closing_at` set is
            // what makes this the retry. The pane stays due, this loop visits it
            // again next frame, and the answer below keeps the backend drawing
            // until the slot frees and `give_back` clears both timers. A busy
            // slot costs a frame, which is what it costs everywhere else.
            if self.panes.get(id).is_some_and(Pane::answered) {
                self.give_back(id, now);
                continue;
            }
            if let Some(pane) = self.panes.get_mut(id) {
                pane.stop_closing();
            }
            let Some(window) = self.panes.get(id).and_then(Pane::client).cloned() else {
                // Nothing to ask. A window whose application never arrived is
                // gone when we say it is, which is the one case where closing
                // is entirely ours to decide. Its fade has just landed, so it
                // is already invisible and there is nothing left to draw.
                //
                // Told first and removed after, as on every route (#126): the
                // pane is still here for `close`'s own snapshot, as a window
                // that is leaving.
                if self.panes.get(id).is_some() {
                    self.trigger_close(id);
                    self.panes.remove(id);
                }
                continue;
            };
            // A request, not a kill: the client decides whether it can close,
            // and the window goes away when it does. Either protocol -- an X11
            // window used to be animated away and then asked *nothing*, so it
            // never closed and never came back, which from the other side of
            // the screen is a window that vanished.
            //
            // **Except the second time, for X11 (#221).** A pane
            // `settle_refused` has already marked `x11_refused_once` asked
            // once, waited `GRACE`, and heard nothing back -- no
            // `WM_DELETE_WINDOW` answer, no dialog (`refused_with_a_dialog`
            // never sets this flag, only `settle_refused`'s silent route
            // does). Asking such a client again is `crate::xwayland::kill_client`
            // rather than `X11Surface::close`: a second polite request learns
            // nothing a second `GRACE` would not already have shown the first
            // one, and it is the one case this compositor can tell apart from
            // an honest, slow close (`settle_refused`'s own note on `GRACE`)
            // without guessing -- the client had its whole `GRACE` and never
            // even started answering.
            if let Some(toplevel) = window.toplevel() {
                toplevel.send_close();
            } else if let Some(x11) = window.x11_surface() {
                let refused_before = self.panes.get(id).is_some_and(Pane::x11_refused_once);
                let pid = refused_before
                    .then(|| crate::xwayland::client_pid(x11))
                    .flatten();
                match pid {
                    Some(pid) => crate::xwayland::kill_client(pid),
                    None => {
                        if refused_before {
                            tracing::warn!(
                                "an unresponsive X11 window has no pid to kill; \
                                 asking it to close again instead"
                            );
                        }
                        if let Err(err) = x11.close() {
                            tracing::warn!(?err, "could not ask an X11 window to close");
                        }
                    }
                }
            }
            // Watched either way. Whether the request went out matters less
            // than whether the window is still here a moment later, and a
            // window we could not even ask is the one most in need of coming
            // back.
            if let Some(pane) = self.panes.get_mut(id) {
                pane.mark_asked(now);
            }
            // **And the keyboard leaves with the pixels.** After `mark_asked`,
            // not before: `stop_closing` above has already cleared the other
            // half of `Pane::leaving`, so until this line the pane would still
            // be a candidate for the keyboard it is about to give up.
            self.hand_off_keyboard(&window);
        }
        // Asked after the loop, not before it: `trigger_close` runs a script,
        // and a script that closes another window during it starts a timer
        // this answer has to count. That was true of `!self.closing.is_empty()`
        // in the same position, and is the reason this is a second pass rather
        // than a flag gathered during the first.
        self.panes.iter().any(|pane| pane.closing_at().is_some())
    }

    /// Drop every pane whose client has gone and whose fade is over, and say
    /// whether any is still fading.
    ///
    /// **On the clock alone** (`pane::LEAVING`), because that is the only
    /// thing about a fade anyone can observe: `present::close` never releases
    /// its transform, so "the animation finished" is not a state. Called from
    /// every frame's `settle`, so a pane is gone within a frame of its
    /// deadline with nothing else having to happen -- no client event, no
    /// `sync_panes`, no layout. `a_window_that_closes_itself_fades_out_and_is_gone_on_time`.
    pub(crate) fn settle_leaving(&mut self, now: std::time::Duration) -> bool {
        let done: Vec<crate::pane::PaneId> = self
            .panes
            .iter()
            .filter(|pane| pane.faded_out(now))
            .map(Pane::id)
            .collect();
        for id in done {
            if self.panes.remove(id) {
                self.redraw = true;
            }
        }
        self.panes.iter().any(Pane::ghost)
    }

    /// Bring back a window that was asked to close and did not.
    ///
    /// The window is animated away before the request goes out, because
    /// waiting for the client would mean nothing happening for as long as the
    /// client took. When the client honours it, that is right. When it does
    /// not -- a terminal asking whether you meant it, an editor with unsaved
    /// work -- the window is left drawn away and invisible, still holding its
    /// place in the layout, until something unrelated happens to move it. From
    /// the other side of the screen that is a window that vanished and a
    /// session that lost it.
    ///
    /// There is no refusal in the protocol, so the only evidence is the window
    /// still being here a moment later. It comes back, fading up out of the
    /// frame it was left at rather than snapping -- `present::clear` starts
    /// from what is on screen, which here is the held, shrunk, transparent end
    /// of the leaving animation, so the recovery is that animation run
    /// backwards.
    ///
    /// Returns whether anything is still being waited on.
    pub(crate) fn settle_refused(&mut self, now: std::time::Duration) -> bool {
        /// How long a client has to act on the request before the compositor
        /// decides it is not going to and gives the window back.
        ///
        /// **A deadline on refusal, and it was set as though it were a deadline
        /// on slowness.** At 400ms this fired on clients that were doing
        /// exactly what they had been asked: Electron runs `before-quit`
        /// handlers on the main JS thread, the JVM runs window listeners behind
        /// class loading, Firefox flushes its session store, and all three
        /// routinely take longer than that between receiving
        /// `xdg_toplevel.close` and destroying the toplevel. What the user saw
        /// on an ordinary, successful close was the window fade away, fade back
        /// in, and then vanish with no animation at all when the client finally
        /// went. That is issue #127's third fault, and the fix is not a bigger
        /// number for its own sake -- it is that the number was measuring the
        /// wrong thing.
        ///
        /// **The two costs are not symmetric.** Too short, and every slow-but-
        /// honest close flickers; that is the common case and the user sees it
        /// daily. Too long, and a window that really did refuse stays invisible
        /// for longer -- but it *does* still come back, so the cost is a wait
        /// rather than a loss. The recoverable failure is the one to take.
        ///
        /// **The upper bound is human, not arithmetic** — and it is measured
        /// from the *press*, which is the only clock the user has. This
        /// constant is not that bound; it is the largest part of it. The whole
        /// span from `super+q` to a refused window standing at full opacity
        /// again is
        ///
        /// ```text
        ///   present::CLOSING  190 ms   the leaving animation, before the ask
        /// + GRACE           1000 ms   this constant: waiting for an answer
        /// + the recovery     150 ms   `give_back`'s fade back in
        /// = 1340 ms
        /// ```
        ///
        /// A window returning inside about a second and a half still reads as
        /// the answer to the key that was pressed; past that it reads as the
        /// session doing something by itself, which is the confusion the
        /// original comment here named and was right to name. One second is
        /// the largest value of *this* term that keeps the total inside that,
        /// so it is the value.
        ///
        /// The test that pins this measures from the request rather than from
        /// the press — it has the pane and not the keystroke — so its bound is
        /// this constant plus the recovery, and the two numbers are the same
        /// claim in two frames of reference. See
        /// `a_client_that_takes_six_hundred_milliseconds_to_close_is_never_shown_again`.
        ///
        /// **This is a deadline, and a deadline is the weakest evidence there
        /// is.** It fires on a client that said nothing, because saying nothing
        /// is all the protocol requires of a refusal. Evidence that arrives
        /// *before* it is better than the clock in every case, and
        /// [`Self::refused_with_a_dialog`] is the one piece of it acted on
        /// today: a client that answers a close by putting a new window on
        /// screen has told us what it is doing in as many words.
        ///
        /// What is still *not* done, deliberately: ending the wait early on the
        /// evidence that a client is honouring the request -- `Self::has_content`
        /// goes false as a client tears its surface down, which distinguishes
        /// "closing" from "refusing" far better than any timeout can. The
        /// asymmetry is the whole reason the dialog case could be taken and this
        /// one could not. Reading the dialog wrong ends the grace early and
        /// gives a window *back* that might have been about to go, which the
        /// next frame's `Panes::sync` corrects for free. Reading a teardown
        /// wrong means never bringing the window back at all, and this deadline
        /// is the only thing standing between a refused close and a lost window.
        const GRACE: std::time::Duration = std::time::Duration::from_millis(1000);

        // Over the panes, for the reason `settle_closing` is: a window that
        // has gone is a window that answered, and there is nothing left of it
        // to bring back.
        let due: Vec<crate::pane::PaneId> = self
            .panes
            .iter()
            .filter(|pane| {
                pane.asked_at()
                    .is_some_and(|at| now.saturating_sub(at) >= GRACE)
            })
            .map(Pane::id)
            .collect();
        for id in due {
            tracing::debug!(
                pane = id.get(),
                "a window refused to close; bringing it back"
            );
            // **#221: silence past the whole of `GRACE` is what
            // `settle_closing` tells an X11 close apart from one a dialog is
            // still answering.** `refused_with_a_dialog` is the other way a
            // pane comes back from a close and never reaches this loop at
            // all -- it retires `asked_at` through `give_back` well inside
            // `CLOSING`'s 190 ms, long before anything here could call it due
            // -- so marking the flag on this route alone is what keeps a
            // client that is visibly answering from ever being killed.
            let x11 = self
                .panes
                .get(id)
                .and_then(Pane::client)
                .is_some_and(|window| window.x11_surface().is_some());
            if x11 && let Some(pane) = self.panes.get_mut(id) {
                pane.mark_x11_refused_once();
            }
            self.give_back(id, now);
        }
        self.panes.iter().any(|pane| pane.asked_at().is_some())
    }

    /// Put a window that was asked to close back on screen, and stop waiting on
    /// it.
    ///
    /// The two halves of undoing a close, in the order that cannot strand a
    /// window: **the transform is restored first, and the wait is retired only
    /// if that worked.**
    ///
    /// The other order is issue #127's review finding 5. `present::clear` is
    /// not guaranteed to do anything -- `with_slot` declines rather than panics
    /// when the transform slot is already borrowed, which is the right trade
    /// for a compositor and the reason the call reports now. Retiring
    /// `asked_at` first meant that on such a frame the pane stopped being
    /// `Pane::leaving` while still holding `present::close`'s non-releasing,
    /// opacity-zero transform. Nothing else clears it: `settle_refused` will
    /// never look at the pane again, `close_pane` would decline a second
    /// `super+q` -- no, worse, it would *accept* one and animate an already
    /// invisible window out -- and the only remaining rescue is `move_pane`'s
    /// `present::from`, which needs a layout sweep that never comes in floating
    /// mode with no layout script. The result is a live window holding its
    /// place in the layout that nobody can see or reach.
    ///
    /// **Answering `false` is a retry, and each caller owns a different half of
    /// it.** On `settle_refused`'s path `asked_at` is left set, so the pane is
    /// still due next frame, `settle_refused` returns `true`, and the backend
    /// keeps drawing. On [`Self::refused_with_a_dialog`]'s path `asked_at` is
    /// `None` — the request has not gone out yet — so that mechanism does not
    /// reach it at all, and `Pane::answered` is what carries the retry:
    /// `settle_closing` finds the flag at the `CLOSING` deadline and calls this
    /// again instead of sending the request. Either way a busy slot costs a
    /// frame, which is what it costs everywhere else.
    ///
    /// That second half is #127's third review, finding 3, and it is the third
    /// comment in this area to have claimed coverage from the shape of the code
    /// rather than from anything that asserts it. The paragraph below used to
    /// be true only when `present::clear` happened to succeed;
    /// `a_dialog_whose_give_back_is_declined_does_not_lose_its_parent` is what
    /// makes it true when it does not.
    ///
    /// **Both timers, not just the one each caller happens to be holding.**
    /// `stop_closing` is a no-op on `settle_refused`'s path, where the request
    /// has long gone out and `closing_at` was cleared with it, and it is the
    /// whole of the point on [`Self::refused_with_a_dialog`]'s: a GTK file
    /// chooser is up well inside the 190 ms `CLOSING` window, and without this
    /// `settle_closing` would go on to send the request and close the parent
    /// out from under its own dialog. Undoing a close is one operation, so it
    /// is written once.
    ///
    /// **And the keyboard, because this is the other side of
    /// [`Self::hand_off_keyboard`].** The handoff leaves the seat holding
    /// nothing when the closing window was the only one open; a window coming
    /// back to a session whose keyboard is idle is exactly the case
    /// `settle_focus` exists for. It declines when something else has focus,
    /// so a window the user has moved on from does not steal it back.
    ///
    /// **Called on the frame the restore starts, and that is safe only because
    /// `settle_focus` judges the destination.** Here the pane is still drawn at
    /// `present::close`'s opacity zero; asked about the present, `settle_focus`
    /// declined the very window being given back, whenever too little real
    /// time had passed since `now` for the fade to show. See [`super::workspaces::SETTLED`], and
    /// `a_refused_close_gives_the_keyboard_back_to_the_window_it_brings_back`,
    /// which pins the progress-zero case rather than hoping for it.
    fn give_back(&mut self, id: crate::pane::PaneId, now: std::time::Duration) -> bool {
        /// How long the window takes to fade back in.
        const RETURN: std::time::Duration = std::time::Duration::from_millis(150);
        // **A let-go the close kept waiting is taken first, before the return
        // is aimed** (`Pane::let_go`). A layout that let the window out of its
        // tile during the fade -- a switch to floating inside it -- has said it
        // is in no tile now, and the return below lands on `pane_outer`, which
        // a tile caps. Aimed with the tile still held, a window wider than its
        // tile would fade back in to the tile's width rather than its own. And
        // it is put back if the give-back declines, because the window is then
        // still fading and its tile is still what cuts it.
        // `a_mode_switched_during_a_fade_does_not_squash_the_window_leaving`
        // asserts where the return lands, and
        // `a_give_back_that_declines_keeps_a_fade_cut_after_a_mode_switch` the
        // putting back.
        let owed = self.panes.get_mut(id).and_then(Pane::take_let_go);
        let Some(pane) = self.panes.get(id) else {
            // No pane, nothing to give back and nothing left waiting: a window
            // that went while this ran answered the close after all.
            return true;
        };
        let outer = self.pane_outer(pane);
        if !present::clear(pane, outer, now, RETURN, solium_animation::Curve::OutCubic) {
            if let (Some(owed), Some(pane)) = (owed, self.panes.get_mut(id)) {
                pane.owe_let_go(owed);
            }
            return false;
        }
        if let Some(pane) = self.panes.get_mut(id) {
            pane.forget_asked();
            pane.stop_closing();
            // And the debt, with the timers it was standing in for. Nothing is
            // owed once the transform has been restored, and leaving it set
            // would have `settle_closing` retry a give-back that already
            // happened on the next close this pane is ever given.
            pane.settled_answer();
        }
        self.redraw = true;
        // **The layout hears the refusal here, which is once per refusal.**
        // After the transform took and the timers were retired, and at no other
        // point: all three routes a refused window comes back by -- the grace
        // deadline in `settle_refused`, a dialog's answer in
        // `refused_with_a_dialog`, and `settle_closing`'s retry of an answer
        // whose first give-back was declined -- end in this function, and a
        // declined attempt returns above without telling anyone. See
        // `a_refusal_tells_the_layout_once_on_the_frame_the_window_comes_back`.
        //
        // The pane is not `leaving` any more, so a layout putting it back moves
        // it like any other window -- and `move_pane` animates from what is on
        // screen, which here is the held, shrunk, transparent end of the close:
        // the window fades in as it slides to wherever the layout puts it, over
        // the layout's duration rather than `RETURN`. Placed by nothing, it
        // plays the fade above. See
        // `a_refused_window_fades_back_in_from_where_it_vanished`, and
        // `a_refusal_by_dialog_fades_the_window_back_and_moves_its_neighbour_once`
        // for the dialog route, which sweeps several times in one dispatch.
        self.trigger_refused(id);
        self.settle_focus();
        true
    }

    /// A window that was asked to close has answered by opening another one.
    ///
    /// **The case that inverts `GRACE`'s argument**, and issue #127's review
    /// finding 3. `settle_refused` reasons that too long a grace period only
    /// makes a genuinely refused window wait, which is true for the client the
    /// grace period was lengthened for -- the honest-but-slow one -- and
    /// backwards for the client that refuses on purpose. "Save your changes
    /// before closing?" is a refusal delivered as a question, and under a flat
    /// deadline it left the parent window a hole for the whole second: the
    /// dialog floating over the space where its document used to be, with
    /// nothing to read and nothing to decide against. A second `super+q` could
    /// not clear it either, because the widened `Pane::leaving` guard correctly
    /// declines to start a second close on a pane that is still in one.
    ///
    /// **A new window from a client we just asked to close is evidence, and it
    /// is the safe kind.** The direction of the risk is what makes this
    /// actionable where reading a teardown is not: acting on it *returns* a
    /// window, so being wrong costs a window coming back that was going to
    /// leave anyway -- which the next `Panes::sync` undoes by itself when the
    /// client does finish closing. See `GRACE`'s note.
    ///
    /// Parentage rather than the client connection, deliberately. A file
    /// chooser is `set_parent`'d to the document that raised it, which says
    /// *this* window is what the dialog is about; a client that happens to open
    /// an unrelated window elsewhere in the same process while a close is
    /// pending has said nothing about the window being closed.
    ///
    /// **Three call sites, named because a claim about coverage is worth
    /// exactly what can be checked against it.** [`Self::parent_of`] reading
    /// both protocols' spellings of a parent is necessary and is not
    /// sufficient: what decides whether an X11 dialog is covered is whether any
    /// X11 *event* reaches this function, and the first draft of this comment
    /// asserted that it did on the strength of `parent_of` alone, while the
    /// only caller was `XdgShellHandler::parent_changed`. An XWayland
    /// application's unsaved-changes dialog left its parent a 1.19 s hole for
    /// the whole of #127's review. The callers are:
    ///
    /// * `XdgShellHandler::parent_changed` — `xdg_toplevel.set_parent`.
    /// * `XwmHandler::map_window_request` — an X11 window appearing with
    ///   `WM_TRANSIENT_FOR` already set, which is where nearly every one of
    ///   them arrives: smithay reads the property at `CreateNotify`, strictly
    ///   before the `MapRequest`.
    /// * `XwmHandler::property_notify` for `WmWindowProperty::TransientFor` —
    ///   the client that maps first and says whose dialog it is afterwards.
    ///
    /// **They are three equivalent call sites because the gate below is in this
    /// body, and the round that added the third listed them as equivalent while
    /// they were not.** The rule that a window which places itself is not an
    /// answer to anything stood as a comment at `map_window_request`'s call,
    /// and `property_notify` did not repeat it — so an X11 menu, tooltip,
    /// notification, splash or override-redirect window that set
    /// `WM_TRANSIENT_FOR` after mapping cancelled its parent's close. `super+q`
    /// faded the window out, brought it back, and never closed it. A rule kept
    /// at one caller is a rule broken at the next one added, which is the same
    /// argument [`Self::map_stacked`] makes about restacking.
    ///
    /// **And the honest limit, which is the part the comment before this one
    /// left out.** Only the first of the three is pinned by a test *through its
    /// own protocol*: an `X11Surface` cannot be built without a live XWayland
    /// and nothing in this suite has one. So the two X11 hooks are verified by
    /// reading them, and what they share with the tested path is the body below
    /// — which is every line of the decision, the `managed` gate included.
    /// `a_window_that_places_itself_does_not_cancel_a_close` pins that gate at
    /// the level the X11 hooks rely on: a child holding the *unmanaged* pane
    /// `take_unmanaged_pane` gives every menu, tooltip and override-redirect
    /// window, driven through the one caller this suite can drive. That is a
    /// weaker claim than "covered" and it is the one that is true.
    pub(crate) fn refused_with_a_dialog(&mut self, child: &Window) {
        // **A window that places itself is not an answer to anything**, and the
        // question is asked of the child before its parent is even looked up.
        //
        // `Pane::managed` is false for exactly the windows that mean nothing
        // here and is set in one place, `take_unmanaged_pane` — which is what
        // both of XWayland's self-placing branches call, and the drag icon's,
        // so a menu, a tooltip, a splash, a notification and an
        // override-redirect window are all caught by one question. A child with
        // no pane at all is not on screen and cannot be an answer either.
        if !self.panes.of(child).is_some_and(Pane::managed) {
            return;
        }
        let Parentage::Window(parent) = self.parent_of(child) else {
            return;
        };
        let Some(id) = self
            .panes
            .iter()
            .map(Pane::id)
            .find(|id| id.get() == parent)
        else {
            return;
        };
        if !self.panes.get(id).is_some_and(|pane| pane.leaving()) {
            return;
        }
        let now = self.clock.now();
        tracing::debug!(
            pane = id.get(),
            "a window answered a close with a dialog; bringing it back"
        );
        // **The answer is recorded before it is acted on, because acting on it
        // can fail.** That is #127's third review, finding 3: `give_back`
        // returns whether `present::clear` took, and this caller dropped the
        // answer. `settle_refused` cannot pick a declined give-back up here —
        // `asked_at` is still `None` inside `CLOSING` by definition, so the pane
        // is on no deadline that function reads — and `settle_closing` went on
        // to send the request and close the parent out from under its own
        // dialog. `Pane::answered` outlives the failed attempt, and
        // `settle_closing` retries from it. `give_back` clears it when it takes.
        if let Some(pane) = self.panes.get_mut(id) {
            pane.mark_answered();
        }
        // **The transform first, and nothing is retired unless it took.** Same
        // rule as `give_back`'s own, for the same reason: a frame that cleared
        // `closing_at` without restoring the presentation would leave a pane
        // that is not `leaving()`, not due at any deadline, and still holding
        // an opacity-zero transform -- finding 5 reintroduced by the fix for
        // finding 3. Declining now leaves the close where it was *and* the debt
        // recorded, rather than leaving the close to run to its end.
        //
        // `give_back` retires both timers, which is what matters for the dialog
        // that beat the 190 ms `CLOSING` deadline: without `stop_closing` the
        // request would still go out afterwards and close the parent out from
        // under its own dialog. See it for why that lives there and not here.
        self.give_back(id, now);
    }

    pub(crate) fn trigger_close(&mut self, pane: crate::pane::PaneId) {
        let id = pane.get();
        // **The close is over, so nothing may bring this window back** (#128).
        // The pane outlives this call: `Self::depart`, which is where a client
        // destroying its toplevel lands, turns it into what fades out for
        // `pane::LEAVING` when there is anything to fade, and otherwise it
        // lasts the rest of the frame -- every frame runs the Wayland dispatch,
        // then `settle`, and only then `sync_panes`, which retires it.
        // A client that went near its grace deadline was still `asked_at` in
        // that `settle`, so `settle_refused` gave the dead window back and the
        // layout was told `refused` after `close`: a leaf kept for a window
        // that no longer exists, for good. See
        // `a_client_that_goes_at_its_grace_deadline_is_not_refused_after_it_closed`.
        //
        // All three fields, because two routes lead back into `give_back`:
        // `settle_refused` on `asked_at`, and `settle_closing`'s retry on a due
        // `closing_at` with an answer owed. Only the first is driven by that
        // test; the second needs a declined give-back to reach, and the one way
        // this suite has to decline one, `present::jam_slot`, never lets go.
        //
        // **And the pane is marked gone, which is what keeps it leaving once
        // the timers are cleared** (#128's review, findings 5 and 7). Clearing
        // them alone made a window that no longer exists an ordinary window
        // for the rest of the frame: listed in `sol.windows()` as not leaving,
        // placed by a stateless layout and drawn at full opacity by
        // `move_pane`, closed a second time by `close_pane`, and put back into a
        // tree by an `adopt`. `Pane::leaving` answers yes for a gone pane, and
        // the snapshot leaves it out of every event but this one. See
        // `a_window_that_has_gone_is_neither_placed_nor_closed_again` and
        // `adopt_in_the_frame_a_window_went_keeps_no_leaf_for_it`.
        //
        // Before the dispatch, now that `gone` answers for it: a layout placing
        // the window in `close` itself -- a stateless one places every row it
        // is handed -- still finds it leaving and leaves alone the transform
        // holding it invisible. See
        // `a_layout_placing_a_closed_window_does_not_show_it_again`. And
        // whether or not there are scripts, because a session with none has a
        // deadline to disarm all the same.
        if let Some(pane) = self.panes.get_mut(pane) {
            pane.forget_asked();
            pane.stop_closing();
            pane.settled_answer();
            pane.went();
        }
        // Only for the snapshot, and the snapshot is the whole of what a script
        // sees. The pane is still here on two of the three routes in -- since
        // #126, `Self::depart` and `settle_closing`'s loading pane both call
        // this before the pane goes, and the first is
        // `a_client_that_disconnects_fades_out_and_is_told_gone_once`'s
        // `close N*` -- so a dialog waiting on this window would otherwise be
        // re-centred on the rect of the window that is leaving, and this pass
        // is the last one: nothing runs again to take it off the window that
        // moves into that space. See [`Self::parented`]. **Not on the third**:
        // the token-adoption merge in [`Self::claim_into`] removes the pane
        // first, because there both panes hold the same `Window` and this
        // snapshot would list it twice. Read in its code; no test drives it.
        self.closing = Some(pane);
        let snapshot = self.snapshot();
        self.closing = None;
        if let Some(mut scripts) = self.scripts.take() {
            let outcome = scripts.closed(id, snapshot);
            self.scripts = Some(scripts);
            self.apply(outcome);
        }
    }

    /// A window is going on its own: its client closed it, quit, crashed or was
    /// killed, its X11 window unmapped, or its application never arrived.
    /// Tell the scripts it has gone, and keep it on screen long enough to fade
    /// out as a window the compositor closes does (#126).
    ///
    /// **Every way a window goes on its own comes here**: `toplevel_destroyed`,
    /// `CompositorHandler::destroyed` for a window's own surface, an X11
    /// unmap, `settle_loading` and a failed spawn. A close the compositor asked
    /// for comes here too, when its client finally goes, and finds its fade
    /// already landed and nothing left to fade (`asked_at` below). Two ways
    /// out do not come here at all: a loading pane closed by hand, which
    /// `settle_closing` removes once its own fade has landed, and the
    /// token-adoption merge in [`Self::claim_into`], which is not a window
    /// leaving.
    ///
    /// **What it leaves is decided before anyone is told**, because telling
    /// is what moves things: `close` runs a layout, which grows the
    /// neighbours into this window's space, and a stateless layout places
    /// every row of `close`'s snapshot, this window's included. So the
    /// rectangle it is drawn at, the selections carrying it, its title and its
    /// picture are all read first, and its fade starts first, from where it
    /// stands. `move_pane` then declines to move a pane that has gone.
    ///
    /// **Then `close`, once, with the pane still here and still the window it
    /// was.** The same event a compositor's own close ends in and nothing
    /// else: no `closing`, because nobody asked for this close and there is
    /// no refusal to come back from. A layout reflows at `close`, and the
    /// window fades where it stood while its neighbours grow in, which is the
    /// picture #128 gives a close the compositor asked for. Its row in that
    /// snapshot says `leaving`, and it is in no snapshot after.
    /// `a_window_that_closes_itself_hands_its_space_over_as_it_fades`.
    ///
    /// **Then it becomes [`crate::pane::Content::Leaving`]** -- out of the
    /// space, and in the stack where it was, over the panes it was over and
    /// over the ones the layout grew into its space at `close`
    /// ([`crate::pane::Left::over`]), drawn from what it left
    /// ([`crate::pane::Remains`]) and from nothing else, and dropped by
    /// [`Self::settle_leaving`] when its fade is over. Or, with nothing to
    /// fade, it goes as it did before #126: a client's pane at the end of the
    /// frame in `sync_panes`, a loading pane at once.
    ///
    /// Asked at most once per window: most windows are heard going twice --
    /// a disconnecting client's surface and then its toplevel, an X11 window's
    /// surface and its unmap -- and `close` means gone exactly once.
    pub(crate) fn depart(&mut self, id: crate::pane::PaneId) {
        use crate::pane::{Left, Remains};
        enum Keep {
            Scene,
            Picture(crate::remains::Picture),
            Lost,
        }

        let now = self.clock.now();
        let Some(pane) = self.panes.get(id) else {
            return;
        };
        if pane.gone() || pane.ghost() {
            return;
        }
        let window = pane.client().cloned();
        let outer = self.pane_outer(pane);
        let geometry = self.pane_geometry(pane);
        let groups = if self.groups.is_empty() {
            Vec::new()
        } else {
            let monitor = self.named_monitor_of(outer);
            self.groups.holding_window(id.get(), monitor.as_deref())
        };
        let title = self.pane_title(id);
        let focused = self.looks_focused(id);
        // **On the fade it is already on, if a close the compositor started is
        // playing**: a client quitting inside those 190 ms leaves on that
        // close's schedule and from that close's transform, rather than
        // starting again from a window already half gone.
        // `a_client_that_quits_during_a_close_leaves_on_that_close`.
        let fading = pane.closing_at();
        let since = fading.map_or(now, |due| due.saturating_sub(present::CLOSING));
        // **Nothing left to fade** once that close has landed: the window has
        // been held at opacity zero since, waiting for its client to answer.
        let landed = pane.asked_at().is_some();
        let keep = if !pane.managed() || landed {
            // A menu, a tooltip or a drag icon goes the way a Wayland popup
            // does, at once.
            None
        } else if pane.has_standing_scene() {
            Some(Keep::Scene)
        } else {
            window.as_ref().and_then(|window| {
                let picture = crate::remains::Picture::of(window, self.textures.as_ref());
                if picture.drawable() {
                    Some(Keep::Picture(picture))
                } else if self.client_ready(window) {
                    Some(Keep::Lost)
                } else {
                    // Never painted, or unmapped itself before it went: there
                    // was nothing of it on screen to fade.
                    None
                }
            })
        };
        // Where it is in the stack, read before `close` lets a layout restack
        // anything. See `crate::pane::Left::over`.
        let mut over = if keep.is_some() {
            self.stacked_under(pane)
        } else {
            Vec::new()
        };
        if keep.is_some()
            && fading.is_none()
            && let Some(pane) = self.panes.get(id)
        {
            present::close(pane, outer, now);
        }
        // Where the layout has every other window, so what it grows into this
        // one's space at `close` can be put under the fade. Only when there
        // will be a fade.
        let laid: Vec<(crate::pane::PaneId, Rectangle<i32, Logical>)> = if keep.is_some() {
            self.panes
                .iter()
                .filter(|each| each.id() != id && !each.ghost())
                .map(|each| (each.id(), self.laid_out_at(each)))
                .collect()
        } else {
            Vec::new()
        };

        self.trigger_close(id);

        for (other, was) in laid {
            let grew_in = self
                .panes
                .get(other)
                .map(|each| self.laid_out_at(each))
                .is_some_and(|now| now != was && now.overlaps(outer));
            if grew_in && !over.contains(&other) {
                over.push(other);
            }
        }

        let Some(keep) = keep else {
            // A client's pane is retired by `sync_panes` once the space has
            // let go of its window, as it always was. A pane that never had a
            // client has no window for the space to let go of, and `sync_panes`
            // keeps every such pane -- so it goes here.
            if window.is_none() {
                self.panes.remove(id);
            }
            self.redraw = true;
            return;
        };
        // Out of the space now rather than at its next `refresh`: until then
        // `sync_panes` would find the element with no pane holding it, and
        // give it a new one.
        if let Some(window) = window.as_ref() {
            self.space.unmap_elem(window);
        }
        let Some(pane) = self.panes.get_mut(id) else {
            return;
        };
        let remains = match keep {
            Keep::Scene => pane
                .take_standing_scene()
                .map_or(Remains::Lost, Remains::Scene),
            Keep::Picture(picture) => Remains::Picture(picture),
            Keep::Lost => Remains::Lost,
        };
        tracing::debug!(
            pane = id.get(),
            remains = match &remains {
                Remains::Picture(_) => "picture",
                Remains::Scene(_) => "scene",
                Remains::Lost => "nothing",
            },
            "a window went on its own and fades out"
        );
        pane.leave(Left {
            since,
            outer,
            geometry,
            groups,
            over,
            title,
            focused,
            remains,
            fill: smithay::backend::renderer::element::Id::new(),
        });
        self.panes.changed();
        self.redraw = true;
    }

    /// End the fade of every window that went and is drawn from `surface`,
    /// if this commit gives `surface` a new buffer: its client kept the
    /// surface and is using it again, and the texture the fade holds is the
    /// one the new buffer will be uploaded into. See
    /// `crate::remains::Picture::holds`.
    ///
    /// The fade ends rather than going on without that surface: this is a
    /// window coming back -- a hide and a show -- and it is about to be drawn
    /// by a pane of its own.
    pub(super) fn let_go_of_reused(&mut self, surface: &WlSurface) {
        if !self.panes.iter().any(Pane::ghost) {
            return;
        }
        let attached = with_states(surface, |states| {
            matches!(
                states
                    .cached_state
                    .get::<smithay::wayland::compositor::SurfaceAttributes>()
                    .current()
                    .buffer,
                Some(smithay::wayland::compositor::BufferAssignment::NewBuffer(_))
            )
        });
        if !attached {
            return;
        }
        let id = surface.id();
        let reused: Vec<crate::pane::PaneId> = self
            .panes
            .iter()
            .filter(|pane| {
                pane.left().is_some_and(|left| {
                    matches!(&left.remains, crate::pane::Remains::Picture(picture) if picture.holds(&id))
                })
            })
            .map(Pane::id)
            .collect();
        for pane in reused {
            tracing::debug!(
                pane = pane.get(),
                "a surface a window that went is drawn from was given a new buffer"
            );
            self.panes.remove(pane);
            self.redraw = true;
        }
    }

    /// Tell scripts a close has begun, so a layout can reflow now.
    ///
    /// See [`Scripts::closing`]. Fired from [`Self::close_pane`] alone.
    fn trigger_closing(&mut self, pane: crate::pane::PaneId) {
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return;
        };
        let outcome = scripts.closing(pane.get(), snapshot);
        self.scripts = Some(scripts);
        self.apply(outcome);
    }

    /// Tell scripts a close was refused and the window is back.
    ///
    /// See [`Scripts::refused`]. Fired from [`Self::give_back`] alone.
    fn trigger_refused(&mut self, pane: crate::pane::PaneId) {
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return;
        };
        let outcome = scripts.refused(pane.get(), snapshot);
        self.scripts = Some(scripts);
        self.apply(outcome);
    }
}
