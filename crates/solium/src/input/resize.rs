//! Interactive resize.
//!
//! A pointer grab, like the move: while a button is held the drag owns every
//! pointer event, whatever surface happens to be under the cursor.
//!
//! Resizing is not animated, and that is deliberate rather than an omission.
//! The window has to be under the pointer's corner *this frame* — an animation
//! would put it where the pointer was a hundred milliseconds ago, which reads
//! as lag rather than as polish. Animations are for motion the user did not
//! personally drag.

use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, CursorIcon, GestureHoldBeginEvent, GestureHoldEndEvent,
        GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
        GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData,
        MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge,
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{Logical, Point, Rectangle},
};

use crate::state::Solium;

/// How close to an edge counts as grabbing it.
///
/// Generous on purpose: a border thin enough to look right is thinner than
/// anyone can reliably hit, so the region extends inside the window as well as
/// outside it.
pub(crate) const RESIZE_BORDER: i32 = 8;

/// The smallest a window may be dragged to.
///
/// Without a floor, a window can be resized to nothing and then cannot be
/// grabbed again to undo it.
const MINIMUM: i32 = 120;

/// Which corner a point is pulling, by which quarter of the window it is in.
///
/// Used for a modifier drag, where there is no edge to be near: the pointer is
/// somewhere in the middle of the window and the direction has to come from
/// where, rather than from what it is touching.
pub(crate) fn quadrant(outer: Rectangle<i32, Logical>, point: Point<f64, Logical>) -> ResizeEdge {
    let middle_x = f64::from(outer.loc.x) + f64::from(outer.size.w) / 2.0;
    let middle_y = f64::from(outer.loc.y) + f64::from(outer.size.h) / 2.0;
    match (point.x < middle_x, point.y < middle_y) {
        (true, true) => ResizeEdge::TopLeft,
        (false, true) => ResizeEdge::TopRight,
        (true, false) => ResizeEdge::BottomLeft,
        (false, false) => ResizeEdge::BottomRight,
    }
}

/// Which edges of a window a point is near, if any.
pub(crate) fn edges_at(outer: Rectangle<i32, Logical>, point: Point<f64, Logical>) -> ResizeEdge {
    let border = f64::from(RESIZE_BORDER);
    let (left, top) = (f64::from(outer.loc.x), f64::from(outer.loc.y));
    let right = left + f64::from(outer.size.w);
    let bottom = top + f64::from(outer.size.h);

    let near_left = (point.x - left).abs() <= border;
    let near_right = (point.x - right).abs() <= border;
    let near_top = (point.y - top).abs() <= border;
    let near_bottom = (point.y - bottom).abs() <= border;

    match (near_top, near_bottom, near_left, near_right) {
        (true, _, true, _) => ResizeEdge::TopLeft,
        (true, _, _, true) => ResizeEdge::TopRight,
        (_, true, true, _) => ResizeEdge::BottomLeft,
        (_, true, _, true) => ResizeEdge::BottomRight,
        (true, ..) => ResizeEdge::Top,
        (_, true, ..) => ResizeEdge::Bottom,
        (_, _, true, _) => ResizeEdge::Left,
        (_, _, _, true) => ResizeEdge::Right,
        _ => ResizeEdge::None,
    }
}

/// The edges a press at `point` would drag, or `None` if it would drag none.
///
/// The whole of the resize region, in one place: the border reaches
/// [`RESIZE_BORDER`] either side of every edge, so a point is on it only if it
/// is inside `drawn` grown by that much. [`edges_at`] cannot answer that on its
/// own — it measures the distance to each of the four *lines* and knows nothing
/// of the rectangle they bound, so a point a mile above the window and four
/// pixels to the left of its left edge comes back `Left`. The grown rectangle
/// is what rules that out, and it lived at the one call site until the cursor
/// became a second reader of the same region.
pub(crate) fn border_edges(
    drawn: Rectangle<i32, Logical>,
    point: Point<f64, Logical>,
) -> ResizeEdge {
    let grown = Rectangle::new(
        (drawn.loc.x - RESIZE_BORDER, drawn.loc.y - RESIZE_BORDER).into(),
        (
            drawn.size.w + RESIZE_BORDER * 2,
            drawn.size.h + RESIZE_BORDER * 2,
        )
            .into(),
    );
    if !grown.to_f64().contains(point) {
        return ResizeEdge::None;
    }
    edges_at(drawn, point)
}

/// The pointer the compositor shows over a resize border.
///
/// Four pictures for eight edges, and the pairing is the one every desktop
/// uses: a double-headed arrow lies along the axis the drag moves, so the two
/// corners on a diagonal share one cursor — top-left and bottom-right pull the
/// same line in opposite directions, which is `NwseResize`, and top-right with
/// bottom-left is `NeswResize`. These are the w3c names the `cursor-shape`
/// protocol speaks; [`crate::cursor::shape`] turns each of them into the file
/// an XCursor theme actually ships, so what is chosen here is a *meaning* and
/// not a picture.
///
/// `ResizeEdge::None` is not a resize target at all — [`border_edges`] returns
/// it for every point that is not near an edge — and it answers with the arrow
/// rather than with an `Option` because the arrow is what the compositor shows
/// over anything of its own it has nothing more specific to say about. The
/// trailing arm is the same answer for the same reason: `ResizeEdge` is a
/// protocol enum and `#[non_exhaustive]`, so it is a future edge rather than an
/// impossible case.
pub(crate) fn cursor(edges: ResizeEdge) -> CursorIcon {
    match edges {
        ResizeEdge::Top | ResizeEdge::Bottom => CursorIcon::NsResize,
        ResizeEdge::Left | ResizeEdge::Right => CursorIcon::EwResize,
        ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorIcon::NwseResize,
        ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorIcon::NeswResize,
        _ => CursorIcon::Default,
    }
}

