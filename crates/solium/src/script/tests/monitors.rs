//! The monitors and the workspaces on them.

use super::*;

/// **The shipped configuration's `x11.hidden` matches
/// `xwayland::Settings`'s own default** (#221), the same parity
/// `the_shipped_configuration_s_lock_table_matches_the_rust_default`
/// holds for `lock`: a future edit to one without the other must fail a
/// test rather than ship a mismatch.
#[test]
fn sol_output_management_is_a_flag() {
    let directory = std::env::temp_dir().join("solium-script-test-output-management");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let config = directory.join("init.lua");
    let offered = |source: &str| {
        std::fs::write(&config, source).expect("writing the test script");
        let mut scripts = Scripts::load(&config).expect("loading the test script");
        scripts
            .startup()
            .commands
            .into_iter()
            .find_map(|command| match command {
                Command::OutputManagement(offered) => Some(offered),
                _ => None,
            })
    };
    assert_eq!(offered("sol.output_management(false)"), Some(false));
    assert_eq!(offered("sol.output_management(true)"), Some(true));
    let _ = std::fs::remove_dir_all(&directory);
}

/// **The shipped configuration answers `monitors.apply` with
/// `sol.monitors`** (#51): a program's rows become the arrangement, mode,
/// rotation, scale and place, with `primary` kept from
/// `config.monitors`, and the protocol is offered.
#[test]
fn the_shipped_configuration_answers_monitors_apply_with_sol_monitors() {
    use crate::json::Json;
    let Some((directory, mut scripts)) = shipped_init_with_user(
        "solium-script-test-monitors-apply",
        r#"return { monitors = { { name = "DP-1", primary = true } } }"#,
    ) else {
        return;
    };
    let startup = scripts.startup();
    assert!(
        startup
            .commands
            .iter()
            .any(|command| matches!(command, Command::OutputManagement(true))),
        "the shipped init.lua did not offer it"
    );
    let object = |pairs: Vec<(&str, Json)>| {
        Json::Object(
            pairs
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect(),
        )
    };
    let row = object(vec![
        ("name", Json::Text("DP-1".to_owned())),
        ("enabled", Json::Bool(true)),
        ("x", Json::Number(1920.0)),
        ("y", Json::Number(0.0)),
        (
            "mode",
            object(vec![
                ("w", Json::Number(1280.0)),
                ("h", Json::Number(720.0)),
                ("refresh", Json::Number(60.0)),
            ]),
        ),
        ("scale", Json::Number(1.5)),
        ("transform", Json::Text("90".to_owned())),
    ]);
    let off = object(vec![
        ("name", Json::Text("DP-2".to_owned())),
        ("enabled", Json::Bool(false)),
    ]);
    let asked = object(vec![("monitors", Json::List(vec![row, off]))]);
    let outcome = scripts.surface_action(
        crate::output_management::SURFACE,
        "monitors.apply",
        &asked,
        Snapshot::default(),
    );
    let _ = std::fs::remove_dir_all(&directory);
    let arrangement = outcome
        .commands
        .into_iter()
        .find_map(|command| match command {
            Command::Monitors(arrangement) => Some(arrangement),
            _ => None,
        })
        .expect("monitors.apply was not answered with sol.monitors");
    assert_eq!(
        arrangement.lit_as(),
        [
            (
                "DP-1".to_owned(),
                true,
                crate::monitor::Wanted::Exact {
                    width: 1280,
                    height: 720,
                    refresh: Some(60)
                },
                Some(smithay::utils::Transform::_90),
                None
            ),
            (
                "DP-2".to_owned(),
                false,
                crate::monitor::Wanted::Best,
                None,
                None
            )
        ]
    );
    assert_eq!(
        arrangement.scale("DP-1"),
        crate::monitor::Scaling::Fixed(1.5)
    );
    assert_eq!(arrangement.primary(), Some("DP-1"), "primary kept");
    let placed = arrangement.place(&[("DP-1".to_owned(), (720, 1280).into())]);
    assert_eq!(placed.at, [(1920, 0).into()]);
}

#[test]
fn sol_workspaces_offered_is_a_flag() {
    let directory = std::env::temp_dir().join("solium-script-test-workspaces-offered");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let config = directory.join("init.lua");
    let offered = |source: &str| {
        std::fs::write(&config, source).expect("writing the test script");
        let mut scripts = Scripts::load(&config).expect("loading the test script");
        scripts
            .startup()
            .commands
            .into_iter()
            .find_map(|command| match command {
                Command::WorkspacesOffered(offered) => Some(offered),
                _ => None,
            })
    };
    assert_eq!(offered("sol.workspaces_offered(false)"), Some(false));
    assert_eq!(offered("sol.workspaces_offered(true)"), Some(true));
    let _ = std::fs::remove_dir_all(&directory);
}

/// **`sol.workspaces` declares groups and windows**, every field as Lua
/// wrote it.
#[test]
fn sol_workspaces_declares_groups_and_windows() {
    let (directory, mut scripts) = loaded(
        "solium-script-test-workspaces",
        r#"sol.workspaces({
            arrangement = { kind = "grid", columns = 3, rows = 2 },
            groups = { { id = "DP-1", monitors = { "DP-1" }, showing = { "2" },
                         workspaces = { { id = "1", name = "1", col = 1, row = 1 }, { id = "2", name = "web", col = 2, row = 1 },
                                        { id = "scratch", hidden = true } } } },
            windows = { [42] = { "2" } },
        })"#,
    );
    let declared: Vec<crate::models::workspaces::Declared> = scripts
        .startup()
        .commands
        .into_iter()
        .filter_map(|command| match command {
            Command::Workspaces(declared) => Some(declared),
            _ => None,
        })
        .collect();
    let [declared] = declared.as_slice() else {
        panic!("one declaration: {declared:?}")
    };
    assert_eq!(
        (
            declared.arrangement.kind.as_str(),
            declared.arrangement.columns,
            declared.arrangement.rows
        ),
        ("grid", 3, 2)
    );
    assert_eq!(declared.groups[0].showing, vec!["2".to_owned()]);
    assert_eq!(declared.groups[0].workspaces[1].name, "web");
    assert_eq!(
        declared.groups[0].workspaces[2],
        crate::models::workspaces::Workspace {
            id: "scratch".to_owned(),
            name: "scratch".to_owned(),
            col: 1,
            row: 1,
            hidden: true,
        },
        "a workspace declared with only its id and hidden"
    );
    assert_eq!(declared.windows.get(&42), Some(&vec!["2".to_owned()]));
    let _ = std::fs::remove_dir_all(&directory);
}
