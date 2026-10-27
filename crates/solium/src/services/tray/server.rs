//! `org.kde.StatusNotifierWatcher` on the session bus, and the host that
//! reads the items it is told of, from threads of their own (#363).
//!
//! An application with a tray icon exports an `org.kde.StatusNotifierItem`
//! and registers it with the watcher, by a bus name it owns or by an object
//! path on its own connection; Solium is both the watcher and the one host
//! that shows what is registered. Three threads, as notifications' server
//! has, so that nothing the event loop does, and nothing a slow application
//! does, holds a registration back:
//!
//! * **the listener** reads every message on the connection. A registration,
//!   or a question about the watcher's properties, is answered there and
//!   then from [`Registry`], the one piece of state the threads share. An
//!   application that leaves the bus takes its items with it, here too. It
//!   never makes a call of its own, so it never waits on the bus.
//! * **the reader** reads items: every property after a registration, the
//!   ones a change signal names after it, and a menu when one is opened. It
//!   hands the event loop [`Fact`]s.
//! * **the caller** makes the calls a click asks for (`Activate`,
//!   `ContextMenu`, a menu entry's `Event`...), apart from the reader, so a
//!   click never waits behind a slow item's properties.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, mpsc},
};

use zbus::{
    message::{Message, Type},
    zvariant::{OwnedValue, Value, as_value},
};

use super::{
    content::{self, Pixmap},
    model::{Call, Entry, Picture, Props, Status, Toggle},
};
use crate::services::notifications::{
    content::Image,
    icon_name,
    server::{Owner, lock},
};

pub(crate) const WATCHER: &str = "org.kde.StatusNotifierWatcher";
pub(crate) const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const ITEM: &str = "org.kde.StatusNotifierItem";
/// Where an item registered by its bus name alone is.
const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU: &str = "com.canonical.dbusmenu";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const DBUS: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";

/// The most items registered at once; past this, a registration is
/// refused. A desktop has a dozen.
pub(crate) const MAX_ITEMS: usize = 64;

/// The most entries a menu keeps, counted over every submenu, and how deep
/// submenus go. What is past either is left out.
const MAX_ENTRIES: usize = 512;
const MAX_DEPTH: usize = 8;

/// The largest answer read for a picture (or a tooltip, which carries
/// pictures) and for a menu, in bytes; a larger one is left unread.
const MAX_PICTURE_REPLY: usize = 8 * 1024 * 1024;
const MAX_MENU_REPLY: usize = 1024 * 1024;
/// The largest answer read for any other property: text and flags.
const MAX_VALUE_REPLY: usize = 64 * 1024;

/// The most sizes of one picture looked at.
const MAX_PIXMAPS: usize = 16;

/// Work the reader has not got to yet, and facts the event loop has not
/// taken: past these, what an item said is dropped (a registration is
/// refused), so a flooding application cannot grow memory.
const WORK_QUEUE: usize = 256;
const FACT_QUEUE: usize = 512;

/// The properties of an item a menu entry asks for, and nothing else: the
/// pictures some menus carry are left on the application's side.
const ENTRY_PROPERTIES: [&str; 8] = [
    "type",
    "label",
    "enabled",
    "visible",
    "icon-name",
    "toggle-type",
    "toggle-state",
    "children-display",
];

/// Which properties a read covers, as bits: everything after a
/// registration, and only what a change signal names after it.
pub(crate) mod groups {
    pub(crate) const TITLE: u8 = 1;
    pub(crate) const ICON: u8 = 1 << 1;
    pub(crate) const ATTENTION: u8 = 1 << 2;
    pub(crate) const OVERLAY: u8 = 1 << 3;
    pub(crate) const TOOLTIP: u8 = 1 << 4;
    pub(crate) const STATUS: u8 = 1 << 5;
    pub(crate) const MENU: u8 = 1 << 6;
    /// `Id` and `Category`, which never change: read once.
    pub(crate) const IDENTITY: u8 = 1 << 7;
    pub(crate) const ALL: u8 = u8::MAX;

    /// What a signal of an item's says changed.
    /// `a_change_signal_names_only_what_it_changed`.
    pub(crate) fn of_signal(member: &str) -> u8 {
        match member {
            "NewTitle" => TITLE,
            "NewIcon" => ICON,
            "NewAttentionIcon" => ATTENTION,
            "NewOverlayIcon" => OVERLAY,
            "NewToolTip" => TOOLTIP,
            "NewStatus" => STATUS,
            "NewMenu" => MENU,
            "NewIconThemePath" => ICON | ATTENTION | OVERLAY,
            _ => 0,
        }
    }
}

/// What the event loop hears from the threads, in order.
#[derive(Debug)]
pub(crate) enum Fact {
    /// An item's properties, read: all of them, the ones read now merged
    /// over the ones read before. `pictures` holds the pixels of each
    /// picture read now.
    Read(Box<ReadItem>),
    /// An item left: its application left the bus, or it never answered.
    Gone { key: String },
    /// An item's menu, read.
    Menu { key: String, entries: Vec<Entry> },
    /// An item's menu could not be read.
    MenuFailed { key: String },
    /// An item said its menu changed.
    MenuChanged { key: String },
    /// An item refused `Activate`.
    ActivateRefused { key: String, x: i32, y: i32 },
    /// Whether Solium has the watcher's name now.
    Serving(bool),
    /// Who has it, when Solium does not.
    Owner(Option<Owner>),
    /// The name was lost: every item is gone with it.
    Cleared,
    /// The connection is gone.
    Down,
}

#[derive(Debug)]
pub(crate) struct ReadItem {
    pub(crate) key: String,
    pub(crate) props: Props,
    pub(crate) pictures: [Option<Image>; 3],
}

