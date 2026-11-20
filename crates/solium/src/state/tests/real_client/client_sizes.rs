//! **#115: what a client says about its own size**, read from the real
//! protocol and told to the layouts.
//!
//! No frames here, for the reason [`tiled_fixture`] gives, so a window's
//! own terms and its client's are the same numbers; that the frame is
//! added when there is one is `limits::a_limit_is_read_in_the_windows_own_terms_frame_included`.

use super::*;

/// One monitor, one client, and whatever scripts a test installs.
pub(super) struct Desk {
    display: Display<Solium>,
    pub(super) state: Solium,
    conn: Connection,
    queue: wayland_client::EventQueue<Client>,
    qh: QueueHandle<Client>,
    client: Client,
}

impl Desk {
    pub(super) fn new() -> Self {
        let mut display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        state
            .decorations
            .set_style(&mut state.panes, Some("none".to_string()));
        a_screen(&mut state, "sizes-test", (0, 0));
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

    /// `body` as the compositor's scripts, against the shipped ones,
    /// for the reason `reflow_on_close::Desk::install` gives.
    pub(super) fn install(&mut self, body: &str) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "solium-sizes-state-{}-{serial}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&directory);
        let entry = directory.join("init.lua");
        std::fs::write(
            &entry,
            format!(
                "package.path = {shipped:?} .. \"/?.lua\"\n{body}\n",
                shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/../solium-lua/lua"),
            ),
        )
        .expect("writing the entry point");
        self.state.scripts = Some(Scripts::load(&entry).expect("loading the test scripts"));
    }

    pub(super) fn evaluate(&self, lua: &str) -> String {
        self.state
            .scripts
            .as_ref()
            .map(|scripts| scripts.evaluate(lua))
            .unwrap_or_default()
    }

    pub(super) fn pump(&mut self) {
        pump(
            &mut self.display,
            &mut self.state,
            &self.conn,
            &self.qh,
            &mut self.queue,
            &mut self.client,
        );
    }

    pub(super) fn open(&mut self) -> (Window, xdg_toplevel::XdgToplevel, wl_surface::WlSurface) {
        let (window, toplevel, surface) = open_surface(
            &mut self.display,
            &mut self.state,
            &self.conn,
            &self.client,
            &self.qh,
        );
        self.state.sync_panes();
        self.pump();
        (window, toplevel, surface)
    }

    /// This window's row in the window list a script is handed now.
    pub(super) fn row(&self, window: &Window) -> WindowInfo {
        let id = self.state.window_id(window);
        self.state
            .snapshot()
            .windows
            .into_iter()
            .find(|row| row.id == id)
            .expect("the window is listed")
    }

    pub(super) fn place(&mut self, command: Command) {
        self.state.apply(Outcome {
            commands: vec![command],
            ..Outcome::default()
        });
    }
}

fn size(w: i32, h: i32) -> Option<Size<i32, Logical>> {
    Some(Size::from((w, h)))
}

fn instant() -> AnimationSpec {
    AnimationSpec {
        duration: Duration::ZERO,
        ..AnimationSpec::default()
    }
}

/// Window `id` alone in a tree of boxes over `area`, its application's own
/// least and largest size counting: what a layout's `sol.arrange` sends.
fn alone(id: u64, x: f64, y: f64, w: f64, h: f64) -> Command {
    let tree = solium_wm_protocol::space::Tree::of(
        &solium_wm_protocol::Rect { x, y, w, h },
        &[solium_wm_protocol::SpaceBox {
            kind: solium_wm_protocol::SpaceKind::Window(id),
            client_min: true,
            client_max: true,
            ..solium_wm_protocol::SpaceBox::default()
        }],
    )
    .expect("a tree");
    Command::Arrange {
        key: "test".to_owned(),
        tree: Some(tree),
        animation: instant(),
        moved: None,
    }
}

/// The client says it cannot be under `w` by `h`, and commits it.
fn least(
    desk: &mut Desk,
    toplevel: &xdg_toplevel::XdgToplevel,
    surface: &wl_surface::WlSurface,
    w: i32,
    h: i32,
) {
    toplevel.set_min_size(w, h);
    surface.commit();
    desk.pump();
}

