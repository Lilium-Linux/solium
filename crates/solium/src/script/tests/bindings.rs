//! Key bindings, and the listeners events go to.

use super::*;

/// **A `user.lua` can add, replace and remove a binding.**
///
/// The fault this is pinned against: bindings were `sol.bind` calls in
/// `init.lua` and `user.lua` was a settings table, so the two could not
/// meet. Binding `super+b` to a browser meant either editing a file the
/// next update overwrites or writing your own `init.lua` -- two hundred
/// lines of layout wiring, mode setup and terminal detection adopted in
/// order to add one key (#117).
///
/// Three assertions, because `config.bindings` makes three promises and
/// only the first is the obvious one:
///
///  1. A combination nothing else uses is bound, and runs what it says.
///  2. A combination `init.lua` already bound is **replaced** -- the
///     decision recorded in `config.lua`'s comment, and the one that makes
///     the section worth having: refusing a clash would permit adding a
///     binding and forbid changing one, and changing one is what people
///     come here for.
///  3. `false` removes a shipped binding outright, so the compositor stops
///     swallowing the key rather than binding it to a handler that does
///     nothing.
///
/// It runs the shipped `init.lua`, which is what makes it a test of the
/// ordering as well as the mechanism: `require("bindings")` is the last
/// line of that file precisely so the user's call to `sol.bind` is the
/// later one, and moving it up would break assertion 2 alone.
#[test]
fn a_user_file_can_add_replace_and_remove_a_binding() {
    let Some((directory, mut scripts)) = shipped_init_with_user(
        "solium-script-test-user-bindings",
        r#"
        return {
            bindings = {
                -- Added: nothing in the shipped configuration binds this.
                ["super+f5"] = "fixture-browser",
                -- Replaced: `init.lua` binds super+q to closing a window.
                ["Super+Q"] = { "fixture-editor", "a file.txt" },
                -- Removed: `init.lua` binds super+g to the tilt demo.
                ["super+g"] = false,
            },
        }
        "#,
    ) else {
        return;
    };

    let added = scripts.key("super+f5", empty_snapshot());
    assert!(
        added.handled,
        "a binding from user.lua did not reach sol.bind"
    );
    assert_eq!(
        spawns(&added),
        vec![("fixture-browser".to_owned(), Vec::new())],
        "a string binding is a command line"
    );

    // Written `Super+Q` in the fixture and pressed as `super+q`: the
    // compositor normalises, so a configuration is not obliged to guess the
    // spelling `init.lua` happened to use.
    let replaced = scripts.key("super+q", empty_snapshot());
    assert!(replaced.handled);
    assert_eq!(
        spawns(&replaced),
        vec![("fixture-editor".to_owned(), vec!["a file.txt".to_owned()])],
        "the user's super+q did not win: a list keeps its spaces, and the shipped \
         binding closes a window rather than spawning anything"
    );
    assert!(
        !replaced
            .commands
            .iter()
            .any(|command| matches!(command, Command::Close { .. })),
        "the shipped super+q ran as well as the user's, which means both handlers \
         are registered and only one of them can be reached"
    );

    assert!(
        !scripts.has_binding("super+g"),
        "`false` left the combination bound, so the compositor still swallows the key"
    );

    // And `--check` says so, which is the other half of the decision: a
    // binding may replace a shipped one, but not quietly.
    let reported = scripts.bindings();
    let note = |combo: &str| {
        reported
            .iter()
            .find(|binding| binding.combo == combo)
            .unwrap_or_else(|| panic!("{combo} is not in what --check would print"))
            .note
            .clone()
    };
    assert_eq!(note("super+f5").as_deref(), Some("config.bindings"));
    assert_eq!(
        note("super+q").as_deref(),
        Some("config.bindings, replacing a shipped binding"),
        "--check must be able to tell a replacement from an addition"
    );
    let removed = scripts.unbound();
    assert_eq!(
        removed
            .iter()
            .map(|binding| binding.combo.as_str())
            .collect::<Vec<_>>(),
        vec!["super+g"],
        "a binding removed on purpose looks, in a list of what survived, exactly \
         like one that was never there -- so it is reported separately"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_failing_script_does_not_swallow_the_key() {
    let directory = std::env::temp_dir().join("solium-script-test-error");
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"sol.bind("super+e", function() error("deliberate") end)"#,
    )
    .expect("writing the test script");

    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let outcome = scripts.key("super+e", Snapshot::default());
    // Not handled: the key goes to the focused client rather than
    // disappearing into a broken script.
    assert!(!outcome.handled);
    assert!(outcome.commands.is_empty());
}

/// **A reload does not lose the session, which is the whole of issue #116.**
///
/// The bug as it was reported: `super+shift+r` on workspace 3, and the
/// desktop is drawn two screen-widths off-stage with no key that brings it
/// back. Underneath it are three separate losses, and this drives all
/// three through the same sequence `Solium::reload` runs -- the reload,
/// carrying what was kept, then `restore`, `monitors`, `layout`:
///
///  * which workspace each monitor is showing, so the desks are carried
///    relative to the one in view and not to desk 1;
///  * which workspace each *window* is on, so three windows on two desks
///    are still on two desks rather than swept onto one;
///  * which layout is in charge, because a tiled session that comes back
///    believing it is floating toggles the wrong way on the next key.
///
/// It runs the **shipped** `workspaces.lua`, `modes.lua` and `tiling.lua`
/// rather than copies, which is what makes it a test of the files under
/// review. That also means a developer's own `~/.config/solium` would
/// answer `require` ahead of them -- it comes first on `package.path` --
/// so the run is skipped there rather than asserting about somebody's
/// configuration. The same guard, for the same reason, as
/// `a_user_file_using_the_old_key_still_chooses_a_style`.
#[test]
fn a_reload_keeps_the_workspace_the_windows_and_the_layout() {
    if let Some(own) = Scripts::user_config_dir()
        && (own.join("config.lua").exists()
            || own.join("user.lua").exists()
            || own.join("workspaces.lua").exists()
            || own.join("modes.lua").exists())
    {
        return;
    }

    let directory = std::env::temp_dir().join("solium-script-test-reload");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        local workspaces = require("workspaces")
        local modes = require("modes")
        local tiling = require("tiling")

        sol.bind("super+f1", function()
            modes.use("tiling")
            workspaces.go(3)
        end)

        -- What the session believes about itself, in one string, so the
        -- assertions read the scripts' own answer rather than a
        -- reconstruction of it.
        sol.bind("super+f2", function()
            sol.status(string.format(
                "%d %s", workspaces.on("test-1"), tostring(tiling.active)))
        end)
        "#,
    )
    .expect("writing the test script");

    let first = &[7_u64, 8];
    let all = &[7_u64, 8, 9];

    let mut scripts = Scripts::load(&config).expect("loading the test script");
    // The screens arrive, which is the moment the desks can be built at
    // all -- a script's top level runs before any output is placed.
    scripts.monitors_changed(one_screen(first));
    // Two windows open on workspace 1, then the session moves to 3 with
    // tiling on, and a third window opens there.
    scripts.opened(7, one_screen(first));
    scripts.opened(8, one_screen(first));
    scripts.key("super+f1", one_screen(first));
    scripts.opened(9, one_screen(all));

    let before = scripts.key("super+f2", one_screen(all));
    assert_eq!(
        before.status.as_deref(),
        Some("3 true"),
        "the session was not set up: this is before any reload"
    );

    // Exactly what `Solium::reload` does, in the order it does it.
    scripts.reload(&config).expect("reloading the test script");
    let restored = scripts.restored(one_screen(all));
    let monitors = scripts.monitors_changed(one_screen(all));
    let layout = scripts.relayout(one_screen(all));

    let after = scripts.key("super+f2", one_screen(all));
    assert_eq!(
        after.status.as_deref(),
        Some("3 true"),
        "after the reload the scripts believe they are somewhere else: the workspace in \
         view and the layout in charge are both part of the session, not of the file"
    );

    let mut commands = restored.commands;
    commands.extend(monitors.commands);
    commands.extend(layout.commands);

    let who = membership(&commands);
    assert_eq!(
        who.get("desk-1@test-1").map(Vec::as_slice),
        Some([7, 8].as_slice()),
        "the windows that were on workspace 1 are not on desk 1 any more; \
         memberships found: {who:?}"
    );
    assert_eq!(
        who.get("desk-3@test-1").map(Vec::as_slice),
        Some([9].as_slice()),
        "the window that was on workspace 3 is not on desk 3 any more; \
         memberships found: {who:?}"
    );

    // And the desks sit relative to the one in view. Desk 3 is what is
    // being looked at, so it is carried nowhere; desk 1 is two cells to
    // its left, at 1600 x 1.06 each. Off by this is the off-stage
    // desktop the issue was reported as.
    let where_they_sit = carried(&commands);
    assert_eq!(
        where_they_sit.get("desk-3@test-1"),
        Some(&(0.0, 0.0)),
        "the workspace in view is not at the origin; offsets: {where_they_sit:?}"
    );
    let (dx, dy) = where_they_sit
        .get("desk-1@test-1")
        .copied()
        .unwrap_or_default();
    assert!(
        (dx - (-2.0 * 1600.0 * 1.06)).abs() < 0.5 && dy == 0.0,
        "desk 1 sits at {dx},{dy}, which is not two screens to the left of desk 3"
    );
}

