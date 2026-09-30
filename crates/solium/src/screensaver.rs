//! `org.freedesktop.ScreenSaver`: the idle inhibitors that come over the
//! session bus instead of over Wayland (#152).
//!
//! A browser keeps the screen on during a film over D-Bus, not over
//! `zwp_idle_inhibit_v1` (`idle.rs`). Chrome and Chromium call
//! `org.freedesktop.ScreenSaver.Inhibit`. So does Firefox, first, for a tab
//! in view: its `widget/gtk/WakeLockListener.cpp` tries its kinds of lock in
//! the order of its `WakeLockType` enum, and `FreeDesktopScreensaver` comes
//! before the portal. The portal's own `Inhibit`, for whatever does use it,
//! goes to xdg-desktop-portal-gtk (`dev/session/lilium-portals.conf`), whose
//! `inhibit.c` makes the same call when no GNOME session is running. Nothing
//! answered it, so a film went dark after `idle.screens_off_after`.
//!
//! So while `idle.dbus_inhibit` is on, Solium owns the name and answers
//! `Inhibit(application, reason) -> cookie` and `UnInhibit(cookie)` at both
//! `/org/freedesktop/ScreenSaver` and `/ScreenSaver`, since clients use
//! either. What it holds counts as a Wayland idle inhibitor on a window in
//! view counts (`Solium::idle_inhibited`): the idle blank waits for it, and
//! so does every `ext-idle-notify` notification that honours inhibitors,
//! which is what `swayidle` asks for; and behind the lock screen it holds
//! nothing. `a_dbus_inhibit_holds_the_idle_blank_off_and_uninhibit_lets_it_happen`,
//! `a_dbus_inhibitor_holds_nothing_behind_the_lock_screen` and
//! `idle_dbus_inhibit_false_owns_nothing`.
//!
//! An inhibitor belongs to the connection that took it, and goes when that
//! connection leaves the bus, so a browser that crashes does not keep the
//! screens on for ever: `a_caller_that_leaves_the_bus_drops_its_inhibitors`.
//! A cookie the caller does not hold is ignored:
//! `an_unknown_cookie_is_ignored`. A name another desktop already owns on
//! this bus is left to it: `another_owner_of_the_name_is_left_alone`.
//!
//! The bus is spoken to from a thread of its own, as `session.rs` does, and
//! the event loop only reads what that thread has written.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::session::Place;

/// The name owned.
const NAME: &str = "org.freedesktop.ScreenSaver";
/// Where it is answered: Chromium and xdg-desktop-portal-gtk call the first,
/// Firefox the second. `a_caller_that_leaves_the_bus_drops_its_inhibitors`
/// takes one at each.
const PATHS: [&str; 2] = ["/org/freedesktop/ScreenSaver", "/ScreenSaver"];
const DBUS: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";

/// One `Inhibit`, until its `UnInhibit` or its caller leaving the bus.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Inhibitor {
    cookie: u32,
    /// The caller's unique name on the bus, `:1.42`.
    owner: String,
    application: String,
    reason: String,
}

/// Every inhibitor held over the bus.
#[derive(Debug, Default)]
struct Held {
    /// The last cookie handed out.
    last: u32,
    inhibitors: Vec<Inhibitor>,
}

impl Held {
    /// A cookie that is never 0 and never one still held: xdg-desktop-portal-gtk
    /// reads 0 as "no answer yet", and would then never let go.
    /// `cookies_are_never_zero_and_never_one_in_use`.
    fn inhibit(&mut self, owner: &str, application: String, reason: String) -> u32 {
        let cookie = loop {
            self.last = self.last.wrapping_add(1);
            let cookie = self.last;
            if cookie != 0 && !self.inhibitors.iter().any(|each| each.cookie == cookie) {
                break cookie;
            }
        };
        self.inhibitors.push(Inhibitor {
            cookie,
            owner: owner.to_owned(),
            application,
            reason,
        });
        cookie
    }