/// **An xdg window says how small and how large it can be, and it is
/// what the window has committed that counts.**
///
/// `set_min_size` and `set_max_size` are double-buffered: the requests
/// alone change nothing a layout sees, and the commit after them does.
/// 0 on a side is no limit on that side, and both 0 is no limit at all.
/// And the application's name comes with it, which is what
/// `tiling.client_size_ignore` matches.
#[test]
fn an_xdg_window_says_how_small_and_how_large_it_can_be() {
    let mut desk = Desk::new();
    let (window, toplevel, surface) = desk.open();
    toplevel.set_app_id("sizes-test".to_owned());
    toplevel.set_min_size(300, 200);
    toplevel.set_max_size(900, 700);
    desk.pump();
    let row = desk.row(&window);
    assert_eq!(
        (row.min, row.max),
        (None, None),
        "a limit asked for and not yet committed was read"
    );

    surface.commit();
    desk.pump();
    let row = desk.row(&window);
    assert_eq!(row.min, size(300, 200));
    assert_eq!(row.max, size(900, 700));
    assert_eq!(row.app_id, "sizes-test");

    toplevel.set_min_size(0, 250);
    toplevel.set_max_size(0, 0);
    surface.commit();
    desk.pump();
    let row = desk.row(&window);
    assert_eq!(row.min, size(0, 250), "one side limited and not the other");
    assert_eq!(row.max, None, "no limit either way is none at all");
}

/// **A change of minimum is told to the layouts once**, through the
/// `layout` event every change to what a layout decides with goes
/// through -- and a commit that changes nothing tells them nothing.
#[test]
fn a_change_of_minimum_is_told_once() {
    let mut desk = Desk::new();
    desk.install("layouts = 0\nsol.on(\"layout\", function() layouts = layouts + 1 end)");
    let (_window, toplevel, surface) = desk.open();
    let told = |desk: &Desk| desk.evaluate("return tostring(layouts)");
    let before: u32 = told(&desk).parse().expect("a count");

    toplevel.set_min_size(400, 300);
    surface.commit();
    desk.pump();
    assert_eq!(
        told(&desk),
        (before + 1).to_string(),
        "the change was not told"
    );

    surface.commit();
    desk.pump();
    surface.commit();
    desk.pump();
    assert_eq!(
        told(&desk),
        (before + 1).to_string(),
        "commits that changed nothing were told as changes"
    );

    toplevel.set_min_size(500, 300);
    surface.commit();
    desk.pump();
    assert_eq!(told(&desk), (before + 2).to_string());
}

/// **A window that opens with a minimum hears it once, at `open`.**
/// The client says 640x480 in the bufferless commit xdg-shell starts a
/// window with, and draws in the next: the layout's `open` has the
/// minimum in the window's row, and no `layout` comes ahead of it --
/// a pass for a window no layout has heard of -- or after it.
#[test]
fn a_window_that_opens_with_a_minimum_hears_it_once() {
    let mut desk = Desk::new();
    desk.install(
        "events = {}\n\
         sol.on(\"open\", function(id)\n\
             for _, window in ipairs(sol.windows()) do\n\
                 if window.id == id then\n\
                     local min = window.min\n\
                     events[#events + 1] = \"open \" .. (min and (min.w .. \"x\" .. min.h) or \"nil\")\n\
                 end\n\
             end\n\
         end)\n\
         sol.on(\"layout\", function() events[#events + 1] = \"layout\" end)",
    );
    let compositor = desk.client.compositor.clone().expect("wl_compositor bound");
    let wm_base = desk.client.wm_base.clone().expect("xdg_wm_base bound");
    let surface = compositor.create_surface(&desk.qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &desk.qh, ());
    let toplevel = xdg_surface.get_toplevel(&desk.qh, ());
    desk.client.leave_frame(&toplevel, &desk.qh);
    toplevel.set_min_size(640, 480);
    surface.commit();
    desk.pump();
    commit_buffer(&desk.client, &desk.qh, &surface, 64, 64);
    desk.pump();
    assert_eq!(
        desk.evaluate("return table.concat(events, \",\")"),
        "open 640x480"
    );
}