/// Resizes a window with the pointer until the button is released.
pub(crate) struct ResizeGrab {
    start_data: GrabStartData<Solium>,
    window: Window,
    edges: ResizeEdge,
    /// The window's rectangle when the drag began.
    ///
    /// Every frame is computed from this and the total pointer movement, not
    /// from the previous frame. Accumulating per-frame deltas drifts, and the
    /// drift is worst exactly when the pointer moves fastest.
    began: Rectangle<i32, Logical>,
    /// The rectangle the *layout* had this pane at when the drag began.
    ///
    /// A second frozen rectangle rather than a second reading of the first, and
    /// the two are genuinely different questions. [`Self::began`] is
    /// `Solium::pane_outer` — where the window *is*, which is what a floating
    /// drag resizes and is rightly the client's own rectangle. This is
    /// `Solium::pane_laid_out` — where the layout *put* it, which is what a
    /// tiled drag has to move a seam from. They differ by however far the
    /// client has drifted from what it was asked for, which for a terminal is
    /// about a cell and for every client is silent. See [`dragged_edge`].
    ///
    /// Frozen at the grab for the same reason `began` is: the layout moves this
    /// pane on every frame of the drag, so a rectangle re-read each frame would
    /// have the previous frame's motion already in it and adding the total
    /// pointer delta to that is the accumulating-delta runaway by another
    /// spelling. It is not merely drift — the error is the whole of the
    /// previous frame's travel, every frame.
    laid_out: LaidOut,
    from: Point<f64, Logical>,
}

/// The rectangle the *layout* put a pane at, as opposed to where the window is.
///
/// A newtype rather than a bare `Rectangle`, because [`ResizeGrab::new`] takes
/// one of each and they were the same type. Swapping the two arguments compiled,
/// left every test in the workspace green, and silently reinstated the #124
/// review's second finding — a tiled drag starting from the client's rectangle
/// instead of the layout's, which moves the seam by the client's rounding
/// residue on the first frame and is invisible. Nothing anywhere constructs a
/// `ResizeGrab`, so no test could have caught it; the type can, and does it at
/// compile time for every call site that will ever exist.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct LaidOut(pub(crate) Rectangle<i32, Logical>);

impl ResizeGrab {
    pub(crate) fn new(
        start_data: GrabStartData<Solium>,
        window: Window,
        edges: ResizeEdge,
        began: Rectangle<i32, Logical>,
        laid_out: LaidOut,
    ) -> Self {
        let from = start_data.location;
        Self {
            start_data,
            window,
            edges,
            began,
            laid_out,
            from,
        }
    }

    /// The window's rectangle for a pointer at `now`.
    fn resized(&self, now: Point<f64, Logical>) -> Rectangle<i32, Logical> {
        resized(self.began, self.edges, self.from, now)
    }
}

/// Whether a drag on these edges moves the window's left edge, and with it its
/// origin.
///
/// Shared with [`crate::resizing`] rather than restated there. That module has
/// to pin the edges a drag is *not* holding when a client refuses the size it
/// was offered, which is the same question asked from the other side, and two
/// spellings of "is this drag pulling the left edge" is how the end of a
/// gesture comes to disagree with the middle of it.
pub(crate) const fn pulls_left(edges: ResizeEdge) -> bool {
    matches!(
        edges,
        ResizeEdge::Left | ResizeEdge::TopLeft | ResizeEdge::BottomLeft
    )
}

/// The same for the top edge. See [`pulls_left`].
pub(crate) const fn pulls_top(edges: ResizeEdge) -> bool {
    matches!(
        edges,
        ResizeEdge::Top | ResizeEdge::TopLeft | ResizeEdge::TopRight
    )
}

/// The sides a drag has hold of: the left-or-right one, and the top-or-bottom
/// one.
///
/// What a script is handed, and what it hands back to `tree:drag_seam`. A pair
/// because a corner drag is genuinely two drags — it moves one seam per axis —
/// and `None` on an axis means that axis is not in play at all, so `if
/// horizontal then` still reads the way it always did in Lua.
///
/// Named sides and not the two booleans this replaced. A layout cannot choose
/// a seam from an axis: a window that is the second child of a vertical split
/// has that split's seam on its left and no seam on its right, and "the
/// horizontal axis is being dragged" is true in both cases. That conflation is
/// #120. See `solium_layout::tree::Edge`.
///
/// `&'static str` rather than a layout type because these cross into Lua,
/// where an edge is spelled — and the spellings have to match
/// `solium_layout::tree::Edge`'s parser in `script.rs`, which is the one place
/// they are read back.
pub(crate) const fn sides(edges: ResizeEdge) -> (Option<&'static str>, Option<&'static str>) {
    // Two questions per axis, and only the first is this function's own:
    // whether the axis is in play at all, and then which of its two sides the
    // hand is on. [`pulls_left`] and [`pulls_top`] already answer the second,
    // and answering it again here is exactly the duplication their own docs
    // warn about -- two spellings of "is this drag pulling the left edge" is
    // how the end of a gesture comes to disagree with the middle of it.
    //
    // The arms below are the other question, which is a genuinely different
    // one: the edges that touch this axis. The catch-all stays `None` rather
    // than a side, so `ResizeEdge::None` -- and any variant the protocol grows
    // later -- reads as "this axis is not being dragged" instead of falling
    // into a direction nobody asked for.
    let horizontal = match edges {
        ResizeEdge::Left
        | ResizeEdge::TopLeft
        | ResizeEdge::BottomLeft
        | ResizeEdge::Right
        | ResizeEdge::TopRight
        | ResizeEdge::BottomRight => Some(if pulls_left(edges) { "left" } else { "right" }),
        _ => None,
    };
    let vertical = match edges {
        ResizeEdge::Top
        | ResizeEdge::TopLeft
        | ResizeEdge::TopRight
        | ResizeEdge::Bottom
        | ResizeEdge::BottomLeft
        | ResizeEdge::BottomRight => Some(if pulls_top(edges) { "top" } else { "bottom" }),
        _ => None,
    };
    (horizontal, vertical)
}

