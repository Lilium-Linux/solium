//! **#224: a held binding, and the session around it.** The key
//! filter's half is `input::tests::held`; this is the half that needs
//! windows and a lock client. Every keyboard is `us,ru` with Russian
//! active.
//! #58: a window that asks for the compositor's shortcuts -- a VM
//! viewer, a remote desktop -- has them while it has the keyboard.
//! On `us,ru` with Russian active throughout, where `super+q` arrives
//! as `super+Cyrillic_shorti`.

use super::*;
use crate::keyboard_change::tests::configured;
use crate::keymap::tests::set_us_ru;

/// Super, Q, Escape, Ctrl, Alt and F2, as xkb keycodes.
const SUPER: u32 = 133;
const Q: u32 = 24;
const ESCAPE: u32 = 9;
const CTRL: u32 = 37;
const ALT: u32 = 64;
const F2: u32 = 68;

/// A binding that every window gives way to, and the shipped
/// take-back binding.
const SCRIPT: &str = r#"
    sol.bind("super+q", function() sol.status("q") end)
    sol.bind("super+Escape", function() sol.take_shortcuts_back() end,
        { inhibited = true })
"#;

fn key(session: &mut Session, code: u32, pressed: bool) {
    let state = if pressed {
        KeyState::Pressed
    } else {
        KeyState::Released
    };
    crate::input::key(&mut session.state, Keycode::new(code), state, 0);
}

/// Press `codes` in order and let them go in reverse; the status a
/// binding left, if one fired.
fn tap(session: &mut Session, codes: &[u32]) -> String {
    for &code in codes {
        key(session, code, true);
    }
    for &code in codes.iter().rev() {
        key(session, code, false);
    }
    std::mem::take(&mut session.state.status)
}

/// The keys the application was sent pressed since the last call,
/// as xkb keycodes.
fn heard(session: &mut Session) -> Vec<u32> {
    session.app.pump(&mut session.display, &mut session.state);
    session
        .app
        .client
        .key_events
        .drain(..)
        .filter(|(_, pressed)| *pressed)
        .map(|(code, _)| code + 8)
        .collect()
}

/// What the application's inhibitor was last told: active or not.
fn held(session: &mut Session) -> Option<bool> {
    session.app.pump(&mut session.display, &mut session.state);
    session.app.client.shortcuts_held.last().copied()
}

/// Two windows on `us,ru` with Russian active, the second focused
/// and asking for the shortcuts.
fn asking(tag: &str) -> (Session, Window, Window) {
    let mut session = Session::new();
    set_us_ru(&mut session.state, 1);
    configured(&mut session.state, tag, SCRIPT);
    let (first, ..) = session.app.open(&mut session.display, &mut session.state);
    let (second, _, surface, _) = session.app.open(&mut session.display, &mut session.state);
    focus(&mut session, &second);
    let manager = session
        .app
        .client
        .shortcuts
        .clone()
        .expect("zwp_keyboard_shortcuts_inhibit_manager_v1 is advertised");
    let seat = session.app.client.seat.clone().expect("wl_seat bound");
    let inhibitor = manager.inhibit_shortcuts(&surface, &seat, &session.app.qh, ());
    session.app.client.shortcuts_request = Some((inhibitor, surface));
    session.app.pump(&mut session.display, &mut session.state);
    let _ = heard(&mut session);
    (session, first, second)
}

