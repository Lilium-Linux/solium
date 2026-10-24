//! The PIN at Solium's own lock (#281): where its files are, what the lock
//! remembers about it between locks and restarts, and when it takes one.
//!
//! **The PIN file** (`$XDG_STATE_HOME/solium/pin`, or
//! `~/.local/state/solium/pin`) holds the argon2id hash `solium pin set`
//! stored, never the PIN (`solium_auth::pin`). The compositor reads it only
//! to know whether a PIN is set; the check is the `solium-auth` helper's.
//!
//! **The state file** beside it (`pin-state`, 0600, written atomically) holds
//! the wrong PINs in a row, and when the password last unlocked the lock, by
//! the wall clock and by `CLOCK_BOOTTIME`, with the boot that was
//! ([`Record`]). So restarting Solium does not give anyone a fresh run of
//! guesses (`lock_auth::tests::the_pin_counts_survive_a_restart`). It is read
//! only as the PIN file is (`solium_auth::pin::read_private`): a file that is
//! a link, not this user's, writable by others or too long, or in a directory
//! that is not this user's or that others can write, is no record, and the
//! password is needed.
//!
//! **When a PIN is set, the lock takes it** ([`Mode::Pin`]), unless one of
//! these says the password is needed instead ([`Fallback`]):
//!
//! * `lock.pin.attempts` wrong PINs in a row, until the password is entered;
//! * the machine has started since the password last unlocked it: the
//!   kernel's boot id is not the one recorded then;
//! * more than `lock.pin.password_after_hours` since the password last
//!   unlocked it, by either clock: `CLOCK_BOOTTIME`, which counts suspend and
//!   which nobody can set, and the wall clock; or a clock that says the
//!   password was used in the future. Setting the wall clock back cannot
//!   stretch the hours;
//! * the state could not be written, or cannot be trusted, so a wrong PIN
//!   might not be counted.
//!
//! A state file that is missing or does not parse records no password
//! unlock, so the password is needed: a PIN is never taken on less than a
//! record that says it may be.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use solium_auth::pin::Stored;

/// Where the kernel says which boot this is: a new random id each boot.
pub(crate) const BOOT_ID: &str = "/proc/sys/kernel/random/boot_id";

/// `lock.pin`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Settings {
    /// Whether a PIN that is set is taken at all: `lock.pin.enabled`.
    pub(crate) enabled: bool,
    /// The fewest digits a PIN has: `lock.pin.min_length`, at least
    /// [`MIN_LENGTH`].
    pub(crate) min_length: usize,
    /// The most: `lock.pin.max_length`, at most [`MAX_LENGTH`].
    pub(crate) max_length: usize,
    /// Wrong PINs in a row before the password is needed:
    /// `lock.pin.attempts`.
    pub(crate) attempts: u32,
    /// How long after the password last unlocked the lock a PIN is still
    /// taken: `lock.pin.password_after_hours`.
    pub(crate) password_after: Duration,
}

/// The fewest digits any PIN may have, whatever `lock.pin.min_length` says.
pub(crate) const MIN_LENGTH: usize = 4;

/// The most digits any PIN may have, whatever `lock.pin.max_length` says.
pub(crate) const MAX_LENGTH: usize = 64;

/// The most wrong PINs `lock.pin.attempts` may allow before the password is
/// needed.
pub(crate) const MAX_ATTEMPTS: u32 = 10;

/// The longest `lock.pin.password_after_hours` may be: three days.
pub(crate) const MAX_PASSWORD_AFTER_HOURS: u32 = 72;

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            min_length: 4,
            max_length: 8,
            attempts: 5,
            password_after: Duration::from_secs(48 * 3600),
        }
    }
}

impl Settings {
    /// `password_after` in whole hours, as the setting and the message say it.
    pub(crate) fn hours(&self) -> u64 {
        self.password_after.as_secs() / 3600
    }
}

