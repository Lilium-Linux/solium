//! Hit-testing: which pane owns a point (`owns`, `shown_at`), which of the compositor's own chrome
//! it is on and whom a press there belongs to (`Chrome`, `PaneHit`, `Claim`), the window or surface
//! under it, whether the pointer is over a pane, and the cursor asserted over the chrome.

use super::*;
use crate::stack::{Band, Owner};

/// Whether `point` is somewhere a window living at `slot` could be drawn: on a
/// screen that draws it, by [`crate::render::drawn_on`].
///
/// **The hit tests' half of the rule [`nothing_on_stage`] is the focus half
/// of**, and asked by every walk that decides where a press goes --
/// `window_under`, `surface_under`, `pane_chrome` -- and by `decorated_under`,
/// which only looks. `Frame::covers` says whether the point is inside what a
/// pane draws; this says whether the screen under the point draws the pane at
/// all. With two monitors side by side the left one's hidden workspace is
/// carried over the right one, where its rectangle covers pixels the right
/// monitor never drew it on, and a press there went into a window nobody could
/// see (#134's third review).
/// `on_two_monitors_a_press_on_the_right_monitor_reaches_what_it_draws`.
///
/// **No screens is not "nowhere"**, for the reason [`Solium::on_stage`] gives:
/// a pointer with no monitor to be on is not a reason to decide that nothing
/// is under it. A point on no screen while there are some is on nothing,
/// because nothing is drawn there -- and `input::confine` keeps the pointer
/// from ever being there. `a_point_is_on_a_window_only_on_a_monitor_that_draws_it`.
///
/// **And never for `remains`: what is left of a window whose client has gone**
/// (#126), `Pane::ghost`. It is drawn, fading where the window stood, but it is
/// a picture of a window and not one: there is no client to hand a press, a
/// key or a hover to, and what it is drawn over is the window the layout has
/// grown into its place (`crate::pane::Left::over`). So every walk looks
/// through it, and it is asked here, once, rather than by each walk beside
/// this rule. `a_window_that_left_is_nobodys_to_find` asks the walks,
/// `a_point_is_on_a_window_only_on_a_monitor_that_draws_it` this.
pub(super) fn shown_at(
    slot: Rectangle<i32, Logical>,
    remains: bool,
    point: Point<f64, Logical>,
    screens: &[Rectangle<i32, Logical>],
) -> bool {
    !remains
        && (screens.is_empty()
            || screens.iter().any(|screen| {
                screen.to_f64().contains(point) && crate::render::drawn_on(slot, *screen)
            }))
}

/// Whether a window living at `slot`, drawn as `frame`, owns the pixel at
/// `point`: on a screen that draws it, [`shown_at`], and inside what it
/// paints there, `Frame::covers`.
///
/// **One question for "is this window under this point", whether Rust or a
/// script is asking.** [`Solium::window_under`] asks it of every pane in its walk,
/// and `sol.window_at` of every window in the snapshot, which carries the
/// slot, the frame and the screens for the purpose (`script::Drawn`). Until
/// #134's fourth review the script's half asked only whether the drawn
/// rectangle held the point, and so found what the Rust half had already
/// learned to see past: a window on a hidden workspace carried over the next
/// monitor, and a window fading out at opacity zero over the neighbour in its
/// place (#135).
/// `on_two_monitors_a_press_on_the_right_monitor_reaches_what_it_draws` asks
/// it from Rust, `on_two_monitors_sol_window_at_answers_what_the_right_monitor_draws`
/// and `sol_window_at_over_a_window_fading_out_answers_the_neighbour_in_its_place`
/// from Lua.
///
/// **One question, asked over two lists, and the lists differ.** The Rust walk
/// is every pane; the snapshot leaves out a pane that is not managed, one
/// whose application has not arrived when `loading.reserves_a_slot` is off,
/// and one scripts have been told has gone. Where the Rust walk stops at one
/// of those, `sol.window_at` sees through it to the window behind. Older than
/// #134 and left as it stands:
/// `sol_window_at_sees_through_the_panes_the_snapshot_leaves_out`. What is
/// left of a window whose client has gone is seen through by both: the
/// snapshot leaves it out, and `remains` has the Rust walk look past it
/// (`a_window_that_left_is_nobodys_to_find`).
pub(crate) fn owns(
    slot: Rectangle<i32, Logical>,
    remains: bool,
    frame: Frame,
    point: Point<f64, Logical>,
    screens: &[Rectangle<i32, Logical>],
) -> bool {
    shown_at(slot, remains, point, screens) && frame.covers(point)
}

/// What is topmost above the windows at a point: [`Solium::topmost_above`].
#[derive(Clone, Debug)]
pub(crate) enum Above {
    /// A client's layer surface: the surface under the point, and the origin
    /// to measure it from, in the compositor's space.
    Client(WlSurface, Point<f64, Logical>),
    /// A script's interactive surface: the monitor, which surface, and where
    /// it is.
    Script(Output, crate::scripted::SurfaceId, Rectangle<i32, Logical>),
    /// The window lifted over the bars, which has the point.
    Lifted,
}

/// What one pane makes of a point, for the pointer.
enum PaneSurface {
    /// A surface of its client's, with the origin to measure it from.
    Surface(WlSurface, Point<f64, Logical>),
    /// It is drawn there with no surface to give the pointer -- a window
    /// whose application has not arrived -- so nothing under it has the
    /// point either.
    Covered,
    /// Not this pane's: ask the next.
    Miss,
}

/// Which region of the compositor's own chrome a point is in.
///
/// Carries no window and no pane on purpose: this is the part that is a
/// *rule* rather than a lookup, and [`Under`] is what carries the rest. See
/// [`Solium::chrome_under`] for what a press and the pointer each do with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Chrome {
    /// The frame's band — the titlebar, its buttons, and any border the
    /// decoration reserved. A press here is the frame's: a button, or a drag
    /// that moves the window.
    Frame,
    /// A resize border, and which edges a drag from it would pull.
    Resize(ResizeEdge),
}

impl Chrome {
    /// The cursor the compositor asserts over this region.
    ///
    /// Over a frame this is the plain arrow rather than a move cursor, and
    /// deliberately: a titlebar is not only a drag handle — it carries buttons
    /// that are pressed, not dragged — and every desktop shows the ordinary
    /// pointer over one. What matters for #108 is that it is the
    /// *compositor's* arrow and overrides whatever the client last set,
    /// because a client drawing its own resize affordance in the shadow margin
    /// under our titlebar is exactly the case that went wrong.
    pub(crate) fn cursor(self) -> CursorIcon {
        match self {
            Self::Frame => CursorIcon::Default,
            Self::Resize(edges) => resize::cursor(edges),
        }
    }
}

