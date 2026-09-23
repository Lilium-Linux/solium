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
#[expect(
    dead_code,
    reason = "steps 4 and 5 of the window-provider migration bring the loading half \
              into use: `Loading::source` is read by whatever draws a pane whose \
              client has not arrived, and `Leaving` is constructed by whatever keeps \
              one on screen while it goes. See the module docs."
)]
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

/// What the compositor draws around this pane's client.
///
/// One value rather than two tables. `Decorations` held `frames:
/// HashMap<PaneId, Decoration>` and `bare: HashSet<PaneId>` as parallel
/// collections keyed by `PaneId`, and two tables answering one question can
/// disagree — `Solium::insets_of` checked `frames` first, so a pane in both had
/// `bare` silently ignored, and nothing tested that. There is nowhere left for
/// that state to be: three arms, one value, and it belongs to the pane it is
/// drawn around.
///
/// **Every reader asks this**, whether it wants a fact about the frame — how
/// much room it takes, whether there is one at all — or the live scene itself.
/// `Decorations` is down to the style a script chose and the code that builds a
/// [`crate::decoration::Decoration`] from it; what it builds it hands to the
/// pane. See `docs/superpowers/plans/2026-09-12-pane-ownership.md`.
///
/// **There is no `large_enum_variant` suppression on this any more, and its
/// absence is the measurement.** There was one, justified by
/// `size_of::<Decoration>()` being 248; a decoration then became a *list* of
/// layers, its scene and its backing moved behind that list's pointer, and it
/// is now **112 bytes** — under clippy's 200-byte threshold, so the lint does
/// not fire at all and an `#[expect]` for it is an unfulfilled expectation and
/// a warning of its own. The niche is unchanged and still load-bearing, which
/// is what `a_frame_costs_what_the_decoration_in_it_costs` pins: a `Vec`'s
/// pointer is non-null, so the discriminant still lands inside the payload and
/// `Frame` is 112 as well. `Pane` is 712.
///
/// Those three literals have gone stale once already — bleed put the pane's
/// size into `Shown`, eight bytes, and all three moved together. They are here
/// to give the threshold a scale and nothing depends on them, which is why the
/// test is a relation and not two numbers.
#[derive(Debug, Default)]
pub(crate) enum Frame {
    /// A client is still coming, or its frame has not been built yet. Keep
    /// reserving the insets a frame will want, or the window jumps when it
    /// finally arrives.
    #[default]
    Pending,
    /// There will never be a frame: the client draws its own, or this is an
    /// override-redirect menu. Not the same as "not yet".
    None,
    /// Drawn by the compositor, from this scene.
    ///
    /// The [`crate::decoration::Decoration`] itself, owned here and nowhere
    /// else: it is a live Qt scene, is not `Clone`, and there is exactly one
    /// per decorated window. So it is **moved** in — a second one would be two
    /// scenes per window, which is a behaviour change wearing a refactor's
    /// clothes — and it leaves when the pane does, which is the table
    /// reconciliation this whole change exists to delete.
    ///
    /// Not boxed. Measured: `size_of::<Decoration>()` is 112 and `Frame` with
    /// this arm inline is also 112, because the discriminant lands in a niche.
    /// A `Box` here buys an allocation per decorated window and saves nothing —
    /// and a decoration's own scenes are already behind one pointer, since a
    /// style is a `Vec` of layers.
    Styled(crate::decoration::Decoration),
}

