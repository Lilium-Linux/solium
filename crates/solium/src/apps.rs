//! Installed applications: the Desktop Entry Specification, parsed by hand
//! rather than through a crate, and only as much of it as the preview dock
//! needs (03 §3.2.13).
//!
//! **What is cut for this first version, and why.** Actions (`[Desktop
//! Action ...]` groups), D-Bus activation, usage scoring and inotify are all
//! marked P1/P2 in the design notes, so none of them are here. Scanning is a
//! synchronous walk on whichever thread calls [`scan`] — no worker thread —
//! run once at startup and again on a config reload (`state/commands.rs`),
//! which is cheap enough for a few hundred `.desktop` files and keeps this
//! version small; a directory watch is the natural P1 if a rescan on demand
//! ever feels slow.

use std::{
    collections::HashMap,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

/// One installed, visible application: `Solium.Apps`' row, less what is
/// cut above.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Entry {
    /// The desktop file id: its path under the `applications` directory that
    /// held it, with each `/` turned into `-` and `.desktop` dropped.
    pub(crate) id: String,
    pub(crate) name: String,
    /// `Name` with no locale suffix, so a session in another language still
    /// finds "Settings".
    pub(crate) untranslated_name: String,
    pub(crate) generic_name: String,
    /// A theme icon name, or an absolute path when `Icon=` gave one.
    pub(crate) icon: String,
    /// The raw `Exec=` value, expanded per launch by [`launch_argv`].
    pub(crate) exec: String,
    pub(crate) categories: Vec<String>,
    pub(crate) keywords: Vec<String>,
    pub(crate) terminal: bool,
    /// `Path=`: the working directory a launch starts in, absent meaning
    /// `$HOME`.
    pub(crate) path: Option<String>,
    /// Where the `.desktop` file itself lives, for `%k`.
    pub(crate) source: PathBuf,
}

/// The XDG applications directories, first wins:
/// `$XDG_DATA_HOME/applications` (or `~/.local/share/applications`), then
/// each of `$XDG_DATA_DIRS/applications` (or the two defaults,
/// `/usr/local/share` and `/usr/share`).
pub(crate) fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    if let Some(home) = home {
        dirs.push(home.join("applications"));
    }
    let data_dirs = std::env::var_os("XDG_DATA_DIRS").filter(|value| !value.is_empty());
    let data_dirs = data_dirs
        .as_deref()
        .and_then(OsStr::to_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
    for dir in data_dirs.split(':').filter(|dir| !dir.is_empty()) {
        dirs.push(PathBuf::from(dir).join("applications"));
    }
    dirs
}

/// The language list a `Name[xx]` lookup tries, most specific first, read
/// from `LANGUAGE` (colon-separated, GNU gettext's own extension), then
/// `LC_ALL`, `LC_MESSAGES` and `LANG`. `tests::locales_read_language_then_the_lc_chain`.
pub(crate) fn preferred_locales() -> Vec<String> {
    let mut locales = Vec::new();
    if let Some(language) = std::env::var("LANGUAGE").ok().filter(|v| !v.is_empty()) {
        locales.extend(language.split(':').map(ToOwned::to_owned));
    }
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(value) = std::env::var(var).ok().filter(|v| !v.is_empty()) {
            // `en_US.UTF-8` -- the encoding is not part of the locale a
            // `Name[xx]` key is written in.
            let value = value.split('.').next().unwrap_or(&value);
            if !locales.iter().any(|already| already == value) {
                locales.push(value.to_owned());
            }
        }
    }
    locales
}

