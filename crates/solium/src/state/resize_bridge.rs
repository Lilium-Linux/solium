//! An edge drag's resize, on this side of `crate::resizing`: holding a pane where the drag puts it
//! until its client answers, the bridge for the panes a layout moves along with a tiled drag, the
//! configures sent on the way and on release, and offering the drag to the scripts' layouts.

use super::*;

impl Solium {
    /// Act on a resize an edge drag asked for.
    ///
    /// Offered to layouts first. Only a window no layout claims is resized
    /// directly, which is what keeps a tiled window from growing over its
    /// neighbour instead of moving the seam between them.
    ///
    /// **This is the fork between the two resize paths, and it forks on who
    /// decides the rectangle — not on who talks to the client.** The claimed
    /// branch is the *tiled* path: the layout moves a seam and `move_pane`
    /// writes what the layout decided. #120 lives entirely on that side, in
    /// `Tiling::drag_seam` — which seam a dragged edge moves. The unclaimed
    /// branch is the *floating* path: `hold_resize` makes the drag's own
    /// rectangle authoritative until the client catches up, which is #113.
    ///
    /// What used to stand here said "nothing here sizes the window" of the
    /// claimed branch and that the two fixes never touch each other's side.
    /// Both were false, and issue #123 is what they cost: `move_pane` sizes the
    /// window, once per pane per frame, and the client-facing half of a resize —
    /// throttle the configure, keep the slot authoritative while the client
    /// catches up, bridge the last buffer, end on any answer or on `PATIENCE` —
    /// is the same problem whichever branch decided the rectangle. It is now the
    /// same code: [`Self::resize_bridge`] for the claimed branch, `resize_hold`
    /// for the unclaimed one, both out of `crate::resizing`.
    ///
    /// **The fork is per frame, not per gesture**, which is why arming is
    /// bracketed around the call rather than done after it. `trigger_resize`
    /// runs `apply` itself, so the layout's whole sweep — every `move_pane` it
    /// causes — happens inside that call and before this function learns
    /// whether the drag was claimed at all. A handler that returns without
    /// placing anything (`scrolling.lua`'s guard at screen x 0 does exactly
    /// that, and calls itself a defect) drops a single frame onto the unclaimed
    /// branch mid-gesture; the two must therefore be able to hand a pane back
    /// and forth without either leaving a rectangle behind.
    ///
    /// Once a frame, not once per pointer event, and the difference is the
    /// whole reason this is here rather than in the motion handler. A mouse
    /// reports movement up to a thousand times a second; each report was
    /// dispatching to Lua, laying out every window, and sending a configure to
    /// every client. Clients cannot answer at that rate and do not try — they
    /// fall behind, and the window being dragged stutters against the pointer
    /// instead of following it.
    ///
    /// `pending_resize` is one slot, so the last position before the frame is
    /// the one that counts. That is exactly the coalescing this wants: the
    /// pointer is wherever it is now, and the positions it passed through since
    /// the last frame are of no interest to anyone.
    ///
    /// Coalescing is the *rate* the client is asked at, not the rate the
    /// window moves at. Since #113 those are two different numbers:
    /// `hold_resize` puts the pane where the drag says on every one of these
    /// calls, and `crate::resizing::TELL_EVERY` decides which of them the
    /// client hears about.
    ///
    /// Returns whether anything was resized.
    pub(crate) fn settle_resize(&mut self) -> bool {
        let now = self.clock.now();
        let dragged = match self.pending_resize.take() {
            Some(request) => {
                // Armed before the layout is asked, because the layout's sweep
                // runs inside the asking. See [`Self::arm_resize_gesture`].
                self.arm_resize_gesture(&request);
                let claimed = self.trigger_resize(&request);
                self.resize_gesture = None;
                if claimed {
                    // A layout took it: the window's rectangle is the layout's
                    // arithmetic and went through `move_pane`, which wrote the
                    // slot, the space and — on the throttle's schedule — the
                    // client. The bridge armed above is now watching every pane
                    // it moved, so a floating hold on this window would be a
                    // second authority over one of them.
                    //
                    // This window's hold and no other's, which is the same care
                    // `begin_resize` takes and for the same reason: a hold on
                    // some other window is a gesture this one knows nothing
                    // about, and dropping it abandons that window's rectangle
                    // half-reconciled.
                    self.drop_resize_hold_for(&request.window);
                } else {
                    // **Nobody placed anything this frame, so the dragged pane
                    // goes back to the floating path.** `hold_resize` takes its
                    // slot, its authority and — since it is the same client in
                    // the same gesture — its hold; see `Hold::retargeted` there
                    // for why the hold is carried rather than rebuilt. Every
                    // *other* pane the gesture has moved keeps its own: those
                    // panes are not on the floating path, nothing else is
                    // watching them, and a layout that unclaims one frame and
                    // claims the next — which is `modes.lua` and `scrolling.lua`
                    // today — would otherwise abandon them mid-gesture with
                    // slots the client never agreed to.
                    self.hold_resize(&request, now);
                }
                self.redraw = true;
                true
            }
            None => false,
        };
        // **Before settling, and on every frame rather than on the ones that
        // carried a motion.** See [`Self::flush_resize`]: the throttle's own
        // trailing edge is the last thing a paused drag is waiting for, and a
        // paused drag is what every drag is for the moment before the button
        // comes up.
        self.flush_resize(now);
        // Whether or not the pointer moved this frame: a hold outlives the
        // gesture by however long the client takes to answer the last
        // configure, and something has to be watching for that answer.
        let settled = self.settle_resize_hold(now) | self.settle_resize_bridge(now);
        // A recorded release exists to be handed to a hold that had not been
        // born yet. If nothing survived this frame, nothing can be: the only
        // things that create one are `hold_resize` and `offers_size`, and the
        // only thing that reaches either is a motion from a grab which the
        // release has already ended. See [`Self::resize_ended`].
        if self.resize_hold.is_none() && self.resize_bridge.is_none() {
            self.resize_ended = None;
        }
        settled || dragged
    }

