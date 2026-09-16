//! Where the compositor's own QML and Lua are on *this* machine.
//!
//! One question, asked once, answered here. Every shipped file the compositor
//! reads at run time — the pane bundles, `cursor.qml`, the loading scenes, the
//! `Solium` design system, the Lua that is the configuration — lives under one
//! directory, and until this module existed each reader baked its own copy of
//! `CARGO_MANIFEST_DIR` and so named a path that is true on exactly one
//! machine: the one that compiled it. A binary copied to `/usr/bin/solium`
//! went looking in the developer's home directory, found nothing, and started
//! a session with no scripts, no decorations and no pointer. That is #66: not
//! that packaging was unfinished, but that it was impossible.
//!
//! The order below is the whole design, so it is worth stating plainly.
//!
//! | place | who sets it |
//! |---|---|
//! | `SOLIUM_DATADIR` baked at compile time | a packager, in a spec file |
//! | the build tree, if it is still there | cargo, in a development build |
//! | `<the binary>/../share/solium` | the install layout itself |
//! | `/usr/share/solium` | nobody; the last-ditch guess |
//!
//! Nearest first, and **every one of them is checked for the assets
//! themselves** — a `qml/` and a `lua/` under it, not merely a directory that
//! exists. That is what lets a single list serve both cases. A development
//! build has nothing baked and a build tree that is right there, so it takes
//! it and behaves exactly as it did before this module. An installed build has
//! a baked datadir, or failing that a build tree that no longer exists on this
//! machine and an executable whose own location says where the prefix is.
//!
//! This is the *shipped* half only. The user's `~/.config/solium` stays ahead
//! of everything here, and that precedence lives at each call site because it
//! is per-asset: `qml::import_path` puts the user's QML directory first on the
//! import path, `style::places` searches the user's `panes/` first,
//! `script::load` puts the user's Lua first on `package.path`. Dropping a
//! single `Solium/Theme.qml` into the user's directory has to restyle
//! everything without copying the rest, and that is most of the point of the
//! design.

use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

/// The datadir the build was told to use, if it was told one.
///
/// `option_env!` rather than any of the alternatives, and the reasons are
/// worth keeping because each alternative looks fine until it is used.
///
/// *Not a wrapper script exporting an environment variable.* That was the
/// obvious fix and it is the wrong one: it makes `/usr/bin/solium` — run by a
/// display manager, by a systemd unit, by someone typing it at a tty — behave
/// differently from the same binary run through the wrapper, and the binary is
/// what everything else in a Linux system points at. The answer belongs inside
/// it.
///
/// *Not a run-time environment variable of our own.* Same objection, and it
/// only moves the problem into whoever remembers to set it.
///
/// *Not a build script writing a path into `OUT_DIR`.* That is a second file
/// to keep in step with this one for no gain; `option_env!` already is a
/// compile-time constant, and it is nothing at all when nobody sets it, which
/// is precisely the development case.
///
/// So a packager writes one line and gets a binary that knows its own prefix:
///
/// ```sh
/// SOLIUM_DATADIR=%{_datadir}/solium cargo build --release
/// install -d       %{buildroot}%{_datadir}/solium
/// cp -r crates/solium/qml crates/solium/lua %{buildroot}%{_datadir}/solium/
/// ```
///
/// `build.rs` carries the matching `rerun-if-env-changed`, so changing it
/// rebuilds rather than silently keeping the old constant.
const BAKED: Option<&str> = option_env!("SOLIUM_DATADIR");

/// The tree this was compiled in.
///
/// Still baked, still by cargo, and still true of exactly one machine — but it
/// is a *candidate* now rather than the answer, and it is used only if it is
/// still on disk. That one existence check is the difference between a binary
/// that can be installed and one that cannot.
const BUILD_TREE: &str = env!("CARGO_MANIFEST_DIR");

/// The last-ditch guess, when nothing else says anything.
///
/// Judgment call, and it goes both ways, so: **yes**, the shipped location is
/// also searched at run time rather than reachable only through the
/// compile-time value. A first packaging attempt that installs the files in
/// the conventional place and forgets `SOLIUM_DATADIR` then still produces a
/// working compositor instead of a black screen with nothing in the log about
/// why. It is *last* precisely so it cannot cost anything: on a developer's
/// machine with a distribution package also installed, the build tree is found
/// first and the dev build keeps using its own assets, which is the one
/// property this change may not break.
const SYSTEM: &str = "/usr/share/solium";

