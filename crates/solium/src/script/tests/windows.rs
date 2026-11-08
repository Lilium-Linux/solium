//! The windows: listing them, focusing, placing, moving and closing them.

use super::*;

/// **`properties` arrive as values, keys sorted**, so a configuration
/// that declares the same table twice declares the same surface twice.
#[test]
fn a_surfaces_properties_reach_the_declaration_as_sorted_values() {
    let directory = std::env::temp_dir().join("solium-script-test-properties");
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"sol.surface("bar", { scene = "/solium-fixture/bar.qml", properties = { b = 2, a = { y = 1, x = "s" } } })"#,
    )
    .expect("writing the test script");
    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let declared: Vec<String> = scripts
        .startup()
        .commands
        .iter()
        .filter_map(|command| match command {
            Command::Surface(declared) => Some(declared.properties.render()),
            _ => None,
        })
        .collect();
    assert_eq!(declared, vec![r#"{"a":{"x":"s","y":1},"b":2}"#.to_owned()]);
    let _ = std::fs::remove_dir_all(&directory);
}

/// **`reserve` names the edges a surface takes out of the work area**,
/// an edge it leaves out reserving nothing (#162, Ruling 10).
#[test]
fn sol_surface_reads_the_edges_it_reserves() {
    let directory = std::env::temp_dir().join("solium-script-test-reserve");
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"sol.surface("bar", { scene = "/solium-fixture/bar.qml", reserve = { bottom = 48, left = 4 } })"#,
    )
    .expect("writing the test script");
    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let reserves: Vec<crate::scripted::Edges> = scripts
        .startup()
        .commands
        .iter()
        .filter_map(|command| match command {
            Command::Surface(declared) => Some(declared.reserve),
            _ => None,
        })
        .collect();
    assert_eq!(
        reserves,
        vec![crate::scripted::Edges {
            bottom: 48,
            left: 4,
            ..Default::default()
        }]
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A negative reserve fails the load**, as a half rect does (Ruling 10).
#[test]
fn a_negative_reserve_is_refused() {
    let directory = std::env::temp_dir().join("solium-script-test-reserve-negative");
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"sol.surface("bar", { scene = "/solium-fixture/bar.qml", reserve = { top = -1 } })"#,
    )
    .expect("writing the test script");
    assert!(
        Scripts::load(&config).is_err(),
        "a negative reserve must fail the load, as a half rect does"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **`outside_click` is a word or a table of them** (Ruling 13): a word
/// is every grab's policy, and a table names grabs, with `default` for
/// the rest; a surface that says nothing swallows.
#[test]
fn sol_surface_reads_outside_click_as_a_word_or_a_table() {
    let directory = std::env::temp_dir().join("solium-script-test-outside");
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        sol.surface("a", { scene = "/solium-fixture/a.qml", outside_click = "pass" })
        sol.surface("b", { scene = "/solium-fixture/b.qml", outside_click = { default = "swallow", ["tray-menu"] = "pass" } })
        sol.surface("c", { scene = "/solium-fixture/c.qml" })
        "#,
    )
    .expect("writing the test script");
    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let policies: Vec<crate::scripted::OutsideClick> = scripts
        .startup()
        .commands
        .iter()
        .filter_map(|command| match command {
            Command::Surface(declared) => Some(declared.outside_click.clone()),
            _ => None,
        })
        .collect();
    let _ = std::fs::remove_dir_all(&directory);
    use crate::scripted::Outside;
    let [a, b, c] = policies.as_slice() else {
        panic!("expected three surfaces, got {policies:?}");
    };
    assert_eq!(
        [
            a.for_grab("anything"),
            b.for_grab("tray-menu"),
            b.for_grab("search"),
            c.for_grab("anything"),
        ],
        [
            Outside::Pass,
            Outside::Pass,
            Outside::Swallow,
            Outside::Swallow
        ],
        "[a word, a grab the table names, one it does not, a surface that says nothing]"
    );
}

/// **`keyboard.bindings` says which bindings still work while the scene
/// holds the keyboard** (Q3, Ruling 14): every one but the claimed keys
/// when it says nothing, as when it says `"except_claimed"`.
#[test]
fn sol_surface_reads_its_keyboard_bindings() {
    let directory = std::env::temp_dir().join("solium-script-test-keyboard-bindings");
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        sol.surface("a", { scene = "/solium-fixture/a.qml", keyboard = { bindings = "all" } })
        sol.surface("b", { scene = "/solium-fixture/b.qml", keyboard = { bindings = "none" } })
        sol.surface("c", { scene = "/solium-fixture/c.qml", keyboard = { bindings = "except_claimed" } })
        sol.surface("d", { scene = "/solium-fixture/d.qml", keyboard = {} })
        sol.surface("e", { scene = "/solium-fixture/e.qml" })
        "#,
    )
    .expect("writing the test script");
    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let policies: Vec<crate::scripted::KeyPolicy> = scripts
        .startup()
        .commands
        .iter()
        .filter_map(|command| match command {
            Command::Surface(declared) => Some(declared.keyboard),
            _ => None,
        })
        .collect();
    let _ = std::fs::remove_dir_all(&directory);
    use crate::scripted::KeyPolicy;
    assert_eq!(
        policies,
        [
            KeyPolicy::All,
            KeyPolicy::NoBindings,
            KeyPolicy::ExceptClaimed,
            KeyPolicy::ExceptClaimed,
            KeyPolicy::ExceptClaimed,
        ],
        "[all, none, except_claimed, an empty table, nothing said]"
    );
}

/// **A `keyboard.bindings` that is none of the three fails the load**, as
/// an unknown `outside_click` does.
#[test]
fn an_unknown_keyboard_bindings_is_refused() {
    let directory = std::env::temp_dir().join("solium-script-test-keyboard-unknown");
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    let mut refused = Vec::new();
    for options in [
        r#"keyboard = { bindings = "some" }"#,
        r#"keyboard = "all""#,
        "keyboard = { bindings = true }",
    ] {
        std::fs::write(
            &config,
            format!(r#"sol.surface("bar", {{ scene = "/solium-fixture/bar.qml", {options} }})"#),
        )
        .expect("writing the test script");
        refused.push(Scripts::load(&config).is_err());
    }
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(
        refused,
        [true, true, true],
        "[a word it does not know, a word, a bool]"
    );
}

/// **An `outside_click` that is neither word fails the load**, as a
/// negative reserve does, rather than quietly swallowing.
#[test]
fn an_unknown_outside_click_is_refused() {
    let directory = std::env::temp_dir().join("solium-script-test-outside-unknown");
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    let mut refused = Vec::new();
    for options in [
        r#"outside_click = "through""#,
        r#"outside_click = { ["tray-menu"] = "through" }"#,
        "outside_click = 1",
    ] {
        std::fs::write(
            &config,
            format!(r#"sol.surface("bar", {{ scene = "/solium-fixture/bar.qml", {options} }})"#),
        )
        .expect("writing the test script");
        refused.push(Scripts::load(&config).is_err());
    }
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(
        refused,
        [true, true, true],
        "[an unknown word, an unknown word for a named grab, not a word or a table]"
    );
}

/// **A `properties` table with a list part keeps its named keys**: the
/// bag is always read as an object, so `{ "x", label = "y" }` loses
/// nothing, as it did not before properties became values.
#[test]
fn a_surfaces_properties_keep_their_named_keys_beside_a_list_part() {
    let directory = std::env::temp_dir().join("solium-script-test-properties-list-part");
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"sol.surface("bar", { scene = "/solium-fixture/bar.qml", properties = { "x", label = "y" } })"#,
    )
    .expect("writing the test script");
    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let declared: Vec<String> = scripts
        .startup()
        .commands
        .iter()
        .filter_map(|command| match command {
            Command::Surface(declared) => Some(declared.properties.render()),
            _ => None,
        })
        .collect();
    assert_eq!(declared, vec![r#"{"1":"x","label":"y"}"#.to_owned()]);
    let _ = std::fs::remove_dir_all(&directory);
}

/// **The three answers about a parent survive the trip into Lua.**
///
/// The whole value of [`Parentage`] is that "no parent" and "a parent I
/// cannot point at" stay apart, and the only place that can be lost is the
/// one line that turns it into a Lua value. A script that has to centre a
/// dialog asks `if window.parent then`, so both of the negative answers
/// have to be falsey -- and a script that wants to tell them apart asks
/// `== false`, so they have to be different values. Both halves are checked
/// here, because getting `Unknown` wrong in the other direction -- `nil` --
/// costs nothing today and quietly deletes the distinction.
///
/// `modal` rides along: it is set on the same line and read by the same
/// scripts, and a boolean that arrives as nil reads as false.
#[test]
fn a_parent_reaches_a_script_as_a_number_false_or_nothing() {
    let directory = std::env::temp_dir().join("solium-script-test-parentage");
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        sol.bind("Super+P", function()
            for _, window in ipairs(sol.windows()) do
                -- Reported as a place, because `sol.place` is the only way
                -- out of a dispatch that carries four numbers.
                sol.place(window.id, {
                    x = window.parent == nil and -1
                        or (window.parent == false and -2 or window.parent),
                    y = window.modal and 1 or 0,
                    w = 1,
                    h = 1,
                })
            end
        end)
        "#,
    )
    .expect("writing the test script");

    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let mut snapshot = empty_snapshot();
    let window = |id: u64, modal: bool, parent: Parentage| WindowInfo {
        icon: None,
        client_side: false,
        blur_region: false,
        insets: crate::decoration::Insets::default(),
        id,
        rect: Rect::default(),
        drawn: Drawn::default(),
        title: String::new(),
        focused: false,
        monitor: "test-1".to_owned(),
        modal,
        parent,
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
    };
    snapshot.windows = vec![
        window(1, false, Parentage::None),
        window(2, true, Parentage::Unknown),
        window(3, true, Parentage::Window(1)),
    ];

    let outcome = scripts.key("super+p", snapshot);
    assert!(outcome.handled);
    let mut seen = Vec::new();
    for command in &outcome.commands {
        if let Command::Place { id, rect, .. } = command {
            seen.push((*id, rect.x, rect.y));
        }
    }
    assert_eq!(
        seen,
        vec![
            // No parent at all: absent from the table, so `nil`.
            (1, -1.0, 0.0),
            // A parent that was named and cannot be pointed at: `false`,
            // which is falsey for `if window.parent then` and still tells
            // a script that asks that something was named.
            (2, -2.0, 1.0),
            // A parent that can be pointed at: its id, as a number.
            (3, 1.0, 1.0),
        ],
        "the parent of a window did not survive the trip into Lua"
    );
}

#[test]
fn sol_act_returns_an_attempt_and_done_hears_the_outcome_once() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-act",
        r#"sol.on("surface", function()
               attempt = sol.act("windows.focus", { id = 9 }, function(ok, reason) sol.status(tostring(ok) .. " " .. tostring(reason)) end)
           end)"#,
    );
    let outcome = scripts.surface_action("shell", "go", &crate::json::Json::Null, one_screen(&[]));
    let acts: Vec<(u64, String, String)> = outcome
        .commands
        .iter()
        .filter_map(|command| match command {
            Command::Act {
                attempt,
                action,
                data,
                ..
            } => Some((*attempt, action.clone(), data.render())),
            _ => None,
        })
        .collect();
    let [(attempt, action, data)] = acts.as_slice() else {
        panic!("one act: {acts:?}")
    };
    assert_eq!(
        scripts.evaluate("return tostring(attempt)"),
        attempt.to_string(),
        "sol.act answered another id than its command carries"
    );
    assert_eq!(
        (action.as_str(), data.as_str()),
        ("windows.focus", r#"{"id":9}"#)
    );
    let settled = [Settled {
        attempt: *attempt,
        ok: false,
        reason: Some("unknown-window"),
    }];
    assert_eq!(
        scripts
            .attempts_settled(&settled, one_screen(&[]))
            .status
            .as_deref(),
        Some("false unknown-window")
    );
    assert_eq!(
        scripts.attempts_settled(&settled, one_screen(&[])).status,
        None,
        "done was called twice"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **`workspaces.lua` does not route a scene's actions by itself**: a
/// configuration without `actions.lua` routes them its own way, and
/// keeping the workspaces does not route each one a second time.
#[test]
fn workspaces_lua_does_not_route_a_scenes_actions_by_itself() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-workspaces-no-actions",
        &format!(
            r#"package.path = {shipped:?} .. "/?.lua"
require("workspaces")
sol.on("surface", function(_, action, data) sol.act(action, data) end)"#,
            shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
        ),
    );
    let data = crate::json::Json::parse(r#"{"id":4}"#).expect("valid JSON");
    let outcome = scripts.surface_action("shell", "windows.close", &data, one_screen(&[4]));
    let acts = outcome
        .commands
        .iter()
        .filter(|command| matches!(command, Command::Act { .. }))
        .count();
    assert_eq!(acts, 1, "windows.close was routed more than once, or never");
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A handler stopped when its `sol.focus_direction` returns is logged
/// at that call**, not with no place, nor at a `direction` listener's
/// line.
#[test]
fn a_handler_stopped_at_its_focus_direction_is_logged_at_that_call() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-direction-where",
        "sol.on(\"direction\", function() end)\nsol.on(\"layout\", function()\n    while true do\n        sol.focus_direction(\"left\")\n    end\nend)",
    );
    let log = logged_while(|| {
        let _ = scripts.relayout(one_screen(&[]));
    });
    // The error carries a traceback here, so the fields are on a line
    // of their own.
    let at = format!("stopped_at={}:4", directory.join("init.lua").display());
    assert!(
        log.contains("was stopped")
            && log
                .lines()
                .any(|line| line.contains("event=\"layout\"") && line.contains(&at)),
        "the stop names {at}:\n{log}"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **Each `sol.act` answers an id of its own, counting up**, with or
/// without a `done`, and data that is no table reaches the command as
/// it is.
#[test]
fn each_sol_act_answers_an_id_of_its_own_counting_up() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-act-ids",
        r#"sol.on("surface", function()
               first = sol.act("windows.close", { id = 1 })
               second = sol.act("windows.close", 2)
           end)"#,
    );
    let outcome = scripts.surface_action("shell", "go", &crate::json::Json::Null, one_screen(&[]));
    let acts: Vec<(u64, String)> = outcome
        .commands
        .iter()
        .filter_map(|command| match command {
            Command::Act { attempt, data, .. } => Some((*attempt, data.render())),
            _ => None,
        })
        .collect();
    let said = |name: &str| -> u64 {
        scripts
            .evaluate(&format!("return tostring({name})"))
            .parse()
            .unwrap_or(0)
    };
    let (first, second) = (said("first"), said("second"));
    assert_eq!(
        (second > first, acts),
        (
            true,
            vec![(first, r#"{"id":1}"#.to_owned()), (second, "2".to_owned())]
        ),
        "(the second id after the first, the commands with their ids and data)"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **Data that contains itself is an error in the handler, not a crash
/// of the compositor**: `sol.act`, `sol.surface`'s properties and
/// `sol.pane_values` each raise an ordinary Lua error, which the handler
/// can catch, and nothing is queued.
#[test]
fn data_that_contains_itself_is_an_error_in_the_handler() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-data-contains-itself",
        r#"sol.on("surface", function()
               local t = {} t.t = t
               local said = {}
               for _, call in ipairs({
                   function() sol.act("windows.focus", t) end,
                   function() sol.surface("bar", { scene = "/nonexistent/bar.qml", properties = t }) end,
                   function() sol.pane_values(t) end,
               }) do
                   local ok, err = pcall(call)
                   said[#said + 1] = tostring(ok) .. " " .. tostring(tostring(err):find("contains itself", 1, true) ~= nil)
               end
               sol.status(table.concat(said, ";"))
           end)"#,
    );
    let outcome = scripts.surface_action("shell", "go", &crate::json::Json::Null, one_screen(&[]));
    assert_eq!(
        (outcome.status.as_deref(), format!("{:?}", outcome.commands)),
        (Some("false true;false true;false true"), "[]".to_owned()),
        "(each call's pcall and whether it said why, the commands queued)"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A `done` that never returns is stopped, and the next one still hears
/// its outcome**, with the whole deadline of its own.
#[test]
fn a_done_that_never_returns_is_stopped_and_the_next_done_still_hears_its_outcome() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-done",
        r#"sol.on("surface", function()
               sol.act("windows.focus", { id = 1 }, function() while true do end end)
               sol.act("windows.focus", { id = 2 }, function(ok) for _ = 1, 200000 do end sol.status("second " .. tostring(ok)) end)
           end)"#,
    );
    let outcome = scripts.surface_action("shell", "go", &crate::json::Json::Null, one_screen(&[]));
    let settled: Vec<Settled> = outcome
        .commands
        .iter()
        .filter_map(|command| match command {
            Command::Act { attempt, .. } => Some(Settled {
                attempt: *attempt,
                ok: true,
                reason: None,
            }),
            _ => None,
        })
        .collect();
    assert_eq!(settled.len(), 2, "two acts: {:?}", outcome.commands);
    let started = std::time::Instant::now();
    let status = scripts.attempts_settled(&settled, one_screen(&[])).status;
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "took {:?}",
        started.elapsed()
    );
    assert_eq!(status.as_deref(), Some("second true"));
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A `done` stopped three times is not called again** until the next
/// reload: its stops are counted by function, as a listener's are.
#[test]
fn a_done_stopped_three_times_is_not_called_again() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-done-strikes",
        r#"runs = 0
           local function done() runs = runs + 1; while true do end end
           sol.on("surface", function() sol.act("windows.focus", { id = 1 }, done) end)"#,
    );
    for _ in 0..4 {
        let outcome =
            scripts.surface_action("shell", "go", &crate::json::Json::Null, one_screen(&[]));
        let settled = all_done(&outcome);
        assert_eq!(settled.len(), 1, "one act: {:?}", outcome.commands);
        let _ = scripts.attempts_settled(&settled, one_screen(&[]));
    }
    assert_eq!(scripts.evaluate("return tostring(runs)"), "3");
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A `done` written inline is a new function at each `sol.act`**, so its
/// stops do not add up: it is stopped every time, and never taken out.
#[test]
fn an_inline_done_is_stopped_each_time_and_never_taken_out() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-done-inline",
        r#"runs = 0
           sol.on("surface", function()
               sol.act("windows.focus", { id = 1 }, function() runs = runs + 1; while true do end end)
           end)"#,
    );
    for _ in 0..4 {
        let outcome =
            scripts.surface_action("shell", "go", &crate::json::Json::Null, one_screen(&[]));
        let settled = all_done(&outcome);
        assert_eq!(settled.len(), 1, "one act: {:?}", outcome.commands);
        let _ = scripts.attempts_settled(&settled, one_screen(&[]));
    }
    assert_eq!(scripts.evaluate("return tostring(runs)"), "4");
    let _ = std::fs::remove_dir_all(&directory);
}

