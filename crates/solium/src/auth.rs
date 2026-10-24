//! The compositor's side of `solium-auth`, the helper that checks a password
//! for Solium's own lock screen (#281).
//!
//! **Opened once.** [`open_at_startup`] runs at the top of `main`, before any
//! thread starts, and holds the helper as an `O_PATH` descriptor for the whole
//! session. Every check starts that file, whatever has happened to the path
//! since: a package upgrade cannot swap the helper under a running session,
//! and nothing chooses another one. The helper is `<prefix>/libexec/solium-auth`
//! beside an installed `<prefix>/bin/solium`, or `solium-auth` beside the
//! binary in a build tree ([`candidates`]).
//!
//! **One process per check.** [`Helper::check`] starts the helper with
//! `posix_spawn`, a socketpair as its standard input, `/dev/null` as its
//! output, an empty environment and no other descriptor, and runs the
//! exchange in `solium_auth::protocol` on a thread of its own, so the event
//! loop never waits on PAM. The outcome arrives through a callback, exactly
//! once, unless the check is cancelled first.
//!
//! **Every failure is [`Outcome::Unavailable`], never a yes.** No PAM service
//! file, no helper, a helper that cannot start, crashes, speaks another
//! version, or does not answer within the timeout (it is killed): each says
//! why, and none unlocks. Cancelling kills the helper with `SIGKILL`.
//!
//! **The secret** stays in a [`Secret`], moved into the check and overwritten
//! when the request has been written. It is never formatted, so it cannot
//! reach a log line.
//!
//! `tests::a_right_password_is_ok_and_a_wrong_one_failed`,
//! `tests::a_helper_of_another_version_is_unavailable`,
//! `tests::a_helper_that_crashes_is_unavailable`,
//! `tests::a_helper_that_hangs_is_killed_at_the_timeout`,
//! `tests::a_missing_helper_is_unavailable`,
//! `tests::a_missing_pam_file_is_unavailable_without_starting_the_helper`,
//! `tests::a_cancelled_check_kills_the_helper_and_answers_nothing`.

use std::{
    ffi::{CString, c_char, c_int},
    io::{self, Read, Write},
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
        unix::{ffi::OsStrExt as _, fs::PermissionsExt as _, net::UnixStream},
    },
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, PoisonError},
    time::{Duration, Instant},
};

pub(crate) use solium_auth::Secret;
use solium_auth::{Answer, protocol};

/// Where PAM reads the `solium` service, in the order it looks: the
/// administrator's directory, then the distribution's.
pub(crate) const PAM_FILES: [&str; 2] = ["/etc/pam.d/solium", "/usr/lib/pam.d/solium"];

/// The helper's name, in `libexec` or beside the binary.
const NAME: &str = "solium-auth";

/// What a check came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The password is the user's. The only outcome that unlocks.
    Ok,
    /// It is not. `delay` is how long PAM's modules asked for before the next
    /// attempt.
    Failed { delay: Duration },
    /// No answer could be had, and why: never a reason to unlock.
    Unavailable { reason: String },
}

/// The helper opened at startup, or why there is none.
static HELPER: OnceLock<Result<Helper, String>> = OnceLock::new();

/// Open the helper, once, for the whole session. Called at the top of `main`,
/// before any thread starts.
pub(crate) fn open_at_startup() {
    let _ = helper();
}

/// The helper [`open_at_startup`] opened, or why it could not.
pub(crate) fn helper() -> Result<&'static Helper, &'static str> {
    HELPER
        .get_or_init(|| find(std::env::current_exe().ok().as_deref()))
        .as_ref()
        .map_err(String::as_str)
}

/// Check `secret` for `user` with the helper opened at startup. See
/// [`Helper::check`]; without a helper, `done` is called at once with why.
pub(crate) fn check(
    user: &str,
    secret: Secret,
    timeout: Duration,
    done: impl FnOnce(Outcome) + Send + 'static,
) -> Check {
    check_on(helper(), user, secret, timeout, done)
}

