//! The tray (#363): Solium as the session's `org.kde.StatusNotifierWatcher`
//! and its host, and the items applications put there (Steam, Discord,
//! Telegram...), for scenes (`Tray`) and for Lua (`sol.tray`,
//! `sol.on("tray")`, `tray.*` actions).
//!
//! * [`content`] makes what an item sends safe to show: its pictures, its
//!   labels, the icon files it names;
//! * [`model`] is the items, their menus and which menu is open, as a pure
//!   model;
//! * [`server`] is the bus, on threads of its own;
//! * this module holds them together for the event loop: it starts the
//!   server when the configuration asks and the session allows, applies what
//!   the server read, hands pictures to Qt, and turns what a scene or Lua
//!   asks into calls. `crate::services::settle` drives it.

pub(crate) mod content;
pub(crate) mod model;
pub(crate) mod server;

use std::{collections::BTreeMap, rc::Rc};

use crate::{
    json::Json,
    services::notifications::{Origin, icon_name, server::Owner},
    session::Place,
};

use model::{Call, Change, Entry, Item, Outcome, Picture, Status, Tray};
use server::{Fact, Server};

/// What the configuration says: `sol.tray{ ... }`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Config {
    /// Own `org.kde.StatusNotifierWatcher`.
    pub(crate) serve: bool,
}

/// Which bus the name may be owned on, as `session::speaks` decides: a
/// nested run never takes the desktop's tray unless it is given a bus of its
/// own. `a_nested_run_serves_no_tray_unless_it_is_given_a_bus`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Bus {
    #[default]
    Nowhere,
    On(Option<String>),
}

/// What snapshots and scenes read.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct View {
    pub(crate) available: bool,
    pub(crate) serving: bool,
    pub(crate) owner: Option<Owner>,
    /// In the order the items first answered.
    pub(crate) items: Vec<BTreeMap<&'static str, Json>>,
}

/// One event for `sol.on("tray")`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Event {
    /// `added`, `changed`, `removed`, `menu` or `owner`.
    pub(crate) what: &'static str,
    pub(crate) id: u32,
    pub(crate) origin: Origin,
}

impl Event {
    pub(crate) fn to_json(&self, view: &View) -> Json {
        let mut fields = BTreeMap::from([
            ("what".to_owned(), Json::Text(self.what.to_owned())),
            (
                "origin".to_owned(),
                Json::Text(self.origin.name().to_owned()),
            ),
        ]);
        if self.id != 0 {
            fields.insert("id".to_owned(), Json::Number(f64::from(self.id)));
        }
        if self.what == "owner" {
            fields.insert("serving".to_owned(), Json::Bool(view.serving));
        }
        if let Some(row) = view
            .items
            .iter()
            .find(|row| row.get("id").and_then(Json::as_u64) == Some(u64::from(self.id)))
        {
            if self.what == "menu" {
                fields.insert(
                    "open".to_owned(),
                    row.get("menuOpen").cloned().unwrap_or(Json::Bool(false)),
                );
            }
            fields.insert(
                "item".to_owned(),
                crate::services::notifications::object(row),
            );
        }
        Json::Object(fields)
    }
}

/// The most facts taken from the server in one settle: a flood waits for the
/// next loops rather than freezing this one.
const FACTS_PER_SETTLE: usize = 64;

/// The service, owned by `Solium`.
#[derive(Debug, Default)]
pub(crate) struct Service {
    bus: Bus,
    config: Config,
    server: Option<Server>,
    pub(crate) model: Tray,
    serving: bool,
    owner: Option<Owner>,
    view: Rc<View>,
    dirty: bool,
    /// For `sol.on("tray")`, in order, drained each settle.
    pub(crate) events: Vec<Event>,
}

