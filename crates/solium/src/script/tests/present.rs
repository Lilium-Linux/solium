//! The presentation transform: where a window is drawn, and how it gets there.

use super::*;

/// **A present's motion is resolved against the batch's own timing**
/// (#293): what a property leaves out is `sol.animate`'s, and a present
/// with no `motion` carries none.
#[test]
fn a_present_motion_fills_what_it_leaves_out_from_the_batch() {
    let directory = std::env::temp_dir().join("solium-script-test-present-motion");
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        sol.bind("super+1", function()
            sol.animate({ duration = 300, easing = "outBack" })
            sol.present(1, { x = 0, y = 0, w = 10, h = 10,
                             motion = { opacity = { duration = 500 },
                                        size = { easing = "linear" } } })
            sol.present(2, { x = 0, y = 0, w = 10, h = 10 })
            sol.present_from(3, { x = 0, y = 0, w = 10, h = 10,
                                  motion = { x = { duration = 120 } } })
        end)
        "#,
    )
    .expect("writing the test script");
    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let outcome = scripts.key("super+1", Snapshot::default());
    let channels: Vec<_> = outcome
        .commands
        .iter()
        .map(|command| match command {
            Command::Present { channels, .. } | Command::PresentFrom { channels, .. } => {
                channels.as_deref().copied()
            }
            other => panic!("expected a present command, got {other:?}"),
        })
        .collect();
    let batch = AnimationSpec {
        duration: Duration::from_millis(300),
        easing: Curve::OutBack,
    };
    assert_eq!(
        channels,
        vec![
            Some(crate::present::Channels {
                x: batch,
                y: batch,
                size: AnimationSpec {
                    duration: Duration::from_millis(300),
                    easing: Curve::Linear,
                },
                opacity: AnimationSpec {
                    duration: Duration::from_millis(500),
                    easing: Curve::OutBack,
                },
                client_opacity: batch,
            }),
            None,
            Some(crate::present::Channels {
                x: AnimationSpec {
                    duration: Duration::from_millis(120),
                    easing: Curve::OutBack,
                },
                y: batch,
                size: batch,
                opacity: batch,
                client_opacity: batch,
            }),
        ]
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **`client_opacity` is a number from 0 to 1, and anything else is
/// refused by name** (#296), with the call that was given it.
#[test]
fn a_client_opacity_is_a_number_from_0_to_1_and_anything_else_is_refused() {
    let lua = mlua::Lua::new();
    let read = |source: &str| {
        let table: mlua::Table = lua.load(source).eval().expect("a table");
        super::client_opacity_from(&table, "sol.present")
    };
    assert_eq!(read("return {}").expect("none"), None);
    assert_eq!(
        read("return { client_opacity = 0.25 }").expect("a quarter"),
        Some(0.25)
    );
    assert_eq!(
        read("return { client_opacity = 0 }").expect("an integer"),
        Some(0.0)
    );
    assert_eq!(
        read("return { client_opacity = 1 }").expect("the end"),
        Some(1.0)
    );
    for (bad, says) in [
        ("return { client_opacity = 'half' }", "got a string"),
        ("return { client_opacity = {} }", "got a table"),
        ("return { client_opacity = 1.5 }", "got 1.5"),
        ("return { client_opacity = -0.1 }", "got -0.1"),
        ("return { client_opacity = 0/0 }", "a number from 0 to 1"),
    ] {
        let refused = read(bad).expect_err(bad).to_string();
        assert!(
            refused.contains("sol.present: `client_opacity` is a number from 0 to 1")
                && refused.contains(says),
            "{bad}: {refused}"
        );
    }
}

/// **Both calls carry `client_opacity`, and `motion` times it** (#296):
/// `sol.present`'s is the client's opacity to land on, `present_from`'s
/// the one to start from; a bad one fails the call that was given it.
#[test]
fn both_presents_carry_a_client_opacity_and_motion_times_it() {
    let directory = std::env::temp_dir().join("solium-script-test-client-opacity");
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        sol.bind("super+1", function()
            sol.animate({ duration = 300, easing = "outBack" })
            sol.present(1, { client_opacity = 0,
                             motion = { client_opacity = { duration = 450 } } })
            sol.present_from(2, { x = 0, y = 0, w = 10, h = 10, client_opacity = 0.5 })
            sol.present(3, {})
        end)
        sol.bind("super+2", function()
            sol.present_from(2, { x = 0, y = 0, w = 10, h = 10, client_opacity = 2 })
        end)
        "#,
    )
    .expect("writing the test script");
    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let outcome = scripts.key("super+1", Snapshot::default());
    let read: Vec<_> = outcome
        .commands
        .iter()
        .map(|command| match command {
            Command::Present {
                client_opacity,
                channels,
                ..
            }
            | Command::PresentFrom {
                client_opacity,
                channels,
                ..
            } => (
                *client_opacity,
                channels.as_deref().map(|channels| channels.client_opacity),
            ),
            other => panic!("expected a present command, got {other:?}"),
        })
        .collect();
    assert_eq!(
        read,
        vec![
            (
                Some(0.0),
                Some(AnimationSpec {
                    duration: Duration::from_millis(450),
                    easing: Curve::OutBack,
                })
            ),
            (Some(0.5), None),
            (None, None),
        ]
    );
    let mut refused = None;
    let log = logged_while(|| {
        refused = Some(scripts.key("super+2", Snapshot::default()));
    });
    assert!(
        refused.is_some_and(|outcome| outcome.commands.is_empty())
            && log.contains("sol.present_from")
            && log.contains("client_opacity")
            && log.contains("got 2"),
        "a client_opacity of 2 was not refused by name:\n{log}"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **`sol.present_from` reads a deform for the frame it starts in**,
/// checked as `sol.present`'s is.
#[test]
fn present_from_carries_the_deform_it_starts_in() {
    let directory = std::env::temp_dir().join("solium-script-test-present-from-deform");
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        sol.bind("super+1", function()
            sol.present_from(1, { x = 0, y = 0, w = 60, h = 60,
                deform = { effect = "genie", to = { x = 0, y = 0, w = 60, h = 60 } } })
            sol.present_from(2, { x = 0, y = 0, w = 60, h = 60 })
        end)
        "#,
    )
    .expect("writing the test script");
    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let outcome = scripts.key("super+1", Snapshot::default());
    let effects: Vec<Option<String>> = outcome
        .commands
        .iter()
        .map(|command| match command {
            Command::PresentFrom { deform, .. } => {
                deform.as_ref().map(|deform| deform.effect.clone())
            }
            other => panic!("expected a present_from command, got {other:?}"),
        })
        .collect();
    assert_eq!(effects, vec![Some("genie".to_owned()), None]);
    let _ = std::fs::remove_dir_all(&directory);
}

/// The whole round trip, without a compositor: a script binds a key, the
/// binding runs, and its transforms come back as commands.
#[test]
fn a_script_binds_a_key_and_its_commands_come_back() {
    let directory = std::env::temp_dir().join("solium-script-test");
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        sol.bind("Super+Space", function()
            sol.animate({ duration = 300, easing = "outBack" })
            for _, window in ipairs(sol.windows()) do
                sol.present(window.id, { x = 0, y = 0, w = 100, h = 50 })
            end
            sol.grab_input(true)
            sol.status("test")
        end)
        "#,
    )
    .expect("writing the test script");

    let mut scripts = Scripts::load(&config).expect("loading the test script");
    assert!(scripts.has_binding("super+space"));
    // Normalisation is not just for the binding side.
    assert!(scripts.has_binding("SUPER+space"));
    assert!(!scripts.has_binding("super+tab"));

    let snapshot = Snapshot {
        locked: false,
        notifications: Default::default(),
        audio: Default::default(),
        tray: Default::default(),
        media: Default::default(),
        keyboard: crate::keymap::State::initial(),
        shortcuts_owner: None,
        windows: vec![WindowInfo {
            icon: None,
            client_side: false,
            blur_region: false,
            insets: crate::decoration::Insets::default(),
            id: 7,
            rect: Rect {
                x: 0.0,
                y: 0.0,
                w: 800.0,
                h: 600.0,
            },
            drawn: Drawn::default(),
            title: "a window".to_owned(),
            focused: true,
            monitor: "test-1".to_owned(),
            modal: false,
            parent: Parentage::None,
            leaving: false,
            minimized: false,
            above: false,
            app_id: String::new(),
            min: None,
            max: None,
            cramped: false,
            shown: true,
            fullscreen: false,
            maximized: false,
            style: String::new(),
            x11_type: None,
            accepts_input: true,
            class: None,
            instance: None,
            content_type: "none",
            pid: None,
        }],
        monitors: vec![MonitorInfo {
            name: "test-1".to_owned(),
            area: Rect {
                x: 0.0,
                y: 34.0,
                w: 1600.0,
                h: 866.0,
            },
            whole: Rect {
                x: 0.0,
                y: 0.0,
                w: 1600.0,
                h: 900.0,
            },
            scale: 1.0,
            focused: true,
            primary: true,
            transform: "normal".to_owned(),
            off: false,
        }],
        work_area: Rect {
            x: 0.0,
            y: 34.0,
            w: 1600.0,
            h: 866.0,
        },
        cursor: (0.0, 0.0),
        screens: Vec::new(),
        text_input: None,
        problems: Vec::new(),
        apps: std::rc::Rc::default(),
    };

    let outcome = scripts.key("super+space", snapshot);
    assert!(outcome.handled);
    assert_eq!(outcome.grab, Some(true));
    assert_eq!(outcome.status.as_deref(), Some("test"));
    assert_eq!(outcome.commands.len(), 1);

    match &outcome.commands[0] {
        Command::Present {
            id,
            rect,
            animation,
            ..
        } => {
            assert_eq!(*id, 7);
            assert_eq!(rect.map(|rect| rect.w), Some(100.0));
            // `animate` applied to the batch queued after it.
            assert_eq!(animation.duration, Duration::from_millis(300));
            assert_eq!(animation.easing, Curve::OutBack);
        }
        other => panic!("expected a present command, got {other:?}"),
    }
}