/// One registered item, as the listener knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Registered {
    /// The watcher's name for it: `service` and `path` together, as
    /// `RegisteredStatusNotifierItems` lists it.
    pub(crate) key: String,
    /// The bus name it registered with, unique or well-known: calls go here,
    /// and it is gone when this leaves the bus.
    pub(crate) service: String,
    pub(crate) path: String,
    /// The unique name behind `service`, which its signals come from. Empty
    /// until the reader has asked.
    pub(crate) owner: String,
    /// Its menu's object path, as last read.
    pub(crate) menu: String,
}

/// Why a registration was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// Neither a bus name nor an object path.
    Invalid,
    /// [`MAX_ITEMS`] are registered already.
    Full,
    /// Solium is not the watcher: another program has the name.
    NotServing,
}

/// The items registered, what of them is waiting to be read again, and
/// whether Solium is the watcher at all.
#[derive(Debug, Default)]
pub(crate) struct Registry {
    serving: bool,
    items: Vec<Registered>,
    /// Properties waiting to be read again, by key: an item that says it
    /// changed ten times before the reader gets to it is read once.
    pending: HashMap<String, u8>,
    /// Menus waiting to be read again.
    menus: HashSet<String>,
}

impl Registry {
    /// `RegisterStatusNotifierItem(service_or_path)` from `sender`: an object
    /// path is on the sender's own connection, and a name is at
    /// `/StatusNotifierItem`. The new item, or `None` for one already
    /// registered.
    /// `an_item_registers_by_its_name_or_by_a_path_on_its_own_connection`.
    pub(crate) fn register(
        &mut self,
        sender: &str,
        service_or_path: &str,
    ) -> Result<Option<Registered>, Refusal> {
        if !self.serving {
            return Err(Refusal::NotServing);
        }
        let (service, path) = if service_or_path.starts_with('/') {
            (sender, service_or_path)
        } else {
            (service_or_path, ITEM_PATH)
        };
        if service.is_empty()
            || zbus::names::BusName::try_from(service).is_err()
            || zbus::zvariant::ObjectPath::try_from(path).is_err()
        {
            return Err(Refusal::Invalid);
        }
        let key = format!("{service}{path}");
        if self.items.iter().any(|item| item.key == key) {
            return Ok(None);
        }
        if self.items.len() >= MAX_ITEMS {
            return Err(Refusal::Full);
        }
        let registered = Registered {
            key,
            service: service.to_owned(),
            path: path.to_owned(),
            owner: if service.starts_with(':') {
                service.to_owned()
            } else {
                String::new()
            },
            menu: String::new(),
        };
        self.items.push(registered.clone());
        Ok(Some(registered))
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Registered> {
        self.items.iter().find(|item| item.key == key)
    }

    pub(crate) fn keys(&self) -> Vec<String> {
        self.items.iter().map(|item| item.key.clone()).collect()
    }

    pub(crate) fn remove(&mut self, key: &str) {
        self.items.retain(|item| item.key != key);
        self.pending.remove(key);
        self.menus.remove(key);
    }

    /// `name` left the bus: every item registered with it, or behind it,
    /// taken out. Their keys.
    /// `an_item_is_gone_when_its_name_leaves_the_bus`.
    pub(crate) fn left(&mut self, name: &str) -> Vec<String> {
        let gone: Vec<String> = self
            .items
            .iter()
            .filter(|item| item.service == name || item.owner == name)
            .map(|item| item.key.clone())
            .collect();
        for key in &gone {
            self.remove(key);
        }
        gone
    }

    /// The items registered with the well-known name `name`, which moved to
    /// another connection: read again from there.
    pub(crate) fn moved(&self, name: &str) -> Vec<String> {
        self.items
            .iter()
            .filter(|item| item.service == name && !name.starts_with(':'))
            .map(|item| item.key.clone())
            .collect()
    }

    /// The item a signal from `sender` at `path` is about.
    /// `a_signal_is_matched_to_its_item_by_sender_and_path`.
    pub(crate) fn item_at(&self, sender: &str, path: &str) -> Option<String> {
        self.items
            .iter()
            .find(|item| !item.owner.is_empty() && item.owner == sender && item.path == path)
            .map(|item| item.key.clone())
    }

    /// The item whose menu, from `sender` at `path`, a signal is about.
    pub(crate) fn menu_at(&self, sender: &str, path: &str) -> Option<String> {
        self.items
            .iter()
            .find(|item| !item.owner.is_empty() && item.owner == sender && item.menu == path)
            .map(|item| item.key.clone())
    }

    fn set_owner(&mut self, key: &str, owner: &str) {
        if let Some(item) = self.items.iter_mut().find(|item| item.key == key) {
            owner.clone_into(&mut item.owner);
        }
    }

    fn set_menu(&mut self, key: &str, menu: &str) {
        if let Some(item) = self.items.iter_mut().find(|item| item.key == key) {
            menu.clone_into(&mut item.menu);
        }
    }

    /// `groups` of `key` to be read again: true when nothing of it was
    /// waiting already, so a read must be queued.
    fn want(&mut self, key: &str, groups: u8) -> bool {
        let waiting = self.pending.entry(key.to_owned()).or_insert(0);
        let queue = *waiting == 0;
        *waiting |= groups;
        queue
    }

    /// What of `key` is waiting to be read, taken.
    fn take(&mut self, key: &str) -> u8 {
        self.pending.remove(key).unwrap_or(0)
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

/// A call the caller makes, with where to.
#[derive(Debug)]
struct Say {
    key: String,
    to: String,
    path: String,
    menu: String,
    call: Call,
}

/// What the listener (and the event loop) hands the reader.
#[derive(Debug)]
enum Work {
    /// Newly registered: who owns it, then everything.
    Register { key: String },
    /// Read what [`Registry::take`] says is waiting.
    Read { key: String },
    Menu {
        key: String,
        about_to_show: Option<i32>,
    },
    /// Find out who owns the watcher's name now.
    Owner,
}

/// The watcher and host, running.
#[derive(Debug)]
pub(crate) struct Server {
    pub(crate) facts: mpsc::Receiver<Fact>,
    work: mpsc::SyncSender<Work>,
    says: mpsc::Sender<Say>,
    registry: Arc<Mutex<Registry>>,
    link: Arc<Mutex<Link>>,
}

impl Server {
    /// Connect to `address` (the session bus for `None`, never in a test) and
    /// ask for the name. `None` when no thread could be made.
    pub(crate) fn start(address: Option<String>) -> Option<Self> {
        let (facts_out, facts) = mpsc::sync_channel(FACT_QUEUE);
        let (work, work_in) = mpsc::sync_channel(WORK_QUEUE);
        let (says, says_in) = mpsc::channel();
        let registry = Arc::new(Mutex::new(Registry::default()));
        let link = Arc::new(Mutex::new(Link::Connecting));
        let spawned = std::thread::Builder::new()
            .name("solium-tray".to_owned())
            .spawn({
                let link = link.clone();
                let registry = registry.clone();
                let work = work.clone();
                move || {
                    run(
                        address.as_deref(),
                        &registry,
                        &link,
                        &facts_out,
                        (work, work_in),
                        says_in,
                    );
                }
            });
        match spawned {
            Ok(_) => Some(Self {
                facts,
                work,
                says,
                registry,
                link,
            }),
            Err(err) => {
                tracing::warn!(?err, "no thread for {WATCHER}: no tray");
                None
            }
        }
    }

    /// Make `call` on the item `key`: a menu read goes to the reader, the
    /// rest to the caller. An item no longer registered is not called.
    pub(crate) fn call(&self, key: &str, call: Call) {
        let mut registry = lock(&self.registry);
        let Some(item) = registry.get(key).cloned() else {
            return;
        };
        match call {
            Call::LoadMenu { about_to_show } => {
                // A menu that says it changed ten times is read once.
                if about_to_show.is_none() && !registry.menus.insert(key.to_owned()) {
                    return;
                }
                drop(registry);
                let _ = self.work.try_send(Work::Menu {
                    key: key.to_owned(),
                    about_to_show,
                });
            }
            call => {
                let _ = self.says.send(Say {
                    key: key.to_owned(),
                    to: item.service,
                    path: item.path,
                    menu: item.menu,
                    call,
                });
            }
        }
    }
}

impl Drop for Server {
    /// The connection is closed, which gives the name up and ends the
    /// threads, from a thread of its own, so letting go never waits on the
    /// bus.
    fn drop(&mut self) {
        let link = std::mem::replace(&mut *lock(&self.link), Link::Ended);
        if let Link::Up(connection) = link {
            let closing = std::thread::Builder::new()
                .name("solium-tray-close".to_owned())
                .spawn(move || connection.close());
            if let Err(err) = closing {
                tracing::warn!(?err, "no thread to close {WATCHER}'s connection");
            }
        }
    }
}

fn run(
    address: Option<&str>,
    registry: &Arc<Mutex<Registry>>,
    link: &Mutex<Link>,
    facts: &mpsc::SyncSender<Fact>,
    (work_out, work): (mpsc::SyncSender<Work>, mpsc::Receiver<Work>),
    says: mpsc::Receiver<Say>,
) {
    let connection = match crate::session::connect(address) {
        Ok(connection) => connection,
        Err(err) => {
            tracing::warn!(?err, bus = address, "no session bus: no tray");
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
    if let Err(err) = serve(&connection, registry, facts, (work_out, work), says) {
        tracing::warn!(?err, "{WATCHER}: the session bus refused this");
        let _ = connection.close();
    }
    let _ = facts.send(Fact::Down);
}

fn serve(
    connection: &zbus::blocking::Connection,
    registry: &Arc<Mutex<Registry>>,
    facts: &mpsc::SyncSender<Fact>,
    (work_out, work): (mpsc::SyncSender<Work>, mpsc::Receiver<Work>),
    says: mpsc::Receiver<Say>,
) -> zbus::Result<()> {
    // Read from before the name is asked for, so no call and no
    // `NameAcquired` is missed.
    let messages = zbus::blocking::MessageIterator::from(connection);
    spawn_reader(connection.clone(), registry.clone(), work, facts.clone())?;
    spawn_caller(connection.clone(), says, facts.clone())?;
    let dbus = zbus::blocking::fdo::DBusProxy::new(connection)?;
    // Every name that moves, to see an item's application leave; and what
    // items and their menus say changed.
    for rule in [
        zbus::MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(DBUS)?
            .path(DBUS_PATH)?
            .interface(DBUS)?
            .member("NameOwnerChanged")?
            .build(),
        zbus::MatchRule::builder()
            .msg_type(Type::Signal)
            .interface(ITEM)?
            .build(),
        zbus::MatchRule::builder()
            .msg_type(Type::Signal)
            .interface(MENU)?
            .build(),
    ] {
        dbus.add_match_rule(rule)?;
    }
    // Queued for politely, and given up to whoever asks to replace it: a
    // desktop around this one that has the name keeps it.
    // `another_watcher_keeps_the_name_and_is_named`.
    let flags = zbus::fdo::RequestNameFlags::AllowReplacement.into();
    match connection.request_name_with_flags(WATCHER, flags) {
        Ok(
            zbus::fdo::RequestNameReply::PrimaryOwner | zbus::fdo::RequestNameReply::AlreadyOwner,
        ) => acquired(connection, registry, facts),
        Ok(_) | Err(zbus::Error::NameTaken) => {
            tracing::info!("{WATCHER} is owned by another program: waiting for it, in the queue");
            let _ = facts.send(Fact::Serving(false));
            let _ = work_out.try_send(Work::Owner);
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
            Type::Signal => heard(connection, &message, &unique, registry, facts, &work_out),
            _ => {}
        }
    }
    Ok(())
}

/// Solium is the watcher now: applications waiting for a host are told
/// there is one.
fn acquired(
    connection: &zbus::blocking::Connection,
    registry: &Mutex<Registry>,
    facts: &mpsc::SyncSender<Fact>,
) {
    // Told twice when the name is had at once: by the answer to the
    // request, and by `NameAcquired`.
    if std::mem::replace(&mut lock(registry).serving, true) {
        return;
    }
    tracing::info!("owning {WATCHER}: Solium is the tray");
    let _ = facts.try_send(Fact::Serving(true));
    let _ = facts.try_send(Fact::Owner(None));
    let _ = connection.emit_signal(
        None::<&str>,
        WATCHER_PATH,
        WATCHER,
        "StatusNotifierHostRegistered",
        &(),
    );
}

/// `key` is no longer registered: every host is told.
fn unregistered(connection: &zbus::blocking::Connection, key: &str) {
    let _ = connection.emit_signal(
        None::<&str>,
        WATCHER_PATH,
        WATCHER,
        "StatusNotifierItemUnregistered",
        &key,
    );
}

/// A signal: the name won or lost, an application gone, or an item or its
/// menu saying it changed.
fn heard(
    connection: &zbus::blocking::Connection,
    message: &Message,
    unique: &str,
    registry: &Mutex<Registry>,
    facts: &mpsc::SyncSender<Fact>,
    work: &mpsc::SyncSender<Work>,
) {
    let header = message.header();
    let sender = header.sender().map(|s| s.as_str()).unwrap_or_default();
    let path = header.path().map(|p| p.as_str()).unwrap_or_default();
    let member = header.member().map(|m| m.as_str()).unwrap_or_default();
    match header.interface().map(|i| i.as_str()) {
        Some(DBUS) if sender == DBUS => match member {
            "NameAcquired" => {
                if message.body().deserialize::<String>().ok().as_deref() == Some(WATCHER) {
                    acquired(connection, registry, facts);
                }
            }
            "NameLost" => {
                if message.body().deserialize::<String>().ok().as_deref() == Some(WATCHER) {
                    tracing::info!("{WATCHER} lost to another program");
                    {
                        let mut registry = lock(registry);
                        registry.serving = false;
                        registry.items.clear();
                        registry.pending.clear();
                        registry.menus.clear();
                    }
                    let _ = facts.try_send(Fact::Serving(false));
                    let _ = facts.try_send(Fact::Cleared);
                    let _ = work.try_send(Work::Owner);
                }
            }
            "NameOwnerChanged" => {
                let Ok((name, _, owner)) = message.body().deserialize::<(String, String, String)>()
                else {
                    return;
                };
                if name == WATCHER && owner != unique {
                    let _ = work.try_send(Work::Owner);
                }
                if owner.is_empty() {
                    let gone = lock(registry).left(&name);
                    for key in gone {
                        unregistered(connection, &key);
                        let _ = facts.try_send(Fact::Gone { key });
                    }
                } else {
                    let moved = lock(registry).moved(&name);
                    for key in moved {
                        let _ = work.try_send(Work::Register { key });
                    }
                }
            }
            _ => {}
        },
        Some(ITEM) => {
            let groups = groups::of_signal(member);
            let mut registry = lock(registry);
            if groups != 0
                && let Some(key) = registry.item_at(sender, path)
                && registry.want(&key, groups)
            {
                drop(registry);
                let _ = work.try_send(Work::Read { key });
            }
        }
        Some(MENU) if matches!(member, "LayoutUpdated" | "ItemsPropertiesUpdated") => {
            if let Some(key) = lock(registry).menu_at(sender, path) {
                let _ = facts.try_send(Fact::MenuChanged { key });
            }
        }
        _ => {}
    }
}

/// One method call, answered here.
fn answer(
    connection: &zbus::blocking::Connection,
    message: &Message,
    registry: &Mutex<Registry>,
    work: &mpsc::SyncSender<Work>,
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
            let xml = if path == WATCHER_PATH {
                INTROSPECTION
            } else {
                "<node/>"
            };
            connection.reply(&message.header(), &xml).map(drop)
        }
        _ if path != WATCHER_PATH => error(
            connection,
            message,
            "org.freedesktop.DBus.Error.UnknownObject",
            "nothing is served at this path",
        ),
        (Some(WATCHER) | None, "RegisterStatusNotifierItem") => {
            match message.body().deserialize::<String>() {
                Ok(service_or_path) => register(
                    connection,
                    message,
                    &sender,
                    &service_or_path,
                    registry,
                    work,
                ),
                Err(err) => error(
                    connection,
                    message,
                    "org.freedesktop.DBus.Error.InvalidArgs",
                    &err.to_string(),
                ),
            }
        }
        (Some(WATCHER) | None, "RegisterStatusNotifierHost") => {
            // Another host is welcome to show the same items.
            let replied = connection.reply(&message.header(), &()).map(drop);
            let _ = connection.emit_signal(
                None::<&str>,
                WATCHER_PATH,
                WATCHER,
                "StatusNotifierHostRegistered",
                &(),
            );
            replied
        }
        (Some(PROPERTIES), "Get") => match message.body().deserialize::<(String, String)>() {
            Ok((interface, name)) if interface == WATCHER => {
                match property(&lock(registry), &name) {
                    Some(value) => connection.reply(&message.header(), &value).map(drop),
                    None => error(
                        connection,
                        message,
                        "org.freedesktop.DBus.Error.UnknownProperty",
                        &format!("no property {name}"),
                    ),
                }
            }
            _ => error(
                connection,
                message,
                "org.freedesktop.DBus.Error.UnknownInterface",
                "no such interface",
            ),
        },
        (Some(PROPERTIES), "GetAll") => {
            let all: HashMap<&str, Value<'static>> = match message.body().deserialize::<String>() {
                Ok(interface) if interface == WATCHER => {
                    let registry = lock(registry);
                    PROPERTY_NAMES
                        .iter()
                        .filter_map(|name| Some((*name, property(&registry, name)?)))
                        .collect()
                }
                _ => HashMap::new(),
            };
            connection.reply(&message.header(), &all).map(drop)
        }
        (Some(PROPERTIES), "Set") => error(
            connection,
            message,
            "org.freedesktop.DBus.Error.PropertyReadOnly",
            "the watcher's properties are read only",
        ),
        _ => error(
            connection,
            message,
            "org.freedesktop.DBus.Error.UnknownMethod",
            &format!(
                "no method {member} on {}",
                interface.as_deref().unwrap_or("")
            ),
        ),
    };
    if let Err(err) = result {
        tracing::debug!(?err, member, "a reply to a {WATCHER} call was not sent");
    }
}

/// `RegisterStatusNotifierItem`: answered at once, then announced, then
/// read. `an_item_registers_answers_and_is_read`.
fn register(
    connection: &zbus::blocking::Connection,
    message: &Message,
    sender: &str,
    service_or_path: &str,
    registry: &Mutex<Registry>,
    work: &mpsc::SyncSender<Work>,
) -> zbus::Result<()> {
    let registered = lock(registry).register(sender, service_or_path);
    match registered {
        Ok(Some(item)) => {
            if work
                .try_send(Work::Register {
                    key: item.key.clone(),
                })
                .is_err()
            {
                lock(registry).remove(&item.key);
                return error(
                    connection,
                    message,
                    "org.freedesktop.DBus.Error.LimitsExceeded",
                    "items are registering faster than they are read",
                );
            }
            connection.reply(&message.header(), &())?;
            connection.emit_signal(
                None::<&str>,
                WATCHER_PATH,
                WATCHER,
                "StatusNotifierItemRegistered",
                &item.key,
            )
        }
        Ok(None) => connection.reply(&message.header(), &()).map(drop),
        Err(Refusal::Invalid) => error(
            connection,
            message,
            "org.freedesktop.DBus.Error.InvalidArgs",
            "neither a bus name nor an object path",
        ),
        Err(Refusal::Full) => error(
            connection,
            message,
            "org.freedesktop.DBus.Error.LimitsExceeded",
            "the tray is full",
        ),
        Err(Refusal::NotServing) => error(
            connection,
            message,
            "org.freedesktop.DBus.Error.Failed",
            "Solium is not the watcher",
        ),
    }
}

const PROPERTY_NAMES: [&str; 3] = [
    "RegisteredStatusNotifierItems",
    "IsStatusNotifierHostRegistered",
    "ProtocolVersion",
];

/// One of the watcher's properties: the items registered, that there is a
/// host (Solium itself), and the protocol's version.
/// `the_watcher_s_properties_list_its_items_and_its_host`.
fn property(registry: &Registry, name: &str) -> Option<Value<'static>> {
    match name {
        "RegisteredStatusNotifierItems" => Some(Value::from(registry.keys())),
        "IsStatusNotifierHostRegistered" => Some(Value::from(registry.serving)),
        "ProtocolVersion" => Some(Value::from(0i32)),
        _ => None,
    }
}

fn error(
    connection: &zbus::blocking::Connection,
    message: &Message,
    name: &str,
    why: &str,
) -> zbus::Result<()> {
    connection
        .reply_error(&message.header(), name, &why)
        .map(drop)
}

const INTROSPECTION: &str = r#"<!DOCTYPE node PUBLIC "-//freedesktop//DTD D-BUS Object Introspection 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd">
<node>
  <interface name="org.kde.StatusNotifierWatcher">
    <method name="RegisterStatusNotifierItem"><arg name="service" type="s" direction="in"/></method>
    <method name="RegisterStatusNotifierHost"><arg name="service" type="s" direction="in"/></method>
    <property name="RegisteredStatusNotifierItems" type="as" access="read"/>
    <property name="IsStatusNotifierHostRegistered" type="b" access="read"/>
    <property name="ProtocolVersion" type="i" access="read"/>
    <signal name="StatusNotifierItemRegistered"><arg type="s"/></signal>
    <signal name="StatusNotifierItemUnregistered"><arg type="s"/></signal>
    <signal name="StatusNotifierHostRegistered"/>
    <signal name="StatusNotifierHostUnregistered"/>
  </interface>
  <interface name="org.freedesktop.DBus.Properties">
    <method name="Get"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="v" direction="out"/></method>
    <method name="GetAll"><arg type="s" direction="in"/><arg type="a{sv}" direction="out"/></method>
  </interface>
  <interface name="org.freedesktop.DBus.Introspectable">
    <method name="Introspect"><arg type="s" direction="out"/></method>
  </interface>
  <interface name="org.freedesktop.DBus.Peer">
    <method name="Ping"/>
  </interface>
</node>
"#;

/// Why an item could not be read.
#[derive(Debug, PartialEq, Eq)]
enum Unread {
    /// It is not there: its application, its object or its interface.
    Gone,
    /// It did not answer in time, or the bus failed: what was read before
    /// stands.
    Unanswered,
}

/// The error names that say an item is not there at all.
fn is_gone(err: &zbus::Error) -> bool {
    matches!(err, zbus::Error::MethodError(name, _, _) if matches!(
        name.as_str(),
        "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.NameHasNoOwner"
            | "org.freedesktop.DBus.Error.UnknownObject"
            | "org.freedesktop.DBus.Error.UnknownInterface"
            | "org.freedesktop.DBus.Error.UnknownMethod"
    ))
}

/// One item's properties, asked one at a time, each answer bounded before it
/// is decoded: a picture as bytes, never as a value per byte.
struct Asker<'a> {
    connection: &'a zbus::blocking::Connection,
    to: &'a str,
    path: &'a str,
    /// Whether any question was answered: an item none of whose properties
    /// answered is not there.
    answered: bool,
    gone: bool,
    /// A question went unanswered in time: the rest are not asked, so an
    /// application that hangs costs the reader one deadline, not one per
    /// property.
    stalled: bool,
}

