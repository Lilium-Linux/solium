//! The desktop folder: `~/Desktop` (`Solium.dirs.desktop`), its entries as
//! `Folder`'s rows (04-ui.md §4.9), and the one-time trust a launcher on it
//! needs before `folder.open` will run it.
//!
//! **What is cut for this first version, and why.** A subdirectory is one
//! `isDir` row, not walked (one icon per entry, not a tree -- Open and a
//! folder window are both `Later`). MIME detection is
//! `/usr/share/mime/globs2`'s extension table, read by hand exactly as
//! `apps.rs` parses `.desktop` by hand, with no content sniffing: a file
//! with no recognised extension resolves to the generic
//! `application/octet-stream` (P1, alongside Open With and thumbnails).
//! Hidden only follows the leading-dot convention, not a `.hidden` list
//! (Nautilus's own further convention). Trust is a durable file of trusted
//! absolute paths, the simplest durable thing this repo already does for
//! small state (a plain file written with `fs::write`, the same shape
//! `session.rs` and `launch.rs` already use) -- tried first: on this
//! machine, `gio set <file> metadata::trusted true` itself answers "Setting
//! attribute metadata::trusted not supported", because GIO's `metadata::`
//! namespace is backed by the gvfs metadata daemon, not a plain extended
//! attribute, and nothing here can assume that daemon is running. `sol.store`
//! would be the natural fit once it exists (`lua/preview/dock.lua`'s own
//! module doc notes it does not yet).

use std::{
    collections::HashMap,
    fs,
    os::fd::{AsFd, OwnedFd},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use smithay::reexports::rustix::event::epoll;

/// One entry on the desktop: `Folder`'s row (04-ui.md §4.9's "Data").
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Entry {
    pub(crate) uri: String,
    /// The bare file name, as it is on disk.
    pub(crate) name: String,
    /// What the icon's label shows: a launcher's own `Name=`, or `name` for
    /// everything else.
    pub(crate) display_name: String,
    pub(crate) mime: String,
    /// A theme icon name, ready for `image://solium/icon/` (`icon.rs`).
    pub(crate) icon: String,
    pub(crate) is_dir: bool,
    pub(crate) is_launcher: bool,
    /// Always `false` for anything that is not a launcher.
    pub(crate) trusted: bool,
    pub(crate) hidden: bool,
    /// Seconds since the epoch; 0 when the file's own modified time cannot
    /// be read.
    pub(crate) modified: u64,
}

/// `Solium.dirs.desktop`: `$XDG_DESKTOP_DIR`, or the same key read from
/// `user-dirs.dirs`, `None` when nothing names it or it names `$HOME` itself
/// (04-ui.md §4.9: "nothing shown when `XDG_DESKTOP_DIR` is unset or is
/// `$HOME`" -- a user whose whole home is "the desktop" gets no icons drawn
/// over every file in it).
pub(crate) fn desktop_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok().filter(|v| !v.is_empty())?;
    let xdg_desktop_dir = std::env::var("XDG_DESKTOP_DIR")
        .ok()
        .filter(|v| !v.is_empty());
    let config_home = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| format!("{home}/.config"));
    let user_dirs = fs::read_to_string(PathBuf::from(config_home).join("user-dirs.dirs")).ok();
    resolve_desktop_dir(&home, xdg_desktop_dir.as_deref(), user_dirs.as_deref())
}

/// The pure core of [`desktop_dir`], taking what the environment and
/// `user-dirs.dirs` would otherwise supply, so it is testable without
/// mutating process-global environment state.
/// `tests::xdg_desktop_dir_wins_over_user_dirs`,
/// `tests::user_dirs_dirs_is_read_when_the_variable_is_unset`,
/// `tests::nothing_names_a_desktop_directory_is_none`,
/// `tests::a_desktop_directory_that_is_home_itself_is_none`.
fn resolve_desktop_dir(
    home: &str,
    xdg_desktop_dir: Option<&str>,
    user_dirs: Option<&str>,
) -> Option<PathBuf> {
    let raw = xdg_desktop_dir
        .map(ToOwned::to_owned)
        .or_else(|| user_dirs.and_then(|contents| parse_user_dirs(contents, "XDG_DESKTOP_DIR")))?;
    let path = PathBuf::from(expand_home(&raw, home));
    (path != Path::new(home)).then_some(path)
}

