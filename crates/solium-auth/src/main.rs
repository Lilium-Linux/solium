//! The `solium-auth` helper: one check, then it ends (#281).
//!
//! Solium starts it for each password or PIN its lock screen checks, and for
//! the password `solium pin set` and `solium pin clear` ask for, with a socket as
//! its standard input, standard output and error on `/dev/null`, an empty
//! environment, and nothing else open. It hardens itself before it reads a
//! byte ([`harden`]), answers the handshake and the one request in
//! `solium_auth::protocol`, and exits. Solium cancels it with `SIGKILL`, which
//! needs nothing from it.
//!
//! Run by hand, it says what it is and exits: there is no socket to answer on.

// A test fails by panicking -- that is the mechanism, not a lapse.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod pam;

use std::{
    io::Write as _,
    os::{
        fd::{FromRawFd as _, OwnedFd},
        unix::net::UnixStream,
    },
    process::ExitCode,
};

/// Started some other way than by Solium: standard input is not a socket.
const NOT_STARTED_BY_SOLIUM: u8 = 2;
/// The exchange ended without an answer: the compositor went away, or speaks
/// another version.
const NO_ANSWER: u8 = 3;
/// The process could not be made non-dumpable, so it does not read a secret.
const STILL_DUMPABLE: u8 = 4;

fn main() -> ExitCode {
    if harden().is_err() {
        return ExitCode::from(STILL_DUMPABLE);
    }
    let Some(mut conn) = connection() else {
        let _ = writeln!(
            std::io::stderr(),
            "solium-auth checks a password for Solium's lock screen. Solium starts it, \
             with a socket to answer on; it is not run by hand."
        );
        return ExitCode::from(NOT_STARTED_BY_SOLIUM);
    };
    match solium_auth::protocol::serve(&mut conn, &mut pam::Pam) {
        Ok(_) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(NO_ANSWER),
    }
}

/// Before any secret is read:
///
/// - every descriptor above stdio is closed, whatever the process that started
///   it left open;
/// - the process is made non-dumpable, so no core dump is written and no
///   process of the same user can attach to it or read its memory through
///   `/proc` (`tests/helper.rs`, `the_helper_is_not_dumpable`). If that fails,
///   it refuses to go on;
/// - core files are switched off as well, for a kernel that would write one
///   anyway;
/// - all of its memory is locked out of swap when the memory-lock limit is
///   unlimited. Under a limit, locking everything to come would make PAM's own
///   allocations fail once the limit is reached, so only the secret's page is
///   locked (`Secret::new`).
fn harden() -> std::io::Result<()> {
    // SAFETY: none of these take pointers to anything but locals, and each
    // changes only this process.
    #[expect(unsafe_code, reason = "calling libc")]
    unsafe {
        // Ignored where the call does not exist (before Linux 5.9): Solium
        // closes them as it starts this process too.
        libc::close_range(3, libc::c_uint::MAX, 0);
        if libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let none = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        libc::setrlimit(libc::RLIMIT_CORE, &raw const none);
        let mut memory = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if libc::getrlimit(libc::RLIMIT_MEMLOCK, &raw mut memory) == 0
            && memory.rlim_cur == libc::RLIM_INFINITY
        {
            libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE);
        }
    }
    Ok(())
}

/// Standard input, when it is the socket Solium hands over.
fn connection() -> Option<UnixStream> {
    // SAFETY: `stat` is a plain C struct, which all zeroes initialises, and
    // `fstat` only writes into it.
    #[expect(unsafe_code, reason = "calling libc")]
    let socket = unsafe {
        let mut stat: libc::stat = std::mem::zeroed();
        libc::fstat(0, &raw mut stat) == 0 && stat.st_mode & libc::S_IFMT == libc::S_IFSOCK
    };
    if !socket {
        return None;
    }
    // SAFETY: descriptor 0 is open (fstat just answered for it) and nothing
    // else in this process owns it.
    #[expect(unsafe_code, reason = "taking ownership of a raw descriptor")]
    let fd = unsafe { OwnedFd::from_raw_fd(0) };
    Some(UnixStream::from(fd))
}
