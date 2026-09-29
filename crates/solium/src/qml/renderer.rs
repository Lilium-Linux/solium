//! Which scene graph QML renders on, decided once per process before Qt starts.
//!
//! Three modes: `auto` (the default), `gpu` and `software`. `auto` on the
//! hardware backend runs the GPU pre-flight in a child process first, because
//! Qt fixes its scene graph for the life of whichever process starts it: a
//! pre-flight that fails inside the compositor leaves no software path to go
//! back to, and one that fails in a child costs only the child. See
//! `a_failed_probe_means_software` and `a_probe_that_hangs_is_killed_and_timed_out`.

use std::{
    io::Write as _,
    path::Path,
    process::{Command, ExitStatus, Stdio},
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

/// The renderer for `entry` given the mode asked for, running `probe` only for
/// `auto` on the hardware.
///
/// `--check-qml` is software in every mode and never probes: it answers whether
/// a file parses, and that must not depend on the machine. Nested `auto` is
/// software, because winit hands the compositor no GBM device to allocate
/// scene buffers from. Says what it chose in one line starting `QML renderer:`.
/// See `check_qml_is_software_in_every_mode`, `auto_is_software_nested`,
/// `a_passing_probe_means_gpu`, `a_failed_probe_means_software` and
/// `a_timed_out_probe_means_software`.
pub(crate) fn choose(
    entry: Entry,
    (mode, source): (Mode, Source),
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
        let timeout = configured.probe_timeout.unwrap_or(PROBE_TIMEOUT);
        choose(entry, resolve(&asked), || probe(timeout))
    })
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
/// and nothing else — and reaped. Its last non-empty line is the answer: stdout
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
                let _ = child.wait();
                return Probed::TimedOut(timeout);
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
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
/// so Qt's own fatal messages still reach stderr.
pub(crate) fn probe_child() -> ! {
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

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        path::Path,
        sync::OnceLock,
        time::{Duration, Instant},
    };

    use super::{
        Asked, Entry, Mode, Probed, Renderer, Source, choose, decide_in, decided_in, flag, resolve,
        run,
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
                choose(Entry::CheckQml, (mode, Source::Flag), never),
                Renderer::Software,
                "--check-qml came up on {mode:?}"
            );
        }
    }

    #[test]
    fn auto_is_software_nested() {
        assert_eq!(
            choose(Entry::Nested, (Mode::Auto, Source::Default), never),
            Renderer::Software
        );
    }

    /// `gpu` and `software` are taken as asked, on either backend, and neither
    /// runs the probe.
    #[test]
    fn a_forced_mode_is_taken_without_a_probe() {
        for entry in [Entry::Tty, Entry::Nested] {
            assert_eq!(
                choose(entry, (Mode::Gpu, Source::Flag), never),
                Renderer::Gpu
            );
            assert_eq!(
                choose(entry, (Mode::Software, Source::Environment), never),
                Renderer::Software
            );
        }
    }

    #[test]
    fn a_passing_probe_means_gpu() {
        let ran = Cell::new(false);
        let renderer = choose(Entry::Tty, (Mode::Auto, Source::Default), || {
            ran.set(true);
            Probed::Passed("gpu".to_owned())
        });
        assert!(ran.get(), "auto on the hardware did not run the probe");
        assert_eq!(renderer, Renderer::Gpu);
    }

    #[test]
    fn a_failed_probe_means_software() {
        let renderer = choose(Entry::Tty, (Mode::Auto, Source::Config), || {
            Probed::Failed("no DRM render node could be found".to_owned())
        });
        assert_eq!(renderer, Renderer::Software);
    }

    #[test]
    fn a_timed_out_probe_means_software() {
        let renderer = choose(Entry::Tty, (Mode::Auto, Source::Default), || {
            Probed::TimedOut(Duration::from_secs(5))
        });
        assert_eq!(renderer, Renderer::Software);
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
}
