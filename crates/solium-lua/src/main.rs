//! `solium-lua`: Solium's Lua window manager, as the compositor starts it.
//!
//! `solium-lua [<configuration>]`, with the socket to the compositor as its
//! standard input. The compositor starts it with no arguments, and it finds
//! its own configuration (`found.rs`: `SOLIUM_LUA_INIT`, the user's
//! `init.lua`, or the shipped one); a `wm.toml` that names it can name the
//! file to load instead. It answers `hello`, then every batch, until the
//! compositor says goodbye. Its log goes to its
//! standard error, which is the compositor's. Started safe (`hello.safe`),
//! it reads none of the user's own: how the compositor starts it in place of
//! a window manager that keeps stopping.

use std::io::IsTerminal;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        // Plain into the compositor's log, as its own lines are.
        .with_ansi(std::io::stderr().is_terminal())
        .init();
    let mut arguments = std::env::args_os().skip(1);
    let config = arguments.next().map(PathBuf::from);
    if arguments.next().is_some() {
        eprintln!(
            "usage: solium-lua [<configuration>], started by solium with the socket as its \
             standard input"
        );
        return ExitCode::from(2);
    }
    let stream = match std::io::stdin()
        .as_fd()
        .try_clone_to_owned()
        .map(UnixStream::from)
    {
        // A socket has a peer; a terminal or a file does not.
        Ok(stream) if stream.peer_addr().is_ok() => stream,
        _ => {
            eprintln!("solium-lua: standard input is not the socket to the compositor");
            return ExitCode::from(2);
        }
    };
    match solium_lua::serve(stream, config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!("{err:#}");
            ExitCode::FAILURE
        }
    }
}