/// Where the PIN's files are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Paths {
    /// The hash.
    pub(crate) pin: PathBuf,
    /// The counts: [`Record`].
    pub(crate) state: PathBuf,
    /// The kernel's boot id: [`BOOT_ID`], or a test's.
    pub(crate) boot_id: PathBuf,
}

impl Paths {
    /// This user's: under `$XDG_STATE_HOME/solium`, or
    /// `~/.local/state/solium`. None without either variable.
    #[cfg(not(test))]
    pub(crate) fn of_this_user() -> Option<Self> {
        crate::state_directory().map(|dir| Self::in_dir(&dir))
    }

    /// In a test, nobody's: no test reads the PIN of whoever runs it. A test
    /// that wants a PIN names its own directory.
    #[cfg(test)]
    pub(crate) fn of_this_user() -> Option<Self> {
        None
    }

    /// The files in `dir`, with the kernel's boot id.
    pub(crate) fn in_dir(dir: &Path) -> Self {
        Self {
            pin: dir.join("pin"),
            state: dir.join("pin-state"),
            boot_id: PathBuf::from(BOOT_ID),
        }
    }
}

/// Now, by the two clocks the PIN's hours are counted on, in seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Clocks {
    /// Since the epoch: survives a restart, but anyone at the session can set
    /// it.
    pub(crate) wall: u64,
    /// `CLOCK_BOOTTIME`: since this boot, suspend included, and settable by
    /// nobody. Only comparable within one boot, which the boot id says.
    pub(crate) boottime: u64,
}

impl Clocks {
    /// The two clocks now.
    pub(crate) fn now() -> Self {
        Self {
            wall: wall_now(),
            boottime: boottime_now(),
        }
    }

    /// A test's: both clocks reading `seconds`, so moving one test clock
    /// moves both.
    #[cfg(test)]
    pub(crate) const fn at(seconds: u64) -> Self {
        Self {
            wall: seconds,
            boottime: seconds,
        }
    }
}

/// What the state file records.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Record {
    /// Wrong PINs in a row.
    pub(crate) failures: u32,
    /// When the password last unlocked the lock, in seconds since the epoch.
    pub(crate) password_at: Option<u64>,
    /// The same moment by `CLOCK_BOOTTIME`, on the boot `boot` names.
    pub(crate) password_boottime: Option<u64>,
    /// The boot id then.
    pub(crate) boot: Option<String>,
}

impl Record {
    /// What a password unlock leaves: no wrong PINs, now, this boot.
    pub(crate) fn after_password(now: Clocks, boot: Option<String>) -> Self {
        Self {
            failures: 0,
            password_at: Some(now.wall),
            password_boottime: Some(now.boottime),
            boot,
        }
    }

    /// The file's text: a line per value.
    pub(crate) fn render(&self) -> String {
        let mut text =
            String::from("# Solium's lock screen: the PIN's counts (docs/ricing.md, \"A PIN\").\n");
        text.push_str(&format!("failures {}\n", self.failures));
        if let Some(at) = self.password_at {
            text.push_str(&format!("password_at {at}\n"));
        }
        if let Some(at) = self.password_boottime {
            text.push_str(&format!("password_boottime {at}\n"));
        }
        if let Some(boot) = &self.boot {
            text.push_str(&format!("boot {boot}\n"));
        }
        text
    }

