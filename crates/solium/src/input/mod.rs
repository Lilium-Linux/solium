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
    qml::hosted::{PointerKind, ScenePointer},
    script,
    scripted::KeyPolicy,
    state::{Chrome, GrabRoute, Request, Solium},
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
    /// A key for the scene holding the keyboard.
    /// `tests::while_the_shell_holds_the_keyboard_russian_letters_reach_it_as_cyrillic`.
    Scene(crate::qml::hosted::SceneKey),
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
    // And a screen that is off comes back on, before the event is delivered
    // -- which it then is, as usual. See `Solium::wake_screens`, and
    // `input_wakes_every_screen_the_idle_blank_turned_off`.
    if wakes(&event) {
        state.wake_screens();
    }

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
    // What the event made the scenes say, read in the dispatch that
    // delivered it (Ruling 11):
    // `state::tests::real_client::reflow_on_close::hosted::a_reserve_the_scene_changes_at_a_press_reflows_the_tiled_windows_once_from_that_instant`.
    state.settle_scenes();
}

/// Whether an event is somebody reaching for the machine, which turns every
/// screen that is off back on.
///
/// A press and never a release, as niri has it: a binding that turns the
/// screens off is pressed, and waking on its release would turn them straight
/// back on -- and the Enter that runs `wlopm --off` in a terminal the same.
/// Motion of any kind, a scroll, a touch, a stylus. Not a device arriving or
/// leaving, which is no reason to light a room. Keys are [`key`]'s, since that
/// is where a test presses them.
/// `input_wakes_every_screen_the_idle_blank_turned_off` plays a key, a
/// motion and a button, each released first; nothing reachable from a test
/// sends a touch or a tablet event.
fn wakes<B: InputBackend>(event: &InputEvent<B>) -> bool {
    match event {
        InputEvent::PointerButton { event } => event.state() == ButtonState::Pressed,
        InputEvent::PointerMotion { .. }
        | InputEvent::PointerMotionAbsolute { .. }
        | InputEvent::PointerAxis { .. }
        | InputEvent::TouchDown { .. }
        | InputEvent::TouchMotion { .. }
        | InputEvent::GestureSwipeBegin { .. }
        | InputEvent::GesturePinchBegin { .. }
        | InputEvent::GestureHoldBegin { .. }
        | InputEvent::TabletToolAxis { .. }
        | InputEvent::TabletToolProximity { .. }
        | InputEvent::TabletToolTip { .. }
        | InputEvent::TabletToolButton { .. } => true,
        _ => false,
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
    // Before the filter, so a binding that turns the screens off is pressed
    // with them on and leaves them off. See [`wakes`], and
    // `a_binding_that_turns_the_screens_off_leaves_them_off_when_it_is_let_go`.
    if pressed {
        state.wake_screens();
    }

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
                // The release of a key whose press went to a scene is the
                // scene's, held or not by now, and no window's, which never
                // saw the press.
                // `tests::a_release_follows_its_press_to_the_scene`.
                if state.keys_to_scene.remove(&code.raw()) && !sealed {
                    return FilterResult::Intercept(Some(Action::Scene(scene_key(
                        &handle, modifiers, false,
                    ))));
                }
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
            match result {
                FilterResult::Forward => {
                    state.keys_forwarded.insert(code.raw());
                }
                FilterResult::Intercept(Some(Action::Scene(_))) => {
                    state.keys_to_scene.insert(code.raw());
                }
                FilterResult::Intercept(_) => {}
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
        Some(Some(Action::Scene(key))) => state.deliver_scene_key(key),
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

    // A scene holding the keyboard (primitive 6). The escape hatches and the
    // lock gate are above; then, by its surface's policy, a key the holding
    // item claims is the scene's, then the bindings, and every other key is
    // the scene's. Claims and bindings alike are tried under both of
    // `combos_for`'s names, so `super+q` binds on Russian and a claim of
    // `Escape` holds on any layout. The policy is the surface's as it is
    // declared now, so a reload that changes it applies from the next key.
    // `tests::a_claimed_key_reaches_the_scene_and_not_its_binding`,
    // `tests::an_unclaimed_super_binding_still_fires_on_russian_while_the_shell_holds_the_keyboard`,
    // `tests::with_bindings_all_a_claimed_binding_wins`,
    // `tests::with_bindings_none_even_super_bindings_reach_the_scene`,
    // `state::tests::real_client::reflow_on_close::hosted::a_bindings_policy_redeclared_while_the_shell_holds_the_keyboard_applies_at_once`.
    if let Some(holder) = state.hosted_keyboard.as_ref() {
        let combos = combos_for(
            modifiers,
            handle.modified_sym(),
            handle.raw_latin_sym_or_raw_current_sym(),
        );
        let claimed = combos.iter().any(|combo| {
            holder
                .claims
                .iter()
                .any(|claim| script::normalise_combo(claim) == *combo)
        });
        let bound = || {
            state.scripts.as_ref().and_then(|scripts| {
                combos
                    .iter()
                    .find(|combo| scripts.has_binding(combo))
                    .cloned()
            })
        };
        let policy = state
            .surfaces
            .get(holder.surface)
            .map_or(holder.policy, |surface| surface.declared.keyboard);
        let binding = match policy {
            KeyPolicy::ExceptClaimed if claimed => None,
            KeyPolicy::ExceptClaimed | KeyPolicy::All => bound(),
            KeyPolicy::NoBindings => None,
        };
        return FilterResult::Intercept(Some(match binding {
            Some(combo) => Action::Bound(combo),
            None => Action::Scene(scene_key(&handle, modifiers, true)),
        }));
    }

    // A press answers to two names, tried in order -- see
    // `combos_for` for why both, why this order, and why the second is
    // the key as a Latin layout names it rather than the active one.
    // The same handle and the same xkb lock `modified_sym` takes, which
    // smithay does not hold while this filter runs.
    let raw = handle.raw_latin_sym_or_raw_current_sym();
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

/// A key as the scene holding the keyboard is told it: what it types from
/// the compositor's own xkb state, with the active group, so `ru` types
/// Cyrillic; Qt's name for it; the modifiers held; and its keycode.
/// `tests::while_the_shell_holds_the_keyboard_russian_letters_reach_it_as_cyrillic`,
/// `tests::the_scene_is_told_each_key_as_qt_names_it`.
///
/// With Control held, a key whose symbol is not Latin-1 is named by the
/// Latin-1 letter a Latin layout has on it, as Qt names it itself
/// (`QXkbCommon::keysymToQtKey`), so a field's `ctrl+a`, `ctrl+c` and
/// `ctrl+v` work with Russian active; its text stays the active group's.
/// `tests::with_control_held_a_cyrillic_letter_is_told_by_its_latin_name`,
/// `tests::ctrl_a_selects_all_in_a_hosted_text_field_on_russian`.
fn scene_key(
    handle: &smithay::input::keyboard::KeysymHandle<'_>,
    modifiers: &ModifiersState,
    pressed: bool,
) -> crate::qml::hosted::SceneKey {
    let sym = handle.modified_sym();
    let text = xkb::keysym_to_utf8(sym);
    let named = match handle.raw_latin_sym_or_raw_current_sym() {
        Some(latin) if modifiers.ctrl && sym.raw() > 0xff && latin.raw() <= 0xff => latin,
        _ => sym,
    };
    crate::qml::hosted::SceneKey {
        pressed,
        qt_key: crate::qml::keys::qt_key(named, &xkb::keysym_to_utf8(named)),
        modifiers: crate::qml::keys::qt_modifiers(modifiers),
        text,
        autorepeat: false,
        code: handle.raw_code().raw(),
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
/// `super+shift+exclam` among them. Then `raw`, the key as a Latin layout names
/// it with no modifiers applied, which is the key a person pressed and the name
/// a combination like `super+shift+1` spells. When the two come out the same
/// string, as they do for a letter on a Latin layout and for any key no held
/// modifier moved off level 0, there is one name.
///
/// `raw` is level 0, which applies no modifiers at all, so it sheds AltGr as
/// well as shift: on `de`, super+AltGr+7 is `super+braceleft` and then
/// `super+7`. Accepted rather than filtered out, because `combo_for` names
/// ctrl, alt, shift and super and nothing else -- a binding could only ever
/// tell those two presses apart by the symbol, and the symbol is still tried
/// first. `altgr_falls_back_to_the_key_it_is_held_on` pins both halves.
///
/// "A Latin layout" is smithay's `raw_latin_sym_or_raw_current_sym` (#132):
/// level 0 of the active layout when that keysym is ASCII or no character at
/// all -- a digit, `Return`, an arrow -- and otherwise level 0 of the first
/// *other* layout in the keymap, in group order, that puts a printable ASCII
/// character there. Without it, a `us,ru` keyboard on Russian sends `super+q`
/// as `super+cyrillic_shorti` under both names, and every letter binding is
/// dead until the layout is switched back. Only a non-ASCII key is looked up
/// again, so a key the active layout already spells in ASCII keeps that
/// spelling even where `us` has something else: on Russian the key `us` calls
/// `slash` is `period`, so it fires `super+period` and not `super+slash`. And a
/// binding written in the other alphabet still wins, because `modified` is
/// tried first. `letters_fire_while_russian_is_active` and
/// `a_binding_written_in_cyrillic_still_wins` pin these.
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
        // A scene holding a grab hears the motion wherever it is, and nothing
        // else does
        // (`state::tests::real_client::reflow_on_close::hosted::while_a_grab_is_held_the_pointer_is_the_scenes`).
        // Otherwise scripted surfaces above the windows see the pointer
        // first, so a button on a bar lights up on hover. Then frames, every
        // time, so one a shell's button came over hears the pointer leave it
        // (`state::tests::real_client::reflow_on_close::hosted::a_hovered_frame_hears_the_pointer_leave_onto_a_shell_button_over_it`).
        // Then the ones below, if none above took it.
        let motion = scene_event(state, PointerKind::Motion);
        let above = state.grab_pointer(location, motion)
            || state.surface_pointer(true, location, Some(motion));
        hover_frame(state, location);
        if !above {
            state.surface_pointer(false, location, Some(motion));
        }
        follow_pointer(state, location, pointer.is_grabbed());
        state.finish_scene_motion();
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
            let motion = scene_event(state, PointerKind::Motion);
            let above = state.grab_pointer(location, motion)
                || state.surface_pointer(true, location, Some(motion));
            hover_frame(state, location);
            if !above {
                state.surface_pointer(false, location, Some(motion));
            }
            follow_pointer(state, location, pointer.is_grabbed());
            state.finish_scene_motion();
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
    // While a hosted scene holds a grab, no window's constraint holds the
    // pointer, nor is one granted: not even the one of a window a press
    // still keeps the pointer on, through the grab smithay started for it
    // (Ruling 12).
    // `state::tests::real_client::reflow_on_close::hosted::a_grab_begun_during_a_press_on_a_locked_window_has_the_pointer_after_the_release`.
    if state.hosted_grab.is_some() {
        return (wanted, false);
    }
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

/// A pointer event as a scene is told it: the Qt buttons held now, the
/// keyboard's modifiers, and the time on the compositor's clock, which every
/// backend and every event the compositor makes up itself share.
/// `state::tests::real_client::reflow_on_close::hosted::a_right_press_on_a_scene_reaches_it_as_the_right_button_with_shift_held`,
/// `state::tests::real_client::reflow_on_close::hosted::a_scene_is_told_when_each_event_happened_on_the_compositors_clock`.
pub(crate) fn scene_event(state: &Solium, kind: PointerKind) -> ScenePointer {
    let modifiers = state.seat.get_keyboard().map_or(0, |keyboard| {
        crate::qml::keys::qt_modifiers(&keyboard.modifier_state())
    });
    ScenePointer {
        kind,
        buttons: state.pointer_buttons,
        modifiers,
        time: u64::try_from(state.clock.now().as_millis()).unwrap_or(u64::MAX),
    }
}

/// Focus whatever the pointer is over, if the profile says so.
///
/// Skipped while a grab is running: a window being dragged is under the
/// cursor the whole time, and windows sliding past underneath it are not a
/// request to focus each of them in turn.
///
/// And over a client's layer surface on top of a window, which is what the
/// pointer is on there, as it is for a press (`pointer_button`'s
/// `on_a_client`), and over a hosted scene where it takes a press, or while
/// it holds one (Rulings 7 and 8), or holds a grab (Ruling 12), or holds the
/// keyboard, which a click takes back and the pointer passing does not
/// (Ruling 14).
/// `focus_follows_mouse_does_not_reach_through_a_bar`,
/// `state::tests::real_client::reflow_on_close::hosted::focus_follows_the_mouse_through_a_shell_only_where_it_takes_no_press`,
/// `state::tests::real_client::reflow_on_close::hosted::while_a_grab_is_held_no_window_takes_focus_frame_or_cursor_from_the_pointer`,
/// `state::tests::real_client::reflow_on_close::hosted::the_pointer_crossing_a_window_does_not_end_the_shells_hold`.
pub(crate) fn follow_pointer(state: &mut Solium, location: Point<f64, Logical>, grabbed: bool) {
    if !state.profile.focus_follows_mouse
        || grabbed
        || state.script_grab
        || state.scene_press.is_some()
        || state.hosted_grab.is_some()
        || state.hosted_keyboard.is_some()
        || state.pointed_above(location)
    {
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
/// Called on every motion while the session is unlocked, after a script's
/// surfaces above the windows were offered it, whether or not one took it.
/// Where `Solium::frames_kept_from` says the pointer is on something over the
/// windows, or a scene holds a press, no frame is hovered, and the one that
/// was is told it left
/// (`state::tests::real_client::reflow_on_close::hosted::a_hovered_frame_hears_the_pointer_leave_onto_a_shell_button_over_it`,
/// `state::tests::real_client::reflow_on_close::hosted::the_frames_are_kept_from_the_pointer_only_where_a_shell_takes_a_press`).
fn hover_frame(state: &mut Solium, location: Point<f64, Logical>) {
    // The whole window, not just the frame band: a decoration that reacts to
    // the cursor wants to know where it is while it crosses the client too.
    let under = if state.frames_kept_from(location) {
        None
    } else {
        state.decorated_under(location)
    };

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
    // The buttons held, before the lock's check, so they are right after it.
    // `state::tests::real_client::reflow_on_close::hosted::a_right_press_on_a_scene_reaches_it_as_the_right_button_with_shift_held`,
    // `state::tests::real_client::lock_focus::a_button_let_go_behind_the_lock_is_not_held_after_it`.
    let pressed = button_state == ButtonState::Pressed;
    let qt = crate::qml::keys::qt_button(button);
    if let Some(bit) = qt {
        if pressed {
            state.pointer_buttons |= bit;
        } else {
            state.pointer_buttons &= !bit;
        }
    }
    let scene = qt.map(|bit| {
        scene_event(
            state,
            if pressed {
                PointerKind::Press(bit)
            } else {
                PointerKind::Release(bit)
            },
        )
    });

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
    let forward = ButtonEvent {
        button,
        state: button_state,
        serial,
        time: event.time_msec(),
    };
    if state.lock.is_some() {
        forward_button(state, &pointer, &forward);
        return;
    }

    // A grab a scene holds has the press first, and a press the compositor
    // swallowed has its release swallowed, wherever it lands (Ruling 12).
    // `state::tests::real_client::reflow_on_close::hosted::a_press_outside_a_grab_dismisses_it_and_is_swallowed_by_default`,
    // `state::tests::real_client::reflow_on_close::hosted::a_swallowed_unnamed_press_swallows_its_release_off_the_scene`.
    match state.grab_button(location, button, pressed, scene) {
        GrabRoute::Taken => {
            state.redraw = true;
            return;
        }
        GrabRoute::Passed | GrabRoute::NoGrab => {}
    }

    // A button Qt has no name for, while a scene holds a press, is none of
    // the compositor's to interpret either: the pointer is the scene's until
    // every button is up (Ruling 7), and no scene is told of such a button
    // (Ruling 9), so it goes on to smithay as it is, as it does behind the
    // lock.
    // `state::tests::real_client::reflow_on_close::hosted::a_button_qt_has_no_name_for_during_a_scenes_press_is_not_the_compositors`,
    // `state::tests::real_client::reflow_on_close::hosted::a_grab_started_during_a_scenes_press_leaves_it_the_wheel_and_the_release`.
    if scene.is_none() && state.scene_press.is_some() {
        forward_button(state, &pointer, &forward);
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
    // A scripted surface above the windows sees the press first when it is on
    // top there -- not under a client's surface, nor under a fullscreen
    // window, in `crate::stack`'s order -- and only when nothing is being
    // dragged. A bar, a panel, an overlay: all the same path, and the
    // compositor knows what none of them are for. A press a scene holds goes
    // first, even through a grab smithay started during it (Ruling 7):
    // `state::tests::real_client::reflow_on_close::hosted::a_grab_started_during_a_scenes_press_leaves_it_the_wheel_and_the_release`.
    // A button Qt has no name for is swallowed where a scene takes a press,
    // and the scene is not told of it, and so is its release, wherever it
    // lands:
    // `state::tests::real_client::reflow_on_close::hosted::a_button_qt_has_no_name_for_is_swallowed_where_a_shell_takes_a_press`,
    // `state::tests::real_client::reflow_on_close::hosted::a_swallowed_unnamed_press_swallows_its_release_off_the_scene`.
    if (state.scene_press.is_some() || !pointer.is_grabbed())
        && state.surface_pointer(true, location, scene)
    {
        if scene.is_none() && pressed {
            state.swallowed.insert(button);
        }
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

    // A client's layer surface on top here -- a bar, a launcher, a
    // notification -- is over the windows and their chrome, in
    // `crate::stack`'s order, so the press is the client's: none of the
    // compositor's interpretations below, only the forward at the end. What
    // `Solium::claim_under` tells the pointer, from the same answer.
    let on_a_client = !pointer.is_grabbed() && state.client_above(location);

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
        && !on_a_client
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

    if button_state == ButtonState::Pressed && !pointer.is_grabbed() && !on_a_client {
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
    if !pointer.is_grabbed() && !on_a_client && state.surface_pointer(false, location, scene) {
        if scene.is_none() && pressed {
            state.swallowed.insert(button);
        }
        return;
    }

    forward_button(state, &pointer, &forward);

    // Outside the grab now: the pointer's lock is released, so a script may
    // ask where the pointer is without stopping the compositor.
    if let Some((window, x, y)) = state.pending_drop.take() {
        state.trigger_drop(&window, x, y);
    }
}

/// A button, on to whoever has the pointer, as it is.
fn forward_button(state: &mut Solium, pointer: &PointerHandle<Solium>, event: &ButtonEvent) {
    pointer.button(state, event);
    pointer.frame(state);
    // The compositor draws the cursor, so the cursor moving is the screen
    // changing. Without this the pointer only moved when something else
    // happened to want a frame -- which on a still screen is never.
    state.redraw = true;
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

    // Over a scene, the wheel is the scene's, in Qt's terms (Ruling 9): a
    // notch away from the user is +120, which Wayland calls -1.
    // `state::tests::real_client::reflow_on_close::hosted::the_wheel_over_a_scene_reaches_it`.
    let location = pointer.current_location();
    let continuous = matches!(event.source(), AxisSource::Finger | AxisSource::Continuous);
    let angle = |axis| {
        -event
            .amount_v120(axis)
            .unwrap_or_else(|| event.amount(axis).unwrap_or_default() * 8.0)
            * direction
    };
    let pixels = |axis| {
        if continuous {
            -event.amount(axis).unwrap_or_default() * direction
        } else {
            0.0
        }
    };
    let (angle, pixels) = (
        (angle(Axis::Horizontal), angle(Axis::Vertical)),
        (pixels(Axis::Horizontal), pixels(Axis::Vertical)),
    );
    // A touchpad's scroll ends, as the fingers lift, with one that moves
    // nothing. That is no wheel turn to a scene, whose handlers would read
    // it as one the other way, so no scene is told of it.
    // `state::tests::real_client::reflow_on_close::hosted::a_touchpad_scroll_reaches_a_scene_in_pixels_and_its_end_does_not`.
    let moves = [angle.0, angle.1, pixels.0, pixels.1]
        .iter()
        .any(|delta| *delta != 0.0);
    let wheel = scene_event(state, PointerKind::Wheel { angle, pixels });
    // A scene holding a grab has the wheel wherever the pointer is.
    // `state::tests::real_client::reflow_on_close::hosted::while_a_grab_is_held_the_wheel_is_the_scenes`.
    if moves && state.grab_pointer(location, wheel) {
        return;
    }
    // A press a scene holds has the wheel too, through a grab smithay started
    // during it, as `pointer_button` gives it the buttons:
    // `state::tests::real_client::reflow_on_close::hosted::a_grab_started_during_a_scenes_press_leaves_it_the_wheel_and_the_release`.
    if moves
        && (state.scene_press.is_some() || !pointer.is_grabbed())
        && (state.surface_pointer(true, location, Some(wheel))
            || (pointer.current_focus().is_none()
                && state.surface_pointer(false, location, Some(wheel))))
    {
        return;
    }

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
    let under = state.touch_under(location);
    let serial = SERIAL_COUNTER.next_serial();

    // Not the window under something over the windows: a client's layer
    // surface, or a shell where it takes a press, which the touch did not
    // reach either (`Solium::touch_under`).
    // `state::tests::real_client::reflow_on_close::hosted::a_touch_on_a_shell_button_neither_reaches_nor_focuses_the_window_under_it`.
    if state.profile.touch_to_focus
        && !state.pointed_above(location)
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
    let under = state.touch_under(location);

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
    fn status_after(name: &str, layout: &str, script: &str, keys: &[u32]) -> String {
        with_keyboard(name, layout, 0, script, |state| chord(state, keys))
    }

    /// `run` against a real `Solium` with `script` loaded, the named xkb
    /// `layout` compiled, and its layout group locked to `group` -- zero-based,
    /// as xkb counts, so `1` on `us,ru` is Russian.
    ///
    /// The whole input path the hardware takes -- `keyboard`, smithay's own
    /// xkb state and its filter, `Scripts::has_binding`, `Solium::trigger` --
    /// with nothing standing in for any of it but the key events. The keymap
    /// is set rather than inherited, because `XkbConfig::default()` reads
    /// `XKB_DEFAULT_LAYOUT`, and a test whose answer depends on the layout of
    /// whoever runs it answers nothing. The group is locked the way
    /// `sol.keyboard` locks it, through smithay's own `set_layout`, and read
    /// back before `run` sees it: a lock that quietly did not take would leave
    /// a test about Russian asking `us` instead, and passing.
    fn with_keyboard<T>(
        name: &str,
        layout: &str,
        group: u32,
        script: &str,
        run: impl FnOnce(&mut Solium) -> T,
    ) -> T {
        use smithay::input::keyboard::{Layout, XkbConfig};

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

        let active = keyboard.with_xkb_state(&mut state, |mut context| {
            context.set_layout(Layout(group));
            context
                .xkb()
                .lock()
                .map(|xkb| xkb.active_layout().0)
                .expect("reading the layout group back")
        });
        assert_eq!(
            active, group,
            "locking {layout:?} to group {group} did not take, so this would test \
             another layout"
        );
        run(&mut state)
    }

    /// Presses `keys` in order and lets go in reverse, and hands back the
    /// status the binding that fired left -- empty if none did -- clearing it,
    /// so the next chord on the same `Solium` starts from nothing.
    fn chord(state: &mut Solium, keys: &[u32]) -> String {
        use smithay::backend::input::KeyState;

        for &code in keys {
            super::keyboard(
                state,
                Key {
                    code,
                    state: KeyState::Pressed,
                },
            );
        }
        for &code in keys.iter().rev() {
            super::keyboard(
                state,
                Key {
                    code,
                    state: KeyState::Released,
                },
            );
        }
        std::mem::take(&mut state.status)
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
        // A key with nothing at level 0 has one name, whether that arrives
        // as no keysym or as `NoSymbol`.
        assert_eq!(combos_for(&shifted, Keysym::Q, None), ["shift+super+q"]);
        assert_eq!(
            combos_for(&shifted, Keysym::Q, Some(Keysym::NoSymbol)),
            ["shift+super+q"]
        );
    }

    /// The keys #132 is about, as xkb keycodes: evdev plus eight, the same
    /// physical key on every layout. `SLASH` is the key `us` calls `slash`.
    const RETURN: u32 = 36;
    const G: u32 = 42;
    const M: u32 = 58;
    const S: u32 = 39;
    const T: u32 = 28;
    const D: u32 = 40;
    const K: u32 = 45;
    const DIGIT_3: u32 = 12;
    const DIGIT_9: u32 = 18;
    const BRACKET_RIGHT: u32 = 35;
    const COMMA: u32 = 59;
    const PERIOD: u32 = 60;
    const SLASH: u32 = 61;

    /// Every binding `russian_script` makes, and the keys that press it.
    const RUSSIAN_CHORDS: &[(&str, &[u32])] = &[
        ("super+q", &[SUPER, Q]),
        ("super+return", &[SUPER, RETURN]),
        ("super+g", &[SUPER, G]),
        ("super+m", &[SUPER, M]),
        ("super+s", &[SUPER, S]),
        ("super+t", &[SUPER, T]),
        ("super+shift+q", &[SUPER, SHIFT, Q]),
        ("super+shift+d", &[SUPER, SHIFT, D]),
        ("super+shift+k", &[SUPER, SHIFT, K]),
        ("super+1", &[SUPER, DIGIT_1]),
        ("super+9", &[SUPER, DIGIT_9]),
        ("super+shift+1", &[SUPER, SHIFT, DIGIT_1]),
        ("super+shift+3", &[SUPER, SHIFT, DIGIT_3]),
        ("super+shift+9", &[SUPER, SHIFT, DIGIT_9]),
        ("super+bracketleft", &[SUPER, BRACKET_LEFT]),
        ("super+shift+bracketleft", &[SUPER, SHIFT, BRACKET_LEFT]),
        ("super+shift+bracketright", &[SUPER, SHIFT, BRACKET_RIGHT]),
        ("super+comma", &[SUPER, COMMA]),
        ("super+period", &[SUPER, PERIOD]),
    ];

    /// `RUSSIAN_CHORDS`, each bound to put its own name in the status.
    fn russian_script() -> String {
        RUSSIAN_CHORDS
            .iter()
            .map(|(combo, _)| {
                format!("sol.bind({combo:?}, function() sol.status({combo:?}) end)\n")
            })
            .collect()
    }

    /// **On `us,ru` with Russian active, a binding fires from the key its
    /// Latin name is on.** #132.
    ///
    /// The second name a press answered to was level 0 of the *active* layout
    /// -- the layout `modified` reads too -- so on Russian the Q key was
    /// `cyrillic_shorti` under both names and every letter binding was dead
    /// until the layout was switched back. The letters, `[`, `]`, `,` and `.`
    /// are the keys Russian labels in Cyrillic, and they are what this caught.
    /// The digits are here because Russian leaves them digits and shifts them
    /// to its own symbols -- `numerosign` on 3 -- so they are the keys most
    /// likely to be broken by a fix that got the letters right.
    ///
    /// The same chords on `us` alone and on `us,ru` with `us` active are the
    /// half that must not move, and every failure is reported, not the first.
    #[test]
    fn letters_fire_while_russian_is_active() {
        let script = russian_script();
        let mut dead = Vec::new();
        for (name, layout, group) in [
            ("latin-us", "us", 0),
            ("latin-us-ru", "us,ru", 0),
            ("russian", "us,ru", 1),
        ] {
            with_keyboard(name, layout, group, &script, |state| {
                for (combo, keys) in RUSSIAN_CHORDS {
                    let fired = chord(state, keys);
                    if fired != *combo {
                        dead.push(format!("{layout} group {group}: {combo} fired {fired:?}"));
                    }
                }
            });
        }
        assert!(
            dead.is_empty(),
            "each of these pressed the key its binding names and did not fire \
             it: {dead:#?}"
        );

        // And the edge of it, pinned so nobody reads more into the fix than
        // it does. A key the active layout already spells in ASCII keeps that
        // spelling: Russian puts `period` where `us` has `slash`, so on
        // Russian that key is `super+period` and not `super+slash`.
        let slash = r#"
            sol.bind("super+slash", function() sol.status("slash") end)
            sol.bind("super+period", function() sol.status("period") end)
        "#;
        for (group, expected) in [(0, "slash"), (1, "period")] {
            assert_eq!(
                with_keyboard("slash", "us,ru", group, slash, |state| {
                    chord(state, &[SUPER, SLASH])
                }),
                expected,
                "on `us,ru` group {group}, the key `us` calls slash"
            );
        }
    }

    /// **A binding written in Cyrillic still wins on Russian.**
    ///
    /// `modified` is tried before the Latin name, so somebody who bound the
    /// Russian letter keeps it and the Latin binding on the same key is only
    /// the fallback. The last assertion is the direction the fallback does
    /// not go: on `us`, the Q key is `q` and never `cyrillic_shorti`.
    #[test]
    fn a_binding_written_in_cyrillic_still_wins() {
        let both = r#"
            sol.bind("super+q", function() sol.status("latin") end)
            sol.bind("super+Cyrillic_shorti", function() sol.status("cyrillic") end)
        "#;
        assert_eq!(
            with_keyboard("cyrillic", "us,ru", 1, both, |state| chord(
                state,
                &[SUPER, Q]
            )),
            "cyrillic",
            "on Russian the Q key is `cyrillic_shorti` first, and a binding on \
             that takes the press"
        );
        assert_eq!(
            with_keyboard("cyrillic-latin", "us,ru", 0, both, |state| {
                chord(state, &[SUPER, Q])
            }),
            "latin",
            "on `us` the same key is `q`"
        );
        let cyrillic =
            r#"sol.bind("super+Cyrillic_shorti", function() sol.status("cyrillic") end)"#;
        assert_eq!(
            with_keyboard("cyrillic-only", "us,ru", 1, cyrillic, |state| {
                chord(state, &[SUPER, Q])
            }),
            "cyrillic",
            "a Cyrillic binding alone fires on Russian"
        );
        assert_eq!(
            with_keyboard("cyrillic-only-latin", "us,ru", 0, cyrillic, |state| {
                chord(state, &[SUPER, Q])
            }),
            "",
            "the fallback is to a Latin name, not to every layout's"
        );
    }

    /// **The #150 keys fire on `us,ru` with Russian active, and reach what
    /// they are for.** #132.
    ///
    /// `every_shipped_binding_fires_on_both_groups_of_us_ru` asks whether each
    /// shipped combination fires at all, with a stand-in bound to it. This
    /// keeps the shipped handlers -- `direction.lua`'s and `modes.lua`'s --
    /// and stands in only for what they call, so a key that fires but calls
    /// the wrong thing fails here too. H, J, K, L, F and M are the keys
    /// Russian labels in Cyrillic; the arrows and space are here because they
    /// are the keys the fix for those must not break. And super+shift+k is
    /// left to the keyboard layout: move-up on k is super+alt+k.
    #[test]
    fn the_direction_and_window_keys_fire_while_russian_is_active() {
        const ALT: u32 = 64;
        const H: u32 = 43;
        const J: u32 = 44;
        const L: u32 = 46;
        const F: u32 = 41;
        const SPACE: u32 = 65;
        const UP: u32 = 111;
        const LEFT: u32 = 113;
        const RIGHT: u32 = 114;
        const DOWN: u32 = 116;

        let shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua");
        let script = format!(
            "package.path = {shipped:?} .. \"/?.lua\"\n\
             sol.toggle_fullscreen = function() sol.status(\"fullscreen\") end\n\
             sol.toggle_maximize = function() sol.status(\"maximize\") end\n\
             require(\"modes\")\n\
             require(\"workspaces\")\n\
             require(\"tiling\")\n\
             require(\"scrolling\")\n\
             require(\"direction\")\n\
             require(\"modes\").toggle_floating = function(id) sol.status(\"floating \" .. id) end\n\
             sol.windows = function() return {{ {{ id = 7, focused = true, monitor = \"spy\", x = 0, y = 0, w = 1, h = 1 }} }} end\n\
             sol.on(\"direction\", function(verb, dir) sol.status(verb .. \" \" .. dir) end)\n"
        );
        let chords: &[(&str, &[u32])] = &[
            ("focus left", &[SUPER, LEFT]),
            ("focus right", &[SUPER, RIGHT]),
            ("focus up", &[SUPER, UP]),
            ("focus down", &[SUPER, DOWN]),
            ("focus left", &[SUPER, H]),
            ("focus down", &[SUPER, J]),
            ("focus up", &[SUPER, K]),
            ("focus right", &[SUPER, L]),
            ("move left", &[SUPER, SHIFT, LEFT]),
            ("move right", &[SUPER, SHIFT, RIGHT]),
            ("move up", &[SUPER, SHIFT, UP]),
            ("move down", &[SUPER, SHIFT, DOWN]),
            ("move left", &[SUPER, SHIFT, H]),
            ("move down", &[SUPER, SHIFT, J]),
            ("move up", &[SUPER, ALT, K]),
            ("move right", &[SUPER, SHIFT, L]),
            ("fullscreen", &[SUPER, F]),
            ("maximize", &[SUPER, SHIFT, M]),
            ("floating 7", &[SUPER, SHIFT, SPACE]),
            ("", &[SUPER, SHIFT, K]),
        ];
        let mut wrong = Vec::new();
        for (name, group) in [("windows-latin", 0), ("windows-russian", 1)] {
            with_keyboard(name, "us,ru", group, &script, |state| {
                for (expected, keys) in chords {
                    let fired = chord(state, keys);
                    if fired != *expected {
                        wrong.push(format!(
                            "group {group}: {keys:?} wanted {expected:?}, got {fired:?}"
                        ));
                    }
                }
            });
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    /// **Every shipped binding fires on both halves of `us,ru`.** #132.
    ///
    /// `letters_fire_while_russian_is_active` asks about chords somebody
    /// chose; this asks about the ones the compositor ships, read out of the
    /// real `init.lua`, so a binding added later is asked about too. Each is
    /// pressed on the key whose level 0 on `us` is the key it names, under
    /// the modifiers it names, and must fire with either group active. The
    /// scripts are loaded only for their list of combinations; what is
    /// pressed is a script binding each one to its own name, because the
    /// shipped handlers open terminals.
    #[test]
    fn every_shipped_binding_fires_on_both_groups_of_us_ru() {
        use smithay::input::keyboard::xkb;

        let shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua");
        let directory = std::env::temp_dir().join("solium-reachable-us-ru");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("creating the entry directory");
        let entry = directory.join("init.lua");
        std::fs::write(
            &entry,
            format!(
                "package.path = {shipped:?} .. \"/?.lua\"\ndofile({shipped:?} .. \"/init.lua\")\n"
            ),
        )
        .expect("writing the entry point");
        let bound: Vec<String> = crate::script::Scripts::load(&entry)
            .expect("loading the shipped init.lua")
            .bindings()
            .into_iter()
            .map(|binding| binding.combo)
            .collect();
        for combo in [
            "super+q",
            "super+return",
            "shift+super+1",
            "shift+super+bracketleft",
        ] {
            assert!(
                bound.iter().any(|bound| bound == combo),
                "the shipped scripts no longer bind {combo:?}, so this checks less than \
                 it says; they bind {bound:?}"
            );
        }

        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let us = xkb::Keymap::new_from_names(
            &context,
            "",
            "",
            "us",
            "",
            None,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .expect("no `us` keymap; xkb data is missing, so this proves nothing");
        let key_on_us = |name: &str| {
            (us.min_keycode().raw()..=us.max_keycode().raw()).find(|&code| {
                us.key_get_syms_by_level(xkb::Keycode::new(code), 0, 0)
                    .first()
                    .is_some_and(|sym| xkb::keysym_get_name(*sym).eq_ignore_ascii_case(name))
            })
        };

        let mut chords = Vec::new();
        for combo in &bound {
            let names: Vec<&str> = combo.split('+').collect();
            let Some((key, held)) = names.split_last() else {
                continue;
            };
            let mut keys: Vec<u32> = held
                .iter()
                .map(|modifier| match *modifier {
                    "ctrl" => 37,
                    "alt" => 64,
                    "shift" => SHIFT,
                    "super" => SUPER,
                    other => panic!("the shipped {combo:?} holds {other:?}, which is no modifier"),
                })
                .collect();
            keys.push(key_on_us(key).unwrap_or_else(|| {
                panic!("the shipped {combo:?} names {key:?}, which is on no `us` key")
            }));
            chords.push((combo.clone(), keys));
        }
        let script: String = bound
            .iter()
            .map(|combo| format!("sol.bind({combo:?}, function() sol.status({combo:?}) end)\n"))
            .collect();

        let mut dead = Vec::new();
        for group in [0, 1] {
            with_keyboard(
                &format!("shipped-{group}"),
                "us,ru",
                group,
                &script,
                |state| {
                    for (combo, keys) in &chords {
                        let fired = chord(state, keys);
                        if fired != *combo {
                            dead.push(format!("group {group}: {combo} fired {fired:?}"));
                        }
                    }
                },
            );
        }
        assert!(
            dead.is_empty(),
            "shipped bindings that do not fire from their own key on `us,ru`: {dead:#?}"
        );
    }

    use crate::scripted::KeyPolicy;
    use smithay::backend::input::KeyState;

    const ESCAPE: u32 = 9;
    const BACKSPACE: u32 = 22;
    const CTRL: u32 = 37;
    const ALT_L: u32 = 64;
    const W: u32 = 25;

    /// A scene holding the keyboard, with no scene behind it: what reaches it
    /// is `Solium::scene_keys`.
    fn holding(state: &mut Solium, claims: &[&str], policy: KeyPolicy) {
        use smithay::output::{Output, PhysicalProperties, Subpixel};
        let output = Output::new(
            "held-1".to_owned(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "solium".to_owned(),
                model: "held".to_owned(),
            },
        );
        state.hosted_keyboard = Some(crate::state::HostedKeyboard {
            surface: crate::scripted::SurfaceId::from_raw(0),
            output,
            claims: claims.iter().map(|claim| (*claim).to_owned()).collect(),
            policy,
            returns_to: None,
        });
    }

    /// The text of every press the scene was told, in order, clearing them.
    fn typed(state: &mut Solium) -> Vec<String> {
        state
            .scene_keys
            .drain(..)
            .filter(|key| key.pressed && !key.text.is_empty())
            .map(|key| key.text)
            .collect()
    }

    /// **#132: while the shell holds the keyboard, Russian letters reach it as
    /// Cyrillic**, and Latin ones as Latin, shifted ones in capitals.
    #[test]
    fn while_the_shell_holds_the_keyboard_russian_letters_reach_it_as_cyrillic() {
        for (group, expected) in [(0, ["q", "W"]), (1, ["й", "Ц"])] {
            with_keyboard(&format!("held-text-{group}"), "us,ru", group, "", |state| {
                holding(state, &[], KeyPolicy::ExceptClaimed);
                chord(state, &[Q]);
                chord(state, &[SHIFT, W]);
                assert_eq!(typed(state), expected.map(str::to_owned), "group {group}");
            });
        }
    }

    /// **The scene is told the key as Qt names it, with the modifiers held**:
    /// `й` is Qt's `Й`, and shift is Qt's shift.
    #[test]
    fn the_scene_is_told_each_key_as_qt_names_it() {
        with_keyboard("held-qt-key", "us,ru", 1, "", |state| {
            holding(state, &[], KeyPolicy::ExceptClaimed);
            chord(state, &[SHIFT, Q]);
            let pressed: Vec<(i32, u32, u32)> = state
                .scene_keys
                .iter()
                .filter(|key| key.pressed)
                .map(|key| (key.qt_key, key.modifiers, key.code))
                .collect();
            assert_eq!(
                pressed,
                [
                    (0x0100_0020, crate::qml::keys::QT_SHIFT, SHIFT),
                    (0x0419, crate::qml::keys::QT_SHIFT, Q),
                ],
                "(the Qt key, the Qt modifiers, the xkb keycode) of shift and of Q"
            );
        });
    }

    /// **A key the holder claims is the scene's, not its binding's**, on the
    /// Cyrillic group too, matched however the scene spelled it.
    #[test]
    fn a_claimed_key_reaches_the_scene_and_not_its_binding() {
        let script = r#"sol.bind("Escape", function() sol.status("bound") end)"#;
        with_keyboard("held-claimed", "us,ru", 1, script, |state| {
            holding(state, &["Escape"], KeyPolicy::ExceptClaimed);
            assert_eq!(
                chord(state, &[ESCAPE]),
                "",
                "the binding took a claimed key"
            );
            assert!(
                state
                    .scene_keys
                    .iter()
                    .any(|key| key.pressed && key.qt_key == 0x0100_0000),
                "Escape did not reach the scene"
            );
        });
    }

    /// **An unclaimed binding still fires while the shell holds the
    /// keyboard**, and the key is not typed into the scene as well.
    #[test]
    fn an_unclaimed_binding_still_fires_while_the_shell_holds_the_keyboard() {
        let script = r#"sol.bind("Escape", function() sol.status("bound") end)"#;
        with_keyboard("held-unclaimed", "us,ru", 1, script, |state| {
            holding(state, &["Return"], KeyPolicy::ExceptClaimed);
            assert_eq!(chord(state, &[ESCAPE]), "bound");
            assert!(
                !state.scene_keys.iter().any(|key| key.code == ESCAPE),
                "the bound key reached the scene as well"
            );
        });
    }

    /// **An unclaimed `super` binding still fires on Russian while the shell
    /// holds the keyboard**, by its Latin name (#132).
    #[test]
    fn an_unclaimed_super_binding_still_fires_on_russian_while_the_shell_holds_the_keyboard() {
        let script = r#"sol.bind("super+q", function() sol.status("super+q") end)"#;
        with_keyboard("held-super", "us,ru", 1, script, |state| {
            holding(state, &["Escape"], KeyPolicy::ExceptClaimed);
            assert_eq!(chord(state, &[SUPER, Q]), "super+q");
            assert!(
                !typed(state).contains(&"й".to_owned()),
                "the bound key was typed into the scene as well"
            );
        });
    }

    /// **A key nobody bound goes to the scene**, and not to the window that
    /// had the keyboard, which a mode would swallow it from otherwise.
    #[test]
    fn a_key_nobody_bound_goes_to_the_scene() {
        with_keyboard("held-unbound", "us,ru", 1, "", |state| {
            holding(state, &["Escape"], KeyPolicy::ExceptClaimed);
            chord(state, &[Q]);
            assert!(
                state.keys_forwarded.is_empty() && !state.scene_keys.is_empty(),
                "(forwarded {:?}, told the scene {:?})",
                state.keys_forwarded,
                state.scene_keys
            );
        });
    }

    /// **With `bindings = "all"`, a claimed key's binding wins.**
    #[test]
    fn with_bindings_all_a_claimed_binding_wins() {
        let script = r#"sol.bind("Escape", function() sol.status("bound") end)"#;
        with_keyboard("held-all", "us,ru", 1, script, |state| {
            holding(state, &["Escape"], KeyPolicy::All);
            assert_eq!(chord(state, &[ESCAPE]), "bound");
        });
    }

    /// **With `bindings = "none"`, even a `super` binding's key reaches the
    /// scene.**
    #[test]
    fn with_bindings_none_even_super_bindings_reach_the_scene() {
        let script = r#"sol.bind("super+q", function() sol.status("super+q") end)"#;
        with_keyboard("held-none", "us,ru", 1, script, |state| {
            holding(state, &[], KeyPolicy::NoBindings);
            assert_eq!(chord(state, &[SUPER, Q]), "");
            assert!(
                state
                    .scene_keys
                    .iter()
                    .any(|key| key.pressed && key.code == Q),
                "the key did not reach the scene"
            );
        });
    }

    /// **The escape hatches beat a shell that holds the keyboard**: they are
    /// the only keys that must work when everything else is broken, claimed
    /// or not, whatever the policy.
    #[test]
    fn the_escape_hatches_beat_a_shell_that_holds_the_keyboard() {
        with_keyboard("held-escape-hatch", "us,ru", 1, "", |state| {
            holding(state, &["ctrl+alt+BackSpace"], KeyPolicy::NoBindings);
            let _ = chord(state, &[CTRL, ALT_L, BACKSPACE]);
            assert!(
                matches!(state.request, Some(Request::Quit)),
                "ctrl+alt+BackSpace did not reach the compositor"
            );
        });
    }

    /// **A release follows its press to the scene**, even once the hold is
    /// over, so no window hears the release of a key it never saw pressed.
    #[test]
    fn a_release_follows_its_press_to_the_scene() {
        with_keyboard("held-release", "us,ru", 1, "", |state| {
            holding(state, &[], KeyPolicy::ExceptClaimed);
            super::keyboard(
                state,
                Key {
                    code: Q,
                    state: KeyState::Pressed,
                },
            );
            state.hosted_keyboard = None;
            super::keyboard(
                state,
                Key {
                    code: Q,
                    state: KeyState::Released,
                },
            );
            assert!(
                state
                    .scene_keys
                    .iter()
                    .any(|key| !key.pressed && key.code == Q),
                "the release went elsewhere"
            );
            assert!(
                state.keys_to_scene.is_empty(),
                "the release was not counted as the scene's"
            );
        });
    }

    /// **A key held for the scene repeats at the keyboard's rate** (Ruling 14):
    /// not before the delay, once after it, again an interval later, and not
    /// once it is let go.
    #[test]
    fn a_held_key_repeats_into_the_scene_at_the_keyboards_rate() {
        with_keyboard("held-repeat", "us,ru", 1, "", |state| {
            holding(state, &[], KeyPolicy::ExceptClaimed);
            super::keyboard(
                state,
                Key {
                    code: Q,
                    state: KeyState::Pressed,
                },
            );
            let start = state.clock.now();
            let delay = std::time::Duration::from_millis(
                u64::try_from(state.keyboard.repeat_delay).unwrap_or(600),
            );
            let interval = std::time::Duration::from_millis(
                1000 / u64::try_from(state.keyboard.repeat_rate).unwrap_or(25),
            );
            let repeats = |state: &Solium| {
                state
                    .scene_keys
                    .iter()
                    .filter(|key| key.autorepeat && key.text == "й")
                    .count()
            };
            state.repeat_scene_key(start + delay / 2);
            let before = repeats(state);
            state.repeat_scene_key(start + delay + std::time::Duration::from_millis(1));
            state.repeat_scene_key(start + delay + interval + std::time::Duration::from_millis(2));
            let held = repeats(state);
            super::keyboard(
                state,
                Key {
                    code: Q,
                    state: KeyState::Released,
                },
            );
            state.repeat_scene_key(start + delay + interval * 3);
            assert_eq!(
                (before, held, repeats(state)),
                (0, 2, 2),
                "(repeats before the delay, after it and an interval on, after the release)"
            );
        });
    }

    /// **A held key keeps the keyboard's rate though the loop notices it
    /// late**: each repeat is due an interval after the last was due, not
    /// after the loop got to it, so the up to 16 ms a loop sleeps between
    /// looks does not slow it down (Ruling 14).
    #[test]
    fn a_held_key_noticed_late_still_repeats_at_the_keyboards_rate() {
        with_keyboard("held-repeat-late", "us,ru", 1, "", |state| {
            holding(state, &[], KeyPolicy::ExceptClaimed);
            super::keyboard(
                state,
                Key {
                    code: Q,
                    state: KeyState::Pressed,
                },
            );
            let due = state.clock.now()
                + std::time::Duration::from_millis(
                    u64::try_from(state.keyboard.repeat_delay).unwrap_or(600),
                );
            let interval = std::time::Duration::from_millis(
                1000 / u64::try_from(state.keyboard.repeat_rate).unwrap_or(25),
            );
            state.repeat_scene_key(due + std::time::Duration::from_millis(15));
            state.repeat_scene_key(due + interval + std::time::Duration::from_millis(1));
            assert_eq!(
                state
                    .scene_keys
                    .iter()
                    .filter(|key| key.autorepeat && key.text == "й")
                    .count(),
                2,
                "repeats, looked for 15 ms late and then an interval after the first was due"
            );
        });
    }

    /// **A held modifier does not repeat into the scene**, and a letter
    /// pressed after it does, with the modifier still down.
    #[test]
    fn a_held_modifier_does_not_repeat_into_the_scene() {
        with_keyboard("held-modifier", "us,ru", 1, "", |state| {
            holding(state, &[], KeyPolicy::ExceptClaimed);
            super::keyboard(
                state,
                Key {
                    code: SHIFT,
                    state: KeyState::Pressed,
                },
            );
            let start = state.clock.now();
            let late = start + std::time::Duration::from_secs(5);
            state.repeat_scene_key(late);
            let alone = state.scene_keys.iter().filter(|key| key.autorepeat).count();
            super::keyboard(
                state,
                Key {
                    code: Q,
                    state: KeyState::Pressed,
                },
            );
            state.repeat_scene_key(late + std::time::Duration::from_secs(5));
            let repeated: Vec<String> = state
                .scene_keys
                .iter()
                .filter(|key| key.autorepeat)
                .map(|key| key.text.clone())
                .collect();
            assert_eq!(
                (alone, repeated),
                (0, vec!["Й".to_owned()]),
                "(repeats of shift alone, what repeated once a letter was held with it)"
            );
        });
    }

    /// **#132, through the compositor: typing on Russian reaches a hosted
    /// `TextField` as Cyrillic.** A real scene on a monitor, its field
    /// focused and wanting the keyboard, takes it at the settle, and the keys
    /// pressed through the real filter on `us,ru` with Russian active type
    /// `й` and, with shift, `Ц` into it.
    #[test]
    fn russian_typed_through_the_compositor_reaches_a_hosted_text_field() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
            let directory = std::env::temp_dir().join("solium-keys-held-field-scene");
            let _ = std::fs::remove_dir_all(&directory);
            std::fs::create_dir_all(&directory).expect("a directory for the scene");
            let path = directory.join("Search.qml");
            std::fs::write(
                &path,
                r#"
                import QtQuick
                import QtQuick.Controls
                import Solium
                Item {
                    readonly property string typed: field.text
                    TextField {
                        id: field
                        width: 200; height: 30
                        focus: true
                        Solium.keyboard.wants: activeFocus
                        Solium.keyboard.claims: [ "Escape" ]
                    }
                }
                "#,
            )
            .expect("writing the scene");
            let typed = with_keyboard("held-field", "us,ru", 1, "", |state| {
                let output = Output::new(
                    "held-field-1".to_owned(),
                    PhysicalProperties {
                        size: (0, 0).into(),
                        subpixel: Subpixel::Unknown,
                        make: "solium".to_owned(),
                        model: "held-field".to_owned(),
                    },
                );
                output.change_current_state(
                    Some(Mode {
                        size: (640, 480).into(),
                        refresh: 60_000,
                    }),
                    None,
                    Some(Scale::Fractional(1.0)),
                    None,
                );
                state.space.map_output(&output, (0, 0));
                state.declare_surface(crate::scripted::Declaration::for_test(
                    "search",
                    path.clone(),
                    crate::scripted::Layer::Top,
                    crate::scripted::On::EveryMonitor,
                ));
                state.settle_scenes();
                assert!(
                    state.hosted_keyboard.is_some(),
                    "the focused field did not take the keyboard"
                );
                chord(state, &[Q]);
                chord(state, &[SHIFT, W]);
                let id = state.surfaces.named("search").expect("declared");
                state
                    .surfaces
                    .get_mut(id)
                    .and_then(|surface| surface.instance_mut(&output))
                    .map(|instance| instance.scene_for_test().get_string_for_test("typed"))
            });
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(typed.as_deref(), Some("йЦ"));
        });
    }

    const A: u32 = 38;
    const E: u32 = 26;

    /// **With Control held, a letter of a layout that is not Latin is told
    /// by its Latin name**, as Qt itself names it (`QXkbCommon::keysymToQtKey`),
    /// so a field's `ctrl+a`, `ctrl+c` and `ctrl+v` work with Russian active
    /// (#132); what the key types is still the Cyrillic letter.
    #[test]
    fn with_control_held_a_cyrillic_letter_is_told_by_its_latin_name() {
        with_keyboard("held-ctrl", "us,ru", 1, "", |state| {
            holding(state, &[], KeyPolicy::ExceptClaimed);
            chord(state, &[CTRL, A]);
            let pressed: Vec<(i32, u32, String)> = state
                .scene_keys
                .iter()
                .filter(|key| key.pressed && key.code == A)
                .map(|key| (key.qt_key, key.modifiers, key.text.clone()))
                .collect();
            assert_eq!(
                pressed,
                [(0x41, crate::qml::keys::QT_CONTROL, "ф".to_owned())],
                "(the Qt key, the Qt modifiers, the text) of ctrl and the key that is A on us"
            );
        });
    }

    /// **#132, through the compositor: `ctrl+a` selects all in a hosted
    /// `TextField` with Russian active**, so what is typed next replaces it.
    #[test]
    fn ctrl_a_selects_all_in_a_hosted_text_field_on_russian() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
            let directory = std::env::temp_dir().join("solium-keys-ctrl-field-scene");
            let _ = std::fs::remove_dir_all(&directory);
            std::fs::create_dir_all(&directory).expect("a directory for the scene");
            let path = directory.join("Search.qml");
            std::fs::write(
                &path,
                r#"
                import QtQuick
                import QtQuick.Controls
                import Solium
                Item {
                    readonly property string typed: field.text
                    TextField {
                        id: field
                        width: 200; height: 30
                        focus: true
                        Solium.keyboard.wants: activeFocus
                    }
                }
                "#,
            )
            .expect("writing the scene");
            let typed = with_keyboard("ctrl-field", "us,ru", 1, "", |state| {
                let output = Output::new(
                    "ctrl-field-1".to_owned(),
                    PhysicalProperties {
                        size: (0, 0).into(),
                        subpixel: Subpixel::Unknown,
                        make: "solium".to_owned(),
                        model: "ctrl-field".to_owned(),
                    },
                );
                output.change_current_state(
                    Some(Mode {
                        size: (640, 480).into(),
                        refresh: 60_000,
                    }),
                    None,
                    Some(Scale::Fractional(1.0)),
                    None,
                );
                state.space.map_output(&output, (0, 0));
                state.declare_surface(crate::scripted::Declaration::for_test(
                    "search",
                    path.clone(),
                    crate::scripted::Layer::Top,
                    crate::scripted::On::EveryMonitor,
                ));
                state.settle_scenes();
                chord(state, &[Q]);
                chord(state, &[W]);
                chord(state, &[CTRL, A]);
                chord(state, &[E]);
                let id = state.surfaces.named("search").expect("declared");
                state
                    .surfaces
                    .get_mut(id)
                    .and_then(|surface| surface.instance_mut(&output))
                    .map(|instance| instance.scene_for_test().get_string_for_test("typed"))
            });
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                typed.as_deref(),
                Some("у"),
                "йц, then ctrl+a, then у: the selection replaced"
            );
        });
    }
}