/// Every installed, visible application under `dirs`, sorted by name:
/// `tests::a_higher_precedence_directory_wins_and_hidden_masks_a_lower_one`,
/// `tests::no_display_and_hidden_entries_are_left_out`.
pub(crate) fn scan(dirs: &[PathBuf], locales: &[String]) -> Vec<Entry> {
    let mut seen: HashMap<String, Option<Entry>> = HashMap::new();
    for dir in dirs {
        for file in walk(dir) {
            let Ok(relative) = file.strip_prefix(dir) else {
                continue;
            };
            let id = desktop_id(relative);
            if seen.contains_key(&id) {
                // A higher-precedence directory already decided this id,
                // masking or not.
                continue;
            }
            let Ok(contents) = fs::read_to_string(&file) else {
                continue;
            };
            let Some(raw) = parse(&contents) else {
                continue;
            };
            if raw.get("Type").map(String::as_str) != Some("Application") {
                seen.insert(id, None);
                continue;
            }
            let hidden = is_true(raw.get("Hidden"));
            let no_display = is_true(raw.get("NoDisplay"));
            if hidden {
                // Masks the id outright, so a system entry of the same id
                // elsewhere on the path never shows either.
                seen.insert(id, None);
                continue;
            }
            if no_display {
                seen.insert(id, None);
                continue;
            }
            let name = localized(&raw, "Name", locales).unwrap_or_else(|| id.clone());
            let untranslated_name = raw.get("Name").cloned().unwrap_or_else(|| id.clone());
            let entry = Entry {
                id: id.clone(),
                name,
                untranslated_name,
                generic_name: localized(&raw, "GenericName", locales).unwrap_or_default(),
                icon: raw.get("Icon").cloned().unwrap_or_default(),
                exec: raw.get("Exec").cloned().unwrap_or_default(),
                categories: list(raw.get("Categories")),
                keywords: list(raw.get("Keywords")),
                terminal: is_true(raw.get("Terminal")),
                path: raw.get("Path").cloned(),
                source: file,
            };
            seen.insert(id, Some(entry));
        }
    }
    let mut entries: Vec<Entry> = seen.into_values().flatten().collect();
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    entries
}

/// Every `.desktop` file under `dir`, recursively (vendor subdirectories such
/// as `kde4/`), in no particular order -- `scan` sorts by precedence itself.
/// A `dir` that does not exist, which is most of `XDG_DATA_DIRS` on most
/// machines, scans to nothing rather than an error.
fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(read) = fs::read_dir(&current) else {
            continue;
        };
        for item in read.flatten() {
            let path = item.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(OsStr::to_str) == Some("desktop") {
                found.push(path);
            }
        }
    }
    found
}

/// `relative` (a `.desktop` file's path inside its applications directory) as
/// a desktop file id: `kde4/konsole.desktop` is `kde4-konsole`.
/// `tests::a_desktop_id_turns_path_separators_into_dashes`.
fn desktop_id(relative: &Path) -> String {
    let without_suffix = relative.with_extension("");
    without_suffix
        .components()
        .filter_map(|part| part.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("-")
}

/// One key's raw values, by group: `"Desktop Entry"`'s plain keys
/// (`Name`) and `Name[xx]` kept under `"Name[xx]"` so [`localized`] can
/// try each in turn. Only the `[Desktop Entry]` group is read; `[Desktop
/// Action ...]` groups are skipped (not in this version, see the module
/// doc).
type Raw = HashMap<String, String>;

/// Parse a `.desktop` file's `[Desktop Entry]` group. `None` when there is
/// no such group, which is not a file this scan should list.
/// `tests::comments_and_blank_lines_are_ignored`,
/// `tests::an_escaped_value_is_unescaped`.
fn parse(contents: &str) -> Option<Raw> {
    let mut fields = Raw::new();
    let mut in_entry = false;
    let mut found_entry = false;
    for line in contents.lines() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(group) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            in_entry = group == "Desktop Entry";
            found_entry |= in_entry;
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        fields.insert(key.trim().to_owned(), unescape(value.trim()));
    }
    found_entry.then_some(fields)
}

