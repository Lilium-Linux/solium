//! Each finger keeps the target it touched until release or cancellation.
//!
//! This capture is independent of pointer presses and grabs. Client fingers
//! use the seat; compositor fingers never enter it, including their releases.

use smithay::{
    backend::input::TouchSlot,
    input::touch::{DownEvent, MotionEvent, UpEvent},
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge,
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER},
};

use super::{grab::MoveDrag, resize::ResizeDrag};

use crate::{
    pane::{Pane, PaneId},
    qml::hosted::{Asking, SceneTouch, TouchKind},
    scripted::Outside,
    state::{Chrome, ScenePress, Solium},
};

#[derive(Clone, Debug)]
pub(crate) enum TouchTarget {
    /// A lock client's surface, through the seat.
    Lock,
    /// Solium's own lock: the scene on the monitor named, drawn across the
    /// rectangle, which hears the finger as Qt touch events and is the only
    /// thing that does (`lock_scene.rs`).
    LockScene(String, Rectangle<i32, Logical>),
    /// A key of the native lock's `SecretKeypad`, typed into the password
    /// when the finger went down. Nothing hears the finger, its scene least
    /// of all (`lock_auth.rs`).
    Keypad,
    Grab(ScenePress),
    Scene(ScenePress),
    Frame(PaneId),
    Border(PaneId, ResizeEdge),
    Move(MoveDrag),
    Resize(ResizeDrag),
    Client,
    Script,
    Swallowed,
}

/// Up events carry no position; retain it for Qt's release and cancellation.
#[derive(Clone, Debug)]
pub(crate) struct Contact {
    target: TouchTarget,
    location: Point<f64, Logical>,
    output: String,
    hit: Hit,
    accepting: bool,
    /// Where it has been, for the velocity every event reports (#284).
    tracker: super::velocity::Tracker,
}

/// The velocity fit's window: `input.touchscreen.velocity_window_ms`, or
/// [`super::velocity::WINDOW_MS`].
fn velocity_window(state: &Solium) -> u32 {
    state
        .input
        .config()
        .default_for(crate::input::devices::DeviceKind::Touchscreen)
        .and_then(|settings| settings.velocity_window_ms)
        .unwrap_or(super::velocity::WINDOW_MS)
}

#[derive(Clone, Debug)]
struct Hit {
    target: &'static str,
    window: Option<u64>,
    control: bool,
    edges: Option<&'static str>,
}

fn edge_name(edges: ResizeEdge) -> Option<&'static str> {
    Some(match edges {
        ResizeEdge::Top => "top",
        ResizeEdge::Bottom => "bottom",
        ResizeEdge::Left => "left",
        ResizeEdge::Right => "right",
        ResizeEdge::TopLeft => "top_left",
        ResizeEdge::TopRight => "top_right",
        ResizeEdge::BottomLeft => "bottom_left",
        ResizeEdge::BottomRight => "bottom_right",
        _ => return None,
    })
}

fn hit(state: &Solium, target: &TouchTarget, location: Point<f64, Logical>) -> Hit {
    let mut hit = Hit {
        target: "background",
        window: None,
        control: false,
        edges: None,
    };
    match target {
        TouchTarget::Frame(id) | TouchTarget::Border(id, _) => {
            hit.window = state
                .panes
                .get(*id)
                .and_then(Pane::client)
                .map(|w| state.window_id(w));
            match target {
                TouchTarget::Border(_, edges) => {
                    hit.target = "border";
                    hit.edges = edge_name(*edges);
                }
                _ => {
                    hit.target = "frame";
                    if let Some(under) = state.chrome_under(location) {
                        hit.control = state
                            .panes
                            .get(*id)
                            .and_then(Pane::decoration)
                            .is_some_and(|d| d.touch_claims(under.local.x, under.local.y));
                    }
                }
            }
        }
        TouchTarget::Client => {
            hit.target = "client";
            if !state.pointed_above(location) {
                hit.window = state
                    .window_under(location)
                    .map(|(w, _)| state.window_id(&w));
            }
        }
        TouchTarget::Scene(_) | TouchTarget::Grab(_) => hit.target = "scene",
        TouchTarget::Script => hit.target = "script",
        TouchTarget::Lock
        | TouchTarget::LockScene(..)
        | TouchTarget::Keypad
        | TouchTarget::Swallowed => {
            hit.target = "blocked";
        }
        TouchTarget::Move(_) | TouchTarget::Resize(_) => {}
    }
    hit
}

