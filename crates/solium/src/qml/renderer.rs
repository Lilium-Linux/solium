//! Which scene graph QML renders on, decided once per process before Qt starts.
//!
//! Three modes: `auto` (the default), `gpu` and `software`. `auto` on the
//! hardware backend runs the GPU pre-flight in a child process first, because
//! Qt fixes its scene graph for the life of whichever process starts it: a
//! pre-flight that fails inside the compositor leaves no software path to go
//! back to, and one that fails in a child costs only the child. See
//! `a_failed_probe_means_software` and `a_probe_that_hangs_is_killed_and_timed_out`.
//!
//! A passing probe is not the compositor's own start passing, and that start
//! cannot fall back when it fails. So `auto` records how it went, and the next
//! start of the same build takes software rather than repeat it. See
//! [`Markers`] and `a_gpu_start_that_never_finished_means_software_next_time`.

use std::{
    io::Write as _,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{OnceLock, mpsc},
    time::{Duration, Instant},
};

/// The hidden subcommand the probe's child is run as.
pub(crate) const PROBE: &str = "--probe-qml-gpu";

/// How long the probe may take before it is killed and QML renders in software.
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// How often a running probe is asked whether it has finished.
const POLL: Duration = Duration::from_millis(5);

/// How QML is asked to render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Auto,
    Gpu,
    Software,
}

impl Mode {
    /// The mode `name` spells, ignoring case and surrounding space.
    pub(crate) fn named(name: &str) -> Option<Self> {
        let name = name.trim();
        [
            ("auto", Self::Auto),
            ("gpu", Self::Gpu),
            ("software", Self::Software),
        ]
        .into_iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(name))
        .map(|(_, mode)| mode)
    }
}

/// Where a mode was asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    Flag,
    Environment,
    Alias,
    Config,
    Default,
}

impl Source {
    fn describe(self) -> &'static str {
        match self {
            Self::Flag => "--qml",
            Self::Environment => "SOLIUM_QML",
            Self::Alias => "SOLIUM_QML_GPU",
            Self::Config => "qml.renderer",
            Self::Default => "the default",
        }
    }
}

/// Everywhere a mode can be asked for.
#[derive(Clone, Debug, Default)]
pub(crate) struct Asked {
    /// `--qml <mode>` or `--qml=<mode>`.
    pub(crate) flag: Option<String>,
    /// `SOLIUM_QML`.
    pub(crate) environment: Option<String>,
    /// Whether `SOLIUM_QML_GPU` is set, to anything.
    pub(crate) alias: bool,
    /// `qml.renderer` in the configuration.
    pub(crate) config: Option<String>,
}

/// The mode asked for, and where: `--qml`, then `SOLIUM_QML`, then
/// `SOLIUM_QML_GPU`, then `qml.renderer`, then `auto`.
///
/// The first source that says anything decides, and a value that is not a mode
/// warns and means `auto` there rather than passing the question down. See
/// `the_flag_beats_the_environment_beats_the_config` and
/// `an_unknown_mode_warns_and_means_auto`.
pub(crate) fn resolve(asked: &Asked) -> (Mode, Source) {
    let said = [
        (Source::Flag, asked.flag.as_deref()),
        (Source::Environment, asked.environment.as_deref()),
        (Source::Alias, asked.alias.then_some("gpu")),
        (Source::Config, asked.config.as_deref()),
    ];
    for (source, value) in said {
        let Some(value) = value else {
            continue;
        };
        return match Mode::named(value) {
            Some(mode) => (mode, source),
            None => {
                tracing::warn!(
                    value,
                    from = source.describe(),
                    "not a QML renderer: one of auto, gpu or software. Using auto"
                );
                (Mode::Auto, source)
            }
        };
    }
    (Mode::Auto, Source::Default)
}

/// The value of `--qml <mode>` or `--qml=<mode>`, first one wins.
///
/// A `--qml` with nothing after it is an empty value, which is not a mode. See
/// `the_flag_is_read_in_either_spelling`.
pub(crate) fn flag(arguments: impl IntoIterator<Item = String>) -> Option<String> {
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        if argument == "--qml" {
            return Some(arguments.next().unwrap_or_default());
        }
        if let Some(value) = argument.strip_prefix("--qml=") {
            return Some(value.to_owned());
        }
    }
    None
}

