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
//! `Space` has not gone away and is not going to. It stays underneath as the
//! authority on stacking and damage for mapped clients, because that
//! bookkeeping is worth keeping and not worth rewriting. `Panes` is the
//! compositor's own view *over* it: everything the compositor decides — which
//! window a script means, what is drawn, what the pointer is over — asks here,
//! and only the mapped case asks `Space` anything.
//!
//! See `docs/spikes/2026-09-06-window-provider.md` for the order the migration
//! goes in and why it goes in that order.
#![expect(
    dead_code,
    reason = "steps 4 and 5 of the migration bring the loading half into use: \
              adopting a client by its process, drawing a pane that has none \
              yet, and keeping one on screen while it leaves. Per-item is not \
              an option -- dead_code reports a whole impl block at one span, \
              so an expect on one method cannot match it."
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
        /// The scene itself, hosted by us. `None` if it would not load: a
        /// window with nothing in it is worse than one with a scene, and much
        /// better than no window at all.
        scene: Option<Box<crate::surface::ShellSurface>>,
    },
    /// A client's window, mapped and drawing for itself.
    Client {
        window: Window,
        /// The scene this window was showing before its application arrived,
        /// kept until the application has actually drawn something.
        ///
        /// A client maps a good while before it paints. Dropping the scene the
        /// moment it maps leaves the window empty for exactly that long, which
        /// is the blank flash between the two that this whole design exists to
        /// remove. The two are never both absent: the scene goes when there is
        /// something to replace it with.
        scene: Option<Box<crate::surface::ShellSurface>>,
        /// When the application painted and the scene began to fade off it.
        ///
        /// `None` while the application still has nothing to show. The scene
        /// is drawn *over* the window for as long as this lasts, so the
        /// application is already there underneath as it goes — a dissolve
        /// rather than a cut.
        faded: Option<Duration>,
    },
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
    /// Whether a client mapped *into* this pane rather than creating it.
    ///
    /// The difference matters exactly once, on that client's first commit: an
    /// adopted pane already has a slot the layout gave it and a layout that
    /// was told it opened, so neither must happen a second time.
    adopted: bool,
    /// Where this pane is *drawn*, when that is not where it lives, and
    /// whether it has ever been on screen. See `present::Slot` — it is here
    /// rather than on the client's window so that it can exist before the
    /// window does, and survive the window arriving.
    drawn: crate::present::Slot,
}

impl Pane {
    /// A pane for an application that has been asked for and has not arrived.
    pub(crate) fn loading(
        program: &str,
        pid: Option<u32>,
        slot: Rectangle<i32, Logical>,
        source: PathBuf,
        scene: Option<crate::surface::ShellSurface>,
        now: Duration,
    ) -> Self {
        Self {
            id: PaneId::next(),
            slot,
            content: Content::Loading {
                program: program.to_owned(),
                pid,
                source,
                scene: scene.map(Box::new),
            },
            opened: now,
            adopted: false,
            drawn: crate::present::Slot::default(),
        }
    }

    /// A pane for a client that arrived without being asked for — anything
    /// started outside the compositor.
    pub(crate) fn mapped(window: Window, slot: Rectangle<i32, Logical>, now: Duration) -> Self {
        Self {
            id: PaneId::next(),
            slot,
            content: Content::Client {
                window,
                scene: None,
                faded: None,
            },
            opened: now,
            adopted: false,
            drawn: crate::present::Slot::default(),
        }
    }

    /// How this pane is being drawn. `present` is the only thing that reads it.
    pub(crate) const fn drawn(&self) -> &crate::present::Slot {
        &self.drawn
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
            Content::Client { window, .. } => Some(window),
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
        // The scene comes across with it. See `Content::Client::scene`: the
        // client has mapped and has not painted, and letting go of what is on
        // screen now would leave a hole for however long that takes.
        let scene = match &mut self.content {
            Content::Loading { scene, .. } => scene.take(),
            Content::Client { scene, .. } => scene.take(),
            Content::Leaving { .. } => None,
        };
        self.content = Content::Client {
            window,
            scene,
            faded: None,
        };
        self.adopted = true;
    }