/// **A script names an effect folder, and its params and anchor come
/// through as it wrote them.**
///
/// The join `script::shipped` cannot see: a deform's effect is a folder's
/// name, resolved by the effect host where the command is applied, its
/// params every key but the deform's own, and its anchor.
///
/// The anchor is the half worth pinning. `{ window = 9 }` must survive as
/// an *identity* all the way to the command, because the moment it becomes
/// a rectangle here it is a rectangle measured when the key was pressed.
#[test]
fn a_script_names_an_effect_and_the_engine_answers() {
    use solium_effects::spec::Value as Param;
    let directory = std::env::temp_dir().join("solium-script-test-deform");
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        sol.bind("super+1", function()
            sol.present(1, { deform = { effect = "genie", axis = "left",
                                        spread = 2.5,
                                        to = { x = 10, y = 20, w = 30, h = 40 } } })
            sol.present(2, { deform = { effect = "genie", axis = "auto", progress = 0.5,
                                        seed = 0.25, failed = "hide", on_reload = "keep",
                                        to = { window = 9 } } })
            sol.present(3, { deform = { effect = "nonsense", to = { window = 9 } } })
            sol.present(4, {})
            sol.present(5, { deform = { effect = "genie", tint = { 1, 0, 0, 1 }, on = true,
                                        to = { surface = "dock" } } })
        end)
        "#,
    )
    .expect("writing the test script");

    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let outcome = scripts.key("super+1", Snapshot::default());
    let deforms: Vec<Option<DeformSpec>> = outcome
        .commands
        .iter()
        .map(|command| match command {
            Command::Present { deform, .. } => deform.clone(),
            other => panic!("expected a present command, got {other:?}"),
        })
        .collect();
    assert_eq!(deforms.len(), 5);

    let spec = |effect: &str, params: Vec<(String, Param)>, aim: Aim| DeformSpec {
        effect: effect.to_owned(),
        params,
        axis: None,
        // Not named, so all the way in, which is what one animates
        // towards.
        progress: 1.0,
        aim,
        seed: None,
        failed: None,
        on_reload: None,
    };
    assert_eq!(
        deforms[0],
        Some(DeformSpec {
            axis: Some("left".to_owned()),
            ..spec(
                "genie",
                vec![("spread".to_owned(), Param::Number(2.5))],
                Aim::Rect(Rect {
                    x: 10.0,
                    y: 20.0,
                    w: 30.0,
                    h: 40.0
                }),
            )
        })
    );
    assert_eq!(
        deforms[1],
        Some(DeformSpec {
            axis: Some("auto".to_owned()),
            progress: 0.5,
            seed: Some(0.25),
            failed: Some(crate::effect::settings::PresentFailed::Hide),
            on_reload: Some(crate::effect::settings::PresentReload::Keep),
            ..spec("genie", Vec::new(), Aim::Window(9))
        })
    );
    // A folder nobody has is the effect host's to say, where the command
    // is applied: here it is only a name.
    assert_eq!(
        deforms[2],
        Some(spec("nonsense", Vec::new(), Aim::Window(9)))
    );
    assert_eq!(deforms[3], None);
    // **A surface survives this far as a name.** It becomes a
    // `scripted::SurfaceId` in `Solium::aimed`, which is the first place
    // that can see whether there is a surface by that name -- and the id is
    // what keeps `present::Frame` `Copy`. Params of every kind, sorted.
    assert_eq!(
        deforms[4],
        Some(spec(
            "genie",
            vec![
                ("on".to_owned(), Param::Bool(true)),
                ("tint".to_owned(), Param::Vec4([1.0, 0.0, 0.0, 1.0])),
            ],
            Aim::Surface("dock".to_owned()),
        ))
    );
}

