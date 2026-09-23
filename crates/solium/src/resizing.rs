//! The rectangle a live edge drag owns, and what fills it until the client
//! catches up.
//!
//! Not to be confused with [`crate::input::resize`], which is the pointer grab:
//! that decides *what rectangle the drag asks for*, and this decides *who is
//! telling the truth about the window's size while the drag lasts*.
//!
//! # Why this exists
//!
//! `state::Solium::resize_to` used to apply a drag by moving the window in the
//! `Space` and *asking* the client for the new size, and everything that reads
//! a window's rectangle reads the client's size back out of it. So the two
//! halves of one rectangle landed at different times: the origin moved on the
//! frame the pointer moved, and the size arrived whenever the client got round
//! to it. Dragging a top-left corner therefore moved the origin at once with
//! the size still old — which puts the *bottom-right* edge somewhere new every
//! frame and snaps it back when the client commits. The shake is proportional
//! to client latency, which is why Firefox was visibly worse than kitty
//! (issue #113).
//!
//! # What replaces it
//!
//! While a drag is live the pane's **slot** is authoritative for the window's
//! size, and the client is catching up. The compositor draws the rectangle the
//! user is dragging, immediately, and bridges the client's last buffer into it
//! with the scale `render.rs` already applies to every window — see
//! [`factor`]. When the client commits the size it was asked for the bridge is
//! exactly 1 again and the window is pixel-exact.
//!
//! This is deliberately *one* of the three places issue #84 names — the
//! `Space`, the pane's slot, the presentation frame — becoming the authority
//! for a bounded while, rather than a fourth place being added. A window's
//! rectangle lives in the pane's slot, exactly where it lived before, and what
//! is here is the bookkeeping that says the slot is in charge and when it stops
//! being. [`Hold::asked`] is a rectangle and is not an exception to that: it is
//! a copy of what was last put on the wire, which nothing reads as a window's
//! geometry and which exists so that the next configure can be compared against
//! the last one.
//!
//! **The `Space` and the slot never disagree about *position*.** Only the size
//! is held back, because only the size needs a client's consent. `state.rs`
//! maps the window to the drag's origin on the frame the drag asks for it, so
//! everything that reads `real_geometry` sees the same position the pane does,
//! and the one deliberate disagreement is the one the bridge is measuring.
//!
//! # Both paths, and why there are several holds
//!
//! `state::Solium::settle_resize` forks on whether a layout claimed the drag,
//! and for two releases everything in this module hung off the *unclaimed*
//! branch. The claimed — tiled — branch dropped the hold and went through
//! `move_pane`, which sizes the window unconditionally, once per pane per
//! frame. So a tiled drag had no throttle (sixty configures a second, which is
//! the stutter issue #123 is about), no inversion (`pane_geometry` fell through
//! to the client's last committed size on every frame that carried no motion,
//! which is the flicker), and no refusal bookkeeping at all. The comment at the
//! fork said "nothing here sizes the window"; the code sized it.
//!
//! The client-facing half of a resize is the same problem whichever branch
//! decided the rectangle, so it is now the same code. What differs is *how many
//! windows one gesture moves*:
//!
//! * Floating: one. `Solium::resize_hold` is one slot and that is right.
//! * Tiled: a seam is two panes, a corner drag is two seams and up to four, and
//!   a layout is free to move every pane on the monitor. Each has its own
//!   client, its own committed size, its own throttle deadline, its own
//!   refusal — and its own moved edge, which is *not* the pointer's: see
//!   [`moved_edges`]. One hold cannot carry that, so `Solium::resize_bridge`
//!   holds one per pane.
//!
//! A hold is still only ever created by a live pointer gesture. `move_pane` is
//! reached by a keyboard nudge, a reload, a monitor change and a workspace
//! switch as well, and a hold created there could never be released —
//! `release_resize` has exactly one caller, the pointer grab — so it would wait
//! for ever and hold the slot and the space apart for ever, which is precisely
//! the disagreement [`PATIENCE`] exists to prevent.
//!
//! # The trap
//!
//! A client may **refuse** the size it is offered — Firefox has a minimum width
//! and will not go under it — and the compositor does not read `min_size` yet
//! (issue #115). If the bridge only ended when the client reached the size it
//! was *asked* for, a refused resize would stretch for ever and the window
//! would be permanently blurry: a worse bug than the one being fixed. So the
//! rule written into [`Hold::settle`] and [`Hold::note`] is that **any answer
//! ends the bridge**, whatever the answer says.
//!
//! **Ending the bridge and taking the stretch away are two different
//! questions**, and reading them as one costs the user something on every
//! drag. A terminal rounds to its cell grid, so it misses the size it was asked
//! for by a few pixels every single time; that is an answer, so it ends the
//! bridge, and it is emphatically not the client that has walked away from the
//! ask — treating it as one means kitty loses the configured fill on the first
//! answer of every seam drag and gets [`Fill::Hold`]'s uncovered strip instead.
//! [`ROUNDING`] is where the line is drawn and why it is drawn there.
//!
//! **And it is a line drawn afresh every frame, not once.** A client answers at
//! the moment it answers and the drag goes on moving afterwards, so a verdict
//! reached about the ask it replied to is out of date as soon as the pointer
//! moves again: the pair the renderer stretches by has one end still
//! travelling. [`SILENCE`] is how much benefit of the doubt that end is given,
//! and it is the same trap from the other side — judged too early it takes the
//! terminal's fill away, judged never it stretches a client that has walked off
//! for the whole of a gesture.

use std::time::Duration;

use smithay::{
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge,
    utils::{Logical, Rectangle, Size},
};

use crate::{
    input::resize::{pulls_left, pulls_top},
    render::ratio,
};

/// How often a client is told its new size while an edge is being dragged.
///
/// A configure per frame is sixty a second, and a client that cannot render at
/// that rate does not try: it falls behind, and the window stutters against the
/// pointer rather than following it. That was true before this module existed —
/// `settle_resize` already coalesced a mouse's thousand reports a second down
/// to one per frame for exactly that reason — and one per frame is still more
/// than Firefox can answer.
///
/// Throttling is only safe *because* the bridge exists. Without it, the window
/// on screen would be whatever the client last answered, so asking less often
/// would mean moving less often, and the drag would visibly step. With the
/// bridge the drag is smooth regardless of when the client is asked, so the
/// only thing the interval governs is how much stretch has accumulated by the
/// time the client answers — and at 100 ms of an ordinary drag that is a few
/// percent, which is not visible.
///
/// **Not deferred to the end of the gesture**, which was the other candidate
/// and is what "one configure per drag" would give. A slow drag across a
/// monitor would then run the bridge up to three or four times and the window
/// would be a smear for the whole of it. Ten configures a second is a rate
/// every client keeps up with and it keeps the stretch invisible; the win over
/// per-frame — sixty renders down to ten — is most of what deferring would
/// have bought anyway.
const TELL_EVERY: Duration = Duration::from_millis(100);

/// How long the pane keeps the rectangle the drag ended on, waiting for the
/// client to answer the last configure.
///
/// This is the backstop for a client that answers *nothing*, which is not only
/// the hung case: asking a client for the size it already has is a configure it
/// has no reason to commit anything new for, and a drag that goes out and comes
/// back ends exactly there. Without a deadline that window would hold a bridge
/// for ever — at a factor of 1, so invisibly, but it would also hold the
/// `Space` and the slot apart for ever, which is precisely the kind of
/// never-resolved disagreement issue #84 is about.
///
/// A quarter second is comfortably past a slow client's round trip (Firefox
/// answers a configure in well under 100 ms even when it is dropping frames)
/// and short enough that a client which really has hung produces one snap
/// rather than a window stuck at the wrong size.
pub(crate) const PATIENCE: Duration = Duration::from_millis(250);

/// What fills a pane between the drag asking for a rectangle and the client
/// filling it.
///
/// A matter of taste rather than of correctness, which is why it is
/// configurable: all three end in the same place, and they differ only in what
/// the gap looks like on the way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Fill {
    /// Scale the client's last buffer into the pane's rectangle.
    ///
    /// The default, and what niri does. Costs nothing — `render.rs` already
    /// scales every window's surface tree into the rectangle its presentation
    /// gives it, and this is that same number computed from a rectangle the
    /// client has not agreed to yet rather than from one it has.
    #[default]
    Stretch,
    /// Keep the last buffer at its own size inside the new rectangle.
    ///
    /// Crisp instead of smooth: nothing is resampled, so what is uncovered
    /// while the pane grows is the pane beneath rather than a stretched
    /// picture. The frame still tracks the pointer exactly; only the picture
    /// inside it lags.
    ///
    /// **Anchored against the edges the drag is not moving**, which is what
    /// makes that claim true for all eight of them rather than for the two easy
    /// ones. Drawing the held buffer at the rectangle's top-left is right for a
    /// bottom or right drag, where the top-left is exactly the corner standing
    /// still, and wrong for every drag that pulls a left or top edge: the
    /// picture would travel with the edge under the pointer and the uncovered
    /// strip would open along the *stationary* edge, so the whole of the
    /// window's contents would slide while the user dragged. [`Hold::pins`] is
    /// the question, `render.rs` offsets by the slack it answers, and the offset
    /// is zero for [`Fill::Stretch`] because a stretched buffer leaves no slack.
    ///
    /// **Asymmetric on purpose.** Growing the pane holds the buffer at 1.0 and
    /// leaves a strip uncovered, which is the whole idea. Shrinking it cannot
    /// do the same without clipping the buffer to the pane, and there is no
    /// clip in the element path while a hold is live — an unclipped oversized
    /// buffer would spill over the frame and over the neighbouring window. So
    /// shrinking falls back to scaling down, which is the direction where
    /// resampling is close to free of artefacts anyway. See [`factor`].
    ///
    /// **Since #133 there is a clip, and this does not use it yet.**
    /// `render::fit` cuts a *settled* tiled client to its tile, and
    /// `Solium::tile_of` answers `None` under a hold, so a held pane is drawn
    /// exactly as this describes. Holding a shrinking buffer at 1.0 inside that
    /// cut is #125's change and not #133's.
    Hold,
    /// Draw the pane's QML scene over the client until it arrives.
    ///
    /// The one only Solium can offer, since a pane already hosts QML — this is
    /// the same handover a window uses before its application has ever painted,
    /// reused for the moment between a client being asked to resize and
    /// answering.
    ///
    /// **Only draws a scene a pane already has**, which in practice means a
    /// window resized before its application first painted. Arming a fresh
    /// scene at the start of a drag was not wired: building a `ShellSurface`
    /// compiles a QML component on the Qt thread, which is a stall at the one
    /// moment a drag must not stall, and `state::Solium::sync_panes` drops any
    /// scene whose client is ready every single frame — so it would also need a
    /// second exception in the one function documented as the place the
    /// compositor's view and the space's are reconciled. That is #84's
    /// reconciliation work, not this issue's. Until then this behaves as
    /// [`Fill::Hold`] under the scene, which is what it would do anyway once
    /// the scene faded.
    Scene,
}