fn check_on(
    helper: Result<&Helper, &str>,
    user: &str,
    secret: Secret,
    timeout: Duration,
    done: impl FnOnce(Outcome) + Send + 'static,
) -> Check {
    match helper {
        Ok(helper) => helper.check(user, secret, timeout, done),
        Err(reason) => {
            done(Outcome::Unavailable {
                reason: reason.to_owned(),
            });
            Check {
                shared: Arc::default(),
            }
        }
    }
}

/// The PAM service file PAM would read, when there is one.
pub(crate) fn pam_file() -> Option<&'static Path> {
    PAM_FILES.iter().map(Path::new).find(|path| path.is_file())
}

/// Where the helper is looked for, for a Solium binary at `exe`: an installed
/// layout's `<prefix>/libexec/solium-auth` first, then `solium-auth` beside
/// the binary, as a build tree has it.
pub(crate) fn candidates(exe: &Path) -> Vec<PathBuf> {
    let Some(dir) = exe.parent() else {
        return Vec::new();
    };
    let mut places = Vec::new();
    if let Some(prefix) = dir.parent() {
        places.push(prefix.join("libexec").join(NAME));
    }
    places.push(dir.join(NAME));
    places
}

/// The first of [`candidates`] that is an executable file, opened.
fn find(exe: Option<&Path>) -> Result<Helper, String> {
    let Some(exe) = exe else {
        return Err("no solium-auth: this binary does not know where it is".to_owned());
    };
    let places = candidates(exe);
    let Some(path) = places.iter().find(|path| {
        std::fs::metadata(path)
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    }) else {
        let looked: Vec<String> = places
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        return Err(format!(
            "no solium-auth at {}, so the lock screen cannot check a password",
            looked.join(" or ")
        ));
    };
    Helper::open(path).map_err(|error| format!("{} would not open: {error}", path.display()))
}

/// The helper, held open: what every check starts.
pub(crate) struct Helper {
    /// The file, as an `O_PATH` descriptor above the ones a child is given.
    exe: Arc<OwnedFd>,
    path: PathBuf,
    /// Where PAM's service file is looked for: [`PAM_FILES`], or a test's.
    pam_files: Vec<PathBuf>,
    /// The arguments and environment the helper is started with: its name
    /// and nothing, except for a test's stand-in.
    args: Vec<CString>,
    env: Vec<CString>,
}

impl std::fmt::Debug for Helper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Helper")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Helper {
    fn open(path: &Path) -> io::Result<Self> {
        let name = CString::new(path.as_os_str().as_bytes())?;
        // SAFETY: a NUL-terminated path; the result is checked before use.
        #[expect(unsafe_code, reason = "calling libc")]
        let fd = unsafe { libc::open(name.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a descriptor just opened, owned by nothing else.
        #[expect(unsafe_code, reason = "taking ownership of a raw descriptor")]
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        Ok(Self {
            exe: Arc::new(above_stdio(&fd)?),
            path: path.to_owned(),
            pam_files: PAM_FILES.iter().map(PathBuf::from).collect(),
            args: vec![CString::from(c"solium-auth")],
            env: Vec::new(),
        })
    }

    /// Where it was found when it was opened.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Check `secret` for `user`, on a thread of its own.
    ///
    /// `done` is called once with the outcome, from that thread, unless the
    /// check is cancelled first: after [`Check::cancel`] returns, it is never
    /// called. The helper is killed when it has not answered within
    /// `timeout`, and the outcome is then [`Outcome::Unavailable`]. Dropping
    /// the [`Check`] cancels it.
    pub(crate) fn check(
        &self,
        user: &str,
        secret: Secret,
        timeout: Duration,
        done: impl FnOnce(Outcome) + Send + 'static,
    ) -> Check {
        let shared = Arc::new(Mutex::new(Shared {
            done: Some(Box::new(done)),
            ..Shared::default()
        }));
        let job = Job {
            exe: Arc::clone(&self.exe),
            pam_files: self.pam_files.clone(),
            args: self.args.clone(),
            env: self.env.clone(),
            user: user.to_owned(),
            secret,
            timeout,
            shared: Arc::clone(&shared),
        };
        let started = std::thread::Builder::new()
            .name("solium-auth".to_owned())
            .spawn(move || job.run());
        if let Err(error) = started {
            deliver(
                &shared,
                Outcome::Unavailable {
                    reason: format!("no thread for the check: {error}"),
                },
            );
        }
        Check { shared }
    }
}

/// A check under way. Dropping it cancels it.
pub(crate) struct Check {
    shared: Arc<Mutex<Shared>>,
}

impl std::fmt::Debug for Check {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Check").finish_non_exhaustive()
    }
}

