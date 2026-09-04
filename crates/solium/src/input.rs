//! Input wiring, deliberately minimal.
//!
//! Enough that a nested terminal can be clicked and typed into. #12 replaces
//! this with an input profile — pointer, touch and gesture handling selected
//! per form factor — and this module's single entry point is that seam, so the
//! backend never learns what kind of device it is feeding.

use smithay::{
    backend::{
        input::{
            AbsolutePositionEvent, ButtonState, Event, InputEvent, KeyboardKeyEvent,
            PointerButtonEvent,
        },
        winit::WinitInput,
    },
    input::{
        keyboard::FilterResult,
        pointer::{ButtonEvent, MotionEvent},
    },
    output::Output,
    utils::SERIAL_COUNTER,
};

use crate::state::Solium;

/// Route one backend event to the seat.
pub(crate) fn handle(state: &mut Solium, output: &Output, event: InputEvent<WinitInput>) {
    match event {
        InputEvent::Keyboard { event } => {
            let Some(keyboard) = state.seat.get_keyboard() else {
                return;
            };
            keyboard.input::<(), _>(
                state,
                event.key_code(),
                event.state(),
                SERIAL_COUNTER.next_serial(),
                event.time_msec(),
                // Nothing is intercepted yet: every key goes to the focused
                // client. Compositor bindings land with the script surface in
                // #16, which is the right place to decide what to swallow.
                |_, _, _| FilterResult::Forward,
            );
        }

        InputEvent::PointerMotionAbsolute { event } => {
            let Some(pointer) = state.seat.get_pointer() else {
                return;
            };
            // The winit backend reports position normalised to the window, so
            // it is scaled by the output mode rather than used directly.
            let size = output
                .current_mode()
                .map(|mode| mode.size)
                .unwrap_or_default();
            let location = (event.x_transformed(size.w), event.y_transformed(size.h)).into();
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

        InputEvent::PointerButton { event } => {
            let Some(pointer) = state.seat.get_pointer() else {
                return;
            };
            let serial = SERIAL_COUNTER.next_serial();
            let button_state = event.state();

            // Click to focus. Skipped while a grab is active, or a popup would
            // dismiss itself and hand focus away on the click that opened it.
            if button_state == ButtonState::Pressed && !pointer.is_grabbed() {
                let location = pointer.current_location();
                let focus = state.surface_under(location).map(|(surface, _)| surface);
                if let Some(keyboard) = state.seat.get_keyboard() {
                    keyboard.set_focus(state, focus, serial);
                }
            }

            pointer.button(
                state,
                &ButtonEvent {
                    button: event.button_code(),
                    state: button_state,
                    serial,
                    time: event.time_msec(),
                },
            );
            pointer.frame(state);
        }

        // Scroll, touch and gestures arrive with the input profile in #12.
        _ => {}
    }
}
