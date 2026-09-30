//! Telling the rest of the user session that Solium is its desktop (#146).
//!
//! Portals, D-Bus-activated programs, XDG autostart and user services bound to
//! `graphical-session.target` are all started by systemd's user manager or by
//! D-Bus activation, and neither has ever heard of this compositor's socket.
//! So once it is up, a Solium started as the session tells them, over the
//! session bus:
//!
//! 1. `org.freedesktop.systemd1.Manager.SetEnvironment` and
//!    `org.freedesktop.DBus.UpdateActivationEnvironment` with
//!    `WAYLAND_DISPLAY`, `XDG_CURRENT_DESKTOP` and `XDG_SESSION_TYPE`, and
//!    again with `DISPLAY` once XWayland has one;
//! 2. `StartUnit` on `solium-session.target`, and on `solium-autostart.target`
//!    while `session.autostart` is on, once X11 has answered one way or the
//!    other, so an X11 program in `~/.config/autostart` finds `DISPLAY`;
//! 3. on exit, `StopUnit` on the session target and `UnsetEnvironment` on the
//!    variables, so a Plasma login afterwards inherits no dead socket.
//!
//! `the_environment_goes_out_when_the_socket_is_up_and_again_with_display`,
//! `the_target_is_stopped_and_the_variables_unset_on_exit` and
//! `the_calls_reach_systemd_and_dbus_as_their_methods` hold it to that.
//!
//! Before any of that, the bus thread asks systemd who has
//! `graphical-session.target` ([`claim`]). Targets a previous Solium left
//! running are stopped first, and another desktop of this user's is left
//! alone: `a_session_left_running_is_stopped_before_the_first_export` and
//! `another_desktop_holding_the_graphical_session_is_left_alone`.
//!
//! The calls are made from a thread of their own, because a session bus that
//! is slow to answer must never hold a frame: see [`Worker`].

use std::{
    collections::HashMap,
    sync::mpsc,
    time::{Duration, Instant},
};

use zbus::zvariant::OwnedObjectPath;

/// The target a session starts, which starts `graphical-session.target`.
/// `dev/session/` ships it, and a user's own units attach to it.
pub(crate) const TARGET: &str = "solium-session.target";
/// XDG autostart, started after [`TARGET`] while `session.autostart` is on,
/// and stopped with it (`PartOf=`). A separate unit so that turning autostart
/// off leaves what is attached to [`TARGET`] starting.
/// `session_autostart_false_starts_the_session_without_autostart`.
pub(crate) const AUTOSTART: &str = "solium-autostart.target";
/// What a desktop's session target binds to, and so what says one is running.
const GRAPHICAL: &str = "graphical-session.target";

/// How long one call may take, and how long exit waits for the last ones.
///
/// Exit waits so that the stop and the unset actually leave the process
/// before it ends, and no longer than this so that a bus that has stopped
/// answering cannot hold the session open.
/// `a_bus_that_never_answers_holds_the_exit_for_at_most_its_patience`.
const PATIENCE: Duration = Duration::from_secs(2);

/// `session` in the configuration, handed over by `sol.session`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Settings {
    /// Export the environment and start the target at all.
    pub(crate) systemd: bool,
    /// Start [`AUTOSTART`] beside [`TARGET`].
    pub(crate) autostart: bool,
    /// How long after SIGTERM, SIGINT or SIGHUP a Solium that has not ended
    /// ends itself, however it was started: see `signals.rs`.
    pub(crate) stop_timeout: Duration,
}

impl Default for Settings {
    /// Both on, and five seconds.
    /// `the_shipped_configuration_tells_the_session_and_starts_autostart`.
    fn default() -> Self {
        Self {
            systemd: true,
            autostart: true,
            stop_timeout: crate::signals::STOP_TIMEOUT,
        }
    }
}

/// How this compositor was started, which says whether it is the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    /// `solium --tty --session`, which is how `solium-session`, the session
    /// file's `Exec`, starts it: this compositor is the user's graphical
    /// session.
    Session,
    /// `solium --tty` started by hand: from a text console, say, while the
    /// user's own desktop runs on another VT with the same systemd and the
    /// same session bus. Told nothing unless `SOLIUM_SESSION_BUS` names a
    /// bus. `a_manual_tty_start_tells_nobody`.
    Console,
    /// A window inside another session, whose environment belongs to that
    /// session. Told nothing unless `SOLIUM_SESSION_BUS` names a bus.
    /// `a_nested_run_tells_nobody_unless_it_is_given_a_bus`.
    Nested,
}

impl Place {
    /// `--tty`, from what came after it on the command line: `--session`
    /// anywhere there, like `--qml`. `a_manual_tty_start_tells_nobody`.
    pub(crate) fn tty(arguments: impl IntoIterator<Item = String>) -> Self {
        if arguments
            .into_iter()
            .any(|argument| argument == "--session")
        {
            Self::Session
        } else {
            Self::Console
        }
    }
}

