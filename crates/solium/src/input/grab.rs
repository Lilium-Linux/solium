//! Interactive window move.
//!
//! A pointer grab, because that is what an interactive drag is: for as long as
//! the button is held, every pointer event belongs to the drag and not to
//! whatever surface happens to be under the cursor. Smithay models that
//! directly, so this is mostly bookkeeping plus one line of intent.
//!
//! Reached two ways, and both matter:
//!
//! * `xdg_toplevel.move`, which client-side decorations send when their own
//!   titlebar is dragged. Handling it is what makes a CSD window movable
//!   without the compositor drawing anything at all.
//! * The drag modifier from the input profile, for windows with no titlebar to
//!   reach for.
//!
//! And a third, the compositor's own frame's titlebar. Every one starts
//! through [`MoveGrab::start`], which shows the window held ([`SHAPE`]) for as
//! long as the drag lasts (#374).

use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, CursorIcon, Focus, GestureHoldBeginEvent, GestureHoldEndEvent,
        GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
        GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData,
        MotionEvent, PointerGrab, PointerHandle, PointerInnerHandle, RelativeMotionEvent,
    },
    utils::{Logical, Point, Serial},
};

use crate::state::Solium;

/// What the pointer shows for as long as a window is being moved (#374):
/// the closed hand, `grabbing`, because the window is held. `move`, the
/// four-way arrow, says that something *can* be moved, which is a promise
/// made before the press; once the button is down and the window follows the
/// hand, every toolkit shows the hand closed on it. A resize shows the edge's
/// double arrow for the same length of time (`resize::cursor`).
pub(crate) const SHAPE: CursorIcon = CursorIcon::Grabbing;

/// Drags a window with the pointer until the button is released.
pub(crate) struct MoveGrab {
    start_data: GrabStartData<Solium>,
    drag: MoveDrag,
}

/// Position maths shared by the pointer and each independently held finger.
#[derive(Clone, Debug)]
pub(crate) struct MoveDrag {
    window: Window,
    /// Pointer-to-window-origin offset, captured when the grab starts.
    ///
    /// Without it the window jumps so its corner meets the cursor on the first
    /// motion event, which reads as the compositor dropping the window.
    offset: Point<f64, Logical>,
}

impl MoveGrab {
    /// Move `window` with `pointer` until the button that started it goes
    /// up, the pointer showing it held ([`SHAPE`]) meanwhile. The shape is
    /// asserted as the compositor's own over its chrome is, and held for the
    /// grab as a resize's is (`Solium::assert_cursor` says nothing while the
    /// pointer is grabbed); the first frame after the grab ends asks again
    /// what is under the pointer.
    /// `state::tests::real_client::reflow_on_close::hosted::a_titlebar_move_shows_the_closed_hand_until_it_ends`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_modifier_move_shows_the_closed_hand_until_it_ends`.
    pub(crate) fn start(
        state: &mut Solium,
        pointer: &PointerHandle<Solium>,
        start_data: GrabStartData<Solium>,
        window: Window,
        window_location: Point<i32, Logical>,
        serial: Serial,
    ) {
        if state.pointer.assert(Some(SHAPE)) {
            state.redraw = true;
        }
        pointer.set_grab(
            state,
            Self::new(start_data, window, window_location),
            serial,
            Focus::Clear,
        );
    }

    fn new(
        start_data: GrabStartData<Solium>,
        window: Window,
        window_location: Point<i32, Logical>,
    ) -> Self {
        let drag = MoveDrag::new(window, window_location, start_data.location);
        Self { start_data, drag }
    }
}

impl MoveDrag {
    pub(crate) fn new(
        window: Window,
        window_location: Point<i32, Logical>,
        from: Point<f64, Logical>,
    ) -> Self {
        Self {
            window,
            offset: window_location.to_f64() - from,
        }
    }

    pub(crate) fn to(&self, state: &mut Solium, at: Point<f64, Logical>) {
        let location = (at + self.offset).to_i32_round();
        // Keep the stacking order alone: dragging does not restack.
        state
            .space
            .map_element(self.window.clone(), location, false);
    }

    pub(crate) fn finish(&self, state: &mut Solium, at: Point<f64, Logical>) {
        state.pending_drop = Some((self.window.clone(), at.x, at.y));
    }
}

impl PointerGrab<Solium> for MoveGrab {
    fn motion(
        &mut self,
        data: &mut Solium,
        handle: &mut PointerInnerHandle<'_, Solium>,
        _focus: Option<(
            smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
            Point<f64, Logical>,
        )>,
        event: &MotionEvent,
    ) {
        // Focus is deliberately dropped for the duration: while dragging, no
        // client should be receiving enter/leave for surfaces sliding under the
        // cursor.
        handle.motion(data, None, event);

        self.drag.to(data, event.location);
    }

    fn relative_motion(
        &mut self,
        data: &mut Solium,
        handle: &mut PointerInnerHandle<'_, Solium>,
        _focus: Option<(
            smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
            Point<f64, Logical>,
        )>,
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
        // Ends when the button that started it is no longer held — not on any
        // release, or a second button going up would drop the window.
        let started_with = self.start_data.button;
        if !handle.current_pressed().contains(&started_with) {
            // Where it was let go, recorded rather than reported. Calling a
            // script from in here would ask the seat where the pointer is,
            // while the seat is already inside this call holding that lock —
            // and the compositor would stop, holding the screen. `input::handle`
            // picks this up once the grab has returned.
            let at = handle.current_location();
            self.drag.finish(data, at);
            // `true` restores focus to whatever is under the cursor now: the
            // window was dragged out from under the pointer, and leaving focus
            // where the drag started would strand it.
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

    /// The drag is over: the next frame asks again what is under the
    /// pointer (`Solium::reassert_cursor`), which takes [`SHAPE`] away.
    /// Asked then rather than here, where the seat is held: see `button`.
    fn unset(&mut self, data: &mut Solium) {
        data.redraw = true;
    }
}