    /// Tell `move_pane` that the layout sweep it is about to see belongs to a
    /// live edge drag.
    ///
    /// Everything about the bridge's lifetime is decided here, so the three
    /// cases are together:
    ///
    /// * **No bridge.** One is created, empty. `offers_size` fills it with
    ///   whichever panes the sweep actually moves, which may be none — a
    ///   `resize` listener that claims the drag by running a command about some
    ///   other window claims it just as hard as one that lays anything out.
    ///   A purely floating drag therefore creates one here and
    ///   `settle_resize_bridge` drops it again at the end of the same frame,
    ///   every frame. That is not worth deferring: `Vec::new` does not
    ///   allocate, so the whole of the per-frame cost is one `Arc` refcount
    ///   either way, and the alternative — the gesture carrying the window so
    ///   the bridge can be built lazily at the first arm — puts a second copy
    ///   of "which window is being dragged" in the compositor for two atomic
    ///   increments a frame.
    /// * **A bridge for this same window.** The previous gesture on it ended and
    ///   its holds are waiting out `PATIENCE`. They are handed to this gesture
    ///   rather than reconciled: reconciling means adopting, and adopting a
    ///   tiled pane takes it off its tile, so a border nudged twice in a quarter
    ///   of a second would snap every pane the first nudge moved. See
    ///   `resizing::Hold::rearm`.
    /// * **A bridge for a different window.** That gesture is over and this one
    ///   will never place its panes, so it is ended the way its own deadline
    ///   would have ended it. The same rule `begin_resize` applies to a floating
    ///   hold, and for the same reason: a deadline expiring in the middle of
    ///   somebody else's gesture adopts whatever size the client happened to be
    ///   at.
    pub(super) fn arm_resize_gesture(&mut self, request: &ResizeRequest) {
        let released = self
            .resize_ended
            .as_ref()
            .filter(|(ended, _)| ended == &request.window)
            .map(|&(_, at)| at);
        match self.resize_bridge.as_mut() {
            Some(bridge) if bridge.window == request.window => {
                for held in &mut bridge.panes {
                    held.hold.rearm(released);
                }
            }
            Some(_) => {
                self.adopt_bridge();
                self.resize_bridge = Some(Bridged {
                    window: request.window.clone(),
                    panes: Vec::new(),
                });
            }
            None => {
                self.resize_bridge = Some(Bridged {
                    window: request.window.clone(),
                    panes: Vec::new(),
                });
            }
        }
        self.resize_gesture = Some(Gesture { released });
    }

    /// Whether this pane's slot is the authority on its window's size just now.
    ///
    /// Either path can be the reason. A pane is under exactly one of them —
    /// `settle_resize` hands a pane from one to the other rather than letting
    /// both claim it — so this is an "or" and not a precedence.
    pub(crate) fn holding_resize(&self, pane: crate::pane::PaneId) -> bool {
        self.held_hold(pane).is_some()
    }

    /// The hold governing this pane, whichever path put it there.
    ///
    /// The floating slot first because it is one comparison; the bridge is a
    /// short list — the panes one gesture moved — walked only when the first
    /// misses.
    pub(super) fn held_hold(&self, pane: crate::pane::PaneId) -> Option<&crate::resizing::Hold> {
        if let Some(held) = self.resize_hold.as_ref().filter(|held| held.pane == pane) {
            return Some(&held.hold);
        }
        self.bridged(pane).map(|held| &held.hold)
    }

    /// The same, to be written to. See [`Self::flush_resize`], which is the
    /// only caller: a trailing flush has to update `asked` and `told` on
    /// whichever of the two authorities is holding this pane, and it has no
    /// business knowing which.
    fn held_hold_mut(&mut self, pane: crate::pane::PaneId) -> Option<&mut crate::resizing::Hold> {
        if let Some(held) = self
            .resize_hold
            .as_mut()
            .filter(|held| held.pane == pane)
            .map(|held| &mut held.hold)
        {
            return Some(held);
        }
        self.resize_bridge
            .as_mut()?
            .panes
            .iter_mut()
            .find(|held| held.pane == pane)
            .map(|held| &mut held.hold)
    }

