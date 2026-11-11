//! Where a configuration is: the user's own directory, and the file the
//! shipped window manager is started on.

use std::path::{Path, PathBuf};

/// Where a user's own configuration lives, whether or not they have any:
/// `solium` under `XDG_CONFIG_HOME`, or under `~/.config`.
pub(crate) fn user_config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .map(|base| base.join("solium"))
}

/// The configuration the shipped window manager is started on:
/// `SOLIUM_LUA_INIT` when it is set, else the user's own `init.lua`, else
/// the one in `shipped`. The user's file wins, and the shipped one is the
/// fallback rather than a default that has to be copied before anything
/// works.
pub(crate) fn config_path(shipped: &Path) -> PathBuf {
    if let Some(path) = std::env::var_os("SOLIUM_LUA_INIT") {
        return PathBuf::from(path);
    }
    if let Some(user) = user_config_dir() {
        let path = user.join("init.lua");
        if path.is_file() {
            return path;
        }
    }
    shipped.join("init.lua")
}
