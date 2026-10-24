//! PAM, as the helper uses it: one `pam_authenticate` against the `solium`
//! service, with a conversation that answers the one hidden prompt with the
//! secret and refuses anything else, and then `pam_acct_mgmt`, so an account
//! that is expired or locked is not let in by its right password
//! ([`verdict`]).
//!
//! Declared here by hand rather than generated from `security/pam_appl.h`, so
//! building needs only the runtime library and never pam-devel. The handful of
//! values used are Linux-PAM's ABI, the same in every distribution's
//! `libpam.so.0`; `tests::the_status_codes_are_libpams` checks them against
//! the library's own messages for them.

use std::{
    ffi::{CStr, CString, c_char, c_int, c_uint, c_void},
    ptr,
    sync::atomic::{AtomicU32, Ordering},
};

use solium_auth::{Answer, Authenticator, Secret};

/// The service: `/etc/pam.d/solium`.
const SERVICE: &CStr = c"solium";

const PAM_SUCCESS: c_int = 0;
const PAM_BUF_ERR: c_int = 5;
const PAM_PERM_DENIED: c_int = 6;
const PAM_AUTH_ERR: c_int = 7;
const PAM_CRED_INSUFFICIENT: c_int = 8;
const PAM_USER_UNKNOWN: c_int = 10;
const PAM_MAXTRIES: c_int = 11;
const PAM_NEW_AUTHTOK_REQD: c_int = 12;
const PAM_CONV_ERR: c_int = 19;

/// `pam_setcred`'s flag for credentials that already exist and are renewed,
/// as a lock screen's are (a Kerberos ticket, say).
const PAM_REFRESH_CRED: c_int = 0x0010;

/// The item that replaces PAM's own sleep after a failure with a function.
const PAM_FAIL_DELAY: c_int = 10;

const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;
const PAM_ERROR_MSG: c_int = 3;
const PAM_TEXT_INFO: c_int = 4;

/// The most messages one conversation call may carry.
const PAM_MAX_NUM_MSG: usize = 32;

/// `struct pam_message`.
#[allow(
    dead_code,
    reason = "C's layout, which PAM builds and the conversation reads through"
)]
#[repr(C)]
struct Message {
    style: c_int,
    text: *const c_char,
}

/// `struct pam_response`. PAM frees both the array and each `text`.
#[allow(dead_code, reason = "C's layout: `calloc` builds it and PAM reads it")]
#[repr(C)]
struct Response {
    text: *mut c_char,
    retcode: c_int,
}

/// `struct pam_conv`.
#[repr(C)]
struct Conversation {
    converse: Option<Converse>,
    appdata: *mut c_void,
}

type Converse =
    unsafe extern "C" fn(c_int, *mut *const Message, *mut *mut Response, *mut c_void) -> c_int;

type Delay = unsafe extern "C" fn(c_int, c_uint, *mut c_void);

/// `pam_handle_t`, which only PAM looks inside.
#[allow(dead_code, reason = "only ever behind a pointer PAM hands out")]
#[repr(C)]
struct Handle {
    _opaque: [u8; 0],
}

#[expect(unsafe_code, reason = "PAM is C; this is its ABI")]
mod ffi {
    use std::ffi::{c_char, c_int, c_void};

    use super::{Conversation, Handle};

    // `libpam.so.0` by its file name, the soname every distribution ships in
    // its runtime package. Plain `-lpam` would look for `libpam.so`, which only
    // the development package has.
    #[link(name = "libpam.so.0", kind = "dylib", modifiers = "+verbatim")]
    unsafe extern "C" {
        pub(super) fn pam_start(
            service: *const c_char,
            user: *const c_char,
            conversation: *const Conversation,
            handle: *mut *mut Handle,
        ) -> c_int;
        #[cfg(test)]
        pub(super) fn pam_start_confdir(
            service: *const c_char,
            user: *const c_char,
            conversation: *const Conversation,
            confdir: *const c_char,
            handle: *mut *mut Handle,
        ) -> c_int;
        pub(super) fn pam_set_item(handle: *mut Handle, item: c_int, value: *const c_void)
        -> c_int;
        pub(super) fn pam_authenticate(handle: *mut Handle, flags: c_int) -> c_int;
        pub(super) fn pam_acct_mgmt(handle: *mut Handle, flags: c_int) -> c_int;
        pub(super) fn pam_setcred(handle: *mut Handle, flags: c_int) -> c_int;
        pub(super) fn pam_end(handle: *mut Handle, status: c_int) -> c_int;
        pub(super) fn pam_strerror(handle: *mut Handle, status: c_int) -> *const c_char;
    }
}

