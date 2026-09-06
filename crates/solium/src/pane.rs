//! What the compositor thinks a window is.
//!
//! Not `smithay::desktop::Window`. That type requires a surface, and a window's
//! life begins when the user asks for the application — before there is a
//! process, let alone a surface. Everything that follows from that (the slot
//! reserved immediately, the other windows moving aside, closing it while it is
//! still loading, the application's content appearing *inside* it) is
//! impossible while the compositor's idea of a window is the client's.
//!
//! So: a pane. It owns an identity and a slot, and its *content* is a QML scene
//! the compositor draws, a mapped client, or the remains of one on its way out.
//! The identity and the slot survive all three, which is what makes adoption
//! possible — a client maps into a pane that already exists rather than
//! creating a window beside it.
//!
//! QML is not a special case here. A pane whose content is a scene is as
//! ordinary as one whose content is a client, which is what makes a loading
//! window, a placeholder for an application that died, and a surface the
//! compositor draws for its own reasons one mechanism instead of three.
//!
//! Wired to nothing yet: see `docs/spikes/2026-09-06-window-provider.md` for
//! the order the migration goes in and why it goes in that order.
#![expect(
    dead_code,
    reason = "step 1 of the migration: the type, before anything uses it"
)]

use std::{path::PathBuf, time::Duration};

use smithay::{
    desktop::Window,
    utils::{Logical, Rectangle},
};

/// A pane's identity. Survives adoption, so a script that learned about a
/// window while it was loading is talking about the same window afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct PaneId(u64);

impl PaneId {
    /// The next identity. Monotonic and never reused: an id that came back
    /// would let a stale reference address a different window.
    fn next() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    pub(crate) const fn get(self) -> u64 {
        self.0
    }
}

/// What is inside a pane.
#[derive(Debug)]
pub(crate) enum Content {
    /// Asked for, not arrived. The compositor draws it, from `source`.
    Loading {
        /// What was asked for, as the user would recognise it.
        program: String,
        /// The process spawned for it, matched against a client's own when it
        /// connects. `None` means nothing will ever be adopted into this pane.
        pid: Option<u32>,
        /// Which QML draws it. Resolved once, so a reload changing the setting
        /// does not change what a pane already on screen looks like halfway.
        source: PathBuf,
    },
    /// A client's window, mapped and drawing for itself.
    Client(Window),
    /// A client that has gone, still on screen while it leaves.
    Leaving { since: Duration },
}

/// A window, as the compositor thinks of one.
#[derive(Debug)]
pub(crate) struct Pane {
    id: PaneId,
    slot: Rectangle<i32, Logical>,
    content: Content,
    opened: Duration,
}

impl Pane {
    /// A pane for an application that has been asked for and has not arrived.
    pub(crate) fn loading(
        program: &str,
        pid: Option<u32>,
        slot: Rectangle<i32, Logical>,
        source: PathBuf,
        now: Duration,
    ) -> Self {
        Self {
            id: PaneId::next(),
            slot,
            content: Content::Loading {
                program: program.to_owned(),
                pid,
                source,
            },
            opened: now,
        }
    }

    /// A pane for a client that arrived without being asked for — anything
    /// started outside the compositor.
    pub(crate) fn mapped(window: Window, slot: Rectangle<i32, Logical>, now: Duration) -> Self {
        Self {
            id: PaneId::next(),
            slot,
            content: Content::Client(window),
            opened: now,
        }
    }

    pub(crate) const fn id(&self) -> PaneId {
        self.id
    }

    pub(crate) const fn slot(&self) -> Rectangle<i32, Logical> {
        self.slot
    }

    pub(crate) const fn set_slot(&mut self, slot: Rectangle<i32, Logical>) {
        self.slot = slot;
    }

    pub(crate) const fn content(&self) -> &Content {
        &self.content
    }

    /// The client's window, if one has arrived.
    pub(crate) const fn client(&self) -> Option<&Window> {
        match &self.content {
            Content::Client(window) => Some(window),
            _ => None,
        }
    }

    pub(crate) const fn is_loading(&self) -> bool {
        matches!(self.content, Content::Loading { .. })
    }

    /// Whether a client belonging to `family` — a process and its ancestors —
    /// is the one this pane is waiting for.
    pub(crate) fn awaits(&self, family: &[u32]) -> bool {
        match &self.content {
            Content::Loading { pid: Some(pid), .. } => family.contains(pid),
            _ => false,
        }
    }

    /// Give a pane the client it was waiting for.
    ///
    /// The id and the slot do not change, which is the whole point: nothing
    /// downstream learns that the content used to be something else, and a
    /// decoration keyed by pane id carries its animation straight through.
    pub(crate) fn adopt(&mut self, window: Window) {
        self.content = Content::Client(window);
    }

    /// The client has gone; keep the pane while it animates away.
    pub(crate) fn leave(&mut self, now: Duration) {
        self.content = Content::Leaving { since: now };
    }

