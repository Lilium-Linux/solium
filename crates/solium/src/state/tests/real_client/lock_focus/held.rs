use super::*;
use crate::keyboard_change::tests::configured;
use crate::keymap::tests::set_us_ru;
use std::time::Duration;

/// Super, J and Q, as xkb keycodes: evdev 125, 36 and 16, plus
/// eight.
const SUPER: u32 = 133;
const J: u32 = 44;
const Q: u32 = 24;

/// Volume up, counting its runs in `n`, repeating -- and allowed
/// at the lock as well, so that at the lock nothing but the lock
/// itself can be what stops it.
const VOLUME: &str = r#"
    n = 0
    sol.bind("XF86AudioRaiseVolume", function() n = n + 1 end,
        { repeating = true, locked = true })
"#;

fn ms(count: u64) -> Duration {
    Duration::from_millis(count)
}

fn key(session: &mut Session, code: u32, pressed: bool) {
    let state = if pressed {
        KeyState::Pressed
    } else {
        KeyState::Released
    };
    crate::input::key(&mut session.state, Keycode::new(code), state, 0);
}

fn volume_up(session: &mut Session) -> u32 {
    crate::keymap::keycode_of(
        &mut session.state,
        smithay::input::keyboard::Keysym::XF86_AudioRaiseVolume,
    )
    .expect("us,ru has a volume-up key")
    .raw()
}

/// How many times the handler ran.
fn runs(session: &Session) -> String {
    session
        .state
        .scripts
        .as_ref()
        .map(|scripts| scripts.evaluate("return tostring(n)"))
        .unwrap_or_default()
}

/// The keyboard's repeat delay and the time between repeats.
fn timing(session: &Session) -> (Duration, Duration) {
    let delay = u64::try_from(session.state.keyboard.repeat_delay).unwrap_or(600);
    let rate = u64::try_from(session.state.keyboard.repeat_rate).unwrap_or(25);
    (ms(delay), ms(1000 / rate))
}

