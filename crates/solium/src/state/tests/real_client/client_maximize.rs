//! A client's own maximise (#445), and what a layout's tile makes of it
//! (#450), with real xdg clients and the shipped modes and tiling.
//! The X11 half is `x11-probe`'s `maximized-before-map`,
//! `maximized-at-map` and `ask-maximized`, nested: the test binary
//! cannot start XWayland.

use super::*;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;

/// The desk every test here starts from: one monitor with a bar
/// along its top, so the work area is smaller than the monitor, and
/// the shipped modes loaded, floating.
struct Desk {
    display: Display<Solium>,
    state: Solium,
    conn: Connection,
    queue: wayland_client::EventQueue<Client>,
    client: Client,
    qh: QueueHandle<Client>,
    area: Rectangle<i32, Logical>,
    _bar: (
        wl_surface::WlSurface,
        zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    ),
}

impl Desk {
    fn new(name: &str) -> Self {
        Self::loading(name, shipped_modes_and_tiling)
    }

    /// [`Self::new`], with `lua/floating.lua` loaded too: what a
    /// maximised window's drag does is its (#452).
    fn with_floating(name: &str) -> Self {
        Self::loading(name, shipped_modes_tiling_and_floating)
    }

    fn loading(name: &str, scripts: fn(&mut Solium, &str)) -> Self {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let _output = one_screen(&mut state);
        let bar = bar(&mut display, &mut state, &conn, &client, &qh, 40);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        scripts(&mut state, name);
        let area = state.work_area().expect("the monitor has a work area");
        let desk = Self {
            display,
            state,
            conn,
            queue,
            client,
            qh,
            area,
            _bar: bar,
        };
        assert_eq!(area.size.h, 1080 - 40, "the premise: a bar takes 40 rows");
        desk
    }

    fn round_trip(&mut self) {
        pump(
            &mut self.display,
            &mut self.state,
            &self.conn,
            &self.qh,
            &mut self.queue,
            &mut self.client,
        );
    }

    /// A second goes by and a frame is drawn, and the client hears
    /// what that sent.
    fn rest(&mut self) {
        self.state.clock.advance(Duration::from_secs(1));
        a_frame(&mut self.state);
        self.round_trip();
    }

    fn press(&mut self, combo: &str) {
        assert!(self.state.trigger(combo), "{combo} is bound");
        self.round_trip();
    }

    /// An ordinary window, shown at once.
    fn open(&mut self) -> (Window, xdg_toplevel::XdgToplevel, wl_surface::WlSurface) {
        let opened = open_surface(
            &mut self.display,
            &mut self.state,
            &self.conn,
            &self.client,
            &self.qh,
        );
        self.round_trip();
        opened
    }

    /// A window whose client asks to be maximised before it has
    /// drawn anything, as an application restoring its last state
    /// does, and then draws at the size it was told.
    fn open_maximised(&mut self) -> (Window, xdg_toplevel::XdgToplevel, wl_surface::WlSurface) {
        let compositor = self.client.compositor.clone().expect("wl_compositor bound");
        let wm_base = self.client.wm_base.clone().expect("xdg_wm_base bound");
        let before: Vec<Window> = self.state.space.elements().cloned().collect();
        let surface = compositor.create_surface(&self.qh, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &self.qh, ());
        let toplevel = xdg_surface.get_toplevel(&self.qh, ());
        self.client.leave_frame(&toplevel, &self.qh);
        toplevel.set_maximized();
        surface.commit();
        self.round_trip();
        let window = self
            .state
            .space
            .elements()
            .find(|window| !before.contains(window))
            .cloned()
            .expect("new_toplevel mapped a window");
        let (w, h) = last_configured(&self.client, &toplevel).expect("the window was configured");
        commit_buffer(&self.client, &self.qh, &surface, w.max(64), h.max(64));
        self.round_trip();
        (window, toplevel, surface)
    }

    /// The client draws at the size it was last told, or at the one
    /// it has when it was told none.
    fn answer(&mut self, toplevel: &xdg_toplevel::XdgToplevel, surface: &wl_surface::WlSurface) {
        let (w, h) = last_configured(&self.client, toplevel)
            .filter(|&(w, h)| w > 0 && h > 0)
            .unwrap_or((64, 64));
        commit_buffer(&self.client, &self.qh, surface, w, h);
        self.round_trip();
        self.rest();
    }

    fn tile(&self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        self.state.panes.of(window).and_then(Pane::placed)
    }
}

