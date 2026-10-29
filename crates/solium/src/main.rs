//! Solium — a configurable Wayland compositor with Lua scripting and QML scenes.
//!
//! See `docs/architecture.md`. Layout and presentation are deliberately not in
//! the protocol handlers; every mode is a transform over window textures.

// A test fails by panicking — that is the mechanism, not a lapse. The workspace
// denies panics because a compositor crash takes the session down with it, and
// that reasoning does not apply to a test binary.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod ages;
mod apps;
mod assets;
mod auth;
mod autoreload;
mod capture;
mod check;
mod cli;
mod clip;
mod clocks;
#[cfg(test)]
mod commit;
mod cursor;
mod decoration;
mod dev;
mod effect;
mod explicit_sync;
mod focus;
mod folder;
mod gputime;
mod group;
mod icon;
mod idle;
mod image_copy;
mod input;
mod json;
mod keyboard_change;
mod keyed;
mod keymap;
mod launch;
mod layer;
mod lock;
mod lock_auth;
mod lock_scene;
mod logind;
mod mat4;
mod models;
mod monitor;
mod offscreen;
mod pacing;
mod pane;
mod pass;
mod pin;
mod pin_cli;
mod pool;
mod power;
mod present;
mod qml;
mod remains;
mod render;
mod resizing;
#[cfg(test)]
mod scenario;
mod screencopy;
mod screensaver;
mod script;
mod scripted;
mod services;
mod session;
mod shortcuts_inhibit;
mod signals;
mod single_pixel;
mod stack;
mod state;
mod style;
mod successor;
mod surface;
mod synth;
mod text_input;
mod tty;
mod warm;
mod warp;
mod winit;
mod xwayland;

use std::{io::IsTerminal, path::PathBuf};

use anyhow::Result;
use tracing_subscriber::fmt::writer::MakeWriterExt as _;

/// `$XDG_STATE_HOME/solium`, or `~/.local/state/solium`.
fn state_directory() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    Some(base.join("solium"))
}

/// Where a hardware session writes its log.
///
/// Returns `None` rather than failing: not being able to write a log is a
/// reason to run without one, never a reason not to start.
fn open_log() -> Option<std::sync::Arc<std::fs::File>> {
    let directory = state_directory()?;
    std::fs::create_dir_all(&directory).ok()?;
    let path = directory.join("session.log");

    // Appended, so the log of the run that went wrong is still there after the
    // run that was meant to fix it.
    //
    // Written synchronously, which is not the usual trade. This log exists for
    // the case where the compositor has taken the screen and stopped
    // responding, and the only way out is the power button -- and a hard reset
    // takes the page cache with it. A log that loses its last seconds loses
    // exactly the seconds worth reading. At a few lines a session that costs
    // nothing; under `RUST_LOG=debug` it is slow, and worth it anyway.
    use std::os::unix::fs::OpenOptionsExt as _;
    let synchronous =
        i32::try_from(smithay::reexports::rustix::fs::OFlags::DSYNC.bits()).unwrap_or(0);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .custom_flags(synchronous)
        .open(&path)
        .ok()?;
    eprintln!("logging to {}", path.display());
    Some(std::sync::Arc::new(file))
}

fn start_logging(log: Option<std::sync::Arc<std::fs::File>>) {
    use tracing_subscriber::{Layer as _, layer::SubscriberExt as _, util::SubscriberInitExt as _};
    let env = std::env::var("RUST_LOG").ok();
    // Escape codes in a redirected log break anything grepping it.
    let ansi = std::io::stderr().is_terminal();

    match log {
        Some(file) => tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(std::io::stderr.and(file))
                    .with_filter(log_filter(env.as_deref())),
            )
            .init(),
        None => tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(ansi)
                    .with_filter(log_filter(env.as_deref())),
            )
            .init(),
    }
}

/// Where a log line can name a key: smithay's keyboard, which traces each
/// keycode and its keysym as it is pressed, and Solium's own key lines.
const KEY_TARGETS: [&str; 3] = [
    "smithay::input::keyboard",
    "smithay::wayland::seat::keyboard",
    "solium::keys",
];

