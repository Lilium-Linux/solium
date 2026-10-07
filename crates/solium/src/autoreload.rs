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
fn resolve_roots(user_dir: Option<&Path>, config: &Path) -> Vec<PathBuf> {
    let Some(user_dir) = user_dir.filter(|dir| dir.is_dir()) else {
        return Vec::new();
    };
    let mut roots = vec![user_dir.to_path_buf()];
    if let Some(parent) = config.parent()
        && parent != user_dir
        && parent.is_dir()
    {
        roots.push(parent.to_path_buf());
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
    )
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
            resolve_roots(None, Path::new("/opt/solium/share/solium/lua/init.lua")),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn a_user_directory_is_watched_even_running_the_shipped_entry_point() {
        let user_dir = tmp("user-dir-shipped-entry");
        let roots = resolve_roots(
            Some(&user_dir),
            Path::new("/opt/solium/share/solium/lua/init.lua"),
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
        let roots = resolve_roots(Some(&user_dir), &config_dir.join("init.lua"));
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
        let roots = resolve_roots(Some(&user_dir), &user_dir.join("init.lua"));
        assert_eq!(roots, vec![user_dir.clone()]);
        let _ = fs::remove_dir_all(&user_dir);
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
}
