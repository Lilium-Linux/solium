//! `ext-session-lock-v1`: the screen you cannot get past.
//!
//! A lock screen is not a feature, it is the reason you can leave a machine on
//! a desk. So the protocol is designed around one rule, and it is the rule
//! this module exists to keep:
//!
//! > **A lock that fails leaves the session locked and blank, never exposed.**
//!
//! Everything below follows from that. The session is locked the instant a
//! client asks, before any surface exists, so there is no window in which the
//! desktop is still on screen. If the locking client then crashes, the session
//! stays locked with nothing on it — inconvenient, and not dangerous. If it
//! never manages to draw, the same. The only way out is the client asking, or
//! the physical console.
//!
//! ## What is deliberately still allowed
//!
//! Switching virtual terminal. That is not a hole: whoever can press
//! `Ctrl+Alt+F2` is standing at the machine, logind owns the seat, and the
//! locked session's contents are not on any other VT. Taking it away would
//! remove the escape hatch that exists for a compositor that has stopped
//! responding.
//!
//! **`Ctrl+Alt+Backspace` is not allowed** while locked, and that one would be
//! a hole: it ends the session, and ending the session is how you get to the
//! desktop underneath.

use smithay::{
    backend::renderer::element::Id,
    input::pointer::MotionEvent,
    output::Output,
    reexports::wayland_server::{DisplayHandle, protocol::wl_output::WlOutput},
    utils::SERIAL_COUNTER,
    wayland::session_lock::{
        LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker,
    },
};

use crate::state::Solium;

/// What the compositor draws instead of the desktop while locked, when it has
/// nothing of the client's to draw.
///
/// Not black: a black screen is indistinguishable from a monitor that has gone
/// to sleep or a compositor that has died, and someone who cannot tell those
/// apart will reach for the power button. A visible, deliberate colour says
/// the machine is locked and working.
pub(crate) const BLANK: [f32; 4] = [0.06, 0.05, 0.11, 1.0];

/// The session lock, while there is one.
#[derive(Debug)]
pub(crate) struct Lock {
    /// One surface per monitor, by the output it was given for.
    ///
    /// A monitor with no entry is drawn blank. That is the honest state: the
    /// client has not covered that screen, and showing it the desktop instead
    /// would be the failure this protocol exists to prevent.
    surfaces: Vec<(Output, LockSurface)>,
    /// The identity of the backdrop, held for as long as the lock is.
    ///
    /// A fresh `Id` every frame would read to the damage tracker as a new
    /// element every frame, so a locked laptop sitting on a desk would repaint
    /// sixty times a second and run its battery down doing nothing.
    blank: Id,
}

impl Default for Lock {
    fn default() -> Self {
        Self {
            surfaces: Vec::new(),
            blank: Id::new(),
        }
    }
}

impl Lock {
    /// The identity to draw the backdrop under.
    pub(crate) fn blank(&self) -> Id {
        self.blank.clone()
    }

    /// The surface covering this monitor, if the client has provided one.
    pub(crate) fn surface_for(&self, output: &Output) -> Option<&LockSurface> {
        self.surfaces
            .iter()
            .find(|(each, _)| each == output)
            .map(|(_, surface)| surface)
    }

    /// Every surface, for hit-testing and focus.
    pub(crate) fn surfaces(&self) -> impl Iterator<Item = &LockSurface> {
        self.surfaces.iter().map(|(_, surface)| surface)
    }

    /// Drop surfaces whose client has gone, so a dead one is not drawn.
    fn sweep(&mut self) {
        self.surfaces.retain(|(_, surface)| surface.alive());
    }
}

