//! **The reference pages are generated, and their sources are checked here.**
//!
//! The documentation site builds its Lua API page from `lua/meta/sol.lua` and
//! its flags-and-environment page from `environment.txt`
//! (`dev/docs/generate.py`). A generated page cannot drift from its source, but
//! the source can drift from the code: a function added to `build_api` and not
//! to `sol.lua` is a function nobody is told about, and a variable renamed in
//! `dev.rs` leaves a row describing a knob that does nothing. These tests are
//! what makes "generated from the code" mean the code as it is now.
//!
//! Both compare in both directions, and both refuse to pass on an empty scan:
//! a walk that finds nothing agrees with anything, which is the one way a test
//! like this can be green and mean nothing.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use mlua::{Lua, Table, Value};

use super::build_api;

/// The definitions file, as the documentation site reads it.
const SOL_LUA: &str = include_str!("../../lua/meta/sol.lua");

/// The production half of `script.rs`: everything before its first
/// `#[cfg(test)]`, which is the test module. The same cut
/// `every_event_listened_for_is_one_that_is_sent` makes, for the same reason.
fn production_script() -> &'static str {
    const SOURCE: &str = include_str!("../script.rs");
    SOURCE.split("#[cfg(test)]").next().unwrap_or_default()
}

/// Every key of a Lua table, as text.
fn keys(table: &Table) -> BTreeSet<String> {
    table
        .pairs::<Value, Value>()
        .filter_map(Result::ok)
        .filter_map(|(key, _)| match key {
            Value::String(name) => Some(name.to_string_lossy()),
            _ => None,
        })
        .collect()
}

/// Every name the compositor puts in `sol`, in each table inside it, and on
/// the two objects `sol.layout` makes, spelled the way `sol.lua` declares them:
/// `sol.keep`, `sol.layout.grid`, `Tree:insert`, `Scroller:consume`.
///
/// The tables are read from the real `sol` table, built the way a load builds
/// it, so nothing here can be a stale copy. Every table-valued key is walked
/// into, not only `sol.layout`: a namespace added later has its functions
/// checked too. A key starting with `_` is the compositor's own bookkeeping,
/// declared in `sol.lua` as a field but not walked into. The objects' methods
/// are read from their `impl mlua::UserData` blocks instead, because mlua keeps
/// a userdata's methods behind a closure that Lua cannot list.
fn registered() -> BTreeSet<String> {
    let lua = Lua::new();
    let sol = build_api(&lua).expect("building the sol table");
    let mut names = BTreeSet::new();
    for (key, value) in sol.pairs::<Value, Value>().filter_map(Result::ok) {
        let Value::String(name) = key else {
            continue;
        };
        let name = name.to_string_lossy();
        if let Value::Table(table) = value
            && !name.starts_with('_')
        {
            names.extend(
                keys(&table)
                    .into_iter()
                    .map(|inner| format!("sol.{name}.{inner}")),
            );
        }
        names.insert(format!("sol.{name}"));
    }
    assert!(
        names.iter().any(|name| name.starts_with("sol.layout.")),
        "no sol.layout.* names found; the walk into sol's tables is broken"
    );

    for (object, userdata) in [("Tree", "TilingTree"), ("Scroller", "Scrolling")] {
        let methods = userdata_methods(userdata);
        assert!(
            methods.len() >= 5,
            "found {} methods on {userdata}; the scan of its impl block is broken",
            methods.len()
        );
        names.extend(
            methods
                .into_iter()
                .map(|method| format!("{object}:{method}")),
        );
    }
    names
}

