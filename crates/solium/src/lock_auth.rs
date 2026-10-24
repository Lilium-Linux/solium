//! The password half of Solium's own lock (#281): what is typed at it, the
//! check of it, and what the lock's scenes are told.
//!
//! **The password lives here and nowhere else.** A key pressed while Solium
//! holds the lock goes from the keyboard filter into a [`Secret`], the
//! zeroised, locked page `solium-auth` shares ([`Solium::secret_edit`]). It
//! never becomes a Qt key event, a `String`, a log line, or anything Lua or an
//! input method sees. A keypad key is hit-tested by the compositor, and the
//! press or finger on it never reaches its scene ([`Solium::keypad_down`]).
//! What QML is told is how many characters there are, whether they are
//! selected, whether the field takes typing, and that a keypad key was
//! pressed, never which: `Lock`, `SecretField` and `SecretKeypad`
//! (`qml/lock.h`).
//! `tests::typed_text_never_reaches_the_lock_scene`,
//! `tests::a_keypad_key_types_into_the_secret_and_its_scene_never_hears_which`.
//!
//! **Only Rust unlocks, and only on the helper's yes.** [`settle`], once a
//! loop, is where `solium-auth`'s answers arrive, and an [`Outcome::Ok`] for
//! the check under way is the one thing here that calls
//! [`Solium::unlock_natively`]. logind's `Unlock`, the administrator's way
//! out, is the only other caller. A scene can ask to submit or to clear, and
//! the request carries nothing; Lua can do neither.
//! `tests::nothing_in_qml_can_unlock_or_read_the_secret`.
//!
//! **The states**, as `Lock.state` names them:
//!
//! * `locking`: the lock has just been taken, and its scenes are coming down
//!   over the desktop for `lock.enter_ms` (`lock_scene.rs`). Whatever is
//!   typed meanwhile is the password's as in any other state; the state is
//!   the entry's until it ends.
//! * `peeking`: no lock at all. The configuration is pulling the scenes down
//!   over the unlocked desktop with a finger (`sol.lock_peek`), and
//!   `Lock.progress` says how far; nothing typed reaches the field, which is
//!   not armed until a lock is taken. Ended without one, the scenes go back
//!   up reading `unlocking` for `lock.exit_ms` at most (`lock_scene.rs`).
//! * `idle`: nothing typed. The field is armed, so the first key goes into it.
//! * `prompting`: something typed.
//! * `checking`: the helper has the password; then, after a no, PAM's failure
//!   delay is waited out here, so the screen never freezes on it.
//! * `failed`: the last check said no. Typing starts again.
//! * `lockedOut`: `lock.lockout.attempts` noes in a row shut the field for
//!   `lock.lockout.seconds`, doubling with each lockout up to `max_seconds`.
//! * `unavailable`: no yes or no could be had. `Lock.message` gives the way
//!   out from another console. Three ways there:
//!   * PAM had the password and answered neither yes nor no: it asked for
//!     more than one password, or ended in a status that is neither
//!     ([`Outcome::Refused`]). That counts as a wrong attempt, toward the
//!     lockout as a no does, and the password is wiped: a stack that asks
//!     twice is not given unlimited guesses, nor the same one again and
//!     again for pam_faillock to count;
//!   * the helper could not answer and trying again cannot mend it: there is
//!     none, it is of another install, or PAM has no `solium` service. The
//!     password is wiped, and the field takes typing again;
//!   * it could not answer this time: it crashed or hung. The password is
//!     kept, in its page, and tried again after `lock.retry.seconds`,
//!     doubling up to `max_seconds` across checks until an answer comes, at
//!     most `lock.retry.attempts` times; then it is wiped.
//!
//!   `tests::a_refused_check_counts_toward_the_lockout_and_wipes_the_password`,
//!   `tests::an_unavailable_check_is_retried_at_most_the_settings_times`,
//!   `tests::a_lasting_cause_wipes_the_password_at_once`.
//! * `unlocking`: the helper said yes, and Rust unlocked.
//!
//! `tests::a_yes_unlocks_and_a_no_counts_an_attempt`,
//! `tests::a_no_waits_out_pams_delay_while_checking`,
//! `tests::lockout_and_its_backoff_follow_the_settings`,
//! `tests::an_unavailable_check_is_retried_with_backoff_and_recovers`,
//! `tests::the_secret_is_zeroed_after_submit_and_on_unlock`.
//!
//! **A PIN, where one is set** (`crate::pin`). While the lock takes one,
//! `Lock.mode` reads `"pin"`, and what is typed goes into the same buffer as
//! a password. Submitted, it is checked as the PIN when it has a PIN's shape
//! (`lock.pin.min_length` to `max_length` digits), by the helper against the
//! PIN file, and as the password otherwise, so the password unlocks whatever
//! the mode. A wrong PIN counts toward the lockout as a wrong password does,
//! and toward `lock.pin.attempts` in the state file, which outlives the lock
//! and Solium; when the PIN is not taken (too many wrong ones, a restart since
//! the password was last used, `lock.pin.password_after_hours`), `Lock.mode`
//! reads `"password"` and `Lock.message` says why, and only the password's
//! unlock lets a PIN be taken again.
//! `tests::a_wrong_pin_counts_and_after_attempts_only_the_password_unlocks`,
//! `tests::a_restart_needs_the_password`,
//! `tests::the_password_is_needed_after_password_after_hours`,
//! `tests::the_pin_counts_survive_a_restart`,
//! `tests::the_password_unlocks_in_pin_mode_and_resets_the_counts`.
//!
//! **The layout a password is typed in.** With `lock.secret_layout = "first"`
//! (the default), taking the lock switches to the keymap's first layout that
//! types Latin letters, and ending it switches back, so a password is not
//! typed in Cyrillic because Russian was live when the lid closed.
//! `tests::with_us_ru_on_russian_a_password_is_typed_in_latin_and_russian_comes_back`.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use smithay::input::keyboard::{Keysym, KeysymHandle, ModifiersState, xkb};

use crate::{
    auth::{Outcome, Secret},
    json::Json,
    state::Solium,
};

/// `lock.*` for Solium's own lock: how a password is checked. Handed over by
/// `sol.lock`, beside the locker settings `logind.rs` reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Settings {
    /// When a run of wrong passwords shuts the field, and for how long.
    pub(crate) lockout: Lockout,
    /// Which layout a password is typed in.
    pub(crate) secret_layout: SecretLayout,
    /// How long the helper has to answer before it is killed and the check
    /// is unavailable: `lock.helper_timeout_ms`.
    pub(crate) helper_timeout: Duration,
    /// How soon an unavailable check is tried again: `lock.retry`.
    pub(crate) retry: Backoff,
    /// How many times it is tried again before the password is wiped:
    /// `lock.retry.attempts`.
    pub(crate) retries: u32,
    /// The rows `SecretKeypad` lays out: `lock.keypad`.
    pub(crate) keypad: Keypad,
    /// Whether, and when, a PIN is taken: `lock.pin`.
    pub(crate) pin: crate::pin::Settings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            lockout: Lockout::default(),
            secret_layout: SecretLayout::First,
            helper_timeout: Duration::from_millis(30_000),
            retry: Backoff {
                seconds: 2,
                max_seconds: 60,
            },
            retries: 5,
            keypad: Keypad::Digits,
            pin: crate::pin::Settings::default(),
        }
    }
}

/// `lock.lockout`: after `attempts` wrong passwords in a row the field is
/// shut, for `wait`'s first step, then twice as long after each lockout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Lockout {
    pub(crate) attempts: u32,
    pub(crate) wait: Backoff,
}

impl Default for Lockout {
    fn default() -> Self {
        Self {
            attempts: 3,
            wait: Backoff {
                seconds: 30,
                max_seconds: 300,
            },
        }
    }
}

/// A wait that doubles each time, from `seconds` up to `max_seconds`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Backoff {
    pub(crate) seconds: u32,
    pub(crate) max_seconds: u32,
}

impl Backoff {
    /// The wait after `before` earlier ones.
    fn after(self, before: u32) -> Duration {
        let doubled = u64::from(self.seconds).saturating_mul(1_u64 << before.min(32));
        let cap = u64::from(self.max_seconds.max(self.seconds));
        Duration::from_secs(doubled.min(cap))
    }
}

/// `lock.secret_layout`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SecretLayout {
    /// The keymap's first layout that types Latin letters while locked, and
    /// the layout that was live before, after.
    First,
    /// Whatever layout is live.
    Current,
}

impl SecretLayout {
    pub(crate) fn named(name: &str) -> Option<Self> {
        match name {
            "first" => Some(Self::First),
            "current" => Some(Self::Current),
            _ => None,
        }
    }
}

/// `lock.keypad`: the keys `SecretKeypad` lays out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Keypad {
    /// A phone's passcode pad: 1 to 9, then delete, 0 and OK.
    Digits,
    /// The same, with the phone letters under the digits.
    Letters,
    /// The digits, the three rows of letters, a space and OK.
    Keyboard,
}

/// What a keypad key does.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PadKey {
    Char(char),
    Erase,
    Submit,
}

/// One key as the scene's delegate is given it: what does it, its face, the
/// letters under it.
struct Face {
    key: PadKey,
    label: &'static str,
    letters: &'static str,
}

const fn face(key: PadKey, label: &'static str, letters: &'static str) -> Face {
    Face {
        key,
        label,
        letters,
    }
}

impl Keypad {
    pub(crate) fn named(name: &str) -> Option<Self> {
        match name {
            "digits" => Some(Self::Digits),
            "letters" => Some(Self::Letters),
            "keyboard" => Some(Self::Keyboard),
            _ => None,
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Digits => "digits",
            Self::Letters => "letters",
            Self::Keyboard => "keyboard",
        }
    }

    /// The rows, top to bottom. Every row is as tall as the others, and
    /// every key in a row as wide as the others in it, which is how
    /// `SecretKeypad` places its delegates and finds the key at a point.
    fn rows(self) -> Vec<Vec<Face>> {
        let letters = matches!(self, Self::Letters);
        let under = |text: &'static str| if letters { text } else { "" };
        let digit = |c: char, label: &'static str, sub: &'static str| {
            face(PadKey::Char(c), label, under(sub))
        };
        match self {
            Self::Digits | Self::Letters => vec![
                vec![
                    digit('1', "1", ""),
                    digit('2', "2", "ABC"),
                    digit('3', "3", "DEF"),
                ],
                vec![
                    digit('4', "4", "GHI"),
                    digit('5', "5", "JKL"),
                    digit('6', "6", "MNO"),
                ],
                vec![
                    digit('7', "7", "PQRS"),
                    digit('8', "8", "TUV"),
                    digit('9', "9", "WXYZ"),
                ],
                vec![
                    face(PadKey::Erase, "Delete", ""),
                    digit('0', "0", ""),
                    face(PadKey::Submit, "OK", ""),
                ],
            ],
            Self::Keyboard => {
                const KEYS: [&str; 36] = [
                    "1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "q", "w", "e", "r", "t", "y",
                    "u", "i", "o", "p", "a", "s", "d", "f", "g", "h", "j", "k", "l", "z", "x", "c",
                    "v", "b", "n", "m",
                ];
                let row = |range: std::ops::Range<usize>| -> Vec<Face> {
                    KEYS.get(range)
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|label| {
                            let c = label.chars().next()?;
                            Some(face(PadKey::Char(c), label, ""))
                        })
                        .collect()
                };
                let mut bottom = row(29..36);
                bottom.push(face(PadKey::Erase, "Delete", ""));
                vec![
                    row(0..10),
                    row(10..20),
                    row(20..29),
                    bottom,
                    vec![
                        face(PadKey::Char(' '), "space", ""),
                        face(PadKey::Submit, "OK", ""),
                    ],
                ]
            }
        }
    }

    /// What the key at `index`, counted row by row from 0, does.
    pub(crate) fn key(self, index: usize) -> Option<PadKey> {
        self.rows()
            .into_iter()
            .flatten()
            .nth(index)
            .map(|face| face.key)
    }

    /// The rows as the scene's delegates are given them: faces and kinds,
    /// never the character a key types, which only its face says.
    fn json(self) -> Json {
        Json::List(
            self.rows()
                .into_iter()
                .map(|row| {
                    Json::List(
                        row.into_iter()
                            .map(|face| {
                                let kind = match face.key {
                                    PadKey::Char(_) => "key",
                                    PadKey::Erase => "erase",
                                    PadKey::Submit => "submit",
                                };
                                Json::Object(
                                    [
                                        ("label", Json::Text(face.label.to_owned())),
                                        ("letters", Json::Text(face.letters.to_owned())),
                                        ("kind", Json::Text(kind.to_owned())),
                                    ]
                                    .into_iter()
                                    .map(|(key, value)| (key.to_owned(), value))
                                    .collect(),
                                )
                            })
                            .collect(),
                    )
                })
                .collect(),
        )
    }
}

/// `Lock.state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Idle,
    Prompting,
    Checking,
    Failed,
    LockedOut,
    Unavailable,
    Unlocking,
}

impl Phase {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Prompting => "prompting",
            Self::Checking => "checking",
            Self::Failed => "failed",
            Self::LockedOut => "lockedOut",
            Self::Unavailable => "unavailable",
            Self::Unlocking => "unlocking",
        }
    }
}

/// One key's edit of the field.
#[derive(Clone, Copy)]
pub(crate) enum Edit {
    /// A character typed.
    Type(char),
    /// Backspace: the last character, or the selection.
    Erase,
    /// `ctrl+BackSpace` and `ctrl+u`: everything.
    EraseAll,
    /// `ctrl+a`: select everything, so the next key replaces it.
    SelectAll,
    /// Escape: everything, and an unavailable check's retries with it.
    Clear,
    /// Return.
    Submit,
}

/// Never the character: an edit is never formatted with what it types.
impl std::fmt::Debug for Edit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Type(_) => "Type(..)",
            Self::Erase => "Erase",
            Self::EraseAll => "EraseAll",
            Self::SelectAll => "SelectAll",
            Self::Clear => "Clear",
            Self::Submit => "Submit",
        })
    }
}

/// What a key pressed at the native lock does to the field, from the
/// compositor's own xkb state: the character it types in the live layout,
/// or an edit. `None` for a key that does nothing there: a modifier, an
/// arrow, a combination with Control, Alt or Super that is not an edit.
///
/// Paste is not one of them: the clipboard belongs to a client behind the
/// lock, and a lock that read it would have that client answering into the
/// password field. Nor does a key repeat while held: a password is typed one
/// key at a time, and a held key repeating into it is a mistake nobody can
/// see.
pub(crate) fn edit_for(modifiers: &ModifiersState, handle: &KeysymHandle<'_>) -> Option<Edit> {
    let sym = handle.modified_sym();
    let latin = handle.raw_latin_sym_or_raw_current_sym();
    match sym {
        Keysym::Return | Keysym::KP_Enter | Keysym::ISO_Enter => return Some(Edit::Submit),
        Keysym::Escape => return Some(Edit::Clear),
        Keysym::BackSpace if modifiers.ctrl => return Some(Edit::EraseAll),
        Keysym::BackSpace => return Some(Edit::Erase),
        _ => {}
    }
    if modifiers.ctrl {
        return match latin {
            Some(Keysym::a) => Some(Edit::SelectAll),
            Some(Keysym::u) => Some(Edit::EraseAll),
            _ => None,
        };
    }
    if modifiers.alt || modifiers.logo {
        return None;
    }
    char::from_u32(xkb::keysym_to_utf32(sym))
        .filter(|c| !c.is_control())
        .map(Edit::Type)
}

/// Whose session this is, as the lock shows it: `Lock.user`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct User {
    /// The login name: what the helper checks the password against.
    name: String,
    /// The passwd entry's name, up to its first comma.
    real_name: String,
    /// AccountsService's picture, as a `file://` URL, or `""`.
    avatar: String,
}

impl User {
    /// The user this process runs as.
    fn current() -> Self {
        let (name, real_name, home) = passwd().unwrap_or_else(|| {
            (
                std::env::var("USER").unwrap_or_default(),
                String::new(),
                std::env::var("HOME").unwrap_or_default(),
            )
        });
        let avatar = [
            std::path::PathBuf::from("/var/lib/AccountsService/icons").join(&name),
            std::path::PathBuf::from(&home).join(".face"),
        ]
        .into_iter()
        .find(|path| !name.is_empty() && path.is_file())
        .map(|path| format!("file://{}", path.display()))
        .unwrap_or_default();
        Self {
            name,
            real_name,
            avatar,
        }
    }
}

/// The login name of the user this process runs as: what a password is
/// checked against.
pub(crate) fn login_name() -> String {
    User::current().name
}