/// One call on the session bus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Call {
    /// `org.freedesktop.systemd1.Manager.SetEnvironment`, each `NAME=value`.
    SetEnvironment(Vec<String>),
    /// `org.freedesktop.DBus.UpdateActivationEnvironment`.
    UpdateActivationEnvironment(Vec<(String, String)>),
    /// `org.freedesktop.systemd1.Manager.StartUnit`, in `replace` mode.
    StartUnit(String),
    /// `org.freedesktop.systemd1.Manager.StopUnit`, in `replace` mode.
    StopUnit(String),
    /// `org.freedesktop.systemd1.Manager.UnsetEnvironment`.
    UnsetEnvironment(Vec<String>),
}

/// Where calls go: the bus thread, or a test's list.
trait Sink {
    fn send(&mut self, call: Call);
    /// No more calls: wait, within [`PATIENCE`], for those sent to be made.
    fn finish(&mut self);
}

/// What this session has told systemd and D-Bus, and what it still owes them.
pub(crate) struct Session {
    /// `None` once ended, and for a session that tells nobody anything.
    sink: Option<Box<dyn Sink>>,
    autostart: bool,
    desktop: String,
    socket: Option<String>,
    display: Option<u32>,
    /// X11 has answered: a display, or none coming.
    settled: bool,
    started: bool,
    /// Every name exported, in the order first sent, for the unset on exit.
    exported: Vec<&'static str>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Session")
            .field("telling", &self.sink.is_some())
            .field("autostart", &self.autostart)
            .field("started", &self.started)
            .field("exported", &self.exported)
            .finish_non_exhaustive()
    }
}

impl Session {
    /// A session that tells nobody anything: `session.systemd = false`, a
    /// start that is not the session, and every `Solium` before its backend
    /// starts one.
    pub(crate) fn off() -> Self {
        Self {
            sink: None,
            autostart: true,
            desktop: String::new(),
            socket: None,
            display: None,
            settled: false,
            started: false,
            exported: Vec::new(),
        }
    }

    /// The session a backend starts, from the configuration's `session`.
    ///
    /// `bus` is `SOLIUM_SESSION_BUS`: the bus to tell instead of the session
    /// bus, and the only way to tell one at all from a start that is not the
    /// session.
    pub(crate) fn begin(settings: Settings, place: Place, bus: Option<String>) -> Self {
        if !settings.systemd {
            tracing::info!(
                "session.systemd is off: systemd and D-Bus activation are not told about \
                 this session, and {TARGET} is not started"
            );
            return Self::off();
        }
        if place != Place::Session && bus.is_none() {
            tracing::info!(
                ?place,
                "not started as the session (the session file's solium-session, or \
                 `solium --tty --session`): systemd and D-Bus activation keep the \
                 environment they have. SOLIUM_SESSION_BUS names a bus to tell instead"
            );
            return Self::off();
        }
        let desktop = desktop(std::env::var("XDG_CURRENT_DESKTOP").ok());
        match Worker::spawn(bus) {
            Ok(worker) => Self::with_sink(settings, Box::new(worker), desktop),
            Err(err) => {
                tracing::warn!(
                    ?err,
                    "no thread for the session bus: the session is not told"
                );
                Self::off()
            }
        }
    }

    fn with_sink(settings: Settings, sink: Box<dyn Sink>, desktop: String) -> Self {
        Self {
            sink: Some(sink),
            autostart: settings.autostart,
            desktop,
            socket: None,
            display: None,
            settled: false,
            started: false,
            exported: Vec::new(),
        }
    }

    /// The Wayland socket is up.
    pub(crate) fn wayland(&mut self, socket: &str) {
        self.socket = Some(socket.to_owned());
        self.export();
        self.start_when_ready();
    }

    /// X11 has answered: XWayland's display, or `None` when none is coming
    /// (not installed, exited, or not ready within its five seconds).
    pub(crate) fn x11(&mut self, display: Option<u32>) {
        if let Some(number) = display
            && self.display != Some(number)
        {
            self.display = Some(number);
            self.export();
        }
        self.settled = true;
        self.start_when_ready();
    }

    /// Stop the target and unset what was exported. Also on drop, for a
    /// backend that returns early.
    pub(crate) fn end(&mut self) {
        let Some(mut sink) = self.sink.take() else {
            return;
        };
        // One stop: the autostart target is `PartOf=` this one, so it goes
        // with it. `the_target_is_stopped_and_the_variables_unset_on_exit`.
        if self.started {
            sink.send(Call::StopUnit(TARGET.to_owned()));
        }
        // Unset only: D-Bus has no call that removes a variable from the
        // activation environment, so nothing is sent there.
        // `the_target_is_stopped_and_the_variables_unset_on_exit`.
        if !self.exported.is_empty() {
            sink.send(Call::UnsetEnvironment(
                self.exported.iter().map(|&name| name.to_owned()).collect(),
            ));
        }
        sink.finish();
    }

    /// Whether anything is being told at all.
    #[cfg(test)]
    fn is_off(&self) -> bool {
        self.sink.is_none()
    }

    fn variables(&self) -> Vec<(&'static str, String)> {
        let mut variables = Vec::with_capacity(4);
        if let Some(socket) = &self.socket {
            variables.push(("WAYLAND_DISPLAY", socket.clone()));
        }
        if let Some(number) = self.display {
            variables.push(("DISPLAY", format!(":{number}")));
        }
        variables.push(("XDG_CURRENT_DESKTOP", self.desktop.clone()));
        variables.push(("XDG_SESSION_TYPE", "wayland".to_owned()));
        variables
    }

