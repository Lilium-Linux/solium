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
//! * `idle`: nothing typed. The field is armed, so the first key goes into it.
//! * `prompting`: something typed.
//! * `checking`: the helper has the password; then, after a no, PAM's failure
//!   delay is waited out here, so the screen never freezes on it.
//! * `failed`: the last check said no. Typing starts again.
//! * `lockedOut`: `lock.lockout.attempts` noes in a row shut the field for
//!   `lock.lockout.seconds`, doubling with each lockout up to `max_seconds`.
//! * `unavailable`: the helper could not answer, whether missing, crashed,
//!   hung or without its PAM file. The password is kept, in its page, and
//!   tried again after `lock.retry.seconds`, doubling up to `max_seconds`,
//!   until an answer comes or the field is cleared. `Lock.message` gives the
//!   way out from another console.
//! * `unlocking`: the helper said yes, and Rust unlocked.
//!
//! `tests::a_yes_unlocks_and_a_no_counts_an_attempt`,
//! `tests::a_no_waits_out_pams_delay_while_checking`,
//! `tests::lockout_and_its_backoff_follow_the_settings`,
//! `tests::an_unavailable_check_is_retried_with_backoff_and_recovers`,
//! `tests::the_secret_is_zeroed_after_submit_and_on_unlock`.
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
    /// The rows `SecretKeypad` lays out: `lock.keypad`.
    pub(crate) keypad: Keypad,
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
            keypad: Keypad::Digits,
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