/// The compositor's own chrome under a point: which region, which pane, and
/// the two things a press there needs.
#[derive(Clone, Debug)]
pub(crate) struct Under {
    /// What is under the pointer, and therefore both what a press does and
    /// what the pointer is drawn as.
    pub(crate) chrome: Chrome,
    pub(crate) pane: crate::pane::PaneId,
    /// The pane's window, when its application has arrived. `None` is a frame
    /// around a window that is still loading, whose buttons work anyway.
    /// [`Solium::pane_chrome`] never reports a resize border without one.
    pub(crate) window: Option<Window>,
    /// The point in the pane's own coordinates, which is what a decoration
    /// hit-tests its buttons against.
    pub(crate) local: Point<f64, Logical>,
    /// The pane's outer rectangle, which a resize drag measures from.
    pub(crate) outer: Rectangle<i32, Logical>,
}

/// Which region a point belongs to, given what each of the two tests said
/// about it — and the only place the overlap between them is resolved.
///
/// **The overlap is real, not theoretical.** A window with a titlebar has a
/// band below its top edge that is inside the frame *and* within
/// [`resize::RESIZE_BORDER`] of the top edge, so both tests answer yes for the
/// same pixel. The frame takes it, for the plain reason that the frame is what
/// a press there does: `pointer_button` has always checked the frame before
/// the resize border, and nothing about the pointer's shape is allowed to
/// disagree with that. The top edge is still draggable from the outside half
/// of its border, which is over the desktop rather than over the titlebar, and
/// now says so.
///
/// Written as a function taking two booleans-worth of answer rather than as a
/// `match` inside [`Solium::pane_chrome`] so that the rule can be pinned: a
/// `Solium` needs a `Display` and cannot be built in a unit test, and a rule
/// that can only be exercised by running the compositor is a rule that goes
/// untested until somebody notices it on hardware. Which is how #108 was
/// found.
pub(crate) fn chrome_of(on_frame: bool, edges: ResizeEdge) -> Option<Chrome> {
    if on_frame {
        return Some(Chrome::Frame);
    }
    match edges {
        ResizeEdge::None => None,
        edges => Some(Chrome::Resize(edges)),
    }
}

/// What one pane makes of a point — which is a wider question than whether
/// that pane's chrome is under it.
///
/// **[`Self::Client`] is the answer that was missing, and it is the whole of
/// issue #111.** A hit test that only ever says "my chrome, or nothing"
/// cannot express *occlusion*: a pane whose client covers the point answers
/// the same "nothing" as a pane the point falls nowhere near, so a walk down
/// the stack carries on past a window that is plainly on top and hands the
/// point to whatever is underneath. What that looked like in use: two
/// overlapping windows, a press on the upper one's client where the lower
/// one's titlebar happened to lie beneath, and the *lower* window raised and
/// took focus. A titlebar took clicks through the window covering it.
///
/// **[`Self::Halo`] is the answer the first fix for #111 was missing.** A
/// pane's chrome and the pixels it paints are not the same region: a resize
/// border reaches [`resize::RESIZE_BORDER`] pixels *outside* the window, over
/// whatever is drawn behind it. A claim made out there is not backed by
/// anything the pane draws, so it cannot be settled by stacking order — see
/// [`topmost_chrome`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PaneHit<T> {
    /// This pane's own chrome is under the point *and* this pane draws there.
    /// The walk has its answer.
    Chrome(T),
    /// This pane's chrome is under the point but the pane draws nothing there:
    /// the outside half of a resize border, hanging over whatever is behind.
    /// A claim, but the weakest one — see [`topmost_chrome`].
    Halo(T),
    /// The point is inside what this pane draws, but on its client rather than
    /// on its chrome. The client owns it and the walk stops here with nothing:
    /// everything below this pane is covered at that point.
    Client,
    /// The point is not this pane's at all. Keep descending.
    Miss,
}

impl<T> PaneHit<T> {
    /// Carry a chrome answer into whatever the caller wanted to say about it,
    /// leaving the two stopping answers alone.
    fn map<U>(self, chrome: impl FnOnce(T) -> U) -> PaneHit<U> {
        match self {
            Self::Chrome(found) => PaneHit::Chrome(chrome(found)),
            Self::Halo(found) => PaneHit::Halo(chrome(found)),
            Self::Client => PaneHit::Client,
            Self::Miss => PaneHit::Miss,
        }
    }
}

/// One pane's complete answer about a point, from the two things that decide
/// it.
///
/// `covers` is whether the point is inside the pane's *drawn* rectangle, and
/// it never suppresses a chrome claim — the frame band and the resize border
/// are both settled by [`chrome_of`] first, and `covers` only grades the
/// claim that came out. That is what keeps the inside half of a resize border
/// working: it lies within the drawn rect, so a rule that answered
/// [`PaneHit::Client`] wherever `covers` held would swallow it and leave every
/// window resizable only from outside.
///
/// **The four answers are the two questions crossed, and the cross is the
/// point.** A pane can claim chrome while `covers` is false — a border reaches
/// [`resize::RESIZE_BORDER`] pixels outside the window — and that claim is a
/// [`PaneHit::Halo`] rather than a [`PaneHit::Chrome`] precisely because the
/// pane paints nothing there to back it up. Collapsing the two, which is what
/// taking `(Some(chrome), _)` did, hands a window's empty margin authority over
/// pixels another window is visibly drawing.
pub(crate) fn pane_hit_of(chrome: Option<Chrome>, covers: bool) -> PaneHit<Chrome> {
    match (chrome, covers) {
        (Some(chrome), true) => PaneHit::Chrome(chrome),
        (Some(chrome), false) => PaneHit::Halo(chrome),
        (None, true) => PaneHit::Client,
        (None, false) => PaneHit::Miss,
    }
}

/// Which of its chrome a pane is allowed to offer at all, before any point is
/// considered.
///
/// Three gates, and all of them are about what a press there could actually
/// *do*:
///
/// - **`shows`.** A pane drawn at opacity zero has no titlebar to press and no
///   edge to drag, because it has nothing on screen at all. Unlike the two
///   below, this one is *also* a statement about covering — see
///   [`Solium::pane_chrome`], which asks it in both places — because an
///   invisible pane is the one kind that offers nothing and occludes nothing
///   either. Issue #127's review finding 1.
/// - **`managed`.** An unmanaged pane is one its client placed and owns: an
///   X11 menu, a tooltip, a dropdown. Nothing here ever sizes it or moves it —
///   `show_if_new` and `snapshot` both ask [`crate::pane::Pane::managed`] and
///   nothing else — and `size_window` refuses an override-redirect surface
///   outright. So a resize border eight pixels outside a Steam menu is a
///   cursor promising a drag that cannot happen, and a press there starts a
///   `ResizeGrab` that does nothing instead of dismissing the menu. Such a
///   pane still *occludes*, which is [`pane_hit_of`]'s business and not this
///   one's: covering is a fact about pixels, offering chrome is a claim about
///   what a press means.
/// - **`window`.** A resize needs a window to resize. A frame does not: a
///   frame around a window whose application has not arrived still has working
///   buttons, which is the point of giving it one.
pub(crate) fn chrome_offered(
    shows: bool,
    managed: bool,
    window: bool,
    framed: bool,
    edges: ResizeEdge,
) -> Option<Chrome> {
    if !shows || !managed {
        return None;
    }
    chrome_of(framed, edges).filter(|chrome| window || !matches!(chrome, Chrome::Resize(_)))
}

