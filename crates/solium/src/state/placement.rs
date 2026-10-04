//! Where a window goes and what its client is told: putting a pane at a rectangle (`move_pane`,
//! `place`, `resize_to`) and whether its client is offered that size on this frame, stacking a
//! window with its modals kept above it, and maximising.

use super::*;

/// What a placement says about the tile a pane is held in.
///
/// Since #133 `Pane::placed` is two things: the layout's own rectangle for a
/// pane, which a tiled edge drag starts from (#124), and the tile a client is
/// held inside, which `Solium::pane_geometry` caps a client at and
/// `render::elements` cuts it to. The second is only right for a *tile*, and
/// `Solium::move_pane` is reached by more than tiles, so every caller says
/// which it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Standing {
    /// A layout's tile. The client is held inside this rectangle from now on.
    Tile,
    /// A layout placing a window it does not tile: `dialogs.lua` centring a
    /// modal over its parent, through `sol.place` with `tile = false`. The pane
    /// is taken out of any tile it was in, because a dialog is a floating
    /// window whichever mode is running -- at once, or, for a window being
    /// closed, once it is back: see `Pane::let_go`.
    Free,
    /// Not a layout at all -- `Solium::rescue_offscreen` dragging a window
    /// back onto a screen. A tiled pane is still tiled, at the rectangle it was
    /// brought back to; a floating one is still floating.
    Kept,
    /// A layout's tile, which is this outer rectangle, with the window placed
    /// inside it smaller than it: `sol.place`'s `tile = { x, y, w, h }`, which
    /// `tiling.lua` says for a window centred at its own maximum size (#115).
    ///
    /// The tile is what `Pane::placed` records, so the client is held inside
    /// the tile rather than inside its smaller pane, and a tiled edge drag
    /// begins from the tile's edges -- `Solium::pane_laid_out` reads that
    /// field, and a seam is a tile's edge, not the window's. Begun from the
    /// window's, the seam would jump inwards by the margin the centring left on
    /// the first frame of the drag, which is #124 by another door.
    /// `real_client::client_sizes::a_centred_window_is_dragged_from_its_tile`.
    Within(Rectangle<i32, Logical>),
}

/// A change a window makes that its scripts are told, and answer with the
/// motion it is drawn with (#49): which event they hear it as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Change {
    /// Fullscreen, entered or left: `sol.on("fullscreen", fn(id, entering))`.
    Fullscreen,
    /// Maximised, or restored: `sol.on("maximize", fn(id, entering))`.
    Maximize,
}

/// Whether this window is fullscreen or maximised, as the compositor last
/// decided it: in the pending state, which is what `Solium::toggle_maximize`
/// and the fullscreen requests write.
fn over_the_arrangement(window: &Window) -> bool {
    window.toplevel().is_some_and(|toplevel| {
        toplevel.with_pending_state(|state| {
            state.states.contains(xdg_toplevel::State::Fullscreen)
                || state.states.contains(xdg_toplevel::State::Maximized)
        })
    })
}

/// `rect`, moved from the monitor at `from` onto the one at `to`: at the same
/// place on it, and kept wholly on it. For `Solium::carry_across`, and
/// `a_fullscreen_or_maximised_window_moved_to_another_monitor_is_so_on_that_one`
/// with no layout.
fn across(
    rect: Rectangle<i32, Logical>,
    from: Rectangle<i32, Logical>,
    to: Rectangle<i32, Logical>,
) -> Rectangle<i32, Logical> {
    let size = Size::from((rect.size.w.min(to.size.w), rect.size.h.min(to.size.h)));
    let x = (to.loc.x + rect.loc.x - from.loc.x).clamp(to.loc.x, to.loc.x + to.size.w - size.w);
    let y = (to.loc.y + rect.loc.y - from.loc.y).clamp(to.loc.y, to.loc.y + to.size.h - size.h);
    Rectangle::new((x, y).into(), size)
}

/// A script's rectangle as a pane's outer one: rounded to whole pixels, and
/// never smaller than one.
pub(super) fn outer_of(rect: Rect) -> Rectangle<i32, Logical> {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a rect from a script is screen-sized"
    )]
    Rectangle::new(
        (rect.x.round() as i32, rect.y.round() as i32).into(),
        (
            (rect.w.round() as i32).max(1),
            (rect.h.round() as i32).max(1),
        )
            .into(),
    )
}

impl Solium {
    /// Put a window in the stack, and keep whatever is waiting on it above it.
    ///
    /// **The one way a managed window is mapped or raised.** Use this rather
    /// than `self.space.map_element`, which cannot know the one thing that has
    /// to be true afterwards.
    ///
    /// A modal dialog is the window that is holding its parent up: "Discard
    /// changes?" is the only thing on screen you are allowed to answer, and the
    /// document behind it is not going to accept a keystroke until you have. So
    /// a modal has to be *above* the window it belongs to, and nothing in the
    /// stack says so on its own. Focusing the parent raised it — one click on a
    /// strip of it left showing, or one pointer crossing with
    /// `focus_follows_mouse` — and the prompt went behind the window that was
    /// waiting on the answer, where it cannot be found and cannot be dismissed.
    ///
    /// **Here rather than in `focus_window`**, which is where the symptom was
    /// seen and not where the cause is. Every raise has the same effect, and
    /// there are a dozen: a click, a fullscreen, a maximise, a client mapping,
    /// a layout pass. `Space::map_element` takes the element out of the stack
    /// and pushes it back on top whatever `activate` says — that flag only
    /// decides who is told they are focused — so *every* call is a restack, and
    /// a rule kept at one caller is a rule broken at eleven.
    ///
    /// This is also why it is not a z-index: a modal belongs above its own
    /// parent, not above everybody, and a second window's prompt has no claim
    /// over the first window's.
    pub(crate) fn map_stacked(
        &mut self,
        window: Window,
        location: impl Into<Point<i32, Logical>>,
        activate: bool,
    ) {
        self.space.map_element(window.clone(), location, activate);
        self.lift_modals_over(&window);
    }

