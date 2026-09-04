//! Input routing.
//!
//! One entry point, [`handle`], for every device. What differs between form
//! factors lives in [`profile`], not in the branches here — see that module for
//! why. Compositor key bindings are intercepted before the focused client sees
//! them; everything else is forwarded.

pub(crate) mod grab;
pub(crate) mod profile;

use smithay::{
    backend::{
        input::{
            AbsolutePositionEvent, Axis, AxisSource, ButtonState, InputEvent, KeyboardKeyEvent,
            PointerAxisEvent, PointerButtonEvent, TouchDownEvent,
            TouchMotionEvent as TouchMotionEventTrait, TouchUpEvent,
        },
        winit::WinitInput,
    },
    input::{
        keyboard::{FilterResult, Keysym},
        pointer::{AxisFrame, ButtonEvent, Focus, GrabStartData, MotionEvent},
        touch::{DownEvent, MotionEvent as TouchMotionEvent, UpEvent},
    },
    output::Output,
    utils::{Logical, Point, SERIAL_COUNTER},
};

use crate::{decoration::Decoration, mode, state::Solium};

use grab::MoveGrab;

/// Something a key binding asked the compositor to do.
///
/// Returned from the keyboard filter rather than performed inside it: the
/// filter runs while the keyboard handle holds the compositor state, and
/// reaching for the space or the seat from in there is how you get a
/// re-entrant borrow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    ToggleOverview,
}

/// Route one backend event to the seat.
pub(crate) fn handle(state: &mut Solium, output: &Output, event: InputEvent<WinitInput>) {
    match event {
        InputEvent::Keyboard { event } => keyboard(state, event),
        InputEvent::PointerMotionAbsolute { event } => pointer_motion(state, output, event),
        InputEvent::PointerButton { event } => pointer_button(state, event),
        InputEvent::PointerAxis { event } => pointer_axis(state, event),
        InputEvent::TouchDown { event } => touch_down(state, output, event),
        InputEvent::TouchMotion { event } => touch_motion(state, output, event),
        InputEvent::TouchUp { event } => touch_up(state, event),
        InputEvent::TouchFrame { .. } => {
            if let Some(touch) = state.seat.get_touch() {
                touch.frame(state);
            }
        }
        InputEvent::TouchCancel { .. } => {
            if let Some(touch) = state.seat.get_touch() {
                touch.cancel(state);
            }
        }
        _ => {}
    }
}

fn keyboard(state: &mut Solium, event: impl KeyboardKeyEvent<WinitInput>) {
    let Some(keyboard) = state.seat.get_keyboard() else {
        return;
    };

    let action = keyboard.input(
        state,
        event.key_code(),
        event.state(),
        SERIAL_COUNTER.next_serial(),
        event.time_msec(),
        |_state, modifiers, handle| {
            if handle.modified_sym() == Keysym::space && modifiers.logo {
                // Intercepted, not forwarded: a binding the compositor acts on
                // must not also reach the focused client.
                return FilterResult::Intercept(Action::ToggleOverview);
            }
            FilterResult::Forward
        },
    );

    match action {
        Some(Action::ToggleOverview) => mode::toggle_overview(state),
        None => {}
    }
}

fn pointer_motion(
    state: &mut Solium,
    output: &Output,
    event: impl AbsolutePositionEvent<WinitInput>,
) {
    let Some(pointer) = state.seat.get_pointer() else {
        return;
    };
    let location = absolute_location(output, &event);

    // The bar is offered the event first and swallows it when the pointer is
    // over it. It reserves its height, so nothing below it is under the cursor
    // there — forwarding to a client as well would send it a hover it cannot
    // see the reason for.
    if let Some(bar) = state.bar.as_mut()
        && bar.pointer(location.x, location.y, None)
    {
        return;
    }

    // Frames see the pointer before clients do, so buttons light up on hover.
    // Motion is *also* forwarded to the client, because the pointer leaving a
    // window has to reach it or it keeps a stale hover state.
    if let Some((window, local)) = state.frame_under(location)
        && let Some(id) = state.toplevel_id(&window)
        && let Some(decoration) = state.decorations.get_mut(&id)
    {
        decoration.pointer(local.x, local.y, None);
    }

    let under = state.surface_under(location);

    pointer.motion(
        state,
        under,
        &MotionEvent {
            location,
            serial: SERIAL_COUNTER.next_serial(),
            time: event.time_msec(),
        },
    );
    pointer.frame(state);
}

