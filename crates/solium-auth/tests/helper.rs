//! The real `solium-auth` binary, as Solium starts it: a socket as its
//! standard input. No test here sends a request, so none reaches PAM; what is
//! checked is everything the helper does before it would.

// A test fails by panicking -- that is the mechanism, not a lapse.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::{
    io::Read as _,
    os::{fd::OwnedFd, unix::fs::MetadataExt as _, unix::net::UnixStream},
    process::{Child, Command, Stdio},
    time::Duration,
};

use solium_auth::protocol::{VERSION, read_hello, write_hello};

const HELPER: &str = env!("CARGO_BIN_EXE_solium-auth");

/// The helper, started on one end of a socketpair; the other end.
fn start() -> (Child, UnixStream) {
    let (ours, theirs) = UnixStream::pair().expect("a socketpair");
    let child = Command::new(HELPER)
        .env_clear()
        .stdin(Stdio::from(OwnedFd::from(theirs)))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the helper starts");
    ours.set_read_timeout(Some(Duration::from_secs(20)))
        .expect("a timeout");
    (child, ours)
}

/// Run by hand there is no socket: it says what it is and exits 2.
#[test]
fn run_by_hand_it_says_what_it_is() {
    let out = Command::new(HELPER)
        .env_clear()
        .stdin(Stdio::null())
        .output()
        .expect("the helper runs");
    assert_eq!(out.status.code(), Some(2));
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("not run by hand"), "{said}");
}

/// A compositor of another version is told the helper's, and the helper ends
/// without reading anything more.
#[test]
fn a_compositor_of_another_version_is_answered_and_left() {
    let (mut child, mut ours) = start();
    write_hello(&mut ours, VERSION + 1).expect("written");
    assert_eq!(read_hello(&mut ours).expect("its hello"), VERSION);
    let mut rest = Vec::new();
    ours.read_to_end(&mut rest).expect("read to the end");
    assert!(rest.is_empty(), "{rest:?}");
    assert_eq!(child.wait().expect("it ends").code(), Some(3));
}

/// Once it has answered the handshake it is non-dumpable: the kernel gives
/// its `/proc` files to root, so no process of the user's can read its memory
/// or attach to it. Checked against this process, which is dumpable, so the
/// test cannot pass by every file being root's. Meaningless as root, where
/// both are.
#[test]
fn the_helper_is_not_dumpable() {
    // SAFETY: `getuid` takes nothing and cannot fail.
    #[expect(unsafe_code, reason = "calling libc")]
    let me = unsafe { libc::getuid() };
    if me == 0 {
        return;
    }
    let mine = std::fs::metadata("/proc/self/environ").expect("this process's environ");
    assert_eq!(
        mine.uid(),
        me,
        "a dumpable process's /proc files are its user's"
    );

    let (mut child, mut ours) = start();
    write_hello(&mut ours, VERSION).expect("written");
    assert_eq!(read_hello(&mut ours).expect("its hello"), VERSION);
    let theirs =
        std::fs::metadata(format!("/proc/{}/environ", child.id())).expect("the helper's environ");
    assert_ne!(theirs.uid(), me, "the helper is still dumpable");

    // No request: the helper reads the end of the stream and stops there.
    drop(ours);
    assert_eq!(child.wait().expect("it ends").code(), Some(3));
}