/// The delay a module asked for after a failure, in microseconds: noted by
/// [`note_delay`] instead of slept through, and reported in the answer. One
/// transaction per process, so one value.
static DELAY_US: AtomicU32 = AtomicU32::new(0);

/// What the conversation knows, and what it found out.
struct Talk<'a> {
    secret: &'a [u8],
    /// Whether the one hidden prompt has had its answer.
    answered: bool,
    /// Why a prompt was refused, when one was.
    refused: Option<&'static str>,
}

/// The authenticator the helper runs: PAM.
pub(crate) struct Pam;

/// The two questions one transaction asks, in [`verdict`]'s order: PAM's
/// own, and a test's stand-in for it.
trait Transaction {
    /// `pam_authenticate`'s status.
    fn authenticate(&mut self) -> c_int;
    /// `pam_acct_mgmt`'s status.
    fn acct_mgmt(&mut self) -> c_int;
    /// Why the conversation refused a prompt, when it did.
    fn refused(&self) -> Option<&'static str>;
    /// The failure delay a module asked for, in microseconds.
    fn delay_us(&self) -> u32;
}

/// A live transaction, between `pam_start` and `pam_end`.
struct Live<'a> {
    handle: *mut Handle,
    talk: *const Talk<'a>,
}

impl Transaction for Live<'_> {
    fn authenticate(&mut self) -> c_int {
        // SAFETY: a handle `pam_start` returned and `pam_end` has not closed.
        #[expect(unsafe_code, reason = "calling PAM")]
        unsafe {
            ffi::pam_authenticate(self.handle, 0)
        }
    }

    fn acct_mgmt(&mut self) -> c_int {
        // SAFETY: as above.
        #[expect(unsafe_code, reason = "calling PAM")]
        unsafe {
            ffi::pam_acct_mgmt(self.handle, 0)
        }
    }

    fn refused(&self) -> Option<&'static str> {
        // SAFETY: the `Talk` the conversation writes through, alive for the
        // transaction; PAM is not inside a call while this reads it.
        #[expect(unsafe_code, reason = "reading what the conversation wrote")]
        unsafe {
            (*self.talk).refused
        }
    }

    fn delay_us(&self) -> u32 {
        DELAY_US.load(Ordering::Relaxed)
    }
}

/// What a transaction comes to: authenticated, and then, only on a yes, the
/// account checked. An account that is expired, locked or not allowed in now
/// fails on the right password; one whose password must be changed is let
/// in, since the lock cannot change it and the session is already the
/// user's.
fn verdict(transaction: &mut impl Transaction) -> (Answer, c_int) {
    let status = transaction.authenticate();
    let answer = judge(status, transaction.refused(), transaction.delay_us());
    if answer != Answer::Ok {
        return (answer, status);
    }
    let account = transaction.acct_mgmt();
    match account {
        PAM_SUCCESS | PAM_NEW_AUTHTOK_REQD => (Answer::Ok, PAM_SUCCESS),
        _ => (
            Answer::Failed {
                delay_ms: transaction.delay_us() / 1000,
            },
            account,
        ),
    }
}

