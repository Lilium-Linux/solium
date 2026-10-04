//! The binary itself, run as a process: what `--help`, `--version` and an
//! unknown flag do before anything starts (#156).
//!
//! **Safety.** These run only against a binary whose parser is in, and never
//! against an unfixed one. Even so, every run has `WAYLAND_DISPLAY` naming a
//! socket that does not exist, no `DISPLAY`, no session bus, `HOME`,
//! `XDG_CONFIG_HOME`, `XDG_RUNTIME_DIR` and `XDG_STATE_HOME` in a directory of
//! its own, `SOLIUM_LUA_INIT` naming a file that does not exist, and a ten
//! second limit: a regression starts a nested backend that fails to connect,
//! never `tty::run`, so it can never take a VT, and it reads none of your
//! configuration.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "a test fails by panicking"
)]

use std::{
    process::Command,
    time::{Duration, Instant},
};

fn run(argument: &str) -> (i32, String, String) {
    let dir = std::env::temp_dir().join(format!(
        "solium-cli-test-{}-{}",
        std::process::id(),
        argument.trim_start_matches('-')
    ));
    let _ = std::fs::create_dir_all(&dir);
    let mut child = Command::new(env!("CARGO_BIN_EXE_solium"))
        .arg(argument)
        .env("WAYLAND_DISPLAY", "solium-cli-test-no-such-socket")
        .env_remove("DISPLAY")
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .env("HOME", &dir)
        .env("XDG_CONFIG_HOME", &dir)
        .env("XDG_RUNTIME_DIR", &dir)
        .env("XDG_STATE_HOME", &dir)
        .env("SOLIUM_LUA_INIT", dir.join("no-such-init.lua"))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("solium starts");
    let started = Instant::now();
    loop {
        if child.try_wait().expect("waiting").is_some() {
            break;
        }
        if started.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("solium {argument} was still running after ten seconds");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let output = child.wait_with_output().expect("its output");
    let _ = std::fs::remove_dir_all(&dir);
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn help_exits_zero_and_starts_nothing() {
    let (code, out, err) = run("--help");
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("--tty"));
    assert!(!err.contains("starting solium"), "it started: {err}");
}

#[test]
fn version_exits_zero_and_starts_nothing() {
    let (code, out, err) = run("--version");
    assert_eq!(code, 0, "{err}");
    assert!(out.starts_with("solium "));
    assert!(!err.contains("starting solium"));
}

/// Exit status **2**, not merely non-zero: a regression that starts the
/// nested backend against the dead socket also exits non-zero, and must not
/// pass.
#[test]
fn an_unknown_flag_exits_two_and_starts_nothing() {
    let (code, _out, err) = run("--hlep");
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("--help"));
    assert!(!err.contains("starting solium"));
}