    /// The file's text read back, or why it does not parse.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let mut record = Self::default();
        let mut failures = false;
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line
                .split_once(' ')
                .map(|(key, value)| (key, value.trim()))
                .ok_or_else(|| format!("{line:?} is not a key and a value"))?;
            match key {
                "failures" => {
                    record.failures = value
                        .parse()
                        .map_err(|_| format!("failures {value:?} is not a count"))?;
                    failures = true;
                }
                "password_at" => {
                    record.password_at = Some(
                        value
                            .parse()
                            .map_err(|_| format!("password_at {value:?} is not a time"))?,
                    );
                }
                "password_boottime" => {
                    record.password_boottime = Some(
                        value
                            .parse()
                            .map_err(|_| format!("password_boottime {value:?} is not a time"))?,
                    );
                }
                "boot" => {
                    if value.is_empty() || value.contains(char::is_whitespace) {
                        return Err(format!("boot {value:?} is not a boot id"));
                    }
                    record.boot = Some(value.to_owned());
                }
                other => return Err(format!("{other:?} is not something it records")),
            }
        }
        if !failures {
            return Err("it has no count of wrong PINs".to_owned());
        }
        Ok(record)
    }

    /// The state file at `path`: a missing one is a fresh record, which
    /// records no password unlock; one that cannot be trusted
    /// (`solium_auth::pin::read_private`) or does not parse is the same, with
    /// why.
    pub(crate) fn read(path: &Path) -> (Self, Option<String>) {
        match solium_auth::pin::read_private(path) {
            Ok(Some(text)) => match Self::parse(&text) {
                Ok(record) => (record, None),
                Err(why) => (
                    Self::default(),
                    Some(format!("{} does not parse: {why}", path.display())),
                ),
            },
            Ok(None) => (Self::default(), None),
            Err(why) => (Self::default(), Some(why)),
        }
    }

    /// Write it to `path`, atomically and 0600.
    pub(crate) fn write(&self, path: &Path) -> std::io::Result<()> {
        solium_auth::pin::write_private(path, self.render().as_bytes())
    }
}

/// The boot id at `path`: [`BOOT_ID`]'s line.
pub(crate) fn boot_id(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|id| !id.is_empty() && !id.contains(char::is_whitespace))
}

/// Seconds since the epoch, now.
pub(crate) fn wall_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Seconds of `CLOCK_BOOTTIME`, now: since this boot, suspend included.
pub(crate) fn boottime_now() -> u64 {
    // SAFETY: `timespec` is plain data; `clock_gettime` writes into it and
    // its answer is checked.
    #[expect(unsafe_code, reason = "calling libc")]
    let now = unsafe {
        let mut now: libc::timespec = std::mem::zeroed();
        (libc::clock_gettime(libc::CLOCK_BOOTTIME, &raw mut now) == 0).then_some(now)
    };
    now.and_then(|now| u64::try_from(now.tv_sec).ok())
        .unwrap_or(0)
}

/// Why the lock asks for the password where it would take a PIN.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fallback {
    /// `lock.pin.attempts` wrong PINs in a row.
    Attempts,
    /// The machine started since the password last unlocked the lock, or no
    /// password unlock is recorded.
    Boot,
    /// More than `lock.pin.password_after_hours` since then.
    Hours,
    /// A wrong PIN could not be recorded.
    Unsaved,
}

impl Fallback {
    /// What `Lock.message` says of it.
    pub(crate) fn message(self, settings: &Settings) -> String {
        match self {
            Self::Attempts => "Too many wrong PINs. Enter your password".to_owned(),
            Self::Boot => "Enter your password to use your PIN after a restart".to_owned(),
            Self::Hours => format!(
                "Your password is needed every {} hours",
                settings.hours().max(1)
            ),
            Self::Unsaved => "Your PIN can't be used right now. Enter your password".to_owned(),
        }
    }
}

/// What the lock takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// The password: there is no PIN, or `lock.pin.enabled` is off.
    NoPin,
    /// A PIN, or the password.
    Pin,
    /// Only the password, and why.
    Password(Fallback),
}

impl Mode {
    /// `Lock.mode`.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Pin => "pin",
            Self::NoPin | Self::Password(_) => "password",
        }
    }
}

/// What the lock knows of the PIN: whether one is set, the record, the boot.
#[derive(Debug, Default)]
pub(crate) struct Known {
    /// Where the files are; none, and there is no PIN.
    pub(crate) paths: Option<Paths>,
    /// Whether the PIN file holds a hash a check can use.
    pub(crate) set: bool,
    pub(crate) record: Record,
    /// The boot id now.
    pub(crate) boot: Option<String>,
    /// Whether a wrong PIN could not be recorded this run.
    pub(crate) unsaved: bool,
    /// Whether the state file, when last read, could not be trusted or did
    /// not parse.
    pub(crate) untrusted: bool,
    /// Why the PIN file or the state file was passed over, last time they
    /// were read: logged when it changes.
    complaint: Option<String>,
}

