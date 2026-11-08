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
    /// The last batch's serial.
    pub(crate) serial: u64,
    /// What the configuration answered its load with: the settings and
    /// requests its top level made, which [`Scripts::startup`] hands over
    /// and the readers of the load (`qml`, `session`, the effect rules)
    /// read.
    pub(crate) loaded: solium_wm_protocol::Answer,
    /// Whether [`Scripts::startup`] has handed `loaded` over.
    pub(crate) started: bool,
    /// What the window manager last declared: its bindings, which the key
    /// filter asks about on every key, and what `--check` reports.
    pub(crate) declared: solium_wm_protocol::Declarations,
}