/// **#445: a client's own `set_maximized` maximises its window, and
/// `unset_maximized` puts it back where it was**, on the floating
/// desktop. smithay's default answered both with a configure and no
/// maximised state, so an Electron window's own maximise button
/// emitted `maximize` and `unmaximize` together and nothing moved.
#[test]
fn a_clients_own_maximise_fills_the_work_area_and_its_restore_goes_back() {
    let mut desk = Desk::new("client-maximise-floating");
    let (window, toplevel, surface) = desk.open();
    desk.answer(&toplevel, &surface);
    let before = desk
        .state
        .real_geometry(&window)
        .expect("the window is mapped");

    toplevel.set_maximized();
    desk.round_trip();
    assert!(
        in_state(&window, State::Maximized),
        "the window was not maximised"
    );
    assert_eq!(
        maximized_sent(&desk.client, &toplevel).last(),
        Some(&true),
        "the client was not told it is maximised"
    );
    assert_eq!(
        last_configured(&desk.client, &toplevel),
        Some((desk.area.size.w, desk.area.size.h)),
        "the client was not told the work area"
    );
    desk.answer(&toplevel, &surface);
    assert_eq!(
        desk.state.real_geometry(&window),
        Some(desk.area),
        "the maximised window does not fill the work area"
    );

    toplevel.unset_maximized();
    desk.round_trip();
    assert!(
        !in_state(&window, State::Maximized),
        "the window is still maximised"
    );
    assert_eq!(
        maximized_sent(&desk.client, &toplevel).last(),
        Some(&false),
        "the client was not told it is restored"
    );
    desk.answer(&toplevel, &surface);
    assert_eq!(
        desk.state.real_geometry(&window),
        Some(before),
        "the restored window is not where it was"
    );
}

/// **#445: a fullscreen client asking to be maximised changes only
/// the state underneath**: it stays fullscreen, and leaving
/// fullscreen goes to maximised.
#[test]
fn a_fullscreen_clients_maximise_changes_only_the_state_underneath() {
    let mut desk = Desk::new("client-maximise-fullscreen");
    let (window, toplevel, surface) = desk.open();
    desk.answer(&toplevel, &surface);
    toplevel.set_fullscreen(None);
    desk.round_trip();
    desk.answer(&toplevel, &surface);
    assert!(
        in_state(&window, State::Fullscreen),
        "the premise: fullscreen"
    );

    toplevel.set_maximized();
    desk.round_trip();
    assert!(
        in_state(&window, State::Fullscreen),
        "a fullscreen client's maximise took it out of fullscreen"
    );
    assert!(
        in_state(&window, State::Maximized),
        "the state under the fullscreen one is not maximised"
    );

    toplevel.unset_fullscreen();
    desk.round_trip();
    assert!(!in_state(&window, State::Fullscreen), "still fullscreen");
    assert!(
        in_state(&window, State::Maximized),
        "not maximised after fullscreen"
    );
    assert_eq!(
        last_configured(&desk.client, &toplevel),
        Some((desk.area.size.w, desk.area.size.h)),
        "leaving fullscreen did not give the work area"
    );
}

/// **#450: a window that opens maximised on the floating desktop is
/// maximised**, at the whole work area, and restored it picks its
/// own size, having been shown at no other.
#[test]
fn a_window_that_opens_maximised_while_floating_is_maximised_and_restored() {
    let mut desk = Desk::new("client-maximise-opens-floating");
    let (window, toplevel, surface) = desk.open_maximised();
    desk.answer(&toplevel, &surface);
    assert!(
        in_state(&window, State::Maximized),
        "the window is not maximised"
    );
    assert_eq!(
        desk.state.real_geometry(&window),
        Some(desk.area),
        "the window opened maximised does not fill the work area"
    );

    toplevel.unset_maximized();
    desk.round_trip();
    assert!(
        !in_state(&window, State::Maximized),
        "the window is still maximised"
    );
    assert_eq!(
        last_configured(&desk.client, &toplevel),
        Some((0, 0)),
        "a window with no way back is not left to pick its own size"
    );
}

