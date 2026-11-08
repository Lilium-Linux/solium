//! The compositor's side of the window manager protocol
//! (`crates/wm-protocol`): what a window manager is told, converted from the
//! compositor's own types, and only as much of it as changed.

pub(crate) mod convert;
pub(crate) mod diff;
