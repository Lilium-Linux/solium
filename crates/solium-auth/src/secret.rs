//! [`Secret`]: a password on its way to PAM, held where it cannot leak.
//!
//! The bytes live in a page of their own, mapped for the secret alone, that
//! never moves, so no reallocation leaves a copy behind. The page is
//!
//! * locked out of swap where the limits allow it;
//! * left out of any core dump (`MADV_DONTDUMP`), and given to a forked child
//!   as zeroes (`MADV_WIPEONFORK`):
//!   `tests::the_page_is_left_out_of_a_core_dump_and_wiped_in_a_fork`;
//! * overwritten, every byte of it, when the secret is cleared or dropped.
//!
//! And while any secret in the process holds a byte, the process is not
//! dumpable (`PR_SET_DUMPABLE` 0): no core dump is written, and no process
//! of the same user can attach to it or read its memory through `/proc`.
//! When the last one is emptied, it is dumpable again as it was before
//! (`tests/dumpable.rs`).
//!
//! The type has no `Debug`, no `Display` and no `Clone`
//! (`tests::a_secret_cannot_be_formatted_or_copied`), so it cannot reach a log
//! line by accident, and the one way to read it is [`Secret::expose`].

use std::{
    io::Read,
    ptr::NonNull,
    sync::{
        Mutex, PoisonError,
        atomic::{Ordering, compiler_fence},
    },
};

/// The longest secret, in bytes: `PAM_MAX_RESP_SIZE`, the most a PAM
/// conversation answers with. `pam_unix` refuses a longer password, so a longer
/// one could never be the right one.
pub const MAX_LEN: usize = 512;

/// The page a secret lives in: an anonymous private mapping of its own, so
/// that locking it and advising the kernel about it never touches memory
/// belonging to anything else.
struct Page {
    at: NonNull<u8>,
    /// The mapping's length: one page, whatever the page size is.
    len: usize,
}

impl Page {
    fn new() -> Self {
        // SAFETY: `sysconf` reads a constant of the system.
        #[expect(unsafe_code, reason = "calling libc")]
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        let len = usize::try_from(page).unwrap_or(4096).max(MAX_LEN);
        // SAFETY: a fresh anonymous mapping, checked before use. Its memory is
        // zeroed by the kernel.
        #[expect(unsafe_code, reason = "calling libc")]
        let mapped = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        let Some(at) = NonNull::new(mapped.cast::<u8>()).filter(|_| mapped != libc::MAP_FAILED)
        else {
            // What a `Box` does when there is no memory for it.
            std::alloc::handle_alloc_error(std::alloc::Layout::new::<[u8; MAX_LEN]>());
        };
        // SAFETY: the range is the mapping just made. Each of these failing
        // (RLIMIT_MEMLOCK spent, a kernel without the advice) leaves the page
        // as it was, which is the "where it can" of the spec, not a reason to
        // refuse a password.
        #[expect(unsafe_code, reason = "calling libc")]
        unsafe {
            libc::mlock(at.as_ptr().cast(), len);
            libc::madvise(at.as_ptr().cast(), len, libc::MADV_DONTDUMP);
            libc::madvise(at.as_ptr().cast(), len, libc::MADV_WIPEONFORK);
        }
        Self { at, len }
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: the mapping is `len >= MAX_LEN` bytes, readable, alive as
        // long as `self`, and only reached through `self`.
        #[expect(unsafe_code, reason = "a slice over the mapping")]
        unsafe {
            std::slice::from_raw_parts(self.at.as_ptr(), MAX_LEN)
        }
    }

    fn bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: as `bytes`, and `&mut self` makes it the only reference.
        #[expect(unsafe_code, reason = "a slice over the mapping")]
        unsafe {
            std::slice::from_raw_parts_mut(self.at.as_ptr(), MAX_LEN)
        }
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        wipe(self.bytes_mut());
        // SAFETY: the mapping this page made, unmapped once, here.
        #[expect(unsafe_code, reason = "calling libc")]
        unsafe {
            libc::munlock(self.at.as_ptr().cast(), self.len);
            libc::munmap(self.at.as_ptr().cast(), self.len);
        }
    }
}

