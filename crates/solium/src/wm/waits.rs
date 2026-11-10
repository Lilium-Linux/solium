//! How long the compositor waits for the window manager.

use std::time::Duration;

/// How long each kind of message is waited for: the `wm` settings section,
/// read in [`crate::script::settings::wm`]. Each holds its default until the
/// window manager's answer to `hello` sets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Waits {
    /// An event's answer. One frame at 260 Hz by default, so a slow window
    /// manager costs a frame and never more.
    pub(crate) ask: Duration,
    /// The hard deadline: a batch unanswered this long is a window manager
    /// that has stopped answering. Above what one of its own listeners may
    /// take, which it stops itself.
    pub(crate) hard: Duration,
    /// A configuration loading, or loading again.
    pub(crate) load: Duration,
}

impl Default for Waits {
    fn default() -> Self {
        Self {
            ask: Duration::from_millis(4),
            hard: Duration::from_secs(1),
            load: Duration::from_secs(10),
        }
    }
}

/// How an event is waited for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Wait {
    /// [`Waits::ask`], when the window manager has answered everything
    /// before it; not at all while it is behind.
    Frame,
    /// The last word before the window manager is replaced or the
    /// compositor ends, which nothing comes after: [`Waits::hard`].
    Last,
}
