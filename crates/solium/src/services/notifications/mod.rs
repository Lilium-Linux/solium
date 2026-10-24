//! Notifications (03 §3.2.11): Solium as the session's notification server,
//! and what it received, for scenes (`Notifications`) and for Lua
//! (`sol.notifications`, `sol.on("notification")`, `sol.on("notifications")`).
//!
//! * [`content`] makes what a sender sent safe to show;
//! * [`model`] is the state machine: ids' lifetimes, banners, the history;
//! * [`server`] is the bus, on threads of its own;
//! * this module holds them together for the event loop: it starts the
//!   server when the configuration asks and the session allows, takes what
//!   the server heard, asks Lua what to do with each arrival, and tells the
//!   server what to say back. `crate::services::settle` drives it.

pub(crate) mod content;
pub(crate) mod model;
pub(crate) mod server;

use std::{
    collections::BTreeMap,
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::{json::Json, session::Place};

use model::{Decision, Effect, Incoming, Item, Notifications, Shown, Urgency};
use server::{Owner, Registry, Server, Signal};

/// What the configuration says: `sol.notifications{ ... }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Config {
    /// Own `org.freedesktop.Notifications`.
    pub(crate) serve: bool,
    pub(crate) settings: model::Settings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            serve: false,
            settings: model::Settings::default(),
        }
    }
}

/// Which bus the name may be owned on, as `session::speaks` decides: a nested
/// run never takes the desktop's notifications unless it is given a bus of its
/// own. `a_nested_run_serves_nothing_unless_it_is_given_a_bus`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Bus {
    #[default]
    Nowhere,
    On(Option<String>),
}

/// Where a notification came from, for `origin` in Lua's events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    /// Over the bus.
    External,
    /// A scene's own direct call (`n.dismiss()`).
    Scene,
    /// Lua (`sol.act`, `sol.notify`).
    Script,
    /// The service itself: a timeout, a full history.
    Service,
}

impl Origin {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::External => "external",
            Self::Scene => "scene",
            Self::Script => "script",
            Self::Service => "service",
        }
    }
}

/// What the sender's process is joined to, filled in on arrival.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Joined {
    /// The window of the sender's process (or its parent's), by script id.
    pub(crate) window: u64,
    /// The monitor that window is on.
    pub(crate) monitor: String,
    /// The desktop entry's name and icon, when the `desktop-entry` hint named
    /// an installed app.
    pub(crate) app_name: String,
    pub(crate) app_icon: String,
}

/// What snapshots and scenes read: the model's state, as rows.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct View {
    pub(crate) available: bool,
    pub(crate) serving: bool,
    pub(crate) dnd: bool,
    pub(crate) owner: Option<Owner>,
    /// Newest first.
    pub(crate) banners: Vec<BTreeMap<&'static str, Json>>,
    /// Newest first.
    pub(crate) history: Vec<BTreeMap<&'static str, Json>>,
}

/// One event for `sol.on("notifications")`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Event {
    pub(crate) what: &'static str,
    pub(crate) id: u32,
    pub(crate) origin: Origin,
    /// `closed`: the reason; `invoked`: the key.
    pub(crate) detail: String,
}

impl Event {
    pub(crate) fn to_json(&self, view: &View) -> Json {
        let mut fields = BTreeMap::from([
            ("what".to_owned(), Json::Text(self.what.to_owned())),
            ("origin".to_owned(), Json::Text(self.origin.name().to_owned())),
        ]);
        if self.id != 0 {
            fields.insert("id".to_owned(), Json::Number(f64::from(self.id)));
        }
        match self.what {
            "closed" => {
                fields.insert("reason".to_owned(), Json::Text(self.detail.clone()));
            }
            "invoked" => {
                fields.insert("key".to_owned(), Json::Text(self.detail.clone()));
            }
            "dnd" => {
                fields.insert("value".to_owned(), Json::Bool(view.dnd));
            }
            "owner" => {
                fields.insert("serving".to_owned(), Json::Bool(view.serving));
            }
            _ => {}
        }
        if let Some(row) = view
            .banners
            .iter()
            .chain(view.history.iter())
            .find(|row| row.get("id").and_then(Json::as_u64) == Some(u64::from(self.id)))
        {
            fields.insert("notification".to_owned(), object(row));
        }
        Json::Object(fields)
    }
}