// SAFETY: the page is owned by its secret alone, as a `Box` would be: moving
// the secret to another thread moves the only way to it, and a shared
// reference only reads.
#[expect(unsafe_code, reason = "a raw pointer owned like a Box")]
unsafe impl Send for Page {}
// SAFETY: as above; nothing mutates through a shared reference.
#[expect(unsafe_code, reason = "a raw pointer owned like a Box")]
unsafe impl Sync for Page {}

/// How many secrets in this process hold a byte, and whether the process was
/// dumpable before the first of them did.
struct Held {
    count: usize,
    before: Option<libc::c_int>,
}

static HELD: Mutex<Held> = Mutex::new(Held {
    count: 0,
    before: None,
});

/// One more secret holds a byte: the first makes the process non-dumpable.
fn hold() {
    let mut held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
    if held.count == 0 {
        // SAFETY: `prctl` with these options reads or sets a flag of this
        // process and takes no pointers.
        #[expect(unsafe_code, reason = "calling libc")]
        unsafe {
            held.before = Some(libc::prctl(libc::PR_GET_DUMPABLE, 0, 0, 0, 0));
            libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
        }
    }
    held.count = held.count.saturating_add(1);
}

/// One fewer: after the last, the process is as dumpable as it was before
/// the first. A process that was not (the helper, which makes itself so)
/// stays not.
fn release() {
    let mut held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
    held.count = held.count.saturating_sub(1);
    if held.count == 0
        && let Some(before) = held.before.take()
        && before > 0
    {
        // SAFETY: as in `hold`.
        #[expect(unsafe_code, reason = "calling libc")]
        unsafe {
            libc::prctl(libc::PR_SET_DUMPABLE, before, 0, 0, 0);
        }
    }
}

/// A password, or a passcode, being typed or being checked.
///
/// Up to [`MAX_LEN`] bytes of UTF-8 in a fixed buffer. Never formatted, never
/// copied: it is handed on by moving it.
#[expect(
    missing_debug_implementations,
    reason = "a secret is never formatted: tests::a_secret_cannot_be_formatted_or_copied"
)]
pub struct Secret {
    page: Page,
    len: usize,
    /// Whether this secret counts in [`HELD`]: it holds a byte.
    holding: bool,
}

impl Default for Secret {
    fn default() -> Self {
        Self::new()
    }
}

impl Secret {
    /// An empty secret, in a page of its own (see the module documentation).
    #[must_use]
    pub fn new() -> Self {
        Self {
            page: Page::new(),
            len: 0,
            holding: false,
        }
    }

    /// Count in [`HELD`] exactly while holding a byte.
    fn settle_hold(&mut self) {
        let holding = self.len > 0;
        if holding != self.holding {
            if holding {
                hold();
            } else {
                release();
            }
            self.holding = holding;
        }
    }

    /// Append `text`. False, and the secret unchanged, when it would not fit
    /// in [`MAX_LEN`] bytes.
    pub fn push_str(&mut self, text: &str) -> bool {
        let end = self.len.saturating_add(text.len());
        let Some(room) = self.page.bytes_mut().get_mut(self.len..end) else {
            return false;
        };
        room.copy_from_slice(text.as_bytes());
        self.len = end;
        self.settle_hold();
        true
    }

    /// Take the last character off, overwriting its bytes. False when there
    /// was none.
    pub fn pop_char(&mut self) -> bool {
        let Some(last) = self
            .as_bytes_mut()
            .iter()
            .rposition(|byte| !is_continuation(*byte))
        else {
            return false;
        };
        let end = self.len;
        if let Some(gone) = self.page.bytes_mut().get_mut(last..end) {
            wipe(gone);
        }
        self.len = last;
        self.settle_hold();
        true
    }

    /// Empty it, overwriting every byte of the page.
    pub fn clear(&mut self) {
        wipe(self.page.bytes_mut());
        self.len = 0;
        self.settle_hold();
    }

    /// Its length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether nothing has been typed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// How many characters it holds: what a field shows as dots, the one
    /// thing about a secret anything else may know.
    #[must_use]
    pub fn chars(&self) -> usize {
        self.expose()
            .iter()
            .filter(|byte| !is_continuation(**byte))
            .count()
    }

