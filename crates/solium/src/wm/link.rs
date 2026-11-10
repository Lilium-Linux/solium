//! The compositor's end of the socket to the window manager.
//!
//! Messages go out whole and come back whole, and that is all this is: who
//! waits for which answer, and for how long, is [`crate::wm::Scripts`]'s. A
//! question the window manager asks, a `query`, is answered here as it is
//! read, from whoever the reader says answers.
//!
//! The window manager is a program of its own, the socket its standard
//! input: the shipped `solium-lua`, or the command the user's `wm.toml`
//! names ([`crate::wm::Chosen`]). A test runs it on a thread of its own instead, over a socket pair
//! all the same, so every test of the configuration crosses the protocol.

use std::cell::RefCell;
use std::io::Read as _;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use solium_wm_protocol::frame;
use solium_wm_protocol::{self as wire, Message};

use crate::script::lua::Ask;
use crate::wm::Chosen;

/// How long a write to the window manager may block. Its socket's buffer
/// holds hundreds of batches, so a write blocks only when the window manager
/// has read none of them: that one has stopped, and the compositor does not
/// stop with it.
const WRITING: Duration = Duration::from_secs(1);

/// The connection, and the window manager at its other end.
pub(crate) struct Link {
    stream: UnixStream,
    reader: RefCell<frame::Reader>,
    peer: Peer,
    /// The connection failed: the window manager is stopped without a
    /// goodbye it would not read.
    abandoned: std::cell::Cell<bool>,
    /// When the window manager last wrote anything.
    heard: std::cell::Cell<Instant>,
}

enum Peer {
    #[cfg_attr(
        test,
        expect(dead_code, reason = "a test runs the window manager on a thread")
    )]
    Process(RefCell<std::process::Child>),
    #[cfg(test)]
    Thread {
        control: Option<std::sync::mpsc::Sender<tests::Control>>,
        thread: Option<std::thread::JoinHandle<()>>,
    },
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.peer {
            Peer::Process(child) => write!(f, "Link(process {})", child.borrow().id()),
            #[cfg(test)]
            Peer::Thread { .. } => f.write_str("Link(thread)"),
        }
    }
}

impl Link {
    /// Start the window manager on the configuration at `config`, and wait
    /// `within` for its `hello-answer`: what the configuration asked for as
    /// it loaded. Its questions while it loads go to `answers`. With
    /// `shipped_only` it reads nothing of the user's own, and is the
    /// shipped window manager whatever `chosen`, the user's `wm.toml`,
    /// names. `keeps` is what the window manager it takes the place of
    /// kept, handed to it in `hello`: nothing, for the first.
    pub(crate) fn start(
        config: &Path,
        shipped_only: bool,
        chosen: &Chosen,
        keeps: Vec<(String, wire::Doc)>,
        answers: &dyn Ask,
        within: Duration,
    ) -> Result<(Self, Vec<wire::Command>)> {
        Self::open(config, shipped_only, chosen)?.greeted(keeps, answers, within)
    }

    /// Say `hello` to the window manager at the other end, and wait
    /// `within` for its `hello-answer`: the link, and what it asked for.
    /// One that speaks another version of the protocol, says goodbye, or
    /// says nothing in that time is the error.
    fn greeted(
        self,
        keeps: Vec<(String, wire::Doc)>,
        answers: &dyn Ask,
        within: Duration,
    ) -> Result<(Self, Vec<wire::Command>)> {
        let link = self;
        // What goes wrong on the socket is said as what the window manager
        // did, for whoever reports it to put a name to.
        let named = |err: anyhow::Error| anyhow!("the window manager {err:#}");
        link.send(&Message::Hello(wire::Hello {
            version: wire::VERSION.to_owned(),
            debug: crate::dev::debug_mode(),
            keeps,
        }))
        .map_err(named)?;
        let answered = link
            .next(Some(Instant::now() + within), answers)
            .map_err(named)?;
        match answered {
            Some(Message::HelloAnswer(answer)) => {
                if !wire::compatible(wire::VERSION, &answer.version) {
                    bail!(
                        "the window manager speaks the protocol {}, the compositor {}",
                        answer.version,
                        wire::VERSION
                    );
                }
                Ok((link, answer.commands))
            }
            Some(Message::Goodbye(why)) => Err(anyhow!(why)),
            Some(other) => bail!("the window manager answered `hello` with {other:?}"),
            None => bail!("the window manager did not answer `hello` within {within:?}"),
        }
    }

    /// Write `message` to the window manager, whole. An error here, and
    /// from [`Self::next`], says what the window manager did: it follows
    /// "the window manager" wherever it is reported.
    pub(crate) fn send(&self, message: &Message) -> Result<()> {
        frame::write(&mut &self.stream, message).context("could not be written to")?;
        if !matches!(message, Message::Hello(_)) {
            self.wake();
        }
        Ok(())
    }

