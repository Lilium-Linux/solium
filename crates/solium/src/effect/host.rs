//! The effect host: where effect folders are, what was loaded from them, and
//! what went wrong, by file and line.
//!
//! An effect is `effects/<name>/effect.lua` and the files beside it (\[16\] §2).
//! The user's folder shadows the shipped one name by name, as a pane style's
//! does (`style::resolve`): `tests::a_user_folder_shadows_the_shipped_one`.

use std::path::{Path, PathBuf};

use solium_effects::spec::{EffectSpec, Severity, Value};

use super::sandbox::Sandbox;

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

/// Something wrong with an effect, where it is.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Problem {
    /// The effect's name, or `"config"` for the configuration itself.
    pub(crate) effect: String,
    pub(crate) file: PathBuf,
    pub(crate) line: Option<u32>,
    pub(crate) column: Option<u32>,
    pub(crate) message: String,
    pub(crate) severity: Severity,
}

impl Problem {
    pub(crate) fn error(effect: &str, file: &Path, line: Option<u32>, message: String) -> Self {
        Self {
            effect: effect.to_owned(),
            file: file.to_owned(),
            line,
            column: None,
            message,
            severity: Severity::Error,
        }
    }

    pub(crate) fn warning(effect: &str, file: &Path, message: String) -> Self {
        Self {
            severity: Severity::Warning,
            ..Self::error(effect, file, None, message)
        }
    }
}

/// One effect, loaded: its folder, what its `effect.lua` said, its Lua.
/// `P` is the program type Task 4 compiles into.
#[derive(Debug)]
pub(crate) struct Loaded<P = ()> {
    name: String,
    dir: PathBuf,
    spec: EffectSpec,
    sandbox: Sandbox,
    hash: u64,
    /// The programs this version needs, by content hash (Task 4).
    pub(crate) needs: Vec<u64>,
    _program: std::marker::PhantomData<P>,
}

/// An effect bound with a rule's overrides.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Bound {
    pub(crate) params: Vec<(String, Value)>,
    pub(crate) reach: f64,
    pub(crate) bleed: f64,
    pub(crate) warnings: Vec<Problem>,
}

impl<P> Loaded<P> {
    /// Load the effect in `dir` into a sandbox of its own.
    /// `tests::the_fixture_effects_load_and_bind_at_their_defaults`.
    pub(crate) fn load(name: &str, dir: &Path) -> Result<Self, Problem> {
        let file = dir.join("effect.lua");
        let mut sandbox = Sandbox::new(name, &file)?;
        let spec = sandbox.load_effect()?;
        Ok(Self {
            name: name.to_owned(),
            dir: dir.to_owned(),
            spec,
            sandbox,
            hash: folder_hash(dir),
            needs: Vec::new(),
            _program: std::marker::PhantomData,
        })
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    pub(crate) fn spec(&self) -> &EffectSpec {
        &self.spec
    }

    pub(crate) fn sandbox(&self) -> &Sandbox {
        &self.sandbox
    }

    pub(crate) fn hash(&self) -> u64 {
        self.hash
    }

    /// Bind with `overrides`: refused for an unknown param or a wrong kind,
    /// clamped with a warning out of range, then `reach` and `bleed` for them.
    /// `tests::the_fixture_effects_load_and_bind_at_their_defaults`,
    /// `tests::binding_names_the_effect_and_warns_of_a_clamp`.
    pub(crate) fn bind(&self, overrides: &[(String, Value)]) -> Result<Bound, Problem> {
        let file = self.dir.join("effect.lua");
        let (params, warnings) = solium_effects::spec::bind(&self.spec.params, overrides)
            .map_err(|message| Problem::error(&self.name, &file, None, message))?;
        let reach = self.sandbox.extent("reach", &params)?;
        let bleed = self.sandbox.extent("bleed", &params)?;
        let warnings = warnings
            .into_iter()
            .map(|message| Problem::warning(&self.name, &file, message))
            .collect();
        Ok(Bound {
            params,
            reach,
            bleed,
            warnings,
        })
    }
}

/// A folder's identity: every file in it, sorted by name, bytes and all
/// (FNV-1a, 64 bits). `tests::a_folders_hash_is_its_files`.
pub(crate) fn folder_hash(dir: &Path) -> u64 {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.is_file())
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for file in files {
        let name = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        for byte in name
            .bytes()
            .chain([0xff])
            .chain(std::fs::read(&file).unwrap_or_default())
            .chain([0xfe])
        {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
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

    /// **The fixture folders load**, each into a sandbox of its own, and
    /// their defaults bind.
    #[test]
    fn the_fixture_effects_load_and_bind_at_their_defaults() {
        let fixtures = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/effects"
        ));
        for name in ["identity", "kawase", "tint", "frost", "three"] {
            let dir = fixtures.join(name);
            let loaded = super::Loaded::<u32>::load(name, &dir)
                .unwrap_or_else(|problem| panic!("{name}: {problem:?}"));
            let bound = loaded
                .bind(&[])
                .unwrap_or_else(|problem| panic!("{name} at its defaults: {problem:?}"));
            assert_eq!(
                (loaded.name(), loaded.dir(), loaded.hash()),
                (name, dir.as_path(), super::folder_hash(&dir))
            );
            assert_eq!(loaded.spec().api, 1, "{name}");
            assert!(!loaded.sandbox().poisoned() && loaded.needs.is_empty());
            if matches!(name, "kawase" | "frost") {
                // kawase's is a function of its defaults, 3 * 2^(3 + 1);
                // frost's a number.
                assert!(
                    (bound.reach - 48.0).abs() < f64::EPSILON,
                    "{name}: {bound:?}"
                );
            }
        }
    }

