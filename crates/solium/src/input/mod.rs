//! Input routing.
//!
//! One entry point, [`handle`], for every device *and every backend*. It is
//! generic over `InputBackend`, so the nested winit backend and libinput on the
//! hardware feed the same code — a binding, a profile or a grab cannot behave
//! differently depending on which one is underneath, because there is only one
//! of them.
//!
//! What differs between form factors lives in [`profile`], not in the branches
//! here. Compositor key bindings are intercepted before the focused client sees
//! them; everything else is forwarded.

pub(crate) mod grab;
pub(crate) mod profile;
pub(crate) mod resize;

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, InputBackend, InputEvent, KeyState,
        KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent, TouchDownEvent,
        TouchMotionEvent as TouchMotionEventTrait, TouchUpEvent,
    },
    input::pointer::CursorImageStatus,
    input::{
        keyboard::{FilterResult, Keysym, ModifiersState, xkb},
        pointer::{AxisFrame, ButtonEvent, Focus, GrabStartData, MotionEvent},
        touch::{DownEvent, MotionEvent as TouchMotionEvent, UpEvent},
    },
    output::Output,
    utils::{Logical, Point, SERIAL_COUNTER},
};

use crate::{
    decoration::Decoration,
    script,
    state::{Request, Solium},
};

use grab::MoveGrab;

/// The right mouse button, as the kernel numbers it.
const BTN_RIGHT: u32 = 0x111;

/// Something the compositor itself will do with a key press.
///
/// Carried out of the keyboard filter rather than acted on inside it: the
/// filter runs while the seat holds its own lock, and a script that focused a
/// window from in there would re-enter the keyboard and deadlock.
#[derive(Clone, Debug)]
enum Action {
    /// A key combination a script has claimed.
    Bound(String),
    /// A backend request: switch VT, or stop.
    Backend(Request),
}

