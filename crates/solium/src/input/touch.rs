//! Each finger keeps the target it touched until release or cancellation.
//!
//! This capture is independent of pointer presses and grabs. Client fingers
//! use the seat; compositor fingers never enter it, including their releases.

use smithay::{
    backend::input::TouchSlot,
    input::touch::{DownEvent, MotionEvent, UpEvent},
    utils::{Logical, Point, SERIAL_COUNTER, Serial},
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
    Lock,
    Grab(ScenePress),
    Scene(ScenePress),
    Frame(PaneId),
    Move(MoveDrag),
    Resize(ResizeDrag),
    Client,
    Swallowed,
}

/// Up events carry no position; retain it for Qt's release and cancellation.
#[derive(Clone, Debug)]
pub(crate) struct Contact {
    target: TouchTarget,
    location: Point<f64, Logical>,
}

fn target(state: &mut Solium, location: Point<f64, Logical>, serial: Serial) -> TouchTarget {
    if state.lock.is_some() {
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
    // A client's layer above the windows occludes their frames too.
    if !state.client_above(location)
        && let Some(under) = state.chrome_under(location)
    {
        match under.chrome {
            Chrome::Frame => {
                let control = state
                    .panes
                    .get(under.pane)
                    .and_then(Pane::decoration)
                    .is_some_and(|decoration| {
                        decoration.touch_claims(under.local.x, under.local.y)
                    });
                if let Some(window) = under.window {
                    state.focus_window(&window, serial, crate::script::FocusSource::Touch);
                    if !control && let Some(geometry) = state.real_geometry(&window) {
                        return TouchTarget::Move(MoveDrag::new(window, geometry.loc, location));
                    }
                }
                return TouchTarget::Frame(under.pane);
            }
            Chrome::Resize(edges) => {
                if let Some(window) = under.window {
                    state.focus_window(&window, serial, crate::script::FocusSource::Touch);
                    // Reconcile an earlier hold before freezing either base,
                    // in the same order as the pointer's resize grab.
                    let began = state.begin_resize(&window).unwrap_or(under.outer);
                    let laid_out = state
                        .pane_laid_out(&window)
                        .unwrap_or(super::resize::LaidOut(began));
                    return TouchTarget::Resize(ResizeDrag::new(
                        window, edges, began, laid_out, location,
                    ));
                }
            }
        }
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

pub(crate) fn down(state: &mut Solium, slot: TouchSlot, location: Point<f64, Logical>, time: u32) {
    let serial = SERIAL_COUNTER.next_serial();
    let target = target(state, location, serial);
    match &target {
        TouchTarget::Client | TouchTarget::Lock => {
            let under = state.touch_under(location);
            if matches!(target, TouchTarget::Client)
                && state.profile.touch_to_focus
                && !state.pointed_above(location)
                && let Some((window, _)) = state.window_under(location)
            {
                state.focus_window(&window, serial, crate::script::FocusSource::Touch);
            }
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
    state
        .touches
        .insert(slot.into(), Contact { target, location });
}

pub(crate) fn motion(
    state: &mut Solium,
    slot: TouchSlot,
    location: Point<f64, Logical>,
    time: u32,
) {
    let Some(contact) = state.touches.get_mut(&slot.into()) else {
        return;
    };
    contact.location = location;
    let target = contact.target.clone();
    match &target {
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
            &target,
            location,
            TouchKind::Motion,
            slot.into(),
            time,
        ),
    }
}

pub(crate) fn up(state: &mut Solium, slot: TouchSlot, time: u32) {
    let Some(contact) = state.touches.remove(&slot.into()) else {
        return;
    };
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
    let time = u32::try_from(state.clock.now().as_millis()).unwrap_or(u32::MAX);
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
    }
    if let Some(touch) = state.seat.get_touch() {
        if pending && clients.is_empty() {
            // All client fingers already lifted; finish their last releases.
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
            TouchKind::Motion => drag.to(state, location),
            TouchKind::Up => {
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
        TouchTarget::Client | TouchTarget::Lock | TouchTarget::Swallowed => {}
    }
    state.redraw = true;
}
