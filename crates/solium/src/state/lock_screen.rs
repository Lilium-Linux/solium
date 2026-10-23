//! The keyboard while the session is locked: pointing it at the lock surface on the monitor the
//! user is working on, or at any lock surface, and `settle_focus`'s answer while locked. And the
//! native lock's scenes, one per monitor, the monitor its prompt is on, and the pointer at them.

use super::*;

impl Solium {
    /// Point the keyboard at the lock screen.
    ///
    /// The surface on the monitor the pointer is on, so that on a two-monitor
    /// desk the password goes into the field the user is looking at; any
    /// surface otherwise, because typing into the wrong screen's lock dialog
    /// still beats typing into nothing.
    ///
    /// The lock client is given the selection like any other focused client.
    /// Withholding it would look like caution and buy none: any client that
    /// can take focus can already read the clipboard, so the only thing the
    /// restriction would achieve is breaking paste from a password manager.
    ///
    /// Only surfaces that are still alive are candidates (see `Lock::surfaces`).
    /// With none left the keyboard is left where it is; `settle_lock_focus`
    /// is the caller that then takes it off a dead one.
    pub(crate) fn focus_lock(&mut self) {
        let Some(lock) = self.lock.as_ref() else {
            return;
        };
        let here = self
            .active_output()
            .and_then(|output| lock.surface_for(&output))
            .map(|surface| surface.wl_surface().clone());
        let Some(surface) = here.or_else(|| {
            lock.surfaces()
                .next()
                .map(|surface| surface.wl_surface().clone())
        }) else {
            return;
        };
        // Through the gate like everything else, and the one call it must
        // always let through: this surface is the lock's own.
        self.give_keyboard(Some(surface), SERIAL_COUNTER.next_serial());
    }