    /// The window manager's next message, its questions answered from
    /// `answers` on the way. Waited for until `until`; with none, only read
    /// if it has already come. `None` when nothing came.
    pub(crate) fn next(
        &self,
        until: Option<Instant>,
        answers: &dyn Ask,
    ) -> Result<Option<Message>> {
        loop {
            let read = self.reader.borrow_mut().next_message();
            match read {
                Ok(Some(Message::Query((serial, query)))) => {
                    let reply = answers
                        .ask(query.clone())
                        .unwrap_or_else(|| unanswered(&query));
                    frame::write(&mut &self.stream, &Message::Reply((serial, reply)))
                        .context("could not be answered")?;
                }
                Ok(Some(other)) => return Ok(Some(other)),
                Ok(None) => {
                    if !self.fill(until)? {
                        return Ok(None);
                    }
                }
                Err(err) => bail!("sent what is not a message: {err}"),
            }
        }
    }

    /// Read more of what the window manager wrote: `false` when nothing
    /// came by `until`, or, with none, when nothing is there to read.
    fn fill(&self, until: Option<Instant>) -> Result<bool> {
        let mut chunk = [0u8; 64 * 1024];
        let read = match until {
            Some(until) => {
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Ok(false);
                }
                self.stream.set_read_timeout(Some(left))?;
                (&self.stream).read(&mut chunk)
            }
            None => {
                self.stream.set_nonblocking(true)?;
                let read = (&self.stream).read(&mut chunk);
                self.stream.set_nonblocking(false)?;
                read
            }
        };
        match read {
            Ok(0) => bail!("closed its connection"),
            Ok(count) => {
                self.heard.set(Instant::now());
                self.reader
                    .borrow_mut()
                    .push(chunk.get(..count).unwrap_or_default());
                Ok(true)
            }
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                Ok(false)
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => Ok(true),
            Err(err) => bail!("could not be read from: {err}"),
        }
    }

    /// The socket, for the event loop to wake on what the window manager
    /// writes.
    pub(crate) fn socket(&self) -> std::io::Result<std::os::fd::OwnedFd> {
        self.stream.try_clone().map(std::os::fd::OwnedFd::from)
    }

    /// The window manager's process, its standard input the socket: the
    /// command `chosen` names, as it is written, or the shipped
    /// `solium-lua` on `config`.
    #[cfg(not(test))]
    fn open(config: &Path, shipped_only: bool, chosen: &Chosen) -> Result<Self> {
        let (ours, theirs) = UnixStream::pair().context("making the window manager's socket")?;
        use std::os::unix::process::CommandExt as _;
        let (program, mut command) = match chosen.command.as_deref() {
            Some([program, arguments @ ..]) if !shipped_only => {
                let mut command = std::process::Command::new(program);
                command.args(arguments);
                (std::path::PathBuf::from(program), command)
            }
            _ => {
                let program = program().map_err(|why| anyhow!(why))?;
                let mut command = std::process::Command::new(&program);
                command.arg(config).arg(crate::assets::lua());
                // Where the user's own Lua is, unless none of it is to be
                // read.
                if let Some(user) = (!shipped_only)
                    .then(crate::script::lua::user_config_dir)
                    .flatten()
                {
                    command.arg(user);
                }
                (program, command)
            }
        };
        let child = command
            .stdin(std::process::Stdio::from(std::os::fd::OwnedFd::from(
                theirs,
            )))
            // A group of its own, so a signal to the compositor's group (a
            // logout, `timeout`) does not end it before the compositor has
            // asked for its stores one last time: it goes when the compositor
            // says goodbye, or when the socket closes with it.
            .process_group(0)
            .spawn()
            .with_context(|| format!("starting {}", program.display()))?;
        tracing::info!(
            pid = child.id(),
            program = %program.display(),
            "window manager started"
        );
        Self::connected(ours, Peer::Process(RefCell::new(child)))
    }

    /// The compositor's end of `stream`, with `peer` at the other.
    fn connected(stream: UnixStream, peer: Peer) -> Result<Self> {
        stream
            .set_write_timeout(Some(WRITING))
            .context("bounding a write to the window manager")?;
        Ok(Self {
            stream,
            reader: RefCell::new(frame::Reader::default()),
            peer,
            abandoned: std::cell::Cell::new(false),
            heard: std::cell::Cell::new(Instant::now()),
        })
    }

    /// The window manager's process, when it is one.
    pub(crate) fn pid(&self) -> Option<u32> {
        match &self.peer {
            Peer::Process(child) => Some(child.borrow().id()),
            #[cfg(test)]
            Peer::Thread { .. } => None,
        }
    }

    /// When the window manager last wrote anything: an answer, a question,
    /// or part of either.
    pub(crate) fn heard(&self) -> Instant {
        self.heard.get()
    }

    /// The connection has failed: when this is dropped the window manager
    /// is stopped at once, not said goodbye to and waited for.
    pub(crate) fn abandon(&self) {
        self.abandoned.set(true);
    }

    /// Tell a window manager on a thread that a message is waiting.
    fn wake(&self) {
        #[cfg(test)]
        if let Peer::Thread {
            control: Some(control),
            ..
        } = &self.peer
        {
            let _ = control.send(tests::Control::Wake(tracing::dispatcher::get_default(
                Clone::clone,
            )));
        }
    }
}

