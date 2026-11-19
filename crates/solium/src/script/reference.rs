//! **The flags-and-environment page is generated, and its source is checked
//! here.**
//!
//! The documentation site builds that page from `environment.txt`
//! (`dev/docs/generate.py`). A generated page cannot drift from its source,
//! but the source can drift from the code: a variable renamed in `dev.rs`
//! leaves a row describing a knob that does nothing. The Lua API page's
//! source, `sol.lua`, is the Lua window manager's, and is checked in its
//! crate (`crates/solium-lua/src/reference.rs`).
//!
//! It compares in both directions, and refuses to pass on an empty scan:
//! a walk that finds nothing agrees with anything, which is the one way a test
//! like this can be green and mean nothing.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

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
                && !test_file(&path)
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                let text = production(&path, &text);
                out.push((path, text));
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut out = Vec::new();
    // The window manager Solium ships, its Rust and its Lua, reads the
    // environment the session runs in too, and the page is the session's.
    for part in ["src", "qml", "../solium-lua/src", "../solium-lua/lua"] {
        walk(&root.join(part), &mut out);
    }
    let build = root.join("build.rs");
    if let Ok(text) = std::fs::read_to_string(&build) {
        let text = production(&build, &text);
        out.push((build, text));
    }
    out
}

/// Whether a Rust file is test code throughout: one in a `tests` directory
/// of `src`, declared from its parent under `#[cfg(test)]`, which has no
/// `#[cfg(test)]` of its own to cut at.
fn test_file(path: &Path) -> bool {
    path.extension().is_some_and(|end| end == "rs")
        && path.components().any(|part| part.as_os_str() == "tests")
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

/// The compositor's own production source, file by file: every `.rs` under
/// `src/` that is not a test file, cut at its first `#[cfg(test)]`, comments
/// kept.
fn compositor_production() -> Vec<(PathBuf, String)> {
    fn walk(directory: &Path, out: &mut Vec<(PathBuf, String)>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|end| end == "rs")
                && !test_file(&path)
                && !test_only_module(&path)
                && !path.ends_with("src/script/reference.rs")
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                let kept = text.split("#[cfg(test)]").next().unwrap_or_default();
                out.push((path, kept.to_owned()));
            }
        }
    }
    let mut out = Vec::new();
    walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut out);
    assert!(
        out.len() >= 150,
        "found {} sources; the walk is broken",
        out.len()
    );
    out
}

/// Whether the module in `path` is compiled for tests only: a `tests.rs`,
/// or one its parent declares under `#[cfg(test)]`.
fn test_only_module(path: &Path) -> bool {
    let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return false;
    };
    if name == "tests" {
        return true;
    }
    let (directory, name) = if name == "mod" {
        let Some(directory) = path.parent() else {
            return false;
        };
        let Some(name) = directory.file_name().and_then(|name| name.to_str()) else {
            return false;
        };
        (directory.parent(), name)
    } else {
        (path.parent(), name)
    };
    let Some(directory) = directory else {
        return false;
    };
    let parents = [
        directory.with_extension("rs"),
        directory.join("mod.rs"),
        directory.join("main.rs"),
    ];
    parents.iter().any(|parent| {
        std::fs::read_to_string(parent).is_ok_and(|text| {
            let lines: Vec<&str> = text.lines().map(str::trim).collect();
            lines.windows(2).any(|pair| {
                pair[0] == "#[cfg(test)]"
                    && [format!("mod {name};"), format!("pub(crate) mod {name};")]
                        .contains(&pair[1].to_owned())
            })
        })
    })
}

/// **The compositor names nothing of the Lua window manager.**
///
/// The window manager is a program of its own, in any language, and the
/// compositor speaks to it only in the protocol's words: a command, an
/// event, a settings section. So its production source, comments included,
/// does not name the Lua API (`sol.place` is the `place` command), and its
/// code does not name the Lua window manager's files, crate or variables:
/// that one finds its own configuration and says in `hello-answer` where it
/// is. `mlua` is the effect engine's, under `src/effect/`, and nobody
/// else's. The one Lua program the compositor names is the window manager
/// it starts when `wm.toml` names none.
#[test]
fn the_compositor_names_nothing_of_the_lua_window_manager() {
    let mut found = Vec::new();
    for (path, text) in compositor_production() {
        let shown = path
            .strip_prefix(env!("CARGO_MANIFEST_DIR"))
            .unwrap_or(&path)
            .display()
            .to_string();
        for (number, line) in text.lines().enumerate() {
            let at = |what: &str| format!("{shown}:{}: {what}: {}", number + 1, line.trim());
            // The Lua API, anywhere: `sol.` and a name, but the file `sol.lua`.
            let api = line.match_indices("sol.").any(|(index, _)| {
                let before = line[..index].chars().next_back();
                let after = &line[index + 4..];
                before.is_none_or(|c| !c.is_alphanumeric() && c != '_')
                    && after.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
                    && !after.starts_with("lua")
            });
            if api {
                found.push(at("names the Lua API"));
            }
            let code = line.split("//").next().unwrap_or_default();
            for word in ["init.lua", "SOLIUM_LUA_INIT", "solium_lua", "package.path"] {
                if code.contains(word) {
                    found.push(at(&format!("names `{word}`")));
                }
            }
            if code.contains("mlua") && !shown.starts_with("src/effect/") {
                found.push(at("uses mlua outside the effect engine"));
            }
        }
    }
    assert!(
        found.is_empty(),
        "the compositor knows the Lua window manager:\n  {}",
        found.join("\n  ")
    );
}