/// A window, as the compositor thinks of one.
#[derive(Debug)]
pub(crate) struct Pane {
    id: PaneId,
    slot: Rectangle<i32, Logical>,
    /// The **outer** rectangle of the tile a layout holds this pane in, while
    /// one does.
    ///
    /// **Not a cache of [`Self::slot`], and the difference is the whole reason
    /// this field exists.** `Panes::sync` writes the space's answer over `slot`
    /// on every frame a pane is not under a resize hold, and the space reports
    /// a mapped window's size as whatever the *client* last committed — so the
    /// moment a client answers a configure with a size of its own, `slot` stops
    /// being the rectangle the layout asked for and becomes the layout's origin
    /// paired with the client's size. That is not a rare case: a terminal on a
    /// cell grid does it on every resize it is ever given, and
    /// `Solium::settle_resize_hold` deliberately adopts such an answer into the
    /// slot rather than fighting it.
    ///
    /// Written by `Solium::move_pane`, and by nothing that hears from a
    /// client. That is what makes it the one place the compositor keeps the
    /// *layout's* opinion of where this pane is, which is what an edge drag has
    /// to start from — see `Solium::pane_laid_out` for why the client's opinion
    /// will not do.
    ///
    /// **And it is the tile the client is held inside (#133).**
    /// `Solium::pane_geometry` caps a settled client's committed size at this
    /// rectangle's client share, and `render::elements` cuts the client's
    /// surfaces to it, so a client that will not shrink as far as its tile —
    /// a terminal on its cell grid, a browser at its minimum width — is drawn
    /// inside the tile rather than over its neighbours. That is why this means
    /// "tiled now" and not "tiled once": a stale rectangle here would cut a
    /// maximised, fullscreen or floating window down to the tile it used to
    /// have. So it is cleared on every way out of a tile — a maximise and a
    /// fullscreen (which keep it in [`Self::left_tile`] for the way back), a
    /// `sol.place` with `tile = false`, and `sol.unplace`, which is what
    /// `modes.use` sends for every window when the layout changes.
    /// `a_maximised_window_is_not_held_in_its_old_tile`,
    /// `a_fullscreen_window_is_not_held_in_its_old_tile` and
    /// `a_window_let_go_by_its_layout_is_not_held_in_its_old_tile` are the
    /// three.
    ///
    /// **Usually from the rectangle a layout handed `sol.place`, but not
    /// always**: `Solium::rescue_offscreen` reaches `move_pane` too, with a
    /// rectangle it worked out itself to drag a window back onto a screen that
    /// went away. So the invariant is the narrower one — no client ever writes
    /// here — and not "this is what the layout last said". The next sweep puts
    /// the layout's answer back, and a drag begun in between starts from a
    /// rectangle the window really is at, which is the right answer anyway. A
    /// rescue keeps a pane's standing as it found it: a tiled pane is still
    /// tiled at the rectangle it was rescued to, and a floating one is still
    /// floating.
    ///
    /// `None` for a pane no layout holds in a tile — a floating window, a
    /// dialog a layout centres over its parent, a maximised or fullscreen
    /// window, or one in the frames between mapping and the first sweep —
    /// where the pane's own rectangle is the only answer there is.
    placed: Option<Rectangle<i32, Logical>>,
    /// The tile this pane left when it was maximised or sent fullscreen, kept
    /// for the way back to put it in again.
    ///
    /// Beside [`Self::restore`] and spent with it: written when
    /// `Solium::toggle_maximize` or `Solium::fullscreen_request` takes the
    /// pane out of [`Self::placed`], and taken by whichever of
    /// `toggle_maximize` and `Solium::unfullscreen_request` puts the window
    /// back at its restore rectangle. Without it a window restored into its
    /// tile would be a floating window inside a tiled layout until the next
    /// sweep, and an edge drag begun in between would start from the client's
    /// rectangle rather than the layout's — #124 again, by another door.
    /// `a_restored_window_goes_back_into_its_tile` pins it.
    left_tile: Option<Rectangle<i32, Logical>>,
    /// Where this window goes back to when it leaves maximised or fullscreen.
    ///
    /// Written by `Solium::toggle_maximize` and `Solium::fullscreen_request`
    /// before they move the window, and taken -- used once -- by whichever of
    /// `toggle_maximize` and `Solium::unfullscreen_request` puts it back.
    ///
    /// **One slot for both**, so a window maximised and then sent fullscreen
    /// has the rect from before the maximise here. Leaving fullscreen leaves it
    /// alone for such a window and puts it back to maximised, and ignores a
    /// client asking to leave a fullscreen it is not in; either way round, the
    /// rect would be spent on a window that is still maximised.
    ///
    /// **A property of the window, not of its frame**, and it lived on the
    /// frame until #92. `Solium::fullscreen_request` wrote it into the pane's
    /// `Decoration` and then, two lines later, dropped that decoration so a
    /// fullscreen window has no titlebar; leaving fullscreen built a new one
    /// with this empty, so there was never a rect to put the window back at.
    /// A window with no server-side frame at all -- one that draws its own, or
    /// any window under `pane = "none"` -- had nowhere to keep it in the first
    /// place. For fullscreen that was reachable: a client drawing its own
    /// frame that went fullscreen had no rect kept at all. For maximise it was
    /// latent, because the only way to maximise is a button on a frame, so a
    /// window with no frame had no button to press.
    ///
    /// Here, it survives every [`Self::set_frame`], which is what
    /// `a_windows_way_back_outlives_its_frame` pins.
    restore: Option<Rectangle<i32, Logical>>,
    content: Content,
    /// What is drawn around this pane's client. See [`Frame`].
    ///
    /// A field rather than a lookup, so it leaves with the pane: a frame keyed
    /// by `PaneId` in a table beside the panes has to be reconciled by hand,
    /// and one that is not is kept for ever.
    frame: Frame,
    /// When this pane's close request goes out, once it has finished leaving.
    ///
    /// A *deadline*, not a start: `Solium::close_pane` animates the window
    /// away first and asks the client afterwards, and this is the moment
    /// "afterwards" arrives. `None` for a window nobody has asked to close.
    ///
    /// A field for the reason `frame` is one — `Solium::closing` was a
    /// `HashMap<PaneId, Duration>` beside the panes, and an entry whose pane
    /// had gone stayed until something remembered to sweep it. This one leaves
    /// with its pane.
    closing_at: Option<Duration>,
    /// When this pane's client was asked to close.
    ///
    /// A close is a request. A client may put up "are you sure?" and stay, and
    /// nothing in the protocol says so — the only evidence is the window still
    /// being here a moment later. `None` once that moment has passed and the
    /// window has either gone or been brought back. See
    /// `Solium::settle_refused`.
    asked_at: Option<Duration>,
    /// Whether this close has already been answered, and the window is owed its
    /// place back.
    ///
    /// Set by `Solium::refused_with_a_dialog` and cleared by
    /// `Solium::give_back` on the frame the give-back actually takes. It exists
    /// because a give-back can *decline*: `present::clear` goes through
    /// `with_slot`, which answers `None` rather than panicking when the
    /// transform slot is already borrowed, and on such a frame nothing is
    /// retired. `settle_refused` cannot retry a pane that is still inside
    /// `CLOSING`, where `asked_at` is `None` by definition — so without this
    /// the declined give-back was simply dropped and `settle_closing` went on
    /// to close the parent out from under its own dialog.
    ///
    /// A fact about the close rather than an instant, because there is no
    /// deadline in it: the retry is "every frame until the slot is free", which
    /// is what a busy slot costs everywhere else.
    answered: bool,
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
    /// Whether a layout may place this, and whether it counts as a window.
    ///
    /// False for an X11 override-redirect surface — a menu, a tooltip, a drag
    /// icon. Those say "do not manage me" and they mean it: they place
    /// themselves, they are gone in a moment, and a layout that treats one as
    /// a window reserves a slot for it and reflows the desktop around a
    /// tooltip. Dragging a text selection out of an application did exactly
    /// that.
    ///
    /// They are still panes, because they are on screen and under the pointer
    /// and every path that asks what is on screen asks for panes. What they
    /// are not is *windows*.
    managed: bool,
    /// The texture this pane's warp captures are drawn into, kept across the
    /// frames of an animation. See [`crate::offscreen::Scratch`].
    ///
    /// A field for the reason `frame` and `closing_at` are fields, and with
    /// rather more at stake: a `HashMap<PaneId, GlesTexture>` beside the panes
    /// would be a sixth table reconciled by hand, and what an entry nobody
    /// swept would keep is not a stale boolean but several megabytes of GBM.
    /// This leaves with its pane.
    ///
    /// Empty on a pane nobody is warping, which is all of them on an ordinary
    /// desktop: `render::prepare` hands it back on the first frame a pane is
    /// not captured on.
    scratch: crate::offscreen::Scratch,
}

