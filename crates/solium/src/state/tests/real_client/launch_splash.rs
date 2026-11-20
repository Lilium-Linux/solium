//! **#128: the layout hears a close when it is asked for, and hears a
//! refusal when the window comes back.**
//!
//! The compositor's half of the contract, against the shipped layouts
//! and a real client: when `closing`, `refused` and `close` are sent,
//! in what order, over which routes, and what the window being closed
//! looks like meanwhile. What the layouts *do* with each event is
//! `script.rs`'s `reflow_on_close` module, which can drive them without
//! a display.
//!
//! #295: a launched application's first window is not always its main
//! one, and its first frame is not always at the size it was told. Its
//! launch's window stays at its slot through both.

use super::*;

/// One monitor, no scripts, no frame style, and a client.
struct Desk {
    display: Display<Solium>,
    state: Solium,
    conn: Connection,
    queue: wayland_client::EventQueue<Client>,
    qh: QueueHandle<Client>,
    client: Client,
}

/// A toplevel of the test client, and what a test needs to answer
/// for it.
struct Opened {
    surface: wl_surface::WlSurface,
    xdg: xdg_surface::XdgSurface,
    toplevel: xdg_toplevel::XdgToplevel,
    window: Window,
}

impl Desk {
    fn new() -> Self {
        let mut display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        state
            .decorations
            .set_style(&mut state.panes, Some("none".to_string()));
        a_screen(&mut state, "splash-test", (0, 0));
        let (conn, queue, client) = connect(&mut display, &mut state);
        let qh = queue.handle();
        Self {
            display,
            state,
            conn,
            queue,
            qh,
            client,
        }
    }

    fn pump(&mut self) {
        pump(
            &mut self.display,
            &mut self.state,
            &self.conn,
            &self.qh,
            &mut self.queue,
            &mut self.client,
        );
    }

    /// A window opened for an application of this process, as
    /// `sol.spawn` opens one.
    fn launch(&mut self) -> crate::pane::PaneId {
        let source = crate::pane::loading_source(None);
        self.state
            .open_loading("app", Some(std::process::id()), source, None)
    }

    /// A toplevel, its limits if `fixed` names them, and its first,
    /// bufferless commit.
    fn toplevel(&mut self, fixed: Option<(i32, i32)>) -> Opened {
        let compositor = self.client.compositor.clone().expect("wl_compositor bound");
        let wm_base = self.client.wm_base.clone().expect("xdg_wm_base bound");
        let before: Vec<Window> = self.state.space.elements().cloned().collect();
        let surface = compositor.create_surface(&self.qh, ());
        let xdg = wm_base.get_xdg_surface(&surface, &self.qh, ());
        let toplevel = xdg.get_toplevel(&self.qh, ());
        self.client.leave_frame(&toplevel, &self.qh);
        if let Some((w, h)) = fixed {
            toplevel.set_min_size(w, h);
            toplevel.set_max_size(w, h);
        }
        surface.commit();
        self.pump();
        let window = self
            .state
            .space
            .elements()
            .find(|window| !before.contains(window))
            .cloned()
            .expect("new_toplevel mapped a window");
        Opened {
            surface,
            xdg,
            toplevel,
            window,
        }
    }

    /// The sizes `opened` has been configured with, in order.
    fn told(&self, opened: &Opened) -> Vec<(i32, i32)> {
        let id = wayland_client::Proxy::id(&opened.toplevel);
        self.client
            .configures
            .iter()
            .filter(|(toplevel, _, _)| *toplevel == id)
            .map(|(_, w, h)| (*w, *h))
            .collect()
    }

    /// `opened` acks the last configure it was sent and draws at
    /// `w`x`h`: a client answering.
    fn answer(&mut self, opened: &Opened, w: i32, h: i32) {
        let id = wayland_client::Proxy::id(&opened.xdg);
        let serial = self
            .client
            .surface_configures
            .iter()
            .rev()
            .find(|(xdg, _)| *xdg == id)
            .map(|(_, serial)| *serial)
            .expect("the window was configured");
        opened.xdg.ack_configure(serial);
        commit_buffer(&self.client, &self.qh, &opened.surface, w, h);
        self.pump();
    }

    /// A frame `after` from now, retired as a frame retires it.
    fn frame_after(&mut self, after: Duration) {
        self.state.clock.advance(after);
        let now = self.state.clock.now();
        self.state.space.refresh();
        self.state.settle(now);
        self.state.sync_panes();
        self.pump();
    }

    /// Where `pane` lives, frame included, and where it is drawn now.
    fn at(&self, pane: crate::pane::PaneId) -> (Rectangle<i32, Logical>, Rectangle<f64, Logical>) {
        let held = self.state.panes.get(pane).expect("the pane is here");
        let outer = self.state.pane_outer(held);
        let drawn = self
            .state
            .drawn_at(held, outer, self.state.clock.now())
            .rect;
        (outer, drawn)
    }

