//! `ext-session-lock-v1`: the screen you cannot get past.
//!
//! A lock screen is not a feature, it is the reason you can leave a machine on
//! a desk. So the protocol is designed around one rule, and it is the rule
//! this module exists to keep:
//!
//! > **A lock that fails leaves the session locked and blank, never exposed.**
//!
//! Everything below follows from that. The session is locked the instant a
//! client asks, before any surface exists: input is taken away and the next
//! frame built for every monitor is the lock's, both before the request
//! returns (`locked_is_not_sent_in_the_dispatch_that_asked_for_it`). What a
//! monitor is already showing stays there until that frame replaces it, so the
//! client is told `locked` only then -- see "When `locked` is sent". If the
//! locking client then crashes, the session stays locked with nothing on it —
//! inconvenient, and not dangerous. If it never manages to draw, the same. The
//! only way out is the client asking, or the physical console.
//!
//! ## When `locked` is sent
//!
//! Once every monitor has *shown* a frame built under this lock, and not
//! before: "the locked event must not be sent until a new 'locked' frame has
//! been presented on all outputs". It is a promise about the screen, and a
//! client acts on it. `swaylock -f` returns once it has heard it, and
//! `swayidle -w before-sleep` lets the machine suspend when it returns: heard
//! early, the machine sleeps with the desktop still being scanned out, and
//! shows it again on waking.
//!
//! A backend asks [`Solium::lock_frame`] which lock a frame is built under as
//! it builds it, and hands the answer to [`Solium::frame_presented`] once that
//! frame is on the monitor. What "on the monitor" means is the backend's:
//!
//! * **tty:** the page flip of *that* frame has completed. The answer rides
//!   through the DRM compositor as the frame's user data and comes back out of
//!   `frame_submitted` at the vblank, so the flip of a frame queued before the
//!   lock -- the desktop -- hands back no lock and counts for nothing.
//!   `a_flip_already_in_flight_at_the_lock_does_not_count` plays that
//!   sequence; the DRM half needs a GPU, and no test reaches it.
//! * **nested:** the frame was handed to the host with `submit`, which is as
//!   far as a compositor inside another one can see.
//! * **tests:** there is no renderer, so they play the backend through the
//!   same two calls.
//!
//! The monitors waited for are the ones there are *now*. One unplugged while
//! `locked` waits is waited for no longer
//! (`a_monitor_unplugged_while_locking_does_not_hold_locked_back`), and nor is
//! one switched off with `enabled = false`, which `tty.rs` drops the same way
//! (`a_switched_off_monitor_does_not_hold_locked_back`). Nor is one that is
//! **dark** -- powered off through `power.rs`, and cleared by the backend --
//! because it shows nothing, so there is nothing on it for the lock to cover
//! (`a_dark_monitor_does_not_hold_locked_back`). Asked to be off is not
//! enough: until the backend has cleared it, it is still showing whatever it
//! had, and it is waited for like any other monitor
//! (`a_monitor_asked_off_but_not_yet_dark_still_holds_locked_back`). Powering
//! one off never makes `locked` wait longer, since it only takes a monitor
//! out of the wait (`a_monitor_powered_off_while_locking_lets_locked_go`),
//! and powering one on while locked puts it back in the wait, where it has
//! to show the lock like any other
//! (`a_monitor_powered_on_while_locking_must_show_the_lock_too`).
//! One plugged in must show the lock as well
//! (`a_monitor_plugged_in_while_locking_must_show_the_lock_too`), and with no
//! monitor at all `locked` goes at once
//! (`with_no_monitor_locked_goes_out_at_once`).
//!
//! **There is no time limit.** A monitor that has not shown the lock holds
//! `locked` back (`locked_waits_for_every_monitor_to_show_the_lock`), for as
//! long as it has not, and the session is locked and blank all the while. The
//! protocol's "reasonable time limit" is on waiting for the client's
//! surfaces, and even then `locked` may go only once the frames are on
//! screen. The two compositors worth comparing:
//!
//! * [niri][niri-lock] keeps showing the desktop for up to a second while it
//!   waits for the lock client's surfaces, then blanks. That timer never sends
//!   `locked`: [the render loop][niri-render] sends it once every output has
//!   rendered a locked frame -- *queued*, on a tty, not flipped -- or has its
//!   monitors off. A locked frame that fails to render gives the lock up
//!   (`finished`) and the session back.
//! * [sway][sway-lock] sends `locked` in the handler for the lock request,
//!   before any frame is drawn: no wait, and no timer.
//!
//! Solium blanks at once, waits for the flip rather than the queue, never
//! sends `locked` early, and never gives a lock up.
//!
//! [niri-lock]: https://github.com/niri-wm/niri/blob/1f03391ea644c2a43597de7f637269e26d1e1b49/src/niri.rs#L6440-L6523
//! [niri-render]: https://github.com/niri-wm/niri/blob/1f03391ea644c2a43597de7f637269e26d1e1b49/src/niri.rs#L4799-L4835
//! [sway-lock]: https://github.com/swaywm/sway/blob/1652c54b73f67df17b7b4ab0b0f7048204aa8104/sway/lock.c#L261-L310
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
//!
//! ## Who holds the lock
//!
//! Exactly one `ext_session_lock_v1`: the one that was granted the session,
//! which is told `locked` once the lock is on every monitor. Every
//! other lock object is answered `finished` the moment it is made, and nothing
//! it asks for afterwards is acted on. That is the whole of what a lock screen
//! is worth, because the gate in `focus.rs` hands the keyboard to "a lock
//! surface", and a lock surface is only as trustworthy as the client that made
//! it. Before this was kept, a second lock simply *replaced* the first:
//!
//! * **A fake lock screen.** Any application could lock the session itself,
//!   wiping the real lock screen's surfaces, then put up a surface of its own
//!   that looked like one. The gate gave it the keyboard, and the password
//!   typed into it.
//! * **An unlock by anyone.** Lock, then `unlock_and_destroy`: two requests,
//!   from any client, and the session was open.
//! * **Locked out of your own session.** `swayidle`'s `before-sleep` starting a
//!   second `swaylock` while the first was showing pushed the real lock screen's
//!   surfaces out, and nothing could be typed into it again.
//!
//! So: [`Lock::holder`] records the lock that was granted; a lock asked for
//! while the holder is alive is refused with `finished` and changes nothing;
//! a lock surface asked for on any other lock is never shown or focused; and
//! an unlock from any other lock is a protocol error and unlocks nothing.
//!
//! ### How an unlock knows who asked
//!
//! [`SessionLockHandler::unlock`] is not told, and smithay 0.7 calls it for an
//! `unlock_and_destroy` on *any* lock object -- even one it has just posted
//! `invalid_unlock` on, because there is no `return` after that error. Nor can
//! liveness stand in for identity: the requesting object is destroyed only
//! after its request has been dispatched, so at the moment `unlock` runs the
//! holder and the stranger are both alive. What does know is the dispatch
//! itself, which is handed the object the request came in on. So
//! `ext_session_lock_v1` is not delegated to smithay wholesale: the
//! `Dispatch` impl at the bottom of this file sees every request first,
//! answers the two that only the holder may make, and passes everything else
//! -- and the holder's own requests, but for an unlock before it has been told
//! `locked` -- to smithay's implementation unchanged. No patch to smithay is
//! needed.
//!
//! ### When the holder dies
//!
//! The protocol is explicit: a lock client dying must not unlock the session.
//! It does not. The session stays locked, the backdrop is drawn where its
//! surfaces were, and the keyboard goes nowhere. What changes is that the lock
//! is now *abandoned* -- its holder's lock object is gone, whether the client
//! crashed, was killed, was disconnected for a protocol error, or gave up
//! before it was told `locked` -- and a new
//! lock request is granted and takes over, which is the recovery the protocol
//! allows ("compositors may allow a new client to create a
//! ext_session_lock_v1 object and take responsibility for unlocking the
//! session"). Without it the only way out of a crashed lock screen would be
//! ending the session, and everything in it with it. The one lock that is
//! never granted is a second one while the holder is still there.

