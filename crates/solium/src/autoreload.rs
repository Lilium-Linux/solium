//! Automatic reload (#223): notice a change under the running configuration
//! and reload on its own, the way `super+shift+r` already does.
//!
//! **Reuses `folder.rs`'s own shape.** One inotify instance, mirrored into a
//! permanent `epoll` descriptor so the event loop can hold one stable
//! `calloop` source for the whole session (`tty.rs`, `winit.rs`), drained in
//! the callback rather than left for a drawn frame to reach -- a
//! level-triggered source left readable with nothing draining it would wake
//! the loop on every iteration, which is exactly what
//! `folder::tests::one_poll_drains_everything_buffered_so_the_source_is_not_left_readable`
//! guards there and what this module's own
//! `tests::one_poll_drains_a_burst_of_writes` guards here.
//!
//! **What is different, and why.**
//!
//!  * *Several roots, not one.* The desktop is a single directory; a
//!    configuration is the user's own `~/.config/solium` (Lua, pane styles,
//!    loading scenes, a wallpaper, a shell) plus, when `SOLIUM_LUA_INIT`
//!    names somewhere else, that file's own directory too. See
//!    [`resolve_roots`].
//!  * *The officially supported overrides are roots too, wherever they
//!    point.* `SOLIUM_SHELL_SCENE`, `SOLIUM_PANE`, `SOLIUM_QML_TITLEBAR` and
//!    `SOLIUM_LOADING` are how a shell, a pane style or titlebar, or a
//!    loading scene is developed as its own project for one run (#223's own
//!    report), so the directory each one names is folded in too, even well
//!    outside `~/.config/solium`. See [`override_root_for`]. The same
//!    absolute path named in `config.lua` directly, with none of these set,
//!    is a gap this does not close -- `state/commands.rs`'s
//!    `warn_about_unwatched_configured_paths` says so instead of leaving it
//!    silent, and `docs/ricing.md`'s "Reloading without pressing anything"
//!    carries the same caveat.
//!  * *Recursive.* inotify has no flag for that, so [`watch_tree`] walks each
//!    root once, when [`Watcher::set_roots`] is called (startup, and again
//!    after every reload -- `state/commands.rs`'s `configure_autoreload`),
//!    and adds one watch per directory it finds. A subdirectory created
//!    afterwards (a new pane style folder, say) is not seen until the next
//!    reload walks the tree again; that is the documented cut, not a bug, and
//!    the common case -- editing a file that is already there -- is
//!    unaffected.
//!  * *Debounced.* A directory watch fires once per file *and* once more for
//!    the directory entry changing, and an editor that saves by writing a
//!    temporary file and renaming it over the original fires it at least
//!    twice more. [`Debounce`] turns a burst of these, however many, into
//!    exactly one reload, timed from the *last* one rather than the first.

use std::{
    fs,
    os::fd::{AsFd, OwnedFd},
    path::{Path, PathBuf},
    time::Duration,
};

use smithay::reexports::rustix::event::epoll;

/// `config.reload`, handed over by `init.lua` through `sol.auto_reload`.
///
/// `Copy` and compared whole (`state/commands.rs`'s `configure_autoreload`
/// logs only when it actually changed), the same shape `idle::Settings`
/// already is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Settings {
    /// Whether a change under the configuration reloads on its own. Turning
    /// this off does not merely let the quiet period run out and do nothing
    /// -- `configure_autoreload` tears every watch down, so the loop source
    /// that would otherwise wake for it never fires at all.
    pub(crate) automatic: bool,
    /// How long a burst of writes waits to go quiet before the one reload it
    /// earned. See [`Settings::default`] for the number and why.
    pub(crate) quiet_ms: u64,
}

impl Default for Settings {
    /// 300 ms: long enough that an editor's own "write a temp file, then
    /// rename it over the original" -- two or three inotify events a few
    /// milliseconds apart -- lands inside one quiet period, short enough
    /// that a single save still reads as instant. `automatic` defaults to
    /// on: issue #223 was reported from daily use precisely because a
    /// compositor that makes you remember a key for this is the thing being
    /// fixed.
    fn default() -> Self {
        Self {
            automatic: true,
            quiet_ms: 300,
        }
    }
}