/// Ownership may change only before this down has reached Qt or a client.
/// First accepted command wins; stale slots and motion-time claims are inert.
pub(crate) fn claim(state: &mut Solium, slot: i32, action: crate::script::TouchAction) {
    let Some(contact) = state.touches.get(&slot).filter(|c| c.accepting).cloned() else {
        return;
    };
    if state.lock.is_some()
        || matches!(
            contact.target,
            TouchTarget::Lock
                | TouchTarget::LockScene(..)
                | TouchTarget::Keypad
                | TouchTarget::Swallowed
        )
    {
        return;
    }
    let target = match action {
        crate::script::TouchAction::Capture => TouchTarget::Script,
        crate::script::TouchAction::Move(id) => {
            let Some(window) = state.window_by_id(id) else {
                return;
            };
            let Some(geometry) = state.real_geometry(&window) else {
                return;
            };
            TouchTarget::Move(MoveDrag::new(window, geometry.loc, contact.location))
        }
        crate::script::TouchAction::Resize(id, edges) => {
            // The pending resize/hold is shared with pointer input.
            if super::resize::busy(state) {
                TouchTarget::Swallowed
            } else {
                let Some(window) = state.window_by_id(id) else {
                    return;
                };
                let Some(began) = state.begin_resize(&window) else {
                    return;
                };
                let laid_out = state
                    .pane_laid_out(&window)
                    .unwrap_or(super::resize::LaidOut(began));
                TouchTarget::Resize(ResizeDrag::new(
                    window,
                    edges,
                    began,
                    laid_out,
                    contact.location,
                ))
            }
        }
    };
    if let Some(contact) = state.touches.get_mut(&slot) {
        contact.target = target;
        contact.accepting = false;
    }
}

fn target(state: &mut Solium, location: Point<f64, Logical>) -> TouchTarget {
    if let Some(lock) = state.lock.as_ref() {
        // Solium's own lock: the scene on the monitor under the finger, and
        // nothing else -- no client, no scene of the session's. A finger on
        // no monitor goes to the seat with nothing under it, which is nobody.
        // `state::tests::real_client::reflow_on_close::hosted::a_finger_at_the_native_lock_reaches_its_scene_and_nothing_else`.
        // A finger on a keypad key types it, and the scene never hears it:
        // `lock_auth::tests::a_keypad_key_types_into_the_secret_and_its_scene_never_hears_which`.
        if lock.is_native()
            && let Some(output) = crate::monitor::at(&state.space, location)
            && let Some(area) = state.space.output_geometry(&output)
        {
            if let Some(key) = state.lock_scenes.keypad_at(&output.name(), area, location) {
                state.keypad_down(key);
                return TouchTarget::Keypad;
            }
            return TouchTarget::LockScene(output.name(), area);
        }
        return TouchTarget::Lock;
    }
    if let Some(grab) = state.hosted_grab.clone() {
        let inside = state
            .surfaces
            .get(grab.surface)
            .is_some_and(|surface| surface.grab_contains(&grab.output, grab.area, location));
        if inside {
            return TouchTarget::Grab(ScenePress {
                surface: grab.surface,
                output: grab.output,
                area: grab.area,
            });
        }
        let policy = state
            .surfaces
            .get(grab.surface)
            .map(|surface| surface.declared.outside_click.for_grab(&grab.name))
            .unwrap_or_default();
        state.dismiss_hosted_grab();
        state.repoint_clients();
        if policy == Outside::Swallow {
            return TouchTarget::Swallowed;
        }
    }
    if let Some((output, surface, area)) = state.surface_claiming(true, location, Asking::Press) {
        return TouchTarget::Scene(ScenePress {
            surface,
            output,
            area,
        });
    }
    // As with pointer input, a mode owns the background while its hosted
    // controls above the windows still receive their own presses.
    if state.script_grab {
        return TouchTarget::Script;
    }
    // A client's layer above the windows occludes their frames too.
    if !state.client_above(location)
        && let Some(under) = state.chrome_under(location)
    {
        return match under.chrome {
            Chrome::Frame => TouchTarget::Frame(under.pane),
            Chrome::Resize(edges) => TouchTarget::Border(under.pane, edges),
        };
    }
    if let Some((output, surface, area)) = state.surface_claiming(false, location, Asking::Press) {
        return TouchTarget::Scene(ScenePress {
            surface,
            output,
            area,
        });
    }
    TouchTarget::Client
}

/// Let go of every finger moving `window`: the rest of its stream is nobody's,
/// as a swallowed finger's is. See [`super::grab::let_go_of`].
pub(crate) fn let_go_of(state: &mut Solium, window: &smithay::desktop::Window) {
    for contact in state.touches.values_mut() {
        if matches!(&contact.target, TouchTarget::Move(drag) if drag.window() == window) {
            contact.target = TouchTarget::Swallowed;
        }
    }
}

