//! logind's `Lock` and sleep requests (#153).
//!
//! Two gaps, both on the system bus rather than Wayland, so nothing in
//! `lock.rs` or `idle.rs` ever saw them:
//!
//! * `loginctl lock-session`, a lid switch handled by logind, or a power menu
//!   that locks that way, sends `org.freedesktop.login1.Session.Lock`. Until
//!   this module, Solium had nothing listening for it.
//! * Before suspending, logind gives every "delay" inhibitor a chance to
//!   finish what it is doing; without one of its own, Solium had no way to
//!   make sure the screen is locked before the machine sleeps, which is what
//!   `swayidle -w before-sleep 'swaylock -f'` has always been needed for.
//!
//! ## What this module does
//!
//! On `Lock`, it locks the session the way [`Solium::lock_now`](crate::state::Solium::lock_now) does for every
//! other caller (`sol.lock_now`, the shipped lock key, `idle.lock_after`):
//! with Solium's own lock while `lock.native` is in effect, which it is
//! unless `lock.command` names a locker
//! (`a_lock_signal_locks_natively_unless_a_locker_is_set`), and otherwise by
//! running `lock.command` (`sol.lock{ command = "swaylock -f" }`), unless the
//! session already has a lock object
//! (`a_lock_signal_while_already_locked_runs_nothing`) — running a second
//! locker would only hear `finished` and exit, see `lock.rs`.
//!
//! While `lock.before_sleep` is on (the default), it holds a logind delay
//! inhibitor for `"sleep"` at every moment the session is unlocked. On
//! `PrepareForSleep(true)` it locks the same way, but with Solium's own lock
//! fully up at once, with no entry and any entry under way cut short
//! (`a_lock_before_sleep_has_no_entry_and_cuts_one_short`), and releases the
//! inhibitor — which is what lets the machine actually suspend — the moment
//! [`Solium::confirm_lock`](crate::state::Solium::confirm_lock) reaches the presentation-accurate `locked` event,
//! the same moment for Solium's own lock as for a locker's
//! (`before_sleep_the_inhibitor_waits_for_the_native_lock_s_locked`,
//! `prepare_for_sleep_locks_before_the_inhibitor_is_released`), or when
//! `InhibitDelayMaxUSec` runs out first, so a locker that never locks does not
//! hold sleep for ever
//! (`a_locker_that_never_locks_releases_the_inhibitor_on_logind_s_own_timeout`).
//! With nothing to lock the session at all (`lock.native = false` and no
//! `lock.command`), it lets go at once
//! (`with_nothing_to_lock_with_sleep_is_not_held`).
//! The inhibitor is retaken once the session unlocks again or the machine
//! wakes, whichever comes first — both are what being unlocked while awake
//! means, so one hook (`Logind::locked`) covers both.
//!
//! On `Unlock` (`loginctl unlock-session`, from another console or over
//! SSH), it ends Solium's own lock, which is the administrator's way out of
//! it (`an_unlock_signal_unlocks_a_native_lock`), unless `lock.logind_unlock`
//! is false: then only the password ends it, for a machine where whoever can
//! reach logind should not be able to unlock this session
//! (`with_logind_unlock_off_an_unlock_signal_is_ignored`). A lock a client holds is
//! left to that client: the protocol gives only the holder the unlock, and a
//! crashed lock client is recovered by starting another, which takes over.
//!
//! The session's `LockedHint` is kept in step with the same two moments
//! (`locked_hint_follows_lock_and_unlock`), so anything else watching
//! `org.freedesktop.login1.Session` — `loginctl`, a greeter — agrees with
//! Solium about whether it is locked.
//!
//! `lock.command` has no default: Solium's own lock is the session's unless
//! one is named, and a machine without a locker installed should not have
//! Solium try to run one every time the lid closes. `swayidle`'s own
//! `before-sleep` keeps working exactly as before — this and it both taking a
//! delay inhibitor only means the lock screen may be asked for twice, and
//! `lock.rs` already answers a second ask with `finished`.
//!
//! ## Off the main thread
//!
//! A round trip to the system bus must never hold a frame, so every call this
//! module makes — `Inhibit`, `SetLockedHint`, the property read for
//! `InhibitDelayMaxUSec` — runs on a thread of its own, as `session.rs` and
//! `screensaver.rs` already do. Releasing the inhibitor needs no call at all:
//! it is a file descriptor, and closing it (dropping the [`OwnedFd`]) is what
//! tells logind to let sleep proceed, so [`settle`] can do that straight from
//! the main thread. What is read back — `Lock`, `PrepareForSleep`, and the fd
//! and timeout a request for the inhibitor comes back with — arrives as an
//! [`Event`] on a queue [`settle`] drains once a frame: "the event loop only
//! reads what that thread has written", as `idle.rs` puts it for
//! `screensaver.rs`.
//!
//! One persistent thread listens for signals once connected and never makes a
//! call itself; asking for a fresh inhibitor, or setting `LockedHint`, each
//! spawns a short-lived thread of their own instead of being funnelled through
//! the listener, which would otherwise have to watch a channel and the bus at
//! once. Both kinds of call are rare — once at start, once around a sleep —
//! so a fresh connection each time costs nothing worth avoiding the
//! complication for.
//!
//! ## Tests
//!
//! Against a private bus of the test's own, with a fake `Manager` and
//! `Session` standing in for logind: never the real system bus, so a test can
//! never lock the real session or reach real `Inhibit`/`SetLockedHint` calls.
//! The fake's `Inhibit` hands back one end of a `UnixStream` pair and keeps
//! the other, so a test can tell a real fd was released — Solium closing its
//! end — from one that is merely unused, the same guarantee the real logind
//! depends on.
//!
//! The fake bus also routes `Lock` and `PrepareForSleep` the way a real one
//! does: only to a connection whose own `AddMatch` asked for it
//! (`Driver::add_match` records each connection's rules, in the test module).
//! A test waits for that subscription before sending, rather than for a
//! connection merely existing, so it tests whether a truly subscribed
//! listener hears a signal, not whether a sleep happened to be long enough.
//! #225's own regression tests go further still: an armed `Trap` sends the
//! signal from inside the matching `AddMatch` call itself, before that call
//! replies, landing it in the exact window `listen` must not drop it in,
//! deterministically rather than by racing for it.

use std::{
    collections::VecDeque,
    os::fd::OwnedFd,
    process::Stdio,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use zbus::zvariant::OwnedObjectPath;

use crate::session::{Place, speaks};

const DESTINATION: &str = "org.freedesktop.login1";
const MANAGER_PATH: &str = "/org/freedesktop/login1";
const MANAGER_INTERFACE: &str = "org.freedesktop.login1.Manager";
const SESSION_INTERFACE: &str = "org.freedesktop.login1.Session";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const DBUS: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";

/// How long to wait for the session path to be resolved before giving up on a
/// `SetLockedHint` call: the listener resolves it in its first moments, so
/// this is only ever worth anything for a call made in the instant after
/// `begin`. `locked_hint_follows_lock_and_unlock` does not need it to be
/// short, only bounded.
const SESSION_WAIT: Duration = Duration::from_secs(2);

/// `lock.*` in the configuration: logind's `Lock` and sleep signals, handed
/// over by `sol.lock`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Settings {
    /// What to run on `Lock`, and before sleep: `swaylock -f`, say. Split on
    /// whitespace, with no quoting, like an autostart `Exec=` line. `None`
    /// runs nothing — see the module documentation for why that, and not
    /// some bundled locker, is the default.
    pub(crate) command: Option<String>,
    /// Hold sleep for the session to lock first. On by default: the whole
    /// point of this module is that a laptop lid closing locks the screen,
    /// and that should not need finding a setting first.
    pub(crate) before_sleep: bool,
    /// Whether logind's `Unlock` ends Solium's own lock: `lock.logind_unlock`.
    /// On by default, as the way out when the password cannot be checked.
    pub(crate) logind_unlock: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            command: None,
            before_sleep: true,
            logind_unlock: true,
        }
    }
}

/// What a background thread has seen, read once a frame by [`settle`].
#[derive(Debug)]
enum Event {
    /// logind's `Lock` signal.
    Lock,
    /// logind's `Unlock` signal.
    Unlock,
    /// `PrepareForSleep`: `true` just before sleep, `false` on waking.
    PrepareForSleep(bool),
    /// The answer to asking for a fresh delay inhibitor: the fd held, if
    /// logind granted one, and `InhibitDelayMaxUSec` as it stands now.
    Inhibitor(Option<OwnedFd>, Duration),
}

type Events = Arc<Mutex<VecDeque<Event>>>;
type SessionPath = Arc<Mutex<Option<OwnedObjectPath>>>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Which bus to speak on, as `session::speaks` decides it: never the real
/// system bus from a nested run or a manual `--tty`, only the one
/// `SOLIUM_LOGIND_BUS` names, so a test or a development session can never
/// reach the real logind. `a_nested_run_hears_nothing_unless_it_is_given_a_bus`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Bus {
    #[default]
    Nowhere,
    On(Option<String>),
}

/// logind's `Lock` and sleep signals, off until [`Logind::begin`].
#[derive(Debug, Default)]
pub(crate) struct Logind {
    settings: Settings,
    bus: Bus,
    events: Events,
    session_path: SessionPath,
    /// Held for as long as the session may sleep without the lock being
    /// confirmed first. Dropping it is the release: see the module
    /// documentation.
    inhibitor: Option<OwnedFd>,
    /// Bounds how long `inhibitor` is held once sleep is imminent, so a
    /// locker that never locks does not hold sleep for ever. `None` outside
    /// that window: the ordinary "held while unlocked" state has nothing to
    /// time out.
    release_by: Option<Instant>,
    /// `InhibitDelayMaxUSec`, as the most recent `Inhibit` found it. Five
    /// seconds, logind's own default, until the first answer arrives.
    max_delay: Duration,
}

