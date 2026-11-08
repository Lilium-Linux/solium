//! Programs a configuration runs and reads back: the table form of `sol.run`
//! (#472).
//!
//! A child is started as every program Solium starts is
//! (`Solium::client_process`: the environment Solium was started with, no
//! descriptor beyond stdio, a session of its own), with stdin on `/dev/null`
//! and stdout and stderr piped back. The pipes are non-blocking and read once
//! each time round the event loop, in [`settle`], as the services are: both
//! loops wake at least every 16 ms, so nothing here ever waits for a child,
//! and a line waits no longer than that to reach Lua.
//!
//! What a child may cost is bounded. Each stream keeps at most `max_output`
//! bytes: past that the pipe is closed, so a child that goes on writing gets
//! `SIGPIPE` as one piped into `head` would, and its result says `truncated`.
//! At `timeout` its whole process group is killed. A child is reaped by the
//! same once-a-loop check that reads it, and one killed by a reload is kept
//! until it has been reaped, so none is left a zombie.

use std::{
    fs::File,
    io::Read as _,
    os::{fd::OwnedFd, unix::process::ExitStatusExt as _},
    process::{Child, ExitStatus, Stdio},
    time::Duration,
};

use crate::state::Solium;

/// How long a child may run when its options do not say, in milliseconds.
pub(crate) const TIMEOUT_MS: u64 = 30_000;
/// How much of each stream is kept when its options do not say, in bytes.
pub(crate) const MAX_OUTPUT: usize = 1 << 20;
/// How much is read from a pipe at a time.
const CHUNK: usize = 64 * 1024;

/// What `sol.run({ argv }, options)` asked for.
#[derive(Clone, Debug)]
pub(crate) struct Request {
    /// The run's id, which its functions are kept under in `sol._runs`.
    pub(crate) id: u64,
    /// The program, then its arguments.
    pub(crate) argv: Vec<String>,
    pub(crate) cwd: Option<String>,
    /// Variables set on top of the child's environment, or removed from it
    /// (`None`).
    pub(crate) env: Vec<(String, Option<String>)>,
    pub(crate) timeout: Duration,
    /// Bytes kept of each stream.
    pub(crate) max_output: usize,
    /// Whether each line is told as it arrives: only when `lines` was given.
    pub(crate) lines: bool,
}

/// Which of a child's streams a line came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stream {
    Stdout,
    Stderr,
}

/// How a child ended, for its `done`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Ended {
    /// Its exit code, when it exited.
    pub(crate) code: Option<i32>,
    /// The signal that ended it, when one did.
    pub(crate) signal: Option<i32>,
    /// It was killed at its timeout.
    pub(crate) timed_out: bool,
    /// A stream wrote past `max_output`.
    pub(crate) truncated: bool,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    /// Why it could not be started, when it could not.
    pub(crate) error: Option<String>,
}

/// What [`Runs::pump`] heard, in the order it happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Heard {
    /// A whole line, without its `\n`; or the last, unterminated one, at the
    /// end.
    Line {
        id: u64,
        stream: Stream,
        line: Vec<u8>,
    },
    /// The child ended, or never started.
    Done { id: u64, ended: Ended },
}

/// One stream of a running child: its pipe until it ends or is closed, and
/// what was read from it.
#[derive(Debug)]
struct Capture {
    pipe: Option<File>,
    text: Vec<u8>,
    /// Where the next line starts: everything before it was told.
    told: usize,
    /// How far `text` has been searched for the end of that line.
    searched: usize,
    truncated: bool,
}

impl Capture {
    fn new(pipe: Option<File>) -> Self {
        Self {
            pipe,
            text: Vec::new(),
            told: 0,
            searched: 0,
            truncated: false,
        }
    }