/// `user-dirs.dirs`' own format: `KEY="value"`, one per line, `#` comments.
/// `tests::user_dirs_reads_a_quoted_value`,
/// `tests::user_dirs_ignores_comments_and_other_keys`.
fn parse_user_dirs(contents: &str, key: &str) -> Option<String> {
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (found, value) = line.split_once('=')?;
        if found.trim() != key {
            continue;
        }
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(value);
        return Some(value.to_owned());
    }
    None
}

/// `$HOME` expanded, as `user-dirs.dirs` writes it (`"$HOME/Desktop"`).
fn expand_home(raw: &str, home: &str) -> String {
    raw.replace("$HOME", home)
}

/// The durable "this launcher may run" list: one absolute path per line
/// under `$XDG_DATA_HOME/solium/desktop-trust`. Keyed by path rather than by
/// content, so a launcher edited in place keeps its trust, the same way a
/// plain executable bit would; a path reused for a different file after the
/// first is deleted is not expected on a desktop folder, and is the trade
/// made for durability with no database (`Later`: a real store once
/// `sol.store` exists).
#[derive(Debug)]
pub(crate) struct Trust {
    path: PathBuf,
    trusted: std::collections::HashSet<String>,
}

impl Trust {
    pub(crate) fn load() -> Self {
        Self::at(trust_path())
    }

    /// `tests::trust_is_remembered_across_a_fresh_load`.
    fn at(path: PathBuf) -> Self {
        let trusted = fs::read_to_string(&path)
            .map(|contents| contents.lines().map(ToOwned::to_owned).collect())
            .unwrap_or_default();
        Self { path, trusted }
    }

    pub(crate) fn is_trusted(&self, absolute: &str) -> bool {
        self.trusted.contains(absolute)
    }

    /// Marks `absolute` trusted, durably. `tests::trust_is_remembered_across_a_fresh_load`.
    pub(crate) fn trust(&mut self, absolute: &str) {
        if self.trusted.insert(absolute.to_owned()) {
            self.save();
        }
    }

    fn save(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut lines: Vec<&str> = self.trusted.iter().map(String::as_str).collect();
        lines.sort_unstable();
        let _ = fs::write(&self.path, lines.join("\n"));
    }
}

fn trust_path() -> PathBuf {
    let data_home = std::env::var("XDG_DATA_HOME")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|home| format!("{home}/.local/share"))
        })
        .unwrap_or_else(|| "/tmp".to_owned());
    PathBuf::from(data_home).join("solium/desktop-trust")
}

/// A single `.desktop` file's `[Desktop Entry]` group, read for exactly what
/// a desktop icon needs: whether it is a launcher at all (`Type=Application`),
/// its label, its icon, and -- built as a full [`crate::apps::Entry`] so
/// `folder.open` can hand it straight to [`crate::apps::launch_argv`], the
/// same field-code expansion and quoting `apps.launch` already uses, rather
/// than a second copy of that parser here. Not `apps.rs`'s own `scan`: this
/// is one file, not an applications directory with precedence and
/// localisation across several. `None` when there is no `[Desktop Entry]`
/// group, or it is not `Type=Application` -- "not a launcher", not "parse
/// failed". `tests::a_desktop_file_becomes_a_launchable_entry`,
/// `tests::a_desktop_file_missing_type_application_is_not_a_launcher`.
fn read_launcher(path: &Path) -> Option<crate::apps::Entry> {
    let contents = fs::read_to_string(path).ok()?;
    let mut in_entry = false;
    let mut found_entry = false;
    let mut fields: HashMap<String, String> = HashMap::new();
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
        if let Some((key, value)) = line.split_once('=') {
            fields.insert(key.trim().to_owned(), value.trim().to_owned());
        }
    }
    if !found_entry || fields.get("Type").map(String::as_str) != Some("Application") {
        return None;
    }
    let name = fields.get("Name").cloned().unwrap_or_default();
    Some(crate::apps::Entry {
        id: String::new(),
        name: name.clone(),
        untranslated_name: name,
        generic_name: String::new(),
        icon: fields.get("Icon").cloned().unwrap_or_default(),
        exec: fields.get("Exec").cloned().unwrap_or_default(),
        categories: Vec::new(),
        keywords: Vec::new(),
        terminal: fields.get("Terminal").map(String::as_str) == Some("true"),
        path: fields.get("Path").cloned(),
        source: path.to_path_buf(),
    })
}