/// A row's values as a JSON object, for Lua.
pub(crate) fn object(row: &BTreeMap<&'static str, Json>) -> Json {
    Json::Object(
        row.iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect(),
    )
}

/// The service, owned by `Solium`.
#[derive(Debug)]
pub(crate) struct Service {
    bus: Bus,
    config: Config,
    server: Option<Server>,
    registry: Arc<Mutex<Registry>>,
    pub(crate) model: Notifications,
    serving: bool,
    owner: Option<Owner>,
    joined: BTreeMap<u32, Joined>,
    /// Images handed to Qt, by id, with the revision they belong to.
    images: BTreeMap<u32, u32>,
    view: Rc<View>,
    dirty: bool,
    /// For `sol.on("notifications")`, in order, drained each settle.
    pub(crate) events: Vec<Event>,
    /// Lua functions behind the actions of `sol.notify`'s own notifications,
    /// by id: the Lua-side token their table is kept under.
    pub(crate) own: BTreeMap<u32, u64>,
}

impl Default for Service {
    fn default() -> Self {
        Self::new(None)
    }
}

impl Service {
    /// With ids written down in `state_file` across runs.
    pub(crate) fn new(state_file: Option<std::path::PathBuf>) -> Self {
        Self {
            bus: Bus::Nowhere,
            config: Config::default(),
            server: None,
            registry: Arc::new(Mutex::new(Registry::new(state_file))),
            model: Notifications::default(),
            serving: false,
            owner: None,
            joined: BTreeMap::new(),
            images: BTreeMap::new(),
            view: Rc::new(View::default()),
            dirty: true,
            events: Vec::new(),
            own: BTreeMap::new(),
        }
    }