/// **A reload that shortens the arrangement still leaves every window
/// somewhere you can reach.**
///
/// The failure the fix for #116 introduced, which the bug it fixed did not
/// have. `workspaces.of` is now carried across the reload, and the
/// `restore` sweep forgot the abandoned *desks* without touching the
/// entries naming them -- so `columns = 4` edited to `columns = 2` left a
/// window remembering workspace 4, in no group at all:
///
///   * `regroup` loops `1..count`, so nothing ever names it -- it is drawn
///     over whatever workspace is in view, on top of windows that belong
///     there;
///   * `visible()` filters it out, so no layout arranges it;
///   * and `go()` clamps to `count()`, so no key switches to where it
///     thinks it is.
///
/// Before #116 `of` was wiped on every reload and the window came back on
/// the workspace in view. The same is true of `showing`, and worse: a
/// session looking at workspace 4 when the arrangement shrinks to two
/// carries *every* surviving desk off to the left of a workspace that no
/// longer exists, which is the off-stage desktop the issue was reported as,
/// reached by the other door.
///
/// Same shape as the reload test above, with the configuration edited
/// between the two loads -- which is what a reload is for.
#[test]
fn a_reload_with_fewer_workspaces_leaves_no_window_on_a_desk_that_is_gone() {
    if let Some(own) = Scripts::user_config_dir()
        && (own.join("config.lua").exists()
            || own.join("user.lua").exists()
            || own.join("workspaces.lua").exists()
            || own.join("modes.lua").exists())
    {
        return;
    }

    let directory = std::env::temp_dir().join("solium-script-test-reload-shorter");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        local workspaces = require("workspaces")

        sol.bind("super+f1", function() workspaces.go(3) end)
        sol.bind("super+f2", function() workspaces.go(4) end)
        sol.bind("super+f3", function()
            sol.status(string.format("%d/%d",
                workspaces.on("test-1"), workspaces.count()))
        end)
        "#,
    )
    .expect("writing the test script");

    let first = &[7_u64];
    let all = &[7_u64, 9];

    let mut scripts = Scripts::load(&config).expect("loading the test script");
    scripts.monitors_changed(one_screen(first));
    // One window on workspace 1, then to workspace 3, a window opens there,
    // and the session is left looking at workspace 4.
    scripts.opened(7, one_screen(first));
    scripts.key("super+f1", one_screen(first));
    scripts.opened(9, one_screen(all));
    scripts.key("super+f2", one_screen(all));

    let before = scripts.key("super+f3", one_screen(all));
    assert_eq!(
        before.status.as_deref(),
        Some("4/4"),
        "the session was not set up: this is before any reload"
    );

    // The edit. `require` searches the configuration's own directory first,
    // so the *second* load finds this and the first did not.
    std::fs::write(
        directory.join("user.lua"),
        "return { workspaces = { columns = 2 } }\n",
    )
    .expect("writing the fixture user.lua");

    scripts.reload(&config).expect("reloading the test script");
    let restored = scripts.restored(one_screen(all));
    let monitors = scripts.monitors_changed(one_screen(all));
    let layout = scripts.relayout(one_screen(all));

    let after = scripts.key("super+f3", one_screen(all));
    assert_eq!(
        after.status.as_deref(),
        Some("2/2"),
        "after the reload the scripts are still looking at a workspace the \
         arrangement no longer has, and `go` clamps to the count -- so no key \
         switches away from it"
    );

    let mut commands = restored.commands;
    commands.extend(monitors.commands);
    commands.extend(layout.commands);

    let who = membership(&commands);
    assert_eq!(
        who.get("desk-1@test-1").map(Vec::as_slice),
        Some([7].as_slice()),
        "the window on workspace 1 is not on desk 1 any more; memberships: {who:?}"
    );
    assert_eq!(
        who.get("desk-2@test-1").map(Vec::as_slice),
        Some([9].as_slice()),
        "the window kept on workspace 3 is in no selection at all: nothing names \
         it, so no layout arranges it and no key reaches it; memberships: {who:?}"
    );

    // And the desk in view is the one in view, rather than one cell to the
    // left of a workspace that no longer exists.
    let where_they_sit = carried(&commands);
    assert_eq!(
        where_they_sit.get("desk-2@test-1"),
        Some(&(0.0, 0.0)),
        "every surviving desk is carried off-stage, relative to a workspace the \
         arrangement no longer has; offsets: {where_they_sit:?}"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// **A scene's action reaches Lua with its surface's name and its data.**
#[test]
fn a_surface_action_reaches_lua_with_its_data() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-action-data",
        r#"sol.on("surface", function(surface, action, data) sol.status(surface .. " " .. action .. " " .. data.id) end)"#,
    );
    let data = crate::json::Json::parse(r#"{"id":7}"#).expect("valid JSON");
    let outcome = scripts.surface_action("shell", "windows.focus", &data, one_screen(&[]));
    assert_eq!(outcome.status.as_deref(), Some("shell windows.focus 7"));
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A frame's listeners hear the window, the name and the data**
/// (#417): `sol.on("frame", function(id, name, data))`.
#[test]
fn frame_listeners_hear_the_window_the_name_and_the_data() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-frame-action",
        r#"sol.on("frame", function(id, name, data) sol.status(id .. " " .. name .. " " .. data.by) end)"#,
    );
    let data = crate::json::Json::parse(r#"{"by":0.05}"#).expect("valid JSON");
    let outcome = scripts.frame_action(7, "volume", &data, one_screen(&[]));
    assert_eq!(outcome.status.as_deref(), Some("7 volume 0.05"));
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn an_action_with_no_data_reaches_lua_as_nil() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-action-nil",
        r#"sol.on("surface", function(surface, action, data) sol.status(tostring(data)) end)"#,
    );
    let outcome = scripts.surface_action(
        "tweaks",
        "pane:border",
        &crate::json::Json::Null,
        one_screen(&[]),
    );
    assert_eq!(outcome.status.as_deref(), Some("nil"));
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn the_first_notification_hook_that_answers_decides() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-notification-hook",
        r#"sol.on("notification", function(n) seen = n.summary end)
           sol.on("notification", function(n) if n.urgency == "low" then return { banner = false } end end)
           sol.on("notification", function(n) return { banner = true, keep = false } end)"#,
    );
    let arrival = |urgency: &str| {
        crate::json::Json::Object(std::collections::BTreeMap::from([
            (
                "summary".to_owned(),
                crate::json::Json::Text("Hi".to_owned()),
            ),
            (
                "urgency".to_owned(),
                crate::json::Json::Text(urgency.to_owned()),
            ),
        ]))
    };
    let (low, _) = scripts.notification_decision(&arrival("low"), one_screen(&[]));
    assert_eq!(
        low.map(|table| table.render()),
        Some(r#"{"banner":false}"#.to_owned())
    );
    let (normal, _) = scripts.notification_decision(&arrival("normal"), one_screen(&[]));
    assert_eq!(
        normal.map(|table| table.render()),
        Some(r#"{"banner":true,"keep":false}"#.to_owned())
    );
    assert_eq!(scripts.evaluate("return seen"), "Hi");
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn notification_events_reach_their_listeners_with_their_origin() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-notification-events",
        r#"sol.on("notifications", function(e) sol.status(e.what .. " " .. e.origin .. " " .. e.id .. " " .. tostring(e.reason)) end)"#,
    );
    let event = crate::json::Json::Object(std::collections::BTreeMap::from([
        (
            "what".to_owned(),
            crate::json::Json::Text("closed".to_owned()),
        ),
        (
            "origin".to_owned(),
            crate::json::Json::Text("scene".to_owned()),
        ),
        ("id".to_owned(), crate::json::Json::Number(7.0)),
        (
            "reason".to_owned(),
            crate::json::Json::Text("dismissed".to_owned()),
        ),
    ]));
    let outcome = scripts.notifications_event(&event, one_screen(&[]));
    assert_eq!(outcome.status.as_deref(), Some("closed scene 7 dismissed"));
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn audio_events_reach_their_listeners_with_their_origin() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-audio-events",
        r#"sol.on("audio", function(e) sol.status(e.what .. " " .. e.origin .. " " .. e.bus .. " " .. tostring(e.output)) end)"#,
    );
    let event = crate::services::audio::event_json(&crate::services::audio::model::Event {
        what: "mute",
        id: 3,
        direction: crate::services::audio::model::Direction::Output,
        output: true,
        value: 0.4,
        muted: true,
        origin: crate::services::audio::volume::Origin::External,
        bus: crate::services::audio::model::Kind::Bluetooth,
        stream: false,
    });
    let outcome = scripts.audio_event(&event, one_screen(&[]));
    assert_eq!(
        outcome.status.as_deref(),
        Some("mute external bluetooth true")
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn tray_events_reach_their_listeners_with_their_origin() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-tray-events",
        r#"sol.on("tray", function(e) sol.status(e.what .. " " .. e.origin .. " " .. e.id .. " " .. tostring(e.open) .. " " .. e.item.title) end)"#,
    );
    let view = crate::services::tray::View {
        items: vec![std::collections::BTreeMap::from([
            ("id", crate::json::Json::Number(4.0)),
            ("title", crate::json::Json::Text("Steam".to_owned())),
            ("menuOpen", crate::json::Json::Bool(true)),
        ])],
        ..crate::services::tray::View::default()
    };
    let event = crate::services::tray::Event {
        what: "menu",
        id: 4,
        origin: crate::services::notifications::Origin::Scene,
    }
    .to_json(&view);
    let outcome = scripts.tray_event(&event, one_screen(&[]));
    assert_eq!(outcome.status.as_deref(), Some("menu scene 4 true Steam"));
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn media_events_reach_their_listeners_with_the_player() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-media-events",
        r#"sol.on("media", function(e) sol.status(e.what .. " " .. e.id .. " " .. e.player.title) end)"#,
    );
    let seen = crate::services::media::Seen {
        view: std::rc::Rc::new(crate::services::media::View {
            players: vec![std::collections::BTreeMap::from([
                ("id", crate::json::Json::Number(3.0)),
                ("title", crate::json::Json::Text("Song".to_owned())),
            ])],
            ..crate::services::media::View::default()
        }),
        now: std::time::Duration::ZERO,
    };
    let event = crate::services::media::Event {
        what: "active",
        id: 3,
    }
    .to_json(&seen);
    let outcome = scripts.media_event(&event, one_screen(&[]));
    assert_eq!(outcome.status.as_deref(), Some("active 3 Song"));
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A stopped binding is logged where it was written and where it was
/// stopped**, as a stopped listener is, and is left unhandled, as a
/// binding that fails is.
#[test]
fn a_stopped_binding_is_logged_where_it_was_written_and_stopped() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-binding-where",
        "sol.bind(\"super+x\", function()\n    while true do end\nend)",
    );
    let mut handled = None;
    let log = logged_while(|| {
        handled = Some(scripts.key("super+x", one_screen(&[])).handled);
    });
    let init = directory.join("init.lua");
    let (written, stopped) = (
        format!("file={} line=1", init.display()),
        format!("stopped_at={}:2", init.display()),
    );
    assert_eq!(
        (
            handled,
            log.lines().any(|line| line.contains("was stopped")
                && line.contains("event=\"binding\"")
                && line.contains(&written)
                && line.contains(&stopped)),
        ),
        (Some(false), true),
        "(whether the key was handled, whether the stop names {written} and {stopped}):\n{log}"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A listener that never returns is stopped, and the others still run**
/// (03 §3.3.3): the compositor answers within the deadline.
#[test]
fn a_listener_that_never_returns_is_stopped_and_the_others_still_run() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline",
        r#"
        sol.on("layout", function() while true do end end)
        sol.on("layout", function() sol.status("ran") end)
        "#,
    );
    let started = std::time::Instant::now();
    let outcome = scripts.relayout(one_screen(&[]));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "took {:?}",
        started.elapsed()
    );
    assert_eq!(outcome.status.as_deref(), Some("ran"));
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_binding_that_never_returns_is_stopped() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-binding",
        r#"sol.bind("super+x", function() while true do end end)"#,
    );
    let started = std::time::Instant::now();
    let _ = scripts.key("super+x", one_screen(&[]));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "took {:?}",
        started.elapsed()
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A listener stopped three times is taken out until the next reload.**
#[test]
fn a_listener_stopped_three_times_is_taken_out() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-strikes",
        r#"
        runs = 0
        sol.on("layout", function() runs = runs + 1; sol.status(tostring(runs)); while true do end end)
        "#,
    );
    for _ in 0..4 {
        let _ = scripts.relayout(one_screen(&[]));
    }
    let outcome = scripts.relayout(one_screen(&[]));
    assert_eq!(
        (outcome.status, scripts.evaluate("return tostring(runs)")),
        (None, "3".to_owned()),
        "(what a fifth call said, how many times the listener ran)"
    );
    let mut reloaded =
        Scripts::load(&directory.join("init.lua")).expect("loading the test script again");
    assert_eq!(
        reloaded.relayout(one_screen(&[])).status.as_deref(),
        Some("1"),
        "a reload did not put the listener back"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **Each listener has the whole deadline**: the clock starts again for
/// the next listener, so one that needs a few milliseconds still runs
/// after another was stopped.
#[test]
fn each_listener_has_the_whole_deadline() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-each",
        r#"
        sol.on("layout", function() while true do end end)
        sol.on("layout", function() for _ = 1, 200000 do end sol.status("whole") end)
        "#,
    );
    let outcome = scripts.relayout(one_screen(&[]));
    assert_eq!(outcome.status.as_deref(), Some("whole"));
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A handler cannot put its own deadline off**: nothing the
/// configuration can call starts the clock again, so a loop that tries
/// to through `sol._deadline` is stopped like any other. It gives up
/// after two seconds, so the test ends either way.
#[test]
fn a_handler_that_calls_sol_deadline_is_still_stopped() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-restart-called",
        r#"sol.on("layout", function()
               local started = os.time()
               while os.time() - started < 2 do
                   if type(sol._deadline) == "function" then sol._deadline() end
               end
               sol.status("gave up")
           end)"#,
    );
    let outcome = scripts.relayout(one_screen(&[]));
    assert_eq!(outcome.status, None, "the handler was not stopped");
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A configuration that replaces `sol._deadline` still leaves each
/// listener its own deadline**: the compositor starts each listener's
/// clock itself, so the one after a stopped listener still runs.
#[test]
fn replacing_sol_deadline_leaves_each_listener_its_own_deadline() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-restart-replaced",
        r#"
        sol._deadline = function() end
        sol.on("layout", function() while true do end end)
        sol.on("layout", function() for _ = 1, 200000 do end sol.status("whole") end)
        "#,
    );
    let outcome = scripts.relayout(one_screen(&[]));
    assert_eq!(outcome.status.as_deref(), Some("whole"));
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A loop inside a coroutine is stopped too**: a coroutine the handler
/// makes is under the same deadline as the handler.
#[test]
fn a_listener_that_never_returns_inside_a_coroutine_is_stopped() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-coroutine",
        r#"
        sol.on("layout", function() coroutine.wrap(function() while true do end end)() end)
        sol.on("layout", function() sol.status("ran") end)
        "#,
    );
    let started = std::time::Instant::now();
    let outcome = scripts.relayout(one_screen(&[]));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "took {:?}",
        started.elapsed()
    );
    assert_eq!(outcome.status.as_deref(), Some("ran"));
    let _ = std::fs::remove_dir_all(&directory);
}