/// A single pending reload's quiet period.
///
/// Holds one deadline, not a queue of them: noting a change always *moves*
/// the deadline `quiet` past that change, so a burst collapses into the one
/// reload that fires `quiet` after the *last* write in it, never one per
/// write. `now` is `Solium::clock`'s own `Duration` (seconds since the
/// process started), passed in rather than read here, for the same
/// testability `surface::reload_if_changed` and `cursor.rs`'s frame-asking
/// tests already use it for -- a test drives the deadline with plain
/// `Duration` arithmetic and needs no clock and no sleep at all.
#[derive(Debug, Default)]
pub(crate) struct Debounce {
    deadline: Option<Duration>,
}

impl Debounce {
    /// A change was seen at `now`; nothing should reload before `now + quiet`
    /// unless another change pushes the deadline out again first.
    /// `tests::a_burst_of_notes_moves_the_deadline_to_the_last_one`.
    pub(crate) fn note(&mut self, now: Duration, quiet: Duration) {
        self.deadline = Some(now + quiet);
    }

    /// `true` the first time `now` has reached a pending deadline -- and
    /// only that once, since answering clears it. `false` with nothing
    /// pending, or before the deadline.
    /// `tests::due_fires_once_at_the_deadline_and_never_before_it`.
    pub(crate) fn due(&mut self, now: Duration) -> bool {
        match self.deadline {
            Some(deadline) if now >= deadline => {
                self.deadline = None;
                true
            }
            _ => false,
        }
    }

    /// How much longer until a pending deadline, for rescheduling the timer
    /// that calls [`Self::due`]: `Duration::ZERO` with nothing pending, so a
    /// caller that asks anyway waits for no time at all rather than
    /// forever. `tests::remaining_counts_down_to_the_deadline`.
    pub(crate) fn remaining(&self, now: Duration) -> Duration {
        self.deadline
            .map_or(Duration::ZERO, |deadline| deadline.saturating_sub(now))
    }
}

/// What the one-shot timer in `tty.rs`/`winit.rs` does when it wakes, decided
/// by [`decide_timer_outcome`] rather than inline in either backend so the
/// decision is one piece of logic instead of two copies that could drift.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TimerOutcome {
    /// The quiet period really has elapsed with automatic reload still on:
    /// reload, and the timer is spent.
    Reload,
    /// Not due yet; sleep for exactly what is left rather than arming a
    /// second timer on top of this one.
    Wait(Duration),
    /// Nothing to do and the timer is spent -- either the debounce was not
    /// due, with nothing having armed it, or `automatic` is now off.
    Drop,
}

/// Whether the automatic-reload timer should reload, wait longer, or drop
/// itself, each time it wakes.
///
/// **`automatic` is read here, not only at the `calloop` fd callback that
/// arms the timer.** Nothing in `tty.rs` or `winit.rs` keeps the
/// `RegistrationToken` an armed `Timer::from_duration` hands back, so
/// `configure_autoreload` turning `automatic` off cannot cancel a timer
/// already counting down -- it still wakes at its deadline. Checking
/// `automatic` again right here, rather than trusting that arming it implied
/// it was still wanted, is what keeps a change noted a moment before the
/// toggle from reloading anyway: `configure_autoreload` also resets the
/// debounce when settings change, but that alone leaves `due` answering
/// `false` forever rather than answering "off", which would reschedule the
/// timer at `Duration::ZERO` forever instead of retiring it.
/// `tests::off_drops_the_timer_without_reloading_even_when_due`,
/// `tests::on_and_due_reloads`, `tests::on_and_not_due_waits_the_remainder`.
pub(crate) fn decide_timer_outcome(
    automatic: bool,
    debounce: &mut Debounce,
    now: Duration,
) -> TimerOutcome {
    if !automatic {
        return TimerOutcome::Drop;
    }
    if debounce.due(now) {
        return TimerOutcome::Reload;
    }
    TimerOutcome::Wait(debounce.remaining(now))
}

