//! **The Lua API's reference, and the events the shipped scripts listen
//! for, checked against this crate's own code.**
//!
//! The documentation site builds its Lua API page from `lua/meta/sol.lua`
//! (`dev/docs/generate.py`). A generated page cannot drift from its source,
//! but the source can drift from the code: a function added to `build_api`
//! and not to `sol.lua` is a function nobody is told about. And a handler
//! put under an event name nothing dispatches is simply never called. These
//! compare in both directions, and refuse to pass on an empty scan: a walk
//! that finds nothing agrees with anything.

use std::collections::BTreeSet;
use std::path::Path;

use mlua::{Lua, Table, Value};

use crate::build_api;

/// The definitions file, as the documentation site reads it.
const SOL_LUA: &str = include_str!("../lua/meta/sol.lua");

/// The production half of this crate: each source file under `src/` before
/// its first `#[cfg(test)]`, read from the directory, so a file added to the
/// crate is in it.
fn production_script() -> &'static str {
    static JOINED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    JOINED.get_or_init(|| {
        let mut files = Vec::new();
        let mut stack = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("the source directory reads") {
                let path = entry.expect("an entry").path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    files.push(path);
                }
            }
        }
        files.sort();
        assert!(
            files.len() >= 20,
            "found {} sources; the walk is broken",
            files.len()
        );
        files
            .iter()
            .map(|path| {
                let text = std::fs::read_to_string(path).expect("a source reads");
                text.split("#[cfg(test)]")
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    })
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

/// Every name this window manager puts in `sol`, in each table inside it,
/// and on the objects it hands out (the timer `sol.after` and `sol.every`
/// make, and the watch `sol.watch` does), spelled the way `sol.lua` declares
/// them: `sol.keep`, `sol.<table>.<name>`, `Timer:cancel`.
///
/// The tables are read from the real `sol` table, built the way a load builds
/// it, so nothing here can be a stale copy. Every table-valued key is walked
/// into, so a namespace added has its functions checked too. A key starting
/// with `_` is its own bookkeeping,
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
        names.len() >= 50,
        "found {} names in sol; the walk of the sol table is broken",
        names.len()
    );

    for (object, userdata, least) in [("Timer", "TimerHandle", 1), ("Watch", "WatchHandle", 1)] {
        let methods = userdata_methods(userdata);
        assert!(
            methods.len() >= least,
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
        .unwrap_or_else(|| panic!("no `{header}` in solium-lua"));
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
/// function sol.table.name(argument) end
/// function Timer:cancel() end
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

/// **`sol.lua` describes exactly the API this window manager registers.**
///
/// In both directions: a name registered and not described is missing from
/// the editor's completion and from the API page; a name `sol.lua`
/// describes and nothing registers is a promise
/// the API page makes and a script cannot keep. And every function has what
/// the page is built from: a description, a `---@param` for each parameter in
/// its signature, and a `---@return`. Every field has a description.
///
/// Also the events: `sol.lua`'s `sol.Event` alias is the event table on the
/// page, so it has to be the set `dispatch.rs` dispatches.
#[test]
fn sol_lua_documents_exactly_the_api_solium_lua_registers() {
    let registered = registered();
    let declared = declared();
    assert!(
        registered.len() >= 60 && declared.len() >= 60,
        "solium-lua registers {} names and sol.lua declares {}; one of the two \
         scans is broken",
        registered.len(),
        declared.len()
    );

    let documented: BTreeSet<String> = declared.iter().map(|each| each.name.clone()).collect();
    let undocumented: Vec<&String> = registered.difference(&documented).collect();
    let imaginary: Vec<&String> = documented.difference(&registered).collect();
    assert!(
        undocumented.is_empty() && imaginary.is_empty(),
        "lua/meta/sol.lua and what solium-lua registers disagree.\n  \
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
    // Events, and decision hooks (`call_deciders`), which are events too.
    let dispatched: BTreeSet<String> = ["call_listeners(lua, \"", "call_deciders(lua, \""]
        .into_iter()
        .flat_map(|marker| {
            production_script()
                .match_indices(marker)
                .filter_map(move |(at, _)| {
                    let rest = &production_script()[at + marker.len()..];
                    rest.split_once('"').map(|(event, _)| event.to_owned())
                })
        })
        .collect();
    assert!(
        dispatched.len() >= 8,
        "only {} events found in solium-lua; the scan is broken",
        dispatched.len()
    );
    assert_eq!(
        events, dispatched,
        "sol.lua's `sol.Event` alias is not the set of events solium-lua dispatches"
    );
}

/// The code of a line of Lua, before its comment, which a quoted `--` does
/// not start.
fn code(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut quoted = false;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' if quoted => index += 1,
            b'"' => quoted = !quoted,
            b'-' if !quoted && bytes.get(index + 1) == Some(&b'-') => return &line[..index],
            _ => {}
        }
        index += 1;
    }
    line
}

/// Every double-quoted literal that follows `marker`, with where it was.
///
/// `marker` is matched literally and the literal must come next, with only
/// spaces between: `easing =` finds `easing = "outCubic"` and `sol.on(`
/// finds `sol.on("open", ...)`. Anything else after the marker -- a
/// variable, a table, a concatenation -- is skipped rather than guessed at.
/// That is the limit this module states up front, and it is why
/// `shell.lua`'s `scene = chosen` never appears here.
fn named(text: &str, marker: &str) -> Vec<(usize, String)> {
    // An empty marker matches at every position and consumes none of them,
    // so the walk below would never move. Refused here rather than left to
    // a caller, because the symptom is a test run that never finishes --
    // which is how this was found.
    assert!(!marker.is_empty(), "a marker has to be something");
    let mut found = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = code(line);
        let mut from = 0;
        while let Some(at) = line[from..].find(marker) {
            let after = from + at + marker.len();
            from = after;
            let rest = line[after..].trim_start_matches(' ');
            let Some(rest) = rest.strip_prefix('"') else {
                continue;
            };
            let Some(end) = rest.find('"') else {
                continue;
            };
            found.push((number + 1, rest[..end].to_owned()));
        }
    }
    found
}