/// **#450: a window that opens maximised while a layout tiles is in
/// its tile, and told it is not maximised.** Steam restoring its
/// maximise opened over its tile, where nothing could size it.
#[test]
fn a_window_that_opens_maximised_while_tiling_is_tiled() {
    let mut desk = Desk::new("client-maximise-opens-tiling");
    let (other, other_toplevel, other_surface) = desk.open();
    desk.press("super+t");
    desk.answer(&other_toplevel, &other_surface);

    let (window, toplevel, surface) = desk.open_maximised();
    desk.answer(&toplevel, &surface);
    desk.answer(&other_toplevel, &other_surface);
    let tile = desk.tile(&window).expect("the window is in a tile");
    assert!(
        !in_state(&window, State::Maximized),
        "the window is maximised over its tile"
    );
    assert_eq!(
        (
            maximized_sent(&desk.client, &toplevel).last(),
            tiled_sent(&desk.client, &toplevel).last()
        ),
        (Some(&false), Some(&true)),
        "the client was not told it is tiled and not maximised"
    );
    assert_eq!(
        desk.state.real_geometry(&window),
        Some(tile),
        "the window is not at its tile"
    );
    let beside = desk.tile(&other).expect("the neighbour is in a tile");
    assert!(
        tile.intersection(beside).is_none(),
        "the two tiles overlap: {tile:?} and {beside:?}"
    );
}

/// **#450: a tiled window's client asking to be maximised stays in
/// its tile**, answered with a configure saying it is tiled and not
/// maximised; **the user's maximise still lifts it** out of the tile
/// and keeps the tile to come back to (#150).
#[test]
fn a_tiled_clients_maximise_keeps_its_tile_and_the_users_still_lifts_it() {
    let mut desk = Desk::new("client-maximise-tiled");
    let (window, toplevel, surface) = desk.open();
    let (_other, other_toplevel, other_surface) = desk.open();
    desk.press("super+t");
    desk.answer(&toplevel, &surface);
    desk.answer(&other_toplevel, &other_surface);
    let tile = desk.tile(&window).expect("the window is in a tile");
    let configures = sizes_sent(&desk.client, &toplevel).len();

    toplevel.set_maximized();
    desk.round_trip();
    desk.rest();
    assert!(
        !in_state(&window, State::Maximized),
        "the client's maximise lifted the window"
    );
    assert_eq!(desk.tile(&window), Some(tile), "the window left its tile");
    assert_eq!(
        sizes_sent(&desk.client, &toplevel).len(),
        configures + 1,
        "the request was not answered with one configure"
    );
    assert_eq!(
        (
            last_configured(&desk.client, &toplevel),
            maximized_sent(&desk.client, &toplevel).last(),
            tiled_sent(&desk.client, &toplevel).last()
        ),
        (Some((tile.size.w, tile.size.h)), Some(&false), Some(&true)),
        "the answer is not the tile, tiled and not maximised"
    );

    desk.state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    desk.press("super+shift+m");
    assert!(
        in_state(&window, State::Maximized),
        "super+shift+m did not maximise"
    );
    assert_eq!(
        desk.tile(&window),
        None,
        "the user's maximise left it in its tile"
    );
    assert_eq!(
        last_configured(&desk.client, &toplevel),
        Some((desk.area.size.w, desk.area.size.h)),
        "the user's maximise did not give the work area"
    );
    desk.answer(&toplevel, &surface);
    desk.press("super+shift+m");
    assert!(
        !in_state(&window, State::Maximized),
        "super+shift+m did not restore"
    );
    assert_eq!(
        desk.tile(&window),
        Some(tile),
        "the restore did not go back into the tile"
    );
}

/// The modifier's move drag (#452): the pointer to `at`, and super
/// and the left button down there, as a hand does it. Returns the
/// time the next event is at.
fn take_by_the_modifier(desk: &mut Desk, at: (f64, f64)) -> u64 {
    move_pointer(&mut desk.state, at, 10_000);
    crate::input::key(
        &mut desk.state,
        133_u32.into(),
        smithay::backend::input::KeyState::Pressed,
        11,
    );
    let region = crate::monitor::union(&desk.state.space).expect("a monitor");
    crate::synth::send_button(
        &mut desk.state,
        region,
        0x110,
        smithay::backend::input::ButtonState::Pressed,
        12_000,
    );
    assert!(
        desk.state
            .seat
            .get_pointer()
            .is_some_and(|pointer| pointer.is_grabbed()),
        "the premise: the press started a drag"
    );
    13_000
}

/// The pointer to `at`, and a pass: the `drag` listeners hear where
/// it got to once a frame (#452).
fn drag_to(desk: &mut Desk, at: (f64, f64), time: u64) {
    move_pointer(&mut desk.state, at, time);
    crate::input::grab::settle_drags(&mut desk.state, false);
}

/// And the button and super up again.
fn let_go(desk: &mut Desk, time: u64) {
    let region = crate::monitor::union(&desk.state.space).expect("a monitor");
    crate::synth::send_button(
        &mut desk.state,
        region,
        0x110,
        smithay::backend::input::ButtonState::Released,
        time,
    );
    crate::input::key(
        &mut desk.state,
        133_u32.into(),
        smithay::backend::input::KeyState::Released,
        u32::try_from(time / 1000).unwrap_or(u32::MAX),
    );
}