    /// Where the ids are written down: `$XDG_STATE_HOME/solium/notification-id`.
    pub(crate) fn state_file() -> Option<std::path::PathBuf> {
        // A test never writes into the developer's state directory.
        if cfg!(test) {
            return None;
        }
        let base = std::env::var_os("XDG_STATE_HOME")
            .filter(|dir| !dir.is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local/state")))?;
        Some(base.join("solium").join("notification-id"))
    }

    /// The backend has started: which bus the name may be owned on.
    pub(crate) fn permit(&mut self, place: Place, bus: Option<String>) {
        self.bus = if crate::session::speaks(place, bus.as_deref()) {
            Bus::On(bus)
        } else {
            if self.config.serve {
                tracing::info!(
                    ?place,
                    "not started as the session: {} is left to the desktop around this one. \
                     SOLIUM_SESSION_BUS names a bus to own it on instead",
                    server::NAME
                );
            }
            Bus::Nowhere
        };
        self.start_or_stop();
    }

    /// `sol.notifications{ ... }`, at load and on every reload.
    pub(crate) fn configure(&mut self, config: Config) {
        let effects = self.model.configure(config.settings.clone());
        self.config = config;
        self.apply_effects(effects, Origin::Service, None);
        self.start_or_stop();
        self.dirty = true;
    }

    fn start_or_stop(&mut self) {
        match (&self.bus, self.config.serve, self.server.is_some()) {
            (Bus::On(address), true, false) => {
                self.server = Server::start(address.clone(), self.registry.clone());
            }
            (_, false, true) | (Bus::Nowhere, _, true) => {
                self.server = None;
                self.serving = false;
                self.dirty = true;
            }
            _ => {}
        }
    }

    pub(crate) fn view(&self) -> Rc<View> {
        self.view.clone()
    }

    /// Whether the server is running at all.
    pub(crate) fn available(&self) -> bool {
        self.server.is_some()
    }

    /// What the server heard since the last call.
    pub(crate) fn take_facts(&mut self) -> Vec<server::Fact> {
        self.server
            .as_ref()
            .map(|server| server.facts.try_iter().collect())
            .unwrap_or_default()
    }

    pub(crate) fn set_serving(&mut self, serving: bool) {
        if self.serving != serving {
            self.serving = serving;
            self.dirty = true;
            self.events.push(Event {
                what: "owner",
                id: 0,
                origin: Origin::External,
                detail: String::new(),
            });
        }
    }

    pub(crate) fn set_owner(&mut self, owner: Option<Owner>) {
        if self.owner != owner {
            self.owner = owner;
            self.dirty = true;
        }
    }

    /// The connection went away: nothing is served until a reload starts it
    /// again.
    pub(crate) fn down(&mut self) {
        self.server = None;
        self.set_serving(false);
    }

    /// An id for one of the shell's own notifications.
    pub(crate) fn allocate_own(&self) -> u32 {
        server::lock(&self.registry).allocate_own()
    }

    /// One arrival, decided. Images go to Qt here; the rows follow at the
    /// next publish.
    pub(crate) fn arrive(
        &mut self,
        incoming: Incoming,
        decision: Decision,
        joined: Joined,
        origin: Origin,
        now: Duration,
    ) {
        let id = incoming.id;
        let replaced = self.model.get(id).is_some();
        self.joined.insert(id, joined);
        let image = incoming.image.clone();
        let effects = self.model.arrive(incoming, decision, now);
        if let Some(item) = self.model.get(id) {
            let revision = item.revision;
            match image {
                Some(image) => {
                    if crate::qml::hosted::set_notification_image(id, revision, &image) {
                        self.images.insert(id, revision);
                    }
                }
                None => {
                    if self.images.remove(&id).is_some() {
                        crate::qml::hosted::drop_notification_image(id);
                    }
                }
            }
            self.events.push(Event {
                what: if replaced { "replaced" } else { "added" },
                id,
                origin,
                detail: String::new(),
            });
        }
        self.apply_effects(effects, Origin::Service, None);
        self.dirty = true;
    }

    /// Banners whose time is up, and the pause while locked or idle. A kept
    /// banner moving to the history tells nobody but the scenes, so the
    /// count of banners is what says the rows changed.
    pub(crate) fn tick(&mut self, now: Duration, paused: bool) {
        self.model.set_paused(paused, now);
        let before = self.model.banners().count();
        let effects = self.model.expire(now);
        if !effects.is_empty() || self.model.banners().count() != before {
            self.dirty = true;
        }
        self.apply_effects(effects, Origin::Service, None);
    }

    /// A banner held or let go by a scene: the row's `held` changes.
    pub(crate) fn hold(&mut self, id: u32, held: bool, now: Duration) {
        if held {
            self.model.hold(id, now);
        } else {
            self.model.release(id, now);
        }
        self.dirty = true;
    }

    /// Apply what the model said must be told: signals to the server, the
    /// registry freed, images dropped, events for Lua. `token` is the
    /// activation token for an invoke.
    pub(crate) fn apply_effects(
        &mut self,
        effects: Vec<Effect>,
        origin: Origin,
        token: Option<String>,
    ) {
        for effect in effects {
            match effect {
                Effect::Closed { id, reason } => {
                    server::lock(&self.registry).forget(id);
                    self.joined.remove(&id);
                    self.own.remove(&id);
                    if self.images.remove(&id).is_some() {
                        crate::qml::hosted::drop_notification_image(id);
                    }
                    if let Some(server) = &self.server {
                        server.say(Signal::Closed {
                            id,
                            reason: reason as u32,
                        });
                    }
                    self.events.push(Event {
                        what: "closed",
                        id,
                        origin,
                        detail: reason.name().to_owned(),
                    });
                    self.dirty = true;
                }
                Effect::Invoked { id, key } => {
                    if let Some(server) = &self.server {
                        server.say(Signal::Invoked {
                            id,
                            key: key.clone(),
                            token: token.clone(),
                        });
                    }
                    self.events.push(Event {
                        what: "invoked",
                        id,
                        origin,
                        detail: key,
                    });
                }
            }
        }
    }

    pub(crate) fn set_dnd(&mut self, on: bool, origin: Origin) {
        if self.model.dnd() != on {
            self.model.set_dnd(on);
            self.dirty = true;
            self.events.push(Event {
                what: "dnd",
                id: 0,
                origin,
                detail: String::new(),
            });
        }
    }

    pub(crate) fn joined(&self, id: u32) -> Option<&Joined> {
        self.joined.get(&id)
    }

    /// The view rebuilt, if anything changed since it was last built: true
    /// when it was.
    pub(crate) fn refresh_view(&mut self) -> bool {
        if !self.dirty {
            return false;
        }
        self.dirty = false;
        let view = View {
            available: self.server.is_some(),
            serving: self.serving,
            dnd: self.model.dnd(),
            owner: self.owner.clone(),
            banners: self.model.banners().map(|item| self.row(item)).collect(),
            history: self.model.history().map(|item| self.row(item)).collect(),
        };
        if *self.view == view {
            return false;
        }
        self.view = Rc::new(view);
        true
    }

    /// One notification as a row: what a scene's delegate and a Lua handler
    /// read.
    fn row(&self, item: &Item) -> BTreeMap<&'static str, Json> {
        let n = &item.incoming;
        let joined = self.joined.get(&n.id).cloned().unwrap_or_default();
        let app_name = if joined.app_name.is_empty() {
            n.app_name.clone()
        } else {
            joined.app_name.clone()
        };
        let icon_name = if n.app_icon.is_empty() {
            joined.app_icon.clone()
        } else {
            n.app_icon.clone()
        };
        let icon = if icon_name.is_empty() {
            String::new()
        } else {
            format!("image://solium/icon/{icon_name}")
        };
        let image = self
            .images
            .get(&n.id)
            .map(|revision| format!("image://solium/notification/{}/{revision}", n.id))
            .unwrap_or_default();
        let actions = n
            .actions
            .iter()
            .filter(|(key, _)| key != "default")
            .map(|(key, label)| {
                Json::Object(BTreeMap::from([
                    ("key".to_owned(), Json::Text(key.clone())),
                    ("label".to_owned(), Json::Text(label.clone())),
                ]))
            })
            .collect();
        let state = match item.shown {
            Shown::Banner { .. } => "banner",
            Shown::Stored => "stored",
        };
        BTreeMap::from([
            ("id", Json::Number(f64::from(n.id))),
            ("revision", Json::Number(f64::from(item.revision))),
            ("appName", Json::Text(app_name)),
            ("appIcon", Json::Text(icon_name)),
            ("desktopId", Json::Text(n.desktop_entry.clone())),
            ("window", Json::Number(joined.window as f64)),
            ("monitor", Json::Text(joined.monitor)),
            ("summary", Json::Text(n.summary.clone())),
            ("body", Json::Text(n.body.plain.clone())),
            ("bodyRich", Json::Text(n.body.rich.clone())),
            ("image", Json::Text(image)),
            ("icon", Json::Text(icon)),
            ("urgency", Json::Text(n.urgency.name().to_owned())),
            ("category", Json::Text(n.category.clone())),
            ("actions", Json::List(actions)),
            ("hasDefault", Json::Bool(item.has_default())),
            ("resident", Json::Bool(n.resident)),
            ("transient", Json::Bool(n.transient)),
            ("received", Json::Number(n.received as f64)),
            ("held", Json::Bool(item.held)),
            ("state", Json::Text(state.to_owned())),
        ])
    }
}

