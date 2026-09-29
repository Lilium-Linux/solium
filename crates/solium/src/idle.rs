//! Idleness: `ext-idle-notify-v1` and `zwp-idle-inhibit-v1`.
//!
//! Two halves of one question, and they are only correct together. The first
//! lets a program ask to be told when nobody has touched the machine for a
//! while — that is how `swayidle` dims a screen, and how it runs a locker, so
//! it is what makes the lock screen in `lock.rs` happen on its own instead of
//! only when asked. The second is the veto: a video player, a presentation or
//! a game says "not now", and the idleness that would have blanked the screen
//! never arrives.
//!
//! Shipping the first without the second is worse than shipping neither. A
//! compositor that reports idleness truthfully and cannot be told to stop will
//! lock the screen half way through a film, and the person it happens to will
//! turn the whole feature off rather than find out which half was missing.
//!
//! ## Written out by hand
//!
//! Smithay has an `IdleNotifierState`, and it keeps its own timers, so it
//! needs a `LoopHandle` typed to the compositor state. Solium's two backends
//! do not agree about what that type is — the nested loop carries `Solium`
//! and the hardware loop carries a `tty::State` that contains one — so there
//! is no single handle to give it. Rather than bend one backend's event loop
//! to a protocol's convenience, the timing is done where the answer already
//! is: both loops wake at least every 16 ms, so `settle` compares two
//! instants once a frame and no timer is needed at all.
//!
//! The inhibitor half *is* Smithay's, because it has no timers in it.
//!
//! ## What counts as an inhibitor
//!
//! The protocol says an inhibitor applies while its surface is visible, and
//! leaves "visible" to the compositor. Solium's answer is in
//! `Solium::idle_inhibited`, and the two cases worth stating are that a window
//! on another workspace does not hold the machine awake, and neither does
//! anything at all while the session is locked. The second is not a detail: a
//! video player left playing behind a lock screen would otherwise keep the
//! machine awake all night showing a lock screen.

use std::time::Duration;

use smithay::{
    reexports::{
        wayland_protocols::ext::idle_notify::v1::server::{
            ext_idle_notification_v1::{self, ExtIdleNotificationV1},
            ext_idle_notifier_v1::{self, ExtIdleNotifierV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            protocol::{wl_seat::WlSeat, wl_surface::WlSurface},
        },
    },
    wayland::idle_inhibit::{IdleInhibitHandler, IdleInhibitManagerState},
};

use crate::state::Solium;

/// Registers `ext_idle_notifier_v1`.
#[derive(Debug)]
pub(crate) struct IdleState {
    #[expect(dead_code, reason = "holds the global; dropping it would remove it")]
    global: smithay::reexports::wayland_server::backend::GlobalId,
}

impl IdleState {
    pub(crate) fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ExtIdleNotifierV1, ()> + 'static,
    {
        // Version 2, for `get_input_idle_notification`: a notification that
        // watches input only and ignores inhibitors. That is what a locker
        // wants -- a film should stop the screen dimming, and should not stop
        // the screen locking after an hour of nobody in the room.
        Self {
            global: display.create_global::<D, ExtIdleNotifierV1, _>(2, ()),
        }
    }
}

/// One client's standing question: "tell me when nothing has happened for this
/// long".
#[derive(Debug)]
pub(crate) struct Notification {
    object: ExtIdleNotificationV1,
    timeout: Duration,
    /// From `get_input_idle_notification`, so an inhibitor does not hold it
    /// off. See the version note above.
    ignores_inhibitors: bool,
    /// What the client currently believes. Kept so that `idled` and `resumed`
    /// are sent on the edges and not once a frame.
    idle: bool,
}

/// What `config.lua`'s `idle` section says: the idle blank, and what a window
/// on a screen that is off is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Settings {
    /// How long without input before every screen is turned off. Zero never
    /// does. See [`Settings::default`] for the number and the reason for it.
    /// `the_idle_blank_turns_every_screen_off_after_the_timeout`.
    pub(crate) screens_off_after: Duration,
    /// How often a window shown only on screens that are off is still told to
    /// draw. Zero never does. See "Frame callbacks on a dark monitor" in
    /// `power.rs`, and
    /// `a_window_on_a_dark_monitor_is_told_to_draw_once_a_second_and_not_every_frame`.
    pub(crate) off_frame_interval: Duration,
}