/// **The clock stops when the dispatch ends**: Lua run between
/// dispatches is no handler's, and is not stopped however long after one
/// it runs.
#[test]
fn lua_run_between_dispatches_is_not_stopped() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-between",
        r#"sol.on("layout", function() end)"#,
    );
    let _ = scripts.relayout(one_screen(&[]));
    std::thread::sleep(std::time::Duration::from_millis(150));
    assert_eq!(
        scripts.evaluate("for _ = 1, 200000 do end return 'finished'"),
        "finished"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A loop that catches the stop with `pcall` is still stopped**, and
/// the stop counts against the listener.
#[test]
fn a_listener_that_retries_with_pcall_is_stopped() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-pcall",
        r#"
        sol.on("layout", function() repeat until pcall(function() while true do end end) end)
        sol.on("layout", function() sol.status("ran") end)
        "#,
    );
    let started = std::time::Instant::now();
    let outcome = scripts.relayout(one_screen(&[]));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "took {:?}",
        started.elapsed()
    );
    assert_eq!(
        (
            outcome.status.as_deref(),
            scripts.evaluate(
                "local n = 0 for _, count in pairs(sol._strikes) do n = n + count end return tostring(n)"
            )
        ),
        (Some("ran"), "1".to_owned()),
        "(what the next listener said, the strikes counted)"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A loop that catches the stop with `xpcall` is still stopped.**
#[test]
fn a_listener_that_retries_with_xpcall_is_stopped() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-xpcall",
        r#"
        sol.on("layout", function()
            repeat until xpcall(function() while true do end end, function(err) return err end)
        end)
        sol.on("layout", function() sol.status("ran") end)
        "#,
    );
    let started = std::time::Instant::now();
    let outcome = scripts.relayout(one_screen(&[]));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "took {:?}",
        started.elapsed()
    );
    assert_eq!(outcome.status.as_deref(), Some("ran"));
    let _ = std::fs::remove_dir_all(&directory);
}