    /// Map a window a layout has placed, raised as [`Self::map_stacked`]
    /// raises it -- but never past a window that is leaving, or drawn at less
    /// than full opacity.
    ///
    /// **Issue #128's review, findings 2 and 6.** Every placement is a raise,
    /// and a layout places the windows it moves one after another, so the
    /// stack after a sweep was the sweep's order. Since #128 a close reflows at
    /// once: the neighbour growing into the space of a window that is fading
    /// there was placed and the fading window was not, so the neighbour was
    /// stacked over the fade and covered most of it -- and the refused window
    /// fading back in was covered the same way by the neighbour giving its
    /// space back. See
    /// `a_closing_window_fades_in_front_of_the_neighbour_moving_into_its_space`
    /// and `a_refused_window_fades_back_in_front_of_the_neighbour_making_room`.
    ///
    /// **"Not past" rather than "raise those again afterwards".** What goes
    /// back on top is the lowest such window above this one *and everything
    /// that was above it*, in the order they were in, so the placed window
    /// ends just below it and the windows above it keep their order. Raising
    /// only the fading window would lift it over windows it had been under.
    ///
    /// Only for layout placements. A click, a new window and a fullscreen raise
    /// through `map_stacked` as they always have: those mean "this one, in
    /// front", and a sweep means nothing about the stack at all.
    fn map_laid_out(&mut self, window: Window, location: Point<i32, Logical>, now: Duration) {
        // Collected before anything moves, bottom to top: everything from the
        // lowest window above this one that must stay above it, upward.
        let kept_over: Vec<Window> = self
            .space
            .elements()
            .skip_while(|element| **element != window)
            .skip(1)
            .skip_while(|element| !self.stays_over_a_layout(element, now))
            .cloned()
            .collect();
        self.map_stacked(window, location, false);
        for element in &kept_over {
            self.space.raise_element(element, false);
        }
    }

    /// Whether a layout placing another window must leave this one above it:
    /// a window being closed, from the press until it is given back or gone,
    /// one drawn translucent -- which is where a refused window is from the
    /// moment it is given back until its return lands -- and one fullscreen or
    /// maximised, which covers the arrangement rather than taking part in it,
    /// so a neighbour a sweep places is not stacked over it.
    /// `a_layout_leaves_a_fullscreen_or_maximised_window_where_it_is`.
    ///
    /// And one still shrinking back out of either (#49), which is drawn over
    /// its neighbours until it has: a sweep part of the way through would
    /// otherwise put a neighbour over it.
    /// `a_tiled_window_leaving_fullscreen_stays_in_front_while_it_shrinks`.
    fn stays_over_a_layout(&self, window: &Window, now: Duration) -> bool {
        self.panes.of(window).is_some_and(|pane| {
            pane.leaving()
                || over_the_arrangement(window)
                || pane
                    .shrinking()
                    .is_some_and(|shrinking| now < shrinking.until)
                || present::frame(pane, self.pane_outer(pane), now).opacity < 1.0
        })
    }

    /// Put every modal waiting on this window back above it, and their own
    /// modals above them.
    ///
    /// The chain is not hypothetical: a file chooser is modal for the document
    /// and its "Replace?" prompt is modal for the chooser, so raising the
    /// document has to lift two windows and in that order.
    ///
    /// `lifted` is what stops a client that names a cycle of parents — which
    /// neither `xdg_toplevel.set_parent` nor `WM_TRANSIENT_FOR` forbids — from
    /// walking for ever: each window is raised at most once.
    pub(super) fn lift_modals_over(&mut self, window: &Window) {
        let Some(pane) = self.panes.id_of(window) else {
            return;
        };
        let mut over = vec![pane];
        let mut lifted: Vec<crate::pane::PaneId> = Vec::new();
        while let Some(parent) = over.pop() {
            // Collected before anything moves: raising borrows the space, and
            // restacking the list being walked is how one gets skipped.
            let children: Vec<Window> = self
                .space
                .elements()
                .filter(|element| self.is_modal(element))
                .filter(|element| self.parent_of(element) == Parentage::Window(parent.get()))
                .cloned()
                .collect();
            for child in children {
                let Some(id) = self.panes.id_of(&child) else {
                    continue;
                };
                if lifted.contains(&id) {
                    continue;
                }
                lifted.push(id);
                // `false`: this is about the stack, not about focus. A dialog
                // lifted because its parent was clicked has not been clicked.
                self.space.raise_element(&child, false);
                over.push(id);
            }
        }
    }

    /// Put a window at an exact rectangle, without animating.
    ///
    /// What a resize drag calls every frame: the window must be under the
    /// pointer's corner *now*, so this deliberately does not go through the
    /// transform the way `place` does.
    pub(crate) fn resize_to(&mut self, window: &Window, outer: Rectangle<i32, Logical>) {
        let client = inner(outer, self.frame_insets(window));

        size_window(window, client);
        self.map_stacked(window.clone(), client.loc, false);
    }

    /// Move and resize a window for real, gliding it there from where it was.
    ///
    /// This is the layout's authority: it changes the geometry everything else
    /// reads. The animation is a *transform* on top — the window is drawn from
    /// its old rectangle and lands on the new one — so a layout change and a
    /// mode use the same machinery and cannot disagree about where a window is
    /// going.
    pub(super) fn place(
        &mut self,
        id: u64,
        rect: Rect,
        animation: AnimationSpec,
        now: Duration,
        standing: Standing,
    ) {
        let Some(pane) = self.panes.by_script_id(id).map(Pane::id) else {
            return;
        };
        // Captured before anything moves: this is where the animation starts.
        let Some(was) = self.pane_outer_of(pane) else {
            return;
        };

        let outer = outer_of(rect);

        self.move_pane(pane, outer, was, animation, now, standing);
    }

