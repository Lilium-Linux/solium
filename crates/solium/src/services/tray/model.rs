//! The tray's items, as a pure model: which items there are, what each says
//! of itself, its menu, and which menu is open. No bus and no Qt: what an
//! item said comes in already read and made safe ([`Props`], [`Entry`]), and
//! what must be said to an item goes out as [`Call`]s.

use std::collections::{BTreeMap, HashMap};

/// What an item says it needs: `Status`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Status {
    /// Nothing to show: a shell may hide it.
    Passive,
    #[default]
    Active,
    /// It asks to be looked at; its attention icon, if it has one, is shown.
    Attention,
}

impl Status {
    /// As the item sends it; anything else reads as `Active`.
    pub(crate) fn from_name(name: &str) -> Self {
        match name {
            "Passive" => Self::Passive,
            "NeedsAttention" => Self::Attention,
            _ => Self::Active,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Passive => "passive",
            Self::Active => "active",
            Self::Attention => "attention",
        }
    }
}

/// An item's three pictures, each a theme icon name or pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Picture {
    Icon = 0,
    Attention = 1,
    Overlay = 2,
}

impl Picture {
    pub(crate) const ALL: [Self; 3] = [Self::Icon, Self::Attention, Self::Overlay];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Icon => "icon",
            Self::Attention => "attention",
            Self::Overlay => "overlay",
        }
    }
}

/// What an item says of itself, from its `org.kde.StatusNotifierItem`
/// properties, already made safe to show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Props {
    /// `Id`: the item's own name for itself, often the application's.
    pub(crate) item_id: String,
    /// `ApplicationStatus`, `Communications`, `SystemServices` or `Hardware`.
    pub(crate) category: String,
    pub(crate) title: String,
    pub(crate) status: Status,
    /// Theme icon names, by [`Picture`]; empty for none.
    pub(crate) names: [String; 3],
    /// A digest of each picture sent as pixels (or found in its
    /// `IconThemePath`), by [`Picture`]; 0 for none.
    pub(crate) pictures: [u64; 3],
    pub(crate) tooltip_title: String,
    pub(crate) tooltip_body: String,
    /// `ItemIsMenu`: a primary click opens its menu instead of activating it.
    pub(crate) item_is_menu: bool,
    /// `Menu`: its `com.canonical.dbusmenu` object, or empty.
    pub(crate) menu: String,
}

/// How a menu entry toggles: `toggle-type`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Toggle {
    #[default]
    None,
    Checkmark,
    Radio,
}

impl Toggle {
    pub(crate) fn from_name(name: &str) -> Self {
        match name {
            "checkmark" => Self::Checkmark,
            "radio" => Self::Radio,
            _ => Self::None,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Checkmark => "checkmark",
            Self::Radio => "radio",
        }
    }
}

/// One entry of an item's menu, as `com.canonical.dbusmenu` lays it out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) id: i32,
    /// Its label, mnemonic marks taken out.
    pub(crate) label: String,
    pub(crate) enabled: bool,
    pub(crate) visible: bool,
    pub(crate) separator: bool,
    pub(crate) toggle: Toggle,
    /// `toggle-state`: 1 on, 0 off, -1 neither.
    pub(crate) toggle_state: i32,
    /// A theme icon name, or empty.
    pub(crate) icon_name: String,
    /// It opens a submenu (`children-display` is `submenu`, or it has
    /// children).
    pub(crate) submenu: bool,
    pub(crate) children: Vec<Entry>,
}

impl Default for Entry {
    fn default() -> Self {
        Self {
            id: 0,
            label: String::new(),
            enabled: true,
            visible: true,
            separator: false,
            toggle: Toggle::None,
            toggle_state: -1,
            icon_name: String::new(),
            submenu: false,
            children: Vec::new(),
        }
    }
}