/// **A script says how deep a window is drawn and what it turns about.**
///
/// All of that pair's surface in one script: both keys read, a table
/// mentioning neither producing exactly the frame every window has had
/// until now, and each pivot axis defaulting on its own -- `pivot_x = 0`
/// means the left edge and says nothing about the vertical, so a script
/// naming one axis must not be given two.
///
/// **The two pivot numbers are deliberately different, and neither is a
/// default.** `(0.5, 0.5)` and `(0.0, 0.0)` are each their own transpose,
/// so a fixture built from either cannot tell `pivot_x` read into the
/// wrong half of the pair from the code being right. `(0.25, 1.0)` can,
/// and the two one-axis cases pin the same swap from the other side:
/// naming only `pivot_x` must move the *first* number and only that one.
#[test]
fn a_script_says_how_deep_a_window_is_and_what_it_turns_about() {
    let directory = std::env::temp_dir().join("solium-script-test-depth");
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        sol.bind("super+3", function()
            sol.present(1, { z = 2.5, pivot_x = 0.25, pivot_y = 1.0 })
            -- All four, because `rect_from` refuses half a rect: a table
            -- that mentions neither new key still has to be a table a
            -- script could really write.
            sol.present(2, { x = 10, y = 20, w = 300, h = 200 })
            sol.present(3, { pivot_x = 0.25 })
            sol.present(4, { pivot_y = 1.0 })
            sol.present(5)
        end)
        "#,
    )
    .expect("writing the test script");

    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let outcome = scripts.key("super+3", Snapshot::default());
    let drawn = presented(&outcome.commands);
    assert_eq!(drawn.len(), 5);

    assert!(
        (drawn[0].0 - 2.5).abs() < f32::EPSILON,
        "the depth the script asked for, got {}",
        drawn[0].0
    );
    assert_eq!(drawn[0].1, (0.25, 1.0), "x into x, y into y");

    // A table mentioning neither is the frame every window on the machine
    // has had until now: depth zero, which ties with every other window
    // and so keeps the order the stack gave them, turning about its own
    // centre.
    assert!((drawn[1].0 - 0.0).abs() < f32::EPSILON);
    assert_eq!(drawn[1].1, (0.5, 0.5));

    // One axis named leaves the other in the middle. Asserted from both
    // sides, because one of them alone is satisfied by a reader that
    // defaults the pair together on whichever axis it was given.
    assert_eq!(drawn[2].1, (0.25, 0.5), "only the horizontal moved");
    assert_eq!(drawn[3].1, (0.5, 1.0), "only the vertical moved");

    // And no options table at all answers the same as a table that says
    // nothing -- `sol.present(id)` clears a window back to its own
    // geometry, and it must not sort or hinge differently for it.
    assert!((drawn[4].0 - 0.0).abs() < f32::EPSILON);
    assert_eq!(drawn[4].1, (0.5, 0.5));

    let _ = std::fs::remove_dir_all(&directory);
}