use smithay::{
    backend::renderer::element::Id,
    input::pointer::MotionEvent,
    output::Output,
    reexports::{
        wayland_protocols::ext::session_lock::v1::server::{
            ext_session_lock_manager_v1::ExtSessionLockManagerV1,
            ext_session_lock_surface_v1::{self, ExtSessionLockSurfaceV1},
            ext_session_lock_v1::{self, ExtSessionLockV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, Resource,
            backend::ClientId,
            protocol::{wl_output::WlOutput, wl_surface::WlSurface},
        },
    },
    utils::SERIAL_COUNTER,
    wayland::session_lock::{
        ExtLockSurfaceUserData, LockSurface, SessionLockHandler, SessionLockManagerGlobalData,
        SessionLockManagerState, SessionLockState, SessionLocker,
    },
};

use std::sync::atomic::{AtomicU64, Ordering};

use crate::state::Solium;

/// What the compositor draws instead of the desktop while locked, when it has
/// nothing of the client's to draw.
///
/// Not black: a black screen is indistinguishable from a monitor that has gone
/// to sleep or a compositor that has died, and someone who cannot tell those
/// apart will reach for the power button. A visible, deliberate dark grey says
/// the machine is locked and working. A grey and not a hue, as every colour
/// Solium ships is for now (the Theme's rule, `qml/Solium/Theme.qml`).
pub(crate) const BLANK: [f32; 4] = [0.10, 0.10, 0.10, 1.0];

/// Which lock a frame was built under.
///
/// A backend asks for it as it builds a monitor's frame and hands it back when
/// that frame is on the monitor. See "When `locked` is sent" in the module
/// documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LockFrame(u64);

/// Where each [`Lock`]'s [`LockFrame`] comes from.
///
/// One count for every lock this process grants, so a frame built under a lock
/// whose client has gone is not mistaken for one built under the lock that
/// took over from it (the second half of
/// `a_flip_already_in_flight_at_the_lock_does_not_count`).
static LOCKS: AtomicU64 = AtomicU64::new(0);

/// The session lock, while there is one.
#[derive(Debug)]
pub(crate) struct Lock {
    /// The lock object that was granted the session, and the only one whose
    /// requests are acted on. See the module documentation.
    ///
    /// Held for as long as the session is locked, including after it has died:
    /// a dead holder is what [`Lock::abandoned`] reads, and it is what lets a
    /// new lock client take over rather than being refused as a second lock.
    holder: ExtSessionLockV1,
    /// The `locked` event, until every monitor has shown this lock.
    ///
    /// `None` once it is sent. Kept, and never sent, if the holder goes
    /// without it: see `destroyed` in the `Dispatch` for
    /// `ext_session_lock_v1` below.
    confirmation: Option<SessionLocker>,
    /// What a frame built under this lock carries.
    frame: LockFrame,
    /// The monitors that have shown a frame built under this lock.
    ///
    /// Compared against the monitors there are *now*, each time, rather than
    /// against a list taken when the lock was asked for: that is what lets a
    /// monitor that went stop counting and makes one that arrived count
    /// (`a_monitor_unplugged_while_locking_does_not_hold_locked_back`,
    /// `a_monitor_plugged_in_while_locking_must_show_the_lock_too`).
    shown: Vec<Output>,
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

impl Lock {
    /// A lock granted to `confirmation`'s lock object, with nothing on any
    /// monitor yet and `locked` not sent.
    fn new(confirmation: SessionLocker) -> Self {
        Self {
            holder: confirmation.ext_session_lock().clone(),
            confirmation: Some(confirmation),
            frame: LockFrame(LOCKS.fetch_add(1, Ordering::Relaxed)),
            shown: Vec::new(),
            surfaces: Vec::new(),
            blank: Id::new(),
        }
    }

    /// Whether `lock` is the lock object holding the session.
    pub(crate) fn is_held_by(&self, lock: &ExtSessionLockV1) -> bool {
        self.holder == *lock
    }

    /// Whether `locked` is still owed: the holder has not been told it.
    pub(crate) fn pending(&self) -> bool {
        self.confirmation.is_some()
    }

    /// Whether the holder has gone without unlocking.
    ///
    /// Its lock object is destroyed. Told `locked`, a held lock object has
    /// only one way to be destroyed that is not its client going:
    /// `unlock_and_destroy`, which clears the lock before the object goes.
    /// (`destroy` while locked is a protocol error, which disconnects the
    /// client.) Before `locked`, `destroy` is legal, and is a lock client
    /// giving up. So this is "the lock client crashed, was killed, was thrown
    /// off, or gave up", and the session is still locked -- see the module
    /// documentation for what follows, and
    /// `a_lock_client_that_goes_before_locked_leaves_the_session_locked`.
    pub(crate) fn abandoned(&self) -> bool {
        !self.holder.is_alive()
    }

    /// The identity to draw the backdrop under.
    pub(crate) fn blank(&self) -> Id {
        self.blank.clone()
    }

    /// The surface covering this monitor, if the client has provided one and
    /// it is still there.
    pub(crate) fn surface_for(&self, output: &Output) -> Option<&LockSurface> {
        self.surfaces
            .iter()
            .find(|(each, surface)| each == output && surface.alive())
            .map(|(_, surface)| surface)
    }

    /// Every surface that is still there, for hit-testing and focus.
    ///
    /// Live ones only. A dead surface can be sent nothing, so a keyboard left
    /// on one is a keyboard on nothing, and counting it as the lock screen's
    /// would let `settle_focus` believe the keyboard already had a home.
    pub(crate) fn surfaces(&self) -> impl Iterator<Item = &LockSurface> {
        self.surfaces
            .iter()
            .map(|(_, surface)| surface)
            .filter(|surface| surface.alive())
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
    /// Locked immediately, and that is the safe order rather than the
    /// convenient one. The alternative — wait until every output has a surface
    /// and lock then — leaves the desktop on screen in the meantime, which is
    /// exactly the window an attacker wants. So `lock` is set, input is taken
    /// away and a frame is asked for, all before this returns, and every frame
    /// built from here on is built under the lock.
    ///
    /// *Confirmed* only once those frames are on every screen. Setting `lock`
    /// changes the next frame and not the one already being scanned out, and
    /// until that one is replaced the desktop is on the monitor. So
    /// `confirmation` is kept, and `locked` goes out from
    /// [`Solium::confirm_lock`] -- here only when there is no monitor to wait
    /// for (`with_no_monitor_locked_goes_out_at_once`). See "When `locked` is
    /// sent" in the module documentation, and
    /// `locked_is_not_sent_in_the_dispatch_that_asked_for_it`, which asserts
    /// both halves.
    ///
    /// Unless the session is already locked by a client that is still here.
    /// Then the answer is `finished`, which is what dropping `confirmation`
    /// sends, and nothing about the existing lock changes: not its surfaces,
    /// not the keyboard, not who may unlock it. That is the case the protocol
    /// names ("there is already another ext_session_lock_v1 object held by a
    /// client"), and in practice it is `swayidle` starting a second `swaylock`
    /// over the first, which reads `finished` and exits.
    ///
    /// A lock whose holder has died is taken over instead. See the module
    /// documentation: it stays locked throughout, and this is the way back in.
    fn lock(&mut self, confirmation: SessionLocker) {
        if let Some(held) = self.lock.as_ref() {
            if !held.abandoned() {
                tracing::info!("refused a second session lock: the session is already locked");
                drop(confirmation);
                return;
            }
            tracing::warn!("a new lock client took over from one that went without unlocking");
        }
        self.lock = Some(Lock::new(confirmation));
        // An unlock still waiting for its key to come up is moot now.
        self.refocus_on_release = false;
        // Input is pointed at whatever the user was doing a moment ago, and it
        // stays pointed there until something moves. Between the lock and the
        // client's first surface -- which is a client starting up, so tens of
        // milliseconds at best -- every keystroke would go into that
        // application. The first thing typed at a lock screen is a password.
        blind(self);
        self.redraw = true;
        tracing::info!("session locking: `locked` waits for every monitor to show the lock");
        self.confirm_lock();
    }

    /// The holder has unlocked.
    ///
    /// Only the holder reaches this, and only once it has been told `locked`
    /// (`an_unlock_before_locked_unlocks_nothing`). Smithay would call it for
    /// an unlock on any lock object at all; the `Dispatch` below answers every
    /// other one itself and never passes it on.
    fn unlock(&mut self) {
        // First, and the order is load-bearing: `settle_focus` below asks the
        // gate in `focus.rs`, and the gate refuses every window for as long as
        // `lock` is `Some`. Cleared after, the session would unlock with the
        // keyboard on nothing.
        self.lock = None;
        self.redraw = true;
        // logind's `LockedHint` follows, and a fresh delay inhibitor for sleep
        // is asked for if `lock.before_sleep` holds one: see `logind.rs`.
        self.logind.locked(false);
        // Cleared before it is re-aimed: keyboard focus is still on a lock
        // surface that is about to be destroyed, and the pointer still thinks
        // it is over one. `settle_focus` then gives the keyboard to whatever
        // the user was using, because a session that unlocks and ignores
        // typing until you move the mouse reads as one that did not unlock.
        blind(self);
        // Unless a key is still down, which at this moment it nearly always
        // is: the Enter that submitted the password, still under the user's
        // finger while the lock client checked it. Handed the keyboard now, the
        // window would be told in `wl_keyboard.enter` that Enter is held -- a
        // key it never saw pressed, typed at the lock screen. So the keyboard
        // stays on nothing until every key is up, a tenth of a second, and
        // `input::key` settles it then. The release itself goes to nobody.
        let held = self
            .seat
            .get_keyboard()
            .is_some_and(|keyboard| !keyboard.pressed_keys().is_empty());
        if held {
            self.refocus_on_release = true;
        } else {
            self.settle_focus();
        }
        tracing::info!("session unlocked");
    }

    /// A surface for one monitor, from the holder.
    ///
    /// Only the holder's reach here: a lock surface asked for on any other lock
    /// is answered by the `Dispatch` below and never shown. That is what stops
    /// an application putting up a lock screen of its own -- on a monitor the
    /// real one has not covered yet, or plugged in while locked, or at all.
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

// `delegate_session_lock!`, less the one interface the `Dispatch` below takes
// over. The macro and that impl cannot both exist -- they would be two impls of
// the same trait -- so a future edit that puts the macro back fails to compile
// rather than quietly handing unlocks back to smithay.
smithay::reexports::wayland_server::delegate_global_dispatch!(Solium: [
    ExtSessionLockManagerV1: SessionLockManagerGlobalData
] => SessionLockManagerState);
smithay::reexports::wayland_server::delegate_dispatch!(Solium: [
    ExtSessionLockManagerV1: ()
] => SessionLockManagerState);
smithay::reexports::wayland_server::delegate_dispatch!(Solium: [
    ExtSessionLockSurfaceV1: ExtLockSurfaceUserData
] => SessionLockManagerState);

/// Every request on an `ext_session_lock_v1`, seen before smithay sees it.
///
/// This is where "who asked" is known, because it is handed the object the
/// request came in on and nothing after it is. See "How an unlock knows who
/// asked" in the module documentation.
///
/// The holder's requests go to smithay untouched, but for one: an
/// `unlock_and_destroy` before it has been told `locked`. That is
/// `invalid_unlock`, answered here for the reason the first bullet gives, and
/// the session stays locked (`an_unlock_before_locked_unlocks_nothing`).
/// `destroy` from anyone goes to smithay too: smithay refuses it for a lock
/// that was told `locked`, which only the holder ever is. The two a non-holder
/// may not make are answered here:
///
/// * `unlock_and_destroy` is `invalid_unlock`, which is the protocol's own
///   error for exactly this -- unlocking on a lock that was never told
///   `locked` -- and disconnects the client. Smithay posts the same error and
///   then unlocks anyway.
/// * `get_lock_surface` is legal (the protocol says such a surface is simply
///   never displayed), so the client is not disconnected for it. It gets an
///   [`Unheld`] object, which does nothing: smithay never sees it, so it is
///   never given the role, configured, drawn or focused, and `new_surface`
///   never hears of it.
impl Dispatch<ExtSessionLockV1, SessionLockState> for Solium {
    fn request(
        state: &mut Self,
        client: &Client,
        lock: &ExtSessionLockV1,
        request: ext_session_lock_v1::Request,
        data: &SessionLockState,
        display: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let holds = state
            .lock
            .as_ref()
            .is_some_and(|held| held.is_held_by(lock));
        let request = if holds {
            match request {
                ext_session_lock_v1::Request::UnlockAndDestroy
                    if state.lock.as_ref().is_some_and(Lock::pending) =>
                {
                    tracing::warn!("refused an unlock from a lock that has not been told `locked`");
                    lock.post_error(
                        ext_session_lock_v1::Error::InvalidUnlock,
                        "the session is not locked yet: `locked` has not been sent",
                    );
                    return;
                }
                other => other,
            }
        } else {
            match request {
                ext_session_lock_v1::Request::UnlockAndDestroy => {
                    tracing::warn!("refused an unlock from a client that does not hold the lock");
                    lock.post_error(
                        ext_session_lock_v1::Error::InvalidUnlock,
                        "this lock was never granted the session",
                    );
                    return;
                }
                ext_session_lock_v1::Request::GetLockSurface { id, .. } => {
                    tracing::debug!("a lock surface on a lock that does not hold the session");
                    data_init.init(id, Unheld);
                    return;
                }
                other => other,
            }
        };
        <SessionLockManagerState as Dispatch<ExtSessionLockV1, SessionLockState, Self>>::request(
            state, client, lock, request, data, display, data_init,
        );
    }

    fn destroyed(
        state: &mut Self,
        client: ClientId,
        lock: &ExtSessionLockV1,
        data: &SessionLockState,
    ) {
        // The holder going while the session is still locked: `unlock` has
        // already cleared the lock by the time an unlocking holder's object is
        // destroyed, so this is the lock client dying, or giving up before it
        // was told `locked`. Nothing is unlocked -- see `Lock::abandoned` --
        // but it is worth a line, because from the other side of the screen
        // it is a lock screen that vanished.
        //
        // A `locked` still owed is *not* dropped here, though dropping it is
        // how smithay says "no": it sends `finished`, and this runs before the
        // backend forgets the object, so a client that gave up with `destroy`
        // would be sent `finished` on the object it has just destroyed. Kept,
        // it is never sent -- `confirm_lock` confirms no lock whose holder has
        // gone -- and it is dropped with the `Lock` when a new lock client
        // takes over, by which time the object is gone and nothing is sent.
        // `a_lock_client_that_goes_before_locked_leaves_the_session_locked`.
        if state
            .lock
            .as_ref()
            .is_some_and(|held| held.is_held_by(lock))
        {
            tracing::warn!(
                owed_locked = state.lock.as_ref().is_some_and(Lock::pending),
                "the lock client went without unlocking; the session stays locked \
                 until a new lock client takes over"
            );
            state.redraw = true;
        }
        <SessionLockManagerState as Dispatch<ExtSessionLockV1, SessionLockState, Self>>::destroyed(
            state, client, lock, data,
        );
    }
}

/// A lock surface asked for on a lock that does not hold the session.
///
/// Inert on purpose: see the `Dispatch` for `ext_session_lock_v1` above. Its
/// two requests are `destroy`, which needs nothing doing, and `ack_configure`
/// for a configure it was never sent.
#[derive(Debug)]
pub(crate) struct Unheld;

impl Dispatch<ExtSessionLockSurfaceV1, Unheld> for Solium {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _surface: &ExtSessionLockSurfaceV1,
        _request: ext_session_lock_surface_v1::Request,
        _data: &Unheld,
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Solium {
    /// Which lock a frame built now is built under, if the session is locked.
    ///
    /// Asked by a backend as it builds a monitor's frame, and handed back to
    /// [`Solium::frame_presented`] once that frame is on the monitor. `Some`
    /// exactly when `lock` is, which is the guard `render::elements` takes its
    /// lock branch on (held to it by
    /// `the_drag_icon_is_emitted_below_the_lock_screens_early_return`); and
    /// `Some` from the dispatch that asked for the lock on
    /// (`locked_is_not_sent_in_the_dispatch_that_asked_for_it`).
    pub(crate) fn lock_frame(&self) -> Option<LockFrame> {
        self.lock.as_ref().map(|lock| lock.frame)
    }

    /// A frame built under `frame` is on `output`'s screen.
    ///
    /// Counts only a frame built under the lock there is now: `None` is a
    /// frame built before the session locked, and another lock's is one
    /// built before this lock took over. Neither says anything about what
    /// this lock has put on the screen. See
    /// `a_flip_already_in_flight_at_the_lock_does_not_count`.
    pub(crate) fn frame_presented(&mut self, output: &Output, frame: Option<LockFrame>) {
        let Some(lock) = self.lock.as_mut() else {
            return;
        };
        if !lock.pending() || frame != Some(lock.frame) {
            return;
        }
        if !lock.shown.contains(output) {
            lock.shown.push(output.clone());
            tracing::info!(monitor = output.name(), "the lock is on this monitor");
        }
        self.confirm_lock();
    }

    /// Tell the holder `locked`, if every monitor there is has shown the lock.
    ///
    /// Asked whenever the answer can change: when the lock is taken (with no
    /// monitor, it goes at once), when a frame reaches a screen, and when the
    /// set of monitors changes (`settle_monitors`), because a monitor that has
    /// gone is no longer waited for and one that has arrived now is. Never for
    /// a holder that has gone: see `destroyed` in the `Dispatch` below.
    ///
    /// And when a monitor goes dark (`Solium::output_dark`): a dark monitor is
    /// not waited for. See the module documentation.
    ///
    /// `locked_waits_for_every_monitor_to_show_the_lock`,
    /// `with_no_monitor_locked_goes_out_at_once`,
    /// `a_monitor_unplugged_while_locking_does_not_hold_locked_back`,
    /// `a_monitor_plugged_in_while_locking_must_show_the_lock_too` and
    /// `a_dark_monitor_does_not_hold_locked_back`.
    pub(crate) fn confirm_lock(&mut self) {
        let Some(lock) = self.lock.as_mut() else {
            return;
        };
        if !lock.pending() || lock.abandoned() {
            return;
        }
        let waiting: Vec<String> = self
            .space
            .outputs()
            .filter(|output| !lock.shown.contains(output) && !self.power.is_dark(output))
            .map(Output::name)
            .collect();
        if !waiting.is_empty() {
            tracing::debug!(
                ?waiting,
                "`locked` waits for these monitors to show the lock"
            );
            return;
        }
        if let Some(confirmation) = lock.confirmation.take() {
            lock.shown.clear();
            confirmation.lock();
            tracing::info!("session locked: every monitor is showing the lock");
            // The presentation-accurate moment `logind.rs` waits for: it
            // releases the delay inhibitor held for sleep, if any, and sets
            // `LockedHint`.
            self.logind.locked(true);
        }
    }

    /// A surface has been destroyed; if it was one of the lock screen's, the
    /// keyboard needs somewhere else to be.
    ///
    /// This is a monitor unplugged or a laptop undocked while locked: the lock
    /// client destroys that monitor's surface when its output goes, and if the
    /// keyboard was on it, it was on nothing from then on. `focus_lock` runs
    /// only when a surface maps, and `settle_focus` is not called for a
    /// surface that is not a window. So the keyboard is resettled here, through
    /// `settle_focus`, which while locked means a surviving lock surface.
    pub(crate) fn lock_surface_destroyed(&mut self, surface: &WlSurface) {
        let Some(lock) = self.lock.as_mut() else {
            return;
        };
        if !lock
            .surfaces
            .iter()
            .any(|(_, each)| each.wl_surface() == surface)
        {
            return;
        }
        lock.surfaces
            .retain(|(_, each)| each.wl_surface() != surface && each.alive());
        self.redraw = true;
        tracing::debug!("a lock surface went; resettling the keyboard");
        self.settle_focus();
    }
}

/// Take keyboard and pointer focus away from everything.
///
/// Separate from pointing them at the lock screen, because there is a moment
/// -- and, if the client never draws, a permanent state -- where there is
/// nothing to point them at, and "nothing" has to be reachable on its own.
///
/// Grabs first. A menu's keyboard grab ignores `set_focus` until the menu
/// closes, so with a menu open at the moment of locking the line after this
/// one did nothing at all, `focus_lock` did nothing either, and every key
/// typed at the lock screen went to the menu's client.
fn blind(state: &mut Solium) {
    state.release_grabs();
    state.give_keyboard(None, SERIAL_COUNTER.next_serial());

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
    // the privileged half, and only the lock's holder can do it: see the
    // `Dispatch` for `ext_session_lock_v1` above.
    SessionLockManagerState::new::<Solium, _>(display, |_| true)
}
