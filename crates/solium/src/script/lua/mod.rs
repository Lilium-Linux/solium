//! The Lua binding of the script API: everything in `script/` that knows Lua.

pub(crate) mod api;
mod bindings;
pub(crate) mod convert;
mod deadline;
pub(crate) mod dispatch;
mod files;
mod layouts;
mod load;
mod motion;
mod pending;
mod runs;
pub(crate) mod to_wire;

pub(crate) use api::*;
pub(crate) use bindings::*;
pub(crate) use deadline::*;
pub(crate) use files::*;
pub(crate) use layouts::*;
pub(crate) use load::{config_path, stores_of, user_config_dir};
pub(crate) use motion::*;
pub(crate) use pending::*;
pub(crate) use runs::*;

/// The window manager the compositor ships: the Lua runtime and the
/// configuration loaded into it, answering the compositor's batches in the
/// protocol's terms.
#[derive(Debug)]
pub(crate) struct Host {
    pub(crate) lua: mlua::Lua,
    /// The desktop as the scripts know it: what `sol.windows()` and the
    /// other reads answer from.
    pub(crate) mirror: solium_wm_protocol::mirror::Mirror,
}
