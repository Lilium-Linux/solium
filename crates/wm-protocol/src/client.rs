//! A window manager's end of the socket.
//!
//! The compositor starts a window manager with the socket as its standard
//! input ([`Client::from_stdin`]), and the conversation is short:
//!
//! 1. the compositor says [`crate::Hello`] ([`Client::hello`]), and the
//!    window manager answers with the commands that set the desktop up
//!    before its first frame ([`Client::ready`]), or says why it cannot run
//!    ([`Client::refuse`]);
//! 2. from then on the compositor sends batches ([`Client::next`]), each
//!    answered once with the same serial ([`Client::answer`],
//!    [`Client::reply`]), in the order they came;
//! 3. while it handles a batch the window manager may ask the compositor a
//!    question ([`Client::query`]), and at any time at all it may send
//!    commands of its own ([`Client::submit`]): what a timer or a socket of
//!    its own led to.
//!
//! Every method takes `&self`, so a handler deep inside a batch can ask a
//! question on the connection the loop around it is reading.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::io;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use crate::frame::{self, FrameError};
use crate::{Answer, Batch, Command, Hello, HelloAnswer, Message, Query, Reloaded, Reply};

/// Who answers a question only the compositor can: the compositor, from
/// what it knows of the desktop; and, at a window manager's end, whatever
/// puts the question on the socket and waits for the reply.
pub trait Ask {
    /// The answer, or none when nobody can give one.
    fn ask(&self, query: Query) -> Option<Reply>;
}

