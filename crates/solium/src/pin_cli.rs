//! `solium pin set` and `solium pin clear` (#281): the lock screen's PIN, set
//! and cleared from a terminal.
//!
//! **The account password first, checked as the lock checks it.** Both ask
//! for it and send it to `solium-auth`, the same helper and the same PAM
//! service the lock screen uses, opened at the top of `main` as the
//! compositor opens it; nothing changes unless it says yes
//! (`tests::a_wrong_password_changes_nothing_and_asks_for_no_pin`). That keeps
//! a PIN from being set or cleared by mistake, or by a script. It does not
//! stop someone already at the unlocked session, who runs as the user and can
//! write the PIN's files directly; that is no more than such a person can do
//! anyway, as `loginctl unlock-session` from inside that session already
//! unlocks it.
//!
//! **Then the PIN, twice**, `lock.pin.min_length` to `max_length` digits, and
//! only its argon2id hash is stored, atomically and 0600
//! (`solium_auth::pin`), with a fresh record of a password unlock beside it,
//! so the next lock takes the PIN (`crate::pin`).
//! `tests::set_stores_a_hash_and_never_the_pin`.
//!
//! **Nothing typed is shown, kept or logged.** Both need a terminal on
//! standard input and refuse to run without one, so nothing is piped in from
//! a file or a script. Echo is off while a secret is read, and back on
//! afterwards, also when Ctrl+C or a hang-up ends it midway. Each secret is
//! read a byte at a time straight into a [`Secret`], the zeroised page the
//! lock uses, and nothing here formats one or logs anything.

use std::{
    io::{self, Write as _},
    path::Path,
    process::ExitCode,
    sync::mpsc,
    time::Duration,
};

use solium_auth::{MAX_LEN, Secret};

use crate::{
    auth::{Kind, Outcome},
    cli::PinAction,
    pin::{Paths, Record},
};

/// The exit status when anything is refused or fails.
const FAILED: u8 = 1;

/// Run `solium pin set` or `solium pin clear`.
pub(crate) fn run(action: PinAction) -> ExitCode {
    let word = match action {
        PinAction::Set => "set",
        PinAction::Clear => "clear",
    };
    // SAFETY: `isatty` reads a descriptor's kind; nothing is passed.
    #[expect(unsafe_code, reason = "calling libc")]
    let terminal = unsafe { libc::isatty(0) } == 1;
    if !terminal {
        eprintln!(
            "solium pin {word} reads your password from a terminal, and standard input is not \
             one. Run it in a terminal; nothing was changed."
        );
        return ExitCode::from(FAILED);
    }
    let Some(paths) = Paths::of_this_user() else {
        eprintln!(
            "solium pin {word}: neither XDG_STATE_HOME nor HOME is set, so there is nowhere \
             for the PIN."
        );
        return ExitCode::from(FAILED);
    };
    let settings = configured();
    let user = crate::lock_auth::login_name();
    if user.is_empty() {
        eprintln!("solium pin {word}: this user has no login name to check a password for.");
        return ExitCode::from(FAILED);
    }
    let mut tty = match Tty::open() {
        Ok(tty) => tty,
        Err(error) => {
            eprintln!("solium pin {word}: the terminal would not answer: {error}");
            return ExitCode::from(FAILED);
        }
    };
    let timeout = settings.helper_timeout;
    let mut check = |secret: Secret| password(&user, secret, timeout);
    let done = match action {
        PinAction::Set => set(
            &mut tty,
            &mut check,
            &Store { paths: &paths },
            &settings.pin,
            crate::pin::Clocks::now(),
        ),
        PinAction::Clear => clear(&mut tty, &mut check, &paths),
    };
    match done {
        Ok(said) => {
            println!("{said}");
            ExitCode::SUCCESS
        }
        Err(said) => {
            eprintln!("{said}");
            ExitCode::from(FAILED)
        }
    }
}

/// `lock.*` as the configuration sets it, for the PIN's lengths and the
/// helper's timeout; the defaults when it does not load.
fn configured() -> crate::lock_auth::Settings {
    let path = crate::script::Scripts::config_path();
    match crate::script::Scripts::load(&path) {
        Ok(mut scripts) => scripts
            .startup()
            .commands
            .into_iter()
            .find_map(|command| match command {
                crate::script::Command::LockAuth(settings) => Some(settings),
                _ => None,
            })
            .unwrap_or_default(),
        Err(_) => {
            eprintln!(
                "(the configuration did not load, so the default lock.pin settings apply; \
                 `solium --check` says why)"
            );
            crate::lock_auth::Settings::default()
        }
    }
}

