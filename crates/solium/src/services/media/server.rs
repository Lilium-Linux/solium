//! The MPRIS players on the session bus, followed from threads of their own
//! (#379).
//!
//! A media player owns a name under `org.mpris.MediaPlayer2.` and exports
//! `org.mpris.MediaPlayer2` and `org.mpris.MediaPlayer2.Player` at
//! `/org/mpris/MediaPlayer2`. Solium owns no name here: it only watches, and
//! calls a player when an action asks. Three threads, as the tray has, so
//! nothing a slow player does holds the event loop or another player back:
//!
//! * **the listener** reads every message on the connection: a player's name
//!   coming or going, `PropertiesChanged` and `Seeked`. What a signal says
//!   is merged into [`Registry`], the one piece of state the threads share,
//!   so a player that changes ten times before it is read is read once, with
//!   the newest of each value. It never makes a call of its own.
//! * **the reader** finds the players there already, reads each one whole
//!   when it appears, and after that what its signals named (with its
//!   position again when its status, rate or track changed: `Position` is
//!   never announced). It reads album art the player names on disk, and
//!   hands the event loop [`Fact`]s.
//! * **the caller** makes the calls an action asks for (`PlayPause`, `Seek`,
//!   setting `Volume`...), apart from the reader, so a key press never waits
//!   behind a slow player's properties.
//!
//! Every call goes to a player's unique name, never to its well-known one: a
//! call to a name nobody owns could start a player through D-Bus activation.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, mpsc},
    time::Instant,
};

use zbus::{
    message::{Message, Type},
    zvariant::{OwnedValue, Value},
};

use super::{
    content,
    model::{Art, Call, Loop, Props, Status},
};
use crate::services::notifications::{content::Image, server::lock};

/// Every player's name starts with this.
pub(crate) const PREFIX: &str = "org.mpris.MediaPlayer2.";
const ROOT: &str = "org.mpris.MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const PATH: &str = "/org/mpris/MediaPlayer2";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const DBUS: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";
/// The track id a player gives when there is no track.
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";

/// The most players followed at once; past this, a new one is not. A
/// desktop has two or three.
pub(crate) const MAX_PLAYERS: usize = 16;

/// The largest message read from or about a player, in bytes: its
/// properties, or a signal of its changes. Large enough for art sent inline
/// as a `data:` URI ([`content::MAX_DATA_URI`]); a larger one is left
/// unread.
const MAX_MESSAGE: usize = 8 * 1024 * 1024;

/// Work the reader has not got to yet, facts the event loop has not taken,
/// and calls the caller has not made: past these, work is dropped (a
/// player's change is read with its next), so a flooding player cannot grow
/// memory.
const WORK_QUEUE: usize = 256;
const FACT_QUEUE: usize = 512;
const CALL_QUEUE: usize = 64;

/// The properties read and followed, by interface; the rest are ignored, so
/// what is waiting to be read stays bounded.
const ROOT_PROPERTIES: [&str; 2] = ["Identity", "DesktopEntry"];
const PLAYER_PROPERTIES: [&str; 13] = [
    "PlaybackStatus",
    "LoopStatus",
    "Rate",
    "Shuffle",
    "Metadata",
    "Volume",
    "Position",
    "CanGoNext",
    "CanGoPrevious",
    "CanPlay",
    "CanPause",
    "CanSeek",
    "CanControl",
];

/// What the event loop hears from the threads, in order.
#[derive(Debug)]
pub(crate) enum Fact {
    /// A player's properties, read: all of them, the ones read now merged
    /// over the ones read before. A `Seeked` comes as one too, with its
    /// position.
    Read(Box<ReadPlayer>),
    /// A player left the bus.
    Gone { key: String },
    /// The connection is gone.
    Down,
}