    fn export(&mut self) {
        let variables = self.variables();
        let Some(sink) = self.sink.as_mut() else {
            return;
        };
        for (name, _) in &variables {
            if !self.exported.contains(name) {
                self.exported.push(name);
            }
        }
        sink.send(Call::SetEnvironment(
            variables
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect(),
        ));
        sink.send(Call::UpdateActivationEnvironment(
            variables
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value))
                .collect(),
        ));
    }

    fn start_when_ready(&mut self) {
        if self.started || !self.settled || self.socket.is_none() {
            return;
        }
        let Some(sink) = self.sink.as_mut() else {
            return;
        };
        sink.send(Call::StartUnit(TARGET.to_owned()));
        if self.autostart {
            sink.send(Call::StartUnit(AUTOSTART.to_owned()));
        }
        self.started = true;
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.end();
    }
}

/// `XDG_CURRENT_DESKTOP` as the session file's `DesktopNames` set it, or
/// `Lilium` when nothing did. `the_desktop_is_lilium_unless_the_session_said_otherwise`.
pub(crate) fn desktop(set: Option<String>) -> String {
    set.filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Lilium".to_owned())
}

/// The thread that owns the bus connection.
///
/// zbus's blocking API, from a thread of this compositor's own: the event loop
/// only ever puts a [`Call`] on a channel.
struct Worker {
    calls: Option<mpsc::Sender<Call>>,
    done: mpsc::Receiver<()>,
}

impl Worker {
    fn spawn(bus: Option<String>) -> std::io::Result<Self> {
        let (calls, incoming) = mpsc::channel::<Call>();
        let (finished, done) = mpsc::channel();
        std::thread::Builder::new()
            .name("solium-session".to_owned())
            .spawn(move || {
                match connect(bus.as_deref()) {
                    Ok(connection) => {
                        let telling = claim(&connection);
                        // Drained either way, so the calls queued meanwhile
                        // and the ones exit sends end here too.
                        // `another_desktop_holding_the_graphical_session_is_left_alone`.
                        for call in incoming {
                            if !telling {
                                continue;
                            }
                            match perform(&connection, &call) {
                                Ok(()) => tracing::info!(?call, "told the session"),
                                Err(err) => {
                                    tracing::warn!(?err, ?call, "the session bus refused this");
                                }
                            }
                        }
                    }
                    Err(err) => tracing::warn!(
                        ?err,
                        bus,
                        "no session bus: systemd and D-Bus activation are not told about \
                         this session, so portals and XDG autostart will not find it"
                    ),
                }
                let _ = finished.send(());
            })?;
        Ok(Self {
            calls: Some(calls),
            done,
        })
    }
}

impl Sink for Worker {
    fn send(&mut self, call: Call) {
        if let Some(calls) = &self.calls {
            let _ = calls.send(call);
        }
    }

    fn finish(&mut self) {
        // Closing the channel is what ends the thread's loop.
        self.calls = None;
        if self.done.recv_timeout(PATIENCE).is_err() {
            tracing::warn!(
                seconds = PATIENCE.as_secs(),
                "the session bus did not answer in time; exiting without its reply"
            );
        }
    }
}

fn connect(bus: Option<&str>) -> zbus::Result<zbus::blocking::Connection> {
    let builder = match bus {
        Some(address) => zbus::blocking::connection::Builder::address(address)?,
        // A test that got this far by mistake would ask the developer's own
        // systemd about their session, and could stop its target as a
        // leftover. `tests_never_reach_the_session_bus`.
        #[cfg(test)]
        None => {
            return Err(zbus::Error::Address(
                "tests never use the session bus".to_owned(),
            ));
        }
        #[cfg(not(test))]
        None => zbus::blocking::connection::Builder::session()?,
    };
    builder.method_timeout(PATIENCE).build()
}

const SYSTEMD: &str = "org.freedesktop.systemd1";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const SYSTEMD_MANAGER: &str = "org.freedesktop.systemd1.Manager";
const DBUS: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";

/// One unit, as `ListUnitsByNames` answers: its active state and its job.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct UnitState {
    active: String,
    job: String,
}

impl UnitState {
    /// Running in any sense, stopping included.
    fn up(&self) -> bool {
        matches!(
            self.active.as_str(),
            "active" | "activating" | "reloading" | "deactivating"
        )
    }

    /// On its way down.
    fn stopping(&self) -> bool {
        self.job == "stop" || self.active == "deactivating"
    }

    /// Up, and staying up.
    fn holds(&self) -> bool {
        self.up() && !self.stopping()
    }
}

/// `ListUnitsByNames`'s `a(ssssssouso)`, of which the fourth and ninth are
/// read.
type UnitInfo = (
    String,
    String,
    String,
    String,
    String,
    String,
    OwnedObjectPath,
    u32,
    String,
    OwnedObjectPath,
);

