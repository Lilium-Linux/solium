//! `solium-auth`: checking a password for Solium's own lock screen (#281).
//!
//! The compositor never runs PAM itself. For each check it starts the
//! `solium-auth` helper, a small binary with no Qt, no Wayland and no DRM,
//! from a descriptor it opened at startup, so a package upgrade cannot swap it
//! under a running session. The helper makes itself non-dumpable, runs one PAM
//! transaction against `/etc/pam.d/solium`, answers yes or no, and ends; the
//! compositor unlocks only on its yes.
//!
//! The lock can also take a PIN in place of the password, while it allows
//! one: the helper checks it too, against the argon2id hash `solium pin set`
//! stored ([`pin`]), so neither the PIN nor its check is ever in the
//! compositor.
//!
//! This library is what the two sides share: the [`protocol`] they speak over
//! a socketpair, the [`Secret`] a password or a PIN is held in on both sides,
//! and the PIN file ([`pin`]). The helper's own code, PAM included, is the
//! binary's (`src/main.rs`, `src/pam.rs`); the compositor's side is Solium's
//! `auth.rs`.

// A test fails by panicking -- that is the mechanism, not a lapse.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod pin;
pub mod protocol;
mod secret;

pub use protocol::{Answer, Authenticator, Request};
pub use secret::{MAX_LEN, Secret};