#[derive(Debug)]
pub(crate) struct ReadPlayer {
    pub(crate) key: String,
    pub(crate) props: Props,
    /// Its position, when this read asked for it, and when.
    pub(crate) position: Option<(i64, Instant)>,
    /// Its art's pixels, when they were read now.
    pub(crate) art: Option<Image>,
}

/// One player, as the listener knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Registered {
    /// Its well-known name, `org.mpris.MediaPlayer2.mpv`.
    pub(crate) key: String,
    /// The unique name that owns it: its signals come from here, and calls go
    /// here.
    pub(crate) owner: String,
}

/// What a player's signals said that the reader has not read yet.
#[derive(Debug, Default)]
struct Pending {
    /// Changed values by interface and name, the newest of each; `None` for
    /// one the player said changed without saying to what.
    values: HashMap<(&'static str, &'static str), Option<OwnedValue>>,
    seeked: Option<(i64, Instant)>,
}

/// The players followed, and what of them is waiting to be read again.
#[derive(Debug, Default)]
pub(crate) struct Registry {
    players: Vec<Registered>,
    pending: HashMap<String, Pending>,
}

impl Registry {
    /// `key` is owned by `owner` now: a new player, or one whose name moved
    /// to another connection (then it is a new player: the old one is gone
    /// first). Whether it is followed: not past [`MAX_PLAYERS`], and only a
    /// player's name.
    /// `a_player_name_is_followed_up_to_the_cap_and_moves_with_its_owner`.
    pub(crate) fn add(&mut self, key: &str, owner: &str) -> Added {
        if !key.starts_with(PREFIX) || key.len() == PREFIX.len() || owner.is_empty() {
            return Added::Refused;
        }
        if let Some(player) = self.players.iter().find(|player| player.key == key) {
            if player.owner == owner {
                return Added::Already;
            }
            self.remove(key);
            self.players.push(Registered {
                key: key.to_owned(),
                owner: owner.to_owned(),
            });
            return Added::Moved;
        }
        if self.players.len() >= MAX_PLAYERS {
            return Added::Refused;
        }
        self.players.push(Registered {
            key: key.to_owned(),
            owner: owner.to_owned(),
        });
        Added::New
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Registered> {
        self.players.iter().find(|player| player.key == key)
    }

    /// Whether `key` is followed, and still owned by `owner`.
    fn holds(&self, key: &str, owner: &str) -> bool {
        self.get(key).is_some_and(|player| player.owner == owner)
    }

    /// Whether `key` was followed.
    pub(crate) fn remove(&mut self, key: &str) -> bool {
        let before = self.players.len();
        self.players.retain(|player| player.key != key);
        self.pending.remove(key);
        self.players.len() != before
    }

    /// The players whose signals come from `sender`: one connection may own
    /// more than one name.
    pub(crate) fn keys_of(&self, sender: &str) -> Vec<String> {
        self.players
            .iter()
            .filter(|player| player.owner == sender)
            .map(|player| player.key.clone())
            .collect()
    }

    /// What a `PropertiesChanged` from `key` said: kept for the reader, the
    /// newest of each value. True when nothing of `key` was waiting already,
    /// so a read must be queued.
    /// `changes_wait_merged_and_a_player_is_queued_once`.
    fn changed(
        &mut self,
        key: &str,
        interface: &str,
        changed: HashMap<String, OwnedValue>,
        invalidated: Vec<String>,
    ) -> bool {
        let Some(interface) = known_interface(interface) else {
            return false;
        };
        let queue = !self.pending.contains_key(key);
        let pending = self.pending.entry(key.to_owned()).or_default();
        let before = pending.values.len() + usize::from(pending.seeked.is_some());
        for (name, value) in changed {
            if let Some(name) = known_property(interface, &name) {
                pending.values.insert((interface, name), Some(value));
            }
        }
        for name in invalidated {
            if let Some(name) = known_property(interface, &name) {
                pending.values.insert((interface, name), None);
            }
        }
        let grew = pending.values.len() + usize::from(pending.seeked.is_some()) > before;
        if queue && !grew {
            // Nothing this reader follows: nothing to read.
            self.pending.remove(key);
            return false;
        }
        queue
    }

    /// `Seeked` from `key`: the newest kept. True when a read must be queued.
    fn seeked(&mut self, key: &str, micros: i64, at: Instant) -> bool {
        let queue = !self.pending.contains_key(key);
        self.pending.entry(key.to_owned()).or_default().seeked = Some((micros, at));
        queue
    }

    fn take(&mut self, key: &str) -> Pending {
        self.pending.remove(key).unwrap_or_default()
    }
}

/// What [`Registry::add`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Added {
    New,
    /// The name has another owner now: a new player in the old one's place.
    Moved,
    Already,
    Refused,
}

