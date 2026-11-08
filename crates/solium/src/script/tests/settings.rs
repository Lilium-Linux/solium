//! The settings sections, read from a configuration.

use super::*;

/// `sol.pane_build` hands `config.pane_build` over as a duration, whole
/// or fractional milliseconds, and a value that is no time at all changes
/// nothing (#226).
#[test]
fn sol_pane_build_hands_the_budget_over() {
    let directory = std::env::temp_dir().join("solium-script-test-pane-build");
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        sol.bind("Super+A", function() sol.pane_build(5) end)
        sol.bind("Super+B", function() sol.pane_build(0.5) end)
        sol.bind("Super+C", function() sol.pane_build(-1) end)
        sol.bind("Super+D", function() sol.pane_build("soon") end)
        "#,
    )
    .expect("writing the test script");

    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let mut budget = |combo: &str| {
        let outcome = scripts.key(combo, empty_snapshot());
        assert!(outcome.handled, "{combo} was not handled");
        match outcome.commands.as_slice() {
            [Command::PaneBuild(budget)] => Some(*budget),
            [] => None,
            other => panic!("expected at most a budget from {combo}, got {other:?}"),
        }
    };
    assert_eq!(budget("super+a"), Some(Duration::from_millis(5)));
    assert_eq!(budget("super+b"), Some(Duration::from_micros(500)));
    assert_eq!(budget("super+c"), None, "a time before now is no budget");
    assert_eq!(budget("super+d"), None, "nor is a word");
}

/// **`sol.decoration` is still `sol.pane`.**
///
/// The setting was renamed when a style stopped being one QML file and
/// became a folder. `sol.decoration` stays because an `init.lua` someone
/// wrote before that calls it by name, and Lua gives no warning for a call
/// to a nil field -- it raises, the script that raised is abandoned, and a
/// configuration that used to work comes up with no bindings and no
/// layouts. One line to keep, and this is what says the line is there.
///
/// Both are driven, and the *same* command has to come back from each: an
/// alias bound to some other function would pass a test that only checked
/// `sol.decoration` was callable.
#[test]
fn the_old_name_for_sol_pane_still_works() {
    let directory = std::env::temp_dir().join("solium-script-test-pane-alias");
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        sol.bind("Super+P", function() sol.pane("border") end)
        sol.bind("Super+D", function() sol.decoration("border") end)
        "#,
    )
    .expect("writing the test script");

    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let named = |scripts: &mut Scripts, combo: &str| {
        let outcome = scripts.key(combo, empty_snapshot());
        assert!(outcome.handled, "{combo} was not handled");
        match outcome.commands.as_slice() {
            [Command::Decoration { name }] => name.clone(),
            other => panic!("expected one style command from {combo}, got {other:?}"),
        }
    };

    assert_eq!(named(&mut scripts, "super+p").as_deref(), Some("border"));
    assert_eq!(
        named(&mut scripts, "super+d").as_deref(),
        Some("border"),
        "the old name reaches the same command as the new one"
    );
}

/// **A `user.lua` written before the rename still chooses a style.**
///
/// The migration path the `sol.decoration` alias does *not* cover, and the
/// one that would have broken quietly. `config.lua` merges the user's table
/// over its defaults key by key, so a file setting the old `decoration`
/// leaves `pane` at `"top"` -- their choice read, merged, and thrown away,
/// with the shipped default on screen and nothing anywhere saying why.
///
/// It runs the **shipped** `config.lua`, not a copy: the temporary
/// directory holds only `init.lua` and `user.lua`, and `Scripts::load` puts
/// that directory ahead of the shipped one on `package.path` -- so
/// `require("user")` finds the fixture and `require("config")` finds the
/// real thing. That is what makes this a test of the file under review
/// rather than of a restatement of it.
///
/// Both halves, because they are two edits with one purpose:
///
/// * `config.pane` is the old key's value, which is the read-across;
/// * `config.decoration` is `nil`, which is the stale key not surviving the
///   merge into the table the compositor reads.
///
/// Guarded on a real `~/.config/solium`, which comes *first* on that path:
/// a developer with their own `user.lua` would have it answer instead of
/// the fixture, and the assertion would then be about their configuration.
#[test]
fn a_user_file_using_the_old_key_still_chooses_a_style() {
    let Some(own) = Scripts::user_config_dir() else {
        return;
    };
    if own.join("user.lua").exists() || own.join("config.lua").exists() {
        // This machine's own configuration would decide it, not the fixture.
        return;
    }

    let directory = std::env::temp_dir().join("solium-script-test-old-key");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    std::fs::write(
        directory.join("user.lua"),
        "return { decoration = \"border\" }\n",
    )
    .expect("writing the fixture user.lua");
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        local config = require("config")
        sol.bind("Super+P", function()
            sol.pane(config.pane)
            sol.status(tostring(config.decoration))
        end)
        "#,
    )
    .expect("writing the test script");

    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let outcome = scripts.key("super+p", empty_snapshot());
    assert!(outcome.handled);

    match outcome.commands.as_slice() {
        [Command::Decoration { name }] => assert_eq!(
            name.as_deref(),
            Some("border"),
            "a user.lua setting the old `decoration` key still names the style"
        ),
        other => panic!("expected one style command, got {other:?}"),
    }
    assert_eq!(
        outcome.status.as_deref(),
        Some("nil"),
        "and the old key does not survive into the configuration table"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// A configured cursor theme and size reach the compositor as written.
///
/// The wiring between `sol.cursor_theme` and `Command::Cursor`, which is the
/// one part of the cursor work that no test in `cursor.rs` can reach: that
/// module's tests start from a `Configured` that they built themselves.
#[test]
fn a_configured_cursor_theme_reaches_the_compositor() {
    let directory = std::env::temp_dir().join("solium-script-test-cursor");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        sol.bind("Super+C", function()
            sol.cursor_theme({ theme = "Fixture", size = 40 })
        end)
        "#,
    )
    .expect("writing the test script");

    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let outcome = scripts.key("super+c", empty_snapshot());
    assert!(outcome.handled);
    match outcome.commands.as_slice() {
        [Command::Cursor(configured)] => {
            assert_eq!(configured.theme.as_deref(), Some("Fixture"));
            assert_eq!(configured.size, Some(40));
        }
        other => panic!("expected one cursor command, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&directory);
}

/// **`cursor.scene` reaches the compositor as written**, for the pointer to
/// look for as the shell's scene is looked for; a value that is not a
/// string is no scene, with a line in the log, rather than a configuration
/// that fails to load.
#[test]
fn a_configured_cursor_scene_reaches_the_compositor() {
    let directory = std::env::temp_dir().join("solium-script-test-cursor-scene");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        sol.bind("Super+C", function()
            sol.cursor_theme({ scene = "~/.config/solium/cursor/Cursor.qml", size = 32 })
        end)
        sol.bind("Super+V", function()
            sol.cursor_theme({ scene = 5 })
        end)
        "#,
    )
    .expect("writing the test script");

    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let scenes: Vec<_> = ["super+c", "super+v"]
        .into_iter()
        .map(
            |key| match scripts.key(key, empty_snapshot()).commands.as_slice() {
                [Command::Cursor(configured)] => configured.scene.clone(),
                other => panic!("expected one cursor command, got {other:?}"),
            },
        )
        .collect();
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(
        scenes,
        [Some("~/.config/solium/cursor/Cursor.qml".to_owned()), None],
        "[a path, a number]"
    );
}

#[test]
fn an_absolute_devices_output_in_user_lua_reaches_the_input_settings() {
    let (directory, mut scripts) = shipped_init_with_user(
        "solium-script-test-input-output",
        r#"return { input = {
            touchscreen = { output = "eDP-1" },
            tablet_tool = { output = "DP-2" },
            devices = { { name = "External", output = "HDMI-A-1" } },
        } }"#,
    )
    .expect("shipped initialization");
    let config = input_config(scripts.startup()).expect("init calls sol.input");
    let mut panel = DeviceInfo {
        id: "panel".into(),
        name: "Panel".into(),
        kind: DeviceKind::Touchscreen,
        vendor: None,
        product: None,
        output: None,
    };
    assert_eq!(config.resolve(&panel).output.as_deref(), Some("eDP-1"));
    panel.kind = DeviceKind::TabletTool;
    assert_eq!(config.resolve(&panel).output.as_deref(), Some("DP-2"));
    panel.name = "External tablet".into();
    assert_eq!(config.resolve(&panel).output.as_deref(), Some("HDMI-A-1"));
    let _ = std::fs::remove_dir_all(directory);
}