impl SessionLockHandler for Solium {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.session_lock_state
    }

    /// A client has asked to lock the session.
    ///
    /// Confirmed immediately, and that is the safe order rather than the
    /// convenient one. The alternative — wait until every output has a surface
    /// and confirm then — leaves the desktop on screen in the meantime, which
    /// is exactly the window an attacker wants. Locking first and confirming
    /// at once is truthful because `render::elements` stops drawing windows
    /// the moment `lock` is set: by the time the client hears "locked", there
    /// is nothing of anyone's data left on any screen.
    fn lock(&mut self, confirmation: SessionLocker) {
        self.lock = Some(Lock::default());
        // Input is pointed at whatever the user was doing a moment ago, and it
        // stays pointed there until something moves. Between the lock and the
        // client's first surface -- which is a client starting up, so tens of
        // milliseconds at best -- every keystroke would go into that
        // application. The first thing typed at a lock screen is a password.
        blind(self);
        self.redraw = true;
        confirmation.lock();
        tracing::info!("session locked");
    }

    fn unlock(&mut self) {
        self.lock = None;
        self.redraw = true;
        // Cleared before it is re-aimed: keyboard focus is still on a lock
        // surface that is about to be destroyed, and the pointer still thinks
        // it is over one. `settle_focus` then gives the keyboard to whatever
        // the user was using, because a session that unlocks and ignores
        // typing until you move the mouse reads as one that did not unlock.
        blind(self);
        self.settle_focus();
        tracing::info!("session unlocked");
    }

    /// A surface for one monitor.
    ///
    /// Told the whole monitor, not the work area: a lock screen covers the
    /// bars too. A dock left visible over a lock screen is a list of what the
    /// user was doing, which is the sort of thing a lock screen is for hiding.
    fn new_surface(&mut self, surface: LockSurface, wl_output: WlOutput) {
        let Some(output) = Output::from_resource(&wl_output) else {
            // No output to put it on. Left unmapped rather than guessed at:
            // the session stays locked and that monitor stays blank.
            tracing::warn!("a lock surface arrived for an output that is gone");
            return;
        };
        let size = self
            .space
            .output_geometry(&output)
            .map(|geometry| geometry.size)
            .unwrap_or_default();
        surface.with_pending_state(|state| {
            state.size = Some((size.w.max(1) as u32, size.h.max(1) as u32).into());
        });
        surface.send_configure();

        if let Some(lock) = self.lock.as_mut() {
            lock.sweep();
            lock.surfaces.retain(|(each, _)| each != &output);
            lock.surfaces.push((output.clone(), surface));
        }
        // The keyboard belongs to the lock screen now, not to whatever had it.
        self.focus_lock();
        self.redraw = true;
        tracing::debug!(monitor = output.name(), "lock surface mapped");
    }
}

smithay::delegate_session_lock!(Solium);

/// Take keyboard and pointer focus away from everything.
///
/// Separate from pointing them at the lock screen, because there is a moment
/// -- and, if the client never draws, a permanent state -- where there is
/// nothing to point them at, and "nothing" has to be reachable on its own.
fn blind(state: &mut Solium) {
    if let Some(keyboard) = state.seat.get_keyboard() {
        keyboard.set_focus(state, None, SERIAL_COUNTER.next_serial());
    }
    state.clear_selection_focus();

    // The pointer does not re-ask what is under it until it moves, so a button
    // press with a still mouse would go to whatever it was over before. Sending
    // it to where it already is costs nothing and makes it ask again.
    if let Some(pointer) = state.seat.get_pointer() {
        let location = pointer.current_location();
        let event = MotionEvent {
            location,
            serial: SERIAL_COUNTER.next_serial(),
            time: u32::try_from(state.clock.now().as_millis()).unwrap_or(u32::MAX),
        };
        pointer.motion(state, None, &event);
        pointer.frame(state);
    }
}

/// Register the global.
pub(crate) fn state(display: &DisplayHandle) -> SessionLockManagerState {
    // No filter: any client may lock the session. That is the same answer every
    // compositor gives, and the reasoning is that locking is not a privilege —
    // it denies access rather than granting it, and a client that could be
    // trusted to run at all can be trusted to blank the screen. *Unlocking* is
    // the privileged half, and only the client holding the lock can do it.
    SessionLockManagerState::new::<Solium, _>(display, |_| true)
}