/// **A number no window can be drawn with is dropped, per key and per
/// axis.**
///
/// One division reaches NaN from a script, and the two fields answer it
/// very differently if nothing stops it here.
///
/// A **pivot** is the dangerous one. `warp.rs` computes
/// `loc + size * pivot` for the point the matrix turns about, so a
/// non-finite one makes that centre non-finite and every vertex of the
/// mesh with it -- and nothing further down declines to draw the result,
/// because `Mat4::project_with_w` guards with `out_w <= 1e-6` and every
/// comparison against a NaN is false. The window disappears and its damage
/// rectangle is nonsense, from a typo.
///
/// A **depth** is quieter to look at and no safer. `render::by_depth`
/// answers `Equal` when `partial_cmp` declines, so a NaN ties with 1.0 and
/// with 2.0 while those two do not tie with each other -- not a total
/// order, and `slice::sort_by` handed one of those **may panic**. That is
/// the render walk, so the failure is the session going black.
///
/// So both fall back to the default and say so in the log -- the answer
/// `deform_from` gives an effect this build does not have, and
/// `easing_from` an easing nobody wrote: the script loses the key it
/// mistyped and keeps its window. An **infinite depth is kept**, in both
/// directions, because it orders perfectly well: `z = math.huge` is a
/// legible spelling of "above everything" and `-math.huge` of "behind
/// everything". Both are asserted, because a guard written
/// `z.is_nan() || z == f32::NEG_INFINITY` passes every other case here.
#[test]
fn a_depth_or_a_pivot_that_cannot_be_drawn_with_falls_back() {
    let directory = std::env::temp_dir().join("solium-script-test-nonfinite");
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        sol.bind("super+4", function()
            sol.present(1, { z = 0/0, pivot_x = 0.25 })
            sol.present(2, { pivot_x = 0/0, pivot_y = 1.0 })
            sol.present(3, { pivot_x = 0.25, pivot_y = 1/0 })
            sol.present(4, { pivot_y = -1/0 })
            sol.present(5, { z = 1/0 })
            sol.present(6, { z = -1/0 })
        end)
        "#,
    )
    .expect("writing the test script");

    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let outcome = scripts.key("super+4", Snapshot::default());
    let drawn = presented(&outcome.commands);
    assert_eq!(drawn.len(), 6);

    // The key that was wrong is the only key that loses anything: the
    // pivot beside the NaN depth is the one the script wrote.
    assert!(
        (drawn[0].0 - 0.0).abs() < f32::EPSILON,
        "a NaN depth is zero"
    );
    assert_eq!(drawn[0].1, (0.25, 0.5));

    // And the axis that was wrong is the only axis. This is the second
    // reading of the transpose: a NaN given as `pivot_x` must come back as
    // a centred *first* number beside the 1.0 that was given as `pivot_y`.
    assert_eq!(drawn[1].1, (0.5, 1.0), "the horizontal fell back, alone");
    assert_eq!(
        drawn[2].1,
        (0.25, 0.5),
        "and an infinity no less than a NaN"
    );
    assert_eq!(drawn[3].1, (0.5, 0.5), "in either direction");

    // A depth is not a coordinate and an infinite one sorts, so it is the
    // one non-finite number here that survives -- **in both directions**.
    // Asserted from each end because a guard that refused only one of them
    // (`z.is_nan() || z == f32::NEG_INFINITY`, the plausible one: NaN and
    // the negative infinity are what a division by zero yields when the
    // numerator went wrong) passes every other case in this test.
    assert!(
        drawn[4].0.is_infinite() && drawn[4].0 > 0.0,
        "an infinite depth orders, so it is kept, got {}",
        drawn[4].0
    );
    assert!(
        drawn[5].0.is_infinite() && drawn[5].0 < 0.0,
        "and so does a negative one, which means behind everything, got {}",
        drawn[5].0
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// **A script names a selection, and both halves of it come back.**
///
/// The round trip for the other primitive this file gained: who is in a
/// group, and what carrying it means. The displacement is the part worth
/// pinning -- `x` on `sol.present_group` is a *delta* where `x` on
/// `sol.present` is a destination, because a selection has no rectangle of
/// its own to be moved to, and reading it as a destination would put every
/// member of every group in the same place.
#[test]
fn a_script_names_a_selection_and_says_where_to_carry_it() {
    let directory = std::env::temp_dir().join("solium-script-test-group");
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        sol.bind("super+2", function()
            sol.animate({ duration = 300, easing = "inOutQuad" })
            sol.group("desk-2", {
                windows = { 3, 7 },
                surfaces = { "wallpaper-2" },
                monitors = { "DP-1" },
                monitor = "DP-1",
            })
            sol.present_group("desk-2", { x = -2560, opacity = 0.5 })
            sol.present_group("desk-1", { y = 40 }, { duration = 90 })
            sol.present_group_clear("desk-3")
            sol.group("desk-4", false)
        end)
        "#,
    )
    .expect("writing the test script");

    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let outcome = scripts.key("super+2", Snapshot::default());
    assert_eq!(outcome.commands.len(), 5);

    match &outcome.commands[0] {
        Command::Group {
            name,
            selection: Some(selection),
            animation,
        } => {
            assert_eq!(name, "desk-2");
            assert_eq!(selection.windows, vec![3, 7]);
            assert_eq!(selection.surfaces, vec!["wallpaper-2".to_owned()]);
            assert_eq!(selection.monitors, vec!["DP-1".to_owned()]);
            assert_eq!(selection.on.as_deref(), Some("DP-1"));
            // A membership change is animated too, and by the ambient
            // setting: that is how long a window takes to get back to where
            // it was looking when it changes desks.
            assert_eq!(animation.duration, Duration::from_millis(300));
        }
        other => panic!("expected a group command, got {other:?}"),
    }

    match &outcome.commands[1] {
        Command::PresentGroup {
            name,
            to,
            animation,
        } => {
            assert_eq!(name, "desk-2");
            assert_eq!(to.offset(), (-2560.0, 0.0));
            assert!((to.opacity - 0.5).abs() < f32::EPSILON);
            assert_eq!(animation.duration, Duration::from_millis(300));
            assert_eq!(animation.easing, Curve::InOutQuad);
        }
        other => panic!("expected a present_group command, got {other:?}"),
    }

    // Its own table overrides the ambient duration and keeps the easing.
    match &outcome.commands[2] {
        Command::PresentGroup { animation, to, .. } => {
            assert_eq!(to.offset(), (0.0, 40.0));
            assert_eq!(animation.duration, Duration::from_millis(90));
            assert_eq!(animation.easing, Curve::InOutQuad);
        }
        other => panic!("expected a present_group command, got {other:?}"),
    }

    assert!(matches!(&outcome.commands[3], Command::ClearGroup { name, .. } if name == "desk-3"));
    // `false` removes a selection, the same spelling `sol.surface` takes.
    assert!(matches!(
        &outcome.commands[4],
        Command::Group {
            name,
            selection: None,
            ..
        } if name == "desk-4"
    ));
}

