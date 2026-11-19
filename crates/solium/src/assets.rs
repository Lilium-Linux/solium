//! Where the compositor's own QML and effects are on *this* machine.
//!
//! One question, asked once, answered here, by the order `solium_paths`
//! keeps for every program Solium ships: a packager's baked
//! `SOLIUM_DATADIR`, the build tree while it is there, `share/solium` beside
//! the binary, and `/usr/share/solium`. Every shipped file the compositor
//! reads at run time (the pane bundles, `cursor.qml`, the loading scenes, the
//! `Solium` design system, the effects) lives under that one directory.
//!
//! This is the *shipped* half only. The user's `~/.config/solium` stays ahead
//! of everything here, and that precedence lives at each call site because it
//! is per-asset: `qml::import_path` puts the user's QML directory first on the
//! import path, and `style::places` searches the user's `panes/` first.
//! Dropping a
//! single `Solium/Theme.qml` into the user's directory is meant to restyle
//! everything without copying the rest, and that is most of the point of the
//! design. It does not yet, and #88 is why: `qml/host.cpp` hands the import
//! path to Qt one `addImportPath` at a time, each of which goes in front, so
//! the shipped `Solium` module is found before the user's.

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
/// cp -r crates/solium/qml crates/solium/effects crates/solium-lua/lua %{buildroot}%{_datadir}/solium/
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

/// The root the shipped QML and effects were found under.
///
/// Resolved once. The answer cannot change during a run — it is a question
/// about where this binary is and what is on disk beside it — and a compositor
/// that re-stats four directories every time a window opens a decoration is
/// paying for nothing.
pub(crate) fn root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let places = solium_paths::candidates(
            BAKED,
            Path::new(BUILD_TREE),
            std::env::current_exe().ok().as_deref(),
        );
        let (chosen, found) = solium_paths::choose(&places, holds_assets);
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
                "the shipped QML and effects are not installed anywhere this build can \
                 see. The session will start with no decorations, no scenes and no pointer. \
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

/// Where a program Solium ships beside itself is looked for, for a Solium
/// binary at `exe`: an installed layout's `<prefix>/libexec/<name>` first,
/// then `<name>` beside the binary, as a build tree has it.
pub(crate) fn helpers(exe: &Path, name: &str) -> Vec<PathBuf> {
    let Some(dir) = exe.parent() else {
        return Vec::new();
    };
    let mut places = Vec::new();
    if let Some(prefix) = dir.parent() {
        places.push(prefix.join("libexec").join(name));
    }
    places.push(dir.join(name));
    places
}

/// The first of [`helpers`] for this binary that is an executable file, or
/// where it was looked for.
pub(crate) fn helper(name: &str) -> Result<PathBuf, String> {
    use std::os::unix::fs::PermissionsExt as _;
    let Some(exe) = std::env::current_exe().ok() else {
        return Err(format!("no {name}: this binary does not know where it is"));
    };
    let places = helpers(&exe, name);
    places
        .iter()
        .find(|path| {
            std::fs::metadata(path)
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
        .cloned()
        .ok_or_else(|| {
            let looked: Vec<String> = places
                .iter()
                .map(|path| path.display().to_string())
                .collect();
            format!("no {name} at {}", looked.join(" or "))
        })
}

/// The shipped effect folders: `effects/<name>/`, beside `qml/`.
///
/// Not part of what `holds_assets` looks for: a build without them is a
/// session without the shipped effects, not one without frames (a root of
/// `qml/` alone is found: `tests::a_root_is_the_compositors_when_it_holds_its_qml`).
/// Beside it: `effect::host::tests::the_shipped_effects_are_beside_the_qml`.
pub(crate) fn effects() -> PathBuf {
    root().join("effects")
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

/// Whether this directory holds what the compositor ships: its QML, without
/// which a session draws no frames and no pointer. The effects are not asked
/// for ([`effects`] says why), and a `lua/` is the window manager's.
fn holds_assets(root: &Path) -> bool {
    root.join("qml").is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A root holding the named parts, under a name no other test uses.
    fn root_with(name: &str, parts: &[&str]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("solium-assets-{name}"));
        let _ = std::fs::remove_dir_all(&path);
        for part in parts {
            let _ = std::fs::create_dir_all(path.join(part));
        }
        let _ = std::fs::create_dir_all(&path);
        path
    }

    /// Its QML is what makes a root the compositor's. A `lua/` is the window
    /// manager's, and says nothing of the compositor: a root that holds only
    /// that is passed over for one that holds the QML.
    #[test]
    fn a_root_is_the_compositors_when_it_holds_its_qml() {
        assert!(holds_assets(&root_with("qml", &["qml"])));
        assert!(!holds_assets(&root_with("effects-only", &["effects"])));
        let lua = root_with("lua-only", &["lua"]);
        assert!(!holds_assets(&lua));
        let qml = root_with("with-qml", &["qml", "effects"]);
        let places = solium_paths::candidates(Some(&lua.to_string_lossy()), &qml, None);
        assert_eq!(solium_paths::choose(&places, holds_assets), (qml, true));
    }

    /// The live resolution, in this build, on this machine: the assets the
    /// tests and the gate read really are where `root` says.
    #[test]
    fn this_build_resolves_to_assets_that_exist() {
        // Guarded rather than asserted flat, because a packaging build is
        // entitled to bake a datadir and then `root` correctly points
        // somewhere else.
        if BAKED.is_none() {
            assert_eq!(root(), Path::new(BUILD_TREE));
        }
        assert!(
            qml().join("probe.qml").is_file(),
            "the QML directory {} has no probe.qml",
            qml().display()
        );
        assert!(
            effects().is_dir(),
            "there is no effects directory at {}",
            effects().display()
        );
    }
}
