//! The compositor's side of the window manager protocol
//! (`crates/wm-protocol`): what a window manager is told, converted from the
//! compositor's own types, only as much of it as changed; and what it asks
//! for, converted to the compositor's commands.

pub(crate) mod convert;
pub(crate) mod diff;
pub(crate) mod events;
pub(crate) mod link;
pub(crate) mod rows;
mod scripts;
pub(crate) mod stores;

pub(crate) use scripts::Scripts;