impl Logind {
    /// Before a backend has said anything: hears nothing, and
    /// [`locked`](Logind::locked) and [`configure`](Logind::configure) are
    /// cheap no-ops.
    pub(crate) fn off() -> Self {
        Self {
            max_delay: Duration::from_secs(5),
            ..Self::default()
        }
    }

    /// The backend has started, with the configuration already read:
    /// `bus` is `SOLIUM_LOGIND_BUS`, read the same way `session.rs` reads
    /// `SOLIUM_SESSION_BUS`.
    pub(crate) fn begin(&mut self, place: Place, bus: Option<String>, settings: Settings) {
        *self = Self::off();
        self.settings = settings;
        if !speaks(place, bus.as_deref()) {
            tracing::info!(
                ?place,
                "not started as the session: logind's Lock and sleep signals are not heard. \
                 SOLIUM_LOGIND_BUS names a bus to hear them on instead"
            );
            self.bus = Bus::Nowhere;
            return;
        }
        self.bus = Bus::On(bus.clone());
        spawn_listener(bus, self.events.clone(), self.session_path.clone());
        if self.settings.before_sleep {
            self.request_inhibitor();
        }
    }

    /// `lock.*`, applied at start and on every reload.
    ///
    /// A reload that turns `before_sleep` on, having started without it, asks
    /// for the inhibitor it did not have — unless the session is already
    /// locked, in which case nothing will ever call [`Logind::locked`] to
    /// release it (see `should_request_inhibitor_on_configure`). One that
    /// turns it off lets go of whatever it was holding. Neither tears down or
    /// restarts the listener: `Lock` is heard either way, since running the
    /// locker does not depend on `before_sleep` at all.
    ///
    /// `locked` is the caller's own `Solium::lock.is_some()`: this module
    /// cannot read that itself, `state/commands.rs` hands it over.
    /// The settings in effect.
    pub(crate) fn settings(&self) -> &Settings {
        &self.settings
    }

    pub(crate) fn configure(&mut self, settings: Settings, locked: bool) {
        let had_before_sleep = self.settings.before_sleep;
        self.settings = settings;
        if matches!(self.bus, Bus::Nowhere) {
            return;
        }
        if should_request_inhibitor_on_configure(
            had_before_sleep,
            self.settings.before_sleep,
            self.inhibitor.is_some(),
            locked,
        ) {
            self.request_inhibitor();
        } else if !self.settings.before_sleep && had_before_sleep {
            self.inhibitor = None;
            self.release_by = None;
        }
    }

    fn request_inhibitor(&self) {
        if let Bus::On(address) = &self.bus {
            spawn_inhibit(address.clone(), self.events.clone());
        }
    }

    /// The presentation-accurate lock state changed:
    /// [`Solium::confirm_lock`](crate::state::Solium) calls this the moment
    /// it sends `locked`, and `lock.rs`'s `unlock` calls it the moment the
    /// session unlocks.
    ///
    /// Locked: whatever sleep was waiting for has happened, so the inhibitor
    /// — if one is held — is released at once
    /// (`prepare_for_sleep_locks_before_the_inhibitor_is_released`). Unlocked:
    /// the session is awake and open again, which is exactly when sleep
    /// should once more wait for it to lock first, so a fresh inhibitor is
    /// asked for if `before_sleep` is on and none is held — covering both an
    /// ordinary unlock and waking with nothing left to release.
    ///
    /// `LockedHint` follows either way: `locked_hint_follows_lock_and_unlock`.
    pub(crate) fn locked(&mut self, locked: bool) {
        if matches!(self.bus, Bus::Nowhere) {
            return;
        }
        if locked {
            self.inhibitor = None;
            self.release_by = None;
        } else if self.settings.before_sleep && self.inhibitor.is_none() {
            self.request_inhibitor();
        }
        if let Bus::On(address) = &self.bus {
            spawn_set_locked_hint(address.clone(), self.session_path.clone(), locked);
        }
    }

    /// Whether a delay inhibitor for sleep is held right now.
    #[cfg(test)]
    fn holding(&self) -> bool {
        self.inhibitor.is_some()
    }

    /// Whether anything is heard at all.
    #[cfg(test)]
    fn is_off(&self) -> bool {
        matches!(self.bus, Bus::Nowhere)
    }
}

/// Drain what the background threads have seen, and act on it: run the
/// locker for `Lock` and for sleep, store a fresh inhibitor as it arrives, and
/// give up on one whose locker never confirmed the lock.
///
/// Called once a frame beside `idle::settle`, in `tty.rs`, `winit.rs` and the
/// test harness in `state/tests.rs`.
pub(crate) fn settle(state: &mut crate::state::Solium) {
    let drained: Vec<Event> = {
        let mut events = lock(&state.logind.events);
        events.drain(..).collect()
    };
    for event in drained {
        match event {
            Event::Lock => on_lock(state),
            Event::Unlock => on_unlock(state),
            Event::PrepareForSleep(true) => on_sleep_imminent(state),
            Event::PrepareForSleep(false) => on_resumed(state),
            Event::Inhibitor(fd, max_delay) => {
                state.logind.max_delay = max_delay;
                if fd.is_none() {
                    tracing::warn!(
                        "no delay inhibitor for sleep: logind will not wait for Solium to lock \
                         the session before suspending"
                    );
                }
                state.logind.inhibitor = fd;
            }
        }
    }
    if state.logind.inhibitor.is_some()
        && state
            .logind
            .release_by
            .is_some_and(|deadline| Instant::now() >= deadline)
    {
        tracing::warn!(
            "the locker never confirmed a lock within logind's InhibitDelayMaxUSec; letting \
             sleep proceed unlocked"
        );
        state.logind.inhibitor = None;
        state.logind.release_by = None;
    }
}

/// Whether `Lock` or sleep should run the locker: not over a session that
/// already has a lock object, pending or confirmed — a second one would only
/// be told `finished` (`lock.rs`) and exit. Its own function so the decision
/// can be tested without a real `ext_session_lock_v1` object, which only a
/// protocol handshake produces: `a_lock_signal_while_already_locked_runs_nothing`.
fn should_run_locker(already_locked: bool) -> bool {
    !already_locked
}

/// Whether waking up should ask logind for a fresh inhibitor: `before_sleep`
/// wants one held, none is currently held, and the session is not locked.
///
/// The last check is the one that matters: a lid opened on a still-locked
/// session (nobody typed the password) will never call [`Logind::locked`]
/// again for this lock, since `confirm_lock`'s own confirmation fires at most
/// once per lock instance — so an inhibitor requested here would dangle until
/// `InhibitDelayMaxUSec`, holding every later sleep attempt off for no
/// reason and logging settle's "never confirmed" warning falsely. Its own
/// function for the same reason as [`should_run_locker`]: a real
/// `Solium::lock` needs a protocol handshake no unit test here can stand in
/// for.
/// `on_resumed_does_not_request_an_inhibitor_over_a_still_locked_session`.
fn should_request_inhibitor_on_resume(before_sleep: bool, held: bool, locked: bool) -> bool {
    before_sleep && !held && !locked
}

/// Whether a config reload should ask logind for a fresh inhibitor: the same
/// decision as [`should_request_inhibitor_on_resume`], for the moment
/// `before_sleep` flips from off to on instead of a wake. A reload while the
/// session is already locked has nothing that will ever release what it took,
/// same as the resume case.
/// `a_config_reload_does_not_request_an_inhibitor_over_a_locked_session`.
fn should_request_inhibitor_on_configure(
    had_before_sleep: bool,
    before_sleep: bool,
    held: bool,
    locked: bool,
) -> bool {
    before_sleep && !had_before_sleep && !held && !locked
}

/// `Lock`: lock the session, as [`Solium::lock_now`](crate::state::Solium::lock_now) does.
/// `a_lock_signal_locks_natively_unless_a_locker_is_set`,
/// `a_lock_signal_runs_the_configured_locker_once`,
/// `a_lock_signal_while_already_locked_runs_nothing`.
fn on_lock(state: &mut crate::state::Solium) {
    tracing::info!("logind's Lock signal");
    state.lock_now();
}

impl crate::state::Solium {
    /// Lock the session the way the configuration says: Solium's own lock
    /// while `lock.native` is in effect, and otherwise `lock.command`'s
    /// locker, unless the session already has a lock. The one way to lock
    /// that logind's `Lock`, the moment before sleep, `idle.lock_after`,
    /// `sol.lock_now` and the shipped lock key all share.
    ///
    /// Only ever locks: over a session already locked, natively or by a
    /// client, it changes nothing, and nothing here can unlock
    /// (`no_lua_function_unlocks_a_native_lock`).
    ///
    /// Whether a `locked` is still to come from it: a native lock or a lock
    /// client's that has not shown on every monitor yet, or a locker just
    /// started. That is what decides whether sleep waits for one.
    pub(crate) fn lock_now(&mut self) -> bool {
        self.lock_now_with(true)
    }

    /// Lock as [`Self::lock_now`] does, but with Solium's own lock fully up
    /// at once: no entry, and an entry already playing cut short. The lock
    /// taken because the machine is about to sleep, which waits for no sheet
    /// coming down. `a_lock_before_sleep_has_no_entry_and_cuts_one_short`.
    pub(crate) fn lock_now_at_once(&mut self) -> bool {
        self.lock_now_with(false)
    }