/// What the log keeps: `RUST_LOG`'s directives (`env`), `info` without
/// them, and never anything about keys below `info`, whatever they say. A
/// password typed at the lock would otherwise be in the log a key at a time
/// under `RUST_LOG=trace`. Capped twice: a directive per key target, which
/// replaces one `RUST_LOG` gives for the same target, and a filter beside the
/// `EnvFilter`, since a directive on a span's fields (`[input]=trace`) can
/// enable what no static directive does.
/// `tests::key_tracing_is_capped_at_info_whatever_rust_log_says`.
fn log_filter<S>(env: Option<&str>) -> impl tracing_subscriber::layer::Filter<S> + use<S>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    use tracing_subscriber::filter::FilterExt as _;
    let mut filter = env
        .and_then(|env| tracing_subscriber::EnvFilter::try_new(env).ok())
        .unwrap_or_else(|| tracing_subscriber::EnvFilter::new("info"));
    for target in KEY_TARGETS {
        if let Ok(directive) = format!("{target}=info").parse() {
            filter = filter.add_directive(directive);
        }
    }
    filter.and(tracing_subscriber::filter::filter_fn(|meta| {
        *meta.level() <= tracing::Level::INFO || !names_keys(meta.target())
    }))
}

/// Whether `target` is one of [`KEY_TARGETS`] or inside one.
fn names_keys(target: &str) -> bool {
    KEY_TARGETS.iter().any(|keys| {
        target
            .strip_prefix(keys)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
    })
}

/// Send panics through the log as well as to stderr.
///
/// A panic prints to stderr, and on a hardware session stderr is the terminal
/// *underneath* the compositor: nobody can read it, and it never reaches the
/// log file either. That is how a crash comes to look identical to a freeze.
fn log_panics() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // The backtrace into the log, whatever RUST_BACKTRACE says: a
        // compositor's panic happens once, on someone's hardware, and it
        // aborts right after this (#283), so this line is all there will be
        // to read.
        tracing::error!(
            location = info.location().map(ToString::to_string),
            backtrace = %std::backtrace::Backtrace::force_capture(),
            "panic: {}",
            info
        );
        previous(info);
        // Stopped where it happened, in every build. Unwinding cannot save a
        // session, and unwound past Qt it ran Qt's exit handlers on state the
        // panic had left half-dropped: the heap corrupted, and the core dump
        // blamed Qt instead of the panic. Only the compositor installs this
        // hook; tests unwind, as `#[should_panic]` and the Qt test thread need.
        std::process::abort();
    }));
}

/// What `main` does first, before any thread starts: note the environment
/// every program Solium starts is given, and only then take the session's
/// input method out of the compositor's own Qt.
/// `launch::tests::the_compositors_qt_takes_no_input_method_from_the_session`,
/// `launch::tests::a_spawned_program_gets_the_input_method_the_compositors_qt_does_not`.
fn prepare_environment() {
    launch::remember();
    qml::keep_input_methods_out();
}