impl Check {
    /// Stop it: the helper is killed with `SIGKILL` if it is running, and the
    /// outcome is never delivered.
    pub(crate) fn cancel(&self) {
        let mut shared = lock(&self.shared);
        shared.done = None;
        if let Some(process) = &shared.process {
            process.kill();
        }
    }

    /// The helper's pid, once it has started.
    #[cfg(test)]
    fn pid(&self) -> Option<libc::pid_t> {
        lock(&self.shared).pid
    }
}

impl Drop for Check {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// What the check's thread and its [`Check`] share.
#[derive(Default)]
struct Shared {
    /// Whom to tell. Taken by the one who tells, or by a cancel.
    done: Option<Box<dyn FnOnce(Outcome) + Send>>,
    /// The helper while it runs, for a cancel to kill.
    process: Option<Arc<Process>>,
    #[cfg(test)]
    pid: Option<libc::pid_t>,
}

fn lock(shared: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Tell whoever is waiting, unless the check was cancelled. Under the lock,
/// so a cancel either comes before (and nobody is told) or after.
fn deliver(shared: &Mutex<Shared>, outcome: Outcome) {
    let mut shared = lock(shared);
    if let Some(done) = shared.done.take() {
        done(outcome);
    }
}

/// One check, as its thread runs it.
struct Job {
    exe: Arc<OwnedFd>,
    pam_files: Vec<PathBuf>,
    args: Vec<CString>,
    env: Vec<CString>,
    user: String,
    secret: Secret,
    timeout: Duration,
    shared: Arc<Mutex<Shared>>,
}

impl Job {
    fn run(self) {
        let shared = Arc::clone(&self.shared);
        let outcome = self.exchange();
        deliver(&shared, outcome);
    }

    fn exchange(self) -> Outcome {
        let deadline = Instant::now() + self.timeout;
        let unavailable = |reason: String| Outcome::Unavailable { reason };
        // Without its service file PAM falls back to `other`, which on most
        // systems refuses everything: every password would look wrong.
        if !self.pam_files.iter().any(|path| path.is_file()) {
            let looked: Vec<String> = self
                .pam_files
                .iter()
                .map(|path| path.display().to_string())
                .collect();
            return unavailable(format!(
                "no PAM service file for solium ({}): install dev/pam/solium as {}",
                looked.join(" or "),
                PAM_FILES[0]
            ));
        }
        let (ours, theirs) = match UnixStream::pair() {
            Ok(pair) => pair,
            Err(error) => return unavailable(format!("no socket for solium-auth: {error}")),
        };
        let theirs = match above_stdio(&OwnedFd::from(theirs)) {
            Ok(fd) => fd,
            Err(error) => return unavailable(format!("no socket for solium-auth: {error}")),
        };
        let pid = match spawn(self.exe.as_fd(), theirs.as_fd(), &self.args, &self.env) {
            Ok(pid) => pid,
            Err(error) => return unavailable(format!("solium-auth would not start: {error}")),
        };
        drop(theirs);
        let process = match Process::new(pid) {
            Ok(process) => Arc::new(process),
            Err(error) => {
                // A kernel without pidfds. Killed by its pid, which nothing
                // can have reused: only this thread reaps it, just below.
                // SAFETY: a signal to a child not yet reaped.
                #[expect(unsafe_code, reason = "calling libc")]
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
                let _ = wait(pid);
                return unavailable(format!("no pidfd for solium-auth: {error}"));
            }
        };
        {
            let mut shared = lock(&self.shared);
            if shared.done.is_none() {
                // Cancelled before it started.
                process.kill();
            }
            shared.process = Some(Arc::clone(&process));
            #[cfg(test)]
            {
                shared.pid = Some(pid);
            }
        }

        let mut conn = Deadline {
            stream: &ours,
            until: deadline,
        };
        let asked = protocol::ask(&mut conn, &self.user, &self.secret);
        drop(self.secret);
        drop(ours);
        let status = process.reap(deadline);
        lock(&self.shared).process = None;

        match asked {
            Ok(Answer::Ok) => Outcome::Ok,
            Ok(Answer::Failed { delay_ms }) => Outcome::Failed {
                delay: Duration::from_millis(u64::from(delay_ms)),
            },
            Ok(Answer::Unavailable { reason }) => unavailable(reason),
            Err(protocol::Error::Io(error)) if error.kind() == io::ErrorKind::TimedOut => {
                unavailable(format!(
                    "solium-auth did not answer within {} ms, and was stopped",
                    self.timeout.as_millis()
                ))
            }
            Err(protocol::Error::Io(error)) => unavailable(match status {
                Ended::Signal(signal) => {
                    format!("solium-auth crashed (signal {signal}) without answering")
                }
                Ended::Exit(code) => {
                    format!("solium-auth ended (status {code}) without answering: {error}")
                }
                Ended::Unknown => format!("solium-auth stopped answering: {error}"),
            }),
            Err(protocol::Error::Version { theirs }) => unavailable(format!(
                "solium-auth speaks version {theirs} and this Solium version {}: they are from different installs",
                protocol::VERSION
            )),
            Err(protocol::Error::NotTheProtocol) => unavailable(
                "the program opened as solium-auth does not speak its protocol".to_owned(),
            ),
            Err(protocol::Error::Malformed(what)) => unavailable(format!("malformed {what}")),
        }
    }
}

/// A duplicate of `fd` numbered above every descriptor a child is given, so
/// none of the child's `dup2`s can land on it before it is read.
fn above_stdio(fd: &OwnedFd) -> io::Result<OwnedFd> {
    // SAFETY: duplicates a descriptor this process owns; checked.
    #[expect(unsafe_code, reason = "calling libc")]
    let high = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 10) };
    if high < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a descriptor just made, owned by nothing else.
    #[expect(unsafe_code, reason = "taking ownership of a raw descriptor")]
    Ok(unsafe { OwnedFd::from_raw_fd(high) })
}