/// The pure core of [`watch_roots`]: which directories to watch, given the
/// user's own configuration directory (`None`, or `Some` only when it is
/// really a directory on this machine) and where the configuration actually
/// in charge lives.
///
/// Parameterised rather than reading `Scripts::user_config_dir` and
/// `Scripts::config_path` itself, the same split `folder::resolve_desktop_dir`
/// makes against `folder::desktop_dir` and for the same reason: a test drives
/// it with real temporary directories instead of mutating this process's own
/// `$HOME` and `$XDG_CONFIG_HOME`.
///
/// **No user directory at all watches nothing** -- not the shipped
/// `lua/` under wherever this build's assets live, which is a system
/// directory, possibly unwritable, and not the user's to have reloaded
/// behind them. `tests::no_user_directory_watches_nothing`.
///
/// **A user directory is watched even running the shipped entry point.**
/// Someone who has only dropped a pane style into
/// `~/.config/solium/qml/panes/` and never written an `init.lua` of their own
/// still gets it reloaded live -- the directory, not merely the files
/// `package.path` would `require`, is what issue #223 asked for.
/// `tests::a_user_directory_is_watched_even_running_the_shipped_entry_point`.
///
/// **The active configuration's own directory is added when it differs.**
/// `SOLIUM_LUA_INIT` can point anywhere, and whatever it loaded through
/// `require` resolves against that directory first
/// (`Scripts::load_carrying`'s own `package.path` order) -- so that directory
/// is watched too, unless it already *is* the user's.
/// `tests::the_configs_own_directory_is_added_when_it_differs`,
/// `tests::the_configs_own_directory_is_not_duplicated_when_it_is_the_users`.
///
/// **`overrides` are added unconditionally, even with no user directory at
/// all.** Unlike the shipped `lua/`, a directory named by
/// `SOLIUM_SHELL_SCENE`/`SOLIUM_PANE`/`SOLIUM_QML_TITLEBAR`/`SOLIUM_LOADING`
/// is not a system directory -- it is wherever this run was explicitly told
/// to look, and the whole point of the override is that nothing else need be
/// set up first. `tests::overrides_are_watched_even_with_no_user_directory`,
/// `tests::overrides_already_covered_by_a_root_are_not_duplicated`.
fn resolve_roots(user_dir: Option<&Path>, config: &Path, overrides: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(user_dir) = user_dir.filter(|dir| dir.is_dir()) {
        roots.push(user_dir.to_path_buf());
        if let Some(parent) = config.parent()
            && parent != user_dir
            && parent.is_dir()
        {
            roots.push(parent.to_path_buf());
        }
    }
    for over in overrides {
        if !roots.contains(over) {
            roots.push(over.clone());
        }
    }
    roots
}

/// [`resolve_roots`], against this process's real configuration -- called
/// from `state/commands.rs`'s `configure_autoreload`, itself reached from
/// `Command::AutoReload` on every start and every reload, so a directory
/// created since the last one (someone just wrote their first `user.lua`) is
/// picked up without restarting the session.
pub(crate) fn watch_roots() -> Vec<PathBuf> {
    resolve_roots(
        crate::script::Scripts::user_config_dir().as_deref(),
        &crate::script::Scripts::config_path(),
        &override_roots(),
    )
}

/// The environment variables a shell, a pane style or titlebar, or a loading
/// scene can be pointed at for one run, in the order each is tried against
/// its own setting (`lua/shell.lua`'s `scene`, `decoration::chosen`,
/// `pane::loading_source`) -- not that it matters here, since every one of
/// them is folded in the same way.
const OVERRIDE_VARS: [&str; 4] = [
    "SOLIUM_SHELL_SCENE",
    "SOLIUM_PANE",
    "SOLIUM_QML_TITLEBAR",
    "SOLIUM_LOADING",
];