fn pointer_button(state: &mut Solium, event: impl PointerButtonEvent<WinitInput>) {
    let Some(pointer) = state.seat.get_pointer() else {
        return;
    };
    let serial = SERIAL_COUNTER.next_serial();
    let button = event.button_code();
    let button_state = event.state();
    let location = pointer.current_location();

    if let Some(bar) = state.bar.as_mut()
        && bar.pointer(
            location.x,
            location.y,
            Some(button_state == ButtonState::Pressed),
        )
    {
        return;
    }

    // A press on a frame belongs to the frame: it either hits a button or
    // starts a drag, and either way no client should see it.
    if !pointer.is_grabbed()
        && let Some((window, local)) = state.frame_under(location)
        && let Some(id) = state.toplevel_id(&window)
    {
        let pressed = button_state == ButtonState::Pressed;
        let on_button = state.decorations.get_mut(&id).is_some_and(|decoration| {
            decoration.pointer(local.x, local.y, Some(pressed));
            decoration.on_button()
        });

        // Acted on release, so a press that lands on the wrong button can be
        // dragged off it and abandoned.
        if let Some(action) = state
            .decorations
            .get_mut(&id)
            .and_then(Decoration::take_action)
        {
            state.frame_action(&window, action);
        }

        if pressed {
            state.focus_window(&window, serial);
            if !on_button && let Some(geometry) = state.real_geometry(&window) {
                let start_data = GrabStartData {
                    focus: None,
                    button,
                    location,
                };
                pointer.set_grab(
                    state,
                    MoveGrab::new(start_data, window, geometry.loc),
                    serial,
                    Focus::Clear,
                );
            }
        }
        return;
    }

    if button_state == ButtonState::Pressed && !pointer.is_grabbed() {
        let modifiers = state
            .seat
            .get_keyboard()
            .map(|keyboard| keyboard.modifier_state());
        let dragging = modifiers.is_some_and(|modifiers| state.profile.drag_held(&modifiers));

        if let Some((window, geometry)) = state.window_under(location) {
            if dragging {
                let start_data = GrabStartData {
                    focus: state.surface_under(location),
                    button,
                    location,
                };
                pointer.set_grab(
                    state,
                    MoveGrab::new(start_data, window, geometry.loc),
                    serial,
                    Focus::Clear,
                );
                return;
            }

            if state.profile.click_to_focus {
                state.focus_window(&window, serial);
            }
        }
    }

    pointer.button(
        state,
        &ButtonEvent {
            button,
            state: button_state,
            serial,
            time: event.time_msec(),
        },
    );
    pointer.frame(state);
}

fn pointer_axis(state: &mut Solium, event: impl PointerAxisEvent<WinitInput>) {
    let Some(pointer) = state.seat.get_pointer() else {
        return;
    };

    let direction = if state.profile.natural_scroll {
        -1.0
    } else {
        1.0
    };
    let mut frame = AxisFrame::new(event.time_msec()).source(AxisSource::Wheel);

    for axis in [Axis::Horizontal, Axis::Vertical] {
        let amount = event.amount(axis).unwrap_or_default() * direction;
        let discrete = event.amount_v120(axis).unwrap_or_default() * direction;

        if amount == 0.0 && discrete == 0.0 {
            continue;
        }
        frame = frame.value(axis, amount);
        if discrete != 0.0 {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "v120 steps are small integers by protocol"
            )]
            let steps = discrete as i32;
            frame = frame.v120(axis, steps);
        }
    }

    pointer.axis(state, frame);
    pointer.frame(state);
}

fn touch_down(state: &mut Solium, output: &Output, event: impl TouchDownEvent<WinitInput>) {
    let Some(touch) = state.seat.get_touch() else {
        return;
    };
    let location = absolute_location(output, &event);
    let under = state.surface_under(location);
    let serial = SERIAL_COUNTER.next_serial();

    if state.profile.touch_to_focus
        && let Some((window, _)) = state.window_under(location)
    {
        state.focus_window(&window, serial);
    }

    touch.down(
        state,
        under,
        &DownEvent {
            slot: event.slot(),
            location,
            serial,
            time: event.time_msec(),
        },
    );
}

fn touch_motion(
    state: &mut Solium,
    output: &Output,
    event: impl TouchMotionEventTrait<WinitInput>,
) {
    let Some(touch) = state.seat.get_touch() else {
        return;
    };
    let location = absolute_location(output, &event);
    let under = state.surface_under(location);

    touch.motion(
        state,
        under,
        &TouchMotionEvent {
            slot: event.slot(),
            location,
            time: event.time_msec(),
        },
    );
}

fn touch_up(state: &mut Solium, event: impl TouchUpEvent<WinitInput>) {
    let Some(touch) = state.seat.get_touch() else {
        return;
    };
    touch.up(
        state,
        &UpEvent {
            slot: event.slot(),
            serial: SERIAL_COUNTER.next_serial(),
            time: event.time_msec(),
        },
    );
}

/// Turn a backend's window-relative position into compositor coordinates.
///
/// The winit backend reports positions normalised to its window, so they are
/// scaled by the output mode rather than used directly.
fn absolute_location(
    output: &Output,
    event: &impl AbsolutePositionEvent<WinitInput>,
) -> Point<f64, Logical> {
    let size = output
        .current_mode()
        .map(|mode| mode.size)
        .unwrap_or_default();
    (event.x_transformed(size.w), event.y_transformed(size.h)).into()
}