    /// Put a pane's outer rectangle somewhere, and make every copy of that
    /// fact agree.
    ///
    /// There are three, and missing any one of them is a window that does not
    /// move -- or worse, moves and comes back. The space is the authority for
    /// a mapped window, so `sync_panes` writes it into the pane's slot every
    /// frame: setting the slot without telling the space is undone before the
    /// next frame is drawn, silently. That is not hypothetical, it is what the
    /// first attempt at `rescue_offscreen` did.
    ///
    /// **This is where a tiled resize meets its client, and it used to send a
    /// configure every time it ran.** A layout's sweep places every leaf on
    /// every visible monitor whether that leaf moved or not, and a drag runs the
    /// sweep once a frame, so an unchanged window was configured sixty times a
    /// second for the length of a gesture — deduplicated on the wire for xdg by
    /// smithay's `has_pending_changes`, and not deduplicated at all for X11,
    /// where `size_window` sends a real `ConfigureWindow` each time. The two
    /// windows that *did* move were configured sixty times a second for real,
    /// which is the rate `crate::resizing` exists to say no client can answer.
    /// [`Self::offers_size`] is the one gate both of those now go through.
    pub(super) fn move_pane(
        &mut self,
        pane: crate::pane::PaneId,
        outer: Rectangle<i32, Logical>,
        was: Rectangle<i32, Logical>,
        animation: AnimationSpec,
        now: Duration,
        standing: Standing,
    ) {
        // **Nothing moves a window that has gone.** Scripts have been told
        // `close`, or its client has left it fading where it stood: a
        // stateless layout placing the rows of `close`'s own snapshot -- which
        // lists the window closing -- would otherwise move the tile a fading
        // picture is cut to, and a script holding the id can `sol.place` it.
        // `a_window_that_left_is_nobodys_to_find` places one.
        if self
            .panes
            .get(pane)
            .is_some_and(|held| held.gone() || held.ghost())
        {
            return;
        }
        // **Nor into a tile, nor where a layout floats it, while it is
        // fullscreen or maximised** (#150's second and third reviews). A
        // script cannot see that a window is either, and a layout places
        // every window it arranges on every pass -- `tiling.apply` does after
        // a window opens, a move by key and a window floated, and
        // `dialogs.place` centres every floated window again at the size it
        // has -- so a sweep resized a fullscreen window into its tile, or
        // into the work area under a bar, still fullscreen. So the window
        // stays where it is. A tile given to one is the tile it goes back to,
        // and leaving fullscreen or maximised goes into the newest tile the
        // layout gave. A placement that is not a tile leaves the tile it had
        // as the way back: leaving goes into it and asks the layout, which
        // floats a window floated while it was fullscreen from there, at the
        // tile's size rather than the monitor's.
        // `a_layout_leaves_a_fullscreen_or_maximised_window_where_it_is` and
        // `a_floated_window_made_fullscreen_is_left_so_by_a_sweep_and_goes_back_where_it_floated`.
        //
        // The one placement that moves such a window is a move by key onto
        // another monitor, and `carry_across` has made that one by now.
        // `a_fullscreen_or_maximised_window_moved_to_another_monitor_is_so_on_that_one`.
        if standing != Standing::Kept
            && self
                .panes
                .get(pane)
                .and_then(Pane::client)
                .is_some_and(over_the_arrangement)
        {
            let tile = match standing {
                Standing::Tile => Some(outer),
                Standing::Within(tile) => Some(tile),
                Standing::Free | Standing::Kept => None,
            };
            if let Some(tile) = tile
                && let Some(held) = self.panes.get_mut(pane)
            {
                held.set_placed(tile);
                held.leave_tile();
            }
            return;
        }
        // The frame's share comes off whichever sides it reserved; what is
        // left is the client's.
        let client = inner(outer, self.insets_of(pane));

        // **The configure, and only the configure.**
        //
        // The guard that shipped with #127 covered the transform at the bottom
        // of this function and nothing else, which left the *client-facing*
        // write running on a dying window -- issue #127's review finding 2.
        // `size_window` resizes `real_geometry` while `present::close` holds
        // `frame.rect` pinned at the rectangle the window was closed at. Those
        // two rectangles are exactly the pair `resizing::factor` divides -- the
        // client's committed buffer against the size the pane is drawn at -- so
        // the moment the layout hands the dying client a different size and the
        // client answers it, the leaving animation stretches the last buffer to
        // fill a rectangle it was never painted for. The window squashes as it
        // fades, which reads as the close going wrong rather than as a reflow
        // happening behind it.
        //
        // The configure is waste even when the client never answers: it asks
        // something in the middle of tearing itself down to re-lay-out at a
        // size that will never be drawn, on the one code path where the answer
        // cannot arrive in time to matter. Electron and the JVM are the clients
        // slow enough to still be running their quit handlers when it lands.
        //
        // **`map_stacked` is none of that, and suppressing it too was the
        // second review's finding 2.** It carries a *location* and no size, so
        // it is not half of any pair `resizing::factor` divides and cannot
        // stretch anything; and it tells no client anything -- it writes into
        // `self.space`, a window's position is not on the xdg wire at all, and
        // for X11 it is `size_window` that sends the `ConfigureWindow`. What it
        // is, is the third copy of the fact this function exists to keep in
        // agreement, and this function's own opening paragraph already says
        // what dropping it costs: *setting the slot without telling the space
        // is undone before the next frame is drawn, silently*. `pane_geometry`
        // answers `real_geometry` for a mapped client, so `sync_panes` copies
        // the space's stale rectangle back over the `set_slot` below on the
        // very next frame -- which made the slot, `Pane::placed` and the space
        // three different answers instead of one.
        //
        // The consequence was not cosmetic. A sweep that moved a pane *during*
        // a close -- a workspace switch, `rescue_offscreen`, a config reload --
        // followed by a refusal handed the window back at its **pre-close**
        // rectangle while `Pane::placed` said the layout had moved it. Those
        // two are exactly what `pane_laid_out` pairs, so #124's edge drag began
        // from a rectangle the window was not at. That path rests on `real.loc`
        // being compositor-set and therefore exact; this line is what keeps it
        // so.
        //
        // **What is still suppressed, said plainly rather than left to be
        // inferred.** The client's *size* stays one configure behind for as
        // long as the pane is leaving, because that configure was never sent.
        // It corrects itself on the first sweep after the window comes back --
        // the configure was suppressed before `offers_size` could record it as
        // told, so re-placing at the same rectangle is a change and goes out --
        // and `a_window_the_layout_moved_mid_close_comes_back_where_it_was_put`
        // asserts that rather than this paragraph asserting it.
        let leaving = self.panes.get(pane).is_some_and(Pane::leaving);

        // A client is moved and resized for real, and the space is told,
        // because the space is the authority for a mapped window.
        if let Some(window) = self.panes.get(pane).and_then(Pane::client).cloned() {
            if !leaving && self.offers_size(pane, &window, client, now) {
                size_window(&window, client);
            }
            // Placing is not focusing. It is still a raise -- see
            // `map_stacked` -- and `map_laid_out` is what keeps that raise from
            // burying a window that is leaving or fading; see the note on the
            // transform below.
            self.map_laid_out(window, client.loc, now);
        }
        // And the pane is told either way. For a mapped window this is what
        // `sync_panes` would write next frame anyway; for a pane whose
        // application has not arrived it is the whole of the move, because
        // there is nothing else holding its geometry.
        if let Some(held) = self.panes.get_mut(pane) {
            held.set_slot(client);
            // **And the layout's own answer, kept where no client can reach
            // it.** The line above is exactly the one `sync_panes` overwrites:
            // it writes the space's rectangle into the slot on every frame this
            // pane is not held, and the space reports a mapped window's size as
            // whatever the client last committed. So `slot` is the rectangle
            // asked for only until the client answers, and a client answering
            // with a size of its own is the ordinary case rather than the
            // exception. `Pane::placed` records the rectangle that was *asked
            // for*, which is the only copy of the layout's opinion the
            // compositor keeps; see `Self::pane_laid_out`.
            //
            // **Only for a tile**, because since #133 that field is also the
            // rectangle the client is held inside, and a placement that is not
            // a tile must not hold anything. See `Standing`.
            match standing {
                Standing::Tile => held.set_placed(outer),
                Standing::Within(tile) => held.set_placed(tile),
                Standing::Free => held.untile(),
                // Moved and not set, so a let-go a leaving pane is waiting on
                // is still owed after a rescue. See `Pane::move_tile`, and
                // `a_rescue_during_a_fade_keeps_the_let_go_it_is_waiting_on`.
                Standing::Kept => held.move_tile(outer),
            }
        }

        // **And the transform, unless this pane is leaving.**
        //
        // `present::from` is released on arrival and aimed at `Frame::real` —
        // full size, full opacity — which is right for every pane that is
        // staying and is the whole of issue #127's first fault for one that is
        // not. `Pane::closing_at` had three readers and this was not one of
        // them, so any layout sweep inside the 190ms `CLOSING` window put the
        // dying window back at full opacity, released the transform, and left
        // it to vanish with no animation at all. A sweep inside that window is
        // not exotic: another window opening, a layer surface's first
        // configure, a GTK4 `set_parent` or `set_modal` each cause one.
        //
        // **What a closing pane does when the layout moves it: it animates out
        // from where it was.** The slot moves under it and the transform does
        // not follow, so the window shrinks and fades at the rectangle it was
        // closed at while its neighbours reflow around the space it is about to
        // give up. The alternative — sliding to the new slot while fading —
        // was rejected on three counts.
        //
        // * It animates towards a place the window will never occupy. The pane
        //   is retired within `CLOSING` plus whatever the client takes, so the
        //   destination is a fiction, and it pulls the eye away from the window
        //   the user just acted on.
        // * `present::close` computes its target *once*, from the frame at the
        //   instant of the press. Following the layout means recomputing that
        //   target on every sweep, and a sweep can run once a frame —
        //   `tiling.lua` runs `tiling.apply` per frame for the whole of a seam
        //   drag. Restarting a 190ms easing at 60Hz is an animation that never
        //   finishes, which is the shape of the defect being fixed here.
        // * The close transform already owns this pane's presentation for the
        //   rest of its life, deliberately: `present::close` is the one
        //   transform written with `release: false`, so that the window stays
        //   invisible between the animation landing and the client acting. The
        //   layout is not the authority over where a leaving pane is *drawn*,
        //   and this line was the only place that said otherwise.
        //
        // The cost is that a closing window overlaps the one moving into its
        // space for up to 190ms -- on every close since #128, because a layout
        // closes up the moment the close is asked for. It is shrinking and
        // fading throughout, and what that reads as depends on which of the two
        // is in front.
        //
        // **The window being closed is, and that is decided rather than left
        // to the sweep** (#128's review, findings 2 and 6). `Space::map_element`
        // puts every window a layout places on top, whatever `activate` says,
        // and the `closing` sweep places every survivor and not the
        // window being closed -- so the neighbour growing into the space was
        // stacked over it on every close and covered most of the fade, which
        // is the reverse of the look this note was written to buy. So
        // `close_pane` raises the window as its close begins, and
        // `map_laid_out` never raises a placed window past one that is leaving
        // or drawn translucent, so no later sweep in the fade buries it either.
        // A refused window fading back in is the same case in reverse. See
        // `a_closing_window_fades_in_front_of_the_neighbour_moving_into_its_space`
        // and `a_refused_window_fades_back_in_front_of_the_neighbour_making_room`.
        //
        // In front costs the window underneath no input once the fade is over:
        // the dying pane is at opacity zero from the end of `CLOSING`, and
        // `Frame::covers` gates every hit test on `shows()`, so a press there
        // reaches what is drawn there. See
        // `a_press_where_a_closed_window_used_to_be_reaches_what_is_drawn_there`.
        //
        // **The bookkeeping is what survives, not the presentation.** All three
        // copies of where this pane lives — the space, the slot and
        // `Pane::placed` — are written for a leaving pane exactly as for a
        // staying one. Exactly two things are suppressed, and they are the two
        // a *client* can observe: the configure at the top of the function, and
        // this transform. The first draft of this guard covered only this line,
        // which left the configure running; the second suppressed the space as
        // well, which left the three copies disagreeing. See the note above
        // them for both.
        //
        // **With one exception, and it is about the presentation too:** a
        // `Standing::Free` placement does not clear `Pane::placed` on a leaving
        // pane. That field is the tile the fading window is cut to, so
        // `Pane::untile` keeps it and owes the let-go until the window is back.
        // See `Pane::let_go`, and
        // `a_mode_switched_during_a_fade_does_not_squash_the_window_leaving`,
        // which places a leaving window with `tile = false`.
        //
        // Note what needs no guard. `present::rebase` — the group path — is
        // safe for a leaving pane by construction: it preserves both the
        // destination and the release flag, so a closing transform rebased by a
        // workspace slide is still a closing transform. It is this function's
        // unconditional `from` that was the exception.
        //
        // **From what is on screen, not from where the books say the pane was**
        // (#128's review, finding 1). `was` is `pane_outer`: the space's
        // location and the size the client last committed. Between two sweeps
        // in one dispatch those are the first sweep's answer and not anything
        // that has been drawn, and a dialog answering a close, sent in one
        // flush, is several sweeps in one dispatch -- `give_back`'s `refused`,
        // then `parent_changed`, then `modal_changed`. Starting from
        // `Frame::real(was)` put the refused window back at full opacity before
        // its fade had drawn a frame, and in tiling threw its neighbour to its
        // new position at its old size. `present::frame` is what the next
        // frame would draw, and for a pane with no transform it is
        // `Frame::real(was)`, so a placement from rest starts where it always
        // did. See
        // `a_refusal_by_dialog_fades_the_window_back_and_moves_its_neighbour_once`.
        if let Some(held) = self.panes.get(pane).filter(|_| !leaving) {
            let start = present::frame(held, was, now);
            present::from(
                held,
                outer,
                start,
                now,
                animation.duration,
                animation.easing,
            );
        }
    }