/// The Lua-facing table for one arrival, before the decision: what the
/// `notification` hook is handed.
pub(crate) fn arrival_json(incoming: &Incoming, joined: &Joined, dnd: bool) -> Json {
    let actions = incoming
        .actions
        .iter()
        .map(|(key, label)| {
            Json::Object(BTreeMap::from([
                ("key".to_owned(), Json::Text(key.clone())),
                ("label".to_owned(), Json::Text(label.clone())),
            ]))
        })
        .collect();
    Json::Object(BTreeMap::from([
        ("id".to_owned(), Json::Number(f64::from(incoming.id))),
        ("appName".to_owned(), Json::Text(incoming.app_name.clone())),
        ("desktopId".to_owned(), Json::Text(incoming.desktop_entry.clone())),
        ("summary".to_owned(), Json::Text(incoming.summary.clone())),
        ("body".to_owned(), Json::Text(incoming.body.plain.clone())),
        ("urgency".to_owned(), Json::Text(incoming.urgency.name().to_owned())),
        ("category".to_owned(), Json::Text(incoming.category.clone())),
        ("actions".to_owned(), Json::List(actions)),
        ("transient".to_owned(), Json::Bool(incoming.transient)),
        ("resident".to_owned(), Json::Bool(incoming.resident)),
        ("window".to_owned(), Json::Number(joined.window as f64)),
        ("monitor".to_owned(), Json::Text(joined.monitor.clone())),
        ("dnd".to_owned(), Json::Bool(dnd)),
    ]))
}

