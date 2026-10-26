//! `sol.press`: one button or key, pressed and let go on a window, as a
//! device would deliver it. The primitive a configuration builds a "back"
//! gesture from -- a tablet shell's edge swipe sends a window `side`, the
//! mouse's back button, or `alt+Left`, or `Escape` -- and what "back" means
//! is the configuration's choice, never the compositor's.
//!
//! **Delivered straight to the seat, past the bindings.** The press is for
//! the window, so nothing of the compositor's acts on it: no binding fires,
//! no click focuses or raises, and no scene hears it. It is what the seat
//! hands a client for a real press, through smithay's own pointer and
//! keyboard, which is what makes it the same for an X11 window as for a
//! Wayland one: both are a `wl_surface` to the seat, and Xwayland passes the
//! press on to the X window under the point it was given.
//!
//! **A button needs the pointer's focus, and borrows it.** A client takes a
//! `wl_pointer.button` only on a surface it was told the pointer entered, so
//! the window is given the pointer -- an `enter` at its centre, without the
//! pointer moving -- is pressed and let go of, and the pointer is given back
//! at once to what is really under it (`Solium::repoint_clients`), with the
//! picture it was showing. A client's own cursor, said in answer to the
//! borrowed `enter`, arrives after the pointer has gone back, and smithay
//! refuses it for the stale serial. The keyboard does not move.
//!
//! **A key needs the keyboard, and takes it.** A key to a window is the
//! window being typed into, so a window that does not have the keyboard is
//! focused first, as `sol.focus` would, and keeps it.
//!
//! **Never behind the lock**, and never while something else holds the
//! pointer: a button held, a drag, a menu, a scene's press or grab, or --
//! when the press would take the pointer off it -- a client's locked or
//! confined pointer. Then nothing is delivered, as a warp is refused there.
//! `tests::real_client::follow::press`,
//! `tests::real_client::lock_focus::nothing_sol_press_asks_reaches_a_window_behind_the_lock`.

use smithay::backend::input::{ButtonState, KeyState};
use smithay::input::keyboard::{FilterResult, Keysym};
use smithay::input::pointer::{ButtonEvent, MotionEvent};

use super::*;
use crate::script::{FocusSource, Press};

impl Solium {
    /// Carry out a `sol.press` on the window a script holds as `id`. A
    /// window that has gone, is leaving or is minimized since the script
    /// asked, a menu or a tooltip, or one with no application yet, is
    /// pressed nowhere.
    pub(crate) fn press(&mut self, id: u64, press: &Press, source: FocusSource) {
        if self.lock.is_some() {
            tracing::debug!("a press behind the lock was refused");
            return;
        }
        let reachable = self
            .panes
            .by_script_id(id)
            .is_some_and(|pane| pane.managed() && !pane.leaving() && !pane.minimized());
        let Some(window) = self.window_by_id(id).filter(|_| reachable) else {
            tracing::debug!(id, "a press for no window that can be reached");
            return;
        };
        match press {
            Press::Button(button) => self.press_button(&window, *button),
            Press::Keys(keysyms) => self.press_keys(&window, keysyms, source),
        }
    }

    /// One button down and up on `window`, with the pointer lent to it.
    fn press_button(&mut self, window: &Window, button: u32) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        if self.pressing
            || pointer.is_grabbed()
            || self.hosted_grab.is_some()
            || self.scene_press.is_some()
        {
            tracing::debug!("a press while the pointer is held was refused");
            return;
        }
        let Some(surface) = window.wl_surface().map(std::borrow::Cow::into_owned) else {
            return;
        };
        let before = pointer.current_focus();
        let lent = before.as_ref() != Some(&surface);
        if lent && crate::input::warp::held_by_a_client(&pointer) {
            tracing::debug!("a press while a client holds the pointer was refused");
            return;
        }
        let showing = self.pointer.said();
        let time = u32::try_from(self.clock.now().as_millis()).unwrap_or(u32::MAX);
        if lent {
            // The window's centre, in its own surface's terms, and the
            // origin that makes the pointer's unmoved location that point.
            let geometry = window.geometry();
            let centre = Point::<f64, Logical>::from((
                f64::from(geometry.loc.x) + f64::from(geometry.size.w) / 2.0,
                f64::from(geometry.loc.y) + f64::from(geometry.size.h) / 2.0,
            ));
            let location = pointer.current_location();
            pointer.motion(
                self,
                Some((surface, location - centre)),
                &MotionEvent {
                    location,
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                },
            );
            pointer.frame(self);
        }
        for state in [ButtonState::Pressed, ButtonState::Released] {
            pointer.button(
                self,
                &ButtonEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                    button,
                    state,
                },
            );
            pointer.frame(self);
        }
        if lent {
            self.repoint_clients();
            // Back on what it was on, the picture is what it was: the
            // borrowed `enter` and its `leave` each put the arrow up.
            if pointer.current_focus() == before {
                self.pointer.show(showing);
            }
            self.redraw = true;
        }
    }

    /// `keysyms` down in order and up in reverse on `window`, which is given
    /// the keyboard first if it does not have it.
    fn press_keys(&mut self, window: &Window, keysyms: &[Keysym], source: FocusSource) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let mut codes = Vec::with_capacity(keysyms.len());
        for &keysym in keysyms {
            let Some(code) = crate::keymap::keycode_of(self, keysym) else {
                tracing::warn!(
                    key = %xkb_name(keysym),
                    "sol.press: no key in this keymap types that, so nothing was pressed"
                );
                return;
            };
            codes.push(code);
        }
        // A scene holding the keyboard has it, whichever window it returns
        // to; focusing the window ends the hold.
        let has_it = |state: &Self| state.hosted_keyboard.is_none() && state.is_focused(window);
        if !has_it(self) {
            self.focus_window(window, SERIAL_COUNTER.next_serial(), source);
        }
        // A `focus` listener can send the keyboard somewhere else, and the
        // lock's rule has the last word.
        if !has_it(self) || !self.keys_may_pass() {
            tracing::debug!("a key for a window that did not take the keyboard was refused");
            return;
        }
        let time = u32::try_from(self.clock.now().as_millis()).unwrap_or(u32::MAX);
        let send = |state: &mut Self, code, key_state| {
            keyboard.input::<(), _>(
                state,
                code,
                key_state,
                SERIAL_COUNTER.next_serial(),
                time,
                |_, _, _| FilterResult::Forward,
            );
        };
        for &code in &codes {
            send(self, code, KeyState::Pressed);
        }
        for &code in codes.iter().rev() {
            send(self, code, KeyState::Released);
        }
        // A combination xkb itself acts on, such as a layout switch, is
        // told as one typed on the keyboard is.
        self.keyboard_changed();
    }
}

/// A keysym's name, for a log line.
fn xkb_name(keysym: Keysym) -> String {
    smithay::input::keyboard::xkb::keysym_get_name(keysym)
}