impl Asker<'_> {
    fn get(&mut self, name: &str, max: usize) -> Option<Message> {
        if self.gone || self.stalled {
            return None;
        }
        match self.connection.call_method(
            Some(self.to),
            self.path,
            Some(PROPERTIES),
            "Get",
            &(ITEM, name),
        ) {
            Ok(reply) => {
                self.answered = true;
                (reply.data().len() <= max).then_some(reply)
            }
            Err(err) => {
                if is_gone(&err) {
                    self.gone = true;
                } else if matches!(err, zbus::Error::MethodError(..)) {
                    // There, without this property: an older item.
                    self.answered = true;
                } else {
                    self.stalled = true;
                }
                None
            }
        }
    }

    fn value(&mut self, name: &str) -> Option<OwnedValue> {
        self.get(name, MAX_VALUE_REPLY)?
            .body()
            .deserialize::<OwnedValue>()
            .ok()
    }

    fn text(&mut self, name: &str) -> String {
        match self.value(name).as_deref() {
            Some(Value::Str(text)) => text.as_str().to_owned(),
            Some(Value::ObjectPath(path)) => path.as_str().to_owned(),
            _ => String::new(),
        }
    }

    fn flag(&mut self, name: &str) -> bool {
        matches!(self.value(name).as_deref(), Some(Value::Bool(true)))
    }

    fn pixmaps(&mut self, name: &str) -> Vec<Pixmap> {
        self.get(name, MAX_PICTURE_REPLY)
            .and_then(|reply| {
                reply
                    .body()
                    .deserialize::<as_value::Deserialize<'_, Vec<(i32, i32, Vec<u8>)>>>()
                    .ok()
                    .map(|sizes| pixmaps(sizes.0))
            })
            .unwrap_or_default()
    }

    /// `ToolTip`, `(sa(iiay)ss)`: its title and its body.
    fn tooltip(&mut self) -> (String, String) {
        type ToolTip = (String, Vec<(i32, i32, Vec<u8>)>, String, String);
        self.get("ToolTip", MAX_PICTURE_REPLY)
            .and_then(|reply| {
                reply
                    .body()
                    .deserialize::<as_value::Deserialize<'_, ToolTip>>()
                    .ok()
                    .map(|tooltip| {
                        let (_, _, title, body) = tooltip.0;
                        (title, body)
                    })
            })
            .unwrap_or_default()
    }
}