/// A floating window, shown, maximised by the user and answered:
/// the window, its client's handles, and where it stood before.
fn maximised_floating(
    desk: &mut Desk,
) -> (
    Window,
    xdg_toplevel::XdgToplevel,
    wl_surface::WlSurface,
    Rectangle<i32, Logical>,
) {
    let (window, toplevel, surface) = desk.open();
    desk.answer(&toplevel, &surface);
    let before = desk.state.real_geometry(&window).expect("mapped");
    desk.state.toggle_maximize(&window);
    desk.round_trip();
    desk.answer(&toplevel, &surface);
    assert_eq!(
        desk.state.real_geometry(&window),
        Some(desk.area),
        "the premise: maximised over the work area"
    );
    (window, toplevel, surface, before)
}

/// **#452: a maximised window does not move under the threshold**,
/// `maximize.restore_after` from where the drag was taken, and comes
/// off past it: so a click on its titlebar is a click, and the
/// setting is the distance. It was dragged at its maximised size
/// while it stayed maximised, half off its monitor.
#[test]
fn a_maximised_window_stays_put_until_the_drag_passes_the_threshold() {
    for (restore_after, short, long) in [(6.0, 4.0, 8.0), (50.0, 40.0, 60.0)] {
        let mut desk = Desk::with_floating("drag-unmaximise-threshold");
        let set = desk.state.scripts.as_ref().map(|scripts| {
            scripts.evaluate(&format!(
                "require(\"config\").maximize.restore_after = {restore_after}; return \"\""
            ))
        });
        assert_eq!(set.as_deref(), Some(""), "setting maximize.restore_after");
        let (window, _toplevel, _surface, _before) = maximised_floating(&mut desk);
        let from = (
            f64::from(desk.area.loc.x) + 480.0,
            f64::from(desk.area.loc.y) + 20.0,
        );
        let time = take_by_the_modifier(&mut desk, from);
        drag_to(&mut desk, (from.0 + short, from.1), time);
        assert!(
            in_state(&window, State::Maximized),
            "{restore_after}: a drag of {short} restored it"
        );
        assert_eq!(
            desk.state.real_geometry(&window),
            Some(desk.area),
            "{restore_after}: a drag of {short} moved it"
        );
        drag_to(&mut desk, (from.0 + long, from.1), time + 1_000);
        assert!(
            !in_state(&window, State::Maximized),
            "{restore_after}: a drag of {long} left it maximised"
        );
        let_go(&mut desk, time + 2_000);
    }
}

/// **#452: let go under the threshold, the window stays maximised
/// where it is**, and no layout is told it was dropped: a click on a
/// maximised window's titlebar is a click.
#[test]
fn a_maximised_window_let_go_under_the_threshold_stays_maximised() {
    let mut desk = Desk::with_floating("drag-unmaximise-click");
    let (window, _toplevel, _surface, _before) = maximised_floating(&mut desk);
    let from = (
        f64::from(desk.area.loc.x) + 480.0,
        f64::from(desk.area.loc.y) + 20.0,
    );
    let time = take_by_the_modifier(&mut desk, from);
    drag_to(&mut desk, (from.0 + 3.0, from.1 + 2.0), time);
    let_go(&mut desk, time + 1_000);
    desk.rest();
    assert!(
        in_state(&window, State::Maximized),
        "the click restored the window"
    );
    assert_eq!(
        desk.state.real_geometry(&window),
        Some(desk.area),
        "the click moved the window"
    );
    assert!(
        desk.state.pending_drop.is_none()
            && desk.state.moves.is_empty()
            && desk.state.drags_told.is_empty(),
        "something is left over from the click"
    );
}