/// Why the conversation cannot go on.
#[derive(Debug)]
pub enum ClientError {
    /// The socket failed, or what came over it was not a message.
    Frame(FrameError),
    /// The compositor said something the protocol does not have here.
    Protocol(String),
    /// The compositor speaks another version of the protocol: its version.
    Version(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Frame(err) => write!(f, "{err}"),
            Self::Protocol(what) => f.write_str(what),
            Self::Version(theirs) => write!(
                f,
                "this window manager speaks the protocol {}, the compositor {theirs}",
                crate::VERSION
            ),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<FrameError> for ClientError {
    fn from(err: FrameError) -> Self {
        Self::Frame(err)
    }
}

impl From<io::Error> for ClientError {
    fn from(err: io::Error) -> Self {
        Self::Frame(FrameError::Io(err))
    }
}

/// What the compositor said next.
#[derive(Debug)]
pub enum Heard {
    /// What changed and what happened: answer it, once, with its serial.
    Batch(Batch),
    /// The configuration at this path is to be loaded again: answer with
    /// [`Client::reloaded`]. A window manager with no configuration of its
    /// own to read answers that it loaded.
    Reload(String),
    /// The compositor is closing the connection, and why; or it closed it.
    Goodbye(String),
}

/// The connection to the compositor.
#[derive(Debug)]
pub struct Client {
    stream: UnixStream,
    reader: RefCell<frame::Reader>,
    /// What came while a question waited for its reply, oldest first.
    waiting: RefCell<VecDeque<Message>>,
    /// The last query's serial.
    queries: Cell<u64>,
}

impl Client {
    /// The connection the compositor handed this process: its standard
    /// input. An error when that is not a socket, which is how a window
    /// manager started by hand finds out.
    pub fn from_stdin() -> io::Result<Self> {
        let stream = UnixStream::from(io::stdin().as_fd().try_clone_to_owned()?);
        // A socket has a peer; a terminal or a file does not.
        stream.peer_addr().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "standard input is not the socket to the compositor",
            )
        })?;
        Ok(Self::over(stream))
    }

    /// The connection over `stream`.
    pub fn over(stream: UnixStream) -> Self {
        Self {
            stream,
            reader: RefCell::new(frame::Reader::default()),
            waiting: RefCell::new(VecDeque::new()),
            queries: Cell::new(0),
        }
    }

    fn send(&self, message: &Message) -> io::Result<()> {
        frame::write(&mut &self.stream, message)
    }

    /// The next message off the socket, waited for.
    fn receive(&self) -> Result<Message, FrameError> {
        self.reader.borrow_mut().read_blocking(&mut &self.stream)
    }

    /// The compositor's `hello`, which is its first message: the protocol
    /// it speaks, and what the window manager this one takes the place of
    /// kept. One of another version ([`compatible`]) is told goodbye, and
    /// is the error.
    pub fn hello(&self) -> Result<Hello, ClientError> {
        let hello = match self.receive() {
            Ok(Message::Hello(hello)) => hello,
            Ok(other) => {
                return Err(ClientError::Protocol(format!(
                    "the compositor's first message was not `hello`: {other:?}"
                )));
            }
            Err(err) => {
                return Err(ClientError::Protocol(format!(
                    "no `hello` from the compositor: {err}"
                )));
            }
        };
        if !compatible(crate::VERSION, &hello.version) {
            let refused = ClientError::Version(hello.version);
            let _ = self.send(&Message::Goodbye(refused.to_string()));
            return Err(refused);
        }
        Ok(hello)
    }

    /// Answer `hello`: the commands that set the desktop up before its
    /// first frame.
    pub fn ready(&self, commands: Vec<Command>) -> io::Result<()> {
        self.send(&Message::HelloAnswer(HelloAnswer {
            version: crate::VERSION.to_owned(),
            commands,
        }))
    }

    /// Say why this window manager cannot run, in place of
    /// [`Self::ready`], or that it is leaving, at any time after.
    pub fn refuse(&self, why: &str) -> io::Result<()> {
        self.send(&Message::Goodbye(why.to_owned()))
    }

    /// What the compositor says next, waited for.
    pub fn next(&self) -> Result<Heard, ClientError> {
        loop {
            let message = match self.waiting.borrow_mut().pop_front() {
                Some(message) => message,
                None => match self.receive() {
                    Ok(message) => message,
                    Err(FrameError::Closed) => {
                        return Ok(Heard::Goodbye(
                            "the compositor closed the connection".to_owned(),
                        ));
                    }
                    Err(err) => return Err(err.into()),
                },
            };
            match message {
                Message::Batch(batch) => return Ok(Heard::Batch(batch)),
                Message::Reload(path) => return Ok(Heard::Reload(path)),
                Message::Goodbye(why) => return Ok(Heard::Goodbye(why)),
                // A reply nobody waits for any more, or a message that is
                // not the compositor's to send: nothing to do with either.
                _ => {}
            }
        }
    }

    /// What the compositor says next, when it says it within `patience`:
    /// for a window manager with a clock of its own to keep.
    pub fn next_within(&self, patience: Duration) -> Result<Option<Heard>, ClientError> {
        if self.waiting.borrow().is_empty() {
            let ready = self.reader.borrow_mut().next_message()?;
            match ready {
                Some(message) => self.waiting.borrow_mut().push_back(message),
                None => {
                    // A zero timeout would mean no timeout at all.
                    let patience = patience.max(Duration::from_micros(1));
                    self.stream.set_read_timeout(Some(patience))?;
                    let mut chunk = [0u8; 64 * 1024];
                    let read = io::Read::read(&mut &self.stream, &mut chunk);
                    self.stream.set_read_timeout(None)?;
                    match read {
                        Ok(0) => {
                            return Ok(Some(Heard::Goodbye(
                                "the compositor closed the connection".to_owned(),
                            )));
                        }
                        Ok(count) => self
                            .reader
                            .borrow_mut()
                            .push(chunk.get(..count).unwrap_or_default()),
                        Err(err)
                            if matches!(
                                err.kind(),
                                io::ErrorKind::WouldBlock
                                    | io::ErrorKind::TimedOut
                                    | io::ErrorKind::Interrupted
                            ) =>
                        {
                            return Ok(None);
                        }
                        Err(err) => return Err(err.into()),
                    }
                    match self.reader.borrow_mut().next_message()? {
                        Some(message) => self.waiting.borrow_mut().push_back(message),
                        // Part of a message: the rest is on its way.
                        None => return Ok(None),
                    }
                }
            }
        }
        self.next().map(Some)
    }

    /// Answer a batch.
    pub fn answer(&self, answer: Answer) -> io::Result<()> {
        self.send(&Message::Answer(answer))
    }

    /// Answer the batch of this serial with commands: `handled` says
    /// whether its key, press or scroll was this window manager's, and not
    /// the client's under it.
    pub fn reply(&self, serial: u64, handled: bool, commands: Vec<Command>) -> io::Result<()> {
        self.answer(Answer {
            serial,
            handled,
            commands,
            motion: None,
            decision: None,
        })
    }

    /// Answer a reload.
    pub fn reloaded(&self, reloaded: Reloaded) -> io::Result<()> {
        self.send(&Message::Reloaded(reloaded))
    }

    /// Commands of the window manager's own, asked for by no batch.
    pub fn submit(&self, commands: Vec<Command>) -> io::Result<()> {
        self.send(&Message::Submit(commands))
    }

    /// Ask the compositor a question, in the middle of a batch, and wait
    /// for its reply. What the compositor sends meanwhile is kept, and is
    /// what [`Self::next`] hears afterwards.
    pub fn query(&self, query: Query) -> Result<Reply, ClientError> {
        let serial = self.queries.get() + 1;
        self.queries.set(serial);
        self.send(&Message::Query((serial, query)))?;
        loop {
            match self.receive()? {
                Message::Reply((answered, reply)) if answered == serial => return Ok(reply),
                other => self.waiting.borrow_mut().push_back(other),
            }
        }
    }
}