/// Route one backend event to the seat.
pub(crate) fn handle<B: InputBackend>(state: &mut Solium, output: &Output, event: InputEvent<B>) {
    match event {
        InputEvent::Keyboard { event } => keyboard(state, event),
        InputEvent::PointerMotion { event } => pointer_relative(state, output, event),
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

fn keyboard<B: InputBackend>(state: &mut Solium, event: impl KeyboardKeyEvent<B>) {
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

            // Escape hatches, before anything else can claim them. These are
            // the only keys that must work when everything else is broken:
            // without them a compositor that mishandles input is a machine you
            // can only recover with the power button.
            if let Some(request) = escape(modifiers, handle.modified_sym()) {
                return FilterResult::Intercept(Some(Action::Backend(request)));
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
            //
            // An *unclaimed* Super combination is logged louder than the rest.
            // Super is the compositor's own modifier, so pressing one and
            // getting nothing is never ordinary typing: it is someone using a
            // binding that is not there, under a name they cannot see. That is
            // worth one line at info, and it is the line that would have
            // answered this question on the first hardware run instead of the
            // fourth.
            if !claimed && modifiers.logo {
                tracing::info!(combo, "no script has bound this");
            } else {
                tracing::debug!(combo, claimed, "key");
            }

            if claimed {
                FilterResult::Intercept(Some(Action::Bound(combo)))
            } else if state.script_grab {
                // A mode owns input: keys it did not bind are swallowed rather
                // than leaking to whatever is underneath it.
                FilterResult::Intercept(None)
            } else {
                FilterResult::Forward
            }
        },
    );

    match bound {
        Some(Some(Action::Bound(combo))) => {
            state.trigger(&combo);
        }
        Some(Some(Action::Backend(request))) => {
            tracing::info!(?request, "backend request from a key");
            state.request = Some(request);
        }
        _ => {}
    }
}

/// The key combinations the compositor keeps for itself, whatever else happens.
///
/// Ctrl+Alt+F-N reaches us as `XF86Switch_VT_N` under any ordinary keymap. The
/// kernel would normally act on it, but not once the VT is in graphics mode, so
/// switching away from Solium is Solium's job. Ctrl+Alt+Backspace stops it
/// outright: the last resort that does not involve the power button.
fn escape(modifiers: &ModifiersState, keysym: Keysym) -> Option<Request> {
    if !(modifiers.ctrl && modifiers.alt) {
        return None;
    }
    if keysym == Keysym::BackSpace {
        return Some(Request::Quit);
    }
    let raw = keysym.raw();
    (Keysym::XF86_Switch_VT_1.raw()..=Keysym::XF86_Switch_VT_12.raw())
        .contains(&raw)
        .then(|| {
            let offset = raw - Keysym::XF86_Switch_VT_1.raw();
            Request::Vt(i32::try_from(offset).unwrap_or(0) + 1)
        })
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

fn pointer_motion<B: InputBackend>(
    state: &mut Solium,
    output: &Output,
    event: impl AbsolutePositionEvent<B>,
) {
    let Some(pointer) = state.seat.get_pointer() else {
        return;
    };
    let location = absolute_location(output, &event);

    // Frames see the pointer before clients do, so buttons light up on hover.
    // Motion is *also* forwarded below, because the pointer leaving a window
    // has to reach it or the window keeps a stale hover state.
    hover_frame(state, location);
    if let Some(area) = state.work_area()
        && let Some(shell) = state.shell()
    {
        shell.pointer(area, location.x, location.y, None);
    }
    follow_pointer(state, location, pointer.is_grabbed());

    let under = state.surface_under(location);

    // Over nothing of a client's, the cursor is the compositor's again. The
    // status is whatever the last client set it to, and a client only sets it
    // while the pointer is over its surface -- so without this the pointer
    // keeps a cursor belonging to a window it has left, and once that surface
    // is gone there is nothing to draw at all: an invisible pointer over the
    // desktop, which is exactly where you need to see it.
    if under.is_none() {
        state.pointer.status = CursorImageStatus::default_named();
    }

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

/// Motion from a device that reports movement, not position — a real mouse.
///
/// The nested backend never sends this: winit is a window, so it always knows
/// where the pointer *is* and reports that. libinput reports how far the mouse
/// moved and leaves the position to us, which means a compositor that only
/// handles the absolute case has a pointer that never moves — and no way to
/// tell that apart from input being dead.
fn pointer_relative<B: InputBackend>(
    state: &mut Solium,
    output: &Output,
    event: impl PointerMotionEvent<B>,
) {
    let Some(pointer) = state.seat.get_pointer() else {
        return;
    };
    let location = confine(output, pointer.current_location() + event.delta());
    let under = state.surface_under(location);
    // As in `pointer_motion`: over nothing of a client's, the cursor is the
    // compositor's again. This is the path a real mouse takes, so leaving it
    // out is leaving it broken on the hardware and fixed nested.
    if under.is_none() {
        state.pointer.status = CursorImageStatus::default_named();
    }

    hover_frame(state, location);
    if let Some(area) = state.work_area()
        && let Some(shell) = state.shell()
    {
        shell.pointer(area, location.x, location.y, None);
    }
    follow_pointer(state, location, pointer.is_grabbed());
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
    settle_resize(state);
}

/// Keep the pointer on the screen.
///
/// Relative motion has no bounds of its own: without this the pointer walks off
/// the output and never comes back, which looks exactly like it froze.
fn confine(output: &Output, location: Point<f64, Logical>) -> Point<f64, Logical> {
    let size = output
        .current_mode()
        .map(|mode| mode.size)
        .unwrap_or_default();
    let last = |edge: i32| f64::from((edge - 1).max(0));
    (
        location.x.clamp(0.0, last(size.w)),
        location.y.clamp(0.0, last(size.h)),
    )
        .into()
}

/// Act on a resize an edge drag asked for, now the pointer's lock is free.
///
/// Offered to layouts first. Only a window that no layout claims is resized
/// directly, which is what keeps a tiled window from growing over its
/// neighbour instead of moving the seam between them.
fn settle_resize(state: &mut Solium) {
    let Some(request) = state.pending_resize.take() else {
        return;
    };
    if !state.trigger_resize(&request) {
        state.resize_to(&request.window, request.wanted);
    }
}

/// Focus whatever the pointer is over, if the profile says so.
///
/// Skipped while a grab is running: a window being dragged is under the
/// cursor the whole time, and windows sliding past underneath it are not a
/// request to focus each of them in turn.
fn follow_pointer(state: &mut Solium, location: Point<f64, Logical>, grabbed: bool) {
    if !state.profile.focus_follows_mouse || grabbed || state.script_grab {
        return;
    }
    let Some((window, _)) = state.window_under(location) else {
        return;
    };
    if state.is_focused(&window) {
        return;
    }
    state.focus_window(&window, SERIAL_COUNTER.next_serial());
}

/// Offer the pointer to the Developer Tweaks panel.
///
/// Returns whether the panel took it: it is drawn above everything, so a
/// press inside it is not also a press on whatever is underneath.
fn tweaks_pointer(
    state: &mut Solium,
    location: Point<f64, Logical>,
    pressed: Option<bool>,
) -> bool {
    let Some(area) = state.tweaks_area() else {
        return false;
    };
    if !area.to_f64().contains(location) {
        return false;
    }
    let Some(panel) = state.tweaks_panel() else {
        return false;
    };
    panel.pointer(area, location.x, location.y, pressed);
    state.redraw = true;
    true
}

/// Let a window frame see the pointer, so its buttons light up on hover.
fn hover_frame(state: &mut Solium, location: Point<f64, Logical>) {
    if tweaks_pointer(state, location, None) {
        return;
    }
    // The whole window, not just the frame band: a decoration that reacts to
    // the cursor wants to know where it is while it crosses the client too.
    let under = state
        .decorated_under(location)
        .and_then(|(window, local)| state.panes.id_of(&window).map(|id| (id, local)));

    // Whatever we were over and are no longer has to be told, or it stays
    // hovered for as long as the window lives.
    let left = match (state.hovered_frame, &under) {
        (Some(previous), Some((now, _))) if previous == *now => None,
        (previous, _) => previous,
    };
    if let Some(id) = left
        && let Some(decoration) = state.decorations.get_mut(id)
    {
        decoration.pointer_left();
    }

    state.hovered_frame = under.as_ref().map(|(id, _)| *id);
    if let Some((id, local)) = under
        && let Some(decoration) = state.decorations.get_mut(id)
    {
        decoration.pointer(local.x, local.y, None);
    }
}

fn pointer_button<B: InputBackend>(state: &mut Solium, event: impl PointerButtonEvent<B>) {
    let Some(pointer) = state.seat.get_pointer() else {
        return;
    };
    let serial = SERIAL_COUNTER.next_serial();
    let button = event.button_code();
    let button_state = event.state();
    let location = pointer.current_location();

    // The shell sees the pointer before clients do, and only when nothing is
    // being dragged.
    if !pointer.is_grabbed()
        && let Some(area) = state.work_area()
        && state.shell().is_some_and(|shell| {
            shell.pointer(
                area,
                location.x,
                location.y,
                Some(button_state == ButtonState::Pressed),
            )
        })
    {
        return;
    }

    // While a mode owns input, every press is the mode's: it decides what a
    // click on a thumbnail means, and nothing underneath should see it.
    if state.script_grab {
        if button_state == ButtonState::Pressed {
            state.trigger_click(location.x, location.y);
        }
        return;
    }

    // A press in the tweaks panel is the panel's, and nothing else's.
    if !pointer.is_grabbed() {
        let pressed = button_state == ButtonState::Pressed;
        if tweaks_pointer(state, location, Some(pressed)) {
            state.settle_tweaks();
            return;
        }
    }

    // A press on a frame belongs to the frame: it either hits a button or
    // starts a drag, and either way no client should see it.
    if !pointer.is_grabbed()
        && let Some((window, local)) = state.frame_under(location)
        && let Some(id) = state.panes.id_of(&window)
    {
        let pressed = button_state == ButtonState::Pressed;
        let on_button = state.decorations.get_mut(id).is_some_and(|decoration| {
            decoration.pointer(local.x, local.y, Some(pressed));
            decoration.on_button()
        });

        // Acted on release, so a press that lands on the wrong button can be
        // dragged off it and abandoned.
        if let Some(action) = state
            .decorations
            .get_mut(id)
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

    // An edge drag resizes. Checked before the ordinary click handling, and
    // before the drag modifier, because the edge is the narrower target and
    // whoever is on it meant to be.
    if button_state == ButtonState::Pressed
        && !pointer.is_grabbed()
        && let Some((window, edges, outer)) = state.resize_target(location)
    {
        state.focus_window(&window, serial);
        let start_data = GrabStartData {
            focus: None,
            button,
            location,
        };
        pointer.set_grab(
            state,
            resize::ResizeGrab::new(start_data, window, edges, outer),
            serial,
            Focus::Clear,
        );
        return;
    }

    if button_state == ButtonState::Pressed && !pointer.is_grabbed() {
        let modifiers = state
            .seat
            .get_keyboard()
            .map(|keyboard| keyboard.modifier_state());
        let dragging = modifiers.is_some_and(|modifiers| state.profile.drag_held(&modifiers));

        // The modifier and the right button resize from wherever the pointer
        // is, rather than from an eight-pixel border. Which corner is being
        // pulled comes from which quarter of the window the pointer is in, so
        // there is no edge to find and no direction that cannot be reached.
        if dragging
            && button == BTN_RIGHT
            && let Some((window, _)) = state.window_under(location)
            && let Some(outer) = state.outer_geometry(&window)
        {
            state.focus_window(&window, serial);
            let start_data = GrabStartData {
                focus: None,
                button,
                location,
            };
            pointer.set_grab(
                state,
                resize::ResizeGrab::new(
                    start_data,
                    window,
                    resize::quadrant(outer, location),
                    outer,
                ),
                serial,
                Focus::Clear,
            );
            return;
        }

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

    // Outside the grab now: the pointer's lock is released, so a script may
    // ask where the pointer is without stopping the compositor.
    if let Some((window, x, y)) = state.pending_drop.take() {
        state.trigger_drop(&window, x, y);
    }
}

fn pointer_axis<B: InputBackend>(state: &mut Solium, event: impl PointerAxisEvent<B>) {
    let Some(pointer) = state.seat.get_pointer() else {
        return;
    };

    // A wheel turn with the compositor's modifier held is the compositor's:
    // it is how a scrolling layout moves its viewport. Unmodified, the wheel
    // belongs to whatever is under the cursor -- a layout that ate every wheel
    // event would make every terminal inside it unusable.
    let held_super = state
        .seat
        .get_keyboard()
        .is_some_and(|keyboard| keyboard.modifier_state().logo);
    if held_super {
        let horizontal = event.amount(Axis::Horizontal).unwrap_or_default();
        let vertical = event.amount(Axis::Vertical).unwrap_or_default();
        let (horizontal, vertical) = if horizontal == 0.0 && vertical == 0.0 {
            (
                event.amount_v120(Axis::Horizontal).unwrap_or_default() / 120.0,
                event.amount_v120(Axis::Vertical).unwrap_or_default() / 120.0,
            )
        } else {
            (horizontal, vertical)
        };
        if (horizontal != 0.0 || vertical != 0.0) && state.trigger_scroll(horizontal, vertical) {
            return;
        }
    }

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

fn touch_down<B: InputBackend>(state: &mut Solium, output: &Output, event: impl TouchDownEvent<B>) {
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

fn touch_motion<B: InputBackend>(
    state: &mut Solium,
    output: &Output,
    event: impl TouchMotionEventTrait<B>,
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

fn touch_up<B: InputBackend>(state: &mut Solium, event: impl TouchUpEvent<B>) {
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
fn absolute_location<B: InputBackend>(
    output: &Output,
    event: &impl AbsolutePositionEvent<B>,
) -> Point<f64, Logical> {
    let size = output
        .current_mode()
        .map(|mode| mode.size)
        .unwrap_or_default();
    (event.x_transformed(size.w), event.y_transformed(size.h)).into()
}

#[cfg(test)]
mod tests {
    use super::{Request, combo_for, confine, escape};
    use smithay::{
        input::keyboard::{Keysym, ModifiersState},
        output::{Mode, Output, PhysicalProperties, Subpixel},
        utils::Transform,
    };

    fn modifiers(ctrl: bool, alt: bool) -> ModifiersState {
        ModifiersState {
            ctrl,
            alt,
            ..Default::default()
        }
    }

    #[test]
    fn ctrl_alt_f3_asks_for_the_third_terminal() {
        assert_eq!(
            escape(&modifiers(true, true), Keysym::XF86_Switch_VT_3),
            Some(Request::Vt(3))
        );
        assert_eq!(
            escape(&modifiers(true, true), Keysym::XF86_Switch_VT_1),
            Some(Request::Vt(1))
        );
        assert_eq!(
            escape(&modifiers(true, true), Keysym::XF86_Switch_VT_12),
            Some(Request::Vt(12))
        );
    }

    #[test]
    fn ctrl_alt_backspace_stops_the_compositor() {
        assert_eq!(
            escape(&modifiers(true, true), Keysym::BackSpace),
            Some(Request::Quit)
        );
    }

    /// The escapes must not fire without their modifiers, or backspace in a
    /// text field would end the session.
    #[test]
    fn the_escapes_need_both_modifiers() {
        assert_eq!(escape(&modifiers(false, false), Keysym::BackSpace), None);
        assert_eq!(escape(&modifiers(true, false), Keysym::BackSpace), None);
        assert_eq!(escape(&modifiers(false, true), Keysym::BackSpace), None);
        assert_eq!(
            escape(&modifiers(true, false), Keysym::XF86_Switch_VT_2),
            None
        );
    }

    /// The spelling a real key press produces must be the spelling scripts
    /// bind. This is not obvious: xkb names `Return` with a capital and
    /// `space` without, and the two have to come out the same shape.
    #[test]
    fn key_presses_are_spelled_the_way_scripts_bind_them() {
        let sup = ModifiersState {
            logo: true,
            ..Default::default()
        };
        assert_eq!(combo_for(&sup, Keysym::Return), "super+return");
        assert_eq!(combo_for(&sup, Keysym::space), "super+space");
        assert_eq!(combo_for(&sup, Keysym::q), "super+q");
        assert_eq!(combo_for(&sup, Keysym::KP_Enter), "super+kp_enter");
    }

    #[test]
    fn ordinary_keys_are_not_escapes() {
        assert_eq!(escape(&modifiers(true, true), Keysym::a), None);
        assert_eq!(escape(&modifiers(true, true), Keysym::Return), None);
    }

    fn output() -> Output {
        let output = Output::new(
            "test".to_owned(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Solium".into(),
                model: "test".into(),
            },
        );
        output.change_current_state(
            Some(Mode {
                size: (1920, 1080).into(),
                refresh: 60_000,
            }),
            Some(Transform::Normal),
            None,
            Some((0, 0).into()),
        );
        output
    }

    /// Relative motion has no bounds of its own. Without this the pointer walks
    /// off the output and never comes back, which looks exactly like a freeze.
    #[test]
    fn the_pointer_stays_on_the_screen() {
        let output = output();
        assert_eq!(confine(&output, (-40.0, -10.0).into()), (0.0, 0.0).into());
        assert_eq!(
            confine(&output, (9999.0, 9999.0).into()),
            (1919.0, 1079.0).into()
        );
    }

    #[test]
    fn a_pointer_already_on_the_screen_is_left_alone() {
        assert_eq!(
            confine(&output(), (640.0, 480.0).into()),
            (640.0, 480.0).into()
        );
    }
}
