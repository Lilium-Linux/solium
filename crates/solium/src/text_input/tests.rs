//! `zwp_text_input_v3` against a real client on a socketpair: what a client
//! says about its text field, and what the compositor makes of it.

use std::os::unix::io::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use smithay::desktop::Window;
use smithay::reexports::wayland_server::Display;
use smithay::utils::{Logical, Rectangle, SERIAL_COUNTER};
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_subcompositor, wl_subsurface, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols::wp::text_input::zv3::client::{
    zwp_text_input_manager_v3, zwp_text_input_v3,
};
use wayland_protocols::xdg::shell::client::{
    xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
};

use crate::present::{self, Frame};
use crate::script::Scripts;
use crate::state::{ClientState, Solium};

/// The client side: a window, and a text input on the seat.
#[derive(Debug, Default)]
struct Client {
    compositor: Option<wl_compositor::WlCompositor>,
    subcompositor: Option<wl_subcompositor::WlSubcompositor>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    shm: Option<wl_shm::WlShm>,
    seat: Option<wl_seat::WlSeat>,
    manager: Option<zwp_text_input_manager_v3::ZwpTextInputManagerV3>,
    /// Every `enter` (true) and `leave` (false), with the surface's id.
    told: Vec<(bool, u32)>,
    /// Every `xdg_surface.configure`, for a popup to ack.
    configures: Vec<(wayland_client::backend::ObjectId, u32)>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Client {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        (): &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name, interface, ..
        } = event
        else {
            return;
        };
        match interface.as_str() {
            "wl_compositor" => state.compositor = Some(registry.bind(name, 1, qh, ())),
            "wl_subcompositor" => state.subcompositor = Some(registry.bind(name, 1, qh, ())),
            "xdg_wm_base" => state.wm_base = Some(registry.bind(name, 1, qh, ())),
            "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
            "wl_seat" => state.seat = Some(registry.bind(name, 1, qh, ())),
            "zwp_text_input_manager_v3" => state.manager = Some(registry.bind(name, 1, qh, ())),
            _ => {}
        }
    }
}

