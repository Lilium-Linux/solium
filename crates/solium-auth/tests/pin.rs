//! The real `solium-auth` binary answering a PIN request, which never reaches
//! PAM. A test binary of its own: holding a `Secret` makes this process
//! non-dumpable, which `tests/helper.rs`'s `the_helper_is_not_dumpable`
//! compares the helper against.

// A test fails by panicking -- that is the mechanism, not a lapse.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::{
    os::{fd::OwnedFd, unix::net::UnixStream},
    process::{Child, Command, Stdio},
    time::Duration,
};

use solium_auth::{Answer, Request, Secret, protocol::ask};

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
    let phc = solium_auth::pin::hash(&secret("2580")).expect("a hash");
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