    /// This pane's entry in the bridge, if a tiled gesture is moving it.
    fn bridged(&self, pane: crate::pane::PaneId) -> Option<&crate::resizing::Held> {
        self.resize_bridge
            .as_ref()?
            .panes
            .iter()
            .find(|held| held.pane == pane)
    }

    /// Take this pane out of the bridge without reconciling anything.
    ///
    /// For the one case where something else has taken over its rectangle: the
    /// floating path claiming a pane the layout has stopped placing.
    pub(super) fn drop_bridged(&mut self, pane: crate::pane::PaneId) {
        drop(self.take_bridged(pane));
    }

    /// The same, handing back what was taken.
    ///
    /// The floating path wants the hold rather than merely wanting it gone:
    /// it is the same client in the same gesture, so its throttle, its `asked`
    /// and its refusal all still apply. See `hold_resize`.
    fn take_bridged(&mut self, pane: crate::pane::PaneId) -> Option<crate::resizing::Held> {
        let bridge = self.resize_bridge.as_mut()?;
        let at = bridge.panes.iter().position(|held| held.pane == pane)?;
        Some(bridge.panes.swap_remove(at))
    }

    /// What fills this pane while its client catches up, or `None` if it is not
    /// being dragged. See [`crate::resizing::factor`].
    pub(crate) fn resize_fill(&self, pane: crate::pane::PaneId) -> Option<crate::resizing::Fill> {
        Some(self.held_hold(pane)?.fill(self.resizing.fill))
    }

    /// Which of this pane's edges a live drag is pulling, or `None` if none is.
    ///
    /// For a fill that does not stretch: the picture has to stay against the
    /// edges that are standing still, or it travels with the pointer and the
    /// window's contents slide about inside their own frame. See
    /// [`crate::resizing::Fill::Hold`].
    ///
    /// **A tiled pane's edges are its own**, which for the neighbour across a
    /// seam is the opposite side from the one the pointer has hold of, and for a
    /// pane merely pushed aside is neither. See [`crate::resizing::moved_edges`].
    pub(crate) fn resize_pins(&self, pane: crate::pane::PaneId) -> Option<(bool, bool)> {
        Some(self.held_hold(pane)?.pins())
    }

    /// The slot a held window's pane is keeping, if this is that window.
    ///
    /// `sync_panes` asks: the space is normally the authority it copies into
    /// every pane's slot, and copying it over a held slot would undo the drag
    /// between one frame and the next — silently, which is exactly how the
    /// first attempt at `rescue_offscreen` went wrong.
    ///
    /// **A tiled drag is the same disagreement, once per moved pane.** Before
    /// #123 the layout wrote a slot and `sync_panes` overwrote it with the
    /// client's committed rectangle on the very next frame, so the authority the
    /// layout had just asserted survived exactly as long as the transform
    /// `move_pane` left behind — which, at `duration = 0`, is until `Solium::settle`
    /// runs after the same frame's render. That is the alternation in #123: a
    /// frame carrying a motion drew the layout's rectangle, and a frame without
    /// one drew the client's.
    pub(super) fn held_slot(&self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        let pane = self
            .resize_hold
            .as_ref()
            .filter(|held| &held.window == window)
            .map(|held| held.pane)
            .or_else(|| {
                self.resize_bridge
                    .as_ref()?
                    .panes
                    .iter()
                    .find(|held| &held.window == window)
                    .map(|held| held.pane)
            })?;
        self.panes.get(pane).map(Pane::slot)
    }