fn focus(session: &mut Session, window: &Window) {
    session.state.focus_window(
        window,
        smithay::utils::SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
}

#[test]
fn configured_escape_still_runs_an_explicit_inhibited_binding() {
    let (mut session, ..) = asking("inhibit-escape-binding");
    configured(
        &mut session.state,
        "inhibit-escape-binding-config",
        "sol.bind('super+Escape', function() sol.status('reclaimed') end, {inhibited=true})",
    );
    assert_eq!(tap(&mut session, &[SUPER, ESCAPE]), "reclaimed");
    assert_eq!(held(&mut session), Some(false));
}

#[test]
fn replacing_a_request_does_not_undo_reclaim_and_destruction_clears_state() {
    let (mut session, first, second) = asking("inhibit-lifetime");
    let _ = tap(&mut session, &[SUPER, ESCAPE]);
    assert_eq!(held(&mut session), Some(false));
    let (inhibitor, surface) = session
        .app
        .client
        .shortcuts_request
        .take()
        .expect("request");
    inhibitor.destroy();
    session.app.pump(&mut session.display, &mut session.state);
    let manager = session.app.client.shortcuts.clone().expect("manager");
    let seat = session.app.client.seat.clone().expect("seat");
    let request = manager.inhibit_shortcuts(&surface, &seat, &session.app.qh, ());
    session.app.pump(&mut session.display, &mut session.state);
    assert!(
        !session.state.inhibitors.active(),
        "new request evaded reclaim"
    );
    focus(&mut session, &first);
    focus(&mut session, &second);
    assert!(session.state.inhibitors.active());
    request.destroy();
    session.app.pump(&mut session.display, &mut session.state);
    assert!(
        !session.state.inhibitors.active(),
        "destroyed request stayed active"
    );
}

#[test]
fn ordinary_wayland_clients_cannot_see_the_xwayland_grab_global() {
    let session = Session::new();
    assert!(!session.app.client.xwayland_grab_global);
}

#[test]
fn configured_escape_works_without_a_script_binding_in_both_layouts() {
    for group in 0..2 {
        let (mut session, first, second) = asking(&format!("inhibit-escape-{group}"));
        set_us_ru(&mut session.state, group);
        configured(
            &mut session.state,
            &format!("inhibit-escape-config-{group}"),
            &format!("{SCRIPT}\nsol.input{{ shortcuts_inhibit = {{ escape = 'super+x' }} }}"),
        );
        assert_eq!(tap(&mut session, &[SUPER, 53]), "");
        assert!(
            !heard(&mut session).contains(&53),
            "escape key went to client"
        );
        assert_eq!(held(&mut session), Some(false));
        // Reconfiguring cannot restore a request explicitly taken back.
        configured(
            &mut session.state,
            &format!("inhibit-escape-reload-{group}"),
            SCRIPT,
        );
        assert_eq!(held(&mut session), Some(false));
        focus(&mut session, &first);
        focus(&mut session, &second);
        assert_eq!(held(&mut session), Some(true));
    }
}

#[test]
fn inhibited_alt_tab_reaches_the_client_without_its_binding_or_warp() {
    let (mut session, ..) = asking("inhibit-alt-tab");
    configured(
        &mut session.state,
        "inhibit-alt-tab-config",
        &format!(
            "{SCRIPT}\nsol.bind('alt+Tab', function() sol.status('tab'); sol.warp{{ x = 500, y = 400 }} end)"
        ),
    );
    for group in 0..2 {
        set_us_ru(&mut session.state, group);
        let before = session
            .state
            .seat
            .get_pointer()
            .expect("pointer")
            .current_location();
        assert_eq!(tap(&mut session, &[ALT, 23]), "");
        assert!(heard(&mut session).contains(&23));
        assert_eq!(
            session
                .state
                .seat
                .get_pointer()
                .expect("pointer")
                .current_location(),
            before
        );
    }
}

#[test]
fn lua_keyboard_reports_the_same_owner_as_the_protocol() {
    let (mut session, first, second) = asking("inhibit-lua-owner");
    configured(
        &mut session.state,
        "inhibit-lua-owner-config",
        &format!(
            r#"{SCRIPT}
        sol.bind('super+i', function()
            local keyboard = sol.keyboard()
            sol.status(tostring(keyboard.shortcuts_inhibited) .. ':' .. tostring(keyboard.shortcuts_owner))
        end, {{inhibited=true}})
    "#
        ),
    );
    assert_eq!(
        tap(&mut session, &[SUPER, 31]),
        format!("true:{}", session.state.window_id(&second))
    );
    focus(&mut session, &first);
    assert_eq!(tap(&mut session, &[SUPER, 31]), "false:nil");
}

#[test]
fn keyboard_model_reports_inhibition_and_owner_then_clears_them() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        let (directory, mut scene) = crate::qml::hosted::tests::hosted(
            "solium-models-inhibit",
            r#"
            import QtQuick
            import Solium
            Item {
                readonly property int active: Keyboard.shortcutsInhibited ? 1 : 0
                readonly property int owner: Keyboard.shortcutsOwner
                property int changes: 0
                Connections {
                    target: Keyboard
                    function onShortcutsChanged() { changes++ }
                }
            }
            "#,
            "inhibit-model-1",
        );
        let (mut session, first, second) = asking("inhibit-model");
        session.state.publish_models();
        assert_eq!(scene.get_int("active"), 1);
        assert_eq!(
            u64::try_from(scene.get_int("owner")).ok(),
            Some(session.state.window_id(&second))
        );
        focus(&mut session, &first);
        session.state.publish_models();
        assert_eq!(scene.get_int("active"), 0);
        assert_eq!(scene.get_int("owner"), 0);
        assert_eq!(scene.get_int("changes"), 2);
        session.state.publish_models();
        assert_eq!(
            scene.get_int("changes"),
            2,
            "unchanged state was notified again"
        );
        focus(&mut session, &second);
        let lock = session.lock();
        session.state.publish_models();
        assert_eq!(scene.get_int("active"), 0);
        assert_eq!(scene.get_int("owner"), 0);
        session.assert_unlocks(lock);
        session.state.publish_models();
        assert_eq!(scene.get_int("active"), 1);
        drop(scene);
        let _ = std::fs::remove_dir_all(directory);
    });
}

