//! `org.freedesktop.Notifications` on the session bus, from threads of its own
//! (03 §3.2.11, "Lifecycle"; §3.1.2).
//!
//! Three threads, so that nothing the event loop does, and nothing a slow
//! peer does, can hold a `Notify` reply back:
//!
//! * **the listener** reads every message on the connection. A `Notify` is
//!   given its id and answered there and then, before anything else is done
//!   with it; what remains goes to the worker. It never makes a call of its
//!   own, so it never waits on the bus while messages queue behind it.
//! * **the worker** does the slow parts in arrival order: decoding images,
//!   asking the bus for a sender's process, finding who owns the name. It
//!   hands the event loop [`Fact`]s, in the order the calls came, so a
//!   `CloseNotification` can never overtake the `Notify` it closes.
//! * **the emitter** sends the signals the event loop asks for. Every signal
//!   about an id goes out after that id's reply, because the reply was sent
//!   before the event loop ever heard of the id.
//!
//! Ids, and who may replace or close which, are decided on the listener by
//! [`Registry`], the one piece of state the threads share with the loop.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc},
};

use zbus::{
    message::{Message, Type},
    zvariant::{OwnedValue, Value},
};

use super::{
    content,
    model::{Incoming, Urgency},
};

pub(crate) const NAME: &str = "org.freedesktop.Notifications";
pub(crate) const PATH: &str = "/org/freedesktop/Notifications";
const DBUS: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";

/// What the server advertises: only what Solium does with what it is sent.
/// Advertising `sound` would make senders stay silent, and nothing plays one
/// yet. `get_capabilities_lists_only_what_solium_does`.
pub(crate) const CAPABILITIES: [&str; 6] = [
    "actions",
    "body",
    "body-hyperlinks",
    "body-markup",
    "icon-static",
    "persistence",
];

/// How many ids are reserved on disk at a time: a run that ends without
/// writing again still starts the next one past every id it gave out.
const ID_BLOCK: u32 = 1024;

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Who another owner of the name is, as far as the bus says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Owner {
    /// Its unique name, `:1.12`.
    pub(crate) name: String,
    pub(crate) pid: Option<u32>,
    /// `/proc/<pid>/comm`: `mako`, `plasmashell`.
    pub(crate) program: String,
}

/// What the event loop hears from the threads, in order.
#[derive(Debug)]
pub(crate) enum Fact {
    Notify(Box<Incoming>),
    /// `CloseNotification` from the one that may.
    Close {
        id: u32,
    },
    /// Whether Solium has the name now.
    Serving(bool),
    /// Who has it, when Solium does not.
    Owner(Option<Owner>),
    /// The connection is gone.
    Down,
}

/// What the event loop asks the emitter to say.
#[derive(Debug)]
pub(crate) enum Signal {
    Closed {
        id: u32,
        reason: u32,
    },
    /// `ActivationToken(id, token)` when there is a token, then
    /// `ActionInvoked(id, key)`.
    Invoked {
        id: u32,
        key: String,
        token: Option<String>,
    },
}

/// One live id: whose it is.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Held {
    /// The app as the sender names it: the `desktop-entry` hint, else the
    /// app name.
    identity: String,
    /// The connection that sent it.
    sender: String,
}

/// Ids, and who holds each.
#[derive(Debug)]
pub(crate) struct Registry {
    /// The last id given out.
    last: u32,
    /// Ids up to here are written down as used.
    reserved: u32,
    state_file: Option<PathBuf>,
    live: HashMap<u32, Held>,
}

impl Registry {
    /// Ids start past every id the last run could have given out, so a client
    /// that kept one from then cannot replace or close someone else's now
    /// (03 §3.2.11, "Ids"). With nowhere to write that down, they start at a
    /// random point instead.
    /// `ids_start_past_the_last_run_s_and_are_never_zero`.
    pub(crate) fn new(state_file: Option<PathBuf>) -> Self {
        let last = state_file
            .as_deref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| text.trim().parse::<u32>().ok())
            .unwrap_or_else(random_start);
        let mut registry = Self {
            last,
            reserved: last,
            state_file,
            live: HashMap::new(),
        };
        registry.reserve();
        registry
    }

    fn reserve(&mut self) {
        self.reserved = self.last.wrapping_add(ID_BLOCK);
        if let Some(path) = &self.state_file {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(err) = std::fs::write(path, self.reserved.to_string()) {
                tracing::debug!(?err, path = %path.display(), "notification ids not written down");
            }
        }
    }

    /// The id for a `Notify`: `replaces` itself when it is live and the same
    /// app's, else a new one, never 0 and never one in use. Whether it
    /// replaced comes back with it.
    /// `only_the_same_app_replaces_a_notification`.
    pub(crate) fn allocate(&mut self, identity: &str, sender: &str, replaces: u32) -> (u32, bool) {
        if replaces != 0
            && let Some(held) = self.live.get_mut(&replaces)
            && held.identity == identity
        {
            held.sender = sender.to_owned();
            return (replaces, true);
        }
        let id = loop {
            self.last = self.last.wrapping_add(1);
            if self.last != 0 && !self.live.contains_key(&self.last) {
                break self.last;
            }
        };
        // At or past the block written down, in wrapping arithmetic: write
        // the next one down before giving out any id from it.
        if self.last.wrapping_sub(self.reserved) < u32::MAX / 2 {
            self.reserve();
        }
        self.live.insert(
            id,
            Held {
                identity: identity.to_owned(),
                sender: sender.to_owned(),
            },
        );
        (id, false)
    }

    /// Whether `sender` may close `id`: only the connection that sent it,
    /// or sent its last replacement.
    /// `only_its_sender_closes_a_notification`.
    pub(crate) fn may_close(&self, id: u32, sender: &str) -> bool {
        self.live.get(&id).is_some_and(|held| held.sender == sender)
    }

    /// `id` is closed: free to be given out again after the counter wraps.
    pub(crate) fn forget(&mut self, id: u32) {
        self.live.remove(&id);
    }

    /// An id for one of the shell's own notifications (`sol.notify`), from
    /// the same counter so it can never clash with a client's.
    pub(crate) fn allocate_own(&mut self) -> u32 {
        self.allocate("solium", "", 0).0
    }
}