/// Where the dragged edge should come to rest, on each axis.
///
/// **The layout's own edge, moved by the pointer.** Each axis in play answers
/// with that side of `laid_out` — `Solium::pane_laid_out`, the rectangle the
/// layout put this pane at, frozen at the grab — plus the drag's *total*
/// pointer movement. So the first frame of every gesture hands back the number
/// the layout itself produced, whatever the pointer is doing, and every frame
/// after it is that number displaced by as far as the hand has gone.
///
/// To the pixel and not beyond it: `Solium::place` rounds the layout's `f64`
/// rectangle to whole pixels before `Pane::set_placed` stores it, so a seam the
/// tree put at `x.5` comes back half a pixel out. Constant for the gesture
/// rather than accumulating — the base is frozen — and a pixel is the unit a
/// client is configured in anyway, but it is a rounding and not an identity.
///
/// Nothing accumulates: the delta is measured from the grab and not from the
/// previous frame, which is the property `ResizeGrab::began`'s own doc insists
/// on — per-frame deltas drift, worst when the pointer moves fastest. The
/// *base* is frozen for the same reason and it is the sharper of the two: the
/// layout moves this pane on every frame of the drag, so re-reading its
/// rectangle each frame and adding the total delta would count the previous
/// frame's travel twice, and then three times, and the window would fly.
///
/// What this replaces is the pointer's own position, which `ResizeRequest`
/// carried to the layout until #124. A seam set from an absolute screen
/// coordinate lands under the cursor, so the grabbed edge teleported there on
/// the first frame of every drag. Worst on `super`+right-button, where
/// [`quadrant`] begins a resize from anywhere inside a window and the nearest
/// corner is therefore most of a window away; a border drag is the same fault
/// in miniature, because the grab region is a band [`RESIZE_BORDER`] wide on
/// either side of the edge and the seam jumped to wherever in it the press
/// landed. `drag_seam`'s arithmetic is untouched by this — it is handed a
/// relatively-derived target instead of an absolute one, and stays the tested
/// thing it was.
///
/// **`laid_out` and not the rectangle [`resized`] produces, which is the #124
/// review's finding and the reason this takes two arguments where it took
/// one.** Reading the edge off `wanted` was wrong twice over, and both faults
/// are first-frame jumps — the very thing #124 exists to remove:
///
/// * `wanted` is in the *client's* space. It is built from `Solium::pane_outer`
///   at grab start, and `pane_outer` goes through `pane_geometry`, which for a
///   mapped client with no hold live answers `real_geometry` — the space's
///   location paired with the size the client last *committed*. A client that
///   quantises, which every terminal does, therefore moved the seam by its own
///   rounding residue on frame one. Only the far edges: `real.loc` is
///   compositor-set, so left and top were exact and right and bottom carried
///   the whole of it. `crate::resizing` puts the threshold for calling such an
///   answer a rounding at `max(asked / 20, CELL)`, which is at or above the
///   half-gap #120 was about, and silent.
/// * `wanted` has already been floored at [`MINIMUM`]. A tile narrower than 120
///   outer pixels on the dragged axis — reachable through `tree:resize`, deep
///   dwindle nesting, or a `split` setting — therefore reported its edge at
///   `began ± 120` from frame one, so grabbing a 48px tile's right edge threw
///   it 72px before the pointer moved, and left it dead in one direction and
///   72px behind in the other for the rest of the gesture. The pointer is never
///   floored, so nothing about that jump was the user's.
///
/// `wanted` is not wrong; it is answering the other question. It is the
/// *client's* rectangle, it is rightly clamped, and it goes on serving the
/// floating path (#113) exactly as before. This function simply stopped asking
/// it about the layout.
///
/// **One edge per axis, each read from its own pair of `laid_out`'s numbers.** A
/// corner drag is genuinely two drags — one seam per axis, which is what
/// [`sides`] says — so the vertical seam is set from `laid_out`'s left-or-right
/// edge and the horizontal one from its top-or-bottom. Neither axis may borrow
/// the other's edge: a `TopRight` drag moving the pointer right and up has to
/// send the right edge right and the top edge up, and a single number cannot
/// be both.
///
/// **The value is in the layout's outer coordinate space, by construction
/// rather than by argument.** `laid_out` is the rectangle `sol.place` was
/// handed — `Solium::place` takes a script's rect as the pane's outer rectangle
/// and subtracts the insets itself — so this is the space `tree:layout` returns
/// and therefore the space `solium_layout::tree::Tiling::node_box` measures a
/// seam in. It is the layout's own number going back to the layout. Verified
/// across that boundary rather than assumed:
/// `crate::state::tests::real_client::a_client_that_rounds_its_size_does_not_move_the_seam`
/// lays a real `Tiling` out through `Solium::place`, lets its client commit a
/// cell less than it was asked for, and pins that the edge this hands over
/// still leaves the tree's own seam where it was.
///
/// **Nothing is floored here, and that is the point.** The layout clamps what
/// the layout owns: `drag_seam` bounds a *ratio* of the seam's box to
/// 0.05..0.95, which is measured against a rectangle that is not this window
/// and knows nothing of its pixels. [`MINIMUM`] is a floor on a *window's*
/// size and belongs to `resized` and the floating path, where a window is what
/// is being sized. Applying it here put a window's floor on a seam's position,
/// which is the sub-`MINIMUM` jump above.
///
/// An axis the drag has no hold of has no dragged edge, and there the
/// pointer's own coordinate is passed through unchanged. The only shipped
/// reader of it on such an axis is `scrolling.lua`, which reads the first
/// coordinate as a delta whatever the drag is doing — that is #122, a defect
/// in that layout rather than in this gesture — so what it reads on an axis
/// nobody is dragging is left exactly what it was.
pub(crate) fn dragged_edge(
    laid_out: LaidOut,
    edges: ResizeEdge,
    from: Point<f64, Logical>,
    now: Point<f64, Logical>,
) -> (f64, f64) {
    // [`sides`] answers only the first of the two questions -- whether this
    // axis is in play at all. Which of its two sides the hand is on comes from
    // [`pulls_left`] and [`pulls_top`], for the reason `sides` gives in its own
    // body: two spellings of "is this drag pulling the left edge" is how the
    // end of a gesture comes to disagree with the middle of it.
    let (horizontal, vertical) = sides(edges);
    let laid_out = laid_out.0;
    // Not rounded, unlike `resized`'s. A window's rectangle is whole pixels
    // because a client is configured in them; a seam's position is not -- every
    // number `drag_seam` works in is an `f64`, and it divides this one by a
    // box's width to get a ratio. Rounding here would quantise a gesture that
    // has no reason to be quantised, and on a scaled output a logical half-pixel
    // is a real one.
    let (dx, dy) = (now.x - from.x, now.y - from.y);
    let x = if horizontal.is_none() {
        now.x
    } else if pulls_left(edges) {
        f64::from(laid_out.loc.x) + dx
    } else {
        f64::from(laid_out.loc.x + laid_out.size.w) + dx
    };
    let y = if vertical.is_none() {
        now.y
    } else if pulls_top(edges) {
        f64::from(laid_out.loc.y) + dy
    } else {
        f64::from(laid_out.loc.y + laid_out.size.h) + dy
    };
    (x, y)
}

