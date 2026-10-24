//! `solium-auth`: checking a password for Solium's own lock screen (#281).
//!
//! The compositor never runs PAM itself. For each check it starts the
//! `solium-auth` helper, a small binary with no Qt, no Wayland and no DRM,
//! from a descriptor it opened at startup, so a package upgrade cannot swap it
//! under a running session. The helper makes itself non-dumpable, runs one PAM
//! transaction against `/etc/pam.d/solium`, answers yes or no, and ends; the
//! compositor unlocks only on its yes.
//!
//! This library is what the two sides share: the [`protocol`] they speak over
//! a socketpair, and the [`Secret`] a password is held in on both sides. The
//! helper's own code, PAM included, is the binary's (`src/main.rs`,
//! `src/pam.rs`); the compositor's side is Solium's `auth.rs`.

// A test fails by panicking -- that is the mechanism, not a lapse.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod protocol;
mod secret;

pub use protocol::{Answer, Authenticator};
pub use secret::{MAX_LEN, Secret};
