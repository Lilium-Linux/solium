//! The event loop, woken by what the window manager writes: an answer that
//! came after the compositor stopped waiting for it is applied as soon as it
//! is there, not when something else next turns the loop.

use std::cell::Cell;
use std::os::fd::OwnedFd;
use std::rc::Rc;

use smithay::reexports::calloop::{
    Interest, LoopHandle, Mode, PostAction, RegistrationToken, generic::Generic,
};

use crate::state::Solium;

/// Keeps one window manager's socket in the event loop: the one it was last
/// given, in place of the one before.
pub(crate) struct Watch(Box<dyn FnMut(Option<OwnedFd>)>);

impl std::fmt::Debug for Watch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Watch")
    }
}

impl Watch {
    /// A watch in `handle`'s loop, where `compositor` finds the compositor
    /// in the loop's data.
    pub(crate) fn new<D: 'static>(
        handle: LoopHandle<'static, D>,
        compositor: fn(&mut D) -> &mut Solium,
    ) -> Self {
        let watched: Rc<Cell<Option<RegistrationToken>>> = Rc::default();
        Self(Box::new(move |socket| {
            if let Some(token) = watched.take() {
                handle.remove(token);
            }
            let Some(socket) = socket else {
                return;
            };
            let source = Generic::new(socket, Interest::READ, Mode::Level);
            let unwatched = Rc::clone(&watched);
            match handle.insert_source(source, move |_, _, data| {
                let state = compositor(data);
                if heard(state) {
                    Ok(PostAction::Continue)
                } else {
                    // A closed socket is readable for ever. What takes its
                    // place is watched when it is started, in `settle`.
                    unwatched.take();
                    Ok(PostAction::Remove)
                }
            }) {
                Ok(token) => watched.set(Some(token)),
                Err(err) => tracing::warn!(
                    %err,
                    "the window manager's socket is not in the event loop: a late answer \
                     waits for the loop's next turn"
                ),
            }
        }))
    }

    /// Watch `socket`, or nothing.
    pub(crate) fn on(&mut self, socket: Option<OwnedFd>) {
        (self.0)(socket);
    }
}

/// Read what the window manager has sent since it was last read, and apply
/// the answers nobody was waiting for: whether it is still there to read.
fn heard(state: &mut Solium) -> bool {
    let Some(scripts) = state.scripts.as_mut() else {
        return false;
    };
    scripts.hear_ready();
    let there = !scripts.down();
    state.apply_late();
    there
}

/// Once each time round the event loop: what the window manager has sent,
/// read and applied, and a window manager that has gone, or has said
/// nothing for too long with a batch waiting, started again
/// ([`Solium::restart_wm`]). Here and not where it is found out, which may
/// be in the middle of applying an answer.
pub(crate) fn settle(state: &mut Solium) {
    let Some(scripts) = state.scripts.as_mut() else {
        return;
    };
    scripts.hear_ready();
    scripts.stalled(std::time::Instant::now());
    if scripts.down() {
        state.restart_wm();
    }
    state.apply_late();
}