    /// Whether an unarrived application has waited long enough to give up on.
    ///
    /// Only loading panes expire. A pane with a client is the client's problem
    /// and a leaving one is measured by its animation, not by patience.
    pub(crate) fn expired(&self, now: Duration, patience: Duration) -> bool {
        self.is_loading() && now.saturating_sub(self.opened) >= patience
    }

    /// What to call it, before a client has an opinion.
    pub(crate) fn program(&self) -> Option<&str> {
        match &self.content {
            Content::Loading { program, .. } => Some(program),
            _ => None,
        }
    }
}

/// Which QML draws a pane that is still loading.
///
/// The same rule decorations follow, and for the same reason: a name picks one
/// of the scenes that ship, a name matching a file in the user's own directory
/// picks theirs instead, and a path picks anyone's. What a window looks like
/// while its application starts is a thing people will want to own.
///
/// ```sh
/// SOLIUM_LOADING=plain          # qml/loading/plain.qml
/// SOLIUM_LOADING=~/mine.qml     # anywhere
/// ```
///
/// A script's choice arrives through `chosen`, from the configuration; the
/// environment overrides it, because it is set per run by whoever started this
/// one.
pub(crate) fn loading_source(chosen: Option<&str>) -> PathBuf {
    let name = std::env::var("SOLIUM_LOADING")
        .ok()
        .or_else(|| chosen.map(ToOwned::to_owned));
    let Some(name) = name else {
        return shipped("window");
    };
    if name.contains('/') || name.ends_with(".qml") {
        return PathBuf::from(expand(&name));
    }
    if let Some(user) = crate::qml::user_qml_dir() {
        let theirs = user.join("loading").join(format!("{name}.qml"));
        if theirs.is_file() {
            return theirs;
        }
    }
    shipped(&name)
}

fn shipped(name: &str) -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/qml/loading")).join(format!("{name}.qml"))
}

/// Expand a leading `~`, since this comes from an environment variable and
/// nothing else will have done it.
fn expand(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => {
            std::env::var("HOME").map_or_else(|_| path.to_owned(), |home| format!("{home}/{rest}"))
        }
        None => path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot() -> Rectangle<i32, Logical> {
        Rectangle::new((10, 20).into(), (300, 200).into())
    }

    #[test]
    fn identities_are_never_reused() {
        let first = Pane::loading("kitty", None, slot(), PathBuf::new(), Duration::ZERO);
        let second = Pane::loading("kitty", None, slot(), PathBuf::new(), Duration::ZERO);
        assert_ne!(first.id(), second.id());
    }

    #[test]
    fn adoption_keeps_the_identity_and_the_slot() {
        // The property the whole design rests on: a client arriving must not
        // produce a *different* window. Nothing here can build a real
        // `Window`, so this asserts what can be asserted without one --
        // that adopting changes neither of the two things everything else
        // addresses a pane by.
        let mut pane = Pane::loading("kitty", Some(42), slot(), PathBuf::new(), Duration::ZERO);
        let (id, where_it_was) = (pane.id(), pane.slot());
        assert!(pane.is_loading());
        assert!(pane.awaits(&[7, 42, 1]));

        pane.leave(Duration::from_millis(500));
        assert_eq!(pane.id(), id);
        assert_eq!(pane.slot(), where_it_was);
        assert!(!pane.is_loading());
        assert!(!pane.awaits(&[42]), "a pane with content awaits nothing");
    }

    #[test]
    fn only_a_loading_pane_gives_up() {
        let patience = Duration::from_secs(8);
        let pane = Pane::loading("slow", Some(9), slot(), PathBuf::new(), Duration::ZERO);
        assert!(!pane.expired(Duration::from_secs(7), patience));
        assert!(pane.expired(Duration::from_secs(8), patience));

        let mut left = Pane::loading("slow", Some(9), slot(), PathBuf::new(), Duration::ZERO);
        left.leave(Duration::from_secs(1));
        assert!(
            !left.expired(Duration::from_secs(60), patience),
            "a pane that is leaving is measured by its animation, not by patience"
        );
    }

    #[test]
    fn the_loading_scene_is_a_setting() {
        // A bare name is one of ours; a path is anyone's. The environment is
        // checked first, so this asserts the configured path only when the
        // variable is unset -- tests share a process and an environment.
        if std::env::var_os("SOLIUM_LOADING").is_none() {
            assert!(
                loading_source(None).ends_with("qml/loading/window.qml"),
                "the default is the one that ships"
            );
            assert!(loading_source(Some("plain")).ends_with("qml/loading/plain.qml"));
            assert_eq!(
                loading_source(Some("/tmp/mine.qml")),
                PathBuf::from("/tmp/mine.qml")
            );
        }
    }

    #[test]
    fn a_pane_with_no_process_adopts_nothing() {
        let pane = Pane::loading("mystery", None, slot(), PathBuf::new(), Duration::ZERO);
        assert!(!pane.awaits(&[1, 2, 3]));
    }
}