/// Whether a finger owns the shared resize pipeline until its up/cancel.
pub(crate) fn resizing(state: &Solium) -> bool {
    state
        .touches
        .values()
        .any(|contact| matches!(contact.target, TouchTarget::Resize(_)))
}

pub(crate) fn down(
    state: &mut Solium,
    slot: TouchSlot,
    location: Point<f64, Logical>,
    time: u32,
    time_us: u64,
) {
    state.touch_time = time;
    let output = crate::monitor::at(&state.space, location)
        .map(|output| output.name())
        .unwrap_or_default();
    let serial = SERIAL_COUNTER.next_serial();
    let target = target(state, location);
    let contact = Contact {
        hit: hit(state, &target, location),
        target,
        location,
        output,
        accepting: true,
        tracker: super::velocity::Tracker::new(
            time_us,
            location.x,
            location.y,
            velocity_window(state),
        ),
    };
    queue(state, &contact, "down", slot.into());
    state.touches.insert(slot.into(), contact);
    // Outside Smithay's seat locks, before any delivery: Lua decides ownership.
    flush(state);
    let Some(contact) = state.touches.get_mut(&slot.into()) else {
        return;
    };
    contact.accepting = false;
    let target = contact.target.clone();
    match &target {
        TouchTarget::Client | TouchTarget::Lock => {
            let under = state.touch_under(location);
            if let Some(touch) = state.seat.get_touch() {
                touch.down(
                    state,
                    under,
                    &DownEvent {
                        slot,
                        location,
                        serial,
                        time,
                    },
                );
                state.touch_frame_pending = true;
            }
        }
        _ => deliver(state, &target, location, TouchKind::Down, slot.into(), time),
    }
}

pub(crate) fn motion(
    state: &mut Solium,
    slot: TouchSlot,
    location: Point<f64, Logical>,
    time: u32,
    time_us: u64,
) {
    state.touch_time = time;
    let Some(contact) = state.touches.get_mut(&slot.into()) else {
        return;
    };
    contact.location = location;
    contact.tracker.push(time_us, location.x, location.y);
    let contact = contact.clone();
    match &contact.target {
        // A drag keeps what it learns as it goes -- whether a maximised
        // window has come off yet (#452) -- so the contact is given it back.
        TouchTarget::Move(drag) => {
            let mut drag = drag.clone();
            drag.to(state, location);
            if let Some(held) = state.touches.get_mut(&slot.into())
                && matches!(held.target, TouchTarget::Move(_))
            {
                held.target = TouchTarget::Move(drag);
            }
            super::grab::settle(state);
            state.redraw = true;
        }
        TouchTarget::Client | TouchTarget::Lock => {
            let under = state.touch_under(location);
            if let Some(touch) = state.seat.get_touch() {
                touch.motion(
                    state,
                    under,
                    &MotionEvent {
                        slot,
                        location,
                        time,
                    },
                );
                state.touch_frame_pending = true;
            }
        }
        _ => deliver(
            state,
            &contact.target,
            location,
            TouchKind::Motion,
            slot.into(),
            time,
        ),
    }
    queue(state, &contact, "motion", slot.into());
}

pub(crate) fn up(state: &mut Solium, slot: TouchSlot, time: u32, time_us: u64) {
    state.touch_time = time;
    let Some(mut contact) = state.touches.remove(&slot.into()) else {
        return;
    };
    contact.tracker.at(time_us);
    match &contact.target {
        TouchTarget::Client | TouchTarget::Lock => {
            if let Some(touch) = state.seat.get_touch() {
                touch.up(
                    state,
                    &UpEvent {
                        slot,
                        serial: SERIAL_COUNTER.next_serial(),
                        time,
                    },
                );
                state.touch_frame_pending = true;
            }
        }
        _ => deliver(
            state,
            &contact.target,
            contact.location,
            TouchKind::Up,
            slot.into(),
            time,
        ),
    }
    queue(state, &contact, "up", slot.into());
}

pub(crate) fn frame(state: &mut Solium) {
    // Remember events rather than active slots: the final release still needs
    // its frame after the slot has been removed.
    if std::mem::take(&mut state.touch_frame_pending)
        && let Some(touch) = state.seat.get_touch()
    {
        touch.frame(state);
    }
}