/// A stand-in for the helper, for a test: the answer to a check of a secret
/// for a user, given the timeout the check would have had.
#[cfg(test)]
pub(crate) type StandIn = Box<dyn FnMut(&str, &Secret, Duration) -> Outcome + Send>;

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
    /// Unavailable answers in a row: what doubles the next retry.
    unanswered: u32,
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

    /// Whether typing goes into the field now.
    pub(crate) fn accepting(&self) -> bool {
        matches!(self.phase, Phase::Idle | Phase::Prompting | Phase::Failed)
    }

    /// The lock was taken: a fresh field and fresh counts.
    pub(crate) fn begin(&mut self) {
        self.reset();
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
    fn submit(&mut self, now: Duration) {
        if !self.accepting() || self.typed.is_empty() {
            return;
        }
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
        self.unanswered = 0;
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
            });
            return;
        }
        let timeout = self.settings.helper_timeout;
        #[cfg(test)]
        if let Some(stand_in) = self.stand_in.as_mut() {
            let outcome = stand_in(&self.user.name, &secret, timeout);
            drop(secret);
            answer(outcome);
            return;
        }
        self.check = Some(crate::auth::check(&self.user.name, secret, timeout, answer));
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
                    tracing::info!("the lock's password check said yes");
                }
                Outcome::Failed { delay } => {
                    self.submitted.clear();
                    self.unanswered = 0;
                    self.attempts = self.attempts.saturating_add(1);
                    self.streak = self.streak.saturating_add(1);
                    // Checking still, through PAM's delay: the answer shows
                    // when it is over.
                    self.until = Some(now + delay);
                    tracing::info!(
                        attempts = self.attempts,
                        "the lock's password check said no"
                    );
                }
                Outcome::Unavailable { reason } => {
                    let wait = self.settings.retry.after(self.unanswered);
                    self.unanswered = self.unanswered.saturating_add(1);
                    self.phase = Phase::Unavailable;
                    self.until = Some(now + wait);
                    tracing::warn!(
                        reason,
                        retry_in_s = wait.as_secs(),
                        "the lock's password could not be checked"
                    );
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
                } else {
                    self.phase = Phase::Failed;
                }
            }
            Phase::LockedOut => self.phase = Phase::Idle,
            Phase::Unavailable => self.start_check(now),
            Phase::Idle | Phase::Prompting | Phase::Failed | Phase::Unlocking => {}
        }
        false
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

    /// The line that goes with the state.
    fn message(&self) -> String {
        match self.phase {
            Phase::Failed => "Wrong password".to_owned(),
            Phase::LockedOut => "Too many attempts".to_owned(),
            Phase::Unavailable => {
                let command = match std::env::var("XDG_SESSION_ID") {
                    Ok(id) if !id.is_empty() => format!("loginctl unlock-session {id}"),
                    _ => "loginctl unlock-sessions".to_owned(),
                };
                format!("From another console: {command}")
            }
            Phase::Idle | Phase::Prompting | Phase::Checking | Phase::Unlocking => String::new(),
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
    /// object: `prompt_on` is the monitor the prompt is on.
    fn view(&self, now: Duration, prompt_on: &str) -> String {
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
        object(vec![
            ("state", text(self.phase.name())),
            ("attempts", number(u64::from(self.attempts))),
            ("retryAfter", number(self.retry_after(now))),
            ("message", text(&self.message())),
            (
                "user",
                object(vec![
                    ("name", text(&self.user.name)),
                    ("realName", text(&self.user.real_name)),
                    ("avatar", text(&self.user.avatar)),
                ]),
            ),
            ("promptOn", text(prompt_on)),
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

    /// Answer every check with `stand_in` rather than the helper.
    #[cfg(test)]
    pub(crate) fn use_stand_in(&mut self, stand_in: StandIn) {
        self.stand_in = Some(stand_in);
    }

    /// Whether the field's page, and the submitted copy's, are all zeroes.
    #[cfg(test)]
    pub(crate) fn wiped_for_test(&self) -> (bool, bool) {
        (self.typed.is_wiped(), self.submitted.is_wiped())
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
        let prompt_on = self
            .active_output()
            .map(|output| output.name())
            .unwrap_or_default();
        self.lock_auth.view(self.clock.now(), &prompt_on)
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

    /// The one password the stand-in says yes to, and the one passcode.
    const RIGHT: &str = "correct horse";
    const PIN: &str = "2580";

    /// What the stand-in was asked, and what it is to answer first.
    #[derive(Default)]
    struct Asked {
        /// The timeout each check was given.
        timeouts: Vec<Duration>,
        /// Answers to give, in order, before judging by the password.
        scripted: VecDeque<Outcome>,
    }

    type Shared = Arc<Mutex<Asked>>;

    fn asked(shared: &Shared) -> std::sync::MutexGuard<'_, Asked> {
        shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A stand-in for the helper: yes to [`RIGHT`] and [`PIN`], no with PAM's
    /// two-second delay to anything else, unless an answer is scripted.
    fn stand_in(shared: &Shared) -> StandIn {
        let shared = Arc::clone(shared);
        Box::new(move |_user, secret, timeout| {
            let mut asked = asked(&shared);
            asked.timeouts.push(timeout);
            if let Some(outcome) = asked.scripted.pop_front() {
                return outcome;
            }
            if secret.expose() == RIGHT.as_bytes() || secret.expose() == PIN.as_bytes() {
                Outcome::Ok
            } else {
                Outcome::Failed {
                    delay: Duration::from_secs(2),
                }
            }
        })
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
        assert_eq!(auth.message(), "Wrong password");
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
            reason: "no solium-auth".to_owned(),
        };
        asked(&shared).scripted = [unavailable(), unavailable(), unavailable()].into();
        submit(&mut first, RIGHT, s(0));
        assert!(!first.settle(s(0)), "unavailable unlocked");
        assert_eq!(first.phase(), Phase::Unavailable);
        assert!(!first.accepting());
        assert!(
            first.message().contains("loginctl unlock-session"),
            "no way out: {:?}",
            first.message()
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

    /// A native lock with `source` as its scene, or the fallback, on one
    /// 1920x1080 monitor, built on the Qt thread and published to.
    struct Locked {
        _display: Display<Solium>,
        state: Solium,
        output: Output,
        shared: Shared,
        directory: PathBuf,
    }

    impl Locked {
        fn new(name: &str, source: Option<&str>) -> Self {
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
            if let Some(source) = source {
                std::fs::create_dir_all(&directory).expect("a directory for the scene");
                let path = directory.join("Lock.qml");
                std::fs::write(&path, source).expect("writing the scene");
                state.lock_scenes.use_scene(Some(path));
            }
            state.lock_natively();
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
    /// was typed like Return does: a wrong password counts an attempt.
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
                    function look(value) {
                        let text = ""
                        try {
                            text = typeof value === "function" ? ""
                                : JSON.stringify(value === undefined ? null : value)
                        } catch (error) {
                            text = String(value)
                        }
                        if (typeof text === "string" && text.indexOf("wrong") >= 0)
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
                        if (go === 2) {
                            try {
                                const lock = []
                                for (const name in Lock) if (typeof Lock[name] !== "function"
                                    && name !== "objectName") lock.push(name)
                                root.write(Lock, lock)
                                root.write(field, ["length", "selected", "accepting"])
                                root.write(pad, ["layout", "pressed"])
                                root.callAll(Lock)
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
}
