//! Solium — the Wayland compositor of Lilium DE.
//!
//! See `docs/architecture.md`. Layout and presentation are deliberately not in
//! the protocol handlers; every mode is a transform over window textures.

// A test fails by panicking — that is the mechanism, not a lapse. The workspace
// denies panics because a compositor crash takes the session down with it, and
// that reasoning does not apply to a test binary.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod assets;
mod capture;
mod cli;
mod clocks;
#[cfg(test)]
mod commit;
mod cursor;
mod decoration;
mod dev;
mod focus;
mod gputime;
mod group;
mod idle;
mod input;
mod json;
mod keyboard_change;
mod keymap;
mod launch;
mod layer;
mod lock;
mod mat4;
mod models;
mod monitor;
mod offscreen;
mod pacing;
mod pane;
mod pass;
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
mod session;
mod signals;
mod single_pixel;
mod stack;
mod state;
mod style;
mod surface;
mod synth;
mod text_input;
mod tty;
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

/// Send panics through the log as well as to stderr.
///
/// A panic prints to stderr, and on a hardware session stderr is the terminal
/// *underneath* the compositor: nobody can read it, and it never reaches the
/// log file either. That is how a crash comes to look identical to a freeze.
fn log_panics() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(
            location = info.location().map(ToString::to_string),
            "panic: {}",
            info
        );
        previous(info);
    }));
}

/// Load one QML file and report the outcome.
/// Load the configuration, report what it would do, and exit.
///
/// Reports the bindings it registered as well as any error, because "it
/// parsed" is not the question a ricer is asking -- "did my binding survive
/// the edit" is.
///
/// ## Why a file that loads can still fail this
///
/// It used to answer that question for bindings alone, and answer "ok" to
/// everything else. `config.lua` merges a `user.lua` over its defaults key by
/// key and validated nothing, so `tilling = { split = 0.6 }` was merged in as a
/// new section, read by nothing, for ever -- and the one command whose entire
/// job is telling you whether your configuration worked said it loaded fine
/// (#117). It had. It just was not doing what the file said.
///
/// So an unrecognised setting exits non-zero, and that is deliberate. The
/// question is not "did Lua run" -- a syntax error already answered that -- it
/// is "will this configuration do what I wrote", and a key nothing reads means
/// no. It also makes this usable from a script or a pre-commit hook, which a
/// command that always succeeds is not.
fn check_config() -> anyhow::Result<()> {
    let path = script::Scripts::config_path();
    println!("checking {}", path.display());
    match script::Scripts::load(&path) {
        Ok(scripts) => {
            let bindings = scripts.bindings();
            println!("  ok: {} binding(s)", bindings.len());
            for binding in bindings {
                // Padded so the notes line up into a column of their own, and
                // only when there is a note -- most configurations have none,
                // and trailing space on every line of the common case is
                // noise.
                match binding.note {
                    Some(note) => println!("    {:<28}{note}", binding.combo),
                    None => println!("    {}", binding.combo),
                }
            }

            // A binding taken away is the one thing the list above cannot
            // show: in a list of what survived, a key removed on purpose looks
            // exactly like one that never existed.
            let unbound = scripts.unbound();
            if !unbound.is_empty() {
                println!(
                    "  {} binding(s) removed by the configuration:",
                    unbound.len()
                );
                for binding in unbound {
                    match binding.note {
                        Some(note) => println!("    {:<28}{note}", binding.combo),
                        None => println!("    {}", binding.combo),
                    }
                }
            }

            let unknown = scripts.unknown_settings();
            if unknown.is_empty() {
                return Ok(());
            }
            println!(
                "  {} unrecognised setting(s) -- written, merged, and read by nothing:",
                unknown.len()
            );
            for setting in unknown {
                match setting.meant {
                    Some(meant) => println!("    {:<28}did you mean {meant}?", setting.key),
                    None => println!("    {}", setting.key),
                }
            }
            std::process::exit(1);
        }
        Err(err) => {
            // Printed rather than only returned: this is a command someone
            // runs to read the answer, and the chain is where the answer is.
            println!("  failed:");
            for (depth, cause) in err.chain().enumerate() {
                println!("    {:indent$}{cause}", "", indent = depth * 2);
            }
            std::process::exit(1);
        }
    }
}

fn check_qml(path: Option<String>) -> Result<()> {
    let Some(path) = path else {
        anyhow::bail!("usage: solium --check-qml <file.qml>");
    };
    // Software in every mode, so this is a software scene by construction
    // rather than by preference — the one caller in the compositor that is
    // right not to go through `qml::Scene::for_host`. It loads one file to say
    // whether it parses and then exits; nothing here is ever drawn, and asking
    // for a dmabuf would make the answer depend on whether the machine has a
    // render node rather than on the QML being checked. See
    // `renderer::check_qml_is_software_in_every_mode`.
    qml::renderer::decide(
        qml::renderer::Entry::CheckQml,
        &qml::renderer::Configured::default(),
    );
    qml::start()?;
    match qml::Scene::software(std::path::Path::new(&path), 400, 200) {
        Ok(_) => {
            println!("ok");
            Ok(())
        }
        Err(err) => {
            let message = err.to_string();
            // A `pragma Singleton` file cannot be built as a component, so Qt
            // answers with nothing rather than a reason. Say so, instead of
            // printing a blank line that reads like success.
            if message.trim().is_empty() || message.trim().ends_with(':') {
                println!("(no component: a singleton, or an empty error)");
            } else {
                println!("{message}");
            }
            Ok(())
        }
    }
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
        cli::Command::CheckQml(path) => check_qml(Some(path.to_string_lossy().into_owned())),
        // Loads the configuration and says whether it would run, without
        // touching a session. A typo found here costs a line of output; the
        // same typo found by reloading costs whatever you were doing.
        cli::Command::Check(_) => check_config(),
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