    /// A fresh edge drag is starting on this window.
    ///
    /// The previous drag's hold may still be live: its deadline runs for a
    /// quarter of a second after the button came up, and a second drag can
    /// easily begin inside that — a border nudged twice, or a double-click that
    /// turns into a drag. Inheriting it would let the *old* gesture's deadline
    /// expire in the middle of the new one and adopt whatever size the client
    /// happened to be at, which is the shake back again with a longer period.
    ///
    /// Holds on other windows are left alone: one of those expiring reconciles
    /// its own window correctly and has nothing to do with this drag.
    ///
    /// **Reconciled rather than abandoned, and the rectangle it lands on is the
    /// answer.** Simply forgetting the old hold leaves the pane's slot holding
    /// a size the client never agreed to with nothing left watching for the
    /// answer, so `pane_geometry` falls back to `real_geometry` — the drag's
    /// origin paired with the client's old size, which is issue #113's
    /// rectangle exactly, for every frame until the new drag's first motion.
    /// Ending it the way the deadline would ends it *somewhere*, which is all
    /// the next gesture needs.
    ///
    /// Returning the rectangle is what stops that reconciliation being visible:
    /// a `ResizeGrab` computes every frame from the rectangle it was given, so
    /// a caller that read one before this ran would drag from a rectangle this
    /// has since changed and the window would jump on the first motion.
    /// `None` for a window with no pane, which is not a case any caller can
    /// reach — every one of them found the window through a pane — and each has
    /// its own rectangle to fall back on.
    pub(crate) fn begin_resize(&mut self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        if self
            .resize_hold
            .as_ref()
            .is_some_and(|held| &held.window == window)
        {
            let taken = window.geometry().size;
            self.adopt_resize(taken);
        }
        // **And the tiled half of the same problem, which is not reconciled but
        // stopped.** `arm_resize_gesture` already rearms this window's bridge,
        // and that runs on the first *motion* — so a press, a pause and then a
        // drag leaves the previous gesture's deadline running across the whole
        // of the pause, and `settle_resize_bridge` runs every frame. A press
        // held for longer than `PATIENCE` therefore adopts every pane the last
        // drag moved, which is exactly the snap `resizing::Hold::rearm` exists
        // to prevent, arriving between the press and the first pixel of motion.
        //
        // Stopped rather than adopted, for `rearm`'s reason: adopting a tiled
        // pane takes it off its tile, so a border nudged twice inside a quarter
        // second would snap every pane the first nudge moved. The new gesture
        // owns these panes and will keep placing them.
        //
        // A bridge belonging to a *different* window is left alone. That
        // gesture's deadline expiring reconciles its own panes correctly and
        // has nothing to do with this drag — the same rule the floating hold
        // above follows, and `arm_resize_gesture` ends it on the first motion.
        if let Some(bridge) = self
            .resize_bridge
            .as_mut()
            .filter(|bridge| &bridge.window == window)
        {
            for held in &mut bridge.panes {
                held.hold.rearm(None);
            }
        }
        // Whatever gesture that record belonged to, it is not this one.
        self.resize_ended = None;
        self.pane_outer_of(self.panes.id_of(window)?)
    }

    /// Let go of a hold without reconciling anything.
    ///
    /// For the cases where the rectangle has been decided by someone else — a
    /// layout claiming the drag, the window going away — rather than by the
    /// client answering. Nothing to adopt: whoever took over owns the
    /// rectangle now.
    fn drop_resize_hold(&mut self) {
        self.resize_hold = None;
    }

    /// The same, for one window's hold and nobody else's.
    pub(super) fn drop_resize_hold_for(&mut self, window: &Window) {
        if self
            .resize_hold
            .as_ref()
            .is_some_and(|held| &held.window == window)
        {
            self.drop_resize_hold();
        }
    }

    /// End a hold by letting the client's own size win.
    ///
    /// The rule [`crate::resizing::Settle::Adopt`] names, in one place because
    /// two things reach it: the deadline expiring, and a fresh gesture arriving
    /// before it does. Either way the rectangle the old hold was waiting on
    /// will never be agreed, and the slot has to stop claiming it — pinned to
    /// the edges that drag was not holding, or the gesture ends by moving the
    /// one edge the user never touched.
    fn adopt_resize(&mut self, taken: Size<i32, Logical>) {
        let Some(held) = self.resize_hold.as_ref() else {
            return;
        };
        let (window, pane, hold) = (held.window.clone(), held.pane, held.hold);
        self.drop_resize_hold();
        self.land_on(&window, pane, &hold, taken);
    }

    /// The same for every pane a tiled gesture moved, all at once.
    ///
    /// Used where a whole bridge has been orphaned — a gesture starting on a
    /// different window while this one's holds are still waiting — which is the
    /// bridge's version of what `begin_resize` does to a stale floating hold.
    /// Each pane lands on its own client's size, because each was waiting on its
    /// own client.
    fn adopt_bridge(&mut self) {
        let Some(bridge) = self.resize_bridge.take() else {
            return;
        };
        for held in bridge.panes {
            let taken = held.window.geometry().size;
            self.land_on(&held.window, held.pane, &held.hold, taken);
        }
    }

    /// Put one pane's slot where the client's own size says, and stop holding it.
    ///
    /// The arithmetic behind both of the above, in one place because there are
    /// now four ways in: the deadline expiring, a fresh gesture arriving before
    /// it does, a client refusing a size it was offered, and a whole tiled
    /// gesture being orphaned.
    ///
    /// **The slot stops filling its tile, and that is the honest outcome rather
    /// than a shortcut.** A tiled pane whose client will not take the size the
    /// layout gave it has a gap on one side of it whatever this does; putting
    /// the gap against the edge that moved — which is what `anchored` does with
    /// this pane's own edges — is the difference between the window staying
    /// where the user put it and its far edge walking across the desktop. Issue
    /// #115 is where reading a client's minimum belongs, and until then a
    /// refusal is only visible here.
    fn land_on(
        &mut self,
        window: &Window,
        pane: crate::pane::PaneId,
        hold: &crate::resizing::Hold,
        taken: Size<i32, Logical>,
    ) {
        let Some(slot) = self.panes.get(pane).map(Pane::slot) else {
            return;
        };
        let landed = hold.anchored(slot, taken);
        if let Some(pane) = self.panes.get_mut(pane) {
            pane.set_slot(landed);
        }
        self.map_stacked(window.clone(), landed.loc, false);
        self.redraw = true;
        tracing::debug!(
            asked = ?slot.size,
            given = ?taken,
            "a client settled at a size of its own"
        );
    }

