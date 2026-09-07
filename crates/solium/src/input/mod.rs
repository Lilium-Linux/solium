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
        pointer::{
            AxisFrame, ButtonEvent, Focus, GrabStartData, MotionEvent, PointerHandle,
            RelativeMotionEvent,
        },
        touch::{DownEvent, MotionEvent as TouchMotionEvent, UpEvent},
    },
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER},
    wayland::pointer_constraints::{PointerConstraint, with_pointer_constraint},
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
///
/// `region` is the part of the global space this device's *absolute* positions
/// are measured against — a touchscreen's own monitor, or the nested window,
/// which is one surface spanning however many monitors are inside it. The
/// backend knows which of those it is and the input layer does not need to.
///
/// A relative device has no region: a mouse reports how far it moved, and the
/// pointer it moves crosses every screen, so that path is bounded by all of
/// them instead.
pub(crate) fn handle<B: InputBackend>(
    state: &mut Solium,
    region: Rectangle<i32, Logical>,
    event: InputEvent<B>,
) {
    match event {
        InputEvent::Keyboard { event } => keyboard(state, event),
        InputEvent::PointerMotion { event } => pointer_relative(state, event),
        InputEvent::PointerMotionAbsolute { event } => pointer_motion(state, region, event),
        InputEvent::PointerButton { event } => pointer_button(state, event),
        InputEvent::PointerAxis { event } => pointer_axis(state, event),
        InputEvent::TouchDown { event } => touch_down(state, region, event),
        InputEvent::TouchMotion { event } => touch_motion(state, region, event),
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
    region: Rectangle<i32, Logical>,
    event: impl AbsolutePositionEvent<B>,
) {
    let Some(pointer) = state.seat.get_pointer() else {
        return;
    };
    let location = absolute_location(region, &event);

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
    // The compositor draws the cursor, so the cursor moving is the screen
    // changing. Without this the pointer only moved when something else
    // happened to want a frame -- which on a still screen is never.
    state.redraw = true;
}

/// Motion from a device that reports movement, not position — a real mouse.
///
/// The nested backend never sends this: winit is a window, so it always knows
/// where the pointer *is* and reports that. libinput reports how far the mouse
/// moved and leaves the position to us, which means a compositor that only
/// handles the absolute case has a pointer that never moves — and no way to
/// tell that apart from input being dead.
fn pointer_relative<B: InputBackend>(state: &mut Solium, event: impl PointerMotionEvent<B>) {
    let Some(pointer) = state.seat.get_pointer() else {
        return;
    };
    // Every screen, not the device's own: a mouse moved right past the edge of
    // one monitor is a pointer arriving on the next, and that is the whole of
    // what crossing between monitors is.
    let screens: Vec<_> = state
        .space
        .outputs()
        .filter_map(|output| state.space.output_geometry(output))
        .collect();
    let was = pointer.current_location();
    let (location, locked) = held(state, &pointer, confine(&screens, was + event.delta()), was);
    let under = state.surface_under(location);
    // A locked pointer does not move, and the protocol is explicit that it is
    // not merely held in place: the compositor sends no motion at all. Sending
    // it with the same coordinates over and over is not the same thing, and a
    // client that reads motion events to decide what the raw deltas mean will
    // make nothing of them.
    //
    // Everything positional is skipped with it. Hover, focus-follows-mouse and
    // the shell all answer the question "where is the pointer now", and while
    // it is locked the answer has not changed.
    if !locked {
        // As in `pointer_motion`: over nothing of a client's, the cursor is the
        // compositor's again. This is the path a real mouse takes, so leaving
        // it out is leaving it broken on the hardware and fixed nested.
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
            under.clone(),
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time_msec(),
            },
        );
    }

    // Raw movement, delivered whatever the pointer's position did.
    //
    // This is the event a game reads to turn a camera, and it is *not* the
    // same information as where the pointer is: with the pointer locked the
    // position does not change at all, and with it free the position stops at
    // the edge of the screen while the mouse keeps going. Sent after the
    // motion, so a client that reads both sees them in the order they
    // happened.
    pointer.relative_motion(
        state,
        under,
        &RelativeMotionEvent {
            delta: event.delta(),
            delta_unaccel: event.delta_unaccel(),
            utime: event.time(),
        },
    );
    pointer.frame(state);
    // The compositor draws the cursor, so the cursor moving is the screen
    // changing. Without this the pointer only moved when something else
    // happened to want a frame -- which on a still screen is never.
    state.redraw = true;
}

