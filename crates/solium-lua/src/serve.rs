//! The window manager's end of the socket: `hello` answered with what the
//! configuration asked for as it loaded, then each batch with what its
//! listeners asked for, and each reload with what the configuration asked
//! for as it loaded again. A handler's question to the compositor is a
//! `query` on the same socket, its `reply` read back before the handler goes
//! on.

use std::cell::{Cell, RefCell};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context, Result, anyhow, bail};
use solium_wm_protocol::frame::{self, FrameError};
use solium_wm_protocol::{self as wire, Message};

use crate::{Ask, Host, Setup};

/// The socket to the compositor, shared by the loop and by the questions a
/// handler asks while the loop is waiting on it.
#[derive(Debug)]
struct Connection {
    stream: UnixStream,
    reader: RefCell<frame::Reader>,
    /// The last query's serial.
    queries: Cell<u64>,
}

impl Connection {
    fn send(&self, message: &Message) -> Result<()> {
        frame::write(&mut &self.stream, message).context("writing to the compositor")
    }

    fn receive(&self) -> Result<Message, FrameError> {
        self.reader.borrow_mut().read_blocking(&mut &self.stream)
    }
}

/// A handler's questions, asked over the socket.
struct Asks(Rc<Connection>);

impl Ask for Asks {
    fn ask(&self, query: wire::Query) -> Option<wire::Reply> {
        let serial = self.0.queries.get() + 1;
        self.0.queries.set(serial);
        if let Err(err) = self.0.send(&Message::Query((serial, query))) {
            tracing::warn!(%err, "a question to the compositor could not be asked");
            return None;
        }
        loop {
            match self.0.receive() {
                Ok(Message::Reply((answered, reply))) if answered == serial => return Some(reply),
                Ok(other) => tracing::warn!(
                    ?other,
                    "the compositor sent something else while a question waited; left out"
                ),
                Err(err) => {
                    tracing::warn!(%err, "a question to the compositor went unanswered");
                    return None;
                }
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

/// The Lua window manager on its socket.
#[derive(Debug)]
pub struct Wm {
    host: Host,
    connection: Rc<Connection>,
}

impl Wm {
    /// Read the compositor's `hello`, load the configuration at `config`
    /// with the shipped Lua at `shipped`, and answer: `hello-answer` with
    /// what it asked for as it loaded, or `goodbye` with why it would not
    /// load, which is then the error.
    pub fn start(stream: UnixStream, config: &Path, shipped: PathBuf) -> Result<Self> {
        let connection = Rc::new(Connection {
            stream,
            reader: RefCell::new(frame::Reader::default()),
            queries: Cell::new(0),
        });
        let hello = match connection.receive() {
            Ok(Message::Hello(hello)) => hello,
            Ok(other) => bail!("the compositor's first message was not `hello`: {other:?}"),
            Err(err) => bail!("no `hello` from the compositor: {err}"),
        };
        if !compatible(wire::VERSION, &hello.version) {
            let why = format!(
                "this window manager speaks the protocol {}, the compositor {}",
                wire::VERSION,
                hello.version
            );
            let _ = connection.send(&Message::Goodbye(why.clone()));
            bail!(why);
        }
        let setup = Setup {
            shipped,
            debug: hello.debug,
        };
        match Host::load(config, setup, Box::new(Asks(Rc::clone(&connection)))) {
            Ok((host, loaded)) => {
                connection.send(&Message::HelloAnswer(wire::HelloAnswer {
                    version: wire::VERSION.to_owned(),
                    commands: loaded.commands,
                }))?;
                Ok(Self { host, connection })
            }
            Err(err) => {
                let _ = connection.send(&Message::Goodbye(format!("{err:#}")));
                Err(err)
            }
        }
    }

    /// Answer the compositor's next message. `false` once it has said
    /// goodbye or closed the socket.
    pub fn step(&mut self) -> Result<bool> {
        match self.connection.receive() {
            Ok(Message::Batch(batch)) => {
                let answer = self.host.handle(batch, self.asks());
                self.connection.send(&Message::Answer(answer))?;
                Ok(true)
            }
            Ok(Message::Reload(path)) => {
                let reloaded = match self.host.reload(Path::new(&path), self.asks()) {
                    Ok(loaded) => wire::Reloaded::Loaded(loaded),
                    Err(err) => wire::Reloaded::Failed(format!("{err:#}")),
                };
                self.connection.send(&Message::Reloaded(reloaded))?;
                Ok(true)
            }
            Ok(Message::Goodbye(_)) | Err(FrameError::Closed) => Ok(false),
            Ok(other) => {
                tracing::warn!(
                    ?other,
                    "a message the window manager does not take; left out"
                );
                Ok(true)
            }
            Err(err) => Err(anyhow!("the connection to the compositor failed: {err}")),
        }
    }

    /// The Lua host, for a test to look into.
    pub fn host(&self) -> &Host {
        &self.host
    }

    /// Questions to the compositor, over the socket.
    pub fn asks(&self) -> Box<dyn Ask> {
        Box::new(Asks(Rc::clone(&self.connection)))
    }
}

/// Run the window manager on `stream` until the compositor says goodbye.
pub fn serve(stream: UnixStream, config: &Path, shipped: PathBuf) -> Result<()> {
    let mut wm = Wm::start(stream, config, shipped)?;
    while wm.step()? {}
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::compatible;

    #[test]
    fn before_one_point_oh_a_minor_version_is_another_protocol() {
        assert!(compatible("0.1.0", "0.1.3"));
        assert!(!compatible("0.1.0", "0.2.0"));
        assert!(compatible("1.2.0", "1.7.1"));
        assert!(!compatible("1.0.0", "2.0.0"));
        assert!(!compatible("0.1.0", "nonsense"));
    }
}