/// `secret` checked as `user`'s password by `solium-auth`, as the lock checks
/// it, waiting for the answer.
fn password(user: &str, secret: Secret, timeout: Duration) -> Outcome {
    let (sender, answer) = mpsc::channel();
    let kind = Kind::Password {
        user: user.to_owned(),
    };
    let _check = crate::auth::check(kind, secret, timeout, move |outcome| {
        let _ = sender.send(outcome);
    });
    answer
        .recv_timeout(timeout + Duration::from_secs(5))
        .unwrap_or_else(|_| Outcome::Unavailable {
            reason: "solium-auth did not answer".to_owned(),
            lasting: false,
        })
}

/// Where a PIN is asked for and what it is told: the terminal, or a test's
/// script.
trait Terminal {
    /// Show `prompt` and read a line without showing it.
    fn secret(&mut self, prompt: &str) -> io::Result<Secret>;
    /// Show a line.
    fn say(&mut self, text: &str);
}

/// Where the PIN goes.
struct Store<'a> {
    paths: &'a Paths,
}

/// Ask for the password and check it; `Ok` once it is the user's.
fn password_first(
    terminal: &mut dyn Terminal,
    check: &mut dyn FnMut(Secret) -> Outcome,
) -> Result<(), String> {
    let typed = terminal
        .secret("Password: ")
        .map_err(|error| format!("No password was read ({error}); nothing was changed."))?;
    if typed.is_empty() {
        return Err("No password was typed; nothing was changed.".to_owned());
    }
    match check(typed) {
        Outcome::Ok => Ok(()),
        Outcome::Failed { delay } => {
            // As long as PAM asked for, as the lock waits: a wrong guess here
            // is no quicker than one there.
            std::thread::sleep(delay);
            Err("That is not your password; nothing was changed.".to_owned())
        }
        Outcome::Refused { reason } => Err(format!(
            "PAM asked for more than the password ({reason}); nothing was changed."
        )),
        Outcome::Unavailable { reason, .. } => Err(format!(
            "Your password could not be checked: {reason}. Nothing was changed."
        )),
    }
}

/// `solium pin set`: the password, the new PIN twice, and its hash stored.
/// What to say, either way.
fn set(
    terminal: &mut dyn Terminal,
    check: &mut dyn FnMut(Secret) -> Outcome,
    store: &Store<'_>,
    settings: &crate::pin::Settings,
    now: crate::pin::Clocks,
) -> Result<String, String> {
    terminal.say("Setting the lock screen's PIN. Your password first.");
    password_first(terminal, check)?;
    let (min, max) = (settings.min_length, settings.max_length);
    let first = terminal
        .secret(&format!("New PIN, {min} to {max} digits: "))
        .map_err(|error| format!("No PIN was read ({error}); nothing was changed."))?;
    if !solium_auth::pin::shaped(&first, min, max) {
        return Err(format!(
            "A PIN is {min} to {max} digits, 0 to 9, and nothing else; nothing was changed."
        ));
    }
    let again = terminal
        .secret("The same PIN again: ")
        .map_err(|error| format!("No PIN was read ({error}); nothing was changed."))?;
    if first.expose() != again.expose() {
        return Err("The two PINs are not the same; nothing was changed.".to_owned());
    }
    drop(again);
    let hash = solium_auth::pin::hash(&first)
        .map_err(|error| format!("The PIN could not be hashed ({error}); nothing was changed."))?;
    drop(first);
    solium_auth::pin::write_private(&store.paths.pin, hash.as_bytes()).map_err(|error| {
        format!(
            "{} could not be written ({error}); nothing was changed.",
            store.paths.pin.display()
        )
    })?;
    // The password was just checked, as an unlock checks it: the next lock
    // takes the PIN. Unwritten, the next lock asks for the password once.
    let boot = crate::pin::boot_id(&store.paths.boot_id);
    let mut said = String::from(
        "PIN set. The lock screen asks for it from the next lock, and your password still \
         unlocks it.",
    );
    if let Err(error) = Record::after_password(now, boot).write(&store.paths.state) {
        said.push_str(&format!(
            "\n{} could not be written ({error}), so the first unlock needs your password.",
            store.paths.state.display()
        ));
    }
    if !settings.enabled {
        said.push_str(
            "\nlock.pin.enabled is false in your configuration: the lock screen does not use \
             the PIN until it is true.",
        );
    }
    Ok(said)
}

