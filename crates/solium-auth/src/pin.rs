//! The PIN for Solium's own lock (#281): a short run of digits that unlocks it
//! in place of the password, while the lock allows one.
//!
//! **Only its hash is ever stored.** The PIN file holds one argon2id PHC
//! string, `$argon2id$v=19$m=…,t=…,p=…$<salt>$<hash>`: the salt and the
//! parameters, and the hash of the PIN under them. The PIN itself is never
//! written anywhere (`tests::the_file_holds_a_hash_and_never_the_pin`). The
//! parameters ([`M_COST_KIB`], [`T_COST`]) make one check take about a quarter
//! of a second on a laptop's processor and 64 MiB of memory, so the ten
//! thousand four-digit PINs cannot be tried from a stolen copy of the file in
//! a moment, nor on a graphics card cheaply.
//!
//! **The file is written atomically** ([`write_private`]): into a new file
//! beside it, mode 0600, which is synced and then renamed over it, so a crash
//! leaves the old file or the new one and never half of either.
//!
//! **A file that cannot be trusted is no PIN** ([`read`]): one that is not a
//! regular file of this user's, one others can write (they could put the hash
//! of a PIN of their own there), one too long, or one that does not parse as
//! an argon2id hash with sane parameters. The lock then asks for the password,
//! and the log says why.
//!
//! The check itself ([`check`]) runs in the `solium-auth` helper, as the
//! password's does, never in the compositor: the compositor sends the typed
//! PIN as a [`Secret`] and is answered yes, no or unavailable
//! (`crate::protocol`). Setting one is `solium pin set`, which checks the
//! account password through the helper first.

use std::{
    io::{self, Read as _, Write as _},
    os::unix::fs::{
        DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _,
    },
    path::Path,
};

use argon2::{
    Algorithm, Argon2, Params, PasswordHasher as _, PasswordVerifier as _, Version,
    password_hash::{self, PasswordHash, SaltString},
};

use crate::{Answer, Secret};

/// The memory a check takes, in KiB: 64 MiB. With [`T_COST`], about 100 ms
/// on a desktop processor of 2021 and 200 to 300 ms on a Surface-class
/// laptop's, measured on this implementation.
pub const M_COST_KIB: u32 = 65_536;

/// The passes over that memory.
pub const T_COST: u32 = 3;

/// The lanes. RustCrypto's argon2 computes them one after another, so more
/// would only make a check slower here and no slower for anyone else.
pub const P_COST: u32 = 1;

/// The longest file read, in bytes: a PHC string with these parameters is
/// about a hundred.
const MAX_FILE: u64 = 1024;

/// The most memory a stored hash may ask a check for, in KiB: 1 GiB. A file
/// asking for more is refused rather than handed to the helper's allocator.
const MAX_M_COST_KIB: u32 = 1 << 20;

/// The most passes a stored hash may ask for.
const MAX_T_COST: u32 = 64;

/// The most lanes a stored hash may ask for.
const MAX_P_COST: u32 = 16;

/// What the PIN file holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stored {
    /// There is none: no PIN is set.
    None,
    /// There is one, but it cannot be used, and why: no PIN, as far as the
    /// lock is concerned.
    Unusable(String),
    /// A PIN's hash: the PHC string. Not secret, but not the PIN either.
    Set(String),
}

