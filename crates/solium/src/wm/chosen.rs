//! Which window manager the compositor starts, and how: `wm.toml`.
//!
//! Everything else a desktop is configured with is the window manager's to
//! read, in whatever form it likes. This is the one file the compositor
//! reads for itself, because it is read before any window manager runs to
//! be asked. It sits in the user's configuration directory:
//!
//! ```toml
//! # ~/.config/solium/wm.toml
//! command = ["my-wm", "--config", "~/.config/my-wm/rules"]
//! cpu_weight = 1000
//! ```
//!
//! - `command`: the program and its arguments, run as they are written, in
//!   place of the shipped Lua window manager. The program is looked for on
//!   `PATH` when it names no directory, and a leading `~/` in the program
//!   or an argument is the home directory. Its standard input is the
//!   socket to the compositor, and it is told nothing else: the protocol
//!   says the rest.
//! - `cpu_weight`: the CPU weight of the scope the window manager runs in,
//!   1 to 10000, as systemd's `CPUWeight=` counts it, where 100 is what
//!   every other program has. A window manager answers in microseconds and
//!   then sleeps, so a high weight costs nothing and keeps its answers
//!   inside a frame while every core is busy.
//!
//! With no file, or no `command` in it, the shipped window manager runs.
//! What the file says is read each time a window manager is started, so a
//! reload after `command` changed starts the new one.

use std::path::{Path, PathBuf};

/// `cpu_weight` when the file does not say: ten times a program's own.
const CPU_WEIGHT: u64 = 1000;

/// What `wm.toml` says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Chosen {
    /// The program and its arguments, in place of the shipped window
    /// manager; `None` for the shipped one.
    pub(crate) command: Option<Vec<String>>,
    /// The CPU weight of the window manager's scope.
    pub(crate) cpu_weight: u64,
}

impl Default for Chosen {
    /// The shipped window manager, at ten times a program's weight.
    fn default() -> Self {
        Self {
            command: None,
            cpu_weight: CPU_WEIGHT,
        }
    }
}

impl Chosen {
    /// `wm.toml` in the user's configuration directory ([`Self::file`]),
    /// and what is wrong with it, a line each: a key that cannot be used
    /// keeps its default, and a file that is not TOML is as good as no
    /// file.
    #[cfg(not(test))]
    pub(crate) fn read() -> (Self, Vec<String>) {
        match crate::wm::paths::user_config_dir() {
            Some(directory) => Self::read_in(&directory),
            None => (Self::default(), Vec::new()),
        }
    }

    /// A test's window manager runs on a thread, whatever the developer's
    /// own `wm.toml` says.
    #[cfg(test)]
    pub(crate) fn read() -> (Self, Vec<String>) {
        (Self::default(), Vec::new())
    }