    /// `settle_focus`, while locked.
    ///
    /// Left alone if the keyboard is on a live lock surface, because the user
    /// may be typing into the one on the other monitor and a window opening
    /// behind the lock is no reason to move them. Otherwise to the lock screen
    /// (`focus_lock`), and to nothing if there is no lock surface left at all
    /// -- a lock client that has died leaves the keyboard on a surface that no
    /// longer exists, and "nothing" is the honest name for that.
    pub(super) fn settle_lock_focus(&mut self) {
        let Some(lock) = self.lock.as_ref() else {
            return;
        };
        let focus = self
            .seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus());
        if focus
            .as_ref()
            .is_some_and(|focus| lock.surfaces().any(|each| each.wl_surface() == focus))
        {
            return;
        }
        if lock.surfaces().next().is_some() {
            self.focus_lock();
        } else if focus.is_some() {
            self.give_keyboard(None, SERIAL_COUNTER.next_serial());
        }
    }

    /// Give every monitor its native lock scene: live while Solium holds the
    /// lock and through its exit, and kept quiet for the next lock otherwise,
    /// while `lock.native` is on. Asked when the lock is taken or ends, when
    /// the exit ends, when the settings change, and when the monitors change,
    /// so one plugged in gets its scene ahead of the next lock and one
    /// plugged in while locked gets its scene or the fallback, never the
    /// desktop. See `lock_scene.rs`.
    /// `a_monitor_plugged_in_while_natively_locked_gets_a_scene_never_the_desktop`,
    /// `lock_scene::tests::the_scenes_are_built_ahead_kept_between_locks_and_follow_the_monitors`.
    pub(crate) fn sync_lock_scenes(&mut self) {
        let native = self.lock.as_ref().is_some_and(crate::lock::Lock::is_native);
        let exiting = self.lock.is_none() && self.lock_scenes.exiting(self.clock.now());
        let outputs: Vec<Output> = self.space.outputs().cloned().collect();
        self.lock_scenes.sync(&outputs, native || exiting);
    }

    /// The monitor `Lock.promptOn` names, as `lock.prompt_on` says: the
    /// primary one, the one the pointer is on, or one by name while it is
    /// plugged in and the primary one while it is not.
    /// `lock_auth::tests::the_prompt_is_on_the_monitor_prompt_on_names_and_no_other`.
    pub(crate) fn lock_prompt_on(&self) -> String {
        let output = match &self.lock_scenes.settings().prompt_on {
            crate::lock_scene::PromptOn::Primary => self.primary_output(),
            crate::lock_scene::PromptOn::Pointer => self.active_output(),
            crate::lock_scene::PromptOn::Monitor(name) => self
                .space
                .outputs()
                .find(|output| output.name() == *name)
                .cloned()
                .or_else(|| self.primary_output()),
        };
        output.map(|output| output.name()).unwrap_or_default()
    }

    /// The monitor under `location`, by name, and where it is drawn.
    fn lock_monitor_at(
        &self,
        location: Point<f64, Logical>,
    ) -> Option<(String, Rectangle<i32, Logical>)> {
        let output = crate::monitor::at(&self.space, location)?;
        let area = self.space.output_geometry(&output)?;
        Some((output.name(), area))
    }

    /// The pointer moved to `location` while the session is locked. Solium's
    /// own lock tells the scene under it, or the scene a press went to, as Qt
    /// does; over a keypad, or within `lock.keypad_margin` of one, the scene
    /// is told the pointer left instead and nothing more, so neither where it
    /// hovers nor its way to a key is the scene's to know. Nothing, for a
    /// lock a client holds: smithay gives the pointer to its surfaces.
    /// `lock_auth::tests::a_press_at_the_native_lock_reaches_its_scene_and_nothing_else`,
    /// `lock_auth::tests::a_keypad_key_types_into_the_secret_and_its_scene_never_hears_which`,
    /// `lock_auth::tests::the_pointer_near_the_keypad_is_kept_from_the_scene`.
    pub(crate) fn lock_scene_motion(&mut self, location: Point<f64, Logical>) {
        if !self.lock.as_ref().is_some_and(crate::lock::Lock::is_native) {
            return;
        }
        let event = crate::input::scene_event(self, crate::qml::hosted::PointerKind::Motion);
        if let Some((monitor, area)) = self.lock_scenes.pointer.held.clone() {
            // A press the scene holds goes on hearing the pointer, but not
            // across the pad.
            if !self.lock_scenes.near_keypad(&monitor, area, location) {
                self.lock_scenes
                    .pointer_event(&monitor, area, location, &event);
                self.redraw = true;
            }
            return;
        }
        if self.lock_scenes.pointer.on_pad {
            return;
        }
        let under = self.lock_monitor_at(location);
        let left = self
            .lock_scenes
            .pointer
            .on
            .clone()
            .filter(|was| under.as_ref().is_none_or(|(now, _)| now != was));
        if let Some(was) = left {
            self.lock_scenes.leave(&was);
            self.lock_scenes.pointer.hidden = false;
        }
        let Some((monitor, area)) = under else {
            self.lock_scenes.pointer.on = None;
            return;
        };
        self.lock_scenes.pointer.on = Some(monitor.clone());
        if self.lock_scenes.near_keypad(&monitor, area, location) {
            if !self.lock_scenes.pointer.hidden {
                self.lock_scenes.leave(&monitor);
                self.lock_scenes.pointer.hidden = true;
            }
            return;
        }
        self.lock_scenes.pointer.hidden = false;
        self.lock_scenes
            .pointer_event(&monitor, area, location, &event);
        self.redraw = true;
    }

    /// A button pressed or let go at `location` while the session is
    /// locked, as `scene` tells it, or `None` for a button Qt has no name
    /// for, which no scene is told. Solium's own lock gives a press on a
    /// keypad key to the password and its release to nobody; any other press
    /// goes to the scene under the pointer, and its release to the same
    /// scene. Nothing, for a lock a client holds.
    /// `lock_auth::tests::a_press_at_the_native_lock_reaches_its_scene_and_nothing_else`,
    /// `lock_auth::tests::a_keypad_key_types_into_the_secret_and_its_scene_never_hears_which`.
    pub(crate) fn lock_scene_button(
        &mut self,
        location: Point<f64, Logical>,
        scene: Option<crate::qml::hosted::ScenePointer>,
    ) {
        if !self.lock.as_ref().is_some_and(crate::lock::Lock::is_native) {
            return;
        }
        let Some(event) = scene else {
            return;
        };
        let pressed = matches!(event.kind, crate::qml::hosted::PointerKind::Press(_));
        let all_up = self.pointer_buttons == 0;
        if self.lock_scenes.pointer.on_pad {
            if !pressed && all_up {
                self.lock_scenes.pointer.on_pad = false;
                self.keypad_up();
            }
            return;
        }
        if let Some((monitor, area)) = self.lock_scenes.pointer.held.clone() {
            self.lock_scenes
                .pointer_event(&monitor, area, location, &event);
            if all_up {
                self.lock_scenes.pointer.held = None;
            }
            self.redraw = true;
            return;
        }
        if !pressed {
            return;
        }
        let Some((monitor, area)) = self.lock_monitor_at(location) else {
            return;
        };
        if let Some(key) = self.lock_scenes.keypad_at(&monitor, area, location) {
            self.lock_scenes.pointer.on_pad = true;
            self.keypad_down(key);
            return;
        }
        self.lock_scenes
            .pointer_event(&monitor, area, location, &event);
        self.lock_scenes.pointer.held = Some((monitor, area));
        self.redraw = true;
    }
}