impl Fill {
    /// Read a fill from the name `config.lua` uses.
    ///
    /// Returns `None` for a name that is not one, so the caller can say which
    /// word it did not understand rather than silently choosing for the user.
    pub(crate) fn named(name: &str) -> Option<Self> {
        match name {
            "stretch" => Some(Self::Stretch),
            "hold" => Some(Self::Hold),
            "scene" => Some(Self::Scene),
            _ => None,
        }
    }
}

/// What `config.lua` said about resizing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Settings {
    pub(crate) fill: Fill,
}

/// A live edge drag, and the window whose pane's slot it has taken charge of.
///
/// Split from [`Hold`] so that everything the issue is actually about can be
/// tested without a Wayland display: a `Window` cannot be constructed in a unit
/// test and a `PaneId` cannot be minted outside `pane.rs`, and neither is
/// involved in any decision this module makes.
#[derive(Debug)]
pub(crate) struct Held {
    pub(crate) window: smithay::desktop::Window,
    pub(crate) pane: crate::pane::PaneId,
    pub(crate) hold: Hold,
}

/// Which way a settled hold went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Settle {
    /// The drag is still live, or the client has not answered the last
    /// configure yet. Keep drawing the pane's rectangle.
    Waiting,
    /// The client is the size the pane is. Nothing left to reconcile: drop the
    /// hold and the slot, the space and the client agree again.
    Done,
    /// The client answered with a size of its own, or never answered at all.
    ///
    /// **The client wins.** The pane takes this size — pinned to the edges the
    /// drag did not touch, see [`Hold::anchored`] — which is one snap at the
    /// end of a gesture instead of a window that is blurry for ever. That snap
    /// is issue #115 made visible: a client refusing a size it was offered is
    /// a size we should not have offered.
    Adopt(Size<i32, Logical>),
}

/// The bookkeeping that says the pane's slot is in charge, and when it stops.
///
/// **Stores no rectangle of its own.** The one rectangle below is a copy of
/// what was last put on the wire, not a place a window's geometry lives: the
/// geometry is the pane's slot, exactly where it has always been. What is here
/// is which edges the user has hold of, what the client was last told, and
/// when.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Hold {
    /// Which edges the pointer is dragging, so an adoption can pin the others.
    edges: ResizeEdge,
    /// The client's size when it was last spoken to.
    ///
    /// Anything else is an answer. Comparing against this rather than hooking
    /// the commit path is deliberate: a commit that does not change the size is
    /// not an answer to a configure about size, and the surface state machine
    /// has more ways to produce one of those than are worth enumerating here.
    since: Size<i32, Logical>,
    /// The client **rectangle** the last configure asked for.
    ///
    /// **The whole rectangle and not the size, because a configure carries a
    /// position too.** `state::size_window` is the only thing that ever tells
    /// an X11 client where it is — `map_stacked` moves the window in the space
    /// and says nothing to the client — and `Solium::offers_size` documents
    /// that in its own case (2). Case (1) compared sizes, so a pane that a
    /// layout *translated* without resizing answered "nothing to say" and the
    /// configure was skipped for the rest of the gesture: `scrolling.lua`'s
    /// `widen` shifts every column sideways at an unchanged width, so an X11
    /// window in one of those columns kept stale geometry and misrouted every
    /// pointer coordinate until the button came up.
    ///
    /// Only its size is ever compared against a client's answer, because a
    /// client answers with a size and has no say in where it is put.
    asked: Rectangle<i32, Logical>,
    /// When it was sent, for [`TELL_EVERY`].
    told: Duration,
    /// When the pointer let go, if it has. [`PATIENCE`] runs from here.
    released: Option<Duration>,
    /// The last size this client was offered and did not take.
    ///
    /// **One size, and it is not a `min_size`.** The compositor still does not
    /// read the client's minimum (#115), so this cannot predict which sizes
    /// will be refused; it only records that one of them was. Guessing a rule
    /// from it — "anything narrower than this will be refused too" — is exactly
    /// the sort of invention that ships a worse bug than the one being fixed,
    /// so it is not attempted here. #115 is where that belongs.
    ///
    /// Two different questions read it, which is why it is a size and not a
    /// flag:
    ///
    /// * [`Self::settle`] asks whether it is *this* size that was declined,
    ///   which is the one case where the answer is already in and the gesture
    ///   need not wait out [`PATIENCE`] to learn it again. Anything else waits,
    ///   because without a minimum to read there is no honest way to tell a
    ///   size the client will refuse from one it is merely slow about. **Any
    ///   mismatch counts here**, however small: the end of a gesture is the
    ///   moment the client's own size wins, and it wins by a pixel as readily
    ///   as by two hundred.
    /// * [`Self::refused`] asks whether the mismatch is big enough to take the
    ///   user's configured fill away, and that is a different question with a
    ///   different answer, and it is asked of the ask the drag has reached
    ///   rather than of this one once the client has stopped answering at all.
    ///   See [`ROUNDING`] and [`SILENCE`].
    declined: Option<Size<i32, Logical>>,
    /// How many configures have gone out since the client last said anything.
    ///
    /// Zero means it has answered everything it has been told, one is the
    /// ordinary in-flight state of a client that is keeping up, and anything
    /// more is silence across a whole [`TELL_EVERY`]. [`SILENCE`] is what reads
    /// it and the whole of why.
    ///
    /// **Counted rather than timed**, and the count is the more direct of the
    /// two: the throttle already spaces configures an interval apart, so
    /// counting them counts intervals, and a duration would have to be threaded
    /// through `Solium::resize_fill` into the renderer to be read at the one
    /// place that asks. A release is deliberately not counted — see
    /// [`Self::release`].
    unanswered: u32,
}

/// How far a client's answer may miss the size it was offered and still count
/// as *tracking* it rather than refusing it, as a denominator: a twentieth is
/// five percent.
///
/// [`Hold::declined`] exists for the client that will not go where it is being
/// asked at all — Firefox has a minimum width and stops there, which is issue
/// #115 — and [`Hold::fill`] answers that by taking the stretch away, because a
/// buffer smeared towards a size nothing will ever agree to only ever gets
/// softer.
///
/// That is the right answer for a refusal and the wrong one for a **rounding**.
/// A terminal answers a configure with the nearest whole number of character
/// cells, so it is a few pixels off *every single time*; with no distinction
/// drawn, kitty — the ordinary tiled client — is declined from its first answer
/// onward and [`Fill::Hold`] is forced for the whole of every seam drag. The
/// user's configured `stretch` would then never once apply to a terminal, and
/// what they would see instead is `Fill::Hold`'s deliberately uncovered strip:
/// a *different* wrong-looking frame rather than no wrong-looking frame, which
/// is the symptom this whole issue is about.
///
/// **A proportion, because what it is bounding is the stretch left over.** If
/// the client settles `asked / given` away from the rectangle the pane is
/// drawing, the buffer is scaled by exactly that ratio for as long as the hold
/// lasts, and five percent of an edge is not a blur anybody can see. Judging it
/// in pixels instead would call the same ratio a refusal on a small pane and a
/// rounding on a large one, which is backwards.
///
/// Mis-judging it is bounded on both sides, which is why a proportion is safe
/// enough to pick. Calling a real refusal a rounding costs at most a five
/// percent stretch, and only until the hold ends — `declined` is still recorded,
/// so [`Hold::settle`] still adopts and the hold still ends, on the deadline at
/// the latest. Calling a rounding a refusal costs the configured fill on every
/// terminal drag there will ever be.
const ROUNDING: i32 = 20;

/// The floor under [`ROUNDING`], in pixels: the tallest character cell an
/// ordinary font produces.
///
/// A proportion alone is wrong at the small end. Five percent of a two-hundred
/// pixel pane is ten pixels and a cell is twice that at a comfortable size, so
/// a terminal in a four-way split would round by more than the proportion
/// allows and be called a refusal — which is precisely the case this is here to
/// stop being called one. The number is a font metric and nothing cleverer:
/// cell heights run about sixteen to twenty-four pixels at the sizes people
/// read code at, and the floor has to clear the top of that range to be worth
/// having.
const CELL: i32 = 24;

/// How many configures a client may leave unanswered before it is judged
/// against the ask the drag has reached rather than the one it last replied to.
///
/// [`Hold::refused`] has to weigh the pair `render.rs` is actually stretching
/// by — the rectangle the pane is drawing against the size the client last
/// committed — and the first of those keeps moving for as long as the gesture
/// does. Weighing the ask the client *answered* instead freezes the verdict at
/// that answer, because [`Hold::note`] returns early when the client has
/// committed nothing new: Firefox stops at its minimum width, answers one ask
/// ten pixels off it, [`ROUNDING`] rightly calls that a rounding, and the
/// verdict is then reasserted unchanged while the drag runs on another four
/// hundred pixels. The stretch that bought is bounded only by the release and
/// [`PATIENCE`], which is the permanent blur this module's documentation says
/// shipped once already.
///
/// **Judging the live ask the moment it moves is the other wrong answer**, and
/// it is the expensive one. The throttle sends a whole [`TELL_EVERY`] of travel
/// in one step and the answer comes a frame or two later, so a client that is
/// keeping up is an interval behind for those frames — every interval, on every
/// drag. That would read as a refusal once per interval and put one frame of
/// [`Fill::Hold`]'s deliberately uncovered strip on screen each time: the band
/// of background [`ROUNDING`] exists to keep off a terminal's drags, back at
/// ten hertz instead of for the whole gesture.
///
/// So the unit is the throttle's own. **One** unanswered configure is the
/// ordinary state of a client that is keeping up, and **two** means a whole
/// interval went by with nothing said at all — which is not a client that is a
/// frame behind, and the ask it is not answering is the live one.
const SILENCE: u32 = 2;