fn units(connection: &zbus::blocking::Connection) -> zbus::Result<HashMap<String, UnitState>> {
    let reply = connection.call_method(
        Some(SYSTEMD),
        SYSTEMD_PATH,
        Some(SYSTEMD_MANAGER),
        "ListUnitsByNames",
        &vec![GRAPHICAL, TARGET, AUTOSTART],
    )?;
    let listed: Vec<UnitInfo> = reply.body().deserialize()?;
    Ok(listed
        .into_iter()
        .map(|(name, _, _, active, _, _, _, _, job, _)| (name, UnitState { active, job }))
        .collect())
}

/// Whether this session may tell systemd and D-Bus anything, asked before
/// the first export.
///
/// Solium's targets up before this Solium has started them are a previous
/// Solium's, left by a crash: they are stopped, and `graphical-session.target`
/// given [`PATIENCE`] to go with them, so what was bound to the dead display
/// stops. `a_session_left_running_is_stopped_before_the_first_export`.
///
/// `graphical-session.target` up and staying up after that belongs to
/// another desktop of this user's, and this session leaves systemd and D-Bus
/// to it. `another_desktop_holding_the_graphical_session_is_left_alone`.
///
/// A systemd that cannot be asked is told anyway: without one there is no
/// target to hold, and D-Bus activation still wants the environment.
/// `a_systemd_that_cannot_be_asked_is_told_anyway`.
fn claim(connection: &zbus::blocking::Connection) -> bool {
    let mut states = match units(connection) {
        Ok(states) => states,
        Err(err) => {
            tracing::warn!(
                ?err,
                "could not ask systemd who has {GRAPHICAL}; telling it about this session anyway"
            );
            return true;
        }
    };
    let state = |states: &HashMap<String, UnitState>, unit: &str| {
        states.get(unit).cloned().unwrap_or_default()
    };
    let leftovers: Vec<&str> = [TARGET, AUTOSTART]
        .into_iter()
        .filter(|unit| state(&states, unit).up())
        .collect();
    if !leftovers.is_empty() {
        tracing::warn!(
            ?leftovers,
            "a Solium session that did not end cleanly left these running: stopping them \
             before this one starts"
        );
        for unit in &leftovers {
            if state(&states, unit).stopping() {
                continue;
            }
            if let Err(err) = perform(connection, &Call::StopUnit((*unit).to_owned())) {
                tracing::warn!(?err, unit, "could not stop it");
            }
        }
        let deadline = Instant::now() + PATIENCE;
        while state(&states, GRAPHICAL).up() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            match units(connection) {
                Ok(now) => states = now,
                Err(err) => {
                    tracing::warn!(?err, "could not ask systemd again");
                    break;
                }
            }
        }
    }
    if state(&states, GRAPHICAL).holds() {
        tracing::warn!(
            "{GRAPHICAL} is already active, and not for a Solium session: another desktop \
             of this user's is running (on another VT, say). This session leaves systemd \
             and D-Bus activation alone, so that desktop keeps its display"
        );
        return false;
    }
    true
}

/// One call, on the wire. `the_calls_reach_systemd_and_dbus_as_their_methods`.
fn perform(connection: &zbus::blocking::Connection, call: &Call) -> zbus::Result<()> {
    let manager = |method: &str, body: &dyn Body| body.call(connection, method);
    match call {
        Call::SetEnvironment(assignments) => manager("SetEnvironment", assignments),
        Call::UnsetEnvironment(names) => manager("UnsetEnvironment", names),
        Call::StartUnit(unit) => manager("StartUnit", &(unit.as_str(), "replace")),
        Call::StopUnit(unit) => manager("StopUnit", &(unit.as_str(), "replace")),
        Call::UpdateActivationEnvironment(variables) => {
            let environment: HashMap<&str, &str> = variables
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect();
            connection
                .call_method(
                    Some(DBUS),
                    DBUS_PATH,
                    Some(DBUS),
                    "UpdateActivationEnvironment",
                    &environment,
                )
                .map(drop)
        }
    }
}

/// A body for a systemd manager method, so [`perform`] can name each call's
/// arguments once without naming serde, which zbus brings but this crate
/// does not depend on.
trait Body {
    fn call(&self, connection: &zbus::blocking::Connection, method: &str) -> zbus::Result<()>;
}

impl Body for Vec<String> {
    fn call(&self, connection: &zbus::blocking::Connection, method: &str) -> zbus::Result<()> {
        connection
            .call_method(
                Some(SYSTEMD),
                SYSTEMD_PATH,
                Some(SYSTEMD_MANAGER),
                method,
                self,
            )
            .map(drop)
    }
}

impl Body for (&str, &str) {
    fn call(&self, connection: &zbus::blocking::Connection, method: &str) -> zbus::Result<()> {
        connection
            .call_method(
                Some(SYSTEMD),
                SYSTEMD_PATH,
                Some(SYSTEMD_MANAGER),
                method,
                self,
            )
            .map(drop)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        rc::Rc,
        sync::{
            Arc, Mutex,
            atomic::{AtomicU32, Ordering},
        },
    };

    use super::*;

    /// A sink that keeps every call, and whether it was finished.
    #[derive(Clone, Default)]
    struct Recorder {
        calls: Rc<RefCell<Vec<Call>>>,
        finished: Rc<RefCell<u32>>,
    }

