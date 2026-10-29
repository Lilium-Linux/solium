//! The compositor's side of a window frame that does more than draw (#417):
//! what its layers send, the grab one of them holds while it is expanded over
//! the window, and the presses and keys that grab takes.

use smithay::utils::{Logical, Point};

use crate::{
    decoration::Action,
    json::Json,
    pane::{Pane, PaneId},
    qml::hosted::ScenePointer,
    state::Solium,
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
}