impl Default for Settings {
    /// **Ten minutes, and on.** The machine this was asked for kept its
    /// screens lit all night, and a feature that only works for someone who
    /// has found the setting is the same machine lit all night.
    ///
    /// Ten rather than GNOME's five, because GNOME dims the screen before it
    /// blanks it, and Solium does not: here the first sign is the screen going
    /// dark, so it should come later than a desktop that warns you first.
    /// Long enough that reading a long page without touching anything does
    /// not blank it; anything that plays -- a film, a call, a presentation --
    /// holds an idle inhibitor, which holds this off
    /// (`an_idle_inhibitor_holds_the_idle_blank_off`). Someone running
    /// `swayidle` to do it their own way sets it to 0. `config.lua` says the
    /// same number (`the_shipped_configuration_turns_the_screens_off_after_ten_minutes`).
    fn default() -> Self {
        Self {
            screens_off_after: Duration::from_secs(600),
            off_frame_interval: Duration::from_secs(1),
        }
    }
}

/// Everything the compositor knows about nobody being at the machine.
#[derive(Debug, Default)]
pub(crate) struct Idle {
    notifications: Vec<Notification>,
    /// Surfaces whose clients have asked the machine to stay awake. Held as
    /// surfaces rather than as a count because whether one of them *counts* is
    /// decided later, and changes without the client saying anything -- a
    /// window moved to another workspace stops holding the machine awake.
    inhibitors: Vec<WlSurface>,
    /// When something last happened. `None` until the first input, which is
    /// treated as "just now" rather than "since the beginning of time": a
    /// compositor that has been up for a minute and never been touched should
    /// not report a minute of idleness to a client that has just connected.
    since: Option<Duration>,
    /// The first frame, which is what the idle blank counts from until there
    /// is any input: a machine started and never touched is a machine nobody
    /// is at. `the_idle_blank_turns_every_screen_off_after_the_timeout`
    /// touches nothing.
    began: Option<Duration>,
    /// The last frame something visible held the machine awake: an idle
    /// inhibitor that counted. Letting go starts the count again from here
    /// rather than from the last input, which is what smithay's own
    /// `IdleNotifierState::set_is_inhibited` and wlroots do: a film that ends
    /// an hour after anybody touched anything does not take the screens with
    /// it, and the next one in a playlist, which takes the inhibitor again a
    /// few seconds later, does not start in the dark
    /// (`an_idle_inhibitor_holds_the_idle_blank_off`,
    /// `an_idle_notification_counts_again_from_when_an_inhibitor_lets_go`).
    held: Option<Duration>,
    /// The idle blank has turned the screens off, and nothing has kept the
    /// machine awake since: no input, and no inhibitor. So it does it once per
    /// absence, and a screen a client turns back on meanwhile stays on
    /// (`the_idle_blank_turns_every_screen_off_after_the_timeout`), while an
    /// inhibitor taken and let go again is an absence of its own
    /// (`an_idle_inhibitor_holds_the_idle_blank_off`).
    blanked: bool,
    settings: Settings,
}

impl Idle {
    /// Whether any client is holding the machine awake.
    pub(crate) fn inhibiting(&self) -> impl Iterator<Item = &WlSurface> {
        self.inhibitors.iter()
    }

    /// Input happened.
    ///
    /// Called for every event from every device, so it is deliberately two
    /// comparisons and a write in the common case: the interesting work is in
    /// `settle`, which runs once a frame rather than once per motion event.
    pub(crate) fn stir(&mut self, now: Duration) {
        self.since = Some(now);
    }

    /// What the configuration says. See [`Settings`].
    pub(crate) fn settings(&self) -> Settings {
        self.settings
    }

    /// Take what the configuration says.
    pub(crate) fn configure(&mut self, settings: Settings) {
        if self.settings != settings {
            tracing::debug!(?settings, "idle settings");
            self.settings = settings;
        }
    }
}

/// Decide, once a frame, who is idle and who is not.
///
/// Separate from `stir` because the answer depends on things no input event
/// knows: whether an inhibiting window is still on screen, whether the session
/// has been locked, whether a client has just asked its first question.
pub(crate) fn settle(state: &mut Solium) {
    let now = state.clock.now();
    // Every frame, and not only once a timeout has passed: an inhibitor let go
    // is only seen by looking, and the count starts again from the last frame
    // it was seen held (see `Idle::held`, and
    // `an_idle_inhibitor_holds_the_idle_blank_off`). With no inhibitor there
    // is nothing to walk.
    let inhibited = state.idle_inhibited();
    if inhibited {
        state.idle.held = Some(now);
    }
    blank(state, now);
    if state.idle.notifications.is_empty() {
        return;
    }
    let since = *state.idle.since.get_or_insert(now);
    let elapsed = now.saturating_sub(since);
    let held = state.idle.held;

    // Dead notifications go here rather than at the top: a client that
    // disconnects mid-frame would otherwise be written to once more, and the
    // list would grow for as long as the compositor runs.
    state
        .idle
        .notifications
        .retain(|notification| notification.object.is_alive());

    for notification in &mut state.idle.notifications {
        let (held_off, elapsed) = if notification.ignores_inhibitors {
            (false, elapsed)
        } else {
            (inhibited, quiet(now, since, held))
        };
        let idle = !held_off && elapsed >= notification.timeout;
        if idle == notification.idle {
            continue;
        }
        notification.idle = idle;
        if idle {
            notification.object.idled();
        } else {
            notification.object.resumed();
        }
    }
}