impl Authenticator for Pam {
    fn authenticate(&mut self, user: &str, secret: &Secret) -> Answer {
        // The protocol refuses a user name with a NUL in it already.
        let Ok(user) = CString::new(user) else {
            return Answer::Failed { delay_ms: 0 };
        };
        let mut talk = Talk {
            secret: secret.expose(),
            answered: false,
            refused: None,
        };
        let conversation = Conversation {
            converse: Some(converse),
            appdata: (&raw mut talk).cast(),
        };
        let mut handle: *mut Handle = ptr::null_mut();
        // SAFETY: every pointer is to a live local or a NUL-terminated
        // string; `conversation` and `talk` outlive the transaction, which
        // `pam_end` closes before this returns.
        #[expect(unsafe_code, reason = "calling PAM")]
        unsafe {
            #[cfg(test)]
            let started = match tests::CONFDIR.with(|dir| dir.borrow().clone()) {
                Some(dir) => ffi::pam_start_confdir(
                    SERVICE.as_ptr(),
                    user.as_ptr(),
                    &raw const conversation,
                    dir.as_ptr(),
                    &raw mut handle,
                ),
                None => ffi::pam_start(
                    SERVICE.as_ptr(),
                    user.as_ptr(),
                    &raw const conversation,
                    &raw mut handle,
                ),
            };
            #[cfg(not(test))]
            let started = ffi::pam_start(
                SERVICE.as_ptr(),
                user.as_ptr(),
                &raw const conversation,
                &raw mut handle,
            );
            if started != PAM_SUCCESS || handle.is_null() {
                return Answer::Unavailable {
                    reason: format!("PAM would not start: {}", message(started)),
                };
            }
            let delay: Delay = note_delay;
            ffi::pam_set_item(handle, PAM_FAIL_DELAY, delay as *const c_void);
            let mut live = Live {
                handle,
                talk: &raw const talk,
            };
            let (answer, status) = verdict(&mut live);
            if answer == Answer::Ok {
                // What it says is not the question asked: the password was
                // right, and a ticket that could not be renewed does not make
                // it wrong.
                ffi::pam_setcred(handle, PAM_REFRESH_CRED);
            }
            ffi::pam_end(handle, status);
            answer
        }
    }
}

/// The answer `pam_authenticate`'s `status` gives. The secret has reached
/// PAM's modules whatever the status, so anything but a yes counts as an
/// attempt: a known failure is a no, and a refused prompt or a status that
/// is neither yes nor a known no is [`Answer::Refused`], which says why and
/// counts as a no.
fn judge(status: c_int, refused: Option<&'static str>, delay_us: u32) -> Answer {
    match (status, refused) {
        (PAM_SUCCESS, _) => Answer::Ok,
        (_, Some(refused)) => Answer::Refused {
            reason: refused.to_owned(),
        },
        (
            PAM_AUTH_ERR
            | PAM_USER_UNKNOWN
            | PAM_MAXTRIES
            | PAM_CRED_INSUFFICIENT
            | PAM_PERM_DENIED,
            None,
        ) => Answer::Failed {
            delay_ms: delay_us / 1000,
        },
        (status, None) => Answer::Refused {
            reason: message(status),
        },
    }
}

/// PAM's own words for `status`.
fn message(status: c_int) -> String {
    // SAFETY: Linux-PAM's `pam_strerror` ignores the handle and returns a
    // static string, never null.
    #[expect(unsafe_code, reason = "calling PAM")]
    let text = unsafe { ffi::pam_strerror(ptr::null_mut(), status) };
    if text.is_null() {
        return format!("PAM error {status}");
    }
    // SAFETY: a NUL-terminated static string, as above.
    #[expect(unsafe_code, reason = "reading a C string")]
    let text = unsafe { CStr::from_ptr(text) };
    text.to_string_lossy().into_owned()
}

/// The failure delay, noted rather than slept.
extern "C" fn note_delay(_status: c_int, delay_us: c_uint, _appdata: *mut c_void) {
    DELAY_US.store(delay_us, Ordering::Relaxed);
}