/// Whether `given` is near enough to `asked` to be a rounding. See [`ROUNDING`].
fn rounds(asked: Size<i32, Logical>, given: Size<i32, Logical>) -> bool {
    let near = |asked: i32, given: i32| {
        let slack = (asked.abs() / ROUNDING).max(CELL);
        given.abs_diff(asked) <= slack.unsigned_abs()
    };
    near(asked.w, given.w) && near(asked.h, given.h)
}

impl Hold {
    /// Take charge of a window's size, having just asked it for `asked`.
    ///
    /// **`released` is an argument and not a default**, and that is the whole
    /// of the fix for a hold that could never be let go of. A hold is not
    /// always born in the middle of its gesture: `input::resize`'s `motion`
    /// only *records* a request and `state::Solium::hold_resize` turns it into
    /// a hold at the frame, so a press, a motion and a release that all land in
    /// one dispatch batch — an ordinary quick nudge of a border, well inside
    /// sixteen milliseconds — produce a hold whose gesture has already ended.
    /// [`Self::settle`] answers `Waiting` unconditionally while `released` is
    /// `None`, so such a hold is **permanent**: the pane's slot outranks its
    /// client for ever and every later size the client chooses for itself is
    /// stretched into a rectangle from a drag that finished long ago.
    ///
    /// Making it a parameter is deliberately not the same as guarding the one
    /// call site that had the bug. A creation site that does not know whether
    /// its gesture is still going cannot compile, so the next one added has to
    /// answer the question rather than inherit `None` by omission.
    pub(crate) const fn new(
        edges: ResizeEdge,
        client: Size<i32, Logical>,
        asked: Rectangle<i32, Logical>,
        now: Duration,
        released: Option<Duration>,
    ) -> Self {
        Self {
            edges,
            since: client,
            asked,
            told: now,
            released,
            declined: None,
            // **One, not none**, because both callers send the configure this
            // hold is built around — `offers_first_size` returns `told` true
            // beside it and `hold_resize` calls `size_window` on the line above
            // — so a configure really is out and unanswered before the first
            // frame this hold sees.
            //
            // It cannot reach a verdict, and saying so is the point. The only
            // reader is [`Self::refused`], which short-circuits while
            // [`Self::declined`] is `None`, and the only thing that sets
            // `declined` is [`Self::note`], which zeroes this counter first —
            // so every value before the client's first answer is discarded
            // unread. Counted honestly anyway, because a field that is right
            // only where nobody looks is one refactor away from being wrong
            // where somebody does.
            unanswered: 1,
        }
    }

    /// Whether this drag is pulling the left edge, and the top.
    ///
    /// The edges whose *opposite* number is standing still, which is what a
    /// picture held at its own size has to be anchored against — see
    /// [`Fill::Hold`] — and the same question [`Self::anchored`] asks when a
    /// client's own size wins.
    pub(crate) const fn pins(&self) -> (bool, bool) {
        (pulls_left(self.edges), pulls_top(self.edges))
    }

    /// The rectangle this client was last told to be, for the trace.
    ///
    /// The one number a report of "it still looks wrong" cannot be settled
    /// without: everything else in the log says what the compositor decided,
    /// and this says what the client was actually asked for, which is the only
    /// way to tell a stretch that is waiting for an answer from one that is
    /// waiting for a question. See [`trace`].
    pub(crate) const fn asked(&self) -> Rectangle<i32, Logical> {
        self.asked
    }

    /// How many configures this client owes an answer to. See [`SILENCE`].
    ///
    /// For the trace and nothing else. [`Self::refused`] weighs two different
    /// pairs either side of [`SILENCE`], and they fail in opposite directions:
    /// a stale-`declined` verdict is the runaway stretch, a silence verdict is
    /// the uncovered strip. A log that records only which answer came out
    /// cannot tell them apart — so the one column saying *why* would be
    /// missing from exactly the runs recorded to find out why.
    pub(crate) const fn unanswered(&self) -> u32 {
        self.unanswered
    }

    /// The same gesture, placed by the compositor's other authority now.
    ///
    /// `settle_resize` forks per frame and not per gesture, so one pane moves
    /// between `Solium::resize_hold` and `Solium::resize_bridge` whenever a
    /// layout changes its mind about claiming the drag — `scrolling.lua`'s
    /// guard at screen x 0 is one frame of exactly that. Destroying the hold
    /// and building a fresh one across that boundary threw away
    /// [`Self::told`], so each flip bought an unthrottled configure in each
    /// direction and a handler that alternated restored the sixty a second
    /// [`TELL_EVERY`] exists to remove. It is the same client, the same
    /// gesture and the same throttle; only the edges are the other authority's
    /// to name, because a tiled pane's moved edge is its own and a floating
    /// one's is the pointer's. See [`moved_edges`].
    pub(crate) const fn retargeted(&mut self, edges: ResizeEdge) {
        self.edges = edges;
    }

    /// Notice whatever the client has said since it was last spoken to.
    ///
    /// **This is the half of the answer that stops a refused resize stretching
    /// for ever.** The client has committed a size that is not the one it had
    /// when we spoke to it, so it has answered; whether it answered with what
    /// it was asked for decides only whether the bridge is measuring a delay or
    /// a refusal. A refusal stops the stretch immediately — [`Self::fill`]
    /// forces [`Fill::Hold`] — because a client that is not going to reach the
    /// size it was offered will never bring the factor back to 1, and a
    /// stretched window that never un-stretches is worse than the shake this
    /// module was written to remove.
    fn note(&mut self, client: Size<i32, Logical>) {
        if client == self.since {
            return;
        }
        self.since = client;
        // It has spoken, so whatever it was told before this is answered for
        // and the count of intervals it has been silent for starts again. See
        // [`SILENCE`].
        self.unanswered = 0;
        if client == self.asked.size {
            // It took what it was offered, so whatever it declined earlier in
            // this gesture it is tracking now: a drag that went under Firefox's
            // minimum width and came back out of it stretches again.
            self.declined = None;
        } else {
            self.declined = Some(self.asked.size);
        }
    }

    /// Offer the client a rectangle. Returns it if it goes out now.
    ///
    /// The one place a configure is decided, with the throttle as a parameter
    /// rather than as two copies of this arithmetic: [`Self::dragged`] and
    /// [`Self::placed`] differ in exactly that and in nothing else, and a
    /// second spelling of "have we already told it this" is how the middle of a
    /// gesture comes to disagree with the end of it.
    fn offered(
        &mut self,
        wanted: Rectangle<i32, Logical>,
        client: Size<i32, Logical>,
        now: Duration,
        throttled: bool,
    ) -> Option<Rectangle<i32, Logical>> {
        self.note(client);
        if wanted == self.asked {
            return None;
        }
        // **A motion after the release is still part of the gesture, and the
        // throttle must not get the last word on it.**
        //
        // This is not an edge case, it is the ordinary end of every drag. A
        // grab's callbacks run during input dispatch and `settle_resize` runs
        // at the frame, so the last motion before the button came up is
        // routinely settled *after* `release` has already been called. Letting
        // the throttle swallow it would leave `asked` naming a size the client
        // was never told, and `settle` would then wait out `PATIENCE` for an
        // answer that cannot come and end the gesture with a snap.
        if throttled && self.released.is_none() && now.saturating_sub(self.told) < TELL_EVERY {
            return None;
        }
        self.asked = wanted;
        self.told = now;
        // One more the client owes an answer to. Saturating because the only
        // thing a count this large could do is wrap back under [`SILENCE`] and
        // hand a silent client its stretch back, and a hold that has sent four
        // billion configures has been alive for thirteen years.
        //
        // **Configures, not intervals**, and those are the same number only on
        // the throttled path. [`Self::placed`] arrives here with `throttled`
        // false — a reload, a workspace switch, a pane shoved aside by someone
        // else's drag — so a burst of them can reach [`SILENCE`] without a
        // whole [`TELL_EVERY`] having passed. That is the safe direction: it
        // weighs a quiet client against the live ask sooner, which is the
        // verdict that stops a runaway stretch rather than the one that opens
        // an uncovered strip. Worth knowing before anyone reads this as a clock.
        self.unanswered = self.unanswered.saturating_add(1);
        // `declined` is deliberately *not* cleared here. A new offer is not an
        // answer: what is on screen is still whatever the client last chose for
        // itself, and resuming the stretch on the strength of having asked
        // again would smear that buffer towards a size nothing has agreed to.
        // Only an answer clears it — see [`Self::note`].
        Some(wanted)
    }

    /// The drag moved this pane. Returns the rectangle to tell the client, if
    /// it is time.
    ///
    /// `wanted` is the client rectangle the drag is asking for this frame — the
    /// pane has already taken it; this only decides whether the client hears
    /// about it yet. See [`TELL_EVERY`] for why not every frame.
    pub(crate) fn dragged(
        &mut self,
        wanted: Rectangle<i32, Logical>,
        client: Size<i32, Logical>,
        now: Duration,
    ) -> Option<Rectangle<i32, Logical>> {
        self.offered(wanted, client, now, true)
    }

    /// Something that is **not** the drag moved this pane. Returns the
    /// rectangle to tell the client, whatever the throttle would have said.
    ///
    /// [`TELL_EVERY`] is a live gesture's own rate limit and it has no business
    /// swallowing anybody else's single configure. A config reload, a
    /// `modes.use` from a keybinding, a workspace switch and `rescue_offscreen`
    /// all reach `Solium::move_pane`, and one of them landing on a pane that
    /// happens to be mid-bridge — or inside the quarter second a released
    /// bridge is still waiting out — used to have its one and only configure
    /// dropped on the floor while `move_pane` went on writing the slot. The
    /// pane was then drawn, stretched, at a rectangle its client had never been
    /// told about, for the rest of the gesture.
    ///
    /// Recorded in the hold rather than sent behind its back, because a hold
    /// whose `asked` names a rectangle the client was not the last to be told
    /// would wait out [`PATIENCE`] for an answer to a question nobody asked.
    pub(crate) fn placed(
        &mut self,
        wanted: Rectangle<i32, Logical>,
        client: Size<i32, Logical>,
        now: Duration,
    ) -> Option<Rectangle<i32, Logical>> {
        self.offered(wanted, client, now, false)
    }

