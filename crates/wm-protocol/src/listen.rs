//! Which events a window manager hears.
//!
//! A window manager is sent only the events it listens to, named in its
//! `listen` command, so that one with no use for every motion of a dragged
//! window is never woken for it, and never has to answer it. Until it has
//! said, it listens to none.
//!
//! A few events are sent whatever it listens to, because each is the answer
//! to something the window manager itself asked for, or the moment it
//! starts: [`ALWAYS`].
//!
//! What changed on the desktop comes with a batch, so a window manager's
//! picture of the desktop is as fresh as the last event it was sent: one
//! that wants to know of every window listens to `opened` and `closed`.

use crate::Event;

/// The events sent whether or not they are listened to: the start, a key
/// combination the window manager bound, and what became of what it asked
/// for (a notification of its own, an action it sent a scene, a program, a
/// timer, a file).
pub const ALWAYS: &[&str] = &[
    "startup",
    "key",
    "notify-forget",
    "notify-action",
    "attempts-settled",
    "runs",
    "timers",
    "files",
];

/// Every event a window manager may listen to, by the name it is listened
/// to by: its case in the WIT's `event`.
pub fn listenable() -> impl Iterator<Item = &'static str> {
    Event::CASES
        .iter()
        .copied()
        .filter(|name| !ALWAYS.contains(name))
}

impl Event {
    /// Whether this event is sent whatever the window manager listens to.
    pub fn always(&self) -> bool {
        ALWAYS.contains(&self.case())
    }
}

#[cfg(test)]
mod tests {
    use super::{ALWAYS, listenable};
    use crate::Event;

    /// Every name in the list is an event's, so none is misspelt, and
    /// between them the two lists are every event once.
    #[test]
    fn every_event_is_always_sent_or_listened_to() {
        for name in ALWAYS {
            assert!(Event::CASES.contains(name), "no event called {name}");
        }
        let listened: Vec<&str> = listenable().collect();
        assert_eq!(listened.len() + ALWAYS.len(), Event::CASES.len());
        assert!(listened.contains(&"opened") && listened.contains(&"dragged"));
        assert!(!listened.contains(&"key"));
    }

    #[test]
    fn an_event_says_its_name_and_whether_it_is_always_sent() {
        assert_eq!(Event::Key("super+t".to_owned()).case(), "key");
        assert!(Event::Key("super+t".to_owned()).always());
        assert_eq!(Event::MonitorsChanged.case(), "monitors-changed");
        assert!(!Event::MonitorsChanged.always());
        assert_eq!(Event::Closed(3).case(), "closed");
    }
}
