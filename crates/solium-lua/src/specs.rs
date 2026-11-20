//! **The shipped layouts' own specs, run.**
//!
//! The arrangements are Lua in the shipped configuration (`lua/layouts/`),
//! and their cases are Lua beside this crate (`specs/`), each file a list of
//! `{ name, run }` written with `specs/expect.lua`. Each file is run in a
//! bare Lua with no `sol` at all, which is also what holds the layouts to
//! being arithmetic: one that reached for the window manager would fail here.

use std::path::{Path, PathBuf};

use mlua::{Lua, Table};

fn crate_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Every case in `specs/<name>.lua` run, and each that failed with its
/// message. Fails the test on any, naming them all.
fn run(name: &str) {
    let lua = Lua::new();
    let lua_dir: PathBuf = crate_dir().join("lua");
    let spec_dir: PathBuf = crate_dir().join("specs");
    let package: Table = lua.globals().get("package").expect("package");
    package
        .set(
            "path",
            format!("{}/?.lua;{}/?.lua", lua_dir.display(), spec_dir.display()),
        )
        .expect("package.path");
    let file = spec_dir.join(format!("{name}.lua"));
    let source = std::fs::read_to_string(&file).expect("the spec reads");
    let cases: Table = lua
        .load(&source)
        .set_name(format!("@{}", file.display()))
        .eval()
        .expect("the spec loads and returns its cases");

    let mut ran = 0;
    let mut failed = Vec::new();
    for case in cases.sequence_values::<Table>() {
        let case = case.expect("a case is a table");
        let title: String = case.get("name").expect("a case has a name");
        let body: mlua::Function = case.get("run").expect("a case has a body");
        ran += 1;
        if let Err(error) = body.call::<()>(()) {
            failed.push(format!("{title}: {error}"));
        }
    }
    assert!(ran > 0, "specs/{name}.lua has no cases");
    assert!(
        failed.is_empty(),
        "{} of {ran} cases in specs/{name}.lua failed:\n\n{}",
        failed.len(),
        failed.join("\n\n")
    );
}

#[test]
fn the_dwindle_tree_does_what_its_specs_say() {
    run("dwindle");
}

#[test]
fn the_scrolling_strip_does_what_its_specs_say() {
    run("scroller");
}

#[test]
fn the_overview_grid_does_what_its_specs_say() {
    run("grid");
}

/// **No arrangement is written in Rust.** Tiling, scrolling and the overview
/// grid are configuration: the shipped modes take their arithmetic from
/// `lua/layouts/`, no crate of the workspace is an arrangement, and no Rust
/// outside a test names one in code -- in the compositor, in this runtime,
/// anywhere. A helper or a fast path in Rust is the hardcoding this replaced.
#[test]
fn no_arrangement_is_written_in_rust() {
    let root = crate_dir().join("../..");
    for (mode, layout) in [
        ("tiling", "layouts.dwindle"),
        ("scrolling", "layouts.scroller"),
        ("overview", "layouts.grid"),
    ] {
        let source = std::fs::read_to_string(crate_dir().join(format!("lua/{mode}.lua")))
            .expect("a shipped mode reads");
        assert!(
            source.contains(&format!("require(\"{layout}\")")),
            "{mode}.lua does not take its arithmetic from {layout}"
        );
    }

    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).expect("the workspace");
    assert!(
        !manifest.contains("crates/layout"),
        "the workspace has a layout crate again"
    );

    // Code, not prose: a comment may say what a dwindle tree is.
    let words = ["dwindle", "scroller", "master_stack", "Tiling", "Scroller"];
    let mut found = Vec::new();
    let mut read = 0;
    let mut ahead = vec![root.join("crates")];
    while let Some(dir) = ahead.pop() {
        for entry in std::fs::read_dir(&dir).expect("a directory reads") {
            let path = entry.expect("an entry").path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if path.is_dir() {
                // Tests may hold a tree, to drive one through the compositor.
                if !matches!(name, "tests" | "target" | "specs" | "preview") {
                    ahead.push(path);
                }
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "rs")
                || matches!(name, "tests.rs" | "specs.rs" | "reference.rs")
            {
                continue;
            }
            read += 1;
            let text = std::fs::read_to_string(&path).expect("a source reads");
            let production = text.split("#[cfg(test)]").next().unwrap_or_default();
            for (number, line) in production.lines().enumerate() {
                let code = line.split("//").next().unwrap_or_default();
                for word in words {
                    if code
                        .split(|c: char| !c.is_alphanumeric() && c != '_')
                        .any(|token| token == word)
                    {
                        found.push(format!(
                            "{}:{}: {}",
                            path.display(),
                            number + 1,
                            line.trim()
                        ));
                    }
                }
            }
        }
    }
    assert!(read >= 250, "read {read} sources; the walk is broken");
    assert!(
        found.is_empty(),
        "an arrangement in Rust, which belongs in lua/layouts/:\n{}",
        found.join("\n")
    );
}

/// Every spec file has a test above, so one added is one that runs.
#[test]
fn every_spec_file_is_run() {
    let mut names: Vec<String> = std::fs::read_dir(crate_dir().join("specs"))
        .expect("specs/ reads")
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            (path.extension()? == "lua").then(|| path.file_stem()?.to_str().map(str::to_owned))?
        })
        .filter(|name| name != "expect")
        .collect();
    names.sort();
    assert_eq!(names, ["dwindle", "grid", "scroller"]);
}