/// `solium pin clear`: the password, and the PIN and its counts removed.
fn clear(
    terminal: &mut dyn Terminal,
    check: &mut dyn FnMut(Secret) -> Outcome,
    paths: &Paths,
) -> Result<String, String> {
    if std::fs::symlink_metadata(&paths.pin).is_err() {
        return Ok("No PIN is set.".to_owned());
    }
    terminal.say("Clearing the lock screen's PIN. Your password first.");
    password_first(terminal, check)?;
    remove(&paths.pin).map_err(|error| {
        format!(
            "{} could not be removed ({error}); the PIN is still set.",
            paths.pin.display()
        )
    })?;
    let _ = remove(&paths.state);
    Ok("PIN cleared. The lock screen asks for your password.".to_owned())
}

/// Remove `path`; one that is not there is removed already.
fn remove(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Standard input, a terminal, with its settings as they were when it was
/// opened, which every read puts back.
struct Tty {
    saved: libc::termios,
}

impl Tty {
    fn open() -> io::Result<Self> {
        // SAFETY: `termios` is plain data, for which all zeroes is a value;
        // `tcgetattr` writes into it, and its answer is checked.
        #[expect(unsafe_code, reason = "calling libc")]
        let saved = unsafe {
            let mut saved: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(0, &raw mut saved) != 0 {
                return Err(io::Error::last_os_error());
            }
            saved
        };
        Ok(Self { saved })
    }

    /// Put the terminal back as it was.
    fn restore(&self) {
        // SAFETY: settings `tcgetattr` gave, for the descriptor they came from.
        #[expect(unsafe_code, reason = "calling libc")]
        unsafe {
            libc::tcsetattr(0, libc::TCSANOW, &raw const self.saved);
        }
    }

    /// One line from standard input with echo off, into `secret`.
    fn read_quietly(&self, secret: &mut Secret) -> io::Result<()> {
        let mut quiet = self.saved;
        quiet.c_lflag &= !(libc::ECHO | libc::ECHOE | libc::ECHOK | libc::ECHONL);
        // SAFETY: settings derived from `tcgetattr`'s, for the same descriptor.
        #[expect(unsafe_code, reason = "calling libc")]
        if unsafe { libc::tcsetattr(0, libc::TCSAFLUSH, &raw const quiet) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // Whatever ends the process while echo is off puts it back on first:
        // Ctrl+C, a hang-up, a kill. `tcsetattr` and `_exit` are
        // async-signal-safe.
        let saved = self.saved;
        let guards: Vec<_> = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT]
            .into_iter()
            .filter_map(|signal| {
                // SAFETY: the action calls only async-signal-safe functions.
                #[expect(unsafe_code, reason = "installing a signal handler")]
                unsafe {
                    signal_hook_registry::register(signal, move || {
                        libc::tcsetattr(0, libc::TCSANOW, &raw const saved);
                        libc::_exit(130);
                    })
                }
                .ok()
            })
            .collect();
        let read = read_line(secret);
        self.restore();
        for guard in guards {
            signal_hook_registry::unregister(guard);
        }
        read
    }
}

impl Terminal for Tty {
    fn secret(&mut self, prompt: &str) -> io::Result<Secret> {
        let mut err = io::stderr();
        let _ = write!(err, "{prompt}");
        let _ = err.flush();
        let mut secret = Secret::new();
        let read = self.read_quietly(&mut secret);
        // The Enter that was not echoed.
        let _ = writeln!(err);
        read.map(|()| secret)
    }

    fn say(&mut self, text: &str) {
        eprintln!("{text}");
    }
}

/// Read standard input up to a newline, a byte at a time, into `secret`,
/// which never sees a byte that is not part of a whole UTF-8 character. A
/// line longer than a secret can be is refused whole.
fn read_line(secret: &mut Secret) -> io::Result<()> {
    let mut character = [0_u8; 4];
    let mut have = 0_usize;
    let mut want = 1_usize;
    let mut too_long = false;
    let read = loop {
        let mut byte = 0_u8;
        // SAFETY: one byte, into a local.
        #[expect(unsafe_code, reason = "calling libc")]
        let got = unsafe { libc::read(0, (&raw mut byte).cast(), 1) };
        if got < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            break Err(error);
        }
        if got == 0 {
            break if secret.is_empty() && have == 0 {
                Err(io::ErrorKind::UnexpectedEof.into())
            } else {
                Ok(())
            };
        }
        if byte == b'\n' || byte == b'\r' {
            break Ok(());
        }
        if have == 0 {
            want = utf8_len(byte);
        }
        if let Some(slot) = character.get_mut(have) {
            *slot = byte;
        }
        wipe(std::slice::from_mut(&mut byte));
        have += 1;
        if have >= want {
            let pushed = character
                .get(..want)
                .and_then(|bytes| std::str::from_utf8(bytes).ok())
                .is_some_and(|text| secret.push_str(text));
            // Only a character that does not fit is refused this way; one
            // that is not UTF-8 is dropped, as a key that types nothing.
            if !pushed && secret.len() + want > MAX_LEN {
                too_long = true;
            }
            wipe(&mut character);
            have = 0;
        }
    };
    wipe(&mut character);
    if too_long || read.is_err() {
        secret.clear();
    }
    if too_long {
        return Err(io::Error::other("longer than any password can be"));
    }
    read
}

