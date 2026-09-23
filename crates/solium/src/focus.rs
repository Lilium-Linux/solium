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
    desktop::{PopupUngrabStrategy, Window, find_popup_root_surface},
    input::keyboard::KeyboardGrab,
    reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface},
    utils::{SERIAL_COUNTER, Serial},
    wayland::{
        seat::WaylandFocus,
        selection::{data_device::set_data_device_focus, primary_selection::set_primary_focus},
    },
};

use crate::state::Solium;

impl Solium {
    /// The rule. Everything else in this file is a way of asking it.
    ///
    /// Unlocked, any surface may hold the keyboard. Locked, only a surface of
    /// the lock client's own may. A window, a popup, a layer surface or an X11
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
            None => true,
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
            None => self.lock.is_none(),
        }
    }

    /// Point the keyboard at `surface`, or at nothing. **This is the only
    /// place in Solium that moves keyboard focus.**
    ///
    /// Refused, with nothing changed, for a surface the rule does not allow.
    /// `None` is always allowed: taking the keyboard away from everything
    /// cannot deliver a key to anyone. Returns whether the focus was set.
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
        if let Some(surface) = surface.as_ref()
            && !self.may_hold_keyboard(surface)
        {
            tracing::debug!("refused the keyboard to a surface behind the lock");
            return false;
        }
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, surface.clone(), serial);
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