/// The root the shipped QML and Lua were found under.
///
/// Resolved once. The answer cannot change during a run — it is a question
/// about where this binary is and what is on disk beside it — and a compositor
/// that re-stats four directories every time a window opens a decoration is
/// paying for nothing.
pub(crate) fn root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let places = candidates(BAKED, BUILD_TREE, std::env::current_exe().ok().as_deref());
        let (chosen, found) = choose(&places);
        if found {
            tracing::debug!(root = %chosen.display(), "shipped assets");
        } else {
            // Loud, once, and not fatal — the same trade `qml::start_on_gpu`
            // makes a few hundred lines away, for the same reason. Quitting
            // here would take the session down over a directory, and the user
            // of a compositor that will not start has no screen to read the
            // reason on. Starting anyway costs them a bare session, which is
            // survivable, *provided they can find out why* — and a bare
            // session with nothing in the log is the failure this project has
            // already been bitten by: no scripts, no decorations, no pointer,
            // and no clue.
            //
            // So this names every place it looked, not merely the one it
            // settled on. "Solium could not find its files" is not a bug
            // report; "it looked in these four directories" is.
            tracing::error!(
                looked_in = ?places,
                using = %chosen.display(),
                "the shipped QML and Lua are not installed anywhere this build can see. \
                 The session will start with no scripts, no decorations and no pointer. \
                 Install them under one of the directories above, or rebuild with \
                 SOLIUM_DATADIR set to where they are"
            );
        }
        chosen
    })
}

/// The shipped QML: `Solium/`, `panes/`, `loading/`, `cursor.qml`, the lot.
pub(crate) fn qml() -> PathBuf {
    root().join("qml")
}

/// The shipped Lua: `init.lua` and every module it pulls in.
pub(crate) fn lua() -> PathBuf {
    root().join("lua")
}

/// Resolve now, so the answer is logged at a known moment.
///
/// Called from `main` immediately after logging starts. Nothing depends on it
/// — [`root`] logs from inside its own initialiser and so cannot be bypassed —
/// but *when* it logs would otherwise be whenever the first consumer happened
/// to ask, which on the hardware backend is somewhere in the middle of session
/// startup. Pinning it here puts the line above everything it explains.
pub(crate) fn announce() {
    let _ = root();
}

/// Every place the shipped assets could be, nearest first.
///
/// Handed its inputs rather than reading them, so the order can be tested
/// against directories a test made rather than against whatever this machine
/// happens to have installed.
fn candidates(baked: Option<&str>, build_tree: &str, exe: Option<&Path>) -> Vec<PathBuf> {
    let mut places = Vec::with_capacity(4);
    places.extend(baked.map(PathBuf::from));
    places.push(PathBuf::from(build_tree));
    // The install layout answering for itself: `/usr/bin/solium` is two
    // `parent()`s away from `/usr`, and `share/solium` from there. This is what
    // makes an unbaked package work, and it is not only about `/usr` — a
    // tarball or AppImage unpacked at `/opt/solium/bin/solium` finds
    // `/opt/solium/share/solium` by the same arithmetic, with nothing
    // hard-coded about either prefix. `current_exe` resolves symlinks on
    // Linux, so a `/usr/local/bin` symlink points at the real installation
    // rather than at itself.
    places.extend(
        exe.and_then(Path::parent)
            .and_then(Path::parent)
            .map(|prefix| prefix.join("share").join("solium")),
    );
    places.push(PathBuf::from(SYSTEM));
    places
}

/// Whether this directory is really the one — both halves of it.
///
/// The question is "are the shipped assets here", not "does this directory
/// exist", and the two differ in a way that costs a session. An empty
/// `/usr/share/solium` left behind by an uninstall, or a stale checkout whose
/// path the build tree still names, is a directory; taking it would stop the
/// search at a place with nothing in it and shadow the candidate that would
/// have worked.
///
/// Both, and not either: a root with Lua and no QML is a compositor that runs
/// its bindings and draws no frames, and a root with QML and no Lua is one
/// that has decorations and no way to open a window. Half an installation is
/// worse than a missing one, because it starts.
fn holds_assets(root: &Path) -> bool {
    root.join("qml").is_dir() && root.join("lua").is_dir()
}