fn pixmaps(sizes: Vec<(i32, i32, Vec<u8>)>) -> Vec<Pixmap> {
    sizes
        .into_iter()
        .take(MAX_PIXMAPS)
        .map(|(width, height, argb)| Pixmap {
            width,
            height,
            argb,
        })
        .collect()
}

/// What the reader keeps of an item between reads: what it said last, and
/// where its own icon files are.
#[derive(Clone, Debug, Default)]
struct Kept {
    props: Props,
    theme_path: String,
}

/// `groups` of an item read, over what was `kept`: the pixels of each
/// picture read now, or why nothing could be.
fn read_item(
    connection: &zbus::blocking::Connection,
    to: &str,
    path: &str,
    groups: u8,
    kept: &mut Kept,
) -> Result<[Option<Image>; 3], Unread> {
    let mut ask = Asker {
        connection,
        to,
        path,
        answered: false,
        gone: false,
        stalled: false,
    };
    let mut props = kept.props.clone();
    if groups & groups::IDENTITY != 0 {
        props.item_id = content::cut(ask.text("Id"), content::MAX_TEXT);
        props.category = content::cut(ask.text("Category"), content::MAX_TEXT);
    }
    if groups & groups::TITLE != 0 {
        props.title = content::cut(ask.text("Title"), content::MAX_TEXT);
    }
    if groups & groups::STATUS != 0 {
        props.status = Status::from_name(&ask.text("Status"));
    }
    if groups & groups::TOOLTIP != 0 {
        let (title, body) = ask.tooltip();
        props.tooltip_title = content::cut(title, content::MAX_TEXT);
        props.tooltip_body = content::plain(&body, content::MAX_TOOLTIP);
    }
    if groups & groups::MENU != 0 {
        let menu = ask.text("Menu");
        // Some items name no menu with the root path, or with this.
        props.menu = if menu == "/" || menu == "/NO_DBUSMENU" {
            String::new()
        } else {
            menu
        };
        props.item_is_menu = ask.flag("ItemIsMenu");
    }
    let mut pictures: [Option<Image>; 3] = [None, None, None];
    let pictured = groups & (groups::ICON | groups::ATTENTION | groups::OVERLAY);
    if pictured != 0 {
        kept.theme_path = ask.text("IconThemePath");
    }
    for (picture, group, name, pixmap) in [
        (Picture::Icon, groups::ICON, "IconName", "IconPixmap"),
        (
            Picture::Attention,
            groups::ATTENTION,
            "AttentionIconName",
            "AttentionIconPixmap",
        ),
        (
            Picture::Overlay,
            groups::OVERLAY,
            "OverlayIconName",
            "OverlayIconPixmap",
        ),
    ] {
        if groups & group == 0 {
            continue;
        }
        let at = picture as usize;
        let named = icon_name(&ask.text(name)).to_owned();
        let image = content::pixmap(&ask.pixmaps(pixmap)).or_else(|| {
            (!kept.theme_path.is_empty() && !named.is_empty())
                .then(|| content::theme_file(&kept.theme_path, &named))
                .flatten()
        });
        props.pictures[at] = image.as_ref().map_or(0, content::digest);
        props.names[at] = named;
        pictures[at] = image;
    }
    if ask.gone || ask.stalled || !ask.answered {
        return Err(if ask.gone || groups == groups::ALL {
            Unread::Gone
        } else {
            Unread::Unanswered
        });
    }
    kept.props = props;
    Ok(pictures)
}

