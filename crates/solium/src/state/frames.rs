//! The compositor's side of a window frame that does more than draw (#417):
//! what its layers send, the grab one of them holds while it is expanded over
//! the window, and the presses and keys that grab takes.

use std::time::Duration;

use smithay::{
    desktop::Window,
    utils::{Logical, Point, Rectangle},
};

use crate::{
    decoration::Action,
    json::Json,
    pane::{Pane, PaneId},
    qml::hosted::ScenePointer,
    state::{Solium, inner, placement::over_the_arrangement},
};

impl Solium {
    /// Read what every frame's layers sent and say of their grabs, after
    /// anything that can run their QML ([`Self::settle_scenes`]).
    ///
    /// A layer's `Solium.send("close" | "maximize" | "minimize")` is the
    /// built-in action, as its `action` property is; any other name goes to
    /// Lua as `sol.on("frame", id, name, data)`, in the order the layers sent
    /// them.
    /// `tests::real_client::reflow_on_close::frames::a_frame_sends_a_named_action_with_its_data_to_lua`.
    ///
    /// One frame holds a grab at a time, as one shell scene does: the one
    /// that began to last, and a frame that begins one dismisses the grab
    /// another held. The holder is let go of when its grab ends, when its
    /// window is minimized, goes, or leaves the keyboard, and at the lock.
    /// When the holder changes, the pointer is given to what is under it now,
    /// so the window under a panel that has just opened loses it at once.
    /// `tests::real_client::reflow_on_close::frames::a_frame_holding_a_grab_takes_the_press_over_the_client_and_is_dismissed_outside`.
    pub(super) fn settle_frames(&mut self) {
        let mut sent = Vec::new();
        let mut began = None;
        let mut holding = Vec::new();
        for pane in self.panes.iter_mut() {
            let id = pane.id();
            let Some(decoration) = pane.decoration_mut() else {
                continue;
            };
            for (name, data) in decoration.take_sent() {
                sent.push((id, name, data));
            }
            let (holds, newly) = decoration.take_grabs();
            if newly {
                began = Some(id);
            }
            if holds {
                holding.push(id);
            }
        }
        let before = self.decorations.holding;
        let wanted = began.or(before.filter(|id| holding.contains(id)));
        let next = wanted.filter(|id| self.may_hold_frame_grab(*id));
        // Told once that it is over: a grab that cannot be held now, and the
        // one a newer grab replaced. One that goes on holding after that is
        // not the frame's grab any more, and is not told again.
        let mut told = None;
        for id in [wanted, before].into_iter().flatten() {
            if Some(id) != next && told != Some(id) && holding.contains(&id) {
                self.dismiss_frame_grab(id);
                told = Some(id);
            }
        }
        if next != before {
            self.decorations.holding = next;
            self.redraw = true;
            self.repoint_clients();
        }
        for (pane, name, data) in sent {
            match Action::named(&name) {
                Some(action) => self.frame_action(pane, action),
                None => self.trigger_frame(pane, &name, &data),
            }
        }
    }

    /// Whether a frame may hold its grab: its window is there, shown, and
    /// has the keyboard, and the session is not locked.
    fn may_hold_frame_grab(&self, id: PaneId) -> bool {
        if self.lock.is_some() {
            return false;
        }
        let Some(pane) = self.panes.get(id) else {
            return false;
        };
        !pane.minimized()
            && !pane.leaving()
            && pane.client().is_some_and(|window| self.is_focused(window))
    }

    /// Tell a frame its grab is over: each grab's `onDismissed` runs.
    pub(crate) fn dismiss_frame_grab(&mut self, id: PaneId) {
        if let Some(decoration) = self.panes.get_mut(id).and_then(Pane::decoration_mut) {
            decoration.dismiss_grabs();
            self.redraw = true;
        }
        if self.decorations.holding == Some(id) {
            self.decorations.holding = None;
            self.repoint_clients();
        }
        // What it does when it is dismissed is read in this settle's next
        // round, or the next settle's.
        self.settle_scenes_once_dispatched();
    }

