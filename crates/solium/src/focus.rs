//! Who holds the keyboard.
//!
//! One rule, and this file is where it is kept:
//!
//! > **While the session is locked, only a lock surface may hold the keyboard.**
//!
//! The first thing typed at a lock screen is a password (see `lock.rs`). A
//! keyboard that has gone to an application instead delivers the password to
//! that application, and in a terminal the Enter after it runs it.
//!
//! ## Why a gate rather than a guard at each caller
//!
//! Guards at each caller are what there were, and they held for as long as
//! every caller remembered. Four did not. A window opening behind the lock
//! took the keyboard, because `new_toplevel` set focus itself. A window
//! closing handed it to the topmost window, because `settle_focus` read the
//! lock surface as "nothing is focused". A menu grab took it, because `grab`
//! set focus itself. And `focus_window`, which scripts, layouts' event
//! handlers and xdg-activation all still reach while locked, never asked.
//! Nothing moved the keyboard back, because `focus_lock` runs only when a lock
//! surface maps.
//!
//! So every call that gives anyone the keyboard, a keyboard grab or the
//! selection lives in this file, and each one asks
//! [`Solium::may_hold_keyboard`] first. `clippy.toml` bans smithay's own
//! versions of those calls everywhere else, so a new route that has never
//! heard of the rule fails the gate instead of shipping.
//!
//! ## What smithay does without asking
//!
//! A popup grab moves the keyboard on its own. `PopupKeyboardGrab` ignores
//! every `set_focus` until its menu closes, so locking could not take the
//! keyboard *off* an open menu. It re-aims focus at the menu on every key. And
//! `PopupPointerGrab::unset` hands the keyboard back to the menu's window. We
//! cannot route any of that through a function of ours, so it is closed from
//! three sides instead:
//!
//! * No popup grab may be installed while locked ([`Solium::grab_keyboard`]
//!   asks the rule, and `grab` asks before it records anything).
//! * Every grab is released the moment the session locks
//!   ([`Solium::release_grabs`]).
//! * At delivery, a key is dropped unless the keyboard is on something the
//!   rule allows and no grab is steering it ([`Solium::keys_may_pass`]).
//!
//! The last of those still holds if everything above it is someday wrong.

use smithay::{
    desktop::{
        LayerSurface, PopupUngrabStrategy, Window, WindowSurfaceType, find_popup_root_surface,
        layer_map_for_output,
    },
    input::keyboard::KeyboardGrab,
    reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface},
    utils::{SERIAL_COUNTER, Serial},
    wayland::{
        seat::WaylandFocus,
        selection::{data_device::set_data_device_focus, primary_selection::set_primary_focus},
        shell::wlr_layer::KeyboardInteractivity,
    },
};

use crate::state::Solium;

/// An exclusive layer's temporary ownership, across replacements by other layers.
#[derive(Debug)]
pub(crate) struct LayerKeyboard {
    surface: WlSurface,
    returns_to: Option<WlSurface>,
}

/// Actual keyboard focus and its owning layer, even after a popup disappears.
#[derive(Debug)]
pub(crate) struct LayerKeyboardFocus {
    focused: WlSurface,
    root: WlSurface,
}

impl Solium {
    fn keyboard_layer(&self, surface: &WlSurface) -> Option<LayerSurface> {
        self.space.outputs().find_map(|output| {
            layer_map_for_output(output)
                .layer_for_surface(surface, WindowSurfaceType::ALL)
                .cloned()
        })
    }

    fn mapped_keyboard_layer(layer: &LayerSurface) -> bool {
        let surface = layer.layer_surface().wl_surface();
        surface.is_alive()
            && smithay::backend::renderer::utils::with_renderer_surface_state(surface, |state| {
                state.buffer().is_some()
            })
            .unwrap_or(false)
    }

