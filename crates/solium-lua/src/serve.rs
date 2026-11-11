//! The window manager's end of the socket: `hello` answered with what the
//! configuration asked for as it loaded, then each batch with what its
//! listeners asked for, and each reload with what the configuration asked
//! for as it loaded again. A handler's question to the compositor is a
//! `query` on the same socket, its `reply` read back before the handler goes
//! on.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Result, anyhow};
use solium_wm_protocol as wire;
use solium_wm_protocol::client::{Client, Heard};

use crate::{Ask, Host, Setup};

/// A handler's questions, asked over the socket.
struct Asks(Rc<Client>);

impl Ask for Asks {
    fn ask(&self, query: wire::Query) -> Option<wire::Reply> {
        match self.0.query(query) {
            Ok(reply) => Some(reply),
            Err(err) => {
                tracing::warn!(%err, "a question to the compositor went unanswered");
                None
            }
        }
    }
}

/// The Lua window manager on its socket.
#[derive(Debug)]
pub struct Wm {
    host: Host,
    client: Rc<Client>,
}

impl Wm {
    /// Read the compositor's `hello`, load the configuration at `config`
    /// with the shipped Lua at `shipped` and the user's own at `user`, and
    /// answer: `hello-answer` with what it asked for as it loaded, or
    /// `goodbye` with why it would not load, which is then the error. What
    /// `hello` hands back of a window manager this one takes the place of
    /// is what `sol.keep` answers with.
    pub fn start(
        stream: UnixStream,
        config: &Path,
        shipped: PathBuf,
        user: Option<PathBuf>,
    ) -> Result<Self> {
        let client = Rc::new(Client::over(stream));
        let hello = client.hello()?;
        let setup = Setup {
            shipped,
            user,
            debug: hello.debug,
        };
        let carried = crate::Keep::from_wire(&hello.keeps);
        match Host::load(config, setup, carried, Box::new(Asks(Rc::clone(&client)))) {
            Ok((host, loaded)) => {
                client.ready(loaded.commands)?;
                Ok(Self { host, client })
            }
            Err(err) => {
                let _ = client.refuse(&format!("{err:#}"));
                Err(err)
            }
        }
    }

    /// Answer the compositor's next message. `false` once it has said
    /// goodbye or closed the socket.
    pub fn step(&mut self) -> Result<bool> {
        match self.client.next() {
            Ok(Heard::Batch(batch)) => {
                let answer = self.host.handle(batch, self.asks());
                self.client.answer(answer)?;
                Ok(true)
            }
            Ok(Heard::Reload(path)) => {
                let reloaded = match self.host.reload(Path::new(&path), self.asks()) {
                    Ok(loaded) => wire::Reloaded::Loaded(loaded),
                    Err(err) => wire::Reloaded::Failed(format!("{err:#}")),
                };
                self.client.reloaded(reloaded)?;
                Ok(true)
            }
            Ok(Heard::Goodbye(_)) => Ok(false),
            Err(err) => Err(anyhow!("the connection to the compositor failed: {err}")),
        }
    }

    /// The Lua host, for a test to look into.
    pub fn host(&self) -> &Host {
        &self.host
    }

    /// Questions to the compositor, over the socket.
    pub fn asks(&self) -> Box<dyn Ask> {
        Box::new(Asks(Rc::clone(&self.client)))
    }
}

/// Run the window manager on `stream` until the compositor says goodbye.
pub fn serve(
    stream: UnixStream,
    config: &Path,
    shipped: PathBuf,
    user: Option<PathBuf>,
) -> Result<()> {
    let mut wm = Wm::start(stream, config, shipped, user)?;
    while wm.step()? {}
    Ok(())
}