    /// `wm.toml` in `directory`.
    pub(crate) fn read_in(directory: &Path) -> (Self, Vec<String>) {
        match std::fs::read_to_string(file_in(directory)) {
            Ok(text) => Self::parse(&text, home().as_deref()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => (Self::default(), Vec::new()),
            Err(err) => (
                Self::default(),
                vec![format!(
                    "could not be read, so the shipped window manager runs: {err}"
                )],
            ),
        }
    }

    /// Where the user's `wm.toml` is, or would be.
    pub(crate) fn file() -> Option<PathBuf> {
        crate::wm::paths::user_config_dir().map(|directory| file_in(&directory))
    }

    /// What `text` says, `~/` standing for `home`, and what is wrong with
    /// it.
    pub(crate) fn parse(text: &str, home: Option<&Path>) -> (Self, Vec<String>) {
        let mut chosen = Self::default();
        let mut said = Vec::new();
        let table = match toml::de::DeTable::parse(text) {
            Ok(table) => table.into_inner(),
            Err(err) => {
                said.push(format!(
                    "not TOML, so the shipped window manager runs: {}",
                    err.message()
                ));
                return (chosen, said);
            }
        };
        for (key, value) in table {
            match (key.get_ref().as_ref(), value.into_inner()) {
                ("command", toml::de::DeValue::Array(items)) => {
                    let mut command = Vec::new();
                    let mut strings = true;
                    for item in items {
                        match item.into_inner() {
                            toml::de::DeValue::String(word) => {
                                command.push(expand(&word, home));
                            }
                            _ => strings = false,
                        }
                    }
                    if !strings || command.first().is_none_or(String::is_empty) {
                        said.push(
                            "`command` is a program and its arguments, each a string: \
                             [\"my-wm\", \"--flag\"]. The shipped window manager runs"
                                .to_owned(),
                        );
                    } else {
                        chosen.command = Some(command);
                    }
                }
                ("command", _) => said.push(
                    "`command` is a list, the program and then its arguments: \
                     [\"my-wm\", \"--flag\"]. The shipped window manager runs"
                        .to_owned(),
                ),
                ("cpu_weight", toml::de::DeValue::Integer(number)) => {
                    let digits = number.as_str().replace('_', "");
                    match u64::from_str_radix(&digits, number.radix()) {
                        Ok(weight) if (1..=10_000).contains(&weight) => {
                            chosen.cpu_weight = weight;
                        }
                        _ => said.push(format!(
                            "`cpu_weight` is 1 to 10000, not {number}: {CPU_WEIGHT} is used"
                        )),
                    }
                }
                ("cpu_weight", _) => said.push(format!(
                    "`cpu_weight` is a whole number, 1 to 10000: {CPU_WEIGHT} is used"
                )),
                (other, _) => said.push(format!(
                    "`{other}` is not a key of this file, which has `command` and \
                     `cpu_weight`; left out"
                )),
            }
        }
        (chosen, said)
    }

    /// The command as a person would type it, for a log line and
    /// `solium --check`.
    pub(crate) fn spelled(command: &[String]) -> String {
        command
            .iter()
            .map(|word| {
                if word.is_empty() || word.contains(char::is_whitespace) {
                    format!("{word:?}")
                } else {
                    word.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Where `wm.toml` is, in the configuration directory `directory`.
pub(crate) fn file_in(directory: &Path) -> PathBuf {
    directory.join("wm.toml")
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// `word` with a leading `~/` as `home`: a file like this one is where a
/// person writes `~/bin/my-wm`, and no shell reads it to say what that is.
fn expand(word: &str, home: Option<&Path>) -> String {
    match (word.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest).display().to_string(),
        _ => word.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::Chosen;

    fn parse(text: &str) -> (Chosen, Vec<String>) {
        Chosen::parse(text, Some(Path::new("/home/someone")))
    }

    /// No file, and an empty one, are the shipped window manager at the
    /// default weight, with nothing to say.
    #[test]
    fn nothing_written_is_the_shipped_window_manager() {
        assert_eq!(parse(""), (Chosen::default(), Vec::new()));
        assert_eq!(parse("# nothing yet\n"), (Chosen::default(), Vec::new()));
        let (chosen, said) =
            Chosen::read_in(Path::new("/nowhere/that/is/a/configuration/directory"));
        assert_eq!((chosen, said), (Chosen::default(), Vec::new()));
        assert_eq!(Chosen::default().command, None);
        assert_eq!(Chosen::default().cpu_weight, 1000);
    }

    /// The command is run as written: its words kept whole, spaces and
    /// all, with only a leading `~/` made the home directory.
    #[test]
    fn a_command_is_its_words_as_written_with_home_filled_in() {
        let (chosen, said) = parse(
            "command = [\"~/bin/my wm\", \"--rules\", \"~/.config/my-wm/rules\", \"a~/b\"]\n\
             cpu_weight = 2_500\n",
        );
        assert_eq!(said, Vec::<String>::new());
        assert_eq!(
            chosen.command.as_deref(),
            Some(
                &[
                    "/home/someone/bin/my wm".to_owned(),
                    "--rules".to_owned(),
                    "/home/someone/.config/my-wm/rules".to_owned(),
                    "a~/b".to_owned(),
                ][..]
            )
        );
        assert_eq!(chosen.cpu_weight, 2500);
        assert_eq!(
            Chosen::spelled(chosen.command.as_deref().unwrap_or_default()),
            "\"/home/someone/bin/my wm\" --rules /home/someone/.config/my-wm/rules a~/b"
        );
    }

    /// A key that cannot be used keeps its default and is said, with the
    /// key named; the keys that can be used still are.
    #[test]
    fn a_key_that_cannot_be_used_is_said_and_keeps_its_default() {
        for wrong in [
            "command = \"my-wm --flag\"",
            "command = []",
            "command = [\"\"]",
            "command = [\"my-wm\", 3]",
        ] {
            let (chosen, said) = parse(&format!("{wrong}\ncpu_weight = 300\n"));
            assert_eq!(chosen.command, None, "{wrong}");
            assert_eq!(chosen.cpu_weight, 300, "{wrong}");
            assert_eq!(said.len(), 1, "{wrong}: {said:?}");
            assert!(said[0].contains("`command`"), "{wrong}: {said:?}");
        }
        for wrong in [
            "cpu_weight = 0",
            "cpu_weight = 10001",
            "cpu_weight = \"high\"",
            "cpu_weight = -5",
        ] {
            let (chosen, said) = parse(&format!("command = [\"my-wm\"]\n{wrong}\n"));
            assert_eq!(chosen.command, Some(vec!["my-wm".to_owned()]), "{wrong}");
            assert_eq!(chosen.cpu_weight, 1000, "{wrong}");
            assert_eq!(said.len(), 1, "{wrong}: {said:?}");
            assert!(said[0].contains("`cpu_weight`"), "{wrong}: {said:?}");
        }
        let (chosen, said) = parse("comand = [\"my-wm\"]\n");
        assert_eq!(chosen, Chosen::default());
        assert!(said[0].contains("`comand`"), "{said:?}");
    }

    /// A file that is not TOML is as good as no file, and says so; and the
    /// file is `wm.toml` in the configuration directory.
    #[test]
    fn a_file_that_is_not_toml_is_said_and_is_no_file() {
        let (chosen, said) = parse("command = [\"my-wm\"\n");
        assert_eq!(chosen, Chosen::default());
        assert!(said[0].starts_with("not TOML"), "{said:?}");

        let directory = std::env::temp_dir().join(format!("solium-wm-toml-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("a directory");
        std::fs::write(
            directory.join("wm.toml"),
            "command = [\"my-wm\"]\ncpu_weight = 99999\n",
        )
        .expect("the file");
        let (chosen, said) = Chosen::read_in(&directory);
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(chosen.command, Some(vec!["my-wm".to_owned()]));
        assert_eq!(said.len(), 1);
        assert!(said[0].starts_with("`cpu_weight`"), "{said:?}");
    }
}