/// Start the file `exe` refers to with `posix_spawn`: `socket` as its standard
/// input, `/dev/null` as its output and error, `args` and `env` as its
/// arguments and environment, every signal at its default and unblocked, a
/// session of its own, and no other descriptor.
///
/// The file is started through `/proc/self/fd/3`, the child's own copy of
/// `exe` placed there, which names the file opened at startup whatever is at
/// its path now. The helper closes that descriptor first thing.
fn spawn(
    exe: BorrowedFd<'_>,
    socket: BorrowedFd<'_>,
    args: &[CString],
    env: &[CString],
) -> io::Result<libc::pid_t> {
    fn checked(status: c_int) -> io::Result<()> {
        if status == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(status))
        }
    }
    let argv: Vec<*mut c_char> = args
        .iter()
        .map(|arg| arg.as_ptr().cast_mut())
        .chain(std::iter::once(std::ptr::null_mut()))
        .collect();
    let envp: Vec<*mut c_char> = env
        .iter()
        .map(|var| var.as_ptr().cast_mut())
        .chain(std::iter::once(std::ptr::null_mut()))
        .collect();
    // SAFETY: the actions and attributes are initialised before use and
    // destroyed after; every pointer handed over is to a NUL-terminated string
    // or a NULL-terminated array of them, alive across the call.
    #[expect(unsafe_code, reason = "calling posix_spawn")]
    unsafe {
        let mut actions: libc::posix_spawn_file_actions_t = std::mem::zeroed();
        checked(libc::posix_spawn_file_actions_init(&raw mut actions))?;
        let mut attr: libc::posix_spawnattr_t = std::mem::zeroed();
        if let Err(error) = checked(libc::posix_spawnattr_init(&raw mut attr)) {
            libc::posix_spawn_file_actions_destroy(&raw mut actions);
            return Err(error);
        }
        let spawned = (|| -> io::Result<libc::pid_t> {
            let actions = &raw mut actions;
            let attr = &raw mut attr;
            checked(libc::posix_spawn_file_actions_adddup2(
                actions,
                socket.as_raw_fd(),
                0,
            ))?;
            checked(libc::posix_spawn_file_actions_addopen(
                actions,
                1,
                c"/dev/null".as_ptr(),
                libc::O_WRONLY,
                0,
            ))?;
            checked(libc::posix_spawn_file_actions_adddup2(actions, 1, 2))?;
            checked(libc::posix_spawn_file_actions_adddup2(
                actions,
                exe.as_raw_fd(),
                3,
            ))?;
            checked(libc::posix_spawn_file_actions_addclosefrom_np(actions, 4))?;
            let mut none: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&raw mut none);
            checked(libc::posix_spawnattr_setsigmask(attr, &raw const none))?;
            let mut every: libc::sigset_t = std::mem::zeroed();
            libc::sigfillset(&raw mut every);
            checked(libc::posix_spawnattr_setsigdefault(attr, &raw const every))?;
            let flags = (libc::POSIX_SPAWN_SETSIGMASK | libc::POSIX_SPAWN_SETSIGDEF)
                as libc::c_short
                | libc::POSIX_SPAWN_SETSID;
            checked(libc::posix_spawnattr_setflags(attr, flags))?;
            let mut pid: libc::pid_t = 0;
            checked(libc::posix_spawn(
                &raw mut pid,
                c"/proc/self/fd/3".as_ptr(),
                actions,
                attr,
                argv.as_ptr(),
                envp.as_ptr(),
            ))?;
            Ok(pid)
        })();
        libc::posix_spawnattr_destroy(&raw mut attr);
        libc::posix_spawn_file_actions_destroy(&raw mut actions);
        spawned
    }
}