/// `/usr/share/mime/globs2`'s own format: `weight:mime/type:pattern`, one per
/// line, `#` comments. Only plain `*.ext` patterns are kept -- the rest of
/// the specification's glob grammar (character classes, literal whole-name
/// patterns such as `Makefile`) is not needed for an extension table and is
/// left for content sniffing to subsume later (this module's own cut, above).
/// On a tie the highest weight wins, matching the specification's own
/// precedence. `tests::globs2_keeps_the_highest_weight_on_a_tied_extension`.
fn parse_globs2(contents: &str) -> HashMap<String, (i32, String)> {
    let mut map: HashMap<String, (i32, String)> = HashMap::new();
    for line in contents.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, ':');
        let (Some(weight), Some(mime), Some(pattern)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let Some(suffix) = pattern.strip_prefix("*.") else {
            continue;
        };
        if suffix.is_empty() || suffix.contains(['*', '?', '[']) {
            continue;
        }
        let weight: i32 = weight.parse().unwrap_or(50);
        let suffix = suffix.to_lowercase();
        if map.get(&suffix).is_none_or(|(known, _)| weight > *known) {
            map.insert(suffix, (weight, mime.to_owned()));
        }
    }
    map
}

fn read_globs2() -> HashMap<String, (i32, String)> {
    fs::read_to_string("/usr/share/mime/globs2")
        .map(|contents| parse_globs2(&contents))
        .unwrap_or_default()
}

/// `name`'s MIME type by its longest matching suffix (`"archive.tar.gz"`
/// tries `"tar.gz"` before `"gz"`), the generic `application/octet-stream`
/// when nothing matches. `tests::the_longest_matching_suffix_wins`.
fn mime_by_name(name: &str, globs: &HashMap<String, (i32, String)>) -> String {
    let lower = name.to_lowercase();
    let mut rest = lower.as_str();
    while let Some(dot) = rest.find('.') {
        rest = &rest[dot + 1..];
        if let Some((_, mime)) = globs.get(rest) {
            return mime.clone();
        }
    }
    "application/octet-stream".to_owned()
}

/// Every entry of `dir`, sorted folders first, then by name (04-ui.md §4.9).
/// Not recursive: a subdirectory is one `isDir` row. `apps.rs`'s own choice
/// of a wholesale rescan over incremental tracking applies here too -- cheap
/// enough for a few hundred files on a desktop, and one scan to keep right
/// rather than a second, incremental model beside it.
pub(crate) fn scan(dir: &Path, trust: &Trust) -> Vec<Entry> {
    scan_with_globs(dir, trust, &read_globs2())
}

fn scan_with_globs(
    dir: &Path,
    trust: &Trust,
    globs: &HashMap<String, (i32, String)>,
) -> Vec<Entry> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<Entry> = Vec::new();
    for item in read.flatten() {
        let path = item.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let is_dir = path.is_dir();
        let hidden = name.starts_with('.');
        let launcher = (!is_dir && name.ends_with(".desktop"))
            .then(|| read_launcher(&path))
            .flatten();
        let is_launcher = launcher.is_some();
        let display_name = launcher
            .as_ref()
            .map(|entry| &entry.name)
            .filter(|name| !name.is_empty())
            .cloned()
            .unwrap_or_else(|| name.to_owned());
        let mime = if is_dir {
            "inode/directory".to_owned()
        } else if is_launcher {
            "application/x-desktop".to_owned()
        } else {
            mime_by_name(name, globs)
        };
        let icon = if is_dir {
            "folder".to_owned()
        } else if let Some(entry) = &launcher {
            if entry.icon.is_empty() {
                "application-x-executable".to_owned()
            } else {
                entry.icon.clone()
            }
        } else {
            mime.replace('/', "-")
        };
        let absolute = path.display().to_string();
        let modified = fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        entries.push(Entry {
            uri: format!("file://{absolute}"),
            name: name.to_owned(),
            display_name,
            mime,
            icon,
            is_dir,
            is_launcher,
            trusted: is_launcher && trust.is_trusted(&absolute),
            hidden,
            modified,
        });
    }
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
    entries
}