/// **What a `done` asked for before it was stopped is dropped**: its
/// commands are not applied, and the attempt it started is forgotten,
/// so a retry that acts again and then never returns is not told again.
#[test]
fn what_a_stopped_done_asked_for_is_dropped() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-done-drops",
        r#"sol.on("surface", function()
               sol.act("windows.focus", { id = 1 }, function()
                   sol.act("windows.focus", { id = 2 }, function() end)
                   sol.close(2)
                   while true do end
               end)
           end)"#,
    );
    let outcome = scripts.surface_action("shell", "go", &crate::json::Json::Null, one_screen(&[]));
    let told = scripts.attempts_settled(&all_done(&outcome), one_screen(&[]));
    assert_eq!(
        (
            format!("{:?}", told.commands),
            scripts.evaluate(
                "local n = 0 for _ in pairs(sol._attempts) do n = n + 1 end return tostring(n)"
            )
        ),
        ("[]".to_owned(), "0".to_owned()),
        "(the commands the stopped done left, the attempts still waiting)"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **A binding that keeps calling `sol.focus_direction` is still
/// stopped**: the clock each `direction` listener starts is not the
/// binding's.
#[test]
fn a_binding_that_loops_on_focus_direction_is_stopped() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-direction",
        r#"
        sol.on("direction", function() end)
        sol.bind("super+x", function() while true do sol.focus_direction("left") end end)
        "#,
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

/// **A `direction` listener stopped at the deadline stops the handler
/// that called `sol.focus_direction` too**: the listeners' time counts
/// against that handler, which is what keeps a loop of these calls
/// stoppable.
#[test]
fn a_direction_listener_stopped_at_the_deadline_stops_its_caller_too() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-deadline-direction-caller",
        r#"
        sol.on("direction", function() while true do end end)
        sol.on("layout", function()
            sol.status("asked")
            sol.focus_direction("left")
            sol.status("went on")
        end)
        "#,
    );
    assert_eq!(
        scripts.relayout(one_screen(&[])).status.as_deref(),
        Some("asked")
    );
    let _ = std::fs::remove_dir_all(&directory);
}