/// This process's passwd entry: the login name, the name before the first
/// comma of its comment field, and its home.
#[expect(unsafe_code, reason = "reading the passwd entry through libc")]
fn passwd() -> Option<(String, String, String)> {
    // SAFETY: `passwd` is plain data, for which all zeroes is a value.
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buffer = vec![0_u8; 16 * 1024];
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: every pointer is to memory this function owns, sized as passed,
    // and alive across the call; the strings it fills are read before
    // `buffer` goes.
    let status = unsafe {
        libc::getpwuid_r(
            libc::getuid(),
            &raw mut entry,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &raw mut found,
        )
    };
    if status != 0 || found.is_null() {
        return None;
    }
    let text = |pointer: *const libc::c_char| {
        if pointer.is_null() {
            return String::new();
        }
        // SAFETY: a NUL-terminated string getpwuid_r wrote into `buffer`.
        unsafe { std::ffi::CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned()
    };
    let gecos = text(entry.pw_gecos);
    let real_name = gecos.split(',').next().unwrap_or_default().to_owned();
    Some((text(entry.pw_name), real_name, text(entry.pw_dir)))
}

/// The answers the helper's thread hands back, with the check each is for.
type Answers = Arc<Mutex<VecDeque<(u64, Outcome)>>>;

/// A stand-in for the helper, for a test: the answer to a check of a secret,
/// as a password or a PIN, given the timeout the check would have had.
#[cfg(test)]
pub(crate) type StandIn = Box<dyn FnMut(&crate::auth::Kind, &Secret, Duration) -> Outcome + Send>;

/// The native lock's password, its check, and what the lock's scenes are
/// told of them. See the module documentation.
pub(crate) struct LockAuth {
    settings: Settings,
    phase: Phase,
    /// What is being typed.
    typed: Secret,
    /// Whether all of it is selected, so the next key replaces it.
    selected: bool,
    /// What was submitted, kept while it is checked and while the helper is
    /// unavailable, so a retry has it; cleared on any answer.
    submitted: Secret,
    /// Wrong passwords since the lock was taken.
    attempts: u32,
    /// Wrong passwords since the last lockout.
    streak: u32,
    /// Lockouts since the lock was taken: what doubles the next one.
    lockouts: u32,
    /// Unavailable answers in a row, across checks, until an answer comes:
    /// what doubles the next retry.
    unanswered: u32,
    /// Retries of the password kept now.
    retried: u32,
    /// Whether the last check was refused, so the wait that follows it ends
    /// in `unavailable` rather than `failed`.
    refused: bool,
    /// Whether the check under way, or the last one, is of a PIN.
    checking_pin: bool,
    /// Whether the lock took a PIN when the last character was typed: what
    /// the scene was showing.
    typed_as_pin: bool,
    /// What is known of the PIN: whether one is set, and its counts.
    pin: crate::pin::Known,
    /// When the wait under way ends, on the compositor's clock: PAM's delay
    /// while checking, the lockout, or the next retry.
    until: Option<Duration>,
    /// The check under way. Dropping it cancels it.
    check: Option<crate::auth::Check>,
    /// Which check an answer must be for to count: each one, and each lock,
    /// moves it on.
    serial: u64,
    answers: Answers,
    /// How many keypad presses and fingers are holding a key now.
    pad_held: u32,
    /// Keypad keys pressed, ever: the scene's `keyPressed()`.
    presses: u64,
    /// The layout that was live when the lock switched to the secret one, to
    /// go back to; zero-based.
    restore_layout: Option<usize>,
    user: User,
    /// What Qt last took.
    published: Option<String>,
    /// Whether what the scenes read may have moved while Solium does not
    /// hold the lock: the lock ending, or the settings. While it holds the
    /// lock, every frame looks.
    stale: bool,
    #[cfg(test)]
    stand_in: Option<StandIn>,
    /// The wall clock a test says it is, in seconds since the epoch.
    #[cfg(test)]
    wall: Option<u64>,
}

/// Never the secrets: an attempt count and a state are all a log may hold.
impl std::fmt::Debug for LockAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LockAuth")
            .field("phase", &self.phase)
            .field("attempts", &self.attempts)
            .finish_non_exhaustive()
    }
}

impl Default for LockAuth {
    fn default() -> Self {
        Self {
            settings: Settings::default(),
            phase: Phase::Idle,
            typed: Secret::new(),
            selected: false,
            submitted: Secret::new(),
            attempts: 0,
            streak: 0,
            lockouts: 0,
            unanswered: 0,
            retried: 0,
            refused: false,
            checking_pin: false,
            typed_as_pin: false,
            pin: crate::pin::Known::new(crate::pin::Paths::of_this_user()),
            until: None,
            check: None,
            serial: 0,
            answers: Answers::default(),
            pad_held: 0,
            presses: 0,
            restore_layout: None,
            user: User::current(),
            published: None,
            stale: true,
            #[cfg(test)]
            stand_in: None,
            #[cfg(test)]
            wall: None,
        }
    }
}

impl LockAuth {
    /// Take `settings` from now on. A lock under way keeps its counts.
    pub(crate) fn configure(&mut self, settings: Settings) {
        if settings != self.settings {
            tracing::debug!(?settings, "the lock's password settings set");
            self.settings = settings;
            self.stale = true;
        }
    }

    pub(crate) fn settings(&self) -> &Settings {
        &self.settings
    }

    #[cfg(test)]
    pub(crate) fn phase(&self) -> Phase {
        self.phase
    }

    /// Wrong passwords since the lock was taken.
    #[cfg(test)]
    pub(crate) fn attempts(&self) -> u32 {
        self.attempts
    }

    /// How many characters are typed: all anything else may know of them.
    pub(crate) fn length(&self) -> usize {
        self.typed.chars()
    }

    /// Whether typing goes into the field now: and while unavailable, once
    /// no password is kept for a retry.
    pub(crate) fn accepting(&self) -> bool {
        match self.phase {
            Phase::Idle | Phase::Prompting | Phase::Failed => true,
            Phase::Unavailable => self.submitted.is_empty(),
            Phase::Checking | Phase::LockedOut | Phase::Unlocking => false,
        }
    }

    /// The lock was taken: a fresh field and fresh counts, and the PIN's
    /// files read again, so a PIN set or cleared since is seen, and the counts
    /// a restart did not reset.
    pub(crate) fn begin(&mut self) {
        self.reset();
        self.pin.refresh();
    }

    /// What the lock takes now: a PIN, or the password and why.
    pub(crate) fn mode(&self) -> crate::pin::Mode {
        self.pin.mode(&self.settings.pin, self.wall_now())
    }

    /// Seconds since the epoch: the wall clock, which the PIN's hours are
    /// counted on, since they outlive the compositor's own clock.
    fn wall_now(&self) -> u64 {
        #[cfg(test)]
        if let Some(wall) = self.wall {
            return wall;
        }
        crate::pin::wall_now()
    }

    /// The lock ended, however it ended: the field and the submitted copy
    /// wiped, and any check cancelled, its helper killed.
    pub(crate) fn end(&mut self) {
        self.reset();
    }

    fn reset(&mut self) {
        self.stale = true;
        self.check = None;
        self.serial += 1;
        self.typed.clear();
        self.submitted.clear();
        self.selected = false;
        self.phase = Phase::Idle;
        self.attempts = 0;
        self.streak = 0;
        self.lockouts = 0;
        self.unanswered = 0;
        self.retried = 0;
        self.refused = false;
        self.checking_pin = false;
        self.until = None;
        self.pad_held = 0;
    }

    /// One edit from a key, a keypad key, or the scene's `Lock.clear()` and
    /// `Lock.submit()`.
    pub(crate) fn edit(&mut self, edit: Edit, now: Duration) {
        match edit {
            Edit::Clear => self.clear(),
            Edit::Submit => self.submit(now),
            _ if !self.accepting() => {}
            Edit::Type(c) => {
                if self.selected {
                    self.typed.clear();
                    self.selected = false;
                }
                let mut bytes = [0_u8; 4];
                // Over the page's size, the character is dropped: no password
                // that long is anyone's (`solium_auth::MAX_LEN`).
                let _ = self.typed.push_str(c.encode_utf8(&mut bytes));
                wipe(&mut bytes);
                self.typed_as_pin = self.mode() == crate::pin::Mode::Pin;
                self.phase = Phase::Prompting;
            }
            Edit::Erase => {
                if self.selected {
                    self.typed.clear();
                    self.selected = false;
                } else {
                    self.typed.pop_char();
                }
                self.settle_typed();
            }
            Edit::EraseAll => {
                self.typed.clear();
                self.selected = false;
                self.settle_typed();
            }
            Edit::SelectAll => self.selected = !self.typed.is_empty(),
        }
    }

    /// After an erase: `idle` with nothing left, `prompting` otherwise.
    fn settle_typed(&mut self) {
        self.phase = if self.typed.is_empty() {
            Phase::Idle
        } else {
            Phase::Prompting
        };
    }

    /// Escape, or `Lock.clear()`: the field emptied. Unavailable, the kept
    /// password goes too, and with it the retries. Not while a check is
    /// under way or the field is locked out: there is nothing typed then.
    fn clear(&mut self) {
        match self.phase {
            Phase::Checking | Phase::LockedOut | Phase::Unlocking => {}
            Phase::Unavailable => {
                self.serial += 1;
                self.check = None;
                self.submitted.clear();
                self.typed.clear();
                self.selected = false;
                self.until = None;
                self.phase = Phase::Idle;
            }
            Phase::Idle | Phase::Prompting | Phase::Failed => {
                self.typed.clear();
                self.selected = false;
                self.phase = Phase::Idle;
            }
        }
    }

    /// Return, a keypad's OK, or `Lock.submit()`: what is typed goes to the
    /// helper, and the field is wiped. Nothing typed, or a field that is not
    /// taking a password, submits nothing.
    ///
    /// Taking a PIN, what has a PIN's shape is checked as the PIN, and
    /// anything else as the password. The PIN's files are read again first:
    /// a PIN-shaped entry made while the lock took a PIN, which no longer
    /// does (a PIN cleared meanwhile, its hours run out), is wiped unchecked
    /// rather than tried as the password, and the message says why.
    fn submit(&mut self, now: Duration) {
        if !self.accepting() || self.typed.is_empty() {
            return;
        }
        let pin = &self.settings.pin;
        let shaped = solium_auth::pin::shaped(&self.typed, pin.min_length, pin.max_length);
        self.pin.refresh();
        let is_pin = self.mode() == crate::pin::Mode::Pin;
        if shaped && self.typed_as_pin && !is_pin {
            self.typed.clear();
            self.selected = false;
            self.phase = Phase::Idle;
            tracing::info!("the lock stopped taking a PIN as one was typed; it was not checked");
            return;
        }
        self.checking_pin = shaped && is_pin;
        self.submitted.clear();
        // The page is UTF-8 by construction: only `push_str` writes it.
        let pushed = std::str::from_utf8(self.typed.expose())
            .is_ok_and(|text| self.submitted.push_str(text));
        self.typed.clear();
        self.selected = false;
        if !pushed {
            self.submitted.clear();
            self.phase = Phase::Idle;
            return;
        }
        // A new password's retries; the backoff goes on from where the last
        // check left it, so submitting again does not hurry it.
        self.retried = 0;
        self.start_check(now);
    }

    /// Ask the helper about the submitted password, in a copy it takes.
    fn start_check(&mut self, _now: Duration) {
        self.phase = Phase::Checking;
        self.until = None;
        self.serial += 1;
        let serial = self.serial;
        let mut secret = Secret::new();
        let copied =
            std::str::from_utf8(self.submitted.expose()).is_ok_and(|text| secret.push_str(text));
        let answers = Arc::clone(&self.answers);
        let answer = move |outcome: Outcome| {
            answers
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push_back((serial, outcome));
        };
        if !copied {
            answer(Outcome::Unavailable {
                reason: "the password could not be copied for the check".to_owned(),
                lasting: false,
            });
            return;
        }
        let timeout = self.settings.helper_timeout;
        let kind = match (&self.pin.paths, self.checking_pin) {
            (Some(paths), true) => crate::auth::Kind::Pin {
                file: paths.pin.clone(),
            },
            _ => crate::auth::Kind::Password {
                user: self.user.name.clone(),
            },
        };
        #[cfg(test)]
        if let Some(stand_in) = self.stand_in.as_mut() {
            let outcome = stand_in(&kind, &secret, timeout);
            drop(secret);
            answer(outcome);
            return;
        }
        self.check = Some(crate::auth::check(kind, secret, timeout, answer));
    }

    /// The answers that came since the last call, and the waits that ended
    /// by `now`. Whether to unlock: only on the helper's yes to the check
    /// under way.
    pub(crate) fn settle(&mut self, now: Duration) -> bool {
        let answers: Vec<(u64, Outcome)> = self
            .answers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect();
        let mut unlock = false;
        for (serial, outcome) in answers {
            if serial != self.serial || self.phase != Phase::Checking || self.until.is_some() {
                continue;
            }
            self.check = None;
            match outcome {
                Outcome::Ok => {
                    self.submitted.clear();
                    self.typed.clear();
                    self.phase = Phase::Unlocking;
                    unlock = true;
                    if self.checking_pin {
                        self.pin.pin_unlocked();
                        tracing::info!("the lock's PIN check said yes");
                    } else {
                        let wall = self.wall_now();
                        self.pin.password_unlocked(wall);
                        tracing::info!("the lock's password check said yes");
                    }
                }
                Outcome::Failed { delay } => {
                    self.submitted.clear();
                    self.unanswered = 0;
                    self.refused = false;
                    self.attempts = self.attempts.saturating_add(1);
                    self.streak = self.streak.saturating_add(1);
                    if self.checking_pin {
                        self.pin.failed();
                    }
                    // Checking still, through PAM's delay: the answer shows
                    // when it is over.
                    self.until = Some(now + delay);
                    tracing::info!(
                        attempts = self.attempts,
                        pin = self.checking_pin,
                        "the lock's check said no"
                    );
                }
                Outcome::Unavailable { reason, .. } if self.pin_gone() => {
                    // No PIN to check it against any more: it was cleared, or
                    // its file was spoiled, since the lock was taken. Nothing
                    // to try again; the password is asked for.
                    self.submitted.clear();
                    self.unanswered = 0;
                    self.phase = Phase::Idle;
                    tracing::warn!(reason, "the lock's PIN could not be checked");
                }
                Outcome::Refused { reason } => {
                    // PAM had it: a wrong attempt, which the lockout counts,
                    // and never tried again.
                    self.submitted.clear();
                    self.unanswered = 0;
                    self.refused = true;
                    self.attempts = self.attempts.saturating_add(1);
                    self.streak = self.streak.saturating_add(1);
                    self.until = Some(now);
                    tracing::warn!(
                        reason,
                        attempts = self.attempts,
                        "PAM would not say yes or no to the lock's password; it counts as an attempt"
                    );
                }
                Outcome::Unavailable {
                    reason,
                    lasting: true,
                } => {
                    self.submitted.clear();
                    self.unanswered = 0;
                    self.phase = Phase::Unavailable;
                    tracing::error!(
                        reason,
                        "the lock's password cannot be checked, and trying again will not mend it"
                    );
                }
                Outcome::Unavailable {
                    reason,
                    lasting: false,
                } => {
                    self.phase = Phase::Unavailable;
                    let wait = self.settings.retry.after(self.unanswered);
                    self.unanswered = self.unanswered.saturating_add(1);
                    if self.retried < self.settings.retries {
                        self.retried = self.retried.saturating_add(1);
                        self.until = Some(now + wait);
                        tracing::warn!(
                            reason,
                            retry_in_s = wait.as_secs(),
                            "the lock's password could not be checked"
                        );
                    } else {
                        self.submitted.clear();
                        tracing::error!(
                            reason,
                            retries = self.retried,
                            "the lock's password could not be checked, and is no longer kept"
                        );
                    }
                }
            }
        }
        if unlock {
            return true;
        }
        let Some(until) = self.until.filter(|until| now >= *until) else {
            return false;
        };
        self.until = None;
        match self.phase {
            Phase::Checking => {
                if self.streak >= self.settings.lockout.attempts.max(1) {
                    let wait = self.settings.lockout.wait.after(self.lockouts);
                    self.lockouts = self.lockouts.saturating_add(1);
                    self.streak = 0;
                    self.phase = Phase::LockedOut;
                    self.until = Some(until.max(now) + wait);
                    tracing::info!(
                        seconds = wait.as_secs(),
                        "too many wrong passwords: the lock's field is shut"
                    );
                } else if self.refused {
                    self.phase = Phase::Unavailable;
                } else {
                    self.phase = Phase::Failed;
                }
                self.refused = false;
            }
            Phase::LockedOut => self.phase = Phase::Idle,
            Phase::Unavailable => self.start_check(now),
            Phase::Idle | Phase::Prompting | Phase::Failed | Phase::Unlocking => {}
        }
        false
    }

    /// After a PIN check that could not be answered: whether that is because
    /// there is no PIN to check against any more, read from the files again.
    fn pin_gone(&mut self) -> bool {
        if !self.checking_pin {
            return false;
        }
        self.pin.refresh();
        self.mode() != crate::pin::Mode::Pin
    }

    /// Whole seconds until the wait under way ends, for a state that shows
    /// one: locked out, or unavailable until the next try.
    fn retry_after(&self, now: Duration) -> u64 {
        match (self.phase, self.until) {
            (Phase::LockedOut | Phase::Unavailable, Some(until)) => {
                let left = until.saturating_sub(now);
                left.as_secs() + u64::from(left.subsec_nanos() > 0)
            }
            _ => 0,
        }
    }

