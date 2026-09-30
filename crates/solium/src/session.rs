//! Telling the rest of the user session that Solium is its desktop (#146).
//!
//! Portals, D-Bus-activated programs, XDG autostart and user services bound to
//! `graphical-session.target` are all started by systemd's user manager or by
//! D-Bus activation, and neither has ever heard of this compositor's socket.
//! So once it is up Solium tells them, over the session bus:
//!
//! 1. `org.freedesktop.systemd1.Manager.SetEnvironment` and
//!    `org.freedesktop.DBus.UpdateActivationEnvironment` with
//!    `WAYLAND_DISPLAY`, `XDG_CURRENT_DESKTOP` and `XDG_SESSION_TYPE`, and
//!    again with `DISPLAY` once XWayland has one;
//! 2. `StartUnit` on `solium-session.target`, once X11 has answered one way or
//!    the other, so an X11 program in `~/.config/autostart` finds `DISPLAY`;
//! 3. on exit, `StopUnit` on that target and `UnsetEnvironment` on the
//!    variables, so a Plasma login afterwards inherits no dead socket.
//!
//! `the_environment_goes_out_when_the_socket_is_up_and_again_with_display`,
//! `the_target_is_stopped_and_the_variables_unset_on_exit` and
//! `the_calls_reach_systemd_and_dbus_as_their_methods` hold it to that.
//!
//! The calls are made from a thread of their own, because a session bus that
//! is slow to answer must never hold a frame: see [`Worker`].

use std::{collections::HashMap, sync::mpsc, time::Duration};

/// The target a session starts, with XDG autostart. `dev/session/` ships it.
pub(crate) const TARGET: &str = "solium-session.target";
/// The same target without `Wants=xdg-desktop-autostart.target`, for
/// `session.autostart = false`: a unit's dependencies are fixed when it is
/// loaded, so the choice is between two units.
/// `session_autostart_false_starts_the_target_without_autostart`.
pub(crate) const TARGET_WITHOUT_AUTOSTART: &str = "solium-session-no-autostart.target";

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
    /// Start [`TARGET`] rather than [`TARGET_WITHOUT_AUTOSTART`].
    pub(crate) autostart: bool,
}

impl Default for Settings {
    /// Both on. `the_shipped_configuration_tells_the_session_and_starts_autostart`.
    fn default() -> Self {
        Self {
            systemd: true,
            autostart: true,
        }
    }
}

/// Which backend is starting a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    /// `--tty`: this compositor *is* the session.
    Hardware,
    /// A window inside another session, whose environment belongs to that
    /// session. Told nothing unless `SOLIUM_SESSION_BUS` names a bus.
    /// `a_nested_run_tells_nobody_unless_it_is_given_a_bus`.
    Nested,
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
    target: &'static str,
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
            .field("target", &self.target)
            .field("started", &self.started)
            .field("exported", &self.exported)
            .finish_non_exhaustive()
    }
}

