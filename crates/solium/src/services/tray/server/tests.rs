use super::*;

#[test]
fn an_item_registers_by_its_name_or_by_a_path_on_its_own_connection() {
    let mut registry = Registry {
        serving: true,
        ..Registry::default()
    };
    let by_path = registry
        .register(":1.7", "/org/ayatana/NotificationItem/discord")
        .expect("registered")
        .expect("new");
    assert_eq!(by_path.key, ":1.7/org/ayatana/NotificationItem/discord");
    assert_eq!(by_path.service, ":1.7");
    assert_eq!(by_path.owner, ":1.7", "its own connection is its owner");
    let by_name = registry
        .register(":1.9", "org.kde.StatusNotifierItem-42-1")
        .expect("registered")
        .expect("new");
    assert_eq!(
        by_name.key,
        "org.kde.StatusNotifierItem-42-1/StatusNotifierItem"
    );
    assert_eq!(by_name.path, "/StatusNotifierItem");
    assert_eq!(by_name.owner, "", "found out by the reader");
    assert_eq!(
        registry.register(":1.9", "org.kde.StatusNotifierItem-42-1"),
        Ok(None),
        "registered once"
    );
    assert_eq!(
        registry.register(":1.9", "not a name"),
        Err(Refusal::Invalid)
    );
    assert_eq!(
        registry.register(":1.9", "/bad//path"),
        Err(Refusal::Invalid)
    );
    assert_eq!(registry.keys(), [by_path.key.clone(), by_name.key.clone()]);
    for n in 0..MAX_ITEMS {
        let _ = registry.register(":1.20", &format!("/item/n{n}"));
    }
    assert_eq!(registry.register(":1.21", "/one/more"), Err(Refusal::Full));
    let mut idle = Registry::default();
    assert_eq!(idle.register(":1.7", "/item"), Err(Refusal::NotServing));
}

#[test]
fn an_item_is_gone_when_its_name_leaves_the_bus() {
    let mut registry = Registry {
        serving: true,
        ..Registry::default()
    };
    let _ = registry.register(":1.7", "/StatusNotifierItem");
    let _ = registry.register(":1.9", "org.kde.StatusNotifierItem-9-1");
    registry.set_owner("org.kde.StatusNotifierItem-9-1/StatusNotifierItem", ":1.9");
    assert_eq!(registry.left(":1.8"), Vec::<String>::new());
    assert_eq!(registry.left(":1.7"), [":1.7/StatusNotifierItem"]);
    // Gone by its owner leaving as much as by its name.
    assert_eq!(
        registry.left(":1.9"),
        ["org.kde.StatusNotifierItem-9-1/StatusNotifierItem"]
    );
    assert!(registry.keys().is_empty());
}

#[test]
fn a_signal_is_matched_to_its_item_by_sender_and_path() {
    let mut registry = Registry {
        serving: true,
        ..Registry::default()
    };
    let _ = registry.register(":1.7", "/org/ayatana/NotificationItem/steam");
    let key = ":1.7/org/ayatana/NotificationItem/steam";
    registry.set_menu(key, "/org/ayatana/NotificationItem/steam/Menu");
    assert_eq!(
        registry.item_at(":1.7", "/org/ayatana/NotificationItem/steam"),
        Some(key.to_owned())
    );
    assert_eq!(
        registry.item_at(":1.8", "/org/ayatana/NotificationItem/steam"),
        None
    );
    assert_eq!(
        registry.menu_at(":1.7", "/org/ayatana/NotificationItem/steam/Menu"),
        Some(key.to_owned())
    );
    // Ten signals before the reader gets to it: one read, of all they named.
    assert!(registry.want(key, groups::TITLE));
    assert!(!registry.want(key, groups::ICON));
    assert_eq!(registry.take(key), groups::TITLE | groups::ICON);
    assert_eq!(registry.take(key), 0);
}