/// The method names in `impl mlua::UserData for <name>`.
///
/// Every `add_method` and `add_method_mut` in the block is counted, and the
/// count has to match the names found: a method registered some other way,
/// or a name that is not a literal right after the call, is a scan that would
/// otherwise narrow in silence.
fn userdata_methods(name: &str) -> BTreeSet<String> {
    let source = production_script();
    let header = format!("impl mlua::UserData for {name} {{");
    let start = source
        .find(&header)
        .unwrap_or_else(|| panic!("no `{header}` in script.rs"));
    // The block ends at the first line that closes an item at column zero.
    let block = &source[start..];
    let end = block.find("\n}\n").map_or(block.len(), |at| at + 3);
    let block = &block[..end];

    let mut found = BTreeSet::new();
    let mut calls = 0;
    for marker in ["methods.add_method(", "methods.add_method_mut("] {
        for (at, _) in block.match_indices(marker) {
            calls += 1;
            let rest = block[at + marker.len()..].trim_start();
            if let Some(rest) = rest.strip_prefix('"')
                && let Some(close) = rest.find('"')
            {
                found.insert(rest[..close].to_owned());
            }
        }
    }
    assert_eq!(
        found.len(),
        calls,
        "{calls} method registrations in {name}'s impl block but {} literal names; \
         one of them is written some other way and this check cannot see it",
        found.len()
    );
    found
}

/// One declaration in `sol.lua` and the `---` block above it.
#[derive(Debug)]
struct Declared {
    name: String,
    line: usize,
    /// The parameter names in the signature, for a function; `None` for a field.
    parameters: Option<Vec<String>>,
    doc: Vec<String>,
}

/// Every name `sol.lua` declares, with its documentation.
///
/// Four shapes, and nothing else counts:
///
/// ```lua
/// function sol.keep(name, defaults) end
/// function sol.layout.grid(sizes, options) end
/// function Tree:insert(id, target, x, y, options) end
/// sol.decoration = sol.pane
/// ```
fn declared() -> Vec<Declared> {
    let mut out = Vec::new();
    let mut doc: Vec<String> = Vec::new();
    for (index, line) in SOL_LUA.lines().enumerate() {
        if let Some(text) = line.strip_prefix("---") {
            doc.push(text.to_owned());
            continue;
        }
        let taken = std::mem::take(&mut doc);
        if let Some(rest) = line.strip_prefix("function ") {
            let Some((name, signature)) = rest.split_once('(') else {
                continue;
            };
            let parameters = signature
                .split_once(')')
                .map(|(inside, _)| inside)
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|parameter| !parameter.is_empty())
                .map(ToOwned::to_owned)
                .collect();
            out.push(Declared {
                name: name.to_owned(),
                line: index + 1,
                parameters: Some(parameters),
                doc: taken,
            });
        } else if let Some(rest) = line.strip_prefix("sol.")
            && let Some((name, _)) = rest.split_once(" = ")
        {
            out.push(Declared {
                name: format!("sol.{name}"),
                line: index + 1,
                parameters: None,
                doc: taken,
            });
        }
    }
    out
}

