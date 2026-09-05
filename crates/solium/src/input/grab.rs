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

use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
        GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent,
        GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerGrab,
        PointerInnerHandle, RelativeMotionEvent,
    },
    utils::{Logical, Point},
};

use crate::state::Solium;

/// Drags a window with the pointer until the button is released.
pub(crate) struct MoveGrab {
    start_data: GrabStartData<Solium>,
    window: Window,
    /// Pointer-to-window-origin offset, captured when the grab starts.
    ///
    /// Without it the window jumps so its corner meets the cursor on the first
    /// motion event, which reads as the compositor dropping the window.
    offset: Point<f64, Logical>,
}

impl MoveGrab {
    pub(crate) fn new(
        start_data: GrabStartData<Solium>,
        window: Window,
        window_location: Point<i32, Logical>,
    ) -> Self {
        let offset = window_location.to_f64() - start_data.location;
        Self {
            start_data,
            window,
            offset,
        }
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

        let location = (event.location + self.offset).to_i32_round();
        // `false` keeps the stacking order alone — a drag should not restack.
        data.space.map_element(self.window.clone(), location, false);
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
            // Where it was let go, before the grab is torn down. A layout is
            // told and decides what that means: snap back, or swap with
            // whatever the cursor is over.
            let at = handle.current_location();
            data.trigger_drop(&self.window, at.x, at.y);
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

    fn unset(&mut self, _data: &mut Solium) {}
}
