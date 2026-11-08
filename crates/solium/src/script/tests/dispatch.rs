//! Events, the deadline and what a dispatch answers.

use super::*;

/// **A rect with a negative size, or a side that is not a number, is
/// refused by name (#282)**, rather than reaching the renderer, which
/// panicked on a negative size. Zero is a size.
#[test]
fn a_rect_with_a_negative_or_non_finite_side_is_refused() {
    let lua = mlua::Lua::new();
    let rect = |source: &str| {
        let table: mlua::Table = lua.load(source).eval().expect("a table");
        super::rect_from(&table)
    };
    assert!(rect("return { x = 0, y = 0, w = 0, h = 10 }").is_ok());
    for bad in [
        "return { x = 0, y = 0, w = -18, h = 13 }",
        "return { x = 0, y = 0, w = 10, h = -1 }",
        "return { x = 0/0, y = 0, w = 10, h = 10 }",
        "return { x = 0, y = 0, w = 1/0, h = 10 }",
    ] {
        let refused = rect(bad).expect_err(bad).to_string();
        assert!(
            refused.contains("a size of zero or more"),
            "{bad}: {refused}"
        );
    }
}

/// **`motion` times a present's properties apart, and anything else
/// under it is refused by name** (#293). A property left out, and a
/// duration or easing left out of one, is left for the batch's own.
#[test]
fn a_present_motion_names_its_properties_and_a_bad_one_is_refused() {
    let lua = mlua::Lua::new();
    let motion = |source: &str| {
        let table: mlua::Table = lua.load(source).eval().expect("a table");
        super::present_motion_from(&table)
    };
    assert_eq!(motion("return {}").expect("no motion"), None);
    let timed = motion(
        "return { motion = { x = { duration = 300, easing = 'linear' }, \
         opacity = { duration = 450 } } }",
    )
    .expect("a motion")
    .expect("one was given");
    assert_eq!(
        timed.x,
        (Some(Duration::from_millis(300)), Some(Curve::Linear))
    );
    assert_eq!(timed.y, (None, None), "left out, so the batch's");
    assert_eq!(timed.size, (None, None));
    assert_eq!(timed.opacity, (Some(Duration::from_millis(450)), None));
    assert_eq!(timed.client_opacity, (None, None));
    for (bad, says) in [
        ("return { motion = 3 }", "a table of timings by property"),
        ("return { motion = { colour = {} } }", "not colour"),
        ("return { motion = { [1] = {} } }", "a key of type integer"),
        (
            "return { motion = { x = 300 } }",
            "`motion.x` is { duration, easing }",
        ),
    ] {
        let refused = motion(bad).expect_err(bad).to_string();
        assert!(refused.contains(says), "{bad}: {refused}");
    }
}

#[test]
fn combos_normalise_to_one_spelling() {
    assert_eq!(normalise_combo("super+space"), "super+space");
    assert_eq!(normalise_combo("Super+Space"), "super+space");
    assert_eq!(normalise_combo("shift+super+q"), "shift+super+q");
    // The whole point: written the other way round, still the same binding.
    assert_eq!(
        normalise_combo("super+shift+q"),
        normalise_combo("shift+super+q")
    );
    assert_eq!(normalise_combo("win+d"), "super+d");
    assert_eq!(normalise_combo("escape"), "escape");
}

#[test]
fn a_repeated_modifier_is_not_repeated() {
    assert_eq!(normalise_combo("ctrl+ctrl+c"), "ctrl+c");
}

/// What `sol.window_at` asks of a window, [`crate::state::owns`], which
/// was `Rect::contains` on the drawn rectangle until #134's fourth review.
#[test]
fn a_drawn_window_covers_its_own_top_left_but_not_its_bottom_right() {
    let drawn = Rectangle::<f64, Logical>::new((10.0, 20.0).into(), (100.0, 50.0).into());
    let slot = drawn.to_i32_round();
    let frame = Frame {
        rect: drawn,
        ..Frame::real(slot)
    };
    let covers = |x: f64, y: f64| crate::state::owns(slot, false, frame, (x, y).into(), &[]);
    assert!(covers(10.0, 20.0));
    assert!(covers(109.0, 69.0));
    // Exclusive, so adjacent thumbnails cannot both claim the same pixel.
    assert!(!covers(110.0, 70.0));
    assert!(!covers(9.0, 20.0));
}

