//! Solium — the Wayland compositor of Lilium DE.
//!
//! See `docs/architecture.md`. Layout and presentation are deliberately not in
//! the protocol handlers; every mode is a transform over window textures.

// A test fails by panicking — that is the mechanism, not a lapse. The workspace
// denies panics because a compositor crash takes the session down with it, and
// that reasoning does not apply to a test binary.
#![cfg_attr(
    test,
    allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]

mod capture;
mod decoration;
mod dev;
mod input;
mod present;
mod qml;
mod render;
mod script;
mod shell;
mod state;
mod winit;

use std::io::IsTerminal;

use anyhow::Result;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        // Escape codes in a redirected log break anything grepping it.
        .with_ansi(std::io::stderr().is_terminal())
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting solium");
    winit::run()
}