impl Dispatch<zwp_text_input_v3::ZwpTextInputV3, ()> for Client {
    fn event(
        state: &mut Self,
        _text_input: &zwp_text_input_v3::ZwpTextInputV3,
        event: zwp_text_input_v3::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwp_text_input_v3::Event::Enter { surface } => {
                state
                    .told
                    .push((true, wayland_client::Proxy::id(&surface).protocol_id()));
            }
            zwp_text_input_v3::Event::Leave { surface } => {
                state
                    .told
                    .push((false, wayland_client::Proxy::id(&surface).protocol_id()));
            }
            _ => {}
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for Client {
    fn event(
        state: &mut Self,
        surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            state
                .configures
                .push((wayland_client::Proxy::id(surface), serial));
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Client {
    fn event(
        _state: &mut Self,
        base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            base.pong(serial);
        }
    }
}

wayland_client::delegate_noop!(Client: ignore wl_compositor::WlCompositor);
wayland_client::delegate_noop!(Client: ignore wl_subcompositor::WlSubcompositor);
wayland_client::delegate_noop!(Client: ignore wl_subsurface::WlSubsurface);
wayland_client::delegate_noop!(Client: ignore wl_surface::WlSurface);
wayland_client::delegate_noop!(Client: ignore wl_shm::WlShm);
wayland_client::delegate_noop!(Client: ignore wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(Client: ignore wl_buffer::WlBuffer);
wayland_client::delegate_noop!(Client: ignore wl_seat::WlSeat);
wayland_client::delegate_noop!(Client: ignore wl_callback::WlCallback);
wayland_client::delegate_noop!(Client: ignore xdg_toplevel::XdgToplevel);
wayland_client::delegate_noop!(Client: ignore xdg_popup::XdgPopup);
wayland_client::delegate_noop!(Client: ignore xdg_positioner::XdgPositioner);
wayland_client::delegate_noop!(Client: ignore zwp_text_input_manager_v3::ZwpTextInputManagerV3);

/// A compositor and a client of it, with every global in the client's
/// registry. Also what `scenario` drives the shipped configuration with.
pub(crate) struct Desk {
    display: Display<Solium>,
    pub(crate) state: Solium,
    conn: Connection,
    queue: EventQueue<Client>,
    qh: QueueHandle<Client>,
    client: Client,
}

impl Desk {
    pub(crate) fn new() -> Self {
        let mut display = Display::<Solium>::new().expect("a test display");
        let mut state = Solium::new(display.handle());
        // No frames: a Qt scene built in a process that holds a raw
        // libwayland connection takes the test binary down, as the real-client
        // tests in `state::tests` say.
        state
            .decorations
            .set_style(&mut state.panes, Some("none".to_owned()));
        let (server, client_side) = UnixStream::pair().expect("a socketpair");
        display
            .handle()
            .insert_client(server, std::sync::Arc::new(ClientState::default()))
            .expect("inserting the test client");
        let conn = Connection::from_socket(client_side).expect("wrapping the client socket");
        let mut queue = conn.new_event_queue::<Client>();
        let qh = queue.handle();
        let mut client = Client::default();
        conn.display().get_registry(&qh, ());
        conn.flush().expect("flushing get_registry");
        display.dispatch_clients(&mut state).expect("dispatching");
        display.flush_clients().expect("flushing the registry");
        queue
            .blocking_dispatch(&mut client)
            .expect("reading the registry");
        Self {
            display,
            state,
            conn,
            queue,
            qh,
            client,
        }
    }

    /// One round trip, made safe to block on by a `sync`.
    pub(crate) fn pump(&mut self) {
        self.conn.display().sync(&self.qh, ());
        self.conn.flush().expect("flushing the round trip");
        self.display
            .dispatch_clients(&mut self.state)
            .expect("dispatching the round trip");
        self.display.flush_clients().expect("flushing the events");
        self.queue
            .blocking_dispatch(&mut self.client)
            .expect("reading the events");
    }

    /// A window, 200 by 100, with the server's `Window` for it.
    pub(crate) fn window(&mut self) -> (Window, wl_surface::WlSurface, xdg_surface::XdgSurface) {
        let compositor = self.client.compositor.clone().expect("wl_compositor bound");
        let wm_base = self.client.wm_base.clone().expect("xdg_wm_base bound");
        let before: Vec<Window> = self.state.space.elements().cloned().collect();
        let surface = compositor.create_surface(&self.qh, ());
        let xdg = wm_base.get_xdg_surface(&surface, &self.qh, ());
        let _toplevel = xdg.get_toplevel(&self.qh, ());
        self.buffer(&surface, 200, 100);
        self.pump();
        let window = self
            .state
            .space
            .elements()
            .find(|window| !before.contains(window))
            .cloned()
            .expect("the window was mapped");
        // Past the window's opening animation, and that animation retired as
        // a frame retires it, so the window is drawn where it lives.
        self.state.clock.advance(Duration::from_secs(1));
        let now = self.state.clock.now();
        if let Some(pane) = self
            .state
            .panes
            .id_of(&window)
            .and_then(|id| self.state.panes.get(id))
        {
            present::settle(pane, now);
        }
        (window, surface, xdg)
    }

    /// Attach a buffer of this size and commit it.
    pub(crate) fn buffer(&self, surface: &wl_surface::WlSurface, width: i32, height: i32) {
        let shm = self.client.shm.clone().expect("wl_shm bound");
        let bytes = width * height * 4;
        let fd = anon_file(bytes);
        let pool = shm.create_pool(fd.as_fd(), bytes, &self.qh, ());
        let buffer = pool.create_buffer(
            0,
            width,
            height,
            width * 4,
            wl_shm::Format::Argb8888,
            &self.qh,
            (),
        );
        surface.attach(Some(&buffer), 0, 0);
        surface.damage(0, 0, width, height);
        surface.commit();
    }

    /// The client's text input on its seat.
    pub(crate) fn text_input(&mut self) -> zwp_text_input_v3::ZwpTextInputV3 {
        let manager = self
            .client
            .manager
            .clone()
            .expect("the text-input manager bound");
        let seat = self.client.seat.clone().expect("wl_seat bound");
        let text_input = manager.get_text_input(&seat, &self.qh, ());
        self.pump();
        text_input
    }

    /// Give `window` the keyboard, as a click on it would.
    pub(crate) fn focus(&mut self, window: &Window) {
        self.state
            .focus_window(window, SERIAL_COUNTER.next_serial());
        self.pump();
    }

    /// The configuration, loaded.
    pub(crate) fn configure(&mut self, name: &str, script: &str) {
        let directory = std::env::temp_dir().join(format!("solium-text-input-{name}"));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        std::fs::write(&config, script).expect("writing the script");
        let scripts = Scripts::load(&config).expect("loading the script");
        self.state.start_scripts(Some(scripts));
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// What the configuration has recorded in `seen`, and cleared.
    pub(crate) fn seen(&self) -> String {
        self.state
            .scripts
            .as_ref()
            .map_or_else(String::new, |scripts| {
                scripts.evaluate("local all = table.concat(seen, ' '); seen = {}; return all")
            })
    }
}

/// Enable a field with its caret at `caret`, and commit.
pub(crate) fn enable(text_input: &zwp_text_input_v3::ZwpTextInputV3, caret: (i32, i32, i32, i32)) {
    text_input.enable();
    text_input.set_cursor_rectangle(caret.0, caret.1, caret.2, caret.3);
    text_input.commit();
}

/// An unlinked file of `size` bytes, for a `wl_shm_pool`.
fn anon_file(size: i32) -> OwnedFd {
    let path = std::env::temp_dir().join(format!(
        "solium-text-input-test-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .expect("creating a backing file");
    std::fs::remove_file(&path).expect("unlinking it");
    file.set_len(u64::from(size.unsigned_abs()))
        .expect("sizing it");
    file.into()
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Rectangle<f64, Logical> {
    Rectangle::new((x, y).into(), (w, h).into())
}

/// **An enabled field has its caret in the global space**: a client that
/// enables a text field and says where its caret is, and commits, has it at
/// its window's place plus the caret's, and `sol.text_input()` says so,
/// with the window's id. Before the commit there is nothing.
#[test]
fn an_enabled_field_has_its_caret_in_the_global_space() {
    let mut desk = Desk::new();
    let (window, _surface, _xdg) = desk.window();
    desk.state
        .space
        .map_element(window.clone(), (300, 200), false);
    desk.focus(&window);
    let text_input = desk.text_input();
    text_input.enable();
    text_input.set_cursor_rectangle(10, 20, 2, 16);
    desk.pump();
    assert_eq!(
        desk.state.text_field(),
        None,
        "nothing is applied before a commit"
    );

    text_input.commit();
    desk.pump();
    let id = desk
        .state
        .panes
        .id_of(&window)
        .expect("the window has a pane")
        .get();
    let field = desk.state.text_field().expect("a field is enabled");
    assert_eq!(field.window, id);
    assert_eq!(field.caret, Some(rect(310.0, 220.0, 2.0, 16.0)));

    desk.configure(
        "global",
        "function where() local f = sol.text_input(); return f and string.format('%d:%d,%d,%d,%d', f.window, f.x, f.y, f.w, f.h) or 'none' end",
    );
    let snapshot = desk.state.snapshot();
    let read = desk
        .state
        .scripts
        .as_ref()
        .map(|scripts| scripts.evaluate_in(snapshot, "return where()"));
    assert_eq!(read, Some(format!("{id}:310,220,2,16")));
}

/// **A field says whether its window is framed**: bare under the style
/// `"none"` the desk runs with, framed once its pane may have a frame again --
/// as a window leaving fullscreen may -- and bare again as a window going
/// fullscreen is left. `sol.text_input()` says the same, as `framed`.
#[test]
fn a_field_says_whether_its_window_is_framed() {
    let mut desk = Desk::new();
    let (window, _surface, _xdg) = desk.window();
    desk.focus(&window);
    let text_input = desk.text_input();
    enable(&text_input, (10, 20, 2, 16));
    desk.pump();
    let framed = |desk: &Desk| desk.state.text_field().map(|field| field.framed);
    assert_eq!(framed(&desk), Some(false), "bare under the style \"none\"");

    let id = desk.state.panes.id_of(&window).expect("a pane");
    desk.state.decorations.unset_bare(&mut desk.state.panes, id);
    assert_eq!(framed(&desk), Some(true), "a frame on its way is a frame");

    desk.configure(
        "framed",
        "function framed() local f = sol.text_input(); return tostring(f and f.framed) end",
    );
    let snapshot = desk.state.snapshot();
    let read = desk
        .state
        .scripts
        .as_ref()
        .map(|scripts| scripts.evaluate_in(snapshot, "return framed()"));
    assert_eq!(
        read.as_deref(),
        Some("true"),
        "and the configuration is told"
    );

    desk.state.decorations.set_bare(&mut desk.state.panes, id);
    assert_eq!(framed(&desk), Some(false), "bare, as fullscreen leaves it");
}

/// **A text input made after the keyboard arrived is entered at once**: a
/// client binding text-input late is told which surface has the focus.
#[test]
fn a_text_input_made_after_the_keyboard_arrived_is_entered_at_once() {
    let mut desk = Desk::new();
    let (window, surface, _xdg) = desk.window();
    desk.focus(&window);
    let _text_input = desk.text_input();
    assert_eq!(
        desk.client.told,
        [(true, wayland_client::Proxy::id(&surface).protocol_id())]
    );
}

/// **The caret follows its window, moved and presented**: moved, the caret
/// moves with it; drawn at half its size somewhere else by a presentation
/// transform, the caret is where the transform draws it, at half its size.
#[test]
fn the_caret_follows_its_window_moved_and_presented() {
    let mut desk = Desk::new();
    let (window, _surface, _xdg) = desk.window();
    desk.focus(&window);
    let text_input = desk.text_input();
    enable(&text_input, (10, 20, 2, 16));
    desk.pump();

    desk.state
        .space
        .map_element(window.clone(), (700, 40), false);
    let caret = desk.state.text_field().and_then(|field| field.caret);
    assert_eq!(caret, Some(rect(710.0, 60.0, 2.0, 16.0)), "moved");

    let id = desk.state.panes.id_of(&window).expect("a pane");
    let insets = desk.state.insets_of(id);
    let pane = desk.state.panes.get(id).expect("the pane");
    let outer = desk.state.pane_outer(pane);
    let half = rect(
        1000.0,
        500.0,
        f64::from(outer.size.w) / 2.0,
        f64::from(outer.size.h) / 2.0,
    );
    let now = desk.state.clock.now();
    present::present(
        pane,
        outer,
        Frame {
            rect: half,
            ..Frame::real(outer)
        },
        now,
        Duration::ZERO,
        solium_animation::Curve::Linear,
    );
    let caret = desk.state.text_field().and_then(|field| field.caret);
    assert_eq!(
        caret,
        Some(rect(
            1000.0 + f64::from(insets.left + 10) / 2.0,
            500.0 + f64::from(insets.top + 20) / 2.0,
            1.0,
            8.0
        )),
        "presented at half its size"
    );
}

/// **A field whose window loses the keyboard is gone**, and so is one the
/// client disables, and nothing a text input says is applied while it has
/// not been entered: before any window has the keyboard, and after `leave`.
#[test]
fn a_field_whose_window_loses_the_keyboard_is_gone() {
    let mut desk = Desk::new();
    let text_input = desk.text_input();
    enable(&text_input, (10, 20, 2, 16));
    desk.pump();
    assert_eq!(
        desk.state.text_field(),
        None,
        "no window has the keyboard, so nothing was entered and the enable is not applied"
    );

    let (first, first_surface, _a) = desk.window();
    let (second, second_surface, _b) = desk.window();
    desk.focus(&first);
    enable(&text_input, (10, 20, 2, 16));
    desk.pump();
    assert!(desk.state.text_field().is_some(), "entered, then enabled");

    text_input.disable();
    text_input.commit();
    desk.pump();
    assert_eq!(desk.state.text_field(), None, "disabled");

    enable(&text_input, (10, 20, 2, 16));
    desk.pump();
    desk.client.told.clear();
    desk.focus(&second);
    assert_eq!(desk.state.text_field(), None, "the keyboard moved");
    let first_id = wayland_client::Proxy::id(&first_surface).protocol_id();
    let second_id = wayland_client::Proxy::id(&second_surface).protocol_id();
    assert_eq!(desk.client.told, [(false, first_id), (true, second_id)]);

    enable(&text_input, (1, 2, 3, 4));
    desk.pump();
    assert!(
        desk.state.text_field().is_some(),
        "enabled where it was entered"
    );
}

/// **A field whose window leaves the keyboard on nothing is gone**, and the
/// client is told `leave`, then `enter` again when the keyboard comes back.
/// Smithay tells `focus_changed` of a new surface and never of none, which is
/// what the keyboard is left on when the last window on screen closes and as
/// the session locks.
#[test]
fn a_field_whose_window_leaves_the_keyboard_on_nothing_is_gone() {
    let mut desk = Desk::new();
    let (window, surface, _xdg) = desk.window();
    desk.focus(&window);
    let text_input = desk.text_input();
    enable(&text_input, (10, 20, 2, 16));
    desk.pump();
    assert!(desk.state.text_field().is_some(), "entered, then enabled");

    let id = wayland_client::Proxy::id(&surface).protocol_id();
    desk.client.told.clear();
    desk.state.give_keyboard(None, SERIAL_COUNTER.next_serial());
    desk.pump();
    assert_eq!(desk.state.text_field(), None, "the keyboard is on nothing");
    assert_eq!(desk.client.told, [(false, id)], "and the field was told so");

    desk.focus(&window);
    assert_eq!(
        desk.client.told,
        [(false, id), (true, id)],
        "and told again when the keyboard comes back"
    );
}

/// **`text_input` is told when a field is enabled and when it is focused**:
/// once for the enable, nothing for a caret that only moves, and once more
/// when its window gets the keyboard back and the client enables it again,
/// as clients do on `enter`.
#[test]
fn text_input_is_told_when_a_field_is_enabled_and_when_it_is_focused() {
    let mut desk = Desk::new();
    let (first, _a, _xa) = desk.window();
    let (second, _b, _xb) = desk.window();
    desk.state.space.map_element(first.clone(), (0, 0), false);
    desk.focus(&first);
    desk.configure(
        "event",
        r#"
        seen = {}
        sol.on("text_input", function(field)
            seen[#seen + 1] = string.format("%g,%g", field.x, field.y)
        end)
        "#,
    );
    let text_input = desk.text_input();
    enable(&text_input, (10, 20, 2, 16));
    desk.pump();
    assert_eq!(desk.seen(), "10,20", "enabled");

    text_input.set_cursor_rectangle(30, 20, 2, 16);
    text_input.commit();
    desk.pump();
    assert_eq!(desk.seen(), "", "a caret that moves is not a field");

    desk.focus(&second);
    desk.focus(&first);
    enable(&text_input, (40, 20, 2, 16));
    desk.pump();
    assert_eq!(desk.seen(), "40,20", "focused again");
}

/// **Only the pane whose window has the caret is given it**, in its own
/// space, past its frame's insets; disabling takes it away again.
#[test]
fn only_the_pane_whose_window_has_the_caret_is_given_it() {
    let mut desk = Desk::new();
    let (first, _a, _xa) = desk.window();
    let (second, _b, _xb) = desk.window();
    desk.focus(&first);
    let text_input = desk.text_input();
    enable(&text_input, (10, 20, 2, 16));
    desk.pump();

    let first_id = desk.state.panes.id_of(&first).expect("a pane");
    let second_id = desk.state.panes.id_of(&second).expect("a pane");
    desk.state.settle_caret();
    let insets = desk.state.insets_of(first_id);
    assert_eq!(
        desk.state.caret_in(first_id),
        Some(Rectangle::new(
            (insets.left + 10, insets.top + 20).into(),
            (2, 16).into()
        ))
    );
    assert_eq!(desk.state.caret_in(second_id), None);

    text_input.disable();
    text_input.commit();
    desk.pump();
    desk.state.settle_caret();
    assert_eq!(desk.state.caret_in(first_id), None, "disabled");
}

/// **A caret in a popup is where the popup is**: the popup's place in its
/// window, plus the caret's in the popup.
#[test]
fn a_caret_in_a_popup_is_where_the_popup_is() {
    let mut desk = Desk::new();
    let (window, _surface, xdg) = desk.window();
    desk.state
        .space
        .map_element(window.clone(), (300, 200), false);
    let compositor = desk.client.compositor.clone().expect("wl_compositor bound");
    let wm_base = desk.client.wm_base.clone().expect("xdg_wm_base bound");
    let surface = compositor.create_surface(&desk.qh, ());
    let popup_xdg = wm_base.get_xdg_surface(&surface, &desk.qh, ());
    let positioner = wm_base.create_positioner(&desk.qh, ());
    positioner.set_size(40, 30);
    positioner.set_anchor_rect(10, 10, 1, 1);
    positioner.set_anchor(xdg_positioner::Anchor::TopLeft);
    positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
    let _popup = popup_xdg.get_popup(Some(&xdg), &positioner, &desk.qh, ());
    surface.commit();
    desk.pump();
    let id = wayland_client::Proxy::id(&popup_xdg);
    let serial = desk
        .client
        .configures
        .iter()
        .rev()
        .find(|(to, _)| *to == id)
        .map(|&(_, serial)| serial)
        .expect("the popup was configured");
    popup_xdg.ack_configure(serial);
    desk.buffer(&surface, 40, 30);
    desk.pump();

    let toplevel = window
        .toplevel()
        .map(|toplevel| toplevel.wl_surface().clone())
        .expect("an xdg toplevel");
    let popup = smithay::desktop::PopupManager::popups_for_surface(&toplevel)
        .next()
        .map(|(popup, _)| popup.wl_surface().clone())
        .expect("the popup is on its parent");
    desk.state
        .give_keyboard(Some(popup), SERIAL_COUNTER.next_serial());
    desk.pump();
    let text_input = desk.text_input();
    enable(&text_input, (5, 6, 2, 10));
    desk.pump();
    let caret = desk.state.text_field().and_then(|field| field.caret);
    assert_eq!(caret, Some(rect(315.0, 216.0, 2.0, 10.0)));
}

/// **A caret in a subsurface is where the subsurface is**: the subsurface's
/// place on its parent, plus the caret's in the subsurface.
#[test]
fn a_caret_in_a_subsurface_is_where_the_subsurface_is() {
    let mut desk = Desk::new();
    let (window, surface, _xdg) = desk.window();
    desk.state
        .space
        .map_element(window.clone(), (300, 200), false);
    let compositor = desk.client.compositor.clone().expect("wl_compositor bound");
    let subcompositor = desk
        .client
        .subcompositor
        .clone()
        .expect("wl_subcompositor bound");
    let child = compositor.create_surface(&desk.qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &surface, &desk.qh, ());
    subsurface.set_position(7, 9);
    desk.buffer(&child, 20, 20);
    // A subsurface's place is its parent's state, applied when that commits.
    surface.commit();
    desk.pump();

    let toplevel = window
        .toplevel()
        .map(|toplevel| toplevel.wl_surface().clone())
        .expect("an xdg toplevel");
    let sub = smithay::wayland::compositor::get_children(&toplevel)
        .into_iter()
        .next()
        .expect("the subsurface is on its parent");
    desk.state
        .give_keyboard(Some(sub), SERIAL_COUNTER.next_serial());
    desk.pump();
    let text_input = desk.text_input();
    enable(&text_input, (5, 6, 2, 10));
    desk.pump();
    let caret = desk.state.text_field().and_then(|field| field.caret);
    assert_eq!(caret, Some(rect(312.0, 215.0, 2.0, 10.0)));
}