/// What directory [`resolve_roots`] should watch for one override's raw
/// value, or `None` for a bare style name -- already somewhere
/// `resolve_roots` watches on its own, the same as a bare `SOLIUM_PANE=mine`
/// resolving under `~/.config/solium/qml/panes/` -- or a value that names
/// nowhere on this machine.
///
/// Parameterised on the value rather than reading the variable itself, the
/// same split `decoration::named_by` makes and for the same reason: a test
/// that exported one of these would decide every other test sharing the
/// process. `tests::override_root_for_a_bare_name_is_none`,
/// `tests::override_root_for_a_path_is_its_directory`,
/// `tests::override_root_for_a_directory_is_itself`,
/// `tests::override_root_for_a_path_nowhere_on_this_machine_is_none`.
fn override_root_for(value: &str) -> Option<PathBuf> {
    if value.is_empty() {
        return None;
    }
    // The same "a separator or `.qml` makes it a path" rule `style::resolve`
    // and `decoration::qml_path` already use for exactly these settings --
    // a bare name is a search under places already watched, not a location
    // of its own.
    if !value.contains('/') && !value.ends_with(".qml") {
        return None;
    }
    let expanded = expand_home(value);
    let path = PathBuf::from(&expanded);
    let dir = if path.is_dir() {
        path
    } else {
        path.parent()?.to_path_buf()
    };
    dir.is_dir().then_some(dir)
}

/// [`override_root_for`], over every variable in [`OVERRIDE_VARS`] set in
/// this process's real environment -- called from [`watch_roots`] only, so a
/// test drives the pure half above instead.
fn override_roots() -> Vec<PathBuf> {
    OVERRIDE_VARS
        .iter()
        .filter_map(|var| std::env::var(var).ok())
        .filter_map(|value| override_root_for(&value))
        .collect()
}

/// Expand a leading `~`, mirroring `decoration::shellexpand` and
/// `pane::expand`: read from an environment variable, nothing between here
/// and the filesystem would have done it.
fn expand_home(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => {
            std::env::var("HOME").map_or_else(|_| path.to_owned(), |home| format!("{home}/{rest}"))
        }
        None => path.to_owned(),
    }
}

/// Whether `path` is already somewhere automatic reload watches -- `path`
/// itself, or any directory under one of `roots`.
///
/// Used both ways: a hit means `watch_roots` already covers a configured
/// scene (nothing to warn about); a miss, checked by
/// `state/commands.rs`'s `warn_about_unwatched_configured_paths` against
/// `roots` padded with the shipped asset directories, means it is worth
/// saying so. `tests::a_path_under_a_root_is_watched`,
/// `tests::a_path_beside_a_root_is_not`.
pub(crate) fn path_is_watched(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| path.starts_with(root))
}

const MASK: inotify::WatchMask = inotify::WatchMask::CREATE
    .union(inotify::WatchMask::DELETE)
    .union(inotify::WatchMask::MODIFY)
    .union(inotify::WatchMask::MOVE)
    .union(inotify::WatchMask::ATTRIB);

/// Live updates over several directory trees at once: `folder::Watcher`'s own
/// epoll-mirrored inotify instance, generalised to more than one root and to
/// watching each root recursively. See this module's own doc for both
/// differences and why each is there.
#[derive(Debug)]
pub(crate) struct Watcher {
    inner: Option<Inner>,
    /// A permanent epoll instance mirroring whichever inotify descriptor
    /// `inner` currently holds, for the same reason `folder::Watcher` keeps
    /// one: the event loop holds one stable descriptor for the whole
    /// session, registered once, and [`Self::set_roots`] only ever swaps what
    /// is inside it.
    outer: Option<OwnedFd>,
}

#[derive(Debug)]
struct Inner {
    inotify: inotify::Inotify,
    watches: Vec<inotify::WatchDescriptor>,
    roots: Vec<PathBuf>,
}