/// The chrome under a point, given what every pane makes of it, topmost first.
///
/// **One pass, and the first pane with anything to say ends it.** This is the
/// fix for issue #111 stated as a rule: a pane that covers the point answers
/// [`PaneHit::Client`], which stops the walk with `None`, and the panes below
/// it are never asked. Before this the walk could only be stopped by a *match*,
/// so a covering window was indistinguishable from an absent one and the point
/// fell through to a lower window's titlebar.
///
/// **A halo is the weakest claim there is, and that is the correction to the
/// first fix.** [`PaneHit::Halo`] — chrome outside the pane's own drawn rect —
/// is remembered and the walk carries on, so it is used only if nothing below
/// paints that pixel. It beats bare desktop, which is what makes an edge
/// grabbable from outside at all, and it loses to any lower pane that actually
/// draws there.
///
/// The rule that stood here briefly was that a higher pane's border simply wins,
/// "because the higher window is on top". That is unanswerable at a pixel the
/// higher pane does not occupy: an upper window's edge floating four pixels
/// above a lower window's close button drew `NsResize` over a visibly drawn,
/// clickable control, and a press there started a resize grab instead of
/// closing the window. Stacking order decides who owns a pixel among the panes
/// that *draw* it; a pane drawing nothing there is not in that contest. Which
/// is also why the two-pass shape this replaces was not wrong for the reason
/// #111's fix first gave: running every frame before any border did give a
/// lower titlebar the point, and at a point outside the upper window that
/// happens to be the right answer. It was wrong because it reached it without
/// consulting stacking order at all, so it got the covered-titlebar case
/// (#111) and the *inside* half of a higher border wrong by the same omission.
///
/// Generic over what a pane answers with, and taking the answers rather than
/// the panes, for the reason [`chrome_of`] and [`claim_of`] are written the
/// same way: a `Solium` needs a `Display` and cannot be stood up in a unit
/// test, and a stacking rule that can only be exercised by running the
/// compositor is a rule that goes untested until somebody notices it on
/// hardware. Which is how #111 was found.
///
/// The iterator is walked lazily and abandoned at the first pane that claims
/// the point with something it draws, so a pane under a covering window is
/// never hit-tested at all. A halo alone does not abandon it: what is under
/// the halo is exactly the question.
pub(crate) fn topmost_chrome<T>(stack: impl IntoIterator<Item = PaneHit<T>>) -> Option<T> {
    // The topmost halo, kept because the walk cannot yet tell whether anything
    // below draws where it hangs. Later halos are lower and never displace it:
    // among panes that all merely hover over a point, the top one still wins.
    let mut halo = None;
    for hit in stack {
        match hit {
            PaneHit::Chrome(chrome) => return Some(chrome),
            PaneHit::Client => return None,
            PaneHit::Halo(chrome) => halo = halo.or(Some(chrome)),
            PaneHit::Miss => {}
        }
    }
    halo
}

/// Who a press at a point belongs to, before any client sees it.
///
/// [`Chrome`] is one link of this and not the whole of it, which is what the
/// first pass at #108 got wrong: the cursor was read off `chrome_under` alone
/// while [`crate::input`]'s `pointer_button` consults two other things first,
/// so in a mode — or over a scripted bar — the pointer went on describing a
/// resize that the press was never going to perform. Same bug, one altitude up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Claim {
    /// A scripted surface above the windows takes it: a bar, a panel, an
    /// overlay. What it does with the press is the script's business and the
    /// compositor has nothing to say about it.
    Surface,
    /// A mode owns input — `sol.grab(true)`, which is overview. Every press is
    /// the mode's, whatever is drawn under the pointer.
    Mode,
    /// The compositor's own chrome: a frame's band, or a resize border.
    Chrome(Chrome),
    /// None of the compositor's: the press reaches a window or a client.
    Nothing,
}

impl Claim {
    /// The cursor the compositor asserts, or `None` to say nothing and leave
    /// the pointer to whoever owns the point.
    ///
    /// **Only `Chrome` names a shape.** The other three are the compositor
    /// declining to describe the press, which is the whole content of this
    /// finding: a thumbnail's corner in overview is within eight pixels of a
    /// window edge and `chrome_under` will happily call it `BottomRight`, but
    /// the press there focuses the window and leaves the mode. A pointer that
    /// promised a resize would be #108's second symptom again, on a key
    /// combination used every day.
    pub(crate) fn cursor(self) -> Option<CursorIcon> {
        match self {
            Self::Chrome(chrome) => Some(chrome.cursor()),
            Self::Surface | Self::Mode | Self::Nothing => None,
        }
    }
}

/// `pointer_button`'s precedence chain, as a rule rather than as a sequence of
/// early returns.
///
/// The three links are asked in the order the press asks them, and the order is
/// the point: a scripted overlay above the windows is offered the press before
/// a mode is consulted, and a mode before any chrome. `pointer_button` is where
/// each answer is *acted* on — it has a surface to deliver to, a click to
/// trigger and an [`Under`] to start a grab from, none of which fit in a value
/// — but which one wins is decided here, and the pointer's shape is read off
/// the result rather than off the last link alone.
///
/// Pure, and taking the links already answered, for the same reason
/// [`chrome_of`] is: a `Solium` needs a `Display` and cannot be stood up in a
/// unit test, so a rule that lives inside one is a rule that is checked by
/// running the compositor and noticing. Which is how both halves of #108 were
/// found.
pub(crate) fn claim_of(surface: bool, mode: bool, chrome: Option<Chrome>) -> Claim {
    if surface {
        return Claim::Surface;
    }
    if mode {
        return Claim::Mode;
    }
    match chrome {
        Some(chrome) => Claim::Chrome(chrome),
        None => Claim::Nothing,
    }
}

/// Whether a point in a pane's own coordinates lands on its frame rather than
/// on its client.
///
/// The band is everything inside the pane's outer rectangle that the insets
/// reserve — a titlebar across the top, a bar down a side, a border all round,
/// whatever the decoration asked for — and the complement is the client's,
/// wherever the frame chose to draw itself inside it. A decoration that
/// reserves nothing owns no band at all, and its clicks belong to the window
/// under it.
///
/// The outer bound is part of the predicate and not a caller's business: the
/// insets say how far in the client starts, so without it every point above a
/// window would be "not the client" and therefore the titlebar.
pub(crate) fn on_frame(
    size: Size<i32, Logical>,
    insets: Insets,
    local: Point<f64, Logical>,
) -> bool {
    let pane = Rectangle::new(Point::from((0.0, 0.0)), size.to_f64());
    let client = Rectangle::new(
        Point::from((f64::from(insets.left), f64::from(insets.top))),
        Size::from((
            f64::from(size.w - insets.horizontal()),
            f64::from(size.h - insets.vertical()),
        )),
    );
    pane.contains(local) && !client.contains(local)
}