/// **`pcall` and `xpcall` still catch an ordinary error in a handler**,
/// and still hand back every value.
#[test]
fn pcall_still_catches_an_ordinary_error_in_a_handler() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-pcall-ordinary",
        r#"
        sol.on("layout", function()
            local ok, err = pcall(error, "mine", 0)
            local good, a, b = pcall(function() return 1, 2 end)
            local xok, xerr = xpcall(function() error("theirs", 0) end, function(e) return "handled " .. e end)
            sol.status(table.concat({ tostring(ok), err, tostring(good), a, b, tostring(xok), xerr }, " "))
        end)
        "#,
    );
    assert_eq!(
        scripts.relayout(one_screen(&[])).status.as_deref(),
        Some("false mine true 1 2 false handled theirs")
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A coroutine can still yield inside `pcall` and `xpcall`**, as it
/// can in plain Lua 5.4.
#[test]
fn a_coroutine_can_still_yield_inside_pcall() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-pcall-yield",
        r#"
        sol.on("layout", function()
            local co = coroutine.wrap(function()
                local _, first = pcall(function() return coroutine.yield("one") end)
                local _, second = xpcall(function() return coroutine.yield("two") end, function(e) return e end)
                return first .. " " .. second
            end)
            sol.status(table.concat({ co(), co("a"), co("b") }, " "))
        end)
        "#,
    );
    assert_eq!(
        scripts.relayout(one_screen(&[])).status.as_deref(),
        Some("one two a b")
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **`xpcall`'s message handler is not called for the stop**: Lua calls
/// it from inside the instruction hook that raised the stop, where no hook
/// runs, so one that never returned there would never be stopped.
#[test]
fn an_xpcall_handler_that_never_returns_is_not_called_for_the_stop() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-xpcall-handler",
        r#"
        sol.on("layout", function()
            xpcall(function() while true do end end, function() while true do end end)
        end)
        sol.on("layout", function() sol.status("ran") end)
        "#,
    );
    let started = std::time::Instant::now();
    let outcome = scripts.relayout(one_screen(&[]));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "took {:?}",
        started.elapsed()
    );
    assert_eq!(outcome.status.as_deref(), Some("ran"));
    let _ = std::fs::remove_dir_all(&directory);
}

