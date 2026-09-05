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
            AbsolutePositionEvent, Axis, AxisSource, ButtonState, InputEvent, KeyState,
            KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, TouchDownEvent,
            TouchMotionEvent as TouchMotionEventTrait, TouchUpEvent,
        },
        winit::WinitInput,
    },
    input::{
        keyboard::{FilterResult, Keysym, ModifiersState, xkb},
        pointer::{AxisFrame, ButtonEvent, Focus, GrabStartData, MotionEvent},
        touch::{DownEvent, MotionEvent as TouchMotionEvent, UpEvent},
    },
    output::Output,
    utils::{Logical, Point, SERIAL_COUNTER},
};

use crate::{decoration::Decoration, script, state::Solium};

use grab::MoveGrab;

/// A key combination a script has claimed.
///
/// Carried out of the keyboard filter rather than acted on inside it: the
/// filter runs while the seat holds its own lock, and a script that focused a
/// window from in there would re-enter the keyboard and deadlock.
#[derive(Clone, Debug)]
struct Bound(String);

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

    let pressed = event.state() == KeyState::Pressed;

    let bound = keyboard.input(
        state,
        event.key_code(),
        event.state(),
        SERIAL_COUNTER.next_serial(),
        event.time_msec(),
        |state, modifiers, handle| {
            if !pressed {
                // Releases are never bindings, but they must still reach a
                // client that received the press, or it holds the key forever.
                return if state.script_grab {
                    FilterResult::Intercept(None)
                } else {
                    FilterResult::Forward
                };
            }

            let combo = combo_for(modifiers, handle.modified_sym());
            let claimed = state
                .scripts
                .as_ref()
                .is_some_and(|scripts| scripts.has_binding(&combo));

            // Logged for every press, because "my binding does nothing" has two
            // very different causes and they are indistinguishable without it:
            // either the key never arrived — the host compositor kept it — or it
            // arrived under a name no script bound.
            tracing::debug!(combo, claimed, "key");

            if claimed {
                FilterResult::Intercept(Some(Bound(combo)))
            } else if state.script_grab {
                // A mode owns input: keys it did not bind are swallowed rather
                // than leaking to whatever is underneath it.
                FilterResult::Intercept(None)
            } else {
                FilterResult::Forward
            }
        },
    );

    if let Some(Some(Bound(combo))) = bound {
        state.trigger(&combo);
    }
}

/// The canonical name of a key combination, as scripts bind them.
fn combo_for(modifiers: &ModifiersState, keysym: Keysym) -> String {
    let mut combo = String::new();
    for (held, name) in [
        (modifiers.ctrl, "ctrl"),
        (modifiers.alt, "alt"),
        (modifiers.shift, "shift"),
        (modifiers.logo, "super"),
    ] {
        if held {
            combo.push_str(name);
            combo.push('+');
        }
    }
    combo.push_str(&xkb::keysym_get_name(keysym));
    script::normalise_combo(&combo)
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

    // Frames see the pointer before clients do, so buttons light up on hover.
    // Motion is *also* forwarded below, because the pointer leaving a window
    // has to reach it or the window keeps a stale hover state.
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

    // While a mode owns input, every press is the mode's: it decides what a
    // click on a thumbnail means, and nothing underneath should see it.
    if state.script_grab {
        if button_state == ButtonState::Pressed {
            state.trigger_click(location.x, location.y);
        }
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
