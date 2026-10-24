//! What the compositor and the helper say to each other, over the socket the
//! compositor hands the helper as its standard input.
//!
//! 1. **The handshake.** Each side sends [`MAGIC`] and its [`VERSION`], the
//!    compositor first. The compositor sends nothing more until the helper's
//!    version is its own, so a secret never reaches a helper that might read
//!    it differently; a helper given another version answers with its own and
//!    ends (`tests::a_helper_of_another_version_is_refused_before_the_secret`,
//!    `tests::a_client_of_another_version_is_told_and_refused`).
//! 2. **One request:** the user name and the secret, each a little-endian
//!    `u16` length and the bytes.
//! 3. **One answer:** a tag byte, then [`Answer::Failed`]'s delay as a `u32`
//!    of milliseconds, or [`Answer::Refused`]'s or [`Answer::Unavailable`]'s
//!    reason as a `u16` length and UTF-8.
//!
//! The helper then ends: one PAM transaction per process. The round trip is
//! `tests::a_round_trip_reaches_the_authenticator_and_back`.

use std::io::{self, Read, Write};

use crate::secret::{MAX_LEN, Secret};

/// What a peer sends first, so a process that is not the helper (or a socket
/// that is not the compositor's) is told apart from one of another version.
pub const MAGIC: &[u8; 11] = b"solium-auth";

/// The protocol's version. Changed whenever a message changes shape: 2 added
/// [`Answer::Refused`].
pub const VERSION: u32 = 2;

/// The longest user name: `LOGIN_NAME_MAX`.
pub const MAX_USER: usize = 256;

/// The longest reason an [`Answer::Unavailable`] carries, in bytes.
pub const MAX_REASON: usize = 1024;

const OK: u8 = 0;
const FAILED: u8 = 1;
const UNAVAILABLE: u8 = 2;
const REFUSED: u8 = 3;

/// What the helper answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The secret is the user's: the one answer that unlocks.
    Ok,
    /// It is not. `delay_ms` is the delay PAM's modules asked for before the
    /// next attempt, which the helper reports instead of sleeping through.
    Failed { delay_ms: u32 },
    /// PAM was given the secret and would not say yes or no: it asked for
    /// something besides the one password (a second password, a one-time
    /// code), or ended with a status that is neither. The secret reached the
    /// modules, so this counts as a wrong attempt, as pam_faillock counts it.
    Refused { reason: String },
    /// No answer could be had before the secret reached PAM: PAM would not
    /// start, or the request was out of shape.
    Unavailable { reason: String },
}

/// Whatever checks a secret: PAM in the helper, and a stand-in in the tests.
pub trait Authenticator {
    /// One check. Called at most once per process.
    fn authenticate(&mut self, user: &str, secret: &Secret) -> Answer;
}