    /// Put the pane where the drag says, now, and let the client catch up.
    ///
    /// **This is the fix for #113.** What it does *not* do is as important as
    /// what it does: it does not wait for the client, and it does not derive
    /// the window's rectangle from the client's size. The pane's slot takes the
    /// dragged rectangle whole — origin and size in the same frame — so the
    /// edge under the pointer is the edge that moves and the opposite edge does
    /// not move at all.
    ///
    /// The space is told the new *position* in the same breath, deliberately.
    /// Position never needed a client's consent; only size does. Holding the
    /// position back too would put the space and the slot into exactly the
    /// disagreement issue #84 is about, for no gain, and would break every
    /// reader of `real_geometry` for the length of a drag.
    pub(super) fn hold_resize(&mut self, request: &ResizeRequest, now: Duration) {
        let Some(pane) = self.panes.id_of(&request.window) else {
            // A window with no pane has nowhere to hold a rectangle. That is
            // not a case anyone should reach — `sync_panes` gives every client
            // in the space a pane — but the old behaviour is a correct
            // fallback rather than a guess, so take it and say nothing.
            self.resize_to(&request.window, request.wanted);
            return;
        };
        // **`insets_of`, which is what every reader of this slot uses.**
        // `frame_insets` is the other spelling and it answers differently for
        // `Frame::Pending`: `is_decorated` is false there, so it reserves
        // nothing, while `insets_for` reserves a titlebar so that a window does
        // not change shape when its frame arrives. A pane whose decoration
        // failed to build is `Pending` *permanently* — see
        // `decoration::Decorations::insert` — so writing the slot with one
        // spelling and reading it back with the other would re-grow the
        // rectangle by a titlebar on every frame of the drag: the top edge a
        // titlebar above where the pointer is and a client sized that much too
        // tall. The round trip `pane_outer(inner(wanted)) == wanted` that this
        // whole fix rests on holds only when both halves ask the same question.
        let client = inner(request.wanted, self.insets_of(pane));
        if let Some(held) = self.panes.get_mut(pane) {
            held.set_slot(client);
        }
        self.map_stacked(request.window.clone(), client.loc, false);

        let size = request.window.geometry().size;
        // **This pane's hold, wherever the last frame left it.** A bridge entry
        // means the layout claimed the drag on an earlier frame and has stopped
        // claiming it — `scrolling.lua`'s guard at screen x 0 is one frame of
        // exactly that — and it is still the same client in the same gesture,
        // so it keeps its throttle, its `asked`, and whether it has refused
        // anything. Rebuilding instead reset `Hold::told`, which handed every
        // flip a free configure in each direction; a handler that alternated
        // therefore restored the sixty a second `crate::resizing::TELL_EVERY`
        // exists to remove.
        //
        // Taken and not copied: leaving it in the bridge would make two things
        // answer `holding_resize` for one pane with two different opinions
        // about which edges are pulled.
        let carried = self.take_bridged(pane).map(|mut held| {
            // The pointer's edges now, because this pane is the one under the
            // hand again. A tiled pane's edges are derived from what moved; a
            // floating one's are the grab's. See `crate::resizing::moved_edges`.
            held.hold.retargeted(request.edges);
            held.hold
        });
        match &mut self.resize_hold {
            // The same drag, still going. The client hears about it on the
            // throttle's schedule, not this frame's.
            //
            // A `carried` here would mean one pane held by both authorities at
            // once, which `settle_resize` hands back and forth precisely to
            // avoid; dropping it is how that is repaired rather than an
            // oversight, and the floating hold is the newer of the two.
            Some(held) if held.pane == pane => {
                if let Some(tell) = held.hold.dragged(client, size, now) {
                    size_window(&request.window, tell);
                }
            }
            // A new drag — or a drag that has moved to a different window,
            // which a grab cannot do but a script rebinding one could. Either
            // way the previous hold is over and its window keeps whatever
            // rectangle it last had.
            //
            // **Born knowing whether its gesture is still going.** It very
            // often is not: the button comes up during input dispatch and this
            // runs at the frame, so every gesture quick enough to fit in one
            // dispatch batch arrives here already over. A hold that took
            // `released: None` regardless would wait for a release that has
            // already happened, which never comes again — see
            // [`Self::resize_ended`] and `resizing::Hold::new`.
            _ => {
                let released = self
                    .resize_ended
                    .as_ref()
                    .filter(|(ended, _)| ended == &request.window)
                    .map(|&(_, at)| at);
                let hold = match carried {
                    Some(mut hold) => {
                        hold.rearm(released);
                        if let Some(tell) = hold.dragged(client, size, now) {
                            size_window(&request.window, tell);
                        }
                        hold
                    }
                    None => {
                        size_window(&request.window, client);
                        crate::resizing::Hold::new(request.edges, size, client, now, released)
                    }
                };
                self.resize_hold = Some(crate::resizing::Held {
                    window: request.window.clone(),
                    pane,
                    hold,
                });
            }
        }
    }