    /// A fullscreen or maximised window that a `super+shift+arrow` has moved
    /// onto another monitor, fullscreen or maximised on that one: fullscreen
    /// covers the monitor its new tile or rectangle is on, maximised fills
    /// that monitor's work area, and the keyboard stays on it. Left over the
    /// old monitor, it was on the new monitor's workspace -- the layout said
    /// so -- over a screen showing another one, so it was drawn nowhere and
    /// still had the keyboard.
    ///
    /// The way back comes too, to the same place on the new monitor as it had
    /// on the old one. Left where it was, leaving fullscreen or maximised put
    /// a window with no tile to go into on the old monitor again, where the
    /// next relayout files it under that monitor's copy of its workspace, out
    /// of view.
    /// `a_fullscreen_or_maximised_window_moved_to_another_monitor_is_so_on_that_one`.
    ///
    /// Only for the window the keys moved (`Command::Place::moved`). A strip
    /// scrolled past its monitor's edge puts the far columns over the next
    /// monitor, and a fullscreen window in one of those is still on its own.
    /// `a_fullscreen_window_whose_column_scrolls_over_the_next_monitor_stays_on_its_own`.
    pub(super) fn carry_across(&mut self, id: u64, rect: Rect, standing: Standing) {
        let Some(pane) = self.panes.by_script_id(id).map(Pane::id) else {
            return;
        };
        let to = match standing {
            Standing::Within(tile) => tile,
            Standing::Tile | Standing::Free | Standing::Kept => outer_of(rect),
        };
        let Some(window) = self
            .panes
            .get(pane)
            .and_then(Pane::client)
            .filter(|window| over_the_arrangement(window))
            .cloned()
        else {
            return;
        };
        let Some(toplevel) = window.toplevel().cloned() else {
            return;
        };
        let Some(here) = self
            .pane_outer_of(pane)
            .and_then(|outer| self.output_of(outer))
        else {
            return;
        };
        let Some(there) = self.output_of(to).filter(|there| *there != here) else {
            return;
        };
        let (Some(from), Some(screen)) = (
            self.space.output_geometry(&here),
            self.space.output_geometry(&there),
        ) else {
            return;
        };
        let fullscreen = toplevel
            .with_pending_state(|state| state.states.contains(xdg_toplevel::State::Fullscreen));
        let rect = if fullscreen {
            Some(screen)
        } else {
            self.maximised(&window, to)
        };
        let Some(rect) = rect else {
            return;
        };

        if let Some(held) = self.panes.get_mut(pane) {
            let back = held.restore().map(|back| across(back, from, screen));
            held.set_restore(back);
            held.set_slot(rect);
        }
        toplevel.with_pending_state(|state| state.size = Some(rect.size));
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
        self.map_stacked(window, rect.loc, false);
        self.redraw = true;
        tracing::debug!(
            fullscreen,
            to = %there.name(),
            "a fullscreen or maximised window was moved to another monitor"
        );
    }

