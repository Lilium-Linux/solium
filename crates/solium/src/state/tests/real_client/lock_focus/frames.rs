//! **#149: layer-shell and lock surfaces are told when they may
//! draw**, as windows always were: by a frame on their own monitor
//! and by no other monitor's, and on a dark monitor at the rate #54
//! tells a window there. See "Frame callbacks on a dark monitor" in
//! `power.rs`.

use super::*;
use zwlr_layer_surface_v1::Anchor;

/// One frame of a 60 Hz monitor.
const REFRESH: Duration = Duration::from_millis(16);

fn at(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

/// A panel as a shell draws one: its layer surface, a
/// subsurface of it and a menu it has open, each asking for
/// frame callbacks of its own.
struct Panel {
    surfaces: [wl_surface::WlSurface; 3],
    _layer: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    _subsurface: wl_subsurface::WlSubsurface,
    _menu: xdg_popup::XdgPopup,
}

impl Side {
    /// A panel across the top of the `index`th monitor, naming
    /// that monitor's `wl_output` the way a shell puts one bar
    /// on each screen.
    fn panel(&mut self, display: &mut Display<Solium>, state: &mut Solium, index: usize) -> Panel {
        let compositor = self.client.compositor.clone().expect("wl_compositor bound");
        let subcompositor = self
            .client
            .subcompositor
            .clone()
            .expect("wl_subcompositor bound");
        let shell = self
            .client
            .layer_shell
            .clone()
            .expect("zwlr_layer_shell_v1 bound");
        let wm_base = self.client.wm_base.clone().expect("xdg_wm_base bound");
        let output = self
            .client
            .outputs
            .get(index)
            .cloned()
            .expect("a wl_output for that monitor");

        let surface = compositor.create_surface(&self.qh, ());
        let layer = shell.get_layer_surface(
            &surface,
            Some(&output),
            zwlr_layer_shell_v1::Layer::Top,
            "frames-test-panel".to_string(),
            &self.qh,
            (),
        );
        layer.set_anchor(Anchor::Top | Anchor::Left | Anchor::Right);
        layer.set_size(0, 32);
        layer.set_exclusive_zone(32);
        let child = compositor.create_surface(&self.qh, ());
        let subsurface = subcompositor.get_subsurface(&child, &surface, &self.qh, ());
        subsurface.set_desync();
        surface.commit();
        self.pump(display, state);

        let menu = compositor.create_surface(&self.qh, ());
        let xdg = wm_base.get_xdg_surface(&menu, &self.qh, ());
        let positioner = wm_base.create_positioner(&self.qh, ());
        positioner.set_size(32, 32);
        positioner.set_anchor_rect(0, 0, 1, 1);
        let popup = xdg.get_popup(None, &positioner, &self.qh, ());
        layer.get_popup(&popup);
        menu.commit();
        self.pump(display, state);

        Panel {
            surfaces: [surface, child, menu],
            _layer: layer,
            _subsurface: subsurface,
            _menu: popup,
        }
    }

    /// Each of `surfaces` asks to be told when it may draw
    /// next, as a client does once it has drawn.
    fn ask_frames(
        &mut self,
        display: &mut Display<Solium>,
        state: &mut Solium,
        surfaces: &[wl_surface::WlSurface],
    ) {
        for surface in surfaces {
            surface.frame(&self.qh, FrameDone);
            surface.commit();
        }
        self.pump(display, state);
    }

    /// How many frame callbacks this client has been told are
    /// done, after a round trip to hear any.
    fn frames_done(&mut self, display: &mut Display<Solium>, state: &mut Solium) -> usize {
        self.pump(display, state);
        self.client.frames_done
    }
}

/// How many layer surfaces the server has on `output`.
fn layers_on(output: &Output) -> usize {
    smithay::desktop::layer_map_for_output(output)
        .layers()
        .count()
}

/// **A layer surface is told it may draw once a frame is drawn
/// on its monitor**, and so are its subsurface and its menu, on
/// every frame after that as well.
///
/// With only windows told, fails at "did not tell the panel".
#[test]
fn a_layer_surface_is_told_to_draw_after_a_frame_on_its_monitor() {
    let mut session = Session::new();
    let monitor = session.output(0);
    let panel = session
        .app
        .panel(&mut session.display, &mut session.state, 0);
    assert_eq!(
        layers_on(&monitor),
        1,
        "the premise: the panel is on the monitor"
    );
    assert_eq!(
        smithay::desktop::layer_map_for_output(&monitor)
            .layers()
            .next()
            .map(
                |layer| smithay::desktop::PopupManager::popups_for_surface(layer.wl_surface())
                    .count()
            ),
        Some(1),
        "the premise: the menu is the panel's popup"
    );

    session
        .app
        .ask_frames(&mut session.display, &mut session.state, &panel.surfaces);
    session.state.send_frames_on(&monitor, at(0), REFRESH);
    assert_eq!(
        session
            .app
            .frames_done(&mut session.display, &mut session.state),
        3,
        "a frame on its monitor did not tell the panel, its subsurface and its menu \
         that they may draw: a panel paced by its frame callbacks draws once and \
         then waits for ever"
    );

    for frame in 1..4 {
        session
            .app
            .ask_frames(&mut session.display, &mut session.state, &panel.surfaces);
        session
            .state
            .send_frames_on(&monitor, at(frame * 16), REFRESH);
    }
    assert_eq!(
        session
            .app
            .frames_done(&mut session.display, &mut session.state),
        12,
        "told once, and not on the frames after"
    );
}

/// **A commit that draws nothing is still told when it may
/// draw.** A client paced by its frame callbacks asks for one
/// with a commit that carries no new buffer: Firefox's vsync
/// does, between the frames it paints. The damage tracker finds
/// nothing new in that commit, so the hardware backend queues no
/// frame, and no flip comes to answer the callback. Told only on
/// a flip, the client waits for ever, and what it draws next (a
/// resize) shows only once something else -- the pointer, a
/// scene -- flips the screen. The nested backend tells every
/// client on every pass, which is why it never showed.
///
/// Played as `tty.rs` plays a pass: the window's own elements
/// through a real damage tracker, a frame queued only when it
/// finds damage, and a pass that queued none answered by
/// `tty::answer_in`, one refresh after the monitor last told its
/// clients. With a flip the only answer, fails at "never told".
#[test]
fn a_commit_that_draws_nothing_is_still_told_when_it_may_draw() {
    use smithay::backend::renderer::damage::OutputDamageTracker;
    use smithay::backend::renderer::element::surface::{
        WaylandSurfaceRenderElement, render_elements_from_surface_tree,
    };
    use smithay::backend::renderer::test::DummyRenderer;

    let mut session = Session::new();
    let monitor = session.output(0);
    let (window, _toplevel, surface, _xdg) =
        session.app.open(&mut session.display, &mut session.state);
    let root = window
        .wl_surface()
        .map(std::borrow::Cow::into_owned)
        .expect("the window's surface");
    let mut tracker =
        OutputDamageTracker::new((1920, 1080), 1.0, smithay::utils::Transform::Normal);
    // One pass: whether the tracker finds anything to draw, which
    // is whether the hardware backend queues a frame.
    let queues = |tracker: &mut OutputDamageTracker| {
        let drawn: Vec<WaylandSurfaceRenderElement<DummyRenderer>> =
            render_elements_from_surface_tree(
                &mut DummyRenderer,
                &root,
                (0, 0),
                1.0,
                1.0,
                smithay::backend::renderer::element::Kind::Unspecified,
            );
        assert!(!drawn.is_empty(), "the premise: the window is drawn");
        tracker
            .damage_output(1, &drawn)
            .expect("a mode")
            .0
            .is_some_and(|damage| !damage.is_empty())
    };

    // Its first picture is queued, and that frame's flip tells
    // the monitor's clients they may draw.
    assert!(
        queues(&mut tracker),
        "the premise: the window's first picture is drawn"
    );
    session.state.send_frames_on(&monitor, at(0), REFRESH);
    let told = at(0);

    // A commit with no new buffer, asking only to be told.
    surface.frame(&session.app.qh, FrameDone);
    surface.commit();
    session.app.pump(&mut session.display, &mut session.state);
    let queued = queues(&mut tracker);
    assert!(
        !queued,
        "the premise: a commit with nothing new in it leaves the tracker nothing to \
         draw, so no frame is queued and no flip comes"
    );

    let now = at(5);
    if let Some(delay) = crate::tty::answer_in(queued, Some(told), now, REFRESH) {
        assert_eq!(
            delay,
            at(11),
            "told sooner than a refresh after the monitor last told its clients"
        );
        session.state.send_frames_on(&monitor, now + delay, REFRESH);
    }
    assert_eq!(
        session
            .app
            .frames_done(&mut session.display, &mut session.state),
        1,
        "a commit that drew nothing was never told it may draw: no frame was queued, \
         so no flip answered it, and the client waits until something else flips \
         the screen"
    );
}

/// **A layer surface is not told it may draw by a monitor it is
/// not on**: two monitors with a panel each, each panel's client
/// its own, and a frame on one tells only its own panel.
///
/// With only windows told, fails at "did not tell its own panel";
/// with every layer surface told by every monitor, at "told the
/// right-hand panel".
#[test]
fn a_layer_surface_is_not_told_to_draw_by_a_monitor_it_is_not_on() {
    let mut session = Session::with_monitors(2);
    let (left, right) = (session.output(0), session.output(1));
    let on_left = session
        .app
        .panel(&mut session.display, &mut session.state, 0);
    let on_right = session
        .locker
        .panel(&mut session.display, &mut session.state, 1);
    assert_eq!(
        (layers_on(&left), layers_on(&right)),
        (1, 1),
        "the premise: one panel on each monitor"
    );

    session
        .app
        .ask_frames(&mut session.display, &mut session.state, &on_left.surfaces);
    session
        .locker
        .ask_frames(&mut session.display, &mut session.state, &on_right.surfaces);
    session.state.send_frames_on(&left, at(0), REFRESH);
    assert_eq!(
        session
            .app
            .frames_done(&mut session.display, &mut session.state),
        3,
        "a frame on the left-hand monitor did not tell its own panel"
    );
    assert_eq!(
        session
            .locker
            .frames_done(&mut session.display, &mut session.state),
        0,
        "a frame on the left-hand monitor told the right-hand panel to draw"
    );

    session
        .app
        .ask_frames(&mut session.display, &mut session.state, &on_left.surfaces);
    session.state.send_frames_on(&right, at(8), REFRESH);
    assert_eq!(
        session
            .locker
            .frames_done(&mut session.display, &mut session.state),
        3,
        "a frame on the right-hand monitor did not tell its own panel"
    );
    assert_eq!(
        session
            .app
            .frames_done(&mut session.display, &mut session.state),
        3,
        "a frame on the right-hand monitor told the left-hand panel to draw"
    );
}

/// **A lock surface is told it may draw while locked**, with a
/// subsurface of it, by its own monitor's frames and not by
/// another monitor's; and once its monitor is off, once a second
/// by the dark pass, as a window there is.
///
/// With only windows told, fails at "did not tell the lock
/// surface".
#[test]
fn a_lock_surface_is_told_to_draw_while_locked() {
    let mut session = Session::with_monitors(2);
    let (left, right) = (session.output(0), session.output(1));
    let _lock = session.lock();

    let on_left = session
        .state
        .lock
        .as_ref()
        .and_then(|lock| lock.surface_for(&left))
        .map(|surface| surface.wl_surface().id().protocol_id())
        .expect("a lock surface on the left-hand monitor");
    let proxy = session
        .lock_surfaces
        .iter()
        .find(|each| wayland_client::Proxy::id(&each.surface).protocol_id() == on_left)
        .expect("the lock client's own end of it");
    let role = wayland_client::Proxy::id(&proxy.role);
    let serial = session
        .locker
        .client
        .lock_configures
        .iter()
        .rev()
        .find(|(to, _)| *to == role)
        .map(|&(_, serial)| serial)
        .expect("the lock surface was configured");
    proxy.role.ack_configure(serial);

    let compositor = session
        .locker
        .client
        .compositor
        .clone()
        .expect("wl_compositor bound");
    let subcompositor = session
        .locker
        .client
        .subcompositor
        .clone()
        .expect("wl_subcompositor bound");
    let indicator = compositor.create_surface(&session.locker.qh, ());
    let subsurface =
        subcompositor.get_subsurface(&indicator, &proxy.surface, &session.locker.qh, ());
    subsurface.set_desync();
    let surfaces = [proxy.surface.clone(), indicator];

    session
        .locker
        .ask_frames(&mut session.display, &mut session.state, &surfaces);
    session.state.send_frames_on(&right, at(0), REFRESH);
    assert_eq!(
        session
            .locker
            .frames_done(&mut session.display, &mut session.state),
        0,
        "a frame on the right-hand monitor told the left-hand lock surface to draw"
    );
    session.state.send_frames_on(&left, at(16), REFRESH);
    assert_eq!(
        session
            .locker
            .frames_done(&mut session.display, &mut session.state),
        2,
        "a frame on its monitor did not tell the lock surface and its subsurface \
         that they may draw: a lock screen paced by its frame callbacks draws once \
         and never again"
    );

    session
        .locker
        .ask_frames(&mut session.display, &mut session.state, &surfaces);
    session.state.send_frames_on(&left, at(33), REFRESH);
    assert_eq!(
        session
            .locker
            .frames_done(&mut session.display, &mut session.state),
        4,
        "told once, and not on the frame after"
    );

    // The idle blank, which is what a locked screen at night is.
    assert!(
        session.state.set_power(&left, false),
        "the premise: the left-hand monitor went off"
    );
    session
        .locker
        .ask_frames(&mut session.display, &mut session.state, &surfaces);
    session.state.send_frames_on(&right, at(1_100), REFRESH);
    assert_eq!(
        session
            .locker
            .frames_done(&mut session.display, &mut session.state),
        4,
        "a lit monitor's refresh told the lock surface on a dark one to draw"
    );
    session.state.send_dark_frames(at(1_100));
    assert_eq!(
        session
            .locker
            .frames_done(&mut session.display, &mut session.state),
        6,
        "the dark pass did not tell the lock surface on a monitor that is off"
    );
    session
        .locker
        .ask_frames(&mut session.display, &mut session.state, &surfaces);
    session.state.send_dark_frames(at(1_116));
    assert_eq!(
        session
            .locker
            .frames_done(&mut session.display, &mut session.state),
        6,
        "told to draw more than once a second on a monitor that is off"
    );
}

/// **A layer surface on a dark monitor is told to draw once a
/// second, and not every frame**, exactly as a window there is
/// (`a_window_on_a_dark_monitor_is_told_to_draw_once_a_second_and_not_every_frame`):
/// a lit monitor's refresh tells it nothing, the dark pass tells
/// it at most once per `idle.off_frame_interval`, and 0 tells it
/// nothing at all.
///
/// With only windows told, fails at "lit, its monitor's refresh";
/// with the dark pass leaving layer surfaces out, at "did not
/// tell it"; with it unthrottled, at "more than once a second".
#[test]
fn a_layer_surface_on_a_dark_monitor_is_told_to_draw_once_a_second_and_not_every_frame() {
    let mut session = Session::with_monitors(2);
    let (left, right) = (session.output(0), session.output(1));
    let panel = session
        .app
        .panel(&mut session.display, &mut session.state, 0);
    assert_eq!(layers_on(&left), 1, "the premise: the panel is on the left");
    let ask = |session: &mut Session| {
        session
            .app
            .ask_frames(&mut session.display, &mut session.state, &panel.surfaces);
    };
    let done = |session: &mut Session| {
        session
            .app
            .frames_done(&mut session.display, &mut session.state)
    };

    ask(&mut session);
    session.state.send_frames_on(&left, at(0), REFRESH);
    assert_eq!(
        done(&mut session),
        3,
        "lit, its monitor's refresh told it to draw"
    );

    let control = session.control(0);
    session.power(&control, false);
    ask(&mut session);
    session.state.send_frames_on(&right, at(1_100), REFRESH);
    session.state.send_frames_on(&left, at(1_100), REFRESH);
    assert_eq!(
        done(&mut session),
        3,
        "a monitor's refresh -- the lit one's, or the dark one's own were a \
         backend to ask -- told a panel on a monitor that is off to draw"
    );
    session.state.send_dark_frames(at(1_100));
    assert_eq!(
        done(&mut session),
        6,
        "a second on, the dark pass did not tell it"
    );

    ask(&mut session);
    for step in 1..60 {
        session.state.send_dark_frames(at(1_100 + step * 16));
    }
    assert_eq!(
        done(&mut session),
        6,
        "told to draw more than once a second on a monitor that is off"
    );
    session.state.send_dark_frames(at(2_150));
    assert_eq!(done(&mut session), 9, "and not at all a second later");

    session.state.idle.configure(crate::idle::Settings {
        off_frame_interval: Duration::ZERO,
        ..crate::idle::Settings::default()
    });
    ask(&mut session);
    session.state.send_dark_frames(at(60_000));
    assert_eq!(
        done(&mut session),
        9,
        "idle.off_frame_interval = 0 still told it to draw"
    );
}
