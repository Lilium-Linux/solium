//! A process holding a secret is not dumpable, and is dumpable again once it
//! holds none (`solium_auth::Secret`). A test binary of its own, with this one
//! test, so no other secret in the process holds the flag down meanwhile.

// A test fails by panicking -- that is the mechanism, not a lapse.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use solium_auth::Secret;

fn dumpable() -> libc::c_int {
    // SAFETY: reads a flag of this process.
    #[expect(unsafe_code, reason = "calling libc")]
    unsafe {
        libc::prctl(libc::PR_GET_DUMPABLE, 0, 0, 0, 0)
    }
}

/// **Not dumpable while a typed or submitted secret is held**, from its first
/// byte to its last being taken back or cleared, across two secrets at once
/// and one moved to another thread, as a submitted password is; dumpable as
/// before once none holds a byte.
#[test]
fn not_dumpable_while_a_secret_holds_a_byte_and_dumpable_again_after() {
    assert_eq!(dumpable(), 1, "the premise: a test binary is dumpable");
    let mut typed = Secret::new();
    assert_eq!(dumpable(), 1, "an empty secret holds nothing");
    assert!(typed.push_str("a"));
    assert_eq!(dumpable(), 0, "typed");
    let mut submitted = Secret::new();
    assert!(submitted.push_str("hunter2"));
    assert!(typed.pop_char());
    assert_eq!(dumpable(), 0, "the submitted copy still holds");
    std::thread::spawn(move || {
        assert_eq!(dumpable(), 0, "on the checking thread");
        drop(submitted);
    })
    .join()
    .expect("the thread");
    assert_eq!(dumpable(), 1, "nothing is held");
    assert!(typed.push_str("b"));
    assert_eq!(dumpable(), 0);
    typed.clear();
    assert_eq!(dumpable(), 1, "cleared");
}