    /// Whether the client hears about this rectangle on this frame, and the
    /// bookkeeping that decides it.
    ///
    /// Three answers, in order, and the order is the design:
    ///
    /// 1. **A pane already under a bridge answers from its own hold.** The
    ///    throttle is per pane, not per gesture, which is what stops a window
    ///    merely pushed aside by someone else's drag from having its one
    ///    configure swallowed by an interval the dragged window opened. Each
    ///    hold's clock starts when that pane first moved.
    /// 2. **A client already at this exact rectangle is told nothing.** This is
    ///    most of a layout's sweep: `tiling.apply` re-places every leaf on every
    ///    visible monitor, and in a dwindle tree two of them changed. The whole
    ///    rectangle is compared and not just the size, because for X11
    ///    `size_window` is the only thing that carries a *position* — `map_stacked`
    ///    moves the window in the space and tells the client nothing — so
    ///    deduplicating on size alone would leave an X11 window told to stay
    ///    where it no longer is.
    /// 3. **Anything else is a real change, and is sent.** If a gesture is live
    ///    this is also the moment a pane joins the bridge, and the immediacy is
    ///    deliberate: the first offer of a new size goes out on the frame it is
    ///    decided, and only the ones after it are throttled.
    ///
    /// **Case (1) is only the drag's throttle when the drag is what is placing
    /// this pane.** A bridge outlives its gesture by up to `PATIENCE`, and
    /// `move_pane` is reached by a config reload, a `modes.use` from a
    /// keybinding, a workspace switch and `rescue_offscreen` as well — none of
    /// which has a next frame to resend anything. Routing one of those through
    /// the throttle dropped its one configure while `move_pane` went on writing
    /// the slot, which leaves the pane drawn, stretched, at a rectangle its
    /// client was never told about for the rest of the gesture.
    /// [`Self::resize_gesture`] is exactly "the layout sweep running right now
    /// belongs to a live drag", so it is the question, and anything else offers
    /// the same rectangle unthrottled through `Hold::placed` — through the hold
    /// rather than around it, so `asked` still names what the client last heard.
    ///
    /// **A toplevel carrying a pending change is sent whatever the throttle
    /// says**, for the same reason and one more: `size_window` is a pending size
    /// *and* a `send_pending_configure`, so a throttled frame skips the flush
    /// too, and a maximise or a decoration mode agreed by somebody else is then
    /// blocked behind an interval for exactly the windows a drag is touching.
    ///
    /// Note what is *not* here: nothing arms a hold without
    /// [`Self::resize_gesture`]. A keyboard nudge, a reload, a monitor change
    /// and a workspace switch all reach `move_pane`, and a hold armed by one of
    /// them could never be released — `Self::release_resize` has one caller and
    /// it is the pointer grab — so it would answer `Settle::Waiting` for ever
    /// and hold the slot and the space apart for ever. They fall to (2) and (3),
    /// which is what they had before minus the configures for panes that did not
    /// move.
    pub(super) fn offers_size(
        &mut self,
        pane: crate::pane::PaneId,
        window: &Window,
        client: Rectangle<i32, Logical>,
        now: Duration,
    ) -> bool {
        let committed = window.geometry().size;
        // **And anything else the toplevel is carrying.** A maximise, a
        // fullscreen or a decoration mode agreed before the initial configure
        // went out is a pending change somebody else wrote and is waiting on.
        // Every one of those sends its own configure today, so this is belt and
        // braces rather than a known hole; it is here because the alternative
        // failure is a window that never hears an answer it is blocked on, and
        // there is no cheaper way to be sure than asking.
        let pending = window
            .toplevel()
            .is_some_and(smithay::wayland::shell::xdg::ToplevelSurface::has_pending_changes);
        // Whether the sweep reaching this pane is the live drag's own. Read
        // before the borrow below, which needs `self` mutably.
        let dragging = self.resize_gesture.is_some();
        // Case (1). The borrow ends on this line, so the arm below can reach
        // `self` again.
        let bridged = self
            .resize_bridge
            .as_mut()
            .and_then(|bridge| bridge.panes.iter_mut().find(|held| held.pane == pane))
            .map(|held| {
                if dragging && !pending {
                    held.hold.dragged(client, committed, now)
                } else {
                    held.hold.placed(client, committed, now)
                }
                .is_some()
            });
        let (told, held) = match bridged {
            Some(told) => (told, true),
            None => self.offers_first_size(pane, window, client, committed, now),
        };
        // A pending flush is worth a configure even when the rectangle is one
        // the client has already been told: the size is not what it is waiting
        // for.
        let told = told || pending;
        if crate::resizing::trace::on() {
            let hold = self.held_hold(pane);
            let asked = hold.map_or(client, crate::resizing::Hold::asked);
            crate::resizing::trace::line(
                "layout",
                format_args!(
                    "pane={} slot={},{} {}x{} committed={}x{} asked={},{} {}x{} told={} \
                     held={} refused={} unanswered={}",
                    pane.get(),
                    client.loc.x,
                    client.loc.y,
                    client.size.w,
                    client.size.h,
                    committed.w,
                    committed.h,
                    asked.loc.x,
                    asked.loc.y,
                    asked.size.w,
                    asked.size.h,
                    u8::from(told),
                    u8::from(held),
                    u8::from(hold.is_some_and(crate::resizing::Hold::refused)),
                    // Which side of `SILENCE` the verdict beside it was taken
                    // on. The two answers fail in opposite directions, so a log
                    // without this cannot say which one it caught.
                    hold.map_or(0, crate::resizing::Hold::unanswered),
                ),
            );
        }
        told
    }