#[test]
fn a_change_signal_names_only_what_it_changed() {
    assert_eq!(groups::of_signal("NewTitle"), groups::TITLE);
    assert_eq!(groups::of_signal("NewIcon"), groups::ICON);
    assert_eq!(
        groups::of_signal("NewIconThemePath"),
        groups::ICON | groups::ATTENTION | groups::OVERLAY
    );
    assert_eq!(groups::of_signal("SomethingElse"), 0);
}

#[test]
fn the_watcher_s_properties_list_its_items_and_its_host() {
    let mut registry = Registry {
        serving: true,
        ..Registry::default()
    };
    let _ = registry.register(":1.7", "/StatusNotifierItem");
    assert_eq!(
        property(&registry, "RegisteredStatusNotifierItems"),
        Some(Value::from(vec![":1.7/StatusNotifierItem".to_owned()]))
    );
    assert_eq!(
        property(&registry, "IsStatusNotifierHostRegistered"),
        Some(Value::from(true))
    );
    assert_eq!(
        property(&registry, "ProtocolVersion"),
        Some(Value::from(0i32))
    );
    assert_eq!(property(&registry, "Nonsense"), None);
}

/// One menu entry, as `GetLayout` sends a child: `(ia{sv}av)`.
fn entry_value(
    id: i32,
    properties: &[(&str, Value<'static>)],
    children: Vec<Value<'static>>,
) -> OwnedValue {
    let mut dict = zbus::zvariant::Dict::new(
        &zbus::zvariant::Signature::Str,
        &zbus::zvariant::Signature::Variant,
    );
    for (key, value) in properties {
        dict.append(Value::from(*key), Value::Value(Box::new(value.clone())))
            .expect("an entry");
    }
    let children = zbus::zvariant::Array::from(children);
    let structure = zbus::zvariant::StructureBuilder::new()
        .add_field(id)
        .append_field(Value::Dict(dict))
        .append_field(Value::Array(children))
        .build()
        .expect("a structure");
    OwnedValue::try_from(Value::Structure(structure)).expect("owned")
}

fn as_child(value: OwnedValue) -> Value<'static> {
    Value::Value(Box::new(Value::from(value)))
}

/// Steam's menu, near enough: an entry, a separator, a submenu with a
/// disabled entry and a check, and one entry nobody sees.
fn steam_layout() -> Layout {
    let online = entry_value(
        41,
        &[
            ("label", Value::from("_Online")),
            ("enabled", Value::from(false)),
        ],
        Vec::new(),
    );
    let away = entry_value(
        42,
        &[
            ("label", Value::from("Away")),
            ("toggle-type", Value::from("radio")),
            ("toggle-state", Value::from(1i32)),
        ],
        Vec::new(),
    );
    (
        3,
        (
            0,
            HashMap::new(),
            vec![
                entry_value(
                    1,
                    &[
                        ("label", Value::from("_Library")),
                        ("icon-name", Value::from("steam")),
                    ],
                    Vec::new(),
                ),
                entry_value(2, &[("type", Value::from("separator"))], Vec::new()),
                entry_value(
                    4,
                    &[
                        ("label", Value::from("Friends")),
                        ("children-display", Value::from("submenu")),
                    ],
                    vec![as_child(online), as_child(away)],
                ),
                entry_value(5, &[("visible", Value::from(false))], Vec::new()),
            ],
        ),
    )
}

#[test]
fn a_menu_layout_becomes_entries_with_their_submenus_and_limits() {
    let steam = entries(&steam_layout());
    assert_eq!(steam.len(), 4);
    assert_eq!(steam[0].label, "Library");
    assert_eq!(steam[0].icon_name, "steam");
    assert!(steam[1].separator);
    assert!(steam[2].submenu);
    assert_eq!(steam[2].children[0].label, "Online");
    assert!(!steam[2].children[0].enabled);
    assert_eq!(steam[2].children[1].toggle, Toggle::Radio);
    assert_eq!(steam[2].children[1].toggle_state, 1);
    assert!(!steam[3].visible);

    // A menu nested past the depth limit is cut there.
    let mut deep = entry_value(100, &[("label", Value::from("deepest"))], Vec::new());
    for id in (0..20).rev() {
        deep = entry_value(id, &[], vec![as_child(deep)]);
    }
    let mut depth = 0;
    let layout: Layout = (1, (0, HashMap::new(), vec![deep]));
    let mut level = entries(&layout);
    while let Some(first) = level.into_iter().next() {
        depth += 1;
        level = first.children;
    }
    assert_eq!(depth, MAX_DEPTH);

    // And a menu with more entries than kept keeps the first.
    let many: Vec<OwnedValue> = (0..MAX_ENTRIES as i32 + 50)
        .map(|id| entry_value(id, &[], Vec::new()))
        .collect();
    assert_eq!(entries(&(1, (0, HashMap::new(), many))).len(), MAX_ENTRIES);
}

mod bus {
    use std::time::{Duration, Instant};

    use zbus::message::Message;

    use super::super::*;

    const WITHIN: Duration = Duration::from_secs(5);
    const ITEM_OWNER: &str = ":1.7";

    /// The bus driver, as far as the watcher asks it anything.
    struct Driver {
        /// Another program already owns the watcher's name.
        taken: bool,
    }

    #[zbus::interface(name = "org.freedesktop.DBus")]
    impl Driver {
        fn hello(&self) -> String {
            ":1.1".to_owned()
        }
        fn add_match(&self, _rule: String) {}
        fn remove_match(&self, _rule: String) {}
        fn request_name(&self, _name: String, _flags: u32) -> u32 {
            // DBUS_REQUEST_NAME_REPLY_IN_QUEUE, or _PRIMARY_OWNER.
            if self.taken { 2 } else { 1 }
        }
        fn release_name(&self, _name: String) -> u32 {
            1
        }
        fn get_name_owner(&self, name: String) -> String {
            if name == WATCHER {
                ":1.99".to_owned()
            } else {
                ITEM_OWNER.to_owned()
            }
        }
        #[zbus(name = "GetConnectionUnixProcessID")]
        fn get_connection_unix_process_id(&self, name: String) -> u32 {
            if name == ":1.99" { 99 } else { 4242 }
        }
    }

    /// What the stand-in application heard.
    type Heard = Arc<Mutex<Vec<String>>>;

    /// `ToolTip`, `(sa(iiay)ss)`: an icon name, pictures, a title and a body.
    type ToolTip = (String, Vec<(i32, i32, Vec<u8>)>, String, String);

    /// An application's tray item, as Steam's would be.
    struct FakeItem {
        title: Arc<Mutex<String>>,
        heard: Heard,
        /// Refuses `Activate`, as an item that is only a menu does.
        refuses: bool,
    }

    #[zbus::interface(name = "org.kde.StatusNotifierItem")]
    impl FakeItem {
        #[zbus(property)]
        fn id(&self) -> String {
            "steam".to_owned()
        }
        #[zbus(property)]
        fn category(&self) -> String {
            "ApplicationStatus".to_owned()
        }
        #[zbus(property)]
        fn title(&self) -> String {
            lock(&self.title).clone()
        }
        #[zbus(property)]
        fn status(&self) -> String {
            "Active".to_owned()
        }
        #[zbus(property)]
        fn icon_name(&self) -> String {
            "steam".to_owned()
        }
        #[zbus(property)]
        fn icon_pixmap(&self) -> Vec<(i32, i32, Vec<u8>)> {
            // 2x2, opaque blue, ARGB in network order.
            vec![(2, 2, [255, 0, 0, 255].repeat(4))]
        }
        #[zbus(property)]
        fn tool_tip(&self) -> ToolTip {
            (
                String::new(),
                Vec::new(),
                "Steam".to_owned(),
                "<b>2</b> friends online".to_owned(),
            )
        }
        #[zbus(property)]
        fn item_is_menu(&self) -> bool {
            false
        }
        #[zbus(property)]
        fn menu(&self) -> zbus::zvariant::OwnedObjectPath {
            zbus::zvariant::OwnedObjectPath::try_from("/MenuBar").expect("a path")
        }
        fn activate(&self, x: i32, y: i32) -> zbus::fdo::Result<()> {
            if self.refuses {
                return Err(zbus::fdo::Error::NotSupported("only a menu".to_owned()));
            }
            lock(&self.heard).push(format!("Activate {x} {y}"));
            Ok(())
        }
        fn provide_xdg_activation_token(&self, token: String) {
            lock(&self.heard).push(format!("Token {token}"));
        }
        fn context_menu(&self, x: i32, y: i32) {
            lock(&self.heard).push(format!("ContextMenu {x} {y}"));
        }
        fn scroll(&self, delta: i32, orientation: String) {
            lock(&self.heard).push(format!("Scroll {delta} {orientation}"));
        }
    }

    /// Its menu.
    struct FakeMenu {
        heard: Heard,
    }

    #[zbus::interface(name = "com.canonical.dbusmenu")]
    impl FakeMenu {
        fn get_layout(&self, _parent: i32, _depth: i32, _properties: Vec<String>) -> Layout {
            super::steam_layout()
        }
        fn about_to_show(&self, id: i32) -> bool {
            lock(&self.heard).push(format!("AboutToShow {id}"));
            false
        }
        fn event(&self, id: i32, event: String, _data: OwnedValue, _timestamp: u32) {
            lock(&self.heard).push(format!("Event {id} {event}"));
        }
    }

    /// A stand-in session bus at a socket of the test's own: the driver, an
    /// application's item and its menu all behind it, and the application's
    /// calls and signals sent down Solium's one connection as the bus would
    /// deliver them. No bus, real or private, anywhere near.
    struct StandIn {
        directory: PathBuf,
        address: String,
        peers: mpsc::Receiver<zbus::blocking::Connection>,
        peer: Option<zbus::blocking::Connection>,
        title: Arc<Mutex<String>>,
        heard: Heard,
    }

    use std::path::PathBuf;

    impl StandIn {
        fn new(name: &str, taken: bool, refuses: bool) -> Self {
            let directory =
                std::env::temp_dir().join(format!("solium-tray-bus-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&directory);
            std::fs::create_dir_all(&directory).expect("a directory");
            let path = directory.join("bus");
            let listener =
                std::os::unix::net::UnixListener::bind(&path).expect("binding the socket");
            let (found, peers) = mpsc::channel();
            let title = Arc::new(Mutex::new("Steam".to_owned()));
            let heard: Heard = Arc::default();
            std::thread::spawn({
                let title = title.clone();
                let heard = heard.clone();
                move || {
                    let Ok((stream, _)) = listener.accept() else {
                        return;
                    };
                    let Ok(connection) =
                        zbus::blocking::connection::Builder::async_io_unix_stream(stream)
                            .server(zbus::Guid::generate())
                            .expect("a server guid")
                            .p2p()
                            .serve_at(DBUS_PATH, Driver { taken })
                            .expect("serving the driver")
                            .serve_at(
                                ITEM_PATH,
                                FakeItem {
                                    title,
                                    heard: heard.clone(),
                                    refuses,
                                },
                            )
                            .expect("serving the item")
                            .serve_at("/MenuBar", FakeMenu { heard })
                            .expect("serving the menu")
                            .build()
                    else {
                        return;
                    };
                    let _ = found.send(connection.clone());
                    connection.closed();
                }
            });
            Self {
                address: format!("unix:path={}", path.display()),
                directory,
                peers,
                peer: None,
                title,
                heard,
            }
        }

        fn peer(&mut self) -> zbus::blocking::Connection {
            if self.peer.is_none() {
                self.peer = self.peers.recv_timeout(WITHIN).ok();
            }
            self.peer.clone().expect("Solium connected")
        }

        /// A call from `from` to the watcher, and its answer.
        fn call<B>(
            &mut self,
            from: &str,
            interface: &str,
            member: &str,
            body: &B,
        ) -> zbus::Result<Message>
        where
            B: zbus::export::serde::Serialize + zbus::zvariant::DynamicType,
        {
            let peer = self.peer();
            let call = Message::method_call(WATCHER_PATH, member)?
                .sender(from)?
                .destination(WATCHER)?
                .interface(interface)?
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

        fn register(&mut self, service_or_path: &str) -> zbus::Result<Message> {
            self.call(
                ITEM_OWNER,
                WATCHER,
                "RegisterStatusNotifierItem",
                &service_or_path,
            )
        }

        /// A signal, as the bus would deliver it from `from`.
        fn signal<B>(&mut self, from: &str, path: &str, interface: &str, member: &str, body: &B)
        where
            B: zbus::export::serde::Serialize + zbus::zvariant::DynamicType,
        {
            let signal = Message::signal(path, interface, member)
                .and_then(|b| b.sender(from))
                .and_then(|b| b.build(body))
                .expect("a signal");
            self.peer().send(&signal).expect("sent");
        }

        /// What the application heard, once it has heard `count` things.
        fn heard(&self, count: usize) -> Vec<String> {
            let deadline = Instant::now() + WITHIN;
            while Instant::now() < deadline {
                let heard = lock(&self.heard).clone();
                if heard.len() >= count {
                    return heard;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            lock(&self.heard).clone()
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

    /// The server, once it has asked for the name and heard the answer: a
    /// real bus sends a connection nothing before its `Hello` is answered,
    /// so neither does a test. Whether it got the name.
    fn started(bus: &StandIn) -> (Server, bool) {
        let server = Server::start(Some(bus.address.clone())).expect("a server");
        let Some(Fact::Serving(serving)) =
            next_fact(&server, |fact| matches!(fact, Fact::Serving(_)))
        else {
            panic!("the server never heard whether it has the name");
        };
        (server, serving)
    }

    fn read(server: &Server) -> ReadItem {
        let Some(Fact::Read(read)) = next_fact(server, |fact| matches!(fact, Fact::Read(_))) else {
            panic!("the item was read");
        };
        *read
    }

    #[test]
    fn an_item_registers_answers_and_is_read() {
        let mut bus = StandIn::new("register", false, false);
        let (server, serving) = started(&bus);
        assert!(serving);
        bus.register(ITEM_PATH).expect("registered");
        let read = read(&server);
        assert_eq!(read.key, ":1.7/StatusNotifierItem");
        assert_eq!(read.props.item_id, "steam");
        assert_eq!(read.props.title, "Steam");
        assert_eq!(read.props.status, Status::Active);
        assert_eq!(read.props.names[0], "steam");
        assert_eq!(read.props.menu, "/MenuBar");
        assert_eq!(read.props.tooltip_title, "Steam");
        assert_eq!(read.props.tooltip_body, "2 friends online");
        let picture = read.pictures[0].clone().expect("its pixels");
        assert_eq!((picture.width, picture.height), (2, 2));
        assert_eq!(&picture.rgba[..4], &[0, 0, 255, 255], "blue, as RGBA");
        assert_ne!(read.props.pictures[0], 0);
        // The watcher lists it now.
        let listed: OwnedValue = bus
            .call(
                ":1.8",
                PROPERTIES,
                "Get",
                &(WATCHER, "RegisteredStatusNotifierItems"),
            )
            .expect("answered")
            .body()
            .deserialize()
            .expect("a value");
        assert_eq!(
            Vec::<String>::try_from(listed).expect("a list"),
            [":1.7/StatusNotifierItem"]
        );
    }

    #[test]
    fn an_item_registered_by_name_is_read_from_its_owner_and_leaves_with_it() {
        let mut bus = StandIn::new("by-name", false, false);
        let (server, _) = started(&bus);
        bus.register("org.kde.StatusNotifierItem-7-1")
            .expect("registered");
        let first = read(&server);
        assert_eq!(
            first.key,
            "org.kde.StatusNotifierItem-7-1/StatusNotifierItem"
        );
        // Its title changes, and it says so, from its unique name.
        *lock(&bus.title) = "Steam - 3 friends".to_owned();
        bus.signal(ITEM_OWNER, ITEM_PATH, ITEM, "NewTitle", &());
        let again = read(&server);
        assert_eq!(again.props.title, "Steam - 3 friends");
        assert_eq!(again.props.item_id, "steam", "kept from the first read");
        assert!(again.pictures[0].is_none(), "its icon was not read again");
        // Its application leaves the bus.
        bus.signal(
            DBUS,
            DBUS_PATH,
            DBUS,
            "NameOwnerChanged",
            &("org.kde.StatusNotifierItem-7-1", ITEM_OWNER, ""),
        );
        let Some(Fact::Gone { key }) = next_fact(&server, |fact| matches!(fact, Fact::Gone { .. }))
        else {
            panic!("the item left");
        };
        assert_eq!(key, "org.kde.StatusNotifierItem-7-1/StatusNotifierItem");
    }

    #[test]
    fn a_click_activates_the_item_and_its_menu_is_read_and_clicked() {
        let mut bus = StandIn::new("click", false, false);
        let (server, _) = started(&bus);
        bus.register(ITEM_PATH).expect("registered");
        let key = read(&server).key;
        server.call(
            &key,
            Call::Activate {
                x: 10,
                y: 20,
                token: Some("token-1".to_owned()),
            },
        );
        server.call(
            &key,
            Call::Scroll {
                delta: 120,
                horizontal: false,
            },
        );
        assert_eq!(
            bus.heard(3),
            ["Token token-1", "Activate 10 20", "Scroll 120 vertical"]
        );
        server.call(
            &key,
            Call::LoadMenu {
                about_to_show: Some(0),
            },
        );
        let Some(Fact::Menu { entries, .. }) =
            next_fact(&server, |fact| matches!(fact, Fact::Menu { .. }))
        else {
            panic!("the menu was read");
        };
        assert_eq!(entries[0].label, "Library");
        server.call(
            &key,
            Call::MenuEvent {
                entry: 1,
                event: "clicked",
            },
        );
        assert_eq!(&bus.heard(5)[3..], ["AboutToShow 0", "Event 1 clicked"]);
    }

    #[test]
    fn a_refused_activate_is_told_to_the_loop() {
        let mut bus = StandIn::new("refused", false, true);
        let (server, _) = started(&bus);
        bus.register(ITEM_PATH).expect("registered");
        let key = read(&server).key;
        server.call(
            &key,
            Call::Activate {
                x: 1,
                y: 2,
                token: None,
            },
        );
        let Some(Fact::ActivateRefused { x, y, .. }) =
            next_fact(&server, |fact| matches!(fact, Fact::ActivateRefused { .. }))
        else {
            panic!("the refusal reached the loop");
        };
        assert_eq!((x, y), (1, 2));
    }

    #[test]
    fn another_watcher_keeps_the_name_and_is_named() {
        let mut bus = StandIn::new("taken", true, false);
        let (server, serving) = started(&bus);
        assert!(!serving);
        let Some(Fact::Owner(Some(owner))) =
            next_fact(&server, |fact| matches!(fact, Fact::Owner(_)))
        else {
            panic!("the owner was named");
        };
        assert_eq!(owner.name, ":1.99");
        assert_eq!(owner.pid, Some(99));
        // An item that reaches Solium anyway is turned away.
        assert!(bus.register(ITEM_PATH).is_err());
    }
}