    /// Whether every byte of its page is zero: it holds nothing, and nothing
    /// it held is left behind. Says nothing a length does not: what the
    /// compositor's lock asks of its buffers after a submit and an unlock.
    #[must_use]
    pub fn is_wiped(&self) -> bool {
        self.page.bytes().iter().all(|byte| *byte == 0)
    }

    /// The bytes themselves, for the one place that hands them on: the
    /// request to the helper, and the helper's answer to PAM.
    #[must_use]
    pub fn expose(&self) -> &[u8] {
        self.page.bytes().get(..self.len).unwrap_or_default()
    }

    fn as_bytes_mut(&mut self) -> &mut [u8] {
        let len = self.len;
        self.page.bytes_mut().get_mut(..len).unwrap_or_default()
    }

    /// Replace it with exactly `len` bytes read from `from`, straight into the
    /// page, so the bytes are never anywhere else in this process. Cleared when
    /// the read fails, or when `len` is more than [`MAX_LEN`].
    pub(crate) fn read_from(&mut self, from: &mut impl Read, len: usize) -> std::io::Result<()> {
        self.clear();
        let Some(into) = self.page.bytes_mut().get_mut(..len) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "longer than a secret can be",
            ));
        };
        if let Err(error) = from.read_exact(into) {
            self.clear();
            return Err(error);
        }
        self.len = len;
        self.settle_hold();
        Ok(())
    }

    /// The whole page, cleared bytes included, for the tests that it is clear.
    #[cfg(test)]
    pub(crate) fn page(&self) -> &[u8] {
        self.page.bytes()
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        // The page wipes and unmaps itself; this lets go of the hold.
        self.clear();
    }
}

/// Whether `byte` continues a UTF-8 character rather than starting one.
fn is_continuation(byte: u8) -> bool {
    byte & 0b1100_0000 == 0b1000_0000
}