/// What the configuration said, through `sol.qml`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Configured {
    /// `qml.renderer`.
    pub(crate) renderer: Option<String>,
    /// `qml.probe_timeout`, in milliseconds there.
    pub(crate) probe_timeout: Option<Duration>,
}

/// Which entry point is starting Qt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Entry {
    Tty,
    Nested,
    /// `solium --check-qml`, which loads one file and exits.
    CheckQml,
}

/// The scene graph Qt is started on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Renderer {
    Gpu,
    Software,
}

/// What the probe's child came back with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Probed {
    /// Exit 0, and the line it printed.
    Passed(String),
    /// Anything else, and why.
    Failed(String),
    /// Killed after this long.
    TimedOut(Duration),
}

/// A GPU start in the compositor, by this build, that did not work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Previous {
    /// The file that says so. Deleting it lets `auto` try the GPU again.
    pub(crate) file: PathBuf,
    /// Why, as it was recorded.
    pub(crate) reason: String,
}

/// The renderer for `entry` given the mode asked for, asking `previous` and
/// running `probe` only for `auto` on the hardware.
///
/// `--check-qml` is software in every mode and never probes: it answers whether
/// a file parses, and that must not depend on the machine. Nested `auto` is
/// software, because winit hands the compositor no GBM device to allocate
/// scene buffers from. `auto` on the hardware is software without a probe when
/// this build's last GPU start in the compositor did not work, and `gpu` never
/// asks. Says what it chose in one line starting `QML renderer:`; a GPU start
/// in the compositor that then fails says so in another. See
/// `check_qml_is_software_in_every_mode`, `auto_is_software_nested`,
/// `a_passing_probe_means_gpu`, `a_failed_probe_means_software`,
/// `a_timed_out_probe_means_software`,
/// `a_gpu_start_that_never_finished_means_software_next_time` and
/// `a_forced_mode_is_taken_without_a_probe`.
pub(crate) fn choose(
    entry: Entry,
    (mode, source): (Mode, Source),
    previous: impl FnOnce() -> Option<Previous>,
    probe: impl FnOnce() -> Probed,
) -> Renderer {
    let from = source.describe();
    match (entry, mode) {
        (Entry::CheckQml, _) => Renderer::Software,
        (_, Mode::Software) => {
            tracing::info!(from, "QML renderer: software, as {from} asked");
            Renderer::Software
        }
        (Entry::Tty, Mode::Gpu) => {
            tracing::info!(from, "QML renderer: gpu, as {from} asked; no probe");
            Renderer::Gpu
        }
        (Entry::Nested, Mode::Gpu) => {
            tracing::info!(
                from,
                "QML renderer: gpu, as {from} asked. Nested there is no GBM device, so QML \
                 scenes have no buffer to render into and none will draw"
            );
            Renderer::Gpu
        }
        (Entry::Nested, Mode::Auto) => {
            tracing::info!(
                from,
                "QML renderer: software, because nested has no GBM device for the GPU path"
            );
            Renderer::Software
        }
        (Entry::Tty, Mode::Auto) => {
            if let Some(Previous { file, reason }) = previous() {
                tracing::warn!(
                    from,
                    file = %file.display(),
                    "QML renderer: software, because this build's last GPU start in the \
                     compositor did not work: {reason}. Delete {} to let auto try the GPU \
                     again, or force it with `--qml gpu` or SOLIUM_QML=gpu",
                    file.display()
                );
                return Renderer::Software;
            }
            let started = Instant::now();
            let probed = probe();
            let probe_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let failed = match probed {
                Probed::Passed(said) => {
                    tracing::info!(from, probe_ms, said = %said, "QML renderer: gpu, the probe passed");
                    return Renderer::Gpu;
                }
                Probed::Failed(reason) => reason,
                Probed::TimedOut(after) => format!(
                    "it did not finish within {} ms and was killed",
                    after.as_millis()
                ),
            };
            tracing::warn!(
                from,
                probe_ms,
                "QML renderer: software, because the GPU probe failed: {failed}. \
                 `--qml gpu` or SOLIUM_QML=gpu forces the GPU; `--qml software` or \
                 SOLIUM_QML=software skips the probe"
            );
            Renderer::Software
        }
    }
}