/// **An `xpcall` message handler that never returns is stopped**, when
/// the error it handles is an ordinary one.
#[test]
fn an_xpcall_handler_that_never_returns_is_stopped() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-xpcall-handler-ordinary",
        r#"
        sol.on("layout", function()
            xpcall(function() error("mine") end, function() while true do end end)
        end)
        sol.on("layout", function() sol.status("ran") end)
        "#,
    );
    let started = std::time::Instant::now();
    let outcome = scripts.relayout(one_screen(&[]));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "took {:?}",
        started.elapsed()
    );
    assert_eq!(outcome.status.as_deref(), Some("ran"));
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A loop that catches the stop in `load`'s reader is still stopped.**
#[test]
fn a_listener_that_retries_load_is_stopped() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-load",
        r#"
        sol.on("layout", function() repeat until load(function() while true do end end) end)
        sol.on("layout", function() sol.status("ran") end)
        "#,
    );
    let started = std::time::Instant::now();
    let outcome = scripts.relayout(one_screen(&[]));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "took {:?}",
        started.elapsed()
    );
    assert_eq!(outcome.status.as_deref(), Some("ran"));
    let _ = std::fs::remove_dir_all(&directory);
}

/// **`load` still answers a chunk, or `nil` and why**, and still takes a
/// reader, a name, a mode and an environment.
#[test]
fn load_still_answers_a_chunk_or_nil_and_why() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-load-ordinary",
        r#"
        sol.on("layout", function()
            local bad, why = load("return (")
            local good = load("return x", "=mine", "t", { x = 5 })
            local pieces, at = { "return ", "7" }, 0
            local read = load(function() at = at + 1 return pieces[at] end)
            sol.status(table.concat({ tostring(bad), type(why), good(), read() }, " "))
        end)
        "#,
    );
    assert_eq!(
        scripts.relayout(one_screen(&[])).status.as_deref(),
        Some("nil string 5 7")
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **The deadline cannot reach a `__close` run by a stop**: the
/// to-be-closed variables of the function a stop interrupts are closed
/// inside the instruction hook, where no hook runs, so one runs to its end
/// however late it is. Bounded here, so the test ends.
#[test]
fn a_close_run_after_a_stop_is_not_under_the_deadline() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-close",
        r#"
        sol.on("layout", function()
            local guard <close> = setmetatable({}, { __close = function()
                for _ = 1, 1e6 do end
                closed = true
            end })
            while true do end
        end)
        "#,
    );
    let _ = scripts.relayout(one_screen(&[]));
    assert_eq!(scripts.evaluate("return tostring(closed)"), "true");
    let _ = std::fs::remove_dir_all(&directory);
}