    impl Sink for Recorder {
        fn send(&mut self, call: Call) {
            self.calls.borrow_mut().push(call);
        }
        fn finish(&mut self) {
            *self.finished.borrow_mut() += 1;
        }
    }

    impl Recorder {
        fn take(&self) -> Vec<Call> {
            std::mem::take(&mut *self.calls.borrow_mut())
        }
    }

    fn session(settings: Settings) -> (Session, Recorder) {
        let recorder = Recorder::default();
        let session = Session::with_sink(settings, Box::new(recorder.clone()), "Lilium".into());
        (session, recorder)
    }

    fn export(assignments: &[&str]) -> [Call; 2] {
        [
            Call::SetEnvironment(assignments.iter().map(|&each| each.to_owned()).collect()),
            Call::UpdateActivationEnvironment(
                assignments
                    .iter()
                    .filter_map(|each| each.split_once('='))
                    .map(|(name, value)| (name.to_owned(), value.to_owned()))
                    .collect(),
            ),
        ]
    }

    fn start() -> [Call; 2] {
        [
            Call::StartUnit(TARGET.to_owned()),
            Call::StartUnit(AUTOSTART.to_owned()),
        ]
    }

    /// Exactly those variables, at both moments, and the targets once X11 has
    /// answered.
    #[test]
    fn the_environment_goes_out_when_the_socket_is_up_and_again_with_display() {
        let (mut session, recorder) = session(Settings::default());

        session.wayland("wayland-7");
        assert_eq!(
            recorder.take(),
            export(&[
                "WAYLAND_DISPLAY=wayland-7",
                "XDG_CURRENT_DESKTOP=Lilium",
                "XDG_SESSION_TYPE=wayland",
            ])
        );

        session.x11(Some(3));
        let mut expected = export(&[
            "WAYLAND_DISPLAY=wayland-7",
            "DISPLAY=:3",
            "XDG_CURRENT_DESKTOP=Lilium",
            "XDG_SESSION_TYPE=wayland",
        ])
        .to_vec();
        expected.extend(start());
        assert_eq!(recorder.take(), expected);

        // XWayland answers once; a repeat says nothing new.
        session.x11(Some(3));
        assert_eq!(recorder.take(), Vec::new());
    }

    /// No X11 coming: the targets start on the first export alone, and a
    /// display that turns up late is still exported.
    #[test]
    fn without_x11_the_target_starts_after_the_first_export() {
        let (mut session, recorder) = session(Settings::default());
        session.wayland("wayland-1");
        session.x11(None);
        let calls = recorder.take();
        assert_eq!(calls.len(), 4, "{calls:?}");
        assert_eq!(calls[2..], start());

        session.x11(Some(0));
        let calls = recorder.take();
        assert_eq!(
            calls,
            export(&[
                "WAYLAND_DISPLAY=wayland-1",
                "DISPLAY=:0",
                "XDG_CURRENT_DESKTOP=Lilium",
                "XDG_SESSION_TYPE=wayland",
            ])
        );
    }

    #[test]
    fn the_target_is_stopped_and_the_variables_unset_on_exit() {
        let (mut session, recorder) = session(Settings::default());
        session.wayland("wayland-2");
        session.x11(Some(1));
        recorder.take();

        session.end();
        assert_eq!(
            recorder.take(),
            vec![
                Call::StopUnit(TARGET.to_owned()),
                Call::UnsetEnvironment(
                    [
                        "WAYLAND_DISPLAY",
                        "XDG_CURRENT_DESKTOP",
                        "XDG_SESSION_TYPE",
                        "DISPLAY",
                    ]
                    .map(str::to_owned)
                    .to_vec()
                ),
            ]
        );
        assert_eq!(*recorder.finished.borrow(), 1);

        // Once: dropping an ended session says nothing more.
        drop(session);
        assert_eq!(recorder.take(), Vec::new());
        assert_eq!(*recorder.finished.borrow(), 1);
    }

    /// A backend that returns early still leaves nothing behind.
    #[test]
    fn dropping_a_session_ends_it() {
        let (mut session, recorder) = session(Settings::default());
        session.wayland("wayland-3");
        recorder.take();
        drop(session);
        // Exported but never started: only the unset.
        assert_eq!(
            recorder.take(),
            vec![Call::UnsetEnvironment(
                ["WAYLAND_DISPLAY", "XDG_CURRENT_DESKTOP", "XDG_SESSION_TYPE"]
                    .map(str::to_owned)
                    .to_vec()
            )]
        );
        assert_eq!(*recorder.finished.borrow(), 1);
    }

    #[test]
    fn session_systemd_false_tells_nobody_anything() {
        let mut session = Session::begin(
            Settings {
                systemd: false,
                ..Settings::default()
            },
            Place::Session,
            None,
        );
        assert!(session.is_off());
        // Nothing to send to, whatever happens.
        session.wayland("wayland-4");
        session.x11(Some(2));
        session.end();
        assert!(!session.started);
        assert!(session.exported.is_empty());
    }