/// The idle blank: every screen off once nobody has touched anything for
/// `idle.screens_off_after`, unless something visible is holding the machine
/// awake. Input turns them back on: see `Solium::wake_screens`.
///
/// The same inhibitors, counted the same way, as the notifications above --
/// so nothing holds it off while the session is locked, and a lock screen goes
/// dark like anything else -- and counted from when the last one let go, as
/// they are (see `Idle::held`).
/// `the_idle_blank_turns_every_screen_off_after_the_timeout`,
/// `an_idle_inhibitor_holds_the_idle_blank_off` and
/// `input_wakes_every_screen_the_idle_blank_turned_off`.
fn blank(state: &mut Solium, now: Duration) {
    let after = state.idle.settings.screens_off_after;
    if after.is_zero() {
        return;
    }
    let began = *state.idle.began.get_or_insert(now);
    let since = state.idle.since.unwrap_or(began);
    if quiet(now, since, state.idle.held) < after {
        state.idle.blanked = false;
        return;
    }
    if state.idle.blanked {
        return;
    }
    state.idle.blanked = true;
    tracing::info!(
        seconds = after.as_secs(),
        "nobody is here: every screen off (idle.screens_off_after)"
    );
    state.power_all(false);
}

/// How long nothing has kept the machine awake: since the last input, or
/// since the last frame an inhibitor held it, whichever was later. While one
/// is held that is no time at all. See `Idle::held`.
fn quiet(now: Duration, since: Duration, held: Option<Duration>) -> Duration {
    now.saturating_sub(held.map_or(since, |held| held.max(since)))
}

impl GlobalDispatch<ExtIdleNotifierV1, ()> for Solium {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ExtIdleNotifierV1>,
        _data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ExtIdleNotifierV1, ()> for Solium {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &ExtIdleNotifierV1,
        request: ext_idle_notifier_v1::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let (id, timeout, ignores_inhibitors) = match request {
            ext_idle_notifier_v1::Request::GetIdleNotification { id, timeout, .. } => {
                (id, timeout, false)
            }
            ext_idle_notifier_v1::Request::GetInputIdleNotification { id, timeout, .. } => {
                (id, timeout, true)
            }
            ext_idle_notifier_v1::Request::Destroy => return,
            _ => return,
        };
        let object = data_init.init(id, ());
        state.idle.notifications.push(Notification {
            object,
            timeout: Duration::from_millis(u64::from(timeout)),
            ignores_inhibitors,
            // Not idle until told otherwise, which is what the client assumes
            // too: the protocol has no "you are already idle" at creation, so
            // a zero timeout is answered by `settle` on the very next frame
            // rather than by guessing here.
            idle: false,
        });
    }
}

impl Dispatch<ExtIdleNotificationV1, ()> for Solium {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &ExtIdleNotificationV1,
        _request: ext_idle_notification_v1::Request,
        _data: &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        // The only request is `destroy`, and the object being gone is what
        // `settle` already watches for.
    }
}

impl IdleInhibitHandler for Solium {
    fn inhibit(&mut self, surface: WlSurface) {
        if !self.idle.inhibitors.contains(&surface) {
            self.idle.inhibitors.push(surface);
        }
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        self.idle.inhibitors.retain(|each| each != &surface);
    }
}

smithay::delegate_idle_inhibit!(Solium);

/// Register `zwp_idle_inhibit_manager_v1`.
pub(crate) fn inhibit_state(display: &DisplayHandle) -> IdleInhibitManagerState {
    IdleInhibitManagerState::new::<Solium>(display)
}

/// Not `GlobalDispatch`/`Dispatch` written out for the seat argument: the
/// protocol carries a `wl_seat` on every notification and Solium has exactly
/// one seat, so filtering by it would be a comparison that is always true.
/// Named here so the omission is a decision rather than an oversight — the day
/// a second seat exists, `Notification` grows a `WlSeat` and `settle` groups
/// by it.
#[expect(dead_code, reason = "documents the single-seat assumption")]
type OneSeatOnly = WlSeat;