    /// The pointer has let go of an edge drag.
    ///
    /// Sends the final configure — the one the throttle must not get the last
    /// word on — and starts the deadline. The pane keeps the rectangle the
    /// gesture ended on until the client answers or the deadline runs out; see
    /// `settle_resize_hold`.
    ///
    /// **Records the release whether or not there is a hold to record it on**,
    /// which is the whole of why a gesture can always be let go of. See
    /// [`Self::resize_ended`].
    ///
    /// Called from inside the pointer grab, so it must not touch the seat. It
    /// does not: a clock, a pane's slot, the request the last motion left
    /// behind, and one configure.
    pub(crate) fn release_resize(&mut self, window: &Window) {
        let now = self.clock.now();
        // **Written down first, and unconditionally.** There may be no hold yet
        // — a press, a motion and this release inside one dispatch batch all
        // run before the frame that creates one — and a release that went
        // unrecorded because there was nothing to record it on would leave the
        // hold born a moment later waiting for it for ever. See
        // [`Self::resize_ended`].
        self.resize_ended = Some((window.clone(), now));
        self.release_bridge(window, now);

        let Some(held) = self.resize_hold.as_ref() else {
            return;
        };
        if &held.window != window {
            return;
        }
        let Some(slot) = self.panes.get(held.pane).map(Pane::slot) else {
            self.drop_resize_hold();
            return;
        };
        // **The rectangle the gesture ended on, which is not always the slot's.**
        // The last motion of a drag is routinely still sitting in
        // `pending_resize` when the button comes up — a grab's callbacks run
        // during input dispatch and `settle_resize` runs at the frame — so the
        // slot is one motion out of date here. Telling the client the stale size
        // sends two configures for one release, and if it answers the first,
        // `Hold::note` records that as a refusal of a size it was never offered:
        // `settle` then takes the `declined == asked` path and adopts the
        // *pre-release* rectangle on the spot, throwing away the end of the
        // drag rather than waiting for the answer to the size that was.
        let pane = held.pane;
        let client = self
            .pending_resize
            .as_ref()
            .filter(|request| &request.window == window)
            .map_or(slot, |request| inner(request.wanted, self.insets_of(pane)));
        let size = window.geometry().size;
        let Some(held) = self.resize_hold.as_mut() else {
            return;
        };
        let tell = held.hold.release(client, size, now);
        size_window(window, tell);
        self.redraw = true;
    }

    /// The same for a tiled gesture: one final configure per pane it moved.
    ///
    /// **This is what stops the throttle losing the end of a drag.** A pane's
    /// hold keeps `asked` at the rectangle the client was last *told*, so a pane
    /// whose last change fell inside an interval has an `asked` the drag has
    /// already moved on from. Left there, its client would answer the size
    /// before that one, `settle` would wait out `PATIENCE` for an answer that
    /// cannot come, and the gesture would end by snapping the pane to a size
    /// from the middle of the drag. `Hold::release` sends unconditionally for
    /// exactly this reason on the floating path; the tiled path needs it once
    /// per moved pane.
    ///
    /// The slot is the rectangle, and unlike the floating path there is no
    /// pending motion to reconcile against: the layout — not the pointer —
    /// decides a tiled pane's rectangle, and the layout's last word is what is
    /// in the slot. A motion still sitting in `pending_resize` reaches the
    /// layout on the next frame and `Hold::dragged` sends it then, unthrottled,
    /// because a hold that has been released no longer consults the interval.
    pub(super) fn release_bridge(&mut self, window: &Window, now: Duration) {
        let Some(bridge) = self.resize_bridge.as_ref() else {
            return;
        };
        if &bridge.window != window {
            return;
        }
        let moved: Vec<(Window, crate::pane::PaneId)> = bridge
            .panes
            .iter()
            .map(|held| (held.window.clone(), held.pane))
            .collect();
        for (client, pane) in moved {
            // The pane or its client has gone, or the pane has been given a
            // different client since the gesture started. The same check
            // `settle_resize_bridge` makes, and for the same reason: a
            // `size_window` on the window this hold remembers would configure a
            // client that no longer owns the rectangle being sent, and the slot
            // read below is not that client's anyway.
            if self.panes.get(pane).and_then(Pane::client) != Some(&client) {
                continue;
            }
            let Some(slot) = self.panes.get(pane).map(Pane::slot) else {
                continue;
            };
            let committed = client.geometry().size;
            let Some(held) = self
                .resize_bridge
                .as_mut()
                .and_then(|bridge| bridge.panes.iter_mut().find(|held| held.pane == pane))
            else {
                continue;
            };
            let tell = held.hold.release(slot, committed, now);
            size_window(&client, tell);
        }
        self.redraw = true;
    }