/// The renderer this process decided on.
static DECIDED: OnceLock<Renderer> = OnceLock::new();

/// Decide the renderer for this process, from its arguments, its environment
/// and `configured`. Only the first call decides; see
/// `the_renderer_is_decided_once_per_process`.
pub(crate) fn decide(entry: Entry, configured: &Configured) -> Renderer {
    decide_in(&DECIDED, || {
        let asked = Asked {
            flag: flag(
                std::env::args_os()
                    .skip(1)
                    .map(|argument| argument.to_string_lossy().into_owned()),
            ),
            environment: crate::dev::qml(),
            alias: crate::dev::qml_gpu(),
            config: configured.renderer.clone(),
        };
        let (mode, source) = resolve(&asked);
        let markers = Markers::for_this_build();
        let timeout = configured.probe_timeout.unwrap_or(PROBE_TIMEOUT);
        let renderer = choose(
            entry,
            (mode, source),
            || markers.as_ref().and_then(Markers::previous),
            || probe(timeout),
        );
        if watched(entry, mode, renderer)
            && let Some(markers) = markers
        {
            let _ = WATCHED.set(markers);
        }
        renderer
    })
}

/// Whether the compositor's own GPU start is recorded for the next start: only
/// when `auto` chose the GPU on the hardware, because that is the only decision
/// a record changes. See `only_a_gpu_start_auto_chose_is_watched`.
fn watched(entry: Entry, mode: Mode, renderer: Renderer) -> bool {
    matches!(
        (entry, mode, renderer),
        (Entry::Tty, Mode::Auto, Renderer::Gpu)
    )
}

/// Where this process records its GPU start, when [`watched`] said to.
static WATCHED: OnceLock<Markers> = OnceLock::new();

/// Qt is about to be committed to the GPU in this process. Records it when
/// `auto` chose the GPU, and does nothing otherwise — in the probe's child
/// most of all, which decides nothing. See [`Markers::starting`] and
/// `only_a_gpu_start_auto_chose_is_watched`.
pub(crate) fn gpu_starting() {
    if let Some(markers) = WATCHED.get() {
        markers.starting();
    }
}

/// The GPU start in this process is over: `failure` is why it did not work, or
/// `None`. The file a failure was recorded in, when one was. See
/// [`Markers::finished`].
pub(crate) fn gpu_started(failure: Option<&str>) -> Option<PathBuf> {
    WATCHED.get()?.finished(failure)
}

/// Written just before Qt is committed to the GPU, removed once it works.
const PENDING: &str = "qml-gpu-pending";

/// Why this build's last GPU start did not work.
const FAILED: &str = "qml-gpu-failed";

/// What a pending start left behind means.
const NEVER_FINISHED: &str = "it never finished: the compositor stopped during it";

/// How the compositor's own GPU start went, kept for its next start.
///
/// The probe passing does not prove it: that start runs later, in-process,
/// once Qt is committed, and a failure there has no software path left in the
/// session it happens in. So `auto` writes `qml-gpu-pending` in the state
/// directory just before committing Qt, removes it when the pre-flight passes,
/// and replaces it with `qml-gpu-failed`, holding the reason, when it does not.
/// A pending file still there at the next start is a start that never finished
/// — Qt aborted, or the machine hung — and counts as a failure too.
///
/// Each file's first line is the build that wrote it. Another build's is
/// removed rather than believed, so a rebuild tries the GPU again. See
/// `a_gpu_start_that_never_finished_means_software_next_time`,
/// `a_failed_gpu_start_is_recorded_for_this_build_only` and
/// `a_gpu_start_that_worked_leaves_nothing_behind`.
#[derive(Clone, Debug)]
pub(crate) struct Markers {
    directory: PathBuf,
    build: String,
}

