//! The keyboard, the pointer and touch, and whether a mode holds input.

use super::*;

/// `sol.cursor` is still where the pointer is, and setting the theme is a
/// different name.
///
/// **The regression this exists for shipped, and was invisible to the
/// gate.** #81 registered the theme setter under `sol.cursor`, which was
/// already the getter that answers with the pointer's position. `sol.set`
/// replaces without complaining, so the getter simply stopped existing.
/// What broke was `tiling.lua`, which asks where the pointer is on every
/// window open to decide which pane the new window splits: every
/// `sol.on("open")` failed with "error converting Lua nil to table", and
/// no window opened in a tiled layout was placed. Nothing caught it — the
/// compositor starts, `--check` passes, and each function's own test
/// passes because each calls its own name. It took running the thing.
///
/// So this asserts the pair, in one script, in one dispatch: the position
/// comes back as the numbers the snapshot was built with, *and* the theme
/// reaches the compositor as a command. Either name overwriting the other
/// fails this, whichever way round it is done.
#[test]
fn two_functions_cannot_share_one_name() {
    let directory = std::env::temp_dir().join("solium-script-test-cursor-names");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        sol.bind("Super+C", function()
            local at = sol.cursor()
            sol.status(string.format("%d,%d", at.x, at.y))
            sol.cursor_theme({ theme = "Fixture" })
        end)
        "#,
    )
    .expect("writing the test script");

    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let mut snapshot = empty_snapshot();
    snapshot.cursor = (12.0, 34.0);
    let outcome = scripts.key("super+c", snapshot);
    assert!(
        outcome.handled,
        "the listener failed, which is exactly how the collision showed up"
    );
    assert_eq!(
        outcome.status.as_deref(),
        Some("12,34"),
        "sol.cursor() did not answer with the pointer's position"
    );
    match outcome.commands.as_slice() {
        [Command::Cursor(configured)] => {
            assert_eq!(configured.theme.as_deref(), Some("Fixture"));
        }
        other => panic!("sol.cursor_theme() did not reach the compositor: {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&directory);
}

/// **The `problems` event fires once per change**, to every listener.
#[test]
fn the_problems_event_reaches_its_listeners() {
    let directory =
        std::env::temp_dir().join(format!("solium-problems-event-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        "local n = 0\nsol.on('problems', function() n = n + 1; sol.status(tostring(n)) end)\n",
    )
    .expect("writing");
    let mut scripts = Scripts::load(&config).expect("loading");
    let outcome = scripts.problems_changed(empty_snapshot());
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(outcome.status.as_deref(), Some("1"));
}

/// **What `sol.keyboard()` reads, handed back to `sol.keyboard{ ... }`,
/// compiles no keymap**, with `us,ru` and Russian live: the live layout's
/// names come back as `layout_name` and `layout_short`, and `layout` is
/// only ever the xkb names a keymap is compiled from.
#[test]
fn sol_keyboard_handed_back_what_it_read_compiles_no_keymap() {
    let directory = std::env::temp_dir().join("solium-script-test-keyboard-round-trip");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        sol.bind("Super+P", function()
            local read = sol.keyboard()
            sol.status(tostring(read.layout_name) .. " " .. tostring(read.layout_short))
            sol.keyboard(read)
        end)
        "#,
    )
    .expect("writing the test script");
    let mut snapshot = empty_snapshot();
    snapshot.keyboard = crate::keymap::State {
        layouts: vec!["English (US)".to_owned(), "Russian".to_owned()],
        short: vec!["EN".to_owned(), "RU".to_owned()],
        active: 2,
        ..crate::keymap::State::initial()
    };
    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let outcome = scripts.key("super+p", snapshot);
    let _ = std::fs::remove_dir_all(&directory);

    assert_eq!(
        outcome.status.as_deref(),
        Some("Russian RU"),
        "the live layout's names"
    );

    let requests: Vec<&crate::keymap::Request> = outcome
        .commands
        .iter()
        .filter_map(|command| match command {
            Command::Keyboard(request) => Some(request),
            _ => None,
        })
        .collect();
    assert_eq!(
        requests.len(),
        1,
        "what was read is handed back as a request"
    );
    assert_eq!(
        requests[0].keymap, None,
        "and it names no keymap to compile: `layout` is not the live layout's name"
    );
}

/// **`actions.lua` routes the vocabulary and leaves the rest alone**: a
/// `windows.*` action becomes a `sol.act`, one a file overrides is
/// answered in Lua instead, and one outside the vocabulary, a tweak's
/// id, is nobody's but its surface's listener's.
#[test]
fn actions_lua_routes_the_vocabulary_and_leaves_the_rest_alone() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-actions-lua",
        &format!(
            r#"package.path = {shipped:?} .. "/?.lua"
local actions = require("actions")
actions.override("windows.focus", function(data, surface) sol.status("mine " .. data.id .. " " .. surface) end)"#,
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
    let closed = scripts.surface_action("shell", "windows.close", &id(4), one_screen(&[]));
    let focused = scripts.surface_action("shell", "windows.focus", &id(3), one_screen(&[]));
    let tweaked = scripts.surface_action(
        "tweaks",
        "pane:border",
        &crate::json::Json::Null,
        one_screen(&[]),
    );
    assert_eq!(
        (
            acts(&closed),
            (acts(&focused), focused.status.as_deref()),
            acts(&tweaked)
        ),
        (
            vec![("windows.close".to_owned(), r#"{"id":4}"#.to_owned())],
            (Vec::new(), Some("mine 3 shell")),
            Vec::new()
        ),
        "(the vocabulary's act, (an overridden one's acts, what its handler said), \
         a tweak's acts)"
    );
    let _ = std::fs::remove_dir_all(&directory);
}