    /// The caller's own inhibitor with this cookie, if it holds one.
    fn uninhibit(&mut self, owner: &str, cookie: u32) -> Option<Inhibitor> {
        let index = self
            .inhibitors
            .iter()
            .position(|each| each.cookie == cookie && each.owner == owner)?;
        Some(self.inhibitors.remove(index))
    }

    /// Everything `owner` held, now that it has left the bus.
    fn left(&mut self, owner: &str) -> Vec<Inhibitor> {
        let (gone, kept) = std::mem::take(&mut self.inhibitors)
            .into_iter()
            .partition(|each| each.owner == owner);
        self.inhibitors = kept;
        gone
    }
}

type Shared = Arc<Mutex<Held>>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Which bus the name may be owned on, as `session::speaks` decides for the
/// session: `a_nested_run_owns_no_name_unless_it_is_given_a_bus`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Bus {
    /// None: every Solium before its backend has said, and a start that is
    /// not the session with no `SOLIUM_SESSION_BUS`.
    #[default]
    Nowhere,
    /// The session bus for `None`, or the bus named.
    On(Option<String>),
}

/// `org.freedesktop.ScreenSaver`, owned while `idle.dbus_inhibit` is on and
/// the backend has said on which bus.
#[derive(Debug, Default)]
pub(crate) struct ScreenSaver {
    bus: Bus,
    service: Option<Service>,
}

impl ScreenSaver {
    /// The backend has started: `place` and `SOLIUM_SESSION_BUS` say which
    /// bus, as they do for the session, and `on` is `idle.dbus_inhibit`.
    pub(crate) fn permit(&mut self, place: Place, bus: Option<String>, on: bool) {
        self.bus = if crate::session::speaks(place, bus.as_deref()) {
            Bus::On(bus)
        } else {
            tracing::info!(
                ?place,
                "not started as the session: {NAME} is left to the desktop around this one. \
                 SOLIUM_SESSION_BUS names a bus to own it on instead"
            );
            Bus::Nowhere
        };
        if !on {
            tracing::info!("idle.dbus_inhibit is off: {NAME} is not owned");
        }
        self.serve(on);
    }

    /// `idle.dbus_inhibit`, at start and on every reload: the name is owned
    /// or let go to match. `idle_dbus_inhibit_false_owns_nothing`.
    pub(crate) fn serve(&mut self, on: bool) {
        match (&self.bus, on, self.service.is_some()) {
            (Bus::On(address), true, false) => self.service = Service::start(address.clone()),
            (_, false, true) => {
                self.service = None;
                tracing::info!("idle.dbus_inhibit is off: {NAME} let go");
            }
            _ => {}
        }
    }

    /// Whether anything is holding the screens on over the bus.
    pub(crate) fn holding(&self) -> bool {
        self.service
            .as_ref()
            .is_some_and(|service| !lock(&service.held).inhibitors.is_empty())
    }

    #[cfg(test)]
    fn serving(&self) -> bool {
        self.service.is_some()
    }
}

/// The thread's connection, as far as the event loop knows it.
#[derive(Debug)]
enum Link {
    Connecting,
    Up(zbus::blocking::Connection),
    /// Let go: a connection made after this is closed straight away.
    Ended,
}

/// The name owned, from a thread of its own.
#[derive(Debug)]
struct Service {
    held: Shared,
    link: Arc<Mutex<Link>>,
}

impl Service {
    fn start(address: Option<String>) -> Option<Self> {
        let held = Shared::default();
        let link = Arc::new(Mutex::new(Link::Connecting));
        let spawned = std::thread::Builder::new()
            .name("solium-screensaver".to_owned())
            .spawn({
                let held = held.clone();
                let link = link.clone();
                move || run(address.as_deref(), &held, &link)
            });
        match spawned {
            Ok(_) => Some(Self { held, link }),
            Err(err) => {
                tracing::warn!(
                    ?err,
                    "no thread for {NAME}: browsers' idle inhibitors are not heard"
                );
                None
            }
        }
    }
}