fn random_start() -> u32 {
    // Not a secret, only unlikely to match an id a client still holds.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    (nanos ^ std::process::id().rotate_left(16)) & 0x0fff_ffff
}

/// The thread's connection, as far as the event loop knows it.
#[derive(Debug)]
enum Link {
    Connecting,
    Up(zbus::blocking::Connection),
    /// Let go: a connection made after this is closed straight away.
    Ended,
}

/// The server, running.
#[derive(Debug)]
pub(crate) struct Server {
    pub(crate) facts: mpsc::Receiver<Fact>,
    signals: mpsc::Sender<Signal>,
    link: Arc<Mutex<Link>>,
}

impl Server {
    /// Connect to `address` (the session bus for `None`, never in a test) and
    /// ask for the name. `None` when no thread could be made.
    pub(crate) fn start(address: Option<String>, registry: Arc<Mutex<Registry>>) -> Option<Self> {
        let (facts_out, facts) = mpsc::channel();
        let (signals, signals_in) = mpsc::channel();
        let link = Arc::new(Mutex::new(Link::Connecting));
        let spawned = std::thread::Builder::new()
            .name("solium-notifications".to_owned())
            .spawn({
                let link = link.clone();
                let registry = registry.clone();
                move || run(address.as_deref(), &registry, &link, &facts_out, signals_in)
            });
        match spawned {
            Ok(_) => Some(Self {
                facts,
                signals,
                link,
            }),
            Err(err) => {
                tracing::warn!(?err, "no thread for {NAME}: notifications are not received");
                None
            }
        }
    }

    pub(crate) fn say(&self, signal: Signal) {
        let _ = self.signals.send(signal);
    }
}

impl Drop for Server {
    /// The connection is closed, which gives the name up and ends the threads,
    /// from a thread of its own, so letting go never waits on the bus.
    fn drop(&mut self) {
        let link = std::mem::replace(&mut *lock(&self.link), Link::Ended);
        if let Link::Up(connection) = link {
            let closing = std::thread::Builder::new()
                .name("solium-notifications-close".to_owned())
                .spawn(move || connection.close());
            if let Err(err) = closing {
                tracing::warn!(?err, "no thread to close {NAME}'s connection");
            }
        }
    }
}

/// What the listener hands the worker, in arrival order.
enum Work {
    Notify {
        id: u32,
        sender: String,
        call: NotifyCall,
    },
    Close {
        id: u32,
    },
    /// Find out who owns the name now.
    Owner,
}

/// `Notify`'s arguments, as sent.
struct NotifyCall {
    app_name: String,
    app_icon: String,
    summary: String,
    body: String,
    actions: Vec<String>,
    hints: HashMap<String, OwnedValue>,
    expire_timeout: i32,
}