impl Markers {
    /// In `$XDG_STATE_HOME/solium`, for the binary that is running.
    fn for_this_build() -> Option<Self> {
        Some(Self {
            directory: crate::state_directory()?,
            build: this_build(),
        })
    }

    fn pending(&self) -> PathBuf {
        self.directory.join(PENDING)
    }

    fn failed(&self) -> PathBuf {
        self.directory.join(FAILED)
    }

    /// What the marker at `path` says after its first line, if this build wrote
    /// it. One another build wrote is removed.
    fn read(&self, path: &Path) -> Option<String> {
        let text = std::fs::read_to_string(path).ok()?;
        let (build, said) = text.split_once('\n').unwrap_or((text.as_str(), ""));
        if build == self.build {
            return Some(said.trim().to_owned());
        }
        let _ = std::fs::remove_file(path);
        None
    }

    /// The last GPU start by this build that did not work, if it did not.
    pub(crate) fn previous(&self) -> Option<Previous> {
        if self.read(&self.pending()).is_some() {
            self.fail(NEVER_FINISHED);
        }
        let reason = self.read(&self.failed())?;
        Some(Previous {
            file: self.failed(),
            reason,
        })
    }

    /// Say a GPU start has begun. Synced, because what ends a start that never
    /// finishes can be a hung machine and a hard reset, and a record lost with
    /// the page cache is the same start again at the next login. See
    /// `a_gpu_start_that_never_finished_means_software_next_time`.
    fn starting(&self) {
        if let Err(err) = write_synced(&self.pending(), &format!("{}\n", self.build)) {
            tracing::warn!(
                ?err,
                file = %self.pending().display(),
                "could not record the GPU start: if it never finishes, the next start tries again"
            );
        }
    }

    /// Say the GPU start is over, and how.
    fn finished(&self, failure: Option<&str>) -> Option<PathBuf> {
        match failure {
            None => {
                let _ = std::fs::remove_file(self.pending());
                None
            }
            Some(reason) => self.fail(reason),
        }
    }

    fn fail(&self, reason: &str) -> Option<PathBuf> {
        let failed = self.failed();
        let written = write_synced(
            &failed,
            &format!("{}\n{}\n", self.build, reason.replace('\n', " ")),
        );
        let _ = std::fs::remove_file(self.pending());
        match written {
            Ok(()) => Some(failed),
            Err(err) => {
                tracing::warn!(?err, file = %failed.display(), "could not record the failed GPU start");
                None
            }
        }
    }
}

fn write_synced(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)?;
    }
    let mut file = std::fs::File::create(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()
}

/// The running build: its version, its binary and when that was written.
fn this_build() -> String {
    let binary = std::env::current_exe().ok();
    let written = binary
        .as_ref()
        .and_then(|binary| std::fs::metadata(binary).ok())
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or_else(|| "?".to_owned(), |since| since.as_nanos().to_string());
    let binary = binary.map_or_else(|| "?".to_owned(), |binary| format!("{binary:?}"));
    format!("solium {} {binary} {written}", env!("CARGO_PKG_VERSION"))
}

fn decide_in(cell: &OnceLock<Renderer>, deciding: impl FnOnce() -> Renderer) -> Renderer {
    *cell.get_or_init(deciding)
}

/// What [`decide`] chose, and software when nothing has decided. See
/// `an_undecided_process_is_software`.
pub(crate) fn decided() -> Renderer {
    decided_in(&DECIDED)
}

fn decided_in(cell: &OnceLock<Renderer>) -> Renderer {
    cell.get().copied().unwrap_or(Renderer::Software)
}

/// Run the pre-flight in a child: this binary, as `--probe-qml-gpu`.
fn probe(timeout: Duration) -> Probed {
    match std::env::current_exe() {
        Ok(binary) => run(&binary, &[PROBE], timeout),
        Err(err) => Probed::Failed(format!(
            "could not find this binary to run the probe: {err}"
        )),
    }
}

