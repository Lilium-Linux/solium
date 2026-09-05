//! Solium — the Wayland compositor of Lilium DE.
//!
//! See `docs/architecture.md`. Layout and presentation are deliberately not in
//! the protocol handlers; every mode is a transform over window textures.

// A test fails by panicking — that is the mechanism, not a lapse. The workspace
// denies panics because a compositor crash takes the session down with it, and
// that reasoning does not apply to a test binary.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod capture;
mod cursor;
mod decoration;
mod dev;
mod input;
mod layer;
mod present;
mod qml;
mod render;
mod script;
mod state;
mod tty;
mod winit;

use std::{io::IsTerminal, path::PathBuf};

use anyhow::Result;
use tracing_subscriber::fmt::writer::MakeWriterExt as _;

/// Where a hardware session writes its log.
///
/// Returns `None` rather than failing: not being able to write a log is a
/// reason to run without one, never a reason not to start.
fn open_log() -> Option<std::sync::Arc<std::fs::File>> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    let directory = base.join("solium");
    std::fs::create_dir_all(&directory).ok()?;
    let path = directory.join("session.log");

    // Appended, so the log of the run that went wrong is still there after the
    // run that was meant to fix it.
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    eprintln!("logging to {}", path.display());
    Some(std::sync::Arc::new(file))
}

fn start_logging(log: Option<std::sync::Arc<std::fs::File>>) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    // Escape codes in a redirected log break anything grepping it.
    let ansi = std::io::stderr().is_terminal();

    match log {
        Some(file) => tracing_subscriber::fmt()
            .with_ansi(false)
            .with_env_filter(filter)
            .with_writer(std::io::stderr.and(file))
            .init(),
        None => tracing_subscriber::fmt()
            .with_ansi(ansi)
            .with_env_filter(filter)
            .init(),
    }
}

fn main() -> Result<()> {
    let backend = std::env::args().nth(1);
    // On the hardware the screen belongs to the compositor, so anything printed
    // to the terminal is printed underneath itself and lost. Whatever went
    // wrong there is exactly what needs reading afterwards, so it also goes to
    // a file — under state rather than the runtime dir, because the first time
    // this went wrong the only way out was a reboot, and the runtime dir does
    // not survive one.
    let log = matches!(backend.as_deref(), Some("--tty"))
        .then(open_log)
        .flatten();
    start_logging(log);

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting solium");

    // Nested when there is a compositor to nest in, on the hardware otherwise.
    // `--probe` reports what the hardware offers without taking it, which is
    // the only one of the three that is safe to run inside another session.
    match backend.as_deref() {
        Some("--probe") => tty::probe(),
        Some("--tty") => tty::run(),
        _ if std::env::var_os("WAYLAND_DISPLAY").is_some()
            || std::env::var_os("DISPLAY").is_some() =>
        {
            winit::run()
        }
        _ => tty::run(),
    }
}
