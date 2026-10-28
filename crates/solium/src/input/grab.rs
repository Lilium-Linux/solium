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
//! A drag holds its window only while the window is the pointer's to place. A
//! window made fullscreen or maximised mid-drag is the compositor's again, and
//! [`let_go_of`] ends the drag there: a fullscreen window kept following the
//! pointer and covered its monitor at an offset (#404).

use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
        GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent,
        GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerGrab,
        PointerInnerHandle, RelativeMotionEvent,
    },
    utils::{Logical, Point, SERIAL_COUNTER},
};

use crate::state::Solium;

/// End every move drag holding `window`, the pointer's and each finger's,
/// without a drop: the window has stopped being the drag's to place.
///
/// For a window going fullscreen or maximised, which covers a rectangle of
/// the compositor's choosing; a drag that went on moving it put it there at an
/// offset, with the desk showing along two edges (#404). No drop, because the
/// window was not let go of anywhere: a layout asked where it landed would
/// move it in its tree while it covers the monitor. The button still held is
/// nobody's once the grab has gone, and its release reaches whatever is under
/// the pointer.
///
/// Never from inside a pointer grab's own callback, which holds the seat's
/// lock: a key, a client's request or a script asks for fullscreen, never the
/// pointer's own events.
/// `state::tests::real_client::fullscreen_glides::a_window_made_fullscreen_mid_drag_covers_its_monitor_and_the_drag_ends`.
pub(crate) fn let_go_of(state: &mut Solium, window: &Window) {
    if let Some(pointer) = state.seat.get_pointer()
        && pointer
            .with_grab(|_, grab| {
                grab.downcast_ref::<MoveGrab>()
                    .is_some_and(|grab| grab.drag.window == *window)
            })
            .unwrap_or(false)
    {
        let time = u32::try_from(state.clock.now().as_millis()).unwrap_or(u32::MAX);
        pointer.unset_grab(state, SERIAL_COUNTER.next_serial(), time);
    }
    super::touch::let_go_of(state, window);
}

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
    pub(crate) fn new(
        start_data: GrabStartData<Solium>,
        window: Window,
        window_location: Point<i32, Logical>,
    ) -> Self {
        let drag = MoveDrag::new(window, window_location, start_data.location);
        Self { start_data, drag }
    }
}

impl MoveDrag {
    /// The window being dragged.
    pub(crate) const fn window(&self) -> &Window {
        &self.window
    }

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

    fn unset(&mut self, _data: &mut Solium) {}
}