impl Drop for Link {
    /// Say goodbye, and wait a moment for the window manager to go: one that
    /// does not is stopped.
    fn drop(&mut self) {
        match &mut self.peer {
            Peer::Process(child) => {
                let child = child.get_mut();
                if self.abandoned.get() {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
                let _ = frame::write(
                    &mut &self.stream,
                    &Message::Goodbye("the compositor is closing".to_owned()),
                );
                let until = Instant::now() + Duration::from_secs(1);
                while Instant::now() < until {
                    if let Ok(Some(_)) = child.try_wait() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                tracing::warn!(pid = child.id(), "the window manager did not go; stopped");
                let _ = child.kill();
                let _ = child.wait();
            }
            #[cfg(test)]
            Peer::Thread { control, thread } => {
                // Its loop ends with the last message it will be told of,
                // or, for one that reads the socket itself, with the socket.
                control.take();
                let _ = self.stream.shutdown(std::net::Shutdown::Both);
                if let Some(thread) = thread.take() {
                    let _ = thread.join();
                }
            }
        }
    }
}

/// The window manager this compositor starts: the `solium-lua` an install
/// put in `<prefix>/libexec`, or the one beside the binary in a build tree.
/// Never one from `PATH`, which may be another install's.
pub(crate) fn program() -> Result<std::path::PathBuf, String> {
    crate::assets::helper("solium-lua").map_err(|why| format!("{why}, so no window manager"))
}

/// The answer to a question nobody here can answer: no window, no store,
/// no styles.
fn unanswered(query: &wire::Query) -> wire::Reply {
    match query {
        wire::Query::WindowAt(_) => wire::Reply::Window(None),
        wire::Query::Store(_) => wire::Reply::Store(None),
        wire::Query::Styles => wire::Reply::Styles(Vec::new()),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::os::unix::net::UnixStream;
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use anyhow::{Context, Result};
    use solium_wm_protocol::{self as wire, Message, frame};

    use super::{Link, Peer};
    use crate::script::lua::Ask;
    use crate::wm::Chosen;

    /// Something for the window manager's thread to run with it, and where
    /// its result goes.
    pub(crate) type Job = Box<dyn FnOnce(&mut solium_lua::Wm) -> String + Send>;

    /// What the compositor's side tells the window manager's thread.
    pub(crate) enum Control {
        /// A message is waiting on the socket. What the window manager logs
        /// answering it goes to the log the test is reading then.
        Wake(tracing::Dispatch),
        /// Run this, outside the protocol: a test looking into the Lua.
        Run(Job, tracing::Dispatch, mpsc::Sender<String>),
    }

    impl Link {
        /// The window manager on a thread of its own, over a socket pair:
        /// the Lua one, whatever `wm.toml` names.
        pub(super) fn open(config: &Path, shipped_only: bool, _chosen: &Chosen) -> Result<Self> {
            let (ours, theirs) =
                UnixStream::pair().context("making the window manager's socket")?;
            let (control, heard) = mpsc::channel::<Control>();
            let config = config.to_owned();
            let user = (!shipped_only).then(solium_lua::user_config_dir).flatten();
            // What it logs as it loads goes to the log the test is reading.
            let log = tracing::dispatcher::get_default(Clone::clone);
            let thread = std::thread::Builder::new()
                .name("window manager".to_owned())
                .spawn(move || {
                    let started = tracing::dispatcher::with_default(&log, || {
                        solium_lua::Wm::start(theirs, &config, crate::assets::lua(), user)
                    });
                    let Ok(mut wm) = started else {
                        return;
                    };
                    while let Ok(control) = heard.recv() {
                        match control {
                            Control::Wake(log) => {
                                match tracing::dispatcher::with_default(&log, || wm.step()) {
                                    Ok(true) => {}
                                    Ok(false) => break,
                                    Err(err) => {
                                        tracing::error!("{err:#}");
                                        break;
                                    }
                                }
                            }
                            Control::Run(job, log, back) => {
                                let said = tracing::dispatcher::with_default(&log, || job(&mut wm));
                                let _ = back.send(said);
                            }
                        }
                    }
                })
                .context("starting the window manager's thread")?;
            Self::connected(
                ours,
                Peer::Thread {
                    control: Some(control),
                    thread: Some(thread),
                },
            )
        }

        /// The compositor's end of `stream`, whose other end a test holds
        /// and answers by hand.
        fn over(stream: UnixStream) -> Self {
            Self::connected(
                stream,
                Peer::Thread {
                    control: None,
                    thread: None,
                },
            )
            .expect("a socket")
        }

        /// A window manager that is not the Lua one, run by `run` on a
        /// thread of its own with the other end of the socket: it reads and
        /// answers for itself, as a program started for it would, and is
        /// greeted as one is.
        pub(crate) fn played(
            run: impl FnOnce(UnixStream) + Send + 'static,
            answers: &dyn Ask,
        ) -> Result<(Self, Vec<wire::Command>)> {
            let (ours, theirs) =
                UnixStream::pair().context("making the window manager's socket")?;
            let thread = std::thread::Builder::new()
                .name("window manager".to_owned())
                .spawn(move || run(theirs))
                .context("starting the window manager's thread")?;
            Self::connected(
                ours,
                Peer::Thread {
                    control: None,
                    thread: Some(thread),
                },
            )?
            .greeted(Vec::new(), answers, Duration::from_secs(10))
        }

        /// End the window manager on its thread, as one that crashed ends:
        /// its end of the socket closes, and it says nothing first.
        pub(crate) fn end_peer(&mut self) {
            if let Peer::Thread { control, thread } = &mut self.peer {
                control.take();
                if let Some(thread) = thread.take() {
                    let _ = thread.join();
                }
            }
        }

        /// Run `job` on the window manager's thread and hand back what it
        /// says, answering the questions it asks on the way from `answers`.
        pub(crate) fn run(&self, job: Job, answers: &dyn Ask) -> String {
            let Peer::Thread {
                control: Some(control),
                ..
            } = &self.peer
            else {
                return "error: no window manager on a thread".to_owned();
            };
            let (back, result) = mpsc::channel();
            let log = tracing::dispatcher::get_default(Clone::clone);
            if control.send(Control::Run(job, log, back)).is_err() {
                return "error: the window manager has gone".to_owned();
            }
            loop {
                match result.try_recv() {
                    Ok(said) => return said,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        return "error: the window manager has gone".to_owned();
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                }
                match self.next(Some(Instant::now() + Duration::from_millis(1)), answers) {
                    Ok(None) => {}
                    Ok(Some(other)) => {
                        panic!("the window manager sent {other:?} outside a request")
                    }
                    Err(err) => return format!("error: {err:#}"),
                }
            }
        }
    }

    /// A window manager played by hand: the test holds its end of the
    /// socket, and answers what it chooses, when it chooses, so what is in
    /// time and what is late is the test's to say.
    pub(crate) struct Hand {
        stream: UnixStream,
        reader: frame::Reader,
    }

    impl Hand {
        /// The compositor's side of a window manager that answered `hello`
        /// with `commands`, and its other end. It keeps the session's time:
        /// an answer is waited for as long as `wm` says, and no longer. It
        /// listens to every event there is, unless `commands` say what it
        /// listens to.
        pub(crate) fn with(mut commands: Vec<wire::Command>) -> (crate::wm::Scripts, Self) {
            if !commands
                .iter()
                .any(|command| matches!(command, wire::Command::Listen(_)))
            {
                commands.push(wire::Command::Listen(
                    wire::listen::listenable().map(str::to_owned).collect(),
                ));
            }
            let (ours, theirs) = UnixStream::pair().expect("a socket pair");
            theirs
                .set_read_timeout(Some(Duration::from_secs(10)))
                .expect("a timeout");
            let mut scripts = crate::wm::Scripts::over(
                Link::over(ours),
                commands,
                std::rc::Rc::default(),
                Path::new("/played/by/hand.lua"),
                false,
            );
            scripts.patient = false;
            (
                scripts,
                Self {
                    stream: theirs,
                    reader: frame::Reader::default(),
                },
            )
        }

        /// The next thing the compositor sent.
        pub(crate) fn read(&mut self) -> Message {
            self.reader
                .read_blocking(&mut &self.stream)
                .expect("a message from the compositor")
        }

        /// The serial of the next batch the compositor sent.
        pub(crate) fn batch(&mut self) -> u64 {
            match self.read() {
                Message::Batch(batch) => batch.serial,
                other => panic!("not a batch: {other:?}"),
            }
        }

        pub(crate) fn write(&mut self, message: &Message) {
            frame::write(&mut &self.stream, message).expect("written to the compositor");
        }

        /// Answer batch `serial` with `commands`.
        pub(crate) fn answer(&mut self, serial: u64, handled: bool, commands: Vec<wire::Command>) {
            self.write(&Message::Answer(wire::Answer {
                serial,
                handled,
                commands,
                motion: None,
                decision: None,
            }));
        }
    }
}