/// Overwrite `bytes` with zeros, in writes the compiler may not drop as dead
/// stores to memory about to be freed.
fn wipe(bytes: &mut [u8]) {
    for byte in bytes.iter_mut() {
        // SAFETY: `byte` is a valid, aligned, exclusive reference.
        #[expect(unsafe_code, reason = "a volatile write")]
        unsafe {
            std::ptr::write_volatile(byte, 0);
        }
    }
    compiler_fence(Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use std::marker::PhantomData;

    use super::{MAX_LEN, Secret};

    /// Whether `T` implements a trait, decided at compile time: the inherent
    /// methods below exist only where their bound holds, and method lookup
    /// falls back to [`Absent`]'s defaults where it does not.
    struct Probe<T>(PhantomData<T>);

    trait Absent {
        fn debug(&self) -> bool {
            false
        }
        fn display(&self) -> bool {
            false
        }
        fn copies(&self) -> bool {
            false
        }
    }

    impl<T> Absent for Probe<T> {}

    impl<T: std::fmt::Debug> Probe<T> {
        fn debug(&self) -> bool {
            true
        }
    }

    impl<T: std::fmt::Display> Probe<T> {
        fn display(&self) -> bool {
            true
        }
    }

    impl<T: Clone> Probe<T> {
        fn copies(&self) -> bool {
            true
        }
    }

    /// No `Debug`, `Display` or `Clone`, so no `{:?}` can print one and no
    /// copy of the bytes can outlive the original. The probe is checked on a
    /// type that has all three, so it cannot pass by saying no to everything.
    #[test]
    fn a_secret_cannot_be_formatted_or_copied() {
        let control = Probe::<String>(PhantomData);
        assert!(control.debug() && control.display() && control.copies());
        let secret = Probe::<Secret>(PhantomData);
        assert!(!secret.debug(), "Secret implements Debug");
        assert!(!secret.display(), "Secret implements Display");
        assert!(!secret.copies(), "Secret implements Clone");
    }

    /// The `VmFlags` of the mapping `at` is in, from `/proc/self/smaps`.
    fn vm_flags(at: *const u8) -> Vec<String> {
        let smaps = std::fs::read_to_string("/proc/self/smaps").expect("smaps");
        let at = at as usize;
        let mut inside = false;
        for line in smaps.lines() {
            // A mapping's first line starts with its range, `start-end` in
            // hex; the lines about it after that start with a `Name:`.
            let range = line
                .split_whitespace()
                .next()
                .and_then(|range| range.split_once('-'))
                .and_then(|(start, end)| {
                    Some((
                        usize::from_str_radix(start, 16).ok()?,
                        usize::from_str_radix(end, 16).ok()?,
                    ))
                });
            if let Some((start, end)) = range {
                inside = start <= at && at < end;
            } else if inside && let Some(flags) = line.strip_prefix("VmFlags:") {
                return flags.split_whitespace().map(str::to_owned).collect();
            }
        }
        panic!("no mapping holds the page");
    }

    /// **Left out of a core dump, and zeroes in a fork**: the page's mapping
    /// carries `dd` (`MADV_DONTDUMP`) and `wf` (`MADV_WIPEONFORK`), so a
    /// crash's core file has none of it and a child forked while it holds a
    /// password has nothing of it.
    #[test]
    fn the_page_is_left_out_of_a_core_dump_and_wiped_in_a_fork() {
        let mut secret = Secret::new();
        assert!(secret.push_str("hunter2"));
        let flags = vm_flags(secret.page().as_ptr());
        assert!(flags.iter().any(|flag| flag == "dd"), "{flags:?}");
        assert!(flags.iter().any(|flag| flag == "wf"), "{flags:?}");
    }

    /// **Holding a byte, the process is not dumpable**: no core file, and no
    /// `/proc/<pid>/mem` for another process of the user. Whether it comes
    /// back once nothing is held is `tests/dumpable.rs`, a process of its
    /// own, as the tests here run beside each other.
    #[test]
    fn a_process_holding_a_secret_is_not_dumpable() {
        let mut secret = Secret::new();
        assert!(secret.push_str("x"));
        // SAFETY: reads a flag of this process.
        #[expect(unsafe_code, reason = "calling libc")]
        let dumpable = unsafe { libc::prctl(libc::PR_GET_DUMPABLE, 0, 0, 0, 0) };
        assert_eq!(dumpable, 0);
    }

    #[test]
    fn typing_and_taking_back_characters() {
        let mut secret = Secret::new();
        assert!(secret.is_empty());
        assert!(secret.push_str("пароль"));
        assert!(secret.push_str("42"));
        assert_eq!(secret.chars(), 8);
        assert_eq!(secret.len(), "пароль42".len());
        assert!(secret.pop_char());
        assert!(secret.pop_char());
        assert!(secret.pop_char());
        assert_eq!(secret.expose(), "парол".as_bytes());
        // What was taken back is gone from the page, not only from the length.
        assert!(secret.page()[secret.len()..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn a_secret_holds_at_most_max_len_bytes() {
        let mut secret = Secret::new();
        assert!(secret.push_str(&"a".repeat(MAX_LEN - 1)));
        assert!(!secret.push_str("я"), "a two-byte character past the end");
        assert_eq!(secret.len(), MAX_LEN - 1);
        assert!(secret.push_str("b"));
        assert!(!secret.push_str("c"));
        assert_eq!(secret.len(), MAX_LEN);
    }

    #[test]
    fn clearing_overwrites_the_whole_page() {
        let mut secret = Secret::new();
        assert!(secret.push_str("correct horse battery staple"));
        secret.clear();
        assert!(secret.is_empty());
        assert!(secret.page().iter().all(|byte| *byte == 0));
        assert!(!secret.pop_char());
    }

    #[test]
    fn a_secret_read_too_long_is_refused_and_left_clear() {
        let mut secret = Secret::new();
        assert!(secret.push_str("left over"));
        let bytes = vec![b'x'; MAX_LEN + 1];
        assert!(
            secret
                .read_from(&mut bytes.as_slice(), MAX_LEN + 1)
                .is_err()
        );
        assert!(secret.is_empty());
        assert!(secret.page().iter().all(|byte| *byte == 0));
        // And one cut short by the end of the stream.
        assert!(secret.read_from(&mut &b"abc"[..], 5).is_err());
        assert!(secret.page().iter().all(|byte| *byte == 0));
        assert!(secret.read_from(&mut &b"abcdef"[..], 4).is_ok());
        assert_eq!(secret.expose(), b"abcd");
    }
}