    /// `pane` is where `slot` is, and is drawn there: `when` says
    /// at which step of the test.
    fn assert_at_slot(&self, pane: crate::pane::PaneId, slot: Rectangle<i32, Logical>, when: &str) {
        let (outer, drawn) = self.at(pane);
        assert_eq!(outer, slot, "{when}: the launch's window left its slot");
        assert_eq!(
            drawn,
            slot.to_f64(),
            "{when}: the launch's window was drawn away from its slot"
        );
    }
}

/// **A splash is shown over the launch, and the main window takes the
/// slot.** Vesktop, launched, maps a 300x350 splash -- one size and
/// only one -- a second before its main window, and closes it as the
/// main window maps. The launch's window took the splash: it shrank to
/// 300x350 in its slot's corner, left with it, and the main window
/// opened as a window of its own. Now the splash is a window of its own
/// over the middle of the launch's, at its size, above it, out of the
/// window list and told it may keep its size; the launch's window is
/// at its slot and drawn there throughout, still waiting, with its
/// patience started again when the splash goes; and the main window
/// arrives in it, told the slot's size in its first configure, held
/// at the slot until it answers, and fills it.
#[test]
fn a_splash_is_shown_over_the_launch_and_the_main_window_takes_the_slot() {
    let mut desk = Desk::new();
    let launch = desk.launch();
    let (slot, _) = desk.at(launch);
    desk.assert_at_slot(launch, slot, "launched");

    let splash = desk.toplevel(Some((300, 350)));
    let stand_in = desk
        .state
        .panes
        .id_of(&splash.window)
        .expect("the splash has a pane");
    assert_ne!(stand_in, launch, "the splash took the launch's window");
    assert!(
        desk.state.panes.get(launch).is_some_and(Pane::is_loading),
        "the launch's window stopped waiting for its application"
    );
    assert_eq!(
        desk.told(&splash).last(),
        Some(&(0, 0)),
        "the splash was not told it may keep its own size"
    );
    commit_buffer(&desk.client, &desk.qh, &splash.surface, 300, 350);
    desk.pump();
    desk.frame_after(Duration::from_millis(16));
    desk.assert_at_slot(launch, slot, "the splash drawn");
    let held = desk.state.panes.get(stand_in).expect("the splash's pane");
    assert!(
        !held.managed(),
        "the splash is a window a layout makes room for"
    );
    assert_eq!(held.stands_for(), Some(launch));
    let centre = |rect: Rectangle<i32, Logical>| {
        (rect.loc.x + rect.size.w / 2, rect.loc.y + rect.size.h / 2)
    };
    let shown = desk.state.pane_geometry(held);
    assert_eq!(
        shown.size,
        Size::from((300, 350)),
        "the splash is not at its size"
    );
    assert!(
        centre(shown).0.abs_diff(centre(slot).0) <= 1
            && centre(shown).1.abs_diff(centre(slot).1) <= 1,
        "the splash {shown:?} is not over the middle of the launch's window {slot:?}"
    );
    let order: Vec<crate::pane::PaneId> = desk.state.panes.iter().map(Pane::id).collect();
    let index = |id| order.iter().position(|each| *each == id);
    assert!(
        index(stand_in) > index(launch),
        "the splash is under the launch's window: {order:?}"
    );
    assert!(
        desk.state
            .snapshot()
            .windows
            .iter()
            .all(|row| row.id != stand_in.get()),
        "the splash is in the window list a layout reads"
    );

    // Its patience is nearly out when the splash goes, and starts
    // again: the main window is on its way.
    let patience = desk.state.loading.patience;
    desk.frame_after(patience.saturating_sub(Duration::from_millis(100)));
    splash.toplevel.destroy();
    splash.xdg.destroy();
    splash.surface.destroy();
    desk.pump();
    desk.frame_after(Duration::from_millis(200));
    assert!(
        desk.state.panes.get(launch).is_some_and(Pane::is_loading),
        "the launch gave up as its splash went"
    );
    assert!(
        desk.state.panes.get(stand_in).is_none(),
        "the splash's pane outlived it"
    );
    desk.assert_at_slot(launch, slot, "the splash gone");

    let main = desk.toplevel(None);
    assert_eq!(
        desk.state.panes.id_of(&main.window),
        Some(launch),
        "the main window did not arrive in the launch's window"
    );
    assert_eq!(
        desk.told(&main).first(),
        Some(&(slot.size.w, slot.size.h)),
        "the main window's first configure was not the slot's size"
    );
    desk.assert_at_slot(launch, slot, "the main window mapped");
    assert!(
        desk.state.holding_resize(launch),
        "the main window is not held at the slot it was told"
    );
    desk.answer(&main, slot.size.w, slot.size.h);
    desk.frame_after(Duration::from_millis(16));
    desk.assert_at_slot(launch, slot, "the main window drawn");
    assert!(
        !desk.state.holding_resize(launch),
        "the main window is held after answering"
    );
}