impl Drop for Service {
    /// Nothing held counts from here on, and the connection is closed, which
    /// gives up the name and ends the thread. Closed from a thread of its own,
    /// so that letting go never waits on the bus:
    /// `letting_go_of_a_bus_that_never_answers_does_not_wait_for_it`.
    fn drop(&mut self) {
        lock(&self.held).inhibitors.clear();
        let link = std::mem::replace(&mut *lock(&self.link), Link::Ended);
        if let Link::Up(connection) = link {
            let closing = std::thread::Builder::new()
                .name("solium-screensaver-close".to_owned())
                .spawn(move || connection.close());
            if let Err(err) = closing {
                tracing::warn!(?err, "no thread to close {NAME}'s connection");
            }
        }
    }
}

/// The thread: connect, own the name, and hear who leaves the bus until the
/// connection closes.
fn run(address: Option<&str>, held: &Shared, link: &Mutex<Link>) {
    let connection = match crate::session::connect(address) {
        Ok(connection) => connection,
        Err(err) => {
            tracing::warn!(
                ?err,
                bus = address,
                "no session bus: browsers' idle inhibitors ({NAME}) are not heard, so a \
                 film in one can go dark"
            );
            return;
        }
    };
    {
        let mut link = lock(link);
        if matches!(*link, Link::Ended) {
            drop(link);
            let _ = connection.close();
            return;
        }
        *link = Link::Up(connection.clone());
    }
    if let Err(err) = serve(&connection, held) {
        tracing::warn!(?err, "{NAME}: the session bus refused this");
        let _ = connection.close();
    }
    // The bus has gone, or the name was let go: nothing it held counts.
    lock(held).inhibitors.clear();
}

fn serve(connection: &zbus::blocking::Connection, held: &Shared) -> zbus::Result<()> {
    // Heard from before the name is owned, so that no caller can leave
    // unheard. From the bus alone: a client could send this signal too, and
    // zbus holds each message to the rule's sender.
    // `a_caller_that_leaves_the_bus_drops_its_inhibitors`.
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(DBUS)?
        .path(DBUS_PATH)?
        .interface(DBUS)?
        .member("NameOwnerChanged")?
        .build();
    let departures = zbus::blocking::MessageIterator::for_match_rule(rule, connection, None)?;
    for path in PATHS {
        connection
            .object_server()
            .at(path, Interface { held: held.clone() })?;
    }
    // Never queued for, and never taken: another desktop on this bus keeps
    // it. `another_owner_of_the_name_is_left_alone`.
    match connection.request_name_with_flags(NAME, zbus::fdo::RequestNameFlags::DoNotQueue.into()) {
        Ok(_) => tracing::info!(
            "owning {NAME}: a browser's idle inhibitor holds the screens on (idle.dbus_inhibit)"
        ),
        Err(zbus::Error::NameTaken) => {
            tracing::warn!(
                "{NAME} is already owned on this bus, by another desktop: browsers' idle \
                 inhibitors go to it and not to Solium, and Solium leaves it the name"
            );
            let _ = connection.clone().close();
            return Ok(());
        }
        Err(err) => return Err(err),
    }
    for message in departures {
        let Ok(message) = message else {
            continue;
        };
        let Ok((name, _, owner)) = message.body().deserialize::<(String, String, String)>() else {
            continue;
        };
        if !owner.is_empty() {
            continue;
        }
        let gone = lock(held).left(&name);
        for inhibitor in gone {
            tracing::info!(
                owner = name,
                application = inhibitor.application,
                cookie = inhibitor.cookie,
                "left the bus: its idle inhibitor goes with it"
            );
        }
    }
    Ok(())
}

/// The interface, at each of [`PATHS`].
struct Interface {
    held: Shared,
}