/// `dead_code` on the *block*, which is as narrow as this one can be: the lint
/// reports every unused method of an impl in a single diagnostic at the impl's
/// own span, so an `expect` on the method it names cannot match it. `content`
/// and `leave` are the two, and they are the same window-provider migration the
/// `Content` arms above are waiting for.
#[expect(
    dead_code,
    reason = "steps 4 and 5 of the window-provider migration call `content` and \
              `leave`: reading what a pane is showing, and keeping one on screen \
              while its client goes. See the module docs."
)]
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
            placed: None,
            left_tile: None,
            restore: None,
            content: Content::Loading {
                program: program.to_owned(),
                pid,
                source,
                scene: scene.map(Box::new),
            },
            frame: Frame::Pending,
            closing_at: None,
            asked_at: None,
            answered: false,
            opened: now,
            adopted: false,
            drawn: crate::present::Slot::default(),
            managed: true,
            scratch: crate::offscreen::Scratch::default(),
        }
    }

    /// A pane for a client that arrived without being asked for — anything
    /// started outside the compositor.
    pub(crate) fn mapped(window: Window, slot: Rectangle<i32, Logical>, now: Duration) -> Self {
        Self {
            id: PaneId::next(),
            slot,
            placed: None,
            left_tile: None,
            restore: None,
            content: Content::Client {
                window,
                scene: None,
                faded: None,
            },
            frame: Frame::Pending,
            closing_at: None,
            asked_at: None,
            answered: false,
            opened: now,
            adopted: false,
            drawn: crate::present::Slot::default(),
            managed: true,
            scratch: crate::offscreen::Scratch::default(),
        }
    }

    /// Whether a layout may place this pane and count it as a window.
    pub(crate) const fn managed(&self) -> bool {
        self.managed
    }

    /// Mark this pane as one that places itself. See the field.
    pub(crate) const fn unmanage(&mut self) {
        self.managed = false;
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

    /// The outer rectangle a layout last asked for this pane. See the field.
    pub(crate) const fn placed(&self) -> Option<Rectangle<i32, Logical>> {
        self.placed
    }

    /// Record the outer rectangle a layout has just asked for this pane.
    ///
    /// Separate from [`Self::set_slot`] rather than folded into it, because the
    /// two have different writers on purpose: every path that hears from a
    /// client sets the slot, and only a layout's sweep sets this.
    pub(crate) const fn set_placed(&mut self, placed: Rectangle<i32, Logical>) {
        self.placed = Some(placed);
    }

    /// Take this pane out of its tile for a maximise or a fullscreen, keeping
    /// the tile for the way back. See [`Self::left_tile`].
    ///
    /// A pane that is in no tile keeps whatever tile it was already waiting to
    /// go back to: a window maximised and then sent fullscreen left its tile
    /// at the maximise, and the fullscreen must not forget it. This and the
    /// two below are `a_tile_left_for_a_maximise_is_kept_for_the_way_back_and_no_longer`.
    pub(crate) const fn leave_tile(&mut self) {
        if let Some(placed) = self.placed.take() {
            self.left_tile = Some(placed);
        }
    }

    /// Put this pane back into the tile it left, now that it is back at its
    /// restore rectangle. See [`Self::left_tile`].
    ///
    /// A tile a layout has placed it in since wins over the kept one: that is
    /// the layout's newer answer. It happens — `tiling.apply` places every
    /// leaf of its trees on every sweep, and a maximise takes no window out of
    /// its tree.
    pub(crate) const fn return_to_tile(&mut self) {
        let left = self.left_tile.take();
        if self.placed.is_none() {
            self.placed = left;
        }
    }

    /// No layout holds this pane in a tile any more, and none is keeping one
    /// for it to go back to.
    ///
    /// Both fields, because a window maximised in tiling and then let go by the
    /// layout — `modes.use` switching to floating — must not be put back in its
    /// old tile by the un-maximise that follows, when there is no layout left
    /// to hold it there.
    pub(crate) const fn untile(&mut self) {
        self.placed = None;
        self.left_tile = None;
    }

    /// Where this window goes back to, if a maximise or a fullscreen kept
    /// one. See the field.
    pub(crate) const fn restore(&self) -> Option<Rectangle<i32, Logical>> {
        self.restore
    }

    /// Remember where this window goes back to, or forget it.
    pub(crate) const fn set_restore(&mut self, restore: Option<Rectangle<i32, Logical>>) {
        self.restore = restore;
    }

    /// Where this window goes back to, and forget it: the way back is used
    /// once.
    pub(crate) const fn take_restore(&mut self) -> Option<Rectangle<i32, Logical>> {
        self.restore.take()
    }

    pub(crate) const fn content(&self) -> &Content {
        &self.content
    }

    /// What is drawn around this pane's client. See [`Frame`].
    pub(crate) const fn frame(&self) -> &Frame {
        &self.frame
    }

    /// This pane's frame scene, if one has been built.
    pub(crate) const fn decoration(&self) -> Option<&crate::decoration::Decoration> {
        match &self.frame {
            Frame::Styled(decoration) => Some(decoration),
            Frame::Pending | Frame::None => None,
        }
    }

    /// The same, to write to: the readers that want the scene itself rather
    /// than a fact about it — drawing it, giving it the pointer, taking its
    /// button presses.
    ///
    /// The narrowing and not a `frame_mut`, deliberately. Handing out
    /// `&mut Frame` would let any caller swap a `Styled` for a `None` without
    /// going through [`crate::decoration::Decorations`] — a second writer of
    /// the fact this change exists to keep in one place. Everything that
    /// decides *whether* there is a frame goes through `set_frame`.
    pub(crate) const fn decoration_mut(&mut self) -> Option<&mut crate::decoration::Decoration> {
        match &mut self.frame {
            Frame::Styled(decoration) => Some(decoration),
            Frame::Pending | Frame::None => None,
        }
    }

    /// The texture this pane's warp captures are drawn into. See the field and
    /// [`crate::offscreen::Scratch`].
    ///
    /// Only `_mut`, because both things anyone does with it — taking a texture
    /// for a capture, handing one back when the warp ends — write to it. There
    /// is nothing to read.
    pub(crate) const fn scratch_mut(&mut self) -> &mut crate::offscreen::Scratch {
        &mut self.scratch
    }

    /// Say what is drawn around this pane's client.
    ///
    /// `Decorations`' five mutators are the only callers: they own the policy
    /// — which QML, whether the style is `none`, whether it loaded — and this
    /// is where the answer lands. A `Styled` replaced here drops the scene it
    /// held, which is what makes "the frame leaves with its pane" true.
    pub(crate) fn set_frame(&mut self, frame: Frame) {
        self.frame = frame;
    }

    /// When this pane's close request goes out. See the field.
    pub(crate) const fn closing_at(&self) -> Option<Duration> {
        self.closing_at
    }

    /// This pane is on its way out; `due` is when to tell its client so.
    pub(crate) const fn begin_closing(&mut self, due: Duration) {
        self.closing_at = Some(due);
    }

    /// The request has gone out, or there is nothing left to ask.
    pub(crate) const fn stop_closing(&mut self) {
        self.closing_at = None;
    }

    /// When this pane's client was asked to close. See the field.
    pub(crate) const fn asked_at(&self) -> Option<Duration> {
        self.asked_at
    }

    /// The client has been asked to close, at `at`. Whether the request
    /// actually went out is deliberately not recorded: a window we could not
    /// even ask is the one most in need of being waited for.
    pub(crate) const fn mark_asked(&mut self, at: Duration) {
        self.asked_at = Some(at);
    }

    /// Stop waiting on an answer that was never going to come in words.
    pub(crate) const fn forget_asked(&mut self) {
        self.asked_at = None;
    }

    /// Whether this close has been answered and the window is owed its place
    /// back. See the field.
    pub(crate) const fn answered(&self) -> bool {
        self.answered
    }

    /// The client answered this close in as many words, by putting a window on
    /// screen. The give-back is owed from here until it is taken.
    pub(crate) const fn mark_answered(&mut self) {
        self.answered = true;
    }

    /// The give-back took. Nothing is owed.
    pub(crate) const fn settled_answer(&mut self) {
        self.answered = false;
    }

    /// Whether this pane is on its way off the screen.
    ///
    /// **One question with one answer, because asking half of it is issue
    /// #127.** "On its way out" spans three states and the two callers that
    /// have to know — `Solium::close_pane`, which must not start a second
    /// close, and `Solium::move_pane`, which must not overwrite the transform
    /// that is playing one — each used to ask a different, narrower question.
    /// `close_pane` asked `closing_at().is_some()`, which is false for the
    /// whole of the grace period after the request has gone out, so a second
    /// `super+q` restarted an invisible animation and asked the client to close
    /// a second time. `move_pane` asked nothing at all, and put a dying window
    /// back at full opacity.
    ///
    /// The three states, in the order a window passes through them:
    ///
    /// 1. `closing_at` — the leaving animation is playing and the request has
    ///    not gone out yet. 190 ms.
    /// 2. `asked_at` — the request has gone out and the client has not
    ///    answered. The window is held invisible for as long as this lasts, so
    ///    it is *more* in need of the guard than (1), not less: there is
    ///    nothing on screen for a second press to have been aimed at.
    /// 3. [`Content::Leaving`] — the client has gone and the pane is still
    ///    being drawn. Nothing constructs this today; it is issue #126's state.
    ///
    /// **What the third arm is and is not.** It is here so that this predicate
    /// is total over `Content` and cannot answer "not leaving" for a variant
    /// whose name is `Leaving` — nothing more than that. It is *not* the
    /// #126-shaped case being handled in advance, and the first draft of this
    /// comment said it was: "the relayout #126 will put on a still-drawn pane
    /// is already covered". That was the same promise-in-a-comment that kept
    /// #126 itself unfiled behind a note about an animation nothing could play,
    /// and it is worth less than nothing, because the next reader trusts it.
    ///
    /// What is actually true is that **the first `Content::Leaving` pane anyone
    /// constructs will never go away.** Three separate places keep it alive, and
    /// #126 has to answer all three:
    ///
    /// * `Panes::sync` retains exactly the panes whose `client()` is `None`,
    ///   which is how a still-loading pane survives a sweep — and a `Leaving`
    ///   pane's `client()` is `None` too, so it is retained by the same line
    ///   with nothing to ever drop it.
    /// * [`Self::expired`] only answers for [`Self::is_loading`], so the
    ///   patience timeout that retires an application that never arrived does
    ///   not look at this state at all.
    /// * `Solium::close_pane` declines any pane that is `leaving()` — this
    ///   function — so it cannot be closed by hand either.
    ///
    /// So #126 owes this state a retirement: something that ends the animation
    /// and drops the pane. Until then the arm is correct and unreachable, which
    /// is the only combination worth writing down.
    pub(crate) const fn leaving(&self) -> bool {
        self.closing_at.is_some()
            || self.asked_at.is_some()
            || matches!(self.content, Content::Leaving { .. })
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
        // window just asked for belongs.
        //
        // **One whose client has gone is dropped here, and vanishes.** Nothing
        // in the compositor constructs `Content::Leaving` -- `Pane::leave` has
        // no production caller -- so there is no state in which a pane outlives
        // its client, and the line below is where a window that closed itself
        // stops being drawn: on the first `sync` after its surface went, with
        // no animation. That is issue #126, and it is a design limit rather
        // than an oversight: animating it means holding a texture for every
        // window on the chance that it is the next to leave. This comment used
        // to say step 5 was "where it lingers to animate out", in the present
        // tense, describing an animation nothing could play -- which is how
        // #126 went unfiled for as long as it did.
        //
        // The compositor's *own* closes do animate; they keep the client alive
        // for the length of it and ask afterwards. See `Solium::close_pane`.
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
    crate::assets::qml()
        .join("loading")
        .join(format!("{name}.qml"))
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

    /// **What `show_if_new`'s placement guard (state.rs) relies on.** That
    /// function is the placement path for issue #100's XWayland half: an
    /// override-redirect window (a Steam context menu) arrives already
    /// placed by its own client, `take_unmanaged_pane` calls `unmanage` on
    /// its pane, and `show_if_new` must then never size or move it -- in
    /// either of its two branches -- on this commit or any later one.
    ///
    /// `show_if_new` itself cannot be exercised here. Like
    /// `adoption_keeps_the_identity_and_the_slot` above, nothing in this
    /// crate's tests can build a real `Window`, X11-backed or otherwise, so
    /// this pins the contract its guard depends on instead: `managed` must
    /// actually flip, and flipping it must be the *only* thing that
    /// happens. A test that stopped at `managed()` would pass even if
    /// `unmanage` also reset the slot to a default -- which would defeat the
    /// guard just as thoroughly as `show_if_new` never asking it to -- so
    /// the geometry is asserted too: the slot a pane already has when it
    /// becomes unmanaged is "the geometry it was mapped with" for a real
    /// override-redirect window, and it must survive untouched.
    #[test]
    fn an_unmanaged_pane_keeps_the_slot_it_was_mapped_with() {
        let mapped_with = Rectangle::new((640, 360).into(), (220, 140).into());
        let mut pane = Pane::loading(
            "steam-menu",
            None,
            mapped_with,
            PathBuf::new(),
            None,
            Duration::ZERO,
        );
        assert!(pane.managed(), "a pane is managed until told otherwise");

        pane.unmanage();

        assert!(
            !pane.managed(),
            "unmanage must flip the exact flag show_if_new's guard and \
             snapshot's window-list filter both read"
        );
        assert_eq!(
            pane.slot(),
            mapped_with,
            "and must do nothing else -- the geometry a pane was mapped \
             with is what the placement guard exists to protect"
        );
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
    fn a_pane_is_pending_until_it_is_told_otherwise() {
        // The default matters more than it looks: a pane in neither table is
        // one whose frame has not been built *yet*, and `insets_of` keeps
        // reserving room for it. A pane that defaulted to `None` would lose
        // that room and the window would jump when its frame arrived.
        let mut pane = Pane::loading("kitty", None, slot(), PathBuf::new(), None, Duration::ZERO);
        assert!(matches!(pane.frame(), Frame::Pending));
        pane.set_frame(Frame::None);
        assert!(matches!(pane.frame(), Frame::None));
    }

    #[test]
    fn a_pane_cannot_be_both_styled_and_bare() {
        // Not a runtime check. `Frame` is an enum and a pane holds exactly one,
        // so "styled *and* bare" cannot be written down — this test exists to
        // say that out loud, and to fail if a second source of truth ever comes
        // back.
        //
        // It could be written down before. `Decorations` held `frames:
        // HashMap<PaneId, Decoration>` and `bare: HashSet<PaneId>`, a pane
        // could be in both, and `Solium::insets_of` read `frames` first — so
        // such a pane was framed and its `bare` was silently ignored, and
        // nothing tested it.
        //
        // The match below is the assertion, and it is the part that would stop
        // compiling: it is exhaustive over `Frame`, so an arm meaning "framed,
        // but also bare" has to come here and be given an answer to the one
        // question everything else asks. What is checked at run time is that
        // both accessors derive their answer from that same single field.
        // `decoration.rs`'s `a_pane_cannot_be_framed_and_bare_at_once` takes
        // the same claim through `set_bare` — the mutator that used to leave a
        // frame standing — on a pane with a real `Decoration` on it.
        let mut pane = Pane::loading("kitty", None, slot(), PathBuf::new(), None, Duration::ZERO);
        for frame in [Frame::Pending, Frame::None] {
            pane.set_frame(frame);
            let styled = match pane.frame() {
                Frame::Styled(_) => true,
                Frame::Pending | Frame::None => false,
            };
            assert_eq!(
                pane.decoration().is_some(),
                styled,
                "whether there is a scene is the frame, not a second opinion \
                 about it"
            );
            assert_eq!(
                pane.decoration_mut().is_some(),
                styled,
                "and writing to it reaches the same value reading it does"
            );
        }

        // `Styled` is not among them because building one needs Qt, which this
        // module has no business starting. It is covered where a frame can
        // actually be built, in `decoration.rs`.
    }

    #[test]
    fn a_pane_has_no_timers_until_somebody_asks_it_to_close() {
        let mut pane = Pane::loading("kitty", None, slot(), PathBuf::new(), None, Duration::ZERO);
        assert!(pane.closing_at().is_none());
        assert!(pane.asked_at().is_none());

        // The sequence `close_pane` and `settle_closing` put a window through:
        // leave, then ask, then wait to see whether it went.
        pane.begin_closing(Duration::from_millis(190));
        assert_eq!(pane.closing_at(), Some(Duration::from_millis(190)));
        pane.stop_closing();
        pane.mark_asked(Duration::from_millis(190));
        assert!(pane.closing_at().is_none(), "the request has gone out");
        assert_eq!(
            pane.asked_at(),
            Some(Duration::from_millis(190)),
            "and it is still being waited on"
        );
        pane.forget_asked();
        assert!(pane.asked_at().is_none());
    }

    #[test]
    fn a_closing_timer_goes_when_the_pane_does() {
        // Why the timers moved in. They were `HashMap<PaneId, Duration>`
        // beside the panes, and an entry whose pane had gone stayed there
        // until `sync_panes` remembered to sweep it. There is no longer
        // anything to remember.
        let mut panes = Panes::default();
        let id = panes.open(Pane::loading(
            "kitty",
            None,
            slot(),
            PathBuf::new(),
            None,
            Duration::ZERO,
        ));
        if let Some(pane) = panes.get_mut(id) {
            pane.begin_closing(Duration::from_millis(10));
            pane.mark_asked(Duration::from_millis(4));
        }
        assert_eq!(
            panes.get(id).and_then(Pane::closing_at),
            Some(Duration::from_millis(10))
        );

        assert!(panes.remove(id));
        assert!(
            panes.get(id).is_none(),
            "and both of its timers went with it; there is nothing left to retain"
        );
    }

    #[test]
    fn a_reconcile_does_not_forget_what_a_pane_was_told() {
        // `sync` drains the list and rebuilds it. A timer that did not ride
        // along would be a window told to close and then never asked -- which
        // is the failure the old `retain` lines could not have caused and a
        // move like this one can.
        let mut panes = Panes::default();
        let id = panes.open(Pane::loading(
            "kitty",
            None,
            slot(),
            PathBuf::new(),
            None,
            Duration::ZERO,
        ));
        if let Some(pane) = panes.get_mut(id) {
            pane.begin_closing(Duration::from_millis(10));
            pane.mark_asked(Duration::from_millis(4));
        }

        panes.sync(&[], Duration::from_secs(1));
        assert_eq!(
            panes.get(id).and_then(Pane::closing_at),
            Some(Duration::from_millis(10))
        );
        assert_eq!(
            panes.get(id).and_then(Pane::asked_at),
            Some(Duration::from_millis(4))
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

    /// **A `Frame` costs what the decoration inside it costs, and not a byte
    /// more.**
    ///
    /// The claim the `#[expect(clippy::large_enum_variant)]` on `Frame` used to
    /// carry in prose, asserted instead — because that comment's number went
    /// stale the moment a `Decoration` changed shape, and a suppression with a
    /// false number attached is worse than one with no reason at all.
    ///
    /// Written as a relation rather than as two literals on purpose. The
    /// numbers move whenever anything in a decoration moves — they already have
    /// once, 104 to 112, when bleed put the pane's size into `Shown` — and the
    /// fact worth keeping is not what they are but that they are *equal*: the
    /// discriminant lands in a niche inside the payload, so the three-armed
    /// enum is free relative to the one arm that carries anything. Lose the
    /// niche and this fails, which is exactly when boxing would be worth
    /// re-arguing.
    #[test]
    fn a_frame_costs_what_the_decoration_in_it_costs() {
        assert_eq!(
            size_of::<Frame>(),
            size_of::<crate::decoration::Decoration>(),
            "`Frame::Styled` stopped fitting its discriminant into a niche, so \
             a pane now pays for the tag as well as for the decoration -- and \
             the case for boxing, which was rejected on these two numbers being \
             equal, is worth making again"
        );
    }

    /// **#133: the tile a window left, kept for its way back and no longer.**
    ///
    /// Four promises the three methods make between them, each asserted on the
    /// one pane in the order a session would reach them:
    ///
    ///  1. Leaving a tile keeps it, and the way back puts it back.
    ///  2. A window maximised and then sent fullscreen left its tile at the
    ///     maximise, and the fullscreen -- a second `leave_tile` with no tile to
    ///     take -- does not forget it.
    ///  3. A tile a layout has placed the window in since wins over the kept
    ///     one.
    ///  4. A window the layout lets go of while it is maximised is not put back
    ///     in its old tile by the un-maximise that follows.
    #[test]
    fn a_tile_left_for_a_maximise_is_kept_for_the_way_back_and_no_longer() {
        let tile = Rectangle::new((0, 0).into(), (494, 600).into());
        let newer = Rectangle::new((506, 0).into(), (494, 600).into());
        let mut pane = Pane::loading("kitty", None, slot(), PathBuf::new(), None, Duration::ZERO);

        pane.set_placed(tile);
        pane.leave_tile();
        assert_eq!(pane.placed(), None, "a maximised window is in no tile");
        pane.leave_tile();
        pane.return_to_tile();
        assert_eq!(
            pane.placed(),
            Some(tile),
            "and the way back puts it in the tile it left, through a second \
             leave with nothing to take"
        );

        pane.leave_tile();
        pane.set_placed(newer);
        pane.return_to_tile();
        assert_eq!(
            pane.placed(),
            Some(newer),
            "a layout's newer tile is not overwritten by the kept one"
        );

        pane.leave_tile();
        pane.untile();
        pane.return_to_tile();
        assert_eq!(
            pane.placed(),
            None,
            "a window let go by its layout stays out of every tile"
        );
    }

    /// **Issue #92: a window's way back is not its frame's to lose.**
    ///
    /// The rect a window goes back to used to live on the `Decoration`, so
    /// the `remove` that drops a window's frame as it goes fullscreen dropped
    /// the rect too. This sets a rect and then changes the frame under it
    /// three times -- `None`, `Pending`, `None`, starting from a loading pane
    /// -- to show that no `set_frame` touches it.
    ///
    /// Not the fullscreen sequence itself, which starts and ends on `Styled`:
    /// `Styled`, `Pending` (`remove`), `None` (`set_bare`), `Pending`
    /// (`unset_bare`), `Styled` (`insert`). Building a `Styled` frame needs Qt,
    /// which this module does not start, so `decoration.rs`'s
    /// `a_rebuilt_frame_does_not_take_the_way_back_with_it` walks that order
    /// on a pane with a real `Decoration` on it.
    #[test]
    fn a_windows_way_back_outlives_its_frame() {
        let mut pane = Pane::loading("kitty", None, slot(), PathBuf::new(), None, Duration::ZERO);
        assert_eq!(
            pane.restore(),
            None,
            "a new window has nowhere to go back to"
        );

        let before = Rectangle::new((400, 300).into(), (640, 480).into());
        pane.set_restore(Some(before));
        pane.set_frame(Frame::None);
        pane.set_frame(Frame::Pending);
        pane.set_frame(Frame::None);

        assert_eq!(
            pane.restore(),
            Some(before),
            "reading it does not use it up"
        );
        assert_eq!(
            pane.take_restore(),
            Some(before),
            "the frame came and went three times, and the way back is still here"
        );
        assert_eq!(
            pane.take_restore(),
            None,
            "and it is used once: the next maximise toggle maximises, rather \
             than jumping back to a rect from before the last one"
        );
    }
}