    /// The application has painted: begin letting go of the scene over it.
    ///
    /// Returns whether this call started the fade, so it is set up once.
    pub(crate) fn fade(&mut self, now: Duration) -> bool {
        match &mut self.content {
            Content::Client {
                scene: Some(_),
                faded: faded @ None,
                ..
            } => {
                *faded = Some(now);
                true
            }
            _ => false,
        }
    }

    /// Whether the scene has been fading for at least `over`.
    pub(crate) fn faded(&self, now: Duration, over: Duration) -> bool {
        match &self.content {
            Content::Client {
                faded: Some(began), ..
            } => now.saturating_sub(*began) >= over,
            _ => false,
        }
    }

    /// How much of the scene is still there.
    ///
    /// Solid until the application has painted, then away over `over`. Eased
    /// rather than linear, so it holds for a moment and then goes: a linear
    /// dissolve spends its whole length looking like a wash over the window,
    /// which reads as something being wrong with the window.
    pub(crate) fn scene_alpha(&self, now: Duration, over: Duration) -> f32 {
        let Content::Client {
            faded: Some(began), ..
        } = &self.content
        else {
            return 1.0;
        };
        if over.is_zero() {
            return 0.0;
        }
        let progress = now.saturating_sub(*began).as_secs_f64() / over.as_secs_f64();
        let gone = solium_animation::Curve::InOutQuad.at(progress.clamp(0.0, 1.0));
        #[expect(
            clippy::cast_possible_truncation,
            reason = "an alpha, clamped to 0..=1 before it is narrowed"
        )]
        let alpha = (1.0 - gone).clamp(0.0, 1.0) as f32;
        alpha
    }

    /// The application has painted: let go of the scene standing in for it.
    ///
    /// Returns whether there was one, so the handover can be reported once.
    pub(crate) fn filled(&mut self) -> bool {
        match &mut self.content {
            Content::Client { scene, .. } => scene.take().is_some(),
            _ => false,
        }
    }

    /// Whether anything of ours is drawing this pane.
    pub(crate) const fn has_scene(&self) -> bool {
        match &self.content {
            Content::Loading { scene, .. } | Content::Client { scene, .. } => scene.is_some(),
            Content::Leaving { .. } => false,
        }
    }

    /// Whether a client mapped into this pane rather than creating it.
    pub(crate) const fn adopted(&self) -> bool {
        self.adopted
    }

    /// Whose process to wait for. Set after the fork, because the window went
    /// up before it — which is the point of the whole exercise.
    pub(crate) fn expect(&mut self, pid: u32) {
        if let Content::Loading { pid: waiting, .. } = &mut self.content {
            *waiting = Some(pid);
        }
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

    /// The scene drawing this pane, while it has no client to draw itself.
    pub(crate) fn scene_mut(&mut self) -> Option<&mut crate::surface::ShellSurface> {
        match &mut self.content {
            Content::Loading { scene, .. } | Content::Client { scene, .. } => scene.as_deref_mut(),
            Content::Leaving { .. } => None,
        }
    }

    /// How long this pane has been waiting for its application.
    pub(crate) fn waited(&self, now: Duration) -> Duration {
        now.saturating_sub(self.opened)
    }

    /// What to call it, before a client has an opinion.
    pub(crate) fn program(&self) -> Option<&str> {
        match &self.content {
            Content::Loading { program, .. } => Some(program),
            _ => None,
        }
    }
}

/// Every pane, bottom to top.
///
/// The same order `Space` stacks its elements in, because it is the order both
/// a hit test and a window list want — reversed, you are looking at what is on
/// top first, which is what "which window is this click for" means.
///
/// This is the compositor's own view. `Space` is still underneath and still the
/// authority on where a mapped client is and what damage it did; `sync` is the
/// one place the two are reconciled, so they cannot drift apart anywhere else.
#[derive(Debug, Default)]
pub(crate) struct Panes {
    panes: Vec<Pane>,
    /// Set when the list changed by some route other than `sync` — a pane
    /// opened, or one removed because its application never came.
    ///
    /// `sync` reports a change by comparing the list it starts with against
    /// the one it ends with, and everything keyed by a pane is tidied on that
    /// report. A change that happened before it started is invisible to that
    /// comparison, and a frame whose pane went that way would be kept forever.
    changed: bool,
}

