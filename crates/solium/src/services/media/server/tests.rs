use super::*;

fn owned(value: Value<'_>) -> OwnedValue {
    OwnedValue::try_from(value).expect("an owned value")
}

/// Metadata as a player sends it, `a{sv}`.
fn metadata(title: &str, art: &str) -> HashMap<String, OwnedValue> {
    HashMap::from([
        ("xesam:title".to_owned(), owned(Value::from(title))),
        (
            "xesam:artist".to_owned(),
            owned(Value::from(vec![
                "A".to_owned(),
                String::new(),
                "B".to_owned(),
            ])),
        ),
        ("xesam:album".to_owned(), owned(Value::from("Album"))),
        ("mpris:artUrl".to_owned(), owned(Value::from(art))),
        (
            "mpris:length".to_owned(),
            owned(Value::from(180_000_000i64)),
        ),
        (
            "mpris:trackid".to_owned(),
            owned(Value::from(
                zbus::zvariant::ObjectPath::try_from("/org/fake/track/1").expect("a path"),
            )),
        ),
    ])
}

#[test]
fn a_player_name_is_followed_up_to_the_cap_and_moves_with_its_owner() {
    let mut registry = Registry::default();
    assert_eq!(
        registry.add("org.mpris.MediaPlayer2.mpv", ":1.7"),
        Added::New
    );
    assert_eq!(
        registry.add("org.mpris.MediaPlayer2.mpv", ":1.7"),
        Added::Already
    );
    assert_eq!(
        registry.add("org.mpris.MediaPlayer2.mpv", ":1.9"),
        Added::Moved,
        "another connection has the name now"
    );
    assert_eq!(registry.keys_of(":1.9"), ["org.mpris.MediaPlayer2.mpv"]);
    assert!(registry.keys_of(":1.7").is_empty());
    assert_eq!(registry.add("org.example.Other", ":1.3"), Added::Refused);
    assert_eq!(
        registry.add("org.mpris.MediaPlayer2.", ":1.3"),
        Added::Refused
    );
    for n in 1..MAX_PLAYERS {
        let _ = registry.add(&format!("org.mpris.MediaPlayer2.p{n}"), ":1.20");
    }
    assert_eq!(
        registry.add("org.mpris.MediaPlayer2.one-more", ":1.21"),
        Added::Refused
    );
    assert!(registry.remove("org.mpris.MediaPlayer2.mpv"));
    assert!(!registry.remove("org.mpris.MediaPlayer2.mpv"));
}

#[test]
fn changes_wait_merged_and_a_player_is_queued_once() {
    let mut registry = Registry::default();
    let key = "org.mpris.MediaPlayer2.mpv";
    registry.add(key, ":1.7");
    let status =
        |name: &str| HashMap::from([("PlaybackStatus".to_owned(), owned(Value::from(name)))]);
    assert!(registry.changed(key, PLAYER, status("Playing"), Vec::new()));
    assert!(!registry.changed(key, PLAYER, status("Paused"), vec!["Volume".to_owned()]));
    assert!(!registry.seeked(key, 5, Instant::now()));
    let pending = registry.take(key);
    assert_eq!(pending.values.len(), 2);
    let newest = pending
        .values
        .get(&(PLAYER, "PlaybackStatus"))
        .and_then(|value| value.as_ref())
        .map(|value| as_str(unwrap(value)).to_owned());
    assert_eq!(newest.as_deref(), Some("Paused"), "the newest of each");
    assert!(
        pending
            .values
            .get(&(PLAYER, "Volume"))
            .is_some_and(Option::is_none)
    );
    assert_eq!(pending.seeked.map(|(micros, _)| micros), Some(5));
    // What this reader does not follow waits for nothing.
    let unknown = HashMap::from([("Fullscreen".to_owned(), owned(Value::from(true)))]);
    assert!(!registry.changed(key, ROOT, unknown, Vec::new()));
    assert!(!registry.changed(
        key,
        "org.example.Other",
        HashMap::new(),
        vec!["X".to_owned()]
    ));
    assert!(registry.take(key).values.is_empty());
}