#[zbus::interface(name = "org.freedesktop.ScreenSaver")]
impl Interface {
    #[zbus(out_args("cookie"))]
    async fn inhibit(
        &self,
        application: String,
        reason: String,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> zbus::fdo::Result<u32> {
        let owner = header
            .sender()
            .map(ToString::to_string)
            .ok_or_else(|| zbus::fdo::Error::Failed("an Inhibit with no sender".to_owned()))?;
        let cookie = lock(&self.held).inhibit(&owner, application.clone(), reason.clone());
        // Held first and asked about second: a caller that left before this
        // ran may already have been heard leaving, and then only the question
        // finds it gone. `a_caller_that_leaves_the_bus_drops_its_inhibitors`.
        match on_the_bus(connection, &owner).await {
            Ok(true) => tracing::info!(
                application,
                reason,
                owner,
                cookie,
                "holds the screens on ({NAME}.Inhibit)"
            ),
            Ok(false) => {
                lock(&self.held).uninhibit(&owner, cookie);
                tracing::info!(
                    application,
                    owner,
                    "left the bus before its Inhibit was answered: nothing held"
                );
            }
            Err(err) => tracing::warn!(
                ?err,
                owner,
                "could not ask the bus whether this caller is still on it; holding it"
            ),
        }
        Ok(cookie)
    }

    #[zbus(name = "UnInhibit")]
    fn un_inhibit(&self, cookie: u32, #[zbus(header)] header: zbus::message::Header<'_>) {
        let owner = header.sender().map(ToString::to_string).unwrap_or_default();
        let released = lock(&self.held).uninhibit(&owner, cookie);
        match released {
            Some(inhibitor) => tracing::info!(
                application = inhibitor.application,
                owner,
                cookie,
                "lets the screens go ({NAME}.UnInhibit)"
            ),
            None => tracing::info!(
                owner,
                cookie,
                "{NAME}.UnInhibit of a cookie this caller does not hold: ignored"
            ),
        }
    }
}

async fn on_the_bus(connection: &zbus::Connection, name: &str) -> zbus::Result<bool> {
    connection
        .call_method(Some(DBUS), DBUS_PATH, Some(DBUS), "NameHasOwner", &name)
        .await?
        .body()
        .deserialize::<bool>()
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        collections::HashSet,
        path::PathBuf,
        sync::{
            atomic::{AtomicU32, Ordering},
            mpsc,
        },
        time::{Duration, Instant},
    };

    use zbus::message::Message;

    use super::*;

    /// Long enough for anything a test waits on to have happened.
    const WITHIN: Duration = Duration::from_secs(5);

    /// What the stand-in bus heard Solium ask it, as `Method args`.
    type Heard = Arc<Mutex<Vec<String>>>;
    /// The unique names the stand-in bus says are on it.
    type Present = Arc<Mutex<HashSet<String>>>;

    struct Driver {
        heard: Heard,
        present: Present,
        /// Another desktop already owns [`NAME`].
        taken: bool,
    }

    #[zbus::interface(name = "org.freedesktop.DBus")]
    impl Driver {
        fn hello(&self) -> String {
            ":1.1".to_owned()
        }
        fn add_match(&self, _rule: String) {}
        fn remove_match(&self, _rule: String) {}
        fn request_name(&self, name: String, flags: u32) -> u32 {
            self.heard
                .lock()
                .expect("the list")
                .push(format!("RequestName {name} {flags}"));
            // DBUS_REQUEST_NAME_REPLY_EXISTS, or _PRIMARY_OWNER.
            if self.taken { 3 } else { 1 }
        }
        fn release_name(&self, _name: String) -> u32 {
            1
        }
        fn name_has_owner(&self, name: String) -> bool {
            self.present.lock().expect("the names").contains(&name)
        }
    }

