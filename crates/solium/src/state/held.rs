//! A binding whose key is held, and which asked to run again while it is
//! (#224): `sol.bind`'s `repeating`.
//!
//! A key that has a binding is the compositor's, so the client that would
//! repeat it never sees it, and libinput reports one press however long the
//! key is down. Without this, holding volume-up moves the volume one step.

use super::*;

/// The binding whose key is down, and when it runs next.
/// `input::tests::held::a_held_binding_marked_repeating_runs_again_at_the_keyboards_rate`.
#[derive(Clone, Debug)]
pub(crate) struct HeldBinding {
    combo: String,
    next: Duration,
    /// Where the keyboard was once the binding last ran. Anything else moving
    /// it ends the repeat; the binding moving it itself does not, because
    /// stepping the keyboard from window to window is what holding such a key
    /// is for.
    /// `state::tests::real_client::lock_focus::held::a_repeat_stops_when_the_keyboard_moves_to_another_window`,
    /// `state::tests::real_client::lock_focus::held::a_repeating_binding_that_moves_the_keyboard_itself_keeps_repeating`.
    focus: Option<WlSurface>,
}

impl Solium {
    /// `combo` just ran from a press of a key that repeats: if its binding
    /// asked to, it runs again once the key has been held for the keyboard's
    /// repeat delay. A keyboard whose rate is zero does not repeat, and nor do
    /// its bindings.
    /// `input::tests::held::a_keyboard_that_does_not_repeat_does_not_repeat_its_bindings`.
    pub(crate) fn hold_binding(&mut self, combo: String) {
        let asked = self
            .scripts
            .as_ref()
            .and_then(|scripts| scripts.binding_options(&combo));
        if !asked.is_some_and(|asked| asked.repeating) || self.keyboard.repeat_rate <= 0 {
            return;
        }
        let delay = Duration::from_millis(u64::try_from(self.keyboard.repeat_delay).unwrap_or(600));
        self.held_binding = Some(HeldBinding {
            combo,
            next: self.clock.now() + delay,
            focus: self.keyboard_focus(),
        });
    }

    /// The held binding runs again if it is due, checked once per loop
    /// iteration as a key held for a scene is (Ruling 14).
    ///
    /// At most once a look: a loop that looks late runs it once and is due an
    /// interval later, rather than once for every interval it missed, so a
    /// volume key held through a stall does not jump by however long the stall
    /// was. `input::tests::held::a_repeat_noticed_late_runs_once_and_does_not_catch_up`.
    ///
    /// Asked again on every run, because any of it can change while the key is
    /// down: the keyboard moved by something else, the binding replaced by a
    /// reload, the repeat rate set to zero. A handler that fails is not run
    /// again: an error twenty-five times a second says nothing the first one
    /// did not. `input::tests::held::a_repeating_binding_whose_handler_fails_is_not_run_again`.
    pub(crate) fn repeat_binding(&mut self, now: Duration) {
        let Some(held) = self.held_binding.as_ref() else {
            return;
        };
        if now < held.next {
            return;
        }
        let rate = u64::try_from(self.keyboard.repeat_rate).unwrap_or(0);
        let asked = self
            .scripts
            .as_ref()
            .and_then(|scripts| scripts.binding_options(&held.combo));
        let may = rate > 0
            && self.keyboard_focus() == held.focus
            && asked.is_some_and(|asked| {
                asked.repeating && (self.lock.is_none() || (asked.locked && self.keys_may_pass()))
            });
        if !may {
            self.held_binding = None;
            return;
        }
        let combo = held.combo.clone();
        let handled = self.trigger(&combo);
        let focus = self.keyboard_focus();
        let interval = Duration::from_millis(1000 / rate);
        match self.held_binding.as_mut() {
            Some(held) if handled => {
                held.next += interval;
                if held.next <= now {
                    held.next = now + interval;
                }
                held.focus = focus;
            }
            _ => self.held_binding = None,
        }
    }

    /// The surface with the keyboard, as the seat has it.
    fn keyboard_focus(&self) -> Option<WlSurface> {
        self.seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus())
    }
}