/// **The deadline cannot reach a `__gc` finalizer**: Lua runs one with no
/// hook, so it runs to its end however long it takes. Bounded here, so
/// the test ends.
#[test]
fn a_gc_finalizer_is_not_under_the_deadline() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-gc",
        r#"
        sol.on("layout", function()
            setmetatable({}, { __gc = function()
                for _ = 1, 1e8 do end
                finalized = true
            end })
            collectgarbage()
        end)
        "#,
    );
    let started = std::time::Instant::now();
    let _ = scripts.relayout(one_screen(&[]));
    assert!(
        started.elapsed() >= HANDLER_DEADLINE,
        "the finalizer ended within the deadline, so it shows nothing: {:?}",
        started.elapsed()
    );
    assert_eq!(scripts.evaluate("return tostring(finalized)"), "true");
    let _ = std::fs::remove_dir_all(&directory);
}

/// **The stops are counted by function**: one function listening for two
/// events is taken out at its third stop, wherever the stops were, of the
/// event it was stopped in, and of the other at its next stop there.
#[test]
fn the_stops_are_counted_by_function_across_events() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-strikes-by-function",
        r#"
        local function loop() while true do end end
        sol.on("layout", loop)
        sol.on("monitors", loop)
        "#,
    );
    let _ = scripts.relayout(one_screen(&[]));
    let _ = scripts.relayout(one_screen(&[]));
    let _ = scripts.monitors_changed(one_screen(&[]));
    let count = "return #sol._handlers.layout .. ' ' .. #sol._handlers.monitors";
    assert_eq!(
        scripts.evaluate(count),
        "1 0",
        "(layout listeners, monitors listeners)"
    );
    let _ = scripts.relayout(one_screen(&[]));
    assert_eq!(
        scripts.evaluate(count),
        "0 0",
        "(layout listeners, monitors listeners)"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A listener written in a chunk of text is logged by the chunk's
/// short name**, as Lua's own messages name it: an `=name` chunk as
/// `name`, and one with no name by its first line, not its whole text.
#[test]
fn a_stopped_listener_from_a_loaded_chunk_is_logged_by_its_short_name() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-where-chunk",
        r#"
        sol.on("layout", load("return function() while true do end end", "=mine")())
        sol.on("layout", load("return function()\n while true do end\n end")())
        "#,
    );
    let log = logged_while(|| {
        let _ = scripts.relayout(one_screen(&[]));
    });
    for at in [
        "file=mine line=1",
        r#"file=[string "return function()..."] line=1"#,
    ] {
        assert!(
            log.lines().any(|line| line.contains(at)),
            "no line names {at}:\n{log}"
        );
    }
    let _ = std::fs::remove_dir_all(&directory);
}

