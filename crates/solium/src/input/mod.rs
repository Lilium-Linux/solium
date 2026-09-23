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
        keyboard::{FilterResult, Keycode, Keysym, ModifiersState, xkb},
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
    pane::Pane,
    script,
    state::{Chrome, Request, Solium},
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
    // Somebody is here. Every event, in one place, because the alternative is
    // eleven places and the twelfth device added later. Device-added and
    // -removed events are deliberately included: plugging a mouse in is a
    // person at the machine.
    state.idle.stir(state.clock.now());

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
    key(state, event.key_code(), event.state(), event.time_msec());
}

/// One key, from whichever backend it came.
///
/// Split out of [`keyboard`] so that a test can press a key through the real
/// filter -- the lock's included -- without building an `InputBackend`, which
/// is some twenty associated types for the sake of three numbers.
pub(crate) fn key(state: &mut Solium, code: Keycode, key_state: KeyState, time: u32) {
    let Some(keyboard) = state.seat.get_keyboard() else {
        return;
    };

    let pressed = key_state == KeyState::Pressed;

    let bound = keyboard.input(
        state,
        code,
        key_state,
        SERIAL_COUNTER.next_serial(),
        time,
        |state, modifiers, handle| {
            let locked = state.lock.is_some();
            // Locked, and the keyboard is on something the lock screen does
            // not own, or a grab is steering it: the key goes nowhere. See
            // `Solium::keys_may_pass`, which is the last word of the rule in
            // `focus.rs` -- nothing should ever get the keyboard into this
            // state, and this is what holds if something does.
            let sealed = locked && !state.keys_may_pass();

            if !pressed {
                // Releases are never bindings, but they must still reach a
                // client that received the press, or it holds the key forever.
                // So the question is whether the *press* was forwarded, not
                // whether a mode is active now. A mode active when the session
                // locked used to intercept every release while the lock screen
                // got every press, and each key of the password auto-repeated
                // at the lock screen until the next one was pressed. A release
                // whose press no one was given stays with the mode that took
                // it. Sealed, nothing goes anywhere, whoever had the press.
                let forwarded = state.keys_forwarded.remove(&code.raw());
                return if sealed || (state.script_grab && !forwarded) {
                    FilterResult::Intercept(None)
                } else {
                    FilterResult::Forward
                };
            }

            let result = press(state, modifiers, handle, locked, sealed);
            if matches!(result, FilterResult::Forward) {
                state.keys_forwarded.insert(code.raw());
            }
            result
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

    // The session unlocked while a key was held -- the Enter that unlocked it,
    // nearly always -- and the keyboard waited for it to come up before going
    // back to a window. See `Solium::unlock`. Out here, not in the filter,
    // because `settle_focus` moves the keyboard and the filter runs inside it.
    if state.refocus_on_release && keyboard.pressed_keys().is_empty() {
        state.refocus_on_release = false;
        state.settle_focus();
    }
}

/// What the keyboard filter answers for a press.
///
/// Split out of [`key`] so that its caller can see the answer: a press it
/// forwards is recorded in `Solium::keys_forwarded`, so that the release
/// follows the press to whoever was given it.
fn press(
    state: &mut Solium,
    modifiers: &ModifiersState,
    handle: smithay::input::keyboard::KeysymHandle<'_>,
    locked: bool,
    sealed: bool,
) -> FilterResult<Option<Action>> {
    // Escape hatches, before anything else can claim them. These are
    // the only keys that must work when everything else is broken:
    // without them a compositor that mishandles input is a machine you
    // can only recover with the power button.
    if let Some(request) = escape(modifiers, handle.modified_sym(), locked) {
        return FilterResult::Intercept(Some(Action::Backend(request)));
    }

    // Locked: nothing is bound. A binding runs a script, and a script
    // can spawn a terminal in one line -- so leaving bindings live
    // would turn the lock screen into a menu of ways past it.
    //
    // What is forwarded reaches a lock surface or nothing. This line
    // used to say so while it was not true: a window opening or
    // closing, a menu grab or a script could each hand the keyboard to
    // an application behind the lock, and nothing handed it back. What
    // makes it true now is the gate in `focus.rs`. Every hand-off of
    // keyboard focus goes through `Solium::give_keyboard`, and every
    // keyboard grab through `Solium::grab_keyboard`, and while locked
    // both refuse anything but a lock surface. `clippy.toml` bans
    // smithay's own `set_focus` and `set_grab` everywhere else, and
    // every grab is released the moment the session locks. `sealed`
    // is the last line: it drops the key if the keyboard is on
    // anything the gate would have refused, or held by any grab.
    if locked {
        return if sealed {
            FilterResult::Intercept(None)
        } else {
            FilterResult::Forward
        };
    }

    // A press answers to two names, tried in order -- see
    // `combos_for` for why both and why this order. `raw_syms` is
    // the key at level 0 of its layout: the same handle and the same
    // xkb lock `modified_sym` takes, which smithay does not hold while
    // this filter runs. A key with more than one keysym at that level
    // gets no second name, the rule `modified_sym` already applies to
    // the first.
    let raw = match handle.raw_syms().as_slice() {
        [only] => Some(*only),
        _ => None,
    };
    let combos = combos_for(modifiers, handle.modified_sym(), raw);
    let claimed = state.scripts.as_ref().and_then(|scripts| {
        combos
            .iter()
            .find(|combo| scripts.has_binding(combo))
            .cloned()
    });
    // What the log calls this press when nothing claimed it: the
    // modified spelling, which is what this line has always printed.
    // The key-cap spelling rides along as `or` when it differs,
    // because the point of the line is to show someone the names a
    // binding could have used, and there are now two.
    let spelled = combos.first().map(String::as_str).unwrap_or_default();
    let or = combos.get(1).map(String::as_str);

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
    match &claimed {
        Some(combo) => tracing::debug!(combo, claimed = true, "key"),
        None if modifiers.logo => {
            tracing::info!(combo = spelled, or, "no script has bound this");
        }
        None => tracing::debug!(combo = spelled, or, claimed = false, "key"),
    }

    if let Some(combo) = claimed {
        // The spelling that matched, not the modified one: the handler
        // is found by the same lookup, and `super+shift+1` has no
        // entry under `super+shift+exclam`.
        FilterResult::Intercept(Some(Action::Bound(combo)))
    } else if state.script_grab {
        // A mode owns input: keys it did not bind are swallowed rather
        // than leaking to whatever is underneath it.
        FilterResult::Intercept(None)
    } else {
        FilterResult::Forward
    }
}

/// The key combinations the compositor keeps for itself, whatever else happens.
///
/// Ctrl+Alt+F-N reaches us as `XF86Switch_VT_N` under any ordinary keymap. The
/// kernel would normally act on it, but not once the VT is in graphics mode, so
/// switching away from Solium is Solium's job. Ctrl+Alt+Backspace stops it
/// outright: the last resort that does not involve the power button.
///
/// Both are given up while the session is locked -- but only one of them.
/// Quitting is refused, because ending the session is precisely how someone
/// would get to the desktop underneath a lock screen, and a lock you can leave
/// with a three-key chord is not a lock. Switching VT stays: whoever can press
/// it is standing at the machine, logind owns the seat, and nothing of this
/// session is visible on another terminal. Taking it away would only remove
/// the way out of a compositor that has stopped answering.
fn escape(modifiers: &ModifiersState, keysym: Keysym, locked: bool) -> Option<Request> {
    if !(modifiers.ctrl && modifiers.alt) {
        return None;
    }
    if keysym == Keysym::BackSpace {
        return (!locked).then_some(Request::Quit);
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

/// Every name a key press answers to, in the order bindings are tried.
///
/// A combination names its modifiers *and* its key, and the keysym xkb reports
/// for a press has the modifiers applied already -- so shift is counted twice.
/// A letter survives that, because `shift+q` arrives as `Q` and
/// `normalise_combo` lowercases it back to `q`. Nothing else does: on a `us`
/// layout `super+shift+1` arrives as `super+shift+exclam`, and the nine
/// `super+shift+N` in `workspaces.lua` and the two shifted brackets in
/// `scrolling.lua` were bindings nothing could produce (#121).
///
/// So a press has two names. `modified` first, which is the only name it had
/// before this and so the one every binding that already fired still fires
/// under -- including one written for a layout where the symbol *is* the key,
/// `super+shift+exclam` among them. Then `raw`, the key as its layout names it
/// with no modifiers applied, which is the key a person pressed and the name a
/// combination like `super+shift+1` spells. When the two come out the same
/// string, as they do for a letter and for any key no held modifier moved off
/// level 0, there is one name.
///
/// `raw` is level 0, which applies no modifiers at all, so it sheds AltGr as
/// well as shift: on `de`, super+AltGr+7 is `super+braceleft` and then
/// `super+7`. Accepted rather than filtered out, because `combo_for` names
/// ctrl, alt, shift and super and nothing else -- a binding could only ever
/// tell those two presses apart by the symbol, and the symbol is still tried
/// first. `altgr_falls_back_to_the_key_it_is_held_on` pins both halves.
///
/// Not the layout-agnostic lookup (`raw_latin_sym_or_raw_current_sym`): that
/// would also rescue bindings under a non-Latin layout, which is a different
/// change and one somebody should decide on.
pub(crate) fn combos_for(
    modifiers: &ModifiersState,
    modified: Keysym,
    raw: Option<Keysym>,
) -> Vec<String> {
    let mut combos = vec![combo_for(modifiers, modified)];
    if let Some(raw) = raw.filter(|raw| *raw != Keysym::NoSymbol) {
        let unmodified = combo_for(modifiers, raw);
        if !combos.contains(&unmodified) {
            combos.push(unmodified);
        }
    }
    combos
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
    //
    // None of it while the session is locked. The pointer still moves and
    // still reaches the lock surface -- a lock screen you cannot click the
    // password field of is no use -- but nothing of the session's may notice
    // it going past.
    if state.lock.is_none() {
        // Scripted surfaces above the windows see the pointer first, so a
        // button on a bar lights up on hover. Then frames, then the ones
        // below.
        if !state.surface_pointer(true, location, None) {
            hover_frame(state, location);
            state.surface_pointer(false, location, None);
        }
        follow_pointer(state, location, pointer.is_grabbed());
    }

    let under = state.surface_under(location);

    // The two halves of the pointer, both of them held by the same grab: see
    // `release_cursor` for `status` and `Solium::assert_cursor` for `chrome`.
    release_cursor(state, under.is_none(), pointer.is_grabbed());
    state.assert_cursor(location, pointer.is_grabbed());

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
        // it out is leaving it broken on the hardware and fixed nested -- and
        // the same goes for the grab test inside it, since a drag with a real
        // mouse arrives here and not above.
        release_cursor(state, under.is_none(), pointer.is_grabbed());
        // And as in `pointer_motion`: over the compositor's own chrome the
        // compositor says what the pointer is. Same reason for the repetition
        // -- this is the path a real mouse takes, and #108 was reported on the
        // hardware.
        state.assert_cursor(location, pointer.is_grabbed());

        // As in `pointer_motion`: while the session is locked, nothing of the
        // session's may notice the pointer going past. Repeated here rather
        // than shared because this is the path a *real mouse* takes -- winit
        // reports position, libinput reports movement -- and a guard that
        // existed only on the nested path would be a lock screen that leaked
        // on the hardware and nowhere else.
        if state.lock.is_none() {
            if !state.surface_pointer(true, location, None) {
                hover_frame(state, location);
                state.surface_pointer(false, location, None);
            }
            follow_pointer(state, location, pointer.is_grabbed());
        }
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

/// Hand the pointer back to the compositor's own arrow, over nothing of a
/// client's.
///
/// The third of the three sources `cursor.rs`'s header sets out. `status` is
/// whatever the last client set it to, by either of its two mechanisms, and a
/// client only sets it while the pointer is over its surface — so without this
/// the pointer keeps a cursor belonging to a window it has left, and once that
/// surface is gone there is nothing to draw at all: an invisible pointer over
/// the desktop, which is exactly where you need to see it.
///
/// **Not while something holds the pointer, and that is not tidiness — it is
/// the same rule as [`Solium::assert_cursor`]'s reaching the other half of the
/// pointer.** `assert_cursor` guards `chrome`; this guards `status`, and
/// `status` is the half a drag writes. Smithay lets it: `wl_pointer.set_cursor`
/// is accepted from whoever holds the grab — `wayland/seat/pointer.rs:521`,
/// whose own comment reads *"Allow client if there is a pointer grab for that
/// client. Like drag and drop"* — which is how a `dnd-copy` or `dnd-move` shape
/// gets onto the pointer at all.
///
/// A drag crosses gaps. Over the desktop between two windows, over the space a
/// tiled layout leaves, over anything Solium draws and no client owns,
/// `surface_under` is `None`; without the grab test the first such gap would
/// replace the drag's shape with the plain arrow and leave it there for the
/// rest of the gesture, because the client has no reason to send `set_cursor`
/// again until the pointer re-enters one of its surfaces. A drag that is still
/// accepting drops would look like one that had stopped — the pointer
/// describing something other than what is happening, which is #108's finding
/// arrived at from a fourth direction.
fn release_cursor(state: &mut Solium, unclaimed: bool, grabbed: bool) {
    if !unclaimed || grabbed {
        return;
    }
    state.pointer.show(CursorImageStatus::default_named());
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

/// Let a window frame see the pointer, so its buttons light up on hover.
///
/// The callers offer the pointer to the surfaces above the windows first, so
/// nothing here has to know about panels.
fn hover_frame(state: &mut Solium, location: Point<f64, Logical>) {
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
        && let Some(decoration) = state.panes.get_mut(id).and_then(Pane::decoration_mut)
    {
        decoration.pointer_left();
    }

    state.hovered_frame = under.as_ref().map(|(id, _)| *id);
    if let Some((id, local)) = under
        && let Some(decoration) = state.panes.get_mut(id).and_then(Pane::decoration_mut)
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

    // Locked: the press is the lock screen's, and the compositor does not get
    // to interpret it. Everything between here and the plain forward below is
    // an interpretation -- the shell's buttons, the tweaks panel, a titlebar,
    // a resize edge, click-to-focus, a script's click handler -- and each one
    // acts on the session that is supposed to be sealed.
    //
    // This is not belt and braces over the checks in `window_under` and
    // `chrome_under`. The resize border used to be a walk of its own that
    // asked neither of them, so before this guard existed a press near where a
    // window's edge used to be started a resize grab: dragging the mouse on a
    // locked screen resized a window nobody could see, and it was still that
    // size when the session unlocked. Found by doing exactly that and
    // measuring the window afterwards. There is one hit test now and its own
    // lock guard covers the border too, which makes this the second of two
    // rather than the only one -- and it stays, because everything below it is
    // an interpretation and not all of it goes through `chrome_under`.
    if state.lock.is_some() {
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
        state.redraw = true;
        return;
    }

    // The next three checks are `state::claim_of`, in its order, and that is
    // not a coincidence to be maintained by hand: the pointer's shape is read
    // off the same rule through `Solium::claim_under`, so wherever this defers
    // the pointer defers with it. Each one is *acted* on here rather than in
    // the rule because acting needs things a value cannot carry -- a surface to
    // deliver the press to, a click to trigger, an `Under` to start a grab
    // from. What must not drift is which of them wins, and that is decided in
    // one place. The first fix for #108 made only the third link agree, which
    // left the pointer promising a resize over an overview thumbnail and over
    // the bottom edge of a bar.
    //
    // Scripted surfaces above the windows see the press before clients do, and
    // only when nothing is being dragged. A bar, a panel, an overlay: all the
    // same path, and the compositor knows what none of them are for.
    if !pointer.is_grabbed()
        && state.surface_pointer(true, location, Some(button_state == ButtonState::Pressed))
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

    // A press on the compositor's own chrome is the compositor's: a frame's
    // button, a move drag, or a resize from an edge. None of it reaches a
    // client.
    //
    // **The same `chrome_under` the pointer's shape was read off on the way
    // here**, which is issue #108: the frame and the resize border overlap,
    // the press had always resolved the overlap in the frame's favour, and
    // nothing told the pointer -- so the band below a window's top edge drew
    // whatever resize cursor a CSD client had set for its shadow and then
    // moved the window when pressed. Asking one predicate for both is what
    // makes that disagreement unrepresentable; two hit tests, however
    // carefully written, drift.
    //
    // The resize border is checked before the ordinary click handling and
    // before the drag modifier, as it always was, because the edge is the
    // narrower target and whoever is on it meant to be.
    if !pointer.is_grabbed()
        && let Some(under) = state.chrome_under(location)
    {
        match under.chrome {
            Chrome::Frame => {
                let pressed = button_state == ButtonState::Pressed;
                let id = under.pane;
                let local = under.local;
                let on_button = state
                    .panes
                    .get_mut(id)
                    .and_then(Pane::decoration_mut)
                    .is_some_and(|decoration| {
                        decoration.pointer(local.x, local.y, Some(pressed));
                        decoration.on_button()
                    });

                // Acted on release, so a press that lands on the wrong button
                // can be dragged off it and abandoned.
                if let Some(action) = state
                    .panes
                    .get_mut(id)
                    .and_then(Pane::decoration_mut)
                    .and_then(Decoration::take_action)
                {
                    state.frame_action(id, action);
                }

                // Focus and dragging need a window. A frame around one that is
                // still loading has neither, and its buttons work anyway --
                // which is the point of giving it a frame: an application that
                // is not coming can be dismissed before it arrives.
                if pressed && let Some(window) = under.window {
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
            Chrome::Resize(edges) if button_state == ButtonState::Pressed => {
                // `pane_chrome` does not report a resize border without a
                // window to resize, so this is that guarantee restated rather
                // than a case that happens.
                if let Some(window) = under.window {
                    state.focus_window(&window, serial);
                    // Before the grab, so the new drag cannot inherit the
                    // previous one's unsettled hold. Its answer is the
                    // rectangle to drag from rather than `under.outer`, which
                    // was read before that hold was reconciled: see
                    // `begin_resize`.
                    let began = state.begin_resize(&window).unwrap_or(under.outer);
                    // And the layout's own rectangle beside it, which is a
                    // different rectangle for any client that has not committed
                    // exactly what it was asked for. `began` is what this
                    // *window* is; `laid_out` is where the layout put it, and a
                    // tiled drag moves a seam from the second. Read after
                    // `begin_resize` like `began` is, though for this one it
                    // makes no difference: reconciling a hold writes the
                    // client's size into the slot and `Pane::placed` is the
                    // field that does not hear about it. See
                    // `Solium::pane_laid_out`.
                    let laid_out = state
                        .pane_laid_out(&window)
                        .unwrap_or(resize::LaidOut(began));
                    let start_data = GrabStartData {
                        focus: None,
                        button,
                        location,
                    };
                    pointer.set_grab(
                        state,
                        resize::ResizeGrab::new(start_data, window, edges, began, laid_out),
                        serial,
                        Focus::Clear,
                    );
                }
                return;
            }
            // A *release* over a resize border is nobody's. The grab begins on
            // the press and ends by releasing that grab, so a release reaching
            // here belongs to whatever is underneath -- which is what it did
            // when the border was a second `if` gated on `Pressed`.
            Chrome::Resize(_) => {}
        }
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
        // `begin_resize` before anything reads a rectangle, and its answer is
        // the rectangle. Two things are going on and both are its
        // documentation's: the previous drag's hold outlives its grab by up to
        // `resizing::PATIENCE`, so this one must not inherit it, and
        // reconciling it *changes* the pane's rectangle — so a rectangle read
        // before that ran is a rectangle this drag would jump away from on its
        // first motion.
        //
        // **`pane_outer`, not `outer_geometry`**, which is what this reached
        // for. `chrome_under`'s own note records that the border drag was moved
        // off `outer_geometry` for exactly this reason: `outer_geometry` is the
        // space's location paired with the *client's* size, and a live hold is
        // a second way for that to differ from the pane's slot — for as long as
        // a client takes to answer. A quadrant drag begun from the stale
        // rectangle jumps the edge nobody is dragging by the unanswered delta,
        // which is issue #113 arriving by the other gesture.
        if dragging
            && button == BTN_RIGHT
            && let Some((window, _)) = state.window_under(location)
            && let Some(outer) = state
                .begin_resize(&window)
                .or_else(|| state.outer_geometry(&window))
        {
            state.focus_window(&window, serial);
            let edges = resize::quadrant(outer, location);
            // #108's first symptom on the *other* resize gesture: the border
            // drag now names its cursor because `chrome_under` answered before
            // the press, and this one had nothing to read -- so `super` and the
            // right button dragged a window's corner about with an I-beam
            // showing, for as long as the drag lasted. Asserted here rather
            // than found by a hit test because there is no border under the
            // pointer to find: the quadrant *is* the answer, and it is known
            // only at the moment the grab starts. It holds for the drag for the
            // same reason a border drag's does -- `Solium::assert_cursor`
            // declines to recompute while the pointer is grabbed -- and the
            // first motion after the release puts it back.
            if state.pointer.assert(Some(resize::cursor(edges))) {
                state.redraw = true;
            }
            // See the border drag above: `outer` is where this window is and
            // `laid_out` is where the layout put it, and only the second is a
            // number the layout will recognise when it comes back.
            let laid_out = state
                .pane_laid_out(&window)
                .unwrap_or(resize::LaidOut(outer));
            let start_data = GrabStartData {
                focus: None,
                button,
                location,
            };
            pointer.set_grab(
                state,
                resize::ResizeGrab::new(start_data, window, edges, outer, laid_out),
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

    // Scripted surfaces *below* the windows, which is where a dock or a
    // desktop menu lives: they get the press only because nothing above
    // wanted it.
    if !pointer.is_grabbed()
        && state.surface_pointer(false, location, Some(button_state == ButtonState::Pressed))
    {
        return;
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
    // Locked, no wheel gesture is the compositor's: `trigger_scroll` runs a
    // script, and a script that can move a layout can do anything else too.
    let held_super = state.lock.is_none()
        && state
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
    use super::{Request, Solium, combo_for, combos_for, confine, escape, release_cursor};
    use smithay::{
        input::keyboard::{Keysym, ModifiersState},
        input::pointer::{CursorIcon, CursorImageStatus},
        reexports::wayland_server::Display,
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
            escape(&modifiers(true, true), Keysym::XF86_Switch_VT_3, false),
            Some(Request::Vt(3))
        );
        assert_eq!(
            escape(&modifiers(true, true), Keysym::XF86_Switch_VT_1, false),
            Some(Request::Vt(1))
        );
        assert_eq!(
            escape(&modifiers(true, true), Keysym::XF86_Switch_VT_12, false),
            Some(Request::Vt(12))
        );
    }

    #[test]
    fn ctrl_alt_backspace_stops_the_compositor() {
        assert_eq!(
            escape(&modifiers(true, true), Keysym::BackSpace, false),
            Some(Request::Quit)
        );
    }

    /// Ctrl+Alt+Backspace is how you get out of a compositor that has stopped
    /// answering -- and it is also how you would get past a lock screen, since
    /// the desktop is behind the session that it ends. Locked, it does nothing.
    #[test]
    fn ctrl_alt_backspace_does_not_end_a_locked_session() {
        assert_eq!(
            escape(&modifiers(true, true), Keysym::BackSpace, true),
            None
        );
    }

    /// Switching terminal stays available while locked, and deliberately.
    /// It exposes nothing: the locked session's contents are not on the
    /// terminal being switched to, and whoever pressed it is at the machine.
    #[test]
    fn a_locked_session_can_still_switch_terminal() {
        assert_eq!(
            escape(&modifiers(true, true), Keysym::XF86_Switch_VT_2, true),
            Some(Request::Vt(2))
        );
    }

    /// The escapes must not fire without their modifiers, or backspace in a
    /// text field would end the session.
    #[test]
    fn the_escapes_need_both_modifiers() {
        assert_eq!(
            escape(&modifiers(false, false), Keysym::BackSpace, false),
            None
        );
        assert_eq!(
            escape(&modifiers(true, false), Keysym::BackSpace, false),
            None
        );
        assert_eq!(
            escape(&modifiers(false, true), Keysym::BackSpace, false),
            None
        );
        assert_eq!(
            escape(&modifiers(true, false), Keysym::XF86_Switch_VT_2, false),
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
        assert_eq!(escape(&modifiers(true, true), Keysym::a, false), None);
        assert_eq!(escape(&modifiers(true, true), Keysym::Return, false), None);
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

    /// **A drag keeps its own shape across the gaps it crosses.**
    ///
    /// `state::tests::a_grabbed_pointer_keeps_the_shape_it_had` pins the other
    /// half of this — `Solium::assert_cursor` holding `chrome` — and this pins
    /// the half the motion handlers own. They are two different guards on two
    /// different fields, and the drag-icon review found the second one missing
    /// while the first was there and tested, so testing only `assert_cursor`
    /// is what let it through.
    ///
    /// The shape being defended is the client's own. Smithay accepts
    /// `wl_pointer.set_cursor` from whoever holds the grab
    /// (`wayland/seat/pointer.rs:521`), so `dnd-copy` and `dnd-move` land in
    /// `status` mid-drag by design; the compositor clearing it on the first
    /// gap between two windows would leave a plain arrow for the rest of the
    /// gesture, which reads as a drag that stopped accepting.
    ///
    /// Run against a real [`Solium`] rather than a bare `cursor::Pointer`,
    /// because a `Pointer` in isolation cannot tell the two callers apart and
    /// the bug was in the callers. One display, no client, no surface and no
    /// toplevel: the fixture note in `state::tests::drag_icon` sets out why
    /// anything more than that has aborted this binary before.
    #[test]
    fn a_grabbed_pointer_keeps_the_shape_its_drag_set() {
        let display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());

        // Where `wl_data_device.start_drag` leaves the pointer: the client
        // holds the grab, so its `set_cursor` is accepted and the shape
        // reaches `status` through `SeatHandler::cursor_image`.
        state
            .pointer
            .show(CursorImageStatus::Named(CursorIcon::Copy));

        // The first gap. Nothing of a client's is under the pointer, which on
        // the ungrabbed path is the compositor's cue to put its arrow back --
        // and is a *write*, which is the one the grab has to suppress.
        release_cursor(&mut state, true, true);
        assert_eq!(
            state.pointer.showing(),
            CursorImageStatus::Named(CursorIcon::Copy),
            "a drag crossing the desktop between two windows must keep the \
             shape its client set; clearing it here leaves a plain arrow for \
             the rest of the gesture, because the client has no reason to say \
             it again"
        );

        // The half that makes this able to fail: the same call holding
        // nothing is the pointer being handed back to the compositor, which is
        // the whole reason `release_cursor` exists. A gate that had swallowed
        // both would pass the assertion above and break every window exit.
        release_cursor(&mut state, true, false);
        assert_eq!(
            state.pointer.showing(),
            CursorImageStatus::default_named(),
            "a pointer that merely left a window, with nothing holding it, \
             gets the compositor's arrow back"
        );

        // And the other axis, unchanged by any of this: over a client's own
        // surface nothing is cleared, grab or no grab.
        state
            .pointer
            .show(CursorImageStatus::Named(CursorIcon::Text));
        release_cursor(&mut state, false, false);
        assert_eq!(
            state.pointer.showing(),
            CursorImageStatus::Named(CursorIcon::Text),
            "a pointer over a client's surface is that client's to describe"
        );
    }

    /// A key event from nowhere, for driving the real keyboard filter.
    ///
    /// `Synthetic` has no keyboard of its own -- keys had `SOLIUM_TRIGGER_AT`
    /// -- but `keyboard` takes any `KeyboardKeyEvent`, so this is all it needs.
    /// `code` is an xkb keycode, evdev plus eight, which is what both real
    /// backends hand over.
    struct Key {
        code: u32,
        state: smithay::backend::input::KeyState,
    }

    impl smithay::backend::input::Event<crate::synth::Synthetic> for Key {
        fn time(&self) -> u64 {
            0
        }
        fn device(&self) -> crate::synth::SynthDevice {
            crate::synth::SynthDevice
        }
    }

    impl smithay::backend::input::KeyboardKeyEvent<crate::synth::Synthetic> for Key {
        fn key_code(&self) -> smithay::backend::input::Keycode {
            self.code.into()
        }
        fn state(&self) -> smithay::backend::input::KeyState {
            self.state
        }
        fn count(&self) -> u32 {
            0
        }
    }

    /// `Super_L`, `Shift_L`, and the top-row keys this is about, as xkb
    /// keycodes on a `us` keymap. Evdev 125, 42, 2, 16 and 26, plus eight.
    const SUPER: u32 = 133;
    const SHIFT: u32 = 50;
    const DIGIT_1: u32 = 10;
    const Q: u32 = 24;
    const BRACKET_LEFT: u32 = 34;
    /// Right Alt, which on a `de` layout is AltGr (evdev 100), and the `7` of
    /// the top row (evdev 8).
    const RIGHT_ALT: u32 = 108;
    const DIGIT_7: u32 = 16;

    /// What a real `Solium` does with `keys`, pressed in order and then let go
    /// in reverse, under `script` and the named xkb `layout`: the status the
    /// binding that fired left behind, or an empty string if none did.
    ///
    /// The whole input path the hardware takes -- `keyboard`, smithay's own
    /// xkb state and its filter, `Scripts::has_binding`, `Solium::trigger` --
    /// with nothing standing in for any of it but the key events. The keymap
    /// is set rather than inherited, because `XkbConfig::default()` reads
    /// `XKB_DEFAULT_LAYOUT`, and a test whose answer depends on the layout of
    /// whoever runs it answers nothing.
    fn status_after(name: &str, layout: &str, script: &str, keys: &[u32]) -> String {
        use smithay::backend::input::KeyState;
        use smithay::input::keyboard::XkbConfig;

        let directory = std::env::temp_dir().join(format!("solium-keys-{name}"));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("creating the script directory");
        let config = directory.join("init.lua");
        std::fs::write(&config, script).expect("writing the test script");

        let display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        let keyboard = state.seat.get_keyboard().expect("the seat has a keyboard");
        keyboard
            .set_xkb_config(
                &mut state,
                XkbConfig {
                    layout,
                    ..Default::default()
                },
            )
            .expect("compiling the keymap; xkb data is missing, so this proves nothing");
        let scripts = crate::script::Scripts::load(&config).expect("loading the test script");
        state.start_scripts(Some(scripts));

        for &code in keys {
            super::keyboard(
                &mut state,
                Key {
                    code,
                    state: KeyState::Pressed,
                },
            );
        }
        for &code in keys.iter().rev() {
            super::keyboard(
                &mut state,
                Key {
                    code,
                    state: KeyState::Released,
                },
            );
        }
        state.status.clone()
    }

    /// **`super+shift+1` fires the binding spelled `super+shift+1`.** #121.
    ///
    /// Before #121, shift+1 arrived as `exclam` and the binding matched on
    /// that alone, so this press produced `super+shift+exclam`, the lookup
    /// missed, and the key did nothing -- which is what all nine
    /// send-to-workspace keys in `workspaces.lua` did, and `scrolling.lua`'s
    /// two shifted brackets. The bracket is here as the second shape of the
    /// same defect: a symbol key rather than a digit.
    #[test]
    fn a_shifted_digit_fires_the_binding_that_names_the_digit() {
        let script = r#"
            sol.bind("super+shift+1", function() sol.status("digit") end)
            sol.bind("super+shift+bracketleft", function() sol.status("bracket") end)
        "#;
        assert_eq!(
            status_after("digit", "us", script, &[SUPER, SHIFT, DIGIT_1]),
            "digit",
            "super+shift+1 on a `us` keyboard must reach `super+shift+1`; it \
             arrives as `exclam`, and matching on that alone is #121"
        );
        assert_eq!(
            status_after("bracket", "us", script, &[SUPER, SHIFT, BRACKET_LEFT]),
            "bracket",
            "super+shift+[ must reach `super+shift+bracketleft`; it arrives as \
             `braceleft`"
        );
    }

    /// **And every binding that already worked still works the same way.**
    ///
    /// The half of the fix that keeps it safe. The modified spelling is tried
    /// first, so a letter under shift is still found under its own name, and a
    /// configuration that bound the *symbol* -- `super+shift+exclam`, which is
    /// the only spelling that worked before -- keeps that key even where
    /// somebody also bound the digit. Were the order reversed, the second
    /// assertion here is the one that would say so.
    #[test]
    fn the_modified_spelling_still_wins() {
        let both = r#"
            sol.bind("super+shift+exclam", function() sol.status("symbol") end)
            sol.bind("super+shift+1", function() sol.status("digit") end)
            sol.bind("super+shift+q", function() sol.status("letter") end)
        "#;
        assert_eq!(
            status_after("letter", "us", both, &[SUPER, SHIFT, Q]),
            "letter",
            "super+shift+q is a letter under shift, and has always worked"
        );
        assert_eq!(
            status_after("symbol", "us", both, &[SUPER, SHIFT, DIGIT_1]),
            "symbol",
            "a press whose modified spelling is bound goes there, and the \
             key-cap spelling is only a fallback"
        );
        // Nothing but the key-cap spelling bound, and the key held without
        // shift: `super+1` is not `super+shift+1`, and the modifiers are never
        // what the fallback drops.
        assert_eq!(
            status_after(
                "unshifted",
                "us",
                r#"sol.bind("super+shift+1", function() sol.status("digit") end)"#,
                &[SUPER, DIGIT_1]
            ),
            "",
            "super+1 must not fire a binding on super+shift+1"
        );
    }

    /// **The fallback drops AltGr as well as shift, and that is a decision.**
    ///
    /// The key-cap spelling is level 0 of the layout, which applies no
    /// modifiers at all, and `combo_for` has no word for AltGr to put back. So
    /// on a `de` keyboard, where AltGr+7 types `{`, super+AltGr+7 is looked up
    /// as `super+braceleft` and then as `super+7` -- and with only the second
    /// bound, it fires workspace 7's key. Before #121 that press did nothing.
    /// Pinned here so the behaviour is one somebody chose rather than one that
    /// came along: the combination syntax cannot name AltGr, so a binding can
    /// only ever tell the two presses apart by the symbol, and the symbol
    /// spelling still wins (the second assertion).
    #[test]
    fn altgr_falls_back_to_the_key_it_is_held_on() {
        let seven = r#"sol.bind("super+7", function() sol.status("seven") end)"#;
        assert_eq!(
            status_after("altgr", "de", seven, &[SUPER, RIGHT_ALT, DIGIT_7]),
            "seven",
            "super+AltGr+7 on `de` has no binding under `super+braceleft` and \
             falls back to `super+7`"
        );
        let brace = r#"
            sol.bind("super+7", function() sol.status("seven") end)
            sol.bind("super+braceleft", function() sol.status("brace") end)
        "#;
        assert_eq!(
            status_after("altgr-brace", "de", brace, &[SUPER, RIGHT_ALT, DIGIT_7]),
            "brace",
            "a binding on the AltGr symbol still takes the press first"
        );
    }

    /// The names themselves, without a keyboard: modified first, the key-cap
    /// spelling second, and one name when the two agree. In the canonical
    /// order `normalise_combo` gives them, which puts shift before super.
    #[test]
    fn a_press_is_named_under_both_spellings() {
        let shifted = ModifiersState {
            shift: true,
            logo: true,
            ..Default::default()
        };
        assert_eq!(
            combos_for(&shifted, Keysym::exclam, Some(Keysym::_1)),
            ["shift+super+exclam", "shift+super+1"]
        );
        // A letter lowercases back to itself, so there is nothing to add.
        assert_eq!(
            combos_for(&shifted, Keysym::Q, Some(Keysym::q)),
            ["shift+super+q"]
        );
        // A key with no single level-0 keysym has one name, and so does a
        // level 0 with nothing on it.
        assert_eq!(combos_for(&shifted, Keysym::Q, None), ["shift+super+q"]);
        assert_eq!(
            combos_for(&shifted, Keysym::Q, Some(Keysym::NoSymbol)),
            ["shift+super+q"]
        );
    }
}