    /// The pointer let go. Returns the rectangle to tell the client, always.
    ///
    /// Unconditional, whatever the throttle would have said — and unconditional
    /// even when it repeats [`Self::asked`], because a release is also where a
    /// pane's *position* is reconciled for an X11 client. Wherever
    /// [`TELL_EVERY`] happened to land, the last thing the client hears has to
    /// be the rectangle the gesture actually ended on, or the window settles at
    /// the last throttled size and the final few pixels of the drag are lost.
    ///
    /// **Not counted against [`SILENCE`]**, though it is a configure like any
    /// other. What that count is for is an ask running away from a client that
    /// has stopped answering, and the ask stops running away here: from this
    /// point [`Self::settle`] bounds the stretch at [`PATIENCE`] whatever the
    /// client does. Counting it would only let a drag that ended in the few
    /// milliseconds between a configure and its answer flash [`Fill::Hold`]'s
    /// uncovered strip at the one moment the user is looking at the result.
    pub(crate) fn release(
        &mut self,
        wanted: Rectangle<i32, Logical>,
        client: Size<i32, Logical>,
        now: Duration,
    ) -> Rectangle<i32, Logical> {
        self.note(client);
        self.asked = wanted;
        self.told = now;
        self.released = Some(now);
        wanted
    }

    /// Where this hold stands. See [`Settle`].
    pub(crate) fn settle(&mut self, client: Size<i32, Logical>, now: Duration) -> Settle {
        self.note(client);
        let Some(released) = self.released else {
            // The pointer is still down. The drag owns the rectangle for as
            // long as that lasts, whatever the client has or has not said.
            return Settle::Waiting;
        };
        // Asked *and* answered, or asked for the size it already had — which is
        // the same thing from here and is why this compares the size rather
        // than watching for a commit.
        if client == self.asked.size {
            return Settle::Done;
        }
        // It has already been offered exactly this and declined it — a minimum
        // width, a cell grid, an aspect ratio — so the answer is in and waiting
        // for it again would only add a quarter of a second of squashed window
        // to the end of the gesture. This is the common shape of a refused
        // drag: people stop moving the pointer before they let go of the
        // button, so the size the release offers is the size the throttle last
        // offered, which is the size that came back refused.
        // Any mismatch at all, including the cell-grid rounding [`ROUNDING`]
        // refuses to call a refusal: the client's own size wins at the end of a
        // gesture whether it missed by four pixels or by two hundred, and
        // waiting a further quarter second to be told the same thing again is
        // only latency.
        if self.declined == Some(self.asked.size) {
            return Settle::Adopt(client);
        }
        // Nothing at all. Give up rather than hold the space and the slot apart
        // for ever; see [`PATIENCE`].
        if now.saturating_sub(released) >= PATIENCE {
            return Settle::Adopt(client);
        }
        Settle::Waiting
    }

    /// The rectangle a `slot` becomes when the client's own `size` wins.
    ///
    /// Pins the edges the drag is not holding, which is the same rule
    /// [`crate::input::resize`] applies every frame and for the same reason: a
    /// client that refuses to go under 450 wide while a *left* edge is being
    /// dragged must give those pixels back on the left, not push its right edge
    /// across the desktop. Adopting the size without this would end the gesture
    /// by moving the one edge the user was not touching — issue #113 again, at
    /// the last frame instead of every frame.
    ///
    /// **Floored at one pixel each way**, which is the same floor `state::inner`
    /// applies and the same thing `Panes::sync` and `Solium::pane_geometry`
    /// refuse to believe. A client's size is nothing at all between unmapping
    /// and its next buffer, and a client that unmaps while the deadline is
    /// running would otherwise hand the pane a slot of no size — pinned, by the
    /// arithmetic below, to the corner the drag was not holding, and mapped
    /// there.
    pub(crate) fn anchored(
        &self,
        slot: Rectangle<i32, Logical>,
        size: Size<i32, Logical>,
    ) -> Rectangle<i32, Logical> {
        let size = Size::from((size.w.max(1), size.h.max(1)));
        let x = if pulls_left(self.edges) {
            slot.loc.x + slot.size.w - size.w
        } else {
            slot.loc.x
        };
        let y = if pulls_top(self.edges) {
            slot.loc.y + slot.size.h - size.h
        } else {
            slot.loc.y
        };
        Rectangle::new((x, y).into(), size)
    }

    /// Hand this hold to a gesture that has just started on the same window.
    ///
    /// **Only [`Self::released`] changes, and deliberately not [`Self::told`].**
    /// A hold that outlives its own gesture is a hold whose deadline is
    /// running, and letting that deadline expire in the middle of the *next*
    /// gesture adopts whatever size the client happened to be at — the shake
    /// back again with a longer period, which is why `Solium::begin_resize`
    /// reconciles the floating hold rather than inheriting it. A tiled pane
    /// cannot be reconciled the same way without snapping every pane the
    /// previous drag moved off its tile, so the deadline is stopped instead:
    /// the new gesture owns these panes and will keep placing them. Leaving
    /// `told` alone is what stops the handover costing a throttle interval of
    /// silence at the one moment a drag has just started.
    pub(crate) const fn rearm(&mut self, released: Option<Duration>) {
        self.released = released;
    }

    /// Whether the client has walked away from what it is being offered, as
    /// opposed to merely landing near it.
    ///
    /// The distinction is [`ROUNDING`]'s and the whole of its reasoning is
    /// there. In one line: a terminal's answer is a whole number of character
    /// cells and so is a few pixels out every time, and calling that a refusal
    /// takes the user's configured fill away from every terminal drag there
    /// will ever be.
    ///
    /// Measured against [`Self::since`] — the size the client actually
    /// committed — rather than against a flag set at the time, so that a hold
    /// carried across a claimed/unclaimed flip carries its verdict with it
    /// instead of re-deriving one from state it no longer has.
    ///
    /// **And against the newest ask the client has had its say on, which is not
    /// always the one it answered.** `since` and [`Self::declined`] are both
    /// frozen by [`Self::note`]'s early return, so a client that answers once
    /// and then says nothing again pins this verdict at the moment of that
    /// answer while the drag — and the rectangle `render.rs` is stretching the
    /// client's buffer into — walks away from it. Once the client has been
    /// silent through a whole [`TELL_EVERY`] the live ask is what it is not
    /// answering, and that is the pair to weigh. [`SILENCE`] is the whole of
    /// why it is not weighed sooner: for a frame or two after every configure,
    /// a client that is keeping up perfectly well looks exactly like one that
    /// has stopped.
    pub(crate) fn refused(&self) -> bool {
        self.declined.is_some_and(|answered| {
            let judged = if self.unanswered >= SILENCE {
                self.asked.size
            } else {
                answered
            };
            !rounds(judged, self.since)
        })
    }

    /// What actually fills the pane, given what the configuration asked for.
    ///
    /// A refusal overrides the setting. See [`Self::declined`]: the client is
    /// not tracking what it is being offered, so a stretch would never return
    /// to 1 and the window would stay soft until something else resized it.
    ///
    /// **A rounding does not**, which is [`Self::refused`] and not
    /// `declined.is_some()`. `Fill::Hold` deliberately leaves an uncovered
    /// strip while a pane grows, so forcing it for a client that is four pixels
    /// off its ask trades a stretch nobody can see for a band of background
    /// nobody asked for.
    pub(crate) fn fill(&self, configured: Fill) -> Fill {
        if self.refused() {
            Fill::Hold
        } else {
            configured
        }
    }
}

/// How much a client's last buffer is scaled by to fill the rectangle it is
/// drawn in.
///
/// `fill` is `None` for a pane that is not mid-drag, and that case must stay
/// exactly what it has always been: the ratio between a pane's drawn rectangle
/// and its client's real one is also how a mode enlarges a window, so an
/// overview thumbnail comes through here and it is free to grow. Only a pane
/// under a live [`Hold`] is clamped, and only when the configuration asked for
/// a fill that does not stretch.
///
/// **"Mid-drag" means holding, not being dragged**, and until issue #123 those
/// were different things. A *tiled* pane was being dragged and had no hold, so
/// it took the `None` arm — the free-to-grow thumbnail arm — and a client
/// sitting at its minimum width was stretched without limit for the whole of a
/// gesture, with no `declined` bookkeeping anywhere to notice. The sentence
/// above was describing the floating path and reading as though it described
/// the function. Every pane a live gesture moves now carries a hold, so it
/// describes the function again.
pub(crate) fn factor(
    fill: Option<Fill>,
    drawn: Size<f64, Logical>,
    real: Size<i32, Logical>,
) -> (f64, f64) {
    let (across, down) = (ratio(drawn.w, real.w), ratio(drawn.h, real.h));
    match fill {
        None | Some(Fill::Stretch) => (across, down),
        // Held at its own size where there is room for it, scaled down where
        // there is not: nothing here can clip, and an oversized buffer drawn
        // unclipped spills over the neighbouring window. See [`Fill::Hold`].
        Some(Fill::Hold | Fill::Scene) => (across.min(1.0), down.min(1.0)),
    }
}