impl Watcher {
    pub(crate) fn new() -> Self {
        let outer = match epoll::create(epoll::CreateFlags::CLOEXEC) {
            Ok(outer) => Some(outer),
            Err(err) => {
                tracing::warn!(
                    ?err,
                    "cannot prepare the configuration watch for the event loop; automatic \
                     reload will not work this session, but `super+shift+r` still does"
                );
                None
            }
        };
        Self { inner: None, outer }
    }

    /// Point the watch at exactly `roots`, recursively, or take it down for
    /// an empty list -- which is what `automatic = false` does
    /// (`state/commands.rs`'s `configure_autoreload`). A no-op when `roots`
    /// is already what is watched, the same guard `folder::Watcher::set_path`
    /// makes against rebuilding a watch nothing asked to change.
    pub(crate) fn set_roots(&mut self, roots: &[PathBuf]) {
        if self
            .inner
            .as_ref()
            .is_some_and(|inner| inner.roots == roots)
        {
            return;
        }
        if let Some(inner) = self.inner.take() {
            if let Some(outer) = &self.outer {
                let _ = epoll::delete(outer, inner.inotify.as_fd());
            }
            for watch in inner.watches {
                let _ = inner.inotify.watches().remove(watch);
            }
        }
        if roots.is_empty() {
            return;
        }
        match install(roots) {
            Ok(inner) => {
                if let Some(outer) = &self.outer
                    && let Err(err) = epoll::add(
                        outer,
                        inner.inotify.as_fd(),
                        epoll::EventData::new_u64(0),
                        epoll::EventFlags::IN,
                    )
                {
                    tracing::warn!(
                        ?err,
                        "cannot put the configuration watch in the event loop; it will only \
                         reload on `super+shift+r`"
                    );
                }
                self.inner = Some(inner);
            }
            Err(err) => {
                tracing::warn!(?err, "cannot watch the configuration for automatic reload");
            }
        }
    }

    /// Whether anything is watched right now: `tests` only, to check
    /// [`Self::set_roots`] without reaching into a private field.
    #[cfg(test)]
    pub(crate) fn is_watching(&self) -> bool {
        self.inner.is_some()
    }

    /// Whether anything changed since the last call: a non-blocking drain of
    /// everything inotify has buffered, the same shape
    /// `folder::Watcher::poll` drains for the same reason -- so the next call
    /// does not see the same events twice and the level-triggered loop
    /// source is not left readable. `false` with nothing watched.
    pub(crate) fn poll(&mut self) -> bool {
        let Some(inner) = &mut self.inner else {
            return false;
        };
        let mut buffer = [0_u8; 4096];
        let mut changed = false;
        while let Ok(events) = inner.inotify.read_events(&mut buffer) {
            if events.count() == 0 {
                break;
            }
            changed = true;
        }
        changed
    }

    /// A dup of the permanent epoll descriptor, for the event loop to hold:
    /// see this struct's own doc. `None` only when [`Self::new`] could not
    /// create it.
    pub(crate) fn source(&self) -> Option<OwnedFd> {
        self.outer.as_ref().and_then(|outer| outer.try_clone().ok())
    }
}

fn install(roots: &[PathBuf]) -> std::io::Result<Inner> {
    let inotify = inotify::Inotify::init()?;
    let mut watches = Vec::new();
    for root in roots {
        watch_tree(&inotify, root, &mut watches);
    }
    Ok(Inner {
        inotify,
        watches,
        roots: roots.to_vec(),
    })
}