fn run(
    address: Option<&str>,
    registry: &Arc<Mutex<Registry>>,
    link: &Mutex<Link>,
    facts: &mpsc::Sender<Fact>,
    signals: mpsc::Receiver<Signal>,
) {
    let connection = match crate::session::connect(address) {
        Ok(connection) => connection,
        Err(err) => {
            tracing::warn!(
                ?err,
                bus = address,
                "no session bus: notifications are not received"
            );
            let _ = facts.send(Fact::Down);
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
    if let Err(err) = serve(&connection, registry, facts, signals) {
        tracing::warn!(?err, "{NAME}: the session bus refused this");
        let _ = connection.close();
    }
    let _ = facts.send(Fact::Down);
}

fn serve(
    connection: &zbus::blocking::Connection,
    registry: &Arc<Mutex<Registry>>,
    facts: &mpsc::Sender<Fact>,
    signals: mpsc::Receiver<Signal>,
) -> zbus::Result<()> {
    // Read from before the name is asked for, so no call and no
    // `NameAcquired` is missed.
    let messages = zbus::blocking::MessageIterator::from(connection);
    let (work_out, work) = mpsc::channel();
    spawn_worker(connection.clone(), work, facts.clone())?;
    spawn_emitter(connection.clone(), signals)?;
    // Who owns the name, as it changes, from the bus alone.
    let changes = zbus::MatchRule::builder()
        .msg_type(Type::Signal)
        .sender(DBUS)?
        .path(DBUS_PATH)?
        .interface(DBUS)?
        .member("NameOwnerChanged")?
        .arg(0, NAME)?
        .build();
    zbus::blocking::fdo::DBusProxy::new(connection)?.add_match_rule(changes)?;
    // Queued for politely, and given up to whoever asks to replace it: a
    // desktop around this one that has the name keeps it.
    // `another_owner_keeps_the_name_and_is_named`.
    let flags = zbus::fdo::RequestNameFlags::AllowReplacement.into();
    match connection.request_name_with_flags(NAME, flags) {
        Ok(
            zbus::fdo::RequestNameReply::PrimaryOwner | zbus::fdo::RequestNameReply::AlreadyOwner,
        ) => {
            tracing::info!("owning {NAME}: Solium is the notification server");
            let _ = facts.send(Fact::Serving(true));
        }
        Ok(_) | Err(zbus::Error::NameTaken) => {
            tracing::info!("{NAME} is owned by another program: waiting for it, in the queue");
            let _ = facts.send(Fact::Serving(false));
            let _ = work_out.send(Work::Owner);
        }
        Err(err) => return Err(err),
    }
    let unique = connection
        .unique_name()
        .map(ToString::to_string)
        .unwrap_or_default();
    for message in messages {
        let Ok(message) = message else {
            continue;
        };
        match message.message_type() {
            Type::MethodCall => answer(connection, &message, registry, &work_out),
            Type::Signal => heard(&message, &unique, facts, &work_out),
            _ => {}
        }
    }
    Ok(())
}

/// A signal from the bus: the name won, lost, or moved between others.
fn heard(message: &Message, unique: &str, facts: &mpsc::Sender<Fact>, work: &mpsc::Sender<Work>) {
    let header = message.header();
    if header.sender().map(|s| s.as_str()) != Some(DBUS) {
        return;
    }
    match header.member().map(|m| m.as_str()) {
        Some("NameAcquired") => {
            if message.body().deserialize::<String>().ok().as_deref() == Some(NAME) {
                tracing::info!("{NAME} acquired: Solium is the notification server now");
                let _ = facts.send(Fact::Serving(true));
                let _ = facts.send(Fact::Owner(None));
            }
        }
        Some("NameLost") => {
            if message.body().deserialize::<String>().ok().as_deref() == Some(NAME) {
                tracing::info!("{NAME} lost to another program");
                let _ = facts.send(Fact::Serving(false));
                let _ = work.send(Work::Owner);
            }
        }
        Some("NameOwnerChanged") => {
            if let Ok((name, _, owner)) = message.body().deserialize::<(String, String, String)>()
                && name == NAME
                && owner != unique
            {
                let _ = work.send(Work::Owner);
            }
        }
        _ => {}
    }
}

/// One method call, answered here.
fn answer(
    connection: &zbus::blocking::Connection,
    message: &Message,
    registry: &Arc<Mutex<Registry>>,
    work: &mpsc::Sender<Work>,
) {
    let header = message.header();
    let interface = header.interface().map(|i| i.as_str().to_owned());
    let member = header
        .member()
        .map(|m| m.as_str().to_owned())
        .unwrap_or_default();
    let path = header
        .path()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_default();
    let sender = header
        .sender()
        .map(|s| s.as_str().to_owned())
        .unwrap_or_default();
    let result = match (interface.as_deref(), member.as_str()) {
        (Some("org.freedesktop.DBus.Peer"), "Ping") => {
            connection.reply(&message.header(), &()).map(drop)
        }
        (Some("org.freedesktop.DBus.Introspectable"), "Introspect") => {
            let xml = if path == PATH {
                INTROSPECTION
            } else {
                EMPTY_NODE
            };
            connection.reply(&message.header(), &xml).map(drop)
        }
        (Some(NAME) | None, _) if path != PATH => unknown_object(connection, message),
        (Some(NAME) | None, "GetCapabilities") => connection
            .reply(&message.header(), &CAPABILITIES.to_vec())
            .map(drop),
        (Some(NAME) | None, "GetServerInformation") => connection
            .reply(
                &message.header(),
                &("Solium", "Lilium-Linux", env!("CARGO_PKG_VERSION"), "1.3"),
            )
            .map(drop),
        (Some(NAME) | None, "Notify") => notify(connection, message, &sender, registry, work),
        (Some(NAME) | None, "CloseNotification") => match message.body().deserialize::<u32>() {
            Ok(id) => {
                let allowed = lock(registry).may_close(id, &sender);
                let replied = connection.reply(&message.header(), &()).map(drop);
                if allowed {
                    let _ = work.send(Work::Close { id });
                } else {
                    tracing::debug!(
                        id,
                        sender,
                        "CloseNotification of another's notification: ignored"
                    );
                }
                replied
            }
            Err(err) => invalid_args(connection, message, &err.to_string()),
        },
        _ => connection
            .reply_error(
                &message.header(),
                "org.freedesktop.DBus.Error.UnknownMethod",
                &format!(
                    "no method {member} on {}",
                    interface.as_deref().unwrap_or("")
                ),
            )
            .map(drop),
    };
    if let Err(err) = result {
        tracing::debug!(?err, member, "a reply to a {NAME} call was not sent");
    }
}

type NotifyArgs = (
    String,
    u32,
    String,
    String,
    String,
    Vec<String>,
    HashMap<String, OwnedValue>,
    i32,
);

/// `Notify`: an id at once, answered before anything else happens to it.
/// An action list of odd length is refused whole.
/// `notify_is_answered_with_an_id_and_reaches_the_loop_after_its_reply`,
/// `an_odd_action_list_is_refused`.
fn notify(
    connection: &zbus::blocking::Connection,
    message: &Message,
    sender: &str,
    registry: &Arc<Mutex<Registry>>,
    work: &mpsc::Sender<Work>,
) -> zbus::Result<()> {
    let (app_name, replaces, app_icon, summary, body, actions, hints, expire_timeout) =
        match message.body().deserialize::<NotifyArgs>() {
            Ok(args) => args,
            Err(err) => return invalid_args(connection, message, &err.to_string()),
        };
    if actions.len() % 2 != 0 {
        return invalid_args(connection, message, "the actions list has an odd length");
    }
    let identity = hint_string(&hints, "desktop-entry")
        .filter(|entry| !entry.is_empty())
        .unwrap_or_else(|| app_name.clone());
    let (id, _) = lock(registry).allocate(&identity, sender, replaces);
    connection.reply(&message.header(), &id)?;
    let _ = work.send(Work::Notify {
        id,
        sender: sender.to_owned(),
        call: NotifyCall {
            app_name,
            app_icon,
            summary,
            body,
            actions,
            hints,
            expire_timeout,
        },
    });
    Ok(())
}

fn invalid_args(
    connection: &zbus::blocking::Connection,
    message: &Message,
    why: &str,
) -> zbus::Result<()> {
    connection
        .reply_error(
            &message.header(),
            "org.freedesktop.DBus.Error.InvalidArgs",
            &why,
        )
        .map(drop)
}

fn unknown_object(connection: &zbus::blocking::Connection, message: &Message) -> zbus::Result<()> {
    connection
        .reply_error(
            &message.header(),
            "org.freedesktop.DBus.Error.UnknownObject",
            &"nothing is served at this path",
        )
        .map(drop)
}

const EMPTY_NODE: &str = "<node/>";

const INTROSPECTION: &str = r#"<!DOCTYPE node PUBLIC "-//freedesktop//DTD D-BUS Object Introspection 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd">
<node>
  <interface name="org.freedesktop.Notifications">
    <method name="GetCapabilities"><arg type="as" direction="out"/></method>
    <method name="Notify">
      <arg type="s" direction="in"/><arg type="u" direction="in"/><arg type="s" direction="in"/>
      <arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="as" direction="in"/>
      <arg type="a{sv}" direction="in"/><arg type="i" direction="in"/><arg type="u" direction="out"/>
    </method>
    <method name="CloseNotification"><arg type="u" direction="in"/></method>
    <method name="GetServerInformation">
      <arg type="s" direction="out"/><arg type="s" direction="out"/>
      <arg type="s" direction="out"/><arg type="s" direction="out"/>
    </method>
    <signal name="NotificationClosed"><arg type="u"/><arg type="u"/></signal>
    <signal name="ActionInvoked"><arg type="u"/><arg type="s"/></signal>
    <signal name="ActivationToken"><arg type="u"/><arg type="s"/></signal>
  </interface>
  <interface name="org.freedesktop.DBus.Introspectable">
    <method name="Introspect"><arg type="s" direction="out"/></method>
  </interface>
  <interface name="org.freedesktop.DBus.Peer">
    <method name="Ping"/>
  </interface>
</node>
"#;

fn hint_string(hints: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    match &**hints.get(key)? {
        Value::Str(text) => Some(text.as_str().to_owned()),
        _ => None,
    }
}

fn hint_bool(hints: &HashMap<String, OwnedValue>, key: &str) -> bool {
    matches!(hints.get(key).map(|v| &**v), Some(Value::Bool(true)))
}

fn hint_byte(hints: &HashMap<String, OwnedValue>, key: &str) -> Option<u8> {
    match &**hints.get(key)? {
        Value::U8(byte) => Some(*byte),
        // Some senders send the urgency as a wider integer.
        Value::U32(n) => u8::try_from(*n).ok(),
        Value::I32(n) => u8::try_from(*n).ok(),
        _ => None,
    }
}

/// `image-data` (or its older spellings), `(iiibiiay)`.
fn hint_image_data(hints: &HashMap<String, OwnedValue>) -> Option<content::Image> {
    let value = ["image-data", "image_data", "icon_data"]
        .iter()
        .find_map(|key| hints.get(*key))?;
    let Value::Structure(structure) = &**value else {
        return None;
    };
    let fields = structure.fields();
    let [
        Value::I32(width),
        Value::I32(height),
        Value::I32(stride),
        Value::Bool(alpha),
        Value::I32(bits),
        Value::I32(channels),
        Value::Array(bytes),
    ] = fields
    else {
        return None;
    };
    let bytes: Vec<u8> = bytes
        .iter()
        .filter_map(|v| match v {
            Value::U8(b) => Some(*b),
            _ => None,
        })
        .collect();
    match content::image_data(*width, *height, *stride, *alpha, *bits, *channels, &bytes) {
        Ok(image) => Some(image),
        Err(why) => {
            tracing::debug!(?why, "a notification's image-data was refused");
            None
        }
    }
}

fn spawn_worker(
    connection: zbus::blocking::Connection,
    work: mpsc::Receiver<Work>,
    facts: mpsc::Sender<Fact>,
) -> zbus::Result<()> {
    std::thread::Builder::new()
        .name("solium-notifications-work".to_owned())
        .spawn(move || {
            let proxy = zbus::blocking::fdo::DBusProxy::new(&connection).ok();
            let mut pids: HashMap<String, Option<u32>> = HashMap::new();
            for each in work {
                let fact = match each {
                    Work::Notify { id, sender, call } => {
                        let pid = *pids.entry(sender.clone()).or_insert_with(|| {
                            proxy.as_ref().and_then(|proxy| pid_of(proxy, &sender))
                        });
                        Fact::Notify(Box::new(incoming(id, sender, pid, call)))
                    }
                    Work::Close { id } => Fact::Close { id },
                    Work::Owner => Fact::Owner(proxy.as_ref().and_then(owner)),
                };
                if facts.send(fact).is_err() {
                    return;
                }
            }
        })
        .map(drop)
        .map_err(|err| zbus::Error::Failure(format!("no worker thread: {err}")))
}

fn pid_of(proxy: &zbus::blocking::fdo::DBusProxy<'_>, name: &str) -> Option<u32> {
    let name = zbus::names::BusName::try_from(name).ok()?;
    proxy.get_connection_unix_process_id(name).ok()
}

fn owner(proxy: &zbus::blocking::fdo::DBusProxy<'_>) -> Option<Owner> {
    let name = zbus::names::BusName::try_from(NAME).ok()?;
    let unique = proxy.get_name_owner(name).ok()?.to_string();
    let pid = zbus::names::BusName::try_from(unique.as_str())
        .ok()
        .and_then(|bus| proxy.get_connection_unix_process_id(bus).ok());
    let program = pid
        .and_then(|pid| std::fs::read_to_string(format!("/proc/{pid}/comm")).ok())
        .map(|comm| comm.trim().to_owned())
        .unwrap_or_default();
    Some(Owner {
        name: unique,
        pid,
        program,
    })
}

/// The call, made safe: markup sanitised, images decoded and capped, an
/// `app_icon` that is a path read as an image and one that is a name kept
/// as a theme icon.
fn incoming(id: u32, sender: String, pid: Option<u32>, call: NotifyCall) -> Incoming {
    let hints = &call.hints;
    let mut image = hint_image_data(hints);
    if image.is_none()
        && let Some(path) =
            hint_string(hints, "image-path").or_else(|| hint_string(hints, "image_path"))
    {
        match content::image_path(&path) {
            Some(Ok(decoded)) => image = Some(decoded),
            Some(Err(why)) => tracing::debug!(?why, "a notification's image-path was refused"),
            None => {}
        }
    }
    let mut app_icon = call.app_icon;
    if let Some(read) = content::image_path(&app_icon) {
        if image.is_none() {
            image = read.ok();
        }
        app_icon.clear();
    }
    let actions = call
        .actions
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[key, label]| (key.clone(), label.clone()))
        .collect();
    let received = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        });
    Incoming {
        id,
        sender,
        pid,
        app_name: call.app_name,
        desktop_entry: hint_string(hints, "desktop-entry").unwrap_or_default(),
        app_icon,
        summary: content::body(&call.summary).plain,
        body: content::body(&call.body),
        image,
        urgency: hint_byte(hints, "urgency").map_or(Urgency::Normal, Urgency::from_byte),
        category: hint_string(hints, "category").unwrap_or_default(),
        actions,
        resident: hint_bool(hints, "resident"),
        transient: hint_bool(hints, "transient"),
        expire_timeout: call.expire_timeout,
        received,
    }
}