/// Why an exchange ended without an answer.
#[derive(Debug)]
pub enum Error {
    /// The socket failed, or the peer went away.
    Io(io::Error),
    /// The peer did not open with [`MAGIC`].
    NotTheProtocol,
    /// The peer speaks another version.
    Version {
        /// The version the peer sent.
        theirs: u32,
    },
    /// A message did not have the shape this version gives it.
    Malformed(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::NotTheProtocol => write!(f, "the peer is not solium-auth"),
            Self::Version { theirs } => {
                write!(f, "the peer speaks version {theirs}, this side {VERSION}")
            }
            Self::Malformed(what) => write!(f, "malformed {what}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Send the handshake: [`MAGIC`] and `version`. Only a test sends a version
/// other than [`VERSION`].
pub fn write_hello(to: &mut impl Write, version: u32) -> io::Result<()> {
    let mut hello = [0; MAGIC.len() + 4];
    hello[..MAGIC.len()].copy_from_slice(MAGIC);
    hello[MAGIC.len()..].copy_from_slice(&version.to_le_bytes());
    to.write_all(&hello)?;
    to.flush()
}

/// Read the peer's handshake: its version.
pub fn read_hello(from: &mut impl Read) -> Result<u32, Error> {
    let mut hello = [0; MAGIC.len() + 4];
    from.read_exact(&mut hello)?;
    let (magic, version) = hello.split_at(MAGIC.len());
    if magic != MAGIC {
        return Err(Error::NotTheProtocol);
    }
    let mut bytes = [0; 4];
    bytes.copy_from_slice(version);
    Ok(u32::from_le_bytes(bytes))
}

/// The helper's side: the handshake, one request, the authenticator's answer.
///
/// A request that does not parse is answered [`Answer::Unavailable`], so the
/// compositor can say why. The secret is overwritten as soon as the
/// authenticator has seen it, before the answer is written.
pub fn serve(
    conn: &mut (impl Read + Write),
    auth: &mut impl Authenticator,
) -> Result<Answer, Error> {
    let theirs = read_hello(conn)?;
    write_hello(conn, VERSION)?;
    if theirs != VERSION {
        return Err(Error::Version { theirs });
    }
    let mut secret = Secret::new();
    let answer = match read_request(conn, &mut secret) {
        Ok(user) => {
            // A NUL ends the C string PAM is handed, so a secret with one in
            // it would be checked as a shorter one: it is never the password.
            if secret.expose().contains(&0) {
                Answer::Failed { delay_ms: 0 }
            } else {
                auth.authenticate(&user, &secret)
            }
        }
        Err(Error::Malformed(what)) => Answer::Unavailable {
            reason: format!("malformed {what}"),
        },
        Err(error) => return Err(error),
    };
    drop(secret);
    write_answer(conn, &answer)?;
    Ok(answer)
}

/// The compositor's side: the handshake, then, only once the helper has
/// answered it with this version, the request, and the answer.
pub fn ask(conn: &mut (impl Read + Write), user: &str, secret: &Secret) -> Result<Answer, Error> {
    if user.is_empty() || user.len() > MAX_USER || user.contains('\0') {
        return Err(Error::Malformed("user name"));
    }
    write_hello(conn, VERSION)?;
    let theirs = read_hello(conn)?;
    if theirs != VERSION {
        return Err(Error::Version { theirs });
    }
    write_bytes(conn, user.as_bytes())?;
    write_bytes(conn, secret.expose())?;
    conn.flush()?;
    read_answer(conn)
}

fn write_bytes(to: &mut impl Write, bytes: &[u8]) -> Result<(), Error> {
    let len = u16::try_from(bytes.len()).map_err(|_| Error::Malformed("length"))?;
    to.write_all(&len.to_le_bytes())?;
    to.write_all(bytes)?;
    Ok(())
}

fn read_len(from: &mut impl Read) -> Result<usize, Error> {
    let mut len = [0; 2];
    from.read_exact(&mut len)?;
    Ok(usize::from(u16::from_le_bytes(len)))
}

/// The request: the user name returned, the secret read into `secret`.
fn read_request(from: &mut impl Read, secret: &mut Secret) -> Result<String, Error> {
    let len = read_len(from)?;
    if len == 0 || len > MAX_USER {
        return Err(Error::Malformed("request: user name length"));
    }
    let mut user = vec![0; len];
    from.read_exact(&mut user)?;
    let user = String::from_utf8(user).map_err(|_| Error::Malformed("request: user name"))?;
    if user.contains('\0') {
        return Err(Error::Malformed("request: user name"));
    }
    let len = read_len(from)?;
    if len > MAX_LEN {
        return Err(Error::Malformed("request: secret length"));
    }
    secret.read_from(from, len)?;
    Ok(user)
}

fn write_answer(to: &mut impl Write, answer: &Answer) -> Result<(), Error> {
    match answer {
        Answer::Ok => to.write_all(&[OK])?,
        Answer::Failed { delay_ms } => {
            to.write_all(&[FAILED])?;
            to.write_all(&delay_ms.to_le_bytes())?;
        }
        Answer::Refused { reason } => {
            to.write_all(&[REFUSED])?;
            write_bytes(to, cut(reason, MAX_REASON).as_bytes())?;
        }
        Answer::Unavailable { reason } => {
            to.write_all(&[UNAVAILABLE])?;
            write_bytes(to, cut(reason, MAX_REASON).as_bytes())?;
        }
    }
    to.flush()?;
    Ok(())
}

fn read_answer(from: &mut impl Read) -> Result<Answer, Error> {
    let mut tag = [0];
    from.read_exact(&mut tag)?;
    match tag[0] {
        OK => Ok(Answer::Ok),
        FAILED => {
            let mut delay = [0; 4];
            from.read_exact(&mut delay)?;
            Ok(Answer::Failed {
                delay_ms: u32::from_le_bytes(delay),
            })
        }
        UNAVAILABLE => Ok(Answer::Unavailable {
            reason: read_reason(from)?,
        }),
        REFUSED => Ok(Answer::Refused {
            reason: read_reason(from)?,
        }),
        _ => Err(Error::Malformed("answer")),
    }
}

fn read_reason(from: &mut impl Read) -> Result<String, Error> {
    let len = read_len(from)?;
    if len > MAX_REASON {
        return Err(Error::Malformed("answer: reason length"));
    }
    let mut reason = vec![0; len];
    from.read_exact(&mut reason)?;
    Ok(String::from_utf8_lossy(&reason).into_owned())
}

/// `text`, cut to at most `max` bytes at a character boundary.
fn cut(text: &str, max: usize) -> &str {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    text.get(..end).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::{os::unix::net::UnixStream, thread};

    use super::*;

    /// What the stand-in was asked: the user and the secret's bytes.
    type Seen = Option<(String, Vec<u8>)>;

    /// The test authenticator: no PAM, one user and one password.
    struct StandIn {
        seen: Seen,
    }

    impl Authenticator for StandIn {
        fn authenticate(&mut self, user: &str, secret: &Secret) -> Answer {
            self.seen = Some((user.to_owned(), secret.expose().to_vec()));
            match (user, secret.expose()) {
                ("illia", b"correct horse") => Answer::Ok,
                ("illia", b"") => Answer::Unavailable {
                    reason: "no password".into(),
                },
                ("illia", b"otp") => Answer::Refused {
                    reason: "a one-time code".into(),
                },
                _ => Answer::Failed { delay_ms: 2000 },
            }
        }
    }

    /// The helper's side served on a thread, with the stand-in; what the
    /// compositor's side got, and what the stand-in saw.
    fn exchange(user: &str, password: &str) -> (Result<Answer, Error>, Seen) {
        let (mut ours, mut theirs) = UnixStream::pair().expect("a socketpair");
        let helper = thread::spawn(move || {
            let mut stand_in = StandIn { seen: None };
            let served = serve(&mut theirs, &mut stand_in);
            (served.ok(), stand_in.seen)
        });
        let mut secret = Secret::new();
        assert!(secret.push_str(password));
        let asked = ask(&mut ours, user, &secret);
        let (served, seen) = helper.join().expect("the helper's thread");
        if let Ok(answer) = &asked {
            assert_eq!(served.as_ref(), Some(answer), "both sides agree");
        }
        (asked, seen)
    }

    #[test]
    fn a_round_trip_reaches_the_authenticator_and_back() {
        let (asked, seen) = exchange("illia", "correct horse");
        assert_eq!(asked.expect("an answer"), Answer::Ok);
        assert_eq!(seen, Some(("illia".into(), b"correct horse".to_vec())));

        let (asked, _) = exchange("illia", "пароль");
        assert_eq!(asked.expect("an answer"), Answer::Failed { delay_ms: 2000 });

        let (asked, _) = exchange("illia", "");
        assert_eq!(
            asked.expect("an answer"),
            Answer::Unavailable {
                reason: "no password".into()
            }
        );

        let (asked, _) = exchange("illia", "otp");
        assert_eq!(
            asked.expect("an answer"),
            Answer::Refused {
                reason: "a one-time code".into()
            }
        );
    }

    /// The longest secret goes through whole.
    #[test]
    fn a_secret_of_max_len_reaches_the_authenticator_whole() {
        let password = "ж".repeat(MAX_LEN / 2);
        let (asked, seen) = exchange("someone", &password);
        assert_eq!(asked.expect("an answer"), Answer::Failed { delay_ms: 2000 });
        assert_eq!(seen.expect("seen").1, password.as_bytes());
    }

    /// A NUL would cut the C string PAM is given short: never checked.
    #[test]
    fn a_secret_with_a_nul_in_it_fails_without_reaching_the_authenticator() {
        let (asked, seen) = exchange("illia", "correct horse\0anything");
        assert_eq!(asked.expect("an answer"), Answer::Failed { delay_ms: 0 });
        assert_eq!(seen, None);
    }

    /// A helper of another version answers the handshake and ends; the
    /// compositor's side sends nothing more, so the secret stays here.
    #[test]
    fn a_helper_of_another_version_is_refused_before_the_secret() {
        let (mut ours, mut theirs) = UnixStream::pair().expect("a socketpair");
        let helper = thread::spawn(move || {
            assert_eq!(read_hello(&mut theirs).expect("a hello"), VERSION);
            write_hello(&mut theirs, VERSION + 1).expect("written");
            // Everything the compositor sends after the handshake.
            let mut after = Vec::new();
            let _ = theirs.read_to_end(&mut after);
            after
        });
        let mut secret = Secret::new();
        assert!(secret.push_str("correct horse"));
        let asked = ask(&mut ours, "illia", &secret);
        assert!(
            matches!(asked, Err(Error::Version { theirs }) if theirs == VERSION + 1),
            "{asked:?}"
        );
        drop(ours);
        assert_eq!(
            helper.join().expect("the helper's thread"),
            Vec::<u8>::new()
        );
    }

    /// A compositor of another version is told the helper's, and its request
    /// is never read.
    #[test]
    fn a_client_of_another_version_is_told_and_refused() {
        let (mut ours, mut theirs) = UnixStream::pair().expect("a socketpair");
        let helper = thread::spawn(move || {
            let mut stand_in = StandIn { seen: None };
            let served = serve(&mut theirs, &mut stand_in);
            (served, stand_in.seen)
        });
        write_hello(&mut ours, VERSION + 7).expect("written");
        assert_eq!(read_hello(&mut ours).expect("its hello"), VERSION);
        let (served, seen) = helper.join().expect("the helper's thread");
        assert!(matches!(served, Err(Error::Version { theirs }) if theirs == VERSION + 7));
        assert_eq!(seen, None);
    }

    /// A peer that is not the helper at all.
    #[test]
    fn a_peer_without_the_magic_is_not_the_protocol() {
        let mut wrong = &b"something else entirely"[..];
        assert!(matches!(read_hello(&mut wrong), Err(Error::NotTheProtocol)));
    }

    /// A request out of shape is answered, so the compositor can say why.
    #[test]
    fn a_malformed_request_is_answered_unavailable() {
        let (mut ours, mut theirs) = UnixStream::pair().expect("a socketpair");
        let helper = thread::spawn(move || {
            let mut stand_in = StandIn { seen: None };
            (serve(&mut theirs, &mut stand_in).ok(), stand_in.seen)
        });
        write_hello(&mut ours, VERSION).expect("written");
        assert_eq!(read_hello(&mut ours).expect("its hello"), VERSION);
        // A user name, then a secret longer than any secret can be.
        ours.write_all(&5u16.to_le_bytes()).expect("written");
        ours.write_all(b"illia").expect("written");
        ours.write_all(&u16::try_from(MAX_LEN + 1).expect("fits").to_le_bytes())
            .expect("written");
        let answer = read_answer(&mut ours).expect("an answer");
        assert!(
            matches!(&answer, Answer::Unavailable { reason } if reason.contains("secret length")),
            "{answer:?}"
        );
        let (served, seen) = helper.join().expect("the helper's thread");
        assert_eq!(served, Some(answer));
        assert_eq!(seen, None);
    }

    /// The compositor's side refuses a user name the helper would refuse.
    #[test]
    fn a_user_name_out_of_shape_is_never_sent() {
        let (mut ours, _theirs) = UnixStream::pair().expect("a socketpair");
        let secret = Secret::new();
        for user in ["", "a\0b", "u".repeat(MAX_USER + 1).as_str()] {
            assert!(matches!(
                ask(&mut ours, user, &secret),
                Err(Error::Malformed(_))
            ));
        }
    }

    /// A reason too long for the answer is cut at a character, not refused.
    #[test]
    fn a_long_reason_is_cut_at_a_character() {
        let mut bytes = Vec::new();
        let reason = "ы".repeat(MAX_REASON);
        write_answer(&mut bytes, &Answer::Unavailable { reason }).expect("written");
        let Answer::Unavailable { reason } = read_answer(&mut bytes.as_slice()).expect("read")
        else {
            panic!("not unavailable");
        };
        assert_eq!(reason.len(), MAX_REASON);
        assert!(reason.chars().all(|c| c == 'ы'));
    }
}