/// `id` in `entries`, at any depth.
pub(crate) fn find(entries: &[Entry], id: i32) -> Option<&Entry> {
    entries.iter().find_map(|entry| {
        if entry.id == id {
            Some(entry)
        } else {
            find(&entry.children, id)
        }
    })
}

/// One item in the tray.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Item {
    /// Solium's id for it, never given out twice in a run.
    pub(crate) id: u32,
    /// The watcher's name for it: its bus name and its object path.
    pub(crate) key: String,
    pub(crate) props: Props,
    /// How many times each picture's pixels changed, by [`Picture`]: a new
    /// one is a new URL, so Qt does not show the cached old one.
    pub(crate) revisions: [u32; 3],
    /// Its menu, once read.
    pub(crate) menu: Option<Vec<Entry>>,
    /// Its menu is open: a shell shows it.
    pub(crate) menu_open: bool,
}

impl Item {
    pub(crate) fn has_menu(&self) -> bool {
        !self.props.menu.is_empty()
    }
}

/// What the bus is asked to do for an item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Call {
    /// `Activate(x, y)`, after `ProvideXdgActivationToken(token)` when there
    /// is a token.
    Activate {
        x: i32,
        y: i32,
        token: Option<String>,
    },
    SecondaryActivate {
        x: i32,
        y: i32,
    },
    Scroll {
        delta: i32,
        horizontal: bool,
    },
    /// `ContextMenu(x, y)`: the item draws its own menu.
    ContextMenu {
        x: i32,
        y: i32,
    },
    /// `AboutToShow(entry)` when there is one, then `GetLayout`: the menu
    /// read, or read again.
    LoadMenu {
        about_to_show: Option<i32>,
    },
    /// `Event(entry, event, 0, timestamp)`: `clicked`, `opened`, `closed`.
    MenuEvent {
        entry: i32,
        event: &'static str,
    },
}

/// What changed, for Lua's events and for the rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Change {
    Added(u32),
    Changed(u32),
    Removed(u32),
    /// Its menu opened, closed, or was read again.
    Menu(u32),
}

/// What an action leads to: calls to make, by item key, and what changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub(crate) calls: Vec<(String, Call)>,
    pub(crate) changes: Vec<Change>,
}

/// What reading an item's properties changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Read {
    pub(crate) id: u32,
    pub(crate) change: Option<Change>,
    /// Pictures whose pixels are new: Qt is to be given them.
    pub(crate) new_pictures: Vec<Picture>,
    /// Pictures that are pixels no more: Qt is to let them go.
    pub(crate) dropped_pictures: Vec<Picture>,
}

/// Every item, in the order they first answered.
#[derive(Clone, Debug, Default)]
pub(crate) struct Tray {
    items: BTreeMap<u32, Item>,
    ids: HashMap<String, u32>,
    last: u32,
}

impl Tray {
    /// In the order they first answered.
    pub(crate) fn items(&self) -> impl Iterator<Item = &Item> {
        self.items.values()
    }

    pub(crate) fn get(&self, id: u32) -> Option<&Item> {
        self.items.get(&id)
    }

    pub(crate) fn id_of(&self, key: &str) -> Option<u32> {
        self.ids.get(key).copied()
    }

