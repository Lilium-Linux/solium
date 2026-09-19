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
    from: Point<f64, Logical>,
}

impl ResizeGrab {
    pub(crate) fn new(
        start_data: GrabStartData<Solium>,
        window: Window,
        edges: ResizeEdge,
        began: Rectangle<i32, Logical>,
    ) -> Self {
        let from = start_data.location;
        Self {
            start_data,
            window,
            edges,
            began,
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
/// **This is the relative half of a tiled drag, and it is relative by
/// construction rather than by arithmetic of its own.** `wanted` is the whole
/// gesture already: [`resized`] builds it from the rectangle the drag began on
/// and the *total* pointer movement since, so an edge read off it starts
/// exactly where that edge was and has travelled exactly as far as the pointer
/// has. Nothing here accumulates anything, which is the property `ResizeGrab`'s
/// own field doc insists on — per-frame deltas drift, worst when the pointer
/// moves fastest — and reusing `wanted` is how this gesture inherits it rather
/// than restating it.
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
/// **One edge per axis, each read from its own pair of `wanted`'s numbers.** A
/// corner drag is genuinely two drags — one seam per axis, which is what
/// [`sides`] says — so the vertical seam is set from `wanted`'s left-or-right
/// edge and the horizontal one from its top-or-bottom. Neither axis may borrow
/// the other's edge: a `TopRight` drag moving the pointer right and up has to
/// send the right edge right and the top edge up, and a single number cannot
/// be both.
///
/// **The value is in the layout's outer coordinate space.** `began` is
/// `Solium::pane_outer` at grab start — see `Solium::begin_resize`, which both
/// grab sites in `crate::input` read it from — and [`resized`] only adds pointer
/// movement to it, so an edge off `wanted` is an outer edge. That is the space
/// `sol.place` writes (`Solium::place` takes a script's rect as the pane's
/// outer rectangle and subtracts the insets itself), therefore the space
/// `tree:layout` handed back, therefore the space
/// `solium_layout::tree::Tiling::node_box` measures a seam in. Verified rather
/// than assumed: `tree::dragged_edge_tests::a_window_handed_its_own_edge_does_not_move`
/// gives a leaf its own laid-out edge at the shipped `gap: 12` and pins that
/// nothing moves, which is exactly where a half-gap of skew — #120's symptom
/// reached by another route — would show.
///
/// The edge handed over is **already floored**: [`resized`] clamps the size to
/// [`MINIMUM`], so a drag shoved past the far edge sends the floored edge and
/// not a crossed-over one. `drag_seam`'s `0.05..0.95` clamp (#115) is a second
/// and unrelated floor — it bounds a *ratio* of the seam's box and knows
/// nothing of pixels or of this one — so whichever bites first wins, and
/// neither can be derived from the other or dropped because the other exists.
///
/// An axis the drag has no hold of has no dragged edge, and there the
/// pointer's own coordinate is passed through unchanged. The only shipped
/// reader of it on such an axis is `scrolling.lua`, which reads the first
/// coordinate as a delta whatever the drag is doing — that is #122, a defect
/// in that layout rather than in this gesture — so what it reads on an axis
/// nobody is dragging is left exactly what it was.
fn dragged_edge(
    wanted: Rectangle<i32, Logical>,
    edges: ResizeEdge,
    pointer: Point<f64, Logical>,
) -> (f64, f64) {
    // [`sides`] answers only the first of the two questions -- whether this
    // axis is in play at all. Which of its two sides the hand is on comes from
    // [`pulls_left`] and [`pulls_top`], for the reason `sides` gives in its own
    // body: two spellings of "is this drag pulling the left edge" is how the
    // end of a gesture comes to disagree with the middle of it.
    let (horizontal, vertical) = sides(edges);
    let x = if horizontal.is_none() {
        pointer.x
    } else if pulls_left(edges) {
        f64::from(wanted.loc.x)
    } else {
        f64::from(wanted.loc.x + wanted.size.w)
    };
    let y = if vertical.is_none() {
        pointer.y
    } else if pulls_top(edges) {
        f64::from(wanted.loc.y)
    } else {
        f64::from(wanted.loc.y + wanted.size.h)
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
        // Both fields now come off the one rectangle, which is the whole of
        // #124: the layout used to be handed `event.location` while `wanted`
        // -- the relative answer, computed a line above it -- went to the
        // floating path alone. See [`dragged_edge`].
        let wanted = self.resized(event.location);
        data.pending_resize = Some(crate::state::ResizeRequest {
            window: self.window.clone(),
            wanted,
            edge_at: dragged_edge(wanted, self.edges, event.location),
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
    /// payload written out: the rectangle the gesture has produced, and the
    /// edge read off it that the layout is given. That is the whole of what
    /// changed — `drag_seam` is untouched — so it is the whole of what has to
    /// be pinned on this side.
    ///
    /// **All six tests below fail against `bf80265`**, where the payload was
    /// `(event.location.x, event.location.y)`. Confirmed rather than assumed,
    /// by replacing [`dragged_edge`]'s body with exactly that expression and
    /// running them: six failures, and six passes again on the restore. The
    /// margins are 3px where a border was grabbed three pixels inside its band,
    /// 50px on each axis for a modifier drag begun a quarter of the way into a
    /// window, 5px for the pass-through case, and 520px where the
    /// minimum-size floor is what the pointer ran past.
    ///
    /// Even [`an_axis_the_drag_does_not_move_passes_the_pointer_through`] fails
    /// there, and that is worth writing down because it is the one test whose
    /// *subject* did not change. Only its first assertion holds on the revert:
    /// the drag's own axis moved as well, and that axis is the broken half. A
    /// test is not a witness for whichever assertion was in mind while writing
    /// it.
    ///
    /// The numbers are integers widened to `f64` — [`dragged_edge`] reads
    /// `i32` corners off `wanted` — or the pointer passed through untouched, so
    /// these compare exactly rather than within a tolerance. A tolerance here
    /// would be a place for a rounding to hide.
    mod dragged_edge_tests {
        use super::*;

        /// One frame of a drag, as `ResizeGrab::motion` assembles it: the
        /// rectangle for this pointer position, and the edge handed to the
        /// layout off that rectangle.
        fn handed(
            began: Rectangle<i32, Logical>,
            edges: ResizeEdge,
            from: Point<f64, Logical>,
            now: Point<f64, Logical>,
        ) -> (f64, f64) {
            dragged_edge(resized(began, edges, from, now), edges, now)
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

        /// The edge handed over is already floored, and that floor is not
        /// `drag_seam`'s.
        ///
        /// [`resized`] clamps the *size* to [`MINIMUM`] pixels, so a left edge
        /// shoved past the right one comes back at 380 — the right edge at 500
        /// less the minimum — rather than at the pointer's 900. `drag_seam`
        /// then clamps a *ratio* to 0.05..0.95 (#115), which is a different
        /// quantity measured against a different rectangle: the seam's box is
        /// not the window, so neither floor can be computed from the other and
        /// whichever is tighter for a given arrangement wins. Both exist; this
        /// pins the one on this side of the call.
        #[test]
        fn the_edge_handed_over_is_already_floored_at_the_minimum() {
            let grab: Point<f64, Logical> = (104.0, 250.0).into();
            let sent = handed(window(), ResizeEdge::Left, grab, (900.0, 250.0).into());
            assert_eq!(
                sent.0,
                f64::from(100 + 400 - MINIMUM),
                "the floored left edge, not the pointer"
            );
        }
    }
}
