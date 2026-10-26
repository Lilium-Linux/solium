use super::*;
use smithay::backend::input::ButtonState;
use wayland_client::Proxy as _;
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::Layer as Band,
    zwlr_layer_surface_v1::{KeyboardInteractivity as Keys, ZwlrLayerSurfaceV1},
};

fn russian_session() -> Session {
    let mut session = Session::new();
    russian(&mut session.state);
    session
}

struct LayerClient {
    side: Side,
    surface: wl_surface::WlSurface,
    role: ZwlrLayerSurfaceV1,
    buffer: Option<wl_buffer::WlBuffer>,
}

impl LayerClient {
    fn new(session: &mut Session, band: Band, keys: Keys, mapped: bool) -> Self {
        Self::on_output(session, band, keys, mapped, None)
    }

    fn on_output(
        session: &mut Session,
        band: Band,
        keys: Keys,
        mapped: bool,
        output: Option<usize>,
    ) -> Self {
        let mut side = Side::connect(&mut session.display, &mut session.state);
        let surface = side
            .client
            .compositor
            .as_ref()
            .unwrap()
            .create_surface(&side.qh, ());
        let role = side.client.layer_shell.as_ref().unwrap().get_layer_surface(
            &surface,
            output
                .and_then(|index| side.client.outputs.get(index))
                .or(side.client.output.as_ref()),
            band,
            "keyboard-test".to_owned(),
            &side.qh,
            (),
        );
        role.set_size(200, 100);
        role.set_anchor(zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Left);
        role.set_exclusive_zone(-1);
        role.set_keyboard_interactivity(keys);
        surface.commit();
        side.pump(&mut session.display, &mut session.state);
        side.pump(&mut session.display, &mut session.state);
        assert!(
            side.client
                .layer_configures
                .iter()
                .any(|(id, _)| *id == role.id())
        );
        let mut result = Self {
            side,
            surface,
            role,
            buffer: None,
        };
        if mapped {
            result.map(session);
        }
        result
    }

    fn pump(&mut self, session: &mut Session) {
        self.side.pump(&mut session.display, &mut session.state);
        session.state.sync_panes();
        self.side.pump(&mut session.display, &mut session.state);
        session.app.pump(&mut session.display, &mut session.state);
    }

    fn map(&mut self, session: &mut Session) {
        self.buffer = Some(commit_buffer(
            &self.side.client,
            &self.side.qh,
            &self.surface,
            200,
            100,
        ));
        self.pump(session);
    }

    fn focused(&self) -> bool {
        self.side.client.keyboard_on == Some(self.surface.id())
    }

    fn escape(&mut self, session: &mut Session) {
        self.side.client.typed.clear();
        crate::input::key(&mut session.state, Keycode::new(9), KeyState::Pressed, 1);
        crate::input::key(&mut session.state, Keycode::new(9), KeyState::Released, 2);
        self.pump(session);
        let id = Some(self.surface.id().protocol_id());
        assert_eq!(self.side.client.typed, [(id, 1), (id, 1)]);
    }

    fn menu(
        &mut self,
        session: &mut Session,
    ) -> (
        wl_surface::WlSurface,
        xdg_surface::XdgSurface,
        xdg_popup::XdgPopup,
    ) {
        self.menu_on(session, None)
    }

    fn menu_on(
        &mut self,
        session: &mut Session,
        parent: Option<&xdg_surface::XdgSurface>,
    ) -> (
        wl_surface::WlSurface,
        xdg_surface::XdgSurface,
        xdg_popup::XdgPopup,
    ) {
        let client = &self.side.client;
        let surface = client
            .compositor
            .as_ref()
            .unwrap()
            .create_surface(&self.side.qh, ());
        let base = client.wm_base.as_ref().unwrap();
        let xdg = base.get_xdg_surface(&surface, &self.side.qh, ());
        let positioner = base.create_positioner(&self.side.qh, ());
        positioner.set_size(32, 32);
        positioner.set_anchor_rect(0, 0, 1, 1);
        let popup = xdg.get_popup(parent, &positioner, &self.side.qh, ());
        if parent.is_none() {
            self.role.get_popup(&popup);
        }
        let serial = if parent.is_some() {
            client.key_serial.expect("the key that opened a submenu")
        } else {
            client.serial
        };
        popup.grab(client.seat.as_ref().unwrap(), serial);
        surface.commit();
        self.pump(session);
        let _buffer = commit_buffer(&self.side.client, &self.side.qh, &surface, 32, 32);
        self.pump(session);
        // Popup grabs re-aim the keyboard at the topmost menu on key delivery.
        crate::input::key(&mut session.state, Keycode::new(9), KeyState::Pressed, 1);
        crate::input::key(&mut session.state, Keycode::new(9), KeyState::Released, 2);
        self.pump(session);
        assert!(session.state.seat.get_keyboard().unwrap().is_grabbed());
        assert_eq!(self.side.client.keyboard_on, Some(surface.id()));
        (surface, xdg, popup)
    }