    /// Send the configure the throttle is still sitting on, once its interval
    /// has passed.
    ///
    /// **`crate::resizing::TELL_EVERY` is a rate, and a rate needs a trailing
    /// edge.** `Hold::dragged` is reached from `move_pane` and from
    /// `hold_resize`, and `settle_resize` reaches either only on a frame whose
    /// `pending_resize` carried a motion. So the offers a drag makes in the
    /// last interval before it stops moving were recorded in the pane's slot,
    /// drawn from the pane's slot, and never sent: the client sits at the size
    /// it was told up to a tenth of a second earlier while the pane is drawn
    /// where the pointer is, and the bridge between them is the whole of that
    /// gap — drag speed times the interval, which on an ordinary seam drag is
    /// tens of pixels of stretch or, under `Fill::Hold`, tens of pixels of
    /// uncovered background.
    ///
    /// **Pausing before releasing is what people do**, so this is the ordinary
    /// end of a drag rather than an edge case, and it is held until the pointer
    /// moves again or the button comes up. Before #123 the tiled path
    /// configured on every frame and so had no tail at all, which makes this
    /// exactly the symptom that was reported.
    ///
    /// Both paths, because neither had it. `settle_resize_hold` looked like the
    /// floating path's answer and is not: `Hold::settle` decides whether a hold
    /// is over and never sends anything, so a paused floating drag sat on its
    /// last offer in the same way. The only thing that ever sent unconditionally
    /// was the release.
    ///
    /// The slot is the rectangle, for the same reason `release_bridge` uses it:
    /// it is what `move_pane` and `hold_resize` wrote, so it is the offer the
    /// throttle swallowed. A hold whose slot it has already sent answers `None`
    /// and costs a comparison.
    pub(super) fn flush_resize(&mut self, now: Duration) {
        let held: Vec<(Window, crate::pane::PaneId)> = self
            .resize_hold
            .iter()
            .chain(self.resize_bridge.iter().flat_map(|bridge| &bridge.panes))
            .map(|held| (held.window.clone(), held.pane))
            .collect();
        for (window, pane) in held {
            // The pane has been given a different client since the gesture
            // started, or has lost the one it had. The same check
            // `release_bridge`, `settle_resize_bridge` and `settle_resize_hold`
            // all make, and this is the site that needs it most: it runs first
            // of the four on every frame, so without it the *new* client's slot
            // is configured onto the window this hold remembers — and
            // `committed` below is read off that stale window too, so the
            // throttle's bookkeeping is answered about one client with the
            // other one's size.
            //
            // Skipped rather than dropped, which is `release_bridge`'s choice
            // and right for the same reason: the two settle passes run
            // immediately after this one on the same frame and each drops what
            // it owns. A flush that dropped holds would be deciding a lifetime
            // question from the function whose whole job is the throttle's
            // trailing edge.
            if self.panes.get(pane).and_then(Pane::client) != Some(&window) {
                continue;
            }
            let Some(slot) = self.panes.get(pane).map(Pane::slot) else {
                continue;
            };
            let committed = window.geometry().size;
            let Some(hold) = self.held_hold_mut(pane) else {
                continue;
            };
            let Some(tell) = hold.dragged(slot, committed, now) else {
                continue;
            };
            size_window(&window, tell);
            self.redraw = true;
            if crate::resizing::trace::on() {
                crate::resizing::trace::line(
                    "flush",
                    format_args!(
                        "pane={} slot={},{} {}x{} committed={}x{} asked={},{} {}x{} told=1 \
                         held=1 refused={} unanswered={}",
                        pane.get(),
                        slot.loc.x,
                        slot.loc.y,
                        slot.size.w,
                        slot.size.h,
                        committed.w,
                        committed.h,
                        tell.loc.x,
                        tell.loc.y,
                        tell.size.w,
                        tell.size.h,
                        u8::from(
                            self.held_hold(pane)
                                .is_some_and(crate::resizing::Hold::refused)
                        ),
                        self.held_hold(pane)
                            .map_or(0, crate::resizing::Hold::unanswered),
                    ),
                );
            }
        }
    }

    /// Watch a live hold for the client's answer, and end it when one comes.
    ///
    /// Returns whether the window's rectangle changed, which it only does in
    /// the case that is the whole reason this is careful: the client answered
    /// with a size that is not the one it was asked for, or answered nothing at
    /// all. See `crate::resizing::Settle`.
    pub(super) fn settle_resize_hold(&mut self, now: Duration) -> bool {
        let Some(held) = self.resize_hold.as_ref() else {
            return false;
        };
        let (window, pane) = (held.window.clone(), held.pane);
        // The pane or its client has gone. A hold pointing at neither would
        // keep `holding_resize` true for a pane id that has been reused by
        // nothing, and there is no rectangle left to reconcile.
        if self.panes.get(pane).and_then(Pane::client) != Some(&window) {
            self.drop_resize_hold();
            return false;
        }
        let size = window.geometry().size;
        let Some(held) = self.resize_hold.as_mut() else {
            return false;
        };
        match held.hold.settle(size, now) {
            crate::resizing::Settle::Waiting => false,
            // The client is the size the pane is. Everything agrees again, so
            // there is nothing to hold and nothing to move.
            crate::resizing::Settle::Done => {
                self.drop_resize_hold();
                false
            }
            // **The client's answer wins.** It refused the size it was offered
            // — a minimum width, a cell grid — or it never answered at all, and
            // either way the alternative is a window drawn at a size its client
            // will never reach, stretched, for as long as it is open. One snap
            // at the end of a gesture is the cheaper of the two, and it is also
            // issue #115 becoming visible rather than staying hidden behind a
            // blur.
            crate::resizing::Settle::Adopt(taken) => {
                self.adopt_resize(taken);
                true
            }
        }
    }