    /// An item's properties, read: the item is added the first time, and
    /// changed after. A picture whose pixels changed gets a new revision.
    /// `an_item_is_added_at_its_first_read_and_changed_at_the_next`,
    /// `a_new_picture_is_a_new_revision_and_the_same_one_is_not`.
    pub(crate) fn read(&mut self, key: &str, props: Props) -> Read {
        let (id, added) = match self.ids.get(key) {
            Some(&id) => (id, false),
            None => {
                self.last = self.last.wrapping_add(1).max(1);
                let id = self.last;
                self.ids.insert(key.to_owned(), id);
                self.items.insert(
                    id,
                    Item {
                        id,
                        key: key.to_owned(),
                        props: Props::default(),
                        revisions: [0; 3],
                        menu: None,
                        menu_open: false,
                    },
                );
                (id, true)
            }
        };
        let mut read = Read {
            id,
            change: None,
            new_pictures: Vec::new(),
            dropped_pictures: Vec::new(),
        };
        let Some(item) = self.items.get_mut(&id) else {
            return read;
        };
        for picture in Picture::ALL {
            let at = picture as usize;
            let (before, after) = (item.props.pictures[at], props.pictures[at]);
            if after != 0 && after != before {
                item.revisions[at] = item.revisions[at].wrapping_add(1);
                read.new_pictures.push(picture);
            } else if after == 0 && before != 0 {
                read.dropped_pictures.push(picture);
            }
        }
        // A menu that moved is not the menu read from the old object.
        if item.props.menu != props.menu {
            item.menu = None;
            item.menu_open = false;
        }
        let changed = item.props != props;
        item.props = props;
        read.change = if added {
            Some(Change::Added(id))
        } else if changed {
            Some(Change::Changed(id))
        } else {
            None
        };
        read
    }

    /// The item left the bus, or left the watcher: its id, if it was shown.
    /// `an_item_that_leaves_is_removed_and_its_id_not_given_out_again`.
    pub(crate) fn gone(&mut self, key: &str) -> Option<u32> {
        let id = self.ids.remove(key)?;
        self.items.remove(&id);
        Some(id)
    }

    /// Every item gone at once: the watcher's name was lost.
    pub(crate) fn clear(&mut self) -> Vec<u32> {
        self.ids.clear();
        let ids = self.items.keys().copied().collect();
        self.items.clear();
        ids
    }

    /// An item's menu, read. Kept only while it is open: a menu nobody shows
    /// is read again when it is next opened, as the item may have changed it
    /// meanwhile.
    pub(crate) fn menu_read(&mut self, key: &str, entries: Vec<Entry>) -> Option<Change> {
        let item = self.items.get_mut(self.ids.get(key)?)?;
        if !item.menu_open {
            return None;
        }
        if item.menu.as_ref() == Some(&entries) {
            return None;
        }
        item.menu = Some(entries);
        Some(Change::Menu(item.id))
    }

    /// An item's menu could not be read: it is not shown open and empty.
    pub(crate) fn menu_failed(&mut self, key: &str) -> Option<Change> {
        let item = self.items.get_mut(self.ids.get(key)?)?;
        if !item.menu_open || item.menu.is_some() {
            return None;
        }
        item.menu_open = false;
        Some(Change::Menu(item.id))
    }