/// **A launched window's shape does not change when the windows that
/// arrive in it draw their own decorations**: the room the launch's
/// window kept for a frame is the client's then, and handed back to
/// the frame when a splash is shown on its own instead. Vesktop's
/// splash and main window both draw their own, and the window shrank
/// by a titlebar as its main window arrived (483 tall in a 515 slot)
/// before growing to the main window's 500 minimum.
#[test]
fn a_launched_window_keeps_its_shape_when_what_arrives_draws_its_own_frame() {
    let mut desk = Desk::new();
    let launch = desk.launch();
    // A frame is coming, as the shipped style's is: room reserved for
    // it. (A built frame is a Qt scene, which this test cannot have.)
    desk.state
        .panes
        .get_mut(launch)
        .expect("the launch's window")
        .set_frame(crate::pane::Frame::Pending);
    let (outer, _) = desk.at(launch);
    let client = desk
        .state
        .panes
        .get(launch)
        .map(Pane::slot)
        .expect("its slot");
    assert!(
        outer.size.h > client.size.h,
        "the premise: room for a frame"
    );

    for (fixed, what) in [(Some((300, 350)), "the splash"), (None, "the main window")] {
        let compositor = desk.client.compositor.clone().expect("wl_compositor bound");
        let wm_base = desk.client.wm_base.clone().expect("xdg_wm_base bound");
        let before: Vec<Window> = desk.state.space.elements().cloned().collect();
        let surface = compositor.create_surface(&desk.qh, ());
        let xdg = wm_base.get_xdg_surface(&surface, &desk.qh, ());
        let toplevel = xdg.get_toplevel(&desk.qh, ());
        if let Some((w, h)) = fixed {
            toplevel.set_min_size(w, h);
            toplevel.set_max_size(w, h);
        }
        desk.pump();
        let window = desk
            .state
            .space
            .elements()
            .find(|window| !before.contains(window))
            .cloned()
            .expect("new_toplevel mapped a window");
        let server = window.toplevel().expect("an xdg toplevel").clone();
        desk.state.decorate(
            &server,
            smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode::ClientSide,
        );
        desk.assert_at_slot(
            launch,
            outer,
            &format!("{what} asked to draw its own frame"),
        );
        surface.commit();
        desk.pump();
        desk.frame_after(Duration::from_millis(16));
        desk.assert_at_slot(launch, outer, &format!("{what} committed"));
        let opened = Opened {
            surface,
            xdg,
            toplevel,
            window,
        };
        if fixed.is_some() {
            // Its frame is rebuilt from the configured style, and the
            // slot is what is left of the window inside it: `none`
            // here, which reserves nothing, so the whole window.
            assert!(
                desk.state.panes.get(launch).is_some_and(Pane::is_loading),
                "the premise: the splash stands in"
            );
            commit_buffer(&desk.client, &desk.qh, &opened.surface, 300, 350);
            desk.pump();
            desk.frame_after(Duration::from_millis(16));
            desk.assert_at_slot(launch, outer, "the splash drawn");
            opened.toplevel.destroy();
            opened.xdg.destroy();
            opened.surface.destroy();
            desk.pump();
            desk.frame_after(Duration::from_millis(16));
        } else {
            assert_eq!(
                desk.told(&opened).last(),
                Some(&(outer.size.w, outer.size.h)),
                "the main window was not told the whole of the launch's window"
            );
            desk.answer(&opened, outer.size.w, outer.size.h);
            desk.frame_after(Duration::from_millis(16));
            desk.assert_at_slot(launch, outer, "the main window drawn");
            assert!(!desk.state.holding_resize(launch), "held after answering");
        }
    }
}

/// **A launched window that draws once at its own size before it
/// answers is drawn at its slot, under its scene** (#295's first
/// report: a 390x748 Qt window, drawn for one frame at 390x748 in its
/// slot's corner over the wallpaper). It is held at the slot until it
/// commits what it was told, and the scene fades only then.
#[test]
fn a_launched_window_that_draws_at_its_own_size_before_it_answers_stays_at_its_slot() {
    let mut desk = Desk::new();
    let launch = desk.launch();
    let (slot, _) = desk.at(launch);
    let main = desk.toplevel(None);
    assert_eq!(desk.state.panes.id_of(&main.window), Some(launch));

    commit_buffer(&desk.client, &desk.qh, &main.surface, 390, 748);
    desk.pump();
    for _ in 0..3 {
        desk.frame_after(Duration::from_millis(16));
        desk.assert_at_slot(launch, slot, "drawn at its own size");
        // What keeps the scene over it too: `sync_panes` fades no
        // scene off a pane that is held. A test client cannot have a
        // scene (the #99 rule), so the hold is what is asked.
        assert!(
            desk.state.holding_resize(launch),
            "a window not yet at its slot's size is not held there"
        );
    }

    desk.answer(&main, slot.size.w, slot.size.h);
    desk.frame_after(Duration::from_millis(16));
    desk.assert_at_slot(launch, slot, "answered");
    assert!(
        !desk.state.holding_resize(launch),
        "held once it has answered"
    );
}