/// A binding a listener makes, or takes away, is declared in the answer to
/// the event it ran for, so the compositor's key filter knows of it before
/// the next key; an answer in which nothing was declared carries no
/// declarations at all, since the key filter asks on every key and the
/// declarations are sent whole.
#[test]
fn a_binding_a_listener_makes_is_declared_in_that_answer_and_only_then() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-declared-by-a-listener",
        r#"
        sol.bind("super+a", function() end)
        local laid = 0
        sol.on("layout", function()
            laid = laid + 1
            if laid == 1 then sol.bind("super+x", function() end, { repeating = true }) end
            if laid == 3 then sol.unbind("super+a", "gone on the third layout") end
        end)
        "#,
    );
    assert!(scripts.has_binding("super+a"), "a binding made at load");
    assert!(!scripts.has_binding("super+x"));

    scripts.relayout(empty_snapshot());
    assert!(scripts.has_binding("Super+X"), "made by a listener");
    assert_eq!(
        scripts
            .binding_options("super+x")
            .map(|asked| asked.repeating),
        Some(true)
    );

    // Straight over the socket, to see the answer as it was written.
    scripts
        .link
        .send(&solium_wm_protocol::Message::Batch(
            solium_wm_protocol::Batch {
                serial: 9,
                at: 0.0,
                changes: Vec::new(),
                events: vec![solium_wm_protocol::Event::Relayout],
            },
        ))
        .expect("sent");
    let Ok(Some(solium_wm_protocol::Message::Answer(answer))) = scripts.link.next(
        Some(std::time::Instant::now() + std::time::Duration::from_secs(10)),
        &crate::wm::events::Answers::over(empty_snapshot()),
    ) else {
        panic!("no answer");
    };
    assert!(
        !answer
            .commands
            .iter()
            .any(|command| matches!(command, solium_wm_protocol::Command::Declare(_))),
        "nothing changed, and the declarations were sent anyway"
    );

    scripts.relayout(empty_snapshot());
    assert!(!scripts.has_binding("super+a"), "taken away by a listener");
    assert_eq!(
        scripts
            .unbound()
            .into_iter()
            .map(|binding| (binding.combo, binding.note))
            .collect::<Vec<_>>(),
        vec![(
            "super+a".to_owned(),
            Some("gone on the third layout".to_owned())
        )]
    );
    let _ = std::fs::remove_dir_all(directory);
}