    fn lock_now_with(&mut self, entry: bool) -> bool {
        let started = if self.lock_scenes.settings().native {
            if entry {
                self.lock_natively();
            } else {
                self.lock_natively_at_once();
            }
            false
        } else if should_run_locker(self.lock.is_some()) {
            run_locker(self)
        } else {
            tracing::info!("asked to lock: the session is already locked");
            false
        };
        started
            || self
                .lock
                .as_ref()
                .is_some_and(|lock| lock.pending() && !lock.abandoned())
    }
}

/// `Unlock`: end Solium's own lock, the administrator's way out of it,
/// unless `lock.logind_unlock` is false. A lock a client holds is that
/// client's to end, so it is left alone.
/// `an_unlock_signal_unlocks_a_native_lock`,
/// `with_logind_unlock_off_an_unlock_signal_is_ignored`.
fn on_unlock(state: &mut crate::state::Solium) {
    match state.lock.as_ref() {
        None => tracing::info!("logind's Unlock signal: the session is not locked"),
        Some(lock) if lock.is_native() && !state.logind.settings.logind_unlock => {
            tracing::warn!(
                "logind's Unlock signal: ignored, as lock.logind_unlock is false; only the password ends Solium's own lock"
            );
        }
        Some(lock) if lock.is_native() => {
            tracing::info!("logind's Unlock signal: ending Solium's own lock");
            state.unlock_natively();
        }
        Some(_) => tracing::info!(
            "logind's Unlock signal: the session's lock is a lock client's, and only it may end it"
        ),
    }
}

/// `PrepareForSleep(true)`: lock the session as [`Solium::lock_now`](crate::state::Solium::lock_now) does,
/// then either bound how long the inhibitor may hold sleep (a `locked` is
/// still to come, from Solium's own lock or a locker's), or let go of it at
/// once — held or not, there is nothing left to wait for once this returns.
/// `before_sleep_the_inhibitor_waits_for_the_native_lock_s_locked`,
/// `prepare_for_sleep_locks_before_the_inhibitor_is_released`,
/// `a_locker_that_never_locks_releases_the_inhibitor_on_logind_s_own_timeout`,
/// `with_nothing_to_lock_with_sleep_is_not_held`.
fn on_sleep_imminent(state: &mut crate::state::Solium) {
    if !state.logind.settings.before_sleep {
        return;
    }
    // Fully up at once: the machine is about to sleep, and a sheet coming
    // down would only hold it off.
    let coming = state.lock_now_at_once();
    if state.logind.inhibitor.is_none() {
        return;
    }
    if coming {
        state.logind.release_by = Some(Instant::now() + state.logind.max_delay);
    } else {
        // Nothing is going to confirm a lock: the session is locked already,
        // or nothing is set to lock it. Holding sleep off would only ever end
        // on the timeout, which is the same outcome with a wait in front of
        // it.
        state.logind.inhibitor = None;
    }
}

/// `PrepareForSleep(false)`: woken up, with nothing left to release (either
/// it already was, or the inhibitor was never taken to begin with). Ask for a
/// fresh one so the next sleep is held for the lock again, the same as an
/// ordinary unlock does — unless the session woke still locked, in which case
/// nothing will ever unlock it to release what this would take.
/// `on_resumed_does_not_request_an_inhibitor_over_a_still_locked_session`.
fn on_resumed(state: &mut crate::state::Solium) {
    state.logind.release_by = None;
    if should_request_inhibitor_on_resume(
        state.logind.settings.before_sleep,
        state.logind.inhibitor.is_some(),
        state.lock.is_some(),
    ) {
        state.logind.request_inhibitor();
    }
}

/// Run `lock.command`, split on whitespace with no quoting, as a client of
/// this compositor's socket — the same environment `Solium::spawn` gives a
/// program, but without a pane: a locker asks for `ext_session_lock_v1`, never
/// an `xdg_toplevel`, so there is no window to place or wait for. Whether it
/// started.
fn run_locker(state: &crate::state::Solium) -> bool {
    let Some(command) = state.logind.settings.command.as_deref() else {
        tracing::warn!(
            "asked to lock the session, but lock.native is false and lock.command is not set: \
             nothing locks it"
        );
        return false;
    };
    let mut parts = command.split_whitespace();
    let Some(program) = parts.next() else {
        tracing::warn!("lock.command is blank; nothing runs");
        return false;
    };
    let args: Vec<&str> = parts.collect();

    let mut process = crate::launch::command(program);
    process
        .args(&args)
        .env("WAYLAND_DISPLAY", &state.socket_name)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    match state.x11_display {
        Some(number) => {
            process.env("DISPLAY", format!(":{number}"));
        }
        None => {
            process.env_remove("DISPLAY");
        }
    }
    match process.spawn() {
        Ok(_) => {
            tracing::info!(command, "ran the locker (lock.command)");
            true
        }
        Err(err) => {
            tracing::warn!(?err, command, "could not start the locker (lock.command)");
            false
        }
    }
}

/// A connection to `bus`, or to the system bus for `None`: logind's own.
/// `tests_never_reach_the_system_bus` keeps a test from asking the developer's
/// real logind about their real session.
fn connect(bus: Option<&str>) -> zbus::Result<zbus::blocking::Connection> {
    let builder = match bus {
        Some(address) => zbus::blocking::connection::Builder::address(address)?,
        #[cfg(test)]
        None => {
            return Err(zbus::Error::Address(
                "tests never use the system bus".to_owned(),
            ));
        }
        #[cfg(not(test))]
        None => zbus::blocking::connection::Builder::system()?,
    };
    builder.build()
}

/// This session, by `$XDG_SESSION_ID` if logind gave it one (a login
/// manager's job), or by this process's own pid otherwise — a `solium --tty
/// --session` started from a text console, say, where nothing has set it.
fn resolve_session(connection: &zbus::blocking::Connection) -> zbus::Result<OwnedObjectPath> {
    let reply = match std::env::var("XDG_SESSION_ID").ok() {
        Some(id) => connection.call_method(
            Some(DESTINATION),
            MANAGER_PATH,
            Some(MANAGER_INTERFACE),
            "GetSession",
            &(id,),
        )?,
        None => connection.call_method(
            Some(DESTINATION),
            MANAGER_PATH,
            Some(MANAGER_INTERFACE),
            "GetSessionByPID",
            &(std::process::id(),),
        )?,
    };
    reply.body().deserialize()
}

/// Hear `interface.member`, from `DESTINATION` alone — a local client could
/// send either signal itself, and the match rule's `sender` is what keeps it
/// from being believed, the same reasoning `session.rs`'s own watch gives.
fn add_match(
    connection: &zbus::blocking::Connection,
    interface: &str,
    member: &str,
    path: &str,
) -> zbus::Result<()> {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(DESTINATION)?
        .path(path)?
        .interface(interface)?
        .member(member)?
        .build();
    connection
        .call_method(
            Some(DBUS),
            DBUS_PATH,
            Some(DBUS),
            "AddMatch",
            &rule.to_string(),
        )
        .map(drop)
}

/// `InhibitDelayMaxUSec`, or five seconds — logind's own default — if it
/// cannot be read: asked anyway, on the same reasoning `session.rs`'s `claim`
/// gives for a systemd that will not answer.
fn max_inhibit_delay(connection: &zbus::blocking::Connection) -> Duration {
    let fallback = Duration::from_secs(5);
    let reply = match connection.call_method(
        Some(DESTINATION),
        MANAGER_PATH,
        Some(PROPERTIES),
        "Get",
        &(MANAGER_INTERFACE, "InhibitDelayMaxUSec"),
    ) {
        Ok(reply) => reply,
        Err(err) => {
            tracing::warn!(
                ?err,
                "could not read InhibitDelayMaxUSec; assuming 5 seconds"
            );
            return fallback;
        }
    };
    match reply
        .body()
        .deserialize::<zbus::zvariant::OwnedValue>()
        .ok()
        .and_then(|value| u64::try_from(value).ok())
    {
        Some(micros) => Duration::from_micros(micros),
        None => fallback,
    }
}

/// `Inhibit("sleep", …, "delay")`, and the property beside it: both read
/// together so a request for a fresh inhibitor always reports the limit as it
/// stands now, rather than a value cached from whenever the first one asked.
fn inhibit(connection: &zbus::blocking::Connection) -> zbus::Result<(OwnedFd, Duration)> {
    let max_delay = max_inhibit_delay(connection);
    let reply = connection.call_method(
        Some(DESTINATION),
        MANAGER_PATH,
        Some(MANAGER_INTERFACE),
        "Inhibit",
        &("sleep", "Solium", "locking before sleep", "delay"),
    )?;
    let fd: zbus::zvariant::OwnedFd = reply.body().deserialize()?;
    Ok((fd.into(), max_delay))
}

/// Ask for a fresh delay inhibitor off the main thread, and post what came
/// back — held or not, and the timeout to use — as an [`Event::Inhibitor`].
fn spawn_inhibit(address: Option<String>, events: Events) {
    let spawned = std::thread::Builder::new()
        .name("solium-logind-inhibit".to_owned())
        .spawn({
            let events = events.clone();
            move || {
                let event =
                    match connect(address.as_deref()).and_then(|connection| inhibit(&connection)) {
                        Ok((fd, max_delay)) => Event::Inhibitor(Some(fd), max_delay),
                        Err(err) => {
                            tracing::warn!(?err, "logind refused the delay inhibitor for sleep");
                            Event::Inhibitor(None, Duration::from_secs(5))
                        }
                    };
                lock(&events).push_back(event);
            }
        });
    if let Err(err) = spawned {
        tracing::warn!(?err, "no thread to take the delay inhibitor for sleep");
        lock(&events).push_back(Event::Inhibitor(None, Duration::from_secs(5)));
    }
}

