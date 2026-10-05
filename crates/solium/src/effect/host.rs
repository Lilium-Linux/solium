//! The effect host: where effect folders are, what was loaded from them, and
//! what went wrong, by file and line.
//!
//! An effect is `effects/<name>/effect.lua` and the files beside it (\[16\] §2).
//! The user's folder shadows the shipped one name by name, as a pane style's
//! does (`style::resolve`): `tests::a_user_folder_shadows_the_shipped_one`.

use std::path::PathBuf;

/// Where effect folders are looked for: the user's, then the shipped.
#[derive(Clone, Debug)]
pub(crate) struct Library {
    user: Option<PathBuf>,
    shipped: PathBuf,
}

impl Library {
    /// This machine's: `~/.config/solium/effects/`, then the build's or the
    /// install's (`assets::effects`).
    #[expect(
        dead_code,
        reason = "Task 4's host is its first caller; the tests build theirs with `with`"
    )]
    pub(crate) fn new() -> Self {
        Self::with(
            crate::script::Scripts::user_config_dir().map(|dir| dir.join("effects")),
            crate::assets::effects(),
        )
    }

    pub(crate) fn with(user: Option<PathBuf>, shipped: PathBuf) -> Self {
        Self { user, shipped }
    }

    /// The folder the effect called `name` is in, or `None`.
    ///
    /// A bare name only (`tests::an_effect_name_is_a_bare_name`), and a folder
    /// with no `effect.lua` is skipped so the shipped one still draws
    /// (`tests::a_folder_without_effect_lua_falls_through`).
    pub(crate) fn resolve(&self, name: &str) -> Option<PathBuf> {
        if !is_name(name) {
            return None;
        }
        self.user
            .iter()
            .chain(std::iter::once(&self.shipped))
            .map(|place| place.join(name))
            .find(|folder| folder.join("effect.lua").is_file())
    }

    /// Every folder in the user's place, sorted: what `--check` reads.
    /// `tests::the_users_folders_are_listed_sorted`.
    pub(crate) fn user_folders(&self) -> Vec<PathBuf> {
        let Some(user) = &self.user else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(user) else {
            return Vec::new();
        };
        let mut folders: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        folders.sort();
        folders
    }
}