/// **And one that never answers is drawn as it is once
/// `resize.patience` after its first frame has run out**: the hold
/// is bounded, as every hold is.
#[test]
fn a_launched_window_that_never_answers_is_drawn_as_it_is_once_its_patience_runs_out() {
    let mut desk = Desk::new();
    let launch = desk.launch();
    let main = desk.toplevel(None);
    commit_buffer(&desk.client, &desk.qh, &main.surface, 390, 748);
    desk.pump();
    desk.frame_after(Duration::from_millis(16));
    assert!(desk.state.holding_resize(launch), "the premise: held");
    desk.frame_after(desk.state.resizing.patience);
    desk.frame_after(Duration::from_millis(16));
    assert!(!desk.state.holding_resize(launch), "held past its patience");
    let held = desk.state.panes.get(launch).expect("the window");
    assert_eq!(
        desk.state.pane_geometry(held).size,
        Size::from((390, 748)),
        "not drawn at the size it committed"
    );
}

/// **A window standing in for a launch that brings the launch's
/// token is still standing in**, and the launch's window still
/// waiting: Vesktop's splash brings the token, a second before the
/// main window maps, and claimed by it the splash went back into the
/// launch's window.
#[test]
fn a_window_standing_in_that_brings_the_launchs_token_still_stands_in() {
    let mut desk = Desk::new();
    let launch = desk.launch();
    let (slot, _) = desk.at(launch);
    let token = desk.state.launch_token(launch);
    let splash = desk.toplevel(Some((300, 350)));
    commit_buffer(&desk.client, &desk.qh, &splash.surface, 300, 350);
    desk.pump();
    let stand_in = desk.state.panes.id_of(&splash.window).expect("a pane");
    assert_ne!(stand_in, launch, "the premise: it stands in");

    let activation = desk
        .client
        .activation
        .clone()
        .expect("xdg_activation_v1 bound");
    activation.activate(token, &splash.surface);
    desk.pump();
    desk.frame_after(Duration::from_millis(16));
    assert_eq!(
        desk.state.panes.id_of(&splash.window),
        Some(stand_in),
        "the splash was claimed into the launch's window by its token"
    );
    assert_eq!(
        desk.state.panes.get(stand_in).and_then(Pane::stands_for),
        Some(launch),
        "the splash stopped standing in"
    );
    assert!(
        desk.state.panes.get(launch).is_some_and(Pane::is_loading),
        "the launch's window stopped waiting"
    );
    desk.assert_at_slot(launch, slot, "the token brought");
}

/// **A window still standing in when the launch's patience runs out is
/// the application's window**: a fixed-size application window, such
/// as a calculator, and not a splash. The launch's window goes, and it
/// becomes an ordinary window, in the window list.
#[test]
fn a_window_still_standing_in_when_the_launchs_patience_runs_out_becomes_the_window() {
    let mut desk = Desk::new();
    let launch = desk.launch();
    let fixed = desk.toplevel(Some((300, 350)));
    commit_buffer(&desk.client, &desk.qh, &fixed.surface, 300, 350);
    desk.pump();
    let stand_in = desk.state.panes.id_of(&fixed.window).expect("a pane");
    let patience = desk.state.loading.patience;
    desk.frame_after(patience.saturating_sub(Duration::from_millis(16)));
    assert!(
        desk.state.panes.get(launch).is_some_and(Pane::is_loading)
            && desk
                .state
                .panes
                .get(stand_in)
                .is_some_and(|held| !held.managed()),
        "the premise: still waiting, and standing in, before its patience is out"
    );
    desk.frame_after(Duration::from_millis(32));
    let held = desk.state.panes.get(stand_in).expect("the window");
    assert!(held.managed(), "it is not an ordinary window");
    assert_eq!(held.stands_for(), None, "it still stands in");
    assert!(
        desk.state
            .panes
            .get(launch)
            .is_none_or(|pane| pane.ghost() || pane.leaving()),
        "the launch's window is still waiting"
    );
    assert!(
        desk.state
            .snapshot()
            .windows
            .iter()
            .any(|row| row.id == stand_in.get()),
        "it is not in the window list"
    );
}