/// The socket, with every read and write bounded by one deadline.
struct Deadline<'a> {
    stream: &'a UnixStream,
    until: Instant,
}

impl Deadline<'_> {
    fn left(&self) -> io::Result<Duration> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            Err(io::ErrorKind::TimedOut.into())
        } else {
            Ok(left)
        }
    }
}

/// A socket timeout reads as `WouldBlock`; here it is the deadline.
fn timed_out(error: io::Error) -> io::Error {
    if error.kind() == io::ErrorKind::WouldBlock {
        io::ErrorKind::TimedOut.into()
    } else {
        error
    }
}

impl Read for Deadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(self.left()?))?;
        (&mut &*self.stream).read(buf).map_err(timed_out)
    }
}

impl Write for Deadline<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.set_write_timeout(Some(self.left()?))?;
        (&mut &*self.stream).write(buf).map_err(timed_out)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// How the helper ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    Exit(c_int),
    Signal(c_int),
    Unknown,
}

/// A started helper, through a pidfd, so a kill reaches it and nothing else,
/// even once it has ended.
struct Process {
    pid: libc::pid_t,
    pidfd: OwnedFd,
}

impl Process {
    fn new(pid: libc::pid_t) -> io::Result<Self> {
        // SAFETY: `pidfd_open` takes a pid and flags; the result is checked.
        #[expect(unsafe_code, reason = "a system call libc does not wrap")]
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        let fd = c_int::try_from(fd).map_err(|_| io::Error::other("a pidfd out of range"))?;
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a descriptor just made, owned by nothing else.
        #[expect(unsafe_code, reason = "taking ownership of a raw descriptor")]
        let pidfd = unsafe { OwnedFd::from_raw_fd(fd) };
        Ok(Self { pid, pidfd })
    }