    /// Binding through a loaded effect names it: a refused override is an
    /// error at its `effect.lua`, and a clamp a warning there.
    #[test]
    fn binding_names_the_effect_and_warns_of_a_clamp() {
        use solium_effects::spec::{Severity, Value};
        let place = scratch("bind");
        let lua = "return { api = 1, frag = 'effect.frag', params = { amount = { 0.5, min = 0, max = 1 } } }";
        let dir = folder(&place, "soft", lua, &[("effect.frag", "x")]);
        let loaded = super::Loaded::<()>::load("soft", &dir).expect("loads");
        let bound = loaded
            .bind(&[("amount".to_owned(), Value::Number(2.0))])
            .expect("clamped");
        assert_eq!(
            bound.params,
            vec![("amount".to_owned(), Value::Number(1.0))]
        );
        assert_eq!(bound.warnings.len(), 1, "{bound:?}");
        let warning = &bound.warnings[0];
        assert_eq!(
            (warning.effect.as_str(), warning.severity, &warning.file),
            ("soft", Severity::Warning, &dir.join("effect.lua"))
        );
        let refused = loaded
            .bind(&[("amout".to_owned(), Value::Number(0.2))])
            .expect_err("a typo");
        assert_eq!(
            (refused.effect.as_str(), refused.severity),
            ("soft", Severity::Error)
        );
        assert!(refused.message.contains("`amount`"), "{refused:?}");
        let _ = std::fs::remove_dir_all(place);
    }

    /// Two folders with the same files have the same hash, and one changed
    /// byte gives another: what Task 4's "unchanged is not reloaded" rests on.
    #[test]
    fn a_folders_hash_is_its_files() {
        let place = scratch("hash");
        let lua = "return { api = 1, frag = 'effect.frag' }";
        let a = folder(&place, "a", lua, &[("effect.frag", "x")]);
        let b = folder(&place, "b", lua, &[("effect.frag", "x")]);
        let c = folder(&place, "c", lua, &[("effect.frag", "y")]);
        let hash = |dir: &Path| super::folder_hash(dir);
        assert_eq!(hash(&a), hash(&b));
        assert_ne!(hash(&a), hash(&c));
        let _ = std::fs::remove_dir_all(place);
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