/// The conversation: the secret for the first hidden prompt, nothing for an
/// information or error message, and a refusal for anything else -- a second
/// hidden prompt, or one shown in the clear such as a one-time code -- since
/// the lock has one password and nothing else to give.
///
/// The replies are allocated with `malloc` because PAM frees them; on a
/// refusal everything allocated so far is overwritten and freed here.
#[expect(unsafe_code, reason = "a C callback")]
unsafe extern "C" fn converse(
    count: c_int,
    messages: *mut *const Message,
    replies: *mut *mut Response,
    appdata: *mut c_void,
) -> c_int {
    let Ok(count) = usize::try_from(count) else {
        return PAM_CONV_ERR;
    };
    if count == 0
        || count > PAM_MAX_NUM_MSG
        || messages.is_null()
        || replies.is_null()
        || appdata.is_null()
    {
        return PAM_CONV_ERR;
    }
    // SAFETY: `appdata` is the `Talk` that `authenticate` gave `pam_start`,
    // alive for the whole transaction, and nothing else holds it meanwhile.
    let talk = unsafe { &mut *appdata.cast::<Talk<'_>>() };
    // SAFETY: a plain allocation of `count` zeroed responses.
    let answers = unsafe { libc::calloc(count, size_of::<Response>()) }.cast::<Response>();
    if answers.is_null() {
        return PAM_BUF_ERR;
    }
    for index in 0..count {
        // SAFETY: PAM passes `count` message pointers.
        let message = unsafe { *messages.add(index) };
        // SAFETY: a message pointer PAM passed, checked for null.
        let style = (!message.is_null()).then(|| unsafe { (*message).style });
        let refusal = match style {
            Some(PAM_PROMPT_ECHO_OFF) if !talk.answered => {
                let copy = c_copy(talk.secret);
                if copy.is_null() {
                    // SAFETY: the `count` responses allocated above.
                    unsafe { drop_answers(answers, count) };
                    return PAM_BUF_ERR;
                }
                talk.answered = true;
                // SAFETY: `index` is below `count`.
                unsafe { (*answers.add(index)).text = copy };
                None
            }
            Some(PAM_ERROR_MSG | PAM_TEXT_INFO) => None,
            Some(PAM_PROMPT_ECHO_OFF) => Some("PAM asked for a second password"),
            Some(PAM_PROMPT_ECHO_ON) => {
                Some("PAM asked for something typed in the clear, such as a one-time code")
            }
            _ => Some("PAM asked something the lock screen cannot answer"),
        };
        if let Some(refusal) = refusal {
            talk.refused = Some(refusal);
            // SAFETY: the `count` responses allocated above.
            unsafe { drop_answers(answers, count) };
            return PAM_CONV_ERR;
        }
    }
    // SAFETY: `replies` is PAM's, checked for null.
    unsafe { *replies = answers };
    PAM_SUCCESS
}

/// `bytes` as a NUL-terminated string from `malloc`, which PAM frees. Null
/// when there is no memory.
fn c_copy(bytes: &[u8]) -> *mut c_char {
    // SAFETY: a plain allocation, then a copy into it of exactly the bytes
    // given, and the NUL after them.
    #[expect(unsafe_code, reason = "an allocation PAM frees")]
    unsafe {
        let copy = libc::malloc(bytes.len().saturating_add(1)).cast::<u8>();
        if copy.is_null() {
            return ptr::null_mut();
        }
        ptr::copy_nonoverlapping(bytes.as_ptr(), copy, bytes.len());
        copy.add(bytes.len()).write(0);
        copy.cast()
    }
}

/// Overwrite and free the replies built so far, and the array.
///
/// # Safety
///
/// `answers` is a `calloc` of `count` responses, each `text` null or from
/// [`c_copy`].
#[expect(unsafe_code, reason = "freeing what C allocated")]
unsafe fn drop_answers(answers: *mut Response, count: usize) {
    for index in 0..count {
        // SAFETY: as the function's contract says.
        unsafe {
            let text = (*answers.add(index)).text;
            if !text.is_null() {
                let len = libc::strlen(text);
                for at in 0..len {
                    text.add(at).write_volatile(0);
                }
                libc::free(text.cast());
            }
        }
    }
    // SAFETY: as the function's contract says.
    unsafe { libc::free(answers.cast()) };
}

#[cfg(test)]
mod tests {
    //! The conversation is tested by calling it as PAM does, with messages of
    //! the test's own. A PAM transaction is started only against a service
    //! of the test's own, in a directory of its own, whose stacks are
    //! `pam_permit` and `pam_deny`: no test checks a real password of a real
    //! account.

    use std::cell::RefCell;

    use super::*;

    thread_local! {
        /// Where `pam_start_confdir` reads the `solium` service, for a test
        /// that runs a transaction against stacks of its own.
        pub(super) static CONFDIR: RefCell<Option<CString>> = const { RefCell::new(None) };
    }

    /// `pam_acct_mgmt`'s answer for an account past its expiry date.
    const PAM_ACCT_EXPIRED: c_int = 13;

    /// The values declared by hand are the library's: its message for each
    /// is the one Linux-PAM gives that status.
    #[test]
    fn the_status_codes_are_libpams() {
        assert_eq!(message(PAM_SUCCESS), "Success");
        assert_eq!(message(PAM_PERM_DENIED), "Permission denied");
        assert_eq!(message(PAM_AUTH_ERR), "Authentication failure");
        assert_eq!(
            message(PAM_CRED_INSUFFICIENT),
            "Insufficient credentials to access authentication data"
        );
        assert_eq!(
            message(PAM_USER_UNKNOWN),
            "User not known to the underlying authentication module"
        );
        assert_eq!(
            message(PAM_MAXTRIES),
            "Have exhausted maximum number of retries for service"
        );
        assert_eq!(message(PAM_CONV_ERR), "Conversation error");
        assert_eq!(
            message(PAM_NEW_AUTHTOK_REQD),
            "Authentication token is no longer valid; new one required"
        );
        assert_eq!(message(PAM_ACCT_EXPIRED), "User account has expired");
    }

    /// One call of the conversation with `styles`: its status, the replies'
    /// texts (null as `None`), and what it refused.
    fn converse_with(
        styles: &[c_int],
        secret: &[u8],
    ) -> (c_int, Vec<Option<Vec<u8>>>, Option<&'static str>) {
        let texts: Vec<CString> = styles
            .iter()
            .map(|style| CString::new(format!("prompt {style}")).expect("a prompt"))
            .collect();
        let messages: Vec<Message> = styles
            .iter()
            .zip(&texts)
            .map(|(style, text)| Message {
                style: *style,
                text: text.as_ptr(),
            })
            .collect();
        let mut pointers: Vec<*const Message> = messages.iter().map(ptr::from_ref).collect();
        let mut talk = Talk {
            secret,
            answered: false,
            refused: None,
        };
        let mut answers: *mut Response = ptr::null_mut();
        let count = c_int::try_from(styles.len()).expect("a count");
        #[expect(unsafe_code, reason = "calling the conversation as PAM does")]
        let status = unsafe {
            converse(
                count,
                pointers.as_mut_ptr(),
                &raw mut answers,
                (&raw mut talk).cast(),
            )
        };
        let mut replies = Vec::new();
        if status == PAM_SUCCESS {
            assert!(!answers.is_null());
            #[expect(unsafe_code, reason = "reading and freeing the replies, as PAM does")]
            unsafe {
                for index in 0..styles.len() {
                    let text = (*answers.add(index)).text;
                    replies
                        .push((!text.is_null()).then(|| CStr::from_ptr(text).to_bytes().to_vec()));
                }
                drop_answers(answers, styles.len());
            }
        } else {
            assert!(answers.is_null(), "a refusal hands nothing back");
        }
        (status, replies, talk.refused)
    }

    #[test]
    fn the_hidden_prompt_gets_the_secret_and_a_message_gets_nothing() {
        let (status, replies, refused) =
            converse_with(&[PAM_TEXT_INFO, PAM_PROMPT_ECHO_OFF], b"correct horse");
        assert_eq!(status, PAM_SUCCESS);
        assert_eq!(replies, vec![None, Some(b"correct horse".to_vec())]);
        assert_eq!(refused, None);
    }

    #[test]
    fn a_second_hidden_prompt_is_refused() {
        let (status, _, refused) =
            converse_with(&[PAM_PROMPT_ECHO_OFF, PAM_PROMPT_ECHO_OFF], b"pw");
        assert_eq!(status, PAM_CONV_ERR);
        assert_eq!(refused, Some("PAM asked for a second password"));
    }

    #[test]
    fn a_prompt_in_the_clear_is_refused() {
        let (status, _, refused) = converse_with(&[PAM_PROMPT_ECHO_ON], b"pw");
        assert_eq!(status, PAM_CONV_ERR);
        assert!(refused.is_some_and(|why| why.contains("in the clear")));
        // And a style no lock screen can answer.
        let (status, _, refused) = converse_with(&[7], b"pw");
        assert_eq!(status, PAM_CONV_ERR);
        assert!(refused.is_some());
    }

    #[test]
    fn what_pam_says_becomes_the_answer() {
        assert_eq!(judge(PAM_SUCCESS, None, 0), Answer::Ok);
        // A module that succeeded is the answer, whatever another refused.
        assert_eq!(judge(PAM_SUCCESS, Some("refused"), 0), Answer::Ok);
        assert_eq!(
            judge(PAM_AUTH_ERR, None, 2_000_000),
            Answer::Failed { delay_ms: 2000 }
        );
        assert_eq!(
            judge(PAM_USER_UNKNOWN, None, 0),
            Answer::Failed { delay_ms: 0 }
        );
        // A refused prompt, or a status that is neither yes nor a known no,
        // is refused: the password reached the modules, so it counts.
        assert_eq!(
            judge(PAM_AUTH_ERR, Some("PAM asked for a second password"), 0),
            Answer::Refused {
                reason: "PAM asked for a second password".into()
            }
        );
        assert_eq!(
            judge(PAM_CONV_ERR, None, 0),
            Answer::Refused {
                reason: "Conversation error".into()
            }
        );
    }

    /// A transaction of the test's own: the statuses its two calls return,
    /// and which of them were made.
    struct Scripted {
        auth: c_int,
        account: c_int,
        asked_account: bool,
    }

    impl Transaction for Scripted {
        fn authenticate(&mut self) -> c_int {
            self.auth
        }
        fn acct_mgmt(&mut self) -> c_int {
            self.asked_account = true;
            self.account
        }
        fn refused(&self) -> Option<&'static str> {
            None
        }
        fn delay_us(&self) -> u32 {
            0
        }
    }

    /// **The account is checked after the password**: a right password on
    /// an account `pam_acct_mgmt` refuses (expired, locked, outside its
    /// hours) fails, one whose password must be changed is let in, and a
    /// wrong password never asks about the account at all.
    #[test]
    fn a_right_password_on_a_refused_account_fails() {
        let run = |auth, account| {
            let mut transaction = Scripted {
                auth,
                account,
                asked_account: false,
            };
            let (answer, _) = verdict(&mut transaction);
            (answer, transaction.asked_account)
        };
        assert_eq!(run(PAM_SUCCESS, PAM_SUCCESS), (Answer::Ok, true));
        assert_eq!(
            run(PAM_SUCCESS, PAM_ACCT_EXPIRED),
            (Answer::Failed { delay_ms: 0 }, true)
        );
        assert_eq!(
            run(PAM_SUCCESS, PAM_PERM_DENIED),
            (Answer::Failed { delay_ms: 0 }, true)
        );
        assert_eq!(run(PAM_SUCCESS, PAM_NEW_AUTHTOK_REQD), (Answer::Ok, true));
        assert_eq!(
            run(PAM_AUTH_ERR, PAM_SUCCESS),
            (Answer::Failed { delay_ms: 0 }, false)
        );
    }

    /// One check by the real `Pam`, against a `solium` service whose file
    /// is `stacks`, in a directory of the test's own.
    fn checked_against(name: &str, stacks: &str) -> Answer {
        let dir =
            std::env::temp_dir().join(format!("solium-auth-pam-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory");
        std::fs::write(dir.join("solium"), stacks).expect("the service");
        let path = CString::new(dir.to_string_lossy().into_owned()).expect("a path");
        CONFDIR.with(|confdir| *confdir.borrow_mut() = Some(path));
        let mut secret = Secret::new();
        assert!(secret.push_str("not checked by pam_permit"));
        let answer = Pam.authenticate("nobody", &secret);
        CONFDIR.with(|confdir| *confdir.borrow_mut() = None);
        let _ = std::fs::remove_dir_all(&dir);
        answer
    }

    /// **libpam's own `pam_acct_mgmt` decides after a yes**: a stack whose
    /// account says no fails a password its auth says yes to, and so does a
    /// service with no account stack at all, which is why the shipped
    /// `dev/pam/solium` has one.
    #[test]
    fn the_account_stack_is_asked_after_a_yes() {
        let permit = "auth required pam_permit.so\n";
        assert_eq!(
            checked_against(
                "permit",
                &format!("{permit}account required pam_permit.so\n")
            ),
            Answer::Ok
        );
        assert_eq!(
            checked_against("deny", &format!("{permit}account required pam_deny.so\n")),
            Answer::Failed { delay_ms: 0 }
        );
        assert_eq!(
            checked_against("none", permit),
            Answer::Failed { delay_ms: 0 },
            "a service with no account stack let the password in"
        );
        assert!(
            matches!(
                checked_against(
                    "auth-deny",
                    "auth required pam_deny.so\naccount required pam_permit.so\n"
                ),
                Answer::Failed { .. }
            ),
            "a no from auth"
        );
        let shipped = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../dev/pam/solium"));
        for stack in ["auth", "account"] {
            assert!(
                shipped
                    .lines()
                    .any(|line| line.split_whitespace().next() == Some(stack)),
                "dev/pam/solium has no {stack} stack"
            );
        }
    }
}
