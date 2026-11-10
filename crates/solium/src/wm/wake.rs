//! The event loop, woken by what the window manager writes: an answer that
//! came after the compositor stopped waiting for it is applied as soon as it
//! is there, not when something else next turns the loop.

use std::os::fd::OwnedFd;

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
        let mut watched: Option<RegistrationToken> = None;
        Self(Box::new(move |socket| {
            if let Some(token) = watched.take() {
                handle.remove(token);
            }
            let Some(socket) = socket else {
                return;
            };
            let source = Generic::new(socket, Interest::READ, Mode::Level);
            match handle.insert_source(source, move |_, _, data| {
                settle(compositor(data));
                Ok(PostAction::Continue)
            }) {
                Ok(token) => watched = Some(token),
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
/// the answers nobody was waiting for.
pub(crate) fn settle(state: &mut Solium) {
    let Some(scripts) = state.scripts.as_mut() else {
        return;
    };
    scripts.hear_ready();
    if scripts.down() {
        // A closed socket is readable for ever.
        state.watch_wm();
    }
    state.apply_late();
}
