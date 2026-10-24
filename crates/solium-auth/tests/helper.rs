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

use solium_auth::{
    Answer, Request, Secret,
    protocol::{VERSION, ask, read_hello, write_hello},
};

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

/// **The helper itself answers a PIN request** from the PIN file it is
/// named, with no PAM anywhere: the right PIN is yes and exits 0, another is
/// no, and a file that does not parse is unavailable.
#[test]
fn the_helper_checks_a_pin_against_the_pin_file() {
    let dir = std::env::temp_dir().join(format!("solium-auth-helper-pin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let file = dir.join("pin");
    let secret = |text: &str| {
        let mut secret = Secret::new();
        assert!(secret.push_str(text));
        secret
    };
    let phc = solium_auth::pin::hash_with(&secret("2580"), 64, 1).expect("a hash");
    solium_auth::pin::write_private(&file, phc.as_bytes()).expect("written");
    let checked = |pin: &str| {
        let (mut child, mut ours) = start();
        let answer = ask(&mut ours, Request::Pin { file: &file }, &secret(pin)).expect("an answer");
        assert_eq!(child.wait().expect("it ends").code(), Some(0));
        answer
    };
    assert_eq!(checked("2580"), Answer::Ok);
    assert_eq!(checked("2581"), Answer::Failed { delay_ms: 0 });
    solium_auth::pin::write_private(&file, b"not a hash").expect("written");
    assert!(
        matches!(checked("2580"), Answer::Unavailable { .. }),
        "a file that does not parse"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