#[test]
fn policy_reload_revokes_and_restores_a_focused_request() {
    let (mut session, ..) = asking("inhibit-policy");
    configured(
        &mut session.state,
        "inhibit-denied",
        &format!("{SCRIPT}\nsol.input{{ shortcuts_inhibit = {{ allow = false }} }}"),
    );
    assert_eq!(held(&mut session), Some(false));
    assert_eq!(tap(&mut session, &[SUPER, Q]), "q");
    configured(
        &mut session.state,
        "inhibit-allowed",
        &format!("{SCRIPT}\nsol.input{{ shortcuts_inhibit = {{ allow = true }} }}"),
    );
    assert_eq!(held(&mut session), Some(true));
    assert_eq!(tap(&mut session, &[SUPER, Q]), "");
    assert!(heard(&mut session).contains(&Q));
}

#[test]
fn fullscreen_only_tracks_fullscreen_without_a_focus_change() {
    let (mut session, _, second) = asking("inhibit-fullscreen");
    configured(
        &mut session.state,
        "inhibit-fullscreen-policy",
        &format!("{SCRIPT}\nsol.input{{ shortcuts_inhibit = {{ allow = 'fullscreen' }} }}"),
    );
    assert_eq!(held(&mut session), Some(false));
    session.state.fullscreen(&second, None);
    assert_eq!(held(&mut session), Some(true));
    assert_eq!(tap(&mut session, &[SUPER, Q]), "");
    session.state.unfullscreen(&second);
    assert_eq!(held(&mut session), Some(false));
    assert_eq!(tap(&mut session, &[SUPER, Q]), "q");
}

#[test]
fn clearing_focus_makes_the_request_inactive() {
    let (mut session, ..) = asking("inhibit-no-focus");
    assert_eq!(held(&mut session), Some(true));
    session
        .state
        .clear_focus(smithay::utils::SERIAL_COUNTER.next_serial());
    assert_eq!(held(&mut session), Some(false));
}

#[test]
fn deny_matches_app_id_and_rechecks_when_it_changes() {
    let (mut session, _, second) = asking("inhibit-app-deny");
    configured(
        &mut session.state,
        "inhibit-app-policy",
        &format!(
            "{SCRIPT}\nsol.input{{ shortcuts_inhibit = {{ apps = {{ deny = {{ 'blocked*' }} }} }} }}"
        ),
    );
    assert_eq!(held(&mut session), Some(true));
    // The client changes its identity while retaining focus.
    let surface = second.toplevel().expect("xdg window").clone();
    smithay::wayland::compositor::with_states(surface.wl_surface(), |states| {
        states
            .data_map
            .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
            .expect("toplevel data")
            .lock()
            .expect("data lock")
            .app_id = Some("blocked.viewer".into());
    });
    smithay::wayland::shell::xdg::XdgShellHandler::app_id_changed(&mut session.state, surface);
    assert_eq!(held(&mut session), Some(false));
}