fn known_interface(interface: &str) -> Option<&'static str> {
    match interface {
        ROOT => Some(ROOT),
        PLAYER => Some(PLAYER),
        _ => None,
    }
}

fn known_property(interface: &'static str, name: &str) -> Option<&'static str> {
    let names: &[&'static str] = if interface == ROOT {
        &ROOT_PROPERTIES
    } else {
        &PLAYER_PROPERTIES
    };
    names.iter().copied().find(|known| *known == name)
}

/// The thread's connection, as far as the event loop knows it.
#[derive(Debug)]
enum Link {
    Connecting,
    Up(zbus::blocking::Connection),
    /// Let go: a connection made after this is closed straight away.
    Ended,
}

/// What the listener (and the event loop) hands the reader.
#[derive(Debug)]
enum Work {
    /// Find the players already on the bus.
    Scan,
    /// Read what is waiting for `key`, or all of it for one not read yet.
    Read { key: String },
}

/// A call the caller makes, with where to.
#[derive(Debug)]
struct Say {
    key: String,
    owner: String,
    call: Call,
}

/// The players, followed.
#[derive(Debug)]
pub(crate) struct Server {
    pub(crate) facts: mpsc::Receiver<Fact>,
    says: mpsc::SyncSender<Say>,
    registry: Arc<Mutex<Registry>>,
    link: Arc<Mutex<Link>>,
}

impl Server {
    /// Connect to `address` (the session bus for `None`, never in a test) and
    /// start following. `None` when no thread could be made.
    pub(crate) fn start(address: Option<String>) -> Option<Self> {
        let (facts_out, facts) = mpsc::sync_channel(FACT_QUEUE);
        let (says, says_in) = mpsc::sync_channel(CALL_QUEUE);
        let registry = Arc::new(Mutex::new(Registry::default()));
        let link = Arc::new(Mutex::new(Link::Connecting));
        let spawned = std::thread::Builder::new()
            .name("solium-media".to_owned())
            .spawn({
                let link = link.clone();
                let registry = registry.clone();
                move || run(address.as_deref(), &registry, &link, &facts_out, says_in)
            });
        match spawned {
            Ok(_) => Some(Self {
                facts,
                says,
                registry,
                link,
            }),
            Err(err) => {
                tracing::warn!(?err, "no thread for MPRIS: no media");
                None
            }
        }
    }

    /// Make `call` on the player `key`. A player no longer followed is not
    /// called, and a call past [`CALL_QUEUE`] is dropped.
    pub(crate) fn call(&self, key: &str, call: Call) {
        let Some(owner) = lock(&self.registry).get(key).map(|p| p.owner.clone()) else {
            return;
        };
        let _ = self.says.try_send(Say {
            key: key.to_owned(),
            owner,
            call,
        });
    }
}

impl Drop for Server {
    /// The connection is closed, which ends the threads, from a thread of its
    /// own, so letting go never waits on the bus.
    fn drop(&mut self) {
        let link = std::mem::replace(&mut *lock(&self.link), Link::Ended);
        if let Link::Up(connection) = link {
            let closing = std::thread::Builder::new()
                .name("solium-media-close".to_owned())
                .spawn(move || connection.close());
            if let Err(err) = closing {
                tracing::warn!(?err, "no thread to close the media connection");
            }
        }
    }
}

fn run(
    address: Option<&str>,
    registry: &Arc<Mutex<Registry>>,
    link: &Mutex<Link>,
    facts: &mpsc::SyncSender<Fact>,
    says: mpsc::Receiver<Say>,
) {
    let connection = match crate::session::connect(address) {
        Ok(connection) => connection,
        Err(err) => {
            tracing::warn!(?err, bus = address, "no session bus: no media");
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
    if let Err(err) = follow(&connection, registry, facts, says) {
        tracing::warn!(?err, "MPRIS: the session bus refused this");
        let _ = connection.close();
    }
    let _ = facts.send(Fact::Down);
}

fn follow(
    connection: &zbus::blocking::Connection,
    registry: &Arc<Mutex<Registry>>,
    facts: &mpsc::SyncSender<Fact>,
    says: mpsc::Receiver<Say>,
) -> zbus::Result<()> {
    // Read from before the rules are added, so no change is missed between
    // the scan and the first signal.
    let messages = zbus::blocking::MessageIterator::from(connection);
    let (work_out, work) = mpsc::sync_channel(WORK_QUEUE);
    spawn_reader(connection.clone(), registry.clone(), work, facts.clone())?;
    spawn_caller(connection.clone(), says)?;
    let dbus = zbus::blocking::fdo::DBusProxy::new(connection)?;
    for rule in [
        // A player's name coming, going or moving.
        zbus::MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(DBUS)?
            .path(DBUS_PATH)?
            .interface(DBUS)?
            .member("NameOwnerChanged")?
            .arg0ns(ROOT)?
            .build(),
        // What a player says changed, and where it seeked to.
        zbus::MatchRule::builder()
            .msg_type(Type::Signal)
            .path(PATH)?
            .interface(PROPERTIES)?
            .member("PropertiesChanged")?
            .build(),
        zbus::MatchRule::builder()
            .msg_type(Type::Signal)
            .path(PATH)?
            .interface(PLAYER)?
            .member("Seeked")?
            .build(),
    ] {
        dbus.add_match_rule(rule)?;
    }
    let _ = work_out.try_send(Work::Scan);
    for message in messages {
        let Ok(message) = message else {
            continue;
        };
        if message.message_type() == Type::Signal {
            heard(&message, registry, facts, &work_out);
        }
    }
    Ok(())
}

/// A signal: a player's name coming or going, or a player saying it changed.
fn heard(
    message: &Message,
    registry: &Mutex<Registry>,
    facts: &mpsc::SyncSender<Fact>,
    work: &mpsc::SyncSender<Work>,
) {
    let header = message.header();
    let sender = header.sender().map(|s| s.as_str()).unwrap_or_default();
    let path = header.path().map(|p| p.as_str()).unwrap_or_default();
    let member = header.member().map(|m| m.as_str()).unwrap_or_default();
    if message.data().len() > MAX_MESSAGE {
        return;
    }
    match (header.interface().map(|i| i.as_str()), member) {
        (Some(DBUS), "NameOwnerChanged") if sender == DBUS => {
            let Ok((name, _, owner)) = message.body().deserialize::<(String, String, String)>()
            else {
                return;
            };
            if !name.starts_with(PREFIX) {
                return;
            }
            appeared_or_left(&name, &owner, registry, facts, work);
        }
        (Some(PROPERTIES), "PropertiesChanged") if path == PATH => {
            type Changed = (String, HashMap<String, OwnedValue>, Vec<String>);
            let Ok((interface, changed, invalidated)) = message.body().deserialize::<Changed>()
            else {
                return;
            };
            let mut registry = lock(registry);
            let keys = registry.keys_of(sender);
            // One connection with two names says it once for both.
            let mut changed = Some(changed);
            for (at, key) in keys.iter().enumerate() {
                let values = if at + 1 == keys.len() {
                    changed.take().unwrap_or_default()
                } else {
                    changed
                        .as_ref()
                        .map(|values| {
                            values
                                .iter()
                                .filter_map(|(name, value)| {
                                    Some((name.clone(), value.try_clone().ok()?))
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                };
                if registry.changed(key, &interface, values, invalidated.clone()) {
                    let _ = work.try_send(Work::Read { key: key.clone() });
                }
            }
        }
        (Some(PLAYER), "Seeked") if path == PATH => {
            let Ok(micros) = message.body().deserialize::<i64>() else {
                return;
            };
            let at = Instant::now();
            let mut registry = lock(registry);
            for key in registry.keys_of(sender) {
                if registry.seeked(&key, micros, at) {
                    let _ = work.try_send(Work::Read { key });
                }
            }
        }
        _ => {}
    }
}

/// `name` is owned by `owner` now, or by nobody for an empty one.
fn appeared_or_left(
    name: &str,
    owner: &str,
    registry: &Mutex<Registry>,
    facts: &mpsc::SyncSender<Fact>,
    work: &mpsc::SyncSender<Work>,
) {
    let mut registry = lock(registry);
    if owner.is_empty() {
        // Told while the registry is held, so a read of it the reader is
        // finishing reaches the loop before this, never after.
        if registry.remove(name) {
            let _ = facts.send(Fact::Gone {
                key: name.to_owned(),
            });
        }
        return;
    }
    match registry.add(name, owner) {
        Added::New => {
            let _ = work.try_send(Work::Read {
                key: name.to_owned(),
            });
        }
        Added::Moved => {
            let _ = facts.send(Fact::Gone {
                key: name.to_owned(),
            });
            let _ = work.try_send(Work::Read {
                key: name.to_owned(),
            });
        }
        Added::Already | Added::Refused => {}
    }
}

/// The error names that say a player is not there at all.
fn is_gone(err: &zbus::Error) -> bool {
    matches!(err, zbus::Error::MethodError(name, _, _) if matches!(
        name.as_str(),
        "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.NameHasNoOwner"
            | "org.freedesktop.DBus.Error.UnknownObject"
    ))
}

/// What the reader keeps of a player between reads.
#[derive(Debug, Default)]
struct Kept {
    owner: String,
    props: Props,
    /// The art URL as it was last sent.
    art_url: String,
}

/// What reading one value changed beyond the value itself.
#[derive(Debug, Default)]
struct Effects {
    /// Its position must be read again: its status, rate or track changed.
    position: bool,
    /// The position it sent itself.
    sent_position: Option<i64>,
    /// Its art must be read again.
    art: bool,
}

/// One property, merged into `kept`.
/// `a_player_s_properties_are_read_into_props_and_its_metadata_made_safe`.
fn merge(kept: &mut Kept, interface: &str, name: &str, value: &Value<'_>, effects: &mut Effects) {
    let props = &mut kept.props;
    let value = unwrap(value);
    let flag = |value: &Value<'_>| matches!(value, Value::Bool(true));
    match (interface, name) {
        (ROOT, "Identity") => props.identity = content::text(as_str(value)),
        (ROOT, "DesktopEntry") => {
            props.desktop_entry = content::text(as_str(value).trim_end_matches(".desktop"));
        }
        (PLAYER, "PlaybackStatus") => {
            props.status = Status::from_name(as_str(value));
            effects.position = true;
        }
        (PLAYER, "LoopStatus") => props.loop_status = Loop::from_name(as_str(value)),
        (PLAYER, "Rate") => {
            props.rate = number(value).filter(|rate| rate.is_finite()).unwrap_or(1.0);
            effects.position = true;
        }
        (PLAYER, "Shuffle") => props.shuffle = Some(flag(value)),
        (PLAYER, "Volume") => {
            props.volume = number(value)
                .filter(|volume| volume.is_finite())
                .map(|volume| volume.clamp(0.0, 1.0));
        }
        (PLAYER, "Position") => effects.sent_position = integer(value),
        (PLAYER, "CanGoNext") => props.can_go_next = flag(value),
        (PLAYER, "CanGoPrevious") => props.can_go_previous = flag(value),
        (PLAYER, "CanPlay") => props.can_play = flag(value),
        (PLAYER, "CanPause") => props.can_pause = flag(value),
        (PLAYER, "CanSeek") => props.can_seek = flag(value),
        (PLAYER, "CanControl") => props.can_control = flag(value),
        (PLAYER, "Metadata") => {
            metadata(kept, value);
            effects.position = true;
            effects.art = true;
        }
        _ => {}
    }
}

/// `Metadata`, `a{sv}`: the track's title, artists, album, art, length and
/// id. A key it leaves out is cleared: metadata is sent whole.
fn metadata(kept: &mut Kept, value: &Value<'_>) {
    let props = &mut kept.props;
    props.title.clear();
    props.artist.clear();
    props.album.clear();
    props.track_id.clear();
    props.length = 0;
    kept.art_url.clear();
    let Value::Dict(dict) = value else {
        return;
    };
    for (key, value) in dict.iter() {
        let Value::Str(key) = unwrap(key) else {
            continue;
        };
        let value = unwrap(value);
        match key.as_str() {
            "xesam:title" => props.title = content::text(as_str(value)),
            "xesam:album" => props.album = content::text(as_str(value)),
            "xesam:artist" => {
                props.artist = match value {
                    Value::Array(names) => {
                        content::artists(names.iter().map(|name| as_str(unwrap(name))))
                    }
                    // Some players send one name, not a list.
                    other => content::text(as_str(other)),
                };
            }
            "mpris:artUrl" => as_str(value).clone_into(&mut kept.art_url),
            "mpris:length" => props.length = integer(value).unwrap_or(0).max(0),
            "mpris:trackid" => {
                let id = as_str(value);
                props.track_id = if id == NO_TRACK
                    || zbus::zvariant::ObjectPath::try_from(id).is_err()
                    || id.len() > content::MAX_TEXT
                {
                    String::new()
                } else {
                    id.to_owned()
                };
            }
            _ => {}
        }
    }
}

/// A value out of the variants it may be wrapped in.
fn unwrap<'v, 'a>(value: &'v Value<'a>) -> &'v Value<'a> {
    let mut value = value;
    while let Value::Value(inner) = value {
        value = inner;
    }
    value
}

fn as_str<'v>(value: &'v Value<'_>) -> &'v str {
    match value {
        Value::Str(text) => text.as_str(),
        Value::ObjectPath(path) => path.as_str(),
        _ => "",
    }
}

fn number(value: &Value<'_>) -> Option<f64> {
    match value {
        Value::F64(n) => Some(*n),
        Value::I32(n) => Some(f64::from(*n)),
        Value::U32(n) => Some(f64::from(*n)),
        _ => None,
    }
}

/// An integer as players send one: `x` as the specification says, and the
/// `t`, `i` and `u` some send instead.
fn integer(value: &Value<'_>) -> Option<i64> {
    match value {
        Value::I64(n) => Some(*n),
        Value::U64(n) => i64::try_from(*n).ok(),
        Value::I32(n) => Some(i64::from(*n)),
        Value::U32(n) => Some(i64::from(*n)),
        Value::F64(n) if n.is_finite() => Some(*n as i64),
        _ => None,
    }
}

/// `GetAll(interface)` from a player, bounded before it is decoded.
fn get_all(
    connection: &zbus::blocking::Connection,
    owner: &str,
    interface: &str,
) -> zbus::Result<HashMap<String, OwnedValue>> {
    let reply =
        connection.call_method(Some(owner), PATH, Some(PROPERTIES), "GetAll", &interface)?;
    if reply.data().len() > MAX_MESSAGE {
        return Err(zbus::Error::Failure("too large".to_owned()));
    }
    reply.body().deserialize()
}

/// `Get(interface, name)` from a player, bounded before it is decoded.
fn get(
    connection: &zbus::blocking::Connection,
    owner: &str,
    interface: &str,
    name: &str,
) -> zbus::Result<OwnedValue> {
    let reply = connection.call_method(
        Some(owner),
        PATH,
        Some(PROPERTIES),
        "Get",
        &(interface, name),
    )?;
    if reply.data().len() > MAX_MESSAGE {
        return Err(zbus::Error::Failure("too large".to_owned()));
    }
    reply.body().deserialize()
}

/// The players already on the bus when the reader starts, followed.
fn scan(
    dbus: Option<&zbus::blocking::fdo::DBusProxy<'_>>,
    registry: &Mutex<Registry>,
) -> Vec<String> {
    let Some(names) = dbus.and_then(|dbus| dbus.list_names().ok()) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for name in names {
        if !name.as_str().starts_with(PREFIX) {
            continue;
        }
        let Some(owner) = dbus
            .and_then(|dbus| dbus.get_name_owner(name.inner().clone()).ok())
            .map(|owner| owner.to_string())
        else {
            continue;
        };
        if lock(registry).add(name.as_str(), &owner) == Added::New {
            found.push(name.as_str().to_owned());
        }
    }
    found
}

fn spawn_reader(
    connection: zbus::blocking::Connection,
    registry: Arc<Mutex<Registry>>,
    work: mpsc::Receiver<Work>,
    facts: mpsc::SyncSender<Fact>,
) -> zbus::Result<()> {
    std::thread::Builder::new()
        .name("solium-media-read".to_owned())
        .spawn(move || {
            let dbus = zbus::blocking::fdo::DBusProxy::new(&connection).ok();
            let mut kept: HashMap<String, Kept> = HashMap::new();
            for each in work {
                let keys = match each {
                    Work::Scan => scan(dbus.as_ref(), &registry),
                    Work::Read { key } => vec![key],
                };
                for key in keys {
                    if !read(&connection, &registry, &key, &mut kept, &facts) {
                        return;
                    }
                }
                // The reader keeps no more players than are followed.
                if kept.len() > MAX_PLAYERS {
                    let registry = lock(&registry);
                    kept.retain(|key, _| registry.get(key).is_some());
                }
            }
        })
        .map(drop)
        .map_err(|err| zbus::Error::Failure(format!("no reader thread: {err}")))
}

/// What is waiting for `key` read, or the whole player when it has not been
/// read from its owner yet, and told to the loop. False once the loop is
/// gone.
fn read(
    connection: &zbus::blocking::Connection,
    registry: &Mutex<Registry>,
    key: &str,
    kept: &mut HashMap<String, Kept>,
    facts: &mpsc::SyncSender<Fact>,
) -> bool {
    let (owner, pending) = {
        let mut registry = lock(registry);
        let Some(owner) = registry.get(key).map(|player| player.owner.clone()) else {
            kept.remove(key);
            return true;
        };
        (owner, registry.take(key))
    };
    let whole = kept.get(key).is_none_or(|player| player.owner != owner);
    let mut player = if whole {
        Kept {
            owner: owner.clone(),
            ..Kept::default()
        }
    } else {
        kept.remove(key).unwrap_or_default()
    };
    let art_before = player.art_url.clone();
    let mut effects = Effects::default();
    if whole {
        for interface in [ROOT, PLAYER] {
            match get_all(connection, &owner, interface) {
                Ok(values) => {
                    for (name, value) in &values {
                        merge(&mut player, interface, name, value, &mut effects);
                    }
                }
                Err(err) => {
                    if is_gone(&err) {
                        let mut registry = lock(registry);
                        if registry.holds(key, &owner) {
                            registry.remove(key);
                            return facts
                                .send(Fact::Gone {
                                    key: key.to_owned(),
                                })
                                .is_ok();
                        }
                        return true;
                    }
                    // Not answered in time: read whole again with its next
                    // change.
                    tracing::debug!(?err, key, "a media player did not answer");
                    return true;
                }
            }
        }
        effects.art = true;
    } else {
        for ((interface, name), value) in pending.values {
            let value = match value {
                Some(value) => Some(value),
                None => get(connection, &owner, interface, name).ok(),
            };
            if let Some(value) = value {
                merge(&mut player, interface, name, &value, &mut effects);
            }
        }
    }
    let mut position = effects.sent_position.map(|micros| (micros, Instant::now()));
    if let Some(seeked) = pending.seeked {
        position = Some(seeked);
    } else if effects.position && position.is_none() {
        // `Position` is never announced: read again whenever how it moves
        // changed.
        position = get(connection, &owner, PLAYER, "Position")
            .ok()
            .and_then(|value| integer(unwrap(&value)))
            .map(|micros| (micros, Instant::now()));
    }
    let mut art = None;
    if effects.art || player.art_url != art_before {
        player.props.art = match content::source(&player.art_url) {
            content::Source::Local(url) => match content::read(&url) {
                Ok(image) => {
                    let digest = crate::services::tray::content::digest(&image);
                    art = Some(image);
                    Art::Read(digest)
                }
                Err(err) => {
                    tracing::debug!(?err, key, "a media player's art could not be read");
                    Art::None
                }
            },
            content::Source::Remote(url) => Art::Remote(url),
            content::Source::None => Art::None,
        };
    }
    let fact = Fact::Read(Box::new(ReadPlayer {
        key: key.to_owned(),
        props: player.props.clone(),
        position,
        art,
    }));
    kept.insert(key.to_owned(), player);
    // Told while the registry is held, and only of a player still followed
    // under the same owner, so a `Gone` the listener tells is never followed
    // by a read from before it.
    let registry = lock(registry);
    if !registry.holds(key, &owner) {
        return true;
    }
    facts.send(fact).is_ok()
}

fn spawn_caller(
    connection: zbus::blocking::Connection,
    says: mpsc::Receiver<Say>,
) -> zbus::Result<()> {
    std::thread::Builder::new()
        .name("solium-media-call".to_owned())
        .spawn(move || {
            for say in says {
                let to = Some(say.owner.as_str());
                let player = Some(PLAYER);
                let made = match say.call {
                    Call::PlayPause => connection.call_method(to, PATH, player, "PlayPause", &()),
                    Call::Play => connection.call_method(to, PATH, player, "Play", &()),
                    Call::Pause => connection.call_method(to, PATH, player, "Pause", &()),
                    Call::Next => connection.call_method(to, PATH, player, "Next", &()),
                    Call::Previous => connection.call_method(to, PATH, player, "Previous", &()),
                    Call::Seek(offset) => connection.call_method(to, PATH, player, "Seek", &offset),
                    Call::SetPosition { track_id, micros } => {
                        match zbus::zvariant::ObjectPath::try_from(track_id.as_str()) {
                            Ok(track) => connection.call_method(
                                to,
                                PATH,
                                player,
                                "SetPosition",
                                &(track, micros),
                            ),
                            Err(_) => continue,
                        }
                    }
                    Call::Volume(volume) => connection.call_method(
                        to,
                        PATH,
                        Some(PROPERTIES),
                        "Set",
                        &(PLAYER, "Volume", Value::F64(volume)),
                    ),
                };
                if let Err(err) = made {
                    tracing::debug!(?err, key = say.key, "a call to a media player failed");
                }
            }
        })
        .map(drop)
        .map_err(|err| zbus::Error::Failure(format!("no caller thread: {err}")))
}

#[cfg(test)]
mod tests;