/// `GetLayout`'s answer, `(u(ia{sv}av))`, as entries: the root's children.
type Layout = (u32, (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>));

/// A menu's entries from its layout, within [`MAX_ENTRIES`] and
/// [`MAX_DEPTH`].
/// `a_menu_layout_becomes_entries_with_their_submenus_and_limits`.
fn entries(layout: &Layout) -> Vec<Entry> {
    let mut budget = MAX_ENTRIES;
    let children: Vec<&Value<'_>> = layout.1.2.iter().map(|child| &**child).collect();
    entries_of(&children, 1, &mut budget)
}

fn entries_of(children: &[&Value<'_>], depth: usize, budget: &mut usize) -> Vec<Entry> {
    if depth > MAX_DEPTH {
        return Vec::new();
    }
    let mut entries = Vec::new();
    for child in children {
        if *budget == 0 {
            break;
        }
        let Value::Structure(structure) = unwrap(child) else {
            continue;
        };
        let [
            Value::I32(id),
            Value::Dict(properties),
            Value::Array(grandchildren),
        ] = structure.fields()
        else {
            continue;
        };
        *budget -= 1;
        let mut entry = Entry {
            id: *id,
            ..Entry::default()
        };
        let mut submenu = false;
        for (key, value) in properties.iter() {
            let Value::Str(key) = unwrap(key) else {
                continue;
            };
            match (key.as_str(), unwrap(value)) {
                ("label", Value::Str(text)) => entry.label = content::label(text),
                ("enabled", Value::Bool(on)) => entry.enabled = *on,
                ("visible", Value::Bool(on)) => entry.visible = *on,
                ("type", Value::Str(kind)) => entry.separator = kind.as_str() == "separator",
                ("icon-name", Value::Str(name)) => {
                    icon_name(name).clone_into(&mut entry.icon_name);
                }
                ("toggle-type", Value::Str(kind)) => entry.toggle = Toggle::from_name(kind),
                ("toggle-state", Value::I32(state)) => entry.toggle_state = (*state).clamp(-1, 1),
                ("children-display", Value::Str(how)) => submenu = how.as_str() == "submenu",
                _ => {}
            }
        }
        let grandchildren: Vec<&Value<'_>> = grandchildren.inner().iter().collect();
        entry.children = entries_of(&grandchildren, depth + 1, budget);
        entry.submenu = submenu || !entry.children.is_empty();
        entries.push(entry);
    }
    entries
}

/// A value out of the variants it may be wrapped in.
fn unwrap<'v, 'a>(value: &'v Value<'a>) -> &'v Value<'a> {
    let mut value = value;
    while let Value::Value(inner) = value {
        value = inner;
    }
    value
}

fn spawn_reader(
    connection: zbus::blocking::Connection,
    registry: Arc<Mutex<Registry>>,
    work: mpsc::Receiver<Work>,
    facts: mpsc::SyncSender<Fact>,
) -> zbus::Result<()> {
    std::thread::Builder::new()
        .name("solium-tray-read".to_owned())
        .spawn(move || {
            let dbus = zbus::blocking::fdo::DBusProxy::new(&connection).ok();
            let mut kept: HashMap<String, Kept> = HashMap::new();
            for each in work {
                let fact = match each {
                    Work::Register { key } => {
                        lock(&registry).take(&key);
                        kept.remove(&key);
                        read(
                            &connection,
                            dbus.as_ref(),
                            &registry,
                            &key,
                            groups::ALL,
                            &mut kept,
                        )
                    }
                    Work::Read { key } => {
                        let groups = lock(&registry).take(&key);
                        if groups == 0 {
                            continue;
                        }
                        read(
                            &connection,
                            dbus.as_ref(),
                            &registry,
                            &key,
                            groups,
                            &mut kept,
                        )
                    }
                    Work::Menu { key, about_to_show } => {
                        let item = {
                            let mut registry = lock(&registry);
                            if about_to_show.is_none() {
                                registry.menus.remove(&key);
                            }
                            registry.get(&key).cloned()
                        };
                        let Some(item) = item else { continue };
                        Some(menu(&connection, &item, about_to_show))
                    }
                    Work::Owner => Some(Fact::Owner(dbus.as_ref().and_then(owner))),
                };
                // The reader keeps no more items than are registered.
                if kept.len() > MAX_ITEMS {
                    let registry = lock(&registry);
                    kept.retain(|key, _| registry.get(key).is_some());
                }
                if let Some(fact) = fact
                    && facts.send(fact).is_err()
                {
                    return;
                }
            }
        })
        .map(drop)
        .map_err(|err| zbus::Error::Failure(format!("no reader thread: {err}")))
}

/// `groups` of `key` read. An item that is not there is unregistered.
fn read(
    connection: &zbus::blocking::Connection,
    dbus: Option<&zbus::blocking::fdo::DBusProxy<'_>>,
    registry: &Mutex<Registry>,
    key: &str,
    groups: u8,
    kept: &mut HashMap<String, Kept>,
) -> Option<Fact> {
    let item = lock(registry).get(key).cloned()?;
    // A well-known name's owner, which its signals come from.
    if groups == groups::ALL && !item.service.starts_with(':') {
        let owner = dbus
            .and_then(|dbus| {
                let name = zbus::names::BusName::try_from(item.service.as_str()).ok()?;
                dbus.get_name_owner(name).ok()
            })
            .map(|owner| owner.to_string());
        match owner {
            Some(owner) => lock(registry).set_owner(key, &owner),
            None => return Some(gone(connection, registry, key)),
        }
    }
    let mut item_kept = kept.remove(key).unwrap_or_default();
    match read_item(
        connection,
        &item.service,
        &item.path,
        groups,
        &mut item_kept,
    ) {
        Ok(pictures) => {
            lock(registry).set_menu(key, &item_kept.props.menu);
            let props = item_kept.props.clone();
            kept.insert(key.to_owned(), item_kept);
            Some(Fact::Read(Box::new(ReadItem {
                key: key.to_owned(),
                props,
                pictures,
            })))
        }
        Err(Unread::Gone) => Some(gone(connection, registry, key)),
        Err(Unread::Unanswered) => {
            tracing::debug!(
                key,
                "a tray item did not answer; what it said before stands"
            );
            kept.insert(key.to_owned(), item_kept);
            None
        }
    }
}

fn gone(connection: &zbus::blocking::Connection, registry: &Mutex<Registry>, key: &str) -> Fact {
    tracing::debug!(key, "a tray item is not there: unregistered");
    lock(registry).remove(key);
    unregistered(connection, key);
    Fact::Gone {
        key: key.to_owned(),
    }
}

/// `AboutToShow(about_to_show)` when asked, then the whole layout.
fn menu(
    connection: &zbus::blocking::Connection,
    item: &Registered,
    about_to_show: Option<i32>,
) -> Fact {
    let key = item.key.clone();
    if item.menu.is_empty() {
        return Fact::MenuFailed { key };
    }
    if let Some(id) = about_to_show {
        // Whether it needs reading again is not trusted: it is read anyway.
        let _ = connection.call_method(
            Some(item.service.as_str()),
            item.menu.as_str(),
            Some(MENU),
            "AboutToShow",
            &id,
        );
    }
    let reply = connection.call_method(
        Some(item.service.as_str()),
        item.menu.as_str(),
        Some(MENU),
        "GetLayout",
        &(0i32, -1i32, ENTRY_PROPERTIES.to_vec()),
    );
    let layout = reply
        .ok()
        .filter(|reply| reply.data().len() <= MAX_MENU_REPLY);
    match layout.and_then(|reply| reply.body().deserialize::<Layout>().ok()) {
        Some(layout) => Fact::Menu {
            key,
            entries: entries(&layout),
        },
        None => Fact::MenuFailed { key },
    }
}

fn owner(proxy: &zbus::blocking::fdo::DBusProxy<'_>) -> Option<Owner> {
    let name = zbus::names::BusName::try_from(WATCHER).ok()?;
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

fn spawn_caller(
    connection: zbus::blocking::Connection,
    says: mpsc::Receiver<Say>,
    facts: mpsc::SyncSender<Fact>,
) -> zbus::Result<()> {
    std::thread::Builder::new()
        .name("solium-tray-call".to_owned())
        .spawn(move || {
            for say in says {
                let to = Some(say.to.as_str());
                let path = say.path.as_str();
                let item = Some(ITEM);
                let made = match say.call {
                    Call::Activate { x, y, token } => {
                        // So the window it raises may take the keyboard. Not
                        // every item knows the method; one that does not is
                        // activated all the same.
                        if let Some(token) = token {
                            let _ = connection.call_method(
                                to,
                                path,
                                item,
                                "ProvideXdgActivationToken",
                                &token,
                            );
                        }
                        let activated = connection.call_method(to, path, item, "Activate", &(x, y));
                        // An answer that is an error is a refusal; no answer
                        // at all is only slowness.
                        // `a_refused_activate_is_told_to_the_loop`.
                        if let Err(zbus::Error::MethodError(..)) = activated {
                            let _ = facts.try_send(Fact::ActivateRefused {
                                key: say.key.clone(),
                                x,
                                y,
                            });
                        }
                        activated.map(drop)
                    }
                    Call::SecondaryActivate { x, y } => connection
                        .call_method(to, path, item, "SecondaryActivate", &(x, y))
                        .map(drop),
                    Call::ContextMenu { x, y } => connection
                        .call_method(to, path, item, "ContextMenu", &(x, y))
                        .map(drop),
                    Call::Scroll { delta, horizontal } => connection
                        .call_method(
                            to,
                            path,
                            item,
                            "Scroll",
                            &(delta, if horizontal { "horizontal" } else { "vertical" }),
                        )
                        .map(drop),
                    Call::MenuEvent { entry, event } => {
                        if say.menu.is_empty() {
                            continue;
                        }
                        let timestamp = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |since| since.as_secs() as u32);
                        connection
                            .call_method(
                                to,
                                say.menu.as_str(),
                                Some(MENU),
                                "Event",
                                &(entry, event, Value::from(0i32), timestamp),
                            )
                            .map(drop)
                    }
                    Call::LoadMenu { .. } => continue,
                };
                if let Err(err) = made {
                    tracing::debug!(?err, key = say.key, "a call to a tray item failed");
                }
            }
        })
        .map(drop)
        .map_err(|err| zbus::Error::Failure(format!("no caller thread: {err}")))
}

#[cfg(test)]
mod tests;