/// Whether `name` can name an effect: lower-case letters, digits, `-` and `_`.
/// `tests::an_effect_name_is_a_bare_name`.
pub(crate) fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::{Path, PathBuf};

    use super::Library;

    /// A temporary directory of this test's own, emptied first, as
    /// `style::tests::scratch` makes them.
    pub(crate) fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("solium-effect-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temporary directory");
        dir
    }

    /// An effect folder `name` in `place`: its `effect.lua` and any other files.
    pub(crate) fn folder(
        place: &Path,
        name: &str,
        effect_lua: &str,
        files: &[(&str, &str)],
    ) -> PathBuf {
        let dir = place.join(name);
        std::fs::create_dir_all(&dir).expect("an effect folder");
        std::fs::write(dir.join("effect.lua"), effect_lua).expect("writing effect.lua");
        for (file, text) in files {
            std::fs::write(dir.join(file), text).expect("writing an effect's file");
        }
        dir
    }

    /// **A user's folder shadows the shipped one of the same name** ([16] §2),
    /// name by name, as a pane style's does.
    #[test]
    fn a_user_folder_shadows_the_shipped_one() {
        let (user, shipped) = (scratch("user-shadow"), scratch("shipped-shadow"));
        folder(&shipped, "blur", "return { api = 1 }", &[]);
        let mine = folder(&user, "blur", "return { api = 1 }", &[]);
        let library = Library::with(Some(user.clone()), shipped.clone());
        assert_eq!(library.resolve("blur"), Some(mine));
        let _ = (
            std::fs::remove_dir_all(&user),
            std::fs::remove_dir_all(&shipped),
        );
    }

    /// **A folder with no `effect.lua` falls through** to the shipped one: an
    /// empty `~/.config/solium/effects/blur/` made and not yet filled does not
    /// cost the blur, as an empty style folder does not cost the frame
    /// (`style::tests::a_bare_name_needs_a_manifest_and_not_merely_a_folder`).
    #[test]
    fn a_folder_without_effect_lua_falls_through() {
        let (user, shipped) = (scratch("user-empty"), scratch("shipped-empty"));
        let theirs = folder(&shipped, "blur", "return { api = 1 }", &[]);
        std::fs::create_dir_all(user.join("blur")).expect("an empty folder");
        let library = Library::with(Some(user.clone()), shipped.clone());
        assert_eq!(library.resolve("blur"), Some(theirs));
        let _ = (
            std::fs::remove_dir_all(&user),
            std::fs::remove_dir_all(&shipped),
        );
    }

    /// **An effect's name is a bare name**: it is written in rules, in `use`
    /// and in `fallback`, where a path means nothing, so `""`, `"a/b"` and
    /// `".."` resolve to nothing even when such a directory exists. Each one
    /// here would find an `effect.lua` if it were joined on as a path.
    #[test]
    fn an_effect_name_is_a_bare_name() {
        let root = scratch("names");
        let (user, shipped) = (root.join("user"), root.join("shipped"));
        folder(&shipped, "ok-name_2", "return { api = 1 }", &[]);
        let elsewhere = folder(&root, "elsewhere", "return { api = 1 }", &[]);
        folder(&user.join("a"), "b", "return { api = 1 }", &[]);
        folder(&user, "Blur", "return { api = 1 }", &[]);
        folder(&user, "blur ", "return { api = 1 }", &[]);
        std::fs::write(user.join("effect.lua"), "return { api = 1 }").expect("\"\" as a path");
        std::fs::write(root.join("effect.lua"), "return { api = 1 }").expect("\"..\" as a path");
        let library = Library::with(Some(user.clone()), shipped.clone());
        let absolute = elsewhere.display().to_string();
        for bad in ["", "a/b", "..", "Blur", "blur ", "/tmp", absolute.as_str()] {
            assert_eq!(library.resolve(bad), None, "{bad:?} is not a name");
        }
        assert!(library.resolve("ok-name_2").is_some());
        assert_eq!(library.resolve("nowhere"), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `--check` reads every folder in the user's place, sorted, whether or
    /// not it has an `effect.lua` (that it lacks one is what it reports).
    #[test]
    fn the_users_folders_are_listed_sorted() {
        let (user, shipped) = (scratch("user-list"), scratch("shipped-list"));
        // Made in an order that is neither sorted nor sorted backwards, so a
        // directory listed in the order it was written fails.
        folder(&user, "fade", "return { api = 1 }", &[]);
        folder(&user, "zoom", "return { api = 1 }", &[]);
        std::fs::create_dir_all(user.join("blur")).expect("a folder");
        std::fs::create_dir_all(user.join("kawase")).expect("a folder");
        std::fs::write(user.join("notes.txt"), "not a folder").expect("a file");
        let library = Library::with(Some(user.clone()), shipped.clone());
        let names = ["blur", "fade", "kawase", "zoom"];
        assert_eq!(
            library.user_folders(),
            names.map(|name| user.join(name)).to_vec()
        );
        assert!(
            Library::with(None, shipped.clone())
                .user_folders()
                .is_empty()
        );
        let _ = (
            std::fs::remove_dir_all(&user),
            std::fs::remove_dir_all(&shipped),
        );
    }

    /// The shipped folders sit beside the shipped QML and Lua, wherever those
    /// were found, so an install that has one has the others.
    #[test]
    fn the_shipped_effects_are_beside_the_qml_and_the_lua() {
        assert_eq!(
            crate::assets::effects().parent(),
            crate::assets::qml().parent()
        );
        assert!(crate::assets::effects().ends_with("effects"));
        assert!(
            crate::assets::effects().join("README.md").is_file(),
            "the reference ships with them"
        );
    }
}