/// Every shipped script, by its file name, with its text.
fn scripts() -> Vec<(String, String)> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("lua");
    let mut found: Vec<(String, String)> = std::fs::read_dir(&directory)
        .expect("the shipped Lua reads")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|end| end == "lua"))
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_owned();
            Some((name, std::fs::read_to_string(&path).ok()?))
        })
        .collect();
    found.sort();
    assert!(
        found.len() >= 8,
        "found {} shipped scripts in {}; the walk is broken, not the scripts",
        found.len(),
        directory.display()
    );
    found
}

/// Every literal after `marker` in every shipped script: file, line, value.
fn everywhere(marker: &str) -> Vec<(String, usize, String)> {
    scripts()
        .into_iter()
        .flat_map(|(file, text)| {
            named(&text, marker)
                .into_iter()
                .map(move |(line, value)| (file.clone(), line, value))
        })
        .collect()
}

/// **Every event a shipped script listens for is one that is dispatched.**
///
/// Worse than an unknown easing, because there is no warning at all:
/// `sol.on` puts the handler in a table under whatever name it was given,
/// and a name nothing dispatches is a handler that is simply never called.
/// A layout that misspells `"monitors"` does not fail -- it stops
/// rearranging when a screen is unplugged, which reads as a compositor bug.
///
/// The vocabulary is read out of `dispatch.rs` itself rather than listed
/// here, because a list is a second copy and two copies of a vocabulary
/// drifting apart is the defect this whole module exists for.
#[test]
fn every_event_listened_for_is_one_that_is_sent() {
    // The production half of the module, each file cut at its first `#[cfg(test)]`
    // because everything below it -- including this test -- talks *about*
    // `call_listeners` and would otherwise be counted as calling it.
    let production = production_script();
    // Decision hooks (`sol.on("notification")`) are dispatched by
    // `call_deciders`, and are events like any other.
    let dispatched: Vec<String> = named(production, "call_listeners(lua,")
        .into_iter()
        .chain(named(production, "call_deciders(lua,"))
        .map(|(_, name)| name)
        .collect();

    // The scrape has to see every dispatch there is, or it narrows the
    // vocabulary in silence and this test starts agreeing with anything.
    // One `call_listeners` is the definition; the rest are calls.
    let calls = production.matches("call_listeners").count() - 1
        + production
            .matches("call_deciders")
            .count()
            .saturating_sub(1);
    assert_eq!(
        dispatched.len(),
        calls,
        "{calls} dispatches in solium-lua but only {} are `call_listeners(lua, \"name\")`; \
         one of them is written some other way and this check cannot see it",
        dispatched.len()
    );
    assert!(
        calls >= 8,
        "only {calls} dispatches found; the scan is broken"
    );

    for (file, line, event) in everywhere("sol.on(") {
        assert!(
            dispatched.contains(&event),
            "{file}:{line} listens for {event:?}, which nothing dispatches. \
             Events: {dispatched:?}"
        );
    }
}
