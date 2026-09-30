//! Ending cleanly when a signal asks, and ending anyway when that cannot
//! happen (#146).
//!
//! logind ends a session with SIGTERM, followed at once by SIGHUP; `kill` and
//! `timeout` send SIGTERM, and a terminal sends SIGINT and SIGHUP. Left to
//! their default, each ends the process where it stands: `Session::end` never
//! runs, so `solium-session.target` and the exported environment outlive the
//! compositor, and no destructor runs either. Qt installs no handler of its
//! own here (`QT_QPA_NO_SIGNAL_HANDLER`, in `qml.rs`).
//!
//! So the first of the three stops the event loop, and the backend leaves the
//! way it leaves for a key: `each_ending_signal_stops_the_loop`. SIGHUP
//! straight after SIGTERM is the same request, not a second one:
//! `logind_s_sigterm_and_sighup_are_one_request`.
//!
//! A loop that has stopped answering (a render, a GPU wait or a Lua callback
//! that never returns) cannot stop, and a handler that only asked it to would
//! leave the process ignoring the signal. So the way out does not go through
//! the loop. The same signal a second time ends the process at once, from the
//! handler itself, which is what `pkill -x solium` twice does:
//! `the_same_signal_twice_ends_a_process_that_cannot_stop`. A second time
//! means [`REPEAT`] or more after the first: `timeout` sends its signal to the
//! process and then to the process group it is in, so one request can arrive
//! twice a moment apart (`the_same_signal_twice_at_once_is_one_request`). And
//! a process still here `session.stop_timeout` after the first signal ends
//! itself, from a thread of its own:
//! `a_process_that_cannot_stop_ends_when_its_stop_timeout_has_passed`. Both
//! are SIGKILL, and `solium-session` then cleans up as it does after a crash.
//! No handler is ever removed, so both hold through teardown.
//!
//! Handlers rather than a blocked mask read through a signalfd: a mask is
//! inherited by every program Solium starts, XWayland included, and would
//! have them ignore the same signals, while a handler is reset by `exec`.
//! `a_program_solium_starts_still_dies_of_sigterm`.