#[test]
fn a_player_s_properties_are_read_into_props_and_its_metadata_made_safe() {
    let mut kept = Kept::default();
    let mut effects = Effects::default();
    merge(
        &mut kept,
        ROOT,
        "Identity",
        &Value::from("Fake\nPlayer"),
        &mut effects,
    );
    merge(
        &mut kept,
        ROOT,
        "DesktopEntry",
        &Value::from("org.fake.Player.desktop"),
        &mut effects,
    );
    merge(
        &mut kept,
        PLAYER,
        "PlaybackStatus",
        &Value::from("Playing"),
        &mut effects,
    );
    merge(
        &mut kept,
        PLAYER,
        "Volume",
        &Value::from(1.7f64),
        &mut effects,
    );
    merge(
        &mut kept,
        PLAYER,
        "LoopStatus",
        &Value::from("Playlist"),
        &mut effects,
    );
    merge(
        &mut kept,
        PLAYER,
        "Position",
        &Value::from(5u64),
        &mut effects,
    );
    let sent = Value::from(zbus::zvariant::Dict::from(metadata(
        "Song",
        "file:///a.png",
    )));
    merge(&mut kept, PLAYER, "Metadata", &sent, &mut effects);
    let props = &kept.props;
    assert_eq!(props.identity, "Fake Player");
    assert_eq!(props.desktop_entry, "org.fake.Player");
    assert_eq!(props.status, Status::Playing);
    assert_eq!(props.volume, Some(1.0), "held to 0..1");
    assert_eq!(props.loop_status, Some(Loop::Playlist));
    assert_eq!(effects.sent_position, Some(5), "a `t` read as an `x`");
    assert_eq!(props.title, "Song");
    assert_eq!(props.artist, "A, B");
    assert_eq!(props.album, "Album");
    assert_eq!(props.length, 180_000_000);
    assert_eq!(props.track_id, "/org/fake/track/1");
    assert_eq!(kept.art_url, "file:///a.png");
    assert!(effects.position && effects.art);
    // Metadata is sent whole: what it leaves out is gone.
    let bare = Value::from(zbus::zvariant::Dict::from(HashMap::from([(
        "mpris:trackid".to_owned(),
        owned(Value::from(NO_TRACK)),
    )])));
    merge(&mut kept, PLAYER, "Metadata", &bare, &mut effects);
    assert_eq!(kept.props.title, "");
    assert_eq!(kept.props.track_id, "", "NoTrack is no track");
    assert_eq!(kept.art_url, "");
}

mod bus {
    use std::{
        path::PathBuf,
        time::{Duration, Instant},
    };

    use super::super::*;

    const WITHIN: Duration = Duration::from_secs(5);
    const OWNER: &str = ":1.7";
    const NAME: &str = "org.mpris.MediaPlayer2.fake";

    /// The bus driver, as far as the reader asks it anything.
    struct Driver;

    #[zbus::interface(name = "org.freedesktop.DBus")]
    impl Driver {
        fn hello(&self) -> String {
            ":1.1".to_owned()
        }
        fn add_match(&self, _rule: String) {}
        fn remove_match(&self, _rule: String) {}
        fn list_names(&self) -> Vec<String> {
            vec![
                "org.freedesktop.DBus".to_owned(),
                NAME.to_owned(),
                "org.example.NotAPlayer".to_owned(),
            ]
        }
        fn get_name_owner(&self, _name: String) -> String {
            OWNER.to_owned()
        }
    }

    /// What the stand-in player heard, and what it says.
    #[derive(Debug)]
    struct State {
        heard: Vec<String>,
        title: String,
        status: String,
        position: i64,
        volume: f64,
    }

    type Shared = Arc<Mutex<State>>;

    /// `org.mpris.MediaPlayer2`.
    struct Root;

    #[zbus::interface(name = "org.mpris.MediaPlayer2")]
    impl Root {
        #[zbus(property)]
        fn identity(&self) -> String {
            "Fake".to_owned()
        }
        #[zbus(property)]
        fn desktop_entry(&self) -> String {
            "fake".to_owned()
        }
    }

    /// `org.mpris.MediaPlayer2.Player`, as mpv's would be.
    struct Player {
        state: Shared,
    }