/// **`sol.lua` describes exactly the API the compositor registers.**
///
/// In both directions: a name the compositor registers and `sol.lua` does not
/// describe is missing from the editor's completion and from the API page; a
/// name `sol.lua` describes and the compositor does not register is a promise
/// the API page makes and a script cannot keep. And every function has what
/// the page is built from: a description, a `---@param` for each parameter in
/// its signature, and a `---@return`. Every field has a description.
///
/// Also the events: `sol.lua`'s `sol.Event` alias is the event table on the
/// page, so it has to be the set `script.rs` dispatches.
#[test]
fn sol_lua_documents_exactly_the_api_the_compositor_registers() {
    let registered = registered();
    let declared = declared();
    assert!(
        registered.len() >= 60 && declared.len() >= 60,
        "the compositor registers {} names and sol.lua declares {}; one of the two \
         scans is broken",
        registered.len(),
        declared.len()
    );

    let documented: BTreeSet<String> = declared.iter().map(|each| each.name.clone()).collect();
    let undocumented: Vec<&String> = registered.difference(&documented).collect();
    let imaginary: Vec<&String> = documented.difference(&registered).collect();
    assert!(
        undocumented.is_empty() && imaginary.is_empty(),
        "crates/solium/lua/meta/sol.lua and the compositor disagree.\n  \
         registered, not in sol.lua: {undocumented:?}\n  \
         in sol.lua, not registered: {imaginary:?}"
    );

    for each in &declared {
        let described = each
            .doc
            .iter()
            .any(|line| !line.starts_with('@') && !line.trim().is_empty());
        assert!(
            described,
            "sol.lua:{} `{}` has no description",
            each.line, each.name
        );
        let Some(parameters) = &each.parameters else {
            continue;
        };
        for parameter in parameters {
            let tagged = format!("@param {parameter}");
            assert!(
                each.doc.iter().any(|line| {
                    line.strip_prefix(&tagged)
                        .is_some_and(|rest| rest.starts_with([' ', '?']))
                }),
                "sol.lua:{} `{}` has no `---@param {parameter}`",
                each.line,
                each.name
            );
        }
        assert!(
            each.doc.iter().any(|line| line.starts_with("@return")),
            "sol.lua:{} `{}` has no `---@return`",
            each.line,
            each.name
        );
    }

    let mut in_alias = false;
    let mut events = BTreeSet::new();
    for line in SOL_LUA.lines() {
        if line.starts_with("---@alias ") {
            in_alias = line.trim() == "---@alias sol.Event";
            continue;
        }
        if !in_alias {
            continue;
        }
        let Some(rest) = line.strip_prefix("---| \"") else {
            in_alias = false;
            continue;
        };
        if let Some((event, _)) = rest.split_once('"') {
            events.insert(event.to_owned());
        }
    }
    let marker = "call_listeners(lua, \"";
    let dispatched: BTreeSet<String> = production_script()
        .match_indices(marker)
        .filter_map(|(at, _)| {
            let rest = &production_script()[at + marker.len()..];
            rest.split_once('"').map(|(event, _)| event.to_owned())
        })
        .collect();
    assert!(
        dispatched.len() >= 8,
        "only {} events found in script.rs; the scan is broken",
        dispatched.len()
    );
    assert_eq!(
        events, dispatched,
        "sol.lua's `sol.Event` alias is not the set of events script.rs dispatches"
    );
}

/// The production code of one of the compositor's source files, with every
/// line that is not production code blanked, so line numbers still match.
///
/// What goes: comment lines (`//`, or `--` in Lua), and in Rust every item
/// under a `#[cfg(test)]` -- a test module, a test-only function, `impl` or
/// `use`. Tests pass arguments to other programs (`--exact` to the test
/// runner) and assert that things are *not* flags, and none of that is a
/// flag or a variable Solium reads.
///
/// An item ends at its first line at the attribute's indentation that ends in
/// `;`, or that closes it: `}`, `};`, `];` or `);`, which is where rustfmt puts
/// a closing bracket. A `#[cfg(test)]` on anything but an item (a statement, a
/// match arm, a variant) is left alone: it is a line or two, and cannot hide a
/// read.
fn production(path: &Path, text: &str) -> String {
    const ITEMS: [&str; 12] = [
        "mod ",
        "fn ",
        "const ",
        "static ",
        "impl",
        "use ",
        "struct ",
        "enum ",
        "trait ",
        "type ",
        "thread_local!",
        "macro_rules!",
    ];
    let rust = path.extension().is_some_and(|end| end == "rs");
    let comment = if path.extension().is_some_and(|end| end == "lua") {
        "--"
    } else {
        "//"
    };
    let lines: Vec<&str> = text.lines().collect();
    let mut kept: Vec<&str> = Vec::with_capacity(lines.len());
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if rust && line.trim() == "#[cfg(test)]" {
            let indent = &line[..line.len() - line.trim_start().len()];
            let mut item = index + 1;
            while lines
                .get(item)
                .is_some_and(|next| next.trim_start().starts_with("#["))
            {
                item += 1;
            }
            let head = lines.get(item).map_or("", |next| next.trim());
            let head = ["pub(crate) ", "pub(super) ", "pub "]
                .iter()
                .find_map(|visibility| head.strip_prefix(visibility))
                .unwrap_or(head);
            if ITEMS.iter().any(|kind| head.starts_with(kind)) {
                let mut end = item;
                while let Some(each) = lines.get(end) {
                    let at_indent = each
                        .strip_prefix(indent)
                        .filter(|rest| !rest.starts_with(char::is_whitespace));
                    let closes = at_indent
                        .is_some_and(|rest| ["}", "};", "];", ");"].contains(&rest.trim_end()));
                    // `mod tests;`, or `fn helper() {}` with its braces
                    // closed on the same line.
                    let first = each.trim_end();
                    let one_line = end == item
                        && (first.ends_with(';')
                            || (first.contains('{')
                                && first.matches('{').count() == first.matches('}').count()));
                    if closes || one_line {
                        break;
                    }
                    end += 1;
                }
                let end = end.min(lines.len() - 1);
                kept.extend(std::iter::repeat_n("", end + 1 - index));
                index = end + 1;
                continue;
            }
        }
        kept.push(if line.trim_start().starts_with(comment) {
            ""
        } else {
            line
        });
        index += 1;
    }
    kept.join("\n")
}