/// **`input.touchscreen.velocity_window_ms` reaches the compositor, and
/// a value that is not a whole number from 1 to 1000 is named and left
/// unset (#284).**
#[test]
fn a_touchscreens_velocity_window_is_read_and_a_bad_one_is_named() {
    for (window, expected) in [
        ("40", Some(40)),
        ("1000", Some(1000)),
        ("0", None),
        ("1.5", None),
        ("2000", None),
        ("\"fast\"", None),
    ] {
        let log = logged_while(|| {
            let (directory, mut scripts) = shipped_init_with_user(
                "solium-script-test-input-velocity-window",
                &format!(
                    "return {{ input = {{ touchscreen = {{ velocity_window_ms = {window} }} }} }}"
                ),
            )
            .expect("shipped initialization");
            let config = input_config(scripts.startup()).expect("init calls sol.input");
            assert_eq!(
                config
                    .default_for(DeviceKind::Touchscreen)
                    .and_then(|settings| settings.velocity_window_ms),
                expected,
                "velocity_window_ms = {window}"
            );
            let _ = std::fs::remove_dir_all(directory);
        });
        assert_eq!(
            log.contains("input.velocity_window_ms"),
            expected.is_none(),
            "velocity_window_ms = {window}: {log}"
        );
    }
}

#[test]
fn an_absolute_devices_output_of_the_wrong_type_is_logged_and_kept_unset() {
    let log = logged_while(|| {
        let (directory, mut scripts) = shipped_init_with_user(
            "solium-script-test-input-output-invalid",
            "return { input = { touchscreen = { output = 3 } } }",
        )
        .expect("shipped initialization");
        let config = input_config(scripts.startup()).expect("init calls sol.input");
        let panel = DeviceInfo {
            id: "panel".into(),
            name: "Panel".into(),
            kind: DeviceKind::Touchscreen,
            vendor: None,
            product: None,
            output: Some("eDP-1".into()),
        };
        let settings = config.resolve(&panel);
        assert_eq!(settings.output, None);
        assert_eq!(
            crate::input::devices::bound_output(&panel, &settings),
            Some("eDP-1"),
            "invalid configuration preserves the device binding"
        );
        let _ = std::fs::remove_dir_all(directory);
    });
    assert!(log.contains("input.output"), "{log}");
    assert!(log.contains("touchscreen"), "{log}");
}

/// **A value of the wrong shape for its field is named in the log and
/// left unset, not fatal.** `accel_profile` only has two sensible
/// spellings, and a third must not take the rest of `input` down with it
/// -- the same policy `idle.dbus_inhibit` already has
/// (`idle_dbus_inhibit_false_owns_nothing`).
#[test]
fn sol_input_warns_on_an_unrecognised_option_value_and_keeps_it_unset() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-input-bad-value",
        r#"sol.input({ mouse = { accel_profile = "fastest" } })"#,
    );
    let config = input_config(scripts.startup()).expect("the call ran");
    let _ = std::fs::remove_dir_all(&directory);

    let mouse = DeviceInfo {
        id: "mouse".into(),
        name: "Any Mouse".into(),
        kind: DeviceKind::Mouse,
        vendor: None,
        product: None,
        output: None,
    };
    assert_eq!(
        config.resolve(&mouse).accel_profile,
        None,
        "\"fastest\" is not \"flat\" or \"adaptive\"; it must not be guessed at"
    );
}

/// **`after_close` is read word by word** (#128): a word, a list in
/// order, a word it does not know left out with the rest kept, an empty
/// list kept empty -- the keyboard left on nothing -- and `nil` or
/// anything else read as `None`, the compositor's own rule.
#[test]
fn sol_focus_mode_reads_after_close_word_by_word() {
    use crate::successor::Successor::{Neighbour, Pointer, Recent, Topmost};
    let directory = std::env::temp_dir().join("solium-script-test-after-close-words");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        sol.focus_mode({ after_close = { "recent", "neighbor", "pointer", 7 } })
        sol.focus_mode({ after_close = "topmost" })
        sol.focus_mode({ after_close = { "neighbour" } })
        sol.focus_mode({ after_close = {} })
        sol.focus_mode({})
        sol.focus_mode({ after_close = 42 })
        "#,
    )
    .expect("writing the test script");
    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let words = after_closes(scripts.startup().commands);
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(
        words,
        [
            Some(vec![Recent, Pointer]),
            Some(vec![Topmost]),
            Some(vec![Neighbour]),
            Some(vec![]),
            None,
            None,
        ]
    );
}

/// **A key `sol.focus_mode` is not given is left to the machine** (#219):
/// unlike `sol.idle`, there is no fixed default for the script to fill
/// in, because a mode calling it knows nothing about the machine's own
/// form factor. So the command says `None`, and the compositor reads
/// that as the form factor's answer (`Command::FocusMode`).
#[test]
fn sol_focus_mode_leaves_a_key_out_to_the_machine() {
    let directory = std::env::temp_dir().join("solium-script-test-focus-mode");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        sol.focus_mode({ follow = false })
        "#,
    )
    .expect("writing the test script");
    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let modes = focus_modes(scripts.startup().commands);
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(
        modes,
        [(None, Some(false), None)],
        "a table naming only `follow` must leave `click` and \
         `clear_on_empty_click` as `None`, to the machine"
    );
}

