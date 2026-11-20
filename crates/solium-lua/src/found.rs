//! Where this window manager's files are, found by itself, as any window
//! manager finds its own: the compositor names none of them.
//!
//! - The configuration it loads, and loads again at every reload:
//!   `SOLIUM_LUA_INIT` when that is set, else the user's own `init.lua`, else
//!   the shipped one. The user's file wins, and the shipped one is the
//!   fallback rather than a default that has to be copied before anything
//!   works.
//! - The shipped Lua, `lua/` under the directory every program of Solium's
//!   finds its shipped files in (`solium_paths`).
//! - The user's own directory, `~/.config/solium`, searched first by
//!   `require`.
//!
//! Started safe (`hello.safe`: in place of a window manager that kept
//! stopping), it reads none of the user's own: the shipped configuration,
//! and no user directory.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The datadir a packager baked in, as the compositor bakes the same one.
const BAKED: Option<&str> = option_env!("SOLIUM_DATADIR");

/// The tree this was compiled in, which holds `lua/` while it is there.
const BUILD_TREE: &str = env!("CARGO_MANIFEST_DIR");

/// What the window manager reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Files {
    /// The configuration it loads, and loads again at a reload.
    pub config: PathBuf,
    /// The shipped Lua.
    pub shipped: PathBuf,
    /// The user's own Lua, searched first; none when started safe.
    pub user: Option<PathBuf>,
}

impl Files {
    /// The files on this machine, for a window manager started `safe` or
    /// not.
    pub fn find(safe: bool) -> Self {
        Self::found(
            safe,
            shipped().to_path_buf(),
            solium_paths::user_config_dir(),
            std::env::var_os("SOLIUM_LUA_INIT").map(PathBuf::from),
        )
    }

    /// [`Self::find`] from what it reads.
    fn found(safe: bool, shipped: PathBuf, user: Option<PathBuf>, init: Option<PathBuf>) -> Self {
        if safe {
            return Self {
                config: shipped.join("init.lua"),
                shipped,
                user: None,
            };
        }
        let config = init
            .or_else(|| {
                user.as_deref()
                    .map(|user| user.join("init.lua"))
                    .filter(|path| path.is_file())
            })
            .unwrap_or_else(|| shipped.join("init.lua"));
        Self {
            config,
            shipped,
            user,
        }
    }
}

/// The shipped Lua: `init.lua` and every module it pulls in. Found once, and
/// said once, loudly, when it is nowhere: a window manager with no
/// configuration has no key that opens a terminal.
pub fn shipped() -> &'static Path {
    static SHIPPED: OnceLock<PathBuf> = OnceLock::new();
    SHIPPED.get_or_init(|| {
        let places = solium_paths::candidates(
            BAKED,
            Path::new(BUILD_TREE),
            std::env::current_exe().ok().as_deref(),
        );
        let (root, found) = solium_paths::choose(&places, holds_lua);
        if !found {
            tracing::error!(
                looked_in = ?places,
                using = %root.display(),
                "the shipped Lua is not installed anywhere this build can see: install \
                 lua/ under one of these, or rebuild with SOLIUM_DATADIR set to where it is"
            );
        }
        root.join("lua")
    })
}

/// Whether `root` holds the shipped Lua.
fn holds_lua(root: &Path) -> bool {
    root.join("lua").join("init.lua").is_file()
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "a test")]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("solium-lua-found-{name}"));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("a directory");
        path
    }

    #[test]
    fn the_users_init_lua_wins_over_the_shipped_one() {
        let user = dir("user");
        let shipped = PathBuf::from("/shipped/lua");
        let none = Files::found(false, shipped.clone(), Some(user.clone()), None);
        assert_eq!(
            none.config,
            shipped.join("init.lua"),
            "the shipped one, while the user has none"
        );
        std::fs::write(user.join("init.lua"), "").expect("written");
        let theirs = Files::found(false, shipped.clone(), Some(user.clone()), None);
        assert_eq!(theirs.config, user.join("init.lua"));
        assert_eq!(theirs.user, Some(user));
    }

    #[test]
    fn solium_lua_init_wins_over_both() {
        let user = dir("user-and-init");
        std::fs::write(user.join("init.lua"), "").expect("written");
        let files = Files::found(
            false,
            PathBuf::from("/shipped/lua"),
            Some(user),
            Some(PathBuf::from("/elsewhere/mine.lua")),
        );
        assert_eq!(files.config, PathBuf::from("/elsewhere/mine.lua"));
    }

    /// Started in place of one that kept stopping: nothing of the user's,
    /// whatever they have and whatever `SOLIUM_LUA_INIT` says.
    #[test]
    fn started_safe_it_reads_none_of_the_users_own() {
        let user = dir("user-safe");
        std::fs::write(user.join("init.lua"), "").expect("written");
        let shipped = PathBuf::from("/shipped/lua");
        let files = Files::found(
            true,
            shipped.clone(),
            Some(user),
            Some(PathBuf::from("/elsewhere/mine.lua")),
        );
        assert_eq!(
            files,
            Files {
                config: shipped.join("init.lua"),
                shipped,
                user: None,
            }
        );
    }

    /// This build finds the Lua it ships, in its own crate.
    #[test]
    fn this_build_finds_its_shipped_lua() {
        if BAKED.is_none() {
            assert_eq!(shipped(), Path::new(BUILD_TREE).join("lua"));
        }
        assert!(shipped().join("init.lua").is_file());
    }
}