impl Service {
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
                    server::WATCHER
                );
            }
            Bus::Nowhere
        };
        self.start_or_stop();
    }

    /// `sol.tray{ ... }`, at load and on every reload.
    pub(crate) fn configure(&mut self, config: Config) {
        self.config = config;
        self.start_or_stop();
        self.dirty = true;
    }

    fn start_or_stop(&mut self) {
        match (&self.bus, self.config.serve, self.server.is_some()) {
            (Bus::On(address), true, false) => {
                self.server = Server::start(address.clone());
                self.dirty = true;
            }
            (_, false, true) | (Bus::Nowhere, _, true) => {
                self.server = None;
                self.set_serving(false);
                self.clear();
            }
            _ => {}
        }
    }

    pub(crate) fn view(&self) -> Rc<View> {
        self.view.clone()
    }

    /// Whether the server is running at all.
    #[cfg(test)]
    pub(crate) fn available(&self) -> bool {
        self.server.is_some()
    }

    /// What the server heard since the last settle, applied.
    pub(crate) fn settle(&mut self) {
        let facts: Vec<Fact> = self
            .server
            .as_ref()
            .map(|server| server.facts.try_iter().take(FACTS_PER_SETTLE).collect())
            .unwrap_or_default();
        for fact in facts {
            self.apply(fact);
        }
    }

    /// One fact from the server.
    pub(crate) fn apply(&mut self, fact: Fact) {
        match fact {
            Fact::Read(read) => {
                let server::ReadItem {
                    key,
                    props,
                    pictures,
                } = *read;
                let outcome = self.model.read(&key, props);
                let id = outcome.id;
                for picture in outcome.new_pictures {
                    let at = picture as usize;
                    if let Some(image) = &pictures[at] {
                        crate::qml::hosted::set_tray_image(id, picture as u32, image);
                    }
                }
                for picture in outcome.dropped_pictures {
                    crate::qml::hosted::drop_tray_image(id, picture as u32);
                }
                if let Some(change) = outcome.change {
                    self.changed(change, Origin::External);
                }
            }
            Fact::Gone { key } => {
                if let Some(id) = self.model.gone(&key) {
                    self.forget(id);
                    self.changed(Change::Removed(id), Origin::External);
                }
            }
            Fact::Menu { key, entries } => {
                if let Some(change) = self.model.menu_read(&key, entries) {
                    self.changed(change, Origin::External);
                }
            }
            Fact::MenuFailed { key } => {
                if let Some(change) = self.model.menu_failed(&key) {
                    self.changed(change, Origin::External);
                }
            }
            Fact::MenuChanged { key } => {
                if let Some((key, call)) = self.model.menu_changed(&key) {
                    self.call(&key, call);
                }
            }
            Fact::ActivateRefused { key, x, y } => {
                let outcome = self.model.activate_refused(&key, x, y);
                self.perform(outcome, Origin::External);
            }
            Fact::Serving(serving) => {
                self.set_serving(serving);
            }
            Fact::Owner(owner) => {
                if self.owner != owner {
                    self.owner = owner;
                    self.dirty = true;
                }
            }
            Fact::Cleared => self.clear(),
            // The connection went away: no tray until a reload starts it
            // again.
            Fact::Down => {
                self.server = None;
                self.set_serving(false);
                self.clear();
            }
        }
    }

    fn set_serving(&mut self, serving: bool) {
        if self.serving != serving {
            self.serving = serving;
            self.dirty = true;
            self.events.push(Event {
                what: "owner",
                id: 0,
                origin: Origin::External,
            });
        }
    }

    /// Every item gone.
    fn clear(&mut self) {
        for id in self.model.clear() {
            self.forget(id);
            self.changed(Change::Removed(id), Origin::External);
        }
    }

    /// An item's pictures let go.
    fn forget(&self, id: u32) {
        for picture in Picture::ALL {
            crate::qml::hosted::drop_tray_image(id, picture as u32);
        }
    }

    fn changed(&mut self, change: Change, origin: Origin) {
        self.dirty = true;
        let (what, id) = match change {
            Change::Added(id) => ("added", id),
            Change::Changed(id) => ("changed", id),
            Change::Removed(id) => ("removed", id),
            Change::Menu(id) => ("menu", id),
        };
        self.events.push(Event { what, id, origin });
    }

    fn call(&self, key: &str, call: Call) {
        if let Some(server) = &self.server {
            server.call(key, call);
        }
    }

    /// What an action led to: its calls made, its changes told.
    pub(crate) fn perform(&mut self, outcome: Outcome, origin: Origin) {
        for (key, call) in outcome.calls {
            self.call(&key, call);
        }
        for change in outcome.changes {
            self.changed(change, origin);
        }
    }

    /// A `tray.*` action from Lua or a scene, done. `token` is the activation
    /// token an `Activate` hands the item, for the window it raises. Returns
    /// why it could not be done.
    /// `a_tray_action_names_its_item_and_answers_why_it_could_not`.
    pub(crate) fn act(
        &mut self,
        action: &str,
        data: &Json,
        origin: Origin,
        token: impl FnOnce(&Item) -> Option<String>,
    ) -> Result<(), &'static str> {
        let number = |key: &str| -> Option<f64> {
            match data.get(key) {
                Some(Json::Number(n)) if n.is_finite() => Some(*n),
                _ => None,
            }
        };
        let int = |key: &str| number(key).map_or(0, |n| n.clamp(-1.0e6, 1.0e6) as i32);
        let id = || {
            data.get("id")
                .and_then(Json::as_u64)
                .and_then(|id| u32::try_from(id).ok())
                .ok_or("bad-data")
        };
        let entry = || {
            number("entry")
                .filter(|n| {
                    n.fract() == 0.0 && *n >= f64::from(i32::MIN) && *n <= f64::from(i32::MAX)
                })
                .map(|n| n as i32)
                .ok_or("bad-data")
        };
        let (x, y) = (int("x"), int("y"));
        let mut outcome = match action {
            "tray.activate" => self.model.activate(id()?, x, y)?,
            "tray.secondary_activate" => self.model.secondary_activate(id()?, x, y)?,
            "tray.context_menu" => self.model.context_menu(id()?, x, y)?,
            "tray.scroll" => {
                let horizontal = match data.get("orientation").and_then(Json::as_str) {
                    None | Some("vertical") => false,
                    Some("horizontal") => true,
                    Some(_) => return Err("bad-data"),
                };
                let delta = number("delta").ok_or("bad-data")?;
                self.model
                    .scroll(id()?, delta.clamp(-1.0e6, 1.0e6) as i32, horizontal)?
            }
            "tray.open_menu" => self.model.open_menu(id()?, x, y)?,
            "tray.close_menu" => match data.get("id") {
                None => self.model.close_menus(),
                Some(_) => self.model.close_menu(id()?)?,
            },
            "tray.menu_click" => self.model.click(id()?, entry()?)?,
            "tray.submenu" => self.model.open_submenu(id()?, entry()?)?,
            _ => return Err("unknown-action"),
        };
        // The token is minted only for a call that is made.
        let mut token = Some(token);
        for (key, call) in &mut outcome.calls {
            if let Call::Activate { token: slot, .. } = call
                && let Some(mint) = token.take()
                && let Some(item) = self.model.id_of(key).and_then(|id| self.model.get(id))
            {
                *slot = mint(item);
            }
        }
        self.perform(outcome, origin);
        Ok(())
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
            owner: self.owner.clone(),
            items: self.model.items().map(row).collect(),
        };
        if *self.view == view {
            return false;
        }
        self.view = Rc::new(view);
        true
    }
}