    /// The line that goes with the state. Unavailable, it is the way out
    /// from another console: logind's `Unlock` while `lock.logind_unlock`
    /// honours it, and ending the session while it does not.
    ///
    /// While the lock asks for the password where it would take a PIN, the
    /// line says why, in place of nothing; a wrong PIN is "Wrong PIN", unless
    /// it was the last one the lock takes.
    fn message(&self, logind_unlock: bool) -> String {
        let fallback = match self.mode() {
            crate::pin::Mode::Password(fallback) => Some(fallback.message(&self.settings.pin)),
            crate::pin::Mode::Pin | crate::pin::Mode::NoPin => None,
        };
        match self.phase {
            Phase::Failed if self.checking_pin => {
                fallback.unwrap_or_else(|| "Wrong PIN".to_owned())
            }
            Phase::Failed => "Wrong password".to_owned(),
            Phase::LockedOut => "Too many attempts".to_owned(),
            Phase::Unavailable => {
                let id = std::env::var("XDG_SESSION_ID")
                    .ok()
                    .filter(|id| !id.is_empty());
                let command = match (logind_unlock, id) {
                    (true, Some(id)) => format!("loginctl unlock-session {id}"),
                    (true, None) => "loginctl unlock-sessions".to_owned(),
                    (false, Some(id)) => format!("loginctl terminate-session {id}"),
                    (false, None) => "loginctl terminate-user $USER".to_owned(),
                };
                format!("From another console: {command}")
            }
            Phase::Idle | Phase::Prompting | Phase::Checking => fallback.unwrap_or_default(),
            Phase::Unlocking => String::new(),
        }
    }

    /// A keypad key pressed, by a press or a finger: held until
    /// [`Self::keypad_up`].
    pub(crate) fn keypad_down(&mut self, index: usize, now: Duration) {
        self.pad_held = self.pad_held.saturating_add(1);
        let Some(key) = self.settings.keypad.key(index) else {
            return;
        };
        self.presses = self.presses.wrapping_add(1);
        let edit = match key {
            PadKey::Char(c) => Edit::Type(c),
            PadKey::Erase => Edit::Erase,
            PadKey::Submit => Edit::Submit,
        };
        self.edit(edit, now);
    }

    pub(crate) fn keypad_up(&mut self) {
        self.pad_held = self.pad_held.saturating_sub(1);
    }

    /// What `Lock`, `SecretField` and `SecretKeypad` read, as one JSON
    /// object: `scene` is what the scenes are told of the lock itself.
    fn view(&self, now: Duration, scene: &SceneView) -> String {
        let text = |value: &str| Json::Text(value.to_owned());
        let object = |pairs: Vec<(&str, Json)>| {
            Json::Object(
                pairs
                    .into_iter()
                    .map(|(key, value)| (key.to_owned(), value))
                    .collect(),
            )
        };
        let number = |value: u64| Json::Number(value as f64);
        // Through the exit the session is unlocked, and the state is the
        // one that says so, whatever the field was reset to; through the
        // entry, the one that says the lock is coming down; through a peek,
        // the one that says it is only being pulled down, and is no lock.
        let state = if scene.peeking {
            "peeking"
        } else if scene.exiting {
            Phase::Unlocking.name()
        } else if scene.entering {
            "locking"
        } else {
            self.phase.name()
        };
        object(vec![
            ("state", text(state)),
            ("mode", text(self.mode().name())),
            ("locked", Json::Bool(scene.locked)),
            ("progress", Json::Number(scene.progress)),
            ("enterMs", number(scene.enter_ms)),
            ("exitMs", number(scene.exit_ms)),
            ("attempts", number(u64::from(self.attempts))),
            ("retryAfter", number(self.retry_after(now))),
            ("message", text(&self.message(scene.logind_unlock))),
            (
                "user",
                object(vec![
                    ("name", text(&self.user.name)),
                    ("realName", text(&self.user.real_name)),
                    ("avatar", text(&self.user.avatar)),
                ]),
            ),
            ("promptOn", text(&scene.prompt_on)),
            (
                "field",
                object(vec![
                    ("length", number(self.length() as u64)),
                    ("selected", Json::Bool(self.selected)),
                    ("accepting", Json::Bool(self.accepting())),
                ]),
            ),
            (
                "keypad",
                object(vec![
                    ("layout", text(self.settings.keypad.name())),
                    ("pressed", Json::Bool(self.pad_held > 0)),
                    ("presses", number(self.presses)),
                    ("rows", self.settings.keypad.json()),
                ]),
            ),
        ])
        .render()
    }

    /// What the scenes read may have moved while Solium does not hold the
    /// lock: the exit ended.
    pub(crate) fn touch(&mut self) {
        self.stale = true;
    }

    /// Answer every check with `stand_in` rather than the helper.
    #[cfg(test)]
    pub(crate) fn use_stand_in(&mut self, stand_in: StandIn) {
        self.stand_in = Some(stand_in);
    }

    /// Read the PIN's files from `paths`, as this user's would be, and take
    /// the wall clock to be `wall`.
    #[cfg(test)]
    pub(crate) fn use_pin_for_test(&mut self, paths: crate::pin::Paths, wall: u64) {
        self.pin = crate::pin::Known::new(Some(paths));
        self.wall = Some(wall);
        self.pin.refresh();
        self.stale = true;
    }

    /// Move the wall clock a test set on.
    #[cfg(test)]
    pub(crate) fn set_wall_for_test(&mut self, wall: u64) {
        self.wall = Some(wall);
    }

    /// Whether the field's page, and the submitted copy's, are all zeroes.
    #[cfg(test)]
    pub(crate) fn wiped_for_test(&self) -> (bool, bool) {
        (self.typed.is_wiped(), self.submitted.is_wiped())
    }
}

/// What the lock's scenes are told of the lock itself, beside the password's
/// state.
struct SceneView {
    /// Whether Solium holds the lock: `Lock.locked`.
    locked: bool,
    /// Whether its entry is coming down over the desktop, which reads as
    /// `"locking"`.
    entering: bool,
    /// Whether the configuration is pulling the scenes down over the
    /// unlocked desktop, which reads as `"peeking"`.
    peeking: bool,
    /// `Lock.progress`, 0 to 1: how far down a peek is, and through the
    /// entry that takes the lock from one, where that peek was; 0 otherwise.
    progress: f64,
    /// `lock.enter_ms`: `Lock.enterMs`.
    enter_ms: u64,
    /// Whether its exit is playing over the desktop, which reads as
    /// `"unlocking"`.
    exiting: bool,
    /// `lock.exit_ms`: `Lock.exitMs`.
    exit_ms: u64,
    /// The monitor the prompt is on: `Lock.promptOn`.
    prompt_on: String,
    /// Whether logind's `Unlock` ends the lock: `lock.logind_unlock`.
    logind_unlock: bool,
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

#[expect(unsafe_code, reason = "the Qt host is C++; this is its C ABI")]
mod ffi {
    use std::ffi::{c_char, c_int};