/// What the PIN file at `path` holds. See the module documentation for what
/// makes a file [`Stored::Unusable`].
#[must_use]
pub fn read(path: &Path) -> Stored {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path);
    let file = match file {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Stored::None,
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
            return Stored::Unusable(format!("{} is a symbolic link", path.display()));
        }
        Err(error) => {
            return Stored::Unusable(format!("{} would not open: {error}", path.display()));
        }
    };
    let meta = match file.metadata() {
        Ok(meta) => meta,
        Err(error) => {
            return Stored::Unusable(format!("{} would not stat: {error}", path.display()));
        }
    };
    if !meta.is_file() {
        return Stored::Unusable(format!("{} is not a regular file", path.display()));
    }
    // SAFETY: `getuid` takes nothing and cannot fail.
    #[expect(unsafe_code, reason = "calling libc")]
    let me = unsafe { libc::getuid() };
    if meta.uid() != me {
        return Stored::Unusable(format!("{} belongs to someone else", path.display()));
    }
    if meta.mode() & 0o022 != 0 {
        return Stored::Unusable(format!(
            "{} can be written by others (mode {:o}); `solium pin set` writes it 0600",
            path.display(),
            meta.mode() & 0o777
        ));
    }
    if meta.len() > MAX_FILE {
        return Stored::Unusable(format!("{} is too long to be a PIN's hash", path.display()));
    }
    let mut text = String::new();
    if let Err(error) = (&file).take(MAX_FILE).read_to_string(&mut text) {
        return Stored::Unusable(format!("{} would not read: {error}", path.display()));
    }
    match parse(text.trim()) {
        Ok(()) => Stored::Set(text.trim().to_owned()),
        Err(why) => Stored::Unusable(format!("{} does not hold a PIN: {why}", path.display())),
    }
}

/// Whether `text` is an argon2id hash, version 0x13, whose parameters are
/// within the bounds above.
fn parse(text: &str) -> Result<(), String> {
    let hash = PasswordHash::new(text).map_err(|error| format!("not a PHC string ({error})"))?;
    if hash.algorithm != Algorithm::Argon2id.ident() {
        return Err(format!("{} is not argon2id", hash.algorithm));
    }
    if hash.version != Some(Version::V0x13.into()) {
        return Err("not argon2 version 19".to_owned());
    }
    if hash.salt.is_none() || hash.hash.is_none() {
        return Err("no salt or no hash".to_owned());
    }
    let params = Params::try_from(&hash).map_err(|error| format!("its parameters: {error}"))?;
    if params.m_cost() > MAX_M_COST_KIB
        || params.t_cost() > MAX_T_COST
        || params.p_cost() > MAX_P_COST
    {
        return Err(format!(
            "it asks for m={} KiB, t={}, p={}, more than a PIN's check may take",
            params.m_cost(),
            params.t_cost(),
            params.p_cost()
        ));
    }
    Ok(())
}

/// Check `pin` against the PIN file at `path`: [`Answer::Ok`] when it is the
/// PIN, [`Answer::Failed`] when it is not, and [`Answer::Unavailable`] when
/// there is no PIN to check it against, or the file cannot be used. What the
/// helper answers a PIN request with.
#[must_use]
pub fn check(path: &Path, pin: &Secret) -> Answer {
    match read(path) {
        Stored::None => Answer::Unavailable {
            reason: "no PIN is set".to_owned(),
        },
        Stored::Unusable(reason) => Answer::Unavailable { reason },
        Stored::Set(phc) => verify(&phc, pin),
    }
}

/// `pin` against the PHC string `phc`, under the parameters it names, in
/// constant time.
fn verify(phc: &str, pin: &Secret) -> Answer {
    let hash = match PasswordHash::new(phc) {
        Ok(hash) => hash,
        Err(error) => {
            return Answer::Unavailable {
                reason: format!("the PIN's hash does not parse: {error}"),
            };
        }
    };
    match Argon2::default().verify_password(pin.expose(), &hash) {
        Ok(()) => Answer::Ok,
        Err(password_hash::Error::Password) => Answer::Failed { delay_ms: 0 },
        Err(error) => Answer::Unavailable {
            reason: format!("the PIN could not be checked: {error}"),
        },
    }
}

/// The PHC string for `pin`, with a fresh random salt and the parameters
/// above: what `solium pin set` stores.
pub fn hash(pin: &Secret) -> io::Result<String> {
    hash_with(pin, M_COST_KIB, T_COST)
}