impl Panes {
    /// Bottom to top. `.rev()` for a hit test.
    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = &Pane> {
        self.panes.iter()
    }

    pub(crate) fn len(&self) -> usize {
        self.panes.len()
    }

    pub(crate) fn get(&self, id: PaneId) -> Option<&Pane> {
        self.panes.iter().find(|pane| pane.id == id)
    }

    pub(crate) fn get_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.panes.iter_mut().find(|pane| pane.id == id)
    }

    /// The pane a script means by an id. Scripts hold the number rather than
    /// the type, because they got it through Lua.
    pub(crate) fn by_script_id(&self, id: u64) -> Option<&Pane> {
        self.panes.iter().find(|pane| pane.id.get() == id)
    }

    /// The pane a client's window is the content of.
    pub(crate) fn of(&self, window: &Window) -> Option<&Pane> {
        self.panes.iter().find(|pane| pane.client() == Some(window))
    }

    pub(crate) fn id_of(&self, window: &Window) -> Option<PaneId> {
        self.of(window).map(Pane::id)
    }

    /// The pane waiting for a client from this process family, if any.
    ///
    /// Topmost first, so the most recently asked-for window wins when someone
    /// has asked for the same program twice.
    pub(crate) fn awaiting(&self, family: &[u32]) -> Option<PaneId> {
        self.panes
            .iter()
            .rev()
            .find(|pane| pane.awaits(family))
            .map(Pane::id)
    }

    /// Open a pane, on top. The window's life begins here.
    pub(crate) fn open(&mut self, pane: Pane) -> PaneId {
        let id = pane.id;
        self.panes.push(pane);
        self.changed = true;
        id
    }

    /// Forget a pane. Everything keyed by it goes at the next `sync`.
    pub(crate) fn remove(&mut self, id: PaneId) -> bool {
        let before = self.panes.len();
        self.panes.retain(|pane| pane.id != id);
        let removed = before != self.panes.len();
        self.changed |= removed;
        removed
    }

    /// Take a pane for a client that arrived without being asked for, on top.
    ///
    /// Called where the window is mapped, so that nothing can observe a mapped
    /// window that has no pane. `sync` would create one at the next refresh
    /// anyway; this is what makes the id available *now*, to the script that is
    /// about to be told the window opened.
    pub(crate) fn mapped(
        &mut self,
        window: Window,
        slot: Rectangle<i32, Logical>,
        now: Duration,
    ) -> PaneId {
        if let Some(existing) = self.id_of(&window) {
            return existing;
        }
        let pane = Pane::mapped(window, slot, now);
        let id = pane.id;
        self.panes.push(pane);
        id
    }

    /// Reconcile against the space, given its elements bottom to top with the
    /// slot each one occupies.
    ///
    /// Three things happen here and nowhere else: a pane whose client has gone
    /// is retired, a client with no pane gets one, and the order is brought
    /// back in line with the stacking `Space` keeps. Doing it in one pass over
    /// one input is the only reason the two views can be trusted to agree —
    /// the alternative is remembering to do it at every site that maps or
    /// unmaps, which is the same discipline written down six times.
    ///
    /// Returns whether anything changed, so a caller can skip work that only
    /// matters when the window list is different.
    pub(crate) fn sync(
        &mut self,
        stack: &[(Window, Rectangle<i32, Logical>)],
        now: Duration,
    ) -> bool {
        let before: Vec<PaneId> = self.panes.iter().map(|pane| pane.id).collect();

        // Taken out one at a time and put back in the space's order. Whatever
        // is still here at the end has no client in the space.
        let mut held: Vec<Option<Pane>> = self.panes.drain(..).map(Some).collect();

        let mut ordered: Vec<Pane> = Vec::with_capacity(stack.len());
        for (window, slot) in stack {
            let mine = held
                .iter_mut()
                .find(|held| {
                    held.as_ref()
                        .is_some_and(|pane| pane.client() == Some(window))
                })
                .and_then(Option::take);
            // The space is the authority on where a mapped client is, and this
            // is the one place a pane is told so. A loading pane's slot is its
            // own, which is why the two cannot be the same assignment.
            let mut pane = mine.unwrap_or_else(|| Pane::mapped(window.clone(), *slot, now));
            // A window with no size has told us nothing: it has been mapped
            // and has not committed a buffer yet. Writing that over the slot
            // the layout gave the pane would throw the layout's answer away --
            // which is exactly the window between a client mapping into a pane
            // and its first commit.
            if slot.size.w > 0 && slot.size.h > 0 {
                pane.set_slot(*slot);
            }
            ordered.push(pane);
        }

        // A pane still waiting for a client stays, and rides on top, where a
        // window just asked for belongs. One whose client has gone is retired
        // here; step 5 is where it lingers to animate out instead of vanishing.
        ordered.extend(
            held.into_iter()
                .flatten()
                .filter(|pane| pane.client().is_none()),
        );
        self.panes = ordered;

        let after: Vec<PaneId> = self.panes.iter().map(|pane| pane.id).collect();
        std::mem::take(&mut self.changed) || before != after
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
        let first = Pane::loading("kitty", None, slot(), PathBuf::new(), None, Duration::ZERO);
        let second = Pane::loading("kitty", None, slot(), PathBuf::new(), None, Duration::ZERO);
        assert_ne!(first.id(), second.id());
    }

    #[test]
    fn adoption_keeps_the_identity_and_the_slot() {
        // The property the whole design rests on: a client arriving must not
        // produce a *different* window. Nothing here can build a real
        // `Window`, so this asserts what can be asserted without one --
        // that adopting changes neither of the two things everything else
        // addresses a pane by.
        let mut pane = Pane::loading(
            "kitty",
            Some(42),
            slot(),
            PathBuf::new(),
            None,
            Duration::ZERO,
        );
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
        let pane = Pane::loading(
            "slow",
            Some(9),
            slot(),
            PathBuf::new(),
            None,
            Duration::ZERO,
        );
        assert!(!pane.expired(Duration::from_secs(7), patience));
        assert!(pane.expired(Duration::from_secs(8), patience));

        let mut left = Pane::loading(
            "slow",
            Some(9),
            slot(),
            PathBuf::new(),
            None,
            Duration::ZERO,
        );
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
    fn syncing_against_an_empty_space_keeps_what_is_still_waiting() {
        // Nothing here can build a real `Window`, so what is testable is the
        // half that matters most anyway: a pane whose application has not
        // arrived must survive a reconcile that finds no client for it. Get
        // this wrong and a loading window is retired the frame after it
        // appears -- which looks exactly like the feature not working.
        let mut panes = Panes::default();
        let mut ids = Vec::new();
        for name in ["kitty", "firefox"] {
            let pane = Pane::loading(name, Some(1), slot(), PathBuf::new(), None, Duration::ZERO);
            ids.push(panes.open(pane));
        }

        assert!(
            panes.sync(&[], Duration::from_secs(1)),
            "opening them is a change, and it is reported once"
        );
        assert!(
            !panes.sync(&[], Duration::from_secs(2)),
            "and then nothing came and nothing went"
        );
        assert_eq!(panes.len(), 2);
        assert_eq!(
            panes.iter().map(Pane::id).collect::<Vec<_>>(),
            ids,
            "and they kept their order"
        );
    }

    #[test]
    fn a_script_addresses_a_pane_by_its_id() {
        let mut panes = Panes::default();
        let id = panes.open(Pane::loading(
            "kitty",
            None,
            slot(),
            PathBuf::new(),
            None,
            Duration::ZERO,
        ));

        assert_eq!(panes.by_script_id(id.get()).map(Pane::id), Some(id));
        assert!(
            panes.by_script_id(id.get() + 1000).is_none(),
            "an id that no longer exists is not found, not a panic"
        );
    }

    #[test]
    fn a_pane_with_no_process_adopts_nothing() {
        let pane = Pane::loading(
            "mystery",
            None,
            slot(),
            PathBuf::new(),
            None,
            Duration::ZERO,
        );
        assert!(!pane.awaits(&[1, 2, 3]));
    }
}
