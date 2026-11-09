//! The compositor's end of the socket to the window manager.
//!
//! Each message to it is answered before the compositor goes on, under a
//! deadline: `hello` with `hello-answer`, a batch with its answer, a reload
//! with `reloaded`. A question the window manager asks on the way, a
//! `query`, is answered from the compositor's state as it is read.
//!
//! The window manager is the `solium-lua` program, the socket its standard
//! input. A test runs it on a thread of its own instead, over a socket pair
//! all the same, so every test of the configuration crosses the protocol.

use std::cell::RefCell;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use solium_wm_protocol::frame::{self, FrameError};
use solium_wm_protocol::{self as wire, Message};

use crate::script::lua::Ask;

/// How long a configuration may take to load, or to load again.
const LOADING: Duration = Duration::from_secs(10);
/// How long a batch may take to be answered. A listener is stopped by the
/// window manager after 100 ms of its own; this is for a window manager
/// that has stopped answering altogether.
const ANSWERING: Duration = Duration::from_secs(1);

/// The connection, and the window manager at its other end.
pub(crate) struct Link {
    stream: UnixStream,
    reader: RefCell<frame::Reader>,
    peer: Peer,
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
    /// for its `hello-answer`: what the configuration asked for as it
    /// loaded. Its questions while it loads go to `answers`.
    pub(crate) fn start(config: &Path, answers: &dyn Ask) -> Result<(Self, Vec<wire::Command>)> {
        let link = Self::open(config)?;
        let hello = Message::Hello(wire::Hello {
            version: wire::VERSION.to_owned(),
            debug: crate::dev::debug_mode(),
            keeps: Vec::new(),
        });
        match link.request(&hello, answers, LOADING)? {
            Message::HelloAnswer(answer) => {
                if !crate::script::lua::compatible(wire::VERSION, &answer.version) {
                    bail!(
                        "the window manager speaks the protocol {}, the compositor {}",
                        answer.version,
                        wire::VERSION
                    );
                }
                Ok((link, answer.commands))
            }
            Message::Goodbye(why) => Err(anyhow!(why)),
            other => bail!("the window manager answered `hello` with {other:?}"),
        }
    }

    /// The window manager's answer to `batch`.
    pub(crate) fn answer(&self, batch: wire::Batch, answers: &dyn Ask) -> Result<wire::Answer> {
        let serial = batch.serial;
        match self.request(&Message::Batch(batch), answers, ANSWERING)? {
            Message::Answer(answer) if answer.serial == serial => Ok(answer),
            other => bail!("the window manager answered batch {serial} with {other:?}"),
        }
    }

    /// Have the window manager load the configuration at `config` again,
    /// carrying what it kept: what it asked for as it loaded, or why it
    /// would not load, in which case the one that was running goes on.
    pub(crate) fn reload(&self, config: &Path, answers: &dyn Ask) -> Result<wire::Answer> {
        let path = config.to_string_lossy().into_owned();
        match self.request(&Message::Reload(path), answers, LOADING)? {
            Message::Reloaded(wire::Reloaded::Loaded(answer)) => Ok(answer),
            Message::Reloaded(wire::Reloaded::Failed(why)) => Err(anyhow!(why)),
            other => bail!("the window manager answered a reload with {other:?}"),
        }
    }

    /// The `solium-lua` process, its standard input the socket.
    #[cfg(not(test))]
    fn open(config: &Path) -> Result<Self> {
        let (ours, theirs) = UnixStream::pair().context("making the window manager's socket")?;
        let program = program().map_err(|why| anyhow!(why))?;
        use std::os::unix::process::CommandExt as _;
        let child = std::process::Command::new(&program)
            .arg(config)
            .arg(crate::assets::lua())
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
        tracing::info!(pid = child.id(), "window manager started");
        Ok(Self {
            stream: ours,
            reader: RefCell::new(frame::Reader::default()),
            peer: Peer::Process(RefCell::new(child)),
        })
    }

    /// Write `message`, and read until the window manager answers it,
    /// answering its questions on the way, for at most `within`.
    fn request(&self, message: &Message, answers: &dyn Ask, within: Duration) -> Result<Message> {
        frame::write(&mut &self.stream, message).context("writing to the window manager")?;
        if !matches!(message, Message::Hello(_)) {
            self.wake();
        }
        let until = Instant::now() + within;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                bail!("the window manager did not answer within {within:?}");
            }
            self.stream.set_read_timeout(Some(left))?;
            let read = self.reader.borrow_mut().read_blocking(&mut &self.stream);
            match read {
                Ok(Message::Query((serial, query))) => {
                    let reply = answers
                        .ask(query.clone())
                        .unwrap_or_else(|| unanswered(&query));
                    frame::write(&mut &self.stream, &Message::Reply((serial, reply)))
                        .context("answering the window manager")?;
                }
                Ok(other) => return Ok(other),
                Err(FrameError::Io(err))
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(err) => bail!("the connection to the window manager failed: {err}"),
            }
        }
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
                let _ = frame::write(
                    &mut &self.stream,
                    &Message::Goodbye("the compositor is closing".to_owned()),
                );
                let child = child.get_mut();
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
                // Its loop ends with the last message it will be told of.
                control.take();
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
    use std::cell::RefCell;
    use std::os::unix::net::UnixStream;
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::Duration;

    use anyhow::{Context, Result};
    use solium_wm_protocol::Message;
    use solium_wm_protocol::frame::{self, FrameError};

    use super::{Link, Peer, unanswered};
    use crate::script::lua::Ask;

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
        /// The window manager on a thread of its own, over a socket pair.
        pub(super) fn open(config: &Path) -> Result<Self> {
            let (ours, theirs) =
                UnixStream::pair().context("making the window manager's socket")?;
            let (control, heard) = mpsc::channel::<Control>();
            let config = config.to_owned();
            // What it logs as it loads goes to the log the test is reading.
            let log = tracing::dispatcher::get_default(Clone::clone);
            let thread = std::thread::Builder::new()
                .name("window manager".to_owned())
                .spawn(move || {
                    let started = tracing::dispatcher::with_default(&log, || {
                        solium_lua::Wm::start(theirs, &config, crate::assets::lua())
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
            Ok(Self {
                stream: ours,
                reader: RefCell::new(frame::Reader::default()),
                peer: Peer::Thread {
                    control: Some(control),
                    thread: Some(thread),
                },
            })
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
            let _ = self.stream.set_read_timeout(Some(Duration::from_millis(1)));
            loop {
                match result.try_recv() {
                    Ok(said) => return said,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        return "error: the window manager has gone".to_owned();
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                }
                let read = self.reader.borrow_mut().read_blocking(&mut &self.stream);
                match read {
                    Ok(Message::Query((serial, query))) => {
                        let reply = answers
                            .ask(query.clone())
                            .unwrap_or_else(|| unanswered(&query));
                        let _ = frame::write(&mut &self.stream, &Message::Reply((serial, reply)));
                    }
                    Ok(other) => panic!("the window manager sent {other:?} outside a request"),
                    Err(FrameError::Io(_)) => {}
                    Err(err) => return format!("error: {err}"),
                }
            }
        }
    }
}