use std::{
    io::{ErrorKind, Read as _, Write as _},
    os::unix::net::UnixStream,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use smithay::reexports::{
    calloop::{
        LoopHandle,
        ping::{Ping, make_ping},
    },
    rustix::{
        process::{Signal, getpid, kill_process},
        time::{ClockId, clock_gettime},
    },
};

/// The signals that end a session.
const ENDING: [Signal; 3] = [Signal::TERM, Signal::INT, Signal::HUP];

/// `session.stop_timeout` until the configuration says otherwise.
pub(crate) const STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// How long after the first a signal has to come to be a second request, in
/// milliseconds. `the_same_signal_twice_at_once_is_one_request`.
const REPEAT: u64 = 500;

/// The listening, once started: where the configuration's
/// `session.stop_timeout` goes once it has loaded.
#[derive(Clone, Debug)]
pub(crate) struct Listener {
    stop_timeout: Arc<AtomicU64>,
}

impl Listener {
    /// How long a process asked to end has before it ends itself, read when
    /// the first signal arrives.
    /// `a_process_that_cannot_stop_ends_when_its_stop_timeout_has_passed`.
    pub(crate) fn stop_timeout(&self, timeout: Duration) {
        self.stop_timeout
            .store(millis(timeout).max(1), Ordering::Relaxed);
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// The monotonic clock, in milliseconds: `clock_gettime(2)`, which a signal
/// handler may call.
fn now() -> u64 {
    let now = clock_gettime(ClockId::Monotonic);
    u64::try_from(now.tv_sec)
        .unwrap_or(0)
        .saturating_mul(1000)
        .saturating_add(u64::try_from(now.tv_nsec).unwrap_or(0) / 1_000_000)
}

/// Call `stop` on the event loop when SIGTERM, SIGINT or SIGHUP first
/// arrives, and end the process when the same signal arrives again, or when
/// it is still here once the stop timeout has passed.
///
/// Not being able to listen is a warning: the session runs, and a signal
/// ends it the old way.
pub(crate) fn listen<D: 'static>(
    handle: &LoopHandle<'static, D>,
    stop: impl FnMut(&mut D) + 'static,
) -> Listener {
    let listener = Listener {
        stop_timeout: Arc::new(AtomicU64::new(millis(STOP_TIMEOUT))),
    };
    if let Err(err) = install(handle, stop, listener.stop_timeout.clone()) {
        tracing::warn!(
            ?err,
            "cannot listen for SIGTERM, SIGINT and SIGHUP: one of them ends this \
             session without stopping solium-session.target"
        );
    }
    listener
}

fn install<D: 'static>(
    handle: &LoopHandle<'static, D>,
    mut stop: impl FnMut(&mut D) + 'static,
    stop_timeout: Arc<AtomicU64>,
) -> anyhow::Result<()> {
    let (ping, source) = make_ping()?;
    handle
        .insert_source(source, move |(), _, data| stop(data))
        .map_err(|err| anyhow::anyhow!("watching for the signal: {:?}", err.error))?;
    // The first signal's number goes from the handler to the watcher here.
    // Written without blocking, so a handler can never wait.
    let (heard, wake) = UnixStream::pair()?;
    wake.set_nonblocking(true)?;
    // The watcher first, so no handler is installed with nobody to read what
    // it writes.
    std::thread::Builder::new()
        .name("solium-signals".to_owned())
        .spawn(move || watch(heard, &ping, &stop_timeout))?;

    let asked = Arc::new(AtomicBool::new(false));
    // When the first signal came; never, until one has.
    let first = Arc::new(AtomicU64::new(u64::MAX));
    // rustix finds the clock on its first call, which is made here rather
    // than in a handler.
    let _ = now();
    for signal in ENDING {
        let number = u8::try_from(signal.as_raw()).unwrap_or(0);
        let asked = asked.clone();
        let first = first.clone();
        let again = AtomicBool::new(false);
        let wake = wake.try_clone()?;
        let action = move || {
            let now = now();
            if again.swap(true, Ordering::SeqCst) {
                if now.saturating_sub(first.load(Ordering::SeqCst)) >= REPEAT {
                    let _ = kill_process(getpid(), Signal::KILL);
                }
            } else if !asked.swap(true, Ordering::SeqCst) {
                first.store(now, Ordering::SeqCst);
                let _ = (&wake).write(&[number]);
            }
        };
        // SAFETY: `register` asks for an action that is async-signal-safe,
        // and this one is: lock-free atomics, `clock_gettime(2)`, then at
        // most a `write(2)` to a non-blocking socket, or `getpid(2)` and
        // `kill(2)`, all async-signal-safe. It allocates nothing, takes no
        // lock and does not log.
        #[expect(unsafe_code, reason = "installing a signal handler")]
        let registered = unsafe { signal_hook_registry::register(signal.as_raw(), action) };
        registered?;
    }
    Ok(())
}

/// The first signal stops the loop; the stop timeout after it ends the
/// process, if the process is still here.
fn watch(mut heard: UnixStream, ping: &Ping, stop_timeout: &AtomicU64) {
    let mut number = [0_u8; 1];
    loop {
        match heard.read(&mut number) {
            Ok(1) => break,
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            other => {
                tracing::warn!(
                    ?other,
                    "no longer reading SIGTERM, SIGINT and SIGHUP: the same one twice \
                     still ends Solium"
                );
                return;
            }
        }
    }
    let timeout = Duration::from_millis(stop_timeout.load(Ordering::Relaxed));
    tracing::info!(
        signal = name(number[0]),
        ?timeout,
        "asked to end by a signal: stopping"
    );
    ping.ping();
    std::thread::sleep(timeout);
    tracing::error!(
        ?timeout,
        "still running session.stop_timeout after being asked to end, so something \
         inside has stopped answering: ending at once. solium-session cleans up after it"
    );
    let _ = kill_process(getpid(), Signal::KILL);
}

fn name(number: u8) -> &'static str {
    ENDING
        .into_iter()
        .zip(["SIGTERM", "SIGINT", "SIGHUP"])
        .find(|(signal, _)| signal.as_raw() == i32::from(number))
        .map_or("a signal", |(_, name)| name)
}