/// Run `program` with `arguments`, and kill it if it takes longer than
/// `timeout`.
///
/// Killed by its PID — [`std::process::Child::kill`] signals that one process
/// and nothing else — and reaped on a thread of its own rather than waited for
/// here; see [`reap_elsewhere`]. Its last non-empty line is the answer: stdout
/// on success, stderr otherwise. `RUST_LOG` is not passed on, so that line is
/// the child's own and not the end of a log. See
/// `a_probe_that_passes_says_what_it_printed`,
/// `a_probe_that_fails_says_why_on_one_line` and
/// `a_probe_that_hangs_is_killed_and_timed_out`.
fn run(program: &Path, arguments: &[&str], timeout: Duration) -> Probed {
    let spawned = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("RUST_LOG")
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(err) => {
            return Probed::Failed(format!("could not start {}: {err}", program.display()));
        }
    };
    let stdout = child.stdout.take().map(drain);
    let stderr = child.stderr.take().map(drain);
    let deadline = Instant::now() + timeout;

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                reap_elsewhere(child);
                return Probed::TimedOut(timeout);
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(err) => {
                let _ = child.kill();
                reap_elsewhere(child);
                return Probed::Failed(format!("could not wait for the probe: {err}"));
            }
        }
    };

    if status.success() {
        let said = last_line(stdout).unwrap_or_else(|| "passed".to_owned());
        return Probed::Passed(said);
    }
    let exited = describe(status);
    Probed::Failed(match last_line(stderr) {
        Some(line) => format!("{line} ({exited})"),
        None => exited,
    })
}

/// Wait for `child` on a thread of its own, so it is still reaped but the
/// caller does not wait: a child asleep inside a GPU driver, which is what a
/// wedged GPU looks like, cannot die of `SIGKILL` until the driver lets it. See
/// `reaping_elsewhere_does_not_wait`.
fn reap_elsewhere(mut child: Child) {
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

/// Read a pipe to its end on a thread of its own, so a child that writes more
/// than a pipe holds cannot stall.
fn drain(mut pipe: impl std::io::Read + Send + 'static) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        let _ = sender.send(String::from_utf8_lossy(&bytes).into_owned());
    });
    receiver
}

/// The last non-empty line a drained pipe held, waiting a moment for it.
fn last_line(pipe: Option<mpsc::Receiver<String>>) -> Option<String> {
    let text = pipe?.recv_timeout(Duration::from_millis(200)).ok()?;
    text.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(str::to_owned)
}

fn describe(status: ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt as _;
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exit status {code}"),
        (None, Some(signal)) => format!("killed by signal {signal}"),
        (None, None) => status.to_string(),
    }
}

