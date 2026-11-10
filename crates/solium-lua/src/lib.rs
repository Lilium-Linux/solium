//! Solium's Lua window manager: the `sol` API, the Lua runtime that runs a
//! configuration, and what it answers the compositor with, in the window
//! manager protocol's terms (`solium-wm-protocol`).
//!
//! Nothing here knows the compositor: [`Host`] is handed a batch of what
//! changed and what happened, and answers. The compositor links it while it
//! runs the window manager in its own process.

#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod api;
mod bindings;
pub mod convert;
mod deadline;
pub mod dispatch;
mod files;
mod keep;
mod layouts;
mod load;
mod motion;
mod pending;
mod runs;
mod serve;
pub mod to_wire;

pub use api::*;
pub use bindings::*;
pub use convert::describe_value;
pub use deadline::*;
pub use files::*;
pub use keep::*;
pub use layouts::*;
pub use load::{config_path, keeps_of, stores_of, user_config_dir};
pub use motion::*;
pub use pending::*;
pub use runs::*;
pub use serve::{Wm, compatible, serve};

pub use solium_animation::AnimationSpec;
pub use solium_wm_protocol::combo::normalise as normalise_combo;
pub use solium_wm_protocol::present::MOTIONS as PRESENT_MOTIONS;
pub use solium_wm_protocol::{Binding, UnknownSetting};

/// The window manager the compositor ships: the Lua runtime and the
/// configuration loaded into it, answering the compositor's batches in the
/// protocol's terms.
#[derive(Debug)]
pub struct Host {
    pub lua: mlua::Lua,
    /// What it was started with, which a reload starts with again.
    pub setup: Setup,
    /// The desktop as the scripts know it: what `sol.windows()` and the
    /// other reads answer from.
    pub mirror: solium_wm_protocol::mirror::Mirror,
}

/// What the Lua host is started with: where the shipped Lua is, which a
/// configuration's `require` finds and which is the configuration when the
/// user has none; where the user's own is, which `require` finds first; and
/// whether the session runs in debug mode, as `hello` says.
#[derive(Clone, Debug)]
pub struct Setup {
    pub shipped: std::path::PathBuf,
    /// None when only what ships is to run: the compositor's way back from
    /// a configuration that keeps stopping.
    pub user: Option<std::path::PathBuf>,
    pub debug: bool,
}

/// Whether a program can be found on `PATH`.
///
/// A plain lookup rather than a spawn: asking costs nothing, and finding out by
/// running it means finding out *after* something has already failed. A desktop
/// cannot assume any particular terminal or editor is installed, and a script
/// that picks the first one that *is* beats one that silently does nothing on a
/// machine set up differently from the author's.
pub fn which(program: &str) -> bool {
    if program.contains('/') {
        return std::path::Path::new(program).is_file();
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| directory.join(program).is_file())
}