    /// `SIGKILL`: the cancel. Reaches nothing once the helper has ended.
    fn kill(&self) {
        // SAFETY: a pidfd this process owns, a signal, no info, no flags.
        #[expect(unsafe_code, reason = "a system call libc does not wrap")]
        unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.pidfd.as_raw_fd(),
                libc::SIGKILL,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            );
        }
    }

    /// Wait for it to end until `deadline`, kill it if it has not, and reap
    /// it, so no helper outlives its check.
    fn reap(&self, deadline: Instant) -> Ended {
        let left = deadline.saturating_duration_since(Instant::now());
        let millis = c_int::try_from(left.as_millis()).unwrap_or(c_int::MAX);
        let mut ready = libc::pollfd {
            fd: self.pidfd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one pollfd, alive across the call.
        #[expect(unsafe_code, reason = "calling libc")]
        let polled = unsafe { libc::poll(&raw mut ready, 1, millis) };
        if polled != 1 {
            self.kill();
        }
        wait(self.pid)
    }
}

/// Reap `pid`, however long it takes: called only once it has ended or been
/// killed.
fn wait(pid: libc::pid_t) -> Ended {
    let mut status: c_int = 0;
    loop {
        // SAFETY: a child of this process, and a local to write its status to.
        #[expect(unsafe_code, reason = "calling libc")]
        let waited = unsafe { libc::waitpid(pid, &raw mut status, 0) };
        if waited == pid {
            break;
        }
        if waited < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return Ended::Unknown;
    }
    if libc::WIFSIGNALED(status) {
        Ended::Signal(libc::WTERMSIG(status))
    } else if libc::WIFEXITED(status) {
        Ended::Exit(libc::WEXITSTATUS(status))
    } else {
        Ended::Unknown
    }
}

#[cfg(test)]
mod tests {
    //! The checks run against a stand-in helper: this test binary again,
    //! running only [`stand_in`], which serves the protocol with a PAM-less
    //! authenticator. No test reaches real PAM or a real account.

    use std::sync::mpsc::{self, RecvTimeoutError};

    use solium_auth::Authenticator;

    use super::*;

    /// Set only for the stand-in: how it behaves.
    const STAND_IN: &str = "SOLIUM_AUTH_STAND_IN";

    /// The stand-in's authenticator: one user, one password.
    struct StandIn(String);

    impl Authenticator for StandIn {
        fn authenticate(&mut self, user: &str, secret: &Secret) -> Answer {
            match self.0.as_str() {
                "crash" => std::process::abort(),
                "hang" => loop {
                    std::thread::sleep(Duration::from_secs(60));
                },
                _ if user == "illia" && secret.expose() == b"correct horse" => Answer::Ok,
                _ => Answer::Failed { delay_ms: 1234 },
            }
        }
    }

    /// Not a test of its own: the helper the tests below start. Without
    /// `SOLIUM_AUTH_STAND_IN` it does nothing.
    #[test]
    fn stand_in() {
        let Ok(mode) = std::env::var(STAND_IN) else {
            return;
        };
        // Never a core file of this whole test binary, whatever the mode.
        // SAFETY: changes only this process.
        #[expect(unsafe_code, reason = "calling libc")]
        unsafe {
            libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
        }
        // SAFETY: standard input is the socket the check handed over, and
        // nothing else in this process uses it.
        #[expect(unsafe_code, reason = "taking ownership of a raw descriptor")]
        let mut conn = UnixStream::from(unsafe { OwnedFd::from_raw_fd(0) });
        match mode.as_str() {
            "other-version" => {
                let _ = protocol::read_hello(&mut conn);
                let _ = protocol::write_hello(&mut conn, protocol::VERSION + 1);
            }
            _ => {
                let _ = protocol::serve(&mut conn, &mut StandIn(mode));
            }
        }
        std::process::exit(0);
    }