/// How many bytes the UTF-8 character `lead` starts has; 1 for a byte that
/// starts none.
fn utf8_len(lead: u8) -> usize {
    match lead {
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    }
}

/// Overwrite `bytes` with zeros, in writes the compiler keeps.
fn wipe(bytes: &mut [u8]) {
    for byte in bytes.iter_mut() {
        // SAFETY: `byte` is a valid, aligned, exclusive reference.
        #[expect(unsafe_code, reason = "a volatile write")]
        unsafe {
            std::ptr::write_volatile(byte, 0);
        }
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, os::unix::fs::MetadataExt as _, path::PathBuf};

    use super::*;

    const RIGHT: &str = "correct horse";
    const PIN: &str = "482916";

    /// A terminal that answers from a script and keeps what it was told.
    #[derive(Default)]
    struct Scripted {
        answers: VecDeque<&'static str>,
        prompts: Vec<String>,
        said: Vec<String>,
    }

    impl Terminal for Scripted {
        fn secret(&mut self, prompt: &str) -> io::Result<Secret> {
            self.prompts.push(prompt.to_owned());
            let answer = self
                .answers
                .pop_front()
                .ok_or(io::Error::from(io::ErrorKind::UnexpectedEof))?;
            let mut secret = Secret::new();
            assert!(secret.push_str(answer));
            Ok(secret)
        }

        fn say(&mut self, text: &str) {
            self.said.push(text.to_owned());
        }
    }

    fn scripted(answers: &[&'static str]) -> Scripted {
        Scripted {
            answers: answers.iter().copied().collect(),
            ..Scripted::default()
        }
    }

    /// The stand-in for PAM: yes to [`RIGHT`] alone. Counts what it is asked.
    fn stand_in(asked: &mut u32) -> impl FnMut(Secret) -> Outcome + '_ {
        move |secret| {
            *asked += 1;
            if secret.expose() == RIGHT.as_bytes() {
                Outcome::Ok
            } else {
                Outcome::Failed {
                    delay: Duration::ZERO,
                }
            }
        }
    }

    /// A directory of the test's own, with a boot id in it.
    fn paths(test: &str) -> (PathBuf, Paths) {
        let dir =
            std::env::temp_dir().join(format!("solium-pin-cli-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a directory");
        std::fs::write(dir.join("boot_id"), "boot-one\n").expect("a boot id");
        let mut paths = Paths::in_dir(&dir.join("solium"));
        paths.boot_id = dir.join("boot_id");
        (dir, paths)
    }

    fn everything_said(terminal: &Scripted, outcome: &Result<String, String>) -> String {
        let mut text = terminal.said.join("\n");
        text.push_str(&terminal.prompts.join("\n"));
        text.push_str(outcome.as_ref().map_or_else(|e| e.as_str(), |o| o.as_str()));
        text
    }

    /// **`solium pin set` stores a hash and never the PIN**: after the right
    /// password and the PIN twice, the PIN file is an argon2id hash the PIN
    /// checks against, mode 0600, with no run of the PIN's digits in it, and
    /// beside it a record of the password unlock on this boot. Nothing it
    /// says repeats the password or the PIN.
    #[test]
    fn set_stores_a_hash_and_never_the_pin() {
        let (dir, paths) = paths("set");
        let mut terminal = scripted(&[RIGHT, PIN, PIN]);
        let mut asked = 0;
        let store = Store { paths: &paths };
        let outcome = set(
            &mut terminal,
            &mut stand_in(&mut asked),
            &store,
            &crate::pin::Settings::default(),
            crate::pin::Clocks::at(1_000),
        );
        assert!(outcome.is_ok(), "{outcome:?}");
        assert_eq!(asked, 1);
        let text = std::fs::read_to_string(&paths.pin).expect("the PIN file");
        assert!(text.starts_with("$argon2id$"), "{text}");
        for start in 0..=PIN.len() - 4 {
            let run = &PIN[start..start + 4];
            assert!(!text.contains(run), "{run} of the PIN is in the file");
        }
        let mode = std::fs::metadata(&paths.pin).expect("its mode").mode() & 0o777;
        assert_eq!(mode, 0o600, "mode {mode:o}");
        let mut pin = Secret::new();
        assert!(pin.push_str(PIN));
        assert_eq!(
            solium_auth::pin::check(&paths.pin, &pin),
            solium_auth::Answer::Ok
        );
        assert_eq!(
            Record::read(&paths.state),
            (
                Record::after_password(crate::pin::Clocks::at(1_000), Some("boot-one".to_owned())),
                None
            )
        );
        let said = everything_said(&terminal, &outcome);
        assert!(!said.contains(PIN) && !said.contains(RIGHT), "{said}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A wrong password changes nothing, and asks for no PIN**; nor does
    /// an empty one, or none at all.
    #[test]
    fn a_wrong_password_changes_nothing_and_asks_for_no_pin() {
        let (dir, paths) = paths("wrong");
        let store = Store { paths: &paths };
        for answers in [&["wrong", PIN, PIN][..], &["", PIN, PIN], &[]] {
            let mut terminal = scripted(answers);
            let mut asked = 0;
            let outcome = set(
                &mut terminal,
                &mut stand_in(&mut asked),
                &store,
                &crate::pin::Settings::default(),
                crate::pin::Clocks::at(1_000),
            );
            assert!(outcome.is_err(), "{answers:?}: {outcome:?}");
            assert_eq!(
                terminal.prompts.len(),
                1,
                "{answers:?}: a PIN was asked for"
            );
            assert!(!paths.pin.exists() && !paths.state.exists(), "{answers:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A PIN out of shape, or two that differ, store nothing.
    #[test]
    fn a_pin_out_of_shape_or_typed_differently_stores_nothing() {
        let (dir, paths) = paths("shape");
        let store = Store { paths: &paths };
        for answers in [
            [RIGHT, "123", "123"],
            [RIGHT, "123456789", "123456789"],
            [RIGHT, "12a456", "12a456"],
            [RIGHT, PIN, "482917"],
        ] {
            let mut terminal = scripted(&answers);
            let mut asked = 0;
            let outcome = set(
                &mut terminal,
                &mut stand_in(&mut asked),
                &store,
                &crate::pin::Settings::default(),
                crate::pin::Clocks::at(1_000),
            );
            assert!(outcome.is_err(), "{answers:?}");
            assert!(!paths.pin.exists(), "{answers:?}");
        }
        // The lengths are the settings'.
        let mut terminal = scripted(&[RIGHT, "123", "123"]);
        let mut asked = 0;
        let three = crate::pin::Settings {
            min_length: 3,
            ..crate::pin::Settings::default()
        };
        let outcome = set(
            &mut terminal,
            &mut stand_in(&mut asked),
            &store,
            &three,
            crate::pin::Clocks::at(1_000),
        );
        assert!(outcome.is_ok(), "{outcome:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **`solium pin clear` needs the password**: a wrong one leaves the PIN
    /// and its counts, the right one removes both; with no PIN, nothing is
    /// asked.
    #[test]
    fn clear_needs_the_password() {
        let (dir, paths) = paths("clear");
        let store = Store { paths: &paths };
        let mut asked = 0;
        set(
            &mut scripted(&[RIGHT, PIN, PIN]),
            &mut stand_in(&mut asked),
            &store,
            &crate::pin::Settings::default(),
            crate::pin::Clocks::at(1_000),
        )
        .expect("set");
        let outcome = clear(&mut scripted(&["wrong"]), &mut stand_in(&mut asked), &paths);
        assert!(outcome.is_err());
        assert!(paths.pin.exists() && paths.state.exists());
        let outcome = clear(&mut scripted(&[RIGHT]), &mut stand_in(&mut asked), &paths);
        assert!(outcome.is_ok(), "{outcome:?}");
        assert!(!paths.pin.exists() && !paths.state.exists());
        let mut terminal = scripted(&[]);
        let outcome = clear(&mut terminal, &mut stand_in(&mut asked), &paths);
        assert_eq!(outcome, Ok("No PIN is set.".to_owned()));
        assert!(terminal.prompts.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
