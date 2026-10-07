//! What the command line asks for, decided before anything starts.
//!
//! Every argument is read here and every one must be known: an unknown flag,
//! `--help` included, used to start a compositor (#156). The backend word may
//! come anywhere, once. `tests::help_and_version_are_answered_without_a_backend`.

use std::path::PathBuf;

/// What to do.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Help,
    Version,
    /// The configuration, its scenes and its styles; or one QML file.
    Check(Option<PathBuf>),
    /// One QML file: the old spelling of `--check <file>`.
    CheckQml(PathBuf),
    Probe,
    ProbeQmlGpu,
    Tty,
    /// Nested when there is a display to nest in, on the hardware otherwise.
    Start,
}

/// Why an argument list was refused.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    Unknown(String),
    Missing {
        flag: &'static str,
        what: &'static str,
    },
    Twice(&'static str, &'static str),
    Misplaced(&'static str),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown(argument) => {
                write!(f, "{argument} is not something solium understands")
            }
            Self::Missing { flag, what } => write!(f, "{flag} needs {what}"),
            Self::Twice(first, second) => {
                write!(f, "{first} and {second} cannot both be asked for")
            }
            Self::Misplaced(flag) => write!(f, "{flag} only goes with --tty"),
        }
    }
}

/// The exit status of a refused command line.
pub(crate) const USAGE_ERROR: u8 = 2;

