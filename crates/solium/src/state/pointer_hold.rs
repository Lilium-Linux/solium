//! Who may hold the pointer (#427): lock it still or confine it, as a game
//! does, and how it is taken back.
//!
//! A pointer constraint is granted on the first motion over the surface that
//! asked (`input::held`). It used to be granted to whatever surface was under
//! the pointer and kept for as long as the pointer stayed there, which, for a
//! lock, is for ever: a locked pointer does not move, so it never leaves. An
//! X11 game through XWayland asks for exactly that, a lock or a confinement
//! made out of its `XGrabPointer` and its warps, and The Finals kept the
//! pointer with no way back to the desktop but quitting it.
//!
//! - **Only the window with the keyboard holds the pointer.** A constraint is
//!   granted only to a surface of the window the keyboard is on, and let go
//!   of when the keyboard moves anywhere else: a key that focuses another
//!   window or another monitor, a click elsewhere, Alt+Tab. That is the rule
//!   the keyboard shortcuts inhibitor already follows (`shortcuts_inhibit`).
//! - **`sol.release_pointer` takes it back** from the window that holds it,
//!   which then holds it again only once it is pressed on. The shipped
//!   `super+Escape` calls it, beside `sol.take_shortcuts_back`, so one key
//!   gets out of a game and out of a virtual machine alike.
//! - **An X11 window also loses the keyboard to it.** An X client's own grab
//!   is the X server's, and no request lets another client end it; what the
//!   window manager can do is take the X input focus away, and a game drops
//!   its grabs when it loses the focus (Wine on `FocusOut`, SDL on losing
//!   keyboard focus). So the escape is a click away from being back in the
//!   game, and never a grab left behind on the desktop.

use super::*;
use smithay::wayland::compositor::get_parent;
use smithay::wayland::pointer_constraints::with_pointer_constraint;

/// The surface a (sub)surface belongs to: its root, which is what holds the
/// keyboard and is a window's own.
fn root_of(surface: &WlSurface) -> WlSurface {
    let mut root = surface.clone();
    while let Some(parent) = get_parent(&root) {
        root = parent;
    }
    root
}

/// Whether a surface whose root is `root` may hold the pointer: the keyboard
/// is on that root (`keyboard`, also a root), and `sol.release_pointer` has
/// not let go of it since it was last pressed on (`released`).
/// `tests::only_the_surface_with_the_keyboard_and_not_released_may_hold_the_pointer`.
pub(crate) fn may_hold<S: PartialEq>(root: &S, keyboard: Option<&S>, released: Option<&S>) -> bool {
    keyboard == Some(root) && released != Some(root)
}

impl Solium {
    /// Whether `surface` may hold the pointer now. See [`may_hold`].
    ///
    /// Never from inside a seat callback such as `SeatHandler::focus_changed`
    /// or a pointer grab's, which smithay calls with that device's lock held:
    /// asking the device there deadlocks. So nothing here runs where the
    /// keyboard moves; [`Self::settle_pointer_hold`] runs once a pass, and
    /// `input::held` at every motion.
    pub(crate) fn may_hold_pointer(&self, surface: &WlSurface) -> bool {
        let keyboard = self
            .seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus())
            .map(|focus| root_of(&focus));
        may_hold(
            &root_of(surface),
            keyboard.as_ref(),
            self.pointer_released.as_ref(),
        )
    }

    /// Let go of the constraint on `surface`, if one holds the pointer.
    /// Returns whether one did.
    fn let_go_of_constraint(&self, surface: &WlSurface) -> bool {
        let Some(pointer) = self.seat.get_pointer() else {
            return false;
        };
        with_pointer_constraint(surface, &pointer, |constraint| match constraint {
            Some(constraint) if constraint.is_active() => {
                constraint.deactivate();
                true
            }
            _ => false,
        })
    }

    /// The surface under the pointer keeps a constraint only while it may
    /// hold the pointer (#427): once a pass, from `sync_panes`, so a key
    /// that moved the keyboard elsewhere lets go of a locked pointer that no
    /// motion will ever reach `input::held` for.
    pub(crate) fn settle_pointer_hold(&mut self) {
        let Some(surface) = self
            .seat
            .get_pointer()
            .and_then(|pointer| pointer.current_focus())
        else {
            return;
        };
        if !self.may_hold_pointer(&surface) && self.let_go_of_constraint(&surface) {
            self.constraint_hint = None;
            tracing::info!("the keyboard left the window holding the pointer; it is let go");
        }
    }

    /// `sol.release_pointer`: take the pointer back from the window holding
    /// it, until that window is pressed on again; and the keyboard too from
    /// an X11 one, so the X11 client lets go of its own grabs (#427; see the
    /// module header).
    pub(crate) fn release_pointer(&mut self) {
        let under = self
            .seat
            .get_pointer()
            .and_then(|pointer| pointer.current_focus());
        let keyboard = self
            .seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus());
        let mut held = false;
        for surface in under.iter().chain(keyboard.iter()) {
            held |= self.let_go_of_constraint(surface);
        }
        // The pointer goes on from where it was held, and the first motion
        // after is not swallowed by a jump to where the client would have
        // left it (`cursor_position_hint`): the person is leaving the window,
        // not the client letting go.
        self.constraint_hint = None;
        self.pointer_released = keyboard.as_ref().or(under.as_ref()).map(root_of);
        let x11 = keyboard
            .as_ref()
            .and_then(|surface| self.window_for(&root_of(surface)))
            .is_some_and(|window| window.x11_surface().is_some());
        if x11 {
            self.clear_focus(SERIAL_COUNTER.next_serial());
        }
        self.redraw = true;
        tracing::info!(
            held,
            x11,
            "the pointer was taken back; the window holds it again once pressed on"
        );
    }

    /// A press on `surface`: a window `sol.release_pointer` let go of may
    /// hold the pointer again.
    pub(crate) fn pressed_on(&mut self, surface: &WlSurface) {
        if self.pointer_released.as_ref() == Some(&root_of(surface)) {
            self.pointer_released = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::may_hold;

    /// **#427**: the window with the keyboard may hold the pointer, and no
    /// other window may, and not one the pointer was taken back from.
    #[test]
    fn only_the_surface_with_the_keyboard_and_not_released_may_hold_the_pointer() {
        let (game, steam) = (1, 2);
        assert!(may_hold(&game, Some(&game), None), "the game being played");
        assert!(
            !may_hold(&game, Some(&steam), None),
            "a key focused Steam: the game under the pointer lets go"
        );
        assert!(
            !may_hold(&game, None, None),
            "the keyboard is on nothing, as after super+Escape on an X11 game"
        );
        assert!(
            !may_hold(&game, Some(&game), Some(&game)),
            "super+Escape took it back and it has not been pressed on since"
        );
        assert!(
            may_hold(&game, Some(&game), Some(&steam)),
            "taken back from another window says nothing about this one"
        );
    }
}