    /// Read what is waiting, up to `max` bytes kept in all. The pipe is let
    /// go at its end, on an error, and once more than `max` was written.
    fn read(&mut self, max: usize, buffer: &mut [u8]) {
        while let Some(pipe) = self.pipe.as_mut() {
            match pipe.read(buffer) {
                Ok(0) => self.pipe = None,
                Ok(read) => {
                    let room = max.saturating_sub(self.text.len());
                    let kept = read.min(room);
                    self.text.extend_from_slice(&buffer[..kept]);
                    if kept < read {
                        self.truncated = true;
                        self.pipe = None;
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(err) => {
                    tracing::debug!(%err, "a run's pipe could not be read");
                    self.pipe = None;
                }
            }
        }
    }

    /// The whole lines read and not yet told, and at the `end` what is left
    /// after the last one.
    fn lines(&mut self, end: bool) -> Vec<Vec<u8>> {
        let mut lines = Vec::new();
        while let Some(at) = self.text[self.searched..].iter().position(|&b| b == b'\n') {
            let stop = self.searched + at;
            lines.push(self.text[self.told..stop].to_vec());
            self.told = stop + 1;
            self.searched = self.told;
        }
        self.searched = self.text.len();
        if end && self.told < self.text.len() {
            lines.push(self.text[self.told..].to_vec());
            self.told = self.text.len();
        }
        lines
    }
}

/// A child `sol.run` started and has not seen end.
#[derive(Debug)]
struct Running {
    id: u64,
    child: Child,
    /// When, on the compositor's clock, it is killed.
    deadline: Duration,
    max_output: usize,
    lines: bool,
    timed_out: bool,
    stdout: Capture,
    stderr: Capture,
}

impl Running {
    fn read(&mut self, buffer: &mut [u8]) {
        self.stdout.read(self.max_output, buffer);
        self.stderr.read(self.max_output, buffer);
    }

    /// The lines read since the last time, stdout's then stderr's, when
    /// lines were asked for.
    fn tell_lines(&mut self, end: bool, heard: &mut Vec<Heard>) {
        if !self.lines {
            return;
        }
        let id = self.id;
        for (stream, capture) in [
            (Stream::Stdout, &mut self.stdout),
            (Stream::Stderr, &mut self.stderr),
        ] {
            heard.extend(capture.lines(end).into_iter().map(|line| Heard::Line {
                id,
                stream,
                line,
            }));
        }
    }

    fn ended(self, status: ExitStatus) -> Ended {
        Ended {
            code: status.code(),
            signal: status.signal(),
            timed_out: self.timed_out,
            truncated: self.stdout.truncated || self.stderr.truncated,
            stdout: self.stdout.text,
            stderr: self.stderr.text,
            error: None,
        }
    }
}

/// Every child the configuration's `sol.run{}` started, until it is reaped.
#[derive(Debug, Default)]
pub(crate) struct Runs {
    running: Vec<Running>,
    /// Children killed by a reload, kept until they are reaped.
    dying: Vec<Child>,
    /// What was heard outside [`Self::pump`]: a child that could not start.
    heard: Vec<Heard>,
    buffer: Vec<u8>,
}

impl Runs {
    /// Start `process`, the child `request` names, at `now`.
    pub(crate) fn start(
        &mut self,
        request: Request,
        mut process: std::process::Command,
        now: Duration,
    ) {
        process
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &request.cwd {
            process.current_dir(cwd);
        }
        for (name, value) in &request.env {
            match value {
                Some(value) => process.env(name, value),
                None => process.env_remove(name),
            };
        }
        let program = request.argv.first().cloned().unwrap_or_default();
        let mut child = match process.spawn() {
            Ok(child) => child,
            Err(err) => {
                tracing::warn!(%err, program, "could not run");
                self.heard.push(Heard::Done {
                    id: request.id,
                    ended: Ended {
                        error: Some(err.to_string()),
                        ..Ended::default()
                    },
                });
                return;
            }
        };
        let stdout = child.stdout.take().map(OwnedFd::from);
        let stderr = child.stderr.take().map(OwnedFd::from);
        let (stdout, stderr) = match (non_blocking(stdout), non_blocking(stderr)) {
            (Ok(stdout), Ok(stderr)) => (stdout, stderr),
            (Err(err), _) | (_, Err(err)) => {
                // A blocking pipe would stall the loop on its first read, so
                // the child goes rather than be read that way.
                tracing::warn!(%err, program, "could not read from a run");
                kill_group(&child);
                self.dying.push(child);
                self.heard.push(Heard::Done {
                    id: request.id,
                    ended: Ended {
                        error: Some(err.to_string()),
                        ..Ended::default()
                    },
                });
                return;
            }
        };
        tracing::debug!(program, pid = child.id(), "ran, reading its output");
        self.running.push(Running {
            id: request.id,
            child,
            deadline: now.saturating_add(request.timeout),
            max_output: request.max_output,
            lines: request.lines,
            timed_out: false,
            stdout: Capture::new(stdout),
            stderr: Capture::new(stderr),
        });
    }

    /// Whether there is nothing to read, reap or tell.
    pub(crate) fn is_idle(&self) -> bool {
        self.running.is_empty() && self.dying.is_empty() && self.heard.is_empty()
    }

    /// Read every child, kill the ones past their deadline, and reap the
    /// ones that ended: what was heard, in order.
    pub(crate) fn pump(&mut self, now: Duration) -> Vec<Heard> {
        let mut heard = std::mem::take(&mut self.heard);
        self.dying
            .retain_mut(|child| matches!(child.try_wait(), Ok(None)));
        if self.buffer.len() != CHUNK {
            self.buffer = vec![0; CHUNK];
        }
        let mut index = 0;
        while let Some(run) = self.running.get_mut(index) {
            run.read(&mut self.buffer);
            run.tell_lines(false, &mut heard);
            if !run.timed_out && now >= run.deadline {
                tracing::debug!(pid = run.child.id(), "a run reached its timeout");
                kill_group(&run.child);
                run.timed_out = true;
            }
            match run.child.try_wait() {
                Ok(None) => index += 1,
                Ok(Some(status)) => {
                    // What it wrote before it exited is all in the pipes now.
                    // What is left of a line at the end is told, and then
                    // the pipes go, held open or not by anything it started.
                    let mut run = self.running.remove(index);
                    run.read(&mut self.buffer);
                    run.tell_lines(true, &mut heard);
                    let id = run.id;
                    heard.push(Heard::Done {
                        id,
                        ended: run.ended(status),
                    });
                }
                Err(err) => {
                    let run = self.running.remove(index);
                    tracing::warn!(%err, pid = run.child.id(), "could not wait for a run");
                    kill_group(&run.child);
                    let id = run.id;
                    self.dying.push(run.child);
                    heard.push(Heard::Done {
                        id,
                        ended: Ended {
                            error: Some(err.to_string()),
                            ..Ended::default()
                        },
                    });
                }
            }
        }
        heard
    }

    /// Kill every child, for a reload: none of them is told anything again,
    /// and each is reaped once it has gone.
    pub(crate) fn stop_all(&mut self) {
        for run in self.running.drain(..) {
            kill_group(&run.child);
            self.dying.push(run.child);
        }
        self.heard.clear();
    }

    /// How many children are running, for a test.
    #[cfg(test)]
    pub(crate) fn running(&self) -> usize {
        self.running.len()
    }

    /// How many killed children are not reaped yet, for a test.
    #[cfg(test)]
    pub(crate) fn dying(&self) -> usize {
        self.dying.len()
    }
}

impl Drop for Runs {
    /// The compositor going takes its children with it, and leaves a thread
    /// to reap each, which a child killed outright does not keep waiting.
    fn drop(&mut self) {
        let children = self
            .running
            .drain(..)
            .map(|run| run.child)
            .chain(self.dying.drain(..));
        for mut child in children {
            kill_group(&child);
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

/// A pipe Solium reads without waiting.
fn non_blocking(fd: Option<OwnedFd>) -> std::io::Result<Option<File>> {
    use std::os::fd::AsRawFd as _;
    let Some(fd) = fd else {
        return Ok(None);
    };
    let raw = fd.as_raw_fd();
    // SAFETY: `raw` is a descriptor this function owns for the whole call,
    // and these two calls take no pointers.
    #[expect(unsafe_code, reason = "calling libc")]
    let set = unsafe {
        let flags = libc::fcntl(raw, libc::F_GETFL);
        flags != -1 && libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK) != -1
    };
    if set {
        Ok(Some(File::from(fd)))
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Kill a child's whole process group: the child leads one of its own
/// (`launch::command`), so what it started goes with it.
fn kill_group(child: &Child) {
    let Ok(pid) = libc::pid_t::try_from(child.id()) else {
        return;
    };
    // SAFETY: takes no pointers. The child is not reaped yet, so its id, and
    // the id of the group it leads, cannot have been given to anyone else.
    #[expect(unsafe_code, reason = "calling libc")]
    let _ = unsafe { libc::kill(-pid, libc::SIGKILL) };
}

/// Once a loop: read the children, and tell the configuration what they said
/// and how they ended.
pub(crate) fn settle(state: &mut Solium) {
    if state.runs.is_idle() {
        return;
    }
    let heard = state.runs.pump(state.clock.now());
    if heard.is_empty() {
        return;
    }
    let snapshot = state.snapshot();
    let Some(mut scripts) = state.scripts.take() else {
        return;
    };
    let outcome = scripts.runs_heard(&heard, snapshot);
    state.scripts = Some(scripts);
    state.apply(outcome);
}

#[cfg(test)]
mod tests {
    //! The children are `sh -c` scripts, started by `launch::command` as the
    //! compositor starts them, and read the way `settle` reads them.

    use std::time::Instant;

    use super::*;

    fn request(id: u64, script: &str) -> Request {
        Request {
            id,
            argv: vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()],
            cwd: None,
            env: Vec::new(),
            timeout: Duration::from_millis(TIMEOUT_MS),
            max_output: MAX_OUTPUT,
            lines: true,
        }
    }

    fn command(request: &Request) -> std::process::Command {
        let mut process = crate::launch::command(&request.argv[0]);
        process.args(&request.argv[1..]);
        process
    }

    /// Everything heard until each of `count` runs is done, on a clock
    /// starting at zero and moved on by `step` a pump.
    fn heard_until_done(runs: &mut Runs, count: usize, step: Duration) -> Vec<Heard> {
        let started = Instant::now();
        let mut now = Duration::ZERO;
        let mut heard = Vec::new();
        while heard
            .iter()
            .filter(|each| matches!(each, Heard::Done { .. }))
            .count()
            < count
        {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "the runs never ended: {heard:?}"
            );
            heard.extend(runs.pump(now));
            now += step;
            std::thread::sleep(Duration::from_millis(5));
        }
        heard
    }

    fn started(runs: &mut Runs, request: Request) {
        let process = command(&request);
        runs.start(request, process, Duration::ZERO);
    }

    fn end_of(heard: &[Heard]) -> Ended {
        heard
            .iter()
            .find_map(|each| match each {
                Heard::Done { ended, .. } => Some(ended.clone()),
                Heard::Line { .. } => None,
            })
            .unwrap_or_default()
    }

    fn lines(heard: &[Heard]) -> Vec<(Stream, String)> {
        heard
            .iter()
            .filter_map(|each| match each {
                Heard::Line { stream, line, .. } => {
                    Some((*stream, String::from_utf8_lossy(line).into_owned()))
                }
                Heard::Done { .. } => None,
            })
            .collect()
    }

    /// **A run's output and exit code come back**, each stream on its own.
    #[test]
    fn a_runs_output_and_exit_code_come_back() {
        let mut runs = Runs::default();
        started(&mut runs, request(1, "echo out; echo err >&2; exit 3"));
        let heard = heard_until_done(&mut runs, 1, Duration::ZERO);
        let ended = end_of(&heard);
        assert_eq!(
            (ended.code, ended.signal, ended.timed_out, ended.truncated),
            (Some(3), None, false, false)
        );
        assert_eq!(ended.stdout, b"out\n");
        assert_eq!(ended.stderr, b"err\n");
        assert!(runs.is_idle(), "the run was kept after it was told");
    }

    /// **A line written in pieces is told once, whole**, and the last
    /// unterminated line at the end.
    #[test]
    fn a_line_split_across_reads_is_told_once_whole() {
        let mut runs = Runs::default();
        started(
            &mut runs,
            request(
                2,
                "printf 'one\\ntw'; sleep 0.2; printf 'o\\nthr'; sleep 0.2; printf 'ee'",
            ),
        );
        let heard = heard_until_done(&mut runs, 1, Duration::ZERO);
        assert_eq!(
            lines(&heard),
            [
                (Stream::Stdout, "one".to_owned()),
                (Stream::Stdout, "two".to_owned()),
                (Stream::Stdout, "three".to_owned()),
            ]
        );
        assert_eq!(end_of(&heard).stdout, b"one\ntwo\nthree");
        assert!(
            matches!(heard.last(), Some(Heard::Done { .. })),
            "a line was told after done: {heard:?}"
        );
    }

    /// **No line is told unless lines were asked for**; the output still is.
    #[test]
    fn no_line_is_told_unless_asked_for() {
        let mut runs = Runs::default();
        let mut quiet = request(3, "echo a; echo b");
        quiet.lines = false;
        started(&mut runs, quiet);
        let heard = heard_until_done(&mut runs, 1, Duration::ZERO);
        assert_eq!(lines(&heard), []);
        assert_eq!(end_of(&heard).stdout, b"a\nb\n");
    }

    /// **A child ended by a signal says which**, with no code.
    #[test]
    fn a_child_ended_by_a_signal_says_which() {
        let mut runs = Runs::default();
        started(&mut runs, request(4, "kill -TERM $$"));
        let ended = end_of(&heard_until_done(&mut runs, 1, Duration::ZERO));
        assert_eq!((ended.code, ended.signal), (None, Some(libc::SIGTERM)));
        assert!(!ended.timed_out);
    }

    /// **At its timeout a child is killed with what it started**, and says
    /// so; before it, it is left alone.
    #[test]
    fn at_its_timeout_a_child_and_its_group_are_killed() {
        let directory =
            std::env::temp_dir().join(format!("solium-runs-timeout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a directory");
        let marker = directory.join("grandchild");
        let mut runs = Runs::default();
        let mut slow = request(
            5,
            &format!(
                "(sleep 1; touch '{}') & echo started; sleep 30",
                marker.display()
            ),
        );
        slow.timeout = Duration::from_millis(500);
        started(&mut runs, slow);
        // Under its timeout, on a clock that has not reached it, it runs.
        let early = runs.pump(Duration::from_millis(100));
        assert!(
            !early.iter().any(|each| matches!(each, Heard::Done { .. })),
            "ended before its timeout: {early:?}"
        );
        let heard = heard_until_done(&mut runs, 1, Duration::from_millis(100));
        let ended = end_of(&heard);
        assert!(ended.timed_out, "{ended:?}");
        assert_eq!((ended.code, ended.signal), (None, Some(libc::SIGKILL)));
        assert_eq!(ended.stdout, b"started\n");
        std::thread::sleep(Duration::from_millis(1500));
        assert!(
            !marker.exists(),
            "what the child started outlived the timeout"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **Past `max_output` reading stops and the result is truncated**, on
    /// each stream on its own; the child is not kept waiting on a full pipe.
    #[test]
    fn past_max_output_reading_stops_and_the_result_is_truncated() {
        let mut runs = Runs::default();
        let mut loud = request(6, "echo short >&2; yes 0123456789");
        loud.max_output = 100;
        started(&mut runs, loud);
        let heard = heard_until_done(&mut runs, 1, Duration::ZERO);
        let ended = end_of(&heard);
        assert!(ended.truncated);
        assert!(!ended.timed_out, "the child waited for its timeout");
        assert_eq!(ended.stdout.len(), 100);
        assert!(ended.stdout.starts_with(b"0123456789\n0123456789\n"));
        assert_eq!(ended.stderr, b"short\n");
        // Nine whole lines, and the tenth as far as it got, at the end.
        let told = lines(&heard);
        assert_eq!(
            told.iter().filter(|(s, _)| *s == Stream::Stdout).count(),
            10
        );
        assert_eq!(told.last().map(|(_, line)| line.as_str()), Some("0"));

        // And at the cap exactly, nothing is cut.
        let mut exact = request(7, "printf 0123456789");
        exact.max_output = 10;
        started(&mut runs, exact);
        let ended = end_of(&heard_until_done(&mut runs, 1, Duration::ZERO));
        assert!(!ended.truncated, "{ended:?}");
        assert_eq!(ended.stdout, b"0123456789");
    }

    /// **A program that cannot start says why**, at once, with no code.
    #[test]
    fn a_program_that_cannot_start_says_why() {
        let mut runs = Runs::default();
        let mut missing = request(8, "");
        missing.argv = vec!["/nonexistent/solium-no-such-program".to_owned()];
        started(&mut runs, missing);
        assert!(!runs.is_idle(), "the failure was not kept to be told");
        let heard = runs.pump(Duration::ZERO);
        let ended = end_of(&heard);
        assert_eq!(ended.code, None);
        assert!(ended.error.is_some(), "{heard:?}");
        assert!(runs.is_idle());
    }

    /// **`cwd` and `env` reach the child**, a variable set and one removed;
    /// stdin is empty.
    #[test]
    fn cwd_and_env_reach_the_child() {
        let directory = std::env::temp_dir();
        let mut runs = Runs::default();
        let mut asked = request(9, "pwd; echo \"$SOLIUM_RUN_SET-${HOME-unset}\"; cat");
        asked.cwd = Some(directory.display().to_string());
        asked.env = vec![
            ("SOLIUM_RUN_SET".to_owned(), Some("yes".to_owned())),
            ("HOME".to_owned(), None),
        ];
        started(&mut runs, asked);
        let ended = end_of(&heard_until_done(&mut runs, 1, Duration::ZERO));
        let canonical = directory.canonicalize().expect("the temporary directory");
        assert_eq!(
            String::from_utf8_lossy(&ended.stdout),
            format!("{}\nyes-unset\n", canonical.display())
        );
    }

    /// **Stopping every run kills each and reaps it**: no zombie is left, and
    /// nothing is told of them again.
    #[test]
    fn stopping_every_run_kills_and_reaps_each() {
        let mut runs = Runs::default();
        started(&mut runs, request(10, "sleep 30"));
        started(&mut runs, request(11, "sleep 30"));
        let pids: Vec<u32> = runs.running.iter().map(|run| run.child.id()).collect();
        runs.stop_all();
        assert_eq!(runs.running(), 0);
        let started = Instant::now();
        while runs.dying() > 0 {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "a killed run was never reaped"
            );
            assert_eq!(runs.pump(Duration::ZERO), [], "a stopped run was told");
            std::thread::sleep(Duration::from_millis(10));
        }
        for pid in pids {
            assert!(
                !std::path::Path::new(&format!("/proc/{pid}")).exists(),
                "{pid} is still there"
            );
        }
        assert!(runs.is_idle());
    }
}
