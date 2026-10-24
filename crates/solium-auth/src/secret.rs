//! [`Secret`]: a password on its way to PAM, held where it cannot leak.
//!
//! The bytes live in one page of their own that never moves, so no
//! reallocation leaves a copy behind; the page is locked out of swap where the
//! limits allow it, and every byte of it is overwritten when the secret is
//! cleared or dropped. The type has no `Debug`, no `Display` and no `Clone`
//! (`tests::a_secret_cannot_be_formatted_or_copied`), so it cannot reach a log
//! line by accident, and the one way to read it is [`Secret::expose`].

use std::{
    io::Read,
    sync::atomic::{Ordering, compiler_fence},
};

/// The longest secret, in bytes: `PAM_MAX_RESP_SIZE`, the most a PAM
/// conversation answers with. `pam_unix` refuses a longer password, so a longer
/// one could never be the right one.
pub const MAX_LEN: usize = 512;

/// The page a secret lives in, aligned so that it is a page of its own: locking
/// and unlocking it never touches memory belonging to anything else.
#[repr(C, align(4096))]
struct Page([u8; MAX_LEN]);

/// A password, or a passcode, being typed or being checked.
///
/// Up to [`MAX_LEN`] bytes of UTF-8 in a fixed buffer. Never formatted, never
/// copied: it is handed on by moving it.
#[expect(
    missing_debug_implementations,
    reason = "a secret is never formatted: tests::a_secret_cannot_be_formatted_or_copied"
)]
pub struct Secret {
    page: Box<Page>,
    len: usize,
}

impl Default for Secret {
    fn default() -> Self {
        Self::new()
    }
}

impl Secret {
    /// An empty secret, its page locked in memory where the limits allow it.
    #[must_use]
    pub fn new() -> Self {
        let page = Box::new(Page([0; MAX_LEN]));
        // SAFETY: the range is the page this secret owns, alive for the call.
        // A failure (RLIMIT_MEMLOCK spent) leaves it unlocked, which is the
        // "where it can" of the spec, not a reason to refuse a password.
        #[expect(unsafe_code, reason = "calling libc")]
        unsafe {
            libc::mlock(page.0.as_ptr().cast(), MAX_LEN);
        }
        Self { page, len: 0 }
    }

    /// Append `text`. False, and the secret unchanged, when it would not fit
    /// in [`MAX_LEN`] bytes.
    pub fn push_str(&mut self, text: &str) -> bool {
        let end = self.len.saturating_add(text.len());
        let Some(room) = self.page.0.get_mut(self.len..end) else {
            return false;
        };
        room.copy_from_slice(text.as_bytes());
        self.len = end;
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
        if let Some(gone) = self.page.0.get_mut(last..end) {
            wipe(gone);
        }
        self.len = last;
        true
    }

    /// Empty it, overwriting every byte of the page.
    pub fn clear(&mut self) {
        wipe(&mut self.page.0);
        self.len = 0;
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
        self.page.0.iter().all(|byte| *byte == 0)
    }

    /// The bytes themselves, for the one place that hands them on: the
    /// request to the helper, and the helper's answer to PAM.
    #[must_use]
    pub fn expose(&self) -> &[u8] {
        self.page.0.get(..self.len).unwrap_or_default()
    }

    fn as_bytes_mut(&mut self) -> &mut [u8] {
        self.page.0.get_mut(..self.len).unwrap_or_default()
    }

    /// Replace it with exactly `len` bytes read from `from`, straight into the
    /// page, so the bytes are never anywhere else in this process. Cleared when
    /// the read fails, or when `len` is more than [`MAX_LEN`].
    pub(crate) fn read_from(&mut self, from: &mut impl Read, len: usize) -> std::io::Result<()> {
        self.clear();
        let Some(into) = self.page.0.get_mut(..len) else {
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
        Ok(())
    }

    /// The whole page, cleared bytes included, for the tests that it is clear.
    #[cfg(test)]
    pub(crate) fn page(&self) -> &[u8] {
        &self.page.0
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.clear();
        // SAFETY: the page this secret owns, still alive until this returns.
        #[expect(unsafe_code, reason = "calling libc")]
        unsafe {
            libc::munlock(self.page.0.as_ptr().cast(), MAX_LEN);
        }
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
