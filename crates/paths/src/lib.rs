//! Where Solium's programs find what they ship, and what the user wrote.
//!
//! One question with one answer, asked by the compositor for its QML and
//! effects and by the shipped window manager for its Lua. Each program bakes
//! its own two constants (`option_env!("SOLIUM_DATADIR")` and its own
//! `CARGO_MANIFEST_DIR`) and asks here; the order of the places, and what
//! counts as finding something, is decided once.
//!
//! The order is the whole design:
//!
//! | place | who sets it |
//! |---|---|
//! | `SOLIUM_DATADIR` baked at compile time | a packager, in a spec file |
//! | the build tree, if it is still there | cargo, in a development build |
//! | `<the binary>/../../share/solium` | the install layout itself |
//! | `/usr/share/solium` | nobody; the last-ditch guess |
//!
//! Why each, in turn:
//!
//! - *A baked datadir, not a run-time variable or a wrapper script.* A
//!   wrapper makes `/usr/bin/solium` behave differently when a display
//!   manager or a systemd unit starts it than when the wrapper does, and a
//!   variable only moves the problem to whoever has to remember it. A
//!   packager writes one line, and the binary knows its own prefix.
//! - *The build tree, but only if it is still on disk.* It is true of the one
//!   machine that compiled the binary, and checking that it is there is the
//!   difference between a binary that can be installed and one that cannot
//!   (#66: a copied binary went looking in the developer's home directory).
//! - *Two directories up from the binary.* `<prefix>/bin/solium` and
//!   `<prefix>/libexec/solium-lua` are both two `parent()`s from `<prefix>`,
//!   so an unbaked package, a tarball unpacked under `/opt` and an AppImage
//!   all find `share/solium` by the same arithmetic, with nothing hard-coded
//!   about the prefix.
//! - *`/usr/share/solium` last,* so that a first package that forgets
//!   `SOLIUM_DATADIR` still works, and so that a development build on a
//!   machine with a package installed keeps finding its own build tree.
//!
//! The user's own directory (`~/.config/solium`) comes before all of these,
//! per file, and each reader decides how: a QML import path, a pane style
//! search, Lua's `package.path`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The last-ditch guess, when nothing else says anything.
pub const SYSTEM: &str = "/usr/share/solium";

/// Every place the shipped files could be, nearest first, for a program
/// built with `baked` as its datadir in `build_tree`, and running as `exe`.
///
/// Handed its inputs rather than reading them, so the order can be tested
/// against directories a test made.
pub fn candidates(baked: Option<&str>, build_tree: &Path, exe: Option<&Path>) -> Vec<PathBuf> {
    let mut places = Vec::with_capacity(4);
    places.extend(baked.map(PathBuf::from));
    places.push(build_tree.to_path_buf());
    // `current_exe` resolves symlinks on Linux, so a `/usr/local/bin`
    // symlink points at the real installation rather than at itself.
    places.extend(
        exe.and_then(Path::parent)
            .and_then(Path::parent)
            .map(|prefix| prefix.join("share").join("solium")),
    );
    places.push(PathBuf::from(SYSTEM));
    places
}