/// Adds a watch on `dir` and, recursively, on every subdirectory under it.
///
/// Symlinks are not followed: `DirEntry::file_type` answers from the
/// directory entry itself rather than from what it points at, which is what
/// keeps a pane style symlinked into its own parent from looping forever.
/// A directory that cannot be watched or read -- removed since
/// [`Watcher::set_roots`] started walking, or no permission -- is skipped
/// with a warning rather than failing every sibling: the same "a warning,
/// not an error" choice `folder::Watcher::set_path` makes about the one
/// directory it watches.
/// `tests::a_change_in_a_subdirectory_is_seen_live`.
fn watch_tree(inotify: &inotify::Inotify, dir: &Path, watches: &mut Vec<inotify::WatchDescriptor>) {
    let watch = match inotify.watches().add(dir, MASK) {
        Ok(watch) => watch,
        Err(err) => {
            tracing::warn!(
                ?dir,
                ?err,
                "cannot watch this configuration directory for changes"
            );
            return;
        }
    };
    watches.push(watch);
    let Ok(read) = fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            watch_tree(inotify, &entry.path(), watches);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "solium-autoreload-test-{name}-{}-{:?}",
            std::process::id(),
            Instant::now()
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn write(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, contents).expect("fixture file");
        path
    }

    /// Retries `poll` for up to a second, the same allowance
    /// `folder::tests::wait_for` gives inotify to actually deliver on a local
    /// filesystem.
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

    #[test]
    fn no_user_directory_watches_nothing() {
        assert_eq!(
            resolve_roots(
                None,
                Path::new("/opt/solium/share/solium/lua/init.lua"),
                &[]
            ),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn a_user_directory_is_watched_even_running_the_shipped_entry_point() {
        let user_dir = tmp("user-dir-shipped-entry");
        let roots = resolve_roots(
            Some(&user_dir),
            Path::new("/opt/solium/share/solium/lua/init.lua"),
            &[],
        );
        assert_eq!(roots, vec![user_dir.clone()]);
        let _ = fs::remove_dir_all(&user_dir);
    }

    #[test]
    fn the_configs_own_directory_is_added_when_it_differs() {
        let user_dir = tmp("user-dir-plus-config");
        // Stands in for `SOLIUM_LUA_INIT` naming a file outside the user's
        // own directory.
        let config_dir = tmp("config-dir-plus-config");
        let roots = resolve_roots(Some(&user_dir), &config_dir.join("init.lua"), &[]);
        assert_eq!(
            roots.len(),
            2,
            "both the user's directory and the config's own"
        );
        assert!(roots.contains(&user_dir));
        assert!(roots.contains(&config_dir));
        let _ = fs::remove_dir_all(&user_dir);
        let _ = fs::remove_dir_all(&config_dir);
    }

    #[test]
    fn the_configs_own_directory_is_not_duplicated_when_it_is_the_users() {
        let user_dir = tmp("user-dir-dup");
        let roots = resolve_roots(Some(&user_dir), &user_dir.join("init.lua"), &[]);
        assert_eq!(roots, vec![user_dir.clone()]);
        let _ = fs::remove_dir_all(&user_dir);
    }

    /// The concrete failure scenario the review reported: a shell developed
    /// as its own project, named by `SOLIUM_SHELL_SCENE`, with no
    /// `~/.config/solium` in the picture at all.
    #[test]
    fn overrides_are_watched_even_with_no_user_directory() {
        let shell_dir = tmp("override-no-user-dir");
        let roots = resolve_roots(
            None,
            Path::new("/opt/solium/share/solium/lua/init.lua"),
            std::slice::from_ref(&shell_dir),
        );
        assert_eq!(
            roots,
            vec![shell_dir.clone()],
            "an override names somewhere real; the lack of a user directory must not drop it"
        );
        let _ = fs::remove_dir_all(&shell_dir);
    }

    #[test]
    fn overrides_already_covered_by_a_root_are_not_duplicated() {
        let user_dir = tmp("override-already-covered");
        let roots = resolve_roots(
            Some(&user_dir),
            &user_dir.join("init.lua"),
            std::slice::from_ref(&user_dir),
        );
        assert_eq!(
            roots,
            vec![user_dir.clone()],
            "the override names exactly the directory already watched"
        );
        let _ = fs::remove_dir_all(&user_dir);
    }

    #[test]
    fn override_root_for_a_bare_name_is_none() {
        assert_eq!(
            override_root_for("mine"),
            None,
            "a bare SOLIUM_PANE=mine resolves under the user's own tree, already watched"
        );
    }

    #[test]
    fn override_root_for_an_empty_value_is_none() {
        assert_eq!(override_root_for(""), None);
    }

    #[test]
    fn override_root_for_a_path_is_its_directory() {
        let dir = tmp("override-root-path");
        let scene = write(&dir, "shell.qml", "");
        assert_eq!(
            override_root_for(scene.to_str().expect("utf-8 temp path")),
            Some(dir.clone()),
            "a file is watched by watching the directory holding it"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn override_root_for_a_directory_is_itself() {
        let dir = tmp("override-root-dir");
        assert_eq!(
            override_root_for(&format!("{}/", dir.display())),
            Some(dir.clone()),
            "a bundle directory named outright is watched directly"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn override_root_for_a_path_nowhere_on_this_machine_is_none() {
        assert_eq!(
            override_root_for("/does/not/exist/anywhere/shell.qml"),
            None,
            "nothing to watch for a directory that is not there"
        );
    }

    #[test]
    fn a_path_under_a_root_is_watched() {
        let root = PathBuf::from("/home/me/dev/my-shell");
        assert!(path_is_watched(
            &root.join("qml").join("Shell.qml"),
            std::slice::from_ref(&root)
        ));
        assert!(
            path_is_watched(&root, std::slice::from_ref(&root)),
            "the root itself counts"
        );
    }

    #[test]
    fn a_path_beside_a_root_is_not() {
        let root = PathBuf::from("/home/me/dev/my-shell");
        let sibling = PathBuf::from("/home/me/dev/my-shell-notes/Shell.qml");
        assert!(!path_is_watched(&sibling, std::slice::from_ref(&root)));
    }

    #[test]
    fn a_file_appearing_is_seen_live() {
        let dir = tmp("live");
        let mut watcher = Watcher::new();
        watcher.set_roots(std::slice::from_ref(&dir));
        watcher.poll();

        write(&dir, "init.lua", "");
        assert!(
            wait_for(|| watcher.poll()),
            "a created file should be seen live"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// An editor's own save: write a temporary file, then rename it over the
    /// original. The directory watch sees the rename as `CREATE`/`MOVE` on
    /// its own entry, with no separate watch on the file needed.
    #[test]
    fn a_rename_save_is_seen_live() {
        let dir = tmp("rename-save");
        let target = write(&dir, "init.lua", "");
        let staged = write(&dir, "init.lua.tmp", "changed");
        let mut watcher = Watcher::new();
        watcher.set_roots(std::slice::from_ref(&dir));
        watcher.poll();

        fs::rename(&staged, &target).expect("renaming over the original");
        assert!(
            wait_for(|| watcher.poll()),
            "a rename-save should be seen live"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A file created two directories under a watched root is still seen --
    /// the recursive half of [`watch_tree`] that a plain, single-directory
    /// `folder::Watcher` has no need of.
    #[test]
    fn a_change_in_a_subdirectory_is_seen_live() {
        let dir = tmp("recursive");
        let sub = dir.join("qml").join("panes").join("mystyle");
        fs::create_dir_all(&sub).expect("nested fixture directories");
        let mut watcher = Watcher::new();
        watcher.set_roots(std::slice::from_ref(&dir));
        watcher.poll();

        write(&sub, "Pane.qml", "import QtQuick\n");
        assert!(
            wait_for(|| watcher.poll()),
            "a file created under a watched subdirectory should be seen live"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// Mirrors `folder::tests::one_poll_drains_everything_buffered_so_the_source_is_not_left_readable`:
    /// a burst of writes, however many, is drained by one `poll`, so the
    /// level-triggered loop source `tty.rs` and `winit.rs` register is not
    /// left readable with nothing to drain it.
    #[test]
    fn one_poll_drains_a_burst_of_writes() {
        let dir = tmp("drain");
        let mut watcher = Watcher::new();
        watcher.set_roots(std::slice::from_ref(&dir));
        watcher.poll();

        for n in 0..300 {
            write(&dir, &format!("file-{n:03}.lua"), "");
        }
        assert!(
            wait_for(|| watcher.poll()),
            "three hundred new files should be seen"
        );
        assert!(
            !watcher.poll(),
            "one poll should have drained every buffered event, leaving nothing for a second"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn due_fires_once_at_the_deadline_and_never_before_it() {
        let quiet = Duration::from_millis(50);
        let mut debounce = Debounce::default();
        assert!(
            !debounce.due(Duration::ZERO),
            "nothing pending fires nothing"
        );

        debounce.note(Duration::ZERO, quiet);
        assert!(
            !debounce.due(quiet - Duration::from_millis(1)),
            "must not fire before the quiet period elapses"
        );
        assert!(
            debounce.due(quiet),
            "must fire once the quiet period has elapsed"
        );
        assert!(
            !debounce.due(quiet),
            "a second check at the same instant must not fire again"
        );
    }

    #[test]
    fn a_burst_of_notes_moves_the_deadline_to_the_last_one() {
        let quiet = Duration::from_millis(50);
        let mut debounce = Debounce::default();
        // Five notes in a row, each well inside the previous one's quiet
        // period, as a burst of saves would arrive.
        for ms in [0, 10, 20, 30, 40] {
            debounce.note(Duration::from_millis(ms), quiet);
        }
        assert!(
            !debounce.due(Duration::from_millis(40) + quiet - Duration::from_millis(1)),
            "must not fire before the *last* note's own quiet period elapses"
        );
        assert!(
            debounce.due(Duration::from_millis(40) + quiet),
            "must fire once the last note's quiet period has elapsed"
        );
        assert!(
            !debounce.due(Duration::from_millis(400)),
            "a burst fires once, not once per write in it"
        );
    }

    #[test]
    fn remaining_counts_down_to_the_deadline() {
        let quiet = Duration::from_millis(50);
        let mut debounce = Debounce::default();
        assert_eq!(
            debounce.remaining(Duration::ZERO),
            Duration::ZERO,
            "nothing pending"
        );

        debounce.note(Duration::from_millis(10), quiet);
        assert_eq!(debounce.remaining(Duration::from_millis(10)), quiet);
        assert_eq!(
            debounce.remaining(Duration::from_millis(30)),
            Duration::from_millis(30)
        );
        assert_eq!(
            debounce.remaining(Duration::from_millis(100)),
            Duration::ZERO
        );
    }

    /// The review's own failure scenario: `automatic` turned off between the
    /// timer being armed and it firing must not reload, whatever the
    /// debounce says -- and it must not be asked to wait either, since
    /// nothing will turn `automatic` back on by itself.
    #[test]
    fn off_drops_the_timer_without_reloading_even_when_due() {
        let quiet = Duration::from_millis(50);
        let mut debounce = Debounce::default();
        debounce.note(Duration::ZERO, quiet);
        assert_eq!(
            decide_timer_outcome(false, &mut debounce, quiet),
            TimerOutcome::Drop,
            "automatic off must win over a debounce that has come due"
        );
    }

    #[test]
    fn on_and_due_reloads() {
        let quiet = Duration::from_millis(50);
        let mut debounce = Debounce::default();
        debounce.note(Duration::ZERO, quiet);
        assert_eq!(
            decide_timer_outcome(true, &mut debounce, quiet),
            TimerOutcome::Reload
        );
    }

    #[test]
    fn on_and_not_due_waits_the_remainder() {
        let quiet = Duration::from_millis(50);
        let mut debounce = Debounce::default();
        // A later note than the timer itself was armed for -- the same
        // "another write arrived" case `remaining` already handles.
        debounce.note(Duration::from_millis(20), quiet);
        assert_eq!(
            decide_timer_outcome(true, &mut debounce, Duration::from_millis(30)),
            TimerOutcome::Wait(Duration::from_millis(40))
        );
    }
}
