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
//! what could not be reached, and keys by name for `SOLIUM_KEY_AT`, because
//! `SOLIUM_TRIGGER_AT` runs a binding and a key that xkb itself acts on --
//! Caps Lock, a layout switch -- is not one.
//! `tests::a_scripted_key_is_pressed_through_the_real_input_path_with_russian_active`.

use smithay::{
    backend::input::{
        ButtonState, Device, DeviceCapability, Event, InputBackend, InputEvent, KeyState,
        KeyboardKeyEvent, Keycode, PointerButtonEvent, PointerMotionEvent, UnusedEvent,
    },
    input::keyboard::xkb,
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

/// One key going down or up, by xkb keycode, the shape both real backends
/// report.
#[derive(Debug)]
pub(crate) struct Key {
    code: Keycode,
    state: KeyState,
    time: u64,
}

impl Event<Synthetic> for Key {
    fn time(&self) -> u64 {
        self.time
    }
    fn device(&self) -> SynthDevice {
        SynthDevice
    }
}

impl KeyboardKeyEvent<Synthetic> for Key {
    fn key_code(&self) -> Keycode {
        self.code
    }
    fn state(&self) -> KeyState {
        self.state
    }
    fn count(&self) -> u32 {
        u32::from(self.state == KeyState::Pressed)
    }
}

/// One scroll: a wheel's turn, in v120 steps on each axis, or a touchpad's,
/// which has no steps.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct Axis {
    /// The steps on each axis, from a wheel; a touchpad sends none.
    v120: Option<(f64, f64)>,
    /// How far it scrolled on each axis, in Wayland's units.
    amount: (f64, f64),
    source: smithay::backend::input::AxisSource,
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
        Some(match axis {
            smithay::backend::input::Axis::Horizontal => self.amount.0,
            smithay::backend::input::Axis::Vertical => self.amount.1,
        })
    }
    fn amount_v120(&self, axis: smithay::backend::input::Axis) -> Option<f64> {
        self.v120.map(|(horizontal, vertical)| match axis {
            smithay::backend::input::Axis::Horizontal => horizontal,
            smithay::backend::input::Axis::Vertical => vertical,
        })
    }
    fn source(&self) -> smithay::backend::input::AxisSource {
        self.source
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
    type KeyboardKeyEvent = Key;
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

/// Press a key combination and let it go, through the real input path:
/// each key down in the order written, then up in reverse, as fingers do.
/// `combo` is keysym names joined by `+`, as `SOLIUM_KEY_AT` takes them:
/// `caps_lock`, `shift+alt_l`, `super+return`; `shift`, `ctrl`, `alt` and
/// `super` are the left-hand keys. Each name is found in the live keymap, in
/// any of its layouts, so the key is the one a person would press whichever
/// layout is live. A name the keymap has no key for presses nothing, and
/// says so. `time` is the moment, in milliseconds, of the first press.
/// `tests::a_scripted_key_is_pressed_through_the_real_input_path_with_russian_active`.
pub(crate) fn key(
    state: &mut Solium,
    region: Rectangle<i32, Logical>,
    combo: &str,
    time: u64,
) -> bool {
    let mut codes = Vec::new();
    for name in combo
        .split('+')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        let name = match name.to_ascii_lowercase().as_str() {
            "shift" => "Shift_L".to_owned(),
            "ctrl" | "control" => "Control_L".to_owned(),
            "alt" => "Alt_L".to_owned(),
            "super" | "logo" => "Super_L".to_owned(),
            _ => name.to_owned(),
        };
        let keysym = xkb::keysym_from_name(&name, xkb::KEYSYM_CASE_INSENSITIVE);
        let Some(code) = (keysym.raw() != 0)
            .then(|| crate::keymap::keycode_of(state, keysym))
            .flatten()
        else {
            tracing::warn!(
                combo,
                key = name,
                "no key in this keymap types that, so nothing was pressed"
            );
            return false;
        };
        codes.push(code);
    }
    let mut at = time * 1000;
    let mut send = |state: &mut Solium, code: Keycode, key_state: KeyState| {
        at += 8_000;
        crate::input::handle::<Synthetic>(
            state,
            region,
            InputEvent::Keyboard {
                event: Key {
                    code,
                    state: key_state,
                    time: at,
                },
            },
        );
    };
    for &code in &codes {
        send(state, code, KeyState::Pressed);
    }
    for &code in codes.iter().rev() {
        send(state, code, KeyState::Released);
    }
    true
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
            event: Axis {
                v120: Some(v120),
                amount: (v120.0 / 8.0, v120.1 / 8.0),
                source: smithay::backend::input::AxisSource::Wheel,
                time,
            },
        },
    );
}

/// One touchpad scroll, through the real input path: `amount` on each axis,
/// in Wayland's units, with no steps, as libinput reports two fingers moving.
/// The fingers lifting are a scroll of `(0.0, 0.0)`.
/// `state::tests::real_client::reflow_on_close::hosted::a_touchpad_scroll_reaches_a_scene_in_pixels_and_its_end_does_not`.
#[cfg(test)]
pub(crate) fn send_finger_scroll(
    state: &mut Solium,
    region: Rectangle<i32, Logical>,
    amount: (f64, f64),
    time: u64,
) {
    crate::input::handle::<Synthetic>(
        state,
        region,
        InputEvent::PointerAxis {
            event: Axis {
                v120: None,
                amount,
                source: smithay::backend::input::AxisSource::Finger,
                time,
            },
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

#[cfg(test)]
mod tests {
    use smithay::utils::Rectangle;

    use crate::keymap::live;
    use crate::keymap::tests::us_ru;

    /// **A scripted key is pressed through the real input path, with
    /// `us,ru` and Russian active**: Caps Lock by name locks Caps, Shift and
    /// the left Alt switch the layout through the keymap's own option, and a
    /// combination a binding claims runs the binding -- the filter, xkb and
    /// all, as a key on the keyboard would. A name no key types presses
    /// nothing.
    #[test]
    fn a_scripted_key_is_pressed_through_the_real_input_path_with_russian_active() {
        let (_display, mut state) = us_ru(1);
        let directory = std::env::temp_dir().join("solium-synth-key");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"pressed = 0
            sol.bind("super+k", function() pressed = pressed + 1 end)"#,
        )
        .expect("writing the script");
        let scripts = crate::script::Scripts::load(&config).expect("loading the script");
        state.start_scripts(Some(scripts));
        let _ = std::fs::remove_dir_all(&directory);
        let region = Rectangle::from_size((1920, 1080).into());

        assert!(super::key(&mut state, region, "caps_lock", 0));
        assert_eq!(live(&mut state), (2, true, false), "Caps on, Russian kept");
        assert!(super::key(&mut state, region, "shift+alt_l", 10));
        assert_eq!(live(&mut state), (1, true, false), "the layout switched");
        assert!(super::key(&mut state, region, "super+k", 20));
        let pressed = state
            .scripts
            .as_ref()
            .map(|scripts| scripts.evaluate("return tostring(pressed)"));
        assert_eq!(pressed.as_deref(), Some("1"), "the binding ran");
        assert!(!super::key(&mut state, region, "no_such_key", 30));
        assert_eq!(live(&mut state), (1, true, false), "and pressed nothing");
    }
}