/// **#452: dragged past the threshold, a maximised window comes off
/// at the size it had before, under the pointer, and follows it.**
/// Taken a quarter of the way along it, it is a quarter of the way
/// along under the pointer, as far down as it was taken; the next
/// motion moves it as far as the pointer goes, and it is left where
/// it was let go, floating and not maximised.
#[test]
fn a_maximised_window_dragged_past_the_threshold_comes_off_under_the_pointer() {
    let mut desk = Desk::with_floating("drag-unmaximise-floating");
    let (window, toplevel, surface, before) = maximised_floating(&mut desk);
    let from = desk.area.loc + Point::from((desk.area.size.w / 4, 20));
    let time = take_by_the_modifier(&mut desk, (f64::from(from.x), f64::from(from.y)));
    let at = from + Point::from((30, 40));
    drag_to(&mut desk, (f64::from(at.x), f64::from(at.y)), time);
    desk.round_trip();
    assert!(
        !in_state(&window, State::Maximized),
        "the drag did not restore the window"
    );
    assert_eq!(
        maximized_sent(&desk.client, &toplevel).last(),
        Some(&false),
        "the client was not told it is restored"
    );
    // A quarter of the way along, and 20 down: no frame here, so the
    // window's own height is the band it is held in.
    assert_eq!(before.size.w % 4, 0, "the premise: a quarter is whole");
    let under = Rectangle::new(at - Point::from((before.size.w / 4, 20)), before.size);
    assert_eq!(
        desk.state.real_geometry(&window).map(|real| real.loc),
        Some(under.loc),
        "the window did not come off under the pointer"
    );
    desk.answer(&toplevel, &surface);
    assert_eq!(
        desk.state.real_geometry(&window),
        Some(under),
        "the window did not come off at the size it had"
    );

    drag_to(
        &mut desk,
        (f64::from(at.x) + 100.0, f64::from(at.y) + 50.0),
        time + 1_000,
    );
    let followed = Rectangle::new(under.loc + Point::from((100, 50)), under.size);
    assert_eq!(
        desk.state.real_geometry(&window),
        Some(followed),
        "the window did not follow the pointer once off"
    );
    let_go(&mut desk, time + 2_000);
    desk.rest();
    assert!(!in_state(&window, State::Maximized), "maximised again");
    assert_eq!(
        desk.state.real_geometry(&window),
        Some(followed),
        "the window is not where it was let go"
    );
}

/// **#452: a window maximised out of its tile comes off at its
/// tile's size, back in its tile, and is dragged as any tiled window
/// is**: under the pointer while it is held, and let go over its
/// neighbour, the layout puts it in the tile it was let go over.
#[test]
fn a_window_maximised_out_of_its_tile_comes_off_at_its_tiles_size() {
    let mut desk = Desk::with_floating("drag-unmaximise-tiled");
    let (window, toplevel, surface) = desk.open();
    let (other, other_toplevel, other_surface) = desk.open();
    desk.press("super+t");
    desk.answer(&toplevel, &surface);
    desk.answer(&other_toplevel, &other_surface);
    let tile = desk.tile(&window).expect("the window is in a tile");
    let beside = desk.tile(&other).expect("the neighbour is in a tile");
    desk.state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    desk.press("super+shift+m");
    desk.answer(&toplevel, &surface);
    assert!(
        in_state(&window, State::Maximized) && desk.tile(&window).is_none(),
        "the premise: maximised out of its tile"
    );

    let from = desk.area.loc + Point::from((desk.area.size.w / 2, 10));
    let time = take_by_the_modifier(&mut desk, (f64::from(from.x), f64::from(from.y)));
    let at = from + Point::from((40, 200));
    drag_to(&mut desk, (f64::from(at.x), f64::from(at.y)), time);
    desk.round_trip();
    assert!(
        !in_state(&window, State::Maximized),
        "the drag did not restore the window"
    );
    assert_eq!(
        desk.tile(&window),
        Some(tile),
        "the window did not go back into its tile"
    );
    assert_eq!(
        (
            last_configured(&desk.client, &toplevel),
            tiled_sent(&desk.client, &toplevel).last()
        ),
        (Some((tile.size.w, tile.size.h)), Some(&true)),
        "the client was not told its tile"
    );
    assert_eq!(tile.size.w % 2, 0, "the premise: a half is whole");
    let under = at - Point::from((tile.size.w / 2, 10));
    assert_eq!(
        desk.state.real_geometry(&window).map(|real| real.loc),
        Some(under),
        "the window is not under the pointer"
    );

    let over = beside.loc + Point::from((beside.size.w / 2, beside.size.h / 2));
    drag_to(
        &mut desk,
        (f64::from(over.x), f64::from(over.y)),
        time + 1_000,
    );
    assert_eq!(
        desk.state.real_geometry(&window).map(|real| real.loc),
        Some(under + (over - at)),
        "the window did not follow the pointer"
    );
    let_go(&mut desk, time + 2_000);
    desk.answer(&toplevel, &surface);
    desk.answer(&other_toplevel, &other_surface);
    assert!(!in_state(&window, State::Maximized), "maximised again");
    let landed = desk.tile(&window).expect("the window is in a tile");
    assert!(
        landed.contains(over),
        "the window is not in the tile it was let go over: {landed:?}"
    );
}