#[cfg(test)]
mod tests {
    //! Each test runs a child process: this test binary again, running only
    //! [`child`], which does what `SOLIUM_SIGNALS_CHILD` says. The handlers
    //! live and die with that child, and the signals sent to it never reach
    //! the tests around it.

    use std::{
        io::{BufRead as _, BufReader, Lines},
        os::unix::process::ExitStatusExt as _,
        process::{ChildStdout, Command, ExitStatus, Stdio},
        time::Instant,
    };

    use smithay::reexports::{calloop::EventLoop, rustix::process::Pid};

    use super::*;

    const ROLE: &str = "SOLIUM_SIGNALS_CHILD";
    const TIMEOUT: &str = "SOLIUM_SIGNALS_CHILD_STOP_TIMEOUT_MS";
    /// What the child prints once its handlers are in place.
    const READY: &str = "solium-signals-child: listening";
    /// How long a child that should end is given to.
    const PROMPTLY: Duration = Duration::from_secs(5);
    /// Longer than any test waits: a stop timeout that must never pass.
    const NEVER: Duration = Duration::from_secs(60);

    /// Not a test of its own: the child process each test below starts.
    /// Without `SOLIUM_SIGNALS_CHILD` it does nothing.
    #[test]
    fn child() {
        let Ok(role) = std::env::var(ROLE) else {
            return;
        };
        let stop_timeout = std::env::var(TIMEOUT)
            .ok()
            .and_then(|millis| millis.parse().ok())
            .map_or(NEVER, Duration::from_millis);
        let mut event_loop: EventLoop<bool> = EventLoop::try_new().expect("an event loop");
        listen(&event_loop.handle(), |stopped: &mut bool| *stopped = true)
            .stop_timeout(stop_timeout);
        println!("{READY}");
        match role.as_str() {
            // A loop that answers, and a teardown that takes a moment.
            "loop" => {
                let mut stopped = false;
                let deadline = Instant::now() + NEVER;
                while !stopped && Instant::now() < deadline {
                    event_loop
                        .dispatch(Some(Duration::from_millis(20)), &mut stopped)
                        .expect("dispatching");
                }
                std::thread::sleep(Duration::from_millis(500));
                std::process::exit(if stopped { 0 } else { 3 });
            }
            // A loop that is never dispatched again.
            "wedged" => {
                std::thread::sleep(NEVER);
                std::process::exit(3);
            }
            "spawns" => {
                let status = Command::new("sh")
                    .args(["-c", "kill -s TERM $$; sleep 5"])
                    .status()
                    .expect("running sh");
                std::process::exit(if status.signal() == Some(15) { 0 } else { 4 });
            }
            _ => std::process::exit(5),
        }
    }

    struct Child {
        process: std::process::Child,
        /// Held open until the child has gone, so it never writes to a
        /// closed pipe.
        _output: Lines<BufReader<ChildStdout>>,
    }