/// **The window with the keyboard gets `super`, and the escapes
/// and the take-back binding still work.** `super+q` reaches the
/// application rather than its binding; Ctrl+Alt+F2 still switches
/// the console; `super+Escape` takes the shortcuts back without the
/// application seeing it, and `super+q` is the binding's again.
#[test]
fn a_window_holding_the_shortcuts_gets_super_and_the_take_back_key_still_works() {
    let (mut session, ..) = asking("inhibit-super");
    assert_eq!(
        held(&mut session),
        Some(true),
        "the focused window asked and was not made active"
    );

    let fired = tap(&mut session, &[SUPER, Q]);
    let sent = heard(&mut session);
    assert_eq!(fired, "", "super+q fired its binding over the window");
    assert!(
        sent.contains(&SUPER) && sent.contains(&Q),
        "the window holding the shortcuts was not sent super+q: {sent:?}"
    );

    let _ = tap(&mut session, &[CTRL, ALT, F2]);
    assert_eq!(
        session.state.request.take(),
        Some(Request::Vt(2)),
        "ctrl+alt+F2 did not switch the console over the window"
    );
    let _ = heard(&mut session);

    let _ = tap(&mut session, &[SUPER, ESCAPE]);
    let sent = heard(&mut session);
    assert!(
        !sent.contains(&ESCAPE),
        "the take-back key reached the window: {sent:?}"
    );
    assert_eq!(
        held(&mut session),
        Some(false),
        "super+Escape did not take the shortcuts back"
    );
    assert_eq!(
        tap(&mut session, &[SUPER, Q]),
        "q",
        "taken back, super+q is still not the binding's"
    );
}

/// **The shortcuts follow the keyboard.** Focus on the other window
/// makes the inhibitor inactive and `super+q` the binding's; focus
/// back makes it active again -- after a take-back too.
#[test]
fn the_shortcuts_follow_the_keyboard() {
    let (mut session, first, second) = asking("inhibit-focus");
    focus(&mut session, &first);
    assert_eq!(
        held(&mut session),
        Some(false),
        "the window kept the shortcuts in the background"
    );
    assert_eq!(
        tap(&mut session, &[SUPER, Q]),
        "q",
        "with another window focused, super+q is not the binding's"
    );

    focus(&mut session, &second);
    assert_eq!(
        held(&mut session),
        Some(true),
        "the window did not get the shortcuts back with the keyboard"
    );

    let _ = tap(&mut session, &[SUPER, ESCAPE]);
    assert_eq!(held(&mut session), Some(false), "the premise");
    focus(&mut session, &first);
    focus(&mut session, &second);
    assert_eq!(
        held(&mut session),
        Some(true),
        "taken back once, the window never held the shortcuts again"
    );
    let _ = heard(&mut session);
    assert_eq!(tap(&mut session, &[SUPER, Q]), "");
    assert!(heard(&mut session).contains(&Q));
}

/// **Locking takes them, and unlocking gives them back** to the
/// window that still has the keyboard. At the lock, `super+q` goes
/// to the lock screen and not to the window.
#[test]
fn the_lock_takes_the_shortcuts_and_gives_them_back() {
    let (mut session, ..) = asking("inhibit-lock");
    let lock = session.lock();
    assert_eq!(
        held(&mut session),
        Some(false),
        "a window behind the lock still held the shortcuts"
    );
    let _ = tap(&mut session, &[SUPER, Q]);
    assert!(
        !heard(&mut session).contains(&Q),
        "a key at the lock reached the window behind it"
    );
    session.assert_unlocks(lock);
    assert_eq!(
        held(&mut session),
        Some(true),
        "unlocked, the window with the keyboard did not get them back"
    );
}