/// `sol.qml` is answered straight after the configuration is read, before
/// `startup` hands anything it asked for to the compositor.
#[test]
fn a_configured_qml_renderer_is_read_before_qt_starts() {
    let directory = std::env::temp_dir().join("solium-script-test-qml");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let config = directory.join("init.lua");
    let configured = |source: &str| {
        std::fs::write(&config, source).expect("writing the test script");
        Scripts::load(&config)
            .expect("loading the test script")
            .qml()
    };

    assert_eq!(
        configured(r#"sol.qml({ renderer = "software", probe_timeout = 1500 })"#),
        crate::qml::renderer::Configured {
            renderer: Some("software".to_owned()),
            probe_timeout: Some(Duration::from_millis(1500)),
            buffers: None,
        }
    );
    // `qml.buffers`, each key on its own, the other left at its default.
    let buffers = |step, shrink| crate::qml::buffers::Buffers::new(step, shrink);
    for (source, wanted) in [
        (
            "sol.qml({ buffers = { step = 256, shrink = 3 } })",
            buffers(256, 3),
        ),
        (
            "sol.qml({ buffers = { step = 0 } })",
            buffers(0, crate::qml::buffers::SHRINK),
        ),
        (
            "sol.qml({ buffers = { shrink = 5 } })",
            buffers(crate::qml::buffers::STEP, 5),
        ),
        // Out of bounds, or not whole numbers: warned about, and the
        // defaults kept.
        ("sol.qml({ buffers = { step = 2000 } })", None),
        ("sol.qml({ buffers = { shrink = 1 } })", None),
        ("sol.qml({ buffers = { step = 64.5 } })", None),
        ("sol.qml({ buffers = 128 })", None),
    ] {
        assert_eq!(configured(source).buffers, wanted, "{source}");
    }
    // Said nothing, three ways.
    for source in ["", "sol.qml(nil)", "sol.qml({})"] {
        assert_eq!(
            configured(source),
            crate::qml::renderer::Configured::default(),
            "{source:?}"
        );
    }
    // Not a string and not a timeout: warned about, and left unsaid.
    assert_eq!(
        configured(r#"sol.qml({ renderer = true, probe_timeout = 0 })"#),
        crate::qml::renderer::Configured::default()
    );

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_configured_session_is_read_by_the_backend() {
    let directory = std::env::temp_dir().join("solium-script-test-session");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let config = directory.join("init.lua");
    let configured = |source: &str| {
        std::fs::write(&config, source).expect("writing the test script");
        Scripts::load(&config)
            .expect("loading the test script")
            .session()
    };
    let settings = |systemd, autostart| crate::session::Settings {
        systemd,
        autostart,
        ..crate::session::Settings::default()
    };

    assert_eq!(
        configured("sol.session({ systemd = false })"),
        settings(false, true)
    );
    assert_eq!(
        configured("sol.session({ autostart = false })"),
        settings(true, false)
    );
    assert_eq!(
        configured("sol.session({ stop_timeout = 1500 })"),
        crate::session::Settings {
            stop_timeout: Duration::from_millis(1500),
            ..settings(true, true)
        }
    );
    // Said nothing, three ways, and said something of the wrong kind.
    for source in [
        "",
        "sol.session(nil)",
        "sol.session({})",
        r#"sol.session({ systemd = "no", autostart = 0 })"#,
        "sol.session({ stop_timeout = 0 })",
        "sol.session({ stop_timeout = -5 })",
        "sol.session({ stop_timeout = 2.5 })",
        r#"sol.session({ stop_timeout = "soon" })"#,
    ] {
        assert_eq!(configured(source), settings(true, true), "{source:?}");
    }

    let _ = std::fs::remove_dir_all(&directory);
}

/// `sol.lock`'s own parsing: a string `command`, a boolean
/// `before_sleep` and `logind_unlock`, an absent table keeping the
/// defaults, and a value of
/// the wrong kind named in the log with the default kept. See
/// `crate::logind::Settings`.
#[test]
fn sol_lock_parses_command_and_before_sleep() {
    let directory = std::env::temp_dir().join("solium-script-test-lock");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let config = directory.join("init.lua");
    let configured = |source: &str| {
        std::fs::write(&config, source).expect("writing the test script");
        let mut scripts = Scripts::load(&config).expect("loading the test script");
        scripts
            .startup()
            .commands
            .into_iter()
            .find_map(|command| match command {
                Command::Lock(settings) => Some(settings),
                _ => None,
            })
            .expect("one Command::Lock")
    };

    assert_eq!(
        configured(r#"sol.lock({ command = "swaylock -f" })"#),
        crate::logind::Settings {
            command: Some("swaylock -f".to_owned()),
            before_sleep: true,
            logind_unlock: true,
        }
    );
    assert_eq!(
        configured("sol.lock({ before_sleep = false })"),
        crate::logind::Settings {
            command: None,
            before_sleep: false,
            logind_unlock: true,
        }
    );
    assert_eq!(
        configured("sol.lock({ logind_unlock = false })"),
        crate::logind::Settings {
            command: None,
            before_sleep: true,
            logind_unlock: false,
        }
    );
    // Said nothing, two ways, and said something of the wrong kind.
    for source in [
        "sol.lock()",
        "sol.lock({})",
        "sol.lock({ command = 5, before_sleep = \"no\", logind_unlock = 1 })",
    ] {
        assert_eq!(
            configured(source),
            crate::logind::Settings::default(),
            "{source:?}"
        );
    }

    let _ = std::fs::remove_dir_all(&directory);
}

/// `sol.lock`'s settings for Solium's own lock (#281): every one read,
/// one alone keeping the others' defaults, a `max_seconds` under its
/// `seconds` taken as `seconds`, and a value of the wrong kind named in
/// the log with the default kept. See `crate::lock_auth::Settings`.
#[test]
fn sol_lock_parses_the_native_locks_settings() {
    use crate::lock_auth::{Backoff, Keypad, Lockout, SecretLayout, Settings};
    let directory = std::env::temp_dir().join("solium-script-test-lock-auth");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let config = directory.join("init.lua");
    let configured = |source: &str| {
        std::fs::write(&config, source).expect("writing the test script");
        let mut scripts = Scripts::load(&config).expect("loading the test script");
        scripts
            .startup()
            .commands
            .into_iter()
            .find_map(|command| match command {
                Command::LockAuth(settings) => Some(settings),
                _ => None,
            })
            .expect("one Command::LockAuth")
    };

    assert_eq!(
        configured(
            r#"sol.lock({
                lockout = { attempts = 5, seconds = 10, max_seconds = 40 },
                secret_layout = "current",
                helper_timeout_ms = 5000,
                retry = { seconds = 1, max_seconds = 8, attempts = 3 },
                keypad = "keyboard",
            })"#
        ),
        Settings {
            lockout: Lockout {
                attempts: 5,
                wait: Backoff {
                    seconds: 10,
                    max_seconds: 40,
                },
            },
            secret_layout: SecretLayout::Current,
            helper_timeout: std::time::Duration::from_millis(5000),
            retry: Backoff {
                seconds: 1,
                max_seconds: 8,
            },
            retries: 3,
            keypad: Keypad::Keyboard,
            pin: crate::pin::Settings::default(),
        }
    );
    assert_eq!(
        configured("sol.lock({ lockout = { attempts = 4 }, keypad = \"letters\" })"),
        Settings {
            lockout: Lockout {
                attempts: 4,
                ..Lockout::default()
            },
            keypad: Keypad::Letters,
            ..Settings::default()
        }
    );
    assert_eq!(
        configured("sol.lock({ retry = { seconds = 50, max_seconds = 10 } })").retry,
        Backoff {
            seconds: 50,
            max_seconds: 50,
        }
    );
    // Said nothing, two ways, and said everything of the wrong kind.
    for source in [
        "sol.lock()",
        "sol.lock({})",
        r#"sol.lock({
            lockout = 3,
            secret_layout = "dvorak",
            helper_timeout_ms = 5,
            retry = { seconds = 0, max_seconds = "a minute", attempts = -1 },
            keypad = 7,
        })"#,
        "sol.lock({ lockout = { attempts = 0, seconds = 1.5 } })",
    ] {
        assert_eq!(configured(source), Settings::default(), "{source:?}");
    }

    let _ = std::fs::remove_dir_all(&directory);
}