/// Which of a pane's *own* edges a rectangle change moved.
///
/// **Not the pointer's edges, and that is the whole reason this exists.** A
/// tiled drag moves a seam, and a seam is at least two panes: the one under the
/// pointer and its neighbour across the seam. The neighbour's moved edge is the
/// opposite one — the user pulls their window's right edge and the pane to the
/// right of it has its *left* edge pulled — and a corner drag moves two seams,
/// so up to four panes each have their own answer. Handing a neighbour's hold
/// the pointer's `edges` would pin [`Hold::anchored`] and [`Fill::Hold`]'s
/// slack against the wrong side of it: the picture would travel with the
/// pointer inside a window the user is not touching, and a refusal would give
/// the pixels back on the edge that never moved.
///
/// An edge is "moved" when its coordinate changed. **Both edges of one axis
/// moving names neither**, which is not a fallback but the honest answer: there
/// is no stationary edge on that axis to hang a held picture against, so
/// [`Hold::pins`] says false and `render.rs` draws at the rectangle's own
/// corner. A pane merely pushed sideways by someone else's drag is exactly that
/// case on one axis and exactly nothing on the other, and it wants no anchoring
/// at all.
pub(crate) fn moved_edges(
    before: Rectangle<i32, Logical>,
    after: Rectangle<i32, Logical>,
) -> ResizeEdge {
    let moved = |before: (i32, i32), after: (i32, i32)| {
        // `.1` is the far edge: the near coordinate plus the extent, which is
        // the coordinate that moves when a window grows without moving.
        (
            before.0 != after.0,
            before.0 + before.1 != after.0 + after.1,
        )
    };
    let (left, right) = moved((before.loc.x, before.size.w), (after.loc.x, after.size.w));
    let (top, bottom) = moved((before.loc.y, before.size.h), (after.loc.y, after.size.h));
    match (
        left && !right,
        right && !left,
        top && !bottom,
        bottom && !top,
    ) {
        (true, _, true, _) => ResizeEdge::TopLeft,
        (true, _, _, true) => ResizeEdge::BottomLeft,
        (true, ..) => ResizeEdge::Left,
        (_, true, true, _) => ResizeEdge::TopRight,
        (_, true, _, true) => ResizeEdge::BottomRight,
        (_, true, ..) => ResizeEdge::Right,
        (_, _, true, _) => ResizeEdge::Top,
        (_, _, _, true) => ResizeEdge::Bottom,
        _ => ResizeEdge::None,
    }
}

/// A per-frame record of what a drag did to one pane, for when a report of
/// "it still stutters" needs numbers rather than another theory.
///
/// # What a line says
///
/// Three sites, told apart by the first word so `grep` can separate them, and
/// between them they carry every number the three faults #123's review found
/// are distinguished by.
///
/// * `drawn`, from `render.rs`, **once per pane per frame and unconditionally**
///   — the only one of the three that is on every frame, because the other two
///   are reached only from a frame that placed something. `frame` is the outer
///   rectangle drawn, `slot` the pane's client rectangle, `committed` what the
///   client last agreed to, `factor` the stretch its buffer is drawn at, `fill`
///   the mode actually chosen once [`Hold::fill`] has had its say, and `held`
///   whether a hold is live.
/// * `layout`, from `Solium::offers_size`, once per pane a placement reached.
///   Adds `asked` — the rectangle the client was last *told*, which is the one
///   number nothing else can supply — `told`, whether a configure went out on
///   this frame, `refused`, whether the client's answer was far enough off to
///   take the stretch away (see [`ROUNDING`]), and `unanswered`, how many
///   configures the client still owes — which says *which* of
///   [`Hold::refused`]'s two rules produced that verdict.
/// * `flush`, from `Solium::flush_resize`, when the throttle's trailing edge
///   sends what a paused drag was sitting on. The same fields as `layout`.
///
/// So each of the three faults reads as a shape rather than as a guess. A
/// **tail** is `drawn` lines whose `slot` has moved away from `committed` with
/// no `layout` or `flush` line between them and `asked` standing still. A
/// **pane moved without being resized** is a `slot` whose origin walks at an
/// unchanged size with `asked` not following. A **rounding mistaken for a
/// refusal** is `refused=1` with `committed` a handful of pixels from `asked`,
/// and `fill` reading `Hold` in a session configured for `Stretch`.
///
/// **`unanswered` is what tells the last of those from its opposite**, because
/// [`Hold::refused`] weighs a different pair either side of [`SILENCE`] and the
/// two go wrong in opposite directions. Below it the verdict is against the ask
/// the client answered, and its failure is a stretch that runs away unchecked;
/// at or above it the verdict is against the live ask, and its failure is an
/// uncovered strip on a client that was merely a frame behind. Both print
/// `refused=` and nothing else distinguishes them, so a log recorded to settle
/// which one is happening has to carry the count that decided it.
///
/// **Off unless [`VARIABLE`] names a file**, and off is one relaxed atomic load
/// on a path that runs once per pane per frame. Not `tracing`: the compositor's
/// subscriber is wired for a session's diagnostics and this is a stream of tens
/// of lines a frame that wants to land somewhere a person can `awk` at. Not
/// `/tmp` either — the user's Xwayland clears it — so the variable takes a path
/// and the caller chooses one under `$XDG_RUNTIME_DIR` or the worktree.
///
/// Appends and never truncates, so two runs of a compositor that was restarted
/// mid-investigation are both still there, and writes unbuffered so a session
/// that ends in a hang has its last frame on disk.
pub(crate) mod trace {
    use std::{
        fs::OpenOptions,
        io::Write,
        sync::{Mutex, OnceLock},
    };

    /// The environment variable naming the file to append to.
    pub(crate) const VARIABLE: &str = "SOLIUM_RESIZE_TRACE";

    static SINK: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();