/// The same with `m_cost_kib` and `t_cost` of the caller's, which [`check`]
/// reads back from the string. For the tests, which would otherwise spend a
/// tenth of a second on every PIN they write.
pub fn hash_with(pin: &Secret, m_cost_kib: u32, t_cost: u32) -> io::Result<String> {
    let params = Params::new(m_cost_kib, t_cost, P_COST, None)
        .map_err(|error| io::Error::other(format!("argon2's parameters: {error}")))?;
    let mut salt = [0_u8; 16];
    random(&mut salt)?;
    let salt = SaltString::encode_b64(&salt)
        .map_err(|error| io::Error::other(format!("the salt: {error}")))?;
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password(pin.expose(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| io::Error::other(format!("argon2: {error}")))
}

/// Fill `bytes` from the kernel's random number generator.
fn random(bytes: &mut [u8]) -> io::Result<()> {
    let mut filled = 0;
    while let Some(rest) = bytes.get_mut(filled..).filter(|rest| !rest.is_empty()) {
        // SAFETY: a pointer to `rest.len()` writable bytes, alive across the
        // call.
        #[expect(unsafe_code, reason = "calling libc")]
        let got = unsafe { libc::getrandom(rest.as_mut_ptr().cast(), rest.len(), 0) };
        match usize::try_from(got) {
            Ok(got) => filled += got,
            Err(_) => {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
    }
    Ok(())
}

/// Whether `pin` has a PIN's shape: from `min` to `max` ASCII digits, and
/// nothing else. What decides, at a lock that allows a PIN, whether what was
/// typed is checked as the PIN or as the password.
#[must_use]
pub fn shaped(pin: &Secret, min: usize, max: usize) -> bool {
    let bytes = pin.expose();
    (min..=max).contains(&bytes.len()) && bytes.iter().all(u8::is_ascii_digit)
}

/// Write `contents` to `path` atomically, readable and writable by this user
/// alone: into a new file beside it, created 0600, synced, then renamed over
/// `path`, and the directory synced. The directory is made, 0700, when it is
/// not there.
pub fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    let Some(dir) = path.parent() else {
        return Err(io::Error::other(format!(
            "{} has no directory",
            path.display()
        )));
    };
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut tag = [0_u8; 8];
    random(&mut tag)?;
    let tag: String = tag.iter().map(|byte| format!("{byte:02x}")).collect();
    let temporary = dir.join(format!(".{name}.{tag}.new"));
    let written = (|| -> io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)?;
        // Whatever the umask made of the mode, it is 0600 now.
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)?;
        std::fs::File::open(dir)?.sync_all()
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A directory of the test's own, emptied.
    fn dir(test: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("solium-auth-pin-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a directory");
        dir
    }

    fn secret(text: &str) -> Secret {
        let mut secret = Secret::new();
        assert!(secret.push_str(text));
        secret
    }

    /// A hash cheap enough for a test: 64 KiB, one pass.
    fn cheap(pin: &str) -> String {
        hash_with(&secret(pin), 64, 1).expect("a hash")
    }

    /// **The file holds a hash, and never the PIN**: what `solium pin set`
    /// writes is an argon2id PHC string with the shipped parameters and a
    /// salt of its own, mode 0600, with no run of the PIN's digits anywhere
    /// in it; two hashes of one PIN differ.
    #[test]
    fn the_file_holds_a_hash_and_never_the_pin() {
        let dir = dir("hash");
        let path = dir.join("solium/pin");
        let pin = "73914625";
        let phc = hash(&secret(pin)).expect("a hash");
        write_private(&path, phc.as_bytes()).expect("written");
        let written = std::fs::read(&path).expect("read back");
        let text = String::from_utf8_lossy(&written);
        assert!(
            text.starts_with(&format!(
                "$argon2id$v=19$m={M_COST_KIB},t={T_COST},p={P_COST}$"
            )),
            "{text}"
        );
        for start in 0..pin.len() - 3 {
            let run = &pin[start..start + 4];
            assert!(
                !text.contains(run),
                "{run} of the PIN is in the file: {text}"
            );
        }
        let mode = std::fs::metadata(&path).expect("its mode").mode() & 0o777;
        assert_eq!(mode, 0o600, "mode {mode:o}");
        let dir_mode = std::fs::metadata(dir.join("solium"))
            .expect("its mode")
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700, "mode {dir_mode:o}");
        assert_ne!(
            phc,
            hash(&secret(pin)).expect("a hash"),
            "the same salt twice"
        );
        assert_eq!(read(&path), Stored::Set(phc));
        // Nothing left beside it.
        let names: Vec<_> = std::fs::read_dir(dir.join("solium"))
            .expect("listed")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(names, ["pin"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The right PIN is yes, any other no, and no file, or one that does not
    /// parse, unavailable: never a yes.
    #[test]
    fn a_pin_is_checked_against_the_file() {
        let dir = dir("check");
        let path = dir.join("pin");
        assert_eq!(
            check(&path, &secret("2580")),
            Answer::Unavailable {
                reason: "no PIN is set".to_owned()
            }
        );
        write_private(&path, cheap("2580").as_bytes()).expect("written");
        assert_eq!(check(&path, &secret("2580")), Answer::Ok);
        assert_eq!(
            check(&path, &secret("2581")),
            Answer::Failed { delay_ms: 0 }
        );
        assert_eq!(check(&path, &secret("")), Answer::Failed { delay_ms: 0 });
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A file that does not parse is no PIN**: text, another algorithm,
    /// parameters past the bounds, a file others can write, a symbolic
    /// link, a directory. Each is unusable, and a check against it is
    /// unavailable, whatever is typed.
    #[test]
    fn a_file_that_cannot_be_trusted_reads_as_no_pin() {
        let dir = dir("unusable");
        let path = dir.join("pin");
        let good = cheap("2580");
        let argon2i = good.replacen("$argon2id$", "$argon2i$", 1);
        let greedy = good.replacen("m=64,", "m=4194304,", 1);
        for text in [
            "2580",
            "",
            "$argon2id$v=19$m=64,t=1,p=1$c2FsdA",
            argon2i.as_str(),
            greedy.as_str(),
        ] {
            write_private(&path, text.as_bytes()).expect("written");
            assert!(
                matches!(read(&path), Stored::Unusable(_)),
                "{text:?}: {:?}",
                read(&path)
            );
            assert!(
                matches!(check(&path, &secret("2580")), Answer::Unavailable { .. }),
                "{text:?}"
            );
        }

        write_private(&path, good.as_bytes()).expect("written");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o622)).expect("chmod");
        assert!(matches!(read(&path), Stored::Unusable(why) if why.contains("written by others")));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        assert_eq!(check(&path, &secret("2580")), Answer::Ok);

        let link = dir.join("link");
        std::os::unix::fs::symlink(&path, &link).expect("a link");
        assert!(matches!(read(&link), Stored::Unusable(why) if why.contains("symbolic link")));
        assert!(matches!(read(&dir), Stored::Unusable(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pin_is_min_to_max_digits_and_nothing_else() {
        for (text, shaped_so) in [
            ("2580", true),
            ("25801234", true),
            ("258", false),
            ("258012345", false),
            ("25a0", false),
            ("２５８０", false),
            ("2580 ", false),
            ("", false),
        ] {
            assert_eq!(shaped(&secret(text), 4, 8), shaped_so, "{text:?}");
        }
        assert!(shaped(&secret("123456"), 6, 6));
        assert!(!shaped(&secret("12345"), 6, 6));
    }

    /// A write replaces the file whole, and a write that fails leaves the old
    /// one and nothing beside it.
    #[test]
    fn a_write_replaces_the_file_whole() {
        let dir = dir("atomic");
        let path = dir.join("state");
        write_private(&path, b"one").expect("written");
        write_private(&path, b"two").expect("written");
        assert_eq!(std::fs::read(&path).expect("read"), b"two");
        // A directory where the file would go: the rename fails.
        let blocked = dir.join("blocked");
        std::fs::create_dir_all(blocked.join("inside")).expect("a directory");
        assert!(write_private(&blocked, b"three").is_err());
        let names: Vec<_> = std::fs::read_dir(&dir)
            .expect("listed")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().all(|name| !name.ends_with(".new")),
            "a temporary file was left: {names:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