/// **A list in `config.bindings` can ask to repeat and to fire at the
/// lock** (#224), with the named fields beside the command and not in it;
/// and a binding there replaces the shipped one whole, so a volume key
/// rebound as a string asks for nothing.
#[test]
fn a_user_file_binding_can_ask_to_repeat_and_to_fire_at_the_lock() {
    let Some((directory, mut scripts)) = shipped_init_with_user(
        "solium-script-test-user-binding-options",
        r#"
        return {
            bindings = {
                ["XF86AudioRaiseVolume"] = {
                    "fixture-mixer", "up", repeating = true, locked = true,
                },
                ["XF86AudioLowerVolume"] = "fixture-mixer down",
                ["super+f5"] = { "fixture-browser" },
            },
        }
        "#,
    ) else {
        return;
    };

    let raised = scripts.key("xf86audioraisevolume", empty_snapshot());
    assert_eq!(
        spawns(&raised),
        vec![("fixture-mixer".to_owned(), vec!["up".to_owned()])],
        "the options became part of the command line"
    );
    let asked = |combo: &str| {
        scripts
            .bindings()
            .into_iter()
            .find(|binding| binding.combo == combo)
            .map(|binding| (binding.repeating, binding.locked))
    };
    assert_eq!(
        [
            asked("xf86audioraisevolume"),
            asked("xf86audiolowervolume"),
            asked("super+f5"),
        ],
        [
            Some((true, true)),
            Some((false, false)),
            Some((false, false))
        ],
        "(repeating, locked) for: the list that asked, the string that replaced a \
         shipped repeating key, the list that asked for nothing"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// **An option a `config.bindings` list does not have is refused**, naming
/// the binding, as a binding of any other wrong shape is: one that asks for
/// something it does not get looks exactly like one that is broken.
#[test]
fn a_user_file_binding_with_an_option_it_does_not_have_is_refused() {
    for (case, entry, says) in [
        (
            "repeat",
            r#"{ "fixture-mixer", ["repeat"] = true }"#,
            "repeating",
        ),
        ("unknown", r#"{ "fixture-mixer", loud = true }"#, "loud"),
        (
            "not-a-boolean",
            r#"{ "fixture-mixer", locked = "yes" }"#,
            "locked",
        ),
    ] {
        let Some((directory, loaded)) = shipped_init_and_user(
            &format!("solium-script-test-user-binding-refused-{case}"),
            &format!("return {{ bindings = {{ [\"super+f5\"] = {entry} }} }}\n"),
        ) else {
            return;
        };
        let error = loaded
            .err()
            .map(|error| format!("{error:#}"))
            .unwrap_or_default();
        assert!(
            error.contains("super+f5") && error.contains(says),
            "{case}: not refused naming the binding and {says:?}: {error:?}"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }
}

/// **`open.motion` and `open.scale` decide the open animation.**
///
/// Both were documented in `config.lua` and read by nothing: `open.lua`
/// held `220 / outBack` and `0.88` as local constants while the
/// configuration advertised `200 / outCubic` and `0.92`, so editing either
/// setting did nothing and there was no way to tell that apart from having
/// misunderstood what it meant (#117).
///
/// The numbers in the fixture are deliberately unlike both pairs, so this
/// cannot pass by agreeing with whichever set of constants happens to be in
/// the file.
#[test]
fn the_open_animation_is_the_one_the_configuration_names() {
    let Some((directory, mut scripts)) = shipped_init_with_user(
        "solium-script-test-open-animation",
        r#"
        return {
            open = {
                motion = { duration = 777, easing = "linear" },
                scale = 0.5,
            },
        }
        "#,
    ) else {
        return;
    };

    // 800x600 at the origin, from `one_screen`. At a scale of 0.5 the
    // window starts 400x300 about its own centre, so at 200,150.
    let outcome = scripts.opened(7, one_screen(&[7]));
    let from = outcome
        .commands
        .iter()
        .find_map(|command| match command {
            Command::PresentFrom {
                id: 7,
                rect,
                animation,
                ..
            } => Some((*rect, *animation)),
            _ => None,
        })
        .expect("the open animation did not run");

    assert_eq!(
        from.1.duration,
        Duration::from_millis(777),
        "open.motion.duration is not what the window arrives with"
    );
    assert_eq!(
        from.1.easing,
        Curve::Linear,
        "open.motion.easing is not what the window arrives with"
    );
    assert!(
        (from.0.w - 400.0).abs() < 0.5 && (from.0.h - 300.0).abs() < 0.5,
        "open.scale did not decide how small the window starts: {:?}",
        from.0
    );
    assert!(
        (from.0.x - 200.0).abs() < 0.5 && (from.0.y - 150.0).abs() < 0.5,
        "and it is still shrunk about its own centre: {:?}",
        from.0
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// **`scrolling.widths` and `scrolling.default_width` decide how wide a
/// column opens.**
///
/// The other two settings that were documented and read by nothing: the
/// strip used the constant list in `crates/layout/src/scroller.rs`, so a
/// configuration asking for quarters got thirds (#117).
///
/// Driven through the shipped `scrolling.lua` rather than by calling
/// `sol.layout.scroller` from a fixture, because the missing link was the
/// *call site* -- the constructor took no argument, and no test of the
/// layout crate could have seen that. 0.8 of a 1600-wide screen is
/// unmistakably neither of the shipped presets near it.
#[test]
fn the_configured_widths_are_what_a_column_opens_at() {
    let Some((directory, mut scripts)) = shipped_init_with_user(
        "solium-script-test-scrolling-widths",
        r#"
        return {
            gap = 0,
            scrolling = { widths = { 0.25, 0.8 }, default_width = 2 },
        }
        "#,
    ) else {
        return;
    };

    scripts.monitors_changed(one_screen(&[]));
    // super+s is the shipped binding that puts the scrolling layout in
    // charge; without it the strip is built but nothing places anything.
    scripts.key("super+s", one_screen(&[]));
    let outcome = scripts.opened(7, one_screen(&[7]));

    let placed = outcome
        .commands
        .iter()
        .rev()
        .find_map(|command| match command {
            Command::Place { id: 7, rect, .. } => Some(*rect),
            _ => None,
        })
        .expect("the scrolling layout placed nothing");
    assert!(
        (placed.w - 1280.0).abs() < 1.0,
        "a new column did not open at `default_width` of `widths` -- 0.8 of a \
         1600-wide view is 1280, and the shipped presets would give 533, 800 or \
         1066: {placed:?}"
    );

    // And the cycle is over the configured list, not the shipped one: one
    // step from the second of two wraps to the first.
    let cycled = scripts.key("super+r", one_screen(&[7]));
    let after = cycled
        .commands
        .iter()
        .rev()
        .find_map(|command| match command {
            Command::Place { id: 7, rect, .. } => Some(*rect),
            _ => None,
        })
        .expect("super+r placed nothing");
    assert!(
        (after.w - 400.0).abs() < 1.0,
        "super+r did not cycle through the configured widths: 0.25 of 1600 is 400, \
         and a lap of a two-entry list is two steps: {after:?}"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// **A key the defaults do not define is reported, not swallowed.**
///
/// `config.lua`'s `merge` validated nothing, so `tilling = { ... }` was
/// merged in as a new section, read by nothing, for ever -- and `solium
/// --check`, the one command whose job is answering "did my configuration
/// work", said it had loaded fine (#117). It had. That was never the
/// question.
///
/// Four fixtures, and the last two are the ones that make this worth
/// having: a check that reports real typos and also reports working
/// settings is a check nobody will keep running.
#[test]
fn an_unrecognised_setting_is_reported_rather_than_merged_in_silence() {
    let Some((directory, scripts)) = shipped_init_with_user(
        "solium-script-test-unknown-settings",
        r#"
        return {
            -- A typo at the top level, and one inside a section.
            tilling = { split = 0.6 },
            open = { scal = 0.5 },
            -- A near miss on the *deprecated* spelling of `pane`. Reported,
            -- because it is not a key anything reads -- and reported with no
            -- suggestion, because the only word within two edits of it is
            -- `decoration`, and answering a typo with a deprecated spelling
            -- walks somebody past the key that actually works (#117 review).
            decoraton = "border",
            -- Not typos, and must not be reported: `keyboard` leaves its
            -- xkb names out on purpose, so they cannot be checked against
            -- the defaults, and a binding combination is whatever you
            -- press. `active`
            -- belongs with a dual layout and is exactly the pair that used
            -- to be called a typo.
            keyboard = { layout = "us,ua", active = 2 },
            bindings = { ["super+f6"] = "fixture-nothing" },
        }
        "#,
    ) else {
        return;
    };

    let found = scripts.unknown_settings();
    let reported: Vec<(&str, Option<&str>)> = found
        .iter()
        .map(|setting| (setting.key.as_str(), setting.meant.as_deref()))
        .collect();
    assert_eq!(
        reported,
        vec![
            ("decoraton", None),
            ("open.scal", Some("open.scale")),
            ("tilling", Some("tiling"))
        ],
        "the unrecognised keys are wrong: a near miss inside a section must be \
         reported by its full path, a near miss on a deprecated name must not be \
         answered with that name, and neither the `keyboard` section nor a binding \
         combination may be called a typo"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// **With nothing configured, no shell is hosted**, and the shipped
/// configuration says so every time it runs rather than by staying quiet.
///
/// The second half is what makes taking a shell out of `user.lua` work on
/// `super+shift+r`: a surface outlives a reload until something removes it
/// by name, and a reload runs the configuration again, so `shell.lua`
/// removes `shell` whenever none is configured.
#[test]
fn the_shipped_configuration_hosts_no_shell_of_its_own() {
    // The environment has already chosen a shell for this process, and
    // that is what it is for; there is nothing for this test to say.
    if std::env::var_os("SOLIUM_SHELL_SCENE").is_some() {
        return;
    }
    let Some((_, commands)) = shell_after_monitors("solium-script-test-no-shell", "return {}")
    else {
        return;
    };
    let (declared, removed) = shell_surfaces(&commands);
    assert!(
        declared.is_empty(),
        "the shipped configuration hosted a shell: {declared:?}"
    );
    assert!(
        removed,
        "with no shell configured, `shell` must be taken away, or one taken out of \
         user.lua stays on screen after a reload"
    );
}

/// **`shell.scene` in `user.lua` hosts that scene**: over the windows,
/// taking the pointer, on every monitor, with `~` expanded, and told
/// nothing by hand about the screen it is on -- and `shell.scene` is a
/// setting, not a typo.
#[test]
fn the_shell_scene_is_read_from_the_configuration() {
    if std::env::var_os("SOLIUM_SHELL_SCENE").is_some() {
        return;
    }
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let Some((scripts, commands)) = shell_after_monitors(
        "solium-script-test-shell-scene",
        r#"return { shell = { scene = "~/solium-fixture-shell/shell.qml" } }"#,
    ) else {
        return;
    };

    let unknown: Vec<String> = scripts
        .unknown_settings()
        .into_iter()
        .map(|setting| setting.key)
        .collect();
    assert!(
        unknown.is_empty(),
        "`shell.scene` was reported as unrecognised: {unknown:?}"
    );

    let (declared, _) = shell_surfaces(&commands);
    let [shell] = declared.as_slice() else {
        panic!("expected one `shell` surface, got {declared:?}");
    };
    assert_eq!(
        shell.scene,
        std::path::PathBuf::from(home).join("solium-fixture-shell/shell.qml"),
        "the scene is not the configured one with `~` expanded"
    );
    assert_eq!(shell.layer, crate::scripted::Layer::Top);
    assert!(shell.interactive, "a shell must take the pointer");
    assert_eq!(
        shell.on,
        crate::scripted::On::EveryMonitor,
        "the shell is not on every monitor"
    );
    assert!(
        shell.properties.get("screenInfo").is_none(),
        "the shell was handed `screenInfo`, which `Solium.monitor` replaces: {:?}",
        shell.properties
    );
}

/// **The shell is on every monitor, unless the configuration names one**:
/// `shell.on` is a setting, `"every-monitor"` by default (#161, Ruling 5).
#[test]
fn the_shell_is_on_every_monitor_unless_the_configuration_names_one() {
    if std::env::var_os("SOLIUM_SHELL_SCENE").is_some() {
        return;
    }
    for (user, expected) in [
        (
            r#"return { shell = { scene = "/solium-fixture/shell.qml" } }"#,
            crate::scripted::On::EveryMonitor,
        ),
        (
            r#"return { shell = { scene = "/solium-fixture/shell.qml", on = "primary" } }"#,
            crate::scripted::On::Primary,
        ),
    ] {
        let Some((scripts, commands)) = shell_after_monitors("solium-script-test-shell-on", user)
        else {
            return;
        };
        assert!(
            scripts.unknown_settings().is_empty(),
            "`shell.on` was reported as unrecognised"
        );
        let (declared, _) = shell_surfaces(&commands);
        let placed: Vec<&crate::scripted::On> = declared.iter().map(|shell| &shell.on).collect();
        assert!(
            !placed.is_empty() && placed.iter().all(|on| **on == expected),
            "{user}: {placed:?}"
        );
    }
}

/// **The hosted shell's outside press is the user's to configure**
/// (Q2, Ruling 13): `shell.outside_click` in `config.lua` is the shell's
/// `outside_click`, a word or a table naming grabs, and swallows by
/// default.
#[test]
fn the_shell_takes_its_outside_click_from_the_configuration() {
    if std::env::var_os("SOLIUM_SHELL_SCENE").is_some() {
        return;
    }
    use crate::scripted::Outside;
    for (user, tray_menu, search) in [
        (
            r#"return { shell = { scene = "/solium-fixture/shell.qml" } }"#,
            Outside::Swallow,
            Outside::Swallow,
        ),
        (
            r#"return { shell = { scene = "/solium-fixture/shell.qml", outside_click = "pass" } }"#,
            Outside::Pass,
            Outside::Pass,
        ),
        (
            r#"return { shell = { scene = "/solium-fixture/shell.qml", outside_click = { default = "swallow", ["tray-menu"] = "pass" } } }"#,
            Outside::Pass,
            Outside::Swallow,
        ),
    ] {
        let Some((scripts, commands)) =
            shell_after_monitors("solium-script-test-shell-outside", user)
        else {
            return;
        };
        assert!(
            scripts.unknown_settings().is_empty(),
            "`shell.outside_click` was reported as unrecognised"
        );
        let (declared, _) = shell_surfaces(&commands);
        let policies: Vec<(Outside, Outside)> = declared
            .iter()
            .map(|shell| {
                (
                    shell.outside_click.for_grab("tray-menu"),
                    shell.outside_click.for_grab("search"),
                )
            })
            .collect();
        assert!(
            !policies.is_empty() && policies.iter().all(|policy| *policy == (tray_menu, search)),
            "{user}: {policies:?}"
        );
    }
}

/// **Which bindings still work while the hosted shell holds the keyboard
/// is the user's to configure** (Q3): `shell.keyboard.bindings` in
/// `config.lua` is the shell's `keyboard.bindings`, and every binding but
/// the claimed keys works by default.
#[test]
fn the_shell_takes_its_keyboard_bindings_from_the_configuration() {
    if std::env::var_os("SOLIUM_SHELL_SCENE").is_some() {
        return;
    }
    use crate::scripted::KeyPolicy;
    for (user, expected) in [
        (
            r#"return { shell = { scene = "/solium-fixture/shell.qml" } }"#,
            KeyPolicy::ExceptClaimed,
        ),
        (
            r#"return { shell = { scene = "/solium-fixture/shell.qml", keyboard = { bindings = "none" } } }"#,
            KeyPolicy::NoBindings,
        ),
        (
            r#"return { shell = { scene = "/solium-fixture/shell.qml", keyboard = { bindings = "all" } } }"#,
            KeyPolicy::All,
        ),
    ] {
        let Some((scripts, commands)) =
            shell_after_monitors("solium-script-test-shell-keyboard", user)
        else {
            return;
        };
        assert!(
            scripts.unknown_settings().is_empty(),
            "`shell.keyboard.bindings` was reported as unrecognised"
        );
        let (declared, _) = shell_surfaces(&commands);
        let policies: Vec<KeyPolicy> = declared.iter().map(|shell| shell.keyboard).collect();
        assert!(
            !policies.is_empty() && policies.iter().all(|policy| *policy == expected),
            "{user}: {policies:?}"
        );
    }
}

/// **`SOLIUM_SHELL_SCENE` wins over `shell.scene`**, because it is set per
/// run -- `dev/run-shell.sh` sets it to the scene being worked on.
///
/// In a child process, because the environment is the process's and the
/// tests share one: setting the variable here would change what every
/// other test running beside this one reads. The child is this binary,
/// running [`shell_scene_under_the_environment`] alone with the variable
/// set.
#[test]
fn the_environment_overrides_the_configured_shell_scene() {
    let binary = std::env::current_exe().expect("this test binary");
    let output = std::process::Command::new(binary)
        .args([
            "--exact",
            "script::tests::dispatch::shell_scene_under_the_environment",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("SOLIUM_SHELL_SCENE", ENVIRONMENT_SHELL)
        .env_remove("RUST_LOG")
        .output()
        .expect("running the child");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the child failed:\n{stdout}\n{stderr}"
    );
    if stdout.contains("shell-override: skipped") {
        return;
    }
    assert!(
        stdout.contains("shell-override: checked"),
        "the child did not run the check:\n{stdout}\n{stderr}"
    );
}

/// The child half of `the_environment_overrides_the_configured_shell_scene`.
#[test]
#[ignore = "run by the_environment_overrides_the_configured_shell_scene, with the variable set"]
fn shell_scene_under_the_environment() {
    // `cargo test -- --include-ignored` runs this in the test process
    // itself, without the variable. Nothing to check there, and the line
    // printed is neither marker that
    // `the_environment_overrides_the_configured_shell_scene` accepts.
    if std::env::var("SOLIUM_SHELL_SCENE").as_deref() != Ok(ENVIRONMENT_SHELL) {
        println!("shell-override: not run by the parent, nothing checked");
        return;
    }
    let Some((_, commands)) = shell_after_monitors(
        "solium-script-test-shell-override",
        &format!("return {{ shell = {{ scene = \"{CONFIGURED_SHELL}\" }} }}"),
    ) else {
        println!("shell-override: skipped");
        return;
    };
    let (declared, _) = shell_surfaces(&commands);
    let scenes: Vec<&std::path::Path> =
        declared.iter().map(|shell| shell.scene.as_path()).collect();
    assert_eq!(
        scenes,
        vec![std::path::Path::new(ENVIRONMENT_SHELL)],
        "the configured scene won over SOLIUM_SHELL_SCENE"
    );
    println!("shell-override: checked");
}

/// **The shipped configuration turns the screens off after ten minutes**,
/// the same number the compositor holds for a configuration that says
/// nothing, and a `user.lua` can say 0. See `crate::idle::Settings`.
#[test]
fn the_shipped_configuration_turns_the_screens_off_after_ten_minutes() {
    for (user, after) in [
        ("return {}", Duration::from_secs(600)),
        (
            "return { idle = { screens_off_after = 0 } }",
            Duration::ZERO,
        ),
    ] {
        let Some((directory, mut scripts)) =
            shipped_init_with_user("solium-script-test-idle-default", user)
        else {
            return;
        };
        let idle: Vec<crate::idle::Settings> = scripts
            .startup()
            .commands
            .into_iter()
            .filter_map(|command| match command {
                Command::Idle(settings) => Some(settings),
                _ => None,
            })
            .collect();
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            idle,
            [crate::idle::Settings {
                screens_off_after: after,
                ..crate::idle::Settings::default()
            }],
            "{user}: the shipped init.lua did not hand `config.idle` over as it says"
        );
        assert_eq!(
            idle.first().map(|settings| settings.off_frame_interval),
            Some(Duration::from_secs(1))
        );
    }
}

/// **The shipped configuration locks after five minutes**, the number the
/// compositor holds for a configuration that says nothing; a `user.lua`
/// can say 0, and one that says something that is not a number of 0 or
/// more keeps the five minutes. See `crate::idle::Settings`.
#[test]
fn the_shipped_configuration_locks_after_five_minutes() {
    for (user, after) in [
        ("return {}", Duration::from_secs(300)),
        ("return { idle = { lock_after = 0 } }", Duration::ZERO),
        (
            "return { idle = { lock_after = 90 } }",
            Duration::from_secs(90),
        ),
        (
            "return { idle = { lock_after = \"soon\" } }",
            Duration::from_secs(300),
        ),
        (
            "return { idle = { lock_after = -1 } }",
            Duration::from_secs(300),
        ),
    ] {
        let Some((directory, mut scripts)) =
            shipped_init_with_user("solium-script-test-idle-lock", user)
        else {
            return;
        };
        let after_seen: Vec<Duration> = scripts
            .startup()
            .commands
            .into_iter()
            .filter_map(|command| match command {
                Command::Idle(settings) => Some(settings.lock_after),
                _ => None,
            })
            .collect();
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            after_seen,
            [after],
            "{user}: the shipped init.lua did not hand `idle.lock_after` over as it says"
        );
    }
    assert_eq!(
        crate::idle::Settings::default().lock_after,
        Duration::from_secs(300)
    );
}

#[test]
fn shortcuts_policy_and_escape_from_user_lua_reach_the_shipped_configuration() {
    let (directory, mut scripts) = shipped_init_with_user(
        "solium-script-test-shortcuts-user",
        r#"return { input = { shortcuts_inhibit = {
            allow = "fullscreen", escape = "super+x", apps = { deny = { "blocked*" } }
        } } }"#,
    )
    .expect("shipped configuration");
    assert!(
        scripts
            .bindings()
            .iter()
            .any(|binding| binding.combo == "super+x" && binding.inhibited)
    );
    let settings = scripts
        .startup()
        .commands
        .into_iter()
        .find_map(|command| match command {
            Command::ShortcutsInhibit(settings) => Some(settings),
            _ => None,
        })
        .expect("shipped init hands over shortcut policy");
    assert_eq!(settings.allow, crate::shortcuts_inhibit::Allow::Fullscreen);
    assert_eq!(settings.escape, "super+x");
    assert!(
        settings
            .deny
            .iter()
            .any(|pattern| pattern.matches("blocked.viewer"))
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn shortcuts_policy_validates_permission_escape_and_application_patterns() {
    use crate::shortcuts_inhibit::Allow;
    let lua = Lua::new();
    for (value, allowed) in [
        ("true", Allow::Always),
        ("false", Allow::Never),
        ("'fullscreen'", Allow::Fullscreen),
    ] {
        let table: Table = lua
            .load(format!(
                "return {{allow={value}, escape='Super+X', apps={{deny={{'blocked*'}}}}}}"
            ))
            .eval()
            .expect("table");
        let settings = crate::script::settings::input::shortcuts(
            &lua::convert::doc_of(&Value::Table(table)).expect("a document"),
        )
        .expect("valid policy");
        assert_eq!(settings.allow, allowed);
        assert_eq!(settings.escape, "super+x");
        assert!(settings.deny[0].matches("blocked.viewer"));
        assert!(!settings.deny[0].matches("trusted.viewer"));
    }
    for spec in [
        "{allow='always'}",
        "{escape=''}",
        "{escape='bogus+q'}",
        "{escape='super+no_such_key'}",
        "{apps={deny={'a*b'}}}",
        "{apps={deny={''}}}",
    ] {
        let table: Table = lua.load(format!("return {spec}")).eval().expect("table");
        assert!(
            crate::script::settings::input::shortcuts(
                &lua::convert::doc_of(&Value::Table(table)).expect("a document")
            )
            .is_err(),
            "accepted invalid policy {spec}"
        );
    }
}

/// **The shipped configuration taps and scrolls naturally on a touchpad,
/// and does neither on a mouse** -- `config.lua`'s own `input` section,
/// read through `sol.input` exactly as `idle` is. A `user.lua` overriding
/// one field of `touchpad` keeps the other.
#[test]
fn the_shipped_configuration_configures_a_touchpad_and_a_mouse() {
    let Some((directory, mut scripts)) =
        shipped_init_with_user("solium-script-test-input-default", "return {}")
    else {
        return;
    };
    let config = input_config(scripts.startup()).expect("`init.lua` calls `sol.input`");
    let _ = std::fs::remove_dir_all(&directory);

    let touchpad = DeviceInfo {
        id: "touchpad".into(),
        name: "Synaptics TouchPad".into(),
        kind: DeviceKind::Touchpad,
        vendor: None,
        product: None,
        output: None,
    };
    let mouse = DeviceInfo {
        id: "mouse".into(),
        name: "Any Mouse".into(),
        kind: DeviceKind::Mouse,
        vendor: None,
        product: None,
        output: None,
    };
    assert_eq!(config.resolve(&touchpad).tap, Some(true));
    assert_eq!(config.resolve(&touchpad).natural_scroll, Some(true));
    assert_eq!(
        config.resolve(&mouse).natural_scroll,
        None,
        "a mouse is not a surface your fingers are on; the shipped config says nothing \
         about it, leaving libinput's own default"
    );
}

/// **A `devices` entry matches the device `config.lua` names, overriding
/// only the fields it sets.** The integration half of
/// `devices::tests::a_name_matched_override_changes_only_the_fields_it_names`:
/// this proves the override actually reaches `Command::Input` from real
/// Lua syntax, parenthesis and all.
#[test]
fn a_devices_entry_in_user_lua_overrides_one_field_of_the_matched_device() {
    let Some((directory, mut scripts)) = shipped_init_with_user(
        "solium-script-test-input-override",
        r#"
        return {
            input = {
                devices = {
                    { name = "Synaptics", natural_scroll = false },
                },
            },
        }
        "#,
    ) else {
        return;
    };
    let config = input_config(scripts.startup()).expect("`init.lua` calls `sol.input`");
    let _ = std::fs::remove_dir_all(&directory);

    let resolved = config.resolve(&DeviceInfo {
        id: "touchpad".into(),
        name: "Synaptics TouchPad".into(),
        kind: DeviceKind::Touchpad,
        vendor: None,
        product: None,
        output: None,
    });
    assert_eq!(
        resolved.natural_scroll,
        Some(false),
        "the override's own field"
    );
    assert_eq!(
        resolved.tap,
        Some(true),
        "the override named nothing about tap, so the touchpad default stands"
    );
}

/// **Each shipped mode hands over its own focus model** (#128): floating
/// click-only, tiling and scrolling following the pointer, each with its
/// own `focus.after_close` -- floating's at load, tiling's and
/// scrolling's as each is switched to. A shipped `"follow"` is handed
/// over as `None`, the machine's own answer, so a tablet or a phone,
/// with no pointer to hover, keeps its own (#159);
/// `on_a_tablet_the_shipped_layouts_leave_hover_to_the_machine` in
/// `state/tests.rs` is that half.
#[test]
fn the_shipped_modes_each_hand_over_their_own_focus_model() {
    use crate::successor::Successor::{Neighbour, Pointer, Recent, Topmost};
    let Some((directory, mut scripts)) =
        shipped_init_with_user("solium-script-test-after-close-shipped", "return {}")
    else {
        return;
    };
    let commands = [
        scripts.startup().commands,
        scripts.key("super+t", empty_snapshot()).commands,
        scripts.key("super+s", empty_snapshot()).commands,
    ];
    let _ = std::fs::remove_dir_all(&directory);
    let [loaded, tiling, scrolling] =
        commands.map(|commands| (focus_modes(commands.clone()), after_closes(commands)));
    assert_eq!(
        loaded,
        (
            vec![(None, Some(false), Some(true))],
            vec![Some(vec![Recent, Topmost])]
        ),
        "floating, at load"
    );
    let layout = (
        vec![(None, None, Some(true))],
        vec![Some(vec![Pointer, Neighbour, Topmost])],
    );
    assert_eq!(tiling, layout, "tiling");
    assert_eq!(scrolling, layout, "scrolling");
}

/// **Whether the pointer focuses what it moves over is your entry for
/// the mode, then your `focus.follow`, then the shipped entry, then the
/// machine's own answer** (#128 review, the ranking of 2026-10-14). So
/// `focus = { follow = false }` never follows, in any mode, unless you
/// also name a mode; merged over the shipped table, the shipped
/// `tiling = "follow"` used to win over it. Each mode is switched to in
/// turn, `custom` -- a mode no table names -- by a binding of the
/// user's own, and the `follow` each hands over is read; `None` is the
/// machine's own answer, which a shipped `"follow"` defers to.
/// An entry of yours that is neither word counts as none.
/// Against the ranking before it -- any entry in `focus.modes`, then
/// `focus.follow` -- it fails at `{ follow = false }`: tiling's shipped
/// `"follow"` won. With a shipped `"follow"` read as yours it fails at
/// the row whose `tiling` is no word, which falls through to the shipped
/// entry: tiling handed over `true`, which would override a tablet's own
/// answer.
#[test]
fn your_mode_entry_then_your_focus_follow_then_the_shipped_entry_then_the_machine() {
    const CUSTOM: &str = r#"bindings = {
        ["super+alt+F12"] = function()
            local modes = require("modes")
            modes.register("custom", {})
            modes.use("custom")
        end,
    }"#;
    // Floating, tiling, scrolling and `custom`, in the order switched to.
    for (row, (focus, expected)) in [
        ("{ follow = false }", [Some(false); 4]),
        ("{ follow = true }", [Some(true); 4]),
        (
            r#"{ follow = false, modes = { tiling = "follow" } }"#,
            [Some(false), Some(true), Some(false), Some(false)],
        ),
        (
            r#"{ follow = true, modes = { floating = "click" } }"#,
            [Some(false), Some(true), Some(true), Some(true)],
        ),
        (
            r#"{ modes = { scrolling = "click", custom = "follow", tiling = "hover" } }"#,
            [Some(false), None, Some(false), Some(true)],
        ),
        ("{}", [Some(false), None, None, None]),
    ]
    .into_iter()
    .enumerate()
    {
        let Some((directory, mut scripts)) = shipped_init_with_user(
            &format!("solium-script-test-focus-ranking-{row}"),
            &format!("return {{ focus = {focus}, {CUSTOM} }}"),
        ) else {
            return;
        };
        let commands = [
            scripts.startup().commands,
            scripts.key("super+t", empty_snapshot()).commands,
            scripts.key("super+s", empty_snapshot()).commands,
            scripts.key("super+alt+F12", empty_snapshot()).commands,
        ];
        let _ = std::fs::remove_dir_all(&directory);
        let follows = commands.map(|commands| {
            let modes = focus_modes(commands);
            assert_eq!(modes.len(), 1, "focus = {focus}: one hand-over per mode");
            modes.first().and_then(|(_, follow, _)| *follow)
        });
        assert_eq!(
            follows, expected,
            "focus = {focus}: floating, tiling, scrolling and a mode no table names"
        );
    }
}

/// **The shipped configuration's default mode, floating, focuses on
/// click and not on hover** (#219): the desktop's windows are free to
/// overlap, and one you were not aiming for sitting under the pointer on
/// the way to the one you want should not steal the keyboard.
#[test]
fn the_shipped_configuration_defaults_floating_to_click_only_focus() {
    let Some((directory, mut scripts)) =
        shipped_init_with_user("solium-script-test-focus-mode-default", "return {}")
    else {
        return;
    };
    let modes = focus_modes(scripts.startup().commands);
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(
        modes,
        [(None, Some(false), Some(true))],
        "the shipped init.lua did not hand `config.focus` over for the \
         default (floating) mode as it says"
    );
}

/// **`config.focus.modes` overrides a mode's own default** from
/// `user.lua`: the spelling that exists works. `nearest` does *not*
/// catch a typo here (`focus.modes.folating` merges in silently) --
/// `open_sections["focus.modes"] = true` accepts any name, the same as
/// `bindings`, because mode names are open-ended too.
#[test]
fn a_user_lua_can_turn_floatings_focus_back_to_follow() {
    let Some((directory, mut scripts)) = shipped_init_with_user(
        "solium-script-test-focus-mode-user",
        r#"return { focus = { modes = { floating = "follow" } } }"#,
    ) else {
        return;
    };
    let modes = focus_modes(scripts.startup().commands);
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(
        modes,
        [(None, Some(true), Some(true))],
        "a user.lua naming `focus.modes.floating` did not reach `sol.focus_mode`"
    );
}

/// **A reload keeps the focus model the session was already in** (#219),
/// the same defect #116 found in `workspaces.lua` and `modes.lua` fixed
/// for the mode itself: `modes.lua` runs fresh on every load, so without
/// `sol.keep` this file could not tell a reload from a fresh start, and
/// `super+shift+r` after switching to tiling would answer `follow =
/// false` -- floating's click-only override -- instead of tiling's own
/// `follow`, every time. Since #128 `config.focus.modes` names tiling's
/// `"follow"`, which as shipped is the machine's own answer: `None`.
#[test]
fn a_reload_keeps_the_focus_model_tiling_was_switched_to() {
    let Some((directory, mut scripts)) =
        shipped_init_with_user("solium-script-test-focus-mode-reload", "return {}")
    else {
        return;
    };
    let _ = scripts.startup();
    let switched = scripts.key("super+t", empty_snapshot());
    assert!(
        switched.handled,
        "super+t: tiling's binding was not reached"
    );
    assert_eq!(
        focus_modes(switched.commands),
        [(None, None, Some(true))],
        "switching to tiling did not hand over its own `follow`"
    );

    let config = directory.join("init.lua");
    let mut reloaded =
        Scripts::load_carrying(&config, scripts.kept()).expect("reloading the test scripts");
    let modes = focus_modes(reloaded.startup().commands);
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(
        modes,
        [(None, None, Some(true))],
        "a reload forgot tiling was in charge and fell back to floating's click-only \
         override (`follow = false`)"
    );
}

/// **The shipped configuration's native lock settings match
/// `lock_auth::Settings`'s own default**, as its locker settings match
/// `logind::Settings`'s below.
#[test]
fn the_shipped_configuration_s_native_lock_settings_match_the_rust_default() {
    let Some((directory, mut scripts)) =
        shipped_init_with_user("solium-script-test-lock-auth-default", "return {}")
    else {
        return;
    };
    let settings: Vec<crate::lock_auth::Settings> = scripts
        .startup()
        .commands
        .into_iter()
        .filter_map(|command| match command {
            Command::LockAuth(settings) => Some(settings),
            _ => None,
        })
        .collect();
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(
        settings,
        [crate::lock_auth::Settings::default()],
        "the shipped init.lua did not hand config.lock over as it says"
    );
}

/// **The shipped configuration's settings for what the native lock shows
/// match `lock_scene::Settings`'s own default.**
#[test]
fn the_shipped_configuration_s_lock_scene_settings_match_the_rust_default() {
    let Some((directory, mut scripts)) =
        shipped_init_with_user("solium-script-test-lock-scene-default", "return {}")
    else {
        return;
    };
    let settings: Vec<crate::lock_scene::Settings> = scripts
        .startup()
        .commands
        .into_iter()
        .filter_map(|command| match command {
            Command::LockScene(settings) => Some(settings),
            _ => None,
        })
        .collect();
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(
        settings,
        [crate::lock_scene::Settings::default()],
        "the shipped init.lua did not hand config.lock over as it says"
    );
}

/// **The shipped configuration's `lock` table matches `logind::Settings`'s
/// own default**, the same parity
/// `the_shipped_configuration_turns_the_screens_off_after_ten_minutes`
/// holds for `idle`: a future edit to `config.lua`'s `lock.before_sleep`
/// or `command` without a matching change to `logind::Settings::default`,
/// or the other way around, must fail a test rather than ship a mismatch.
/// See `crate::logind::Settings`.
#[test]
fn the_shipped_configuration_s_lock_table_matches_the_rust_default() {
    let Some((directory, mut scripts)) =
        shipped_init_with_user("solium-script-test-lock-default", "return {}")
    else {
        return;
    };
    let lock: Vec<crate::logind::Settings> = scripts
        .startup()
        .commands
        .into_iter()
        .filter_map(|command| match command {
            Command::Lock(settings) => Some(settings),
            _ => None,
        })
        .collect();
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(
        lock,
        [crate::logind::Settings::default()],
        "the shipped init.lua did not hand config.lock over as it says"
    );
}

/// **The shipped configuration's `reload` table matches
/// `autoreload::Settings`'s own default**, the same parity
/// `the_shipped_configuration_s_lock_table_matches_the_rust_default` holds
/// for `lock`: a future edit to `config.lua`'s `reload.automatic` or
/// `reload.quiet_ms` without a matching change to
/// `autoreload::Settings::default`, or the other way around, must fail a
/// test rather than ship a mismatch.
#[test]
fn the_shipped_configuration_s_reload_table_matches_the_rust_default() {
    let Some((directory, mut scripts)) =
        shipped_init_with_user("solium-script-test-auto-reload-default", "return {}")
    else {
        return;
    };
    let reload: Vec<crate::autoreload::Settings> = scripts
        .startup()
        .commands
        .into_iter()
        .filter_map(|command| match command {
            Command::AutoReload(settings) => Some(settings),
            _ => None,
        })
        .collect();
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(
        reload,
        [crate::autoreload::Settings::default()],
        "the shipped init.lua did not hand config.reload over as it says"
    );
}

#[test]
fn the_shipped_workspaces_offer_ext_workspace() {
    let Some((directory, mut scripts)) =
        shipped_init_with_user("solium-script-test-workspaces-offered-default", "return {}")
    else {
        return;
    };
    let offered: Vec<bool> = scripts
        .startup()
        .commands
        .into_iter()
        .filter_map(|command| match command {
            Command::WorkspacesOffered(offered) => Some(offered),
            _ => None,
        })
        .collect();
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(
        offered,
        [true],
        "the shipped workspaces.lua did not offer them"
    );
}

#[test]
fn the_shipped_configuration_s_toplevels_table_matches_the_rust_default() {
    let Some((directory, mut scripts)) =
        shipped_init_with_user("solium-script-test-toplevels-default", "return {}")
    else {
        return;
    };
    let toplevels: Vec<crate::toplevels::Settings> = scripts
        .startup()
        .commands
        .into_iter()
        .filter_map(|command| match command {
            Command::Toplevels(settings) => Some(settings),
            _ => None,
        })
        .collect();
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(
        toplevels,
        [crate::toplevels::Settings::default()],
        "the shipped init.lua did not hand config.toplevels over as it says"
    );
}

#[test]
fn the_shipped_configuration_s_frames_table_matches_the_rust_default() {
    let Some((directory, mut scripts)) =
        shipped_init_with_user("solium-script-test-frames-default", "return {}")
    else {
        return;
    };
    let frames: Vec<crate::framing::Settings> = scripts
        .startup()
        .commands
        .into_iter()
        .filter_map(|command| match command {
            Command::Frames(settings) => Some(settings),
            _ => None,
        })
        .collect();
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(
        frames,
        [crate::framing::Settings::default()],
        "the shipped init.lua did not hand config.frames over as it says"
    );
}

#[test]
fn the_shipped_configuration_s_layer_shell_table_matches_the_rust_default() {
    let Some((directory, mut scripts)) =
        shipped_init_with_user("solium-script-test-layer-shell-default", "return {}")
    else {
        return;
    };
    let layer_shell: Vec<crate::layer::Settings> = scripts
        .startup()
        .commands
        .into_iter()
        .filter_map(|command| match command {
            Command::LayerShell(settings) => Some(settings),
            _ => None,
        })
        .collect();
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(
        layer_shell,
        [crate::layer::Settings::default()],
        "the shipped init.lua did not hand config.layer_shell over as it says"
    );
}

#[test]
fn the_shipped_configuration_s_x11_table_matches_the_rust_default() {
    let Some((directory, mut scripts)) =
        shipped_init_with_user("solium-script-test-x11-default", "return {}")
    else {
        return;
    };
    let x11: Vec<crate::xwayland::Settings> = scripts
        .startup()
        .commands
        .into_iter()
        .filter_map(|command| match command {
            Command::X11(settings) => Some(settings),
            _ => None,
        })
        .collect();
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(
        x11,
        [crate::xwayland::Settings::default()],
        "the shipped init.lua did not hand config.x11 over as it says"
    );
}

/// **The shipped configuration runs a moving window's effects on every
/// pass** (`effects.motion_hz = 0`, 2026-10-16), and that is the Rust
/// default too: the glass is read where the window is on every frame of
/// a glide, a drag or a resize, and a lower rate is a choice made in
/// `config.lua` for a weak GPU. The same parity
/// `the_shipped_configuration_s_lock_table_matches_the_rust_default`
/// holds for `lock`.
#[test]
fn the_shipped_configuration_runs_a_moving_windows_effects_on_every_pass() {
    let Some((directory, scripts)) =
        shipped_init_with_user("solium-script-test-motion-hz-default", "return {}")
    else {
        return;
    };
    let settings = scripts.effect_settings_at_load();
    let _ = std::fs::remove_dir_all(&directory);
    let Some(Ok(settings)) = settings else {
        panic!("the shipped init.lua handed over no effects settings: {settings:?}");
    };
    let default = crate::effect::settings::Settings::default().motion_hz;
    assert!(
        settings.motion_hz.abs() < f64::EPSILON && default.abs() < f64::EPSILON,
        "the shipped effects.motion_hz is {} and the Rust default {default}, not both 0",
        settings.motion_hz
    );
}

/// **The shipped configuration shows the problems on the primary
/// monitor**, in its top-right corner as `file:line: message` rows, at
/// most eight and then a count, taking no input; and takes the overlay
/// away when none are left.
#[test]
fn the_shipped_overlay_lists_the_problems_on_the_primary_monitor() {
    let Some((directory, mut scripts)) =
        shipped_init_with_user("solium-problems-overlay", "return {}")
    else {
        return;
    };
    let mut snapshot = one_screen(&[]);
    snapshot.problems = (1..=10)
        .map(|line| super::ProblemRow {
            effect: "blur".to_owned(),
            file: "/x/blur/down.frag".to_owned(),
            line: Some(line),
            column: None,
            message: "`p_ofset` is not a param".to_owned(),
            severity: if line == 1 { "warning" } else { "error" },
        })
        .collect();
    let shown = scripts.problems_changed(snapshot).commands;
    let gone = scripts.problems_changed(one_screen(&[])).commands;
    let _ = std::fs::remove_dir_all(&directory);

    let declared: Vec<&crate::scripted::Declaration> = shown
        .iter()
        .filter_map(|command| match command {
            Command::Surface(declared) if declared.name == "problems" => Some(&**declared),
            _ => None,
        })
        .collect();
    let [overlay] = declared.as_slice() else {
        panic!("one overlay, not {}: {shown:?}", declared.len());
    };
    assert!(
        overlay.scene.ends_with("problems.qml"),
        "{:?}",
        overlay.scene
    );
    assert_eq!(overlay.layer, crate::scripted::Layer::Overlay);
    assert!(!overlay.interactive, "the overlay takes no input");
    assert_eq!(
        overlay.on,
        crate::scripted::On::Rect(smithay::utils::Rectangle::new(
            (840, 0).into(),
            (760, 240).into()
        )),
        "the top-right corner of the 1600-wide primary monitor"
    );
    let properties = overlay.properties.render();
    assert!(
        properties.contains(
            r#"{"severity":"warning","text":"/x/blur/down.frag:1: `p_ofset` is not a param"}"#
        ),
        "{properties}"
    );
    assert!(
        properties.contains(r#""text":"/x/blur/down.frag:8: "#),
        "{properties}"
    );
    assert!(
        !properties.contains(r#""text":"/x/blur/down.frag:9: "#),
        "{properties}"
    );
    assert!(
        properties.contains(r#""text":"and 2 more""#),
        "{properties}"
    );
    assert!(
        gone.iter()
            .any(|command| matches!(command, Command::SurfaceGone(name) if name == "problems")),
        "the overlay stays with nothing left to show: {gone:?}"
    );
}

/// **A deform's own keys are refused outside their words**, each naming
/// the key: `seed` in [0, 1), `failed`, `on_reload`, `axis`, and a param
/// that is no param's value.
#[test]
fn a_deforms_own_keys_are_refused_outside_their_words() {
    let lua = Lua::new();
    let cases = [
        ("seed = 1", "`seed` is a number from 0 up to 1"),
        ("seed = -0.5", "`seed` is a number from 0 up to 1"),
        ("failed = 'fade'", "`failed` is \"flat\" or \"hide\""),
        (
            "on_reload = 'switch'",
            "`on_reload` is \"flat\" or \"keep\"",
        ),
        ("axis = 'sideways'", "`axis` is \"auto\" or \"down\""),
        ("progress = 0/0", "`progress` is a finite number"),
        ("spread = { 1, 2 }", "`spread` is not a param's value"),
    ];
    for (key, says) in cases {
        let options: Table = lua
            .load(format!(
                "return {{ deform = {{ effect = 'genie', {key}, to = {{ window = 2 }} }} }}"
            ))
            .eval()
            .expect("the test's table");
        let error = deform_from(&options).expect_err(key).to_string();
        assert!(error.contains(says), "{key}: {error}");
    }
    let options: Table = lua
        .load("return { deform = { effect = 'genie', seed = 0, failed = 'flat', on_reload = 'flat', to = { window = 2 } } }")
        .eval()
        .expect("the test's table");
    assert!(deform_from(&options).is_ok_and(|deform| deform.is_some()));
}

/// **A keyboard section that says what layout to start on is not a typo.**
///
/// `config.lua` restates `sol.keyboard`'s key set, because `keyboard`
/// leaves the xkb names out on purpose and so cannot be the list. The
/// restatement left `active` out, and `sol.keyboard` reads it -- so a
/// dual-layout configuration, the only kind that has an `active` to name,
/// was told its working setting is read by nothing and `solium --check`
/// exited 1 (#117 review).
///
/// That is worse than the silence #117 replaced. `--check` answers "did my
/// configuration work", and a check that is wrong about a setting people
/// really write is a check they stop running. See
/// `every_key_a_section_accepts_is_one_the_compositor_reads`, which is what
/// stops the list drifting again.
#[test]
fn a_keyboard_section_naming_its_layouts_and_which_is_live_is_not_a_typo() {
    let Some((directory, scripts)) = shipped_init_with_user(
        "solium-script-test-keyboard-active",
        r#"
        return {
            keyboard = { layout = "us,ru", active = 2 },
        }
        "#,
    ) else {
        return;
    };

    let reported: Vec<String> = scripts
        .unknown_settings()
        .into_iter()
        .map(|setting| setting.key)
        .collect();
    assert!(
        reported.is_empty(),
        "a configuration naming two layouts and which of them is live is reported \
         as read by nothing, and `--check` exits 1 over a setting that works: \
         {reported:?}"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// **The shipped configuration frames every window in `glass`**: the
/// shipped `init.lua` hands the compositor `pane = "glass"` when a
/// `user.lua` says nothing, and the style a `user.lua` names when it
/// names one, which is how to go back to `top`.
#[test]
fn the_shipped_configuration_frames_windows_in_glass() {
    for (user, wanted) in [("return {}", "glass"), ("return { pane = \"top\" }", "top")] {
        let Some((directory, mut scripts)) =
            shipped_init_with_user("solium-script-test-pane", user)
        else {
            return;
        };
        let handed: Vec<Option<String>> = scripts
            .startup()
            .commands
            .into_iter()
            .filter_map(|command| match command {
                Command::Decoration { name } => Some(name),
                _ => None,
            })
            .collect();
        assert_eq!(handed, vec![Some(wanted.to_owned())], "{user}");
        let _ = std::fs::remove_dir_all(&directory);
    }
}

/// **`fullscreen.covers` reaches the compositor from the shipped
/// `init.lua`**: the default when a `user.lua` says nothing, `"none"` when
/// it says that, and the default kept for a word that is neither.
#[test]
fn the_shipped_fullscreen_setting_reaches_the_compositor() {
    for (user, wanted) in [
        ("return {}", crate::stack::Covers::Top),
        (
            "return { fullscreen = { covers = \"none\" } }",
            crate::stack::Covers::Nothing,
        ),
        (
            "return { fullscreen = { covers = \"everything\" } }",
            crate::stack::Covers::Top,
        ),
    ] {
        let Some((directory, mut scripts)) =
            shipped_init_with_user("solium-script-test-fullscreen", user)
        else {
            return;
        };
        let handed: Vec<crate::stack::Covers> = scripts
            .startup()
            .commands
            .into_iter()
            .filter_map(|command| match command {
                Command::Fullscreen(covers) => Some(covers),
                _ => None,
            })
            .collect();
        assert_eq!(handed, vec![wanted], "{user}");
        let _ = std::fs::remove_dir_all(&directory);
    }
}

/// **The shipped `resize` setting reaches the compositor (#233)**: the
/// default is `anchor` on `#1c1c1c`, and each key a user changes changes
/// what the compositor is told -- a name that is not one keeps the
/// default, as `fill`'s always did. `patience` is milliseconds from 0 to
/// `resizing::PATIENCE_MAX`, anything else keeping the quarter second.
#[test]
fn the_shipped_resize_setting_reaches_the_compositor() {
    use crate::resizing::{BACKGROUND, EVERY_FRAME, Fill, Follow, PATIENCE, SYNC, Settings};
    let waits = |millis| Settings {
        patience: std::time::Duration::from_millis(millis),
        ..Settings::default()
    };
    let syncs = |millis| Settings {
        sync: std::time::Duration::from_millis(millis),
        ..Settings::default()
    };
    let tells = |hz: f64| Settings {
        tell: std::time::Duration::from_secs_f64(1.0 / hz),
        ..Settings::default()
    };
    for (user, wanted) in [
        (
            "return {}",
            Settings {
                fill: Fill::Crop,
                background: BACKGROUND,
                patience: PATIENCE,
                tell: EVERY_FRAME,
                follow: Follow::App,
                sync: SYNC,
                overlap: true,
            },
        ),
        (
            "return { resize = { fill = \"stretch\" } }",
            Settings {
                fill: Fill::Stretch,
                background: BACKGROUND,
                patience: PATIENCE,
                tell: EVERY_FRAME,
                follow: Follow::App,
                sync: SYNC,
                overlap: true,
            },
        ),
        (
            "return { resize = { fill = \"crop\", background = \"#10203080\" } }",
            Settings {
                fill: Fill::Crop,
                background: crate::resizing::Background::Colour([0x10, 0x20, 0x30, 0x80]),
                patience: PATIENCE,
                tell: EVERY_FRAME,
                follow: Follow::App,
                sync: SYNC,
                overlap: true,
            },
        ),
        (
            "return { resize = { background = \"none\" } }",
            Settings {
                fill: Fill::Crop,
                background: crate::resizing::Background::None,
                patience: PATIENCE,
                tell: EVERY_FRAME,
                follow: Follow::App,
                sync: SYNC,
                overlap: true,
            },
        ),
        (
            "return { resize = { fill = \"hold\", background = \"dark\" } }",
            Settings {
                fill: Fill::Crop,
                background: BACKGROUND,
                patience: PATIENCE,
                tell: EVERY_FRAME,
                follow: Follow::App,
                sync: SYNC,
                overlap: true,
            },
        ),
        (
            "return { resize = { background = \"#1c1c1c\" } }",
            Settings {
                background: crate::resizing::Background::Colour([0x1c, 0x1c, 0x1c, 0xff]),
                ..Settings::default()
            },
        ),
        (
            "return { resize = { background = \"edge\" } }",
            Settings::default(),
        ),
        ("return { resize = { patience = 600 } }", waits(600)),
        ("return { resize = { patience = 0 } }", waits(0)),
        ("return { resize = { patience = 5000 } }", waits(5000)),
        ("return { resize = { patience = 5001 } }", waits(250)),
        ("return { resize = { patience = -1 } }", waits(250)),
        ("return { resize = { patience = \"long\" } }", waits(250)),
        ("return { resize = { sync = 0 } }", syncs(0)),
        ("return { resize = { sync = 50 } }", syncs(50)),
        ("return { resize = { sync = 200 } }", syncs(200)),
        ("return { resize = { sync = 5000 } }", syncs(5000)),
        ("return { resize = { sync = 5001 } }", syncs(0)),
        ("return { resize = { sync = -1 } }", syncs(0)),
        ("return { resize = { sync = \"long\" } }", syncs(0)),
        (
            "return { resize = { overlap = false } }",
            Settings {
                overlap: false,
                ..Settings::default()
            },
        ),
        (
            "return { resize = { overlap = \"no\" } }",
            Settings::default(),
        ),
        ("return { resize = { tell_hz = 30 } }", tells(30.0)),
        ("return { resize = { tell_hz = 1000 } }", tells(1000.0)),
        ("return { resize = { tell_hz = 0 } }", Settings::default()),
        (
            "return { resize = { tell_hz = 1001 } }",
            Settings::default(),
        ),
        ("return { resize = { tell_hz = 0.5 } }", Settings::default()),
        ("return { resize = { tell_hz = -1 } }", Settings::default()),
        (
            "return { resize = { tell_hz = \"fast\" } }",
            Settings::default(),
        ),
        (
            "return { resize = { follow = \"pointer\" } }",
            Settings {
                follow: Follow::Pointer,
                ..Settings::default()
            },
        ),
        (
            "return { resize = { follow = \"app\" } }",
            Settings::default(),
        ),
        (
            "return { resize = { follow = \"edge\" } }",
            Settings::default(),
        ),
    ] {
        let Some((directory, mut scripts)) =
            shipped_init_with_user("solium-script-test-resize", user)
        else {
            return;
        };
        let handed: Vec<Settings> = scripts
            .startup()
            .commands
            .into_iter()
            .filter_map(|command| match command {
                Command::Resize(settings) => Some(settings),
                _ => None,
            })
            .collect();
        assert_eq!(handed, vec![wanted], "{user}");
        let _ = std::fs::remove_dir_all(&directory);
    }
}

/// **Each mode fits a resize by its own `resize.modes` entry, or by
/// `resize.fill`, the one for every mode (#233)**, handed over as the
/// focus model is: once at load for the mode the session opens in, and
/// again at each switch, with the background and the patience beside it.
/// A mode the table does not name takes `fill`; a table naming no mode
/// hands nothing over past `init.lua`'s one call, so the shipped
/// configuration says it once.
#[test]
fn each_mode_fits_a_resize_by_its_own_fill_or_the_one_for_every_mode() {
    use crate::resizing::{BACKGROUND, Fill, Settings};
    let last = |commands: Vec<Command>| {
        commands
            .into_iter()
            .filter_map(|command| match command {
                Command::Resize(settings) => Some(settings),
                _ => None,
            })
            .next_back()
    };
    let fill = |fill: Fill| {
        Some(Settings {
            fill,
            background: BACKGROUND,
            ..Settings::default()
        })
    };
    let patient = |fill: Fill| {
        Some(Settings {
            fill,
            patience: std::time::Duration::from_millis(600),
            ..Settings::default()
        })
    };
    let capped = |fill: Fill| {
        Some(Settings {
            fill,
            tell: std::time::Duration::from_secs_f64(1.0 / 30.0),
            ..Settings::default()
        })
    };
    // At load (floating), then tiling, then scrolling.
    for (resize, expected) in [
        (
            r#"{ fill = "anchor", modes = { tiling = "stretch" } }"#,
            [fill(Fill::Anchor), fill(Fill::Stretch), fill(Fill::Anchor)],
        ),
        (
            r#"{ modes = { floating = "stretch", scrolling = "anchor" } }"#,
            [fill(Fill::Stretch), fill(Fill::Crop), fill(Fill::Anchor)],
        ),
        (
            r#"{ background = "none", modes = { scrolling = "stretch" } }"#,
            [
                Some(Settings {
                    fill: Fill::Crop,
                    background: crate::resizing::Background::None,
                    ..Settings::default()
                }),
                Some(Settings {
                    fill: Fill::Crop,
                    background: crate::resizing::Background::None,
                    ..Settings::default()
                }),
                Some(Settings {
                    fill: Fill::Stretch,
                    background: crate::resizing::Background::None,
                    ..Settings::default()
                }),
            ],
        ),
        (
            r#"{ patience = 600, modes = { tiling = "stretch" } }"#,
            [
                patient(Fill::Crop),
                patient(Fill::Stretch),
                patient(Fill::Crop),
            ],
        ),
        // `tell_hz` goes over with every switch, as the patience does: a
        // switch that dropped it would put the drag back to every frame.
        (
            r#"{ tell_hz = 30, modes = { tiling = "stretch" } }"#,
            [
                capped(Fill::Crop),
                capped(Fill::Stretch),
                capped(Fill::Crop),
            ],
        ),
        ("{}", [fill(Fill::Crop), None, None]),
    ] {
        let Some((directory, mut scripts)) = shipped_init_with_user(
            "solium-script-test-resize-modes",
            &format!("return {{ resize = {resize} }}"),
        ) else {
            return;
        };
        let handed = [
            last(scripts.startup().commands),
            last(scripts.key("super+t", empty_snapshot()).commands),
            last(scripts.key("super+s", empty_snapshot()).commands),
        ];
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            handed, expected,
            "resize = {resize}: at load, then tiling, then scrolling"
        );
    }
}

/// **The shipped configuration declares no rules**, and hands that over,
/// so a reload that removes a user's rules takes them away; a `user.lua`'s
/// rules replace the empty list whole.
#[test]
fn the_shipped_configuration_declares_no_rules() {
    for (user, wanted) in [
        ("return {}", 0),
        (
            "return { effects = { rules = { { match = '*', part = 'client', slot = 'behind', effect = 'glow' } } } }",
            1,
        ),
    ] {
        let Some((directory, mut scripts)) = shipped_init_with_user("solium-effects-none", user)
        else {
            return;
        };
        let unknown = scripts.unknown_settings().len();
        let rules = scripts
            .startup()
            .commands
            .into_iter()
            .find_map(|command| match command {
                Command::Effects { rules, .. } => Some(rules),
                _ => None,
            });
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            rules.expect("sent").expect("parsed").len(),
            wanted,
            "{user}"
        );
        assert_eq!(unknown, 0, "{user}: `effects` is a setting");
    }
}

/// **The shipped configuration writes out the engine's defaults**:
/// `config.lua`'s `effects.sandbox`, `limits` and `present` are the
/// numbers Rust defaults to, so the two cannot drift; and a `user.lua`
/// that raises one reaches the compositor.
#[test]
fn the_shipped_configuration_writes_out_the_engines_defaults() {
    for (user, wanted) in [
        ("return {}", crate::effect::settings::Settings::default()),
        (
            "return { effects = { sandbox = { load_ms = 300 } } }",
            crate::effect::settings::Settings {
                sandbox: crate::effect::settings::Caps {
                    load: std::time::Duration::from_millis(300),
                    ..crate::effect::settings::Caps::default()
                },
                ..crate::effect::settings::Settings::default()
            },
        ),
    ] {
        let Some((directory, mut scripts)) =
            shipped_init_with_user("solium-effects-settings", user)
        else {
            return;
        };
        let unknown = scripts.unknown_settings().len();
        let settings = scripts
            .startup()
            .commands
            .into_iter()
            .find_map(|command| match command {
                Command::Effects { settings, .. } => Some(settings),
                _ => None,
            });
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(settings, Some(Ok(wanted)), "{user}");
        assert_eq!(unknown, 0, "{user}: every key is a setting");
    }
}

/// **The key sets `config.lua` restates are the ones the compositor reads.**
///
/// `keyboard` and `cursor` default to `{}` -- empty *means* "whatever the
/// session already said" -- so the defaults cannot double as the list of
/// what exists, and `config.lua` writes the list out. #117 said in a comment
/// that a reader growing a key would make this report that key as a typo,
/// and called that loud. It was not loud: the list shipped already missing
/// `active`, and nothing failed.
///
/// So the two halves are compared here instead, and in both directions. A
/// key the compositor reads and the list omits is a working setting called
/// a typo, which exits `--check` non-zero over nothing; a key the list
/// accepts and nothing reads is #117's original fault, read from the other
/// side -- a real typo waved through.
#[test]
fn every_key_a_section_accepts_is_one_the_compositor_reads() {
    for (section, function) in [("keyboard", "keyboard"), ("cursor", "cursor_theme")] {
        let read = keys_read_by(function);
        let accepted = accepted_keys(section);
        // Either scrape coming back empty would make this agree with
        // anything, and both have a way of doing that quietly: a renamed
        // `open_sections`, or a `sol.` function that stopped being reached.
        assert!(
            read.len() >= 2 && accepted.len() >= 2,
            "sol.{function} was seen reading {read:?} and config.lua's \
             open_sections.{section} accepts {accepted:?}; one of the two walks \
             is broken, not the lists"
        );
        assert_eq!(
            accepted, read,
            "config.lua's open_sections.{section} and the keys sol.{function} \
             actually reads have drifted apart: a key only the compositor reads \
             is a working setting `--check` calls a typo, and a key only the \
             list has is a typo `--check` waves through"
        );
    }
}

#[test]
fn a_tray_action_is_refused_from_a_binding_at_the_lock() {
    let act = |action: &str| Command::Act {
        attempt: 0,
        action: action.to_owned(),
        data: crate::json::Json::Null,
        source: FocusSource::Binding,
    };
    assert!(act("tray.activate").reaches_past_the_lock());
    assert!(act("tray.open_menu").reaches_past_the_lock());
    assert!(!act("audio.mute").reaches_past_the_lock());
    // A media key plays and pauses at the lock: it reaches no window.
    assert!(!act("media.play_pause").reaches_past_the_lock());
}

/// **The shipped `workspaces.lua` declares its workspaces**: one group per
/// monitor when workspaces are per monitor, showing what each monitor shows.
#[test]
fn the_shipped_workspaces_declare_what_each_monitor_shows() {
    let Some((scripts, commands)) = shell_after_monitors(
        "solium-script-test-shipped-workspaces",
        "return { workspaces = { per_monitor = true } }",
    ) else {
        return;
    };
    let _ = scripts;
    let last = commands.iter().rev().find_map(|command| match command {
        Command::Workspaces(declared) => Some(declared.clone()),
        _ => None,
    });
    let declared = last.expect("workspaces.lua declared nothing");
    assert_eq!(
        declared
            .groups
            .iter()
            .map(|group| group.id.as_str())
            .collect::<Vec<_>>(),
        vec!["test-1"]
    );
    assert_eq!(declared.groups[0].showing, vec!["1".to_owned()]);
}

/// **With workspaces not per monitor, one group has every monitor**, and
/// shows what every monitor shows.
#[test]
fn with_workspaces_together_one_group_has_every_monitor() {
    let Some((directory, mut scripts)) = shipped_init_with_user(
        "solium-script-test-workspaces-together",
        "return { workspaces = { per_monitor = false } }",
    ) else {
        return;
    };
    let _ = scripts.startup();
    let mut two = one_screen(&[]);
    let mut right = two.monitors[0].clone();
    right.name = "test-2".to_owned();
    right.whole.x = 1600.0;
    right.area.x = 1600.0;
    right.focused = false;
    two.monitors.push(right);
    let outcome = scripts.monitors_changed(two);
    let groups = outcome
        .commands
        .iter()
        .rev()
        .find_map(|command| match command {
            Command::Workspaces(declared) => Some(
                declared
                    .groups
                    .iter()
                    .map(|group| (group.monitors.clone(), group.showing.clone()))
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        });
    assert_eq!(
        groups,
        Some(vec![(
            vec!["test-1".to_owned(), "test-2".to_owned()],
            vec!["1".to_owned()]
        )]),
        "(monitors, showing) of each group"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **`workspaces.go` from a scene switches the monitor it names**, through
/// `actions.lua` to `workspaces.lua`, which then declares the new state.
/// An id that is no workspace's number, `2.5` or none at all, switches
/// nothing and is logged.
#[test]
fn a_workspaces_go_from_a_scene_switches_the_monitor_it_names() {
    let Some((directory, mut scripts)) = shipped_init_with_user(
        "solium-script-test-workspaces-go",
        "return { workspaces = { per_monitor = true } }",
    ) else {
        return;
    };
    // Two monitors, the pointer on the left one: the right one is named,
    // so it is the one that switches.
    let two = || {
        let mut snapshot = one_screen(&[]);
        let mut right = snapshot.monitors[0].clone();
        right.name = "test-2".to_owned();
        right.whole.x = 1600.0;
        right.area.x = 1600.0;
        right.focused = false;
        snapshot.monitors.push(right);
        snapshot
    };
    let _ = scripts.startup();
    let _ = scripts.monitors_changed(two());
    let go = |scripts: &mut Scripts, data: &str| {
        let data = crate::json::Json::parse(data).expect("valid JSON");
        scripts
            .surface_action("shell", "workspaces.go", &data, two())
            .commands
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::Workspaces(declared) => Some(
                    declared
                        .groups
                        .iter()
                        .map(|group| (group.id.clone(), group.showing.clone()))
                        .collect::<Vec<_>>(),
                ),
                _ => None,
            })
    };
    let mut others = Vec::new();
    let log = logged_while(|| {
        for data in [
            r#"{"id":2.5,"monitor":"test-2"}"#,
            r#"{"monitor":"test-2"}"#,
        ] {
            others.push(go(&mut scripts, data));
        }
    });
    let showing = go(&mut scripts, r#"{"id":"2","monitor":"test-2"}"#);
    assert_eq!(
        (
            showing,
            others,
            log.matches("workspaces.go: answered only as").count()
        ),
        (
            Some(vec![
                ("test-1".to_owned(), vec!["1".to_owned()]),
                ("test-2".to_owned(), vec!["2".to_owned()])
            ]),
            vec![None, None],
            2
        ),
        "(group, showing) after the monitor named switches, not the one in front; \
         declared after each other form; lines logged"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A window that opens is declared on the workspace its monitor
/// shows**, at the layout that places it, so a hosted shell counts it
/// at once.
#[test]
fn a_window_that_opens_is_declared_on_the_workspace_its_monitor_shows() {
    let Some((directory, mut scripts)) = shipped_init_with_user(
        "solium-script-test-workspaces-layout",
        "return { workspaces = { per_monitor = true } }",
    ) else {
        return;
    };
    let _ = scripts.startup();
    let _ = scripts.monitors_changed(one_screen(&[]));
    let _ = scripts.opened(9, one_screen(&[9]));
    let outcome = scripts.relayout(one_screen(&[9]));
    let windows = outcome
        .commands
        .iter()
        .rev()
        .find_map(|command| match command {
            Command::Workspaces(declared) => Some(declared.windows.clone()),
            _ => None,
        });
    assert_eq!(
        windows,
        Some(std::collections::BTreeMap::from([(
            9,
            vec!["1".to_owned()]
        )]))
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A window that opens while floating (the default mode) is declared
/// on the workspace its monitor shows too, with no `relayout` to ask for
/// it**: `"open"` is the only event a floating window ever sees, since
/// nothing arranges it, so a hosted shell reading `Workspaces`/
/// `WindowList` must learn its workspace from `"open"` alone, not only
/// from the `"layout"` a tiling mode would have fired next (a bar that
/// read `Workspaces.showing(monitor).id` never saw a window opened this
/// way).
#[test]
fn a_window_that_opens_while_floating_is_declared_with_no_relayout_to_ask_for_it() {
    let Some((directory, mut scripts)) = shipped_init_with_user(
        "solium-script-test-workspaces-floating-open",
        "return { workspaces = { per_monitor = true } }",
    ) else {
        return;
    };
    let _ = scripts.startup();
    let _ = scripts.monitors_changed(one_screen(&[]));
    // Only `opened`, exactly what a floating window gets: nothing here
    // asks for a layout.
    let outcome = scripts.opened(9, one_screen(&[9]));
    let windows = outcome
        .commands
        .iter()
        .rev()
        .find_map(|command| match command {
            Command::Workspaces(declared) => Some(declared.windows.clone()),
            _ => None,
        });
    assert_eq!(
        windows,
        Some(std::collections::BTreeMap::from([(
            9,
            vec!["1".to_owned()]
        )])),
        "a floating window's open must declare its workspace by itself"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **The declared arrangement is the shape the workspaces make**: the
/// shipped row of four is four by one, not the grid's `rows`; a column
/// is one wide; and a row of none is the one workspace there is.
#[test]
fn the_declared_arrangement_is_the_shape_of_its_workspaces() {
    let mut shapes = Vec::new();
    for (name, user) in [
        ("row", "return {}"),
        (
            "column",
            r#"return { workspaces = { arrangement = "vertical", rows = 3 } }"#,
        ),
        ("none", "return { workspaces = { columns = 0 } }"),
    ] {
        let Some((_scripts, commands)) =
            shell_after_monitors(&format!("solium-script-test-workspaces-shape-{name}"), user)
        else {
            return;
        };
        let declared = commands
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::Workspaces(declared) => Some(declared.clone()),
                _ => None,
            })
            .expect("workspaces.lua declared nothing");
        let last = declared.groups[0]
            .workspaces
            .last()
            .map(|workspace| (workspace.col, workspace.row));
        shapes.push((
            declared.arrangement.kind,
            declared.arrangement.columns,
            declared.arrangement.rows,
            last,
        ));
    }
    assert_eq!(
        shapes,
        vec![
            ("horizontal".to_owned(), 4, 1, Some((4, 1))),
            ("vertical".to_owned(), 1, 3, Some((1, 3))),
            ("horizontal".to_owned(), 1, 1, Some((1, 1))),
        ],
        "(kind, columns, rows, the last workspace's cell)"
    );
}

/// **`windows.send` from a scene moves the window it names**, not the
/// focused one, through `actions.lua` to `workspaces.lua`, which then
/// declares where every window is.
#[test]
fn a_windows_send_from_a_scene_moves_the_window_it_names() {
    let Some((directory, mut scripts)) = shipped_init_with_user(
        "solium-script-test-windows-send",
        "return { workspaces = { per_monitor = true } }",
    ) else {
        return;
    };
    let _ = scripts.startup();
    let _ = scripts.monitors_changed(one_screen(&[7, 8]));
    let data = crate::json::Json::parse(r#"{"id":7,"workspace":"3"}"#).expect("valid JSON");
    let outcome = scripts.surface_action("shell", "windows.send", &data, one_screen(&[7, 8]));
    let windows = outcome
        .commands
        .iter()
        .rev()
        .find_map(|command| match command {
            Command::Workspaces(declared) => Some(declared.windows.clone()),
            _ => None,
        });
    assert_eq!(
        windows,
        Some(std::collections::BTreeMap::from([
            (7, vec!["3".to_owned()]),
            (8, vec!["1".to_owned()])
        ])),
        "window 7 sent to 3, window 8 left on the one its monitor shows"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **`windows.send` names its window by number or by digits, and logs
/// any other form**: `{ id: "8" }` sends window 8 as `{ id: 8 }` would,
/// while `{ id, monitor }`, a window that is not open, no id at all and
/// a workspace that is no workspace's number, `2.5`, each move nothing
/// and say so.
#[test]
fn a_windows_send_names_its_window_by_number_or_digits_and_logs_any_other_form() {
    let Some((directory, mut scripts)) = shipped_init_with_user(
        "solium-script-test-windows-send-forms",
        "return { workspaces = { per_monitor = true } }",
    ) else {
        return;
    };
    let _ = scripts.startup();
    let _ = scripts.monitors_changed(one_screen(&[7, 8]));
    let send = |scripts: &mut Scripts, data: &str| {
        let data = crate::json::Json::parse(data).expect("valid JSON");
        scripts
            .surface_action("shell", "windows.send", &data, one_screen(&[7, 8]))
            .commands
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::Workspaces(declared) => Some(declared.windows.clone()),
                _ => None,
            })
    };
    let digits = send(&mut scripts, r#"{"id":"8","workspace":"2"}"#);
    let mut others = Vec::new();
    let log = logged_while(|| {
        for data in [
            r#"{"id":7,"monitor":"test-1"}"#,
            r#"{"id":99,"workspace":"2"}"#,
            r#"{"workspace":"2"}"#,
            r#"{"id":7,"workspace":2.5}"#,
        ] {
            others.push(send(&mut scripts, data));
        }
    });
    assert_eq!(
        (
            digits,
            others,
            log.matches("windows.send: answered only as").count()
        ),
        (
            Some(std::collections::BTreeMap::from([
                (7, vec!["1".to_owned()]),
                (8, vec!["2".to_owned()])
            ])),
            vec![None, None, None, None],
            4
        ),
        "(declared after an id in digits, declared after each other form, lines logged)"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **An override stopped three times is taken out alone**: it is a
/// `surface` listener of its own, so the stops are its and not the
/// router's. Every other action is still routed, and its own action
/// goes to the compositor again.
#[test]
fn an_override_stopped_three_times_is_taken_out_alone() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-actions-override-stopped",
        &format!(
            r#"package.path = {shipped:?} .. "/?.lua"
local actions = require("actions")
runs = 0
actions.override("windows.close", function() runs = runs + 1; while true do end end)"#,
            shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
        ),
    );
    let acts = |outcome: &Outcome| -> Vec<(String, String)> {
        outcome
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::Act { action, data, .. } => Some((action.clone(), data.render())),
                _ => None,
            })
            .collect()
    };
    let id = |id: u64| crate::json::Json::parse(&format!(r#"{{"id":{id}}}"#)).expect("valid JSON");
    let stopped: Vec<Vec<(String, String)>> = (0..3)
        .map(|_| acts(&scripts.surface_action("shell", "windows.close", &id(4), one_screen(&[]))))
        .collect();
    let focused = scripts.surface_action("shell", "windows.focus", &id(3), one_screen(&[]));
    let closed = scripts.surface_action("shell", "windows.close", &id(5), one_screen(&[]));
    assert_eq!(
        (
            stopped,
            acts(&focused),
            acts(&closed),
            scripts.evaluate("return tostring(runs)")
        ),
        (
            vec![Vec::new(), Vec::new(), Vec::new()],
            vec![("windows.focus".to_owned(), r#"{"id":3}"#.to_owned())],
            vec![("windows.close".to_owned(), r#"{"id":5}"#.to_owned())],
            "3".to_owned()
        ),
        "(the acts of the three stopped closes, another action's acts, \
         the same action's acts once the override is out, how many times it ran)"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **An override of `nil` gives its action back to the compositor**, as
/// it did when the overrides were a table: no error, and `sol.act` hears
/// the action.
#[test]
fn an_override_of_nil_gives_its_action_back_to_the_compositor() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-actions-override-nil",
        &format!(
            r#"package.path = {shipped:?} .. "/?.lua"
local actions = require("actions")
actions.override("windows.close", function() heard = true end)
actions.override("windows.close", nil)"#,
            shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
        ),
    );
    let data = crate::json::Json::parse(r#"{"id":4}"#).expect("valid JSON");
    let mut outcome = None;
    let log = logged_while(|| {
        outcome = Some(scripts.surface_action("shell", "windows.close", &data, one_screen(&[])));
    });
    let acts: Vec<(String, String)> = outcome
        .iter()
        .flat_map(|outcome| outcome.commands.iter())
        .filter_map(|command| match command {
            Command::Act { action, data, .. } => Some((action.clone(), data.render())),
            _ => None,
        })
        .collect();
    assert_eq!(
        (
            scripts.evaluate("return tostring(heard)"),
            acts,
            log.contains("failed")
        ),
        (
            "nil".to_owned(),
            vec![("windows.close".to_owned(), r#"{"id":4}"#.to_owned())],
            false
        ),
        "(whether the replaced override heard it, the acts queued, whether a listener failed):\n{log}"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A stopped override is logged where it was stopped**: the listener
/// struck is `actions.lua`'s own, so the log also names the file and line
/// the handler's time ran out at, which is the override's.
#[test]
fn a_stopped_override_is_logged_where_it_was_stopped() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-actions-override-where",
        &format!(
            r#"package.path = {shipped:?} .. "/?.lua"
local actions = require("actions")
actions.override("windows.close", function()
while true do end
end)"#,
            shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
        ),
    );
    let data = crate::json::Json::parse(r#"{"id":4}"#).expect("valid JSON");
    let log = logged_while(|| {
        let _ = scripts.surface_action("shell", "windows.close", &data, one_screen(&[]));
    });
    let at = format!("stopped_at={}:4", directory.join("init.lua").display());
    assert!(
        log.lines()
            .any(|line| line.contains("was stopped") && line.contains(&at)),
        "the stop names {at}:\n{log}"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A later override of the same name replaces the earlier one**: only
/// the last answers, and the action does not also go to the compositor.
#[test]
fn a_later_override_of_the_same_name_replaces_the_earlier_one() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-actions-override-replaced",
        &format!(
            r#"package.path = {shipped:?} .. "/?.lua"
local actions = require("actions")
heard = ""
actions.override("windows.close", function() heard = heard .. "first;" end)
actions.override("windows.close", function() heard = heard .. "second;" end)"#,
            shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
        ),
    );
    let data = crate::json::Json::parse(r#"{"id":4}"#).expect("valid JSON");
    let outcome = scripts.surface_action("shell", "windows.close", &data, one_screen(&[]));
    assert_eq!(
        (
            scripts.evaluate("return heard"),
            format!("{:?}", outcome.commands)
        ),
        ("second;".to_owned(), "[]".to_owned()),
        "(which overrides answered, the commands queued)"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **An error in the configuration names it as Lua names a file**,
/// `<path>:<line>:`, not as a string chunk, `[string "<path>"]:<line>:`.
/// Lua shortens a long path from the front, so only its end is asserted.
#[test]
fn an_error_in_the_configuration_names_it_as_a_file() {
    let directory = std::env::temp_dir().join("solium-script-test-config-error-where");
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    let _ = std::fs::write(&config, "\nerror(\"mine\")\n");
    let said = Scripts::load(&config)
        .err()
        .map(|err| err.to_string())
        .unwrap_or_default();
    assert!(
        said.contains("init.lua:2: mine") && !said.contains("[string"),
        "{said}"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A stopped listener is logged with its file and line**, and so is
/// one taken out.
#[test]
fn a_stopped_listener_is_logged_with_its_file_and_line() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-where",
        "\nsol.on(\"layout\", function() while true do end end)\n",
    );
    let log = logged_while(|| {
        for _ in 0..3 {
            let _ = scripts.relayout(one_screen(&[]));
        }
    });
    let at = format!("file={} line=2", directory.join("init.lua").display());
    let named: Vec<&str> = log.lines().filter(|line| line.contains(&at)).collect();
    assert!(
        named.len() == 4 && named.iter().any(|line| line.contains("taken out")),
        "three stops and one take-out, each naming {at}:\n{log}"
    );
    let _ = std::fs::remove_dir_all(&directory);
}