impl Known {
    pub(crate) fn new(paths: Option<Paths>) -> Self {
        Self {
            paths,
            ..Self::default()
        }
    }

    /// Read the files again: whether a PIN is set, the record, the boot.
    pub(crate) fn refresh(&mut self) {
        let Some(paths) = &self.paths else {
            self.set = false;
            return;
        };
        let mut complaints = Vec::new();
        self.set = match solium_auth::pin::read(&paths.pin) {
            Stored::Set(_) => true,
            Stored::None => false,
            Stored::Unusable(why) => {
                complaints.push(why);
                false
            }
        };
        let (record, why) = Record::read(&paths.state);
        self.untrusted = why.is_some();
        complaints.extend(why);
        self.record = record;
        self.boot = boot_id(&paths.boot_id);
        let complaint = (!complaints.is_empty()).then(|| complaints.join("; "));
        if complaint != self.complaint {
            if let Some(complaint) = &complaint {
                tracing::warn!(
                    complaint,
                    "the lock's PIN is not used: the password is needed"
                );
            }
            self.complaint = complaint;
        }
    }

    /// What the lock takes now.
    pub(crate) fn mode(&self, settings: &Settings, now: Clocks) -> Mode {
        if !settings.enabled || !self.set {
            return Mode::NoPin;
        }
        if self.unsaved || self.untrusted {
            return Mode::Password(Fallback::Unsaved);
        }
        if self.record.failures >= settings.attempts.max(1) {
            return Mode::Password(Fallback::Attempts);
        }
        match (&self.boot, &self.record.boot) {
            (Some(this), Some(then)) if this == then => {}
            _ => return Mode::Password(Fallback::Boot),
        }
        // Within the hours by both clocks; the boot id above already says the
        // boottime recorded is this boot's.
        let hours = settings.password_after.as_secs();
        let within = |at: u64, now: u64| at <= now && now - at <= hours;
        match (self.record.password_at, self.record.password_boottime) {
            (Some(wall), Some(boottime)) => {
                if within(wall, now.wall) && within(boottime, now.boottime) {
                    Mode::Pin
                } else {
                    Mode::Password(Fallback::Hours)
                }
            }
            _ => Mode::Password(Fallback::Boot),
        }
    }

    /// Save the record. Failing, the PIN is not taken again this run: a
    /// wrong PIN that cannot be counted must not be tried again.
    fn save(&mut self) {
        let Some(paths) = &self.paths else {
            return;
        };
        if let Err(error) = self.record.write(&paths.state) {
            tracing::error!(
                %error,
                path = %paths.state.display(),
                "the lock's PIN counts could not be saved: the password is needed until they can"
            );
            self.unsaved = true;
        }
    }

    /// A wrong PIN: counted, and saved before anything else is checked.
    pub(crate) fn failed(&mut self) {
        self.record.failures = self.record.failures.saturating_add(1);
        self.save();
    }

    /// The right PIN: the run of wrong ones ends.
    pub(crate) fn pin_unlocked(&mut self) {
        if self.record.failures > 0 {
            self.record.failures = 0;
            self.save();
        }
    }

