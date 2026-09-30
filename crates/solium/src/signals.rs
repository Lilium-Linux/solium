//! Ending cleanly when a signal asks (#146).
//!
//! logind ends a session with SIGTERM, and `kill` and `timeout` send it too;
//! a terminal sends SIGINT and SIGHUP. Unhandled, each ends the process where
//! it stands: `Session::end` never runs, so `solium-session.target` and the
//! exported environment outlive the compositor, and no destructor runs
//! either. Qt installs no handler of its own here (`QT_QPA_NO_SIGNAL_HANDLER`,
//! in `qml.rs`).
//!
//! So both backends stop their event loop on any of the three and leave the
//! way they leave for a key: `each_ending_signal_stops_the_loop`. The
//! handlers only write to a socket (async-signal, over signal-hook-registry),
//! and the loop reads it. Handlers rather than a blocked mask read through a
//! signalfd: a mask is inherited by every program Solium starts, XWayland
//! included, and would have them ignore the same signals, while a handler is
//! reset by `exec`. `a_program_solium_starts_still_dies_of_sigterm`.

use std::{pin::Pin, task::Poll};

use async_signal::{Signal, Signals};
use futures_core::Stream as _;
use smithay::reexports::calloop::{
    Interest, LoopHandle, Mode, PostAction,
    generic::{Generic, NoIoDrop},
};

/// The signals that end a session.
const ENDING: [Signal; 3] = [Signal::Term, Signal::Int, Signal::Hup];

/// Call `stop` on the event loop whenever SIGTERM, SIGINT or SIGHUP arrives.
///
/// Not being able to listen is a warning: the session runs, and a signal
/// ends it the old way.
pub(crate) fn listen<D: 'static>(
    handle: &LoopHandle<'static, D>,
    mut stop: impl FnMut(&mut D) + 'static,
) {
    let signals = match Signals::new(ENDING) {
        Ok(signals) => signals,
        Err(err) => {
            tracing::warn!(
                ?err,
                "cannot listen for SIGTERM, SIGINT and SIGHUP: one of them ends this \
                 session without stopping solium-session.target"
            );
            return;
        }
    };
    let inserted = handle.insert_source(
        Generic::new(signals, Interest::READ, Mode::Level),
        move |_, signals: &mut NoIoDrop<Signals>, data| {
            let mut context = std::task::Context::from_waker(std::task::Waker::noop());
            let mut signals: &Signals = signals;
            while let Poll::Ready(Some(heard)) = Pin::new(&mut signals).poll_next(&mut context) {
                match heard {
                    Ok(signal) => {
                        tracing::info!(?signal, "asked to end by a signal: stopping");
                        stop(data);
                    }
                    Err(err) => {
                        tracing::warn!(?err, "reading a signal");
                        break;
                    }
                }
            }
            Ok(PostAction::Continue)
        },
    );
    if let Err(err) = inserted {
        tracing::warn!(?err, "cannot watch for SIGTERM, SIGINT and SIGHUP");
    }
}

#[cfg(test)]
mod tests {
    use std::{os::unix::process::ExitStatusExt as _, time::Duration};

    use smithay::reexports::{
        calloop::EventLoop,
        rustix::process::{Signal as Raw, getpid, kill_process},
    };

    use super::*;

    /// Each of the three reaches the loop, once each, and the process is
    /// still here to see it.
    #[test]
    fn each_ending_signal_stops_the_loop() {
        let mut event_loop: EventLoop<u32> = EventLoop::try_new().expect("an event loop");
        listen(&event_loop.handle(), |stops: &mut u32| *stops += 1);

        let mut stops = 0;
        for (sent, raw) in [Raw::TERM, Raw::INT, Raw::HUP].into_iter().enumerate() {
            kill_process(getpid(), raw).expect("signalling this process");
            let expected = u32::try_from(sent).expect("three") + 1;
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while stops < expected && std::time::Instant::now() < deadline {
                event_loop
                    .dispatch(Some(Duration::from_millis(50)), &mut stops)
                    .expect("dispatching");
            }
            assert_eq!(stops, expected, "{raw:?}");
        }
    }

    /// The handler is Solium's alone: a program it starts has the default
    /// back, and SIGTERM still ends it.
    #[test]
    fn a_program_solium_starts_still_dies_of_sigterm() {
        let event_loop: EventLoop<()> = EventLoop::try_new().expect("an event loop");
        listen(&event_loop.handle(), |()| {});
        let status = std::process::Command::new("sh")
            .args(["-c", "kill -s TERM $$; sleep 5"])
            .status()
            .expect("running sh");
        assert_eq!(status.signal(), Some(15), "{status:?}");
    }
}