    /// Cases (2) and (3) of [`Self::offers_size`]: a pane with no bridge entry.
    ///
    /// Returns whether the client is told and whether a hold was armed, which
    /// are different questions. A pane is told without being bridged by every
    /// caller that is not a drag.
    fn offers_first_size(
        &mut self,
        pane: crate::pane::PaneId,
        window: &Window,
        client: Rectangle<i32, Logical>,
        committed: Size<i32, Logical>,
        now: Duration,
    ) -> (bool, bool) {
        // The client's own rectangle: where the space has it, at the size it
        // last committed. Read before `map_stacked` moves it, which is why this
        // is answered here rather than after the move.
        //
        // **This is the right question for case (2) and the wrong one for the
        // edges below**, and the two used to share it. "Has the client already
        // been put here" is about where the *client* is, so it asks the space.
        // "Which of this pane's edges did this placement move" is about the
        // *pane*, and the pane's previous rectangle is its slot: during a drag
        // the client's committed size is frames behind the slot, so deriving an
        // edge from it mixes the client's latency into the answer and can name
        // an edge the placement never touched — `moved_edges` would see both
        // sides of an axis move where only one did, or a far edge move where
        // only the near one did, and `Hold::pins` and `anchored` would then
        // hang a held picture against the wrong side of a window.
        let before = self.real_geometry(window);
        let changed = before != Some(client);
        // A hold is armed by a live gesture and by nothing else, and only for a
        // pane whose rectangle actually changed: a layout re-placing a leaf
        // exactly where it already is has moved nothing, and a hold for it
        // would pin a slot that needs no pinning until the gesture ended.
        let released = self.resize_gesture.as_ref().map(|gesture| gesture.released);
        let Some(released) = released.filter(|_| changed) else {
            return (changed, false);
        };
        // **The pane's own moved edge, not the pointer's.** See
        // `crate::resizing::moved_edges`: the neighbour across a seam has the
        // opposite edge pulled, and a pane shoved sideways by someone else's
        // drag has neither. Against the pane's previous slot for the reason
        // `before` gives; a pane with no slot yet cannot have moved an edge, so
        // `client` against itself answers `ResizeEdge::None`, which is the
        // honest "nothing to anchor against".
        let previous = self.panes.get(pane).map_or(client, Pane::slot);
        let edges = crate::resizing::moved_edges(previous, client);
        // **The same drag's hold if this pane already had one, rather than a
        // fresh one.** `settle_resize` forks per frame, so a layout that claims
        // a drag on one frame and not the next hands this pane back and forth
        // between the floating path and the bridge. Building a new hold each
        // way reset `Hold::told`, so every flip bought an unthrottled configure
        // and an alternating handler restored the sixty a second this fix
        // removes. Same client, same gesture, same throttle — only the edges
        // are this path's to name. See `Hold::retargeted`.
        //
        // `released` is reasserted because `arm_resize_gesture` rearms the
        // bridge's holds before the sweep and a floating hold arriving during it
        // missed that: its deadline must stop for the same reason theirs did.
        let carried = self
            .resize_hold
            .take_if(|held| held.pane == pane && &held.window == window)
            .map(|held| held.hold);
        let (hold, told) = match carried {
            Some(mut hold) => {
                hold.retargeted(edges);
                hold.rearm(released);
                let told = hold.dragged(client, committed, now).is_some();
                (hold, told)
            }
            // A pane joining the bridge hears immediately, and only the offers
            // after it are throttled: `Hold::new` records this frame as the one
            // the client was spoken to on, and the caller sends.
            //
            // `committed` and not `before.size`, which is the same number —
            // `real_geometry` builds its size from `window.geometry()` — read
            // from the one source that answers for a window the space has let
            // go of as well.
            None => (
                crate::resizing::Hold::new(edges, committed, client, now, released),
                true,
            ),
        };
        let held = crate::resizing::Held {
            window: window.clone(),
            pane,
            hold,
        };
        match self.resize_bridge.as_mut() {
            Some(bridge) => {
                bridge.panes.push(held);
                (told, true)
            }
            // `arm_resize_gesture` creates the bridge with the gesture, so this
            // is unreachable rather than a case: answered instead of asserted
            // because a compositor may not panic. The client is still told,
            // because losing a hold is not a reason to lose a configure.
            None => (true, false),
        }
    }

