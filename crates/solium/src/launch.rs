//! What a program Solium starts inherits from it (#175).
//!
//! Its environment is the one Solium was started with, and it holds no
//! descriptor beyond stdio. Neither is what this process holds by the time it
//! starts anything: Qt, EGL and the libraries they load write their own
//! settings into its environment, and Qt's eglfs opens a socketpair that is
//! not close-on-exec.
//!
//! The environment is taken whole, at the top of `main`, rather than scrubbed
//! of names known to leak. Two of the names a child was measured inheriting
//! are written by sdl2-compat's constructor, which no Solium code calls, and
//! the next library can add more:
//! `tests::a_spawned_program_gets_the_environment_solium_started_with`. The
//! descriptors are all marked close-on-exec in the child, rather than chased
//! one by one in the compositor:
//! `tests::a_spawned_program_holds_no_descriptor_beyond_stdio`.

use std::{
    ffi::{OsStr, OsString},
    os::unix::process::CommandExt as _,
    process::Command,
    sync::OnceLock,
};

/// The environment this process was started with. See [`remember`].
static STARTED_WITH: OnceLock<Vec<(OsString, OsString)>> = OnceLock::new();

/// Note the environment this process was started with.
///
/// A process that never calls this, such as a test, has it noted the first
/// time it starts a program instead.
pub(crate) fn remember() {
    let _ = started_with();
}

fn started_with() -> &'static [(OsString, OsString)] {
    STARTED_WITH.get_or_init(|| std::env::vars_os().collect())
}

/// A program to start as a child of this compositor: with the environment
/// Solium was started with, and every descriptor above stdio closed when it
/// executes.
pub(crate) fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    command
        .env_clear()
        .envs(started_with().iter().map(|(name, value)| (name, value)));
    // SAFETY: the hook runs in the child, between fork and exec, where only
    // async-signal-safe calls may be made. It makes one system call, reads
    // errno and allocates nothing.
    #[expect(unsafe_code, reason = "a hook that runs between fork and exec")]
    unsafe {
        command.pre_exec(close_on_exec_above_stdio);
    }
    command
}

