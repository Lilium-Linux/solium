//! The Lua binding of the script API: everything in `script/` that knows Lua.

pub(crate) mod api;
mod bindings;
pub(crate) mod convert;
mod deadline;
pub(crate) mod dispatch;
mod files;
mod keep;
mod layouts;
mod load;
mod motion;
mod pending;
mod runs;
pub(crate) mod to_wire;

pub(crate) use api::*;
pub(crate) use bindings::*;
pub(crate) use convert::describe_value;
pub(crate) use deadline::*;
pub(crate) use files::*;
pub(crate) use keep::*;
pub(crate) use layouts::*;
pub(crate) use load::{config_path, stores_of, user_config_dir};
pub(crate) use motion::*;
pub(crate) use pending::*;
pub(crate) use runs::*;

pub(crate) use solium_animation::AnimationSpec;
pub(crate) use solium_wm_protocol::combo::normalise as normalise_combo;
pub(crate) use solium_wm_protocol::present::MOTIONS as PRESENT_MOTIONS;
pub(crate) use solium_wm_protocol::{Binding, UnknownSetting};

/// The window manager the compositor ships: the Lua runtime and the
/// configuration loaded into it, answering the compositor's batches in the
/// protocol's terms.
#[derive(Debug)]
pub(crate) struct Host {
    pub(crate) lua: mlua::Lua,
    /// What it was started with, which a reload starts with again.
    pub(crate) setup: Setup,
    /// The desktop as the scripts know it: what `sol.windows()` and the
    /// other reads answer from.
    pub(crate) mirror: solium_wm_protocol::mirror::Mirror,
}

/// What the Lua host is started with: where the shipped Lua is, which a
/// configuration's `require` finds and which is the configuration when the
/// user has none, and whether the session runs in debug mode, as `hello`
/// says.
#[derive(Clone, Debug)]
pub(crate) struct Setup {
    pub(crate) shipped: std::path::PathBuf,
    pub(crate) debug: bool,
}

/// Whether a program can be found on `PATH`.
///
/// A plain lookup rather than a spawn: asking costs nothing, and finding out by
/// running it means finding out *after* something has already failed. A desktop
/// cannot assume any particular terminal or editor is installed, and a script
/// that picks the first one that *is* beats one that silently does nothing on a
/// machine set up differently from the author's.
pub(crate) fn which(program: &str) -> bool {
    if program.contains('/') {
        return std::path::Path::new(program).is_file();
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| directory.join(program).is_file())
}