    /// A stand-in session bus: the bus driver, served by zbus at a socket of
    /// the test's own to the one connection Solium makes, and the other
    /// clients' calls and departures, sent down that connection as the bus
    /// would deliver them. No bus, real or private, anywhere near.
    pub(crate) struct StandInBus {
        pub(crate) address: String,
        directory: PathBuf,
        heard: Heard,
        present: Present,
        peers: mpsc::Receiver<zbus::blocking::Connection>,
        peer: Mutex<Option<zbus::blocking::Connection>>,
        closed: mpsc::Receiver<()>,
    }

    impl StandInBus {
        pub(crate) fn new(taken: bool) -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let directory = std::env::temp_dir().join(format!(
                "solium-screensaver-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&directory);
            std::fs::create_dir_all(&directory).expect("a temporary directory");
            let path = directory.join("bus");
            let listener =
                std::os::unix::net::UnixListener::bind(&path).expect("binding the socket");
            let heard = Heard::default();
            let present = Present::default();
            let (found, peers) = mpsc::channel();
            let (ended, closed) = mpsc::channel();
            {
                let heard = heard.clone();
                let present = present.clone();
                std::thread::spawn(move || {
                    let Ok((stream, _)) = listener.accept() else {
                        return;
                    };
                    let Ok(connection) =
                        zbus::blocking::connection::Builder::async_io_unix_stream(stream)
                            .server(zbus::Guid::generate())
                            .expect("a server guid")
                            .p2p()
                            .serve_at(
                                DBUS_PATH,
                                Driver {
                                    heard,
                                    present,
                                    taken,
                                },
                            )
                            .expect("serving the driver")
                            .build()
                    else {
                        return;
                    };
                    let _ = found.send(connection.clone());
                    connection.closed();
                    let _ = ended.send(());
                });
            }
            Self {
                address: format!("unix:path={}", path.display()),
                directory,
                heard,
                present,
                peers,
                peer: Mutex::new(None),
                closed,
            }
        }

        /// The bus's end of Solium's connection, once it has made one within
        /// `within`.
        fn peer(&self, within: Duration) -> Option<zbus::blocking::Connection> {
            let mut peer = self.peer.lock().expect("the peer");
            if peer.is_none() {
                *peer = self.peers.recv_timeout(within).ok();
            }
            peer.clone()
        }

        /// Whether Solium connected at all within `within`.
        pub(crate) fn connected(&self, within: Duration) -> bool {
            self.peer(within).is_some()
        }

        /// Every `RequestName` Solium has made, once it has made one.
        pub(crate) fn requests(&self) -> Vec<String> {
            let deadline = Instant::now() + WITHIN;
            loop {
                let heard = self.heard.lock().expect("the list").clone();
                if !heard.is_empty() || Instant::now() > deadline {
                    return heard;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        /// Whether Solium closed its connection within `within`.
        pub(crate) fn closed(&self, within: Duration) -> bool {
            self.closed.recv_timeout(within).is_ok()
        }

        /// A client with this unique name is on the bus.
        pub(crate) fn arrive(&self, name: &str) {
            self.present
                .lock()
                .expect("the names")
                .insert(name.to_owned());
        }

        /// It leaves, and the bus says so.
        pub(crate) fn leave(&self, name: &str) {
            self.present.lock().expect("the names").remove(name);
            self.announce(DBUS, name);
        }

        /// `NameOwnerChanged` for `name` going, sent by `sender`: the bus, or
        /// a client pretending to be it.
        pub(crate) fn announce(&self, sender: &str, name: &str) {
            let signal = Message::signal(DBUS_PATH, DBUS, "NameOwnerChanged")
                .and_then(|builder| builder.sender(sender))
                .and_then(|builder| builder.build(&(name, name, "")))
                .expect("a NameOwnerChanged signal");
            self.peer(WITHIN)
                .expect("Solium connected")
                .send(&signal)
                .expect("sending the signal");
        }

        /// A method call from `from` to [`NAME`] at `path`, and its answer.
        fn call(
            &self,
            from: &str,
            path: &str,
            member: &str,
            build: impl FnOnce(zbus::message::Builder<'_>) -> zbus::Result<Message>,
        ) -> zbus::Result<Message> {
            let peer = self.peer(WITHIN).expect("Solium connected");
            let call = Message::method_call(path, member)
                .and_then(|builder| builder.sender(from))
                .and_then(|builder| builder.destination(NAME))
                .and_then(|builder| builder.interface(NAME))
                .and_then(build)?;
            let serial = call.primary_header().serial_num();
            let (answered, answer) = mpsc::channel();
            let replies = zbus::blocking::MessageIterator::from(&peer);
            std::thread::spawn(move || {
                for reply in replies.flatten() {
                    if reply.header().reply_serial() == Some(serial) {
                        let _ = answered.send(reply);
                        return;
                    }
                }
            });
            peer.send(&call)?;
            let reply = answer
                .recv_timeout(WITHIN)
                .map_err(|_| zbus::Error::Failure(format!("{member}: no answer")))?;
            match reply.message_type() {
                zbus::message::Type::Error => Err(zbus::Error::from(reply)),
                _ => Ok(reply),
            }
        }

        /// `Inhibit` from `from`, at `path`: the cookie.
        pub(crate) fn inhibit(&self, from: &str, path: &str) -> u32 {
            self.call(from, path, "Inhibit", |builder| {
                builder.build(&("firefox", "video-playing"))
            })
            .expect("Inhibit answered")
            .body()
            .deserialize::<u32>()
            .expect("a cookie")
        }

        /// `UnInhibit` from `from`, answered without an error.
        pub(crate) fn uninhibit(&self, from: &str, cookie: u32) {
            self.call(from, PATHS[0], "UnInhibit", |builder| {
                builder.build(&cookie)
            })
            .expect("UnInhibit answered without an error");
        }
    }

    impl Drop for StandInBus {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    /// Whether `test` comes true within [`WITHIN`].
    pub(crate) fn soon(mut test: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + WITHIN;
        while Instant::now() < deadline {
            if test() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        test()
    }

    /// The name owned on `bus`, from a nested run given it.
    fn owning(bus: &StandInBus) -> ScreenSaver {
        let mut saver = ScreenSaver::default();
        saver.permit(Place::Nested, Some(bus.address.clone()), true);
        assert_eq!(
            bus.requests(),
            [format!("RequestName {NAME} 4")],
            "the name was not asked for, or not with DO_NOT_QUEUE alone"
        );
        saver
    }

    #[test]
    fn cookies_are_never_zero_and_never_one_in_use() {
        let mut held = Held {
            last: u32::MAX - 1,
            ..Held::default()
        };
        let a = held.inhibit(":1.2", "a".into(), String::new());
        assert_eq!(a, u32::MAX);
        let b = held.inhibit(":1.2", "b".into(), String::new());
        assert_eq!(b, 1, "0 was handed out, or the count did not wrap");
        held.last = 0;
        let c = held.inhibit(":1.2", "c".into(), String::new());
        assert_eq!(c, 2, "1 is still held, and was handed out again");
    }

    /// `UnInhibit` of a cookie nobody holds, or one another caller holds, is
    /// answered without an error and lets nothing go.
    #[test]
    fn an_unknown_cookie_is_ignored() {
        let bus = StandInBus::new(false);
        let saver = owning(&bus);
        bus.arrive(":1.42");
        bus.arrive(":1.43");
        let cookie = bus.inhibit(":1.42", PATHS[0]);
        assert!(saver.holding(), "an Inhibit held nothing");

        bus.uninhibit(":1.42", cookie.wrapping_add(100));
        assert!(saver.holding(), "a cookie nobody holds let the film go");
        bus.uninhibit(":1.43", cookie);
        assert!(
            saver.holding(),
            "another caller's UnInhibit let this one's film go"
        );
        bus.uninhibit(":1.42", cookie);
        assert!(!saver.holding(), "its own UnInhibit let nothing go");
    }

    /// A caller that leaves the bus -- closes, crashes, is killed -- takes
    /// its inhibitors with it, at either path. So does one that has left
    /// before its `Inhibit` is answered. A departure announced by a client
    /// rather than by the bus changes nothing.
    ///
    /// Without the `NameOwnerChanged` watch, fails at "left the bus"; without
    /// the question after holding, at "before its Inhibit was answered";
    /// without the sender checked, at "pretending to be the bus".
    #[test]
    fn a_caller_that_leaves_the_bus_drops_its_inhibitors() {
        let bus = StandInBus::new(false);
        let saver = owning(&bus);
        bus.arrive(":1.42");
        bus.arrive(":1.43");
        bus.inhibit(":1.42", PATHS[0]);
        bus.inhibit(":1.43", PATHS[1]);

        // In order on one connection: once the second is heard, so is the
        // first.
        bus.announce(":1.66", ":1.42");
        bus.leave(":1.43");
        assert!(
            soon(|| lock(&saver.service.as_ref().expect("serving").held)
                .inhibitors
                .iter()
                .all(|each| each.owner == ":1.42")),
            "a caller left the bus and its inhibitor at /ScreenSaver stayed"
        );
        assert!(
            saver.holding(),
            "a client pretending to be the bus let another's film go"
        );

        bus.leave(":1.42");
        assert!(
            soon(|| !saver.holding()),
            "a caller left the bus and its inhibitor stayed: a crashed browser keeps the \
             screens on for ever"
        );

        // Never arrived, so gone by the time it is asked about.
        bus.inhibit(":1.50", PATHS[0]);
        assert!(
            !saver.holding(),
            "a caller that left before its Inhibit was answered is still holding the \
             screens on"
        );
    }

    /// Another desktop on this bus owns the name: asked for once, without
    /// replacing it or queueing for it, and the connection closed. Nothing
    /// is held, and Solium carries on.
    ///
    /// With `ReplaceExisting` or without `DoNotQueue`, fails at "4"; staying
    /// on the bus, at "stayed on the bus".
    #[test]
    fn another_owner_of_the_name_is_left_alone() {
        let bus = StandInBus::new(true);
        let saver = owning(&bus);
        assert!(
            bus.closed(WITHIN),
            "the name was taken and Solium stayed on the bus"
        );
        assert!(!saver.holding());
    }

    /// A nested run, or `solium --tty` by hand, with no `SOLIUM_SESSION_BUS`
    /// owns nothing, and never connects to the session bus around it.
    #[test]
    fn a_nested_run_owns_no_name_unless_it_is_given_a_bus() {
        for place in [Place::Nested, Place::Console] {
            let mut saver = ScreenSaver::default();
            saver.permit(place, None, true);
            assert!(
                !saver.serving(),
                "{place:?} started serving the session bus"
            );
        }
        let bus = StandInBus::new(false);
        let _saver = owning(&bus);
    }

    /// A bus that accepts and never speaks: letting go returns at once rather
    /// than waiting on it.
    #[test]
    fn letting_go_of_a_bus_that_never_answers_does_not_wait_for_it() {
        let directory = std::env::temp_dir().join(format!(
            "solium-screensaver-test-silent-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let path = directory.join("bus");
        let listener = std::os::unix::net::UnixListener::bind(&path).expect("binding the socket");
        let holder = std::thread::spawn(move || listener.accept().map(|(stream, _)| stream));

        let mut saver = ScreenSaver::default();
        saver.permit(
            Place::Nested,
            Some(format!("unix:path={}", path.display())),
            true,
        );
        std::thread::sleep(Duration::from_millis(100));
        let started = Instant::now();
        saver.serve(false);
        assert!(!saver.serving());
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "{:?}",
            started.elapsed()
        );
        drop(holder);
        let _ = std::fs::remove_dir_all(&directory);
    }
}
