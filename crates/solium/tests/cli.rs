//! The binary itself, run as a process: what `--help`, `--version` and an
//! unknown flag do before anything starts (#156), and the exit status of a
//! check.
//!
//! **Safety.** These run only against a binary whose parser is in, and never
//! against an unfixed one. Even so, every run has `WAYLAND_DISPLAY` naming a
//! socket that does not exist, no `DISPLAY`, no session bus, `HOME`,
//! `XDG_CONFIG_HOME`, `XDG_RUNTIME_DIR` and `XDG_STATE_HOME` in a directory of
//! its own, `SOLIUM_LUA_INIT` naming a file there that does not exist unless
//! the test writes it, and a ten
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
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

fn run(argument: &str) -> (i32, String, String) {
    run_args(&[argument], argument.trim_start_matches('-'))
}

fn run_args(arguments: &[&str], name: &str) -> (i32, String, String) {
    run_args_with(arguments, name, |_| {})
}

/// Run with `arguments` in a directory of the test's own, named by `name` and
/// emptied first: `setup` writes into it before the process starts (it is
/// `HOME` and `XDG_CONFIG_HOME`), and `SOLIUM_LUA_INIT` names its `init.lua`,
/// which is there only if `setup` writes it.
fn run_args_with(
    arguments: &[&str],
    name: &str,
    setup: impl FnOnce(&Path),
) -> (i32, String, String) {
    let argument = arguments.join(" ");
    let dir = std::env::temp_dir().join(format!("solium-cli-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory of the test's own");
    setup(&dir);
    let mut child = Command::new(env!("CARGO_BIN_EXE_solium"))
        .args(arguments)
        .env("WAYLAND_DISPLAY", "solium-cli-test-no-such-socket")
        .env_remove("DISPLAY")
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .env("HOME", &dir)
        .env("XDG_CONFIG_HOME", &dir)
        .env("XDG_RUNTIME_DIR", &dir)
        .env("XDG_STATE_HOME", &dir)
        .env("SOLIUM_LUA_INIT", dir.join("init.lua"))
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

/// A QML file that does not load exits **1**: `--check-qml` used to print
/// the error and exit 0, so nothing but a person reading could tell.
#[test]
fn check_qml_on_a_broken_file_exits_one() {
    let broken = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/check/broken.qml"
    );
    let (code, out, err) = run_args(&["--check-qml", broken], "check-qml");
    assert_eq!(code, 1, "{out}{err}");
    assert!(out.contains("broken.qml"), "{out}");
}

/// `solium --check <effect folder>` exits 1 on a broken one, naming its
/// `effect.lua`, and starts nothing (the same isolation as every test here).
#[test]
fn check_exits_1_on_a_broken_effect_folder() {
    let folder = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/effects/api2");
    let (code, out, err) = run_args(&["--check", folder], "check-effect");
    assert_eq!(code, 1, "{out}{err}");
    assert!(out.contains("api2/effect.lua"), "{out}");
}

/// **A good effect folder exits 0**, and is not also handed to the QML-file
/// check, which would start Qt and fail on a directory.
#[test]
fn check_exits_0_on_a_good_effect_folder() {
    let folder = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/effects/identity"
    );
    let (code, out, err) = run_args(&["--check", folder], "check-effect-good");
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("identity: ok"), "{out}");
    assert!(
        !out.contains("Qt"),
        "the QML check ran on a directory: {out}"
    );
}

/// **`--check <folder>` reads the configuration's `effects.sandbox`**, as
/// the session loads the folder under it: one that builds 20 MiB exits 1
/// with no configuration (the default 16 MiB) and 0 under one that gives
/// 64.
#[test]
fn check_of_a_folder_reads_the_configured_sandbox() {
    let check = |name: &str, configured: bool| {
        let folder = std::env::temp_dir()
            .join(format!("solium-cli-test-{}-{name}", std::process::id()))
            .join("big");
        let argument = folder.to_string_lossy().into_owned();
        run_args_with(&["--check", &argument], name, |dir| {
            std::fs::create_dir_all(dir.join("big")).expect("the folder");
            std::fs::write(
                dir.join("big/effect.lua"),
                "local big = string.rep('x', 20 * 1024 * 1024)\nreturn { api = 1, inputs = { 'self' }, frag = 'effect.frag' }\n",
            )
            .expect("its effect.lua");
            std::fs::write(
                dir.join("big/effect.frag"),
                "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n",
            )
            .expect("its frag");
            if configured {
                std::fs::write(
                    dir.join("init.lua"),
                    "sol.effects({ sandbox = { memory_mib = 64, load_ms = 5000 } })\n",
                )
                .expect("an init.lua");
            }
        })
    };
    let (code, out, err) = check("check-caps-default", false);
    assert_eq!(code, 1, "the premise: 20 MiB under 16: {out}{err}");
    let (code, out, err) = check("check-caps-roomy", true);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("big: ok"), "{out}");
}

/// **Plain `--check` exits 1 on a broken folder in the user's effects/**
/// (spec §8.4), and 0 with the same configuration and no such folder.
#[test]
fn check_exits_1_on_a_broken_user_effect_folder() {
    let config = |dir: &Path, broken: bool| {
        std::fs::write(dir.join("init.lua"), "sol.pane('none')\n").expect("an init.lua");
        if broken {
            let api2 = Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/effects/api2"
            ));
            let into = dir.join("solium/effects/api2");
            std::fs::create_dir_all(&into).expect("the user's effects folder");
            for file in ["effect.lua", "effect.frag"] {
                std::fs::copy(api2.join(file), into.join(file)).expect("copying the fixture");
            }
        }
    };
    let (clean, out, err) =
        run_args_with(&["--check"], "check-user-clean", |dir| config(dir, false));
    assert_eq!(clean, 0, "the control: {out}{err}");
    let (broken, out, err) =
        run_args_with(&["--check"], "check-user-broken", |dir| config(dir, true));
    assert_eq!(broken, 1, "{out}{err}");
    assert!(out.contains("api2/effect.lua"), "{out}");
}