    /// The session target, and what users attach to it, still starts; only
    /// XDG autostart is left out.
    #[test]
    fn session_autostart_false_starts_the_session_without_autostart() {
        let (mut session, recorder) = session(Settings {
            autostart: false,
            ..Settings::default()
        });
        session.wayland("wayland-5");
        session.x11(None);
        let calls = recorder.take();
        assert_eq!(calls.last(), Some(&Call::StartUnit(TARGET.to_owned())));
        assert!(
            !calls.contains(&Call::StartUnit(AUTOSTART.to_owned())),
            "{calls:?}"
        );
        session.end();
        assert_eq!(
            recorder.take().first(),
            Some(&Call::StopUnit(TARGET.to_owned()))
        );
    }

    /// Never the session a nested window runs inside: that is the developer's
    /// own desktop, and its `WAYLAND_DISPLAY` is not ours to replace.
    #[test]
    fn a_nested_run_tells_nobody_unless_it_is_given_a_bus() {
        assert!(Session::begin(Settings::default(), Place::Nested, None).is_off());
    }

    /// `solium --tty` by hand, from a text console beside a desktop on another
    /// VT: that desktop's systemd and bus are this one's too, and are left
    /// alone. Only `--session`, which `solium-session` passes, tells them.
    #[test]
    fn a_manual_tty_start_tells_nobody() {
        let place = |arguments: &[&str]| Place::tty(arguments.iter().map(|&each| each.to_owned()));
        assert_eq!(place(&[]), Place::Console);
        assert_eq!(place(&["--qml", "gpu"]), Place::Console);
        assert_eq!(place(&["--sessions"]), Place::Console);
        assert_eq!(place(&["--session"]), Place::Session);
        assert_eq!(place(&["--qml", "gpu", "--session"]), Place::Session);
        let mut manual = Session::begin(Settings::default(), place(&[]), None);
        assert!(manual.is_off());
        manual.wayland("wayland-11");
        manual.x11(Some(5));
        manual.end();
        assert!(manual.exported.is_empty());
        assert!(!manual.started);
    }

    /// Read when the session starts: a configuration applied later -- which
    /// is what a reload does -- neither ends the session nor starts another.
    #[test]
    fn a_reload_leaves_the_session_as_it_began() {
        let display = smithay::reexports::wayland_server::Display::<crate::state::Solium>::new()
            .expect("a test wayland display");
        let mut state = crate::state::Solium::new(display.handle());
        let (session, recorder) = session(Settings::default());
        state.session = session;
        state.session.wayland("wayland-10");
        state.session.x11(None);
        recorder.take();

        let directory = std::env::temp_dir().join("solium-session-test-reload");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            "sol.session({ systemd = false, autostart = false })",
        )
        .expect("writing the test script");
        let scripts = crate::script::Scripts::load(&config).expect("loading the test script");
        let _ = std::fs::remove_dir_all(&directory);
        state.start_scripts(Some(scripts));

