//! Solium — the Wayland compositor of Lilium DE.
//!
//! See `docs/architecture.md`. Layout and presentation are deliberately not in
//! the protocol handlers; every mode is a transform over window textures.

mod capture;
mod decoration;
mod input;
mod mode;
mod present;
mod qml;
mod render;
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