    fn item(&self, id: u32) -> Result<&Item, &'static str> {
        self.items.get(&id).ok_or("unknown-item")
    }

    /// A primary click: its menu, for an item that is one (`ItemIsMenu`),
    /// else `Activate`.
    /// `a_primary_click_on_an_item_that_is_a_menu_opens_the_menu`.
    pub(crate) fn activate(&mut self, id: u32, x: i32, y: i32) -> Result<Outcome, &'static str> {
        let item = self.item(id)?;
        if item.props.item_is_menu && item.has_menu() {
            return self.open_menu(id, x, y);
        }
        Ok(Outcome {
            calls: vec![(item.key.clone(), Call::Activate { x, y, token: None })],
            changes: Vec::new(),
        })
    }

    /// `Activate` was refused, as an item that has only a menu refuses it:
    /// its menu opens instead, or it is asked to show its own.
    /// `a_refused_activate_opens_the_menu_or_asks_for_the_item_s_own`.
    pub(crate) fn activate_refused(&mut self, key: &str, x: i32, y: i32) -> Outcome {
        let Some(&id) = self.ids.get(key) else {
            return Outcome::default();
        };
        self.open_menu(id, x, y).unwrap_or_default()
    }

    pub(crate) fn secondary_activate(
        &self,
        id: u32,
        x: i32,
        y: i32,
    ) -> Result<Outcome, &'static str> {
        Ok(self.call(self.item(id)?, Call::SecondaryActivate { x, y }))
    }

    pub(crate) fn scroll(
        &self,
        id: u32,
        delta: i32,
        horizontal: bool,
    ) -> Result<Outcome, &'static str> {
        Ok(self.call(self.item(id)?, Call::Scroll { delta, horizontal }))
    }

    pub(crate) fn context_menu(&self, id: u32, x: i32, y: i32) -> Result<Outcome, &'static str> {
        Ok(self.call(self.item(id)?, Call::ContextMenu { x, y }))
    }

    fn call(&self, item: &Item, call: Call) -> Outcome {
        Outcome {
            calls: vec![(item.key.clone(), call)],
            changes: Vec::new(),
        }
    }

    /// Its menu, opened for a shell to draw: read again, and every other
    /// menu closed, as only one is open at a time. An item with no menu of
    /// its own to hand over is asked to show its own (`ContextMenu`).
    /// `opening_a_menu_reads_it_and_closes_any_other`.
    pub(crate) fn open_menu(&mut self, id: u32, x: i32, y: i32) -> Result<Outcome, &'static str> {
        let item = self.item(id)?;
        if !item.has_menu() {
            return Ok(self.call(item, Call::ContextMenu { x, y }));
        }
        let mut outcome = self.close_all_but(id);
        let Some(item) = self.items.get_mut(&id) else {
            return Err("unknown-item");
        };
        if !item.menu_open {
            item.menu_open = true;
            item.menu = None;
            outcome.changes.push(Change::Menu(id));
            outcome.calls.push((
                item.key.clone(),
                Call::MenuEvent {
                    entry: 0,
                    event: "opened",
                },
            ));
        }
        outcome.calls.push((
            item.key.clone(),
            Call::LoadMenu {
                about_to_show: Some(0),
            },
        ));
        Ok(outcome)
    }

    fn close_all_but(&mut self, keep: u32) -> Outcome {
        let open: Vec<u32> = self
            .items
            .values()
            .filter(|item| item.menu_open && item.id != keep)
            .map(|item| item.id)
            .collect();
        let mut outcome = Outcome::default();
        for id in open {
            outcome.extend(self.close_menu(id).unwrap_or_default());
        }
        outcome
    }

    /// Its menu closed. Closing one that is not open does nothing.
    pub(crate) fn close_menu(&mut self, id: u32) -> Result<Outcome, &'static str> {
        let item = self.items.get_mut(&id).ok_or("unknown-item")?;
        if !item.menu_open {
            return Ok(Outcome::default());
        }
        item.menu_open = false;
        item.menu = None;
        Ok(Outcome {
            calls: vec![(
                item.key.clone(),
                Call::MenuEvent {
                    entry: 0,
                    event: "closed",
                },
            )],
            changes: vec![Change::Menu(id)],
        })
    }

    /// Every open menu closed.
    pub(crate) fn close_menus(&mut self) -> Outcome {
        self.close_all_but(0)
    }

    /// An entry of its open menu clicked: `Event(entry, "clicked")`, and the
    /// menu closed. An entry that opens a submenu is opened with
    /// [`Self::open_submenu`] instead, and a disabled one does nothing.
    /// `clicking_an_entry_tells_the_item_and_closes_the_menu`.
    pub(crate) fn click(&mut self, id: u32, entry: i32) -> Result<Outcome, &'static str> {
        let item = self.item(id)?;
        let entries = item.menu.as_deref().ok_or("menu-closed")?;
        let found = find(entries, entry).ok_or("unknown-entry")?;
        if found.separator || !found.enabled || !found.visible {
            return Err("disabled");
        }
        if found.submenu {
            return Err("submenu");
        }
        let mut outcome = Outcome {
            calls: vec![(
                item.key.clone(),
                Call::MenuEvent {
                    entry,
                    event: "clicked",
                },
            )],
            changes: Vec::new(),
        };
        outcome.extend(self.close_menu(id)?);
        Ok(outcome)
    }

    /// A submenu of its open menu shown: `AboutToShow` and `opened` for it,
    /// and the menu read again, as many items fill a submenu only then.
    pub(crate) fn open_submenu(&self, id: u32, entry: i32) -> Result<Outcome, &'static str> {
        let item = self.item(id)?;
        let entries = item.menu.as_deref().ok_or("menu-closed")?;
        let found = find(entries, entry).ok_or("unknown-entry")?;
        if !found.submenu {
            return Err("not-a-submenu");
        }
        Ok(Outcome {
            calls: vec![
                (
                    item.key.clone(),
                    Call::MenuEvent {
                        entry,
                        event: "opened",
                    },
                ),
                (
                    item.key.clone(),
                    Call::LoadMenu {
                        about_to_show: Some(entry),
                    },
                ),
            ],
            changes: Vec::new(),
        })
    }

    /// The item said its menu changed (`LayoutUpdated`,
    /// `ItemsPropertiesUpdated`): read again if it is open.
    pub(crate) fn menu_changed(&self, key: &str) -> Option<(String, Call)> {
        let item = self.items.get(self.ids.get(key)?)?;
        item.menu_open.then(|| {
            (
                item.key.clone(),
                Call::LoadMenu {
                    about_to_show: None,
                },
            )
        })
    }
}

