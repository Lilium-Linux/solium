//! What outlives a reload, what is wrong, and what the log hears.

use super::*;

/// **A broken effect reaches `sol.problems()` with its file and line.**
#[test]
fn a_broken_effect_reaches_sol_problems_with_its_file_and_line() {
    let directory = std::env::temp_dir().join(format!("solium-problems-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        sol.bind("Super+P", function()
            local row = sol.problems()[1]
            sol.status(string.format("%s %s %d %s", row.effect, row.file, row.line, row.message))
        end)
    "#,
    )
    .expect("writing the test script");
    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let mut snapshot = empty_snapshot();
    snapshot.problems = vec![super::ProblemRow {
        effect: "blur".to_owned(),
        file: "/x/blur/down.frag".to_owned(),
        line: Some(2),
        column: None,
        message: "`p_ofset` is not a param".to_owned(),
        severity: "error",
    }];
    let outcome = scripts.key("super+p", snapshot);
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(
        outcome.status.as_deref(),
        Some("blur /x/blur/down.frag 2 `p_ofset` is not a param")
    );
}