    unsafe extern "C" {
        pub(super) fn solium_qml_lock_publish(json: *const c_char) -> c_int;
        pub(super) fn solium_qml_lock_take_requests() -> c_int;
        #[cfg(test)]
        pub(super) fn solium_qml_lock_api_for_test(which: *const c_char) -> *const c_char;
    }
}

/// `Lock.submit()` and `Lock.clear()` since the last call: `(submit, clear)`.
#[expect(unsafe_code, reason = "calling into the Qt host")]
fn take_requests() -> (bool, bool) {
    // SAFETY: no arguments; the host answers 0 before Qt has started.
    let bits = unsafe { ffi::solium_qml_lock_take_requests() };
    (bits & 1 != 0, bits & 2 != 0)
}

#[expect(unsafe_code, reason = "calling into the Qt host")]
fn apply(json: &str) -> bool {
    crate::qml::hosted::touched();
    let Ok(json) = std::ffi::CString::new(json) else {
        return false;
    };
    // SAFETY: `json` outlives the call; the host copies what it keeps.
    unsafe { ffi::solium_qml_lock_publish(json.as_ptr()) != 0 }
}

/// What a native type declares, one line each, from the host.
#[cfg(test)]
#[expect(unsafe_code, reason = "calling into the Qt host")]
fn api_for_test(which: &std::ffi::CStr) -> String {
    // SAFETY: `which` outlives the call; the host answers a string it keeps
    // until the next call, read at once.
    let text = unsafe { ffi::solium_qml_lock_api_for_test(which.as_ptr()) };
    if text.is_null() {
        return String::new();
    }
    // SAFETY: a NUL-terminated string the host keeps until the next call.
    unsafe { std::ffi::CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned()
}

/// Once a loop, in both backends, beside `logind::settle`: the scene's
/// requests, the helper's answers and the waits that ended. The one place a
/// check's yes unlocks. Nothing, while Solium does not hold the lock, but
/// requests and answers are drained so none outlives its lock.
/// `tests::a_yes_unlocks_and_a_no_counts_an_attempt`.
pub(crate) fn settle(state: &mut Solium) {
    let (submit, clear) = take_requests();
    let native = state
        .lock
        .as_ref()
        .is_some_and(crate::lock::Lock::is_native);
    let now = state.clock.now();
    if native {
        if clear {
            state.lock_auth.edit(Edit::Clear, now);
        }
        if submit {
            state.lock_auth.edit(Edit::Submit, now);
        }
    }
    let unlock = state.lock_auth.settle(now);
    if unlock && native {
        state.unlock_natively();
    }
    if native {
        // A count going down, a state changing: the scenes see it on the
        // next frame, so there has to be one.
        state.lock_auth_changed();
    }
}

/// Send what the lock's scenes read, if it moved since Qt last took it:
/// looked at every frame while Solium holds the lock, and otherwise only
/// once something has moved it.
pub(crate) fn publish(state: &mut Solium) {
    let native = state
        .lock
        .as_ref()
        .is_some_and(crate::lock::Lock::is_native);
    if !native && !state.lock_auth.stale {
        return;
    }
    let view = state.lock_view();
    if state.lock_auth.published.as_deref() == Some(view.as_str()) {
        state.lock_auth.stale = false;
        return;
    }
    if apply(&view) {
        state.lock_auth.published = Some(view);
        state.lock_auth.stale = false;
    }
}

impl Solium {
    /// What the lock's scenes would read now.
    fn lock_view(&self) -> String {
        let now = self.clock.now();
        let exit = self.lock_scenes.settings().exit;
        let enter = self.lock_scenes.settings().enter;
        let locked = self.lock.as_ref().is_some_and(crate::lock::Lock::is_native);
        let entering = locked && self.lock_scenes.entering(now);
        let peek = self
            .lock
            .is_none()
            .then(|| self.lock_scenes.peeking())
            .flatten();
        let progress = match peek {
            Some(progress) => progress,
            None if entering => self.lock_scenes.entry_from(),
            None => 0.0,
        };
        let scene = SceneView {
            locked,
            entering,
            peeking: peek.is_some(),
            progress,
            enter_ms: u64::try_from(enter.as_millis()).unwrap_or(u64::MAX),
            exiting: self.lock.is_none() && self.lock_scenes.exiting(now),
            exit_ms: u64::try_from(exit.as_millis()).unwrap_or(u64::MAX),
            prompt_on: self.lock_prompt_on(),
            logind_unlock: self.logind.settings().logind_unlock,
        };
        self.lock_auth.view(now, &scene)
    }

    /// Ask for a frame if what the lock's scenes read has moved: a count
    /// going down, a state changing, a dot more.
    pub(crate) fn lock_auth_changed(&mut self) {
        if self.lock_auth.published.as_deref() != Some(self.lock_view().as_str()) {
            self.redraw = true;
        }
    }

    /// A key at the native lock, from the keyboard filter.
    pub(crate) fn secret_edit(&mut self, edit: Edit) {
        let now = self.clock.now();
        self.lock_auth.edit(edit, now);
        self.lock_auth_changed();
    }

    /// A keypad key pressed, by the pointer or a finger, at the native lock.
    pub(crate) fn keypad_down(&mut self, index: usize) {
        let now = self.clock.now();
        self.lock_auth.keypad_down(index, now);
        self.redraw = true;
    }

    /// That press or finger let go.
    pub(crate) fn keypad_up(&mut self) {
        self.lock_auth.keypad_up();
        self.redraw = true;
    }

    /// Solium took the lock: a fresh field, and the secret layout if the
    /// settings ask for it.
    pub(crate) fn begin_lock_auth(&mut self) {
        self.lock_auth.begin();
        self.lock_auth.restore_layout = None;
        if self.lock_auth.settings().secret_layout == SecretLayout::First
            && let Some(latin) = crate::keymap::first_latin(self)
        {
            let live = crate::keymap::live(self).0.saturating_sub(1);
            if live != latin && self.set_layout(latin) {
                self.lock_auth.restore_layout = Some(live);
                tracing::debug!("the lock switched to a Latin layout for the password");
            }
        }
        self.lock_auth_changed();
    }

    /// The lock ended: the field wiped, and the layout that was live before
    /// the lock is live again.
    pub(crate) fn end_lock_auth(&mut self) {
        self.lock_auth.end();
        if let Some(layout) = self.lock_auth.restore_layout.take() {
            self.set_layout(layout);
        }
        self.lock_auth_changed();
    }

    /// Make the zero-based `layout` live.
    fn set_layout(&mut self, layout: usize) -> bool {
        let changed = crate::keymap::apply(
            self,
            &crate::keymap::Request {
                active: Some(layout + 1),
                ..Default::default()
            },
        );
        self.keyboard_changed();
        changed
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use smithay::{
        backend::input::ButtonState,
        output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
        reexports::wayland_server::Display,
        utils::{Logical, Point, Rectangle},
    };

    use super::*;
    use crate::keymap::tests::{ALT, SHIFT, tap, us_ru};

    /// The passwords the stand-in says yes to: a phrase, and digits typed on
    /// a keypad.
    const RIGHT: &str = "correct horse";
    const PIN: &str = "2580";
    /// The PIN a test that sets one sets, which is not the password.
    const SET_PIN: &str = "1379";
    /// The wall clock a test with a PIN starts at, and the password unlock
    /// its record says happened then.
    const WALL: u64 = 1_761_300_000;

    /// What the stand-in was asked, and what it is to answer first.
    #[derive(Default)]
    struct Asked {
        /// The timeout each check was given.
        timeouts: Vec<Duration>,
        /// Answers to give, in order, before judging by the password.
        scripted: VecDeque<Outcome>,
        /// What each check was of: `"password"` or `"pin"`.
        kinds: Vec<&'static str>,
    }

    type Shared = Arc<Mutex<Asked>>;

    fn asked(shared: &Shared) -> std::sync::MutexGuard<'_, Asked> {
        shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A stand-in for the helper, unless an answer is scripted: as a
    /// password, yes to [`RIGHT`] and [`PIN`] and no with PAM's two-second
    /// delay to anything else; as a PIN, what the helper's own check of the
    /// PIN file says (`solium_auth::pin::check`), which no PAM is part of.
    fn stand_in(shared: &Shared) -> StandIn {
        let shared = Arc::clone(shared);
        Box::new(move |kind, secret, timeout| {
            let mut asked = asked(&shared);
            asked.timeouts.push(timeout);
            asked.kinds.push(match kind {
                crate::auth::Kind::Password { .. } => "password",
                crate::auth::Kind::Pin { .. } => "pin",
            });
            if let Some(outcome) = asked.scripted.pop_front() {
                return outcome;
            }
            match kind {
                crate::auth::Kind::Pin { file } => match solium_auth::pin::check(file, secret) {
                    solium_auth::Answer::Ok => Outcome::Ok,
                    solium_auth::Answer::Failed { delay_ms } => Outcome::Failed {
                        delay: Duration::from_millis(u64::from(delay_ms)),
                    },
                    solium_auth::Answer::Refused { reason } => Outcome::Refused { reason },
                    solium_auth::Answer::Unavailable { reason } => Outcome::Unavailable {
                        reason,
                        lasting: false,
                    },
                },
                crate::auth::Kind::Password { .. }
                    if secret.expose() == RIGHT.as_bytes() || secret.expose() == PIN.as_bytes() =>
                {
                    Outcome::Ok
                }
                crate::auth::Kind::Password { .. } => Outcome::Failed {
                    delay: Duration::from_secs(2),
                },
            }
        })
    }

    /// [`SET_PIN`] set in a directory of the test's own, as `solium pin set`
    /// leaves it: its hash (a cheap one), and a record of a password unlock
    /// at [`WALL`] on the boot `boot-one`, which the boot id file names.
    fn pin_set(test: &str) -> (PathBuf, crate::pin::Paths) {
        let dir =
            std::env::temp_dir().join(format!("solium-lock-pin-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a directory");
        let mut paths = crate::pin::Paths::in_dir(&dir.join("solium"));
        paths.boot_id = dir.join("boot_id");
        std::fs::write(&paths.boot_id, "boot-one\n").expect("a boot id");
        let mut pin = Secret::new();
        assert!(pin.push_str(SET_PIN));
        let phc = solium_auth::pin::hash_with(&pin, 64, 1).expect("a hash");
        solium_auth::pin::write_private(&paths.pin, phc.as_bytes()).expect("the PIN file");
        crate::pin::Record::after_password(WALL, Some("boot-one".to_owned()))
            .write(&paths.state)
            .expect("the state file");
        (dir, paths)
    }

    /// A lock's password state with [`SET_PIN`] set, at [`WALL`].
    fn pin_auth(test: &str, settings: Settings) -> (LockAuth, Shared, PathBuf, crate::pin::Paths) {
        let (mut auth, shared) = auth(settings);
        let (dir, paths) = pin_set(test);
        auth.use_pin_for_test(paths.clone(), WALL);
        auth.begin();
        (auth, shared, dir, paths)
    }

    /// The record in the state file now.
    fn record(paths: &crate::pin::Paths) -> crate::pin::Record {
        crate::pin::Record::read(&paths.state).0
    }

    fn last_kind(shared: &Shared) -> &'static str {
        asked(shared).kinds.last().copied().unwrap_or("none")
    }

    /// A lock's password state on its own, with `settings` and the stand-in,
    /// as a native lock begins it.
    fn auth(settings: Settings) -> (LockAuth, Shared) {
        let shared = Shared::default();
        let mut auth = LockAuth::default();
        auth.configure(settings);
        auth.use_stand_in(stand_in(&shared));
        auth.begin();
        (auth, shared)
    }

    fn type_text(auth: &mut LockAuth, text: &str, now: Duration) {
        for c in text.chars() {
            auth.edit(Edit::Type(c), now);
        }
    }

    fn submit(auth: &mut LockAuth, text: &str, now: Duration) {
        type_text(auth, text, now);
        auth.edit(Edit::Submit, now);
    }

    const fn s(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }

    /// **A no is waited out through PAM's delay while checking**, and only
    /// then shows as `failed`: the delay the helper reported, on the
    /// compositor's clock, with the field taking nothing until it is over.
    #[test]
    fn a_no_waits_out_pams_delay_while_checking() {
        let (mut auth, _) = auth(Settings::default());
        submit(&mut auth, "wrong", s(10));
        assert_eq!(auth.phase(), Phase::Checking);
        assert!(!auth.settle(s(10)), "a no unlocked");
        assert_eq!(
            auth.phase(),
            Phase::Checking,
            "the delay was not waited out"
        );
        auth.edit(Edit::Type('x'), s(11));
        assert_eq!(auth.length(), 0, "typed into the field while checking");
        assert!(!auth.settle(s(11)));
        assert_eq!(auth.phase(), Phase::Checking);
        assert!(!auth.settle(s(12)));
        assert_eq!(auth.phase(), Phase::Failed);
        assert_eq!(auth.attempts(), 1);
        assert_eq!(auth.message(true), "Wrong password");
        auth.edit(Edit::Type('a'), s(13));
        assert_eq!((auth.phase(), auth.length()), (Phase::Prompting, 1));
    }

    /// **Lockout and its backoff, every number from the settings**: two
    /// noes shut the field for five seconds, then ten, then twelve, the cap,
    /// and again twelve. Locked out, nothing is typed or checked, and
    /// `retryAfter` counts down.
    #[test]
    fn lockout_and_its_backoff_follow_the_settings() {
        let settings = Settings {
            lockout: Lockout {
                attempts: 2,
                wait: Backoff {
                    seconds: 5,
                    max_seconds: 12,
                },
            },
            ..Settings::default()
        };
        let (mut auth, shared) = auth(settings);
        let mut now = s(100);
        for (round, wait) in [5, 10, 12, 12].into_iter().enumerate() {
            for attempt in 0..2 {
                submit(&mut auth, "wrong", now);
                auth.settle(now);
                now += s(2);
                auth.settle(now);
                let expected = if attempt == 0 {
                    Phase::Failed
                } else {
                    Phase::LockedOut
                };
                assert_eq!(auth.phase(), expected, "round {round}, attempt {attempt}");
            }
            assert_eq!(auth.retry_after(now), wait, "round {round}");
            let checks = asked(&shared).timeouts.len();
            submit(&mut auth, RIGHT, now);
            assert_eq!(auth.length(), 0, "typed while locked out");
            assert_eq!(
                asked(&shared).timeouts.len(),
                checks,
                "checked while locked out"
            );
            now += s(wait - 1);
            auth.settle(now);
            assert_eq!(auth.phase(), Phase::LockedOut, "round {round}: out early");
            assert_eq!(auth.retry_after(now), 1);
            now += s(1);
            auth.settle(now);
            assert_eq!(auth.phase(), Phase::Idle, "round {round}: still out");
        }
        assert_eq!(auth.attempts(), 8);
        submit(&mut auth, RIGHT, now);
        assert!(auth.settle(now), "the right password after the lockouts");
    }

    /// **Unavailable, and recovery**: a check the helper cannot answer is
    /// `unavailable`, with the way out from another console, and the same
    /// password is tried again after three seconds, six, then seven, the
    /// cap, until an answer comes; the yes then unlocks. Cleared, the
    /// retries stop and the field takes typing again.
    #[test]
    fn an_unavailable_check_is_retried_with_backoff_and_recovers() {
        let settings = Settings {
            retry: Backoff {
                seconds: 3,
                max_seconds: 7,
            },
            ..Settings::default()
        };
        let (mut first, shared) = auth(settings.clone());
        let unavailable = || Outcome::Unavailable {
            reason: "solium-auth crashed".to_owned(),
            lasting: false,
        };
        asked(&shared).scripted = [unavailable(), unavailable(), unavailable()].into();
        submit(&mut first, RIGHT, s(0));
        assert!(!first.settle(s(0)), "unavailable unlocked");
        assert_eq!(first.phase(), Phase::Unavailable);
        assert!(!first.accepting());
        assert!(
            first.message(true).contains("loginctl unlock-session"),
            "no way out: {:?}",
            first.message(true)
        );
        assert!(
            first.message(false).contains("loginctl terminate-"),
            "a way out that `lock.logind_unlock = false` ignores: {:?}",
            first.message(false)
        );
        let mut now = s(0);
        for wait in [3, 6, 7] {
            assert_eq!(first.retry_after(now), wait);
            assert!(!first.settle(now + s(wait - 1)));
            assert_eq!(first.phase(), Phase::Unavailable, "tried early");
            now += s(wait);
            let unlocked = first.settle(now) | first.settle(now);
            if wait < 7 {
                assert!(!unlocked);
                assert_eq!(first.phase(), Phase::Unavailable);
            } else {
                assert!(
                    unlocked,
                    "the helper's yes after it came back did not unlock"
                );
                assert_eq!(first.phase(), Phase::Unlocking);
            }
        }
        assert_eq!(asked(&shared).timeouts.len(), 4);

        let (mut second, shared) = auth(settings);
        asked(&shared).scripted = std::iter::repeat_with(unavailable).take(10).collect();
        submit(&mut second, RIGHT, s(0));
        second.settle(s(0));
        assert_eq!(second.phase(), Phase::Unavailable);
        second.edit(Edit::Clear, s(1));
        assert_eq!(second.phase(), Phase::Idle);
        assert_eq!(
            second.wiped_for_test(),
            (true, true),
            "the kept password stayed"
        );
        second.settle(s(100));
        second.settle(s(100));
        assert_eq!(
            asked(&shared).timeouts.len(),
            1,
            "retried after it was cleared"
        );
        assert!(second.accepting());
    }

    /// **A refused check counts toward the lockout, and wipes the
    /// password**: PAM asking for a second password is a wrong attempt, the
    /// field shows `unavailable` with nothing kept and takes typing again,
    /// nothing is tried again behind the user's back, and two refusals with
    /// `attempts = 2` shut the field as two noes do.
    #[test]
    fn a_refused_check_counts_toward_the_lockout_and_wipes_the_password() {
        let settings = Settings {
            lockout: Lockout {
                attempts: 2,
                wait: Backoff {
                    seconds: 30,
                    max_seconds: 30,
                },
            },
            ..Settings::default()
        };
        let (mut auth, shared) = auth(settings);
        let refused = || Outcome::Refused {
            reason: "PAM asked for a second password".to_owned(),
        };
        asked(&shared).scripted = [refused(), refused()].into();
        submit(&mut auth, RIGHT, s(0));
        assert!(!auth.settle(s(0)), "a refusal unlocked");
        assert_eq!(auth.phase(), Phase::Unavailable);
        assert_eq!(auth.attempts(), 1, "a refusal is not an attempt");
        assert_eq!(auth.wiped_for_test(), (true, true), "the password was kept");
        assert!(auth.accepting(), "the field does not take typing again");
        auth.settle(s(100));
        auth.settle(s(100));
        assert_eq!(
            asked(&shared).timeouts.len(),
            1,
            "retried behind the user's back"
        );

        submit(&mut auth, RIGHT, s(100));
        auth.settle(s(100));
        assert_eq!(auth.attempts(), 2);
        assert_eq!(
            auth.phase(),
            Phase::LockedOut,
            "two refusals did not shut the field"
        );
        assert_eq!(auth.retry_after(s(100)), 30);
    }

    /// **An unavailable check is tried again at most `retry.attempts`
    /// times**, then the password is wiped and the field takes typing again;
    /// and a new submit does not start the backoff over: two retries after
    /// two and four seconds, a third unavailable wipes the password, and the
    /// next password's first retry waits sixteen.
    #[test]
    fn an_unavailable_check_is_retried_at_most_the_settings_times() {
        let settings = Settings {
            retry: Backoff {
                seconds: 2,
                max_seconds: 60,
            },
            retries: 2,
            ..Settings::default()
        };
        let (mut auth, shared) = auth(settings);
        let unavailable = || Outcome::Unavailable {
            reason: "solium-auth crashed".to_owned(),
            lasting: false,
        };
        asked(&shared).scripted = std::iter::repeat_with(unavailable).take(10).collect();
        submit(&mut auth, RIGHT, s(0));
        let mut now = s(0);
        auth.settle(now);
        for wait in [2, 4] {
            assert_eq!(auth.phase(), Phase::Unavailable);
            assert!(
                !auth.accepting(),
                "the field took typing over a kept password"
            );
            assert_eq!(auth.retry_after(now), wait);
            now += s(wait);
            auth.settle(now);
            auth.settle(now);
        }
        assert_eq!(
            asked(&shared).timeouts.len(),
            3,
            "the first check and two retries"
        );
        assert_eq!(auth.phase(), Phase::Unavailable);
        assert_eq!(auth.retry_after(now), 0, "a third retry is waiting");
        assert_eq!(
            auth.wiped_for_test(),
            (true, true),
            "the password is still kept"
        );
        assert!(auth.accepting());
        auth.settle(now + s(1000));
        assert_eq!(asked(&shared).timeouts.len(), 3, "retried past the cap");
        assert_eq!(auth.attempts(), 0, "an unavailable check is no attempt");

        submit(&mut auth, RIGHT, now);
        auth.settle(now);
        assert_eq!(
            auth.retry_after(now),
            16,
            "a new submit started the backoff over"
        );
    }

    /// **A lasting cause wipes the password at once**: no helper, one of
    /// another install, no PAM file. Nothing is kept or tried again, and the
    /// field takes typing, so a PAM file installed meanwhile is used by the
    /// next submit.
    #[test]
    fn a_lasting_cause_wipes_the_password_at_once() {
        let (mut auth, shared) = auth(Settings::default());
        asked(&shared).scripted = [Outcome::Unavailable {
            reason: "no PAM service file for solium".to_owned(),
            lasting: true,
        }]
        .into();
        submit(&mut auth, RIGHT, s(0));
        assert!(!auth.settle(s(0)));
        assert_eq!(auth.phase(), Phase::Unavailable);
        assert_eq!(auth.wiped_for_test(), (true, true), "the password was kept");
        assert_eq!(auth.retry_after(s(0)), 0);
        auth.settle(s(1000));
        assert_eq!(asked(&shared).timeouts.len(), 1, "tried again");
        assert!(auth.accepting());
        type_text(&mut auth, "a", s(1000));
        assert_eq!(auth.phase(), Phase::Prompting);
        auth.edit(Edit::Clear, s(1000));
        submit(&mut auth, RIGHT, s(1000));
        assert!(auth.settle(s(1000)), "the next submit was not checked");
    }

    /// **The secret is zeroed after submit and on unlock**: the field's page
    /// as it is submitted, the copy kept for the check as its answer comes,
    /// and the field again as the lock ends.
    #[test]
    fn the_secret_is_zeroed_after_submit_and_on_unlock() {
        let (mut auth, _) = auth(Settings::default());
        type_text(&mut auth, "hunter2", s(0));
        assert_eq!(auth.wiped_for_test(), (false, true));
        auth.edit(Edit::Submit, s(0));
        assert_eq!(
            auth.wiped_for_test(),
            (true, false),
            "the field after submit, and the copy being checked"
        );
        auth.settle(s(0));
        assert_eq!(auth.wiped_for_test(), (true, true), "after the answer");
        auth.settle(s(2));

        submit(&mut auth, RIGHT, s(3));
        assert!(auth.settle(s(3)));
        assert_eq!(auth.wiped_for_test(), (true, true), "after the yes");

        type_text(&mut auth, "abc", s(4));
        auth.end();
        assert_eq!(auth.wiped_for_test(), (true, true), "on unlock");
        assert_eq!(auth.length(), 0);
    }

    /// **A PIN is taken where one is set, and the password still is**: the
    /// lock reads `"pin"`; `min_length` to `max_length` digits are checked
    /// as the PIN, by the helper's check of the PIN file, and anything else
    /// as the password; the right PIN unlocks.
    #[test]
    fn a_pin_shaped_entry_is_checked_as_the_pin_and_anything_else_as_the_password() {
        let settings = Settings {
            lockout: Lockout {
                attempts: 100,
                ..Lockout::default()
            },
            ..Settings::default()
        };
        let (mut auth, shared, dir, _) = pin_auth("shape", settings);
        assert_eq!(auth.mode(), crate::pin::Mode::Pin);
        assert_eq!(auth.mode().name(), "pin");
        let mut now = s(0);
        for (typed, kind) in [
            ("12345678", "pin"),
            ("0000", "pin"),
            ("123", "password"),
            ("123456789", "password"),
            ("13a9", "password"),
            ("wrong", "password"),
        ] {
            submit(&mut auth, typed, now);
            assert!(!auth.settle(now), "{typed} unlocked");
            assert_eq!(last_kind(&shared), kind, "{typed}");
            now += s(3);
            auth.settle(now);
        }
        submit(&mut auth, SET_PIN, now);
        assert_eq!(last_kind(&shared), "pin");
        assert!(auth.settle(now), "the right PIN did not unlock");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A wrong PIN counts, and after `lock.pin.attempts` only the password
    /// unlocks**: each wrong PIN is counted in the state file as it comes;
    /// after the third, the lock reads `"password"` with the reason, and the
    /// right PIN is checked as the password, which it is not, so it does not
    /// unlock; the password then does, and the count starts again.
    #[test]
    fn a_wrong_pin_counts_and_after_attempts_only_the_password_unlocks() {
        let settings = Settings {
            lockout: Lockout {
                attempts: 100,
                ..Lockout::default()
            },
            pin: crate::pin::Settings {
                attempts: 3,
                ..crate::pin::Settings::default()
            },
            ..Settings::default()
        };
        let (mut auth, shared, dir, paths) = pin_auth("attempts", settings);
        for wrong in 1..=3 {
            assert_eq!(auth.mode(), crate::pin::Mode::Pin, "after {}", wrong - 1);
            submit(&mut auth, "0000", s(0));
            assert!(!auth.settle(s(0)));
            assert_eq!(last_kind(&shared), "pin");
            assert_eq!(record(&paths).failures, wrong, "not counted as it came");
        }
        assert_eq!(auth.phase(), Phase::Failed);
        assert_eq!(
            auth.mode(),
            crate::pin::Mode::Password(crate::pin::Fallback::Attempts)
        );
        assert_eq!(auth.mode().name(), "password");
        assert!(
            auth.message(true).contains("Too many wrong PINs"),
            "{:?}",
            auth.message(true)
        );

        submit(&mut auth, SET_PIN, s(1));
        assert_eq!(
            last_kind(&shared),
            "password",
            "a PIN taken after the attempts"
        );
        assert!(
            !auth.settle(s(1)),
            "the right PIN unlocked after the attempts"
        );
        auth.settle(s(4));
        assert_eq!(
            record(&paths).failures,
            3,
            "a password's no counted as a PIN's"
        );

        submit(&mut auth, RIGHT, s(5));
        assert!(auth.settle(s(5)), "the password did not unlock");
        assert_eq!(
            record(&paths),
            crate::pin::Record::after_password(WALL, Some("boot-one".to_owned()))
        );
        auth.begin();
        assert_eq!(auth.mode(), crate::pin::Mode::Pin);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The lockout applies to wrong PINs too**: `lock.lockout.attempts`
    /// wrong PINs in a row shut the field as wrong passwords do.
    #[test]
    fn the_lockout_applies_to_wrong_pins() {
        let settings = Settings {
            lockout: Lockout {
                attempts: 2,
                wait: Backoff {
                    seconds: 30,
                    max_seconds: 30,
                },
            },
            ..Settings::default()
        };
        let (mut auth, _, dir, _) = pin_auth("lockout", settings);
        submit(&mut auth, "0000", s(0));
        auth.settle(s(0));
        assert_eq!(auth.phase(), Phase::Failed);
        submit(&mut auth, "0001", s(0));
        auth.settle(s(0));
        assert_eq!(auth.phase(), Phase::LockedOut);
        assert_eq!(auth.retry_after(s(0)), 30);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A restart needs the password**: with another boot id than the one
    /// the last password unlock recorded, the lock reads `"password"` with
    /// the reason and checks the PIN as the password; the password unlocks
    /// and records this boot, and the next lock takes the PIN again.
    #[test]
    fn a_restart_needs_the_password() {
        let (mut auth, shared, dir, paths) = pin_auth("boot", Settings::default());
        assert_eq!(auth.mode(), crate::pin::Mode::Pin);
        std::fs::write(&paths.boot_id, "boot-two\n").expect("another boot");
        auth.begin();
        assert_eq!(
            auth.mode(),
            crate::pin::Mode::Password(crate::pin::Fallback::Boot)
        );
        assert!(
            auth.message(true).contains("restart"),
            "{:?}",
            auth.message(true)
        );
        submit(&mut auth, SET_PIN, s(0));
        assert_eq!(last_kind(&shared), "password");
        assert!(!auth.settle(s(0)), "the PIN unlocked after a restart");
        auth.settle(s(3));
        submit(&mut auth, RIGHT, s(3));
        assert!(auth.settle(s(3)));
        assert_eq!(record(&paths).boot.as_deref(), Some("boot-two"));
        auth.begin();
        assert_eq!(auth.mode(), crate::pin::Mode::Pin);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The password is needed after `password_after_hours`**: a PIN is
    /// taken up to 48 hours after the last password unlock and not a second
    /// later, nor when the clock says that unlock is still to come; a PIN
    /// typed while it was taken and submitted after is wiped unchecked.
    #[test]
    fn the_password_is_needed_after_password_after_hours() {
        let (mut auth, shared, dir, _) = pin_auth("hours", Settings::default());
        let hours = 48 * 3600;
        auth.set_wall_for_test(WALL + hours);
        assert_eq!(auth.mode(), crate::pin::Mode::Pin);
        type_text(&mut auth, SET_PIN, s(0));
        auth.set_wall_for_test(WALL + hours + 1);
        assert_eq!(
            auth.mode(),
            crate::pin::Mode::Password(crate::pin::Fallback::Hours)
        );
        assert!(
            auth.message(true).contains("every 48 hours"),
            "{:?}",
            auth.message(true)
        );
        auth.edit(Edit::Submit, s(0));
        assert!(
            asked(&shared).kinds.is_empty(),
            "checked as the mode changed"
        );
        assert_eq!((auth.phase(), auth.length()), (Phase::Idle, 0));
        submit(&mut auth, SET_PIN, s(0));
        assert_eq!(last_kind(&shared), "password");
        assert!(!auth.settle(s(0)));
        auth.set_wall_for_test(WALL - 1);
        assert_eq!(
            auth.mode(),
            crate::pin::Mode::Password(crate::pin::Fallback::Hours),
            "a password unlock in the future"
        );
        let short = Settings {
            pin: crate::pin::Settings {
                password_after: Duration::from_secs(3600),
                ..crate::pin::Settings::default()
            },
            ..Settings::default()
        };
        auth.configure(short);
        auth.set_wall_for_test(WALL + 3601);
        assert_eq!(
            auth.mode(),
            crate::pin::Mode::Password(crate::pin::Fallback::Hours),
            "the setting's hours"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The counts survive a restart**: two wrong PINs, then a new
    /// `LockAuth`, as a restarted Solium has, reads them from the state file,
    /// and one more wrong PIN is the third of three.
    #[test]
    fn the_pin_counts_survive_a_restart() {
        let settings = Settings {
            lockout: Lockout {
                attempts: 100,
                ..Lockout::default()
            },
            pin: crate::pin::Settings {
                attempts: 3,
                ..crate::pin::Settings::default()
            },
            ..Settings::default()
        };
        let (mut first, _, dir, paths) = pin_auth("restart", settings.clone());
        for _ in 0..2 {
            submit(&mut first, "0000", s(0));
            first.settle(s(0));
        }
        drop(first);

        let (mut again, _) = auth(settings);
        again.use_pin_for_test(paths.clone(), WALL);
        again.begin();
        assert_eq!(again.mode(), crate::pin::Mode::Pin);
        submit(&mut again, "0000", s(0));
        again.settle(s(0));
        assert_eq!(
            again.mode(),
            crate::pin::Mode::Password(crate::pin::Fallback::Attempts),
            "a restart gave a fresh run of guesses"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The password unlocks in PIN mode, and resets the counts**: after a
    /// wrong PIN, the password, checked as the password, unlocks and leaves
    /// a fresh record; the right PIN unlocks too and ends a run of wrong
    /// ones without touching the password's time.
    #[test]
    fn the_password_unlocks_in_pin_mode_and_resets_the_counts() {
        let (mut auth, shared, dir, paths) = pin_auth("password", Settings::default());
        submit(&mut auth, "0000", s(0));
        auth.settle(s(0));
        assert_eq!(record(&paths).failures, 1);
        auth.set_wall_for_test(WALL + 60);
        submit(&mut auth, RIGHT, s(1));
        assert_eq!(last_kind(&shared), "password");
        assert!(auth.settle(s(1)), "the password did not unlock in PIN mode");
        assert_eq!(
            record(&paths),
            crate::pin::Record::after_password(WALL + 60, Some("boot-one".to_owned()))
        );

        auth.begin();
        submit(&mut auth, "0000", s(2));
        auth.settle(s(2));
        auth.set_wall_for_test(WALL + 120);
        submit(&mut auth, SET_PIN, s(3));
        assert!(auth.settle(s(3)), "the right PIN did not unlock");
        assert_eq!(
            record(&paths),
            crate::pin::Record::after_password(WALL + 60, Some("boot-one".to_owned())),
            "the PIN moved the password's time, or kept the wrong one"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The secret is zeroed after a PIN check**, right or wrong: the field
    /// as it is submitted, the copy as the helper answers.
    #[test]
    fn the_secret_is_zeroed_after_a_pin_check() {
        let (mut auth, shared, dir, _) = pin_auth("zeroed", Settings::default());
        for pin in ["0000", SET_PIN] {
            type_text(&mut auth, pin, s(0));
            assert_eq!(auth.wiped_for_test(), (false, true));
            auth.edit(Edit::Submit, s(0));
            assert_eq!(last_kind(&shared), "pin");
            assert!(auth.wiped_for_test().0, "the field after submit");
            auth.settle(s(0));
            assert_eq!(
                auth.wiped_for_test(),
                (true, true),
                "after the answer to {pin}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A PIN file that does not parse is no PIN**: the lock reads
    /// `"password"` with nothing to say, and digits are checked as the
    /// password. Nor is a PIN taken with `lock.pin.enabled` off.
    #[test]
    fn an_unparsable_pin_file_or_enabled_off_is_no_pin() {
        let (mut auth, shared, dir, paths) = pin_auth("unparsable", Settings::default());
        assert_eq!(auth.mode(), crate::pin::Mode::Pin, "the premise");
        solium_auth::pin::write_private(&paths.pin, b"1379\n").expect("spoiled");
        auth.begin();
        assert_eq!(auth.mode(), crate::pin::Mode::NoPin);
        assert_eq!(auth.message(true), "");
        submit(&mut auth, SET_PIN, s(0));
        assert_eq!(last_kind(&shared), "password");
        assert!(!auth.settle(s(0)));

        let (mut off, shared, other, _) = pin_auth(
            "enabled-off",
            Settings {
                pin: crate::pin::Settings {
                    enabled: false,
                    ..crate::pin::Settings::default()
                },
                ..Settings::default()
            },
        );
        assert_eq!(off.mode(), crate::pin::Mode::NoPin);
        submit(&mut off, SET_PIN, s(0));
        assert_eq!(last_kind(&shared), "password");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&other);
    }

    /// A PIN cleared while the lock is up: the PIN typed is not checked as
    /// one, and the lock asks for the password.
    #[test]
    fn a_pin_cleared_under_the_lock_is_not_checked() {
        let (mut auth, shared, dir, paths) = pin_auth("cleared", Settings::default());
        type_text(&mut auth, SET_PIN, s(0));
        std::fs::remove_file(&paths.pin).expect("cleared");
        auth.edit(Edit::Submit, s(0));
        assert!(asked(&shared).kinds.is_empty());
        assert_eq!(auth.mode(), crate::pin::Mode::NoPin);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The xkb keycode of each character a test types, on `us`.
    fn code(c: char) -> u32 {
        let row = |keys: &str, first: u32, c: char| {
            keys.find(c)
                .and_then(|at| u32::try_from(at).ok())
                .map(|at| first + at)
        };
        row("1234567890", 10, c)
            .or_else(|| row("qwertyuiop", 24, c))
            .or_else(|| row("asdfghjkl", 38, c))
            .or_else(|| row("zxcvbnm", 52, c))
            .or_else(|| (c == ' ').then_some(65))
            .expect("a key this test knows")
    }

    const RETURN: u32 = 36;
    const ESCAPE: u32 = 9;
    const BACKSPACE: u32 = 22;

    /// Type `text` through the real input path.
    fn keys(state: &mut Solium, text: &str) {
        for c in text.chars() {
            tap(state, &[code(c)]);
        }
    }

    /// A `Solium` on `us,ru` with `group` live, answered by the stand-in.
    fn solium(group: u32) -> (Display<Solium>, Solium, Shared) {
        let (display, mut state) = us_ru(group);
        let shared = Shared::default();
        state.lock_auth.use_stand_in(stand_in(&shared));
        (display, state, shared)
    }

    fn natively_locked(state: &Solium) -> bool {
        state
            .lock
            .as_ref()
            .is_some_and(crate::lock::Lock::is_native)
    }

    /// **Submit with the stand-in's yes unlocks, and its no counts an
    /// attempt**, typed through the real keyboard path: a wrong password and
    /// Return leave the session locked with one attempt counted; Backspace
    /// and Escape edit the field; the right one and Return unlock, with the
    /// helper's timeout the settings'.
    #[test]
    fn a_yes_unlocks_and_a_no_counts_an_attempt() {
        let (_display, mut state, shared) = solium(0);
        state.lock_auth.configure(Settings {
            helper_timeout: Duration::from_millis(1234),
            ..Settings::default()
        });
        state.lock_natively();
        keys(&mut state, "wrong");
        tap(&mut state, &[RETURN]);
        settle(&mut state);
        assert!(natively_locked(&state), "a no unlocked");
        assert_eq!(state.lock_auth.attempts(), 1);
        state.clock.advance(s(3));
        settle(&mut state);
        assert_eq!(state.lock_auth.phase(), Phase::Failed);

        keys(&mut state, "xy");
        tap(&mut state, &[BACKSPACE]);
        assert_eq!(state.lock_auth.length(), 1);
        tap(&mut state, &[ESCAPE]);
        assert_eq!(state.lock_auth.length(), 0);

        keys(&mut state, RIGHT);
        assert_eq!(state.lock_auth.length(), RIGHT.len());
        assert!(natively_locked(&state), "typing alone unlocked");
        tap(&mut state, &[RETURN]);
        settle(&mut state);
        assert!(state.lock.is_none(), "the yes did not unlock");
        assert_eq!(
            asked(&shared).timeouts,
            [Duration::from_millis(1234); 2],
            "the checks were not given the configured timeout"
        );
    }

    /// **`us,ru` with Russian live types the password in Latin under
    /// `secret_layout = "first"`**, and Russian is live again after the
    /// unlock. With `"current"` the same keys type Cyrillic, which is not
    /// the password, and the layout is never touched.
    #[test]
    fn with_us_ru_on_russian_a_password_is_typed_in_latin_and_russian_comes_back() {
        let (_display, mut state, _) = solium(1);
        assert_eq!(crate::keymap::live(&mut state).0, 2, "the premise: Russian");
        state.lock_natively();
        assert_eq!(
            crate::keymap::live(&mut state).0,
            1,
            "the lock did not switch to the Latin layout"
        );
        keys(&mut state, RIGHT);
        tap(&mut state, &[RETURN]);
        settle(&mut state);
        assert!(
            state.lock.is_none(),
            "the password typed on Russian did not unlock"
        );
        assert_eq!(
            crate::keymap::live(&mut state).0,
            2,
            "Russian is not live again after the unlock"
        );

        // Switched to Russian by hand at the lock, and the unlock still
        // leaves the layout that was live before it.
        state.lock_natively();
        tap(&mut state, &[ALT, SHIFT]);
        assert_eq!(crate::keymap::live(&mut state).0, 2);
        state.unlock_natively();
        assert_eq!(crate::keymap::live(&mut state).0, 2);

        let (_display, mut state, _) = solium(1);
        state.lock_auth.configure(Settings {
            secret_layout: SecretLayout::Current,
            ..Settings::default()
        });
        state.lock_natively();
        assert_eq!(crate::keymap::live(&mut state).0, 2, "\"current\" switched");
        keys(&mut state, RIGHT);
        tap(&mut state, &[RETURN]);
        settle(&mut state);
        assert!(
            natively_locked(&state),
            "Cyrillic was taken for the password"
        );
        assert_eq!(state.lock_auth.attempts(), 1);
    }

    /// Which scene a [`Locked`] shows.
    enum Shows<'a> {
        /// A scene of this source, in a file of the test's own.
        Own(&'a str),
        /// The shipped `Lock.qml`.
        Shipped,
        /// The compiled-in fallback.
        Fallback,
    }

    /// A native lock with `source` as its scene, or the fallback, on one
    /// 1920x1080 monitor, built on the Qt thread and published to. The
    /// scene's file is in a directory of the test's own. Its entry is over
    /// unless it is made [`Locked::entering`].
    struct Locked {
        _display: Display<Solium>,
        state: Solium,
        output: Output,
        shared: Shared,
        directory: PathBuf,
    }

    impl Locked {
        fn new(name: &str, source: Option<&str>) -> Self {
            let shows = source.map_or(Shows::Fallback, Shows::Own);
            let mut locked = Self::entering(name, &shows);
            let enter = locked.state.lock_scenes.settings().enter;
            locked.state.clock.advance(enter);
            crate::lock_scene::settle(&mut locked.state);
            locked.state.publish_models();
            locked
        }

        /// The lock just taken, its scene coming down.
        fn entering(name: &str, shows: &Shows<'_>) -> Self {
            let mut locked = Self::unlocked(name, shows);
            locked.state.lock_natively();
            locked.state.publish_models();
            locked
        }

        /// No lock yet: the scene built ahead, and kept quiet.
        fn unlocked(name: &str, shows: &Shows<'_>) -> Self {
            crate::qml::start().expect("Qt starts");
            // Whatever a test before this one left asked.
            let _ = take_requests();
            let (display, mut state, shared) = solium(0);
            let output = Output::new(
                format!("lock-auth-{name}"),
                PhysicalProperties {
                    size: (0, 0).into(),
                    subpixel: Subpixel::Unknown,
                    make: "solium".to_owned(),
                    model: "lock-auth".to_owned(),
                },
            );
            output.change_current_state(
                Some(Mode {
                    size: (1920, 1080).into(),
                    refresh: 60_000,
                }),
                None,
                Some(Scale::Fractional(1.0)),
                None,
            );
            state.space.map_output(&output, (0, 0));
            let directory = std::env::temp_dir()
                .join(format!("solium-lock-auth-{name}-{}", std::process::id()));
            match shows {
                Shows::Own(source) => {
                    std::fs::create_dir_all(&directory).expect("a directory for the scene");
                    let path = directory.join("Lock.qml");
                    std::fs::write(&path, source).expect("writing the scene");
                    state.lock_scenes.use_scene(path);
                }
                Shows::Shipped => {}
                Shows::Fallback => state.lock_scenes.use_fallback(),
            }
            state.sync_lock_scenes();
            state
                .lock_scenes
                .instance_for_test(&output)
                .expect("the lock's scene")
                .scene_for_test()
                .resize(1920, 1080, 1.0);
            state.publish_models();
            Self {
                _display: display,
                state,
                output,
                shared,
                directory,
            }
        }

        fn scene(&mut self) -> &mut crate::qml::Scene {
            self.state
                .lock_scenes
                .instance_for_test(&self.output)
                .expect("the lock's scene")
                .scene_for_test()
        }

        fn int(&mut self, name: &str) -> i32 {
            self.state.publish_models();
            self.scene().get_int(name)
        }

        fn region(&self) -> Rectangle<i32, Logical> {
            Rectangle::from_size((1920, 1080).into())
        }

        /// The pointer to `at`, through the real input path.
        fn point(&mut self, at: (f64, f64)) {
            let was = self
                .state
                .seat
                .get_pointer()
                .map(|pointer| pointer.current_location())
                .unwrap_or_default();
            let region = self.region();
            crate::synth::send_motion(&mut self.state, region, Point::from(at) - was, 1);
        }

        /// A left click at `at`.
        fn click(&mut self, at: (f64, f64)) {
            self.point(at);
            let region = self.region();
            for button_state in [ButtonState::Pressed, ButtonState::Released] {
                crate::synth::send_button(&mut self.state, region, 0x110, button_state, 2);
            }
        }
    }

    impl Drop for Locked {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    /// **Typing reaches the buffer and never the scene**: with a focused
    /// item handling keys and a focused `TextInput` in the lock's scene, a
    /// password typed at the lock is the field's length, and the shipped
    /// `SecretInput`'s, and nothing else there: no key event, no text.
    #[test]
    fn typed_text_never_reaches_the_lock_scene() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            let mut locked = Locked::new(
                "typed",
                Some(
                    r#"import QtQuick
                import Solium
                FocusScope {
                    id: root
                    focus: true
                    property int keys: 0
                    property string seen: ""
                    readonly property int seenLength: seen.length + input.text.length
                    readonly property int length: field.length
                    readonly property int prompting: Lock.state === "prompting" ? 1 : 0
                    Keys.onPressed: (event) => { root.keys += 1; root.seen += event.text }
                    Keys.onReleased: (event) => { root.keys += 1 }
                    TextInput { id: input; focus: true; width: 200; height: 30 }
                    SecretField { id: field }
                    SecretInput { id: drawn }
                    readonly property int dots: drawn.length
                }"#,
                ),
            );
            keys(&mut locked.state, "hunter2");
            assert_eq!(
                locked.int("length"),
                7,
                "the field does not show what was typed"
            );
            assert_eq!(
                locked.int("dots"),
                7,
                "the shipped SecretInput does not either"
            );
            assert_eq!(locked.int("prompting"), 1);
            assert_eq!(locked.int("keys"), 0, "the scene heard a key");
            assert_eq!(locked.int("seenLength"), 0, "the scene saw typed text");
        });
    }

    /// **Nothing in QML can unlock, or read the secret.** `Lock` declares
    /// two methods, `submit()` and `clear()`, and read-only properties;
    /// `SecretField` and `SecretKeypad` declare no method, and nothing but a
    /// keypad's `delegate` can be written. A scene that writes every one of
    /// them and calls every function there is leaves the session locked,
    /// finds nothing typed in any of them, and its `submit()` checks what
    /// was typed like Return does: a wrong password counts an attempt. With
    /// a PIN set, the same: `Lock.mode` reads `"pin"` and cannot be written,
    /// the digits typed are nowhere to be read, and the PIN is not.
    #[test]
    fn nothing_in_qml_can_unlock_or_read_the_secret() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            let api = |which: &std::ffi::CStr| -> Vec<String> {
                let mut lines: Vec<String> =
                    api_for_test(which).lines().map(str::to_owned).collect();
                lines.sort();
                lines
            };
            let methods = |lines: &[String]| -> Vec<String> {
                lines
                    .iter()
                    .filter(|line| line.starts_with("method "))
                    .cloned()
                    .collect()
            };
            let writable = |lines: &[String]| -> Vec<String> {
                lines
                    .iter()
                    .filter(|line| line.ends_with(" writable"))
                    .cloned()
                    .collect()
            };
            let lock = api(c"lock");
            assert_eq!(
                methods(&lock),
                ["method clear", "method submit"],
                "{lock:?}"
            );
            assert!(writable(&lock).is_empty(), "{lock:?}");
            let field = api(c"field");
            assert!(methods(&field).is_empty(), "{field:?}");
            assert!(writable(&field).is_empty(), "{field:?}");
            let keypad = api(c"keypad");
            assert!(methods(&keypad).is_empty(), "{keypad:?}");
            assert_eq!(
                writable(&keypad),
                ["property delegate writable"],
                "{keypad:?}"
            );

            let mut locked = Locked::new(
                "nothing",
                Some(
                    r#"import QtQuick
                import Solium
                Item {
                    id: root
                    property int go: 0
                    property int took: 0
                    property int leaked: 0
                    property int calls: 0
                    SecretField { id: field }
                    SecretKeypad { id: pad; width: 300; height: 400; delegate: Item {} }
                    property string mode: ""
                    function look(value) {
                        let text = ""
                        try {
                            text = typeof value === "function" ? ""
                                : JSON.stringify(value === undefined ? null : value)
                        } catch (error) {
                            text = String(value)
                        }
                        if (typeof text === "string"
                                && (text.indexOf("wrong") >= 0 || text.indexOf("1357") >= 0
                                    || text.indexOf("1379") >= 0))
                            root.leaked += 1
                    }
                    function write(target, names) {
                        for (const name of names) {
                            const was = target[name]
                            root.look(was)
                            const next = typeof was === "number" ? 999
                                : typeof was === "boolean" ? !was : "unlocking"
                            try { target[name] = next } catch (error) {}
                            if (JSON.stringify(target[name]) !== JSON.stringify(was)) root.took += 1
                        }
                    }
                    function callAll(target) {
                        for (const name in target) {
                            const value = target[name]
                            root.look(value)
                            if (typeof value !== "function" || name === "deleteLater"
                                || name === "destroy" || name === "destroyed") continue
                            try { value.call(target, "unlock", true) } catch (error) {}
                            root.calls += 1
                        }
                    }
                    property string failure: ""
                    onGoChanged: {
                        if (go === 1) Lock.submit()
                        if (go === 3) root.mode = Lock.mode
                        if (go === 2 || go === 3) {
                            try {
                                // `Lock` is an attached object, reached
                                // through its name: every property and
                                // method its type declares, by name.
                                root.write(Lock, ["state", "mode", "attempts", "retryAfter",
                                    "message", "user", "promptOn", "locked", "progress",
                                    "enterMs", "exitMs"])
                                root.write(field, ["length", "selected", "accepting"])
                                root.write(pad, ["layout", "pressed"])
                                try { Lock.submit("unlock", true) } catch (error) {}
                                try { Lock.clear("unlock", true) } catch (error) {}
                                root.calls += 2
                                root.callAll(field)
                                root.callAll(pad)
                            } catch (error) {
                                root.failure = String(error)
                            }
                        }
                    }
                }"#,
                ),
            );
            keys(&mut locked.state, "wrong");
            locked.state.publish_models();
            locked.scene().set_int("go", 1);
            settle(&mut locked.state);
            settle(&mut locked.state);
            assert!(
                natively_locked(&locked.state),
                "the scene's submit unlocked"
            );
            assert_eq!(
                locked.state.lock_auth.attempts(),
                1,
                "the scene's submit checked nothing"
            );
            locked.state.clock.advance(s(3));
            settle(&mut locked.state);

            keys(&mut locked.state, "wrong");
            locked.state.publish_models();
            locked.scene().set_int("go", 2);
            settle(&mut locked.state);
            settle(&mut locked.state);
            let calls = locked.int("calls");
            assert!(
                calls > 10,
                "the walk did not get through: {calls} calls, {:?}",
                locked.scene().take_string("failure")
            );
            assert_eq!(locked.int("took"), 0, "a property took a write");
            assert_eq!(locked.int("leaked"), 0, "the typed text was readable");
            assert!(natively_locked(&locked.state), "a call from QML unlocked");
            assert_eq!(
                asked(&locked.shared).timeouts.len(),
                1,
                "the walk's clear() and submit() checked something"
            );

            // A PIN set, and digits typed: the walk again.
            let (dir, paths) = pin_set("qml-walk");
            locked.state.lock_auth.use_pin_for_test(paths, WALL);
            locked.state.lock_auth_changed();
            assert_eq!(locked.state.lock_auth.mode(), crate::pin::Mode::Pin);
            locked.scene().set_int("calls", 0);
            keys(&mut locked.state, "1357");
            locked.state.publish_models();
            locked.scene().set_int("go", 3);
            settle(&mut locked.state);
            settle(&mut locked.state);
            assert_eq!(
                locked.scene().take_string("mode").as_deref(),
                Some("pin"),
                "Lock.mode does not read \"pin\""
            );
            assert!(locked.int("calls") > 10, "the walk did not get through");
            assert_eq!(locked.int("took"), 0, "a property took a write");
            assert_eq!(locked.int("leaked"), 0, "the PIN typed was readable");
            assert!(natively_locked(&locked.state), "a call from QML unlocked");
            assert_eq!(
                asked(&locked.shared).timeouts.len(),
                1,
                "the walk checked the PIN"
            );
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// **The shipped scene asks for the PIN with its pad up, while a PIN is
    /// taken**: it says "Enter PIN" and shows the digit pad as the lock
    /// arrives; once the lock stops taking the PIN, the pad goes down and it
    /// asks for the password.
    #[test]
    fn the_shipped_scene_asks_for_a_pin_with_the_pad_up() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            let mut locked = Locked::unlocked("pin-shipped", &Shows::Shipped);
            let (dir, paths) = pin_set("shipped");
            locked.state.lock_auth.use_pin_for_test(paths.clone(), WALL);
            locked.state.lock_natively();
            locked.state.publish_models();
            let enter = locked.state.lock_scenes.settings().enter;
            locked.state.clock.advance(enter);
            crate::lock_scene::settle(&mut locked.state);
            locked.state.publish_models();
            assert!(
                locked.scene().get_bool("prompt"),
                "the premise: the prompt is here"
            );
            assert!(locked.scene().get_bool("asksForPin"), "no \"Enter PIN\"");
            assert!(locked.scene().get_bool("padShown"), "the pad is not up");

            locked.state.lock_auth.set_wall_for_test(WALL + 49 * 3600);
            locked.state.lock_auth_changed();
            locked.state.publish_models();
            run_qml_clock(400);
            locked.state.publish_models();
            assert!(!locked.scene().get_bool("asksForPin"));
            assert!(!locked.scene().get_bool("padShown"), "the pad stayed up");

            locked.state.unlock_natively();
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// **A keypad key types into the secret, and its scene never hears
    /// which**: the delegates are laid out in the settings' rows; a click
    /// or a finger on a key types it, with `keyPressed()` and nothing else
    /// told, while the scene's `MouseArea` and `TapHandler` under the pad
    /// hear neither the presses nor the pointer passing over it. The
    /// passcode and OK unlock.
    #[test]
    fn a_keypad_key_types_into_the_secret_and_its_scene_never_hears_which() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            let mut locked = Locked::new(
                "keypad",
                Some(
                    r#"import QtQuick
                import Solium
                Item {
                    id: root
                    property int presses: 0
                    property int moves: 0
                    property int taps: 0
                    property int keyPresses: 0
                    readonly property int keys: pad.children.length
                    readonly property int fiveX: pad.children.length > 4 ? pad.children[4].x : -1
                    readonly property int fiveY: pad.children.length > 4 ? pad.children[4].y : -1
                    readonly property int fiveWidth: pad.children.length > 4 ? pad.children[4].width : -1
                    readonly property int fiveIsFive: pad.children.length > 4
                        && pad.children[4].label === "5" && pad.children[4].letters === "JKL" ? 1 : 0
                    readonly property int length: field.length
                    MouseArea {
                        anchors.fill: parent
                        hoverEnabled: true
                        onPressed: root.presses += 1
                        onPositionChanged: root.moves += 1
                    }
                    TapHandler { onTapped: root.taps += 1 }
                    SecretField { id: field }
                    SecretKeypad {
                        id: pad
                        width: 300
                        height: 400
                        onKeyPressed: root.keyPresses += 1
                        delegate: Rectangle {
                            required property string label
                            property string letters
                            property string kind
                            property int row
                            property int column
                        }
                    }
                }"#,
                ),
            );
            locked.state.lock_auth.configure(Settings {
                keypad: Keypad::Letters,
                ..Settings::default()
            });
            locked.state.lock_auth_changed();
            assert_eq!(locked.int("keys"), 12, "the delegates for 4 rows of 3");
            assert_eq!(
                [
                    locked.int("fiveX"),
                    locked.int("fiveY"),
                    locked.int("fiveWidth")
                ],
                [100, 100, 100]
            );
            assert_eq!(locked.int("fiveIsFive"), 1, "the fifth key's face");

            locked.point((590.0, 590.0));
            locked.click((600.0, 600.0));
            let moves = locked.int("moves");
            let taps = locked.int("taps");
            assert!(
                moves > 0,
                "the premise: the scene hears the pointer off the pad"
            );
            assert_eq!(locked.int("presses"), 1, "the premise: and its presses");

            // 2, 5, 8, 0, in the middle of each key.
            for at in [
                (150.0, 50.0),
                (150.0, 150.0),
                (150.0, 250.0),
                (150.0, 350.0),
            ] {
                locked.click(at);
            }
            // A finger on 1, and on Delete.
            let region = locked.region();
            crate::synth::send_touch(&mut locked.state, region, (50.0, 50.0), 10);
            assert_eq!(locked.int("length"), 5, "a finger on a key typed nothing");
            crate::synth::send_touch(&mut locked.state, region, (50.0, 350.0), 20);
            assert_eq!(locked.int("length"), 4);
            assert_eq!(locked.int("keyPresses"), 6);
            assert_eq!(
                locked.int("presses"),
                1,
                "a press on the pad reached the scene"
            );
            assert_eq!(
                locked.int("taps"),
                taps,
                "a finger on the pad reached the scene"
            );
            assert_eq!(
                locked.int("moves"),
                moves,
                "the scene saw the pointer cross the pad"
            );
            assert!(natively_locked(&locked.state));

            locked.click((250.0, 350.0));
            settle(&mut locked.state);
            assert!(
                locked.state.lock.is_none(),
                "the passcode and OK did not unlock"
            );
        });
    }

    /// **The fallback shows what is typed as dots**, so a native lock whose
    /// own scene will not load can still be unlocked: its field counts the
    /// characters, from `SecretField`, compiled in like the fallback.
    #[test]
    fn the_fallback_shows_what_is_typed_as_dots() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            let mut locked = Locked::new("fallback", None);
            assert_eq!(
                locked.state.lock_scenes.shown_on(&locked.output),
                crate::lock_scene::Shown::Fallback
            );
            keys(&mut locked.state, "abc");
            assert_eq!(locked.int("secretLength"), 3);
            tap(&mut locked.state, &[ESCAPE]);
            assert_eq!(locked.int("secretLength"), 0);
        });
    }

    /// The lock's scene settings, with `change` made to them.
    fn scene_settings(
        state: &Solium,
        change: impl FnOnce(&mut crate::lock_scene::Settings),
    ) -> crate::lock_scene::Settings {
        let mut settings = state.lock_scenes.settings().clone();
        change(&mut settings);
        settings
    }

    /// **The exit plays over the desktop for `lock.exit_ms`, and input is the
    /// desktop's at once**: from the yes on the session is unlocked, a click
    /// reaches no lock scene, and the scene reads `Lock.state` as
    /// `"unlocking"` and `Lock.locked` false; it is drawn over the desktop
    /// until `lock.exit_ms` is up, though it animates nothing, and is gone at
    /// the bound, reading `"idle"` again. A shorter `exit_ms` is a shorter
    /// exit, and 0 none at all.
    #[test]
    fn the_exit_plays_over_the_desktop_for_exit_ms_and_input_is_the_desktops_at_once() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            let mut locked = Locked::new(
                "exit",
                Some(
                    r#"import QtQuick
                import Solium
                Item {
                    id: root
                    property int presses: 0
                    readonly property int unlocking: Lock.state === "unlocking" ? 1 : 0
                    readonly property int locked: Lock.locked ? 1 : 0
                    readonly property int exitMs: Lock.exitMs
                    MouseArea { anchors.fill: parent; onPressed: root.presses += 1 }
                }"#,
                ),
            );
            let settings = scene_settings(&locked.state, |settings| {
                settings.exit = Duration::from_millis(400);
            });
            locked.state.lock_scenes.configure(settings);
            assert_eq!(locked.int("locked"), 1);
            assert_eq!(
                locked.int("exitMs"),
                400,
                "`Lock.exitMs` is not the setting"
            );
            locked.click((100.0, 100.0));
            assert_eq!(
                locked.int("presses"),
                1,
                "the premise: the scene hears a click"
            );

            keys(&mut locked.state, RIGHT);
            tap(&mut locked.state, &[RETURN]);
            settle(&mut locked.state);
            crate::lock_scene::settle(&mut locked.state);
            assert!(locked.state.lock.is_none(), "the yes did not unlock");
            let now = locked.state.clock.now();
            assert!(
                locked.state.lock_scenes.exiting(now),
                "the scenes are not drawn over the desktop"
            );
            assert_eq!(
                locked.int("unlocking"),
                1,
                "the exit does not read \"unlocking\""
            );
            assert_eq!(locked.int("locked"), 0, "`Lock.locked` through the exit");
            locked.click((100.0, 100.0));
            assert_eq!(
                locked.int("presses"),
                1,
                "a click during the exit reached the lock's scene, not the desktop"
            );

            locked.state.clock.advance(Duration::from_millis(399));
            crate::lock_scene::settle(&mut locked.state);
            assert!(
                locked.state.lock_scenes.exiting(locked.state.clock.now()),
                "the exit ended before `exit_ms`"
            );
            locked.state.clock.advance(Duration::from_millis(1));
            crate::lock_scene::settle(&mut locked.state);
            assert!(
                !locked.state.lock_scenes.exiting(locked.state.clock.now()),
                "the exit outlived `exit_ms`"
            );
            assert_eq!(
                locked.int("unlocking"),
                0,
                "\"unlocking\" outlived the exit"
            );

            // Shorter, and none.
            for (ms, lasts) in [(100, true), (0, false)] {
                let settings = scene_settings(&locked.state, |settings| {
                    settings.exit = Duration::from_millis(ms);
                });
                locked.state.lock_scenes.configure(settings);
                locked.state.lock_natively();
                locked.state.unlock_natively();
                let now = locked.state.clock.now();
                assert_eq!(
                    locked.state.lock_scenes.exiting(now),
                    lasts,
                    "exit_ms = {ms}"
                );
                locked.state.clock.advance(Duration::from_millis(ms));
                crate::lock_scene::settle(&mut locked.state);
                assert!(
                    !locked.state.lock_scenes.exiting(locked.state.clock.now()),
                    "exit_ms = {ms} outlived its bound"
                );
            }
        });
    }

    /// **The entry is bounded even if the scene never finishes**: from the
    /// instant the lock is taken the scene reads `Lock.state` as
    /// `"locking"`, `Lock.locked` true and `Lock.enterMs` the setting, and
    /// the desktop is drawn under it; a scene that animates nothing is fully
    /// up at `lock.enter_ms` all the same, with the desktop gone and the
    /// state the field's. A shorter `enter_ms` is a shorter entry, and 0
    /// none at all.
    #[test]
    fn the_entry_is_bounded_even_if_the_scene_never_finishes() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            let mut locked = Locked::entering(
                "entry-bound",
                &Shows::Own(
                    r#"import QtQuick
                import Solium
                Item {
                    readonly property int locking: Lock.state === "locking" ? 1 : 0
                    readonly property int locked: Lock.locked ? 1 : 0
                    readonly property int enterMs: Lock.enterMs
                }"#,
                ),
            );
            let output = locked.output.clone();
            let enter = locked.state.lock_scenes.settings().enter;
            assert_eq!(
                enter,
                Duration::from_millis(u64::from(crate::lock_scene::ENTER_MS)),
                "the premise: the default entry"
            );
            assert_eq!(
                locked.int("locking"),
                1,
                "the entry does not read \"locking\""
            );
            assert_eq!(
                locked.int("locked"),
                1,
                "`Lock.locked` is not true from the first instant"
            );
            assert_eq!(
                i64::from(locked.int("enterMs")),
                i64::from(crate::lock_scene::ENTER_MS),
                "`Lock.enterMs` is not the setting"
            );
            let desktop = |locked: &Locked| {
                let now = locked.state.clock.now();
                locked.state.desktop_under_lock(&locked.output, now, false)
            };
            assert!(
                desktop(&locked),
                "the desktop is not under the scene in its entry"
            );

            // Half way, and then past the bound: the compositor's clock runs
            // in real time too, so not to the millisecond.
            locked.state.clock.advance(enter / 2);
            crate::lock_scene::settle(&mut locked.state);
            assert!(
                locked.state.lock_scenes.entry_pending() && desktop(&locked),
                "the entry ended before `enter_ms`"
            );
            locked.state.clock.advance(enter / 2);
            crate::lock_scene::settle(&mut locked.state);
            assert!(
                !locked.state.lock_scenes.entry_pending(),
                "the entry outlived `enter_ms`"
            );
            assert!(!desktop(&locked), "the desktop outlived the entry");
            assert_eq!(locked.int("locking"), 0, "\"locking\" outlived the entry");
            assert!(natively_locked(&locked.state));
            assert_eq!(
                locked.state.lock_scenes.shown_on(&output),
                crate::lock_scene::Shown::Own,
                "the scene was taken away at the bound"
            );

            // Shorter, and none.
            for (ms, plays) in [(120, true), (0, false)] {
                let settings = scene_settings(&locked.state, |settings| {
                    settings.enter = Duration::from_millis(ms);
                });
                locked.state.lock_scenes.configure(settings);
                locked.state.unlock_natively();
                locked.state.lock_natively();
                assert_eq!(
                    locked.state.lock_scenes.entry_pending(),
                    plays,
                    "enter_ms = {ms}"
                );
                assert_eq!(desktop(&locked), plays, "enter_ms = {ms}: the desktop");
                locked.state.clock.advance(Duration::from_millis(ms));
                crate::lock_scene::settle(&mut locked.state);
                assert!(
                    !locked.state.lock_scenes.entry_pending(),
                    "enter_ms = {ms} outlived its bound"
                );
            }
        });
    }

    /// Run QML's clock `ms` on, a frame at a time, on real time since the
    /// first use, as `cursor::scene`'s tests run it.
    fn run_qml_clock(ms: u64) {
        static ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        let origin = *ORIGIN.get_or_init(std::time::Instant::now);
        let until = origin.elapsed() + Duration::from_millis(ms);
        loop {
            crate::qml::tick(origin.elapsed());
            if origin.elapsed() >= until {
                break;
            }
            std::thread::sleep(Duration::from_millis(16));
        }
    }

    /// **The shipped scene comes down as it locks and goes back up as it
    /// unlocks**, as one sheet: through `"locking"` it starts above the top
    /// and moves down, with its shadow showing; once the entry is over it
    /// is down; at `"unlocking"` it goes back up and off the top. A lock
    /// with no entry, before sleep, has it down at once.
    #[test]
    fn the_shipped_scene_comes_down_as_it_locks_and_goes_back_up_as_it_unlocks() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            let mut locked = Locked::entering("sheet", &Shows::Shipped);
            assert_eq!(
                locked.state.lock_scenes.shown_on(&locked.output),
                crate::lock_scene::Shown::Shipped,
                "the premise: the shipped scene"
            );
            let down = |locked: &mut Locked| {
                locked.state.publish_models();
                locked.scene().get_real("down", -1.0)
            };
            assert!(
                down(&mut locked).abs() < 1e-9,
                "the sheet does not start above the top"
            );
            run_qml_clock(150);
            let partway = down(&mut locked);
            assert!(
                partway > 0.0 && partway < 1.0,
                "the sheet is not coming down: {partway}"
            );
            assert!(
                locked.scene().get_bool("moving"),
                "no shadow while the sheet moves"
            );

            let enter = locked.state.lock_scenes.settings().enter;
            locked.state.clock.advance(enter);
            crate::lock_scene::settle(&mut locked.state);
            assert!(
                (down(&mut locked) - 1.0).abs() < 1e-9,
                "the sheet is not down after the entry"
            );
            assert!(
                !locked.scene().get_bool("moving"),
                "a shadow on a sheet at rest"
            );

            locked.state.unlock_natively();
            locked.state.publish_models();
            run_qml_clock(100);
            let going = down(&mut locked);
            assert!(
                going > 0.0 && going < 1.0,
                "the sheet is not going back up: {going}"
            );
            let exit = locked.state.lock_scenes.settings().exit;
            run_qml_clock(u64::try_from(exit.as_millis()).unwrap_or(600) + 100);
            assert!(down(&mut locked) < 0.01, "the sheet did not go off the top");

            locked.state.clock.advance(exit);
            crate::lock_scene::settle(&mut locked.state);
            locked.state.lock_natively_at_once();
            assert!(
                (down(&mut locked) - 1.0).abs() < 1e-9,
                "a lock with no entry is not down at once"
            );
        });
    }

    /// A scene that says what `Lock` reads: its state, as one flag each,
    /// whether it is locked, and `progress` in thousandths.
    const PEEK_SCENE: &str = r#"import QtQuick
                import Solium
                Item {
                    readonly property int peeking: Lock.state === "peeking" ? 1 : 0
                    readonly property int locking: Lock.state === "locking" ? 1 : 0
                    readonly property int unlocking: Lock.state === "unlocking" ? 1 : 0
                    readonly property int locked: Lock.locked ? 1 : 0
                    readonly property int progress: Math.round(Lock.progress * 1000)
                }"#;

    /// Whether the lock's scene on `output` is drawn over the unlocked
    /// desktop now.
    fn over_desktop(state: &Solium, output: &Output) -> bool {
        state.lock_scene_over_desktop(output, state.clock.now())
    }

    /// **`Lock.state` and `Lock.progress` follow `sol.lock_peek`**: a peek
    /// reads `"peeking"`, with `progress` where the configuration put it,
    /// held to 0 to 1, and a number that is not one changes nothing; the
    /// scene is live and drawn over the desktop, and nothing is locked. A
    /// lock taken from it reads `"locking"` with `progress` where the peek
    /// was, and 0 once the entry is over.
    #[test]
    fn lock_state_and_progress_follow_sol_lock_peek() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            let mut peeked = Locked::unlocked("peek-follow", &Shows::Own(PEEK_SCENE));
            let output = peeked.output.clone();
            assert_eq!(peeked.int("peeking"), 0, "the premise: no peek");
            assert!(
                !peeked.state.lock_scenes.is_live() && !over_desktop(&peeked.state, &output),
                "the premise: the scene is kept quiet"
            );

            for (asked, reads) in [(0.25, 250), (0.6, 600), (1.7, 1000), (-0.3, 0), (0.4, 400)] {
                peeked.state.lock_peek(Some(asked));
                assert_eq!(peeked.int("peeking"), 1, "{asked}: not \"peeking\"");
                assert_eq!(peeked.int("progress"), reads, "{asked}: `Lock.progress`");
                assert_eq!(peeked.int("locked"), 0, "{asked}: `Lock.locked` in a peek");
                assert!(peeked.state.lock.is_none(), "{asked}: a peek locked");
                assert!(
                    peeked.state.lock_scenes.is_live() && over_desktop(&peeked.state, &output),
                    "{asked}: the scene is not drawn over the desktop"
                );
            }
            peeked.state.lock_peek(Some(f64::NAN));
            assert_eq!(peeked.int("progress"), 400, "a NaN moved the peek");

            // Locked from a peek: the entry goes on from where it was.
            peeked.state.lock_peek(Some(0.5));
            peeked.state.lock_natively();
            assert_eq!(
                peeked.int("locking"),
                1,
                "the lock from a peek is not \"locking\""
            );
            assert_eq!(peeked.int("locked"), 1);
            assert_eq!(
                peeked.int("progress"),
                500,
                "`Lock.progress` through the entry is not where the peek was"
            );
            let enter = peeked.state.lock_scenes.settings().enter;
            peeked.state.clock.advance(enter);
            crate::lock_scene::settle(&mut peeked.state);
            assert_eq!(peeked.int("locking"), 0, "the entry did not end");
            assert_eq!(
                peeked.int("progress"),
                0,
                "`Lock.progress` outlived the entry"
            );
            assert!(natively_locked(&peeked.state));
        });
    }

    /// **Ending a peek takes the scene back up within its bound, and leaves
    /// nothing drawn**: `sol.lock_peek(false)` reads `"unlocking"` and keeps
    /// the scene over the desktop for `lock.exit_ms`, and not a moment more;
    /// then it is quiet again, reading `"idle"`, drawn on no monitor. A
    /// second end does nothing, and with `exit_ms` 0 it is gone at once.
    #[test]
    fn ending_a_peek_animates_back_within_its_bound_and_leaves_nothing_drawn() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            let mut peeked = Locked::unlocked("peek-end", &Shows::Own(PEEK_SCENE));
            let output = peeked.output.clone();
            let settings = scene_settings(&peeked.state, |settings| {
                settings.exit = Duration::from_millis(400);
            });
            peeked.state.lock_scenes.configure(settings);

            peeked.state.lock_peek(Some(0.5));
            peeked.state.lock_peek(None);
            assert_eq!(peeked.int("peeking"), 0, "\"peeking\" outlived the peek");
            assert_eq!(
                peeked.int("unlocking"),
                1,
                "the way back up is not the exit"
            );
            assert!(
                over_desktop(&peeked.state, &output),
                "the scene was taken away at once, not taken back up"
            );
            peeked.state.clock.advance(Duration::from_millis(399));
            crate::lock_scene::settle(&mut peeked.state);
            assert!(
                over_desktop(&peeked.state, &output),
                "the way back up ended before `exit_ms`"
            );
            peeked.state.clock.advance(Duration::from_millis(1));
            crate::lock_scene::settle(&mut peeked.state);
            assert!(
                !over_desktop(&peeked.state, &output),
                "the scene outlived the way back up"
            );
            assert!(
                !peeked.state.lock_scenes.is_live(),
                "the scene is not quiet again"
            );
            assert_eq!(
                (
                    peeked.int("unlocking"),
                    peeked.int("peeking"),
                    peeked.int("locked")
                ),
                (0, 0, 0),
                "(unlocking, peeking, locked) after the way back up"
            );
            assert!(peeked.state.lock.is_none(), "ending a peek locked");

            peeked.state.lock_peek(None);
            assert!(
                !peeked.state.lock_scenes.exiting(peeked.state.clock.now()),
                "ending no peek played an exit"
            );

            let settings = scene_settings(&peeked.state, |settings| {
                settings.exit = Duration::ZERO;
            });
            peeked.state.lock_scenes.configure(settings);
            peeked.state.lock_peek(Some(0.5));
            peeked.state.lock_peek(None);
            assert!(
                !over_desktop(&peeked.state, &output) && !peeked.state.lock_scenes.is_live(),
                "with no exit, the scene outlived the peek"
            );
        });
    }

    /// **A peek while locked does nothing**: at the lock, and through its
    /// entry, `sol.lock_peek` with a number or with `false` changes nothing
    /// the scene reads, plays no exit, and leaves the lock that was
    /// confirmed, confirmed.
    #[test]
    fn a_peek_while_locked_does_nothing() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            let mut locked = Locked::entering("peek-locked", &Shows::Own(PEEK_SCENE));
            for _ in 0..2 {
                let before = (
                    locked.int("locking"),
                    locked.int("progress"),
                    locked.state.lock_scenes.entry_pending(),
                );
                locked.state.lock_peek(Some(0.5));
                locked.state.lock_peek(None);
                assert_eq!(
                    locked.int("peeking"),
                    0,
                    "a peek at the lock reads \"peeking\""
                );
                assert_eq!(
                    (
                        locked.int("locking"),
                        locked.int("progress"),
                        locked.state.lock_scenes.entry_pending(),
                    ),
                    before,
                    "a peek at the lock changed what it reads"
                );
                assert!(
                    locked.state.lock_scenes.peeking().is_none()
                        && !locked.state.lock_scenes.exiting(locked.state.clock.now()),
                    "a peek at the lock is under way, or played an exit"
                );
                assert!(
                    natively_locked(&locked.state),
                    "a peek at the lock unlocked it"
                );
                // Then the same once the entry is over.
                let enter = locked.state.lock_scenes.settings().enter;
                locked.state.clock.advance(enter);
                crate::lock_scene::settle(&mut locked.state);
            }
            let output = locked.output.clone();
            let frame = locked.state.lock_frame();
            locked.state.frame_presented(&output, frame);
            assert!(
                !locked
                    .state
                    .lock
                    .as_ref()
                    .is_some_and(crate::lock::Lock::pending),
                "the premise: the lock is confirmed"
            );
            locked.state.lock_peek(Some(0.9));
            assert_eq!(
                locked.state.lock_frame(),
                frame,
                "a peek at the lock took a fresh lock"
            );
            assert!(
                !locked
                    .state
                    .lock
                    .as_ref()
                    .is_some_and(crate::lock::Lock::pending)
            );
        });
    }

    /// **The shipped scene follows `Lock.progress` while peeking**: its sheet
    /// is exactly where the peek is, with no lag behind the finger and no
    /// QML frame in between; let go, it goes back up and off the top
    /// through the exit; locked from a peek, it comes the rest of the way
    /// down from there, never from the top.
    #[test]
    fn the_shipped_scene_follows_progress_while_peeking() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            let mut peeked = Locked::unlocked("peek-sheet", &Shows::Shipped);
            assert_eq!(
                peeked.state.lock_scenes.shown_on(&peeked.output),
                crate::lock_scene::Shown::Shipped,
                "the premise: the shipped scene"
            );
            let down = |peeked: &mut Locked| {
                peeked.state.publish_models();
                peeked.scene().get_real("down", -1.0)
            };
            for progress in [0.3, 0.7, 0.45] {
                peeked.state.lock_peek(Some(progress));
                let at = down(&mut peeked);
                assert!(
                    (at - progress).abs() < 1e-9,
                    "the sheet is at {at}, not at the peek's {progress}"
                );
                assert!(
                    peeked.scene().get_bool("moving"),
                    "no shadow under a sheet pulled part of the way"
                );
            }

            peeked.state.lock_peek(None);
            peeked.state.publish_models();
            run_qml_clock(100);
            let going = down(&mut peeked);
            assert!(
                going > 0.0 && going < 0.45,
                "the sheet is not going back up from where it was: {going}"
            );
            let exit = peeked.state.lock_scenes.settings().exit;
            run_qml_clock(u64::try_from(exit.as_millis()).unwrap_or(600) + 100);
            assert!(down(&mut peeked) < 0.01, "the sheet did not go off the top");
            peeked.state.clock.advance(exit);
            crate::lock_scene::settle(&mut peeked.state);

            peeked.state.lock_peek(Some(0.5));
            assert!((down(&mut peeked) - 0.5).abs() < 1e-9);
            peeked.state.lock_natively();
            let from = down(&mut peeked);
            assert!(
                (from - 0.5).abs() < 1e-9,
                "the sheet jumped as the lock was taken from a peek: {from}"
            );
            run_qml_clock(60);
            let coming = down(&mut peeked);
            assert!(
                coming > 0.5 && coming < 1.0,
                "the sheet is not coming the rest of the way down: {coming}"
            );
            let enter = peeked.state.lock_scenes.settings().enter;
            peeked.state.clock.advance(enter / 2);
            crate::lock_scene::settle(&mut peeked.state);
            assert!(
                !peeked.state.lock_scenes.entry_pending(),
                "the entry from half way lasted longer than half of `enter_ms`"
            );
            assert!(
                (down(&mut peeked) - 1.0).abs() < 1e-9,
                "the sheet is not down after the entry"
            );
        });
    }

    /// **The prompt is on the monitor `lock.prompt_on` names, and no
    /// other**: with the shipped scene on two monitors, `"pointer"` puts it
    /// where the pointer is and follows the pointer, `"primary"` on the
    /// primary monitor, and a name on that monitor, or on the primary one
    /// while no monitor has that name.
    #[test]
    fn the_prompt_is_on_the_monitor_prompt_on_names_and_no_other() {
        use crate::lock_scene::PromptOn;
        crate::qml::qt_test::on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let _ = take_requests();
            let (_display, mut state, _shared) = solium(0);
            let monitor = |name: &str| {
                let output = Output::new(
                    name.to_owned(),
                    PhysicalProperties {
                        size: (0, 0).into(),
                        subpixel: Subpixel::Unknown,
                        make: "solium".to_owned(),
                        model: "lock-prompt".to_owned(),
                    },
                );
                output.change_current_state(
                    Some(Mode {
                        size: (1920, 1080).into(),
                        refresh: 60_000,
                    }),
                    None,
                    Some(Scale::Fractional(1.0)),
                    None,
                );
                output
            };
            let (left, right) = (monitor("lock-prompt-left"), monitor("lock-prompt-right"));
            state.space.map_output(&left, (0, 0));
            state.space.map_output(&right, (1920, 0));
            state.lock_natively();
            for output in [&left, &right] {
                assert_eq!(
                    state.lock_scenes.shown_on(output),
                    crate::lock_scene::Shown::Shipped,
                    "the premise: the shipped scene"
                );
                state
                    .lock_scenes
                    .instance_for_test(output)
                    .expect("the scene")
                    .scene_for_test()
                    .resize(1920, 1080, 1.0);
            }
            let prompted = |state: &mut Solium| -> (bool, bool) {
                state.publish_models();
                let mut on = |output: &Output| {
                    state
                        .lock_scenes
                        .instance_for_test(output)
                        .expect("the scene")
                        .scene_for_test()
                        .get_bool("prompt")
                };
                (on(&left), on(&right))
            };
            let region = Rectangle::from_size((3840, 1080).into());
            let point = |state: &mut Solium, to: (f64, f64)| {
                let was = state
                    .seat
                    .get_pointer()
                    .map(|pointer| pointer.current_location())
                    .unwrap_or_default();
                crate::synth::send_motion(state, region, Point::from(to) - was, 1);
            };

            point(&mut state, (100.0, 100.0));
            assert_eq!(
                prompted(&mut state),
                (true, false),
                "\"pointer\", on the left"
            );
            point(&mut state, (2000.0, 100.0));
            assert_eq!(
                prompted(&mut state),
                (false, true),
                "\"pointer\", moved right"
            );

            for (prompt_on, expected) in [
                (PromptOn::Primary, (true, false)),
                (
                    PromptOn::Monitor("lock-prompt-right".to_owned()),
                    (false, true),
                ),
                (PromptOn::Monitor("unplugged".to_owned()), (true, false)),
            ] {
                let settings = scene_settings(&state, |settings| {
                    settings.prompt_on = prompt_on.clone();
                });
                state.lock_scenes.configure(settings);
                assert_eq!(prompted(&mut state), expected, "{prompt_on:?}");
            }
            // Nothing is typed here: each character is a dot that scales in
            // on a `Behavior`, and a scene freed with one in flight leaves
            // the animation clock running into the next test on the Qt
            // thread (`qml::wake::tests::a_clock_scene_repaints_once_a_second_with_no_other_damage`).
            // `typed_text_never_reaches_the_lock_scene` counts the dots.
        });
    }

    /// A scene hosted on `locked`'s monitor as any `sol.surface` is, not as
    /// a lock's, from `source`, in the test's directory.
    fn session_scene(locked: &Locked, source: &str) -> crate::surface::ShellSurface {
        // A directory of its own: Qt has listed the lock scene's already.
        let directory = locked.directory.join("session");
        std::fs::create_dir_all(&directory).expect("a directory for the scene");
        let path = directory.join("Session.qml");
        std::fs::write(&path, source).expect("writing the scene");
        crate::surface::ShellSurface::hosted(path, "{}", &locked.output.name())
            .expect("the session's scene builds")
    }

    /// What reads the session's models, in a lock scene and in a desktop
    /// one alike.
    const READS_THE_SESSION: &str = r#"import QtQuick
                import Solium
                Item {
                    readonly property string status: Solium.status
                    readonly property string desktop: Solium.dirs.desktop
                    readonly property int apps: Apps.count
                    readonly property int workspaces: Workspaces.count
                    readonly property int listed: list.count
                    readonly property int arranged: Object.keys(Workspaces.arrangement).length
                    readonly property string monitor: Solium.monitor.name
                    readonly property int layouts: Keyboard.layouts.length
                    WorkspaceList { id: list }
                }"#;

    /// **A lock scene reads only what a lock may show, at all times**:
    /// `Solium.status`, `Apps`, `Workspaces`, a `WorkspaceList` and the
    /// arrangement read empty in it while Solium holds the lock, and still
    /// empty once it is unlocked and the scene waits, quiet, for the next
    /// lock, while a desktop scene beside it reads them all; the monitor and
    /// the keyboard read in both. While locked, the session's models read
    /// empty for every scene still drawn, the pointer's among them.
    #[test]
    fn a_lock_scene_reads_only_what_a_lock_may_show() {
        use crate::models::workspaces::{Arrangement, Declared, Group, Workspace};
        crate::qml::qt_test::on_the_qt_thread(|| {
            let mut locked = Locked::new("redacted", Some(READS_THE_SESSION));
            let mut desktop = session_scene(&locked, READS_THE_SESSION);
            let name = locked.output.name();
            locked.state.status = "a window's secret title".to_owned();
            locked.state.apps_scan_pending = false;
            locked.state.apps = vec![crate::apps::Entry {
                id: "solium-lock-redaction-test".to_owned(),
                name: "Redacted".to_owned(),
                untranslated_name: "Redacted".to_owned(),
                generic_name: String::new(),
                icon: String::new(),
                exec: "true".to_owned(),
                categories: Vec::new(),
                keywords: Vec::new(),
                terminal: false,
                path: None,
                source: PathBuf::from("/nonexistent/solium-lock-redaction-test.desktop"),
            }];
            locked.state.workspaces = Some(Declared {
                arrangement: Arrangement {
                    kind: "grid".to_owned(),
                    columns: 2,
                    rows: 1,
                },
                groups: vec![Group {
                    id: "lock-redaction".to_owned(),
                    monitors: vec![name.clone()],
                    showing: vec!["1".to_owned()],
                    workspaces: (1..=2)
                        .map(|index| Workspace {
                            id: index.to_string(),
                            name: format!("desk {index}"),
                            col: index,
                            row: 1,
                            hidden: false,
                        })
                        .collect(),
                }],
                windows: std::collections::BTreeMap::new(),
            });
            let read = |scene: &mut crate::qml::Scene| {
                (
                    scene.get_string_for_test("status"),
                    scene.get_int("apps"),
                    scene.get_int("workspaces"),
                    scene.get_int("listed"),
                    scene.get_int("arranged"),
                )
            };
            let empty = (String::new(), 0, 0, 0, 0);

            locked.state.publish_models();
            assert_eq!(read(locked.scene()), empty, "the lock scene, locked");
            assert_eq!(
                read(desktop.scene_for_test()),
                empty,
                "a desktop scene, locked"
            );
            for scene in [locked.scene(), desktop.scene_for_test()] {
                assert_eq!(scene.get_string_for_test("monitor"), name);
                assert!(scene.get_int("layouts") > 0, "the keyboard does not read");
            }

            locked.state.unlock_natively();
            locked.state.publish_models();
            let (status, apps, workspaces, listed, arranged) = read(desktop.scene_for_test());
            assert_eq!(status, "a window's secret title");
            assert!(apps >= 1 && workspaces >= 2 && listed >= 2 && arranged > 0);
            assert_eq!(
                read(locked.scene()),
                empty,
                "the lock scene, kept for the next lock, reads the session"
            );

            locked.state.lock_natively();
            locked.state.publish_models();
            assert_eq!(read(locked.scene()), empty, "the lock scene, the next lock");
            // Through that lock's entry the desktop is still on the screen,
            // under the scene coming down, and does not empty under it; from
            // the entry's end it reads nothing.
            assert_eq!(
                read(desktop.scene_for_test()).0,
                "a window's secret title",
                "a desktop scene emptied under the entry"
            );
            let enter = locked.state.lock_scenes.settings().enter;
            locked.state.clock.advance(enter);
            crate::lock_scene::settle(&mut locked.state);
            locked.state.publish_models();
            assert_eq!(
                read(desktop.scene_for_test()),
                empty,
                "a desktop scene, after the entry"
            );
        });
    }

    /// **A desktop scene reads nothing of the lock, and cannot submit**:
    /// with a password being typed at the lock, a scene hosted as any
    /// `sol.surface` is reads `Lock.state` as `"idle"`, no attempts and no
    /// user, its `SecretField` reads nothing and its `SecretKeypad` lays out
    /// no key and hears no press; its `Lock.submit()` and `Lock.clear()` go
    /// nowhere. The lock's own scene reads it all, and its own `submit()`
    /// still checks.
    #[test]
    fn a_session_scene_reads_nothing_of_the_lock_and_cannot_submit() {
        const WATCHES: &str = r#"import QtQuick
                import Solium
                Item {
                    id: root
                    readonly property string phase: Lock.state
                    readonly property int attempts: Lock.attempts
                    readonly property string user: (Lock.user && Lock.user.name) || ""
                    readonly property int length: field.length
                    readonly property int accepting: field.accepting ? 1 : 0
                    readonly property int keys: pad.children.length
                    readonly property string layout: pad.layout
                    property int keyPresses: 0
                    property int go: 0
                    onGoChanged: {
                        if (go === 1) Lock.clear()
                        if (go === 2) Lock.submit()
                    }
                    SecretField { id: field }
                    SecretKeypad {
                        id: pad
                        width: 300
                        height: 400
                        onKeyPressed: root.keyPresses += 1
                        delegate: Item {}
                    }
                }"#;
        crate::qml::qt_test::on_the_qt_thread(|| {
            let mut locked = Locked::new("session", Some(WATCHES));
            let mut desktop = session_scene(&locked, WATCHES);
            keys(&mut locked.state, "wrong");
            locked.state.publish_models();
            assert_eq!(
                locked.int("length"),
                5,
                "the premise: the lock scene reads it"
            );
            assert_eq!(locked.int("keys"), 12, "the premise: the lock's pad");
            assert_eq!(locked.scene().get_string_for_test("phase"), "prompting");
            let desktop_reads = |desktop: &mut crate::surface::ShellSurface| {
                let scene = desktop.scene_for_test();
                (
                    scene.get_string_for_test("phase"),
                    scene.get_int("attempts"),
                    scene.get_string_for_test("user"),
                    scene.get_int("length"),
                    scene.get_int("accepting"),
                    scene.get_int("keys"),
                    scene.get_string_for_test("layout"),
                )
            };
            let nothing = ("idle".to_owned(), 0, String::new(), 0, 0, 0, String::new());
            assert_eq!(desktop_reads(&mut desktop), nothing);

            // A key on the lock's own pad: its scene hears it, the desktop's
            // does not.
            let region = locked.region();
            crate::synth::send_touch(&mut locked.state, region, (50.0, 50.0), 10);
            locked.state.publish_models();
            assert_eq!(locked.int("length"), 6, "the premise: the pad typed");
            assert_eq!(desktop.scene_for_test().get_int("keyPresses"), 0);

            desktop.scene_for_test().set_int("go", 1);
            settle(&mut locked.state);
            desktop.scene_for_test().set_int("go", 2);
            settle(&mut locked.state);
            settle(&mut locked.state);
            assert_eq!(
                locked.state.lock_auth.length(),
                6,
                "the desktop's clear() cleared the password"
            );
            assert!(
                asked(&locked.shared).timeouts.is_empty(),
                "the desktop's submit() checked it"
            );
            assert_eq!(desktop_reads(&mut desktop), nothing);

            locked.scene().set_int("go", 2);
            settle(&mut locked.state);
            settle(&mut locked.state);
            assert_eq!(
                asked(&locked.shared).timeouts.len(),
                1,
                "the lock scene's own submit() checked nothing"
            );
            assert!(natively_locked(&locked.state));
        });
    }

    /// **The pointer near the keypad is kept from the scene**: within
    /// `lock.keypad_margin` of the pad, beside it and not on it, the scene
    /// hears no motion and is told the pointer left; with the margin 0, the
    /// same point is the scene's again.
    #[test]
    fn the_pointer_near_the_keypad_is_kept_from_the_scene() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            let mut locked = Locked::new(
                "margin",
                Some(
                    r#"import QtQuick
                import Solium
                Item {
                    id: root
                    property int moves: 0
                    readonly property int hovered: area.containsMouse ? 1 : 0
                    MouseArea {
                        id: area
                        anchors.fill: parent
                        hoverEnabled: true
                        onPositionChanged: root.moves += 1
                    }
                    SecretKeypad { width: 300; height: 400; delegate: Item {} }
                }"#,
                ),
            );
            locked.point((600.0, 600.0));
            locked.point((590.0, 590.0));
            let moves = locked.int("moves");
            assert!(moves > 0, "the premise: the scene hears the pointer");
            assert_eq!(locked.int("hovered"), 1);

            // Twenty pixels right of the pad, inside the default margin.
            locked.point((320.0, 200.0));
            locked.point((325.0, 210.0));
            assert_eq!(
                locked.int("moves"),
                moves,
                "the scene heard the pointer beside the pad"
            );
            assert_eq!(locked.int("hovered"), 0, "the scene was not told it left");

            let settings = scene_settings(&locked.state, |settings| settings.keypad_margin = 0);
            locked.state.lock_scenes.configure(settings);
            locked.point((320.0, 220.0));
            locked.point((330.0, 230.0));
            assert!(
                locked.int("moves") > moves,
                "with no margin, the point beside the pad is still kept"
            );
            assert_eq!(locked.int("hovered"), 1);
        });
    }

    /// **At the native lock a `locked` binding never takes a character**: a
    /// letter bound with `locked = true`, plain or with Shift, goes into the
    /// password and its handler does not run; one bound to `super` and a
    /// letter, which types nothing there, and a media key still fire. All
    /// four fire while unlocked.
    #[test]
    fn at_the_native_lock_a_locked_binding_never_takes_a_character() {
        const SUPER: u32 = 133;
        let (_display, mut state, _shared) = solium(0);
        crate::keyboard_change::tests::configured(
            &mut state,
            "lock-auth-bindings",
            r#"
                sol.bind("a", function() sol.status("a") end, { locked = true })
                sol.bind("shift+b", function() sol.status("B") end, { locked = true })
                sol.bind("super+c", function() sol.status("c") end, { locked = true })
                sol.bind("XF86AudioMute", function() sol.status("mute") end, { locked = true })
            "#,
        );
        let mute = crate::keymap::keycode_of(&mut state, Keysym::XF86_AudioMute)
            .expect("us,ru has a mute key")
            .raw();
        let fired = |state: &mut Solium, codes: &[u32]| {
            tap(state, codes);
            std::mem::take(&mut state.status)
        };
        let presses = [
            vec![code('a')],
            vec![SHIFT, code('b')],
            vec![SUPER, code('c')],
            vec![mute],
        ];
        let unlocked: Vec<String> = presses
            .iter()
            .map(|codes| fired(&mut state, codes))
            .collect();
        assert_eq!(unlocked, ["a", "B", "c", "mute"], "the premise, unlocked");

        state.lock_natively();
        let at_the_lock: Vec<String> = presses
            .iter()
            .map(|codes| fired(&mut state, codes))
            .collect();
        assert_eq!(
            at_the_lock,
            ["", "", "c", "mute"],
            "(a, shift+b, super+c, mute) at the native lock"
        );
        assert_eq!(
            state.lock_auth.length(),
            2,
            "the letters did not reach the password"
        );
    }
}