    fn click(&mut self, session: &mut Session) {
        move_pointer(&mut session.state, (20.0, 20.0), 1);
        let region = crate::monitor::union(&session.state.space).unwrap();
        crate::synth::send_button(&mut session.state, region, 0x110, ButtonState::Pressed, 2);
        crate::synth::send_button(&mut session.state, region, 0x110, ButtonState::Released, 3);
        self.pump(session);
    }
}

#[test]
fn an_exclusive_layer_receives_escape_and_survives_focus_settlement() {
    let mut session = russian_session();
    let _window = session.app.open(&mut session.display, &mut session.state);
    let mut layer = LayerClient::new(&mut session, Band::Overlay, Keys::Exclusive, true);
    assert!(
        layer.focused(),
        "a mapped exclusive layer must receive keyboard enter"
    );
    layer.escape(&mut session);
}

#[test]
fn an_initial_bufferless_layer_does_not_take_the_keyboard() {
    let mut session = russian_session();
    let (_, _, surface, _) = session.app.open(&mut session.display, &mut session.state);
    let mut layer = LayerClient::new(&mut session, Band::Overlay, Keys::Exclusive, false);
    assert!(!layer.focused());
    assert_eq!(session.app.client.keyboard_on, Some(surface.id()));
    layer.map(&mut session);
    assert!(layer.focused());
}

#[test]
fn layer_interactivity_changes_only_on_commit() {
    let mut session = russian_session();
    let _window = session.app.open(&mut session.display, &mut session.state);
    let mut layer = LayerClient::new(&mut session, Band::Overlay, Keys::None, true);
    layer.role.set_keyboard_interactivity(Keys::Exclusive);
    layer.pump(&mut session);
    assert!(!layer.focused(), "pending state cannot take focus");
    layer.surface.commit();
    layer.pump(&mut session);
    assert!(layer.focused());
    layer.role.set_keyboard_interactivity(Keys::None);
    layer.surface.commit();
    layer.pump(&mut session);
    assert!(!layer.focused());
    assert!(session.app.client.keyboard_on.is_some());
}

#[test]
fn an_exclusive_overlay_outranks_a_top_layer_and_window_focus() {
    let mut session = russian_session();
    let (window, _, _, _) = session.app.open(&mut session.display, &mut session.state);
    let mut overlay = LayerClient::new(&mut session, Band::Overlay, Keys::Exclusive, true);
    let mut top = LayerClient::new(&mut session, Band::Top, Keys::Exclusive, true);
    overlay.pump(&mut session);
    assert!(overlay.focused());
    assert!(!top.focused());
    session.state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Binding,
    );
    overlay.pump(&mut session);
    assert!(overlay.focused(), "a window cannot steal exclusive focus");
    overlay.surface.attach(None, 0, 0);
    overlay.surface.commit();
    overlay.pump(&mut session);
    top.pump(&mut session);
    assert!(top.focused(), "the next exclusive layer takes over");
}

#[test]
fn on_demand_layers_take_focus_on_click_and_allow_a_window_to_take_it_back() {
    let mut session = russian_session();
    let (window, _, _, _) = session.app.open(&mut session.display, &mut session.state);
    let mut layer = LayerClient::new(&mut session, Band::Overlay, Keys::OnDemand, true);
    assert!(!layer.focused());
    layer.click(&mut session);
    assert!(layer.focused());
    layer.escape(&mut session);
    session.state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Binding,
    );
    layer.pump(&mut session);
    assert!(!layer.focused());
    assert!(session.app.client.keyboard_on.is_some());
}