    /// Tell the scripts a frame sent something: `sol.on("frame", id, name,
    /// data)`.
    fn trigger_frame(&mut self, pane: PaneId, name: &str, data: &Json) {
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return;
        };
        let outcome = scripts.frame_action(pane.get(), name, data, snapshot);
        self.scripts = Some(scripts);
        self.apply(outcome);
    }

    /// One pointer event for a frame, at `location`, in the pane's own
    /// space: what its layers hear, then the action a button there asked
    /// for, acted on. Whether some layer says the pointer is on a button.
    pub(crate) fn frame_pointer(
        &mut self,
        pane: PaneId,
        location: Point<f64, Logical>,
        event: &ScenePointer,
    ) -> bool {
        let Some(local) = self.pane_local(pane, location) else {
            return false;
        };
        let Some(decoration) = self.panes.get_mut(pane).and_then(Pane::decoration_mut) else {
            return false;
        };
        decoration.pointer_event(local.x, local.y, event);
        let on_button = decoration.on_button();
        if let Some(action) = decoration.take_action() {
            self.frame_action(pane, action);
        }
        on_button
    }

    /// Whether `location` is inside the target of the grab a frame holds,
    /// where that frame is the one the pointer is on there: a window above it
    /// covers its panel as it covers the rest of it.
    pub(crate) fn in_frame_grab(&self, location: Point<f64, Logical>) -> bool {
        let Some(holder) = self.decorations.holding else {
            return false;
        };
        self.chrome_under(location).is_some_and(|under| {
            under.pane == holder
                && matches!(under.chrome, crate::state::Chrome::Frame)
                && self.frame_grab_at(holder, under.local)
        })
    }

    /// Whether a point in a pane's own space is inside the target of the
    /// grab its frame holds.
    pub(crate) fn frame_grab_at(&self, pane: PaneId, local: Point<f64, Logical>) -> bool {
        self.decorations.holding == Some(pane)
            && self
                .panes
                .get(pane)
                .and_then(Pane::decoration)
                .is_some_and(|decoration| decoration.grab_contains(local.x, local.y))
    }

    /// Follow every frame's `growTop` (#423), once a frame, after the QML
    /// clock has moved and before anything is laid out or drawn
    /// (`render::frame_tick`), so a titlebar growing by a `Behavior` moves
    /// its window's content on the frame it draws itself taller.
    ///
    /// The frame reserves `insets.top` and the largest `growTop` of its
    /// layers above the client, and its window's top edge stays where it is:
    ///
    /// * **A floating window** keeps its client's size. Its pane grows
    ///   taller and the client moves down, on the frame the value moved.
    ///   `tests::real_client::reflow_on_close::frames::a_floating_windows_frame_grows_and_its_client_moves_down_at_its_size`.
    /// * **A window held in a size**, in a layout's tile or maximised, keeps
    ///   its pane, and its client is given that much less height. Not on
    ///   every frame of an animation: while the value moves the client's
    ///   last picture is held in the smaller rectangle, as `resize.fill`
    ///   says, and the client is configured once, on the first frame the
    ///   value has stood still.
    ///   `tests::real_client::reflow_on_close::frames::a_held_windows_client_is_configured_smaller_once_its_frame_has_grown`.
    ///
    /// A fullscreen window has no frame, and a window leaving keeps the
    /// rectangle it left at, so neither is followed.
    pub(crate) fn follow_growth(&mut self, now: Duration) {
        let mut moved = Vec::new();
        let mut settled = Vec::new();
        for pane in self.panes.iter_mut() {
            if pane.leaving() || pane.gone() {
                continue;
            }
            let id = pane.id();
            let Some(decoration) = pane.decoration_mut() else {
                continue;
            };
            let wanted = decoration.wanted_grow();
            if wanted != decoration.grow() {
                moved.push((id, wanted));
            } else if decoration.take_grow_settled() {
                settled.push(id);
            }
        }
        for (id, wanted) in moved {
            self.grow_frame(id, wanted, now);
        }
        for id in settled {
            self.configure_grown(id, now);
        }
    }

    /// Whether a pane is held in a size: in a layout's tile, or maximised.
    fn held_in_a_size(pane: &Pane) -> bool {
        pane.placed().is_some() || pane.client().is_some_and(over_the_arrangement)
    }

    /// Make one pane's frame reserve `wanted` more at the top, its window's
    /// top edge where it was. See [`Self::follow_growth`].
    fn grow_frame(&mut self, id: PaneId, wanted: i32, now: Duration) {
        let Some(pane) = self.panes.get(id) else {
            return;
        };
        // Before the grow moves: where the window's frame is now, and so
        // where it stays.
        let outer = self.pane_outer(pane);
        let geometry = self.pane_geometry(pane);
        let slot = pane.slot();
        let held = Self::held_in_a_size(pane);
        let window = pane.client().cloned();
        let was = pane
            .decoration()
            .map_or(0, crate::decoration::Decoration::grow);
        if let Some(decoration) = self.panes.get_mut(id).and_then(Pane::decoration_mut) {
            decoration.set_grow(wanted);
        }
        self.redraw = true;
        let Some(window) = window else {
            // Nothing to move but the slot a scene stands in for its
            // application, which is the client's rectangle as anything's is.
            if let Some(pane) = self.panes.get_mut(id) {
                pane.set_slot(Rectangle::new(
                    (slot.loc.x, slot.loc.y + wanted - was).into(),
                    slot.size,
                ));
            }
            return;
        };
        if held {
            // The pane stays; the client is what is left of it under the
            // taller frame, held there until it is told.
            let client = inner(outer, self.insets_of(id));
            if let Some(pane) = self.panes.get_mut(id) {
                pane.set_slot(client);
            }
            self.relocate(&window, client.loc);
            // Held again at every step, so the hold's patience runs from the
            // last one: never out while the frame is still moving, and out
            // `resize.patience` after it stops being followed at all -- a
            // style switched to none part of the way -- where the client's
            // own size wins, as at the end of any change.
            self.hold_for_answer(id, &window, now, now);
        } else {
            // The client keeps its size and moves down by what the frame
            // took. The slot too, which a drag on the window's edge holds.
            let by = wanted - was;
            let to: Point<i32, Logical> = (geometry.loc.x, geometry.loc.y + by).into();
            if let Some(pane) = self.panes.get_mut(id) {
                pane.set_slot(Rectangle::new(
                    (slot.loc.x, slot.loc.y + by).into(),
                    slot.size,
                ));
            }
            self.relocate(&window, to);
            // A glide it is on was aimed at the rectangle it had (#438).
            if let Some(pane) = self.panes.get(id) {
                crate::present::reaim(pane, self.pane_outer(pane), now);
            }
            // An X11 client is told where it is; a Wayland one never is.
            if let Some(x11) = window.x11_surface()
                && !x11.is_override_redirect()
                && let Err(err) = x11.configure(Some(Rectangle::new(to, window.geometry().size)))
            {
                tracing::warn!(?err, "could not move an X11 window under its frame");
            }
        }
    }

    /// A window held in a size whose frame has stopped growing: its client
    /// is configured, once, to what the frame leaves it, and held there
    /// until it answers, or `resize.patience` runs out.
    fn configure_grown(&mut self, id: PaneId, now: Duration) {
        let Some(pane) = self.panes.get(id).filter(|pane| Self::held_in_a_size(pane)) else {
            return;
        };
        let Some(window) = pane.client().cloned() else {
            return;
        };
        let client = pane.slot();
        crate::state::size_window(&window, client);
        self.hold_for_answer(id, &window, now, now);
        self.redraw = true;
    }

    /// Move a window in the space without changing where it is in the
    /// stack: what a frame growing over it does, which is not a raise.
    fn relocate(&mut self, window: &Window, to: Point<i32, Logical>) {
        let above: Vec<Window> = self
            .space
            .elements()
            .skip_while(|element| *element != window)
            .skip(1)
            .cloned()
            .collect();
        self.space.map_element(window.clone(), to, false);
        for element in &above {
            self.space.raise_element(element, false);
        }
    }
}