/// Where the pointer may go, once the window under it has had its say.
///
/// A window can ask for the pointer to be *locked* — held exactly where it is,
/// however far the mouse moves — or *confined* to a region of itself. Both are
/// how a game stops the cursor wandering off the window it is being played in,
/// and both are meaningless without relative motion, which is why they arrive
/// together.
///
/// Only an active constraint counts. One exists from the moment a client asks
/// for it and is activated when the compositor agrees, which is the compositor
/// deciding that the window really is the one being used.
fn held(
    state: &mut Solium,
    pointer: &PointerHandle<Solium>,
    wanted: Point<f64, Logical>,
    was: Point<f64, Logical>,
) -> (Point<f64, Logical>, bool) {
    let Some(surface) = pointer.current_focus() else {
        return (wanted, false);
    };
    // The surface's own origin, to read a region against: a constraint's region
    // is in the surface's coordinates and the pointer is in the screen's.
    let origin = state
        .window_for(&surface)
        .and_then(|window| state.real_geometry(&window))
        .map(|real| real.loc.to_f64());

    let holding = with_pointer_constraint(&surface, pointer, |constraint| {
        let constraint = constraint?;
        // Granted here rather than when it was asked for. A constraint exists
        // from the moment a client requests it; agreeing to it is the
        // compositor's decision, and the honest test is that the pointer is
        // over the window asking -- which is exactly what being here means,
        // since this runs for the surface the pointer is focused on.
        //
        // Later than the request on purpose: see `new_constraint`.
        if !constraint.is_active() {
            constraint.activate();
            tracing::debug!("a window took the pointer");
        }
        match &*constraint {
            PointerConstraint::Locked(_) => Some(Held::Still),
            PointerConstraint::Confined(_) => {
                Some(Held::Inside(constraint.region().map(|region| {
                    let inside = wanted - origin.unwrap_or_default();
                    #[expect(clippy::cast_possible_truncation, reason = "a point on this screen")]
                    region.contains((inside.x.round() as i32, inside.y.round() as i32))
                })))
            }
        }
    });

    match holding {
        // Held still. The mouse still moves and the client still hears about it
        // through `relative_motion`; what does not move is the cursor, which is
        // the entire point.
        Some(Held::Still) => (was, true),
        // Confined to the whole surface, or to one we could not place: staying
        // put beats escaping.
        Some(Held::Inside(None)) => (wanted, false),
        // Stops at the edge rather than sliding along it. Cruder than clamping
        // to the region's nearest point, and honest: an arbitrary region has no
        // single nearest point, and a pointer that stops is one a client can
        // still reason about.
        Some(Held::Inside(Some(true))) => (wanted, false),
        Some(Held::Inside(Some(false))) => (was, false),
        // Nothing holds it now. If a client asked, while it *was* holding it,
        // for the cursor to be left somewhere in particular, this is the moment
        // that was about: a game unlocking the pointer wants it back in the
        // middle of its window rather than wherever the lock happened to catch
        // it. Taken once.
        None => (state.constraint_hint.take().unwrap_or(wanted), false),
    }
}

/// What the window under the pointer is doing to it.
enum Held {
    /// Locked: the pointer does not move at all.
    Still,
    /// Confined to a region. `Some(false)` means this step would leave it;
    /// `None` means there is no region to test and anywhere is allowed.
    Inside(Option<bool>),
}

/// Keep the pointer on a screen — any screen.
///
/// Relative motion has no bounds of its own: without this the pointer walks off
/// the desktop and never comes back, which looks exactly like it froze.
///
/// Two monitors are not one big rectangle. A 2560x1440 beside a 1920x1080 with
/// their top edges aligned leaves 360 rows below the smaller one that belong to
/// no screen at all, and an L-shaped arrangement is mostly hole. So the test is
/// "is this point on *a* monitor", and a point that is on none is pulled into
/// the nearest one rather than clamped to a bounding box — clamping to the box
/// is what would let the pointer sit in the dead corner, visible, unable to
/// reach anything, which is worse than not moving at all.
fn confine(
    screens: &[Rectangle<i32, Logical>],
    location: Point<f64, Logical>,
) -> Point<f64, Logical> {
    // The edge, not one past it: a pointer at exactly x = width is off the
    // right-hand monitor, and on a single screen it was off the desktop.
    let inside = |screen: &Rectangle<i32, Logical>| {
        let (left, top) = (f64::from(screen.loc.x), f64::from(screen.loc.y));
        let right = left + f64::from((screen.size.w - 1).max(0));
        let bottom = top + f64::from((screen.size.h - 1).max(0));
        (left, top, right, bottom)
    };

    if screens.iter().any(|screen| {
        let (left, top, right, bottom) = inside(screen);
        location.x >= left && location.x <= right && location.y >= top && location.y <= bottom
    }) {
        return location;
    }

    // Nowhere valid: the closest point on the closest screen. Measured to the
    // *clamped* point rather than to the screen's centre, so a pointer just
    // below the small monitor lands on its bottom edge instead of being thrown
    // to the middle of the big one.
    let nearest = screens
        .iter()
        .map(|screen| {
            let (left, top, right, bottom) = inside(screen);
            let at = Point::from((location.x.clamp(left, right), location.y.clamp(top, bottom)));
            let (dx, dy) = (at.x - location.x, at.y - location.y);
            (at, dx * dx + dy * dy)
        })
        .min_by(|(_, a), (_, b)| a.total_cmp(b));

    // No screens at all. Nothing is on the desktop to be off the edge of, so
    // the pointer is left where it was asked to go.
    nearest.map_or(location, |(at, _)| at)
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
    let under = state.decorated_under(location);

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
        && let Some((id, window, local)) = state.frame_under(location)
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
            state.frame_action(id, action);
        }

        // Focus and dragging need a window. A frame around one that is still
        // loading has neither, and its buttons work anyway -- which is the
        // point of giving it a frame: an application that is not coming can be
        // dismissed before it arrives.
        if pressed && let Some(window) = window {
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
    // The compositor draws the cursor, so the cursor moving is the screen
    // changing. Without this the pointer only moved when something else
    // happened to want a frame -- which on a still screen is never.
    state.redraw = true;

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
    // The compositor draws the cursor, so the cursor moving is the screen
    // changing. Without this the pointer only moved when something else
    // happened to want a frame -- which on a still screen is never.
    state.redraw = true;
}

