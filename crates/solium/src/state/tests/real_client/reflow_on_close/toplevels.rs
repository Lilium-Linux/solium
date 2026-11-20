//! The windows for a shell that is its own program (#50): both
//! window protocols against a real client binding them as a dock
//! does.

use super::*;
use zwlr_foreign_toplevel_handle_v1::State;

/// Let the client's requests in, tell both protocols what
/// changed, and let the client read it.
fn publish(desk: &mut Desk) {
    desk.pump();
    desk.state.publish_toplevels();
    desk.pump();
}

/// The client binds both, as a dock does, and hears what there is.
fn dock(desk: &mut Desk) {
    desk.client.bind_toplevels(&desk.qh);
    publish(desk);
}

/// The desk's monitor as a `wl_output` global, which the client
/// binds: a monitor the fixture maps has none, and a client can
/// only be told it entered an output it holds.
fn advertise_monitor(desk: &mut Desk) -> smithay::reexports::wayland_server::backend::GlobalId {
    let output = desk
        .state
        .space
        .outputs()
        .next()
        .cloned()
        .expect("the desk's monitor");
    let global = output.create_global::<Solium>(&desk.display.handle());
    desk.pump();
    global
}

fn wlr(desk: &Desk) -> Vec<Told> {
    desk.client
        .wlr_told
        .iter()
        .map(|(_, told)| told.clone())
        .collect()
}

fn ext(desk: &Desk) -> Vec<Told> {
    desk.client
        .ext_told
        .iter()
        .map(|(_, told)| told.clone())
        .collect()
}

/// **A dock is told every window on both lists, and each change
/// once**: a window open before it bound, one opened after, a
/// title changed, nothing changed, and a window that went.
#[test]
fn a_dock_is_told_every_window_on_both_lists_and_each_change_once() {
    let mut desk = Desk::new();
    let _monitor = advertise_monitor(&mut desk);
    let first = desk.open_surface();
    first.toplevel.set_title("one".to_owned());
    first.toplevel.set_app_id("org.example.one".to_owned());
    publish(&mut desk);
    dock(&mut desk);
    let (wlr_told, ext_told) = (wlr(&desk), ext(&desk));
    assert_eq!(
        (wlr_told.len(), ext_told.len()),
        (1, 1),
        "the window open before"
    );
    assert_eq!(
        (
            wlr_told[0].title.as_str(),
            wlr_told[0].app_id.as_str(),
            wlr_told[0].outputs,
            wlr_told[0].done,
            wlr_told[0].closed,
        ),
        ("one", "org.example.one", 1, 1, false),
        "(the wlr handle's title, app id, monitors, dones, closed)"
    );
    assert_eq!(
        (
            ext_told[0].title.as_str(),
            ext_told[0].app_id.as_str(),
            ext_told[0].done
        ),
        ("one", "org.example.one", 1)
    );

    let second = desk.open_surface();
    publish(&mut desk);
    assert_eq!(
        (wlr(&desk).len(), ext(&desk).len()),
        (2, 2),
        "the window opened after"
    );

    // Counted from here: the second window took the keyboard, and
    // the first was told it is no longer activated.
    let before = (wlr(&desk)[0].done, ext(&desk)[0].done);
    first.toplevel.set_title("two".to_owned());
    publish(&mut desk);
    let (wlr_told, ext_told) = (wlr(&desk), ext(&desk));
    assert_eq!(
        (
            wlr_told[0].title.as_str(),
            wlr_told[0].done,
            ext_told[0].title.as_str(),
            ext_told[0].done
        ),
        ("two", before.0 + 1, "two", before.1 + 1),
        "a new title, each told once with its done"
    );
    publish(&mut desk);
    assert_eq!(
        (wlr(&desk)[0].done, ext(&desk)[0].done),
        (before.0 + 1, before.1 + 1),
        "nothing changed, and nothing was told"
    );

    second.toplevel.destroy();
    second.xdg.destroy();
    second.surface.destroy();
    desk.pump();
    desk.state.sync_panes();
    publish(&mut desk);
    assert_eq!(
        (
            wlr(&desk)[1].closed,
            ext(&desk)[1].closed,
            wlr(&desk)[0].closed
        ),
        (true, true, false),
        "(the window that went closed on both, the other not)"
    );
}

/// **The wlr states follow the window**: focused is `activated`,
/// and a minimize is `minimized`.
#[test]
fn the_states_follow_the_window() {
    let mut desk = Desk::new();
    let opened = desk.open_surface();
    dock(&mut desk);
    let window = desk
        .state
        .panes
        .get(opened.pane)
        .and_then(Pane::client)
        .cloned()
        .expect("its client");
    desk.state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    publish(&mut desk);
    assert!(
        wlr(&desk)[0].states.contains(&u32::from(State::Activated)),
        "{:?}",
        wlr(&desk)[0].states
    );
    desk.state.apply(crate::script::Outcome {
        commands: vec![crate::script::Command::Minimize {
            id: opened.pane.get(),
        }],
        ..crate::script::Outcome::default()
    });
    publish(&mut desk);
    assert!(
        wlr(&desk)[0].states.contains(&u32::from(State::Minimized)),
        "{:?}",
        wlr(&desk)[0].states
    );
}

