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
//! **The configuration hears every move drag and may steer it** (#452). Each
//! one is told to the `drag` listeners as it begins, moves and ends
//! ([`settle_drags`]), and `sol.drag_hold` and `sol.drag_grip` hold its window
//! still while the pointer goes on, and say where the pointer holds it. That
//! is all the compositor does: what a drag of a maximised window does --
//! nothing for a few pixels, then restored under the pointer -- is
//! `lua/floating.lua`'s.

use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, CursorIcon, Focus, GestureHoldBeginEvent, GestureHoldEndEvent,
        GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
        GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData,
        MotionEvent, PointerGrab, PointerHandle, PointerInnerHandle, RelativeMotionEvent,
    },
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Serial},
};

use crate::state::Solium;

/// How far a move drag has got, as the `drag` listeners hear it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Begin,
    Move,
    End,
}

impl Phase {
    /// The name a listener is handed.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Begin => "begin",
            Self::Move => "move",
            Self::End => "end",
        }
    }
}

/// One move drag under way, the pointer's or a finger's: what the
/// configuration can change about it. Kept on `Solium`, not in the grab,
/// because a command reaches it from a script, and smithay hands out its
/// grab only to be read.
#[derive(Clone, Debug)]
pub(crate) struct Moving {
    drag: u64,
    window: Window,
    /// From the pointer to the window's origin: the window is drawn at the
    /// pointer plus this.
    offset: Point<f64, Logical>,
    /// Where the pointer, or the finger, is now.
    at: Point<f64, Logical>,
    /// Whether `sol.drag_hold` holds the window where it is.
    held: bool,
    /// Let go of, and `end` not told yet: whether it is to be dropped. It is
    /// kept until then, so a motion told after the release -- the pointer let
    /// go within a frame of its last move -- can still hold or grip it, and
    /// whether it is dropped is decided by whether it is held then.
    ending: Option<bool>,
}

/// Which kind of drag a listener is told about, and so which event it hears.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gesture {
    /// A move drag: the `drag` event.
    Move,
    /// An edge or modifier resize (#454): the `resizing` event, told as it
    /// begins and ends. Its motion is the `resize` event's, offered to the
    /// layouts once a pass (`Solium::settle_resize`), so it has no `move`.
    Resize,
}

/// What a `drag` or `resizing` listener is told, waiting to be told. See
/// [`settle_drags`].
#[derive(Clone, Debug)]
pub(crate) struct Told {
    gesture: Gesture,
    window: Window,
    phase: Phase,
    at: Point<f64, Logical>,
}

/// Note that `window`'s drag reached `phase` at `at`, to be told once the
/// grab has returned. A move replaces a move of the same window still
/// waiting, so a pass tells one, wherever the pointer got to.
pub(crate) fn tell(
    state: &mut Solium,
    gesture: Gesture,
    window: &Window,
    phase: Phase,
    at: Point<f64, Logical>,
) {
    if phase == Phase::Move
        && let Some(last) = state.drags_told.last_mut()
        && last.phase == Phase::Move
        && last.gesture == gesture
        && last.window == *window
    {
        last.at = at;
        return;
    }
    state.drags_told.push(Told {
        gesture,
        window: window.clone(),
        phase,
        at,
    });
}