#[test]
fn a_noninteractive_layer_never_takes_focus_on_click() {
    let mut session = russian_session();
    let (_, _, surface, _) = session.app.open(&mut session.display, &mut session.state);
    let mut layer = LayerClient::new(&mut session, Band::Overlay, Keys::None, true);
    layer.click(&mut session);
    assert!(!layer.focused());
    assert_eq!(session.app.client.keyboard_on, Some(surface.id()));
}

#[test]
fn unmapping_or_destroying_a_layer_restores_the_previous_window() {
    for destroy in [false, true] {
        let mut session = russian_session();
        let (first, _, surface, _) = session.app.open(&mut session.display, &mut session.state);
        let _second = session.app.open(&mut session.display, &mut session.state);
        session.state.focus_window(
            &first,
            SERIAL_COUNTER.next_serial(),
            crate::script::FocusSource::Binding,
        );
        let mut layer = LayerClient::new(&mut session, Band::Overlay, Keys::Exclusive, true);
        assert!(layer.focused());
        if destroy {
            layer.role.destroy();
            layer.surface.destroy();
        } else {
            layer.surface.attach(None, 0, 0);
            layer.surface.commit();
        }
        layer.pump(&mut session);
        assert_eq!(
            session.app.client.keyboard_on,
            Some(surface.id()),
            "destroy={destroy}"
        );
        assert_eq!(session.type_key(), (true, false));
    }
}

#[test]
fn exclusive_layers_never_steal_focus_from_the_lock() {
    for before_lock in [false, true] {
        let mut session = russian_session();
        let _window = session.app.open(&mut session.display, &mut session.state);
        let mut layer = before_lock
            .then(|| LayerClient::new(&mut session, Band::Overlay, Keys::Exclusive, true));
        let lock = session.lock();
        let layer = layer.get_or_insert_with(|| {
            LayerClient::new(&mut session, Band::Overlay, Keys::Exclusive, true)
        });
        layer.click(&mut session);
        assert!(!layer.focused());
        let selections = session.app.client.selections;
        session.assert_sealed("exclusive layer mapped/clicked", selections);
        layer.surface.attach(None, 0, 0);
        layer.surface.commit();
        layer.pump(&mut session);
        session.assert_sealed("exclusive layer unmapped", selections);
        session.assert_unlocks(lock);
    }
}

#[test]
fn an_exclusive_layer_preempts_a_popup_keyboard_grab() {
    let mut session = russian_session();
    let (_, _, _, xdg) = session.app.open(&mut session.display, &mut session.state);
    let _popup = session
        .app
        .menu(&mut session.display, &mut session.state, &xdg);
    let mut layer = LayerClient::new(&mut session, Band::Overlay, Keys::Exclusive, true);
    assert!(layer.focused());
    layer.escape(&mut session);
}

#[test]
fn plain_escape_reaches_an_exclusive_layer_while_a_mode_holds_input() {
    let mut session = russian_session();
    let _window = session.app.open(&mut session.display, &mut session.state);
    session.state.script_grab = true;
    let mut layer = LayerClient::new(&mut session, Band::Overlay, Keys::Exclusive, true);
    assert!(layer.focused());
    layer.escape(&mut session);
}

#[test]
fn clearing_window_focus_cannot_drop_an_exclusive_owner() {
    let mut session = russian_session();
    let _window = session.app.open(&mut session.display, &mut session.state);
    let mut layer = LayerClient::new(&mut session, Band::Overlay, Keys::Exclusive, true);
    assert!(layer.focused());
    assert!(
        !session
            .state
            .give_keyboard(None, SERIAL_COUNTER.next_serial())
    );
    layer.escape(&mut session);
}

#[test]
fn an_exclusive_layer_ends_and_blocks_a_hosted_keyboard_hold() {
    let mut session = russian_session();
    let _window = session.app.open(&mut session.display, &mut session.state);
    let shell = stand_in(
        &mut session.state,
        "keyboard-shell",
        crate::scripted::Layer::Top,
        Rectangle::new((0, 0).into(), (1920, 1080).into()),
        crate::scripted::Stand::solid(),
    );
    let want = |session: &mut Session| {
        session
            .state
            .surfaces
            .get_mut(shell)
            .unwrap()
            .stand_mut()
            .unwrap()
            .keyboard = Some(crate::qml::hosted::KeyboardReport::Wanted(vec![
            "Escape".to_owned(),
        ]));
        session.state.settle_scenes();
    };
    want(&mut session);
    assert!(session.state.hosted_keyboard.is_some());
    let mut layer = LayerClient::new(&mut session, Band::Overlay, Keys::Exclusive, true);
    assert!(session.state.hosted_keyboard.is_none());
    want(&mut session);
    assert!(session.state.hosted_keyboard.is_none());
    layer.escape(&mut session);
}