    #[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
    impl Player {
        #[zbus(property)]
        fn playback_status(&self) -> String {
            lock(&self.state).status.clone()
        }
        #[zbus(property)]
        fn metadata(&self) -> HashMap<String, OwnedValue> {
            super::metadata(&lock(&self.state).title, "https://example.org/a.jpg")
        }
        #[zbus(property)]
        fn position(&self) -> i64 {
            lock(&self.state).position
        }
        #[zbus(property)]
        fn rate(&self) -> f64 {
            1.0
        }
        #[zbus(property)]
        fn volume(&self) -> f64 {
            lock(&self.state).volume
        }
        #[zbus(property)]
        fn set_volume(&mut self, volume: f64) {
            let mut state = lock(&self.state);
            state.volume = volume;
            state.heard.push(format!("Volume {volume}"));
        }
        #[zbus(property)]
        fn can_play(&self) -> bool {
            true
        }
        #[zbus(property)]
        fn can_pause(&self) -> bool {
            true
        }
        #[zbus(property)]
        fn can_go_next(&self) -> bool {
            true
        }
        #[zbus(property)]
        fn can_go_previous(&self) -> bool {
            true
        }
        #[zbus(property)]
        fn can_seek(&self) -> bool {
            true
        }
        #[zbus(property)]
        fn can_control(&self) -> bool {
            true
        }
        fn play_pause(&self) {
            lock(&self.state).heard.push("PlayPause".to_owned());
        }
        fn next(&self) {
            lock(&self.state).heard.push("Next".to_owned());
        }
        fn seek(&self, offset: i64) {
            lock(&self.state).heard.push(format!("Seek {offset}"));
        }
        fn set_position(&self, track: zbus::zvariant::ObjectPath<'_>, position: i64) {
            lock(&self.state)
                .heard
                .push(format!("SetPosition {} {position}", track.as_str()));
        }
    }

    /// A stand-in session bus at a socket of the test's own: the driver and
    /// a player behind it, and the player's signals sent down Solium's one
    /// connection as the bus would deliver them. No bus, real or private,
    /// anywhere near.
    struct StandIn {
        directory: PathBuf,
        address: String,
        peers: mpsc::Receiver<zbus::blocking::Connection>,
        peer: Option<zbus::blocking::Connection>,
        state: Shared,
    }