fn touch_down<B: InputBackend>(
    state: &mut Solium,
    region: Rectangle<i32, Logical>,
    event: impl TouchDownEvent<B>,
) {
    let Some(touch) = state.seat.get_touch() else {
        return;
    };
    let location = absolute_location(region, &event);
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
    region: Rectangle<i32, Logical>,
    event: impl TouchMotionEventTrait<B>,
) {
    let Some(touch) = state.seat.get_touch() else {
        return;
    };
    let location = absolute_location(region, &event);
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

/// Turn a device's own position into a point in the global space.
///
/// An absolute device reports where it is within *its* region normalised to
/// 0..1 — the nested window, or the monitor a touchscreen is glued to. So the
/// value is scaled by that region's size and then offset by where the region
/// sits, which is the step that was missing while there was only ever one
/// region and it was always at the origin.
fn absolute_location<B: InputBackend>(
    region: Rectangle<i32, Logical>,
    event: &impl AbsolutePositionEvent<B>,
) -> Point<f64, Logical> {
    let at: Point<f64, Logical> = (
        event.x_transformed(region.size.w),
        event.y_transformed(region.size.h),
    )
        .into();
    at + region.loc.to_f64()
}

#[cfg(test)]
mod tests {
    use super::{Request, combo_for, confine, escape};
    use smithay::{
        input::keyboard::{Keysym, ModifiersState},
        utils::Rectangle,
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

    /// One screen, and the case this started as: relative motion has no bounds
    /// of its own, so without confinement the pointer walks off and never comes
    /// back, which looks exactly like a freeze.
    #[test]
    fn the_pointer_stays_on_the_screen() {
        let screens = [Rectangle::new((0, 0).into(), (1920, 1080).into())];
        assert_eq!(confine(&screens, (-40.0, -10.0).into()), (0.0, 0.0).into());
        assert_eq!(
            confine(&screens, (9999.0, 9999.0).into()),
            (1919.0, 1079.0).into()
        );
    }

    #[test]
    fn a_pointer_already_on_the_screen_is_left_alone() {
        let screens = [Rectangle::new((0, 0).into(), (1920, 1080).into())];
        assert_eq!(
            confine(&screens, (640.0, 480.0).into()),
            (640.0, 480.0).into()
        );
    }

    /// The whole point of the change: the edge between two monitors is not an
    /// edge. A pointer that stopped at x = 1919 was a second screen nothing
    /// could reach.
    #[test]
    fn the_pointer_crosses_between_monitors() {
        let screens = [
            Rectangle::new((0, 0).into(), (1920, 1080).into()),
            Rectangle::new((1920, 0).into(), (1920, 1080).into()),
        ];
        assert_eq!(
            confine(&screens, (1920.0, 500.0).into()),
            (1920.0, 500.0).into()
        );
        // And still stops at the far edge of the far monitor.
        assert_eq!(
            confine(&screens, (5000.0, 500.0).into()),
            (3839.0, 500.0).into()
        );
    }

    /// Two monitors of different heights leave rows that belong to no screen.
    /// Clamping to the bounding box would let the pointer sit in that dead
    /// corner — on nothing, over nothing, unable to reach anything.
    #[test]
    fn the_pointer_cannot_sit_in_the_gap_between_monitors() {
        let screens = [
            Rectangle::new((0, 0).into(), (2560, 1440).into()),
            Rectangle::new((2560, 0).into(), (1920, 1080).into()),
        ];
        let at = confine(&screens, (3000.0, 1300.0).into());
        assert!(
            screens.iter().any(|screen| {
                at.x >= f64::from(screen.loc.x)
                    && at.x <= f64::from(screen.loc.x + screen.size.w - 1)
                    && at.y >= f64::from(screen.loc.y)
                    && at.y <= f64::from(screen.loc.y + screen.size.h - 1)
            }),
            "the pointer was left at {at:?}, which is on no monitor"
        );
        // Pulled to the nearest edge rather than to a centre: it was just below
        // the small monitor, so it belongs on the bottom of the small monitor.
        assert_eq!(at, (3000.0, 1079.0).into());
    }

    /// No outputs mapped at all. Nothing to be off the edge of, and a pointer
    /// snapped to the origin on every event would be a pointer that cannot move
    /// while a monitor is being reconfigured.
    #[test]
    fn with_no_screens_the_pointer_is_left_where_it_was_put() {
        assert_eq!(confine(&[], (640.0, 480.0).into()), (640.0, 480.0).into());
    }
}