/// A decision table returned by a `notification` hook: `banner` and `keep`,
/// each defaulting to the shipped rule when absent.
/// `a_hook_that_returns_banner_false_keeps_it_out_of_the_banners`.
pub(crate) fn decision_from(table: &Json, fallback: Decision) -> Decision {
    let flag = |key: &str, default: bool| match table.get(key) {
        Some(Json::Bool(value)) => *value,
        _ => default,
    };
    Decision {
        banner: flag("banner", fallback.banner),
        keep: flag("keep", fallback.keep),
    }
}

/// `sol.notify{ ... }`: one of the shell's own, with no bus behind it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Own {
    pub(crate) summary: String,
    pub(crate) body: String,
    pub(crate) icon: String,
    pub(crate) urgency: Urgency,
    /// `(key, label)`.
    pub(crate) actions: Vec<(String, String)>,
    /// Where Lua keeps the action functions, 0 for none.
    pub(crate) token: u64,
    pub(crate) expire_timeout: i32,
}

impl Own {
    pub(crate) fn incoming(&self, id: u32) -> Incoming {
        let received = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX));
        Incoming {
            id,
            sender: String::new(),
            pid: Some(std::process::id()),
            app_name: "Solium".to_owned(),
            desktop_entry: String::new(),
            app_icon: self.icon.clone(),
            summary: content::body(&self.summary).plain,
            body: content::body(&self.body),
            image: None,
            urgency: self.urgency,
            category: String::new(),
            actions: self.actions.clone(),
            resident: false,
            transient: false,
            expire_timeout: self.expire_timeout,
            received,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hook_that_returns_banner_false_keeps_it_out_of_the_banners() {
        let fallback = Decision {
            banner: true,
            keep: true,
        };
        let table = Json::Object(BTreeMap::from([("banner".to_owned(), Json::Bool(false))]));
        assert_eq!(
            decision_from(&table, fallback),
            Decision {
                banner: false,
                keep: true
            }
        );
        assert_eq!(decision_from(&Json::Null, fallback), fallback);
    }

    #[test]
    fn a_nested_run_serves_nothing_unless_it_is_given_a_bus() {
        let mut service = Service::new(None);
        service.configure(Config {
            serve: true,
            ..Config::default()
        });
        service.permit(Place::Nested, None);
        assert!(!service.available());
        assert_eq!(service.bus, Bus::Nowhere);
    }

    #[test]
    fn a_closed_notification_frees_its_id_and_tells_lua_why() {
        let mut service = Service::new(None);
        let now = Duration::ZERO;
        let id = service.allocate_own();
        let own = Own {
            summary: "Hello".to_owned(),
            body: "<b>there</b>".to_owned(),
            icon: "dialog-information".to_owned(),
            urgency: Urgency::Normal,
            actions: Vec::new(),
            token: 0,
            expire_timeout: -1,
        };
        service.arrive(
            own.incoming(id),
            Decision {
                banner: true,
                keep: true,
            },
            Joined::default(),
            Origin::Script,
            now,
        );
        assert!(service.refresh_view());
        let view = service.view();
        assert_eq!(view.banners.len(), 1);
        assert_eq!(
            view.banners[0].get("bodyRich"),
            Some(&Json::Text("<b>there</b>".to_owned()))
        );
        assert_eq!(
            view.banners[0].get("icon"),
            Some(&Json::Text("image://solium/icon/dialog-information".to_owned()))
        );
        let effects = service.model.dismiss(id);
        service.apply_effects(effects, Origin::Scene, None);
        let events: Vec<_> = std::mem::take(&mut service.events);
        assert_eq!(events[0].what, "added");
        assert_eq!(events[1].what, "closed");
        assert_eq!(events[1].detail, "dismissed");
        assert_eq!(events[1].origin, Origin::Scene);
        assert!(service.refresh_view());
        assert!(service.view().banners.is_empty());
    }
}
