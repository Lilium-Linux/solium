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
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
        GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent,
        GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerGrab,
        PointerInnerHandle, RelativeMotionEvent,
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
    let pulls_left = matches!(
        edges,
        ResizeEdge::Left | ResizeEdge::TopLeft | ResizeEdge::BottomLeft
    );
    let pulls_right = matches!(
        edges,
        ResizeEdge::Right | ResizeEdge::TopRight | ResizeEdge::BottomRight
    );
    let pulls_top = matches!(
        edges,
        ResizeEdge::Top | ResizeEdge::TopLeft | ResizeEdge::TopRight
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
    if pulls_left {
        rect.size.w = (began.size.w - dx).max(MINIMUM);
        rect.loc.x = began.loc.x + began.size.w - rect.size.w;
    }
    if pulls_top {
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
        data.pending_resize = Some(crate::state::ResizeRequest {
            window: self.window.clone(),
            wanted: self.resized(event.location),
            at: (event.location.x, event.location.y),
            horizontal: matches!(
                self.edges,
                ResizeEdge::Left
                    | ResizeEdge::Right
                    | ResizeEdge::TopLeft
                    | ResizeEdge::TopRight
                    | ResizeEdge::BottomLeft
                    | ResizeEdge::BottomRight
            ),
            vertical: matches!(
                self.edges,
                ResizeEdge::Top
                    | ResizeEdge::Bottom
                    | ResizeEdge::TopLeft
                    | ResizeEdge::TopRight
                    | ResizeEdge::BottomLeft
                    | ResizeEdge::BottomRight
            ),
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

    fn unset(&mut self, _data: &mut Solium) {}
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