/// `solium --probe-qml-gpu`: the pre-flight in this process, then exit.
///
/// Exit 0 with one line on stdout when it passes; non-zero with one line on
/// stderr when it does not. Logs at `error` unless `RUST_LOG` says otherwise,
/// so Qt's own fatal messages still reach stderr. Its core limit is 0, so a
/// probe that aborts — how Qt fails — writes no core; see
/// `the_probe_writes_no_core`.
pub(crate) fn probe_child() -> ! {
    no_core();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("error"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .without_time()
        .with_target(false)
        .try_init();
    match super::probe_gpu() {
        Ok(said) => {
            let _ = writeln!(std::io::stdout(), "{said}");
            std::process::exit(0)
        }
        Err(err) => {
            let reason = format!("{err:#}").replace('\n', " ");
            let _ = writeln!(std::io::stderr(), "{reason}");
            std::process::exit(1)
        }
    }
}

/// Set this process's soft core limit to 0.
fn no_core() {
    use smithay::reexports::rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
    let _ = setrlimit(
        Resource::Core,
        Rlimit {
            current: Some(0),
            maximum: getrlimit(Resource::Core).maximum,
        },
    );
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        path::{Path, PathBuf},
        process::{Command, Stdio},
        sync::OnceLock,
        time::{Duration, Instant},
    };

    use super::{
        Asked, Entry, FAILED, Markers, Mode, NEVER_FINISHED, PENDING, Previous, Probed, Renderer,
        Source, choose, decide_in, decided_in, flag, no_core, reap_elsewhere, resolve, run,
        watched,
    };

    fn asked(
        flag: Option<&str>,
        environment: Option<&str>,
        alias: bool,
        config: Option<&str>,
    ) -> Asked {
        Asked {
            flag: flag.map(str::to_owned),
            environment: environment.map(str::to_owned),
            alias,
            config: config.map(str::to_owned),
        }
    }

    /// A probe that must not be run.
    fn never() -> Probed {
        panic!("the probe ran where it must not")
    }

    /// A record of an earlier start that must not be asked for.
    fn unasked() -> Option<Previous> {
        panic!("an earlier GPU start was consulted where it must not be")
    }

    /// No earlier start went wrong.
    fn clean() -> Option<Previous> {
        None
    }

    /// A directory of its own for one test's markers, empty.
    fn markers(name: &str, build: &str) -> (Markers, PathBuf) {
        let directory =
            std::env::temp_dir().join(format!("solium-qml-markers-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        (
            Markers {
                directory: directory.clone(),
                build: build.to_owned(),
            },
            directory,
        )
    }

    #[test]
    fn every_mode_is_named_ignoring_case_and_space() {
        assert_eq!(Mode::named("auto"), Some(Mode::Auto));
        assert_eq!(Mode::named("GPU"), Some(Mode::Gpu));
        assert_eq!(Mode::named(" Software\n"), Some(Mode::Software));
        assert_eq!(Mode::named("opengl"), None);
    }

    /// Flag over environment over the alias over the configuration, each one
    /// taking over only where everything above it said nothing.
    #[test]
    fn the_flag_beats_the_environment_beats_the_config() {
        let all = asked(Some("software"), Some("gpu"), true, Some("auto"));
        assert_eq!(resolve(&all), (Mode::Software, Source::Flag));

        let no_flag = asked(None, Some("software"), true, Some("gpu"));
        assert_eq!(resolve(&no_flag), (Mode::Software, Source::Environment));

        let only_config = asked(None, None, false, Some("software"));
        assert_eq!(resolve(&only_config), (Mode::Software, Source::Config));

        assert_eq!(resolve(&Asked::default()), (Mode::Auto, Source::Default));
    }

    /// `SOLIUM_QML_GPU` still means `gpu`, below `SOLIUM_QML` and above the
    /// configuration.
    #[test]
    fn the_old_knob_is_an_alias_for_gpu() {
        assert_eq!(
            resolve(&asked(None, None, true, None)),
            (Mode::Gpu, Source::Alias)
        );
        assert_eq!(
            resolve(&asked(None, None, true, Some("software"))),
            (Mode::Gpu, Source::Alias)
        );
        assert_eq!(
            resolve(&asked(None, Some("software"), true, None)),
            (Mode::Software, Source::Environment)
        );
    }

    /// A value that is not a mode means `auto`, and it still decides: a typo
    /// in `SOLIUM_QML` does not hand the question to the configuration.
    #[test]
    fn an_unknown_mode_warns_and_means_auto() {
        assert_eq!(
            resolve(&asked(None, Some("sofware"), false, Some("software"))),
            (Mode::Auto, Source::Environment)
        );
        assert_eq!(
            resolve(&asked(Some(""), None, false, Some("gpu"))),
            (Mode::Auto, Source::Flag)
        );
        assert_eq!(
            resolve(&asked(None, None, false, Some("vulkan"))),
            (Mode::Auto, Source::Config)
        );
    }

    #[test]
    fn the_flag_is_read_in_either_spelling() {
        let arguments = |list: &[&str]| list.iter().map(|&s| s.to_owned()).collect::<Vec<_>>();
        assert_eq!(
            flag(arguments(&["--tty", "--qml", "gpu"])),
            Some("gpu".to_owned())
        );
        assert_eq!(
            flag(arguments(&["--qml=software", "--tty"])),
            Some("software".to_owned())
        );
        assert_eq!(flag(arguments(&["--tty", "--qml"])), Some(String::new()));
        assert_eq!(flag(arguments(&["--check-qml", "a.qml"])), None);
        assert_eq!(
            flag(arguments(&["--qml", "gpu", "--qml", "software"])),
            Some("gpu".to_owned())
        );
    }

    #[test]
    fn check_qml_is_software_in_every_mode() {
        for mode in [Mode::Auto, Mode::Gpu, Mode::Software] {
            assert_eq!(
                choose(Entry::CheckQml, (mode, Source::Flag), unasked, never),
                Renderer::Software,
                "--check-qml came up on {mode:?}"
            );
        }
    }

    #[test]
    fn auto_is_software_nested() {
        assert_eq!(
            choose(Entry::Nested, (Mode::Auto, Source::Default), unasked, never),
            Renderer::Software
        );
    }

    /// `gpu` and `software` are taken as asked, on either backend: neither
    /// runs the probe, and neither asks how an earlier GPU start went.
    #[test]
    fn a_forced_mode_is_taken_without_a_probe() {
        for entry in [Entry::Tty, Entry::Nested] {
            assert_eq!(
                choose(entry, (Mode::Gpu, Source::Flag), unasked, never),
                Renderer::Gpu
            );
            assert_eq!(
                choose(entry, (Mode::Software, Source::Environment), unasked, never),
                Renderer::Software
            );
        }
    }

    #[test]
    fn a_passing_probe_means_gpu() {
        let ran = Cell::new(false);
        let renderer = choose(Entry::Tty, (Mode::Auto, Source::Default), clean, || {
            ran.set(true);
            Probed::Passed("gpu".to_owned())
        });
        assert!(ran.get(), "auto on the hardware did not run the probe");
        assert_eq!(renderer, Renderer::Gpu);
    }

    #[test]
    fn a_failed_probe_means_software() {
        let ran = Cell::new(false);
        let renderer = choose(Entry::Tty, (Mode::Auto, Source::Config), clean, || {
            ran.set(true);
            Probed::Failed("no DRM render node could be found".to_owned())
        });
        assert!(ran.get(), "auto on the hardware did not run the probe");
        assert_eq!(renderer, Renderer::Software);
    }

    #[test]
    fn a_timed_out_probe_means_software() {
        let ran = Cell::new(false);
        let renderer = choose(Entry::Tty, (Mode::Auto, Source::Default), clean, || {
            ran.set(true);
            Probed::TimedOut(Duration::from_secs(5))
        });
        assert!(ran.get(), "auto on the hardware did not run the probe");
        assert_eq!(renderer, Renderer::Software);
    }

    /// The compositor stopped inside its GPU start — Qt aborted, or the
    /// machine hung — so the pending marker was never cleared. The next start
    /// of the same build reads that as a failure and takes software without
    /// probing, because a probe passing is what led to the stop.
    #[test]
    fn a_gpu_start_that_never_finished_means_software_next_time() {
        let (markers, directory) = markers("never-finished", "build 1");
        markers.starting();
        assert!(directory.join(PENDING).is_file());

        let previous = markers.previous();
        assert_eq!(
            previous,
            Some(Previous {
                file: directory.join(FAILED),
                reason: NEVER_FINISHED.to_owned(),
            })
        );
        assert!(
            !directory.join(PENDING).exists(),
            "the pending marker was not turned into a failed one"
        );
        let renderer = choose(
            Entry::Tty,
            (Mode::Auto, Source::Default),
            || previous,
            never,
        );
        assert_eq!(renderer, Renderer::Software);
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// A pre-flight that failed in the compositor is recorded with its reason
    /// and holds for this build; another build removes it and tries again.
    #[test]
    fn a_failed_gpu_start_is_recorded_for_this_build_only() {
        let (markers, directory) = markers("failed", "build 1");
        markers.starting();
        let recorded = markers.finished(Some("the fence\nfailed"));
        assert_eq!(recorded, Some(directory.join(FAILED)));
        assert!(!directory.join(PENDING).exists());
        assert_eq!(
            markers.previous().map(|previous| previous.reason),
            Some("the fence failed".to_owned())
        );

        let rebuilt = Markers {
            directory: directory.clone(),
            build: "build 2".to_owned(),
        };
        assert_eq!(rebuilt.previous(), None);
        assert!(
            !directory.join(FAILED).exists(),
            "another build's record was left in place"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_gpu_start_that_worked_leaves_nothing_behind() {
        let (markers, directory) = markers("worked", "build 1");
        markers.starting();
        assert_eq!(markers.finished(None), None);
        assert_eq!(markers.previous(), None);
        assert!(!directory.join(PENDING).exists());
        assert!(!directory.join(FAILED).exists());
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn only_a_gpu_start_auto_chose_is_watched() {
        assert!(watched(Entry::Tty, Mode::Auto, Renderer::Gpu));
        assert!(!watched(Entry::Tty, Mode::Auto, Renderer::Software));
        assert!(!watched(Entry::Tty, Mode::Gpu, Renderer::Gpu));
        assert!(!watched(Entry::Nested, Mode::Auto, Renderer::Gpu));
        assert!(!watched(Entry::CheckQml, Mode::Auto, Renderer::Gpu));
    }

    /// The first decision stands: Qt cannot change scene graph once started.
    #[test]
    fn the_renderer_is_decided_once_per_process() {
        let cell = OnceLock::new();
        assert_eq!(decide_in(&cell, || Renderer::Gpu), Renderer::Gpu);
        assert_eq!(decide_in(&cell, || Renderer::Software), Renderer::Gpu);
        assert_eq!(decided_in(&cell), Renderer::Gpu);
    }

    #[test]
    fn an_undecided_process_is_software() {
        assert_eq!(decided_in(&OnceLock::new()), Renderer::Software);
    }

    #[test]
    fn a_probe_that_passes_says_what_it_printed() {
        let probed = run(
            Path::new("/bin/sh"),
            &["-c", "echo noise; echo 'gpu: rendered'"],
            Duration::from_secs(5),
        );
        assert_eq!(probed, Probed::Passed("gpu: rendered".to_owned()));
    }

    #[test]
    fn a_probe_that_fails_says_why_on_one_line() {
        let probed = run(
            Path::new("/bin/sh"),
            &["-c", "echo early >&2; echo 'no render node' >&2; exit 3"],
            Duration::from_secs(5),
        );
        assert_eq!(
            probed,
            Probed::Failed("no render node (exit status 3)".to_owned())
        );

        let silent = run(
            Path::new("/bin/sh"),
            &["-c", "kill -KILL $$"],
            Duration::from_secs(5),
        );
        assert_eq!(silent, Probed::Failed("killed by signal 9".to_owned()));

        let missing = run(
            Path::new("/nonexistent/solium"),
            &[],
            Duration::from_secs(5),
        );
        assert!(
            matches!(&missing, Probed::Failed(reason) if reason.starts_with("could not start")),
            "{missing:?}"
        );
    }

    /// A child that never answers is killed at the deadline, not waited for.
    #[test]
    fn a_probe_that_hangs_is_killed_and_timed_out() {
        let timeout = Duration::from_millis(100);
        let started = Instant::now();
        let probed = run(Path::new("/bin/sh"), &["-c", "sleep 30"], timeout);
        assert_eq!(probed, Probed::TimedOut(timeout));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "waited {:?} for a child that should have been killed at {timeout:?}",
            started.elapsed()
        );
    }

    /// The caller gets its answer at once, even from a child that has not
    /// exited, and the child is still waited for.
    #[test]
    fn reaping_elsewhere_does_not_wait() {
        let child = Command::new("/bin/sh")
            .args(["-c", "sleep 2"])
            .stdin(Stdio::null())
            .spawn();
        let Ok(child) = child else {
            panic!("could not start /bin/sh");
        };
        let started = Instant::now();
        reap_elsewhere(child);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "waited {:?} for a child that is reaped elsewhere",
            started.elapsed()
        );
    }

    #[test]
    fn the_probe_writes_no_core() {
        use smithay::reexports::rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
        let before = getrlimit(Resource::Core);
        // As high as it goes first, so a soft limit that was 0 already does
        // not pass this for nothing.
        let _ = setrlimit(
            Resource::Core,
            Rlimit {
                current: before.maximum,
                maximum: before.maximum,
            },
        );
        no_core();
        let after = getrlimit(Resource::Core);
        let _ = setrlimit(Resource::Core, before);
        assert_eq!(after.current, Some(0));
        assert_eq!(after.maximum, before.maximum, "the hard limit was changed");
    }
}
