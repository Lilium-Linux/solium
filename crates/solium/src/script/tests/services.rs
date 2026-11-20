//! The session services: notifications, the tray, media and audio.

use super::*;

/// **`sol.act` queues the action as a command and answers an attempt id;
/// `done` hears the outcome once** (03 §3.3.2).
#[test]
fn sol_notifications_sets_the_service_and_answers_its_rows() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-notifications",
        r#"sol.notifications{ serve = true, timeouts = { low = 1000, normal = 9000, critical = 0 }, history = { max = 5 } }"#,
    );
    let configs: Vec<crate::services::notifications::Config> = scripts
        .startup()
        .commands
        .into_iter()
        .filter_map(|command| match command {
            Command::Notifications(config) => Some(config),
            _ => None,
        })
        .collect();
    let [config] = configs.as_slice() else {
        panic!("one sol.notifications: {configs:?}")
    };
    assert!(config.serve);
    assert_eq!(
        config.settings.timeouts.low,
        Some(Duration::from_millis(1000))
    );
    assert_eq!(
        config.settings.timeouts.normal,
        Some(Duration::from_millis(9000))
    );
    assert_eq!(config.settings.timeouts.critical, None);
    assert_eq!(config.settings.history_max, 5);

    let mut snapshot = one_screen(&[]);
    snapshot.notifications = std::rc::Rc::new(crate::services::notifications::View {
        available: true,
        serving: true,
        dnd: false,
        owner: None,
        banners: vec![std::collections::BTreeMap::from([
            ("id", crate::json::Json::Number(7.0)),
            ("summary", crate::json::Json::Text("New mail".to_owned())),
        ])],
        history: Vec::new(),
    });
    assert_eq!(
        scripts.evaluate_in(
            snapshot,
            "local n = sol.notifications(); return n.banners[1].summary .. ' ' .. n.banners[1].id .. ' ' .. tostring(n.serving) .. ' ' .. #n.history"
        ),
        "New mail 7 true 0"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn sol_audio_sets_its_limits_and_answers_its_endpoints() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-audio",
        r#"sol.audio{ max = 1.2, step = 0.02 }"#,
    );
    let settings: Vec<Option<crate::services::audio::model::Settings>> = scripts
        .startup()
        .commands
        .into_iter()
        .filter_map(|command| match command {
            Command::Audio(settings) => Some(settings),
            _ => None,
        })
        .collect();
    assert_eq!(
        settings,
        [Some(crate::services::audio::model::Settings {
            max: 1.2,
            step: 0.02
        })]
    );
    let mut snapshot = one_screen(&[]);
    snapshot.audio = std::rc::Rc::new(crate::services::audio::View {
        available: true,
        ready: true,
        output: Some(std::collections::BTreeMap::from([
            ("label", crate::json::Json::Text("Speakers".to_owned())),
            ("volume", crate::json::Json::Number(0.5)),
        ])),
        input: None,
        outputs: Vec::new(),
        inputs: Vec::new(),
        streams: vec![std::collections::BTreeMap::from([
            ("id", crate::json::Json::Number(41.0)),
            ("app", crate::json::Json::Text("Firefox".to_owned())),
            (
                "windows",
                crate::json::Json::List(vec![crate::json::Json::Number(7.0)]),
            ),
        ])],
        windows: std::collections::BTreeMap::from([(
            7,
            std::collections::BTreeMap::from([
                ("volume", crate::json::Json::Number(0.25)),
                ("count", crate::json::Json::Number(1.0)),
            ]),
        )]),
    });
    assert_eq!(
        scripts.evaluate_in(
            snapshot,
            "local a = sol.audio(); return a.output.label .. ' ' .. a.output.volume .. ' ' .. tostring(a.input) .. ' ' .. #a.outputs \
             .. ' ' .. a.streams[1].app .. ' ' .. a.streams[1].windows[1] .. ' ' .. a.windows[7].volume"
        ),
        "Speakers 0.5 nil 0 Firefox 7 0.25"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn sol_tray_sets_the_service_and_answers_its_items() {
    let (directory, mut scripts) = loaded("solium-script-test-tray", r#"sol.tray{ serve = true }"#);
    let configs: Vec<crate::services::tray::Config> = scripts
        .startup()
        .commands
        .into_iter()
        .filter_map(|command| match command {
            Command::Tray(config) => Some(config),
            _ => None,
        })
        .collect();
    assert_eq!(configs, [crate::services::tray::Config { serve: true }]);
    let mut snapshot = one_screen(&[]);
    snapshot.tray = std::rc::Rc::new(crate::services::tray::View {
        available: true,
        serving: true,
        owner: None,
        items: vec![std::collections::BTreeMap::from([
            ("id", crate::json::Json::Number(4.0)),
            ("title", crate::json::Json::Text("Steam".to_owned())),
        ])],
    });
    assert_eq!(
        scripts.evaluate_in(
            snapshot,
            "local t = sol.tray(); return t.items[1].title .. ' ' .. t.items[1].id .. ' ' .. tostring(t.serving) .. ' ' .. tostring(t.owner)"
        ),
        "Steam 4 true nil"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn sol_media_sets_the_service_and_answers_its_players() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-media",
        r#"sol.media{ watch = true, ignore = { "kdeconnect" }, position_interval_ms = 250 }"#,
    );
    let configs: Vec<crate::services::media::Config> = scripts
        .startup()
        .commands
        .into_iter()
        .filter_map(|command| match command {
            Command::Media(config) => Some(config),
            _ => None,
        })
        .collect();
    assert_eq!(
        configs,
        [crate::services::media::Config {
            watch: true,
            ignore: vec!["kdeconnect".to_owned()],
            position_interval_ms: 250,
        }]
    );
    let mut snapshot = one_screen(&[]);
    let row = std::collections::BTreeMap::from([
        ("id", crate::json::Json::Number(3.0)),
        ("title", crate::json::Json::Text("Song".to_owned())),
        ("active", crate::json::Json::Bool(true)),
        ("playing", crate::json::Json::Bool(true)),
        ("rate", crate::json::Json::Number(1.0)),
        ("position", crate::json::Json::Number(10.0)),
        ("positionAt", crate::json::Json::Number(1000.0)),
    ]);
    snapshot.media = crate::services::media::Seen {
        view: std::rc::Rc::new(crate::services::media::View {
            available: true,
            active: 3,
            position_interval_ms: 1000,
            players: vec![row],
        }),
        now: std::time::Duration::from_secs(4),
    };
    assert_eq!(
        scripts.evaluate_in(
            snapshot,
            "local m = sol.media(); return m.active.title .. ' ' .. m.players[1].id .. ' ' .. m.active.position .. ' ' .. tostring(m.available)"
        ),
        "Song 3 13 true"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn sol_notify_queues_one_of_the_shells_own_with_its_actions() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-notify",
        r#"sol.on("surface", function()
               sol.notify{ summary = "Battery low", body = "10% left", icon = "battery-caution", urgency = "critical",
                           actions = { { key = "open", label = "Settings", fn = function() sol.status("opened") end } } }
           end)"#,
    );
    let outcome = scripts.surface_action("shell", "go", &crate::json::Json::Null, one_screen(&[]));
    let owns: Vec<crate::services::notifications::Own> = outcome
        .commands
        .into_iter()
        .filter_map(|command| match command {
            Command::Notify(own) => Some(own),
            _ => None,
        })
        .collect();
    let [own] = owns.as_slice() else {
        panic!("one sol.notify: {owns:?}")
    };
    assert_eq!(own.summary, "Battery low");
    assert_eq!(own.icon, "battery-caution");
    assert_eq!(
        own.urgency,
        crate::services::notifications::model::Urgency::Critical
    );
    assert_eq!(own.actions, [("open".to_owned(), "Settings".to_owned())]);
    assert_ne!(own.token, 0);
    let ran = scripts.notify_action(own.token, "open", one_screen(&[]));
    assert_eq!(ran.status.as_deref(), Some("opened"));
    // Run once: the functions are let go after it.
    let again = scripts.notify_action(own.token, "open", one_screen(&[]));
    assert_eq!(again.status, None);
    let _ = std::fs::remove_dir_all(&directory);
}