pub(crate) fn cancel(state: &mut Solium) {
    let contacts = std::mem::take(&mut state.touches);
    let pending = std::mem::take(&mut state.touch_frame_pending);
    let mut clients = Vec::new();
    // Input backend timestamps have their own epoch. Include other fingers'
    // most recent events, including up, to keep the client stream ordered.
    let time = state.touch_time;
    for (slot, contact) in contacts {
        match &contact.target {
            TouchTarget::Client | TouchTarget::Lock => clients.push((slot, contact.location)),
            _ => deliver(
                state,
                &contact.target,
                contact.location,
                TouchKind::Cancel,
                slot,
                time,
            ),
        }
        queue(state, &contact, "cancel", slot);
    }
    if let Some(touch) = state.seat.get_touch() {
        if pending {
            // Finish every outstanding release, including one client's last
            // up while a different client still has a finger to cancel.
            touch.frame(state);
        }
        if !clients.is_empty() {
            // Smithay 0.7 skips cancel for a slot whose last event has already
            // been framed (TouchInternal::cancel compares current/pending).
            // A stationary motion advances pending through the public API,
            // so cancel clears the focus and reaches that client too. Keep
            // the original position; do not re-hit-test the locked desktop.
            for (slot, location) in clients {
                let slot = u32::try_from(slot).ok().into();
                touch.motion(
                    state,
                    None,
                    &MotionEvent {
                        slot,
                        location,
                        time,
                    },
                );
            }
            touch.cancel(state);
        }
    }
}

fn queue(state: &mut Solium, contact: &Contact, kind: &'static str, slot: i32) {
    // Unlock also cancels contacts, after removing state.lock. Never reveal
    // a lock-screen finger then, or replay observations delayed by locking.
    if state.lock.is_some()
        || matches!(
            contact.target,
            TouchTarget::Lock | TouchTarget::LockScene(..) | TouchTarget::Keypad
        )
    {
        return;
    }
    state.touch_events.push(crate::script::TouchEvent {
        kind,
        slot,
        x: contact.location.x,
        y: contact.location.y,
        output: contact.output.clone(),
        target: contact.hit.target,
        window: contact.hit.window,
        control: contact.hit.control,
        edges: contact.hit.edges,
        time: state.touch_time,
        t: contact.tracker.seconds(),
        velocity: contact.tracker.velocity(),
    });
}

pub(crate) fn flush(state: &mut Solium) {
    let events = std::mem::take(&mut state.touch_events);
    for event in events {
        if state.lock.is_some() {
            break;
        }
        let snapshot = state.snapshot();
        let Some(mut scripts) = state.scripts.take() else {
            break;
        };
        let outcome = scripts.touch(event, snapshot);
        state.scripts = Some(scripts);
        state.apply(outcome);
    }
}

fn deliver(
    state: &mut Solium,
    target: &TouchTarget,
    location: Point<f64, Logical>,
    kind: TouchKind,
    slot: i32,
    time: u32,
) {
    let event = SceneTouch {
        kind,
        slot,
        time: u64::from(time),
    };
    match target {
        TouchTarget::LockScene(monitor, area) => {
            state.lock_scenes.touch(monitor, *area, location, &event);
            // What a finger does to a scene shows on the next frame.
            state.redraw = true;
        }
        // Typed when it went down, in `target`; let go, the pad is not
        // pressed by it any more.
        TouchTarget::Keypad => {
            if matches!(kind, TouchKind::Up | TouchKind::Cancel) {
                state.keypad_up();
            }
        }
        TouchTarget::Scene(held) | TouchTarget::Grab(held) => {
            if let Some(surface) = state.surfaces.get_mut(held.surface) {
                surface.deliver_touch(&held.output, held.area, location, &event);
            }
        }
        TouchTarget::Frame(id) => {
            let Some(pane) = state.panes.get(*id) else {
                return;
            };
            let outer = state.pane_outer(pane);
            let drawn = state.drawn_at(pane, outer, state.clock.now());
            let local =
                crate::present::to_window_space(drawn, outer, location) - outer.loc.to_f64();
            if let Some(decoration) = state.panes.get_mut(*id).and_then(Pane::decoration_mut) {
                decoration.touch(local.x, local.y, &event);
                let action = decoration.take_action();
                if kind != TouchKind::Cancel
                    && let Some(action) = action
                {
                    state.frame_action(*id, action);
                }
            }
        }
        TouchTarget::Move(drag) => match kind {
            // `motion`'s, which keeps what the drag learns.
            TouchKind::Motion => {}
            TouchKind::Up => {
                super::grab::settle(state);
                drag.finish(state, location);
                if let Some((window, x, y)) = state.pending_drop.take() {
                    state.trigger_drop(&window, x, y);
                }
            }
            TouchKind::Down | TouchKind::Cancel => {}
        },
        TouchTarget::Resize(drag) => match kind {
            TouchKind::Motion => drag.to(state, location),
            TouchKind::Up | TouchKind::Cancel => drag.finish(state),
            TouchKind::Down => {}
        },
        TouchTarget::Client
        | TouchTarget::Lock
        | TouchTarget::Swallowed
        | TouchTarget::Script
        | TouchTarget::Border(..) => {}
    }
    state.redraw = true;
}