    /// The same, once per pane a tiled gesture moved.
    ///
    /// Each pane settles on its own client's answer and on its own deadline,
    /// which is the whole reason there is a hold per pane rather than one for
    /// the gesture: a seam's two windows are two applications, and Firefox
    /// answering in 80 ms says nothing about the terminal beside it.
    ///
    /// **Every pane is visited, and a hold is never left behind for a pane or a
    /// client that has gone.** A hold pointing at neither would answer
    /// `holding_resize` for a pane id nothing owns, and `pane_geometry` would
    /// keep returning a slot forever.
    pub(super) fn settle_resize_bridge(&mut self, now: Duration) -> bool {
        let Some(bridge) = self.resize_bridge.as_ref() else {
            return false;
        };
        let watching: Vec<(Window, crate::pane::PaneId)> = bridge
            .panes
            .iter()
            .map(|held| (held.window.clone(), held.pane))
            .collect();
        let mut moved = false;
        for (window, pane) in watching {
            // The pane or its client has gone; there is no rectangle left to
            // reconcile and nothing to reconcile it against.
            if self.panes.get(pane).and_then(Pane::client) != Some(&window) {
                self.drop_bridged(pane);
                continue;
            }
            let committed = window.geometry().size;
            let Some(held) = self
                .resize_bridge
                .as_mut()
                .and_then(|bridge| bridge.panes.iter_mut().find(|held| held.pane == pane))
            else {
                continue;
            };
            match held.hold.settle(committed, now) {
                // Still being dragged, or the client has not answered the last
                // configure yet. The layout's rectangle stays authoritative.
                crate::resizing::Settle::Waiting => {}
                // The client is the size the layout made it. Slot, space and
                // client agree again, so there is nothing left to hold.
                crate::resizing::Settle::Done => self.drop_bridged(pane),
                // **Any answer ends the bridge, including a refusal**, which is
                // the trap `crate::resizing` names and the one that cost a
                // permanently blurred window once already. A client with a
                // minimum size — Firefox has one, a terminal rounds to its cell
                // grid — will never reach what the layout offered, so waiting
                // for it means stretching a buffer towards a size nothing will
                // ever agree to, for as long as the window is open.
                crate::resizing::Settle::Adopt(taken) => {
                    let hold = held.hold;
                    self.drop_bridged(pane);
                    self.land_on(&window, pane, &hold, taken);
                    moved = true;
                }
            }
        }
        // An empty bridge is no bridge: `settle_resize` reads this to know
        // whether a recorded release can still be handed to anything.
        if self
            .resize_bridge
            .as_ref()
            .is_some_and(|bridge| bridge.panes.is_empty())
        {
            self.resize_bridge = None;
        }
        moved
    }

    /// Offer a resize to scripts. Returns whether a layout took it.
    ///
    /// **`request.edge_at` is where the dragged edge should go, per axis.** Not
    /// a delta, and since #124 not the pointer either. This doc has been wrong
    /// about it twice: it called it "the delta" for the whole life of the event
    /// while `ResizeGrab::motion` recorded `event.location`, and #120 corrected
    /// that to "where the pointer is" — accurate about the code, and the code
    /// was the defect. A seam set from the pointer lands *under the cursor*, so
    /// a drag begun anywhere but exactly on the edge threw that edge to the
    /// cursor on its first frame.
    ///
    /// It is still a position rather than a delta, for the reason
    /// `ResizeRequest` gives: a seam set from a position is idempotent, and one
    /// accumulated from deltas feeds the layout's own response back in as its
    /// next input. It is nonetheless relative to the grab, because
    /// `crate::input::resize::dragged_edge` builds it from the pane's own
    /// laid-out edge and the drag's total movement rather than from the seat.
    ///
    /// It is in the layout's **outer** space — the space `sol.place` writes and
    /// `tree:layout` returns — because it comes from `Solium::pane_laid_out`,
    /// which is the rectangle `sol.place` was last handed for this pane. Not
    /// from `Solium::pane_outer`, which is that rectangle only until the client
    /// commits a size of its own.
    ///
    /// The pair after it is the side of the window being dragged on each axis
    /// — `"left"` or `"right"`, `"top"` or `"bottom"`, or nil for an axis that
    /// is not in play. Sides and not an axis pair, because which seam a tiled
    /// drag moves depends on which edge the hand is on; see
    /// [`crate::input::resize::sides`].
    pub(crate) fn trigger_resize(&mut self, request: &ResizeRequest) -> bool {
        let id = self.window_id(&request.window);
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return false;
        };
        let outcome = scripts.resized(
            id,
            request.edge_at,
            crate::input::resize::sides(request.edges),
            snapshot,
        );
        self.scripts = Some(scripts);
        let handled = outcome.handled && !outcome.commands.is_empty();
        self.apply(outcome);
        handled
    }
}