impl Solium {
    /// Whether the pointer is over a pane, frame included.
    ///
    /// **A question about a rectangle, and deliberately not a hit test.** It is
    /// asked once per frame by `render::chrome`, for
    /// `decoration::Look::pointer_inside` — the flag a titlebar reads to light
    /// a close button up as the cursor crosses it. Nothing routes an event by
    /// it: presses go through `chrome_under` and `window_under`, motion and
    /// buttons through `surface_under`, and all three walk `drawn_at` and
    /// `Frame::covers`.
    ///
    /// That is why it takes `pane_outer_of` rather than the drawn frame, and
    /// why it is right that it does. The alternative was raised by #127's
    /// review and is worth answering once so it is not raised again: were this
    /// gated on `covers` like the walks are, it would still decide nothing
    /// about where input goes, and a pane it answers `true` for while invisible
    /// draws no decoration to light up — `render::chrome` is reached through
    /// the same transform, and a frame at opacity zero paints nothing. Asking a
    /// cheaper question here and the exact one there is the split, not an
    /// oversight in this line.
    pub(crate) fn pointer_over(&self, id: crate::pane::PaneId) -> bool {
        // Never over a window that has gone: a close button lighting up as
        // the cursor crosses a pane fading out offers a press that nothing
        // will take (`a_window_that_left_is_nobodys_to_find`).
        if self.panes.get(id).is_some_and(Pane::ghost) {
            return false;
        }
        let Some(outer) = self.pane_outer_of(id) else {
            return false;
        };
        let Some(pointer) = self.seat.get_pointer() else {
            return false;
        };
        outer.to_f64().contains(pointer.current_location())
    }

    /// The compositor's own chrome under `location`, if any.
    ///
    /// **The single hit test behind both what a press does and what the pointer
    /// looks like, and that is the whole of issue #108.** The two regions
    /// overlap — the outer eight pixels of a titlebar are inside the top resize
    /// border — and before this there was nothing that resolved the overlap
    /// once. A press resolved it by asking `frame_under` first and
    /// `resize_target` second, so the band below a window's top edge moved the
    /// window. The pointer resolved it not at all: the compositor asked for no
    /// cursor but the default, so whatever a client had last set stayed on
    /// screen, and a CSD toolkit that names a resize shape for its own shadow
    /// margin left a resize arrow sitting over a band that moves. The pointer
    /// was not merely missing a shape; it was confidently describing a
    /// different action from the one a press would take.
    ///
    /// Callers get the answer and never the ingredients, so a second, parallel
    /// hit test for the cursor cannot be written by accident — which is the
    /// failure mode this shape is chosen against, because two hit tests drift
    /// and the bug comes back wearing a different face.
    ///
    /// **One walk, topmost first, and the first pane that claims the point with
    /// something it *draws* ends it — including when what it claims is "my
    /// client owns this".** That last case is issue #111 and is what
    /// [`topmost_chrome`] exists to state. This walk used to ask every pane for
    /// a [`Chrome::Frame`] and then every pane again for a [`Chrome::Resize`],
    /// and in neither pass could a pane stop the descent by *covering* the
    /// point: [`Self::pane_chrome`] returned the same `None` for "the point is
    /// on my client" as for "the point is nowhere near me". So a press on the
    /// top window, at a spot where a lower window's titlebar lay underneath,
    /// raised and focused the lower window — a titlebar taking clicks through
    /// whatever covered it.
    ///
    /// "Something it draws" is the qualification the first fix was missing: a
    /// resize border hanging in the empty margin outside its own window claims
    /// the point only against bare desktop, and yields to whatever a lower pane
    /// paints there. [`topmost_chrome`] has the argument.
    pub(crate) fn chrome_under(&self, location: Point<f64, Logical>) -> Option<Under> {
        // A titlebar is the compositor's own surface, so it would otherwise
        // still take clicks with the session locked -- close and maximise
        // included. The resize border used to sit outside this guard, since
        // `resize_target` walked the panes itself and asked nothing: a press
        // near where a window's edge used to be started a resize grab on a
        // locked screen, and the window was still that size when the session
        // unlocked. There is one guard now because there is one hit test.
        if self.lock.is_some() {
            return None;
        }
        let now = self.clock.now();
        let screens = self.screens();

        // Topmost first, through `panes_front_first`: `panes` is in stacking
        // order, bottom-first, and the rule is topmost-first -- which is now
        // load-bearing in a way it was not before, since the first pane to
        // cover the point ends the walk, and a halo is only kept until a lower
        // pane is found drawing under it.
        topmost_chrome(
            self.panes_front_first(location)
                .map(|pane| self.pane_chrome(pane, location, now, &screens)),
        )
    }