/// **A dock's requests reach Lua as the `windows` actions of the
/// surface "foreign-toplevel"**, each with the window's id, and a
/// request that would change nothing is not sent.
#[test]
fn a_docks_requests_are_the_windows_actions_of_the_foreign_toplevel_surface() {
    let mut desk = Desk::new();
    desk.install(
        r#"heard = ""
sol.on("surface", function(surface, action, data)
heard = heard .. surface .. " " .. action .. " " .. tostring(data.id) .. ";"
end)"#,
    );
    let opened = desk.open_surface();
    dock(&mut desk);
    let id = opened.pane.get();
    let handle = desk.client.wlr_told[0].0.clone();
    let seat = desk.client.seat.clone().expect("wl_seat bound");
    handle.unset_maximized();
    handle.unset_fullscreen();
    handle.unset_minimized();
    handle.set_maximized();
    handle.set_fullscreen(None);
    handle.set_minimized();
    handle.activate(&seat);
    handle.close();
    desk.pump();
    let heard = desk
        .state
        .scripts
        .as_ref()
        .map(|scripts| scripts.evaluate("return heard"))
        .unwrap_or_default();
    let one = |action: &str| format!("foreign-toplevel {action} {id};");
    assert_eq!(
        heard,
        [
            "windows.maximize",
            "windows.fullscreen",
            "windows.minimize",
            "windows.focus",
            "windows.close",
        ]
        .map(one)
        .concat(),
        "the unsets of what was not set were sent as nothing"
    );
}

/// **Through the shipped `actions.lua`, a dock's request does
/// what the taskbar's does**: minimize puts the window away and
/// close closes it.
#[test]
fn through_actions_lua_a_docks_minimize_and_close_act_on_the_window() {
    let mut desk = Desk::new();
    desk.install(r#"require("actions")"#);
    let opened = desk.open_surface();
    dock(&mut desk);
    let handle = desk.client.wlr_told[0].0.clone();
    handle.set_minimized();
    desk.pump();
    let row = |desk: &Desk| {
        desk.state
            .snapshot()
            .windows
            .into_iter()
            .find(|window| window.id == opened.pane.get())
    };
    assert_eq!(row(&desk).map(|window| window.minimized), Some(true));
    handle.close();
    desk.pump();
    desk.ask();
    assert!(
        desk.client
            .closes
            .contains(&wayland_client::Proxy::id(&opened.toplevel)),
        "the client was not asked to close"
    );
}

/// **`set_rectangle` is the window's `icon`**, in the
/// compositor's space, and a rectangle of nothing takes it away.
#[test]
fn set_rectangle_gives_the_window_its_icon() {
    let mut desk = Desk::new();
    let opened = desk.open_surface();
    dock(&mut desk);
    let window = desk
        .state
        .panes
        .get(opened.pane)
        .and_then(Pane::client)
        .cloned()
        .expect("its client");
    let at = desk.state.space.element_location(&window).expect("mapped");
    let handle = desk.client.wlr_told[0].0.clone();
    handle.set_rectangle(&opened.surface, 10, 20, 30, 40);
    desk.pump();
    let icon = |desk: &Desk| {
        desk.state
            .snapshot()
            .windows
            .into_iter()
            .find(|each| each.id == opened.pane.get())
            .and_then(|each| each.icon)
    };
    assert_eq!(
        icon(&desk),
        Some(crate::script::Rect {
            x: f64::from(at.x + 10),
            y: f64::from(at.y + 20),
            w: 30.0,
            h: 40.0,
        })
    );
    handle.set_rectangle(&opened.surface, 0, 0, 0, 0);
    desk.pump();
    assert_eq!(icon(&desk), None);
}

/// **`toplevels = { list = false, manage = false }` takes both
/// away**: every handle given out is closed, the manager is told
/// it is finished, a client connecting afterwards is offered
/// neither, and offered again both are back.
#[test]
fn taking_the_protocols_away_closes_their_handles_and_hides_them() {
    let mut desk = Desk::new();
    let _opened = desk.open_surface();
    dock(&mut desk);
    let offer = |desk: &mut Desk, list: bool, manage: bool| {
        desk.state.apply(crate::script::Outcome {
            commands: vec![crate::script::Command::Toplevels(
                crate::toplevels::Settings {
                    list,
                    manage,
                    ..crate::toplevels::Settings::default()
                },
            )],
            ..crate::script::Outcome::default()
        });
        publish(desk);
    };
    offer(&mut desk, false, false);
    assert_eq!(
        (
            wlr(&desk)[0].closed,
            ext(&desk)[0].closed,
            desk.client.toplevel_manager_finished
        ),
        (true, true, true)
    );
    let (_conn, _queue, late) = connect(&mut desk.display, &mut desk.state);
    assert!(
        late.toplevel_globals.is_empty(),
        "offered {:?}",
        late.toplevel_globals
    );
    offer(&mut desk, true, true);
    let (_conn, _queue, later) = connect(&mut desk.display, &mut desk.state);
    let mut offered: Vec<String> = later
        .toplevel_globals
        .iter()
        .map(|(interface, _)| interface.clone())
        .collect();
    offered.sort();
    assert_eq!(
        offered,
        [
            "ext_foreign_toplevel_list_v1",
            "zwlr_foreign_toplevel_manager_v1"
        ]
    );
}
