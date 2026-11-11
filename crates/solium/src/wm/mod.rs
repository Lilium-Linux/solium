//! The compositor's side of the window manager protocol
//! (`crates/wm-protocol`): what a window manager is told, converted from the
//! compositor's own types, only as much of it as changed; and what it asks
//! for, converted to the compositor's commands.

pub(crate) mod chosen;
pub(crate) mod convert;
pub(crate) mod diff;
pub(crate) mod events;
pub(crate) mod link;
pub(crate) mod paths;
pub(crate) mod rows;
mod scripts;
mod settings;
pub(crate) mod stores;
mod timing;
mod wake;

pub(crate) use chosen::Chosen;
pub(crate) use scripts::{Keeps, Late, Scripts};
pub(crate) use settings::{Restarts, Settings, Start, Wait};
pub(crate) use wake::{Watch, before_frame, settle};