/// **A limit past any screen is read as the most there is**, from a
/// real client: `set_min_size(i32::MAX, 0)` is a column of 32767 in
/// the scrolling layout, not one whose neighbour starts past the end
/// of `i32`.
#[test]
fn an_xdg_limit_past_any_screen_is_read_as_the_most_there_is() {
    let mut desk = Desk::new();
    let (window, toplevel, surface) = desk.open();
    toplevel.set_min_size(i32::MAX, 0);
    toplevel.set_max_size(i32::MAX, i32::MAX);
    surface.commit();
    desk.pump();
    let row = desk.row(&window);
    assert_eq!(row.min, size(super::super::snapshot::MOST, 0));
    assert_eq!(
        row.max,
        size(super::super::snapshot::MOST, super::super::snapshot::MOST)
    );
}

/// **A window launched with `sol.spawn` whose minimum does not fit the
/// tile it was given goes where `overflow` says**, as a window whose
/// minimum was known at its `open` would.
///
/// Window 1 fills the 1896x1056 inside the gap; window 2 is launched
/// and given half of it before its application exists. The
/// application then says 1800x1000 -- in its first, bufferless commit,
/// and again with its first frame in the same commit, which is the one
/// that needs the limits noticed before the frame is. Beside window 1,
/// held at the 160 minimum, the most it can have is 1724 across; below
/// it, 948 down. So there is no room either way, and the shipped chain
/// sends it to workspace 2 -- which a launched window used to miss,
/// being rebalanced in its half and left cramped. Once: a frame after
/// that moves nothing.
#[test]
fn a_launched_window_whose_minimum_does_not_fit_goes_where_overflow_says() {
    for with_its_first_frame in [false, true] {
        let mut desk = Desk::new();
        desk.install(
            "require(\"modes\")\n\
             require(\"workspaces\")\n\
             require(\"tiling\")\n\
             require(\"scrolling\")",
        );
        assert!(desk.state.trigger("super+t"), "super+t was not handled");
        let _working = desk.open();
        let source = crate::pane::loading_source(None);
        let launched = desk
            .state
            .open_loading("app", Some(std::process::id()), source, None);
        let workspace = |desk: &Desk| {
            desk.evaluate(&format!(
                "return tostring(require(\"workspaces\").of[{}])",
                launched.get()
            ))
        };
        assert_eq!(
            workspace(&desk),
            "1",
            "the premise: launched beside window 1"
        );

        let compositor = desk.client.compositor.clone().expect("wl_compositor bound");
        let wm_base = desk.client.wm_base.clone().expect("xdg_wm_base bound");
        let surface = compositor.create_surface(&desk.qh, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &desk.qh, ());
        let toplevel = xdg_surface.get_toplevel(&desk.qh, ());
        desk.client.leave_frame(&toplevel, &desk.qh);
        toplevel.set_min_size(1800, 1000);
        if with_its_first_frame {
            commit_buffer(&desk.client, &desk.qh, &surface, 64, 64);
        } else {
            surface.commit();
        }
        desk.pump();
        let says = if with_its_first_frame {
            "said with its first frame"
        } else {
            "said before its first frame"
        };
        assert!(
            desk.state
                .panes
                .get(launched)
                .and_then(Pane::client)
                .is_some(),
            "{says}: the premise: the application arrived in the window it was \
             launched into"
        );
        assert_eq!(
            workspace(&desk),
            "2",
            "{says}: the launched window stayed in a tile that cannot hold it"
        );
        assert_eq!(
            desk.state.panes.get(launched).and_then(Pane::placed),
            Some(at(12, 12, 1896, 1056)),
            "{says}: it was not given workspace 2 whole"
        );

        commit_buffer(&desk.client, &desk.qh, &surface, 64, 64);
        desk.pump();
        assert_eq!(
            workspace(&desk),
            "2",
            "{says}: a frame after it moved it again"
        );
    }
}