/// The URL of one of an item's pictures: its pixels, through the tray's
/// image route, else its theme icon, else nothing.
fn picture_url(item: &Item, picture: Picture) -> String {
    let at = picture as usize;
    if item.props.pictures[at] != 0 {
        return format!(
            "image://solium/tray/{}/{}/{}",
            item.id,
            picture.name(),
            item.revisions[at]
        );
    }
    let name = icon_name(&item.props.names[at]);
    if name.is_empty() {
        String::new()
    } else {
        format!("image://solium/icon/{name}")
    }
}

/// One item as a row: what a scene's delegate and a Lua handler read.
/// `an_item_s_row_shows_its_attention_icon_when_it_asks_for_attention`.
fn row(item: &Item) -> BTreeMap<&'static str, Json> {
    let props = &item.props;
    let normal = picture_url(item, Picture::Icon);
    let attention = picture_url(item, Picture::Attention);
    let icon = if props.status == Status::Attention && !attention.is_empty() {
        attention.clone()
    } else {
        normal
    };
    BTreeMap::from([
        ("id", Json::Number(f64::from(item.id))),
        ("service", Json::Text(item.key.clone())),
        ("itemId", Json::Text(props.item_id.clone())),
        ("category", Json::Text(props.category.clone())),
        ("title", Json::Text(props.title.clone())),
        ("status", Json::Text(props.status.name().to_owned())),
        ("icon", Json::Text(icon)),
        (
            "iconName",
            Json::Text(props.names[Picture::Icon as usize].clone()),
        ),
        ("attentionIcon", Json::Text(attention)),
        (
            "overlayIcon",
            Json::Text(picture_url(item, Picture::Overlay)),
        ),
        ("tooltipTitle", Json::Text(props.tooltip_title.clone())),
        ("tooltipBody", Json::Text(props.tooltip_body.clone())),
        ("itemIsMenu", Json::Bool(props.item_is_menu)),
        ("hasMenu", Json::Bool(item.has_menu())),
        ("menuOpen", Json::Bool(item.menu_open)),
        (
            "menu",
            Json::List(item.menu.as_deref().map(entries_json).unwrap_or_default()),
        ),
    ])
}