#[test]
fn unplugging_an_exclusive_layers_output_restores_a_visible_window() {
    let mut session = Session::with_monitors(2);
    russian(&mut session.state);
    let (_, _, surface, _) = session.app.open(&mut session.display, &mut session.state);
    let mut layer = LayerClient::new(&mut session, Band::Overlay, Keys::Exclusive, true);
    assert!(layer.focused());
    session.unplug(1);
    layer.pump(&mut session);
    assert_eq!(session.app.client.keyboard_on, Some(surface.id()));
    assert_eq!(session.type_key(), (true, false));
}

#[test]
fn a_button_release_alone_cannot_focus_an_on_demand_layer() {
    let mut session = russian_session();
    let (_, _, surface, _) = session.app.open(&mut session.display, &mut session.state);
    let mut layer = LayerClient::new(&mut session, Band::Overlay, Keys::OnDemand, true);
    move_pointer(&mut session.state, (20.0, 20.0), 1);
    let region = crate::monitor::union(&session.state.space).unwrap();
    crate::synth::send_button(&mut session.state, region, 0x110, ButtonState::Released, 2);
    layer.pump(&mut session);
    assert!(!layer.focused());
    assert_eq!(session.app.client.keyboard_on, Some(surface.id()));
}

#[test]
fn replacing_an_exclusive_layer_keeps_its_output_when_the_pointer_moves() {
    let mut session = Session::with_monitors(2);
    russian(&mut session.state);
    let mut first =
        LayerClient::on_output(&mut session, Band::Overlay, Keys::Exclusive, true, Some(0));
    assert!(first.focused());
    let mut other =
        LayerClient::on_output(&mut session, Band::Overlay, Keys::Exclusive, true, Some(1));
    first.pump(&mut session);
    assert!(first.focused());
    move_pointer(&mut session.state, (1940.0, 20.0), 1);
    let mut replacement =
        LayerClient::on_output(&mut session, Band::Overlay, Keys::Exclusive, true, Some(0));
    other.pump(&mut session);
    assert!(
        replacement.focused(),
        "the new topmost layer on the owning output wins"
    );
    assert!(!other.focused());
    replacement.escape(&mut session);
}

#[test]
fn destroying_only_an_on_demand_role_leaves_no_keyboard_owner() {
    let mut session = russian_session();
    let mut layer = LayerClient::new(&mut session, Band::Overlay, Keys::OnDemand, true);
    layer.click(&mut session);
    assert!(layer.focused());
    layer.role.destroy();
    layer.pump(&mut session);
    assert!(
        !layer.focused(),
        "the still-live wl_surface must receive leave"
    );
    layer.side.client.typed.clear();
    crate::input::key(&mut session.state, Keycode::new(9), KeyState::Pressed, 1);
    crate::input::key(&mut session.state, Keycode::new(9), KeyState::Released, 2);
    layer.pump(&mut session);
    assert!(layer.side.client.typed.is_empty());
}

#[test]
fn unplugging_an_on_demand_output_leaves_no_keyboard_owner() {
    let mut session = russian_session();
    let mut layer = LayerClient::new(&mut session, Band::Overlay, Keys::OnDemand, true);
    layer.click(&mut session);
    assert!(layer.focused());
    session.unplug(0);
    layer.pump(&mut session);
    assert!(!layer.focused());
    layer.side.client.typed.clear();
    crate::input::key(&mut session.state, Keycode::new(9), KeyState::Pressed, 1);
    crate::input::key(&mut session.state, Keycode::new(9), KeyState::Released, 2);
    layer.pump(&mut session);
    assert!(layer.side.client.typed.is_empty());
}