/// `uri`'s absolute path, for looking an [`Entry`] up by what `folder.open`
/// and `folder.trust` are handed: the `file://` scheme stripped, nothing
/// else decoded (no entry this module makes ever percent-encodes its own
/// path, so a round trip through `uri` needs none).
pub(crate) fn path_from_uri(uri: &str) -> Option<PathBuf> {
    uri.strip_prefix("file://").map(PathBuf::from)
}

/// `path`'s own `.desktop` file read again as a launchable
/// [`crate::apps::Entry`], for `folder.open` on a trusted launcher. `None`
/// when it no longer parses as one (removed, or edited since the scan that
/// listed it).
pub(crate) fn launcher_entry(path: &Path) -> Option<crate::apps::Entry> {
    read_launcher(path)
}

/// The desktop entry id of the default application for `mime`
/// (`folder.open`), by the freedesktop convention: first
/// `$XDG_CONFIG_HOME/mimeapps.list`'s `[Default Applications]` group, else
/// an applications directory's own `mimeinfo.cache`'s `[MIME Cache]` group
/// (`crate::apps::search_dirs`, most specific first), the first
/// semicolon-separated id either names. No "Added Associations" merging and
/// no `$XDG_CONFIG_DIRS` search in this version (P1): a user's own
/// `mimeapps.list` is the overwhelming common case a desktop's "open with"
/// dialog already writes to, and every installed application already
/// advertises itself through `mimeinfo.cache`.
/// `tests::mimeapps_list_names_the_first_id`, `tests::mime_cache_is_read_the_same_way`.
pub(crate) fn default_app_id(mime: &str) -> Option<String> {
    let config_home = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|home| format!("{home}/.config"))
        })?;
    if let Ok(contents) = fs::read_to_string(PathBuf::from(&config_home).join("mimeapps.list"))
        && let Some(id) = default_from_mimeapps(&contents, mime)
    {
        return Some(id);
    }
    for dir in crate::apps::search_dirs() {
        if let Ok(contents) = fs::read_to_string(dir.join("mimeinfo.cache"))
            && let Some(id) = default_from_mime_cache(&contents, mime)
        {
            return Some(id);
        }
    }
    None
}

/// `group`'s own `key=value` line, the `.desktop`/`.list`/`.cache` files'
/// shared ini-like shape (as `apps::parse` reads `[Desktop Entry]`, here for
/// whichever bracketed group is asked for).
fn group_value<'a>(contents: &'a str, group: &str, key: &str) -> Option<&'a str> {
    let mut in_group = false;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            in_group = name == group;
            continue;
        }
        if !in_group {
            continue;
        }
        if let Some((found_key, value)) = line.split_once('=')
            && found_key.trim() == key
        {
            return Some(value.trim());
        }
    }
    None
}

fn first_id(value: &str) -> Option<String> {
    value
        .split(';')
        .map(str::trim)
        .find(|id| !id.is_empty())
        .map(ToOwned::to_owned)
}

fn default_from_mimeapps(contents: &str, mime: &str) -> Option<String> {
    group_value(contents, "Default Applications", mime).and_then(first_id)
}

fn default_from_mime_cache(contents: &str, mime: &str) -> Option<String> {
    group_value(contents, "MIME Cache", mime).and_then(first_id)
}

/// Live updates: one inotify watch on the desktop directory, read
/// non-blocking off a descriptor the event loop itself watches -- `tty.rs`
/// and `winit.rs` register [`Watcher::source`] once, at start-up, the same
/// `Generic`+`Interest::READ`+`Mode::Level` shape every other descriptor
/// `qml::wake` already polls -- so a change is seen on an otherwise idle
/// desktop, not only when some other redraw happens to reach
/// [`Watcher::poll`] first. This module does not compute what changed, only
/// *that* something did, and asks for a wholesale [`scan`] again (its own
/// module doc above says why that is the right size here).
#[derive(Debug)]
pub(crate) struct Watcher {
    inner: Option<WatcherInner>,
    /// A permanent epoll instance that mirrors whichever inotify descriptor
    /// `inner` currently holds. Permanent so the event loop can hold one
    /// stable descriptor for the whole session (registered once, at
    /// start-up) instead of one that comes and goes with [`Watcher::set_path`]
    /// -- `set_path` only adds or removes the current inotify descriptor
    /// from it, it never rebuilds it. `None` when creating it failed, the
    /// same kind of warning as not being able to watch at all.
    outer: Option<OwnedFd>,
}