impl Session {
    /// A session that tells nobody anything: `session.systemd = false`, a
    /// nested run, and every `Solium` before its backend starts one.
    pub(crate) fn off() -> Self {
        Self {
            sink: None,
            target: TARGET,
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
    /// bus, and nested the only way to tell one at all.
    pub(crate) fn begin(settings: Settings, place: Place, bus: Option<String>) -> Self {
        if !settings.systemd {
            tracing::info!(
                "session.systemd is off: systemd and D-Bus activation are not told about \
                 this session, and {TARGET} is not started"
            );
            return Self::off();
        }
        if place == Place::Nested && bus.is_none() {
            tracing::debug!(
                "nested: the session this runs inside keeps its own environment \
                 (SOLIUM_SESSION_BUS names a bus to tell instead)"
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
            target: if settings.autostart {
                TARGET
            } else {
                TARGET_WITHOUT_AUTOSTART
            },
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
        if self.started {
            sink.send(Call::StopUnit(self.target.to_owned()));
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
        sink.send(Call::StartUnit(self.target.to_owned()));
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
fn desktop(set: Option<String>) -> String {
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
                        for call in incoming {
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
        None => zbus::blocking::connection::Builder::session()?,
    };
    builder.method_timeout(PATIENCE).build()
}

const SYSTEMD: &str = "org.freedesktop.systemd1";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const SYSTEMD_MANAGER: &str = "org.freedesktop.systemd1.Manager";
const DBUS: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";

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
        sync::{Arc, Mutex},
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

    /// Exactly those variables, at both moments, and the target once X11 has
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
        expected.push(Call::StartUnit(TARGET.to_owned()));
        assert_eq!(recorder.take(), expected);

        // XWayland answers once; a repeat says nothing new.
        session.x11(Some(3));
        assert_eq!(recorder.take(), Vec::new());
    }

    /// No X11 coming: the target starts on the first export alone, and a
    /// display that turns up late is still exported.
    #[test]
    fn without_x11_the_target_starts_after_the_first_export() {
        let (mut session, recorder) = session(Settings::default());
        session.wayland("wayland-1");
        session.x11(None);
        let calls = recorder.take();
        assert_eq!(calls.len(), 3, "{calls:?}");
        assert_eq!(calls[2], Call::StartUnit(TARGET.to_owned()));

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
                autostart: true,
            },
            Place::Hardware,
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

    #[test]
    fn session_autostart_false_starts_the_target_without_autostart() {
        let (mut session, recorder) = session(Settings {
            systemd: true,
            autostart: false,
        });
        session.wayland("wayland-5");
        session.x11(None);
        assert_eq!(
            recorder.take().last(),
            Some(&Call::StartUnit(TARGET_WITHOUT_AUTOSTART.to_owned()))
        );
        session.end();
        assert_eq!(
            recorder.take().first(),
            Some(&Call::StopUnit(TARGET_WITHOUT_AUTOSTART.to_owned()))
        );
    }

    /// Never the session a nested window runs inside: that is the developer's
    /// own desktop, and its `WAYLAND_DISPLAY` is not ours to replace.
    #[test]
    fn a_nested_run_tells_nobody_unless_it_is_given_a_bus() {
        assert!(Session::begin(Settings::default(), Place::Nested, None).is_off());
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
        assert_eq!(state.session.target, TARGET);
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
        let directory =
            std::env::temp_dir().join(format!("solium-session-test-silent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
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

    /// What the stand-in heard, as `interface.Method args`.
    type Heard = Arc<Mutex<Vec<String>>>;

    struct StandInSystemd(Heard);

    #[zbus::interface(name = "org.freedesktop.systemd1.Manager")]
    impl StandInSystemd {
        fn set_environment(&self, assignments: Vec<String>) {
            self.0
                .lock()
                .expect("the list")
                .push(format!("systemd1.Manager.SetEnvironment {assignments:?}"));
        }
        fn unset_environment(&self, names: Vec<String>) {
            self.0
                .lock()
                .expect("the list")
                .push(format!("systemd1.Manager.UnsetEnvironment {names:?}"));
        }
        fn start_unit(&self, name: String, mode: String) -> zbus::zvariant::OwnedObjectPath {
            self.0
                .lock()
                .expect("the list")
                .push(format!("systemd1.Manager.StartUnit {name} {mode}"));
            zbus::zvariant::OwnedObjectPath::try_from("/org/freedesktop/systemd1/job/1")
                .expect("a job path")
        }
        fn stop_unit(&self, name: String, mode: String) -> zbus::zvariant::OwnedObjectPath {
            self.0
                .lock()
                .expect("the list")
                .push(format!("systemd1.Manager.StopUnit {name} {mode}"));
            zbus::zvariant::OwnedObjectPath::try_from("/org/freedesktop/systemd1/job/2")
                .expect("a job path")
        }
    }

    struct StandInDriver(Heard);

    #[zbus::interface(name = "org.freedesktop.DBus")]
    impl StandInDriver {
        fn update_activation_environment(&self, environment: HashMap<String, String>) {
            let mut sorted: Vec<_> = environment.into_iter().collect();
            sorted.sort();
            self.0
                .lock()
                .expect("the list")
                .push(format!("DBus.UpdateActivationEnvironment {sorted:?}"));
        }
    }

    /// Each [`Call`] arrives as the method, interface, path and arguments
    /// systemd and the bus driver answer to. Served by zbus over a socket
    /// pair, so no bus, real or private, is anywhere near it.
    #[test]
    fn the_calls_reach_systemd_and_dbus_as_their_methods() {
        let heard = Heard::default();
        let (server_end, client_end) =
            std::os::unix::net::UnixStream::pair().expect("a socket pair");
        let server = {
            let heard = heard.clone();
            std::thread::spawn(move || {
                zbus::blocking::connection::Builder::async_io_unix_stream(server_end)
                    .server(zbus::Guid::generate())
                    .expect("a server guid")
                    .p2p()
                    .serve_at(SYSTEMD_PATH, StandInSystemd(heard.clone()))
                    .expect("serving systemd")
                    .serve_at(DBUS_PATH, StandInDriver(heard))
                    .expect("serving the driver")
                    .build()
                    .expect("the stand-in's connection")
            })
        };
        let client = zbus::blocking::connection::Builder::async_io_unix_stream(client_end)
            .p2p()
            .build()
            .expect("the client's connection");
        let _server = server.join().expect("the stand-in");

        let (mut session, recorder) = session(Settings::default());
        session.wayland("wayland-9");
        session.x11(Some(4));
        session.end();
        for call in recorder.take() {
            perform(&client, &call).expect("the stand-in answers");
        }

        let vars = |display: bool| {
            let mut sorted = vec![
                ("WAYLAND_DISPLAY".to_owned(), "wayland-9".to_owned()),
                ("XDG_CURRENT_DESKTOP".to_owned(), "Lilium".to_owned()),
                ("XDG_SESSION_TYPE".to_owned(), "wayland".to_owned()),
            ];
            if display {
                sorted.push(("DISPLAY".to_owned(), ":4".to_owned()));
            }
            sorted.sort();
            format!("DBus.UpdateActivationEnvironment {sorted:?}")
        };
        assert_eq!(
            *heard.lock().expect("the list"),
            vec![
                "systemd1.Manager.SetEnvironment [\"WAYLAND_DISPLAY=wayland-9\", \
                 \"XDG_CURRENT_DESKTOP=Lilium\", \"XDG_SESSION_TYPE=wayland\"]"
                    .to_owned(),
                vars(false),
                "systemd1.Manager.SetEnvironment [\"WAYLAND_DISPLAY=wayland-9\", \
                 \"DISPLAY=:4\", \"XDG_CURRENT_DESKTOP=Lilium\", \"XDG_SESSION_TYPE=wayland\"]"
                    .to_owned(),
                vars(true),
                "systemd1.Manager.StartUnit solium-session.target replace".to_owned(),
                "systemd1.Manager.StopUnit solium-session.target replace".to_owned(),
                "systemd1.Manager.UnsetEnvironment [\"WAYLAND_DISPLAY\", \
                 \"XDG_CURRENT_DESKTOP\", \"XDG_SESSION_TYPE\", \"DISPLAY\"]"
                    .to_owned(),
            ]
        );
    }
}