/// **A window going fullscreen or maximised is told to the scripts, each
/// change by its own event, and answered with the timing `sol.animate`
/// set** (#49): the listener hears `(id, entering)`, and what it set is
/// the outcome's `motion`. A dispatch that called nothing answers `None`,
/// which the compositor reads as "at once", and is not the 220 ms a
/// command queued with no `sol.animate` is given.
#[test]
fn a_change_is_answered_with_the_motion_sol_animate_set() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-change-motion",
        r#"
        sol.on("fullscreen", function(id, entering)
            sol.status("fullscreen " .. id .. " " .. tostring(entering))
            sol.animate({ duration = 260, easing = "linear" })
        end)
        sol.on("maximize", function(id, entering)
            sol.status("maximize " .. id .. " " .. tostring(entering))
            if entering then sol.animate({ duration = 120 }) end
        end)
        "#,
    );

    let entering = scripts.fullscreen(7, true, one_screen(&[7]));
    assert_eq!(entering.status.as_deref(), Some("fullscreen 7 true"));
    let motion = entering.motion.expect("the listener set a motion");
    assert_eq!(
        (motion.duration, motion.easing),
        (Duration::from_millis(260), Curve::Linear)
    );

    let leaving = scripts.fullscreen(7, false, one_screen(&[7]));
    assert_eq!(leaving.status.as_deref(), Some("fullscreen 7 false"));

    let maximised = scripts.maximize(7, true, one_screen(&[7]));
    assert_eq!(maximised.status.as_deref(), Some("maximize 7 true"));
    let motion = maximised.motion.expect("the listener set a motion");
    assert_eq!(
        (motion.duration, motion.easing),
        (Duration::from_millis(120), Curve::OutCubic),
        "the easing it did not name is the default's"
    );

    let restored = scripts.maximize(7, false, one_screen(&[7]));
    assert_eq!(restored.status.as_deref(), Some("maximize 7 false"));
    assert!(
        restored.motion.is_none(),
        "nothing called sol.animate, so there is no motion: {:?}",
        restored.motion
    );
    let _ = std::fs::remove_dir_all(&directory);
}