/// Tell the `drag` listeners what move drags have done since they were last
/// told: `(id, phase, x, y)`, `phase` being `"begin"`, `"move"` or `"end"`,
/// and `x, y` the pointer; and the `resizing` listeners, in the same order
/// and on the same terms, the `"begin"` and `"end"` of each resize (#454).
///
/// **Never from inside a grab's callback**, which holds the seat's lock, as
/// `Solium::pending_drop` says: a listener asking where the pointer is would
/// wait for ever. So the grab records, and this tells:
///
/// * `edges` true, after each input event and before a drop is reported:
///   everything up to the last `begin` or `end` waiting, so a listener holds
///   a window on `begin` before the first motion can move it, and hears `end`
///   before `drop`. A `move` after them waits.
/// * `edges` false, once a pass, before the frame is decided: everything,
///   one `move` a window at most, however many times the mouse reported it,
///   as `settle_resize` coalesces an edge drag.
///
/// A resize's `end` waits while its last motion is still to be offered to
/// the `resize` listeners, and is told once it has been.
///
/// A drag is over once its `end` is told, and dropped then unless a script
/// holds the window: a release within the pass of the last motion has that
/// motion told first, and what a listener does with it -- a grip, a hold
/// let go -- counts.
///
/// `state::tests::real_client::reflow_on_close::drag_primitives::a_drag_is_told_as_it_begins_moves_once_a_pass_and_ends`,
/// `state::tests::real_client::reflow_on_close::drag_primitives::a_grip_puts_the_window_under_the_pointer_and_the_drag_goes_on_from_it`.
pub(crate) fn settle_drags(state: &mut Solium, edges: bool) {
    let upto = if edges {
        match state
            .drags_told
            .iter()
            .rposition(|told| told.phase != Phase::Move)
        {
            Some(last) => last + 1,
            None => return,
        }
    } else {
        state.drags_told.len()
    };
    // A resize's `end` waits for the motion still to be offered to the
    // `resize` listeners, so they hear the last of it before it is over: the
    // pass that offers it tells the rest (`Solium::settle_resize`).
    let upto = state.drags_told[..upto]
        .iter()
        .position(|told| {
            told.gesture == Gesture::Resize
                && told.phase == Phase::End
                && state
                    .pending_resize
                    .as_ref()
                    .is_some_and(|request| request.window == told.window)
        })
        .unwrap_or(upto);
    let told: Vec<Told> = state.drags_told.drain(..upto).collect();
    for Told {
        gesture,
        window,
        phase,
        at,
    } in told
    {
        state.trigger_drag(gesture, &window, phase.name(), at.x, at.y);
        // A resize is over, and forgotten, as its grab ends
        // (`crate::input::resize::ResizeDrag::finish`); only a move drag is
        // kept until its `end` is told, for its drop.
        if phase != Phase::End || gesture != Gesture::Move {
            continue;
        }
        // Over now, and dropped unless a script holds it: a held window
        // has not been moved by the drag, so it was dropped nowhere.
        let Some(index) = state
            .moves
            .iter()
            .position(|moving| moving.window == window && moving.ending.is_some())
        else {
            continue;
        };
        let moving = state.moves.remove(index);
        if moving.ending == Some(true) && !moving.held {
            state.pending_drop = Some((window, at.x, at.y));
        }
    }
}

/// A script holding a dragged window where it is, or letting it go on
/// (`sol.drag_hold`). Let go, it moves on from where it stands, so nothing
/// jumps.
/// `state::tests::real_client::reflow_on_close::drag_primitives::a_held_window_stays_while_the_pointer_moves_and_goes_on_from_where_it_is`.
pub(crate) fn hold(state: &mut Solium, window: &Window, held: bool) {
    let Some(location) = state.space.element_location(window) else {
        return;
    };
    for moving in state
        .moves
        .iter_mut()
        .filter(|moving| moving.window == *window)
    {
        if moving.held && !held {
            moving.offset = location.to_f64() - moving.at;
        }
        moving.held = held;
    }
}

/// Where a script has the pointer hold a dragged window (`sol.drag_grip`):
/// `along` of its frame's width and `down` of its height, and `by` pixels
/// further, from the frame's top-left corner, held inside the frame. The
/// window is put there at once, a glide it is part of the way through is
/// aimed there, and the drag goes on from there.
///
/// The frame's size is the size the window was last asked for while its
/// client has not drawn it (`Solium::asked_outer`), and the one it has drawn
/// otherwise: a window just restored has been told its new size and has not
/// drawn it yet.
/// `state::tests::real_client::reflow_on_close::drag_primitives::a_grip_puts_the_window_under_the_pointer_and_the_drag_goes_on_from_it`.
pub(crate) fn grip(state: &mut Solium, window: &Window, along: (f64, f64), by: (f64, f64)) {
    let Some(at) = state
        .moves
        .iter()
        .find(|moving| moving.window == *window)
        .map(|moving| moving.at)
    else {
        return;
    };
    let Some(pane) = state.panes.of(window) else {
        return;
    };
    let id = pane.id();
    let outer = state.asked_outer(pane);
    let insets = state.insets_of(id);
    let width = f64::from(outer.size.w);
    let height = f64::from(outer.size.h);
    let x = (along.0 * width + by.0).clamp(0.0, width);
    let y = (along.1 * height + by.1).clamp(0.0, (height - 1.0).max(0.0));
    let corner = Point::<f64, Logical>::from((at.x - x, at.y - y)).to_i32_round();
    let location = corner + Point::from((insets.left, insets.top));
    state.space.map_element(window.clone(), location, false);
    if let Some(pane) = state.panes.get(id) {
        crate::present::reaim(pane, Rectangle::new(corner, outer.size), state.clock.now());
    }
    for moving in state
        .moves
        .iter_mut()
        .filter(|moving| moving.window == *window)
    {
        moving.offset = location.to_f64() - moving.at;
    }
    state.redraw = true;
}

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

