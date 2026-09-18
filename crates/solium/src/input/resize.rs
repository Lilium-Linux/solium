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
    let horizontal = match edges {
        ResizeEdge::Left | ResizeEdge::TopLeft | ResizeEdge::BottomLeft => Some("left"),
        ResizeEdge::Right | ResizeEdge::TopRight | ResizeEdge::BottomRight => Some("right"),
        _ => None,
    };
    let vertical = match edges {
        ResizeEdge::Top | ResizeEdge::TopLeft | ResizeEdge::TopRight => Some("top"),
        ResizeEdge::Bottom | ResizeEdge::BottomLeft | ResizeEdge::BottomRight => Some("bottom"),
        _ => None,
    };
    (horizontal, vertical)
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
        data.pending_resize = Some(crate::state::ResizeRequest {
            window: self.window.clone(),
            wanted: self.resized(event.location),
            at: (event.location.x, event.location.y),
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
}