/// Where a drag from `from` to `now` puts a window that started at `began`.
///
/// A free function so it can be tested without a compositor: this is the whole
/// of resizing that can be wrong, and it should not need a Wayland display to
/// check.
fn resized(
    began: Rectangle<i32, Logical>,
    edges: ResizeEdge,
    from: Point<f64, Logical>,
    now: Point<f64, Logical>,
) -> Rectangle<i32, Logical> {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a pointer delta is screen-sized"
    )]
    let (dx, dy) = (
        (now.x - from.x).round() as i32,
        (now.y - from.y).round() as i32,
    );

    let mut rect = began;
    let pulls_right = matches!(
        edges,
        ResizeEdge::Right | ResizeEdge::TopRight | ResizeEdge::BottomRight
    );
    let pulls_bottom = matches!(
        edges,
        ResizeEdge::Bottom | ResizeEdge::BottomLeft | ResizeEdge::BottomRight
    );

    if pulls_right {
        rect.size.w = (began.size.w + dx).max(MINIMUM);
    }
    if pulls_bottom {
        rect.size.h = (began.size.h + dy).max(MINIMUM);
    }
    // Dragging a left or top edge moves the window as well as resizing it, and
    // the opposite edge must stay put — so the size is clamped first and the
    // position derived from it, not the other way round.
    if pulls_left(edges) {
        rect.size.w = (began.size.w - dx).max(MINIMUM);
        rect.loc.x = began.loc.x + began.size.w - rect.size.w;
    }
    if pulls_top(edges) {
        rect.size.h = (began.size.h - dy).max(MINIMUM);
        rect.loc.y = began.loc.y + began.size.h - rect.size.h;
    }
    rect
}

impl PointerGrab<Solium> for ResizeGrab {
    fn motion(
        &mut self,
        data: &mut Solium,
        handle: &mut PointerInnerHandle<'_, Solium>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        // Recorded, not applied. A layout may want this to move a seam rather
        // than change one window's size, and asking it from in here would call
        // a script while the seat holds the pointer's lock — the deadlock the
        // move grab already taught us about.
        //
        // `edges` alone, because it already says everything the two derived
        // booleans said and one thing they could not: which side. See
        // [`sides`], which does the derivation once, where the script call is
        // made, instead of here where it was thrown away.
        //
        // Two rectangles and two answers, because there are two questions.
        // `wanted` is where this *window* would go and is the floating path's
        // (#113); `edge_at` is where the layout's *seam* should go, and it is
        // built from the layout's own rectangle rather than from `wanted`,
        // which is in the client's space and already floored. The layout used
        // to be handed `event.location` here, which is the whole of #124. See
        // [`dragged_edge`].
        let wanted = self.resized(event.location);
        data.pending_resize = Some(crate::state::ResizeRequest {
            window: self.window.clone(),
            wanted,
            edge_at: dragged_edge(self.laid_out, self.edges, self.from, event.location),
            edges: self.edges,
        });
    }

    fn relative_motion(
        &mut self,
        data: &mut Solium,
        handle: &mut PointerInnerHandle<'_, Solium>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, None, event);
    }