/// One move drag, shared by the pointer and each independently held finger:
/// its window, and which of `Solium::moves` it is.
#[derive(Clone, Debug)]
pub(crate) struct MoveDrag {
    window: Window,
    drag: u64,
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
        let grab = Self::new(state, start_data, window, window_location);
        pointer.set_grab(state, grab, serial, Focus::Clear);
    }

    fn new(
        state: &mut Solium,
        start_data: GrabStartData<Solium>,
        window: Window,
        window_location: Point<i32, Logical>,
    ) -> Self {
        let drag = MoveDrag::new(state, window, window_location, start_data.location);
        Self { start_data, drag }
    }
}

impl MoveDrag {
    /// The window being dragged.
    pub(crate) const fn window(&self) -> &Window {
        &self.window
    }

    /// A drag of `window`, standing at `window_location`, taken at `from`:
    /// kept on `Solium::moves`, and its `begin` told once the grab has
    /// returned.
    pub(crate) fn new(
        state: &mut Solium,
        window: Window,
        window_location: Point<i32, Logical>,
        from: Point<f64, Logical>,
    ) -> Self {
        state.next_drag += 1;
        let drag = state.next_drag;
        state.moves.push(Moving {
            drag,
            window: window.clone(),
            // Without it the window jumps so its corner meets the cursor on
            // the first motion, which reads as the compositor dropping it.
            offset: window_location.to_f64() - from,
            at: from,
            held: false,
            ending: None,
        });
        tell(state, Gesture::Move, &window, Phase::Begin, from);
        Self { window, drag }
    }

    /// The pointer, or the finger, is at `at`: the window follows it, unless
    /// a script holds it (`sol.drag_hold`).
    pub(crate) fn to(&self, state: &mut Solium, at: Point<f64, Logical>) {
        let Some(moving) = state
            .moves
            .iter_mut()
            .find(|moving| moving.drag == self.drag && moving.ending.is_none())
        else {
            return;
        };
        moving.at = at;
        let (held, offset) = (moving.held, moving.offset);
        tell(state, Gesture::Move, &self.window, Phase::Move, at);
        if held {
            return;
        }
        let location = (at + offset).to_i32_round();
        let was = state.space.element_location(&self.window);
        // Keep the stacking order alone: dragging does not restack.
        state
            .space
            .map_element(self.window.clone(), location, false);
        // A glide the window is part of the way through goes with it, so it
        // is drawn under the pointer while it lands: a restore's shrink, a
        // layout's or an opening's glide (#452).
        if let Some(was) = was
            && was != location
            && let Some(pane) = state.panes.of(&self.window)
        {
            crate::present::carry(pane, (location - was).to_f64());
        }
    }

    /// Let go at `at`: `end` told once the grab has returned, and then the
    /// drop ([`settle_drags`]).
    pub(crate) fn finish(&self, state: &mut Solium, at: Point<f64, Logical>) {
        self.ending(state, at, true);
    }

    /// Over without a drop, as a drag let go of is (`let_go_of`), or a
    /// finger cancelled.
    pub(crate) fn end(&self, state: &mut Solium, at: Point<f64, Logical>) {
        self.ending(state, at, false);
    }

    fn ending(&self, state: &mut Solium, at: Point<f64, Logical>, drop: bool) {
        let Some(moving) = state
            .moves
            .iter_mut()
            .find(|moving| moving.drag == self.drag && moving.ending.is_none())
        else {
            return;
        };
        moving.ending = Some(drop);
        moving.at = at;
        tell(state, Gesture::Move, &self.window, Phase::End, at);
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
        // Ended without a release -- `let_go_of`, or another grab taking the
        // pointer -- the drag is over all the same. After a release it is
        // over already, and this does nothing.
        let at = data
            .moves
            .iter()
            .find(|moving| moving.drag == self.drag.drag && moving.ending.is_none())
            .map(|moving| moving.at);
        if let Some(at) = at {
            self.drag.end(data, at);
        }
        data.redraw = true;
    }
}