    /// The password unlocked the lock: a fresh record, now and on this boot,
    /// which lets a PIN be taken again. Written only where a PIN is set.
    pub(crate) fn password_unlocked(&mut self, now: Clocks) {
        if !self.set {
            return;
        }
        self.record = Record::after_password(now, self.boot.clone());
        self.unsaved = false;
        self.untrusted = false;
        self.save();
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    #[test]
    fn a_record_reads_back_what_it_wrote_and_refuses_anything_else() {
        let record = Record {
            failures: 3,
            password_at: Some(1_761_300_000),
            password_boottime: Some(86_400),
            boot: Some("0b5e7c2a-6d1f-4c39-9d0e-6f1a2b3c4d5e".to_owned()),
        };
        assert_eq!(Record::parse(&record.render()), Ok(record));
        assert_eq!(
            Record::parse(&Record::default().render()),
            Ok(Record::default())
        );
        for text in [
            "",
            "failures many\n",
            "failures 1\npassword_at yesterday\n",
            "failures 1\npassword_boottime -5\n",
            "failures 1\nboot \n",
            "failures 1\nunlocks 7\n",
            "failures\n",
        ] {
            assert!(Record::parse(text).is_err(), "{text:?}");
        }
    }

    /// The rules, in order: attempts, then the boot, then the hours.
    #[test]
    fn the_mode_follows_the_rules() {
        let settings = Settings {
            attempts: 2,
            password_after: Duration::from_secs(10 * 3600),
            ..Settings::default()
        };
        let at = Clocks::at;
        let mut known = Known {
            set: true,
            record: Record::after_password(at(1000), Some("a".to_owned())),
            boot: Some("a".to_owned()),
            ..Known::default()
        };
        assert_eq!(known.mode(&settings, at(1000)), Mode::Pin);
        assert_eq!(known.mode(&settings, at(1000 + 10 * 3600)), Mode::Pin);
        assert_eq!(
            known.mode(&settings, at(1001 + 10 * 3600)),
            Mode::Password(Fallback::Hours)
        );
        assert_eq!(
            known.mode(&settings, at(999)),
            Mode::Password(Fallback::Hours),
            "a password unlock in the future"
        );
        known.boot = Some("b".to_owned());
        assert_eq!(
            known.mode(&settings, at(1000)),
            Mode::Password(Fallback::Boot)
        );
        known.boot = None;
        assert_eq!(
            known.mode(&settings, at(1000)),
            Mode::Password(Fallback::Boot)
        );
        known.boot = Some("a".to_owned());
        known.record.failures = 2;
        assert_eq!(
            known.mode(&settings, at(1000)),
            Mode::Password(Fallback::Attempts)
        );
        known.record.failures = 0;
        known.record.password_boottime = None;
        assert_eq!(
            known.mode(&settings, at(1000)),
            Mode::Password(Fallback::Boot)
        );
        known.record.password_boottime = Some(1000);
        known.record.password_at = None;
        assert_eq!(
            known.mode(&settings, at(1000)),
            Mode::Password(Fallback::Boot)
        );
        known.record.password_at = Some(1000);
        known.untrusted = true;
        assert_eq!(
            known.mode(&settings, at(1000)),
            Mode::Password(Fallback::Unsaved)
        );
        known.untrusted = false;
        known.set = false;
        assert_eq!(known.mode(&settings, at(1000)), Mode::NoPin);
        known.set = true;
        let off = Settings {
            enabled: false,
            ..settings
        };
        assert_eq!(known.mode(&off, at(1000)), Mode::NoPin);
    }

    /// **The hours are measured by a clock nobody can set as well**: with
    /// the wall clock set back to an hour after the password unlock while
    /// `CLOCK_BOOTTIME` says 49 hours have passed on this boot, the password
    /// is needed; and the other way round, a wall clock run forward past the
    /// hours needs it too, whichever is stricter.
    #[test]
    fn the_hours_are_counted_on_boottime_as_well_as_the_wall_clock() {
        let settings = Settings::default();
        let known = Known {
            set: true,
            record: Record::after_password(
                Clocks {
                    wall: 1_000_000,
                    boottime: 500,
                },
                Some("a".to_owned()),
            ),
            boot: Some("a".to_owned()),
            ..Known::default()
        };
        let hour = 3600;
        let now = |wall: u64, boottime: u64| Clocks { wall, boottime };
        assert_eq!(
            known.mode(&settings, now(1_000_000 + hour, 500 + hour)),
            Mode::Pin
        );
        assert_eq!(
            known.mode(&settings, now(1_000_000 + hour, 500 + 49 * hour)),
            Mode::Password(Fallback::Hours),
            "the wall clock set back stretched the hours"
        );
        assert_eq!(
            known.mode(&settings, now(1_000_000 + 49 * hour, 500 + hour)),
            Mode::Password(Fallback::Hours)
        );
        assert_eq!(
            known.mode(&settings, now(1_000_000 + hour, 499)),
            Mode::Password(Fallback::Hours),
            "a boottime before the unlock"
        );
        assert!(boottime_now() > 0, "CLOCK_BOOTTIME reads nothing");
    }

    /// **The state file is read only as the PIN file is**: a symbolic link,
    /// one others can write, one too long, one in a directory others can
    /// write: each is no record, and with a PIN set, the password is needed;
    /// a save into that directory is refused, logged and leaves the PIN off.
    #[test]
    fn a_state_file_that_cannot_be_trusted_needs_the_password() {
        let dir = std::env::temp_dir().join(format!("solium-pin-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a directory");
        let mut paths = Paths::in_dir(&dir.join("solium"));
        paths.boot_id = dir.join("boot_id");
        std::fs::write(&paths.boot_id, "a\n").expect("a boot id");
        let mut pin = solium_auth::Secret::new();
        assert!(pin.push_str("1379"));
        let phc = solium_auth::pin::hash(&pin).expect("a hash");
        solium_auth::pin::write_private(&paths.pin, phc.as_bytes()).expect("the PIN");
        let good = Record::after_password(Clocks::at(1000), Some("a".to_owned()));
        good.write(&paths.state).expect("the state");
        let settings = Settings::default();
        let mut known = Known::new(Some(paths.clone()));
        known.refresh();
        assert_eq!(
            known.mode(&settings, Clocks::at(1000)),
            Mode::Pin,
            "the premise"
        );

        // A link to a file that says the same.
        let elsewhere = dir.join("elsewhere");
        std::fs::write(&elsewhere, good.render()).expect("written");
        std::fs::remove_file(&paths.state).expect("removed");
        std::os::unix::fs::symlink(&elsewhere, &paths.state).expect("a link");
        known.refresh();
        assert_eq!(
            known.mode(&settings, Clocks::at(1000)),
            Mode::Password(Fallback::Unsaved),
            "a symbolic link was read"
        );
        std::fs::remove_file(&paths.state).expect("removed");

        // Writable by others.
        good.write(&paths.state).expect("the state");
        std::fs::set_permissions(&paths.state, std::fs::Permissions::from_mode(0o666))
            .expect("chmod");
        known.refresh();
        assert_eq!(
            known.mode(&settings, Clocks::at(1000)),
            Mode::Password(Fallback::Unsaved),
            "a file others can write was read"
        );
        std::fs::remove_file(&paths.state).expect("removed");

        // Too long.
        let long = format!("{}{}", good.render(), "#".repeat(2048));
        solium_auth::pin::write_private(&paths.state, long.as_bytes()).expect("written");
        known.refresh();
        assert_eq!(
            known.mode(&settings, Clocks::at(1000)),
            Mode::Password(Fallback::Unsaved),
            "a file too long was read"
        );

        // A directory others can write: a save into it is refused, and the
        // PIN is off; read again, the PIN in it is no PIN.
        good.write(&paths.state).expect("the state");
        known.refresh();
        assert_eq!(known.mode(&settings, Clocks::at(1000)), Mode::Pin);
        let state_dir = dir.join("solium");
        std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o777))
            .expect("chmod");
        known.password_unlocked(Clocks::at(2000));
        assert!(known.unsaved, "a save into a directory others can write");
        assert_eq!(
            known.mode(&settings, Clocks::at(2000)),
            Mode::Password(Fallback::Unsaved)
        );
        known.refresh();
        assert_eq!(known.mode(&settings, Clocks::at(2000)), Mode::NoPin);
        let mode = std::fs::metadata(&state_dir)
            .expect("its mode")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o777, "the directory was mended silently");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
