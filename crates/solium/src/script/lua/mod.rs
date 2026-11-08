//! The Lua binding of the script API: everything in `script/` that knows Lua.

mod api;
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
pub(crate) use motion::*;
pub(crate) use pending::*;
pub(crate) use runs::*;

/// The Lua runtime and the scripts loaded into it.
#[derive(Debug)]
pub(crate) struct Scripts {
    pub(crate) lua: mlua::Lua,
    /// What the scripts were last told of the desktop, so each dispatch
    /// tells them only what changed.
    pub(crate) told: crate::wm::diff::Told,
    /// The desktop as the scripts know it: what `sol.windows()` and the
    /// other reads answer from.
    pub(crate) mirror: solium_wm_protocol::mirror::Mirror,
}