/// The first candidate that really holds them, and whether one did.
///
/// When none does — the other half of the judgment call above — this still
/// returns a path, the first candidate, rather than `None`. Two reasons. Every
/// caller wants a directory to join a file name onto, and threading an
/// `Option` out to all nine of them buys a `None` branch at each that can only
/// be "give up", which is what a missing file already does. And the first
/// candidate is the place the *build* meant: a packaged binary names its baked
/// datadir, a development one names its build tree. So the errors that follow
/// say `/usr/share/solium/lua/init.lua`, which is a sentence someone can act
/// on, rather than naming nothing at all.
fn choose(candidates: &[PathBuf]) -> (PathBuf, bool) {
    if let Some(found) = candidates.iter().find(|path| holds_assets(path)) {
        return (found.clone(), true);
    }
    (
        candidates
            .first()
            .cloned()
            .unwrap_or_else(|| PathBuf::from(SYSTEM)),
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A root with the shipped assets in it, under a name no other test uses.
    ///
    /// `qml/` and `lua/` and not merely the directory, because that is what
    /// [`holds_assets`] asks and a fixture that answers a different question
    /// than production does is not a fixture.
    ///
    /// A name per test. `cargo test` runs these in one process on several
    /// threads and two tests sharing a directory is one test deleting the
    /// other's fixture, which fails in whichever order the scheduler picks and
    /// so reads as flakiness.
    fn real(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("solium-assets-{name}"));
        let _ = std::fs::create_dir_all(path.join("qml"));
        let _ = std::fs::create_dir_all(path.join("lua"));
        path
    }

    /// A directory that is there and holds nothing — a leftover.
    fn empty(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("solium-assets-empty-{name}"));
        let _ = std::fs::remove_dir_all(&path);
        let _ = std::fs::create_dir_all(&path);
        path
    }

    /// A path that is definitely not there.
    fn absent(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("solium-assets-absent-{name}"));
        let _ = std::fs::remove_dir_all(&path);
        path
    }

    /// The development case, and the one that may not regress: `cargo build`
    /// and run it, with nothing set and no install step anywhere.
    #[test]
    fn a_development_build_finds_its_own_build_tree() {
        let tree = real("dev-tree");
        let places = candidates(None, &tree.to_string_lossy(), None);
        assert_eq!(choose(&places), (tree, true));
    }

    /// The packaged case: the build was told a datadir and the files are there.
    #[test]
    fn a_baked_datadir_wins() {
        let datadir = real("baked");
        let tree = real("baked-tree");
        // The build tree exists *too* — which is the situation inside an
        // rpmbuild, where both are on disk at once. The baked one is the
        // deliberate instruction, so it is the one that counts.
        let places = candidates(
            Some(&datadir.to_string_lossy()),
            &tree.to_string_lossy(),
            None,
        );
        assert_eq!(choose(&places), (datadir, true));
    }

    /// An empty directory is not an installation.
    ///
    /// The one an existence check gets wrong: `/usr/share/solium` left behind
    /// by an uninstall, or a stale checkout at the path the build tree names,
    /// is a directory and holds nothing. Stopping there would shadow the
    /// candidate that would have worked, and present as a session with no
    /// scripts on a machine where the files are plainly installed.
    #[test]
    fn a_directory_with_nothing_in_it_does_not_count() {
        let leftover = empty("leftover");
        let tree = real("leftover-tree");
        let places = candidates(
            Some(&leftover.to_string_lossy()),
            &tree.to_string_lossy(),
            None,
        );
        assert_eq!(choose(&places), (tree, true));
    }

    /// And half of one is not either — the half that is missing is the half
    /// that draws, or the half that binds a key.
    #[test]
    fn a_root_with_only_half_the_assets_does_not_count() {
        let half = empty("half");
        let _ = std::fs::create_dir_all(half.join("qml"));
        assert!(!holds_assets(&half), "no lua/ is not an installation");
        let both = real("half-complete");
        let places = candidates(Some(&half.to_string_lossy()), &both.to_string_lossy(), None);
        assert_eq!(choose(&places), (both, true));
    }

    /// A baked datadir that is not installed is not an answer.
    ///
    /// The case that makes `cargo test` work in a packaging environment: the
    /// spec baked `/usr/share/solium`, `%check` runs before `%install` put
    /// anything there, and the build tree is still the truth on that machine.
    #[test]
    fn a_baked_datadir_that_is_not_there_falls_through() {
        let missing = absent("baked-uninstalled");
        let tree = real("fallthrough-tree");
        let places = candidates(
            Some(&missing.to_string_lossy()),
            &tree.to_string_lossy(),
            None,
        );
        assert_eq!(choose(&places), (tree, true));
    }

    /// The installed binary with nothing baked, which is what a first
    /// packaging attempt produces: `/usr/bin/solium` finds `/usr/share/solium`
    /// by looking at where it is.
    #[test]
    fn an_installed_binary_finds_the_prefix_beside_it() {
        let prefix = std::env::temp_dir().join("solium-assets-prefix");
        let shipped = prefix.join("share").join("solium");
        let _ = std::fs::create_dir_all(shipped.join("qml"));
        let _ = std::fs::create_dir_all(shipped.join("lua"));
        let exe = prefix.join("bin").join("solium");
        // The build tree is gone: this binary was compiled somewhere else and
        // copied here, which is the entire point of an installable build.
        let tree = absent("prefix-tree");
        let places = candidates(None, &tree.to_string_lossy(), Some(&exe));
        assert_eq!(choose(&places), (shipped, true));
    }

    /// Nearest first, all four of them, in the order the module documents.
    #[test]
    fn the_order_is_baked_then_build_tree_then_prefix_then_system() {
        let exe = PathBuf::from("/opt/solium/bin/solium");
        let places = candidates(Some("/baked"), "/tree", Some(&exe));
        assert_eq!(
            places,
            vec![
                PathBuf::from("/baked"),
                PathBuf::from("/tree"),
                PathBuf::from("/opt/solium/share/solium"),
                PathBuf::from(SYSTEM),
            ]
        );
    }

    /// Nothing baked is one fewer candidate, not an empty entry.
    #[test]
    fn an_unbaked_build_has_no_datadir_candidate() {
        let places = candidates(None, "/tree", None);
        assert_eq!(places, vec![PathBuf::from("/tree"), PathBuf::from(SYSTEM)]);
    }

    /// Nowhere at all: a path anyway, the one the build meant, and `false` so
    /// the caller can say so out loud.
    #[test]
    fn nothing_installed_still_names_the_place_the_build_meant() {
        let missing = absent("nowhere-baked");
        let tree = absent("nowhere-tree");
        let places = candidates(
            Some(&missing.to_string_lossy()),
            &tree.to_string_lossy(),
            None,
        );
        let (chosen, found) = choose(&places);
        assert!(!found, "nothing was installed, so nothing was found");
        assert_eq!(chosen, missing, "the baked datadir is what the build meant");
    }

    /// Same, with nothing baked: the build tree is what that build meant.
    #[test]
    fn nothing_installed_and_nothing_baked_names_the_build_tree() {
        let tree = absent("nowhere-unbaked-tree");
        let places = candidates(None, &tree.to_string_lossy(), None);
        assert_eq!(choose(&places), (tree, false));
    }

    /// `qml` and `lua` are the two subdirectories, not two independent
    /// searches — a build that found its Lua and not its QML would be a
    /// configuration that half-loads, which is worse than either.
    #[test]
    fn qml_and_lua_are_both_under_one_root() {
        assert_eq!(qml(), root().join("qml"));
        assert_eq!(lua(), root().join("lua"));
    }

    /// The live resolution, in this build, on this machine: the assets the
    /// tests and the gate read really are where `root` says.
    #[test]
    fn this_build_resolves_to_assets_that_exist() {
        // Guarded rather than asserted flat, because a packaging build is
        // entitled to bake a datadir and then `root` correctly points
        // somewhere else — asserting the build tree unconditionally would make
        // this fail for being configured, which is not a defect.
        if BAKED.is_none() {
            assert_eq!(root(), Path::new(BUILD_TREE));
        }
        assert!(
            qml().join("probe.qml").is_file(),
            "the QML directory {} has no probe.qml",
            qml().display()
        );
        assert!(
            lua().join("init.lua").is_file(),
            "the Lua directory {} has no init.lua",
            lua().display()
        );
    }
}