fn main() -> Result<std::process::ExitCode> {
    // First, before Qt, EGL or a library they load has written anything into
    // the environment, and before any thread starts: it is what every program
    // Solium starts is given. See
    // `launch::tests::a_spawned_program_gets_the_environment_solium_started_with`.
    prepare_environment();
    // The password helper, opened while this is the only thread and held for
    // the whole session, so an upgrade cannot swap it under the lock (`auth`).
    auth::open_at_startup();
    // Every argument decided before anything starts: `cli::tests`, and the
    // process tests in `tests/cli.rs`.
    let command = match cli::parse(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(refused) => {
            eprintln!("solium: {refused}. See solium --help.");
            return Ok(std::process::ExitCode::from(cli::USAGE_ERROR));
        }
    };
    match command {
        cli::Command::Help => {
            print!("{}", cli::help());
            return Ok(std::process::ExitCode::SUCCESS);
        }
        cli::Command::Version => {
            println!("{}", cli::version());
            return Ok(std::process::ExitCode::SUCCESS);
        }
        // Before logging starts: the probe's child answers in one line.
        cli::Command::ProbeQmlGpu => qml::renderer::probe_child(),
        // Before logging starts too: nothing it reads is logged, and it
        // starts nothing but the helper.
        cli::Command::Pin(action) => return Ok(pin_cli::run(action)),
        _ => {}
    }
    // On the hardware the screen belongs to the compositor, so anything printed
    // to the terminal is printed underneath itself and lost. Whatever went
    // wrong there is exactly what needs reading afterwards, so it also goes to
    // a file — under state rather than the runtime dir, because the first time
    // this went wrong the only way out was a reboot, and the runtime dir does
    // not survive one.
    let log = (command == cli::Command::Tty).then(open_log).flatten();
    start_logging(log);

    log_panics();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting solium");
    // Where this build's QML and Lua turned out to be, said once and early. If
    // they are nowhere, this is the line that says so -- see `assets`.
    assets::announce();

    // Nested when there is a compositor to nest in, on the hardware otherwise.
    // `--probe` reports what the hardware offers without taking it, which is
    // the only one of the three that is safe to run inside another session.
    let ran = match command {
        // Loads one QML file and says what went wrong, without starting a
        // compositor. Writing a shell means walking a chain of "type X
        // unavailable" errors, and doing that through a real session costs ten
        // seconds a link.
        cli::Command::CheckQml(path) => return Ok(check::run(Some(&path))),
        // Loads the configuration and says whether it would run, without
        // touching a session. A typo found here costs a line of output; the
        // same typo found by reloading costs whatever you were doing.
        cli::Command::Check(file) => return Ok(check::run(file.as_deref())),
        cli::Command::Probe => tty::probe(),
        cli::Command::Tty => tty::run(session::Place::tty(std::env::args().skip(1))),
        _ if std::env::var_os("WAYLAND_DISPLAY").is_some()
            || std::env::var_os("DISPLAY").is_some() =>
        {
            winit::run()
        }
        _ => tty::run(session::Place::Console),
    };
    ran.map(|()| std::process::ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, PoisonError};

    use tracing_subscriber::{Layer as _, layer::SubscriberExt as _};

    /// A log written into memory.
    #[derive(Clone, Default)]
    struct Written(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Written {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Written {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap_or_else(PoisonError::into_inner))
                .into_owned()
        }
    }

    /// What the log keeps of a key pressed, as smithay's keyboard traces it,
    /// and of lines beside it, under `RUST_LOG=env`.
    fn logged(env: &str) -> String {
        let written = Written::default();
        let writer = written.clone();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(move || writer.clone())
                .with_filter(super::log_filter(Some(env))),
        );
        tracing::subscriber::with_default(subscriber, || {
            let span =
                tracing::trace_span!(target: "smithay::input::keyboard", "input", keycode = 38);
            let _entered = span.enter();
            tracing::trace!(target: "smithay::input::keyboard", sym = "a", "Calling input filter");
            tracing::debug!(target: "solium::keys", combo = "a", "key");
            tracing::info!(target: "smithay::input::keyboard", "keymap loaded");
            tracing::trace!(target: "solium::render", "a frame");
            tracing::info!(target: "solium::input", "inside the key's span");
        });
        written.text()
    }

    /// **Key tracing is capped at `info`, whatever `RUST_LOG` says**: under
    /// `trace`, under a directive naming smithay's keyboard itself, and
    /// under one on the span's fields, no keysym, no keycode and no combo is
    /// logged, while an `info` line from the keyboard and a `trace` line from
    /// elsewhere still are.
    #[test]
    fn key_tracing_is_capped_at_info_whatever_rust_log_says() {
        for env in [
            "trace",
            "smithay=trace",
            "smithay::input::keyboard=trace,solium::keys=trace",
            "[input]=trace",
            "smithay::input::keyboard[input]=trace",
            "[{keycode}]=trace",
        ] {
            let log = logged(env);
            assert!(log.contains("keymap loaded"), "{env}: {log}");
            for secret in ["Calling input filter", "sym=", "keycode", "combo"] {
                assert!(!log.contains(secret), "{env}: {secret:?} logged: {log}");
            }
        }
        let log = logged("trace");
        assert!(
            log.contains("a frame"),
            "the premise: trace is on elsewhere: {log}"
        );
    }
}
