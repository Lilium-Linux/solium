//! The keyboard while the session is locked: pointing it at the lock surface on the monitor the
//! user is working on, or at any lock surface, and `settle_focus`'s answer while locked.

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
}
