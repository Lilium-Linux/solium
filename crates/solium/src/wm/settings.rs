//! How long the compositor waits for the window manager, and what it does
//! about one that has stopped.

use std::time::Duration;

/// The `wm` settings section, read in [`crate::script::settings::wm`]: how
/// long each kind of message is waited for, and how often a window manager
/// that stopped is started again. Each holds its default until the window
/// manager's answer to `hello` sets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Settings {
    /// An event's answer. One frame at 260 Hz by default, so a slow window
    /// manager costs a frame and never more.
    pub(crate) ask: Duration,
    /// The hard deadline: a batch unanswered this long is a window manager
    /// that has stopped answering. Above what one of its own listeners may
    /// take, which it stops itself.
    pub(crate) hard: Duration,
    /// A configuration loading, or loading again.
    pub(crate) load: Duration,
    /// How many hard deadlines running the window manager may answer
    /// nothing in, while a batch waits, before it has stopped.
    pub(crate) strikes: u32,
    /// How many times a window manager that stopped is started again
    /// within [`Self::restart_window`], before something else is.
    pub(crate) restarts: u32,
    pub(crate) restart_window: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ask: Duration::from_millis(4),
            hard: Duration::from_secs(1),
            load: Duration::from_secs(10),
            strikes: 3,
            restarts: 3,
            restart_window: Duration::from_secs(60),
        }
    }
}

/// How an event is waited for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Wait {
    /// [`Settings::ask`], when the window manager has answered everything
    /// before it; not at all while it is behind.
    Frame,
    /// The last word before the window manager is replaced or the
    /// compositor ends, which nothing comes after: [`Settings::hard`].
    Last,
}

/// What is started in place of a window manager that stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Start {
    /// The same again.
    Again,
    /// The shipped configuration, reading nothing of the user's.
    Shipped,
    /// Nothing: the shipped configuration stopped as often.
    Nothing,
}

/// When the window manager stopped, of late.
#[derive(Debug, Default)]
pub(crate) struct Restarts(std::collections::VecDeque<std::time::Instant>);

impl Restarts {
    /// The window manager stopped at `now`: what is started in its place.
    /// It is started again up to `restarts` times in `restart_window`; one
    /// stop more and the shipped configuration takes over, with the count
    /// begun again, and when that was what stopped (`shipped_only`),
    /// nothing does.
    pub(crate) fn stopped(
        &mut self,
        now: std::time::Instant,
        settings: &Settings,
        shipped_only: bool,
    ) -> Start {
        self.0
            .retain(|at| now.saturating_duration_since(*at) < settings.restart_window);
        self.0.push_back(now);
        if u32::try_from(self.0.len()).is_ok_and(|stops| stops <= settings.restarts) {
            return Start::Again;
        }
        self.0.clear();
        if shipped_only {
            Start::Nothing
        } else {
            Start::Shipped
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{Restarts, Settings, Start};

    #[test]
    fn a_window_manager_that_keeps_stopping_gives_way_to_the_shipped_one_and_that_to_nothing() {
        let settings = Settings::default();
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let mut restarts = Restarts::default();
        for second in 0..3 {
            assert_eq!(restarts.stopped(at(second), &settings, false), Start::Again);
        }
        assert_eq!(restarts.stopped(at(3), &settings, false), Start::Shipped);
        for second in 4..7 {
            assert_eq!(restarts.stopped(at(second), &settings, true), Start::Again);
        }
        assert_eq!(restarts.stopped(at(7), &settings, true), Start::Nothing);
    }

    #[test]
    fn stops_further_apart_than_the_window_are_each_the_first() {
        let settings = Settings::default();
        let start = Instant::now();
        let mut restarts = Restarts::default();
        for minutes in 0..10 {
            let now = start + Duration::from_secs(61 * minutes);
            assert_eq!(restarts.stopped(now, &settings, false), Start::Again);
        }
    }
}