/// **A window centred in its tile is dragged from its tile.** Its
/// application will be no larger than 800x600, and its box is 1600x1000:
/// it is placed in the box's middle, the client is told 800x600, the box
/// is what it is held in, and the edge a drag starts from is the box's --
/// the seam -- and not the window's, 400 pixels inside it.
#[test]
fn a_centred_window_is_dragged_from_its_tile() {
    let mut desk = Desk::new();
    let (window, toplevel, surface) = desk.open();
    let pane = desk
        .state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");
    let id = desk.state.window_id(&window);
    toplevel.set_max_size(800, 600);
    surface.commit();
    desk.pump();
    desk.place(alone(id, 0.0, 0.0, 1600.0, 1000.0));
    desk.pump();
    let tile = at(0, 0, 1600, 1000);
    assert_eq!(
        desk.state.panes.get(pane).and_then(Pane::placed),
        Some(tile)
    );
    let laid_out = desk
        .state
        .pane_laid_out(&window)
        .expect("the window has a pane");
    assert_eq!(
        laid_out.0, tile,
        "a drag would start from the window's own edge"
    );
    let pointer: Point<f64, Logical> = (1190.0, 500.0).into();
    let (edge, _) =
        crate::input::resize::dragged_edge(laid_out, ResizeEdge::Right, pointer, pointer);
    assert!((edge - 1600.0).abs() < 1e-9, "{edge}");
    let told = desk
        .client
        .configures
        .iter()
        .rev()
        .find(|(of, ..)| *of == wayland_client::Proxy::id(&toplevel))
        .map(|(_, w, h)| (*w, *h));
    assert_eq!(
        told,
        Some((800, 600)),
        "the client was told the tile's size"
    );
}

/// **A window in a box smaller than its application's least size is
/// cramped, and the window list says so** -- for as long as its box is. A
/// box with room, a layout letting go and a placement out of any tree each
/// leave the window not cramped.
#[test]
fn a_window_in_a_box_under_its_least_size_is_cramped_and_the_list_says_so() {
    let mut desk = Desk::new();
    let (window, toplevel, surface) = desk.open();
    let id = desk.state.window_id(&window);
    least(&mut desk, &toplevel, &surface, 800, 600);
    let small = || alone(id, 0.0, 0.0, 600.0, 400.0);
    desk.place(small());
    assert!(
        desk.row(&window).cramped,
        "a 600x400 box for an 800x600 least"
    );
    desk.place(alone(id, 0.0, 0.0, 1000.0, 800.0));
    assert!(!desk.row(&window).cramped, "a box with room");
    desk.place(small());
    desk.place(Command::Unplace { id });
    assert!(!desk.row(&window).cramped, "let go by its layout");
    desk.place(small());
    desk.place(Command::Place {
        id,
        rect: Rect {
            x: 0.0,
            y: 0.0,
            w: 600.0,
            h: 400.0,
        },
        animation: instant(),
        tile: false,
        moved: false,
    });
    assert!(!desk.row(&window).cramped, "placed out of any tree");
}

/// **A window that becomes cramped is told again, so a bar can see
/// it.** A layout arranges a window in a box too small for it on a
/// `layout` pass, and a bar reading
/// `sol.windows()` in the same pass cannot see it -- the list is the
/// snapshot from before -- so the compositor runs the pass once more,
/// and the bar sees it there. A pass that changes nothing is not told
/// again; and a layout that changes its mind on every pass is told
/// once, not for ever.
#[test]
fn a_window_that_becomes_cramped_is_told_again_so_a_bar_can_see_it() {
    let mut desk = Desk::new();
    let (window, toplevel, surface) = desk.open();
    let id = desk.state.window_id(&window);
    least(&mut desk, &toplevel, &surface, 800, 600);
    desk.install(&format!(
        "seen = {{}}\n\
         sol.on(\"layout\", function()\n\
             sol.arrange(\"t\", {{ x = 0, y = 0, w = 600, h = 400 }}, {{ {{ window = {id}, client_min = true }} }})\n\
         end)\n\
         sol.on(\"layout\", function()\n\
             for _, window in ipairs(sol.windows()) do\n\
                 if window.id == {id} then seen[#seen + 1] = tostring(window.cramped) end\n\
             end\n\
         end)"
    ));
    let seen = |desk: &Desk| desk.evaluate("return table.concat(seen, \",\")");
    desk.state.trigger_relayout();
    assert_eq!(
        seen(&desk),
        "false,true",
        "the bar never saw the window cramped"
    );
    desk.state.trigger_relayout();
    assert_eq!(
        seen(&desk),
        "false,true,true",
        "told again with nothing changed"
    );

    desk.install(&format!(
        "passes, flip = 0, true\n\
         sol.on(\"layout\", function()\n\
             passes = passes + 1\n\
             flip = not flip\n\
             local w, h = 600, 400\n\
             if not flip then w, h = 1000, 800 end\n\
             sol.arrange(\"t\", {{ x = 0, y = 0, w = w, h = h }}, {{ {{ window = {id}, client_min = true }} }})\n\
         end)"
    ));
    desk.state.trigger_relayout();
    assert_eq!(desk.evaluate("return tostring(passes)"), "2");
}