pub(crate) fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Command, Refused> {
    let mut chosen: Option<(&'static str, Command)> = None;
    let mut session = false;
    let mut arguments = arguments.into_iter().peekable();
    while let Some(argument) = arguments.next() {
        let backend = match argument.as_str() {
            "--help" | "-h" => Some(("--help", Command::Help)),
            "--version" | "-V" => Some(("--version", Command::Version)),
            "--tty" => Some(("--tty", Command::Tty)),
            "--probe" => Some(("--probe", Command::Probe)),
            "--check" => {
                let file = arguments
                    .next_if(|next| !next.starts_with('-'))
                    .map(PathBuf::from);
                Some(("--check", Command::Check(file)))
            }
            "--check-qml" => {
                let Some(file) = arguments.next_if(|next| !next.starts_with('-')) else {
                    return Err(Refused::Missing {
                        flag: "--check-qml",
                        what: "a QML file",
                    });
                };
                Some(("--check-qml", Command::CheckQml(PathBuf::from(file))))
            }
            "--debug-mode" => None,
            "--session" => {
                session = true;
                None
            }
            "--qml" => {
                if arguments.next().is_none() {
                    return Err(Refused::Missing {
                        flag: "--qml",
                        what: "auto, gpu or software",
                    });
                }
                None
            }
            other if other.starts_with("--qml=") => None,
            other if other == crate::qml::renderer::PROBE => {
                Some((crate::qml::renderer::PROBE, Command::ProbeQmlGpu))
            }
            other => return Err(Refused::Unknown(other.to_owned())),
        };
        if let Some((name, command)) = backend {
            if let Some((held, _)) = &chosen {
                return Err(Refused::Twice(held, name));
            }
            chosen = Some((name, command));
        }
    }
    let command = chosen.map_or(Command::Start, |(_, command)| command);
    if session && command != Command::Tty {
        return Err(Refused::Misplaced("--session"));
    }
    Ok(command)
}

/// `--help`: how to run Solium, the flags, and the variables that change a
/// session, from `environment.txt`, so the page and this cannot disagree.
/// `tests::help_names_every_flag_environment_txt_lists`.
pub(crate) fn help() -> String {
    let mut out = String::from(
        "solium: the Wayland compositor of Lilium DE\n\n\
         Usage: solium [--tty [--session]] [--debug-mode] [--qml <mode>]\n       \
         solium --check [<file.qml> | <effect folder>] | --probe | --help | --version\n\n\
         Nested when WAYLAND_DISPLAY or DISPLAY is set, on the hardware otherwise.\n",
    );
    let mut wanted = false;
    for line in include_str!("../environment.txt").lines() {
        if let Some(group) = line.strip_prefix("## ") {
            wanted = group == "Flags" || group == "Running a session";
            if wanted {
                out.push_str(&format!("\n{group}:\n"));
            }
            continue;
        }
        if !wanted || line.starts_with('#') {
            continue;
        }
        if line.starts_with("--") || line.starts_with("SOLIUM_") {
            out.push_str(&format!("  {line}\n"));
        } else if let Some(text) = line.strip_prefix("    ") {
            out.push_str(&format!("      {text}\n"));
        }
    }
    out.push_str(
        "\nEvery variable: crates/solium/environment.txt, or the documentation's \
         \"Flags and environment\" page.\n",
    );
    out
}

/// What `--version` prints: the version, and the commit this build was made
/// from when it was made from a git checkout (`build.rs`, `crate::commit`).
/// `tests::the_version_names_its_commit_when_built_from_one`.
pub(crate) fn version() -> String {
    versioned(env!("CARGO_PKG_VERSION"), env!("BUILD_COMMIT"))
}

fn versioned(version: &str, commit: &str) -> String {
    if commit.is_empty() {
        format!("solium {version}")
    } else {
        format!("solium {version} ({commit})")
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Command, Refused, help, parse, versioned};

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|each| (*each).to_owned()).collect()
    }

    #[test]
    fn help_and_version_are_answered_without_a_backend() {
        assert_eq!(parse(args(&["--help"])), Ok(Command::Help));
        assert_eq!(parse(args(&["-h"])), Ok(Command::Help));
        assert_eq!(parse(args(&["--version"])), Ok(Command::Version));
        assert_eq!(parse(args(&["-V"])), Ok(Command::Version));
    }

    /// `--version` names the commit the build was made from when it was made
    /// from a git checkout, and only the version otherwise.
    #[test]
    fn the_version_names_its_commit_when_built_from_one() {
        assert_eq!(versioned("0.0.0", "dfc95ce"), "solium 0.0.0 (dfc95ce)");
        assert_eq!(versioned("0.0.0", ""), "solium 0.0.0");
    }

    #[test]
    fn an_unknown_first_argument_is_refused() {
        assert_eq!(
            parse(args(&["--hlep"])),
            Err(Refused::Unknown("--hlep".to_owned()))
        );
    }

    #[test]
    fn an_unknown_flag_after_the_backend_is_refused() {
        assert_eq!(
            parse(args(&["--tty", "--sesion"])),
            Err(Refused::Unknown("--sesion".to_owned()))
        );
    }

    /// The flags read anywhere still are, and the backend word may come after
    /// them: `solium --debug-mode --tty` is the TTY, not a silent nested start.
    #[test]
    fn the_flags_read_anywhere_are_accepted() {
        assert_eq!(parse(args(&[])), Ok(Command::Start));
        assert_eq!(parse(args(&["--debug-mode"])), Ok(Command::Start));
        assert_eq!(parse(args(&["--qml", "gpu"])), Ok(Command::Start));
        assert_eq!(
            parse(args(&["--tty", "--qml=software", "--session"])),
            Ok(Command::Tty)
        );
        assert_eq!(parse(args(&["--debug-mode", "--tty"])), Ok(Command::Tty));
    }

    #[test]
    fn session_needs_tty() {
        assert_eq!(
            parse(args(&["--session"])),
            Err(Refused::Misplaced("--session"))
        );
    }

    #[test]
    fn check_qml_needs_a_file() {
        assert_eq!(
            parse(args(&["--check-qml"])),
            Err(Refused::Missing {
                flag: "--check-qml",
                what: "a QML file"
            })
        );
        assert_eq!(
            parse(args(&["--check-qml", "a.qml"])),
            Ok(Command::CheckQml(PathBuf::from("a.qml")))
        );
        assert_eq!(parse(args(&["--check"])), Ok(Command::Check(None)));
        assert_eq!(
            parse(args(&["--check", "b.qml"])),
            Ok(Command::Check(Some(PathBuf::from("b.qml"))))
        );
    }

    #[test]
    fn the_probe_child_is_still_recognised() {
        assert_eq!(
            parse(args(&[crate::qml::renderer::PROBE])),
            Ok(Command::ProbeQmlGpu)
        );
    }

    #[test]
    fn two_backends_are_refused() {
        assert_eq!(
            parse(args(&["--tty", "--probe"])),
            Err(Refused::Twice("--tty", "--probe"))
        );
    }

    /// `--help` names every flag `environment.txt` lists, so the two cannot
    /// drift.
    #[test]
    fn help_names_every_flag_environment_txt_lists() {
        let text = help();
        for line in include_str!("../environment.txt")
            .lines()
            .filter(|line| line.starts_with("--"))
        {
            let flag = line.split([' ', '=']).next().unwrap_or(line);
            assert!(text.contains(flag), "--help does not mention {flag}");
        }
    }
}