        assert_eq!(recorder.take(), Vec::new());
        assert!(state.session.started);
        assert!(state.session.autostart);
    }

    /// Every test that needs a bus names one; a session that reaches for the
    /// session bus instead, from a test, gets none.
    #[test]
    fn tests_never_reach_the_session_bus() {
        assert!(connect(None).is_err());
    }

    #[test]
    fn the_desktop_is_lilium_unless_the_session_said_otherwise() {
        assert_eq!(desktop(None), "Lilium");
        assert_eq!(desktop(Some(String::new())), "Lilium");
        assert_eq!(desktop(Some("Lilium:GNOME".into())), "Lilium:GNOME");
    }

    /// A bus address that leads nowhere: a warning, and an exit that is not
    /// held up by it.
    #[test]
    fn a_bus_that_is_not_there_costs_a_warning_and_not_the_session() {
        let address = "unix:path=/nonexistent/solium-session-test/bus".to_owned();
        let mut session = Session::begin(Settings::default(), Place::Nested, Some(address));
        assert!(!session.is_off());
        session.wayland("wayland-6");
        session.x11(None);
        let started = std::time::Instant::now();
        session.end();
        assert!(started.elapsed() < PATIENCE, "{:?}", started.elapsed());
    }

    /// A socket that accepts and never speaks: the thread is stuck in the
    /// handshake for ever, and exit waits for it no longer than [`PATIENCE`].
    #[test]
    fn a_bus_that_never_answers_holds_the_exit_for_at_most_its_patience() {
        let directory = scratch("silent");
        let path = directory.join("bus");
        let listener = std::os::unix::net::UnixListener::bind(&path).expect("binding the socket");
        // Accepted and held, so the peer is connected and hears nothing.
        let holder = std::thread::spawn(move || listener.accept().map(|(stream, _)| stream));

        let mut session = Session::begin(
            Settings::default(),
            Place::Nested,
            Some(format!("unix:path={}", path.display())),
        );
        session.wayland("wayland-8");
        let started = std::time::Instant::now();
        session.end();
        let waited = started.elapsed();
        assert!(waited >= PATIENCE, "{waited:?}");
        assert!(waited < PATIENCE + Duration::from_secs(1), "{waited:?}");
        drop(holder);
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// A directory of this test's own.
    fn scratch(name: &str) -> std::path::PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let directory = std::env::temp_dir().join(format!(
            "solium-session-test-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        directory
    }

    /// What the stand-in heard, as `interface.Method args`.
    type Heard = Arc<Mutex<Vec<String>>>;
    /// Each unit's active state and job, as the stand-in systemd has them.
    type Units = Arc<Mutex<HashMap<String, UnitState>>>;

    struct StandInSystemd {
        heard: Heard,
        units: Units,
        /// Whether `ListUnitsByNames` is a method this systemd has.
        lists: bool,
    }

    impl StandInSystemd {
        fn hear(&self, what: String) {
            self.heard.lock().expect("the list").push(what);
        }

        /// A unit started or stopped, and `graphical-session.target`, which is
        /// `StopWhenUnneeded=`, up exactly while one of Solium's is.
        fn set(&self, unit: &str, active: &str) {
            let state = |active: &str| UnitState {
                active: active.to_owned(),
                job: String::new(),
            };
            let mut units = self.units.lock().expect("the units");
            units.insert(unit.to_owned(), state(active));
            let needed = [TARGET, AUTOSTART]
                .iter()
                .any(|unit| units.get(*unit).is_some_and(UnitState::up));
            units.insert(
                GRAPHICAL.to_owned(),
                state(if needed { "active" } else { "inactive" }),
            );
        }
    }

    #[zbus::interface(name = "org.freedesktop.systemd1.Manager")]
    impl StandInSystemd {
        fn set_environment(&self, assignments: Vec<String>) {
            self.hear(format!("systemd1.Manager.SetEnvironment {assignments:?}"));
        }
        fn unset_environment(&self, names: Vec<String>) {
            self.hear(format!("systemd1.Manager.UnsetEnvironment {names:?}"));
        }
        fn start_unit(&self, name: String, mode: String) -> OwnedObjectPath {
            self.hear(format!("systemd1.Manager.StartUnit {name} {mode}"));
            self.set(&name, "active");
            OwnedObjectPath::try_from("/org/freedesktop/systemd1/job/1").expect("a job path")
        }
        fn stop_unit(&self, name: String, mode: String) -> OwnedObjectPath {
            self.hear(format!("systemd1.Manager.StopUnit {name} {mode}"));
            self.set(&name, "inactive");
            OwnedObjectPath::try_from("/org/freedesktop/systemd1/job/2").expect("a job path")
        }
        fn list_units_by_names(&self, names: Vec<String>) -> zbus::fdo::Result<Vec<UnitInfo>> {
            if !self.lists {
                return Err(zbus::fdo::Error::UnknownMethod("not this systemd".into()));
            }
            self.hear(format!("systemd1.Manager.ListUnitsByNames {names:?}"));
            let units = self.units.lock().expect("the units");
            let path = OwnedObjectPath::try_from("/").expect("a path");
            Ok(names
                .into_iter()
                .map(|name| {
                    let state = units.get(&name).cloned().unwrap_or_else(|| UnitState {
                        active: "inactive".to_owned(),
                        job: String::new(),
                    });
                    (
                        name,
                        String::new(),
                        "loaded".to_owned(),
                        state.active,
                        String::new(),
                        String::new(),
                        path.clone(),
                        0,
                        state.job,
                        path.clone(),
                    )
                })
                .collect())
        }
    }

    struct StandInDriver(Heard);

    #[zbus::interface(name = "org.freedesktop.DBus")]
    impl StandInDriver {
        /// What a client says first on a bus.
        fn hello(&self) -> String {
            ":1.1".to_owned()
        }
        fn add_match(&self, _rule: String) {}
        fn update_activation_environment(&self, environment: HashMap<String, String>) {
            let mut sorted: Vec<_> = environment.into_iter().collect();
            sorted.sort();
            self.0
                .lock()
                .expect("the list")
                .push(format!("DBus.UpdateActivationEnvironment {sorted:?}"));
        }
    }

    /// A stand-in session bus: systemd's manager and the bus driver, served by
    /// zbus at a socket of the test's own to the one connection a [`Worker`]
    /// makes. A whole session plays through it, with no bus, real or
    /// private, anywhere near.
    struct StandIn {
        address: String,
        heard: Heard,
        directory: std::path::PathBuf,
        _stop: mpsc::Sender<()>,
    }

    impl StandIn {
        fn new(units: &[(&str, &str, &str)], lists: bool) -> Self {
            let directory = scratch("bus");
            let path = directory.join("bus");
            let listener =
                std::os::unix::net::UnixListener::bind(&path).expect("binding the socket");
            let heard = Heard::default();
            let states = Units::default();
            for &(unit, active, job) in units {
                states.lock().expect("the units").insert(
                    unit.to_owned(),
                    UnitState {
                        active: active.to_owned(),
                        job: job.to_owned(),
                    },
                );
            }
            let (stop, stopped) = mpsc::channel::<()>();
            {
                let heard = heard.clone();
                std::thread::spawn(move || {
                    let Ok((stream, _)) = listener.accept() else {
                        return;
                    };
                    let systemd = StandInSystemd {
                        heard: heard.clone(),
                        units: states,
                        lists,
                    };
                    let connection =
                        zbus::blocking::connection::Builder::async_io_unix_stream(stream)
                            .server(zbus::Guid::generate())
                            .expect("a server guid")
                            .p2p()
                            .serve_at(SYSTEMD_PATH, systemd)
                            .expect("serving systemd")
                            .serve_at(DBUS_PATH, StandInDriver(heard))
                            .expect("serving the driver")
                            .build();
                    // Served until the test is done with it.
                    let _ = stopped.recv();
                    drop(connection);
                });
            }
            Self {
                address: format!("unix:path={}", path.display()),
                heard,
                directory,
                _stop: stop,
            }
        }

        /// A session, from its first export to its exit, told to this bus.
        fn play(&self, settings: Settings) -> Vec<String> {
            let mut session = Session::begin(settings, Place::Session, Some(self.address.clone()));
            assert!(!session.is_off());
            session.wayland("wayland-9");
            session.x11(Some(4));
            session.end();
            self.heard.lock().expect("the list").clone()
        }
    }

    impl Drop for StandIn {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    const LISTED: &str = "systemd1.Manager.ListUnitsByNames \
                          [\"graphical-session.target\", \"solium-session.target\", \
                          \"solium-autostart.target\"]";

    /// Everything a session sends after it has been let in, as heard.
    fn told(display_socket: &str) -> Vec<String> {
        let vars = |display: bool| {
            let mut sorted = vec![
                ("WAYLAND_DISPLAY".to_owned(), display_socket.to_owned()),
                ("XDG_CURRENT_DESKTOP".to_owned(), "Lilium".to_owned()),
                ("XDG_SESSION_TYPE".to_owned(), "wayland".to_owned()),
            ];
            if display {
                sorted.push(("DISPLAY".to_owned(), ":4".to_owned()));
            }
            sorted.sort();
            format!("DBus.UpdateActivationEnvironment {sorted:?}")
        };
        vec![
            format!(
                "systemd1.Manager.SetEnvironment [\"WAYLAND_DISPLAY={display_socket}\", \
                 \"XDG_CURRENT_DESKTOP=Lilium\", \"XDG_SESSION_TYPE=wayland\"]"
            ),
            vars(false),
            format!(
                "systemd1.Manager.SetEnvironment [\"WAYLAND_DISPLAY={display_socket}\", \
                 \"DISPLAY=:4\", \"XDG_CURRENT_DESKTOP=Lilium\", \"XDG_SESSION_TYPE=wayland\"]"
            ),
            vars(true),
            "systemd1.Manager.StartUnit solium-session.target replace".to_owned(),
            "systemd1.Manager.StartUnit solium-autostart.target replace".to_owned(),
            "systemd1.Manager.StopUnit solium-session.target replace".to_owned(),
            "systemd1.Manager.UnsetEnvironment [\"WAYLAND_DISPLAY\", \
             \"XDG_CURRENT_DESKTOP\", \"XDG_SESSION_TYPE\", \"DISPLAY\"]"
                .to_owned(),
        ]
    }

    /// Each [`Call`] arrives as the method, interface, path and arguments
    /// systemd and the bus driver answer to, through the bus thread and in
    /// the order the session sent them, after the one question [`claim`]
    /// asks.
    #[test]
    fn the_calls_reach_systemd_and_dbus_as_their_methods() {
        let bus = StandIn::new(&[], true);
        let mut expected = vec![LISTED.to_owned()];
        expected.extend(told("wayland-9"));
        assert_eq!(bus.play(Settings::default()), expected);
    }

    /// A crash leaves the targets up and `graphical-session.target` with them,
    /// bound to a display that has gone. The next session stops them before
    /// it says anything, and then starts its own.
    #[test]
    fn a_session_left_running_is_stopped_before_the_first_export() {
        let bus = StandIn::new(
            &[
                (GRAPHICAL, "active", ""),
                (TARGET, "active", ""),
                (AUTOSTART, "active", ""),
            ],
            true,
        );
        let mut expected = vec![
            LISTED.to_owned(),
            "systemd1.Manager.StopUnit solium-session.target replace".to_owned(),
            "systemd1.Manager.StopUnit solium-autostart.target replace".to_owned(),
            LISTED.to_owned(),
        ];
        expected.extend(told("wayland-9"));
        assert_eq!(bus.play(Settings::default()), expected);
    }

    /// Plasma on another VT holds `graphical-session.target`, and shares this
    /// user's systemd and bus. Nothing is exported, started, stopped or unset
    /// over it, before or at exit.
    #[test]
    fn another_desktop_holding_the_graphical_session_is_left_alone() {
        let bus = StandIn::new(&[(GRAPHICAL, "active", "")], true);
        assert_eq!(bus.play(Settings::default()), vec![LISTED.to_owned()]);

        // On its way down it holds nothing: a logout of that desktop is ending.
        let bus = StandIn::new(&[(GRAPHICAL, "active", "stop")], true);
        let mut expected = vec![LISTED.to_owned()];
        expected.extend(told("wayland-9"));
        assert_eq!(bus.play(Settings::default()), expected);
    }

    #[test]
    fn a_systemd_that_cannot_be_asked_is_told_anyway() {
        let bus = StandIn::new(&[], false);
        assert_eq!(bus.play(Settings::default()), told("wayland-9"));
    }
}