    impl Child {
        /// A child in `role`, with its handlers in place.
        fn start(role: &str, stop_timeout: Duration) -> Self {
            let mut process = Command::new(std::env::current_exe().expect("this test binary"))
                .args([
                    "--exact",
                    "signals::tests::child",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(ROLE, role)
                .env(TIMEOUT, millis(stop_timeout).to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("starting the child");
            let mut output = BufReader::new(process.stdout.take().expect("its output")).lines();
            loop {
                match output.next() {
                    // After the harness's own `test ... ` on the same line.
                    Some(Ok(line)) if line.ends_with(READY) => break,
                    Some(Ok(_)) => {}
                    _ => panic!("the child ended before it listened: {:?}", process.wait()),
                }
            }
            Self {
                process,
                _output: output,
            }
        }

        fn send(&self, signal: Signal) {
            kill_process(Pid::from_child(&self.process), signal).expect("signalling the child");
        }

        /// How it ended, if it did within `within`; killed, and `None`, if not.
        fn ends_within(&mut self, within: Duration) -> Option<ExitStatus> {
            let deadline = Instant::now() + within;
            loop {
                if let Some(status) = self.process.try_wait().expect("waiting for the child") {
                    return Some(status);
                }
                if Instant::now() >= deadline {
                    let _ = self.process.kill();
                    let _ = self.process.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    /// Each of the three stops the loop, and the process then ends the way
    /// it chose to, rather than of the signal.
    #[test]
    fn each_ending_signal_stops_the_loop() {
        for signal in ENDING {
            let mut child = Child::start("loop", NEVER);
            child.send(signal);
            let status = child.ends_within(PROMPTLY);
            assert_eq!(
                status.and_then(|status| status.code()),
                Some(0),
                "{signal:?}: {status:?}"
            );
        }
    }

    /// systemd stops a session's scope with SIGTERM and then, straight away,
    /// SIGHUP (logind asks for `SendSIGHUP=yes`): one request to end, and the
    /// clean stop it asks for is not cut short.
    #[test]
    fn logind_s_sigterm_and_sighup_are_one_request() {
        let mut child = Child::start("loop", NEVER);
        child.send(Signal::TERM);
        child.send(Signal::HUP);
        let status = child.ends_within(PROMPTLY);
        assert_eq!(
            status.and_then(|status| status.code()),
            Some(0),
            "{status:?}"
        );
    }

    /// A loop that never runs again cannot stop. The same signal a second
    /// time ends the process anyway, without waiting for the stop timeout.
    #[test]
    fn the_same_signal_twice_ends_a_process_that_cannot_stop() {
        for signal in ENDING {
            let mut child = Child::start("wedged", NEVER);
            child.send(signal);
            std::thread::sleep(Duration::from_millis(REPEAT + 200));
            child.send(signal);
            let status = child.ends_within(PROMPTLY);
            assert_eq!(
                status.and_then(|status| status.signal()),
                Some(Signal::KILL.as_raw()),
                "{signal:?}: {status:?}"
            );
        }
    }

    /// `timeout -s TERM` signals the process it runs, and then the process
    /// group it is in, which the process is in too: one request, heard twice
    /// a moment apart. The clean stop it asks for is not cut short.
    #[test]
    fn the_same_signal_twice_at_once_is_one_request() {
        let mut child = Child::start("loop", NEVER);
        child.send(Signal::TERM);
        // Apart, so the kernel cannot fold the two into one.
        std::thread::sleep(Duration::from_millis(50));
        child.send(Signal::TERM);
        let status = child.ends_within(PROMPTLY);
        assert_eq!(
            status.and_then(|status| status.code()),
            Some(0),
            "{status:?}"
        );
    }

    /// One signal, and a loop that never runs again: the process ends itself
    /// once the stop timeout has passed, and not before.
    #[test]
    fn a_process_that_cannot_stop_ends_when_its_stop_timeout_has_passed() {
        let stop_timeout = Duration::from_millis(400);
        let mut child = Child::start("wedged", stop_timeout);
        let sent = Instant::now();
        child.send(Signal::TERM);
        let status = child.ends_within(PROMPTLY);
        let took = sent.elapsed();
        assert_eq!(
            status.and_then(|status| status.signal()),
            Some(Signal::KILL.as_raw()),
            "{status:?}"
        );
        assert!(took >= stop_timeout, "{took:?}");
    }

    /// The handler is Solium's alone: a program it starts has the default
    /// back, and SIGTERM still ends it.
    #[test]
    fn a_program_solium_starts_still_dies_of_sigterm() {
        let mut child = Child::start("spawns", NEVER);
        let status = child.ends_within(PROMPTLY);
        assert_eq!(
            status.and_then(|status| status.code()),
            Some(0),
            "{status:?}"
        );
    }
}