    /// Where a pane is drawn now, as the start of a change it is about to
    /// make: read before the change, because the change is what moves it.
    ///
    /// What [`present::frame`] answers, so for a pane at rest it is its old
    /// rectangle, and for one part of the way through a change it is where
    /// that change has got to: a second toggle mid-flight starts from what is
    /// on screen, not from the slot it left.
    /// `a_second_toggle_mid_flight_starts_from_where_the_window_is_drawn`.
    ///
    /// `None` for a pane with nothing on screen to move from: one never shown,
    /// or of no size -- a player started with `--fs` asks for fullscreen
    /// before it has drawn, and its first picture is `open`'s to bring in --
    /// and one that is leaving, whose picture is its close's.
    pub(super) fn drawn_before(&self, pane: crate::pane::PaneId) -> Option<Frame> {
        let held = self
            .panes
            .get(pane)
            .filter(|held| present::was_shown(held) && !held.leaving())?;
        let outer = self.pane_outer(held);
        if outer.is_empty() {
            return None;
        }
        Some(present::frame(held, outer, self.clock.now()))
    }

    /// Tell the scripts a window has gone fullscreen or maximised, or left
    /// either, and draw it moving there with the motion they answer (#49).
    ///
    /// **Called once the change is made**: the client has been sent its new
    /// size on the toggle, not at the end of the animation, the space and the
    /// slot say where the window now lives, and a layout has placed it if it
    /// went back into a tile. What is left is the picture, which is moved the
    /// way a layout moves one: [`present::from`], from `start` -- where it was
    /// drawn before the change, [`Self::drawn_before`] -- to the slot with the
    /// frame it now has, and released when it lands, so at rest a window
    /// holds no transform and a fullscreen game or video is a plain element
    /// again. Until its client commits at the new size its last picture is
    /// stretched into the rectangle drawn, as on every glide.
    /// `a_window_glides_into_fullscreen_and_out_again`,
    /// `the_client_is_told_its_new_size_on_the_toggle`.
    ///
    /// **The motion is the scripts'**: the listeners of `change`'s event set
    /// it with `sol.animate`, and with no answer -- no listener, or no
    /// scripts -- the window is where it lives on the next frame, as before
    /// #49. A layout's own glide for a window going back into a tile is
    /// replaced, so the change's motion is the one it is drawn with, there as
    /// anywhere else. `tests::a_change_is_answered_with_the_motion_sol_animate_set`,
    /// and `tests/scenarios/fullscreen-tiled.lua` for the tile.
    ///
    /// **The compositor's move comes after the listeners' commands**, so a
    /// `sol.present` of the window in one is replaced by it, and a change a
    /// listener makes -- `sol.toggle_fullscreen` back again -- is made at once
    /// and not told: telling it would ask the same listener again, for ever.
    /// `a_listener_that_toggles_the_change_back_is_not_told_it_again`.
    pub(super) fn transition(
        &mut self,
        pane: crate::pane::PaneId,
        change: Change,
        entering: bool,
        start: Option<Frame>,
    ) {
        let motion = if self.telling_change {
            None
        } else {
            self.tell_change(pane, change, entering)
        };
        let now = self.clock.now();
        let motion = motion.unwrap_or(AnimationSpec {
            duration: Duration::ZERO,
            easing: present::Curve::OutCubic,
        });
        if let Some(held) = self.panes.get_mut(pane) {
            held.set_shrinking(None);
        }
        let Some(held) = self.panes.get(pane).filter(|held| !held.leaving()) else {
            return;
        };
        // **A mode's picture is the mode's.** A window the overview draws as
        // a thumbnail stays one: the change is made, and the mode letting go
        // brings the window to its new rectangle. Replacing the thumbnail
        // with the glide landed it over the overview's grid while the mode
        // still held the input.
        // `a_window_a_mode_presents_stays_where_the_mode_draws_it`.
        if present::presented(held) {
            return;
        }
        let Some(start) = start else {
            return;
        };
        // **Instant is no transform at all**, not one of no length: that
        // one's target -- the new rectangle -- is what the next frame draws,
        // with the old picture stretched into it, and it is released only
        // after that frame. A game on the `instant` list flashed its old
        // picture across the monitor before snapping back to its own size. A
        // glide in flight is stopped too, and the window is drawn as it is
        // from the next frame, as before #49.
        // `an_instant_change_draws_the_window_as_it_is_on_the_next_frame`.
        if motion.duration.is_zero() {
            present::release(held);
            self.redraw = true;
            return;
        }
        let to = grown(held.slot(), self.insets_of(pane));
        present::from(held, to, start, now, motion.duration, motion.easing);
        let lands = now + motion.duration;
        let window = held.client().cloned();
        // **In front while it shrinks, and over the bars leaving fullscreen.**
        // A window going fullscreen is lifted by being fullscreen, from the
        // moment it starts to grow; one leaving stops being fullscreen on the
        // toggle, and dropped under the bars then, it would be shrinking from
        // the size of the monitor with the bars drawn over it. And a window
        // going back into a tile has just been placed by the layout's sweep
        // with its neighbours, and one placed after it was stacked over it: it
        // shrank from behind that neighbour. So it is raised once the sweep is
        // done, and stays in front and lifted for as long as it moves.
        // `stacking::a_window_is_lifted_as_it_starts_to_grow_and_dropped_once_it_has_shrunk`,
        // `a_tiled_window_leaving_fullscreen_stays_in_front_while_it_shrinks`.
        if !entering {
            if let Some(held) = self.panes.get_mut(pane) {
                held.set_shrinking(Some(crate::pane::Shrinking {
                    until: lands,
                    lifted: change == Change::Fullscreen,
                }));
            }
            if let Some(window) = window {
                self.space.raise_element(&window, false);
                self.lift_modals_over(&window);
            }
        }
        self.redraw = true;
    }

    /// The `fullscreen` or `maximize` event, told, and the motion its
    /// listeners answered with. See [`Self::transition`].
    fn tell_change(
        &mut self,
        pane: crate::pane::PaneId,
        change: Change,
        entering: bool,
    ) -> Option<AnimationSpec> {
        let snapshot = self.snapshot();
        let mut scripts = self.scripts.take()?;
        let outcome = match change {
            Change::Fullscreen => scripts.fullscreen(pane.get(), entering, snapshot),
            Change::Maximize => scripts.maximize(pane.get(), entering, snapshot),
        };
        self.scripts = Some(scripts);
        let motion = outcome.motion;
        self.telling_change = true;
        self.apply(outcome);
        self.telling_change = false;
        motion
    }