/// Mark every descriptor above stdio close-on-exec, in one system call.
///
/// Marked rather than closed, because in a child std reports a failed exec
/// back to the parent through a pipe of its own, which is one of these. A
/// child that closed it would be reported as started:
/// `tests::a_program_that_does_not_exist_still_fails_to_start`.
pub(crate) fn close_on_exec_above_stdio() -> std::io::Result<()> {
    const CLOEXEC: libc::c_int = libc::CLOSE_RANGE_CLOEXEC as libc::c_int;
    // SAFETY: takes no pointers, and changes only this process's descriptor
    // flags.
    #[expect(unsafe_code, reason = "calling libc")]
    let marked = unsafe { libc::close_range(3, libc::c_uint::MAX, CLOEXEC) };
    if marked == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    //! The tests that start a compositor run a child process: this test binary
    //! again, running only [`child`]. It plays a compositor that has come up --
    //! its environment noted, then written to, Qt started, and a descriptor
    //! open without close-on-exec -- and then starts `sh`, which writes what it
    //! was given into a directory the test reads.

    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
        process::Stdio,
        time::{Duration, Instant},
    };

    use smithay::reexports::{
        rustix::net::{AddressFamily, SocketFlags, SocketType, socketpair},
        wayland_server::Display,
    };

    use super::*;
    use crate::state::Solium;

    /// Where the child's `sh` writes; set only for the child.
    const DIRECTORY: &str = "SOLIUM_LAUNCH_CHILD";
    /// How the child starts `sh`: `spawn` or `plain`.
    const ROLE: &str = "SOLIUM_LAUNCH_CHILD_ROLE";
    /// A variable of the user's, set before the compositor started.
    const USERS: &str = "SOLIUM_LAUNCH_USERS_OWN";
    const SOCKET: &str = "wayland-solium-launch-test";

    /// What the compositor and the libraries it loads write into its
    /// environment once it is running, as spike SVC-S3 measured them, apart
    /// from the two Qt's software host writes for itself in [`child`]. Written
    /// here because what writes them needs a GPU (`qml::keep_qt_off_the_hardware`),
    /// a host compositor (`winit.rs`) or Qt's eglfs plugin, which loads
    /// sdl2-compat.
    const WRITTEN_LATER: [(&str, &str); 8] = [
        ("QT_QPA_EGLFS_KMS_CONFIG", "solium-eglfs-kms.json"),
        ("QT_QPA_EGLFS_DISABLE_INPUT", "1"),
        ("QT_QPA_EGLFS_KMS_NO_EVENT_READER_THREAD", "1"),
        ("QT_QPA_NO_SIGNAL_HANDLER", "1"),
        ("QT_QPA_ENABLE_TERMINAL_KEYBOARD", "1"),
        ("__GL_SYNC_TO_VBLANK", "0"),
        ("SDL3_VERSION", "3.4.16"),
        ("SDL2_COMPAT", "1"),
    ];

    /// `$1` is the directory. `ls` runs in a process of its own and lists the
    /// shell's descriptors, so its own reading of the directory is not among
    /// them.
    const SCRIPT: &str = r#"env -0 > "$1/env"; ls /proc/$$/fd > "$1/fd"; : > "$1/done""#;

    /// Not a test of its own: the child process the tests below start.
    /// Without `SOLIUM_LAUNCH_CHILD` it does nothing.
    #[test]
    fn child() {
        let Some(directory) = std::env::var_os(DIRECTORY).map(PathBuf::from) else {
            return;
        };
        let role = std::env::var(ROLE).unwrap_or_default();

        // The first line of `main`.
        remember();
        // SAFETY: this process runs this one test, and nothing else in it has
        // started a thread yet.
        #[expect(unsafe_code, reason = "std::env::set_var is unsafe in edition 2024")]
        unsafe {
            for (name, value) in WRITTEN_LATER {
                std::env::set_var(name, value);
            }
        }
        // The real host, which writes `QT_QPA_PLATFORM=offscreen` and
        // `QT_QUICK_BACKEND=software` over whatever was there.
        crate::qml::start().expect("Qt starts");
        // As Qt's eglfs makes one inside `QGuiApplication`.
        let _pair = socketpair(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::empty(),
            None,
        )
        .expect("a socketpair without close-on-exec");

        let arguments = [
            "-c".to_owned(),
            SCRIPT.to_owned(),
            "sh".to_owned(),
            directory.to_string_lossy().into_owned(),
        ];
        match role.as_str() {
            // `sol.spawn`.
            "spawn" => {
                let display = Display::<Solium>::new().expect("a wayland display");
                let mut state = Solium::new(display.handle());
                state.socket_name = SOCKET.to_owned();
                state.spawn("sh", &arguments);
                let deadline = Instant::now() + Duration::from_secs(10);
                while !directory.join("done").exists() {
                    assert!(Instant::now() < deadline, "sh never finished");
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            // Anything started without `command`, as smithay starts Xwayland,
            // once this process has marked what it holds.
            "plain" => {
                close_on_exec_above_stdio().expect("marking this process's descriptors");
                let status = Command::new("sh")
                    .args(&arguments)
                    .stdin(Stdio::null())
                    .status()
                    .expect("running sh");
                assert!(status.success(), "sh failed: {status}");
            }
            other => panic!("no such role: {other}"),
        }
    }

    /// What `sh` was given.
    struct Given {
        environment: HashMap<String, String>,
        descriptors: Vec<String>,
    }

    /// What `sh` was given, in a child compositor playing `role`, for the test
    /// called `test`.
    fn started_in(role: &str, test: &str) -> Given {
        let directory =
            std::env::temp_dir().join(format!("solium-launch-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a directory for the child");
        let mut child = Command::new(std::env::current_exe().expect("this test binary"));
        // Started from a session that holds none of what a compositor writes,
        // as a real one is. This test process may hold some of it by now: a
        // test beside this one that started Qt had the host write its two.
        for (name, _) in WRITTEN_LATER {
            child.env_remove(name);
        }
        child.env_remove("QT_QUICK_BACKEND");
        let output = child
            .args([
                "--exact",
                "launch::tests::child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(DIRECTORY, &directory)
            .env(ROLE, role)
            // The user's own, which the software host writes over.
            .env("QT_QPA_PLATFORM", "wayland")
            .env(USERS, "kept")
            .env_remove("RUST_LOG")
            .stdin(Stdio::null())
            .output()
            .expect("running the child");
        assert!(
            output.status.success(),
            "the child failed ({}):\n{}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let given = Given {
            environment: read(&directory.join("env"))
                .split('\0')
                .filter_map(|entry| entry.split_once('='))
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect(),
            descriptors: read(&directory.join("fd"))
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
        };
        let _ = std::fs::remove_dir_all(&directory);
        given
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
    }

    /// **A program `sol.spawn` starts gets the environment Solium started
    /// with**, plus the session's own variables -- not what Qt and its
    /// libraries wrote into this process afterwards.
    ///
    /// Inherited, `QT_QPA_PLATFORM=offscreen` or `eglfs` gives a Qt program no
    /// window at all, and on the GPU host that is every Qt program (#175). A
    /// user's own `QT_QPA_PLATFORM`, set before Solium started, is theirs and
    /// is kept.
    #[test]
    fn a_spawned_program_gets_the_environment_solium_started_with() {
        let given = started_in("spawn", "environment");
        let value = |name: &str| given.environment.get(name).map(String::as_str);

        for (name, _) in WRITTEN_LATER {
            assert_eq!(value(name), None, "{name} reached the child");
        }
        assert_eq!(
            value("QT_QUICK_BACKEND"),
            None,
            "the software host's QT_QUICK_BACKEND reached the child"
        );
        assert_eq!(
            value("QT_QPA_PLATFORM"),
            Some("wayland"),
            "the child gets the user's own QT_QPA_PLATFORM, not the host's"
        );
        assert_eq!(value(USERS), Some("kept"), "and the rest of theirs");
        // Rules out a child given nothing but the snapshot.
        assert_eq!(
            value("WAYLAND_DISPLAY"),
            Some(SOCKET),
            "and the session's own variables"
        );
        assert!(value("XDG_ACTIVATION_TOKEN").is_some());
    }

    /// **A program `sol.spawn` starts holds no descriptor beyond stdio.**
    ///
    /// Qt's eglfs makes a socketpair inside `QGuiApplication` without
    /// close-on-exec, and one byte from a child holding it makes the
    /// compositor `_exit(1)` or stop itself (#175). Nested, NVIDIA's EGL leaves
    /// a render node open the same way, and anything Solium inherited from its
    /// own launcher reaches every child too. The child here is given one such
    /// descriptor to keep.
    #[test]
    fn a_spawned_program_holds_no_descriptor_beyond_stdio() {
        assert_eq!(
            started_in("spawn", "descriptors").descriptors,
            ["0", "1", "2"]
        );
    }

    /// **A descriptor marked by [`close_on_exec_above_stdio`] reaches no
    /// child, however the child is started** -- which is how smithay starts
    /// Xwayland: with a `Command` of its own, that [`command`]'s hook is not
    /// on.
    #[test]
    fn a_descriptor_marked_in_the_parent_reaches_no_child_started_any_other_way() {
        assert_eq!(started_in("plain", "marked").descriptors, ["0", "1", "2"]);
    }

    /// **A program that does not exist still fails to start.**
    ///
    /// std reports a failed exec back to the parent through a pipe of its
    /// own, which is one of the descriptors [`command`]'s hook deals with. A
    /// child that closed it instead of marking it would be reported as
    /// started, and a launch that can never arrive would keep its window open
    /// for the whole of `patience`.
    #[test]
    fn a_program_that_does_not_exist_still_fails_to_start() {
        let started = command("/nonexistent/solium-launch-test").spawn();
        assert_eq!(
            started.map(|_| ()).map_err(|err| err.kind()),
            Err(std::io::ErrorKind::NotFound)
        );
    }
}