#[derive(Debug)]
struct WatcherInner {
    inotify: inotify::Inotify,
    watch: inotify::WatchDescriptor,
    path: PathBuf,
}

impl Watcher {
    pub(crate) fn new() -> Self {
        let outer = match epoll::create(epoll::CreateFlags::CLOEXEC) {
            Ok(outer) => Some(outer),
            Err(err) => {
                tracing::warn!(
                    ?err,
                    "cannot prepare the desktop folder's watch for the event loop; it will \
                     only refresh when something else asks for a frame"
                );
                None
            }
        };
        Self { inner: None, outer }
    }

    /// Point the watch at `dir`, or take it down for `None`. A no-op when
    /// already watching the same directory. Not being able to watch (the
    /// directory does not exist, or `inotify_init1` is refused) is a
    /// warning, not an error: the desktop still shows from the scan that
    /// asked for this, it just will not update again until the next reload.
    pub(crate) fn set_path(&mut self, dir: Option<&Path>) {
        if self.inner.as_ref().map(|watching| watching.path.as_path()) == dir {
            return;
        }
        if let Some(inner) = self.inner.take() {
            if let Some(outer) = &self.outer {
                let _ = epoll::delete(outer, inner.inotify.as_fd());
            }
            let _ = inner.inotify.watches().remove(inner.watch);
        }
        self.inner = dir.and_then(|dir| match install(dir) {
            Ok(inner) => {
                let added = self.outer.as_ref().map(|outer| {
                    epoll::add(
                        outer,
                        inner.inotify.as_fd(),
                        epoll::EventData::new_u64(0),
                        epoll::EventFlags::IN,
                    )
                });
                if let Some(Err(err)) = added {
                    tracing::warn!(
                        ?dir,
                        ?err,
                        "cannot put the desktop folder's watch in the event loop; it will \
                         only refresh when something else asks for a frame"
                    );
                }
                Some(inner)
            }
            Err(err) => {
                tracing::warn!(?dir, ?err, "cannot watch the desktop folder for changes");
                None
            }
        });
    }

    /// Whether anything changed since the last call: a non-blocking drain of
    /// whatever inotify has buffered, so the next call does not see the same
    /// events twice. `false` with nothing watched.
    pub(crate) fn poll(&mut self) -> bool {
        let Some(inner) = &mut self.inner else {
            return false;
        };
        let mut buffer = [0_u8; 4096];
        match inner.inotify.read_events(&mut buffer) {
            Ok(events) => events.count() > 0,
            Err(_) => false,
        }
    }

    /// A dup of the permanent epoll descriptor that mirrors whichever
    /// directory is watched, for the event loop to hold: see this struct's
    /// own doc. `None` when [`Watcher::new`] could not create it.
    pub(crate) fn source(&self) -> Option<OwnedFd> {
        self.outer.as_ref().and_then(|outer| outer.try_clone().ok())
    }
}