/// `script` on `us,ru` with Russian active, and two windows with
/// the second focused.
fn two_windows(tag: &str, script: &str) -> (Session, Window, Window) {
    let mut session = Session::new();
    set_us_ru(&mut session.state, 1);
    configured(&mut session.state, tag, script);
    let (first, ..) = session.app.open(&mut session.display, &mut session.state);
    let (second, ..) = session.app.open(&mut session.display, &mut session.state);
    session.state.focus_window(
        &second,
        smithay::utils::SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    assert_eq!(
        session.state.focused_window().as_ref(),
        Some(&second),
        "the premise: the second window has the keyboard"
    );
    (session, first, second)
}

/// **A repeat stops when the keyboard moves to another window**
/// by anything but the binding itself: the held key went down for
/// whatever had the keyboard then.
#[test]
fn a_repeat_stops_when_the_keyboard_moves_to_another_window() {
    let (mut session, first, _) = two_windows("held-focus", VOLUME);
    let volume = volume_up(&mut session);
    key(&mut session, volume, true);
    let start = session.state.clock.now();
    let (delay, interval) = timing(&session);
    session.state.repeat_binding(start + delay + ms(1));
    let held = runs(&session);
    session.state.focus_window(
        &first,
        smithay::utils::SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    session
        .state
        .repeat_binding(start + delay + interval + ms(2));
    key(&mut session, volume, false);
    assert_eq!(
        (held, runs(&session)),
        ("2".into(), "2".into()),
        "(runs while held, runs once another window took the keyboard)"
    );
}

/// **A repeating binding that moves the keyboard itself keeps
/// repeating**: stepping the keyboard from window to window is
/// what holding such a key is for. `super+j` on Russian, where J
/// is `о`.
#[test]
fn a_repeating_binding_that_moves_the_keyboard_itself_keeps_repeating() {
    const STEP: &str = r#"
        n = 0
        sol.bind("super+j", function()
            n = n + 1
            for _, window in ipairs(sol.windows()) do
                if not window.focused then
                    sol.focus(window.id)
                    return
                end
            end
        end, { repeating = true })
    "#;
    let (mut session, first, second) = two_windows("held-step", STEP);
    key(&mut session, SUPER, true);
    key(&mut session, J, true);
    let start = session.state.clock.now();
    let (delay, interval) = timing(&session);
    let stepped = session.state.focused_window();
    session.state.repeat_binding(start + delay + ms(1));
    let back = session.state.focused_window();
    session
        .state
        .repeat_binding(start + delay + interval + ms(2));
    let held = runs(&session);
    key(&mut session, J, false);
    key(&mut session, SUPER, false);
    assert_eq!(
        (stepped, back),
        (Some(first), Some(second)),
        "the premise: the press and the first repeat each moved the keyboard"
    );
    assert_eq!(
        held, "3",
        "the binding's own move of the keyboard stopped its repeat"
    );
}

/// **The lock stops a held binding repeating**, even one allowed
/// at the lock: the key went down at the desktop.
#[test]
fn the_lock_stops_a_held_binding_repeating() {
    let mut session = Session::new();
    set_us_ru(&mut session.state, 1);
    configured(&mut session.state, "held-lock", VOLUME);
    session.app.open(&mut session.display, &mut session.state);
    let volume = volume_up(&mut session);
    key(&mut session, volume, true);
    let start = session.state.clock.now();
    let (delay, interval) = timing(&session);
    session.state.repeat_binding(start + delay + ms(1));
    let held = runs(&session);
    let lock = session.lock();
    session
        .state
        .repeat_binding(start + delay + interval + ms(2));
    session
        .state
        .repeat_binding(start + delay + interval * 2 + ms(3));
    let at_the_lock = runs(&session);
    key(&mut session, volume, false);
    session.assert_unlocks(lock);
    assert_eq!(
        (held, at_the_lock),
        ("2".into(), "2".into()),
        "(runs while held, runs once the session locked under it)"
    );
}

fn volume_down(session: &mut Session) -> u32 {
    crate::keymap::keycode_of(
        &mut session.state,
        smithay::input::keyboard::Keysym::XF86_AudioLowerVolume,
    )
    .expect("us,ru has a volume-down key")
    .raw()
}

/// Press `codes` in order and let them go in reverse, and answer
/// the status the binding that fired left, if one did.
fn tap(session: &mut Session, codes: &[u32]) -> String {
    for &code in codes {
        key(session, code, true);
    }
    for &code in codes.iter().rev() {
        key(session, code, false);
    }
    std::mem::take(&mut session.state.status)
}

/// `script` on `us,ru` with Russian active, one window, and the
/// session locked.
fn locked(tag: &str, script: &str) -> (Session, ext_session_lock_v1::ExtSessionLockV1) {
    let mut session = Session::new();
    set_us_ru(&mut session.state, 1);
    configured(&mut session.state, tag, script);
    session.app.open(&mut session.display, &mut session.state);
    let lock = session.lock();
    (session, lock)
}

/// **A binding allowed while locked fires at the lock screen, and
/// no other does**: not the volume key beside it, and not a letter
/// binding, which is still how a lock screen is typed at. The key
/// the binding took is not sent to the lock screen; the one no
/// binding took is, as before.
#[test]
fn a_binding_allowed_while_locked_fires_at_the_lock_and_no_other_does() {
    const SCRIPT: &str = r#"
        sol.bind("XF86AudioRaiseVolume", function() sol.status("up") end,
            { locked = true })
        sol.bind("XF86AudioLowerVolume", function() sol.status("down") end)
        sol.bind("super+q", function() sol.status("q") end)
    "#;
    let (mut session, lock) = locked("locked-fires", SCRIPT);
    let (up, down) = (volume_up(&mut session), volume_down(&mut session));
    session.locker.client.key_events.clear();
    let fired = (
        tap(&mut session, &[up]),
        tap(&mut session, &[down]),
        tap(&mut session, &[SUPER, Q]),
    );
    session
        .locker
        .pump(&mut session.display, &mut session.state);
    let heard: Vec<u32> = session
        .locker
        .client
        .key_events
        .iter()
        .filter(|(_, pressed)| *pressed)
        .map(|(code, _)| *code + 8)
        .collect();
    session.assert_unlocks(lock);
    assert_eq!(
        fired,
        ("up".into(), String::new(), String::new()),
        "(volume up, allowed; volume down, not; super+q, not), at the lock"
    );
    assert!(
        !heard.contains(&up),
        "the lock screen was sent the key a binding took: {heard:?}"
    );
    assert!(
        heard.contains(&down),
        "the key no binding took at the lock did not reach the lock screen: {heard:?}"
    );
}

/// **A binding allowed while locked cannot act on windows**: what
/// it asks of one is refused, as an error its handler can catch,
/// and what it asks of the keyboard is not. Once the session
/// unlocks, the same binding acts on windows again.
#[test]
fn a_binding_allowed_while_locked_cannot_act_on_windows() {
    const SCRIPT: &str = r#"
        sol.bind("XF86AudioMute", function()
            local window = sol.windows()[1]
            local closed = pcall(sol.close, window and window.id or 1)
            local keyed = pcall(sol.keyboard, { repeat_rate = 30 })
            sol.status(tostring(closed) .. "," .. tostring(keyed))
        end, { locked = true })
    "#;
    let (mut session, lock) = locked("locked-windows", SCRIPT);
    let mute = crate::keymap::keycode_of(
        &mut session.state,
        smithay::input::keyboard::Keysym::XF86_AudioMute,
    )
    .expect("us,ru has a mute key")
    .raw();
    let at_the_lock = tap(&mut session, &[mute]);
    session.assert_unlocks(lock);
    let unlocked = tap(&mut session, &[mute]);
    assert_eq!(
        (at_the_lock, unlocked),
        ("false,true".into(), "true,true".into()),
        "(closing a window, setting the keyboard), at the lock and unlocked"
    );
}

/// **A held binding allowed while locked repeats at the lock
/// screen**: the volume key held there goes on stepping.
#[test]
fn a_held_binding_allowed_while_locked_repeats_at_the_lock() {
    let (mut session, lock) = locked("locked-repeats", VOLUME);
    let volume = volume_up(&mut session);
    key(&mut session, volume, true);
    let start = session.state.clock.now();
    let (delay, interval) = timing(&session);
    session.state.repeat_binding(start + delay + ms(1));
    session
        .state
        .repeat_binding(start + delay + interval + ms(2));
    let held = runs(&session);
    key(&mut session, volume, false);
    session.assert_unlocks(lock);
    assert_eq!(
        held, "3",
        "a held volume key at the lock screen did not repeat"
    );
}