/// A menu's entries as a scene and Lua read them, submenus as `children`.
fn entries_json(entries: &[Entry]) -> Vec<Json> {
    entries
        .iter()
        .map(|entry| {
            let icon = if entry.icon_name.is_empty() {
                String::new()
            } else {
                format!("image://solium/icon/{}", entry.icon_name)
            };
            Json::Object(BTreeMap::from([
                ("id".to_owned(), Json::Number(f64::from(entry.id))),
                ("label".to_owned(), Json::Text(entry.label.clone())),
                ("enabled".to_owned(), Json::Bool(entry.enabled)),
                ("visible".to_owned(), Json::Bool(entry.visible)),
                (
                    "type".to_owned(),
                    Json::Text(
                        if entry.separator {
                            "separator"
                        } else {
                            "standard"
                        }
                        .to_owned(),
                    ),
                ),
                (
                    "toggleType".to_owned(),
                    Json::Text(entry.toggle.name().to_owned()),
                ),
                (
                    "toggleState".to_owned(),
                    Json::Number(f64::from(entry.toggle_state)),
                ),
                ("iconName".to_owned(), Json::Text(entry.icon_name.clone())),
                ("icon".to_owned(), Json::Text(icon)),
                ("submenu".to_owned(), Json::Bool(entry.submenu)),
                (
                    "children".to_owned(),
                    Json::List(entries_json(&entry.children)),
                ),
            ]))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::Props;

    fn steam() -> Props {
        Props {
            item_id: "steam".to_owned(),
            title: "Steam".to_owned(),
            names: [
                "steam".to_owned(),
                "steam-attention".to_owned(),
                String::new(),
            ],
            menu: "/MenuBar".to_owned(),
            ..Props::default()
        }
    }

    fn read(service: &mut Service, key: &str, props: Props) {
        service.apply(Fact::Read(Box::new(server::ReadItem {
            key: key.to_owned(),
            props,
            pictures: [None, None, None],
        })));
    }

    #[test]
    fn a_nested_run_serves_no_tray_unless_it_is_given_a_bus() {
        let mut service = Service::default();
        service.configure(Config { serve: true });
        service.permit(Place::Nested, None);
        assert!(!service.available());
        assert_eq!(service.bus, Bus::Nowhere);
    }

    #[test]
    fn an_item_s_row_shows_its_attention_icon_when_it_asks_for_attention() {
        let mut service = Service::default();
        read(&mut service, ":1.7/StatusNotifierItem", steam());
        assert!(service.refresh_view());
        let view = service.view();
        let row = &view.items[0];
        assert_eq!(
            row.get("icon"),
            Some(&Json::Text("image://solium/icon/steam".to_owned()))
        );
        assert_eq!(row.get("hasMenu"), Some(&Json::Bool(true)));
        assert_eq!(row.get("menu"), Some(&Json::List(Vec::new())));
        let mut calling = steam();
        calling.status = Status::Attention;
        calling.pictures[Picture::Attention as usize] = 5;
        read(&mut service, ":1.7/StatusNotifierItem", calling);
        assert!(service.refresh_view());
        let view = service.view();
        let id = view.items[0].get("id").and_then(Json::as_u64).unwrap_or(0);
        assert_eq!(
            view.items[0].get("icon"),
            Some(&Json::Text(format!("image://solium/tray/{id}/attention/1")))
        );
        assert_eq!(
            view.items[0].get("status"),
            Some(&Json::Text("attention".to_owned()))
        );
        let events: Vec<&str> = service.events.iter().map(|event| event.what).collect();
        assert_eq!(events, ["added", "changed"]);
    }

    #[test]
    fn a_tray_action_names_its_item_and_answers_why_it_could_not() {
        let mut service = Service::default();
        read(&mut service, ":1.7/StatusNotifierItem", steam());
        let id = service
            .model
            .id_of(":1.7/StatusNotifierItem")
            .map_or(0.0, f64::from);
        let data = |fields: &[(&str, Json)]| {
            Json::Object(
                fields
                    .iter()
                    .map(|(key, value)| ((*key).to_owned(), value.clone()))
                    .collect(),
            )
        };
        let none = |_: &Item| None;
        assert_eq!(
            service.act("tray.activate", &data(&[]), Origin::Scene, none),
            Err("bad-data")
        );
        assert_eq!(
            service.act(
                "tray.activate",
                &data(&[("id", Json::Number(99.0))]),
                Origin::Scene,
                none
            ),
            Err("unknown-item")
        );
        assert_eq!(
            service.act(
                "tray.scroll",
                &data(&[
                    ("id", Json::Number(id)),
                    ("delta", Json::Number(120.0)),
                    ("orientation", Json::Text("diagonal".to_owned()))
                ]),
                Origin::Scene,
                none
            ),
            Err("bad-data")
        );
        assert_eq!(
            service.act("tray.nonsense", &data(&[]), Origin::Scene, none),
            Err("unknown-action")
        );
        // A token is minted for an activation, and only for one.
        let mut minted = 0;
        service
            .act(
                "tray.activate",
                &data(&[("id", Json::Number(id))]),
                Origin::Scene,
                |item: &Item| {
                    minted += 1;
                    Some(item.props.item_id.clone())
                },
            )
            .expect("done");
        assert_eq!(minted, 1);
        service
            .act(
                "tray.open_menu",
                &data(&[("id", Json::Number(id))]),
                Origin::Script,
                |_: &Item| panic!("no token for a menu"),
            )
            .expect("done");
        assert!(
            service
                .events
                .iter()
                .any(|event| event.what == "menu" && event.origin == Origin::Script)
        );
        assert_eq!(
            service.act(
                "tray.menu_click",
                &data(&[("id", Json::Number(id)), ("entry", Json::Number(1.5))]),
                Origin::Scene,
                none
            ),
            Err("bad-data")
        );
    }

    #[test]
    fn an_item_that_leaves_is_removed_with_an_event_and_a_lost_name_empties_the_tray() {
        let mut service = Service::default();
        read(&mut service, ":1.7/StatusNotifierItem", steam());
        read(&mut service, ":1.9/StatusNotifierItem", steam());
        service.apply(Fact::Gone {
            key: ":1.7/StatusNotifierItem".to_owned(),
        });
        assert!(service.refresh_view());
        assert_eq!(service.view().items.len(), 1);
        service.apply(Fact::Cleared);
        assert!(service.refresh_view());
        assert!(service.view().items.is_empty());
        let events: Vec<&str> = service.events.iter().map(|event| event.what).collect();
        assert_eq!(events, ["added", "added", "removed", "removed"]);
    }

    #[test]
    fn a_menu_event_tells_lua_whether_it_is_open_and_hands_over_its_entries() {
        let mut service = Service::default();
        let key = ":1.7/StatusNotifierItem";
        read(&mut service, key, steam());
        let id = service.model.id_of(key).unwrap_or(0);
        let opened = service.model.open_menu(id, 0, 0).expect("opened");
        service.perform(opened, Origin::Scene);
        service.apply(Fact::Menu {
            key: key.to_owned(),
            entries: vec![Entry {
                id: 7,
                label: "Quit".to_owned(),
                ..Entry::default()
            }],
        });
        assert!(service.refresh_view());
        let view = service.view();
        let event = Event {
            what: "menu",
            id,
            origin: Origin::External,
        }
        .to_json(&view);
        assert_eq!(event.get("open"), Some(&Json::Bool(true)));
        let menu = event
            .get("item")
            .and_then(|item| item.get("menu"))
            .cloned()
            .unwrap_or(Json::Null);
        let Json::List(entries) = menu else {
            panic!("a list of entries");
        };
        assert_eq!(
            entries[0].get("label"),
            Some(&Json::Text("Quit".to_owned()))
        );
        assert_eq!(
            entries[0].get("type"),
            Some(&Json::Text("standard".to_owned()))
        );
    }
}