/// The Desktop Entry Specification's value escapes: `\s`, `\n`, `\t`, `\r`
/// and `\\`. `tests::an_escaped_value_is_unescaped`.
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// `key` localised by `locales`, most specific first: `key[xx_YY@mod]`,
/// `key[xx_YY]`, `key[xx@mod]`, `key[xx]`, falling back to the bare `key`.
/// `tests::localisation_tries_country_and_modifier_before_the_bare_language`.
fn localized(raw: &Raw, key: &str, locales: &[String]) -> Option<String> {
    for locale in locales {
        let (lang, modifier) = locale.split_once('@').unwrap_or((locale, ""));
        let (lang, country) = lang.split_once('_').unwrap_or((lang, ""));
        let tries: Vec<String> = if !country.is_empty() && !modifier.is_empty() {
            vec![
                format!("{key}[{lang}_{country}@{modifier}]"),
                format!("{key}[{lang}_{country}]"),
                format!("{key}[{lang}@{modifier}]"),
                format!("{key}[{lang}]"),
            ]
        } else if !country.is_empty() {
            vec![format!("{key}[{lang}_{country}]"), format!("{key}[{lang}]")]
        } else if !modifier.is_empty() {
            vec![
                format!("{key}[{lang}@{modifier}]"),
                format!("{key}[{lang}]"),
            ]
        } else {
            vec![format!("{key}[{lang}]")]
        };
        for name in tries {
            if let Some(value) = raw.get(&name) {
                return Some(value.clone());
            }
        }
    }
    raw.get(key).cloned()
}

/// `true` only for the spelling the specification requires.
fn is_true(value: Option<&String>) -> bool {
    value.is_some_and(|value| value == "true")
}

