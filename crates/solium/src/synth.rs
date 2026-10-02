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

/// One wheel turn, in v120 steps on each axis, as a mouse wheel sends it.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct Axis {
    v120: (f64, f64),
    time: u64,
}

#[cfg(test)]
impl Event<Synthetic> for Axis {
    fn time(&self) -> u64 {
        self.time
    }
    fn device(&self) -> SynthDevice {
        SynthDevice
    }
}

#[cfg(test)]
impl smithay::backend::input::PointerAxisEvent<Synthetic> for Axis {
    fn amount(&self, axis: smithay::backend::input::Axis) -> Option<f64> {
        self.amount_v120(axis).map(|v120| v120 / 8.0)
    }
    fn amount_v120(&self, axis: smithay::backend::input::Axis) -> Option<f64> {
        Some(match axis {
            smithay::backend::input::Axis::Horizontal => self.v120.0,
            smithay::backend::input::Axis::Vertical => self.v120.1,
        })
    }
    fn source(&self) -> smithay::backend::input::AxisSource {
        smithay::backend::input::AxisSource::Wheel
    }
    fn relative_direction(
        &self,
        _axis: smithay::backend::input::Axis,
    ) -> smithay::backend::input::AxisRelativeDirection {
        smithay::backend::input::AxisRelativeDirection::Identical
    }
}

/// One finger on a touchscreen, at a point of the region it is glued to.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct Touch {
    at: Point<f64, Logical>,
    time: u64,
}

#[cfg(test)]
impl Event<Synthetic> for Touch {
    fn time(&self) -> u64 {
        self.time
    }
    fn device(&self) -> SynthDevice {
        SynthDevice
    }
}

#[cfg(test)]
impl smithay::backend::input::TouchEvent<Synthetic> for Touch {
    fn slot(&self) -> smithay::backend::input::TouchSlot {
        Some(0).into()
    }
}

#[cfg(test)]
impl smithay::backend::input::AbsolutePositionEvent<Synthetic> for Touch {
    fn x(&self) -> f64 {
        self.at.x
    }
    fn y(&self) -> f64 {
        self.at.y
    }
    fn x_transformed(&self, _width: i32) -> f64 {
        self.at.x
    }
    fn y_transformed(&self, _height: i32) -> f64 {
        self.at.y
    }
}

#[cfg(test)]
impl smithay::backend::input::TouchDownEvent<Synthetic> for Touch {}

#[cfg(test)]
impl smithay::backend::input::TouchUpEvent<Synthetic> for Touch {}

impl InputBackend for Synthetic {
    type Device = SynthDevice;
    type KeyboardKeyEvent = UnusedEvent;
    #[cfg(test)]
    type PointerAxisEvent = Axis;
    #[cfg(not(test))]
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
    #[cfg(test)]
    type TouchDownEvent = Touch;
    #[cfg(not(test))]
    type TouchDownEvent = UnusedEvent;
    #[cfg(test)]
    type TouchUpEvent = Touch;
    #[cfg(not(test))]
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

/// One relative motion, through the real input path.
pub(crate) fn send_motion(
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

/// One button press or release, through the real input path.
pub(crate) fn send_button(
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

/// One wheel turn, through the real input path, in v120 steps.
/// `state::tests::real_client::reflow_on_close::hosted::the_wheel_over_a_scene_reaches_it`.
#[cfg(test)]
pub(crate) fn send_axis(
    state: &mut Solium,
    region: Rectangle<i32, Logical>,
    v120: (f64, f64),
    time: u64,
) {
    crate::input::handle::<Synthetic>(
        state,
        region,
        InputEvent::PointerAxis {
            event: Axis { v120, time },
        },
    );
}

/// A tap on a touchscreen at `at`, a point in the global space: the finger
/// down, then up, through the real input path.
/// `state::tests::real_client::reflow_on_close::hosted::a_touch_on_a_shell_button_neither_reaches_nor_focuses_the_window_under_it`.
#[cfg(test)]
pub(crate) fn send_touch(
    state: &mut Solium,
    region: Rectangle<i32, Logical>,
    at: (f64, f64),
    time: u64,
) {
    let at = Point::from(at) - region.loc.to_f64();
    crate::input::handle::<Synthetic>(
        state,
        region,
        InputEvent::TouchDown {
            event: Touch { at, time },
        },
    );
    crate::input::handle::<Synthetic>(
        state,
        region,
        InputEvent::TouchUp {
            event: Touch { at, time: time + 1 },
        },
    );
}