fn spawn_emitter(
    connection: zbus::blocking::Connection,
    signals: mpsc::Receiver<Signal>,
) -> zbus::Result<()> {
    std::thread::Builder::new()
        .name("solium-notifications-say".to_owned())
        .spawn(move || {
            for signal in signals {
                let sent = match signal {
                    Signal::Closed { id, reason } => connection.emit_signal(
                        None::<&str>,
                        PATH,
                        NAME,
                        "NotificationClosed",
                        &(id, reason),
                    ),
                    Signal::Invoked { id, key, token } => {
                        let token_sent = match token {
                            Some(token) => connection.emit_signal(
                                None::<&str>,
                                PATH,
                                NAME,
                                "ActivationToken",
                                &(id, token.as_str()),
                            ),
                            None => Ok(()),
                        };
                        token_sent.and_then(|()| {
                            connection.emit_signal(
                                None::<&str>,
                                PATH,
                                NAME,
                                "ActionInvoked",
                                &(id, key.as_str()),
                            )
                        })
                    }
                };
                if let Err(err) = sent {
                    tracing::debug!(?err, "a {NAME} signal was not sent");
                }
            }
        })
        .map(drop)
        .map_err(|err| zbus::Error::Failure(format!("no emitter thread: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "solium-notification-ids-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn ids_start_past_the_last_run_s_and_are_never_zero() {
        let dir = scratch("seed");
        let file = dir.join("notification-id");
        let mut first = Registry::new(Some(file.clone()));
        let (a, _) = first.allocate("mail", ":1.1", 0);
        let (b, _) = first.allocate("mail", ":1.1", 0);
        assert_ne!(a, 0);
        assert_eq!(b, a.wrapping_add(1));
        drop(first);
        let mut second = Registry::new(Some(file));
        let (c, _) = second.allocate("mail", ":1.1", 0);
        assert!(c > b, "{c} is past {b}");
        let _ = std::fs::remove_dir_all(dir);

        let mut wrapping = Registry {
            last: u32::MAX,
            reserved: u32::MAX,
            state_file: None,
            live: HashMap::new(),
        };
        assert_eq!(wrapping.allocate("mail", ":1.1", 0).0, 1);
    }

    #[test]
    fn only_the_same_app_replaces_a_notification() {
        let mut registry = Registry::new(None);
        let (id, replaced) = registry.allocate("org.gnome.Fractal", ":1.4", 0);
        assert!(!replaced);
        assert_eq!(
            registry.allocate("org.gnome.Fractal", ":1.4", id),
            (id, true)
        );
        let (other, replaced) = registry.allocate("evil", ":1.9", id);
        assert!(!replaced);
        assert_ne!(other, id);
        registry.forget(id);
        let (fresh, replaced) = registry.allocate("org.gnome.Fractal", ":1.4", id);
        assert!(!replaced, "a closed id is not replaced");
        assert_ne!(fresh, id);
    }

    #[test]
    fn only_its_sender_closes_a_notification() {
        let mut registry = Registry::new(None);
        let (id, _) = registry.allocate("mail", ":1.4", 0);
        assert!(registry.may_close(id, ":1.4"));
        assert!(!registry.may_close(id, ":1.9"));
        assert!(!registry.may_close(id.wrapping_add(1), ":1.4"));
    }

    mod bus {
        use std::{
            sync::atomic::{AtomicU32, Ordering},
            time::{Duration, Instant},
        };

        use zbus::message::Message;

        use super::super::*;

        const WITHIN: Duration = Duration::from_secs(5);

        /// The bus driver, as far as the server asks it anything.
        struct Driver {
            /// Another program already owns the name.
            taken: bool,
            requests: Arc<AtomicU32>,
        }

        #[zbus::interface(name = "org.freedesktop.DBus")]
        impl Driver {
            fn hello(&self) -> String {
                ":1.1".to_owned()
            }
            fn add_match(&self, _rule: String) {}
            fn remove_match(&self, _rule: String) {}
            fn request_name(&self, _name: String, _flags: u32) -> u32 {
                self.requests.fetch_add(1, Ordering::SeqCst);
                // DBUS_REQUEST_NAME_REPLY_IN_QUEUE, or _PRIMARY_OWNER.
                if self.taken { 2 } else { 1 }
            }
            fn release_name(&self, _name: String) -> u32 {
                1
            }
            fn get_name_owner(&self, _name: String) -> String {
                ":1.99".to_owned()
            }
            #[zbus(name = "GetConnectionUnixProcessID")]
            fn get_connection_unix_process_id(&self, name: String) -> u32 {
                if name == ":1.99" { 99 } else { 4242 }
            }
            fn name_has_owner(&self, _name: String) -> bool {
                true
            }
        }

        /// A stand-in session bus at a socket of the test's own: the driver,
        /// and the other clients' calls sent down Solium's one connection as
        /// the bus would deliver them. No bus, real or private, anywhere near.
        struct StandIn {
            address: String,
            directory: PathBuf,
            peers: mpsc::Receiver<zbus::blocking::Connection>,
            peer: Option<zbus::blocking::Connection>,
        }

        impl StandIn {
            fn new(name: &str, taken: bool) -> Self {
                let directory = std::env::temp_dir().join(format!(
                    "solium-notifications-bus-{}-{name}",
                    std::process::id()
                ));
                let _ = std::fs::remove_dir_all(&directory);
                std::fs::create_dir_all(&directory).expect("a directory");
                let path = directory.join("bus");
                let listener =
                    std::os::unix::net::UnixListener::bind(&path).expect("binding the socket");
                let (found, peers) = mpsc::channel();
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
                                    taken,
                                    requests: Arc::new(AtomicU32::new(0)),
                                },
                            )
                            .expect("serving the driver")
                            .build()
                    else {
                        return;
                    };
                    let _ = found.send(connection.clone());
                    connection.closed();
                });
                Self {
                    address: format!("unix:path={}", path.display()),
                    directory,
                    peers,
                    peer: None,
                }
            }

            fn peer(&mut self) -> zbus::blocking::Connection {
                if self.peer.is_none() {
                    self.peer = self.peers.recv_timeout(WITHIN).ok();
                }
                self.peer.clone().expect("Solium connected")
            }

            /// A call from `from` to the server, and its answer.
            fn call<B>(&mut self, from: &str, member: &str, body: &B) -> zbus::Result<Message>
            where
                B: zbus::export::serde::Serialize + zbus::zvariant::DynamicType,
            {
                let peer = self.peer();
                let call = Message::method_call(PATH, member)?
                    .sender(from)?
                    .destination(NAME)?
                    .interface(NAME)?
                    .build(body)?;
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
                    Type::Error => Err(zbus::Error::from(reply)),
                    _ => Ok(reply),
                }
            }

            fn notify(
                &mut self,
                from: &str,
                app: &str,
                replaces: u32,
                actions: &[&str],
            ) -> zbus::Result<u32> {
                let hints: HashMap<&str, zbus::zvariant::Value<'_>> =
                    HashMap::from([("urgency", zbus::zvariant::Value::U8(2))]);
                let actions: Vec<&str> = actions.to_vec();
                self.call(
                    from,
                    "Notify",
                    &(
                        app,
                        replaces,
                        "mail-unread",
                        "<b>New</b> mail",
                        "from &lt;Ann&gt;",
                        actions,
                        hints,
                        -1i32,
                    ),
                )?
                .body()
                .deserialize::<u32>()
            }
        }

        impl Drop for StandIn {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.directory);
            }
        }

        fn next_fact(server: &Server, wanted: impl Fn(&Fact) -> bool) -> Option<Fact> {
            let deadline = Instant::now() + WITHIN;
            while Instant::now() < deadline {
                match server.facts.recv_timeout(Duration::from_millis(50)) {
                    Ok(fact) if wanted(&fact) => return Some(fact),
                    Ok(_) | Err(_) => {}
                }
            }
            None
        }

        /// The server, once it has asked for the name and heard the answer:
        /// a real bus sends a connection nothing before its `Hello` is
        /// answered, so neither does a test. Whether it got the name.
        fn started(bus: &StandIn) -> (Server, bool) {
            let server = Server::start(
                Some(bus.address.clone()),
                Arc::new(Mutex::new(Registry::new(None))),
            )
            .expect("a server");
            let Some(Fact::Serving(serving)) =
                next_fact(&server, |fact| matches!(fact, Fact::Serving(_)))
            else {
                panic!("the server never heard whether it has the name");
            };
            (server, serving)
        }

        #[test]
        fn notify_is_answered_with_an_id_and_reaches_the_loop_after_its_reply() {
            let mut bus = StandIn::new("notify", false);
            let (server, serving) = started(&bus);
            assert!(serving);
            let id = bus
                .notify(":1.7", "Mail", 0, &["default", "", "reply", "Reply"])
                .expect("an id");
            assert_ne!(id, 0);
            let Some(Fact::Notify(incoming)) =
                next_fact(&server, |fact| matches!(fact, Fact::Notify(_)))
            else {
                panic!("the notification reached the loop");
            };
            assert_eq!(incoming.id, id);
            assert_eq!(incoming.sender, ":1.7");
            assert_eq!(incoming.pid, Some(4242));
            assert_eq!(incoming.summary, "New mail");
            assert_eq!(incoming.body.plain, "from <Ann>");
            assert_eq!(incoming.app_icon, "mail-unread");
            assert_eq!(incoming.urgency, Urgency::Critical);
            assert_eq!(
                incoming.actions,
                [
                    ("default".to_owned(), String::new()),
                    ("reply".to_owned(), "Reply".to_owned())
                ]
            );
            // The same app replaces it; another app gets an id of its own.
            assert_eq!(bus.notify(":1.7", "Mail", id, &[]).expect("an id"), id);
            assert_ne!(bus.notify(":1.8", "Other", id, &[]).expect("an id"), id);
        }

        #[test]
        fn an_odd_action_list_is_refused() {
            let mut bus = StandIn::new("odd", false);
            let _server = started(&bus);
            assert!(bus.notify(":1.7", "Mail", 0, &["default"]).is_err());
            // And the server is still answering.
            assert!(bus.notify(":1.7", "Mail", 0, &[]).is_ok());
        }

        #[test]
        fn only_the_sender_closes_and_its_close_follows_its_notify() {
            let mut bus = StandIn::new("close", false);
            let (server, _) = started(&bus);
            let id = bus.notify(":1.7", "Mail", 0, &[]).expect("an id");
            bus.call(":1.9", "CloseNotification", &id)
                .expect("answered");
            bus.call(":1.7", "CloseNotification", &id)
                .expect("answered");
            let mut seen = Vec::new();
            while let Some(fact) = next_fact(&server, |fact| {
                matches!(fact, Fact::Notify(_) | Fact::Close { .. })
            }) {
                seen.push(match fact {
                    Fact::Notify(n) => format!("notify {}", n.id),
                    Fact::Close { id } => format!("close {id}"),
                    _ => String::new(),
                });
                if seen.len() == 2 {
                    break;
                }
            }
            assert_eq!(seen, [format!("notify {id}"), format!("close {id}")]);
        }

        #[test]
        fn get_capabilities_and_server_information_answer() {
            let mut bus = StandIn::new("caps", false);
            let _server = started(&bus);
            let caps: Vec<String> = bus
                .call(":1.7", "GetCapabilities", &())
                .expect("answered")
                .body()
                .deserialize()
                .expect("a list");
            assert_eq!(caps, CAPABILITIES.map(str::to_owned));
            let (name, vendor, _, spec): (String, String, String, String) = bus
                .call(":1.7", "GetServerInformation", &())
                .expect("answered")
                .body()
                .deserialize()
                .expect("four strings");
            assert_eq!(
                (name.as_str(), vendor.as_str(), spec.as_str()),
                ("Solium", "Lilium-Linux", "1.3")
            );
        }

        #[test]
        fn an_invoke_says_its_token_before_its_action() {
            let mut bus = StandIn::new("invoke", false);
            let (server, _) = started(&bus);
            let id = bus
                .notify(":1.7", "Mail", 0, &["default", ""])
                .expect("an id");
            let peer = bus.peer();
            let heard = zbus::blocking::MessageIterator::from(&peer);
            server.say(Signal::Invoked {
                id,
                key: "default".to_owned(),
                token: Some("token-1".to_owned()),
            });
            server.say(Signal::Closed { id, reason: 2 });
            let mut signals = Vec::new();
            for message in heard.flatten() {
                if message.message_type() != Type::Signal {
                    continue;
                }
                let member = message
                    .header()
                    .member()
                    .map(|m| m.to_string())
                    .unwrap_or_default();
                signals.push(member);
                if signals.len() == 3 {
                    break;
                }
            }
            assert_eq!(
                signals,
                ["ActivationToken", "ActionInvoked", "NotificationClosed"]
            );
        }

        #[test]
        fn another_owner_keeps_the_name_and_is_named() {
            let bus = StandIn::new("taken", true);
            let (server, serving) = started(&bus);
            assert!(!serving);
            let Some(Fact::Owner(Some(owner))) =
                next_fact(&server, |fact| matches!(fact, Fact::Owner(_)))
            else {
                panic!("the owner was named");
            };
            assert_eq!(owner.name, ":1.99");
            assert_eq!(owner.pid, Some(99));
        }

        #[test]
        fn a_flood_of_calls_does_not_hold_a_notify_back() {
            let mut bus = StandIn::new("flood", false);
            let (_server, serving) = started(&bus);
            assert!(serving);
            let peer = bus.peer();
            for _ in 0..2000 {
                let junk = Message::method_call(PATH, "Nonsense")
                    .and_then(|b| b.sender(":1.66"))
                    .and_then(|b| b.destination(NAME))
                    .and_then(|b| b.interface(NAME))
                    .and_then(|b| b.build(&()))
                    .expect("a call");
                peer.send(&junk).expect("sent");
            }
            let started = Instant::now();
            bus.notify(":1.7", "Mail", 0, &[]).expect("an id");
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "answered in {:?}",
                started.elapsed()
            );
        }
    }

    #[test]
    fn get_capabilities_lists_only_what_solium_does() {
        assert!(!CAPABILITIES.contains(&"sound"));
        assert!(CAPABILITIES.contains(&"actions"));
        assert!(CAPABILITIES.contains(&"persistence"));
    }
}
