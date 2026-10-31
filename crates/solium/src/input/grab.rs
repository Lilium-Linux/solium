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
//!
//! A drag holds its window only while the window is the pointer's to place. A
//! window made fullscreen or maximised mid-drag is the compositor's again, and
//! [`let_go_of`] ends the drag there: a fullscreen window kept following the
//! pointer and covered its monitor at an offset (#404).
//!
//! **A maximised window does not move** (#452). A drag taken of one holds it
//! where it is until the pointer has gone `maximize.restore_after` pixels,
//! so a click or a double click on its titlebar is still one; past that, it
//! is restored to the size it had before the maximise, put so the point it
//! was taken by is under the pointer, and the drag goes on moving it. It was
//! moved at its maximised size while it stayed maximised, half off its
//! monitor, and jumped back when it was restored. The restore is made after
//! the grab's callback has returned, as a drop is ([`Unmaximize`]).

use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, CursorIcon, Focus, GestureHoldBeginEvent, GestureHoldEndEvent,
        GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
        GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData,
        MotionEvent, PointerGrab, PointerHandle, PointerInnerHandle, RelativeMotionEvent,
    },
    utils::{Logical, Point, SERIAL_COUNTER, Serial},
};

use crate::state::{Solium, States, window_states};

/// What the pointer shows for as long as a window is being moved (#374):
/// the closed hand, `grabbing`, because the window is held. `move`, the
/// four-way arrow, says that something *can* be moved, which is a promise
/// made before the press; once the button is down and the window follows the
/// hand, every toolkit shows the hand closed on it. A resize shows the edge's
/// double arrow for the same length of time (`resize::cursor`).
pub(crate) const SHAPE: CursorIcon = CursorIcon::Grabbing;

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
    /// Where the drag was taken, which is the point on a maximised window
    /// that stays under the pointer when it is restored.
    from: Point<f64, Logical>,
    held: Held,
}

/// Whether a drag moves its window yet (#452).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Held {
    /// It does: the window follows the pointer by `MoveDrag::offset`.
    Moving,
    /// The window is maximised, and stays where it is until the pointer has
    /// gone `maximize.restore_after` from where the drag was taken.
    Maximized,
    /// The pointer has gone that far at this point, and the restore is
    /// recorded ([`Unmaximize`]) and not made yet.
    Restoring(Point<f64, Logical>),
}

/// A maximised window a drag has pulled far enough to come off (#452):
/// restored, with the point the drag was taken by, `from`, under the pointer
/// at `at`.
///
/// **Recorded inside the grab and made after it**, as `Solium::pending_drop`
/// is and for its reason: a restore tells the scripts, and a script asking
/// where the pointer is from inside the pointer's own grab waits on a lock the
/// grab holds. [`settle`] makes it, once the motion has gone through.
#[derive(Clone, Debug)]
pub(crate) struct Unmaximize {
    window: Window,
    from: Point<f64, Logical>,
    at: Point<f64, Logical>,
}

/// Make the restore a drag recorded ([`Unmaximize`]), if there is one: after
/// each pointer motion and finger motion, outside the grab, and before a
/// release reports its drop. The drag holding the window reads where it was
/// put on its next motion, and moves it on from there.
/// `state::tests::real_client::client_maximize::a_maximised_window_dragged_past_the_threshold_comes_off_under_the_pointer`.
pub(crate) fn settle(state: &mut Solium) {
    if let Some(Unmaximize { window, from, at }) = state.pending_unmaximize.take() {
        state.restore_under(&window, from, at);
    }
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
    /// The window being dragged.
    pub(crate) const fn window(&self) -> &Window {
        &self.window
    }

    /// A drag of `window`, standing at `window_location`, taken at `from`.
    /// A maximised window is held where it is to begin with (#452); a
    /// fullscreen one moves as it always has.
    pub(crate) fn new(
        window: Window,
        window_location: Point<i32, Logical>,
        from: Point<f64, Logical>,
    ) -> Self {
        let States {
            fullscreen,
            maximized,
        } = window_states(&window);
        Self {
            window,
            offset: window_location.to_f64() - from,
            from,
            held: if maximized && !fullscreen {
                Held::Maximized
            } else {
                Held::Moving
            },
        }
    }

    /// The pointer, or the finger, is at `at`: the window follows it, once
    /// it may (#452).
    ///
    /// A maximised window stays where it is until `at` is
    /// `maximize.restore_after` from where the drag was taken, and is then
    /// recorded to be restored there ([`Unmaximize`]); the motion after
    /// [`settle`] has restored it moves it on from where it was put. One
    /// restored by something else while it was held -- the double click on
    /// its titlebar that started this drag -- moves on from where it is.
    pub(crate) fn to(&mut self, state: &mut Solium, at: Point<f64, Logical>) {
        match self.held {
            Held::Moving => {}
            Held::Maximized => {
                let States {
                    fullscreen,
                    maximized,
                } = window_states(&self.window);
                if maximized && !fullscreen {
                    let gone = at - self.from;
                    if gone.x.hypot(gone.y) < state.maximize.restore_after {
                        return;
                    }
                    state.pending_unmaximize = Some(Unmaximize {
                        window: self.window.clone(),
                        from: self.from,
                        at,
                    });
                    self.held = Held::Restoring(at);
                    return;
                }
                self.moving_from(state, at);
            }
            Held::Restoring(asked) => {
                if state
                    .pending_unmaximize
                    .as_ref()
                    .is_some_and(|pending| pending.window == self.window)
                {
                    return;
                }
                self.moving_from(state, asked);
            }
        }
        let location = (at + self.offset).to_i32_round();
        let was = state.space.element_location(&self.window);
        // Keep the stacking order alone: dragging does not restack.
        state
            .space
            .map_element(self.window.clone(), location, false);
        // A glide the window is part of the way through goes with it, so
        // it is drawn under the pointer while it lands: the restore's
        // shrink, or a layout's or an opening's glide (#452).
        if let Some(was) = was
            && was != location
            && let Some(pane) = state.panes.of(&self.window)
        {
            crate::present::carry(pane, (location - was).to_f64());
        }
    }

    /// Move the window on from where it stands now, with `at` the pointer
    /// it stands under.
    fn moving_from(&mut self, state: &Solium, at: Point<f64, Logical>) {
        if let Some(location) = state.space.element_location(&self.window) {
            self.offset = location.to_f64() - at;
        }
        self.held = Held::Moving;
    }

    /// Let go at `at`: the drop is reported once the grab has returned. Not
    /// for a maximised window that never came off, which has not moved: a
    /// click on its titlebar is a click, and a layout asked where it was
    /// dropped would move it in its tree while it covers the work area.
    pub(crate) fn finish(&self, state: &mut Solium, at: Point<f64, Logical>) {
        if self.held == Held::Maximized {
            return;
        }
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