    impl StandIn {
        fn new(name: &str) -> Self {
            let directory = std::env::temp_dir()
                .join(format!("solium-media-bus-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&directory);
            std::fs::create_dir_all(&directory).expect("a directory");
            let path = directory.join("bus");
            let listener =
                std::os::unix::net::UnixListener::bind(&path).expect("binding the socket");
            let (found, peers) = mpsc::channel();
            let state: Shared = Arc::new(Mutex::new(State {
                heard: Vec::new(),
                title: "Song".to_owned(),
                status: "Playing".to_owned(),
                position: 42_000_000,
                volume: 0.5,
            }));
            std::thread::spawn({
                let state = state.clone();
                move || {
                    let Ok((stream, _)) = listener.accept() else {
                        return;
                    };
                    let Ok(connection) =
                        zbus::blocking::connection::Builder::async_io_unix_stream(stream)
                            .server(zbus::Guid::generate())
                            .expect("a server guid")
                            .p2p()
                            .serve_at(DBUS_PATH, Driver)
                            .expect("serving the driver")
                            .serve_at(PATH, Root)
                            .expect("serving the player's root")
                            .serve_at(PATH, Player { state })
                            .expect("serving the player")
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
                state,
            }
        }

        fn peer(&mut self) -> zbus::blocking::Connection {
            if self.peer.is_none() {
                self.peer = self.peers.recv_timeout(WITHIN).ok();
            }
            self.peer.clone().expect("Solium connected")
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

        /// What the player said changed.
        fn changed(&mut self, values: HashMap<String, Value<'_>>, invalidated: &[&str]) {
            let invalidated: Vec<String> = invalidated.iter().map(|s| (*s).to_owned()).collect();
            self.signal(
                OWNER,
                PATH,
                PROPERTIES,
                "PropertiesChanged",
                &(PLAYER, values, invalidated),
            );
        }

        /// What the player heard, once it has heard `count` things.
        fn heard(&self, count: usize) -> Vec<String> {
            let deadline = Instant::now() + WITHIN;
            while Instant::now() < deadline {
                let heard = lock(&self.state).heard.clone();
                if heard.len() >= count {
                    return heard;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            lock(&self.state).heard.clone()
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

    fn read(server: &Server) -> ReadPlayer {
        let Some(Fact::Read(read)) = next_fact(server, |fact| matches!(fact, Fact::Read(_))) else {
            panic!("the player was read");
        };
        *read
    }

    #[test]
    fn a_player_already_on_the_bus_is_found_read_and_followed() {
        let mut bus = StandIn::new("found");
        let server = Server::start(Some(bus.address.clone())).expect("a server");
        let first = read(&server);
        assert_eq!(first.key, NAME);
        assert_eq!(first.props.identity, "Fake");
        assert_eq!(first.props.desktop_entry, "fake");
        assert_eq!(first.props.status, Status::Playing);
        assert_eq!(first.props.title, "Song");
        assert_eq!(first.props.artist, "A, B");
        assert_eq!(first.props.length, 180_000_000);
        assert_eq!(first.props.track_id, "/org/fake/track/1");
        assert_eq!(first.props.volume, Some(0.5));
        assert!(first.props.can_control && first.props.can_seek);
        assert_eq!(
            first.props.art,
            Art::Remote("https://example.org/a.jpg".to_owned())
        );
        assert_eq!(first.position.map(|(micros, _)| micros), Some(42_000_000));

        // A new track: its metadata says so, and its position is read again,
        // as no player announces it.
        {
            let mut state = lock(&bus.state);
            "Next song".clone_into(&mut state.title);
            state.position = 1_000_000;
        }
        let sent = Value::from(zbus::zvariant::Dict::from(super::metadata(
            "Next song",
            "https://example.org/a.jpg",
        )));
        bus.changed(HashMap::from([("Metadata".to_owned(), sent)]), &[]);
        let next = read(&server);
        assert_eq!(next.props.title, "Next song");
        assert_eq!(next.props.identity, "Fake", "kept from the first read");
        assert_eq!(next.position.map(|(micros, _)| micros), Some(1_000_000));

        // A status said to have changed, without saying to what: asked.
        lock(&bus.state).status = "Paused".to_owned();
        bus.changed(HashMap::new(), &["PlaybackStatus"]);
        assert_eq!(read(&server).props.status, Status::Paused);

        // A seek.
        bus.signal(OWNER, PATH, PLAYER, "Seeked", &7_000_000i64);
        let seeked = read(&server);
        assert_eq!(seeked.position.map(|(micros, _)| micros), Some(7_000_000));
        assert_eq!(seeked.props.title, "Next song");

        // It leaves the bus.
        bus.signal(
            DBUS,
            DBUS_PATH,
            DBUS,
            "NameOwnerChanged",
            &(NAME, OWNER, ""),
        );
        let Some(Fact::Gone { key }) = next_fact(&server, |fact| matches!(fact, Fact::Gone { .. }))
        else {
            panic!("the player left");
        };
        assert_eq!(key, NAME);
    }

    #[test]
    fn a_player_that_appears_later_is_read_and_a_name_that_is_no_player_is_not() {
        let mut bus = StandIn::new("later");
        let server = Server::start(Some(bus.address.clone())).expect("a server");
        assert_eq!(read(&server).key, NAME);
        bus.signal(
            DBUS,
            DBUS_PATH,
            DBUS,
            "NameOwnerChanged",
            &("org.example.NotAPlayer", "", ":1.9"),
        );
        bus.signal(
            DBUS,
            DBUS_PATH,
            DBUS,
            "NameOwnerChanged",
            &("org.mpris.MediaPlayer2.fake.instance2", "", OWNER),
        );
        assert_eq!(read(&server).key, "org.mpris.MediaPlayer2.fake.instance2");
    }

    #[test]
    fn actions_reach_the_player_at_its_unique_name() {
        let bus = StandIn::new("actions");
        let server = Server::start(Some(bus.address.clone())).expect("a server");
        let key = read(&server).key;
        server.call(&key, Call::PlayPause);
        server.call(&key, Call::Next);
        server.call(&key, Call::Seek(-5_000_000));
        server.call(
            &key,
            Call::SetPosition {
                track_id: "/org/fake/track/1".to_owned(),
                micros: 1_000_000,
            },
        );
        server.call(&key, Call::Volume(0.25));
        // A player nobody follows is not called.
        server.call("org.mpris.MediaPlayer2.nobody", Call::PlayPause);
        assert_eq!(
            bus.heard(5),
            [
                "PlayPause",
                "Next",
                "Seek -5000000",
                "SetPosition /org/fake/track/1 1000000",
                "Volume 0.25",
            ]
        );
    }
}