impl Outcome {
    pub(crate) fn extend(&mut self, other: Self) {
        self.calls.extend(other.calls);
        self.changes.extend(other.changes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(title: &str, menu: &str) -> Props {
        Props {
            item_id: "steam".to_owned(),
            title: title.to_owned(),
            names: ["steam".to_owned(), String::new(), String::new()],
            menu: menu.to_owned(),
            ..Props::default()
        }
    }

    fn menu() -> Vec<Entry> {
        vec![
            Entry {
                id: 1,
                label: "Library".to_owned(),
                ..Entry::default()
            },
            Entry {
                id: 2,
                separator: true,
                ..Entry::default()
            },
            Entry {
                id: 3,
                label: "Friends".to_owned(),
                submenu: true,
                children: vec![Entry {
                    id: 4,
                    label: "Online".to_owned(),
                    enabled: false,
                    ..Entry::default()
                }],
                ..Entry::default()
            },
            Entry {
                id: 5,
                label: "Exit Steam".to_owned(),
                ..Entry::default()
            },
        ]
    }

    #[test]
    fn an_item_is_added_at_its_first_read_and_changed_at_the_next() {
        let mut tray = Tray::default();
        let first = tray.read(":1.7/StatusNotifierItem", props("Steam", ""));
        assert_eq!(first.change, Some(Change::Added(first.id)));
        let again = tray.read(":1.7/StatusNotifierItem", props("Steam", ""));
        assert_eq!(again.id, first.id);
        assert_eq!(again.change, None, "nothing changed");
        let renamed = tray.read(":1.7/StatusNotifierItem", props("Steam - 1 friend", ""));
        assert_eq!(renamed.change, Some(Change::Changed(first.id)));
        let other = tray.read(
            ":1.9/org/ayatana/NotificationItem/discord",
            props("Discord", ""),
        );
        assert_ne!(other.id, first.id);
        let order: Vec<u32> = tray.items().map(|item| item.id).collect();
        assert_eq!(order, [first.id, other.id]);
    }

    #[test]
    fn a_new_picture_is_a_new_revision_and_the_same_one_is_not() {
        let mut tray = Tray::default();
        let key = ":1.7/StatusNotifierItem";
        let mut with = props("Steam", "");
        with.pictures[Picture::Icon as usize] = 11;
        let read = tray.read(key, with.clone());
        assert_eq!(read.new_pictures, [Picture::Icon]);
        assert_eq!(tray.get(read.id).map(|item| item.revisions[0]), Some(1));
        assert!(tray.read(key, with.clone()).new_pictures.is_empty());
        with.pictures[Picture::Icon as usize] = 12;
        assert_eq!(tray.read(key, with.clone()).new_pictures, [Picture::Icon]);
        assert_eq!(tray.get(read.id).map(|item| item.revisions[0]), Some(2));
        with.pictures[Picture::Icon as usize] = 0;
        assert_eq!(tray.read(key, with).dropped_pictures, [Picture::Icon]);
    }

    #[test]
    fn an_item_that_leaves_is_removed_and_its_id_not_given_out_again() {
        let mut tray = Tray::default();
        let key = ":1.7/StatusNotifierItem";
        let first = tray.read(key, props("Steam", "")).id;
        assert_eq!(tray.gone(key), Some(first));
        assert_eq!(tray.gone(key), None, "gone once");
        assert!(tray.get(first).is_none());
        let back = tray.read(key, props("Steam", "")).id;
        assert_ne!(back, first, "a new item, with a new id");
        assert_eq!(tray.clear(), [back]);
        assert_eq!(tray.items().count(), 0);
    }

    #[test]
    fn a_primary_click_on_an_item_that_is_a_menu_opens_the_menu() {
        let mut tray = Tray::default();
        let key = ":1.9/org/ayatana/NotificationItem/discord";
        let mut is_menu = props("Discord", "/MenuBar");
        is_menu.item_is_menu = true;
        let id = tray.read(key, is_menu).id;
        let outcome = tray.activate(id, 10, 20).expect("done");
        assert!(
            outcome
                .calls
                .iter()
                .all(|(_, call)| !matches!(call, Call::Activate { .. }))
        );
        assert!(tray.get(id).is_some_and(|item| item.menu_open));

        let plain = tray
            .read(":1.7/StatusNotifierItem", props("Steam", "/MenuBar"))
            .id;
        let outcome = tray.activate(plain, 10, 20).expect("done");
        assert_eq!(
            outcome.calls,
            [(
                ":1.7/StatusNotifierItem".to_owned(),
                Call::Activate {
                    x: 10,
                    y: 20,
                    token: None
                }
            )]
        );
        assert_eq!(tray.activate(999, 0, 0), Err("unknown-item"));
    }

    #[test]
    fn a_refused_activate_opens_the_menu_or_asks_for_the_item_s_own() {
        let mut tray = Tray::default();
        let with_menu = ":1.7/StatusNotifierItem";
        let id = tray.read(with_menu, props("Steam", "/MenuBar")).id;
        tray.activate_refused(with_menu, 1, 2);
        assert!(tray.get(id).is_some_and(|item| item.menu_open));
        let without = ":1.8/StatusNotifierItem";
        tray.read(without, props("Tool", ""));
        let outcome = tray.activate_refused(without, 1, 2);
        assert_eq!(
            outcome.calls,
            [(without.to_owned(), Call::ContextMenu { x: 1, y: 2 })]
        );
    }

    #[test]
    fn opening_a_menu_reads_it_and_closes_any_other() {
        let mut tray = Tray::default();
        let steam = tray
            .read(":1.7/StatusNotifierItem", props("Steam", "/MenuBar"))
            .id;
        let discord = tray
            .read(":1.9/StatusNotifierItem", props("Discord", "/MenuBar"))
            .id;
        let opened = tray.open_menu(steam, 0, 0).expect("opened");
        assert_eq!(
            opened.calls,
            [
                (
                    ":1.7/StatusNotifierItem".to_owned(),
                    Call::MenuEvent {
                        entry: 0,
                        event: "opened"
                    }
                ),
                (
                    ":1.7/StatusNotifierItem".to_owned(),
                    Call::LoadMenu {
                        about_to_show: Some(0)
                    }
                ),
            ]
        );
        // Nothing is kept until it is read, and a menu read for a closed
        // item is not kept at all.
        assert_eq!(tray.get(steam).and_then(|item| item.menu.clone()), None);
        assert_eq!(tray.menu_read(":1.9/StatusNotifierItem", menu()), None);
        assert_eq!(
            tray.menu_read(":1.7/StatusNotifierItem", menu()),
            Some(Change::Menu(steam))
        );
        assert_eq!(tray.menu_read(":1.7/StatusNotifierItem", menu()), None);

        let other = tray.open_menu(discord, 0, 0).expect("opened");
        assert!(other.changes.contains(&Change::Menu(steam)));
        assert!(other.calls.contains(&(
            ":1.7/StatusNotifierItem".to_owned(),
            Call::MenuEvent {
                entry: 0,
                event: "closed"
            }
        )));
        assert!(tray.get(steam).is_some_and(|item| !item.menu_open));
        assert!(tray.get(discord).is_some_and(|item| item.menu_open));
        assert!(
            tray.menu_changed(":1.9/StatusNotifierItem").is_some(),
            "an open menu is read again when it changes"
        );
        assert_eq!(tray.menu_changed(":1.7/StatusNotifierItem"), None);

        // An item with no menu of its own is asked for its own.
        let tool = tray.read(":1.8/StatusNotifierItem", props("Tool", "")).id;
        assert_eq!(
            tray.open_menu(tool, 3, 4).expect("asked").calls,
            [(
                ":1.8/StatusNotifierItem".to_owned(),
                Call::ContextMenu { x: 3, y: 4 }
            )]
        );
    }

    #[test]
    fn clicking_an_entry_tells_the_item_and_closes_the_menu() {
        let mut tray = Tray::default();
        let key = ":1.7/StatusNotifierItem";
        let id = tray.read(key, props("Steam", "/MenuBar")).id;
        assert_eq!(tray.click(id, 5), Err("menu-closed"));
        tray.open_menu(id, 0, 0).expect("opened");
        tray.menu_read(key, menu());
        assert_eq!(tray.click(id, 2), Err("disabled"), "a separator");
        assert_eq!(tray.click(id, 4), Err("disabled"));
        assert_eq!(tray.click(id, 3), Err("submenu"));
        assert_eq!(tray.click(id, 77), Err("unknown-entry"));
        let submenu = tray.open_submenu(id, 3).expect("opened");
        assert_eq!(
            submenu.calls[1].1,
            Call::LoadMenu {
                about_to_show: Some(3)
            }
        );
        let clicked = tray.click(id, 5).expect("clicked");
        assert_eq!(
            clicked.calls,
            [
                (
                    key.to_owned(),
                    Call::MenuEvent {
                        entry: 5,
                        event: "clicked"
                    }
                ),
                (
                    key.to_owned(),
                    Call::MenuEvent {
                        entry: 0,
                        event: "closed"
                    }
                ),
            ]
        );
        assert!(tray.get(id).is_some_and(|item| !item.menu_open));
    }

    #[test]
    fn a_menu_that_cannot_be_read_does_not_stay_open_and_empty() {
        let mut tray = Tray::default();
        let key = ":1.7/StatusNotifierItem";
        let id = tray.read(key, props("Steam", "/MenuBar")).id;
        tray.open_menu(id, 0, 0).expect("opened");
        assert_eq!(tray.menu_failed(key), Some(Change::Menu(id)));
        assert!(tray.get(id).is_some_and(|item| !item.menu_open));
    }

    #[test]
    fn a_menu_that_moves_to_another_object_is_closed() {
        let mut tray = Tray::default();
        let key = ":1.7/StatusNotifierItem";
        let id = tray.read(key, props("Steam", "/MenuBar")).id;
        tray.open_menu(id, 0, 0).expect("opened");
        tray.menu_read(key, menu());
        tray.read(key, props("Steam", "/OtherMenu"));
        let item = tray.get(id).expect("still there");
        assert!(!item.menu_open);
        assert_eq!(item.menu, None);
    }
}