/// `lock.pin` (#281): every key read, one alone keeping the others'
/// defaults, a `max_length` under `min_length` taken as `min_length`, and
/// a value of the wrong kind or out of bounds (a PIN shorter than four
/// digits, no attempts, no hours) named in the log with the default kept.
/// See `crate::pin::Settings`.
#[test]
fn sol_lock_parses_the_pins_settings() {
    use crate::pin::Settings;
    use std::time::Duration;
    let directory = std::env::temp_dir().join("solium-script-test-lock-pin");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let config = directory.join("init.lua");
    let configured = |source: &str| {
        std::fs::write(&config, source).expect("writing the test script");
        let mut scripts = Scripts::load(&config).expect("loading the test script");
        scripts
            .startup()
            .commands
            .into_iter()
            .find_map(|command| match command {
                Command::LockAuth(settings) => Some(settings.pin),
                _ => None,
            })
            .expect("one Command::LockAuth")
    };
    assert_eq!(
        configured(
            r#"sol.lock({ pin = {
                enabled = false, min_length = 6, max_length = 10,
                attempts = 3, password_after_hours = 12,
            } })"#
        ),
        Settings {
            enabled: false,
            min_length: 6,
            max_length: 10,
            attempts: 3,
            password_after: Duration::from_secs(12 * 3600),
        }
    );
    assert_eq!(
        configured("sol.lock({ pin = { attempts = 9 } })"),
        Settings {
            attempts: 9,
            ..Settings::default()
        }
    );
    let longer = configured("sol.lock({ pin = { min_length = 9 } })");
    assert_eq!((longer.min_length, longer.max_length), (9, 9));
    // Past the caps, the caps: ten wrong PINs, three days.
    let loose = configured("sol.lock({ pin = { attempts = 50, password_after_hours = 1000 } })");
    assert_eq!(
        (loose.attempts, loose.password_after),
        (10, Duration::from_secs(72 * 3600))
    );
    let edge = configured("sol.lock({ pin = { attempts = 10, password_after_hours = 72 } })");
    assert_eq!(
        (edge.attempts, edge.password_after),
        (10, Duration::from_secs(72 * 3600))
    );
    for source in [
        "sol.lock({})",
        "sol.lock({ pin = true })",
        r#"sol.lock({ pin = {
            enabled = "yes", min_length = 3, max_length = 65,
            attempts = 0, password_after_hours = 0.5,
        } })"#,
        "sol.lock({ pin = { password_after_hours = -1, attempts = \"five\" } })",
    ] {
        assert_eq!(configured(source), Settings::default(), "{source:?}");
    }
    let _ = std::fs::remove_dir_all(&directory);
}