#[test]
fn a_withdrawn_layer_releases_its_popup_grabs() {
    for keys in [Keys::Exclusive, Keys::OnDemand] {
        for withdrawal in ["unmap", "none", "role", "output"] {
            let mut session = russian_session();
            let mut layer = LayerClient::new(&mut session, Band::Overlay, keys, true);
            if keys == Keys::OnDemand {
                layer.click(&mut session);
            }
            let _menu = layer.menu(&mut session);
            match withdrawal {
                "unmap" => {
                    layer.surface.attach(None, 0, 0);
                    layer.surface.commit();
                }
                "none" => {
                    layer.role.set_keyboard_interactivity(Keys::None);
                    layer.surface.commit();
                }
                "role" => layer.role.destroy(),
                "output" => session.unplug(0),
                _ => unreachable!(),
            }
            layer.pump(&mut session);
            assert!(
                layer.side.client.keyboard_on.is_none(),
                "{keys:?} {withdrawal}: keyboard leave"
            );
            assert!(
                !session.state.seat.get_keyboard().unwrap().is_grabbed(),
                "{keys:?} {withdrawal}: keyboard grab"
            );
            assert!(
                !session.state.seat.get_pointer().unwrap().is_grabbed(),
                "{keys:?} {withdrawal}: pointer grab"
            );
            layer.side.client.typed.clear();
            crate::input::key(&mut session.state, Keycode::new(9), KeyState::Pressed, 1);
            crate::input::key(&mut session.state, Keycode::new(9), KeyState::Released, 2);
            layer.pump(&mut session);
            assert!(
                layer.side.client.typed.is_empty(),
                "{keys:?} {withdrawal}: no keys after withdrawal"
            );
        }
    }
}

#[test]
fn an_exclusive_layer_returns_to_the_previous_layers_root_after_dismissing_its_menu() {
    let mut session = russian_session();
    let mut previous = LayerClient::new(&mut session, Band::Top, Keys::OnDemand, true);
    previous.click(&mut session);
    let _menu = previous.menu(&mut session);
    let mut exclusive = LayerClient::new(&mut session, Band::Overlay, Keys::Exclusive, true);
    assert!(exclusive.focused());
    exclusive.surface.attach(None, 0, 0);
    exclusive.surface.commit();
    exclusive.pump(&mut session);
    previous.pump(&mut session);
    assert!(previous.focused());
    previous.escape(&mut session);
}

#[test]
fn withdrawing_a_layer_after_its_submenu_dies_releases_the_parent_grab() {
    for destroy_role in [false, true] {
        let mut session = russian_session();
        let mut layer = LayerClient::new(&mut session, Band::Overlay, Keys::OnDemand, true);
        layer.click(&mut session);
        let parent = layer.menu(&mut session);
        let child = layer.menu_on(&mut session, Some(&parent.1));
        child.2.destroy();
        child.1.destroy();
        child.0.destroy();
        if destroy_role {
            layer.role.destroy();
        } else {
            layer.surface.attach(None, 0, 0);
            layer.surface.commit();
        }
        layer.pump(&mut session);
        // The destroyed submenu cannot receive a leave event naming its dead
        // wl_surface. Assert the seat itself is clear, then prove no keys leak.
        assert!(
            session
                .state
                .seat
                .get_keyboard()
                .unwrap()
                .current_focus()
                .is_none(),
            "destroy_role={destroy_role}: actual seat focus; tracked={:?}",
            session.state.layer_keyboard_focus
        );
        assert!(!session.state.seat.get_keyboard().unwrap().is_grabbed());
        assert!(!session.state.seat.get_pointer().unwrap().is_grabbed());
        layer.side.client.typed.clear();
        crate::input::key(&mut session.state, Keycode::new(9), KeyState::Pressed, 1);
        crate::input::key(&mut session.state, Keycode::new(9), KeyState::Released, 2);
        layer.pump(&mut session);
        assert!(layer.side.client.typed.is_empty());
    }
}

#[test]
fn exclusive_preemption_after_a_submenu_dies_restores_the_retained_layer_root() {
    let mut session = russian_session();
    let mut previous = LayerClient::new(&mut session, Band::Top, Keys::OnDemand, true);
    previous.click(&mut session);
    let parent = previous.menu(&mut session);
    let child = previous.menu_on(&mut session, Some(&parent.1));
    child.2.destroy();
    child.1.destroy();
    child.0.destroy();
    let mut exclusive = LayerClient::new(&mut session, Band::Overlay, Keys::Exclusive, true);
    assert!(exclusive.focused());
    exclusive.surface.attach(None, 0, 0);
    exclusive.surface.commit();
    exclusive.pump(&mut session);
    previous.pump(&mut session);
    assert!(
        previous.focused(),
        "restore the live root instead of its destroyed submenu"
    );
    previous.escape(&mut session);
}