    /// What one pane's chrome makes of a point.
    ///
    /// **Two regions in two coordinate spaces, and both of those spaces are
    /// deliberate.** The frame's band is hit-tested in the pane's *own*
    /// coordinates, because the frame is rasterised at its unscaled size: a
    /// titlebar drawn at two-thirds size in overview must still be measured
    /// against the QML that was drawn at full size, or its buttons move out
    /// from under the cursor. The resize border is hit-tested where the window
    /// is *drawn*, because a window in a mode should be resized by its
    /// thumbnail's edge or not at all, never by an edge that is somewhere else
    /// on screen. Both were already true separately; what was missing is that
    /// they meet, and [`chrome_of`] is where they are reconciled.
    ///
    /// `pane_outer` rather than `outer_geometry`, which is what the resize half
    /// used to reach for. They agree for a mapped, sized window and differ for
    /// one that has mapped and not yet answered a size, where the space reports
    /// a rectangle of nothing and the pane's slot is still the truth. Every
    /// other hit test in this file already went through `pane_outer`; this is
    /// the one that did not.
    ///
    /// **Four answers rather than two, which is issue #111 and its
    /// correction.** A pane covering the point with its client says so
    /// ([`PaneHit::Client`]) instead of declining, because declining is what a
    /// pane the point misses entirely does and [`Solium::chrome_under`]'s walk
    /// has to tell those apart. A pane with no geometry yet is a
    /// [`PaneHit::Miss`]: it draws nothing, so there is nothing for it to cover
    /// the point with. And chrome the pane claims *outside* what it draws is a
    /// [`PaneHit::Halo`], which is a claim the walk may yet overrule — the one
    /// question `covers` answers that the chrome tests cannot.
    ///
    /// `drawn.rect.contains(location)` is what `covers` is, in both places it
    /// is asked: the frame band already required it, and [`pane_hit_of`] grades
    /// the resize border by the same rectangle. Not `outer`, and not the
    /// client rect — where a pane is *drawn* is where it paints, which in a
    /// mode is its thumbnail and nowhere near where the window lives.
    ///
    /// What a pane may offer at all is [`chrome_offered`]'s, including the
    /// `managed` gate: an unmanaged pane occludes like any other and offers no
    /// chrome whatsoever.
    fn pane_chrome(
        &self,
        pane: &Pane,
        location: Point<f64, Logical>,
        now: std::time::Duration,
        screens: &[Rectangle<i32, Logical>],
    ) -> PaneHit<Under> {
        let outer = self.pane_outer(pane);
        // On a screen that does not draw the pane, it has nothing there to
        // press and nothing to occlude with -- a `Miss`, as an invisible pane
        // is below, and not a `Halo`, which would still win over bare desktop.
        // See [`shown_at`];
        // `on_two_monitors_a_press_on_the_right_monitor_reaches_what_it_draws`
        // asks both. Nor has what is left of a window whose client has gone:
        // `a_window_that_left_is_nobodys_to_find`.
        if !shown_at(outer, pane.ghost(), location, screens) {
            return PaneHit::Miss;
        }
        let drawn = self.drawn_at(pane, outer, now);
        let in_outer = present::to_window_space(drawn, outer, location) - outer.loc.to_f64();

        // Only a *built* frame has a band to press. A pane reserving room for
        // one that has not arrived reports insets -- `insets_of` answers for
        // `Frame::Pending` on purpose, so the window does not change shape the
        // moment its frame appears -- but there is no titlebar there yet for a
        // click to land on, and there never was: this is the `decoration()?`
        // that gated `frame_under`.
        let framed = pane.decoration().is_some()
            && drawn.covers(location)
            && on_frame(outer.size, self.insets_of(pane.id()), in_outer);

        #[expect(
            clippy::cast_possible_truncation,
            reason = "a drawn rect is screen-sized"
        )]
        let drawn_rect = Rectangle::new(
            (
                drawn.rect.loc.x.round() as i32,
                drawn.rect.loc.y.round() as i32,
            )
                .into(),
            (
                drawn.rect.size.w.round() as i32,
                drawn.rect.size.h.round() as i32,
            )
                .into(),
        );
        // What this pane is allowed to offer -- the `shows`, `managed` and
        // `window` gates -- is `chrome_offered`'s, and all of them decline by
        // answering `None` here rather than by returning out of the function.
        // That is the #111-shaped difference: a loading window, and a
        // client-placed menu, both still cover what is behind them, and a press
        // on either is its own and nobody else's. Occluding is a fact about
        // pixels; offering chrome is a claim about what a press would do.
        //
        // **An invisible pane is the one case that fails both questions**, and
        // it has to fail them together. Gating only `covers` below would turn
        // its `Some(chrome)` into a `PaneHit::Halo` -- a claim that survives
        // the walk and wins wherever nothing lower paints -- so a closed
        // window's resize border would go on being draggable, invisibly, over
        // bare desktop for the whole grace period. `shows` is therefore asked
        // here as well, and the pair answers `pane_hit_of(None, false)`:
        // `PaneHit::Miss`, the walk descends, and the pane is gone from the hit
        // test exactly as it is gone from the screen.
        let window = pane.client().cloned();
        let chrome = chrome_offered(
            drawn.shows(),
            pane.managed(),
            window.is_some(),
            framed,
            resize::border_edges(drawn_rect, location),
        );

        pane_hit_of(chrome, drawn.covers(location)).map(|chrome| Under {
            chrome,
            pane: pane.id(),
            window,
            local: in_outer,
            outer,
        })
    }

    /// The window drawn at a point, topmost first, with its real geometry.
    ///
    /// Hit-testing follows the transform: in overview a window is clickable
    /// where the thumbnail is, not where the window lives.
    ///
    /// **The walk stops at the first pane that covers the point, whether or not
    /// that pane has a window to hand back.** This is issue #111 in the third
    /// of the three walks: `client()?` used to sit inside a `find_map`, where
    /// `None` means "keep looking" rather than "stop", so a window still
    /// loading — drawn, on screen, under the cursor and with no client yet —
    /// was descended straight past. `chrome_under` now correctly answers
    /// nothing over its body, `pointer_button` falls through to click-to-focus,
    /// and this raised and focused the window *behind* it. Same reasoning and
    /// the same line as [`Self::surface_under`], which has always got this
    /// right by holding its `?` in a `for` loop instead.
    ///
    /// `pane_outer` rather than `outer_geometry` for the same reason, and it is
    /// what makes stopping possible at all: `outer_geometry` needs the window,
    /// so the old shape could not ask whether a client-less pane covered the
    /// point even in principle.
    pub(crate) fn window_under(
        &self,
        location: Point<f64, Logical>,
    ) -> Option<(Window, Rectangle<i32, Logical>)> {
        self.window_under_at(location, self.clock.now())
    }

    /// The same walk, at an instant the caller names.
    ///
    /// [`Self::window_under`] is this at the present, which is what a press is
    /// answered from and must stay. The one other caller is
    /// [`Self::settle_focus`]'s pointer arm, which is a focus decision and so
    /// asks at [`Self::settling`] — see [`super::workspaces::SETTLED`] for the rule and
    /// `a_pointer_over_the_desk_being_left_does_not_hand_it_the_keyboard` for
    /// the two asked of one pixel on one frame.
    pub(super) fn window_under_at(
        &self,
        location: Point<f64, Logical>,
        now: Duration,
    ) -> Option<(Window, Rectangle<i32, Logical>)> {
        // Locked, so there is no window under the pointer however many are
        // still mapped. Everything built on this -- click to focus, focus
        // follows mouse, drag, resize -- stops at once, in one place.
        if self.lock.is_some() {
            return None;
        }
        let screens = self.screens();
        for pane in self.panes_front_first(location) {
            let outer = self.pane_outer(pane);
            // `covers`, not `rect.contains`: a pane drawn at opacity zero is
            // not on screen and owns no pixel, however solid the rectangle it
            // would be drawn at. See [`present::Frame::covers`], and #127's
            // review finding 1 -- this walk ends in `focus_window` through
            // click-to-focus, so an invisible pane winning it took the
            // keyboard as well as the click.
            //
            // And only on a screen that draws the pane, which is [`shown_at`]:
            // a hidden workspace carried over the next monitor covers pixels
            // that monitor never drew it on (#134's third review).
            //
            // Both through [`owns`], which is also what `sol.window_at` asks,
            // so a script and this walk put the same question to each window --
            // `on_two_monitors_sol_window_at_answers_what_the_right_monitor_draws`.
            // And never of what is left of a window whose client has gone.
            if !owns(
                outer,
                pane.ghost(),
                self.drawn_at(pane, outer, now),
                location,
                &screens,
            ) {
                continue;
            }
            // Covered. A pane whose application has not arrived has no window
            // to focus -- but it is on screen and it is under the cursor, so
            // nothing behind it may be focused or raised by this press either.
            let window = pane.client()?;
            return Some((window.clone(), self.real_geometry(window)?));
        }
        None
    }

    /// The surface at a point, and the origin to measure it from.
    ///
    /// The origin is chosen so that `location - origin` is the point in
    /// surface-local coordinates. That is what keeps a scaled window honest:
    /// the client is told where in *itself* the pointer is, and never learns
    /// that it is being drawn at half size.
    pub(crate) fn surface_under(
        &self,
        location: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.surface_at(location, true)
    }

    /// [`Self::surface_under`], for the pointer, or [`Self::touch_under`].
    fn surface_at(
        &self,
        location: Point<f64, Logical>,
        pointer: bool,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        // Locked: the only surface anyone may point at is the lock screen's,
        // and on a monitor it has not covered, none at all. Returning early
        // rather than filtering afterwards is deliberate -- a later `return`
        // that forgets the check is a click landing in the session.
        if let Some(lock) = self.lock.as_ref() {
            let output = monitor::at(&self.space, location)?;
            let geometry = self.space.output_geometry(&output)?;
            let surface = lock.surface_for(&output)?;
            return under_from_surface_tree(
                surface.wl_surface(),
                location - geometry.loc.to_f64(),
                (0, 0),
                WindowSurfaceType::ALL,
            )
            .map(|(surface, offset)| (surface, (geometry.loc + offset).to_f64()));
        }

        // A press a hosted scene took holds the pointer for that scene until
        // every button is up (Ruling 7), and a grab one holds holds it until
        // it is let go of (Ruling 12), so no client has it meanwhile.
        // `tests::real_client::reflow_on_close::hosted::a_release_after_dragging_off_a_shell_button_reaches_the_scene`,
        // `tests::real_client::reflow_on_close::hosted::while_a_grab_is_held_the_pointer_is_the_scenes`.
        if pointer && (self.scene_press.is_some() || self.hosted_grab.is_some()) {
            return None;
        }

        // Over the windows first, in `crate::stack`'s order: a client's layer
        // surface there is the one the pointer reaches -- a panel reserved its
        // strip precisely so nothing of a window's would be under the cursor
        // there -- unless the window lifted over the bars covers it; and where
        // a script's scene takes a press, no client has the pointer (Ruling
        // 8). A scene that takes only hover leaves it to what is under it.
        // `tests::real_client::reflow_on_close::hosted::a_press_on_a_shell_button_does_not_reach_the_window`,
        // `tests::real_client::reflow_on_close::hosted::a_hover_strip_hears_the_motion_and_leaves_the_window_its_press`.
        match self.topmost_above(location, Some(Asking::Press)) {
            Some(Above::Client(surface, origin)) => return Some((surface, origin)),
            Some(Above::Script(..)) => return None,
            Some(Above::Lifted) | None => {}
        }

        let now = self.clock.now();
        let screens = self.screens();

        // The lifted window first, which is where the bands above leave off.
        for pane in self.panes_front_first(location) {
            match self.pane_surface_at(pane, location, now, &screens) {
                PaneSurface::Surface(surface, origin) => return Some((surface, origin)),
                PaneSurface::Covered => return None,
                PaneSurface::Miss => {}
            }
        }

        None
    }

    /// The surface a touch at `location` lands on: the pointer's answer, but
    /// for a press a hosted scene holds, which is the pointer's and keeps no
    /// touch from a client. Where a scene takes a press, a touch reaches no
    /// client, as the pointer does not (Ruling 8), and a scene takes no
    /// touch, so it is nobody's.
    /// `tests::real_client::reflow_on_close::hosted::a_touch_on_a_shell_button_neither_reaches_nor_focuses_the_window_under_it`.
    pub(crate) fn touch_under(
        &self,
        location: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.surface_at(location, false)
    }

    /// What one pane makes of a point, for the pointer: [`Self::surface_under`]'s
    /// question, asked of each pane in its walk.
    fn pane_surface_at(
        &self,
        pane: &Pane,
        location: Point<f64, Logical>,
        now: Duration,
        screens: &[Rectangle<i32, Logical>],
    ) -> PaneSurface {
        let outer = self.pane_outer(pane);
        // Nothing of a pane -- its surface or its popups, which the
        // renderer draws inside the same cull -- is at a point on a screen
        // that does not draw it. [`shown_at`] says why, and
        // `on_two_monitors_a_press_on_the_right_monitor_reaches_what_it_draws`
        // is this walk. Nor anything of a window whose client has gone:
        // `a_window_that_left_is_nobodys_to_find`.
        if !shown_at(outer, pane.ghost(), location, screens) {
            return PaneSurface::Miss;
        }
        let frame = self.drawn_at(pane, outer, now);
        // Invisible is not covered. Same rule and same reason as
        // [`Self::window_under`]: this walk is what delivers motion,
        // buttons and — through the focus a press sets — keystrokes, so a
        // pane held at opacity zero across a close winning it is where the
        // typing went. [`present::Frame::covers`] argues the predicate.
        //
        // **Except that a window's popups are not inside its rectangle.**
        // `render::elements` draws them uncut, reaching past the parent's
        // tile, so a point outside the parent's frame can still be on one
        // of its menus -- and since #133 that includes the whole strip
        // between a tiled window's tile and the edge its client committed,
        // where a menu opened from an oversized Firefox lands. The point
        // went to the neighbour instead, and a press on it -- when the
        // neighbour is another application -- had smithay's popup grab
        // dismiss the menu rather than choose the item under it
        // (`PopupPointerGrab::button`). So a visible pane that does not
        // cover the point is still asked about its popups, and only about
        // those. `a_menu_past_its_parents_tile_takes_the_press` pins it.
        let covered = frame.covers(location);
        let (window, kind) = match pane.client() {
            Some(window) if covered => (window, WindowSurfaceType::ALL),
            // A popup and its own subsurfaces, and not the toplevel's tree.
            Some(window) if frame.shows() => (
                window,
                WindowSurfaceType::POPUP | WindowSurfaceType::SUBSURFACE,
            ),
            // A window whose application has not arrived has no surface to
            // give the pointer -- but it is on screen and it is under the
            // cursor, so nothing behind it may have the click either.
            // Falling through would type into whatever the window is
            // covering.
            None if covered => return PaneSurface::Covered,
            _ => return PaneSurface::Miss,
        };

        // Into the client's own space through the very fit its picture is
        // drawn with (#133): off the buffer's drawn corner, and divided by
        // what the buffer was scaled by. A point in the titlebar lands
        // above the client and finds no surface, which is what should
        // happen: the frame is the compositor's, not the client's.
        //
        // It used to go through `to_window_space`, which reads the drawn
        // rectangle as a scale of the pane's own, and take its inset from
        // `frame_insets`. That is the picture's arithmetic for a decorated
        // window at rest or in a thumbnail and for nothing else:
        //
        // * a tiled window on a frame of a glide is drawn 1:1 and cut, and
        //   a press there landed as far from the pixel under it as the
        //   glide was from its destination --
        //   `a_press_on_a_gliding_window_lands_on_the_pixel_under_it`;
        // * a window under a resize hold has its last buffer stretched
        //   into the dragged rectangle, and a press was mapped 1:1 against
        //   the rectangle instead --
        //   `a_press_on_a_held_window_lands_on_the_pixel_its_picture_has`;
        // * a pane still reserving a titlebar is drawn below it, and
        //   `frame_insets` answers nothing for a pane that is not yet
        //   decorated --
        //   `a_press_on_a_window_reserving_a_titlebar_lands_on_the_pixel_under_it`.
        //
        // It is still the picture's arithmetic for the *frame*, which is
        // stretched from the pane's outer size to the drawn rect, so
        // `pane_chrome` keeps it.
        let placed =
            crate::render::place_client(self, pane, &frame, outer.size, window.geometry().size);
        let undo = |drawn: f64, factor: f64| {
            if factor.abs() > f64::EPSILON {
                drawn / factor
            } else {
                drawn
            }
        };
        let in_window: Point<f64, Logical> = (
            undo(location.x - placed.origin.x, placed.fit.factor.x),
            undo(location.y - placed.origin.y, placed.fit.factor.y),
        )
            .into();

        // Into the *buffer's* coordinates, which is what `surface_under`
        // wants and is not the same point.
        //
        // A client that draws its own decorations commits a surface bigger
        // than its window: the invisible resize shadow is part of the
        // buffer, and `xdg_surface.set_window_geometry` is how it says
        // which sub-rectangle is the real window. `geometry().loc` is that
        // offset -- around (26, 26) for a GTK application.
        //
        // `in_window` above is relative to the window the user can see.
        // Smithay's `Window::surface_under` ends in
        // `under_from_surface_tree(&surface, point, (0, 0), ..)` -- offset
        // zero -- so the point it expects is relative to the surface tree
        // root, the buffer origin. Its own `SpaceElement` wrapper puts that
        // origin at `location - geometry().loc`
        // (`desktop/space/mod.rs:510`), so the two differ by exactly
        // `geometry().loc`, and dropping it is issue #101: every click in
        // Firefox and in Qt applications landed a shadow's width up and
        // left of where it was aimed, which for a row of buttons means the
        // one next door.
        //
        // Zero for a client with no decorations of its own, so a terminal
        // never noticed.
        let in_buffer = in_window + window.geometry().loc.to_f64();

        match window.surface_under(in_buffer, kind) {
            Some((surface, surface_offset)) => {
                let in_surface = in_buffer - surface_offset.to_f64();
                PaneSurface::Surface(surface, location - in_surface)
            }
            None => PaneSurface::Miss,
        }
    }

    /// What is topmost at a point among the bands above the windows:
    /// [`crate::stack::above`], on the monitor under the point, asked band by
    /// band until one has something there.
    ///
    /// **The hit tests' half of the one order**, which the renderer reads in
    /// `render::stacked`. `scripts` is what a script's surfaces are asked
    /// for, and `None` leaves them out: a surface is there only where its
    /// scene's items claim the point for that
    /// (`tests::real_client::reflow_on_close::hosted::a_hover_strip_hears_the_motion_and_leaves_the_window_its_press`).
    /// The stacking tests in `state::tests` ask each.
    pub(crate) fn topmost_above(
        &self,
        location: Point<f64, Logical>,
        scripts: Option<Asking>,
    ) -> Option<Above> {
        let output = monitor::at(&self.space, location)?;
        let geometry = self.space.output_geometry(&output)?;
        let lifted = self.lifted_on(geometry);
        let now = self.clock.now();
        let screens = self.screens();
        crate::stack::above(lifted.is_some()).find_map(|band| match band {
            Band::Layer(layer, Owner::Client) => {
                // A layer map's geometry is in its own output's coordinates.
                layer::surface_under(&output, layer, location - geometry.loc.to_f64())
                    .map(|(surface, origin)| Above::Client(surface, origin + geometry.loc.to_f64()))
            }
            Band::Layer(layer, Owner::Script) => scripts
                .and_then(|asking| self.script_at(&output, geometry, layer, location, asking))
                .map(|(id, area)| Above::Script(output.clone(), id, area)),
            Band::Windows => None,
            Band::Fullscreen => lifted
                .and_then(|id| self.panes.get(id))
                .filter(|pane| {
                    !matches!(
                        self.pane_surface_at(pane, location, now, &screens),
                        PaneSurface::Miss
                    )
                })
                .map(|_| Above::Lifted),
        })
    }

    /// Whether something over the windows is what the pointer is on at
    /// `location`: a client's layer surface, or a script's scene whose items
    /// take a press there (Ruling 8). The window under either is not.
    /// `tests::real_client::reflow_on_close::hosted::focus_follows_the_mouse_through_a_shell_only_where_it_takes_no_press`,
    /// `focus_follows_mouse_does_not_reach_through_a_bar`.
    pub(crate) fn pointed_above(&self, location: Point<f64, Logical>) -> bool {
        matches!(
            self.topmost_above(location, Some(Asking::Press)),
            Some(Above::Client(..) | Above::Script(..))
        )
    }

    /// Whether the window frames are kept from the pointer at `location`:
    /// something over the windows is what it is on there
    /// ([`Self::pointed_above`]), or a scene holds a press (Ruling 7) or a
    /// grab (Ruling 12). A scene that takes only hover leaves the frame under
    /// it the pointer, as it leaves the window its press (Ruling 8).
    /// `tests::real_client::reflow_on_close::hosted::the_frames_are_kept_from_the_pointer_only_where_a_shell_takes_a_press`,
    /// `tests::real_client::reflow_on_close::hosted::while_a_grab_is_held_no_window_takes_focus_frame_or_cursor_from_the_pointer`.
    pub(crate) fn frames_kept_from(&self, location: Point<f64, Logical>) -> bool {
        self.scene_press.is_some() || self.hosted_grab.is_some() || self.pointed_above(location)
    }

    /// Whether a client's layer surface is what is on top at `location`, over
    /// the windows and their chrome: a press there is the client's.
    pub(crate) fn client_above(&self, location: Point<f64, Logical>) -> bool {
        matches!(
            self.topmost_above(location, Some(Asking::Press)),
            Some(Above::Client(..))
        )
    }

    /// Whether a window has `location`, over everything below the windows: a
    /// pane drawn over it, which is what [`Self::window_under`] asks of each
    /// ([`owns`]), or a menu of one reaching past its frame, which is what
    /// [`Self::surface_under`] asks (`pane_surface_at`). A script's surface
    /// below the windows is offered no press there:
    /// `a_window_over_a_scripted_dock_keeps_the_press`.
    pub(crate) fn windows_have(&self, location: Point<f64, Logical>) -> bool {
        let now = self.clock.now();
        let screens = self.screens();
        self.panes_front_first(location).any(|pane| {
            let outer = self.pane_outer(pane);
            owns(
                outer,
                pane.ghost(),
                self.drawn_at(pane, outer, now),
                location,
                &screens,
            ) || !matches!(
                self.pane_surface_at(pane, location, now, &screens),
                PaneSurface::Miss
            )
        })
    }

    /// The panes in the order the hit tests walk them at `location`: the one
    /// lifted over the bars on the monitor under it, and then the rest,
    /// topmost first.
    ///
    /// The renderer draws the lifted one over every other window
    /// (`crate::stack`'s `a_lifted_window_is_under_overlay_and_over_top`), so
    /// the walks that ask a window whether it owns a point ask it first.
    fn panes_front_first(&self, location: Point<f64, Logical>) -> impl Iterator<Item = &Pane> {
        let lifted = monitor::at(&self.space, location)
            .and_then(|output| self.space.output_geometry(&output))
            .and_then(|screen| self.lifted_on(screen));
        lifted.and_then(|id| self.panes.get(id)).into_iter().chain(
            self.panes
                .iter()
                .rev()
                .filter(move |pane| Some(pane.id()) != lifted),
        )
    }

    /// Who a press at `location` would belong to.
    ///
    /// The three links of [`claim_of`], fetched in the order `pointer_button`
    /// fetches them. Read-only: the surface link is what the scene's items
    /// claim for a press, asked rather than delivered, for the reasons
    /// [`Self::surface_claiming`] gives
    /// (`tests::real_client::reflow_on_close::hosted::a_press_on_a_shell_button_does_not_reach_the_window`).
    ///
    /// No chrome under a client's surface: one over the windows is over their
    /// chrome too, and the press is the client's, which is what
    /// `pointer_button` does with it.
    /// `an_overlay_mapped_before_a_bar_is_drawn_over_it_and_takes_the_press`.
    /// While a scene holds a grab, a press anywhere is that scene's to take
    /// or to be dismissed by, so it is the surface's claim everywhere.
    /// `tests::real_client::reflow_on_close::hosted::while_a_grab_is_held_no_window_takes_focus_frame_or_cursor_from_the_pointer`.
    pub(crate) fn claim_under(&self, location: Point<f64, Logical>) -> Claim {
        if self.hosted_grab.is_some() {
            return claim_of(true, self.script_grab, None);
        }
        let above = self.topmost_above(location, Some(Asking::Press));
        let chrome = if matches!(above, Some(Above::Client(..))) {
            None
        } else {
            self.chrome_under(location).map(|under| under.chrome)
        };
        claim_of(
            matches!(above, Some(Above::Script(..))),
            self.script_grab,
            chrome,
        )
    }

    /// Say what the pointer is over the compositor's own chrome, or stop
    /// saying.
    ///
    /// **The one writer of the assertion, and it follows the press's whole
    /// precedence chain rather than its last link.** Issue #108 is the pointer
    /// describing an action other than the one a press will take, and the first
    /// fix for it read [`Self::chrome_under`] alone — which is the third thing
    /// `pointer_button` asks and not the first. So a thumbnail's corner in
    /// overview drew `NwseResize` while the press focused the window and left
    /// the mode, and the bottom edge of a scripted bar drew `NsResize` while
    /// the press went to the bar. [`Self::claim_under`] is the whole chain, and
    /// `Claim::cursor` says nothing for every link that is not chrome: where
    /// the press defers, the pointer defers with it.
    ///
    /// **Not while a drag is in progress.** A resize grab takes the pointer off
    /// the border it started on within a pixel of movement — the window
    /// follows, but the pointer is ahead of it, and past the window's edge
    /// entirely once the drag hits a minimum size or a screen edge.
    /// Recomputing would drop the resize cursor mid-drag and hand the pointer
    /// back to whatever client the cursor happened to be over, which is the one
    /// moment the shape must not change. Holding the last assertion for the
    /// length of the grab is also what makes a move drag keep the arrow, and
    /// what lets `input::pointer_button` assert a shape *as* it starts a grab
    /// and have it stay for the drag. A press a hosted scene holds is held
    /// the same way, so the shape stays the scene's for the press (Ruling 8):
    /// `tests::real_client::reflow_on_close::hosted::a_press_a_scene_holds_keeps_its_shape_over_a_resize_border`.
    pub(crate) fn assert_cursor(&mut self, location: Point<f64, Logical>, grabbed: bool) {
        if grabbed || self.scene_press.is_some() {
            return;
        }
        let icon = self.claim_under(location).cursor();
        if self.pointer.assert(icon) {
            self.redraw = true;
        }
    }

    /// Say again what the pointer is over, for a pointer that has not moved.
    ///
    /// **The other half of #108, and the one a motion handler cannot reach.**
    /// The compositor asserts its cursor when the pointer crosses into a
    /// titlebar or onto a resize border — but the crossing can also be the
    /// *window's*: a layout change, an animation landing, a workspace switch,
    /// a window resized by a script all move chrome under a pointer that is
    /// standing still, and there is no motion event for that. Without this a
    /// titlebar that slid under the pointer would keep whatever resize arrow
    /// was being shown a moment ago, which is the same disagreement between
    /// the shape and the action, arrived at from the other direction. Entering
    /// overview is the same crossing: nothing moved but the claim, and the
    /// pointer has to hear about it.
    ///
    /// **Called from [`crate::render::prepare`], not from [`Self::settle`].**
    /// `settle` runs after the frame it settles, so a titlebar sliding under a
    /// stationary pointer was drawn once with the previous shape and only
    /// corrected on the frame the self-inflicted damage bought. `prepare` runs
    /// once per frame ahead of every output, before any cursor element is
    /// built, so the shape this finds is the shape that frame draws.
    ///
    /// Once a frame is enough, and cheap: [`crate::cursor::Pointer::assert`]
    /// answers `false` when nothing changed, so the ordinary case costs one
    /// hit test and no damage. A frame that is not drawn is a screen on which
    /// nothing moved, so there is nothing to have missed.
    pub(crate) fn reassert_cursor(&mut self) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        self.assert_cursor(pointer.current_location(), pointer.is_grabbed());
    }

    /// The decorated window under `location`, frame or client, and where the
    /// pointer lands in its frame's own space.
    ///
    /// Wider than [`Self::chrome_under`] on purpose: a decoration that glows
    /// where the cursor is has to be told about the cursor while it is over
    /// the client, which is the client's surface and reports nothing to us.
    /// Ownership of clicks -- and, since #108, the pointer's shape -- is still
    /// decided by `chrome_under`; this is only for looking.
    pub(crate) fn decorated_under(
        &self,
        location: Point<f64, Logical>,
    ) -> Option<(crate::pane::PaneId, Point<f64, Logical>)> {
        let now = self.clock.now();
        let screens = self.screens();
        self.panes_front_first(location).find_map(|pane| {
            // Only a built frame is listening. There is no scene to tell about
            // the pointer until there is one.
            pane.decoration()?;
            let outer = self.pane_outer(pane);
            // Nor one on a screen that does not draw it, nor the frame of a
            // window whose client has gone: [`shown_at`], whose test is
            // `a_point_is_on_a_window_only_on_a_monitor_that_draws_it`. A pane
            // with a built frame needs Qt, which this binary's tests cannot
            // start, so this walk is not driven by one.
            if !shown_at(outer, pane.ghost(), location, &screens) {
                return None;
            }
            let drawn = self.drawn_at(pane, outer, now);
            // An invisible frame has nothing to glow. This walk only forwards
            // the pointer to a decoration's scene, so the cost of getting it
            // wrong is a hover state on a window that is not there rather than
            // a lost click -- but it is the same question as the other three
            // walks and it gets the same answer. See `present::Frame::covers`.
            if !drawn.covers(location) {
                return None;
            }
            let in_outer = present::to_window_space(drawn, outer, location) - outer.loc.to_f64();
            Some((pane.id(), in_outer))
        })
    }

    /// Whether the pointer is anywhere over this window, frame included.
    ///
    /// A decoration that lights up as the pointer approaches needs this even
    /// while the pointer is over the client area, which is the client's
    /// surface and sends us nothing.
    pub(crate) fn pointer_inside(&self, window: &Window) -> bool {
        let Some(outer) = self.outer_geometry(window) else {
            return false;
        };
        let Some(pointer) = self.seat.get_pointer() else {
            return false;
        };
        outer.to_f64().contains(pointer.current_location())
    }
}