    /// A PAM service file that exists, for the checks that should get past
    /// it.
    fn pam_file(test: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("solium-auth-pam-{test}-{}", std::process::id()));
        std::fs::write(&path, "auth required pam_deny.so\n").expect("a PAM file");
        path
    }

    /// This test binary as the helper, running the stand-in in `mode`.
    fn stand_in_helper(mode: &str, pam: PathBuf) -> Helper {
        let exe = std::env::current_exe().expect("this test binary");
        let mut helper = Helper::open(&exe).expect("this test binary opens");
        helper.pam_files = vec![pam];
        helper.args = [
            "solium-auth",
            "--exact",
            "auth::tests::stand_in",
            "--test-threads=1",
        ]
        .into_iter()
        .map(|arg| CString::new(arg).expect("an argument"))
        .collect();
        helper.env = vec![CString::new(format!("{STAND_IN}={mode}")).expect("a variable")];
        helper
    }

    /// A check whose outcome arrives on the returned channel.
    fn checked(
        helper: &Helper,
        user: &str,
        password: &str,
        timeout: Duration,
    ) -> (Check, mpsc::Receiver<Outcome>) {
        let (sender, outcome) = mpsc::channel();
        let mut secret = Secret::new();
        assert!(secret.push_str(password));
        let check = helper.check(user, secret, timeout, move |outcome| {
            let _ = sender.send(outcome);
        });
        (check, outcome)
    }

    fn unavailable(outcome: &Outcome) -> &str {
        match outcome {
            Outcome::Unavailable { reason } => reason,
            other => panic!("not unavailable: {other:?}"),
        }
    }

    /// Whether `pid` is gone, reaped and all: a zombie still answers a kill.
    fn reaped(pid: libc::pid_t) -> bool {
        // SAFETY: signal 0 only asks whether the process exists.
        #[expect(unsafe_code, reason = "calling libc")]
        let exists = unsafe { libc::kill(pid, 0) } == 0;
        !exists
    }

    const LONG: Duration = Duration::from_secs(30);

    #[test]
    fn a_right_password_is_ok_and_a_wrong_one_failed() {
        let helper = stand_in_helper("answer", pam_file("answer"));
        let (_check, outcome) = checked(&helper, "illia", "correct horse", LONG);
        assert_eq!(outcome.recv_timeout(LONG).expect("an outcome"), Outcome::Ok);

        let (_check, outcome) = checked(&helper, "illia", "correct horsE", LONG);
        let outcome = outcome.recv_timeout(LONG).expect("an outcome");
        assert!(
            matches!(outcome, Outcome::Failed { delay } if delay == Duration::from_millis(1234)),
            "{outcome:?}"
        );
    }

    #[test]
    fn a_helper_of_another_version_is_unavailable() {
        let helper = stand_in_helper("other-version", pam_file("version"));
        let (_check, outcome) = checked(&helper, "illia", "correct horse", LONG);
        let outcome = outcome.recv_timeout(LONG).expect("an outcome");
        assert!(
            unavailable(&outcome).contains("different installs"),
            "{outcome:?}"
        );
    }

    #[test]
    fn a_helper_that_crashes_is_unavailable() {
        let helper = stand_in_helper("crash", pam_file("crash"));
        let (check, outcome) = checked(&helper, "illia", "correct horse", LONG);
        let outcome = outcome.recv_timeout(LONG).expect("an outcome");
        assert!(
            unavailable(&outcome).contains(&format!("signal {}", libc::SIGABRT)),
            "{outcome:?}"
        );
        assert!(reaped(check.pid().expect("it started")));
    }

    #[test]
    fn a_helper_that_hangs_is_killed_at_the_timeout() {
        let helper = stand_in_helper("hang", pam_file("hang"));
        let started = Instant::now();
        let timeout = Duration::from_millis(500);
        let (check, outcome) = checked(&helper, "illia", "correct horse", timeout);
        let outcome = outcome.recv_timeout(LONG).expect("an outcome");
        assert!(
            unavailable(&outcome).contains("did not answer within 500 ms"),
            "{outcome:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        assert!(reaped(check.pid().expect("it started")));
    }

    #[test]
    fn a_missing_helper_is_unavailable() {
        // None was found at startup.
        let found = find(Some(Path::new("/nonexistent/bin/solium")));
        let Err(reason) = found else {
            panic!("a helper in /nonexistent");
        };
        assert!(
            reason.contains("/nonexistent/libexec/solium-auth"),
            "{reason}"
        );
        let (sender, outcome) = mpsc::channel();
        let _check = check_on(
            Err(reason.as_str()),
            "illia",
            Secret::new(),
            LONG,
            move |outcome| {
                let _ = sender.send(outcome);
            },
        );
        let outcome = outcome.recv_timeout(LONG).expect("an outcome");
        assert_eq!(unavailable(&outcome), reason);

        // One found, but not a program.
        let text = pam_file("not-a-program");
        let mut helper = Helper::open(&text).expect("it opens");
        helper.pam_files = vec![text];
        let (_check, outcome) = checked(&helper, "illia", "correct horse", LONG);
        let outcome = outcome.recv_timeout(LONG).expect("an outcome");
        assert!(
            unavailable(&outcome).contains("would not start"),
            "{outcome:?}"
        );
    }

    /// Checked before anything starts: a stand-in that would say yes is never
    /// asked.
    #[test]
    fn a_missing_pam_file_is_unavailable_without_starting_the_helper() {
        let helper = stand_in_helper("answer", PathBuf::from("/nonexistent/pam.d/solium"));
        let (check, outcome) = checked(&helper, "illia", "correct horse", LONG);
        let outcome = outcome.recv_timeout(LONG).expect("an outcome");
        assert!(
            unavailable(&outcome).contains("install dev/pam/solium as /etc/pam.d/solium"),
            "{outcome:?}"
        );
        assert_eq!(check.pid(), None);
    }

    #[test]
    fn a_cancelled_check_kills_the_helper_and_answers_nothing() {
        let helper = stand_in_helper("hang", pam_file("cancel"));
        let (check, outcome) = checked(&helper, "illia", "correct horse", LONG);
        let started = Instant::now();
        let pid = loop {
            if let Some(pid) = check.pid() {
                break pid;
            }
            assert!(started.elapsed() < LONG, "the helper never started");
            std::thread::sleep(Duration::from_millis(10));
        };
        check.cancel();
        // Never told: the callback, and its sender, were dropped.
        assert_eq!(
            outcome.recv_timeout(LONG),
            Err(RecvTimeoutError::Disconnected)
        );
        while !reaped(pid) {
            assert!(started.elapsed() < LONG, "the helper was never reaped");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The test binary lives in `target/<profile>/deps`, where no
    /// `solium-auth` is ever built: the helper opened at startup is missing,
    /// and a check through it says so instead of answering.
    #[test]
    fn without_a_helper_from_startup_a_check_is_unavailable() {
        let Err(reason) = helper() else {
            panic!("a solium-auth beside the test binary");
        };
        let (sender, outcome) = mpsc::channel();
        let _check = check("illia", Secret::new(), LONG, move |outcome| {
            let _ = sender.send(outcome);
        });
        let outcome = outcome.recv_timeout(LONG).expect("an outcome");
        assert_eq!(unavailable(&outcome), reason);
    }

    #[test]
    fn the_helper_is_looked_for_in_libexec_then_beside_the_binary() {
        assert_eq!(
            candidates(Path::new("/usr/bin/solium")),
            [
                PathBuf::from("/usr/libexec/solium-auth"),
                PathBuf::from("/usr/bin/solium-auth")
            ]
        );
        let root = std::env::temp_dir().join(format!("solium-auth-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("bin")).expect("bin");
        std::fs::create_dir_all(root.join("libexec")).expect("libexec");
        let program = |path: &Path, mode: u32| {
            std::fs::write(path, "#!/bin/sh\n").expect("a file");
            std::fs::set_permissions(
                path,
                <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(mode),
            )
            .expect("its mode");
        };
        let exe = root.join("bin/solium");
        // Beside the binary only, as in a build tree.
        program(&root.join("bin/solium-auth"), 0o755);
        assert_eq!(
            find(Some(exe.as_path())).expect("found").path(),
            root.join("bin/solium-auth")
        );
        // A file in libexec that cannot be run is passed over.
        program(&root.join("libexec/solium-auth"), 0o644);
        assert_eq!(
            find(Some(exe.as_path())).expect("found").path(),
            root.join("bin/solium-auth")
        );
        // One that can comes first.
        program(&root.join("libexec/solium-auth"), 0o755);
        assert_eq!(
            find(Some(exe.as_path())).expect("found").path(),
            root.join("libexec/solium-auth")
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