    fn sink() -> Option<&'static Mutex<std::fs::File>> {
        SINK.get_or_init(|| open(std::env::var_os(VARIABLE)?))
            .as_ref()
    }

    /// Open one trace file, warning rather than failing if it cannot be opened.
    ///
    /// Split from [`sink`] so it can be tested. A `OnceLock` latches for the
    /// life of the process and `cargo test` is one process, so a test that set
    /// the variable would be racing every other test for who initialises it
    /// first — and the half worth testing is this one, where a mistyped
    /// directory or a read-only path decides whether the user gets numbers or
    /// silence.
    fn open(path: std::ffi::OsString) -> Option<Mutex<std::fs::File>> {
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => Some(Mutex::new(file)),
            Err(err) => {
                tracing::warn!(?err, ?path, "could not open the resize trace");
                None
            }
        }
    }

    /// Whether anything is listening, for a caller that would have to compute
    /// something to say.
    pub(crate) fn on() -> bool {
        sink().is_some()
    }

    /// One line. `what` names the site so two sites can be told apart by
    /// `grep`; the rest is `key=value` pairs.
    pub(crate) fn line(what: &str, fields: std::fmt::Arguments<'_>) {
        let Some(sink) = sink() else {
            return;
        };
        // A poisoned lock means some other thread panicked mid-line. Losing a
        // diagnostic is not worth taking a compositor down for, and the lints
        // that forbid `unwrap` here are the same rule said once.
        let Ok(mut file) = sink.lock() else {
            return;
        };
        let _ = writeln!(file, "{what} {fields}");
    }

    #[cfg(test)]
    mod tests {
        use std::io::Write as _;

        /// The trace appends rather than truncating, and a path it cannot open
        /// costs a warning rather than a session.
        ///
        /// Both halves are the difference between the user running a drag and
        /// sending back numbers, and the user running a drag and sending back
        /// nothing — which is where this issue started.
        #[test]
        fn a_trace_appends_to_its_file_and_survives_one_it_cannot_open() {
            let path = std::env::temp_dir().join(format!(
                "solium-resize-trace-test-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::write(&path, "earlier\n").expect("seeding the trace file");

            let sink = super::open(path.clone().into_os_string()).expect("opening the trace");
            writeln!(sink.lock().expect("locking the trace").by_ref(), "later")
                .expect("writing the trace");
            assert_eq!(
                std::fs::read_to_string(&path).expect("reading the trace back"),
                "earlier\nlater\n",
                "a second run of a restarted compositor must not erase the \
                 first one's frames"
            );
            std::fs::remove_file(&path).expect("removing the trace file");

            assert!(
                super::open(path.join("not-a-directory").into_os_string()).is_none(),
                "a path that cannot be opened turns the trace off, and takes \
                 nothing else with it"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Fill, Hold, PATIENCE, Settle, TELL_EVERY, factor, moved_edges};
    use smithay::{
        reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge,
        utils::{Logical, Rectangle, Size},
    };

    fn size(w: i32, h: i32) -> Size<i32, Logical> {
        Size::from((w, h))
    }

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    const fn ms(millis: u64) -> std::time::Duration {
        std::time::Duration::from_millis(millis)
    }

    /// A client rectangle of this size at one fixed origin.
    ///
    /// A hold's `asked` is the whole rectangle, because a configure carries a
    /// position for an X11 client and a pane that only *moves* has to be told
    /// about it — see `Hold::asked`. Nearly every test in this module is about
    /// what a client answers, which is a size and has no position in it, so
    /// they name a size and this puts it somewhere. The origin is the same for
    /// every call on purpose: a test that meant to change the size and changed
    /// the position as well would be asking a different question.
    fn want(w: i32, h: i32) -> Rectangle<i32, Logical> {
        rect(100, 100, w, h)
    }

    /// A hold born in the middle of its gesture, which is the ordinary case and
    /// the one nearly every test below is about.
    ///
    /// Named rather than passing `None` eight times, so that the one test that
    /// passes something else stands out as the case it is.
    fn dragging(
        edges: ResizeEdge,
        client: Size<i32, Logical>,
        asked: Rectangle<i32, Logical>,
        now: std::time::Duration,
    ) -> Hold {
        Hold::new(edges, client, asked, now, None)
    }

    /// The arithmetic of issue #113, and the bridge that makes the fix
    /// drawable.
    ///
    /// The end-to-end version of this — a real client, through `settle_resize`
    /// and `pane_geometry` — is
    /// `state::tests::real_client::the_edge_being_dragged_is_the_edge_that_moves_before_any_client_answers`,
    /// and that is the one that would go red on `stage`. This one is here for
    /// the half that test cannot see: the factor the client's last buffer is
    /// drawn at while the pane is ahead of it, which is what lets the pane be
    /// right before the client is.
    #[test]
    fn the_dragged_edge_moves_first_and_the_other_one_does_not_move_at_all() {
        let began = rect(100, 100, 400, 300);
        // A top-left drag of 50 right and 40 down: smaller, and moved.
        let wanted = rect(150, 140, 350, 260);
        // The client has committed nothing. This is its size, still.
        let client = size(400, 300);

        // What the pane draws now that its slot is authoritative.
        let pane = wanted;
        assert_eq!(
            (pane.loc.x + pane.size.w, pane.loc.y + pane.size.h),
            (began.loc.x + began.size.w, began.loc.y + began.size.h),
            "the bottom-right edge is not being dragged and must not move"
        );

        // And the control: `stage` pairs the new origin with the old size.
        let on_stage = Rectangle::new(wanted.loc, client);
        assert_eq!(
            (
                on_stage.loc.x + on_stage.size.w,
                on_stage.loc.y + on_stage.size.h
            ),
            (550, 440),
            "stage moves the bottom-right corner by the drag's whole delta, \
             which is the shake #113 reports"
        );
        assert_ne!(
            (
                on_stage.loc.x + on_stage.size.w,
                on_stage.loc.y + on_stage.size.h
            ),
            (began.loc.x + began.size.w, began.loc.y + began.size.h),
        );

        // And the bridge that makes the pane's rectangle drawable: the client's
        // old buffer scaled into it, which is how the pane can be right before
        // the client is.
        let (across, down) = factor(Some(Fill::Stretch), pane.size.to_f64(), client);
        assert!((across - 350.0 / 400.0).abs() < f64::EPSILON);
        assert!((down - 260.0 / 300.0).abs() < f64::EPSILON);
    }

    /// The bridge ends when the client commits what it was asked for, and what
    /// it ends at is a factor of exactly 1 — pixel-exact, not nearly.
    #[test]
    fn the_stretch_ends_when_the_client_commits_the_size_it_was_asked_for() {
        let mut hold = dragging(
            ResizeEdge::BottomRight,
            size(400, 300),
            want(500, 380),
            ms(0),
        );
        // Mid-drag the client has said nothing, so the hold stands.
        assert_eq!(hold.settle(size(400, 300), ms(8)), Settle::Waiting);
        // The pointer lets go and the client is told the final size.
        assert_eq!(
            hold.release(want(520, 400), size(400, 300), ms(200)),
            want(520, 400)
        );
        assert_eq!(hold.settle(size(400, 300), ms(216)), Settle::Waiting);
        // It answers with exactly that.
        assert_eq!(hold.settle(size(520, 400), ms(280)), Settle::Done);
        let (across, down) = factor(Some(Fill::Stretch), size(520, 400).to_f64(), size(520, 400));
        assert!((across - 1.0).abs() < f64::EPSILON);
        assert!((down - 1.0).abs() < f64::EPSILON);
    }

    /// **The trap.** A client that answers with something other than what it
    /// was asked for has still answered, and the stretch has to end there.
    ///
    /// Firefox has a minimum width and will not go under it (#115, open:
    /// Solium never reads `min_size`). If the bridge waited for the size it
    /// asked for it would wait for ever, and the window would be blurry until
    /// something else resized it — a worse bug than the shake.
    #[test]
    fn a_refused_size_stops_the_stretch_instead_of_stretching_for_ever() {
        let mut hold = dragging(ResizeEdge::Left, size(800, 600), want(300, 600), ms(0));
        assert_eq!(
            hold.fill(Fill::Stretch),
            Fill::Stretch,
            "nothing refused yet"
        );

        // Firefox answers with its minimum, mid-drag, which is not what it was
        // asked for.
        assert_eq!(hold.settle(size(450, 600), ms(120)), Settle::Waiting);
        assert_eq!(
            hold.fill(Fill::Stretch),
            Fill::Hold,
            "a client that will not reach the size it was offered would never \
             bring the factor back to 1"
        );
        // And the clamp is what makes that safe to draw: the pane is 300 wide
        // and the buffer is 450, so it comes down rather than spilling.
        let (across, _) = factor(
            Some(hold.fill(Fill::Stretch)),
            size(300, 600).to_f64(),
            size(450, 600),
        );
        assert!(across < 1.0);
        // Growing past the buffer is where holding actually holds.
        let (wider, _) = factor(Some(Fill::Hold), size(900, 600).to_f64(), size(450, 600));
        assert!((wider - 1.0).abs() < f64::EPSILON);

        // The gesture ends. The client's answer wins, and the edge the drag was
        // not holding stays where it was.
        let slot = rect(200, 100, 300, 600);
        assert_eq!(
            hold.release(want(300, 600), size(450, 600), ms(300)),
            want(300, 600)
        );
        let Settle::Adopt(taken) = hold.settle(size(450, 600), ms(320)) else {
            panic!("a client that answered with a size of its own must be adopted");
        };
        assert_eq!(taken, size(450, 600));
        let landed = hold.anchored(slot, taken);
        assert_eq!(
            landed.loc.x + landed.size.w,
            slot.loc.x + slot.size.w,
            "a left drag gives the refused pixels back on the left; the right \
             edge is the one nobody touched"
        );
        assert_eq!(landed, rect(50, 100, 450, 600));
    }

    /// A refusal is not permanent, but only an *answer* lifts it.
    ///
    /// Dragging back out of Firefox's minimum stretches again — once Firefox
    /// says so. In between, asking is not answering: the buffer on screen is
    /// still the one Firefox chose for itself, and stretching it towards a size
    /// nothing has agreed to is the same mistake as stretching towards one that
    /// was refused.
    #[test]
    fn a_refusal_lifts_on_an_answer_and_not_on_a_new_offer() {
        let mut hold = dragging(ResizeEdge::Left, size(800, 600), want(300, 600), ms(0));
        hold.settle(size(450, 600), ms(100));
        assert_eq!(hold.fill(Fill::Stretch), Fill::Hold);
        // Dragged back out, and told so.
        assert_eq!(
            hold.dragged(want(700, 600), size(450, 600), ms(200)),
            Some(want(700, 600))
        );
        assert_eq!(
            hold.fill(Fill::Stretch),
            Fill::Hold,
            "the client has not answered yet; what is on screen is still its \
             own size, and 450 smeared across 700 is not an improvement"
        );
        // And now it answers.
        hold.settle(size(700, 600), ms(300));
        assert_eq!(hold.fill(Fill::Stretch), Fill::Stretch);
        // So a release at a size it never declined waits for it rather than
        // pre-empting it with the size it happened to be at.
        hold.release(want(760, 600), size(700, 600), ms(400));
        assert_eq!(
            hold.settle(size(700, 600), ms(420)),
            Settle::Waiting,
            "a client that is keeping up must be given its round trip, or the \
             last stretch of every drag is thrown away"
        );
        assert_eq!(hold.settle(size(760, 600), ms(450)), Settle::Done);
    }

    /// A client that answers nothing at all, including the one that answers
    /// nothing *because* it was asked for the size it already had.
    #[test]
    fn a_client_that_never_answers_is_given_up_on_at_the_deadline() {
        let mut hold = dragging(ResizeEdge::Bottom, size(400, 300), want(400, 420), ms(0));
        hold.release(want(400, 420), size(400, 300), ms(500));
        assert_eq!(hold.settle(size(400, 300), ms(600)), Settle::Waiting);
        assert_eq!(
            hold.settle(size(400, 300), ms(500) + PATIENCE),
            Settle::Adopt(size(400, 300)),
            "a hold nothing ends holds the space and the slot apart for ever"
        );

        // And the case that produces no commit at all: the drag came back to
        // where it started, so the configure asked for the size the client is.
        let mut same = dragging(ResizeEdge::Bottom, size(400, 300), want(400, 420), ms(0));
        same.release(want(400, 300), size(400, 300), ms(500));
        assert_eq!(
            same.settle(size(400, 300), ms(501)),
            Settle::Done,
            "asked for the size it already is, so there is nothing to wait for"
        );
    }

    /// The client is asked at [`TELL_EVERY`], not at the frame rate.
    #[test]
    fn the_client_is_not_asked_more_often_than_the_throttle_allows() {
        let mut hold = dragging(ResizeEdge::Right, size(400, 300), want(410, 300), ms(0));
        // Sixteen milliseconds is a frame. Nothing goes out.
        assert_eq!(hold.dragged(want(420, 300), size(400, 300), ms(16)), None);
        assert_eq!(hold.dragged(want(430, 300), size(400, 300), ms(32)), None);
        assert_eq!(hold.dragged(want(460, 300), size(400, 300), ms(99)), None);
        // Past the interval, one configure, carrying the latest size rather
        // than any of the ones that were skipped.
        assert_eq!(
            hold.dragged(want(470, 300), size(400, 300), ms(0) + TELL_EVERY),
            Some(want(470, 300))
        );
        // And the throttle restarts from there.
        assert_eq!(hold.dragged(want(480, 300), size(400, 300), ms(120)), None);
    }

    /// **A placement that is not the drag's own goes out whatever the interval
    /// says.**
    ///
    /// [`TELL_EVERY`] is a live gesture's rate limit. A config reload, a
    /// `modes.use` from a keybinding and `rescue_offscreen` all reach
    /// `Solium::move_pane` and have no next frame to resend anything, so a
    /// throttle that swallowed one of them would leave the pane drawn at a
    /// rectangle its client was never told about for the rest of the gesture.
    ///
    /// Through the hold rather than around it, which is the second assertion:
    /// an `asked` that does not name what the client last heard would wait out
    /// [`PATIENCE`] for an answer to a question nobody asked.
    #[test]
    fn a_placement_that_is_not_the_drags_is_not_throttled_by_it() {
        let mut hold = dragging(ResizeEdge::Right, size(400, 300), want(410, 300), ms(0));
        assert_eq!(
            hold.dragged(want(420, 300), size(400, 300), ms(16)),
            None,
            "the control: the same frame, asked for by the drag, is throttled"
        );
        assert_eq!(
            hold.placed(want(260, 180), size(400, 300), ms(16)),
            Some(want(260, 180))
        );
        assert_eq!(
            hold.asked(),
            want(260, 180),
            "the hold has to know what the client was last told, or it waits \
             for an answer to a size it never offered"
        );
        // And it is still a deduplicated offer rather than an unconditional
        // send: the same rectangle twice is nothing to say.
        assert_eq!(hold.placed(want(260, 180), size(400, 300), ms(32)), None);
    }

    /// **A pane that only *moved* is still something to tell the client.**
    ///
    /// `state::size_window` is the only thing that carries a position to an X11
    /// client, and [`Hold::asked`] is the whole rectangle for exactly that
    /// reason. Comparing sizes meant a bridged pane whose slot translated
    /// answered "nothing to say" for the rest of the gesture; `scrolling.lua`'s
    /// `widen` shifts every column sideways at an unchanged width, so that is
    /// the whole of a drag in the scrolling layout.
    #[test]
    fn a_pane_that_only_moved_is_still_told_where_it_went() {
        let mut hold = dragging(ResizeEdge::Right, size(400, 300), want(400, 300), ms(0));
        let sideways = rect(140, 100, 400, 300);
        assert_eq!(
            hold.dragged(sideways, size(400, 300), TELL_EVERY),
            Some(sideways),
            "the same size at a different origin is a real change"
        );
        assert_eq!(
            hold.dragged(sideways, size(400, 300), TELL_EVERY * 2),
            None,
            "and the same rectangle twice is not"
        );
    }

    /// **A terminal rounds; it does not refuse.** See [`ROUNDING`].
    ///
    /// The two questions `declined` answers come apart here, which is the whole
    /// of the change. A cell-grid answer is close enough that the stretch it
    /// leaves behind is invisible, so the user's configured fill stands —
    /// otherwise kitty, the ordinary tiled client, loses `stretch` on the first
    /// answer of every seam drag and gets [`Fill::Hold`]'s uncovered strip
    /// instead. It is still an answer, so the end of the gesture still adopts
    /// it: a client's own size wins by six pixels as readily as by two hundred.
    ///
    /// The boundary is asserted from both sides rather than described, because
    /// a tolerance nothing pins is a tolerance the next edit widens.
    #[test]
    fn a_cell_grid_answer_keeps_the_stretch_and_still_ends_the_gesture() {
        let mut hold = dragging(ResizeEdge::Right, size(400, 300), want(300, 200), ms(0));
        // Six across and ten down: a cell or two of an ordinary font.
        assert_eq!(hold.settle(size(294, 190), ms(50)), Settle::Waiting);
        assert!(!hold.refused());
        assert_eq!(
            hold.fill(Fill::Stretch),
            Fill::Stretch,
            "a rounding is not the refusal `declined` exists to catch"
        );
        // But it is still an answer, so the gesture ends on it rather than
        // waiting out the deadline for one that will never differ.
        hold.release(want(300, 200), size(294, 190), ms(200));
        assert_eq!(
            hold.settle(size(294, 190), ms(210)),
            Settle::Adopt(size(294, 190)),
            "the client's own size wins at the end whatever the fill did in \
             the middle"
        );

        // The other side of the line, which is what stops this being a fix that
        // simply never calls anything a refusal: Firefox's minimum width.
        let mut firefox = dragging(ResizeEdge::Left, size(800, 600), want(300, 600), ms(0));
        firefox.settle(size(450, 600), ms(50));
        assert!(firefox.refused());
        assert_eq!(firefox.fill(Fill::Stretch), Fill::Hold);

        // The proportion, on a pane big enough for it to be the term that
        // decides: a twentieth of a thousand is fifty.
        let refused = |asked: (i32, i32), given: (i32, i32)| {
            let mut hold = dragging(
                ResizeEdge::Right,
                size(asked.0, asked.1),
                want(asked.0, asked.1),
                ms(0),
            );
            hold.settle(size(given.0, given.1), ms(50));
            hold.refused()
        };
        assert!(!refused((1000, 1000), (950, 1000)), "five percent");
        assert!(refused((1000, 1000), (949, 1000)), "and past it");
        // And the floor, which is what a short pane needs: a twentieth of two
        // hundred is ten, and one row of text is more than that.
        assert!(
            !refused((300, 200), (300, 180)),
            "a rule that calls one character cell a refusal is the rule this \
             replaces"
        );
    }

    /// **A verdict about a pair that has stopped moving says nothing about the
    /// pair the renderer is stretching by.**
    ///
    /// `refused` used to ask `rounds(declined, since)`, and `Hold::note`'s
    /// `client == self.since` early return freezes *both* of those the moment
    /// the client last spoke. Firefox has a minimum width, answers an ask of
    /// 790 with 800, and [`ROUNDING`] quite rightly calls ten pixels a
    /// rounding — a verdict that was correct about the ask it was made on and
    /// is reasserted, unchanged, for every frame after it. The drag then runs
    /// on to 400 with no second commit to notice, so the verdict never moves
    /// and the pane goes on drawing a rectangle the client walked away from two
    /// hundred pixels ago.
    ///
    /// **Both halves are asserted because they fail at different depths.**
    /// `fill` is the verdict — what the trace's `refused=` column reports and
    /// what #115's guard is — and it is wrong from the first frame of the
    /// runaway. `factor` is the picture, and it is only wrong where
    /// [`Fill::Hold`] has teeth: the element path cannot clip, so a *shrinking*
    /// pane scales down under either fill and the two draw the same thing. The
    /// growing half below is the one a user sees, and it is the same fault.
    #[test]
    fn a_client_that_answers_once_and_then_says_nothing_stops_stretching() {
        // 850 wide, a left edge pulled in to 790, and 800 is as narrow as this
        // client goes.
        let mut hold = dragging(ResizeEdge::Left, size(850, 600), want(790, 600), ms(0));
        assert_eq!(hold.settle(size(800, 600), ms(20)), Settle::Waiting);
        assert!(
            !hold.refused(),
            "ten pixels off 790 is inside `ROUNDING`, and about the ask it was \
             answering that is the right answer"
        );

        // The drag runs on. The client is at its minimum and commits nothing
        // further, so `note` is never called with anything new again.
        let mut at = ms(20);
        for width in [700, 600, 500, 400] {
            at += TELL_EVERY;
            assert_eq!(
                hold.dragged(want(width, 600), size(800, 600), at),
                Some(want(width, 600)),
                "the ask keeps moving even though the answer does not"
            );
            assert_eq!(hold.settle(size(800, 600), at + ms(16)), Settle::Waiting);
        }
        assert!(
            hold.refused(),
            "the pane is drawing 400 against a client stuck at 800, and a \
             client that has been told four times and said nothing is not \
             tracking the ask by four hundred pixels"
        );
        assert_eq!(
            hold.fill(Fill::Stretch),
            Fill::Hold,
            "#115's guard is that a refusing client must not be stretched, and \
             a guard that lasts one frame of a drag is not one"
        );

        // The same shape in the direction where the fill decides the picture: a
        // terminal answers one ask a cell short — a rounding, correctly — and
        // then stops answering while the drag grows the pane past it.
        let mut hung = dragging(ResizeEdge::Right, size(400, 300), want(420, 300), ms(0));
        assert_eq!(hung.settle(size(412, 292), ms(10)), Settle::Waiting);
        assert!(!hung.refused(), "eight pixels is a cell, not a refusal");
        for (width, at) in [(620, TELL_EVERY), (820, TELL_EVERY * 2)] {
            assert_eq!(
                hung.dragged(want(width, 300), size(412, 292), at),
                Some(want(width, 300))
            );
        }
        let (across, _) = factor(
            Some(hung.fill(Fill::Stretch)),
            size(820, 300).to_f64(),
            size(412, 292),
        );
        assert!(
            (across - 1.0).abs() < f64::EPSILON,
            "a buffer drawn at twice its own size for the rest of a gesture is \
             the blur this module was written to prevent; got {across}"
        );
    }

    /// **The other side of [`SILENCE`]: a client that is still answering keeps
    /// the stretch while it does.**
    ///
    /// The ask moves a whole [`TELL_EVERY`] of travel in one step and the
    /// answer arrives a frame or two later, so a client that is keeping up
    /// perfectly well is *always* an interval behind for those frames. Judging
    /// the live ask the moment it moves would call that a refusal once per
    /// interval, and one frame of [`Fill::Hold`] is one frame of its
    /// deliberately uncovered strip — which is the band of background
    /// [`ROUNDING`] exists to keep off a terminal's drags, back again at ten
    /// hertz instead of for the whole gesture.
    #[test]
    fn a_terminal_that_is_still_answering_keeps_the_stretch_across_a_configure() {
        // A 400-wide tile grown by a hundred pixels an interval, which is an
        // ordinary flick of a seam at about a thousand pixels a second.
        let mut hold = dragging(ResizeEdge::Right, size(400, 300), want(420, 300), ms(0));
        assert_eq!(hold.settle(size(412, 292), ms(10)), Settle::Waiting);
        assert!(!hold.refused());

        assert_eq!(
            hold.dragged(want(520, 300), size(412, 292), TELL_EVERY),
            Some(want(520, 300))
        );
        assert!(
            !hold.refused(),
            "one configure in flight is what a client that is keeping up looks \
             like on the frame it is sent: the hundred pixels between the ask \
             and the answer are the throttle's, not the client's"
        );
        assert_eq!(hold.fill(Fill::Stretch), Fill::Stretch);

        // And it answers, a cell short again, which is where it was all along.
        assert_eq!(
            hold.settle(size(512, 292), TELL_EVERY + ms(10)),
            Settle::Waiting
        );
        assert!(!hold.refused());
        assert_eq!(hold.fill(Fill::Stretch), Fill::Stretch);
    }

    /// Whatever the throttle did, the gesture always ends with a configure for
    /// the size it actually ended on.
    #[test]
    fn letting_go_always_tells_the_client_where_the_drag_ended() {
        let mut hold = dragging(ResizeEdge::Right, size(400, 300), want(410, 300), ms(0));
        assert_eq!(hold.dragged(want(470, 300), size(400, 300), ms(16)), None);
        assert_eq!(
            hold.release(want(473, 300), size(400, 300), ms(20)),
            want(473, 300),
            "the last few pixels of a drag are lost if the throttle gets the \
             final word"
        );
    }

    /// **The last motion of a gesture is settled after the release.**
    ///
    /// Not an edge case: a grab's callbacks run during input dispatch and
    /// `settle_resize` runs at the frame, so this is the ordinary end of every
    /// drag. If the throttle swallowed it, `asked` would name a size the client
    /// was never told and the gesture would end by waiting out `PATIENCE` for
    /// an answer that cannot come, and then snapping.
    #[test]
    fn a_motion_settled_after_the_release_still_reaches_the_client() {
        let mut hold = dragging(ResizeEdge::Right, size(400, 300), want(400, 300), ms(0));
        // The control: six milliseconds into a live drag is throttled.
        assert_eq!(hold.dragged(want(460, 300), size(400, 300), ms(6)), None);
        // The button comes up.
        assert_eq!(
            hold.release(want(470, 300), size(400, 300), ms(50)),
            want(470, 300)
        );
        // And the frame after it settles the motion that preceded it. The same
        // six milliseconds, and now it goes out.
        assert_eq!(
            hold.dragged(want(474, 300), size(400, 300), ms(56)),
            Some(want(474, 300))
        );
        assert_eq!(
            hold.settle(size(474, 300), ms(120)),
            Settle::Done,
            "the client answered the size it was actually told"
        );
    }

    /// A pane that is not mid-drag is not clamped, whatever the setting says.
    ///
    /// The ratio between a drawn rectangle and a real one is also how a mode
    /// enlarges a window, and an overview thumbnail scaled *up* comes through
    /// the same arithmetic. Clamping it because someone set `fill = "hold"`
    /// would shrink every enlarged window on the desktop.
    #[test]
    fn a_window_that_is_not_being_dragged_is_never_clamped() {
        let (across, down) = factor(None, size(1600, 1200).to_f64(), size(800, 600));
        assert!((across - 2.0).abs() < f64::EPSILON);
        assert!((down - 2.0).abs() < f64::EPSILON);
    }

    /// **A hold born after its own gesture ended still lets go.**
    ///
    /// The whole gesture — press, motion, release — fits inside one dispatch
    /// batch whenever a border is nudged quickly, and the hold is not born
    /// until the frame after all three. A hold that took `released: None`
    /// because that is what a fresh hold usually has would then wait for a
    /// release that already happened, for ever: `settle` answers `Waiting`
    /// unconditionally without one, so nothing would ever end it.
    ///
    /// The control below is the shape of the bug rather than a description of
    /// it: the same hold with `None` is still `Waiting` a full second past a
    /// deadline of a quarter of one, and would be at any time that could be
    /// put there.
    #[test]
    fn a_hold_born_after_its_gesture_ended_is_still_let_go_of() {
        let mut hold = Hold::new(
            ResizeEdge::TopLeft,
            size(400, 300),
            want(380, 288),
            ms(16),
            Some(ms(12)),
        );
        // The client answers nothing at all, which is what the deadline is for.
        assert_eq!(hold.settle(size(400, 300), ms(20)), Settle::Waiting);
        assert_eq!(
            hold.settle(size(400, 300), ms(12) + PATIENCE),
            Settle::Adopt(size(400, 300)),
            "the deadline runs from the release, and this hold was born after it"
        );

        // And the control: the same hold that never heard about its release.
        let mut deaf = dragging(ResizeEdge::TopLeft, size(400, 300), want(380, 288), ms(16));
        assert_eq!(
            deaf.settle(size(400, 300), ms(12) + PATIENCE + ms(1000)),
            Settle::Waiting,
            "a hold that cannot observe its own release is permanent, and a \
             permanent hold is a permanently soft window"
        );
    }

    /// A client that goes away mid-deadline does not leave a slot of no size.
    ///
    /// `window.geometry().size` is nothing at all between a client unmapping
    /// and its next buffer, and the deadline can expire in exactly that gap.
    /// Without a floor the pane is given a zero-size rectangle pinned to the
    /// corner the drag was not holding — and mapped there, which is a window
    /// that cannot be grabbed to undo it.
    #[test]
    fn an_adopted_size_never_collapses_the_slot_to_nothing() {
        let hold = dragging(ResizeEdge::TopLeft, size(400, 300), want(380, 288), ms(0));
        let slot = rect(200, 100, 380, 288);
        let landed = hold.anchored(slot, size(0, 0));
        assert!(
            landed.size.w > 0 && landed.size.h > 0,
            "the same floor `inner` applies, for the same reason"
        );
        // Still pinned: a top-left drag gives everything back on the top left.
        assert_eq!(
            (landed.loc.x + landed.size.w, landed.loc.y + landed.size.h),
            (slot.loc.x + slot.size.w, slot.loc.y + slot.size.h),
        );
    }

    /// Which edges are standing still, for a picture that is not stretched.
    #[test]
    fn a_hold_says_which_edges_the_picture_has_to_stay_against() {
        let pins = |edges| dragging(edges, size(400, 300), want(400, 300), ms(0)).pins();
        assert_eq!(pins(ResizeEdge::TopLeft), (true, true));
        assert_eq!(pins(ResizeEdge::BottomRight), (false, false));
        assert_eq!(pins(ResizeEdge::Left), (true, false));
        assert_eq!(pins(ResizeEdge::Top), (false, true));
        assert_eq!(pins(ResizeEdge::BottomLeft), (true, false));
        assert_eq!(pins(ResizeEdge::TopRight), (false, true));
    }

    /// The slack `render.rs` offsets a held picture by, and the fact that a
    /// stretched one has none.
    ///
    /// A left drag that grows the pane holds the buffer at 1.0, so there is a
    /// strip the buffer does not cover; it has to open along the edge under the
    /// pointer, not along the edge standing still. A stretch covers the
    /// rectangle exactly, so the same arithmetic offsets it by nothing and the
    /// default path is untouched.
    #[test]
    fn a_held_picture_leaves_its_slack_on_the_edge_being_dragged() {
        let drawn: Size<f64, Logical> = Size::from((900.0, 600.0));
        let real = size(450, 600);
        let (across, _) = factor(Some(Fill::Hold), drawn, real);
        assert!((drawn.w - f64::from(real.w) * across - 450.0).abs() < f64::EPSILON);

        let (stretched, _) = factor(Some(Fill::Stretch), drawn, real);
        assert!(
            (drawn.w - f64::from(real.w) * stretched).abs() < f64::EPSILON,
            "a stretched buffer fills its rectangle, so anchoring it is a no-op"
        );
    }

    #[test]
    fn a_fill_is_named_in_the_configuration_or_it_is_not_one() {
        assert_eq!(Fill::named("stretch"), Some(Fill::Stretch));
        assert_eq!(Fill::named("hold"), Some(Fill::Hold));
        assert_eq!(Fill::named("scene"), Some(Fill::Scene));
        assert_eq!(Fill::named("Stretch"), None);
        assert_eq!(Fill::named(""), None);
        assert_eq!(Fill::default(), Fill::Stretch);
    }

    /// **The two panes a seam moves are pulled by opposite edges.**
    ///
    /// The scenario is the ordinary one: a vertical seam at x 700 between a
    /// pane occupying 400..700 and its neighbour occupying 700..900, and the
    /// user drags the left pane's *right* edge out to 760. The left pane's
    /// right edge moved and the right pane's *left* edge moved, and handing the
    /// second one the pointer's `Right` would anchor its held picture and any
    /// refusal against the side that did not move — the window's contents would
    /// slide inside their own frame for the length of the drag.
    #[test]
    fn a_seams_two_panes_are_pulled_by_opposite_edges() {
        assert_eq!(
            moved_edges(rect(400, 300, 300, 400), rect(400, 300, 360, 400)),
            ResizeEdge::Right,
            "the pane under the pointer keeps the edge the pointer has"
        );
        assert_eq!(
            moved_edges(rect(700, 300, 200, 400), rect(760, 300, 140, 400)),
            ResizeEdge::Left,
            "and its neighbour across the seam has the opposite one: its right \
             edge is the far wall and has not moved at all"
        );
    }

    /// **A pane merely pushed aside has no pulled edge, and must not be given
    /// one.**
    ///
    /// Both of its edges travelled by the same amount, so nothing on that axis
    /// is standing still and there is nothing for a held picture to be anchored
    /// against. Naming an edge anyway would offset the buffer by the slack
    /// `render.rs` computes, inside a window the user is not touching.
    #[test]
    fn a_pane_that_only_moved_has_no_edge_to_anchor_against() {
        let hold = Hold::new(
            moved_edges(rect(700, 300, 200, 400), rect(760, 300, 200, 400)),
            size(200, 400),
            want(200, 400),
            ms(0),
            None,
        );
        assert_eq!(hold.pins(), (false, false));
        assert_eq!(
            moved_edges(rect(400, 300, 300, 400), rect(400, 300, 300, 400)),
            ResizeEdge::None,
            "and a rectangle that did not change moved no edge either"
        );
    }

    /// A corner drag moves two seams, and each of the up-to-four panes it
    /// touches has its own pair.
    #[test]
    fn a_corner_names_both_of_its_axes() {
        assert_eq!(
            moved_edges(rect(400, 300, 300, 400), rect(400, 300, 360, 460)),
            ResizeEdge::BottomRight
        );
        assert_eq!(
            moved_edges(rect(700, 700, 200, 300), rect(760, 760, 140, 240)),
            ResizeEdge::TopLeft,
            "the pane diagonally opposite has both of its near edges pulled"
        );
        assert_eq!(
            moved_edges(rect(700, 300, 200, 400), rect(760, 300, 140, 460)),
            ResizeEdge::BottomLeft
        );
    }

    /// **A hold handed to a new gesture stops counting down, and does not lose
    /// its place in the throttle.**
    ///
    /// `rearm` exists for a border nudged twice inside a quarter of a second:
    /// the first gesture's holds are waiting out `PATIENCE` when the second
    /// starts. Letting that deadline expire mid-gesture would adopt whatever
    /// size each client happened to be at — every pane the first nudge moved
    /// snapping off its tile in the middle of the second.
    #[test]
    fn a_rearmed_hold_stops_its_deadline_without_resetting_its_throttle() {
        let mut hold = Hold::new(
            ResizeEdge::Right,
            size(300, 200),
            want(320, 200),
            ms(0),
            Some(ms(0)),
        );
        // Well past the deadline: this hold would adopt on the next look.
        assert_eq!(
            hold.settle(size(300, 200), PATIENCE + ms(10)),
            Settle::Adopt(size(300, 200)),
            "which is what the second gesture must not be handed"
        );

        let mut hold = Hold::new(
            ResizeEdge::Right,
            size(300, 200),
            want(320, 200),
            ms(0),
            Some(ms(0)),
        );
        hold.rearm(None);
        assert_eq!(
            hold.settle(size(300, 200), PATIENCE + ms(10)),
            Settle::Waiting,
            "the new gesture owns this pane now, and a gesture that is still \
             going ends nothing"
        );
        assert_eq!(
            hold.dragged(want(340, 200), size(300, 200), TELL_EVERY),
            Some(want(340, 200)),
            "and the interval still runs from when the client was last spoken \
             to, not from the handover: resetting it would cost a tenth of a \
             second of silence at the one moment a drag has just started"
        );
    }
}