/// A floating drag of `window` by its bottom-right corner, begun at
/// 400x300 from (100, 100), to the pointer at `to`: (300, 250) asks
/// for 200x150.
fn dragged(desk: &Desk, window: &Window, to: (f64, f64)) -> Rectangle<i32, Logical> {
    crate::input::resize::drag_rect(
        &desk.state,
        window,
        at(100, 100, 400, 300),
        ResizeEdge::BottomRight,
        (500.0, 400.0).into(),
        to.into(),
    )
}

/// A window whose application calls itself `app_id` and says it
/// cannot be under 300x200 or over 900x700.
fn limited_window(
    desk: &mut Desk,
    app_id: &str,
) -> (Window, xdg_toplevel::XdgToplevel, wl_surface::WlSurface) {
    let (window, toplevel, surface) = desk.open();
    toplevel.set_app_id(app_id.to_owned());
    toplevel.set_min_size(300, 200);
    toplevel.set_max_size(900, 700);
    surface.commit();
    desk.pump();
    (window, toplevel, surface)
}

/// **A floating drag is held to what the client accepts**: the limits
/// in the window's own terms, and the rectangle the drag makes held
/// to them -- asked of `drag_rect`, which is what the grab makes its
/// rectangle with.
#[test]
fn a_floating_drag_is_held_to_what_the_client_accepts() {
    let mut desk = Desk::new();
    let (window, _toplevel, _surface) = limited_window(&mut desk, "sizes-test");
    assert_eq!(
        desk.state.outer_limits(&window),
        (Size::from((300, 200)), Size::from((900, 700)))
    );
    assert_eq!(
        dragged(&desk, &window, (300.0, 250.0)),
        at(100, 100, 300, 200)
    );
    assert_eq!(
        dragged(&desk, &window, (2100.0, 1600.0)),
        at(100, 100, 900, 700)
    );
}

/// **A floating drag of an application the user does not believe is
/// not held**: one named in `tiling.client_size_ignore`, or any at all
/// under `floating.client_limits = "ignore"` -- as `sizes.lua` hands
/// them over, in `Command::ClientSizes`. An application not on the list
/// is still held.
#[test]
fn a_floating_drag_of_an_application_not_believed_is_not_held() {
    let mut desk = Desk::new();
    let (window, _toplevel, _surface) = limited_window(&mut desk, "liar");
    let small = |desk: &Desk| dragged(desk, &window, (300.0, 250.0));
    for (sizes, says) in [
        (
            crate::script::ClientSizes {
                floating: true,
                ignored: vec!["liar".to_owned()],
            },
            "an application in client_size_ignore",
        ),
        (
            crate::script::ClientSizes {
                floating: false,
                ignored: Vec::new(),
            },
            "floating.client_limits = \"ignore\"",
        ),
    ] {
        desk.place(Command::ClientSizes(sizes));
        assert_eq!(
            small(&desk),
            at(100, 100, 200, 150),
            "{says}: held all the same"
        );
    }
    desk.place(Command::ClientSizes(crate::script::ClientSizes {
        floating: true,
        ignored: vec!["someone-else".to_owned()],
    }));
    assert_eq!(
        small(&desk),
        at(100, 100, 300, 200),
        "an application not on the list was not held"
    );
    assert!(
        crate::script::ClientSizes {
            floating: true,
            ignored: vec![String::new()],
        }
        .believes(""),
        "a window with no name was taken for one on the list"
    );
}