/// The production code of every file of the compositor that could read the
/// environment or its arguments -- the Rust, the C++ QML host, the shipped Lua
/// and QML -- except this one, whose strings are the names it looks for.
fn compositor_sources() -> Vec<(PathBuf, String)> {
    fn walk(directory: &Path, out: &mut Vec<(PathBuf, String)>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path
                .extension()
                .and_then(|end| end.to_str())
                .is_some_and(|end| ["rs", "lua", "cpp", "h", "qml"].contains(&end))
                && !path.ends_with("src/script/reference.rs")
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                let text = production(&path, &text);
                out.push((path, text));
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut out = Vec::new();
    for part in ["src", "lua", "qml"] {
        walk(&root.join(part), &mut out);
    }
    let build = root.join("build.rs");
    if let Ok(text) = std::fs::read_to_string(&build) {
        let text = production(&build, &text);
        out.push((build, text));
    }
    out
}

/// Every double-quoted literal in `text` that starts with `prefix` and runs on
/// in characters `allowed` accepts, with where its opening quote is.
fn literals(text: &str, prefix: &str, allowed: impl Fn(char) -> bool) -> Vec<(usize, String)> {
    let opening = format!("\"{prefix}");
    let mut found = Vec::new();
    for (at, _) in text.match_indices(&opening) {
        let rest = &text[at + 1..];
        let name: String = rest.chars().take_while(|c| allowed(*c)).collect();
        // Only a whole literal, and a name: `"SOLIUM_PACING"`, not a log
        // message that happens to start with a variable's name, and not
        // `"---"`, which is the start of a Lua doc comment rather than a flag.
        let named = name[prefix.len()..].starts_with(|c: char| c.is_ascii_alphanumeric());
        if named && rest[name.len()..].starts_with('"') {
            found.push((at, name));
        }
    }
    found
}

/// Whether the literal whose opening quote is at `at` is compared with
/// something: `== "--x"`, `Some("--x")`, `strip_prefix("--x")`, a match arm
/// `"--x" =>`, or a named constant, `const FLAG: &str = "--x"`. That is how a
/// flag is read; a `"--x"` handed to another program, or written in a
/// message, is not.
fn compared(text: &str, at: usize, length: usize) -> bool {
    let before = text[..at].trim_end();
    let after = text[at + length..].trim_start();
    [
        "==",
        "!=",
        "Some(",
        "strip_prefix(",
        "starts_with(",
        "&str =",
    ]
    .iter()
    .any(|context| before.ends_with(context))
        || after.starts_with("=>")
        || after.starts_with('|')
}

/// **`environment.txt` lists exactly the variables and flags the code reads.**
///
/// A variable is a `"SOLIUM_..."` literal in the compositor's production code
/// -- the Rust, the QML host, the shipped Lua -- and a flag is a `"--..."`
/// literal in its Rust that is compared with something (see [`compared`]).
/// Tests and comments are not read (see [`production`]). A name the code reads
/// and the file does not list is a knob nobody is told about; a name the file
/// lists and nothing reads is a row on the reference page describing
/// something that does nothing.
#[test]
fn the_environment_reference_lists_exactly_what_the_code_reads() {
    let registry = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/environment.txt"))
        .expect("reading crates/solium/environment.txt");
    let listed: BTreeSet<String> = registry
        .lines()
        .filter(|line| line.starts_with("SOLIUM_") || line.starts_with("--"))
        .filter_map(|line| line.split([' ', '=']).next())
        .map(ToOwned::to_owned)
        .collect();

    let mut read = BTreeSet::new();
    for (path, text) in compositor_sources() {
        read.extend(
            literals(&text, "SOLIUM_", |c| {
                c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'
            })
            .into_iter()
            .map(|(_, name)| name),
        );
        if path.extension().is_some_and(|end| end == "rs") {
            read.extend(
                literals(&text, "--", |c| c.is_ascii_lowercase() || c == '-')
                    .into_iter()
                    .filter(|(at, name)| compared(&text, *at, name.len() + 2))
                    .map(|(_, name)| name),
            );
        }
    }
    assert!(
        read.iter()
            .filter(|name| name.starts_with("SOLIUM_"))
            .count()
            >= 10
            && read.iter().filter(|name| name.starts_with("--")).count() >= 4,
        "found {read:?} in the compositor's sources; the scan is broken"
    );

    let unlisted: Vec<&String> = read.difference(&listed).collect();
    let unread: Vec<&String> = listed.difference(&read).collect();
    assert!(
        unlisted.is_empty() && unread.is_empty(),
        "crates/solium/environment.txt and the code disagree.\n  \
         read by the code, not listed: {unlisted:?}\n  \
         listed, read by nothing: {unread:?}"
    );
}

/// The scan that test relies on, on a file shaped like the ones it reads: a
/// flag in a comparison counts, a test module and a comment do not, and the
/// production code after a test item is still read.
#[test]
fn the_environment_scan_reads_production_code_only() {
    let source = r#"#[cfg(test)]
use std::fmt;
fn parse() {
    // "--not-this"
    if argument == "--yes" || matches!(first, Some("--also")) {}
    run(&["--handed-on"]);
}
#[cfg(test)]
fn helper() {}
fn between() {
    std::env::var("SOLIUM_BETWEEN");
}
#[cfg(test)]
struct Probe { a: u8 }
#[cfg(test)]
mod tests {
    fn given() {
        run(&["--exact"]);
        assert!(!flag("--sessions"));
        std::env::var("SOLIUM_TEST_ONLY");
    }
}
fn after() {
    std::env::var("SOLIUM_AFTER");
}
"#;
    let text = production(Path::new("main.rs"), source);
    assert_eq!(text.lines().count(), source.lines().count());
    let flags: Vec<String> = literals(&text, "--", |c| c.is_ascii_lowercase() || c == '-')
        .into_iter()
        .filter(|(at, name)| compared(&text, *at, name.len() + 2))
        .map(|(_, name)| name)
        .collect();
    assert_eq!(flags, ["--yes", "--also"]);
    let variables: Vec<String> = literals(&text, "SOLIUM_", |c| c.is_ascii_uppercase() || c == '_')
        .into_iter()
        .map(|(_, name)| name)
        .collect();
    assert_eq!(variables, ["SOLIUM_BETWEEN", "SOLIUM_AFTER"]);
}