fn install(dir: &Path) -> std::io::Result<WatcherInner> {
    let inotify = inotify::Inotify::init()?;
    let watch = inotify.watches().add(
        dir,
        inotify::WatchMask::CREATE
            | inotify::WatchMask::DELETE
            | inotify::WatchMask::MODIFY
            | inotify::WatchMask::MOVE
            | inotify::WatchMask::ATTRIB,
    )?;
    Ok(WatcherInner {
        inotify,
        watch,
        path: dir.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "solium-folder-test-{name}-{}-{:?}",
            std::process::id(),
            std::time::Instant::now()
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn write(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, contents).expect("fixture file");
        path
    }

    fn no_trust() -> Trust {
        Trust::at(tmp("trust-unused").join("trust"))
    }

    #[test]
    fn user_dirs_reads_a_quoted_value() {
        let contents = "# comment\nXDG_DESKTOP_DIR=\"$HOME/Desktop\"\n";
        assert_eq!(
            parse_user_dirs(contents, "XDG_DESKTOP_DIR").as_deref(),
            Some("$HOME/Desktop")
        );
    }

    #[test]
    fn user_dirs_ignores_comments_and_other_keys() {
        let contents = "# XDG_DESKTOP_DIR=\"$HOME/Nope\"\nXDG_DOWNLOAD_DIR=\"$HOME/Downloads\"\n";
        assert_eq!(parse_user_dirs(contents, "XDG_DESKTOP_DIR"), None);
    }

    #[test]
    fn xdg_desktop_dir_wins_over_user_dirs() {
        let user_dirs = "XDG_DESKTOP_DIR=\"$HOME/FromFile\"\n";
        let resolved = resolve_desktop_dir("/home/x", Some("/home/x/FromVar"), Some(user_dirs));
        assert_eq!(resolved, Some(PathBuf::from("/home/x/FromVar")));
    }

    #[test]
    fn user_dirs_dirs_is_read_when_the_variable_is_unset() {
        let user_dirs = "XDG_DESKTOP_DIR=\"$HOME/Desktop\"\n";
        let resolved = resolve_desktop_dir("/home/x", None, Some(user_dirs));
        assert_eq!(resolved, Some(PathBuf::from("/home/x/Desktop")));
    }

    #[test]
    fn nothing_names_a_desktop_directory_is_none() {
        assert_eq!(resolve_desktop_dir("/home/x", None, None), None);
    }

    #[test]
    fn a_desktop_directory_that_is_home_itself_is_none() {
        let resolved = resolve_desktop_dir("/home/x", Some("/home/x"), None);
        assert_eq!(resolved, None);
    }

    #[test]
    fn globs2_keeps_the_highest_weight_on_a_tied_extension() {
        let globs = parse_globs2("40:text/plain:*.log\n60:application/x-log:*.log\n");
        assert_eq!(
            globs.get("log").map(|(_, mime)| mime.as_str()),
            Some("application/x-log")
        );
    }

    #[test]
    fn the_longest_matching_suffix_wins() {
        let globs = parse_globs2("50:application/gzip:*.gz\n50:application/x-tar-gz:*.tar.gz\n");
        assert_eq!(
            mime_by_name("archive.tar.gz", &globs),
            "application/x-tar-gz"
        );
        assert_eq!(mime_by_name("plain.gz", &globs), "application/gzip");
        assert_eq!(
            mime_by_name("no-extension", &globs),
            "application/octet-stream"
        );
    }

    #[test]
    fn a_desktop_file_becomes_a_launchable_entry() {
        let dir = tmp("launcher");
        let path = write(
            &dir,
            "krita.desktop",
            "[Desktop Entry]\nType=Application\nName=Krita\nIcon=krita\nExec=krita %f\n",
        );
        let entry = read_launcher(&path).expect("a launcher");
        assert_eq!(entry.name, "Krita");
        assert_eq!(entry.icon, "krita");
        assert_eq!(entry.exec, "krita %f");
    }

    #[test]
    fn a_desktop_file_missing_type_application_is_not_a_launcher() {
        let dir = tmp("not-a-launcher");
        let path = write(
            &dir,
            "link.desktop",
            "[Desktop Entry]\nType=Link\nURL=https://example.com\n",
        );
        assert!(read_launcher(&path).is_none());
    }

    #[test]
    fn scan_lists_files_folders_first_then_by_name() {
        let dir = tmp("sort");
        write(&dir, "b.txt", "");
        write(&dir, "a.txt", "");
        fs::create_dir(dir.join("Photos")).expect("a subdirectory");
        let entries = scan_with_globs(&dir, &no_trust(), &HashMap::new());
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, vec!["Photos", "a.txt", "b.txt"]);
        assert!(entries[0].is_dir);
        assert_eq!(entries[0].mime, "inode/directory");
        assert_eq!(entries[0].icon, "folder");
    }

    #[test]
    fn mime_and_icon_resolve_from_the_extension() {
        let dir = tmp("mime");
        write(&dir, "report.pdf", "");
        let globs = parse_globs2("50:application/pdf:*.pdf\n");
        let entries = scan_with_globs(&dir, &no_trust(), &globs);
        assert_eq!(entries[0].mime, "application/pdf");
        assert_eq!(entries[0].icon, "application-pdf");
    }

    #[test]
    fn hidden_files_are_flagged() {
        let dir = tmp("hidden");
        write(&dir, ".secrets", "");
        write(&dir, "visible.txt", "");
        let entries = scan_with_globs(&dir, &no_trust(), &HashMap::new());
        let hidden: HashMap<&str, bool> = entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.hidden))
            .collect();
        assert_eq!(hidden.get(".secrets"), Some(&true));
        assert_eq!(hidden.get("visible.txt"), Some(&false));
    }

    #[test]
    fn a_launcher_is_untrusted_until_folder_trust() {
        let dir = tmp("trust-scan");
        let desktop_file = write(
            &dir,
            "krita.desktop",
            "[Desktop Entry]\nType=Application\nName=Krita\nExec=krita\n",
        );
        let mut trust = Trust::at(dir.join("trust-store"));
        let before = scan_with_globs(&dir, &trust, &HashMap::new());
        assert!(before[0].is_launcher);
        assert!(!before[0].trusted);

        trust.trust(&desktop_file.display().to_string());
        let after = scan_with_globs(&dir, &trust, &HashMap::new());
        assert!(after[0].trusted);
    }

    #[test]
    fn trust_is_remembered_across_a_fresh_load() {
        let store = tmp("trust-durable").join("trust-store");
        let mut trust = Trust::at(store.clone());
        trust.trust("/home/x/Desktop/app.desktop");
        let reloaded = Trust::at(store);
        assert!(reloaded.is_trusted("/home/x/Desktop/app.desktop"));
    }

    #[test]
    fn a_file_appearing_and_disappearing_is_seen_live() {
        let dir = tmp("live");
        let mut watcher = Watcher::new();
        watcher.set_path(Some(&dir));
        // Draining whatever the watch's own creation buffered.
        watcher.poll();

        write(&dir, "new.txt", "");
        assert!(
            wait_for(|| watcher.poll()),
            "a created file should be seen live"
        );

        fs::remove_file(dir.join("new.txt")).expect("removing the fixture file");
        assert!(
            wait_for(|| watcher.poll()),
            "a removed file should be seen live"
        );
    }

    /// Unlike the test above, this never calls `poll` to find out: it
    /// registers [`Watcher::source`] with a real `calloop` event loop, the
    /// same way `tty.rs` and `winit.rs` do, and only dispatches that loop --
    /// so a change reaching it with no `poll` in the loop proves the
    /// descriptor itself wakes the loop, not just that the data is there
    /// once something else asks.
    #[test]
    fn the_event_loop_source_wakes_on_a_change_with_no_poll_in_the_loop() {
        use smithay::reexports::calloop::{
            EventLoop, Interest, Mode, PostAction, generic::Generic,
        };

        let dir = tmp("live-source");
        let mut watcher = Watcher::new();
        watcher.set_path(Some(&dir));
        // Draining whatever the watch's own creation buffered, same as above.
        watcher.poll();

        let source = watcher.source().expect("an epoll instance");
        let mut event_loop: EventLoop<bool> = EventLoop::try_new().expect("an event loop");
        event_loop
            .handle()
            .insert_source(
                Generic::new(source, Interest::READ, Mode::Level),
                |_, _, woke| {
                    *woke = true;
                    Ok(PostAction::Continue)
                },
            )
            .expect("inserting the source");

        write(&dir, "new.txt", "");

        let mut woke = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !woke && Instant::now() < deadline {
            event_loop
                .dispatch(Some(Duration::from_millis(20)), &mut woke)
                .expect("dispatching");
        }
        assert!(
            woke,
            "the registered source should wake the loop on its own, with nothing polling \
             the watcher directly"
        );
        // Draining so the fd is not left readable for whatever runs next.
        watcher.poll();
    }

    #[test]
    fn mimeapps_list_names_the_first_id() {
        let contents =
            "[Default Applications]\napplication/pdf=org.kde.okular.desktop;evince.desktop;\n";
        assert_eq!(
            default_from_mimeapps(contents, "application/pdf").as_deref(),
            Some("org.kde.okular.desktop")
        );
        assert_eq!(default_from_mimeapps(contents, "text/plain"), None);
    }

    #[test]
    fn mime_cache_is_read_the_same_way() {
        let contents = "[MIME Cache]\ntext/plain=gedit.desktop;\n";
        assert_eq!(
            default_from_mime_cache(contents, "text/plain").as_deref(),
            Some("gedit.desktop")
        );
    }

    #[test]
    fn no_desktop_folder_configured_draws_nothing() {
        let entries = scan_with_globs(Path::new("/does/not/exist"), &no_trust(), &HashMap::new());
        assert!(entries.is_empty());
    }

    /// Retries `poll` for up to a second: inotify delivers promptly on a
    /// local filesystem, but a non-blocking read taken the instant after the
    /// write can still race the kernel's own notification.
    fn wait_for(mut poll: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if poll() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
