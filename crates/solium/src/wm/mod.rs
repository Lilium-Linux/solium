//! The compositor's side of the window manager protocol
//! (`crates/wm-protocol`): what a window manager is told, converted from the
//! compositor's own types, only as much of it as changed; and what it asks
//! for, converted to the compositor's commands.

pub(crate) mod convert;
#[cfg(test)]
pub(crate) mod diff;
#[cfg(test)]
pub(crate) mod rows;