/// Whether two versions of the protocol speak to each other: the same major
/// version, and before 1.0, the same minor one.
pub fn compatible(ours: &str, theirs: &str) -> bool {
    let parts = |version: &str| -> Vec<u64> {
        version
            .split('.')
            .map(|part| part.parse().unwrap_or(u64::MAX))
            .collect()
    };
    let (ours, theirs) = (parts(ours), parts(theirs));
    match (ours.first(), theirs.first()) {
        (Some(0), Some(0)) => ours.get(1) == theirs.get(1),
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    use super::{Client, ClientError, Heard, compatible};
    use crate::frame::{self, Reader};
    use crate::{Batch, Command, Event, Hello, Message, Query, Reloaded, Reply};

    /// The compositor's end, played by the test.
    struct Compositor {
        stream: UnixStream,
        reader: Reader,
    }

    impl Compositor {
        fn say(&mut self, message: &Message) {
            frame::write(&mut self.stream, message).expect("written");
        }

        fn hear(&mut self) -> Message {
            self.reader
                .read_blocking(&mut self.stream)
                .expect("a message")
        }
    }

    fn pair() -> (Compositor, Client) {
        let (ours, theirs) = UnixStream::pair().expect("a socket pair");
        (
            Compositor {
                stream: ours,
                reader: Reader::default(),
            },
            Client::over(theirs),
        )
    }

    fn batch(serial: u64, event: Event) -> Message {
        Message::Batch(Batch {
            serial,
            at: 0.0,
            changes: Vec::new(),
            events: vec![event],
        })
    }

    fn hello(version: &str) -> Message {
        Message::Hello(Hello {
            version: version.to_owned(),
            debug: false,
            keeps: Vec::new(),
        })
    }

    #[test]
    fn before_one_point_oh_a_minor_version_is_another_protocol() {
        assert!(compatible("0.1.0", "0.1.3"));
        assert!(!compatible("0.1.0", "0.2.0"));
        assert!(compatible("1.2.0", "1.7.1"));
        assert!(!compatible("1.0.0", "2.0.0"));
        assert!(!compatible("0.1.0", "nonsense"));
    }

    /// The whole conversation, once: hello and its answer, a batch and its
    /// answer, a reload, commands of the window manager's own, goodbye.
    #[test]
    fn a_conversation_from_hello_to_goodbye() {
        let (mut compositor, client) = pair();
        compositor.say(&hello(crate::VERSION));
        let heard = client.hello().expect("hello");
        assert_eq!(heard.version, crate::VERSION);
        client
            .ready(vec![Command::Status("up".to_owned())])
            .expect("sent");
        let Message::HelloAnswer(answer) = compositor.hear() else {
            panic!("not hello-answer");
        };
        assert_eq!(answer.version, crate::VERSION);
        assert_eq!(answer.commands, [Command::Status("up".to_owned())]);

        compositor.say(&batch(1, Event::Key("super+t".to_owned())));
        let Heard::Batch(heard) = client.next().expect("a batch") else {
            panic!("not a batch");
        };
        assert_eq!(heard.events, [Event::Key("super+t".to_owned())]);
        client
            .reply(heard.serial, true, vec![Command::Close(4)])
            .expect("sent");
        let Message::Answer(answer) = compositor.hear() else {
            panic!("not an answer");
        };
        assert_eq!(
            (answer.serial, answer.handled, &answer.commands[..]),
            (1, true, &[Command::Close(4)][..])
        );

        compositor.say(&Message::Reload("/some/where".to_owned()));
        let Heard::Reload(path) = client.next().expect("a reload") else {
            panic!("not a reload");
        };
        assert_eq!(path, "/some/where");
        client
            .reloaded(Reloaded::Failed("no".to_owned()))
            .expect("sent");
        assert_eq!(
            compositor.hear(),
            Message::Reloaded(Reloaded::Failed("no".to_owned()))
        );

        client.submit(vec![Command::Quit]).expect("sent");
        assert_eq!(compositor.hear(), Message::Submit(vec![Command::Quit]));

        compositor.say(&Message::Goodbye("closing".to_owned()));
        assert!(matches!(client.next(), Ok(Heard::Goodbye(why)) if why == "closing"));
        drop(compositor);
        assert!(
            matches!(client.next(), Ok(Heard::Goodbye(_))),
            "a closed connection is a goodbye too"
        );
    }

    /// A compositor of another version is told goodbye with both versions
    /// named, and is the error.
    #[test]
    fn a_compositor_of_another_version_is_refused() {
        let (mut compositor, client) = pair();
        compositor.say(&hello("7.0.0"));
        let refused = client.hello().expect_err("refused");
        assert!(matches!(&refused, ClientError::Version(theirs) if theirs == "7.0.0"));
        let Message::Goodbye(why) = compositor.hear() else {
            panic!("not goodbye");
        };
        assert_eq!(why, refused.to_string());
        assert!(
            why.contains(crate::VERSION) && why.contains("7.0.0"),
            "{why}"
        );

        let (mut compositor, client) = pair();
        compositor.say(&batch(1, Event::Relayout));
        assert!(matches!(client.hello(), Err(ClientError::Protocol(_))));
    }

    /// The compositor does not wait for a window manager that is behind: it
    /// may have sent the next batch before it reads a question. That batch
    /// is not the reply, and is not lost: it is heard next.
    #[test]
    fn a_batch_that_comes_while_a_question_waits_is_heard_after_the_reply() {
        let (mut compositor, client) = pair();
        compositor.say(&batch(2, Event::Relayout));
        compositor.say(&batch(3, Event::Closed(9)));
        compositor.say(&Message::Reply((1, Reply::Window(Some(5)))));
        let reply = client
            .query(Query::WindowAt((10.0, 20.0, None)))
            .expect("a reply");
        assert_eq!(reply, Reply::Window(Some(5)));
        assert_eq!(
            compositor.hear(),
            Message::Query((1, Query::WindowAt((10.0, 20.0, None))))
        );
        for serial in [2, 3] {
            let Ok(Heard::Batch(heard)) = client.next() else {
                panic!("batch {serial} was lost");
            };
            assert_eq!(heard.serial, serial);
        }
    }

    /// A window manager with a clock of its own waits only so long, and
    /// hears what came, whole, when it did.
    #[test]
    fn a_wait_with_patience_ends_when_nothing_comes() {
        let (mut compositor, client) = pair();
        assert!(matches!(
            client.next_within(Duration::from_millis(5)),
            Ok(None)
        ));
        let whole = frame::encode(&batch(4, Event::Relayout));
        let (head, tail) = whole.split_at(3);
        std::io::Write::write_all(&mut compositor.stream, head).expect("written");
        assert!(
            matches!(client.next_within(Duration::from_millis(5)), Ok(None)),
            "part of a message is not one yet"
        );
        std::io::Write::write_all(&mut compositor.stream, tail).expect("written");
        assert!(matches!(
            client.next_within(Duration::from_millis(500)),
            Ok(Some(Heard::Batch(heard))) if heard.serial == 4
        ));
        drop(compositor);
        assert!(matches!(
            client.next_within(Duration::from_millis(500)),
            Ok(Some(Heard::Goodbye(_)))
        ));
    }
}
