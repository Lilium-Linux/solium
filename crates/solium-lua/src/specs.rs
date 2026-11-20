//! **The shipped layouts' own specs, run.**
//!
//! The arrangements are Lua in the shipped configuration (`lua/layouts/`),
//! and their cases are Lua beside this crate (`specs/`), each file a list of
//! `{ name, run }` written with `specs/expect.lua`. Each file is run in a
//! bare Lua with no `sol` at all, which is also what holds the layouts to
//! being arithmetic: one that reached for the window manager would fail here.
//!
//! What a layout leaves to the compositor -- solving its boxes, moving a seam
//! for a dragged edge or a key -- is there too, as the global `space`, run by
//! the compositor's own code (`solium_wm_protocol::space`): `space.solve` is
//! `sol.solve`, and `space.drag` and `space.grow` do what the compositor does
//! with a drag and a `sol.grow`, answering the split's id and shares the
//! `seam` event would say.

use std::path::{Path, PathBuf};

use mlua::{Lua, Table, Value};
use solium_wm_protocol::space::{Limits, Seam, Side, Tree};

fn crate_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// A tree from Lua's area, boxes and limits, and the limits as the solver
/// reads them.
fn tree_of(
    area: &Table,
    boxes: &Table,
    limits: Option<&Table>,
) -> mlua::Result<(Tree, Vec<(u64, Limits)>)> {
    let area = crate::api::area_of(area, "space")?;
    let boxes = crate::api::boxes_from(boxes)?;
    let tree = Tree::of(&area, &boxes).map_err(mlua::Error::runtime)?;
    let limits = match limits {
        Some(limits) => crate::api::limits_from(limits)?,
        None => Vec::new(),
    };
    Ok((tree, limits))
}

fn lookup(limits: &[(u64, Limits)], id: u64) -> Limits {
    limits
        .iter()
        .find(|(each, _)| *each == id)
        .map(|(_, limits)| limits.clone())
        .unwrap_or_default()
}

/// `{ id, shares }` for the split a seam is in, as the `seam` event says it.
fn moved(lua: &Lua, tree: &Tree, seam: Seam) -> mlua::Result<Value> {
    let Some((id, shares)) = tree.shares(seam.split) else {
        return Ok(Value::Nil);
    };
    let out = lua.create_table()?;
    out.set("id", id)?;
    out.set("shares", lua.create_sequence_from(shares)?)?;
    Ok(Value::Table(out))
}

/// The global `space`: the compositor's arithmetic, for a bare Lua.
fn space(lua: &Lua) -> mlua::Result<Table> {
    let space = lua.create_table()?;
    space.set("solve", crate::solve_function(lua)?)?;
    // `space.drag(area, boxes, window, side, x, y, limits)`: false for a
    // side that is none of the four, nil when no seam lies along it.
    space.set(
        "drag",
        lua.create_function(
            |lua,
             (area, boxes, window, side, x, y, limits): (
                Table,
                Table,
                u64,
                String,
                f64,
                f64,
                Option<Table>,
            )| {
                let Some(side) = Side::named(&side) else {
                    return Ok(Value::Boolean(false));
                };
                let (mut tree, limits) = tree_of(&area, &boxes, limits.as_ref())?;
                let Some(seam) = tree.seam_beside(window, side) else {
                    return Ok(Value::Nil);
                };
                let edge = match side {
                    Side::Left | Side::Right => x,
                    Side::Top | Side::Bottom => y,
                };
                tree.drag(seam, edge, |id| lookup(&limits, id));
                moved(lua, &tree, seam)
            },
        )?,
    )?;
    // `space.grow(area, boxes, window, axis, by, limits)`: nil when no seam
    // lies on that axis.
    space.set(
        "grow",
        lua.create_function(
            |lua,
             (area, boxes, window, axis, by, limits): (
                Table,
                Table,
                u64,
                String,
                f64,
                Option<Table>,
            )| {
                let axis = crate::api::axis_named(&axis)?;
                let (mut tree, limits) = tree_of(&area, &boxes, limits.as_ref())?;
                match tree.grow(window, axis, by, |id| lookup(&limits, id)) {
                    Some(seam) => moved(lua, &tree, seam),
                    None => Ok(Value::Nil),
                }
            },
        )?,
    )?;
    Ok(space)
}

/// Every case in `specs/<name>.lua` run, and each that failed with its
/// message. Fails the test on any, naming them all.
fn run(name: &str) {
    let lua = Lua::new();
    lua.globals()
        .set("space", space(&lua).expect("the space table"))
        .expect("space set");
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
