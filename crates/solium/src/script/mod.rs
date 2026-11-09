//! The Lua scripting surface.
//!
//! Modes — overview, the app switcher, peek, the icon→window genie — are
//! scripts over the presentation transform, not features with their own
//! renderers. This module is the whole boundary between them and the
//! compositor, and it is deliberately narrow: a script enumerates windows,
//! sets targets, and binds triggers. **Nothing here mentions Wayland.** A mode
//! that had to know a window was a `wl_surface` would be a mode that could not
//! be written by anyone but us.
//!
//! ## How a call works
//!
//! Scripts never hold compositor state. Each dispatch is:
//!
//! 1. Rust builds a [`Snapshot`] — windows, work area, cursor — and hands it to
//!    Lua as app data.
//! 2. The handler runs. Reads come from the snapshot; writes are pushed onto a
//!    queue as [`Command`]s.
//! 3. Rust drains the queue and applies it.
//!
//! Which is the "commands are not state" rule (see CONTRIBUTING.md) made
//! structural: a script cannot mutate the compositor directly, so there is no
//! way for its idea of a window's geometry to drift from the compositor's. It
//! also means no borrow of `Solium` is alive while Lua runs, which is what
//! stops a script calling back into the compositor mid-dispatch and deadlocking
//! on the seat.

mod command;
pub(crate) mod doc;
pub(crate) use solium_lua as lua;
pub(crate) mod settings;
mod types;

pub(crate) use command::*;
pub(crate) use doc::{Doc, Table as DocTable};
pub(crate) use types::*;

pub(crate) use crate::wm::Scripts;

pub(crate) use solium_wm_protocol::combo::normalise as normalise_combo;

// The generated reference pages check their sources against the code here.
// Declared right above the tests, never higher: two tests read this file's
// production code as everything before its first `#[cfg(test)]`.
#[cfg(test)]
use crate::present::Curve;
#[cfg(test)]
pub(crate) use crate::wm::convert::deform_for_test;
#[cfg(test)]
use lua::*;
#[cfg(test)]
use mlua::{Lua, Table, Value};
#[cfg(test)]
use std::time::Duration;
#[cfg(test)]
mod reference;
#[cfg(test)]
mod source;

/// The one test helper other modules' tests share.
/// `state::tests::real_client::reflow_on_close::hosted::an_unknown_action_is_warned_of_the_first_time_only`.
#[cfg(test)]
pub(crate) use tests::logged_while;

#[cfg(test)]
mod tests;