/// A `;`-separated list value (`Categories=`, `Keywords=`), its trailing
/// separator (every list value ends with one) dropped along with any empty
/// pieces.
fn list(value: Option<&String>) -> Vec<String> {
    value
        .map(|value| {
            value
                .split(';')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// What one launch is for: the field codes `%f`, `%F`, `%u` and `%U` expand
/// into, nothing (a plain launch with no files or URIs), unless a caller
/// (`apps.open`, P1) ever has them.
#[derive(Clone, Debug, Default)]
pub(crate) struct LaunchContext {
    pub(crate) uris: Vec<String>,
    pub(crate) files: Vec<String>,
}

/// `entry`'s `Exec=` expanded into a program and its arguments, field codes
/// resolved per the Desktop Entry Specification:
/// `tests::field_codes_expand_name_icon_and_the_desktop_file_itself`,
/// `tests::file_and_uri_codes_expand_from_the_launch_context`,
/// `tests::a_field_with_nothing_to_fill_it_is_dropped`,
/// `tests::quoted_arguments_keep_their_spaces_and_unescape`.
/// `Err` for `Exec=` that is empty or has unbalanced quotes.
pub(crate) fn launch_argv(entry: &Entry, ctx: &LaunchContext) -> Result<Vec<String>, String> {
    let tokens = tokenize(&entry.exec)?;
    if tokens.is_empty() {
        return Err("Exec= is empty".to_owned());
    }
    let mut argv = Vec::with_capacity(tokens.len());
    for token in tokens {
        expand_token(&token, entry, ctx, &mut argv);
    }
    if argv.is_empty() {
        return Err("Exec= expanded to nothing".to_owned());
    }
    Ok(argv)
}

/// One `Exec=` token (already past the spec's quoting) expanded. A token
/// that is only a field code with nothing to fill it contributes nothing, as
/// `%f` does with no file given; one with other text around the code (rare,
/// but not forbidden) keeps that text and drops only the code.
fn expand_token(token: &str, entry: &Entry, ctx: &LaunchContext, argv: &mut Vec<String>) {
    // The common case, and the only one that can expand to *several*
    // arguments (`%F`, `%U`): a token that is exactly one field code.
    match token {
        "%f" => {
            if let Some(file) = ctx.files.first() {
                argv.push(file.clone());
            }
            return;
        }
        "%F" => {
            argv.extend(ctx.files.iter().cloned());
            return;
        }
        "%u" => {
            if let Some(uri) = ctx.uris.first() {
                argv.push(uri.clone());
            }
            return;
        }
        "%U" => {
            argv.extend(ctx.uris.iter().cloned());
            return;
        }
        "%i" => {
            if !entry.icon.is_empty() {
                argv.push("--icon".to_owned());
                argv.push(entry.icon.clone());
            }
            return;
        }
        "%c" => {
            argv.push(entry.name.clone());
            return;
        }
        "%k" => {
            argv.push(entry.source.display().to_string());
            return;
        }
        "%%" => {
            argv.push("%".to_owned());
            return;
        }
        _ => {}
    }
    // A code beside other text in the same token (`--icon=%i`, say): expand
    // in place, single-valued codes only -- `%F`/`%U` cannot sensibly split
    // one token into several arguments mid-token, so the Specification's
    // deprecated multi-value codes are left as the single value when one is
    // there, nothing otherwise.
    let mut out = String::with_capacity(token.len());
    let mut chars = token.chars();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some('f' | 'F') => {
                if let Some(file) = ctx.files.first() {
                    out.push_str(file);
                }
            }
            Some('u' | 'U') => {
                if let Some(uri) = ctx.uris.first() {
                    out.push_str(uri);
                }
            }
            Some('i') => out.push_str(&entry.icon),
            Some('c') => out.push_str(&entry.name),
            Some('k') => out.push_str(&entry.source.display().to_string()),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    if !out.is_empty() {
        argv.push(out);
    }
}

/// `Exec=` into its tokens, the Specification's own quoting: unquoted
/// whitespace separates arguments; a double-quoted argument may contain
/// spaces, and inside it `\"`, `` \` ``, `\$` and `\\` are the only escapes
/// (everything else keeps its backslash). `Err` for a quote never closed.
fn tokenize(exec: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut chars = exec.chars();
    while let Some(ch) = chars.next() {
        match ch {
            ' ' | '\t' if !in_token => {}
            c if c.is_whitespace() => {
                if in_token {
                    tokens.push(std::mem::take(&mut current));
                    in_token = false;
                }
            }
            '"' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(escaped @ ('"' | '`' | '$' | '\\')) => current.push(escaped),
                            Some(other) => {
                                current.push('\\');
                                current.push(other);
                            }
                            None => return Err("unterminated escape in Exec=".to_owned()),
                        },
                        Some(other) => current.push(other),
                        None => return Err("unterminated quote in Exec=".to_owned()),
                    }
                }
            }
            other => {
                in_token = true;
                current.push(other);
            }
        }
    }
    if in_token {
        tokens.push(current);
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, relative: &str, contents: &str) {
        let path = dir.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("fixture directory");
        }
        fs::write(path, contents).expect("fixture file");
    }

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "solium-apps-test-{name}-{}-{:?}",
            std::process::id(),
            std::time::Instant::now()
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn a_desktop_id_turns_path_separators_into_dashes() {
        assert_eq!(desktop_id(Path::new("firefox.desktop")), "firefox");
        assert_eq!(
            desktop_id(Path::new("kde4/konsole.desktop")),
            "kde4-konsole"
        );
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let raw =
            parse("# a comment\n[Desktop Entry]\n\nName=Thing\n# also a comment\nIcon=thing\n")
                .expect("a Desktop Entry group");
        assert_eq!(raw.get("Name").map(String::as_str), Some("Thing"));
        assert_eq!(raw.get("Icon").map(String::as_str), Some("thing"));
    }

    #[test]
    fn an_escaped_value_is_unescaped() {
        assert_eq!(unescape(r"a\sb\nc\td\\e"), "a b\nc\td\\e");
    }

    #[test]
    fn localisation_tries_country_and_modifier_before_the_bare_language() {
        let raw = Raw::from([
            ("Name".to_owned(), "Settings".to_owned()),
            ("Name[ru]".to_owned(), "Настройки".to_owned()),
            ("Name[ru_RU]".to_owned(), "Настройки РУ".to_owned()),
        ]);
        let locales = vec!["ru_RU.UTF-8".replacen(".UTF-8", "", 1)];
        assert_eq!(
            localized(&raw, "Name", &locales).as_deref(),
            Some("Настройки РУ")
        );
        let locales = vec!["de".to_owned()];
        assert_eq!(
            localized(&raw, "Name", &locales).as_deref(),
            Some("Settings")
        );
    }

    #[test]
    fn a_higher_precedence_directory_wins_and_hidden_masks_a_lower_one() {
        let user = tmp("precedence-user");
        let system = tmp("precedence-system");
        write(
            &system,
            "app.desktop",
            "[Desktop Entry]\nType=Application\nName=System Version\nExec=app\n",
        );
        write(
            &user,
            "app.desktop",
            "[Desktop Entry]\nType=Application\nName=User Version\nExec=app --user\nHidden=true\n",
        );
        let entries = scan(&[user, system], &[]);
        assert!(
            entries.is_empty(),
            "a user Hidden=true should mask the system entry of the same id, not just itself"
        );
    }

    #[test]
    fn no_display_and_hidden_entries_are_left_out() {
        let dir = tmp("nodisplay");
        write(
            &dir,
            "a.desktop",
            "[Desktop Entry]\nType=Application\nName=A\nExec=a\nNoDisplay=true\n",
        );
        write(
            &dir,
            "b.desktop",
            "[Desktop Entry]\nType=Application\nName=B\nExec=b\n",
        );
        let entries = scan(&[dir], &[]);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "b");
    }

    fn entry(exec: &str) -> Entry {
        Entry {
            id: "thing".to_owned(),
            name: "The Thing".to_owned(),
            untranslated_name: "The Thing".to_owned(),
            generic_name: String::new(),
            icon: "thing-icon".to_owned(),
            exec: exec.to_owned(),
            categories: Vec::new(),
            keywords: Vec::new(),
            terminal: false,
            path: None,
            source: PathBuf::from("/usr/share/applications/thing.desktop"),
        }
    }

    #[test]
    fn field_codes_expand_name_icon_and_the_desktop_file_itself() {
        let e = entry("thing %i --name %c --desktop %k");
        let argv = launch_argv(&e, &LaunchContext::default()).expect("expands");
        assert_eq!(
            argv,
            vec![
                "thing",
                "--icon",
                "thing-icon",
                "--name",
                "The Thing",
                "--desktop",
                "/usr/share/applications/thing.desktop",
            ]
        );
    }

    #[test]
    fn file_and_uri_codes_expand_from_the_launch_context() {
        let e = entry("thing %F");
        let ctx = LaunchContext {
            files: vec!["/a".to_owned(), "/b".to_owned()],
            uris: Vec::new(),
        };
        assert_eq!(
            launch_argv(&e, &ctx).expect("expands"),
            vec!["thing", "/a", "/b"]
        );
    }

    #[test]
    fn a_field_with_nothing_to_fill_it_is_dropped() {
        let e = entry("thing %f %u");
        let argv = launch_argv(&e, &LaunchContext::default()).expect("expands");
        assert_eq!(argv, vec!["thing"]);
    }

    #[test]
    fn quoted_arguments_keep_their_spaces_and_unescape() {
        let e = entry(r#"thing "an arg with spaces" "a \"quoted\" word""#);
        let argv = launch_argv(&e, &LaunchContext::default()).expect("expands");
        assert_eq!(
            argv,
            vec!["thing", "an arg with spaces", "a \"quoted\" word"]
        );
    }

    /// SAFETY: the test harness runs tests for this crate on one thread
    /// (panics are denied, so no test catches another's env mutation
    /// mid-flight) -- see `rustfmt.toml`'s neighbour, `clippy.toml`, for the
    /// crate-wide "no concurrency surprises" stance this relies on.
    #[expect(unsafe_code, reason = "scoping env vars to one test")]
    fn set_env(name: &str, value: &str) {
        unsafe {
            std::env::set_var(name, value);
        }
    }

    #[expect(unsafe_code, reason = "scoping env vars to one test")]
    fn clear_env(name: &str) {
        unsafe {
            std::env::remove_var(name);
        }
    }

    #[test]
    fn locales_read_language_then_the_lc_chain() {
        set_env("LANGUAGE", "ru:en");
        clear_env("LC_ALL");
        clear_env("LC_MESSAGES");
        set_env("LANG", "en_US.UTF-8");
        let locales = preferred_locales();
        // `LANGUAGE`'s own order first, in full; `LANG`'s `en_US` (the
        // encoding stripped) still appended after, harmless and never
        // reached first since plain `en` already precedes it -- the
        // dedup only drops an *exact* repeat, not a less specific one.
        assert_eq!(
            locales,
            vec!["ru".to_owned(), "en".to_owned(), "en_US".to_owned()]
        );
        clear_env("LANGUAGE");
        clear_env("LANG");
    }
}