/// `Session.SetLockedHint`, off the main thread. Waits briefly for the
/// listener to have resolved the session path — only ever more than an
/// instant for a call made the moment `begin` returns.
fn spawn_set_locked_hint(address: Option<String>, session_path: SessionPath, value: bool) {
    let spawned = std::thread::Builder::new()
        .name("solium-logind-hint".to_owned())
        .spawn(move || {
            let Some(path) = wait_for_session(&session_path) else {
                tracing::warn!("logind: no session resolved yet; LockedHint was not set");
                return;
            };
            let connection = match connect(address.as_deref()) {
                Ok(connection) => connection,
                Err(err) => {
                    tracing::warn!(?err, "no system bus: LockedHint was not set");
                    return;
                }
            };
            let outcome = connection.call_method(
                Some(DESTINATION),
                path.as_str(),
                Some(SESSION_INTERFACE),
                "SetLockedHint",
                &(value,),
            );
            if let Err(err) = outcome {
                tracing::warn!(?err, value, "logind refused SetLockedHint");
            }
        });
    if let Err(err) = spawned {
        tracing::warn!(?err, "no thread to set LockedHint");
    }
}

fn wait_for_session(session_path: &SessionPath) -> Option<OwnedObjectPath> {
    let deadline = Instant::now() + SESSION_WAIT;
    loop {
        if let Some(path) = lock(session_path).clone() {
            return Some(path);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The persistent listener: connects, starts reading its own
/// `MessageIterator` before anything else, resolves the session, subscribes
/// to `Lock` and `PrepareForSleep`, and then only ever reads — every call
/// this module makes, including the first `Inhibit` (`Logind::begin` asks for
/// it through [`Logind::request_inhibitor`] like any other), is a thread of
/// its own. See the module documentation for why listening and calling are
/// kept apart.
///
/// The iterator is built right after `connect`, before `resolve_session` and
/// `add_match`, rather than after them: a real bus routes a signal to a
/// connection the instant a matching `AddMatch` is registered on it, which
/// is before the call that registered it can have replied, let alone before
/// a second `add_match` and this iterator after it. A `Lock` or
/// `PrepareForSleep` delivered in that window used to have nothing on this
/// connection subscribed to read it yet, and was dropped for good, not
/// merely delayed (#225, `a_lock_sent_while_add_match_is_in_flight_is_not_lost`,
/// `a_prepare_for_sleep_sent_while_add_match_is_in_flight_is_not_lost`).
fn listen(address: Option<String>, events: Events, session_path: SessionPath) {
    let connection = match connect(address.as_deref()) {
        Ok(connection) => connection,
        Err(err) => {
            tracing::warn!(
                ?err,
                bus = address,
                "no system bus: logind's Lock and sleep signals are not heard"
            );
            return;
        }
    };
    let messages = zbus::blocking::MessageIterator::from(&connection);

    let path = match resolve_session(&connection) {
        Ok(path) => path,
        Err(err) => {
            tracing::warn!(
                ?err,
                "could not ask logind which session this is; Lock and sleep signals are not heard"
            );
            return;
        }
    };
    *lock(&session_path) = Some(path.clone());

    if let Err(err) = add_match(&connection, SESSION_INTERFACE, "Lock", path.as_str()) {
        tracing::warn!(?err, "could not hear logind's Lock signal");
    }
    if let Err(err) = add_match(&connection, SESSION_INTERFACE, "Unlock", path.as_str()) {
        tracing::warn!(?err, "could not hear logind's Unlock signal");
    }
    if let Err(err) = add_match(
        &connection,
        MANAGER_INTERFACE,
        "PrepareForSleep",
        MANAGER_PATH,
    ) {
        tracing::warn!(?err, "could not hear logind's PrepareForSleep signal");
    }

    // `messages` has no match rule of its own, so it also sees the method
    // replies to `resolve_session` and both `add_match` calls above, now that
    // it exists before them: those carry no `interface`/`member` header
    // field at all, so the guard below already drops them exactly like
    // everything else this module never asked for.
    for message in messages {
        let Ok(message) = message else {
            continue;
        };
        let header = message.header();
        let (Some(interface), Some(member)) = (header.interface(), header.member()) else {
            continue;
        };
        match (interface.as_str(), member.as_str()) {
            (SESSION_INTERFACE, "Lock") => lock(&events).push_back(Event::Lock),
            (SESSION_INTERFACE, "Unlock") => lock(&events).push_back(Event::Unlock),
            (MANAGER_INTERFACE, "PrepareForSleep") => {
                if let Ok(asleep) = message.body().deserialize::<bool>() {
                    lock(&events).push_back(Event::PrepareForSleep(asleep));
                }
            }
            _ => {}
        }
    }
}

fn spawn_listener(address: Option<String>, events: Events, session_path: SessionPath) {
    let spawned = std::thread::Builder::new()
        .name("solium-logind".to_owned())
        .spawn(move || listen(address, events, session_path));
    if let Err(err) = spawned {
        tracing::warn!(
            ?err,
            "no thread for logind: Lock and sleep signals are not heard"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::{
        os::unix::net::UnixStream,
        path::PathBuf,
        sync::atomic::{AtomicU32, Ordering},
        time::{Duration, Instant},
    };

    use zbus::message::Message;

    use super::*;

    /// Long enough for anything a test waits on to have happened.
    const WITHIN: Duration = Duration::from_secs(5);
    const SESSION_PATH: &str = "/org/freedesktop/login1/session/_3130";

    /// What the stand-in heard, as `member args`.
    type Heard = Arc<Mutex<Vec<String>>>;

    /// The match rules one connection has registered with `AddMatch`, in
    /// registration order, exactly as the rule string arrived: what
    /// `StandIn::broadcast` and `StandIn::wait_for_subscription` check
    /// against, so a signal is routed (and waited for) the way a real bus
    /// would, instead of being fired at every connection regardless of
    /// whether it ever asked for it.
    type Rules = Arc<Mutex<Vec<String>>>;

    /// A one-shot, deliberately mistimed delivery: `Driver::add_match` fires
    /// this the moment it sees a rule naming [`Trap::on_member`], down the
    /// very connection that rule arrived on, *before* that `AddMatch` call
    /// replies. That is the exact window #225's race lives in -- a real bus
    /// may route the signal the instant the match rule is registered, which
    /// is earlier than a reply can possibly reach the caller -- so a test
    /// armed with a `Trap` reproduces it every time, rather than racing for
    /// it. Used by `a_lock_sent_while_add_match_is_in_flight_is_not_lost`
    /// and `a_prepare_for_sleep_sent_while_add_match_is_in_flight_is_not_lost`.
    #[derive(Clone, Copy)]
    enum Trap {
        Lock,
        PrepareForSleep(bool),
    }

    impl Trap {
        /// The `AddMatch` rule this trap waits for: a substring of the rule
        /// string, the same check [`StandIn::subscribed`] makes.
        fn on_member(self) -> &'static str {
            match self {
                Trap::Lock => "Lock",
                Trap::PrepareForSleep(_) => "PrepareForSleep",
            }
        }

        fn message(self) -> zbus::message::Message {
            match self {
                Trap::Lock => lock_message(),
                Trap::PrepareForSleep(asleep) => prepare_for_sleep_message(asleep),
            }
        }
    }

    /// `org.freedesktop.DBus`, just enough to let a `blocking::Connection`
    /// finish its handshake and register match rules: `session.rs`'s own
    /// `StandInDriver` needs exactly this much and no more.
    ///
    /// Unlike a no-op `AddMatch`, this one records every rule its connection
    /// registers into `rules`, so the rest of the stand-in can tell a
    /// connection that has subscribed to a signal from one that merely
    /// exists, and fires an armed `trap` (see [`Trap`]) before replying.
    struct Driver {
        rules: Rules,
        trap: Arc<Mutex<Option<Trap>>>,
    }

    #[zbus::interface(name = "org.freedesktop.DBus")]
    impl Driver {
        fn hello(&self) -> String {
            ":1.1".to_owned()
        }

        async fn add_match(&self, rule: String, #[zbus(connection)] connection: &zbus::Connection) {
            let fire = {
                let mut slot = self.trap.lock().expect("the trap");
                match *slot {
                    Some(trap) if rule.contains(&format!("member='{}'", trap.on_member())) => {
                        slot.take()
                    }
                    _ => None,
                }
            };
            if let Some(trap) = fire {
                // #225: sent from inside the handler for this very
                // `AddMatch`, before the call replies -- the window
                // production's `listen()` used to create its
                // `MessageIterator` too late to catch.
                let _ = connection.send(&trap.message()).await;
            }
            self.rules.lock().expect("the rules").push(rule);
        }
    }

    /// `org.freedesktop.login1.Manager`: resolves to [`SESSION_PATH`] always,
    /// answers `InhibitDelayMaxUSec` from `max_delay_micros`, and `Inhibit`
    /// with one end of a `UnixStream` pair, keeping the other in `held` so a
    /// test can tell whether Solium has let it go.
    struct FakeManager {
        heard: Heard,
        max_delay_micros: u64,
        held: Arc<Mutex<Vec<UnixStream>>>,
        refuses_inhibit: bool,
    }

    #[zbus::interface(name = "org.freedesktop.login1.Manager")]
    impl FakeManager {
        fn get_session(&self, id: String) -> zbus::fdo::Result<OwnedObjectPath> {
            self.heard
                .lock()
                .expect("the list")
                .push(format!("GetSession {id}"));
            OwnedObjectPath::try_from(SESSION_PATH)
                .map_err(|err| zbus::fdo::Error::Failed(err.to_string()))
        }
        #[zbus(name = "GetSessionByPID")]
        fn get_session_by_pid(&self, pid: u32) -> zbus::fdo::Result<OwnedObjectPath> {
            self.heard
                .lock()
                .expect("the list")
                .push(format!("GetSessionByPID {pid}"));
            OwnedObjectPath::try_from(SESSION_PATH)
                .map_err(|err| zbus::fdo::Error::Failed(err.to_string()))
        }
        #[zbus(property, name = "InhibitDelayMaxUSec")]
        fn inhibit_delay_max_usec(&self) -> u64 {
            self.max_delay_micros
        }
        fn inhibit(
            &self,
            what: String,
            who: String,
            why: String,
            mode: String,
        ) -> zbus::fdo::Result<zbus::zvariant::OwnedFd> {
            self.heard
                .lock()
                .expect("the list")
                .push(format!("Inhibit {what} {who} {why} {mode}"));
            if self.refuses_inhibit {
                return Err(zbus::fdo::Error::Failed(
                    "no delay inhibitors today".to_owned(),
                ));
            }
            let (ours, kept) = UnixStream::pair().expect("a socket pair");
            self.held.lock().expect("the held ends").push(kept);
            Ok(OwnedFd::from(ours).into())
        }
    }

    /// `org.freedesktop.login1.Session`: only `SetLockedHint` is ever called
    /// on it.
    struct FakeSession {
        heard: Heard,
    }

    #[zbus::interface(name = "org.freedesktop.login1.Session")]
    impl FakeSession {
        fn set_locked_hint(&self, locked: bool) {
            self.heard
                .lock()
                .expect("the list")
                .push(format!("SetLockedHint {locked}"));
        }
    }

    /// logind's `Lock`, on the session path, as if `loginctl lock-session` or
    /// a lid switch had just asked for it. Its own function, rather than a
    /// closure inline at each call site, so `StandIn::send_lock` and
    /// `Trap::message` build the exact same message.
    fn lock_message() -> zbus::message::Message {
        Message::signal(SESSION_PATH, SESSION_INTERFACE, "Lock")
            .and_then(|builder| builder.sender(":1.50"))
            .and_then(|builder| builder.build(&()))
            .expect("a Lock signal")
    }

    /// logind's `Unlock`, on the session path, as `loginctl unlock-session`
    /// sends it. See [`lock_message`].
    fn unlock_message() -> zbus::message::Message {
        Message::signal(SESSION_PATH, SESSION_INTERFACE, "Unlock")
            .and_then(|builder| builder.sender(":1.50"))
            .and_then(|builder| builder.build(&()))
            .expect("an Unlock signal")
    }

    /// `PrepareForSleep`, on the manager path. See [`lock_message`].
    fn prepare_for_sleep_message(asleep: bool) -> zbus::message::Message {
        Message::signal(MANAGER_PATH, MANAGER_INTERFACE, "PrepareForSleep")
            .and_then(|builder| builder.sender(":1.50"))
            .and_then(|builder| builder.build(&(asleep,)))
            .expect("a PrepareForSleep signal")
    }

    /// A private bus standing in for logind: never the real system bus, and
    /// nothing on it but this test's own fakes. Modelled on `session.rs`'s own
    /// `StandIn` and `screensaver.rs`'s `StandInBus`, but keeping every
    /// connection rather than one: unlike those modules, which hold a single
    /// persistent connection, `logind.rs` opens a fresh one for every
    /// `Inhibit` and `SetLockedHint`, on top of the listener's own long-lived
    /// one, so there is no single "the" peer to send a signal down.
    struct StandIn {
        address: String,
        heard: Heard,
        held: Arc<Mutex<Vec<UnixStream>>>,
        directory: PathBuf,
        connections: Arc<Mutex<Vec<(zbus::blocking::Connection, Rules)>>>,
    }

    impl StandIn {
        fn new(max_delay_micros: u64, refuses_inhibit: bool) -> Self {
            Self::build(max_delay_micros, refuses_inhibit, None)
        }

        /// Like [`StandIn::new`], but every connection's `Driver` is armed
        /// with `trap` (see [`Trap`]): used only by the #225 regression
        /// tests, which need the signal sent from inside a specific
        /// `AddMatch` call rather than from `broadcast`.
        fn new_trapping(max_delay_micros: u64, refuses_inhibit: bool, trap: Trap) -> Self {
            Self::build(max_delay_micros, refuses_inhibit, Some(trap))
        }

        fn build(max_delay_micros: u64, refuses_inhibit: bool, trap: Option<Trap>) -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let directory = std::env::temp_dir().join(format!(
                "solium-logind-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&directory);
            std::fs::create_dir_all(&directory).expect("a temporary directory");
            let path = directory.join("bus");
            let listener =
                std::os::unix::net::UnixListener::bind(&path).expect("binding the socket");
            let heard = Heard::default();
            let held = Arc::new(Mutex::new(Vec::new()));
            let connections = Arc::new(Mutex::new(Vec::new()));
            let trap = Arc::new(Mutex::new(trap));
            {
                let heard = heard.clone();
                let held = held.clone();
                let connections = connections.clone();
                std::thread::spawn(move || {
                    for accepted in listener.incoming() {
                        let Ok(stream) = accepted else {
                            continue;
                        };
                        let heard = heard.clone();
                        let held = held.clone();
                        let connections = connections.clone();
                        let rules = Rules::default();
                        let trap = trap.clone();
                        std::thread::spawn(move || {
                            let Ok(connection) =
                                zbus::blocking::connection::Builder::async_io_unix_stream(stream)
                                    .server(zbus::Guid::generate())
                                    .expect("a server guid")
                                    .p2p()
                                    .serve_at(
                                        DBUS_PATH,
                                        Driver {
                                            rules: rules.clone(),
                                            trap,
                                        },
                                    )
                                    .expect("serving the driver")
                                    .serve_at(
                                        MANAGER_PATH,
                                        FakeManager {
                                            heard: heard.clone(),
                                            max_delay_micros,
                                            held,
                                            refuses_inhibit,
                                        },
                                    )
                                    .expect("serving the manager")
                                    .serve_at(SESSION_PATH, FakeSession { heard })
                                    .expect("serving the session")
                                    .build()
                            else {
                                return;
                            };
                            connections
                                .lock()
                                .expect("the connections")
                                .push((connection.clone(), rules));
                            connection.closed();
                        });
                    }
                });
            }
            Self {
                address: format!("unix:path={}", path.display()),
                heard,
                held,
                directory,
                connections,
            }
        }

        /// Whether some connection's recorded rules would have the bus route
        /// `interface`/`member` to it: a plain substring check, not a real
        /// match-rule parse, but exact for the one shape `add_match` ever
        /// builds (`type='signal',sender='…',interface='…',member='…'`, with
        /// `interface` always written immediately before `member`).
        fn subscribed(rules: &Rules, interface: &str, member: &str) -> bool {
            let needle = format!("interface='{interface}',member='{member}'");
            rules
                .lock()
                .expect("the rules")
                .iter()
                .any(|rule| rule.contains(needle.as_str()))
        }

        /// Every connection currently subscribed to `interface`/`member`.
        fn subscribers(&self, interface: &str, member: &str) -> Vec<zbus::blocking::Connection> {
            self.connections
                .lock()
                .expect("the connections")
                .iter()
                .filter(|(_, rules)| Self::subscribed(rules, interface, member))
                .map(|(connection, _)| connection.clone())
                .collect()
        }

        /// Block until some connection has registered a match rule for
        /// `interface`/`member` -- the listener's own `AddMatch`, in every
        /// test that waits for this before sending a signal, so the send
        /// tests whether a truly subscribed listener hears it, not whether a
        /// sleep happened to be long enough.
        fn wait_for_subscription(&self, interface: &str, member: &str) {
            let subscribed = soon(|| !self.subscribers(interface, member).is_empty());
            assert!(
                subscribed,
                "nothing ever subscribed to interface='{interface}',member='{member}'"
            );
        }

        /// Send `signal` down every connection subscribed to
        /// `interface`/`member`, waiting for at least one -- the same
        /// routing a real bus does, which is exactly what #225's race
        /// depends on. A one-off caller (`Inhibit`, `SetLockedHint`) never
        /// calls `AddMatch` at all, so it is never among them.
        fn broadcast(
            &self,
            interface: &str,
            member: &str,
            build: impl Fn() -> zbus::message::Message,
        ) {
            let deadline = Instant::now() + WITHIN;
            loop {
                let subscribers = self.subscribers(interface, member);
                if !subscribers.is_empty() {
                    for connection in subscribers {
                        let _ = connection.send(&build());
                    }
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "nothing ever subscribed to interface='{interface}',member='{member}'"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        /// logind's `Lock`, on the session path, as if `loginctl lock-session`
        /// or a lid switch had just asked for it.
        fn send_lock(&self) {
            self.broadcast(SESSION_INTERFACE, "Lock", lock_message);
        }

        /// logind's `Unlock`, on the session path, as if
        /// `loginctl unlock-session` had just asked for it.
        fn send_unlock(&self) {
            self.broadcast(SESSION_INTERFACE, "Unlock", unlock_message);
        }

        /// `PrepareForSleep`, on the manager path.
        fn send_prepare_for_sleep(&self, asleep: bool) {
            self.broadcast(MANAGER_INTERFACE, "PrepareForSleep", move || {
                prepare_for_sleep_message(asleep)
            });
        }

        /// Every method Solium has called, once it has called one.
        fn requests(&self) -> Vec<String> {
            let deadline = Instant::now() + WITHIN;
            loop {
                let heard = self.heard.lock().expect("the list").clone();
                if !heard.is_empty() || Instant::now() > deadline {
                    return heard;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        /// Whether the one inhibitor this test granted is still held: a
        /// non-blocking read of the end this bus kept finds either nothing
        /// available yet (still held, `WouldBlock`) or EOF, `Ok(0)` (Solium
        /// closed its end). Nothing is ever written down this pair, so a read
        /// is as safe as a peek would be and needs no unstable feature.
        fn inhibitor_held(&self) -> bool {
            use std::io::Read;
            let mut held = self.held.lock().expect("the held ends");
            let Some(kept) = held.first_mut() else {
                return false;
            };
            kept.set_nonblocking(true).expect("non-blocking");
            let mut buffer = [0u8; 1];
            match kept.read(&mut buffer) {
                Ok(0) => false,
                Ok(_) => true,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => true,
                Err(_) => false,
            }
        }
    }

    impl Drop for StandIn {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    /// A `Logind` listening on `bus`, with its locker set to `command` (a
    /// program that just exits, so running it leaves a trace without putting
    /// anything on screen) and whatever `before_sleep` says.
    fn begin(bus: &StandIn, command: Option<&str>, before_sleep: bool) -> Logind {
        let mut logind = Logind::off();
        logind.begin(
            Place::Nested,
            Some(bus.address.clone()),
            Settings {
                command: command.map(str::to_owned),
                before_sleep,
                ..Settings::default()
            },
        );
        logind
    }

    /// A marker file `command` touches, so a test can tell the locker ran
    /// without a real one installed: `touch <path>`.
    fn marker() -> PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        std::env::temp_dir().join(format!(
            "solium-logind-test-marker-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    /// A bare compositor state, with no renderer and no real Wayland client
    /// anywhere near it: enough to give `settle` the fields it reads
    /// (`lock`, `socket_name`, `x11_display`) and nothing else. The same
    /// construction `session.rs`'s own
    /// `a_reload_leaves_the_session_as_it_began` uses.
    ///
    /// `lock.native` as `sol.lock` leaves it when only `command` is given:
    /// Solium's own lock unless a locker is named.
    fn solium_with(logind: Logind) -> crate::state::Solium {
        let native = logind.settings.command.is_none();
        solium_locking(logind, native)
    }

    /// [`solium_with`], with `lock.native` as `native` says.
    fn solium_locking(logind: Logind, native: bool) -> crate::state::Solium {
        let display = smithay::reexports::wayland_server::Display::<crate::state::Solium>::new()
            .expect("a test wayland display");
        let mut state = crate::state::Solium::new(display.handle());
        state.logind = logind;
        state.lock_scenes.configure(crate::lock_scene::Settings {
            native,
            ..crate::lock_scene::Settings::default()
        });
        state
    }

    /// A monitor in `state`'s space, which a lock waits for: until it has
    /// shown a frame built under the lock, `locked` is owed.
    fn monitor(state: &mut crate::state::Solium) -> smithay::output::Output {
        use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
        let output = Output::new(
            "logind-test".to_owned(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "solium".to_owned(),
                model: "logind".to_owned(),
            },
        );
        output.change_current_state(
            Some(Mode {
                size: (1920, 1080).into(),
                refresh: 60_000,
            }),
            None,
            Some(Scale::Fractional(1.0)),
            None,
        );
        state.space.map_output(&output, (0, 0));
        output
    }

    fn natively_locked(state: &crate::state::Solium) -> bool {
        state
            .lock
            .as_ref()
            .is_some_and(crate::lock::Lock::is_native)
    }

    /// Whether `test` comes true within [`WITHIN`].
    fn soon(mut test: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + WITHIN;
        while Instant::now() < deadline {
            if test() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        test()
    }

    /// Never the session a nested window runs inside: that is the
    /// developer's own desktop, and its logind is not ours to ask about.
    /// Only `SOLIUM_LOGIND_BUS`, which `begin`'s caller reads and hands over
    /// as `bus`, tells a nested run anything.
    #[test]
    fn a_nested_run_hears_nothing_unless_it_is_given_a_bus() {
        let mut logind = Logind::off();
        logind.begin(Place::Nested, None, Settings::default());
        assert!(logind.is_off());
        // Nothing to connect to, whatever happens.
        logind.locked(true);
        logind.configure(
            Settings {
                command: Some("true".to_owned()),
                before_sleep: true,
                ..Settings::default()
            },
            false,
        );
    }

    #[test]
    fn tests_never_reach_the_system_bus() {
        assert!(connect(None).is_err());
    }

    /// `Lock` runs the configured locker exactly once.
    /// `a_lock_signal_runs_the_configured_locker_once`.
    #[test]
    fn a_lock_signal_runs_the_configured_locker_once() {
        let bus = StandIn::new(5_000_000, false);
        let mark = marker();
        let logind = begin(&bus, Some(&format!("touch {}", mark.display())), false);
        let mut state = solium_with(logind);

        // Wait for the listener to have actually subscribed, rather than
        // just connected: otherwise this test would be racing the same gap
        // #225 lives in, instead of testing what happens once it is closed.
        bus.wait_for_subscription(SESSION_INTERFACE, "Lock");
        bus.send_lock();
        // `Lock` is heard asynchronously; `settle` is what turns it into a
        // process, so poll both until the marker exists.
        let seen = soon(|| {
            settle(&mut state);
            mark.exists()
        });
        assert!(seen, "the locker never ran");
        assert!(
            state.lock.is_none(),
            "Solium's own lock was taken although a locker is named"
        );
        let _ = std::fs::remove_file(&mark);
    }

    /// **`Lock` takes Solium's own lock unless a locker is named**, and with
    /// `lock.native = true` beside a locker, still Solium's own, the locker
    /// never run: `native` is what decides, and it only defaults to off when
    /// `command` is set.
    #[test]
    fn a_lock_signal_locks_natively_unless_a_locker_is_set() {
        let bus = StandIn::new(5_000_000, false);
        let mut state = solium_with(begin(&bus, None, false));
        bus.wait_for_subscription(SESSION_INTERFACE, "Lock");
        bus.send_lock();
        assert!(
            soon(|| {
                settle(&mut state);
                natively_locked(&state)
            }),
            "logind's Lock did not take Solium's own lock"
        );

        let bus = StandIn::new(5_000_000, false);
        let mark = marker();
        let logind = begin(&bus, Some(&format!("touch {}", mark.display())), false);
        let mut state = solium_locking(logind, true);
        bus.wait_for_subscription(SESSION_INTERFACE, "Lock");
        bus.send_lock();
        assert!(
            soon(|| {
                settle(&mut state);
                natively_locked(&state)
            }),
            "`lock.native = true` beside a locker did not lock natively"
        );
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            !mark.exists(),
            "the locker ran although Solium's own lock is the session's"
        );
        let _ = std::fs::remove_file(&mark);
    }

    /// **`Unlock` ends Solium's own lock**: `loginctl unlock-session` from
    /// another console is the administrator's way out of it, and the one way
    /// out that exists before the password does.
    #[test]
    fn an_unlock_signal_unlocks_a_native_lock() {
        let bus = StandIn::new(5_000_000, false);
        let logind = begin(&bus, None, false);
        let mut state = solium_with(logind);
        state.lock_natively();
        assert!(
            state
                .lock
                .as_ref()
                .is_some_and(crate::lock::Lock::is_native),
            "the premise: natively locked"
        );

        bus.wait_for_subscription(SESSION_INTERFACE, "Unlock");
        bus.send_unlock();
        let unlocked = soon(|| {
            settle(&mut state);
            state.lock.is_none()
        });
        assert!(unlocked, "logind's Unlock did not end the native lock");
    }

    /// **With `lock.logind_unlock` off, `Unlock` is ignored**: the native lock
    /// stays, and only the password ends it.
    #[test]
    fn with_logind_unlock_off_an_unlock_signal_is_ignored() {
        let bus = StandIn::new(5_000_000, false);
        let mut logind = begin(&bus, None, false);
        logind.settings.logind_unlock = false;
        let mut state = solium_with(logind);
        state.lock_natively();
        bus.wait_for_subscription(SESSION_INTERFACE, "Unlock");
        bus.send_unlock();
        // As long as `an_unlock_signal_unlocks_a_native_lock` ever needs, and
        // more.
        let until = Instant::now() + Duration::from_secs(1);
        while Instant::now() < until {
            settle(&mut state);
            assert!(
                state.lock.is_some(),
                "logind's Unlock ended the native lock"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            state
                .lock
                .as_ref()
                .is_some_and(crate::lock::Lock::is_native)
        );
    }

    /// #225: a `Lock` a real bus sends the instant the listener's `AddMatch`
    /// for it is registered -- before that call has even replied -- must
    /// still run the locker. The `Trap` fires from inside that exact
    /// `AddMatch`, so this reaches the production race deterministically
    /// instead of by timing: against the unfixed `listen()`, which created
    /// its `MessageIterator` only after both `add_match` calls returned,
    /// this failed every time, not now and then.
    /// `a_lock_sent_while_add_match_is_in_flight_is_not_lost`.
    #[test]
    fn a_lock_sent_while_add_match_is_in_flight_is_not_lost() {
        let bus = StandIn::new_trapping(5_000_000, false, Trap::Lock);
        let mark = marker();
        let logind = begin(&bus, Some(&format!("touch {}", mark.display())), false);
        let mut state = solium_with(logind);

        // No `send_lock()` here: the stand-in already sent `Lock` on its own,
        // from inside `Driver::add_match`, the moment `begin` made it that
        // far.
        let seen = soon(|| {
            settle(&mut state);
            mark.exists()
        });
        assert!(
            seen,
            "a Lock sent while AddMatch was still in flight was lost"
        );
        let _ = std::fs::remove_file(&mark);
    }

    /// A second `Lock` over Solium's own lock changes nothing: the same lock,
    /// still the one owed to the same monitors, not a fresh one.
    #[test]
    fn a_lock_signal_over_a_native_lock_takes_no_second_one() {
        let bus = StandIn::new(5_000_000, false);
        let mut state = solium_with(begin(&bus, None, false));
        let _monitor = monitor(&mut state);
        state.lock_natively();
        let frame = state.lock_frame();
        assert!(frame.is_some(), "the premise: natively locked");

        bus.wait_for_subscription(SESSION_INTERFACE, "Lock");
        bus.send_lock();
        assert!(
            soon(|| !lock(&state.logind.events).is_empty()),
            "the second Lock was never heard"
        );
        settle(&mut state);
        assert_eq!(
            state.lock_frame(),
            frame,
            "a second Lock replaced the native lock"
        );
    }

    /// `Lock` or sleep must not run a second locker over a session that
    /// already has one: `lock.rs` only ever grants one, and a second asking
    /// would just be told `finished` and exit at once. Pulled out as its own
    /// function (see [`should_run_locker`]) because fabricating a real
    /// `ext_session_lock_v1` object to set `Solium::lock` to `Some` needs a
    /// protocol handshake no unit test here can stand in for.
    /// `a_lock_signal_while_already_locked_runs_nothing`.
    #[test]
    fn a_lock_signal_while_already_locked_runs_nothing() {
        assert!(
            !should_run_locker(true),
            "ran a second locker over one already held"
        );
        assert!(
            should_run_locker(false),
            "refused to lock an unlocked session"
        );
    }

    /// Waking up with the session still locked (the lid closed, slept, and
    /// opened again with nobody typing the password) must not ask for a
    /// fresh inhibitor: `Logind::locked(true)` already fired once for this
    /// lock and will not fire again, so one taken here would dangle until
    /// logind's own timeout. Every other combination still asks, same as
    /// before this check existed.
    /// `on_resumed_does_not_request_an_inhibitor_over_a_still_locked_session`.
    #[test]
    fn on_resumed_does_not_request_an_inhibitor_over_a_still_locked_session() {
        assert!(
            !should_request_inhibitor_on_resume(true, false, true),
            "asked for an inhibitor over a session that woke up still locked"
        );
        assert!(
            should_request_inhibitor_on_resume(true, false, false),
            "refused to ask for an inhibitor on an ordinary, unlocked wake"
        );
        assert!(
            !should_request_inhibitor_on_resume(false, false, false),
            "asked for an inhibitor although before_sleep is off"
        );
        assert!(
            !should_request_inhibitor_on_resume(true, true, false),
            "asked for a second inhibitor while one is already held"
        );
    }

    /// The same dangling-inhibitor gap, on a config reload instead of a wake:
    /// flipping `before_sleep` on while the session is already locked must
    /// not take an inhibitor nothing will ever release.
    /// `a_config_reload_does_not_request_an_inhibitor_over_a_locked_session`.
    #[test]
    fn a_config_reload_does_not_request_an_inhibitor_over_a_locked_session() {
        assert!(
            !should_request_inhibitor_on_configure(false, true, false, true),
            "asked for an inhibitor on a reload over an already-locked session"
        );
        assert!(
            should_request_inhibitor_on_configure(false, true, false, false),
            "refused to ask for an inhibitor on an ordinary before_sleep-on reload"
        );
        assert!(
            !should_request_inhibitor_on_configure(true, true, false, false),
            "asked again although before_sleep was already on"
        );
        assert!(
            !should_request_inhibitor_on_configure(false, true, true, false),
            "asked for a second inhibitor while one is already held"
        );
    }

    /// Before sleep, with a locker configured: the inhibitor is released the
    /// moment [`Logind::locked`] hears the presentation-accurate `locked` —
    /// standing in here for `Solium::confirm_lock`'s real call, which needs a
    /// GPU and a real lock client to reach.
    /// `prepare_for_sleep_locks_before_the_inhibitor_is_released`.
    #[test]
    fn prepare_for_sleep_locks_before_the_inhibitor_is_released() {
        let bus = StandIn::new(5_000_000, false);
        let logind = begin(&bus, Some("true"), true);
        let mut state = solium_with(logind);
        // `state.logind.holding()`, not `bus.inhibitor_held()`: the fake
        // `Inhibit` pushes onto `held` as it builds its reply, which is
        // before Solium's own `Inhibit` call on its own, separate connection
        // has returned and `settle` has drained the `Event::Inhibitor` it
        // posts. Under load, that gap is wide enough that the rest of this
        // test ran against a `state.logind.inhibitor` that was still `None`,
        // so `locked(true)` had nothing to release and the real fd outlived
        // it -- a second, independent cause of the flakiness #225 was filed
        // against, this one in the precondition rather than in `listen()`.
        assert!(
            soon(|| {
                settle(&mut state);
                state.logind.holding()
            }),
            "the initial inhibitor was never taken"
        );

        // As in `a_lock_signal_runs_the_configured_locker_once`: wait for the
        // subscription itself, not just for the unrelated inhibitor request
        // to have gone through on its own, separate connection.
        bus.wait_for_subscription(MANAGER_INTERFACE, "PrepareForSleep");
        bus.send_prepare_for_sleep(true);
        assert!(
            soon(|| {
                settle(&mut state);
                bus.inhibitor_held()
            }),
            "the inhibitor was released before the lock was confirmed"
        );

        // The presentation-accurate moment: `lock.rs`'s `confirm_lock` calls
        // this the instant it sends `locked`.
        state.logind.locked(true);
        // Dropping `state.logind.inhibitor` closes Solium's own end at once,
        // but `on_sleep_imminent` (above, inside the `soon` before this one)
        // also forked `lock.command` itself: the child inherits a duplicate
        // of every open fd until it execs, and close_on_exec_above_stdio
        // (launch.rs) only marks that duplicate close-on-exec in the gap
        // between fork and exec, not before it. Under load, this process not
        // yet having been scheduled far enough to reach its own exec is
        // enough to still see the fd held immediately after this returns --
        // a bare, un-retried assert here was a second, independent cause of
        // the flakiness #225 was filed against.
        assert!(
            soon(|| !bus.inhibitor_held()),
            "the inhibitor outlived the confirmed lock"
        );
    }

    /// #225, for `PrepareForSleep`: the same mistimed delivery as
    /// `a_lock_sent_while_add_match_is_in_flight_is_not_lost`, since
    /// `listen()` registers this `AddMatch` the exact same way, and just as
    /// late. `before_sleep` must be on here -- unlike that test -- since
    /// `on_sleep_imminent` only runs the locker at all when it is.
    /// `a_prepare_for_sleep_sent_while_add_match_is_in_flight_is_not_lost`.
    #[test]
    fn a_prepare_for_sleep_sent_while_add_match_is_in_flight_is_not_lost() {
        let bus = StandIn::new_trapping(5_000_000, false, Trap::PrepareForSleep(true));
        let mark = marker();
        let logind = begin(&bus, Some(&format!("touch {}", mark.display())), true);
        let mut state = solium_with(logind);

        // No `send_prepare_for_sleep()` here either: see
        // `a_lock_sent_while_add_match_is_in_flight_is_not_lost`.
        let seen = soon(|| {
            settle(&mut state);
            mark.exists()
        });
        assert!(
            seen,
            "a PrepareForSleep sent while AddMatch was still in flight was lost"
        );
        let _ = std::fs::remove_file(&mark);
    }

    /// **Before sleep, with Solium's own lock: the inhibitor is released only
    /// once the native lock's `locked` is confirmed**, which is every monitor
    /// having shown a frame built under it, the rule a locker's lock keeps.
    #[test]
    fn before_sleep_the_inhibitor_waits_for_the_native_lock_s_locked() {
        let bus = StandIn::new(5_000_000, false);
        let mut state = solium_with(begin(&bus, None, true));
        let monitor = monitor(&mut state);
        assert!(
            soon(|| {
                settle(&mut state);
                state.logind.holding()
            }),
            "the initial inhibitor was never taken"
        );

        bus.wait_for_subscription(MANAGER_INTERFACE, "PrepareForSleep");
        bus.send_prepare_for_sleep(true);
        assert!(
            soon(|| {
                settle(&mut state);
                natively_locked(&state)
            }),
            "sleep did not take Solium's own lock"
        );
        assert!(
            state.lock.as_ref().is_some_and(crate::lock::Lock::pending),
            "the premise: `locked` is owed to the monitor"
        );
        settle(&mut state);
        assert!(
            state.logind.holding() && bus.inhibitor_held(),
            "the inhibitor was released before the native lock was confirmed"
        );

        // The monitor shows a frame built under the lock: `locked`.
        let frame = state.lock_frame();
        state.frame_presented(&monitor, frame);
        assert!(
            !state.lock.as_ref().is_some_and(crate::lock::Lock::pending),
            "the native lock was not confirmed"
        );
        assert!(
            soon(|| !bus.inhibitor_held()),
            "the inhibitor outlived the confirmed native lock"
        );
    }

    /// **A lock before sleep has no entry, and cuts one short**: with a lock
    /// scene that would come down over the desktop, the lock `PrepareForSleep`
    /// takes is fully up at once, and the inhibitor goes with the first frame
    /// that shows it. A lock taken by hand a moment before is still coming
    /// down when sleep comes: its entry ends there, and the inhibitor waits
    /// for a frame built after it, not one built during it.
    #[test]
    fn a_lock_before_sleep_has_no_entry_and_cuts_one_short() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let directory =
                std::env::temp_dir().join(format!("solium-logind-entry-{}", std::process::id()));
            std::fs::create_dir_all(&directory).expect("a directory for the scene");
            let scene = directory.join("Lock.qml");
            std::fs::write(&scene, "import QtQuick\nItem {}\n").expect("writing the scene");

            let bus = StandIn::new(5_000_000, false);
            let mut state = solium_with(begin(&bus, None, true));
            state.lock_scenes.use_scene(scene);
            let monitor = monitor(&mut state);
            let held = |state: &mut crate::state::Solium| {
                soon(|| {
                    settle(state);
                    state.logind.holding()
                })
            };
            assert!(held(&mut state), "the initial inhibitor was never taken");
            bus.wait_for_subscription(MANAGER_INTERFACE, "PrepareForSleep");

            bus.send_prepare_for_sleep(true);
            assert!(
                soon(|| {
                    settle(&mut state);
                    natively_locked(&state)
                }),
                "sleep did not take Solium's own lock"
            );
            assert_eq!(
                state.lock_scenes.shown_on(&monitor),
                crate::lock_scene::Shown::Own,
                "the premise: a scene that would come down"
            );
            assert!(
                !state.lock_scenes.entry_pending(),
                "the lock taken before sleep has an entry"
            );
            let now = state.clock.now();
            assert!(!state.desktop_under_lock(&monitor, now, false));
            let frame = state.lock_frame();
            state.frame_presented(&monitor, frame);
            assert!(
                soon(|| !bus.inhibitor_held()),
                "the inhibitor outlived the lock taken before sleep"
            );

            // Unlocked; then locked by hand, and asleep during its entry.
            // The stand-in watches only the first inhibitor it handed out,
            // so the second one is watched from this side: held while
            // `Logind` keeps its descriptor, released once it drops it.
            state.unlock_natively();
            assert!(held(&mut state), "no inhibitor after the unlock");
            let exit = state.lock_scenes.settings().exit;
            state.clock.advance(exit);
            crate::lock_scene::settle(&mut state);
            assert!(state.lock_now(), "the hand's lock owes no `locked`");
            assert!(
                state.lock_scenes.entry_pending(),
                "the premise: the hand's lock comes down"
            );
            let during = state.lock_frame();
            state.frame_presented(&monitor, during);
            settle(&mut state);
            assert!(
                state.logind.holding(),
                "the inhibitor went during the entry"
            );

            bus.send_prepare_for_sleep(true);
            assert!(
                soon(|| {
                    settle(&mut state);
                    !state.lock_scenes.entry_pending()
                }),
                "sleep did not cut the entry short"
            );
            assert!(
                state.logind.holding(),
                "the inhibitor went before a frame showed the lock"
            );
            state.frame_presented(&monitor, during);
            settle(&mut state);
            assert!(
                state.logind.holding(),
                "a frame built during the entry released the inhibitor"
            );
            let after = state.lock_frame();
            state.frame_presented(&monitor, after);
            settle(&mut state);
            assert!(
                !state.logind.holding(),
                "the inhibitor outlived the lock's first full frame"
            );
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    /// **A peek is no lock to logind**: the lock's scenes pulled down over the
    /// desktop and let go again leave `LockedHint` alone and the inhibitor
    /// for sleep held, since nothing is locked; the lock taken from a peek
    /// then sets it as any lock does, once a frame shows it whole.
    #[test]
    fn a_peek_is_no_lock_to_logind() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let directory =
                std::env::temp_dir().join(format!("solium-logind-peek-{}", std::process::id()));
            std::fs::create_dir_all(&directory).expect("a directory for the scene");
            let scene = directory.join("Lock.qml");
            std::fs::write(&scene, "import QtQuick\nItem {}\n").expect("writing the scene");

            let bus = StandIn::new(5_000_000, false);
            let mut state = solium_with(begin(&bus, None, true));
            state.lock_scenes.use_scene(scene);
            let monitor = monitor(&mut state);
            state.sync_lock_scenes();
            assert!(
                soon(|| {
                    settle(&mut state);
                    state.logind.holding()
                }),
                "the initial inhibitor was never taken"
            );
            let hinted = |bus: &StandIn| {
                bus.heard
                    .lock()
                    .expect("the list")
                    .iter()
                    .any(|call| call.starts_with("SetLockedHint"))
            };

            state.lock_peek(Some(0.8));
            let now = state.clock.now();
            assert!(
                state.lock_scene_over_desktop(&monitor, now),
                "the premise: a peek"
            );
            state.lock_peek(None);
            settle(&mut state);
            std::thread::sleep(Duration::from_millis(100));
            settle(&mut state);
            assert!(!hinted(&bus), "a peek set `LockedHint`");
            assert!(
                state.logind.holding(),
                "a peek released the inhibitor for sleep"
            );

            state.lock_peek(Some(0.8));
            assert!(state.lock_now(), "the lock from a peek owes no `locked`");
            let enter = state.lock_scenes.settings().enter;
            state.clock.advance(enter);
            crate::lock_scene::settle(&mut state);
            let frame = state.lock_frame();
            state.frame_presented(&monitor, frame);
            assert!(
                soon(|| hinted(&bus)),
                "the lock taken from a peek never set `LockedHint`"
            );
            let _ = std::fs::remove_dir_all(&directory);
        });
    }

    /// **With nothing to lock the session, sleep is not held**: `lock.native
    /// = false` and no `lock.command` means nothing will ever confirm a lock,
    /// so the inhibitor goes at once rather than on logind's timeout.
    #[test]
    fn with_nothing_to_lock_with_sleep_is_not_held() {
        let bus = StandIn::new(5_000_000, false);
        let mut state = solium_locking(begin(&bus, None, true), false);
        assert!(soon(|| {
            settle(&mut state);
            state.logind.holding()
        }));

        bus.wait_for_subscription(MANAGER_INTERFACE, "PrepareForSleep");
        bus.send_prepare_for_sleep(true);
        assert!(
            soon(|| {
                settle(&mut state);
                !state.logind.holding()
            }),
            "sleep was held with nothing to lock the session"
        );
        assert!(state.lock.is_none(), "something locked the session");
    }

    /// A locker that never locks — nothing ever calls [`Logind::locked`] —
    /// does not hold sleep past `InhibitDelayMaxUSec`.
    /// `a_locker_that_never_locks_releases_the_inhibitor_on_logind_s_own_timeout`.
    #[test]
    fn a_locker_that_never_locks_releases_the_inhibitor_on_logind_s_own_timeout() {
        // 300ms: short enough that this test does not wait for logind's real
        // five seconds, long enough that the release is seen to depend on it
        // rather than happening at once, and with headroom for scheduling
        // under a full, parallel test run.
        let bus = StandIn::new(300_000, false);
        let logind = begin(&bus, Some("true"), true);
        let mut state = solium_with(logind);
        // `state.logind.holding()`: see the same precondition in
        // `prepare_for_sleep_locks_before_the_inhibitor_is_released` for why
        // `bus.inhibitor_held()` is not enough here either.
        assert!(soon(|| {
            settle(&mut state);
            state.logind.holding()
        }));

        // #225's own title: this is the test that failed once under a full,
        // parallel run, by sending before the listener had subscribed.
        bus.wait_for_subscription(MANAGER_INTERFACE, "PrepareForSleep");
        bus.send_prepare_for_sleep(true);
        let released = soon(|| {
            settle(&mut state);
            !bus.inhibitor_held()
        });
        assert!(
            released,
            "a locker that never locked held sleep off for ever"
        );
    }

    /// `LockedHint` follows lock and unlock, in either direction.
    /// `locked_hint_follows_lock_and_unlock`.
    #[test]
    fn locked_hint_follows_lock_and_unlock() {
        let bus = StandIn::new(5_000_000, false);
        let mut logind = begin(&bus, None, false);
        assert!(
            bus.requests()
                .iter()
                .any(|call| call.starts_with("GetSession"))
        );

        logind.locked(true);
        assert!(soon(|| bus
            .heard
            .lock()
            .expect("the list")
            .contains(&"SetLockedHint true".to_owned())));

        logind.locked(false);
        assert!(soon(|| bus
            .heard
            .lock()
            .expect("the list")
            .contains(&"SetLockedHint false".to_owned())));
    }

    /// A bus with nobody on the other end of `Inhibit`: the fallback timeout
    /// is used instead of hanging on a property read that will never answer,
    /// and the inhibitor itself is simply not held.
    #[test]
    fn an_inhibit_logind_refuses_is_not_held() {
        let bus = StandIn::new(5_000_000, true);
        let logind = begin(&bus, Some("true"), true);
        let mut state = solium_with(logind);
        settle(&mut state);
        assert!(
            soon(|| {
                settle(&mut state);
                !state.logind.holding()
            }),
            "held an inhibitor logind refused to grant"
        );
    }
}