    /// Only committed, buffered Top/Overlay layers claim the seat automatically.
    pub(crate) fn exclusive_keyboard_layer(&self) -> Option<WlSurface> {
        if self.lock.is_some() || self.refocus_on_release {
            return None;
        }
        let focused = self
            .seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus());
        let owning_output = focused.as_ref().and_then(|surface| {
            self.space
                .outputs()
                .find(|output| {
                    layer_map_for_output(output)
                        .layer_for_surface(surface, WindowSurfaceType::ALL)
                        .is_some()
                })
                .cloned()
        });
        let active = self.active_output();
        for band in [crate::scripted::Layer::Overlay, crate::scripted::Layer::Top] {
            let candidates: Vec<_> = self
                .space
                .outputs()
                .filter_map(|output| {
                    let map = layer_map_for_output(output);
                    crate::layer::on(&map, band)
                        .find(|layer| {
                            Self::mapped_keyboard_layer(layer)
                                && layer.cached_state().keyboard_interactivity
                                    == KeyboardInteractivity::Exclusive
                        })
                        .map(|layer| (output.clone(), layer.layer_surface().wl_surface().clone()))
                })
                .collect();
            // A monitor tie keeps the current winner; a newly mapped surface
            // above it on that monitor still wins the same ordering as drawing.
            if let Some((_, surface)) = candidates
                .iter()
                .find(|(output, _)| Some(output) == owning_output.as_ref())
            {
                return Some(surface.clone());
            }
            if let Some((_, surface)) = candidates
                .iter()
                .find(|(output, _)| Some(output) == active.as_ref())
                .or_else(|| candidates.first())
            {
                return Some(surface.clone());
            }
        }
        None
    }

    pub(crate) fn exclusive_layer_has_keyboard(&self) -> bool {
        let Some(winner) = self.exclusive_keyboard_layer() else {
            return false;
        };
        self.seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus())
            .and_then(|surface| self.keyboard_layer(&surface))
            .is_some_and(|layer| layer.layer_surface().wl_surface() == &winner)
    }

    /// Remember actual layer focus while its role and output still exist.
    /// Smithay also changes focus through popup grabs, so observe seat callbacks.
    pub(crate) fn remember_layer_focus(&mut self, focused: Option<&WlSurface>) {
        let Some(focused) = focused else {
            self.layer_keyboard_focus = None;
            return;
        };
        let layer = self.keyboard_layer(focused).or_else(|| {
            // A grab can focus a popup before its first commit puts it in
            // the mapped tree. Resolve its protocol parent in that interval.
            let popup = self.popups.find_popup(focused)?;
            let root = find_popup_root_surface(&popup).ok()?;
            self.keyboard_layer(&root)
        });
        if let Some(layer) = layer {
            self.layer_keyboard_focus = Some(LayerKeyboardFocus {
                focused: focused.clone(),
                root: layer.layer_surface().wl_surface().clone(),
            });
        } else if self
            .layer_keyboard_focus
            .as_ref()
            .is_none_or(|held| &held.focused != focused)
        {
            self.layer_keyboard_focus = None;
        }
        // The same focus with a missing role/tree keeps its root until
        // settlement can dismiss that owner's grab and send keyboard leave.
    }

    /// Reconcile layer ownership before settling ordinary window focus.
    pub(crate) fn settle_layer_keyboard(&mut self) -> bool {
        if self.lock.is_some() || self.refocus_on_release {
            return false;
        }
        if let Some(surface) = self.exclusive_keyboard_layer() {
            if self
                .layer_keyboard
                .as_ref()
                .is_none_or(|held| held.surface != surface)
            {
                let returns_to = self
                    .layer_keyboard
                    .take()
                    .map(|held| held.returns_to)
                    .unwrap_or_else(|| {
                        self.focused_window()
                            .and_then(|window| window.wl_surface().map(|root| root.into_owned()))
                            .or_else(|| {
                                self.hosted_keyboard
                                    .as_ref()
                                    .and_then(|held| held.returns_to.clone())
                            })
                            .or_else(|| {
                                self.seat
                                    .get_keyboard()
                                    .and_then(|keyboard| keyboard.current_focus())
                            })
                    });
                // A dismissed popup cannot be a restoration target. Preserve
                // the layer root while it is still in its output's tree.
                let returns_to = returns_to.map(|surface| {
                    self.keyboard_layer(&surface)
                        .map(|layer| layer.layer_surface().wl_surface().clone())
                        .unwrap_or(surface)
                });
                self.end_keyboard_hold(false);
                self.dismiss_keyboard_popups(None);
                self.layer_keyboard = Some(LayerKeyboard {
                    surface: surface.clone(),
                    returns_to,
                });
            }
            if !self.exclusive_layer_has_keyboard() {
                self.give_keyboard(Some(surface), SERIAL_COUNTER.next_serial());
            }
            return true;
        }
        if let Some(held) = self.layer_keyboard.take() {
            self.dismiss_keyboard_popups(Some(&held.surface));
            self.give_keyboard(None, SERIAL_COUNTER.next_serial());
            if let Some(surface) = held
                .returns_to
                .filter(|surface| surface.is_alive() && self.may_hold_keyboard(surface))
            {
                if let Some(window) = self.restorable_keyboard_window(&surface) {
                    self.focus_window(
                        &window,
                        SERIAL_COUNTER.next_serial(),
                        crate::script::FocusSource::Settle,
                    );
                    return true;
                } else if self.keyboard_layer(&surface).is_some() {
                    self.give_keyboard(Some(surface), SERIAL_COUNTER.next_serial());
                    return true;
                }
            }
        }
        if let Some(surface) = self
            .seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus())
        {
            let root = self
                .keyboard_layer(&surface)
                .map(|layer| layer.layer_surface().wl_surface().clone())
                .or_else(|| {
                    self.layer_keyboard_focus
                        .as_ref()
                        .filter(|held| held.focused == surface)
                        .map(|held| held.root.clone())
                });
            if let Some(root) = root {
                if self.keyboard_layer(&root).is_some() && self.may_hold_keyboard(&root) {
                    return true;
                }
                self.dismiss_keyboard_popups(Some(&root));
                self.give_keyboard(None, SERIAL_COUNTER.next_serial());
            }
        }
        false
    }

    /// Dismiss a keyboard popup chain before changing focus. `owner` limits
    /// withdrawal cleanup to that chain's owning layer root; exclusive
    /// preemption passes None because it must release any keyboard grab.
    fn dismiss_keyboard_popups(&mut self, owner: Option<&WlSurface>) {
        if owner.is_some_and(|owner| {
            self.popup_grab
                .as_ref()
                .is_none_or(|grab| grab.keyboard_grab_start_data().focus.as_ref() != Some(owner))
        }) {
            return;
        }
        let serials = self.popup_grab.as_ref().map(|grab| {
            (
                grab.serial(),
                grab.previous_serial().unwrap_or_else(|| grab.serial()),
            )
        });
        if let Some(mut grab) = self.popup_grab.take() {
            grab.ungrab(PopupUngrabStrategy::All);
        }
        // Keyboard first: unsetting a matching popup pointer grab can
        // otherwise give the keyboard back to the withdrawn root.
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.unset_grab(self);
        }
        if let Some(pointer) = self.seat.get_pointer()
            && serials.is_some_and(|(serial, previous)| {
                pointer.has_grab(serial) || pointer.has_grab(previous)
            })
        {
            let time = u32::try_from(self.clock.now().as_millis()).unwrap_or(u32::MAX);
            pointer.unset_grab(self, SERIAL_COUNTER.next_serial(), time);
        }
    }

    /// The actual hit surface determines whether a click can focus a layer.
    pub(crate) fn focus_layer_on_click(&mut self, surface: &WlSurface, serial: Serial) -> bool {
        let Some(layer) = self.keyboard_layer(surface) else {
            return false;
        };
        if layer.cached_state().keyboard_interactivity == KeyboardInteractivity::None {
            return false;
        }
        let root = layer.layer_surface().wl_surface();
        if Self::mapped_keyboard_layer(&layer) && self.may_hold_keyboard(root) {
            self.end_keyboard_hold(false);
            self.give_keyboard(Some(root.clone()), serial);
        }
        true
    }

    /// The rule. Everything else in this file is a way of asking it.
    ///
    /// Unlocked, layer interactivity and exclusive ownership decide who may
    /// hold the keyboard. Locked, only a surface of the lock client's own may.
    /// A window, a popup, a layer surface or an X11
    /// window never qualifies, because each of them belongs to the session the
    /// lock is there to seal.
    ///
    /// "The lock client's own" is `Lock::surfaces`, and this is only as sound
    /// as what can get into that list. For a while, anything could: any client
    /// could take the lock over with a lock of its own and fill the list with
    /// its surfaces, which this then waved through. What keeps the list to the
    /// real lock screen is `lock.rs`, where the lock has one holder and only
    /// the holder's surfaces are ever added.
    pub(crate) fn may_hold_keyboard(&self, surface: &WlSurface) -> bool {
        match self.lock.as_ref() {
            None => {
                let layer = self.keyboard_layer(surface);
                if let Some(layer) = layer.as_ref()
                    && (self.refocus_on_release
                        || !Self::mapped_keyboard_layer(layer)
                        || layer.cached_state().keyboard_interactivity
                            == KeyboardInteractivity::None)
                {
                    return false;
                }
                self.exclusive_keyboard_layer().is_none_or(|winner| {
                    layer.is_some_and(|layer| layer.layer_surface().wl_surface() == &winner)
                })
            }
            Some(lock) => lock.surfaces().any(|each| each.wl_surface() == surface),
        }
    }

    /// The rule, asked of a window.
    ///
    /// A window with no surface yet (an X11 window between being mapped and
    /// being associated) has nothing to give the keyboard to. *Focusing* it
    /// still takes the keyboard off whatever had it, though, and while locked
    /// that is the lock screen: a background client able to do that can stop
    /// the password being typed at all. So it is refused while locked too.
    pub(crate) fn may_focus(&self, window: &Window) -> bool {
        match window.wl_surface() {
            Some(surface) => self.may_hold_keyboard(&surface),
            None => self.lock.is_none() && self.exclusive_keyboard_layer().is_none(),
        }
    }

    /// Point the keyboard at `surface`, or at nothing. **This is the only
    /// place in Solium that moves keyboard focus.**
    ///
    /// Refused, with nothing changed, for a surface the rule does not allow.
    /// Clearing focus is allowed unless an exclusive layer owns the seat.
    /// The lock and post-unlock held-key barrier outrank that ownership.
    /// Returns whether the focus was set.
    ///
    /// The selection moves with the keyboard, because the two are halves of
    /// one thing. A client can read the clipboard only while it holds the data
    /// device's focus, which is separate from keyboard focus, and if the two
    /// part company every paste hangs: the client asks for the selection and
    /// no offer ever arrives. Moving them together also puts the clipboard
    /// under the rule with no extra code, since an application behind the lock
    /// never becomes the selection's client and is never sent what the lock
    /// client copies.
    #[expect(
        clippy::disallowed_methods,
        reason = "the gate itself: `clippy.toml` sends every other caller here"
    )]
    pub(crate) fn give_keyboard(&mut self, surface: Option<WlSurface>, serial: Serial) -> bool {
        if surface.is_none() && self.exclusive_keyboard_layer().is_some() {
            return false;
        }
        if let Some(surface) = surface.as_ref()
            && !self.may_hold_keyboard(surface)
        {
            tracing::debug!("refused keyboard focus by the seat policy");
            return false;
        }
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, surface.clone(), serial);
            // Text-input focus is wherever the keyboard now is. Smithay 0.7
            // tells `SeatHandler::focus_changed` of a new surface and never
            // of none, so the keyboard left on nothing -- the last window on
            // screen closing, the session locking -- is told here, and the
            // text field gets its `leave`. A grab that ignored the
            // `set_focus` leaves the keyboard where it was, and this nothing.
            // `text_input::tests::a_field_whose_window_leaves_the_keyboard_on_nothing_is_gone`.
            let focused = keyboard.current_focus();
            self.remember_layer_focus(focused.as_ref());
            self.text_input_focus(focused.as_ref());
        }
        let client = surface.and_then(|surface| self.display_handle.get_client(surface.id()).ok());
        set_data_device_focus(&self.display_handle, &self.seat, client.clone());
        set_primary_focus(&self.display_handle, &self.seat, client);
        true
    }

    /// Install a keyboard grab for `owner`, under the same rule.
    ///
    /// A keyboard grab outranks focus. It sees every key before the focused
    /// surface does and decides where each one goes, and smithay's popup grab
    /// ignores every other `set_focus` until its menu closes. So a grab held
    /// for anything but a lock surface while locked would be focus by the back
    /// door, and it is refused the same way. Returns whether it was installed.
    #[expect(
        clippy::disallowed_methods,
        reason = "the gate itself: `clippy.toml` sends every other caller here"
    )]
    pub(crate) fn grab_keyboard<G: KeyboardGrab<Self> + 'static>(
        &mut self,
        owner: &WlSurface,
        grab: G,
        serial: Serial,
    ) -> bool {
        if !self.may_hold_keyboard(owner) {
            tracing::debug!("refused a keyboard grab for a surface behind the lock");
            return false;
        }
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_grab(self, grab, serial);
        }
        true
    }

    /// Take every grab off both devices, and close the menus holding them.
    ///
    /// This runs when the session locks, because a grab is a device promised
    /// to someone and at that moment everyone is behind the lock. A menu's
    /// keyboard grab would refuse the `set_focus` that is about to point the
    /// keyboard at nothing. A menu's pointer grab keeps the pointer on the
    /// menu's client, so the lock screen could not be clicked until a press
    /// had dismissed the menu, and dismissing it hands the keyboard back to
    /// the menu's window.
    ///
    /// The menus are dismissed, not just ungrabbed. A chain left recorded on
    /// the seat with nothing servicing it outlives the lock. After unlock, the
    /// next menu any other client opened would be told it was not the topmost
    /// popup, which is a protocol error and kills that client.
    ///
    /// The keyboard goes before the pointer, for the reason `grab` gives:
    /// unsetting a popup pointer grab takes off the keyboard grab with the
    /// matching serial and gives the keyboard to the menu's window. Once the
    /// keyboard grab is gone there is nothing left for it to match.
    pub(crate) fn release_grabs(&mut self) {
        self.layer_keyboard = None;
        // A hosted scene's grab goes first, as the menus do, and the presses
        // swallowed before the lock are forgotten: their releases are the
        // lock screen's.
        // `state::tests::real_client::reflow_on_close::hosted::locking_the_session_dismisses_a_hosted_grab`,
        // `state::tests::real_client::reflow_on_close::hosted::the_lock_forgets_the_presses_a_shell_swallowed`.
        self.dismiss_hosted_grab();
        self.swallowed.clear();
        // And a window frame's, and the press a frame was given, whose
        // release is the lock screen's (#417).
        // `state::tests::real_client::reflow_on_close::frames::locking_the_session_dismisses_a_frames_grab_and_its_press`.
        if let Some(pane) = self.decorations.holding.take()
            && let Some(decoration) = self
                .panes
                .get_mut(pane)
                .and_then(crate::pane::Pane::decoration_mut)
        {
            decoration.dismiss_grabs();
        }
        self.decorations.pressed = None;
        // And a scene's hold on the keyboard, which is the lock screen's now.
        // `state::tests::real_client::lock_focus::no_scene_holds_the_keyboard_while_the_session_is_locked`.
        self.end_keyboard_hold(false);
        if let Some(mut grab) = self.popup_grab.take() {
            grab.ungrab(PopupUngrabStrategy::All);
        }
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.unset_grab(self);
        }
        if let Some(pointer) = self.seat.get_pointer()
            && pointer.is_grabbed()
        {
            let time = u32::try_from(self.clock.now().as_millis()).unwrap_or(u32::MAX);
            pointer.unset_grab(self, SERIAL_COUNTER.next_serial(), time);
        }
        // A scene's press hold and its hover go too: behind the lock nothing
        // of the session's may notice the pointer. The press is ended inside
        // the scene first, by telling it the buttons went up unseen -- a
        // release of no button, with none held -- so whatever held it is
        // cancelled rather than clicked, and does not wake after the unlock
        // still pressed.
        // `state::tests::real_client::lock_focus::the_lock_lets_go_of_a_hosted_scenes_press_and_its_hover`,
        // `qml::hosted::tests::a_press_let_go_of_unseen_is_cancelled_not_clicked`.
        if let Some(held) = self.scene_press.take() {
            let location = self
                .seat
                .get_pointer()
                .map(|pointer| pointer.current_location())
                .unwrap_or_default();
            let let_go = crate::qml::hosted::ScenePointer {
                buttons: 0,
                ..crate::input::scene_event(self, crate::qml::hosted::PointerKind::Release(0))
            };
            if let Some(surface) = self.surfaces.get_mut(held.surface) {
                surface.deliver(&held.output, held.area, location, let_go);
            }
            self.redraw = true;
        }
        self.scene_hover_seen = None;
        if let Some((id, output)) = self.scene_hovered.take()
            && let Some(surface) = self.surfaces.get_mut(id)
        {
            surface.leave(&output);
        }
    }

    /// [`Self::release_grabs`] for one window: dismiss the menus `window` has
    /// open, and take their grabs off both devices.
    ///
    /// For a window that is going and taking the keyboard with it. A menu's
    /// keyboard grab ignores every `set_focus` until its chain ends, and
    /// [`Self::focused_window`] answers for a menu with the window it belongs
    /// to, so giving the keyboard to nothing did nothing: a window closed with
    /// a menu open kept the keyboard on that menu, and every key typed after
    /// the close went to it, for the whole of the close's grace period.
    /// `a_close_with_a_menu_open_takes_the_keyboard_off_the_menu` is the case.
    ///
    /// The order is `release_grabs`'s, keyboard before pointer, for its reason.
    /// A grab is taken off only if its serial is the menu's: a device grabbed
    /// by something else, a drag of another window, is not this window's to
    /// end.
    pub(crate) fn release_grabs_of(&mut self, window: &Window) {
        let Some(root) = window.wl_surface() else {
            return;
        };
        let Some(grab) = self.popup_grab.as_ref() else {
            return;
        };
        // The chain's topmost menu, or its root once no menu is left.
        let ours = !grab.has_ended()
            && grab.current_grab().is_some_and(|top| {
                top == *root
                    || self
                        .popups
                        .find_popup(&top)
                        .and_then(|popup| find_popup_root_surface(&popup).ok())
                        .is_some_and(|found| found == *root)
            });
        if !ours {
            return;
        }
        let serial = grab.serial();
        if let Some(mut grab) = self.popup_grab.take() {
            grab.ungrab(PopupUngrabStrategy::All);
        }
        if let Some(keyboard) = self.seat.get_keyboard()
            && keyboard.has_grab(serial)
        {
            keyboard.unset_grab(self);
        }
        if let Some(pointer) = self.seat.get_pointer()
            && pointer.has_grab(serial)
        {
            let time = u32::try_from(self.clock.now().as_millis()).unwrap_or(u32::MAX);
            pointer.unset_grab(self, SERIAL_COUNTER.next_serial(), time);
        }
    }

    /// Whether a key may go on to whatever the keyboard is pointed at.
    ///
    /// This is the rule's last word, and the keyboard filter asks it for every
    /// key. Everything above keeps the keyboard off the session while it is
    /// locked. This is what still holds if one of those ever does not, whether
    /// through a route nobody has found yet or a grab installed by something
    /// that bypassed this file. While locked, a key goes on only to a surface
    /// the rule allows, and only when no grab is steering it. Anything else is
    /// dropped. A lock screen that misses a keystroke is an annoyance; an
    /// application that receives one may have received a password.
    pub(crate) fn keys_may_pass(&self) -> bool {
        if self.lock.is_none() {
            return true;
        }
        let Some(keyboard) = self.seat.get_keyboard() else {
            // No keyboard, so no key to deliver.
            return true;
        };
        !keyboard.is_grabbed()
            && keyboard
                .current_focus()
                .is_none_or(|surface| self.may_hold_keyboard(&surface))
    }

    /// Whether an X11 client may read a selection a Wayland client owns.
    ///
    /// Not while the session is locked. The rule keeps the Wayland clipboard
    /// with the lock client while locked, but X11 has no focus-gated
    /// clipboard: through the bridge, any X11 client can read the selection at
    /// any moment. Every X11 client is behind the lock, since a lock client is
    /// never one, so while locked none of them may read it. Otherwise whatever
    /// the lock screen copies would reach the session it is locking.
    pub(crate) fn x11_may_read_selection(&self) -> bool {
        self.lock.is_none()
    }
}