/// The first candidate that `holds` what the program ships, and whether
/// one did.
///
/// `holds` asks whether the files are there, not whether the directory is:
/// an empty `/usr/share/solium` left by an uninstall, or a build tree whose
/// files were moved, is a directory, and stopping there would shadow the
/// candidate that works.
///
/// When none holds them this still returns a path, the first candidate,
/// which is the place the build meant: a packaged binary names its baked
/// datadir and a development one its build tree. The errors that follow
/// then name a file someone can go and look for.
pub fn choose(candidates: &[PathBuf], holds: impl Fn(&Path) -> bool) -> (PathBuf, bool) {
    if let Some(found) = candidates.iter().find(|path| holds(path)) {
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

/// Where the user's own configuration lives, whether or not there is any:
/// `solium` under `XDG_CONFIG_HOME`, or under `~/.config`.
pub fn user_config_dir() -> Option<PathBuf> {
    user_config_dir_from(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

/// [`user_config_dir`] from the two variables it reads. An empty
/// `XDG_CONFIG_HOME` is unset, as the base directory specification says.
pub fn user_config_dir_from(xdg: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    xdg.filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|home| PathBuf::from(home).join(".config")))
        .map(|base| base.join("solium"))
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "a test")]
mod tests {
    use super::*;

    /// A directory under a name no other test uses: `cargo test` runs these
    /// on several threads, and two tests sharing one delete each other's.
    fn dir(name: &str, with: &[&str]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("solium-paths-{name}"));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("a directory");
        for part in with {
            std::fs::create_dir_all(path.join(part)).expect("a part");
        }
        path
    }

    fn holds_qml(root: &Path) -> bool {
        root.join("qml").is_dir()
    }

    #[test]
    fn the_order_is_baked_then_build_tree_then_prefix_then_system() {
        let exe = Path::new("/opt/solium/bin/solium");
        assert_eq!(
            candidates(Some("/baked"), Path::new("/tree"), Some(exe)),
            [
                PathBuf::from("/baked"),
                PathBuf::from("/tree"),
                PathBuf::from("/opt/solium/share/solium"),
                PathBuf::from(SYSTEM),
            ]
        );
        assert_eq!(
            candidates(None, Path::new("/tree"), None),
            [PathBuf::from("/tree"), PathBuf::from(SYSTEM)],
            "an unbaked build has no datadir candidate"
        );
    }

    /// The window manager is installed in `libexec`, the compositor in `bin`:
    /// both find the same `share/solium`.
    #[test]
    fn a_helper_in_libexec_finds_the_same_share_as_the_binary_in_bin() {
        let from_bin = candidates(None, Path::new("/tree"), Some(Path::new("/usr/bin/solium")));
        let from_libexec = candidates(
            None,
            Path::new("/tree"),
            Some(Path::new("/usr/libexec/solium-lua")),
        );
        assert_eq!(from_bin[1], PathBuf::from("/usr/share/solium"));
        assert_eq!(from_bin, from_libexec);
    }

    #[test]
    fn the_first_place_that_holds_the_files_wins() {
        let tree = dir("tree", &["qml"]);
        let baked = dir("baked", &["qml"]);
        let places = candidates(Some(&baked.to_string_lossy()), &tree, None);
        assert_eq!(choose(&places, holds_qml), (baked, true));
    }

    #[test]
    fn a_place_that_is_there_and_holds_nothing_does_not_count() {
        let tree = dir("full", &["qml"]);
        let leftover = dir("leftover", &[]);
        let places = candidates(Some(&leftover.to_string_lossy()), &tree, None);
        assert_eq!(choose(&places, holds_qml), (tree, true));
    }

    #[test]
    fn an_installed_binary_finds_the_prefix_beside_it() {
        let prefix = dir("prefix", &["share/solium/qml", "bin"]);
        let exe = prefix.join("bin").join("solium");
        let places = candidates(None, Path::new("/no/such/tree"), Some(&exe));
        assert_eq!(
            choose(&places, holds_qml),
            (prefix.join("share").join("solium"), true)
        );
    }

    #[test]
    fn nothing_found_still_names_the_place_the_build_meant() {
        let places = candidates(Some("/no/such/baked"), Path::new("/no/such/tree"), None);
        assert_eq!(
            choose(&places, holds_qml),
            (PathBuf::from("/no/such/baked"), false)
        );
        let places = candidates(None, Path::new("/no/such/tree"), None);
        assert_eq!(
            choose(&places, holds_qml),
            (PathBuf::from("/no/such/tree"), false)
        );
    }

    #[test]
    fn the_users_directory_is_under_xdg_config_home_or_home() {
        let read = |xdg: Option<&str>, home: Option<&str>| {
            user_config_dir_from(xdg.map(OsString::from), home.map(OsString::from))
        };
        assert_eq!(
            read(Some("/x"), Some("/home/a")),
            Some(PathBuf::from("/x/solium"))
        );
        assert_eq!(
            read(None, Some("/home/a")),
            Some(PathBuf::from("/home/a/.config/solium"))
        );
        assert_eq!(
            read(Some(""), Some("/home/a")),
            Some(PathBuf::from("/home/a/.config/solium")),
            "an empty XDG_CONFIG_HOME is unset"
        );
        assert_eq!(read(None, None), None);
    }
}