/// `sol.lock`'s settings for what Solium's own lock shows (#281): every
/// one read, `native` true by default and false by default once
/// `command` names a locker, `enter_ms` and `exit_ms` held to their
/// maximums, and a value of the wrong kind named in the log with the
/// default kept. See `crate::lock_scene::Settings`.
#[test]
fn sol_lock_parses_what_the_native_lock_shows() {
    use crate::lock_scene::{ENTER_MAX_MS, EXIT_MAX_MS, PromptOn, Settings};
    use std::time::Duration;
    let directory = std::env::temp_dir().join("solium-script-test-lock-scene");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let config = directory.join("init.lua");
    let configured = |source: &str| {
        std::fs::write(&config, source).expect("writing the test script");
        let mut scripts = Scripts::load(&config).expect("loading the test script");
        scripts
            .startup()
            .commands
            .into_iter()
            .find_map(|command| match command {
                Command::LockScene(settings) => Some(settings),
                _ => None,
            })
            .expect("one Command::LockScene")
    };

    assert_eq!(
        configured(
            r#"sol.lock({
                native = false,
                scene = "~/my-lock.qml",
                prompt_on = "DP-2",
                enter_ms = 500,
                exit_ms = 400,
                keypad_margin = 0,
            })"#
        ),
        Settings {
            native: false,
            scene: "~/my-lock.qml".to_owned(),
            prompt_on: PromptOn::Monitor("DP-2".to_owned()),
            enter: Duration::from_millis(500),
            exit: Duration::from_millis(400),
            keypad_margin: 0,
        }
    );
    assert_eq!(
        configured(r#"sol.lock({ prompt_on = "primary", enter_ms = 0, exit_ms = 0 })"#),
        Settings {
            prompt_on: PromptOn::Primary,
            enter: Duration::ZERO,
            exit: Duration::ZERO,
            ..Settings::default()
        }
    );
    assert_eq!(
        configured("sol.lock({ exit_ms = 5000 })").exit,
        Duration::from_millis(u64::from(EXIT_MAX_MS)),
        "an exit past the maximum was not held to it"
    );
    assert_eq!(
        configured("sol.lock({ enter_ms = 5000 })").enter,
        Duration::from_millis(u64::from(ENTER_MAX_MS)),
        "an entry past the maximum was not held to it"
    );
    assert!(
        !configured(r#"sol.lock({ command = "swaylock -f" })"#).native,
        "a locker named, and Solium's own lock is still the default"
    );
    assert!(
        configured(r#"sol.lock({ command = "swaylock -f", native = true })"#).native,
        "`native = true` beside a locker was not taken"
    );
    // Said nothing, two ways, and said everything of the wrong kind.
    for source in [
        "sol.lock()",
        "sol.lock({})",
        r#"sol.lock({ native = "yes", scene = "", prompt_on = 3, enter_ms = -1, exit_ms = -5, keypad_margin = -1 })"#,
        "sol.lock({ scene = 7, enter_ms = \"slow\", exit_ms = 12.5, keypad_margin = \"wide\" })",
    ] {
        assert_eq!(configured(source), Settings::default(), "{source:?}");
    }

    let _ = std::fs::remove_dir_all(&directory);
}

/// `sol.auto_reload`'s own parsing (#223): a boolean `automatic`, a
/// non-negative `quiet_ms`, an absent table keeping both defaults, and a
/// value of the wrong kind named in the log with the default kept. See
/// `crate::autoreload::Settings`.
#[test]
fn sol_auto_reload_parses_automatic_and_quiet_ms() {
    let directory = std::env::temp_dir().join("solium-script-test-auto-reload");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let config = directory.join("init.lua");
    let configured = |source: &str| {
        std::fs::write(&config, source).expect("writing the test script");
        let mut scripts = Scripts::load(&config).expect("loading the test script");
        scripts
            .startup()
            .commands
            .into_iter()
            .find_map(|command| match command {
                Command::AutoReload(settings) => Some(settings),
                _ => None,
            })
            .expect("one Command::AutoReload")
    };

    assert_eq!(
        configured("sol.auto_reload({ automatic = false })"),
        crate::autoreload::Settings {
            automatic: false,
            ..crate::autoreload::Settings::default()
        }
    );
    assert_eq!(
        configured("sol.auto_reload({ quiet_ms = 50 })"),
        crate::autoreload::Settings {
            quiet_ms: 50,
            ..crate::autoreload::Settings::default()
        }
    );
    // Said nothing, two ways, and said something of the wrong kind.
    for source in [
        "sol.auto_reload()",
        "sol.auto_reload({})",
        r#"sol.auto_reload({ automatic = "yes", quiet_ms = "soon" })"#,
    ] {
        assert_eq!(
            configured(source),
            crate::autoreload::Settings::default(),
            "{source:?}"
        );
    }

    let _ = std::fs::remove_dir_all(&directory);
}

/// `sol.x11`'s own parsing (#221): a table of strings for `hidden`, an
/// absent table or key keeping the default, and a value of the wrong
/// kind named in the log with the default kept, the same contract
/// `sol_lock_parses_command_and_before_sleep` checks for `sol.lock`.
#[test]
fn sol_x11_parses_hidden_as_a_list_of_strings() {
    let directory = std::env::temp_dir().join("solium-script-test-x11");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let config = directory.join("init.lua");
    let configured = |source: &str| {
        std::fs::write(&config, source).expect("writing the test script");
        let mut scripts = Scripts::load(&config).expect("loading the test script");
        scripts
            .startup()
            .commands
            .into_iter()
            .find_map(|command| match command {
                Command::X11(settings) => Some(settings),
                _ => None,
            })
            .expect("one Command::X11")
    };

    assert_eq!(
        configured(r#"sol.x11({ hidden = { "my-tray-helper", "another-one" } })"#),
        crate::xwayland::Settings {
            hidden: vec!["my-tray-helper".to_owned(), "another-one".to_owned()],
        },
        "a configured list replaces the default rather than adding to it"
    );
    assert_eq!(
        configured(r#"sol.x11({ hidden = {} })"#),
        crate::xwayland::Settings { hidden: vec![] },
        "an empty list is a real answer: hide nothing at all"
    );
    // Said nothing, three ways, and said something of the wrong kind.
    for source in [
        "sol.x11()",
        "sol.x11({})",
        r#"sol.x11({ hidden = "xwaylandvideobridge" })"#,
        "sol.x11({ hidden = { 5, 6 } })",
    ] {
        assert_eq!(
            configured(source),
            crate::xwayland::Settings::default(),
            "{source:?}"
        );
    }

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn sol_toplevels_reads_each_protocol_as_a_flag() {
    use crate::toplevels::{Settings, WindowCapture};
    let directory = std::env::temp_dir().join("solium-script-test-toplevels");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let config = directory.join("init.lua");
    let configured = |source: &str| {
        std::fs::write(&config, source).expect("writing the test script");
        let mut scripts = Scripts::load(&config).expect("loading the test script");
        scripts
            .startup()
            .commands
            .into_iter()
            .find_map(|command| match command {
                Command::Toplevels(settings) => Some(settings),
                _ => None,
            })
            .expect("one Command::Toplevels")
    };
    assert_eq!(
        configured("sol.toplevels({ list = false, manage = true })"),
        Settings {
            list: false,
            manage: true,
            ..Settings::default()
        }
    );
    assert_eq!(
        configured("sol.toplevels({ manage = false })"),
        Settings {
            list: true,
            manage: false,
            ..Settings::default()
        },
        "one left out keeps its default"
    );
    for (written, capture) in [
        ("\"pane\"", WindowCapture::Pane),
        ("\"client\"", WindowCapture::Client),
        ("false", WindowCapture::Off),
        ("\"frame\"", WindowCapture::Client),
        ("true", WindowCapture::Client),
    ] {
        assert_eq!(
            configured(&format!("sol.toplevels({{ capture = {written} }})")).capture,
            capture,
            "capture = {written}"
        );
    }
    assert_eq!(
        configured(r#"sol.toplevels({ list = "no", manage = 0 })"#),
        Settings::default(),
        "a word or a number is no flag"
    );
    assert_eq!(configured("sol.toplevels()"), Settings::default());
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn sol_frames_reads_the_policy_and_its_rules() {
    use crate::effect::rules::Glob;
    use crate::framing::{Policy, Rule, Settings};
    let directory = std::env::temp_dir().join("solium-script-test-frames");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let config = directory.join("init.lua");
    let configured = |source: &str| {
        std::fs::write(&config, source).expect("writing the test script");
        let mut scripts = Scripts::load(&config).expect("loading the test script");
        scripts
            .startup()
            .commands
            .into_iter()
            .find_map(|command| match command {
                Command::Frames(settings) => Some(settings),
                _ => None,
            })
            .expect("one Command::Frames")
    };

    assert_eq!(
        configured(
            r#"sol.frames({ policy = "force", rules = {
                { match = { app_id = "org.gnome.*" }, policy = "prefer" },
                { match = { app_id = "firefox" }, policy = "allow" },
            } })"#
        ),
        Settings {
            policy: Policy::Force,
            rules: vec![
                Rule {
                    app_id: Glob::parse("org.gnome.*"),
                    policy: Policy::Prefer,
                },
                Rule {
                    app_id: Glob::parse("firefox"),
                    policy: Policy::Allow,
                },
            ],
        }
    );
    assert_eq!(
        configured(r#"sol.frames({ policy = "allow" })"#),
        Settings {
            policy: Policy::Allow,
            rules: Vec::new(),
        }
    );
    for broken in [
        r#"sol.frames({ policy = "server" })"#,
        "sol.frames({ policy = 1 })",
        r#"sol.frames({ rules = "force" })"#,
        r#"sol.frames({ rules = { { policy = "force" } } })"#,
        r#"sol.frames({ rules = { { match = { app_id = "x" } } } })"#,
        r#"sol.frames({ rules = { { match = { app_id = 5 }, policy = "force" } } })"#,
        r#"sol.frames({ policy = "force", rules = { { match = { app_id = "x" }, policy = "both" } } })"#,
    ] {
        assert_eq!(
            configured(broken),
            Settings::default(),
            "{broken} was not refused whole"
        );
    }
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn sol_layer_shell_reads_input_below_as_a_flag_or_a_list_of_namespaces() {
    use crate::effect::rules::Glob;
    use crate::layer::{InputBelow, Settings};
    let directory = std::env::temp_dir().join("solium-script-test-layer-shell");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let config = directory.join("init.lua");
    let configured = |source: &str| {
        std::fs::write(&config, source).expect("writing the test script");
        let mut scripts = Scripts::load(&config).expect("loading the test script");
        scripts
            .startup()
            .commands
            .into_iter()
            .find_map(|command| match command {
                Command::LayerShell(settings) => Some(settings),
                _ => None,
            })
            .expect("one Command::LayerShell")
    };
    let below = |input_below| Settings { input_below };

    assert_eq!(
        configured("sol.layer_shell({ input_below = true })"),
        below(InputBelow::Every)
    );
    assert_eq!(
        configured("sol.layer_shell({ input_below = false })"),
        below(InputBelow::Nothing)
    );
    assert_eq!(
        configured(r#"sol.layer_shell({ input_below = { "desktop", "dock*", "*" } })"#),
        below(InputBelow::Named(vec![
            Glob::parse("desktop"),
            Glob::parse("dock*"),
            Glob::Any,
        ])),
        "each name is a namespace, a prefix or every one"
    );
    assert_eq!(
        configured("sol.layer_shell({ input_below = {} })"),
        below(InputBelow::Named(Vec::new())),
        "an empty list names none"
    );
    for broken in [
        "sol.layer_shell({ input_below = 5 })",
        r#"sol.layer_shell({ input_below = { "desktop", 5 } })"#,
        "sol.layer_shell({})",
        "sol.layer_shell()",
    ] {
        assert_eq!(
            configured(broken),
            Settings::default(),
            "{broken} kept the default"
        );
    }
    let _ = std::fs::remove_dir_all(&directory);
}

/// The shipped `config.lua` and `init.lua` tell the session and start
/// autostart, and `--check` knows both keys. Shipped files only: the
/// entry points `package.path` at them, past the developer's own
/// `~/.config/solium`.
#[test]
fn the_shipped_configuration_tells_the_session_and_starts_autostart() {
    let shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua");
    let directory = std::env::temp_dir().join("solium-script-test-session-shipped");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let entry = directory.join("init.lua");
    let load = |source: String| {
        std::fs::write(&entry, source).expect("writing the entry point");
        Scripts::load(&entry).expect("loading the shipped scripts")
    };

    let scripts = load(format!(
        "package.path = {shipped:?} .. \"/?.lua\"\ndofile({shipped:?} .. \"/init.lua\")\n"
    ));
    assert_eq!(scripts.session(), crate::session::Settings::default());

    std::fs::write(
        directory.join("user.lua"),
        "return { session = { systemd = false, autostart = false, stop_timeout = 900, \
         autostrat = true } }",
    )
    .expect("writing the user file");
    let scripts = load(format!(
        "package.path = {here:?} .. \"/?.lua;\" .. {shipped:?} .. \"/?.lua\"\n\
         sol.session(require(\"config\").session)\n",
        here = directory.to_string_lossy(),
    ));
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(
        scripts.session(),
        crate::session::Settings {
            systemd: false,
            autostart: false,
            stop_timeout: Duration::from_millis(900),
        }
    );
    let unknown: Vec<String> = scripts
        .unknown_settings()
        .into_iter()
        .map(|setting| setting.key)
        .collect();
    assert_eq!(unknown, vec!["session.autostrat".to_owned()]);
}

/// The shipped `config.lua` asks for `auto` and a five second probe, through
/// the shipped `init.lua`'s own call.
#[test]
fn the_shipped_configuration_asks_for_auto() {
    let Some(own) = Scripts::user_config_dir() else {
        return;
    };
    if own.join("user.lua").exists() || own.join("config.lua").exists() {
        return;
    }
    let directory = std::env::temp_dir().join("solium-script-test-qml-default");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        local config = require("config")
        sol.qml(config.qml)
        "#,
    )
    .expect("writing the test script");

    let scripts = Scripts::load(&config).expect("loading the test script");
    assert_eq!(
        scripts.qml(),
        crate::qml::renderer::Configured {
            renderer: Some("auto".to_owned()),
            probe_timeout: Some(crate::qml::renderer::PROBE_TIMEOUT),
            buffers: Some(crate::qml::buffers::Buffers::default()),
        }
    );
    let shipped = std::fs::read_to_string(crate::assets::lua().join("init.lua"))
        .expect("the shipped init.lua");
    assert!(
        shipped.contains("sol.qml(config.qml)"),
        "the shipped init.lua does not hand qml to the compositor"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// **The shipped `config.lua` says nothing about the cursor, and that is
/// the setting.**
///
/// The one property of this configuration surface that a plausible-looking
/// edit would destroy without failing anything else. `cursor = {}` is not
/// an empty placeholder waiting to be filled in with sensible defaults:
/// writing `cursor = { size = 24 }` there would make the configuration
/// *always* have an opinion, `XCURSOR_SIZE` would never be reached, and
/// the precedence tests in `cursor::theme` would all still pass because
/// they never read this file.
///
/// Runs the shipped `config.lua` rather than a copy, the same way the
/// old-key test above does and with the same guard on a real
/// `~/.config/solium`, which would otherwise answer first.
#[test]
fn the_shipped_configuration_leaves_the_cursor_to_the_environment() {
    let Some(own) = Scripts::user_config_dir() else {
        return;
    };
    if own.join("user.lua").exists() || own.join("config.lua").exists() {
        return;
    }

    let directory = std::env::temp_dir().join("solium-script-test-cursor-default");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        local config = require("config")
        sol.bind("Super+C", function()
            sol.cursor_theme(config.cursor)
        end)
        "#,
    )
    .expect("writing the test script");

    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let outcome = scripts.key("super+c", empty_snapshot());
    assert!(outcome.handled);
    match outcome.commands.as_slice() {
        [Command::Cursor(configured)] => assert_eq!(
            configured,
            &crate::cursor::theme::Configured::default(),
            "the shipped config.lua names a cursor theme or size, so XCURSOR_THEME and \
             XCURSOR_SIZE can never win"
        ),
        other => panic!("expected one cursor command, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&directory);
}

/// **A configuration with no `cursor` key at all still loads.**
///
/// The documented way to change one setting is a single
/// `~/.config/solium/config.lua`, and shipped `init.lua` calls
/// `sol.cursor_theme(config.cursor)` against it. A copy written before this
/// setting existed — which is every copy made before #81 — has no `cursor`
/// key, so that call is `sol.cursor_theme(nil)`. Taking a `Table` made that
/// an error at load, and an error at load is not a pointer that falls back:
/// it is *no configuration*, no layouts, no bindings and no decorations,
/// over a setting the user never heard of.
///
/// The same class of break as `two_functions_cannot_share_one_name` below,
/// and the same shape of guard. `sol.keyboard` and `sol.monitors` take an
/// optional table for exactly this reason.
///
/// Asserted at load *and* in a binding, because those are two different
/// call sites with two different consequences: the one in `init.lua` runs
/// while the configuration is being built, and the one under a key runs
/// after.
#[test]
fn a_configuration_that_says_nothing_about_the_cursor_still_loads() {
    let directory = std::env::temp_dir().join("solium-script-test-cursor-nil");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        -- A user's own config.lua, written before the cursor setting
        -- existed: no `cursor` key, so this is sol.cursor_theme(nil).
        local config = { pane = {} }
        sol.cursor_theme(config.cursor)
        sol.bind("Super+C", function()
            sol.cursor_theme(config.cursor)
            sol.status("alive")
        end)
        "#,
    )
    .expect("writing the test script");

    let mut scripts =
        Scripts::load(&config).expect("a configuration with no cursor key failed to load at all");
    let outcome = scripts.key("super+c", empty_snapshot());
    assert!(outcome.handled);
    assert_eq!(outcome.status.as_deref(), Some("alive"));
    match outcome.commands.as_slice() {
        [Command::Cursor(configured)] => assert_eq!(
            configured,
            &crate::cursor::theme::Configured::default(),
            "no table is not 'reset it': it means the configuration did not say, so \
             XCURSOR_THEME and XCURSOR_SIZE still get their turn"
        ),
        other => panic!("expected one cursor command, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&directory);
}

/// And a `size` that is not a whole number is ignored rather than fatal.
///
/// `Settings::resolve` is the one place that decides what a size *out of
/// range* means, and it is handed the number to decide about. A value that
/// is not a number is a different thing: mlua answers an `Option<i32>` with
/// a conversion *error* for `"big"` and for `24.5`, and a `?` on it would
/// take the whole configuration down — the same failure as the missing
/// table above, reached through a typo in one field. The `theme` key eight
/// lines above has always read this way; this makes the pair agree.
#[test]
fn a_cursor_size_that_is_not_a_number_is_ignored_and_not_fatal() {
    let directory = std::env::temp_dir().join("solium-script-test-cursor-size");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        sol.bind("Super+C", function()
            sol.cursor_theme({ theme = "Fixture", size = "big" })
        end)
        sol.bind("Super+D", function()
            sol.cursor_theme({ theme = "Fixture", size = 24.5 })
        end)
        "#,
    )
    .expect("writing the test script");

    let mut scripts = Scripts::load(&config).expect("loading the test script");
    for key in ["super+c", "super+d"] {
        let outcome = scripts.key(key, empty_snapshot());
        assert!(
            outcome.handled,
            "{key}: the listener failed, so the whole configuration went with it"
        );
        match outcome.commands.as_slice() {
            [Command::Cursor(configured)] => {
                assert_eq!(
                    configured.theme.as_deref(),
                    Some("Fixture"),
                    "{key}: the rest of the table was lost with the bad size"
                );
                assert_eq!(
                    configured.size, None,
                    "{key}: a size that is not a whole number reached the compositor"
                );
            }
            other => panic!("{key}: expected one cursor command, got {other:?}"),
        }
    }

    let _ = std::fs::remove_dir_all(&directory);
}

/// **A configuration that fails to load names its file and line** as
/// `<path>:<line>:`, the form every other Lua error in Solium takes, so
/// a failed reload can be put on the overlay where it is
/// (`effect::host::config_problem`). Named as a bare string, the chunk
/// read `[string "<path>"]:2:`.
#[test]
fn a_configuration_error_names_its_file_and_line() {
    let directory =
        std::env::temp_dir().join(format!("solium-config-error-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(&config, "sol.pane('none')\nthis is not lua\n")
        .expect("writing the test script");
    let error = Scripts::load(&config)
        .err()
        .map(|err| format!("{err:#}"))
        .expect("a configuration that is not Lua fails to load");
    let _ = std::fs::remove_dir_all(&directory);
    assert!(
        error.contains(&format!("{}:2: ", config.display())),
        "{error}"
    );
}

/// **`sol.effects` reaches the compositor as rules**, parsed.
#[test]
fn sol_effects_reaches_the_compositor_as_rules() {
    let directory =
        std::env::temp_dir().join(format!("solium-effects-rules-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"sol.effects({ rules = { { match = { app_id = "mpv" }, part = "client", slot = "replace", effect = { "blur", source = "self" } } } })"#,
    )
    .expect("writing");
    let mut scripts = Scripts::load(&config).expect("loading");
    let commands = scripts.startup().commands;
    let _ = std::fs::remove_dir_all(&directory);
    let rules = commands.iter().find_map(|command| match command {
        Command::Effects { rules, .. } => Some(rules.clone()),
        _ => None,
    });
    assert_eq!(rules.expect("sent").expect("parsed").len(), 1);
}

/// **`sol.effects` reaches the compositor with its settings**: the
/// engine's keys beside the rules, parsed; a key out of its bounds is
/// the set's error; no options at all are the defaults.
#[test]
fn sol_effects_reaches_the_compositor_with_its_settings() {
    let directory =
        std::env::temp_dir().join(format!("solium-effects-settings-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        "sol.effects({ rules = {}, sandbox = { load_ms = 250 }, present = { failed = 'hide' } })\n\
         sol.effects({ limits = { params = 99 } })\nsol.effects()\n",
    )
    .expect("writing");
    let mut scripts = Scripts::load(&config).expect("loading");
    let commands = scripts.startup().commands;
    let _ = std::fs::remove_dir_all(&directory);
    let sets: Vec<_> = commands
        .into_iter()
        .filter_map(|command| match command {
            Command::Effects { settings, .. } => Some(settings),
            _ => None,
        })
        .collect();
    let [set, refused, none] = sets.as_slice() else {
        panic!("three sets: {sets:?}");
    };
    let set = set.as_ref().expect("parsed");
    assert_eq!(set.sandbox.load, std::time::Duration::from_millis(250));
    assert_eq!(
        set.present.failed,
        crate::effect::settings::PresentFailed::Hide
    );
    assert!(
        refused
            .as_ref()
            .is_err_and(|error| error.contains("effects.limits.params")),
        "{refused:?}"
    );
    assert_eq!(none, &Ok(crate::effect::settings::Settings::default()));
}

/// **`sol.effects` reads `xray` and `memo`** (Rulings F29, F30; amended
/// 2026-10-08, configurable): both reach the compositor in its settings,
/// parsed; a word outside `split`'s words is the set's error, naming it;
/// and a number that is not finite is refused by name rather than
/// dropped for the default, as `effect::settings::tree_of` keeps it in
/// both tables and in their lists, so `budget_mib = 1/0` is not "no
/// cap", `scale = 0/0` is not 1, and `layers = { "background", 0/0 }`
/// is not the wallpaper alone.
#[test]
fn sol_effects_reads_xray_and_memo() {
    let directory =
        std::env::temp_dir().join(format!("solium-effects-xray-memo-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        "sol.effects({ xray = { keep_ms = 0 }, memo = { split = 'off' } })\n\
         sol.effects({ memo = { split = 'short' } })\n\
         sol.effects({ memo = { budget_mib = 1/0 } })\n\
         sol.effects({ xray = { scale = 0/0 } })\n\
         sol.effects({ xray = { keep_ms = -1/0 } })\n\
         sol.effects({ xray = 1/0 })\n\
         sol.effects({ xray = { layers = { 0/0 } } })\n\
         sol.effects({ xray = { layers = { 'background', 0/0 } } })\n",
    )
    .expect("writing");
    let mut scripts = Scripts::load(&config).expect("loading");
    let commands = scripts.startup().commands;
    let _ = std::fs::remove_dir_all(&directory);
    let sets: Vec<_> = commands
        .into_iter()
        .filter_map(|command| match command {
            Command::Effects { settings, .. } => Some(settings),
            _ => None,
        })
        .collect();
    let [read, refused @ ..] = sets.as_slice() else {
        panic!("eight sets: {sets:?}");
    };
    let read = read.as_ref().expect("parsed");
    assert_eq!(
        (read.xray.keep_ms, read.memo.split),
        (0, solium_effects::stage::Split::Off)
    );
    assert_eq!(
        (read.xray.scale, read.memo.budget_mib),
        (1.0, 0),
        "what was not said is the default"
    );
    let says = [
        "`effects.memo.split`",
        "`effects.memo.budget_mib`",
        "`effects.xray.scale`",
        "`effects.xray.keep_ms`",
        "`effects.xray` is a table",
        "`effects.xray.layers` is a list of words",
        "`effects.xray.layers` is a list of words",
    ];
    assert_eq!(refused.len(), says.len(), "{refused:?}");
    for (refused, says) in refused.iter().zip(says) {
        assert!(
            refused.as_ref().is_err_and(|error| error.contains(says)),
            "{says}: {refused:?}"
        );
    }
}

/// **`sol.startup` reaches the compositor**: `first_frame` read and
/// checked into `Command::Startup`, with no table the default, and a
/// refused value, a number that is not finite or an unknown key an
/// `Err` naming what is wrong.
#[test]
fn sol_startup_reaches_the_compositor() {
    let directory = std::env::temp_dir().join(format!("solium-startup-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        "sol.startup({ first_frame = { max_ms = 500 } })\n\
         sol.startup()\n\
         sol.startup({ first_frame = { max_ms = -1 } })\n\
         sol.startup({ first_frame = { max_ms = 1/0 } })\n\
         sol.startup({ frist_frame = {} })\n\
         sol.startup({ first_frame = 5 })\n",
    )
    .expect("writing");
    let mut scripts = Scripts::load(&config).expect("loading");
    let commands = scripts.startup().commands;
    let _ = std::fs::remove_dir_all(&directory);
    let read: Vec<_> = commands
        .into_iter()
        .filter_map(|command| match command {
            Command::Startup(first_frame) => Some(first_frame),
            _ => None,
        })
        .collect();
    let [given, none, refused @ ..] = read.as_slice() else {
        panic!("six calls: {read:?}");
    };
    assert_eq!(
        given,
        &Ok(crate::warm::FirstFrame {
            max_ms: 500,
            ..crate::warm::FirstFrame::default()
        })
    );
    assert_eq!(none, &Ok(crate::warm::FirstFrame::default()));
    let says = [
        "`startup.first_frame.max_ms`",
        "`startup.first_frame.max_ms`",
        "did you mean `first_frame`?",
        "`startup.first_frame` is a table",
    ];
    assert_eq!(refused.len(), says.len(), "{refused:?}");
    for (refused, says) in refused.iter().zip(says) {
        assert!(
            refused.as_ref().is_err_and(|error| error.contains(says)),
            "{says}: {refused:?}"
        );
    }
}

/// **A broken rule reaches the compositor as its errors**, so the
/// compositor can keep the rules that ran and list them; and
/// `sol.effects{}` with no `rules` is an empty set, not an error.
#[test]
fn a_broken_rule_reaches_the_compositor_as_its_errors() {
    let directory =
        std::env::temp_dir().join(format!("solium-effects-broken-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        "sol.effects({ rules = { { match = '*', part = 'cleint', slot = 'behind', effect = false } } })\nsol.effects({})\n",
    )
    .expect("writing");
    let mut scripts = Scripts::load(&config).expect("loading");
    let commands = scripts.startup().commands;
    let _ = std::fs::remove_dir_all(&directory);
    let sets: Vec<_> = commands
        .into_iter()
        .filter_map(|command| match command {
            Command::Effects { rules, .. } => Some(rules),
            _ => None,
        })
        .collect();
    let [broken, empty] = sets.as_slice() else {
        panic!("two sets: {sets:?}");
    };
    let errors = broken.as_ref().expect_err("refused");
    assert_eq!((errors[0].rule, errors[0].key), (1, "part"));
    assert_eq!(empty, &Ok(Vec::new()));
}

/// **`sol.frame` hands one window its values** (#417): the window's id
/// and the table, as a command, and nothing for anything not a table.
#[test]
fn sol_frame_hands_one_window_its_values() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-frame-values",
        r#"sol.frame(7, { opacity = 0.5, label = "a" })
           sol.status(tostring(pcall(sol.frame, 7, 1)))"#,
    );
    let outcome = scripts.startup();
    let handed: Vec<(u64, String)> = outcome
        .commands
        .iter()
        .filter_map(|command| match command {
            Command::FrameValues { id, fields } => {
                Some((*id, crate::json::Json::Object(fields.clone()).render()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(handed, [(7, r#"{"label":"a","opacity":0.5}"#.to_owned())]);
    assert_eq!(outcome.status.as_deref(), Some("false"));
    let _ = std::fs::remove_dir_all(&directory);
}

/// **`sol.read` stops at `files.max_read`** (#473): the setting
/// `sol.files` hands over is the cap, and past it the function is told
/// so; a value of the wrong kind keeps the default.
#[test]
fn sol_read_stops_at_files_max_read() {
    let directory = std::env::temp_dir().join("solium-script-test-read-cap");
    let _ = std::fs::create_dir_all(&directory);
    std::fs::write(directory.join("ten"), "0123456789").expect("writing");
    let ten = directory.join("ten");
    let (_, mut scripts) = loaded(
        "solium-script-test-read-cap",
        &format!(
            r#"seen = {{}}
               local function note(data, err) seen[#seen + 1] = tostring(data) .. " " .. tostring(err) end
               sol.files({{ max_read = 4 }})
               sol.read("{ten}", note)
               sol.files({{ max_read = 10 }})
               sol.read("{ten}", note)
               sol.files({{ max_read = "lots" }})
               sol.read("{ten}", function() end)"#,
            ten = ten.display()
        ),
    );
    let outcome = scripts.startup();
    let caps: Vec<usize> = outcome
        .commands
        .iter()
        .filter_map(|command| match command {
            Command::FileRead(request) => Some(request.max),
            _ => None,
        })
        .collect();
    assert_eq!(caps, [4, 10, crate::script::lua::MAX_READ]);
    let mut files = crate::files::Files::default();
    play_files(&mut files, &outcome);
    assert_eq!(
        pump_files(&mut scripts, &mut files, 2),
        ["0123456789 nil", "nil longer than files.max_read"]
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **`sol.files` reads the store settings** (#473): `store_interval` is
/// how often the compositor asks for the stores, handed to it as a setting,
/// and `store_max` caps a store file; values of the wrong kind keep the
/// defaults.
#[test]
fn sol_files_reads_the_store_settings() {
    let interval = |scripts: &mut Scripts| {
        scripts
            .startup()
            .commands
            .into_iter()
            .find_map(|command| match command {
                Command::StoreInterval(every) => Some(every),
                _ => None,
            })
    };
    let state = crate::files::tests::scratch("script-store-settings");
    crate::files::tests::use_state_dir(&state);
    std::fs::write(state.join("big.json"), r#"{"a": "0123456789"}"#).expect("writing");
    let (directory, mut scripts) = loaded(
        "solium-script-test-store-settings",
        r#"sol.files({ store_interval = 5000, store_max = 10 })
           big = sol.store("big", { a = "default" })"#,
    );
    assert_eq!(interval(&mut scripts), Some(Duration::from_secs(5)));
    assert_eq!(scripts.evaluate("return big.a"), "default");
    assert!(
        state.join("big.json.broken").exists(),
        "past store_max was read"
    );
    let _ = std::fs::remove_dir_all(&directory);
    let (directory, mut scripts) = loaded(
        "solium-script-test-store-settings-wrong",
        "sol.files({ store_interval = 0, store_max = -1 })",
    );
    assert_eq!(interval(&mut scripts), Some(crate::files::STORE_INTERVAL));
    let _ = std::fs::remove_dir_all(&directory);
    let _ = std::fs::remove_dir_all(&state);
}