    fn button(
        &mut self,
        data: &mut Solium,
        handle: &mut PointerInnerHandle<'_, Solium>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        let started_with = self.start_data.button;
        if !handle.current_pressed().contains(&started_with) {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(
        &mut self,
        data: &mut Solium,
        handle: &mut PointerInnerHandle<'_, Solium>,
        details: AxisFrame,
    ) {
        handle.axis(data, details);
    }

    fn frame(&mut self, data: &mut Solium, handle: &mut PointerInnerHandle<'_, Solium>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut Solium,
        handle: &mut PointerInnerHandle<'_, Solium>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut Solium,
        handle: &mut PointerInnerHandle<'_, Solium>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut Solium,
        handle: &mut PointerInnerHandle<'_, Solium>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut Solium,
        handle: &mut PointerInnerHandle<'_, Solium>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut Solium,
        handle: &mut PointerInnerHandle<'_, Solium>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut Solium,
        handle: &mut PointerInnerHandle<'_, Solium>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut Solium,
        handle: &mut PointerInnerHandle<'_, Solium>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut Solium,
        handle: &mut PointerInnerHandle<'_, Solium>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &GrabStartData<Solium> {
        &self.start_data
    }

    /// The gesture is over, however it ended.
    ///
    /// Smithay calls this when the grab is removed — the button coming up, and
    /// also a grab being replaced or the seat being reset, which is why the
    /// release is hooked here rather than in [`Self::button`]. Any of them ends
    /// the drag, and a drag that ended without saying so leaves the pane's slot
    /// holding a rectangle nothing will ever reconcile.
    ///
    /// Safe to call from inside a grab callback because it touches no seat: it
    /// sets a deadline and sends one configure. Anything that asked the seat
    /// where the pointer is would deadlock here — see `Solium::pending_drop`
    /// for the version of that lesson that cost a frozen compositor.
    fn unset(&mut self, data: &mut Solium) {
        data.release_resize(&self.window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    #[test]
    fn corners_win_over_single_edges() {
        let window = rect(100, 100, 400, 300);
        // Within the border of both the left and the top edge.
        assert_eq!(
            edges_at(window, (102.0, 103.0).into()),
            ResizeEdge::TopLeft,
            "a corner must resize both axes, not whichever was checked first"
        );
        assert_eq!(edges_at(window, (300.0, 102.0).into()), ResizeEdge::Top);
        assert_eq!(edges_at(window, (498.0, 250.0).into()), ResizeEdge::Right);
        assert_eq!(edges_at(window, (300.0, 250.0).into()), ResizeEdge::None);
    }

    #[test]
    fn the_border_reaches_both_ways() {
        let window = rect(100, 100, 400, 300);
        // Just outside, and just inside. A border only on one side is half as
        // wide as it looks, and misses.
        assert_eq!(edges_at(window, (94.0, 250.0).into()), ResizeEdge::Left);
        assert_eq!(edges_at(window, (106.0, 250.0).into()), ResizeEdge::Left);
    }

    /// The border is a region, not four unbounded lines.
    ///
    /// [`edges_at`] answers `Left` for a point a mile above the window, because
    /// all it measures is the distance to the left edge's *line*. That was
    /// harmless while the only caller checked the grown rectangle first and
    /// stopped being harmless the moment the cursor became a second reader: a
    /// pointer nowhere near a window would have drawn a resize arrow.
    #[test]
    fn the_border_stops_where_the_window_does() {
        let window = rect(100, 100, 400, 300);
        assert_eq!(
            edges_at(window, (102.0, -900.0).into()),
            ResizeEdge::Left,
            "the control: the naked edge test claims a point nowhere near the \
             window, which is why `border_edges` exists"
        );
        assert_eq!(
            border_edges(window, (102.0, -900.0).into()),
            ResizeEdge::None
        );
        assert_eq!(
            border_edges(window, (102.0, 103.0).into()),
            ResizeEdge::TopLeft
        );
        // Eight pixels outside is still the border; nine is not. This is the
        // outside half of it, which is the only part of a framed window's top
        // edge a drag can reach -- see `state::chrome_of`.
        assert_eq!(border_edges(window, (300.0, 93.0).into()), ResizeEdge::Top);
        assert_eq!(border_edges(window, (300.0, 91.0).into()), ResizeEdge::None);
        // And the pixel itself, which the two above straddled: 92 is exactly
        // `RESIZE_BORDER` from the top edge at 100, and it is the last one that
        // counts. Skipping the boundary is skipping the only value the
        // comparison can get wrong -- 93 and 91 pass with `<` or `<=` alike.
        assert_eq!(border_edges(window, (300.0, 92.0).into()), ResizeEdge::Top);

        // The far side is *not* its mirror, and it is worth saying so rather
        // than leaving it to be rediscovered. `edges_at` measures distance and
        // is closed at both ends, but `border_edges` first asks a half-open
        // rectangle: `grown` runs from 92 up to but not including 408, so the
        // top and left edges reach a full eight pixels out and the bottom and
        // right reach eight minus an epsilon. Nobody can hit a tenth of a pixel
        // with a mouse and the asymmetry is invisible in use, so it is pinned
        // as it stands rather than papered over -- growing the rectangle by one
        // to even it up would make `RESIZE_BORDER` mean nine on two sides.
        assert_eq!(
            edges_at(window, (300.0, 408.0).into()),
            ResizeEdge::Bottom,
            "the distance test is closed at both ends"
        );
        assert_eq!(
            border_edges(window, (300.0, 408.0).into()),
            ResizeEdge::None,
            "and the half-open rectangle is what shortens the far side"
        );
        assert_eq!(
            border_edges(window, (300.0, 407.0).into()),
            ResizeEdge::Bottom
        );
    }

    /// Every edge names its own cursor, and the two diagonals are not the same
    /// one.
    ///
    /// Issue #108's first symptom was that dragging a corner resized the window
    /// with the pointer still an arrow, so what this pins is the mapping that
    /// was missing entirely. The pairing matters as much as the coverage: a
    /// `NwseResize` on all four corners looks right in a screenshot of one
    /// corner and wrong on the other diagonal, which is the sort of thing
    /// nobody notices until they are dragging the top-right of a window.
    #[test]
    fn every_edge_names_the_cursor_that_lies_along_it() {
        assert_eq!(cursor(ResizeEdge::Top), CursorIcon::NsResize);
        assert_eq!(cursor(ResizeEdge::Bottom), CursorIcon::NsResize);
        assert_eq!(cursor(ResizeEdge::Left), CursorIcon::EwResize);
        assert_eq!(cursor(ResizeEdge::Right), CursorIcon::EwResize);
        assert_eq!(cursor(ResizeEdge::TopLeft), CursorIcon::NwseResize);
        assert_eq!(cursor(ResizeEdge::BottomRight), CursorIcon::NwseResize);
        assert_eq!(cursor(ResizeEdge::TopRight), CursorIcon::NeswResize);
        assert_eq!(cursor(ResizeEdge::BottomLeft), CursorIcon::NeswResize);
        assert_ne!(
            cursor(ResizeEdge::TopLeft),
            cursor(ResizeEdge::TopRight),
            "the two diagonals are mirror images and must not share a cursor"
        );
        assert_eq!(
            cursor(ResizeEdge::None),
            CursorIcon::Default,
            "no edge is the compositor's own arrow, not a resize arrow for an \
             edge that is not there"
        );
    }

    #[test]
    fn dragging_the_right_edge_only_changes_the_width() {
        let began = rect(100, 100, 400, 300);
        assert_eq!(
            resized(
                began,
                ResizeEdge::Right,
                (500.0, 250.0).into(),
                (600.0, 250.0).into()
            ),
            rect(100, 100, 500, 300)
        );
    }

    #[test]
    fn dragging_a_left_edge_moves_the_window_and_pins_the_right() {
        let began = rect(100, 100, 400, 300);
        let after = resized(
            began,
            ResizeEdge::Left,
            (100.0, 250.0).into(),
            (150.0, 250.0).into(),
        );
        assert_eq!(after, rect(150, 100, 350, 300));
        assert_eq!(
            after.loc.x + after.size.w,
            began.loc.x + began.size.w,
            "the edge that was not grabbed must not move"
        );
    }

    #[test]
    fn a_window_cannot_be_dragged_smaller_than_the_minimum() {
        let began = rect(100, 100, 400, 300);
        // Dragged far past the opposite edge.
        let after = resized(
            began,
            ResizeEdge::Left,
            (100.0, 250.0).into(),
            (900.0, 250.0).into(),
        );
        assert_eq!(after.size.w, MINIMUM);
        assert_eq!(
            after.loc.x + after.size.w,
            began.loc.x + began.size.w,
            "clamping must not let the window drift away from the pinned edge"
        );
    }

    #[test]
    fn every_frame_is_computed_from_where_the_drag_started() {
        // Not from the previous frame: accumulating deltas drifts, and drifts
        // worst exactly when the pointer moves fastest.
        let began = rect(100, 100, 400, 300);
        let from: Point<f64, Logical> = (500.0, 400.0).into();
        let direct = resized(began, ResizeEdge::BottomRight, from, (700.0, 500.0).into());
        assert_eq!(direct, rect(100, 100, 600, 400));
    }

    /// #124: a drag moves the edge it has hold of, from where that edge is.
    ///
    /// Every test here goes through [`handed`], which is `ResizeGrab::motion`'s
    /// payload written out: the pane's laid-out rectangle, and the edge the
    /// layout is handed off it for a given pointer travel. That is the whole of
    /// what changed — `drag_seam` is untouched — so it is the whole of what has
    /// to be pinned on this side.
    ///
    /// **Every test below fails against `bf80265`**, where the payload was
    /// `(event.location.x, event.location.y)`. Confirmed rather than assumed,
    /// by replacing [`dragged_edge`]'s body with exactly that expression and
    /// running them. The margins are 3px where a border was grabbed three
    /// pixels inside its band, 50px on each axis for a modifier drag begun a
    /// quarter of the way into a window, 5px for the pass-through case, and
    /// 800px where the pointer was shoved clear across the screen.
    ///
    /// Even [`an_axis_the_drag_does_not_move_passes_the_pointer_through`] fails
    /// there, and that is worth writing down because it is the one test whose
    /// *subject* did not change. Only its first assertion holds on the revert:
    /// the drag's own axis moved as well, and that axis is the broken half. A
    /// test is not a witness for whichever assertion was in mind while writing
    /// it.
    ///
    /// [`a_tile_under_the_minimum_keeps_its_own_edge`] additionally fails
    /// against `ec1da24`, where [`dragged_edge`] read its edge off `resized`'s
    /// output and inherited that function's [`MINIMUM`] floor. It is the one
    /// test here that is about the first fix rather than about the defect.
    ///
    /// The numbers are integers widened to `f64` plus an exact pointer travel,
    /// or the pointer passed through untouched, so these compare exactly rather
    /// than within a tolerance. A tolerance here would be a place for a
    /// rounding to hide.
    mod dragged_edge_tests {
        use super::*;

        /// One frame of a drag, as `ResizeGrab::motion` assembles it: the edge
        /// the layout is handed for a pane laid out at `laid_out`, grabbed at
        /// `from`, with the pointer now at `now`.
        ///
        /// `laid_out` and not the rectangle [`resized`] returns. The two are the
        /// same number for a client sitting at exactly the size it was asked
        /// for, which every case in this module is; what they do *not* share is
        /// a floor, and [`a_tile_under_the_minimum_keeps_its_own_edge`] is
        /// where that separation is pinned. The case where they differ by a
        /// client's own rounding cannot be reached from here at all — it needs
        /// a real client to commit a real buffer — and lives in
        /// `crate::state::tests::real_client`.
        fn handed(
            laid_out: Rectangle<i32, Logical>,
            edges: ResizeEdge,
            from: Point<f64, Logical>,
            now: Point<f64, Logical>,
        ) -> (f64, f64) {
            dragged_edge(LaidOut(laid_out), edges, from, now)
        }

        /// A window whose four edges are at 100, 500, 100 and 400.
        fn window() -> Rectangle<i32, Logical> {
            rect(100, 100, 400, 300)
        }

        /// The first frame of a border drag moves nothing, wherever in the
        /// band the press landed.
        ///
        /// This is the test that matters. A border grab is a region
        /// [`RESIZE_BORDER`] wide on *either* side of the edge — sixteen pixels
        /// across, and `the_border_reaches_both_ways` above pins that it is —
        /// so the press is almost never on the edge itself. Handed the pointer,
        /// the layout put the seam wherever in that band the button went down,
        /// and the window jumped before it moved. Each grab below is therefore
        /// deliberately off-centre in its band, and off-centre along the other
        /// axis too, so a mixed-up pair of coordinates cannot pass.
        #[test]
        fn a_border_drag_begun_off_the_edge_does_not_move_it_on_the_first_frame() {
            for (edges, grab, axis, edge) in [
                (ResizeEdge::Right, (497.0, 137.0), 0, 500.0),
                (ResizeEdge::Left, (104.0, 362.0), 0, 100.0),
                (ResizeEdge::Top, (233.0, 106.0), 1, 100.0),
                (ResizeEdge::Bottom, (411.0, 395.0), 1, 400.0),
            ] {
                let grab: Point<f64, Logical> = grab.into();
                let sent = handed(window(), edges, grab, grab);
                let sent = if axis == 0 { sent.0 } else { sent.1 };
                assert_eq!(
                    sent, edge,
                    "{edges:?} grabbed at {grab:?} must hand over the edge at \
                     {edge}, not the press"
                );
            }
        }

        /// The same for `super`+right-button, which begins from anywhere at all.
        ///
        /// [`quadrant`] picks the nearest corner of a window the pointer is
        /// somewhere inside, so there is no band and no near-miss: the corner
        /// being dragged is most of a window away from the hand. The last case
        /// is the worst one on purpose — a press a pixel from the centre, whose
        /// corner is 199px and 149px off.
        #[test]
        fn a_modifier_drag_from_inside_a_window_does_not_throw_the_corner() {
            for (grab, corner) in [
                ((150.0, 150.0), (100.0, 100.0)),
                ((450.0, 150.0), (500.0, 100.0)),
                ((150.0, 350.0), (100.0, 400.0)),
                ((450.0, 350.0), (500.0, 400.0)),
                ((301.0, 251.0), (500.0, 400.0)),
            ] {
                let grab: Point<f64, Logical> = grab.into();
                // The edges the gesture itself would choose, not edges chosen
                // by the test: `quadrant` is what decides them at the press,
                // and a test that named them would be pinning a different
                // gesture from the one the user makes.
                let edges = quadrant(window(), grab);
                assert_eq!(
                    handed(window(), edges, grab, grab),
                    corner,
                    "a press at {grab:?} chose {edges:?} and must hand over \
                     that corner of the window, not the press"
                );
            }
        }

        /// And once it is moving, the edge moves as far as the pointer does.
        ///
        /// The other half of "relative": not teleporting is worthless if the
        /// edge then lags or doubles. Each case starts off-edge, so a payload
        /// that was accidentally still absolute would land on the pointer
        /// rather than on these.
        #[test]
        fn a_pointer_delta_of_n_moves_the_edge_by_n() {
            const N: f64 = 37.0;
            for (edges, grab, axis, edge, sign) in [
                (ResizeEdge::Right, (497.0, 137.0), 0, 500.0, 1.0),
                (ResizeEdge::Left, (104.0, 362.0), 0, 100.0, -1.0),
                (ResizeEdge::Top, (233.0, 106.0), 1, 100.0, -1.0),
                (ResizeEdge::Bottom, (411.0, 395.0), 1, 400.0, 1.0),
            ] {
                let grab: Point<f64, Logical> = grab.into();
                let now: Point<f64, Logical> = if axis == 0 {
                    (grab.x + sign * N, grab.y).into()
                } else {
                    (grab.x, grab.y + sign * N).into()
                };
                let sent = handed(window(), edges, grab, now);
                let sent = if axis == 0 { sent.0 } else { sent.1 };
                assert_eq!(
                    sent,
                    edge + sign * N,
                    "{edges:?} moved by {} and the edge must move with it",
                    sign * N
                );
            }
        }

        /// A corner is two drags, and neither axis may borrow the other's edge.
        ///
        /// Pulled in opposite directions on purpose — right and *up* — so the
        /// two answers are different numbers moving different ways. A payload
        /// that sent one axis's edge on both would be visible as equal
        /// displacements; one that sent the pointer would land on the pointer,
        /// which is neither.
        #[test]
        fn a_corner_drag_gives_each_axis_its_own_edge() {
            let grab: Point<f64, Logical> = (450.0, 150.0).into();
            let edges = quadrant(window(), grab);
            assert_eq!(edges, ResizeEdge::TopRight, "the fixture, restated");

            let now: Point<f64, Logical> = (490.0, 110.0).into();
            let sent = handed(window(), edges, grab, now);
            assert_eq!(
                sent,
                (540.0, 60.0),
                "the right edge goes right by 40 and the top edge up by 40"
            );
            assert_ne!(
                sent,
                (now.x, now.y),
                "and neither of them is where the pointer is"
            );
        }

        /// An axis with no dragged edge hands the pointer through, unchanged.
        ///
        /// Deliberate and pinned rather than left to be inferred. `sides` says
        /// nil for such an axis and any handler that checks its side before
        /// reading the coordinate never sees this value; the one that does not
        /// is `scrolling.lua`, which reads the first coordinate as a delta
        /// whatever the drag is. That is #122, a defect in that layout, and
        /// leaving what it reads on a top-or-bottom drag exactly as it was
        /// keeps this branch out of it.
        ///
        /// The two assertions are split because only the first of them is an
        /// unchanged claim: it is the sole assertion in this module that still
        /// holds against `bf80265`. The second is here so the test cannot pass
        /// on a payload that left *both* axes alone, and it is why the test as
        /// a whole fails on the revert like every other one.
        #[test]
        fn an_axis_the_drag_does_not_move_passes_the_pointer_through() {
            let grab: Point<f64, Logical> = (411.0, 395.0).into();
            let now: Point<f64, Logical> = (418.0, 432.0).into();
            let sent = handed(window(), ResizeEdge::Bottom, grab, now);
            assert_eq!(
                sent.0, now.x,
                "no horizontal edge is being dragged, so x is the pointer's"
            );
            assert_eq!(
                sent.1, 437.0,
                "and the bottom edge, which is being dragged, moved the 37 the \
                 pointer did rather than landing on it"
            );
        }

        /// The edge handed over is **not** floored, and the floor it used to
        /// carry was a first-frame jump of its own.
        ///
        /// **This is the #124 review's first finding.** [`dragged_edge`] read
        /// its edge off [`resized`]'s output, and `resized` clamps a window's
        /// *size* to [`MINIMUM`] — so for any tile already narrower than 120
        /// outer pixels on the dragged axis, the reported edge was
        /// `opposite ± MINIMUM` from the very first frame, with the pointer
        /// still on the pixel it pressed. The 48px tile below would have handed
        /// over 72px of travel nobody asked for, and then been dead in one
        /// direction and 72px behind in the other for the rest of the gesture.
        ///
        /// Sub-`MINIMUM` tiles are not hypothetical: `tree:resize` will make
        /// one from the keyboard, deep dwindle nesting produces them on its own,
        /// and a small `split` setting does it at the first window. Nothing
        /// bounds a *tile* at 120px, because `MINIMUM` is a floor on a
        /// floating window's size and a tiled window does not have one.
        ///
        /// The two floors were never the same quantity: `drag_seam` bounds a
        /// *ratio* of the seam's own box, which is a different number in
        /// different units against a rectangle that is not this window. That
        /// clamp is now the only one on a tiled drag, which is where a clamp on
        /// the layout's arrangement belongs. `resized` keeps its floor for the
        /// floating path, which is what it was always for.
        ///
        /// Both halves are asserted: an untouched pointer leaves a tiny tile's
        /// edge exactly where it is, and a pointer that runs clear off the far
        /// side is still followed rather than stopping at a window's minimum.
        /// Against `ec1da24` the first assertion is out by 72px and the second
        /// by 748.
        #[test]
        fn a_tile_under_the_minimum_keeps_its_own_edge() {
            // 48 outer pixels wide: well under `MINIMUM`, and a perfectly
            // ordinary tile.
            let tile = rect(300, 100, 48, 300);
            let grab: Point<f64, Logical> = (346.0, 250.0).into();

            assert_eq!(
                handed(tile, ResizeEdge::Right, grab, grab).0,
                348.0,
                "a drag that has not moved hands back the tile's own right \
                 edge, whatever `MINIMUM` says a window may be"
            );
            assert_eq!(
                handed(tile, ResizeEdge::Right, grab, (1146.0, 250.0).into()).0,
                1148.0,
                "and once it moves, the edge goes the whole 800 the pointer \
                 did -- clamping a seam is the layout's job and it has \
                 its own"
            );

            // **All four, and not because the arithmetic looks symmetric.**
            // The #124 review's second finding was asymmetric in exactly this
            // way -- left and top were exact while right and bottom skewed,
            // because `real.loc` is compositor-set and `real.size` is the
            // client's -- so a suite that pins one side of one axis and trusts
            // symmetry for the rest is the shape that let that through.
            let short = rect(300, 100, 48, 32);
            for (edges, at, moved) in [
                (ResizeEdge::Left, 300.0, -800.0),
                (ResizeEdge::Right, 348.0, 800.0),
                (ResizeEdge::Top, 100.0, -800.0),
                (ResizeEdge::Bottom, 132.0, 800.0),
            ] {
                let vertical = matches!(edges, ResizeEdge::Top | ResizeEdge::Bottom);
                let read = |sent: (f64, f64)| if vertical { sent.1 } else { sent.0 };
                assert_eq!(
                    read(handed(short, edges, grab, grab)),
                    at,
                    "{edges:?}: a drag that has not moved hands back the tile's \
                     own edge, on a tile under `MINIMUM` in both axes"
                );
                let to: Point<f64, Logical> = if vertical {
                    (grab.x, grab.y + moved).into()
                } else {
                    (grab.x + moved, grab.y).into()
                };
                assert_eq!(
                    read(handed(short, edges, grab, to)),
                    at + moved,
                    "{edges:?}: and it follows the pointer the whole way, in \
                     both directions"
                );
            }
        }

        /// Sub-pixel pointer travel survives the trip to the layout.
        ///
        /// [`resized`] rounds its delta to whole pixels because it is building
        /// a rectangle a client will be configured with, and configures are in
        /// whole pixels. `drag_seam` is under no such constraint — it divides
        /// this number by a box's width to get a ratio, and every term in that
        /// arithmetic is an `f64` — so the rounding is dropped here rather than
        /// inherited.
        ///
        /// Worth a test rather than a comment because it is the one behaviour
        /// this change takes *away* from a value that had it: reading the edge
        /// off `wanted` quantised the layout's target to whole logical pixels,
        /// which on a fractionally-scaled output is more than one device pixel.
        #[test]
        fn the_layout_is_handed_the_pointer_travel_unrounded() {
            let grab: Point<f64, Logical> = (497.0, 137.0).into();
            let sent = handed(window(), ResizeEdge::Right, grab, (497.5, 137.0).into());
            assert_eq!(
                sent.0, 500.5,
                "half a pixel of travel is half a pixel at the seam, not none \
                 of one and not a whole one"
            );
        }
    }
}
