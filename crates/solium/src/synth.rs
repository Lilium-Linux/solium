//! Synthetic input, for exercising the input path without a hand on a mouse.
//!
//! `input::handle` is generic over `InputBackend` precisely so that libinput
//! and winit cannot drift apart. This is a third implementation of that trait
//! whose events come from a script rather than from hardware — so a drag can
//! be *run*, through the real grab, the real hit-testing and the real layout
//! scripts, on a machine with nobody sitting at it.
//!
//! This exists because it was missing. Two bugs shipped in the drag path in
//! three commits, and the second one deadlocked a compositor holding the
//! screen. Neither could be reproduced without a physical mouse, and a path
//! that can only be tested by a person is a path that will be tested rarely
//! and late.
//!
//! Deliberately not a general robot: it drives the pointer, because that is
//! what could not be reached. Keys already had `SOLIUM_TRIGGER_AT`.

use smithay::{
    backend::input::{
        ButtonState, Device, DeviceCapability, Event, InputBackend, InputEvent, PointerButtonEvent,
        PointerMotionEvent, UnusedEvent,
    },
    utils::{Logical, Point, Rectangle},
};

use crate::state::Solium;

/// The left mouse button, as the kernel numbers it.
const BTN_LEFT: u32 = 0x110;

/// A backend that no hardware is behind.
#[derive(Debug)]
pub(crate) struct Synthetic;

#[derive(Debug, PartialEq, Eq, Hash)]
pub(crate) struct SynthDevice;

impl Device for SynthDevice {
    fn id(&self) -> String {
        "synthetic".to_owned()
    }
    fn name(&self) -> String {
        "synthetic pointer".to_owned()
    }
    fn has_capability(&self, capability: DeviceCapability) -> bool {
        capability == DeviceCapability::Pointer
    }
    fn usb_id(&self) -> Option<(u32, u32)> {
        None
    }
    fn syspath(&self) -> Option<std::path::PathBuf> {
        None
    }
}

/// A relative motion, the shape a real mouse reports.
#[derive(Debug)]
pub(crate) struct Motion {
    delta: Point<f64, Logical>,
    time: u64,
}

impl Event<Synthetic> for Motion {
    fn time(&self) -> u64 {
        self.time
    }
    fn device(&self) -> SynthDevice {
        SynthDevice
    }
}

impl PointerMotionEvent<Synthetic> for Motion {
    fn delta_x(&self) -> f64 {
        self.delta.x
    }
    fn delta_y(&self) -> f64 {
        self.delta.y
    }
    fn delta_x_unaccel(&self) -> f64 {
        self.delta.x
    }
    fn delta_y_unaccel(&self) -> f64 {
        self.delta.y
    }
}

#[derive(Debug)]
pub(crate) struct Button {
    button: u32,
    state: ButtonState,
    time: u64,
}

impl Event<Synthetic> for Button {
    fn time(&self) -> u64 {
        self.time
    }
    fn device(&self) -> SynthDevice {
        SynthDevice
    }
}

impl PointerButtonEvent<Synthetic> for Button {
    fn button_code(&self) -> u32 {
        self.button
    }
    fn state(&self) -> ButtonState {
        self.state
    }
}

impl InputBackend for Synthetic {
    type Device = SynthDevice;
    type KeyboardKeyEvent = UnusedEvent;
    type PointerAxisEvent = UnusedEvent;
    type PointerButtonEvent = Button;
    type PointerMotionEvent = Motion;
    type PointerMotionAbsoluteEvent = UnusedEvent;
    type GestureSwipeBeginEvent = UnusedEvent;
    type GestureSwipeUpdateEvent = UnusedEvent;
    type GestureSwipeEndEvent = UnusedEvent;
    type GesturePinchBeginEvent = UnusedEvent;
    type GesturePinchUpdateEvent = UnusedEvent;
    type GesturePinchEndEvent = UnusedEvent;
    type GestureHoldBeginEvent = UnusedEvent;
    type GestureHoldEndEvent = UnusedEvent;
    type TouchDownEvent = UnusedEvent;
    type TouchUpEvent = UnusedEvent;
    type TouchMotionEvent = UnusedEvent;
    type TouchCancelEvent = UnusedEvent;
    type TouchFrameEvent = UnusedEvent;
    type TabletToolAxisEvent = UnusedEvent;
    type TabletToolProximityEvent = UnusedEvent;
    type TabletToolTipEvent = UnusedEvent;
    type TabletToolButtonEvent = UnusedEvent;
    type SwitchToggleEvent = UnusedEvent;
    type SpecialEvent = UnusedEvent;
}

/// A drag, run through the real input path from press to release.
///
/// Motion is delivered in steps rather than as one jump, because that is what
/// a mouse does and because a grab that only works for a single large delta is
/// not a working grab.
pub(crate) fn drag(
    state: &mut Solium,
    region: Rectangle<i32, Logical>,
    from: Point<f64, Logical>,
    to: Point<f64, Logical>,
    steps: u32,
) {
    let steps = steps.max(1);
    let mut time = 0_u64;
    let tick = |time: &mut u64| {
        *time += 8_000;
        *time
    };

    // Put the pointer on the target first. Relative motion has no absolute
    // form, so getting somewhere means moving there from wherever it is.
    let start = current(state);
    send_motion(state, region, from - start, tick(&mut time));

    send_button(
        state,
        region,
        BTN_LEFT,
        ButtonState::Pressed,
        tick(&mut time),
    );

    let span = to - from;
    for step in 1..=steps {
        let progress = f64::from(step) / f64::from(steps);
        let previous = f64::from(step - 1) / f64::from(steps);
        let delta = (
            span.x * (progress - previous),
            span.y * (progress - previous),
        );
        send_motion(state, region, delta.into(), tick(&mut time));
    }

    send_button(
        state,
        region,
        BTN_LEFT,
        ButtonState::Released,
        tick(&mut time),
    );
}

fn current(state: &Solium) -> Point<f64, Logical> {
    state
        .seat
        .get_pointer()
        .map(|pointer| pointer.current_location())
        .unwrap_or_default()
}

fn send_motion(
    state: &mut Solium,
    region: Rectangle<i32, Logical>,
    delta: Point<f64, Logical>,
    time: u64,
) {
    crate::input::handle::<Synthetic>(
        state,
        region,
        InputEvent::PointerMotion {
            event: Motion { delta, time },
        },
    );
}

fn send_button(
    state: &mut Solium,
    region: Rectangle<i32, Logical>,
    button: u32,
    button_state: ButtonState,
    time: u64,
) {
    crate::input::handle::<Synthetic>(
        state,
        region,
        InputEvent::PointerButton {
            event: Button {
                button,
                state: button_state,
                time,
            },
        },
    );
}
