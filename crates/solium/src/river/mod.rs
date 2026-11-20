//! River's window management protocol, as a second way in for a window
//! manager: one written for river runs on Solium unchanged.
//!
//! `wm.toml` says `protocol = "river"`, and its `command` is started as an
//! ordinary Wayland client of the compositor's. It binds
//! `river_window_manager_v1` and `river_xkb_bindings_v1`
//! ([`dispatch::create_globals`]), and the [`Bridge`] at the far end of the
//! compositor's window manager socket speaks for it: everything above the
//! socket is the same for both protocols. `bridge.rs` says where river and
//! Solium differ.

mod bridge;
pub(crate) mod dispatch;
pub(crate) mod protocol;

pub(crate) use bridge::Bridge;