    /// Fill the work area, or go back to where the window was.
    pub(super) fn toggle_maximize(&mut self, window: &Window) {
        let Some(id) = self.panes.id_of(window) else {
            return;
        };
        let Some(toplevel) = window.toplevel().cloned() else {
            return;
        };

        // **A fullscreen window leaves fullscreen for it.** Its way back is the
        // one slot on the pane, kept for `unfullscreen_request`, and toggled
        // from here it was spent: the window stayed fullscreen with nothing
        // kept, and leaving fullscreen then had nowhere to go. So the
        // maximised state under the fullscreen one is flipped, and the window
        // leaves fullscreen into it -- which is what `unfullscreen_request`
        // does already for a window that was maximised when it went
        // fullscreen, the kept rect and the tile included.
        // `maximising_a_fullscreen_window_leaves_fullscreen_for_it`.
        let (fullscreen, was_maximized) = toplevel.with_pending_state(|state| {
            (
                state.states.contains(xdg_toplevel::State::Fullscreen),
                state.states.contains(xdg_toplevel::State::Maximized),
            )
        });
        if fullscreen {
            toplevel.with_pending_state(|state| {
                if was_maximized {
                    state.states.unset(xdg_toplevel::State::Maximized);
                } else {
                    state.states.set(xdg_toplevel::State::Maximized);
                }
            });
            XdgShellHandler::unfullscreen_request(self, toplevel);
            return;
        }

        let Some(current) = self.real_geometry(window) else {
            return;
        };
        let Some(filled) = self.maximised(window, current) else {
            return;
        };
        let start = self.drawn_before(id);

        // On the pane and not on its frame, which is where it was until #92.
        // For this toggle a window with no frame to keep it on was latent, not
        // seen: its one caller is `frame_action`, reached only from a button
        // on a `Styled` frame, so a window with no frame had no button to
        // press either. The frameless case was reachable only through
        // fullscreen, where a client drawing its own frame had no rect kept
        // at all.
        let restore = self.panes.get_mut(id).and_then(Pane::take_restore);

        let (back, maximized) = match restore {
            // Restoring: back to exactly where it was, because that rect was
            // stored rather than recomputed -- unless the monitor it was on
            // has gone since, see `back_on_a_screen`.
            Some(previous) => (Some(self.back_on_a_screen(window, previous)), false),
            // Maximised with no way back kept: a window that went fullscreen
            // before it had drawn, so `fullscreen_request` kept no rect, and
            // was maximised from there. Restored as leaving fullscreen restores
            // it, with no rect and so no size, and the client picks its own;
            // maximising it again left the key dead for a press.
            // `a_window_maximised_with_no_way_back_is_restored_with_no_size`.
            None if was_maximized => (None, false),
            None => (Some(filled), true),
        };

        // **And out of its tile, or back into it (#133).** A tiled client is
        // held inside `Pane::placed`, and a maximised one is not tiled: left
        // there, the work area it is about to be configured to would be cut
        // down to the tile it is leaving. The tile is kept for the way back,
        // so a window restored into it is tiled again at once rather than at
        // the next sweep -- which is what a tiled edge drag started from it
        // reads (#124).
        let tiled = self.panes.get_mut(id).is_some_and(|pane| {
            if maximized {
                pane.set_restore(Some(current));
                pane.leave_tile();
                false
            } else {
                pane.return_to_tile();
                pane.placed().is_some()
            }
        });

        toplevel.with_pending_state(|state| {
            state.size = back.map(|back| back.size);
            if maximized {
                state.states.set(xdg_toplevel::State::Maximized);
            } else {
                state.states.unset(xdg_toplevel::State::Maximized);
            }
        });
        toplevel.send_pending_configure();
        if let Some(back) = back {
            // The slot as well as the space, as `unfullscreen_request` writes
            // both: it is what `transition` moves the picture to.
            if let Some(pane) = self.panes.get_mut(id) {
                pane.set_slot(back);
            }
            self.map_stacked(window.clone(), back.loc, true);
        }
        // Back in a tile, and the layout says where that is now: a sweep while
        // the window was maximised kept the tile it gave, which need not be the
        // one the window left -- a move by key traded it for its neighbour's --
        // so it is asked, as `unfullscreen_request` asks.
        // `a_layout_leaves_a_fullscreen_or_maximised_window_where_it_is`.
        if tiled {
            self.trigger_relayout();
        }
        // Moved there on screen, from where it was drawn. Not when it is
        // restored with no size, which its client picks: there is no
        // rectangle yet to move it to.
        // `a_window_glides_into_maximised_and_out_again`.
        self.transition(
            id,
            Change::Maximize,
            maximized,
            start.filter(|_| back.is_some() || tiled),
        );
        tracing::debug!(maximized, "window maximise toggled");
    }

    /// Where this window goes when it is maximised: the work area of the
    /// monitor `on` is on, less the window's frame.
    ///
    /// The monitor the window is on, not the one the pointer is on: a window
    /// maximised while you point at the other screen must fill its own, and
    /// jumping across is the last thing a maximise should do.
    ///
    /// The frame's height comes out of the client's share, which is the same
    /// arithmetic as placement: a maximised window and its frame together fill
    /// the work area exactly. Asked by [`Self::toggle_maximize`], and by
    /// `unfullscreen_request` for a window that was maximised when it went
    /// fullscreen and so goes back to being maximised.
    pub(super) fn maximised(
        &self,
        window: &Window,
        on: Rectangle<i32, Logical>,
    ) -> Option<Rectangle<i32, Logical>> {
        Some(inner(self.work_area_of(on)?, self.frame_insets(window)))
    }

    /// A kept rect this window is being put back at, moved onto a screen if
    /// no part of it, frame included, is on one.
    ///
    /// **The rect was stored, and the monitor it was stored on may have gone
    /// since** -- unplugged, or disconnected when it slept. `rescue_offscreen`
    /// brings the window itself onto a remaining screen when that happens, but
    /// not the rect it goes back to, and it runs only when the monitors
    /// change: a window put back at the rect as it was sat on no screen until
    /// the next hotplug. Moved by the same rule as that rescue, so the two
    /// agree about where a stranded window goes.
    pub(super) fn back_on_a_screen(
        &self,
        window: &Window,
        back: Rectangle<i32, Logical>,
    ) -> Rectangle<i32, Logical> {
        let insets = self.frame_insets(window);
        self.rescued(grown(back, insets))
            .map_or(back, |outer| inner(outer, insets))
    }
}
