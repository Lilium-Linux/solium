//! Scripts are installed straight into `Solium::scripts` rather than
//! through `start_scripts`, and after the windows have opened. Both are
//! to keep the arrangement down to `adopt`'s, which is one call with a
//! known order, and to keep what the scripts asked for at load -- a
//! wallpaper, in the shipped `init.lua` -- from being built at all: a
//! Qt scene in a process holding a libwayland connection of its own
//! aborts the test binary (see the #99 test).

use super::*;

/// Writes down every event it hears, in order, as `"name id"` --
/// with a `*` when the window's own row in that event's snapshot
/// says `leaving`, and a `?` when the window is not in it at all.
const RECORDER: &str = r#"
events = {}
local function row(id)
for _, window in ipairs(sol.windows()) do
if window.id == id then return window end
end
end
for _, name in ipairs({ "open", "closing", "refused", "close" }) do
sol.on(name, function(id)
local window = row(id)
local mark = (window == nil and "?") or (window.leaving and "*") or ""
events[#events + 1] = name .. " " .. id .. mark
end)
end
"#;

/// The same, for the two events every layout written before #128
/// knows: a layout that listens for neither of the new ones.
const OLD_RECORDER: &str = r#"
events = {}
for _, name in ipairs({ "open", "close" }) do
sol.on(name, function(id) events[#events + 1] = name .. " " .. id end)
end
"#;

/// One monitor, one client, and the scripts in `body`.
struct Desk {
    display: Display<Solium>,
    state: Solium,
    conn: Connection,
    queue: wayland_client::EventQueue<Client>,
    qh: QueueHandle<Client>,
    client: Client,
}

impl Desk {
    fn new() -> Self {
        Self::on(|state| {
            a_screen(state, "reflow-test", (0, 0));
        })
    }

    /// [`Self::new`] with a second monitor to the right of the
    /// first: see [`side_by_side`]. The left one keeps the name
    /// every helper here asks `workspaces` about.
    fn side_by_side() -> Self {
        Self::on(|state| {
            side_by_side(state, "reflow-test");
        })
    }

    /// The desk, on the monitors `screens` maps.
    fn on(screens: impl FnOnce(&mut Solium)) -> Self {
        let mut display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        state
            .decorations
            .set_style(&mut state.panes, Some("none".to_string()));
        screens(&mut state);
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

    /// Load an entry point that runs `body` against the shipped
    /// scripts, and hand the compositor those scripts.
    ///
    /// `package.path` is written by the entry for the reason the
    /// layout harness in `script.rs` gives: `Scripts::load` would
    /// otherwise search the developer's own configuration first.
    /// A directory per call, for the reason it gives too.
    fn install(&mut self, body: &str) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "solium-reflow-state-{}-{serial}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&directory);
        let entry = directory.join("init.lua");
        std::fs::write(
            &entry,
            format!(
                "package.path = {shipped:?} .. \"/?.lua\"\n{body}\n",
                shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
            ),
        )
        .expect("writing the entry point");
        self.state.scripts = Some(Scripts::load(&entry).expect("loading the test scripts"));
    }

    /// A shipped layout, switched on over the windows already
    /// open, with `before` run ahead of it and the recorder after.
    fn arrange(&mut self, layout: &str, before: &str) {
        self.install(&format!(
            "{before}\nrequire(\"modes\")\nrequire({layout:?})\n{RECORDER}"
        ));
        let key = if layout == "tiling" {
            "super+t"
        } else {
            "super+s"
        };
        assert!(
            self.state.trigger(key),
            "{layout}: the layout key was not handled"
        );
        // And let the arrangement land, so a close starts from a
        // window drawn where it lives rather than part way along
        // the slide into its slot.
        self.state.clock.advance(Duration::from_secs(1));
        let now = self.state.clock.now();
        self.state.settle(now);
    }

    fn open(&mut self) -> (Window, xdg_toplevel::XdgToplevel, crate::pane::PaneId) {
        let (window, toplevel, _surface, _xdg) = open_xdg(
            &mut self.display,
            &mut self.state,
            &self.conn,
            &self.client,
            &self.qh,
        );
        self.state.sync_panes();
        self.pump();
        let pane = self
            .state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");
        (window, toplevel, pane)
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

    /// What the recorder heard, comma-separated.
    fn events(&self) -> String {
        self.state
            .scripts
            .as_ref()
            .map(|scripts| scripts.evaluate("return table.concat(events, \",\")"))
            .unwrap_or_default()
    }

    /// The rectangle the layout last asked for this pane.
    fn placed(&self, pane: crate::pane::PaneId) -> Rectangle<i32, Logical> {
        self.state
            .panes
            .get(pane)
            .and_then(Pane::placed)
            .expect("the layout has placed this pane")
    }

    /// The pane the keyboard is on.
    fn focused(&self) -> crate::pane::PaneId {
        self.state
            .focused_window()
            .and_then(|window| self.state.panes.id_of(&window))
            .expect("a window has the keyboard")
    }

    /// The close's request goes out: past `CLOSING`, and the
    /// client hears it. Returns the instant it was asked at.
    fn ask(&mut self) -> Duration {
        self.state
            .clock
            .advance(present::CLOSING + Duration::from_millis(10));
        let asked = self.state.clock.now();
        self.state.settle_closing(asked);
        self.pump();
        asked
    }

    /// The client says nothing, and the grace period runs out.
    /// Returns the instant the window was given back at.
    fn refuse(&mut self) -> Duration {
        self.state.clock.advance(Duration::from_millis(1100));
        let now = self.state.clock.now();
        self.state.settle_refused(now);
        self.pump();
        now
    }

    /// [`Self::open`], with the client's surface as well: for a
    /// test that has to make the client answer a configure with a
    /// buffer of the size it was given, as a real one does.
    fn open_surface(&mut self) -> Opened {
        let (window, toplevel, surface, xdg) = open_xdg(
            &mut self.display,
            &mut self.state,
            &self.conn,
            &self.client,
            &self.qh,
        );
        self.state.sync_panes();
        self.pump();
        let pane = self
            .state
            .panes
            .id_of(&window)
            .expect("a client in the space has a pane");
        Opened {
            toplevel,
            pane,
            surface,
            xdg,
        }
    }

    /// The client commits a buffer of exactly the client area the
    /// layout last gave this pane -- the answer to its configure.
    fn answer(&mut self, opened: &Opened) {
        let client = inner(self.placed(opened.pane), self.state.insets_of(opened.pane));
        commit_buffer(
            &self.client,
            &self.qh,
            &opened.surface,
            client.size.w,
            client.size.h,
        );
        self.pump();
    }

    /// A modal dialog for `parent`: the toplevel, `set_parent`,
    /// `set_modal` and its first buffer in one flush, so the
    /// compositor reads every one of them in a single dispatch.
    /// Returns the dialog.
    fn open_dialog_for(&mut self, parent: &xdg_toplevel::XdgToplevel) -> Window {
        let compositor = self.client.compositor.clone().expect("wl_compositor bound");
        let wm_base = self.client.wm_base.clone().expect("xdg_wm_base bound");
        let dialogs = self.client.dialogs.clone().expect("xdg_wm_dialog_v1 bound");
        let before: Vec<Window> = self.state.space.elements().cloned().collect();
        let surface = compositor.create_surface(&self.qh, ());
        let xdg = wm_base.get_xdg_surface(&surface, &self.qh, ());
        let toplevel = xdg.get_toplevel(&self.qh, ());
        self.client.leave_frame(&toplevel, &self.qh);
        toplevel.set_parent(Some(parent));
        dialogs.get_xdg_dialog(&toplevel, &self.qh, ()).set_modal();
        commit_buffer(&self.client, &self.qh, &surface, 64, 64);
        self.conn.flush().expect("flushing the dialog's requests");
        self.display
            .dispatch_clients(&mut self.state)
            .expect("dispatching the dialog's requests");
        self.state
            .space
            .elements()
            .find(|window| !before.contains(window))
            .cloned()
            .expect("new_toplevel mapped the dialog")
    }
}

/// A window [`Desk::open_surface`] opened.
struct Opened {
    toplevel: xdg_toplevel::XdgToplevel,
    pane: crate::pane::PaneId,
    surface: wl_surface::WlSurface,
    /// What a menu names as its parent.
    xdg: xdg_surface::XdgSurface,
}

/// A window a test closes under `layout`, and the windows beside
/// it: `(closed, others)`.
///
/// Chosen so that a sweep places some other window *after* the one
/// being closed, which is the one `Space::map_element` leaves above
/// it: both shipped layouts place left to right. In tiling that is
/// the left of two windows. In scrolling it is the middle of three,
/// because closing the left of two moves nothing: the strip keeps
/// the column that is left where it was. Which of the others moves
/// into the space is the layout's business, so the tests find out
/// rather than say.
fn closing_scene(desk: &mut Desk, layout: &str) -> (Opened, Vec<Opened>) {
    let count = if layout == "tiling" { 2 } else { 3 };
    let mut opened: Vec<Opened> = (0..count).map(|_| desk.open_surface()).collect();
    desk.arrange(layout, "");
    opened.sort_by_key(|each| desk.placed(each.pane).loc.x);
    let closed = opened.remove(count - 2);
    assert!(
        opened
            .iter()
            .all(|other| desk.placed(other.pane).loc.x != desk.placed(closed.pane).loc.x),
        "{layout}: the premise is windows side by side"
    );
    (closed, opened)
}

/// Close `closed`, and say which of `others` the layout moved.
fn close_and_see_who_moved<'a>(
    desk: &mut Desk,
    layout: &str,
    closed: &Opened,
    others: &'a [Opened],
) -> &'a Opened {
    let before: Vec<Rectangle<i32, Logical>> =
        others.iter().map(|other| desk.placed(other.pane)).collect();
    desk.state.close_pane(closed.pane);
    others
        .iter()
        .zip(before)
        .find(|(other, was)| desk.placed(other.pane) != *was)
        .map(|(other, _)| other)
        .unwrap_or_else(|| panic!("{layout}: the premise is a neighbour moving into the space"))
}

/// How far a pane can have moved in `seconds` of real time without
/// jumping, in pixels, and how far its opacity can have changed.
///
/// The clock is real time, so the sweeps of one dispatch are
/// microseconds apart and each starts from where the one before it
/// had got to by then. That is motion rather than a jump, and it is
/// bounded: no placement these tests drive is faster than 180 ms
/// (tiling's `snap`), the return fade is 150 ms, and `OutCubic`'s
/// steepest slope is 3, at the start -- across at most a screen's
/// width and one unit of opacity.
fn drift(seconds: f64) -> (f64, f32) {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "an opacity bound, far inside f32"
    )]
    let opacity = (3.0 / 0.15 * seconds) as f32;
    (1.0 + 1920.0 * 3.0 / 0.18 * seconds, opacity)
}

/// Whether `top` is drawn over `under`: earlier in `on_screen`,
/// which is topmost first and the order `render` draws in, after
/// `sync_panes` has brought it in line with the space the way every
/// frame does.
fn above(state: &mut Solium, top: crate::pane::PaneId, under: crate::pane::PaneId) -> bool {
    state.sync_panes();
    let order: Vec<crate::pane::PaneId> = state
        .on_screen()
        .into_iter()
        .map(|(pane, _)| pane)
        .collect();
    let at = |pane| order.iter().position(|each| *each == pane);
    at(top)
        .zip(at(under))
        .is_some_and(|(top, under)| top < under)
}

/// What `sol.window_at` answers at `point`, asked from Lua of the
/// compositor as it is this instant -- the snapshot a handler
/// dispatched now would be handed -- as text: a window's id, or
/// `nil`.
fn window_at_from_lua(desk: &Desk, point: Point<f64, Logical>) -> String {
    let snapshot = desk.state.snapshot();
    desk.state
        .scripts
        .as_ref()
        .map(|scripts| {
            scripts.evaluate_in(
                snapshot,
                &format!(
                    "return tostring(sol.window_at({:?}, {:?}))",
                    point.x, point.y
                ),
            )
        })
        .unwrap_or_default()
}

const LAYOUTS: [&str; 2] = ["tiling", "scrolling"];

/// Two windows side by side under `layout`, and which is which:
/// `(the one the keyboard is on, the other)`.
fn two(desk: &mut Desk, layout: &str, before: &str) -> (crate::pane::PaneId, crate::pane::PaneId) {
    let (_, _, first) = desk.open();
    let (_, _, second) = desk.open();
    desk.arrange(layout, before);
    let focused = desk.focused();
    let other = if focused == first { second } else { first };
    assert_ne!(
        desk.placed(focused).loc.x,
        desk.placed(other).loc.x,
        "{layout}: the premise is two windows side by side"
    );
    (focused, other)
}

/// **Close a window and the layout closes up around it at once,
/// while the client is still there and has not even been asked.**
///
/// The window closed is the one the keyboard is on, and the
/// keyboard stays on it: it leaves when the animation lands
/// (`hand_off_keyboard`), and a layout that moved it now -- the
/// scrolling layout's `settle` would -- takes it off a window that
/// is still on screen.
///
/// The window closed fades where it stood. Nothing places it once
/// it is out of the arrangement, and #127's pinning keeps it from
/// following anything that does.
#[test]
fn a_closing_window_hands_its_space_over_before_its_client_is_gone() {
    for layout in LAYOUTS {
        let mut desk = Desk::new();
        let (dying, survivor) = two(&mut desk, layout, "");
        let was = desk.placed(survivor);
        let stood = desk.placed(dying);
        let outer = desk
            .state
            .pane_outer_of(dying)
            .expect("a mapped pane has a rectangle");

        let pressed = desk.state.clock.now();
        desk.state.close_pane(dying);
        desk.pump();

        assert!(
            desk.client.closes.is_empty() && desk.state.panes.get(dying).is_some(),
            "{layout}: the premise -- the client has not even been asked, so \
             everything below happened before it could have gone"
        );
        assert_eq!(
            desk.events(),
            format!("closing {}*", dying.get()),
            "{layout}"
        );
        let now = desk.placed(survivor);
        assert_ne!(
            now, was,
            "{layout}: the survivor is where it was, so the closed window's space \
             is still reserved for it"
        );
        match layout {
            "tiling" => assert!(
                now.size.w > was.size.w,
                "tiling: the neighbour did not grow into the space: {was:?} -> {now:?}"
            ),
            _ => assert_eq!(
                now.loc.x, stood.loc.x,
                "scrolling: the strip did not close the gap: {was:?} -> {now:?}"
            ),
        }
        assert_eq!(
            desk.focused(),
            dying,
            "{layout}: the keyboard left the closing window before its animation \
             landed"
        );
        let fading = drawn_now(&desk.state, dying, pressed + Duration::from_millis(95));
        assert!(
            fading.opacity < 0.9 && (fading.rect.loc.x - f64::from(outer.loc.x)).abs() < 100.0,
            "{layout}: the closing window is not fading where it stood: {fading:?}, \
             and it stood at {outer:?}"
        );
    }
}

/// **Refuse the close, and the window comes back beside its old
/// neighbour.**
///
/// Two windows, so the neighbour was a single window and tiling
/// puts everything back exactly; scrolling is asserted as order and
/// adjacency, because putting a window back focuses its column and
/// the view may follow.
///
/// And `refused` is sent once, and `open` not at all: the window
/// never went, and an `open` would animate it in a second time and
/// run the scrolling layout's `settle`.
#[test]
fn a_refused_close_comes_back_beside_its_old_neighbour() {
    for layout in LAYOUTS {
        let mut desk = Desk::new();
        let (dying, survivor) = two(&mut desk, layout, "");
        let before = (desk.placed(dying), desk.placed(survivor));

        desk.state.close_pane(dying);
        desk.ask();
        desk.refuse();
        // And more frames, well past every deadline, which must not
        // produce a second one.
        desk.state.clock.advance(Duration::from_secs(3));
        let later = desk.state.clock.now();
        desk.state.settle(later);
        desk.pump();

        let id = dying.get();
        assert_eq!(
            desk.events(),
            format!("closing {id}*,refused {id}"),
            "{layout}: the refusal was not told once, and once only, with the window \
             no longer leaving"
        );
        let after = (desk.placed(dying), desk.placed(survivor));
        match layout {
            "tiling" => assert_eq!(
                after, before,
                "tiling: the window did not come back where it was"
            ),
            _ => {
                let (back, other) = after;
                let (left, right) = if back.loc.x < other.loc.x {
                    (back, other)
                } else {
                    (other, back)
                };
                assert_eq!(
                    before.0.loc.x < before.1.loc.x,
                    back.loc.x < other.loc.x,
                    "scrolling: the window came back on the other side of its \
                     neighbour"
                );
                assert_eq!(
                    left.loc.x + left.size.w + 12,
                    right.loc.x,
                    "scrolling: the two are not neighbours: {left:?}, {right:?}"
                );
            }
        }
    }
}

/// **A refused window fades back in from where it vanished**, and
/// does not appear at once over the neighbour that grew into it.
///
/// `give_back` starts the return from the held, transparent end of
/// the leaving animation; the layout putting the window back then
/// replaces that with `move_pane`'s slide, which begins from what is
/// on screen -- that same transparent frame -- rather than from full
/// opacity at the rectangle the window had.
#[test]
fn a_refused_window_fades_back_in_from_where_it_vanished() {
    for layout in LAYOUTS {
        let mut desk = Desk::new();
        let (dying, _) = two(&mut desk, layout, "");
        desk.state.close_pane(dying);
        desk.ask();
        let back = desk.refuse();
        assert!(
            desk.events().ends_with(&format!("refused {}", dying.get())),
            "{layout}: the premise is a refusal the layout heard"
        );

        // The slide starts from the fade as it was when the layout
        // placed the window, a moment of real time after `back`.
        let (_, fading) = drift((desk.state.clock.now() - back).as_secs_f64());
        let first = drawn_now(&desk.state, dying, back);
        assert!(
            first.opacity < 0.05 + fading,
            "{layout}: the refused window was drawn at opacity {} on the frame it \
             came back, rather than fading in from nothing",
            first.opacity
        );
        let landed = drawn_now(&desk.state, dying, back + Duration::from_millis(400));
        let slot = desk.placed(dying);
        assert!(
            (landed.opacity - 1.0).abs() < f32::EPSILON
                && (landed.rect.loc.x - f64::from(slot.loc.x)).abs() < 1.0,
            "{layout}: the return did not land where the layout put the window: \
             {landed:?}, placed at {slot:?}"
        );
    }
}

/// **A client that goes at its grace deadline is not refused after
/// it closed.**
///
/// The order a frame runs in is the whole of it: the Wayland
/// dispatch -- where the client's destroy arrives and `close` is
/// sent -- then `settle`, where the grace deadline is read, and
/// only then `sync_panes`, which retires the pane. A client
/// destroying its toplevel on the frame its grace ran out was
/// still `asked_at` in that `settle`, was given back, and the
/// layout was told `refused` after `close` -- and put a window that
/// no longer exists back into its tree for good.
#[test]
fn a_client_that_goes_at_its_grace_deadline_is_not_refused_after_it_closed() {
    let mut desk = Desk::new();
    let (_, _, kept) = desk.open();
    let (_, toplevel, going) = desk.open();
    desk.arrange("tiling", "");
    let whole = desk.placed(kept);

    desk.state.close_pane(going);
    let asked = desk.ask();
    // The frame on which the grace runs out: the client goes in the
    // dispatch, and `settle` runs after it at the deadline.
    toplevel.destroy();
    desk.pump();
    desk.state.settle(asked + Duration::from_millis(1000));
    desk.state.space.refresh();
    desk.state.sync_panes();
    desk.pump();

    let id = going.get();
    assert_eq!(
        desk.events(),
        format!("closing {id}*,close {id}*"),
        "the layout was told something after the window was gone"
    );
    assert!(
        desk.state.panes.get(going).is_none(),
        "the premise: the pane is retired"
    );
    // And the arrangement holds no leaf for it: laid out again, the
    // window that stayed has the screen.
    desk.state.trigger_relayout();
    assert!(
        desk.placed(kept).size.w > whole.size.w,
        "the tree still divides the screen with a window that is gone: {:?}",
        desk.placed(kept)
    );
}

/// **A layout placing a window in `close` does not show it again.**
///
/// A stateless layout places every row it is handed, and `close`'s
/// snapshot still lists the window that went. The close timers are
/// disarmed after that dispatch rather than before it, so the pane
/// is still leaving while the handler runs and `move_pane` leaves
/// the invisible transform alone; the other order made it an
/// ordinary pane for one frame and put it back at full opacity.
#[test]
fn a_layout_placing_a_closed_window_does_not_show_it_again() {
    let mut desk = Desk::new();
    let (_, toplevel, pane) = desk.open();
    desk.install(
        r#"
sol.on("close", function()
for _, window in ipairs(sol.windows()) do
sol.place(window.id, { x = window.x, y = window.y, w = window.w, h = window.h })
end
end)
"#,
    );

    desk.state.close_pane(pane);
    let asked = desk.ask();
    assert!(
        drawn_now(&desk.state, pane, asked).opacity.abs() < f32::EPSILON,
        "the premise: the window is held invisible while the client decides"
    );
    toplevel.destroy();
    desk.pump();
    let now = desk.state.clock.now();
    assert!(
        desk.state.panes.get(pane).is_some(),
        "the premise: the pane is still here until `sync_panes`"
    );
    assert!(
        drawn_now(&desk.state, pane, now).opacity.abs() < f32::EPSILON,
        "a layout placing the window in `close` drew it again at opacity {}",
        drawn_now(&desk.state, pane, now).opacity
    );
}

/// **Every way to close a window tells the layout at once.**
///
/// Script events are dispatched by taking the scripts out of
/// `Solium::scripts` for the length of the call, so a close that
/// reached `close_pane` *during* a dispatch would find nothing there
/// and `closing` would be lost without a word. Each route is driven
/// the way it arrives: the frame's close button through
/// `frame_action`, which the pointer calls; `sol.close` from a
/// binding and from an event handler, both of which queue a
/// command that `apply` runs after the dispatch has put the
/// scripts back; the same from inside a `closing` handler, which is
/// a dispatch inside `apply`; and `super+q`, the shipped binding in
/// `init.lua`.
#[test]
fn every_close_route_tells_the_layout_the_close_has_begun() {
    let mut desk = Desk::new();
    let (_, _, button) = desk.open();
    let (_, _, bound) = desk.open();
    let (_, _, handled) = desk.open();
    let (_, _, first) = desk.open();
    let (_, _, second) = desk.open();
    desk.install(&format!(
        "{RECORDER}\n\
         sol.bind(\"super+x\", function() sol.close({bound}) end)\n\
         local armed = true\n\
         sol.on(\"layout\", function()\n\
             if armed then armed = false; sol.close({handled}) end\n\
         end)\n\
         sol.on(\"closing\", function(id)\n\
             if id == {first} then sol.close({second}) end\n\
         end)\n",
        bound = bound.get(),
        handled = handled.get(),
        first = first.get(),
        second = second.get(),
    ));

    desk.state.frame_action(button, Action::Close);
    assert!(desk.state.trigger("super+x"), "the binding was not handled");
    desk.state.trigger_relayout();
    desk.state.close_pane(first);

    assert_eq!(
        desk.events(),
        format!(
            "closing {}*,closing {}*,closing {}*,closing {}*,closing {}*",
            button.get(),
            bound.get(),
            handled.get(),
            first.get(),
            second.get()
        ),
        "a close route did not tell the layout it had begun"
    );

    // `super+q`, as it ships: the whole of `init.lua`, whose
    // binding closes the window the keyboard is on.
    let mut desk = Desk::new();
    let (_, _, focused) = desk.open();
    assert_eq!(
        desk.focused(),
        focused,
        "the premise: a window has the keyboard"
    );
    desk.install(&format!(
        "dofile({init:?})\n{RECORDER}",
        init = concat!(env!("CARGO_MANIFEST_DIR"), "/lua/init.lua"),
    ));
    assert!(desk.state.trigger("super+q"), "super+q was not handled");
    assert_eq!(
        desk.events(),
        format!("closing {}*", focused.get()),
        "super+q did not tell the layout the close had begun"
    );
}

/// **A layout that listens for neither new event hears exactly what
/// it always heard.** `close` when the window is gone and not a
/// moment before -- whether it was asked to close or went by
/// itself -- and nothing at all for a close that was refused: not a
/// `close` for the moment it was asked, and not an `open` for the
/// moment it came back.
///
/// This is not a change and did not fail before it; it is here so
/// that one cannot be made quietly. Against a `give_back` that
/// told scripts `opened` instead of `refused`, it fails at "a
/// refused close".
#[test]
fn a_layout_that_knows_neither_new_event_hears_what_it_always_did() {
    let mut desk = Desk::new();
    let (_, toplevel, asked) = desk.open();
    let (_, itself_toplevel, itself) = desk.open();
    desk.install(OLD_RECORDER);

    desk.state.close_pane(asked);
    desk.ask();
    assert_eq!(
        desk.events(),
        "",
        "the close was announced before the window went"
    );
    desk.refuse();
    assert_eq!(
        desk.events(),
        "",
        "a refused close told the layout something"
    );

    desk.state.close_pane(asked);
    desk.ask();
    toplevel.destroy();
    desk.pump();
    itself_toplevel.destroy();
    desk.pump();
    assert_eq!(
        desk.events(),
        format!("close {},close {}", asked.get(), itself.get()),
        "the layout was not told each window went, once"
    );
}

/// **The window list says which windows are on their way out**, in
/// every event about one: `closing`'s, `close`'s -- including for a
/// window that closed itself and was never `closing` -- and not
/// `refused`'s, where the window is staying.
#[test]
fn the_window_list_says_which_windows_are_leaving() {
    let mut desk = Desk::new();
    let (_, _, refused) = desk.open();
    let (_, itself_toplevel, itself) = desk.open();
    desk.install(RECORDER);

    desk.state.close_pane(refused);
    desk.ask();
    desk.refuse();
    itself_toplevel.destroy();
    desk.pump();

    assert_eq!(
        desk.events(),
        format!(
            "closing {r}*,refused {r},close {i}*",
            r = refused.get(),
            i = itself.get()
        )
    );
}

/// **A refusal is told once, on the frame the window comes back**,
/// by whichever route brings it back -- and not on a frame whose
/// give-back was declined, when the window is still invisible and a
/// layout putting it back would be putting back nothing.
///
/// The dialog route here; the grace deadline is
/// `a_refused_close_comes_back_beside_its_old_neighbour`. The
/// declined one is driven with `present::jam_slot`, which never
/// lets go, so what is asserted is the half this can reach: nothing
/// is told while the give-back is owed.
#[test]
fn a_refusal_tells_the_layout_once_on_the_frame_the_window_comes_back() {
    let mut desk = Desk::new();
    let (_, parent_top, parent) = desk.open();
    let (_, jammed_top, jammed) = desk.open();
    desk.install(RECORDER);

    desk.state.close_pane(parent);
    let (_, dialog_top, _) = desk.open();
    dialog_top.set_parent(Some(&parent_top));
    desk.pump();
    desk.ask();
    desk.refuse();
    let id = parent.get();
    let heard = desk.events();
    assert!(
        heard.starts_with(&format!("closing {id}*,")) && heard.ends_with(&format!("refused {id}")),
        "the dialog's answer was not told as a refusal: {heard}"
    );
    assert_eq!(
        heard.matches("refused").count(),
        1,
        "a refusal was told more than once: {heard}"
    );

    desk.state.close_pane(jammed);
    if let Some(busy) = desk.state.panes.get(jammed) {
        present::jam_slot(busy);
    }
    let (_, answer_top, _) = desk.open();
    answer_top.set_parent(Some(&jammed_top));
    desk.pump();
    desk.ask();
    desk.state.clock.advance(Duration::from_millis(100));
    let later = desk.state.clock.now();
    desk.state.settle_closing(later);
    assert!(
        desk.state.panes.get(jammed).is_some_and(Pane::leaving),
        "the premise: the give-back is still owed"
    );
    assert!(
        !desk.events().contains(&format!("refused {}", jammed.get())),
        "a refusal was told on a frame its give-back was declined: {}",
        desk.events()
    );
}

/// **A dialog that answers a close keeps the keyboard, in the
/// scrolling layout too.** Putting the window back focuses its
/// column, and `settle` would hand the keyboard to that column --
/// off the "save your changes?" the user is being asked. `refused`
/// uses `apply`, and `open` is not sent, because its handler
/// settles.
#[test]
fn a_dialog_that_refuses_a_close_keeps_the_keyboard() {
    let mut desk = Desk::new();
    let (_, parent_top, parent) = desk.open();
    let (_, _, _other) = desk.open();
    desk.arrange("scrolling", "");

    desk.state.close_pane(parent);
    let (dialog, dialog_top, _) = desk.open();
    assert!(
        desk.state.is_focused(&dialog),
        "the premise: the dialog has the keyboard"
    );
    dialog_top.set_parent(Some(&parent_top));
    desk.pump();
    assert!(
        desk.events()
            .ends_with(&format!("refused {}", parent.get())),
        "the premise: the dialog's answer was a refusal the layout heard: {}",
        desk.events()
    );
    assert!(
        desk.state.is_focused(&dialog),
        "the layout took the keyboard off the dialog when it put its window back"
    );
}

/// **A window refused by a dialog fades back in, and its neighbour
/// moves once, from where it was drawn.**
///
/// The dialog route is the common refusal -- "save your changes?"
/// -- and a client that sends the toplevel, `set_parent` and
/// `set_modal` in one flush (GTK, by the account of the review that
/// found this) makes it several layout sweeps in one dispatch:
/// `refused` runs one from inside `give_back`, `parent_changed`
/// another, and `modal_changed` a third. Each sweep after the
/// first used to start every window it placed from `Frame::real` of
/// the rectangle the previous sweep had just written to the space:
/// the refused window at full opacity, and -- in tiling, where the
/// neighbour changes size as well as place -- the neighbour at its
/// new position and its old, committed size, a 954px jump before
/// anything had been drawn. So both are read at the instant the
/// dialog arrived, which is before any of those animations has
/// moved more than real time allows, and must be what was on screen
/// then.
///
/// The neighbour's client answers its configure first, as a real
/// one does: the space holding the size the layout gave it is what
/// makes the jump a jump, rather than an artefact of a fixture that
/// never resizes.
#[test]
fn a_refusal_by_dialog_fades_the_window_back_and_moves_its_neighbour_once() {
    for layout in LAYOUTS {
        let mut desk = Desk::new();
        let (dying, others) = closing_scene(&mut desk, layout);
        let survivor = close_and_see_who_moved(&mut desk, layout, &dying, &others);
        desk.ask();
        desk.answer(survivor);
        // Past the neighbour's slide into the space, and well inside
        // the grace period.
        desk.state.clock.advance(Duration::from_millis(300));
        let settled = desk.state.clock.now();
        desk.state.settle(settled);

        let arrived = desk.state.clock.now();
        let neighbour = drawn_now(&desk.state, survivor.pane, arrived);
        let dialog = desk.open_dialog_for(&dying.toplevel);
        let (moving, fading) = drift((desk.state.clock.now() - arrived).as_secs_f64());
        assert!(
            desk.state.is_modal(&dialog)
                && desk
                    .events()
                    .contains(&format!("refused {}", dying.pane.get())),
            "{layout}: the premise is a modal dialog whose answer was a refusal the \
             layout heard: {}",
            desk.events()
        );

        let back = drawn_now(&desk.state, dying.pane, arrived);
        assert!(
            back.opacity < 0.05 + fading,
            "{layout}: the refused window was drawn at opacity {} on the frame its \
             dialog arrived, rather than fading in from nothing",
            back.opacity
        );
        let moved = drawn_now(&desk.state, survivor.pane, arrived);
        assert!(
            (moved.rect.loc.x - neighbour.rect.loc.x).abs() < moving
                && (moved.rect.size.w - neighbour.rect.size.w).abs() < moving,
            "{layout}: the neighbour jumped on the frame the dialog arrived: it was \
             drawn at {:?} and is now drawn at {:?}",
            neighbour.rect,
            moved.rect
        );

        // And both still land where the layout put them.
        let later = arrived + Duration::from_secs(1);
        let landed = drawn_now(&desk.state, dying.pane, later);
        let slot = desk.placed(dying.pane);
        assert!(
            (landed.opacity - 1.0).abs() < f32::EPSILON
                && (landed.rect.loc.x - f64::from(slot.loc.x)).abs() < 1.0,
            "{layout}: the refused window did not land in its slot: {landed:?}, \
             placed at {slot:?}"
        );
        let slot = desk.placed(survivor.pane);
        let landed = drawn_now(&desk.state, survivor.pane, later);
        assert!(
            (landed.rect.loc.x - f64::from(slot.loc.x)).abs() < 1.0,
            "{layout}: the neighbour did not land in its slot: {landed:?}, placed \
             at {slot:?}"
        );
    }
}

/// **A window being closed fades in front of the neighbour that
/// moves into its space.**
///
/// The sweep `closing` runs places every survivor and none of the
/// dying window, and `Space::map_element` puts each window it places
/// on top -- so the neighbour growing into the space (tiling) or
/// sliding into it (scrolling) was stacked over the window fading
/// there, and covered most of the fade. And in scrolling the
/// window the keyboard is on can be below its neighbour to begin
/// with, because the strip's own sweeps stack left to right. So a
/// window is raised as its close begins, and a later sweep inside
/// the fade -- another window opening, a client's `set_parent` --
/// does not bury it either.
#[test]
fn a_closing_window_fades_in_front_of_the_neighbour_moving_into_its_space() {
    for layout in LAYOUTS {
        let mut desk = Desk::new();
        let (dying, others) = closing_scene(&mut desk, layout);
        let moved = close_and_see_who_moved(&mut desk, layout, &dying, &others);
        assert!(
            above(&mut desk.state, dying.pane, moved.pane),
            "{layout}: the neighbour moving into the closed window's space is drawn \
             over the window fading there"
        );
        for other in &others {
            assert!(
                above(&mut desk.state, dying.pane, other.pane),
                "{layout}: a window the close did not move was stacked over the \
                 window fading"
            );
        }

        desk.state.clock.advance(Duration::from_millis(80));
        desk.state.trigger_relayout();
        for other in &others {
            assert!(
                above(&mut desk.state, dying.pane, other.pane),
                "{layout}: a layout sweep during the fade stacked a neighbour over \
                 the window fading"
            );
        }
    }
}

/// **#233: a neighbour growing into a closed window's tile keeps its
/// picture against the edge that does not move.**
///
/// Since #128 the layout gives a closed window's tile back the
/// moment the close is asked for: the neighbour glides into it while
/// the closed window fades in place, in front of it. In tiling the
/// window closed is the left of two, so its neighbour grows to the
/// left -- its left edge moves, its right edge stands still -- and its
/// client has not drawn at the new size yet. Before #233 its
/// half-width picture was stretched across the rectangle the glide
/// had reached, its text up to twice as wide behind the fade. Under
/// `anchor` the old picture is drawn at its own size against
/// the right edge, and the strip the neighbour has grown into is
/// `resize.background`'s, behind the fading window; once the client
/// answers, its new, wider picture stands against that edge too, cut
/// on the left until the glide gets there, so the neighbour's contents
/// do not move at all. `crop`, the default, keeps the picture at the
/// rectangle's left, going with the moving edge instead.
#[test]
fn a_neighbour_growing_into_a_closed_windows_tile_keeps_its_picture_against_its_far_edge() {
    let mut desk = Desk::new();
    desk.state.resizing.fill = crate::resizing::Fill::Anchor;
    let (dying, others) = closing_scene(&mut desk, "tiling");
    for opened in others.iter().chain([&dying]) {
        desk.answer(opened);
    }
    desk.state.clock.advance(Duration::from_secs(1));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    let survivor = close_and_see_who_moved(&mut desk, "tiling", &dying, &others);
    let pane = survivor.pane;
    let start = desk.state.clock.now();
    assert!(
        above(&mut desk.state, dying.pane, pane),
        "the premise: the closed window fades in front of the neighbour"
    );
    assert_eq!(
        desk.state.resize_pins(pane),
        (true, false),
        "the neighbour grows to the left: its left edge moves and its right \
         edge stands still"
    );

    // The neighbour's client picture at `at`, as `render::elements`
    // places it, and the size it is placed from.
    let placed = |state: &Solium, at: Duration| {
        let held = state.panes.get(pane).expect("the pane is here");
        let committed = held
            .client()
            .map(|window| window.geometry().size)
            .expect("the pane has its client");
        let outer = state.pane_outer(held);
        let frame = state.drawn_at(held, outer, at);
        (
            committed,
            crate::render::place_client(state, held, &frame, outer.size, committed),
        )
    };
    // How far the picture's right edge is from the rectangle's.
    let off_the_right = |placed: &crate::render::Placed, committed: Size<i32, Logical>| {
        placed.origin.x + f64::from(committed.w) * placed.fit.factor.x
            - (placed.client.loc.x + placed.client.size.w)
    };

    let half_way = start + Duration::from_millis(120);
    let (committed, old) = placed(&desk.state, half_way);
    assert!(
        old.client.size.w > f64::from(committed.w) + 1.0,
        "the glide has grown past the old picture, or this proves nothing: {old:?}"
    );
    assert_eq!(
        old.fit.factor,
        smithay::utils::Scale::from((1.0, 1.0)),
        "the old picture at its own size, not stretched across the glide"
    );
    assert!(
        off_the_right(&old, committed).abs() < 1e-9,
        "and against the right edge, which is standing still: {old:?}"
    );
    let strips = crate::render::uncovered(old.client, old.origin, committed.to_f64(), 1.0);
    assert!(
        strips[2].size.w > 0 && strips[3].size.w == 0,
        "the background is the strip it has grown into, on the left: {strips:?}"
    );

    desk.state.resizing.fill = crate::resizing::Fill::Crop;
    let (_, cropped) = placed(&desk.state, half_way);
    assert!(
        (cropped.origin.x - cropped.client.loc.x).abs() < 1e-9,
        "`crop` keeps the picture at the left edge, which is moving: {cropped:?}"
    );
    desk.state.resizing.fill = crate::resizing::Fill::Anchor;

    desk.answer(survivor);
    let (answered, new) = placed(&desk.state, half_way + Duration::from_millis(20));
    assert!(
        f64::from(answered.w) > new.client.size.w + 1.0,
        "the premise: the client answered wider than the glide has reached: {new:?}"
    );
    assert_eq!(new.fit.factor, smithay::utils::Scale::from((1.0, 1.0)));
    assert_eq!(new.fit.crop, Some(new.client), "cut to the rectangle");
    assert!(
        off_the_right(&new, answered).abs() < 1e-9,
        "the answer against the right edge too, cut on the left: {new:?}"
    );
}

/// **A refused window fades back in front of the neighbour giving
/// its space back.**
///
/// The mirror of the close: the sweep `refused` runs places every
/// window, left to right, so a neighbour placed after the window
/// coming back -- giving back the space it had moved into -- was
/// stacked over the window fading into it. And the dialog route
/// sweeps again inside the return, so a relayout during it is
/// driven too.
#[test]
fn a_refused_window_fades_back_in_front_of_the_neighbour_making_room() {
    for layout in LAYOUTS {
        let mut desk = Desk::new();
        let (dying, others) = closing_scene(&mut desk, layout);
        close_and_see_who_moved(&mut desk, layout, &dying, &others);
        desk.ask();
        desk.refuse();
        assert!(
            desk.events()
                .ends_with(&format!("refused {}", dying.pane.get())),
            "{layout}: the premise is a refusal the layout heard"
        );
        for other in &others {
            assert!(
                above(&mut desk.state, dying.pane, other.pane),
                "{layout}: a neighbour giving the space back is drawn over the \
                 window fading back into it"
            );
        }

        desk.state.clock.advance(Duration::from_millis(50));
        desk.state.trigger_relayout();
        for other in &others {
            assert!(
                above(&mut desk.state, dying.pane, other.pane),
                "{layout}: a layout sweep during the return stacked a neighbour \
                 over the window fading back in"
            );
        }
    }
}

/// **#135: `sol.window_at` over a window fading out answers the
/// neighbour that grew into its place.** The close has landed and
/// the request has gone out, so the window is held at opacity zero,
/// stacked in front of the neighbour the layout moved into its
/// space -- which is what the user sees there. The Rust hit test
/// has seen past it since #127 (`Frame::covers`); `sol.window_at`
/// matched the rectangle and found the window nobody can see, so a
/// scrolling drop there was dropped onto a window in no strip and
/// went nowhere: here the third window, dropped there, joins the
/// neighbour's column.
#[test]
fn sol_window_at_over_a_window_fading_out_answers_the_neighbour_in_its_place() {
    for layout in LAYOUTS {
        let mut desk = Desk::new();
        let (dying, others) = closing_scene(&mut desk, layout);
        // Every client draws the tile it was given, as a real one
        // does, so each is drawn over the whole of it.
        for opened in others.iter().chain([&dying]) {
            desk.answer(opened);
        }
        // The keyboard elsewhere, so the request going out hands
        // nothing on: a window given the keyboard is raised, and
        // that would put a neighbour back over the fade.
        let elsewhere = desk
            .state
            .panes
            .get(others[0].pane)
            .and_then(Pane::client)
            .cloned()
            .expect("the pane has its client");
        desk.state.focus_window(
            &elsewhere,
            SERIAL_COUNTER.next_serial(),
            crate::script::FocusSource::Script,
        );
        desk.state.clock.advance(Duration::from_secs(1));
        let now = desk.state.clock.now();
        desk.state.settle(now);
        let tile = desk.placed(dying.pane);
        let point = Point::<f64, Logical>::from((
            f64::from(tile.loc.x + tile.size.w / 2),
            f64::from(tile.loc.y + tile.size.h / 2),
        ));
        let moved = close_and_see_who_moved(&mut desk, layout, &dying, &others);
        desk.answer(moved);
        let moved = moved.pane;
        desk.ask();
        desk.state.clock.advance(Duration::from_millis(300));
        let now = desk.state.clock.now();
        desk.state.settle(now);
        desk.state.sync_panes();
        desk.pump();

        let now = desk.state.clock.now();
        let fading = drawn_now(&desk.state, dying.pane, now);
        assert!(
            !fading.shows()
                && fading.rect.contains(point)
                && drawn_now(&desk.state, moved, now).covers(point)
                && above(&mut desk.state, dying.pane, moved),
            "{layout}: the premise: the window being closed is drawn at nothing over \
             {point:?}, in front of the neighbour now drawn there"
        );
        assert_eq!(
            window_at_from_lua(&desk, point),
            moved.get().to_string(),
            "{layout}: `sol.window_at` over the neighbour that grew into a closing \
             window's place answered the window drawn at nothing, {}",
            dying.pane.get()
        );

        if layout == "scrolling"
            && let Some(third) = others.iter().find(|other| other.pane != moved)
        {
            let dropped = desk
                .state
                .panes
                .get(third.pane)
                .and_then(Pane::client)
                .cloned()
                .expect("the pane has its client");
            desk.state.trigger_drop(&dropped, point.x, point.y);
            desk.state.clock.advance(Duration::from_secs(1));
            let now = desk.state.clock.now();
            desk.state.settle(now);
            assert_eq!(
                desk.placed(third.pane).loc.x,
                desk.placed(moved).loc.x,
                "a window dropped over the neighbour in a closing window's place did \
                 not join its column"
            );
        }
    }
}

/// **Pinned as it stands: `sol.window_at` sees through the panes
/// the snapshot leaves out, where the Rust walk stops at them.**
/// One rule, [`owns`], asked over two lists: `window_under_at`
/// walks every pane, and `sol.window_at` the snapshot, which
/// leaves out a pane that is not managed (a menu, a tooltip), a
/// pane whose application has not arrived when
/// `loading.reserves_a_slot` is off, and a pane scripts have been
/// told has gone. Over each, the Rust walk answers it -- or, for
/// a pane with no client, nothing -- and a script is answered with
/// the window behind. Older than #134, and a change of its own;
/// when one of the lists changes, this is the test that has to.
#[test]
fn sol_window_at_sees_through_the_panes_the_snapshot_leaves_out() {
    /// A floating window at `loc`, drawing `size`.
    fn floating(desk: &mut Desk, loc: (i32, i32), size: (i32, i32)) -> Opened {
        let opened = desk.open_surface();
        commit_buffer(&desk.client, &desk.qh, &opened.surface, size.0, size.1);
        desk.pump();
        let window = desk
            .state
            .panes
            .get(opened.pane)
            .and_then(Pane::client)
            .cloned()
            .expect("the pane has its client");
        desk.state.map_stacked(window, loc, false);
        desk.state.sync_panes();
        opened
    }

    let point = Point::<f64, Logical>::from((960.0, 540.0));
    for kind in ["unmanaged", "a slot not reserved", "gone"] {
        let mut desk = Desk::new();
        desk.install("");
        let under = floating(&mut desk, (100, 100), (1600, 900));
        let over = if kind == "a slot not reserved" {
            desk.state.loading.reserves_a_slot = false;
            let source = crate::pane::loading_source(None);
            desk.state.open_loading("app", None, source, None)
        } else {
            floating(&mut desk, (200, 200), (1200, 700)).pane
        };
        desk.state.clock.advance(Duration::from_secs(1));
        let now = desk.state.clock.now();
        desk.state.settle(now);
        if let Some(pane) = desk.state.panes.get_mut(over) {
            match kind {
                "unmanaged" => pane.unmanage(),
                "gone" => pane.went(),
                _ => {}
            }
        }
        let client = desk
            .state
            .panes
            .get(over)
            .and_then(Pane::client)
            .map(|window| desk.state.window_id(window));

        assert_eq!(
            desk.state
                .window_under(point)
                .map(|(window, _)| desk.state.window_id(&window)),
            client,
            "{kind}: the Rust walk does not stop at the pane in front"
        );
        assert!(
            desk.state
                .snapshot()
                .windows
                .iter()
                .all(|row| row.id != over.get()),
            "{kind}: the snapshot lists the pane in front"
        );
        assert_eq!(
            window_at_from_lua(&desk, point),
            under.pane.get().to_string(),
            "{kind}: `sol.window_at` does not see through the pane in front to the \
             window behind -- the two lists agree now, and `owns`'s note is out of date"
        );
    }
}

/// **A window that has gone is not laid out again, or closed again,
/// in the frame it went.**
///
/// The pane outlives `close` by the rest of the frame -- it is
/// retired in `sync_panes` -- and `trigger_close` retires its close
/// timers so nothing gives it back. That used to make it an
/// ordinary window to everything else in the frame: listed in
/// `sol.windows()` as not leaving, so a stateless layout placed it
/// and `move_pane` replaced the transform holding it invisible with
/// one at full opacity, and `close_pane` would start a close on it,
/// telling a layout `closing` after `close`.
#[test]
fn a_window_that_has_gone_is_neither_placed_nor_closed_again() {
    let mut desk = Desk::new();
    let (_, toplevel, pane) = desk.open();
    desk.install(&format!(
        r#"{RECORDER}
sol.on("layout", function()
for _, window in ipairs(sol.windows()) do
sol.place(window.id, {{ x = window.x, y = window.y, w = window.w, h = window.h }})
end
end)
"#
    ));
    desk.state.close_pane(pane);
    desk.ask();
    toplevel.destroy();
    desk.pump();
    let id = pane.get();
    assert!(
        desk.events() == format!("closing {id}*,close {id}*")
            && desk.state.panes.get(pane).is_some(),
        "the premise: `close` was told and the pane is still here until \
         `sync_panes`: {}",
        desk.events()
    );

    desk.state.trigger_relayout();
    let now = desk.state.clock.now();
    let drawn = drawn_now(&desk.state, pane, now);
    assert!(
        drawn.opacity.abs() < f32::EPSILON,
        "a layout sweep after `close` drew the window that went at opacity {}",
        drawn.opacity
    );

    desk.state.close_pane(pane);
    assert_eq!(
        desk.events(),
        format!("closing {id}*,close {id}*"),
        "the window that went was closed again, after `close`"
    );
}

/// **A `monitors` event in the frame a window went keeps no leaf
/// for it.** `adopt` exists to put back whatever the arrangement is
/// missing, and the window that went was still listed, as a window
/// that was not leaving -- so it was put back into its tree, for
/// good: nothing sends a second `close`.
///
/// Both settings: `"when_gone"` treats a leaving window as an
/// ordinary one, so for it only leaving the window that went out of
/// the list entirely is enough.
#[test]
fn adopt_in_the_frame_a_window_went_keeps_no_leaf_for_it() {
    for before in [
        "",
        "require(\"config\").tiling.reflow_on_close = \"when_gone\"",
    ] {
        let mut desk = Desk::new();
        let (_, _, kept) = desk.open();
        let (_, toplevel, going) = desk.open();
        desk.arrange("tiling", before);
        let half = desk.placed(kept);

        desk.state.close_pane(going);
        desk.ask();
        toplevel.destroy();
        desk.pump();
        desk.state.trigger_monitors_changed();
        desk.state.space.refresh();
        desk.state.sync_panes();
        desk.pump();
        assert!(
            desk.state.panes.get(going).is_none(),
            "the premise: the pane is retired"
        );

        desk.state.trigger_relayout();
        assert!(
            desk.placed(kept).size.w > half.size.w,
            "{before:?}: the tree still divides the screen with a window that is \
             gone: {:?}",
            desk.placed(kept)
        );
    }
}

/// **A client slower to quit than the grace period moves the layout
/// three times.** Not a fault this suite can fix and not one it
/// hides: the compositor cannot tell a client that is still quitting
/// from one that has decided to stay, so at the grace deadline the
/// window is given back, and it goes when the client finally does.
/// A layout that closes up at once therefore grows the neighbour at
/// the press, shrinks it when `refused` puts the window back, and
/// grows it again at `close`. `reflow_on_close = "when_gone"` is the
/// setting that moves it once; `config.lua` says so.
#[test]
fn a_client_slower_than_the_grace_period_moves_the_layout_three_times() {
    let mut desk = Desk::new();
    let (_, _, kept) = desk.open();
    let (_, toplevel, slow) = desk.open();
    desk.arrange("tiling", "");
    let half = desk.placed(kept);

    desk.state.close_pane(slow);
    let grown = desk.placed(kept);
    desk.ask();
    desk.refuse();
    let back = desk.placed(kept);
    toplevel.destroy();
    desk.pump();
    let gone = desk.placed(kept);

    let id = slow.get();
    assert_eq!(
        desk.events(),
        format!("closing {id}*,refused {id},close {id}*")
    );
    assert!(
        grown.size.w > half.size.w && back == half && gone.size.w > half.size.w,
        "the neighbour was {half:?}, {grown:?} at the press, {back:?} at the \
         refusal and {gone:?} once the client went"
    );
}

// #128 and #133 together.
//
// Every test above commits a 64x64 buffer, which no tile cuts, and
// no #133 test closes a window, so the cut never engaged in a close
// or a refusal. The ones below give a window a client that will not
// shrink as far as its tile, and close it.

/// Two windows side by side under tiling, and the left one's client
/// committing a buffer 200 pixels wider than its tile -- a browser
/// at its minimum width -- while the right one's fills its own:
/// `(left, right, the left one's tile)`. The left one, because a
/// tiling sweep places it first and its neighbour after it, which
/// is the order a close has to raise it against.
fn an_oversized_window_beside_another(
    desk: &mut Desk,
    before: &str,
) -> (Opened, Opened, Rectangle<i32, Logical>) {
    let mut opened: Vec<Opened> = (0..2).map(|_| desk.open_surface()).collect();
    desk.arrange("tiling", before);
    opened.sort_by_key(|each| desk.placed(each.pane).loc.x);
    let right = opened.pop().expect("two windows were opened");
    let left = opened.pop().expect("two windows were opened");
    let tile = desk.placed(left.pane);
    desk.answer(&right);
    commit_buffer(
        &desk.client,
        &desk.qh,
        &left.surface,
        tile.size.w + 200,
        tile.size.h,
    );
    desk.pump();
    desk.state.sync_panes();
    assert_eq!(
        committed(&desk.state, left.pane),
        Size::from((tile.size.w + 200, tile.size.h)),
        "the premise: the client committed more than its tile"
    );
    assert_eq!(
        desk.state.pane_outer_of(left.pane),
        Some(tile),
        "the premise: and it is held inside the tile"
    );
    (left, right, tile)
}

/// The size `pane`'s client last committed.
fn committed(state: &Solium, pane: crate::pane::PaneId) -> Size<i32, Logical> {
    state
        .panes
        .get(pane)
        .and_then(Pane::client)
        .map(|window| window.geometry().size)
        .expect("the pane has a client")
}

/// How `pane` is drawn at `at`, and how its client's buffer goes
/// into that frame: through `render::place_client`, which is what
/// `render::elements` draws the surfaces with and what
/// `surface_under` inverts.
fn pictured(
    state: &Solium,
    pane: crate::pane::PaneId,
    at: Duration,
) -> (Frame, crate::render::Placed) {
    let held = state.panes.get(pane).expect("the pane is here");
    let outer = state.pane_outer(held);
    let frame = state.drawn_at(held, outer, at);
    let drawn =
        crate::render::place_client(state, held, &frame, outer.size, committed(state, pane));
    (frame, drawn)
}

/// What is wrong with how a client wider than its picture is drawn
/// into a frame, or `None` when nothing is.
///
/// Across, where every window these tests assert on is wider than
/// anything its frame pictures, the buffer is cut to the drawn
/// client rectangle and scaled by the frame's zoom and by nothing
/// else: a picture of the window, cut, and not the window squashed
/// into it. Down, a glide can picture the window taller than its
/// buffer and stretch it, as every glide does, so there it is only
/// held to not being scaled below the zoom. A thousandth under is
/// allowed for the half pixel `render::fit` does not call a cut;
/// the squash this looks for is a sixth.
fn why_not_cut(frame: &Frame, drawn: &crate::render::Placed) -> Option<String> {
    let factor = drawn.fit.factor;
    if drawn.fit.crop != Some(drawn.client) {
        return Some(format!("not cut to its picture: {:?}", drawn.fit));
    }
    if (factor.x - frame.zoom.0).abs() > 1e-9 {
        return Some(format!(
            "scaled across by {} at a zoom of {}",
            factor.x, frame.zoom.0
        ));
    }
    if factor.y < frame.zoom.1 * (1.0 - 1e-3) {
        return Some(format!(
            "squashed down to {} at a zoom of {}",
            factor.y, frame.zoom.1
        ));
    }
    None
}

/// Whether `rect` lies inside `tile`, give or take half a pixel.
fn inside(rect: Rectangle<f64, Logical>, tile: Rectangle<i32, Logical>) -> bool {
    let tile = tile.to_f64();
    rect.loc.x >= tile.loc.x - 0.5
        && rect.loc.y >= tile.loc.y - 0.5
        && rect.loc.x + rect.size.w <= tile.loc.x + tile.size.w + 0.5
        && rect.loc.y + rect.size.h <= tile.loc.y + tile.size.h + 0.5
}

/// Whether two rectangles are the same to within a millionth of a
/// pixel: a blend's last frame is float arithmetic.
fn same(one: Rectangle<f64, Logical>, other: Rectangle<f64, Logical>) -> bool {
    (one.loc.x - other.loc.x).abs() < 1e-6
        && (one.loc.y - other.loc.y).abs() < 1e-6
        && (one.size.w - other.size.w).abs() < 1e-6
        && (one.size.h - other.size.h).abs() < 1e-6
}

/// Put the pointer at `point`, as a motion with no surface under it
/// would: what `sol.cursor()` reads when a layout decides where a
/// new window goes.
fn point_at(state: &mut Solium, point: Point<f64, Logical>) {
    let pointer = state.seat.get_pointer().expect("the seat has a pointer");
    pointer.motion(
        state,
        None,
        &smithay::input::pointer::MotionEvent {
            location: point,
            serial: SERIAL_COUNTER.next_serial(),
            time: 0,
        },
    );
    pointer.frame(state);
}

/// **A window being closed is cut to the tile it left for every
/// frame of its fade.**
///
/// `closing` takes the window out of the tiling tree and the
/// neighbour grows into its space at once, and nothing lets the
/// window out of its tile: `Pane::placed` is kept on purpose,
/// because it is what `render::fit` cuts the fading window to. The
/// client committed 200 pixels more than its tile. Cleared either
/// way below, the assertion on `Pane::placed` fails first, and the
/// two under it fail on their own without it: cleared at `closing`,
/// that whole buffer is scaled into every frame of the fade and the
/// cut assertion fails; cleared before `present::close` reads the
/// pane's rectangle, the fade starts at the committed width, over
/// the neighbour, and the one before it fails.
///
/// Every 10 ms from the press to past `CLOSING`, where the window
/// is held invisible, and across a relayout 80 ms in -- a window
/// opening or a client's `set_parent` causes one.
#[test]
fn a_closed_window_is_cut_to_the_tile_it_left_for_the_whole_fade() {
    let mut desk = Desk::new();
    let (dying, survivor, tile) = an_oversized_window_beside_another(&mut desk, "");
    let beside = desk.placed(survivor.pane);

    let pressed = desk.state.clock.now();
    desk.state.close_pane(dying.pane);
    desk.pump();
    assert!(
        desk.placed(survivor.pane).size.w > beside.size.w,
        "the premise: the neighbour grew into the space at once"
    );

    for step in 0..=21_u64 {
        if step == 8 {
            desk.state.clock.advance(Duration::from_millis(80));
            desk.state.trigger_relayout();
        }
        let ms = 10 * step;
        assert_eq!(
            desk.state.panes.get(dying.pane).and_then(Pane::placed),
            Some(tile),
            "{ms}ms into the fade the window was let out of the tile it left"
        );
        let (frame, drawn) = pictured(&desk.state, dying.pane, pressed + Duration::from_millis(ms));
        assert!(
            inside(frame.rect, tile),
            "{ms}ms into the fade the window reaches outside the tile it left: \
             {:?}, and the tile is {tile:?}",
            frame.rect
        );
        if let Some(why) = why_not_cut(&frame, &drawn) {
            panic!("{ms}ms into the fade the window is {why}");
        }
    }
    let (held, _) = pictured(
        &desk.state,
        dying.pane,
        pressed + Duration::from_millis(210),
    );
    assert!(
        held.opacity.abs() < f32::EPSILON && (held.zoom.0 - 0.86).abs() < 1e-9,
        "the premise: the fade ran to its held end: {held:?}"
    );
}

/// **A mode switched during a fade does not squash the window
/// leaving.**
///
/// `modes.use` lets every window out of its tile when the layout in
/// charge changes, and that includes a window being closed. On
/// stage the let-go cleared `Pane::placed` at once while the fade's
/// frames stayed pinned at the size of the tile the window was
/// closed in, so `render::fit`, which cuts only a tiled pane,
/// scaled the whole of a buffer 200 pixels wider than that tile
/// into them for the rest of the fade. A let-go now waits while the
/// window is leaving (`Pane::let_go`), and each way one arrives is
/// driven 60 ms in: a switch to floating, a switch to scrolling, a
/// script placing the window with `tile = false`, and a switch to
/// scrolling with both layouts waiting for the client -- where
/// scrolling adopts the leaving window and gives it a column during
/// the fade.
///
/// And a let-go that waited is not lost. Refused after the switch to
/// floating, the window is in no tile, and its return lands at the
/// size its client committed, because `give_back` takes the let-go
/// before it aims the return; aimed first, the return lands at the
/// tile's width. Refused after scrolling gave it a column, it is
/// still in a tile: the column cancelled the let-go. That last case
/// passes on stage too, where nothing waited to be cancelled; it is
/// here for the cancel, and fails without it.
#[test]
fn a_mode_switched_during_a_fade_does_not_squash_the_window_leaving() {
    // Scrolling loaded as well as tiling, so that `super+s` has a
    // layout to switch to.
    const SCROLLING: &str = "require(\"scrolling\")";
    const BOTH_WAIT: &str = "require(\"scrolling\")\n\
                             require(\"config\").tiling.reflow_on_close = \"when_gone\"\n\
                             require(\"config\").scrolling.reflow_on_close = \"when_gone\"";
    for (name, before, key) in [
        ("a switch to floating", SCROLLING, Some("super+t")),
        ("a switch to scrolling", SCROLLING, Some("super+s")),
        ("a place with tile = false", SCROLLING, None),
        (
            "a switch to scrolling waiting for the client",
            BOTH_WAIT,
            Some("super+s"),
        ),
    ] {
        let mut desk = Desk::new();
        let (dying, _survivor, tile) = an_oversized_window_beside_another(&mut desk, before);
        let pressed = desk.state.clock.now();
        desk.state.close_pane(dying.pane);
        desk.pump();

        desk.state.clock.advance(Duration::from_millis(60));
        match key {
            Some(key) => {
                assert!(desk.state.trigger(key), "{name}: {key} was not handled");
            }
            None => desk.state.apply(Outcome {
                commands: vec![Command::Place {
                    id: dying.pane.get(),
                    rect: to_rect(tile),
                    animation: AnimationSpec::default(),
                    tile: false,
                    inside: None,
                    cramped: false,
                    moved: false,
                }],
                ..Outcome::default()
            }),
        }
        assert!(
            desk.state
                .panes
                .get(dying.pane)
                .and_then(Pane::placed)
                .is_some(),
            "{name}: the window being closed was let out of its tile during its fade"
        );
        for step in 6..=21_u64 {
            let ms = 10 * step;
            let (frame, drawn) =
                pictured(&desk.state, dying.pane, pressed + Duration::from_millis(ms));
            if let Some(why) = why_not_cut(&frame, &drawn) {
                panic!("{name}: {ms}ms into the fade the window is {why}");
            }
        }

        desk.ask();
        let back = desk.refuse();
        assert!(
            desk.events()
                .ends_with(&format!("refused {}", dying.pane.get())),
            "{name}: the premise is a refusal the layout heard: {}",
            desk.events()
        );
        let placed = desk.state.panes.get(dying.pane).and_then(Pane::placed);
        if key == Some("super+t") {
            assert_eq!(
                placed, None,
                "{name}: the window came back floating and still held in its old tile"
            );
            let outer = desk
                .state
                .pane_outer_of(dying.pane)
                .expect("the pane is here");
            assert_eq!(
                outer.size,
                committed(&desk.state, dying.pane),
                "{name}: the premise: out of every tile, the pane is the size its \
                 client committed"
            );
            let landed = drawn_now(&desk.state, dying.pane, back + Duration::from_secs(1));
            assert!(
                same(landed.rect, outer.to_f64()),
                "{name}: the return lands at {:?} rather than at the size the client \
                 committed, {outer:?}",
                landed.rect
            );
        } else {
            assert!(
                placed.is_some(),
                "{name}: the layout in charge holds the window in a tile, and the \
                 refusal let it out"
            );
        }
    }
}

/// **A give-back that declines keeps the fade cut, after a mode
/// switch as before one.**
///
/// `give_back` takes a let-go that waited before it aims the
/// window's return, and a give-back can decline -- `present::clear`
/// answers `false` when the transform slot is busy -- which leaves
/// the window fading. So what it took is put back, and the fade is
/// still cut to its tile. Declined here on the dialog route, inside
/// `CLOSING`, with `present::jam_slot`, 60 ms after a switch to
/// floating let the window go.
#[test]
fn a_give_back_that_declines_keeps_a_fade_cut_after_a_mode_switch() {
    let mut desk = Desk::new();
    let (dying, _survivor, tile) = an_oversized_window_beside_another(&mut desk, "");
    desk.state.close_pane(dying.pane);
    desk.state.clock.advance(Duration::from_millis(60));
    assert!(desk.state.trigger("super+t"), "super+t was not handled");
    if let Some(busy) = desk.state.panes.get(dying.pane) {
        present::jam_slot(busy);
    }
    let _dialog = desk.open_dialog_for(&dying.toplevel);

    let held = desk.state.panes.get(dying.pane).expect("the pane is here");
    assert!(
        held.leaving() && held.answered(),
        "the premise: the dialog answered the close, and the give-back declined"
    );
    assert_eq!(
        held.placed(),
        Some(tile),
        "a give-back that declined let the fading window out of the tile it is cut to"
    );
}

/// **A rescue during a fade keeps the let-go the window is waiting
/// on.**
///
/// `rescue_offscreen` drags a window left on no screen back onto
/// one, and keeps its standing as it found it -- which for a window
/// let go of during its close is "tiled for the fade, and owed the
/// let-go". So it moves the tile and does not set it: setting it is
/// a layout's newer word and cancels the let-go, and the window,
/// refused after the switch to floating, would come back held in
/// the tile it was rescued to.
#[test]
fn a_rescue_during_a_fade_keeps_the_let_go_it_is_waiting_on() {
    let mut desk = Desk::new();
    let (dying, _survivor, tile) = an_oversized_window_beside_another(&mut desk, "");
    desk.state.close_pane(dying.pane);
    desk.state.clock.advance(Duration::from_millis(60));
    assert!(desk.state.trigger("super+t"), "super+t was not handled");
    let window = desk
        .state
        .panes
        .get(dying.pane)
        .and_then(Pane::client)
        .cloned()
        .expect("the pane has a client");
    desk.state.map_stacked(window, (-5000, 300), false);
    desk.state.sync_panes();

    desk.state.rescue_offscreen();
    let rescued = desk.state.panes.get(dying.pane).and_then(Pane::placed);
    assert!(
        rescued.is_some_and(|rescued| rescued != tile && rescued.loc.x >= 0),
        "the premise: the rescue brought the fading window back, still in the tile \
         it is cut to, moved with it: {rescued:?}"
    );
    desk.ask();
    desk.refuse();
    assert_eq!(
        desk.state.panes.get(dying.pane).and_then(Pane::placed),
        None,
        "the rescue cancelled the let-go, and the window came back floating and held \
         in a tile"
    );
}

/// **A refused window wider than its tile is cut to its new tile
/// from the frame it comes back on, while its zoom grows from the
/// close's 0.86 to 1.**
///
/// A third window opens while the first is held invisible, so the
/// tile the refused window comes back to is not the one it was
/// closed in. Its return starts from the held end of the close --
/// the old tile's picture, shrunk to 0.86 -- and glides into the new
/// tile at the layout's pace. The pane is held inside the new tile
/// from the first frame; every frame draws the buffer at its own
/// size times the zoom, cut to the picture and never squashed into
/// it; and the zoom only grows, reaching 1 as the window lands cut
/// to exactly its new tile.
#[test]
fn a_refused_oversized_window_is_cut_to_its_new_tile_as_it_grows_back() {
    let mut desk = Desk::new();
    let (dying, _survivor, tile) = an_oversized_window_beside_another(&mut desk, "");
    let wide = committed(&desk.state, dying.pane);
    desk.state.close_pane(dying.pane);
    desk.ask();
    let _third = desk.open_surface();
    let back = desk.refuse();
    assert!(
        desk.events()
            .ends_with(&format!("refused {}", dying.pane.get())),
        "the premise is a refusal the layout heard: {}",
        desk.events()
    );
    let new_tile = desk.placed(dying.pane);
    assert!(
        new_tile != tile && new_tile.size.w < wide.w,
        "the premise: back in a different tile, which it is still wider than: \
         {tile:?} became {new_tile:?}"
    );
    assert_eq!(
        desk.state.pane_outer_of(dying.pane),
        Some(new_tile),
        "the refused window is held inside its new tile from the frame it comes back"
    );

    let (_, fading) = drift((desk.state.clock.now() - back).as_secs_f64());
    let (first, _) = pictured(&desk.state, dying.pane, back);
    assert!(
        (first.zoom.0 - 0.86).abs() < 1e-9 + 0.14 * f64::from(fading)
            && first.opacity < 0.05 + fading,
        "the return starts from the held end of the close, not from the window at \
         rest: {first:?}"
    );
    let mut zoom = first.zoom.0;
    for step in 0..=30_u64 {
        let ms = 10 * step;
        let (frame, drawn) = pictured(&desk.state, dying.pane, back + Duration::from_millis(ms));
        if let Some(why) = why_not_cut(&frame, &drawn) {
            panic!("{ms}ms into the return the window is {why}");
        }
        assert!(
            frame.zoom.0 >= zoom - 1e-9,
            "{ms}ms into the return the zoom went back from {zoom} to {}",
            frame.zoom.0
        );
        zoom = frame.zoom.0;
    }
    let (landed, drawn) = pictured(&desk.state, dying.pane, back + Duration::from_secs(1));
    assert!(
        (landed.zoom.0 - 1.0).abs() < 1e-9 && (landed.zoom.1 - 1.0).abs() < 1e-9,
        "the return lands at a zoom of {:?}",
        landed.zoom
    );
    assert!(
        same(landed.rect, new_tile.to_f64())
            && drawn
                .fit
                .crop
                .is_some_and(|crop| same(crop, new_tile.to_f64())),
        "the return lands cut to its new tile {new_tile:?}: drawn at {:?}, cut to {:?}",
        landed.rect,
        drawn.fit.crop
    );
}

/// **A new window's layout pass part of the way through another
/// window's open, or through its return from a refused close,
/// carries that animation on, still cut.**
///
/// `move_pane` starts every glide from what is on screen
/// (`present::frame`), so a window a sweep moves while it is still
/// opening, or still fading back in, goes on from where it had got
/// to -- the zoom it had reached and the opacity -- rather than
/// from a window at rest. For a client wider than its tile that
/// zoom is what the cut is a picture of, so both are driven with
/// one: 80 ms in, with the pointer over it, a new window opens and
/// the layout splits the window being watched. On the frame the new
/// window arrives it is drawn where it was, every frame after is
/// cut at its zoom, and it lands cut to the tile the pass gave it.
///
/// The open is the shipped `open.lua`'s, which `tiling`'s own
/// placement carries on from, in the order `init.lua` loads them.
#[test]
fn a_layout_pass_part_way_through_an_open_or_a_return_keeps_the_window_cut() {
    for case in ["open", "return"] {
        let mut desk = Desk::new();
        let pane = if case == "open" {
            desk.arrange("tiling", "require(\"open\")");
            let opening = desk.open_surface();
            let tile = desk.placed(opening.pane);
            commit_buffer(
                &desk.client,
                &desk.qh,
                &opening.surface,
                tile.size.w + 200,
                tile.size.h,
            );
            desk.pump();
            desk.state.sync_panes();
            opening.pane
        } else {
            let (dying, _survivor, _tile) = an_oversized_window_beside_another(&mut desk, "");
            desk.state.close_pane(dying.pane);
            desk.ask();
            desk.refuse();
            dying.pane
        };
        let was = desk.placed(pane);
        assert!(
            committed(&desk.state, pane).w > was.size.w,
            "{case}: the premise: the client is wider than its tile"
        );

        desk.state.clock.advance(Duration::from_millis(80));
        let arrived = desk.state.clock.now();
        let (before, _) = pictured(&desk.state, pane, arrived);
        assert!(
            before.zoom.0 > 0.9 && before.zoom.0 < 0.99 && before.opacity < 0.99,
            "{case}: the premise: part of the way in: {before:?}"
        );
        let centre = was.to_f64();
        point_at(
            &mut desk.state,
            (
                centre.loc.x + centre.size.w / 2.0,
                centre.loc.y + centre.size.h / 2.0,
            )
                .into(),
        );
        let _new = desk.open_surface();
        let now = desk.placed(pane);
        assert_ne!(
            now, was,
            "{case}: the premise: the new window's pass gave the window a new tile"
        );
        assert!(
            committed(&desk.state, pane).w > now.size.w,
            "{case}: the premise: which the client is wider than too"
        );

        let (moving, fading) = drift((desk.state.clock.now() - arrived).as_secs_f64());
        let (after, _) = pictured(&desk.state, pane, arrived);
        assert!(
            (after.rect.loc.x - before.rect.loc.x).abs() < moving
                && (after.rect.loc.y - before.rect.loc.y).abs() < moving
                && (after.rect.size.w - before.rect.size.w).abs() < moving
                && (after.rect.size.h - before.rect.size.h).abs() < moving
                && (after.zoom.0 - before.zoom.0).abs() < f64::from(fading)
                && (after.opacity - before.opacity).abs() < fading,
            "{case}: the window jumped on the frame the new window arrived: drawn at \
             {before:?}, and then at {after:?}"
        );
        let mut zoom = after.zoom.0;
        for step in 0..=30_u64 {
            let ms = 10 * step;
            let (frame, drawn) = pictured(&desk.state, pane, arrived + Duration::from_millis(ms));
            if let Some(why) = why_not_cut(&frame, &drawn) {
                panic!("{case}: {ms}ms after the new window arrived the window is {why}");
            }
            assert!(
                frame.zoom.0 >= zoom - 1e-9,
                "{case}: {ms}ms after the new window arrived the zoom went back from \
                 {zoom} to {}",
                frame.zoom.0
            );
            zoom = frame.zoom.0;
        }
        let (landed, drawn) = pictured(&desk.state, pane, arrived + Duration::from_secs(1));
        assert!(
            (landed.zoom.0 - 1.0).abs() < 1e-9
                && same(landed.rect, now.to_f64())
                && drawn.fit.crop.is_some_and(|crop| same(crop, now.to_f64())),
            "{case}: the window lands cut to the tile the pass gave it, {now:?}: drawn \
             at {landed:?}, cut to {:?}",
            drawn.fit.crop
        );
    }
}

/// **A menu reaching past the tile of a window fading out in front
/// is that window's while it shows, and not once it has gone.**
///
/// A menu is drawn uncut, past its parent's tile, and a press on it
/// is the menu's even over the neighbour's tile
/// (`a_menu_past_its_parents_tile_takes_the_press`). A close raises
/// the window and fades it in front of the neighbour growing into
/// its space, so the menu is in front too, over a neighbour that
/// now reaches under it. Part of the way through the fade a press
/// on the menu lands on the menu's own pixel through the fade's
/// zoom -- a menu scales with its window -- and once the window is
/// invisible the neighbour has it.
#[test]
fn a_menu_past_the_tile_of_a_window_fading_out_is_the_windows_while_it_shows() {
    let mut desk = Desk::new();
    // The neighbour's picture stretched over all of the space it is
    // growing into, so that a press there has a client to land on
    // once the menu has gone. Under the default since #233 its
    // picture has not reached the point yet; that is asserted at the
    // end.
    desk.state.resizing.fill = crate::resizing::Fill::Stretch;
    let (dying, survivor, tile) = an_oversized_window_beside_another(&mut desk, "");
    let anchor = (tile.size.w - 20, 50);
    let _menu = drawn_popup(
        &mut desk.display,
        &mut desk.state,
        &desk.conn,
        &desk.qh,
        &mut desk.queue,
        &mut desk.client,
        &dying.xdg,
        anchor,
        (120, 80),
    );
    desk.state.sync_panes();
    // The compositor's side of each surface, which is what
    // `surface_under` answers with.
    let toplevel_of = |state: &Solium, pane| {
        state
            .panes
            .get(pane)
            .and_then(Pane::client)
            .and_then(Window::toplevel)
            .map(|toplevel| toplevel.wl_surface().clone())
            .expect("the pane has an xdg toplevel")
    };
    let menu =
        smithay::desktop::PopupManager::popups_for_surface(&toplevel_of(&desk.state, dying.pane))
            .next()
            .map(|(popup, _)| popup.wl_surface().clone())
            .expect("the menu is tracked on its parent");
    let neighbour = toplevel_of(&desk.state, survivor.pane);

    desk.state.close_pane(dying.pane);
    desk.pump();
    desk.state.sync_panes();
    let beside = desk.placed(survivor.pane);

    desk.state.clock.advance(Duration::from_millis(60));
    let (frame, drawn) = pictured(&desk.state, dying.pane, desk.state.clock.now());
    assert!(
        frame.shows() && frame.opacity < 0.95 && frame.zoom.0 < 0.99,
        "the premise: part of the way through the fade: {frame:?}"
    );
    // The menu's pixel 60,40, where the fade draws it: the buffer's
    // corner, and the menu's place in the window plus the pixel's
    // place in the menu, scaled with the window.
    let into = (60.0, 40.0);
    let on_the_menu = |drawn: &crate::render::Placed, into: (f64, f64)| -> Point<f64, Logical> {
        (
            drawn.origin.x + (f64::from(anchor.0) + into.0) * drawn.fit.factor.x,
            drawn.origin.y + (f64::from(anchor.1) + into.1) * drawn.fit.factor.y,
        )
            .into()
    };
    let point = on_the_menu(&drawn, into);
    assert!(
        point.x > f64::from(tile.loc.x + tile.size.w) && beside.to_f64().contains(point),
        "the premise: the point is past the fading window's tile, over the neighbour \
         that has grown into its space: {point:?}, {tile:?}, {beside:?}"
    );
    let (under, origin) = desk
        .state
        .surface_under(point)
        .expect("something is under the point");
    assert_eq!(
        under, menu,
        "the menu of a window fading in front of its neighbour lost the pixel it drew"
    );
    let within = point - origin;
    assert!(
        (within.x - into.0).abs() < 1.0 && (within.y - into.1).abs() < 1.0,
        "a press on the fading menu reached {within:?} of it, not {into:?}"
    );

    // And where the held end of the fade would draw a pixel of the
    // menu, had it anything to draw: the menu shrinks with its
    // window, so the point it had 60 ms in is no longer on it. Its
    // pixel 110,40, which the held end still draws past the tile.
    desk.state.clock.advance(Duration::from_millis(150));
    let (gone, drawn) = pictured(&desk.state, dying.pane, desk.state.clock.now());
    assert!(!gone.shows(), "the premise: the fade is over: {gone:?}");
    let point = on_the_menu(&drawn, (110.0, 40.0));
    assert!(
        point.x > f64::from(tile.loc.x + tile.size.w) && beside.to_f64().contains(point),
        "the premise: that point is past the tile too, over the neighbour: {point:?}"
    );
    assert_eq!(
        desk.state.surface_under(point).map(|(surface, _)| surface),
        Some(neighbour),
        "once the window is invisible its menu has no pixel, and the neighbour \
         has the press"
    );
    // Under the default the neighbour's picture is still at its own
    // size against its right edge, which is standing still, and the
    // point is in the background it is growing over (#233): no
    // client's pixel, so no client has the press.
    desk.state.resizing.fill = crate::resizing::Fill::Anchor;
    assert_eq!(
        desk.state.surface_under(point).map(|(surface, _)| surface),
        None,
        "a press on the background a picture does not cover reached a client"
    );
}

// #126: a window its own application closes.
//
// Every test above closes a window through `close_pane`, which
// animates a live client and asks it afterwards. The ones below
// close it from the client's side, which is what `exit`, `Ctrl+D`
// and an application's own Quit are -- and a disconnect, which is
// the same objects going in another order.

/// What a pane kept of a window whose client has gone.
fn left_of(state: &Solium, pane: crate::pane::PaneId) -> &crate::pane::Left {
    state
        .panes
        .get(pane)
        .and_then(Pane::left)
        .expect("the pane survives its client, as what fades out")
}

/// Let every animation so far land, so what follows starts from a
/// window drawn where it lives.
fn land(desk: &mut Desk) {
    desk.state.clock.advance(Duration::from_secs(1));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    desk.state.space.refresh();
    desk.state.sync_panes();
}

/// A second client of the desk's compositor, for a test that has
/// to disconnect one: `(connection, queue, client)`.
fn another_client(desk: &mut Desk) -> (Connection, wayland_client::EventQueue<Client>, Client) {
    connect(&mut desk.display, &mut desk.state)
}

/// **#126: a window whose client destroys its own toplevel fades
/// out as one the compositor closes does, and is gone on time with
/// nothing else happening.**
///
/// It used to get no frames at all: the pane was retired by the
/// first `sync_panes` after the client went, and the frame drawn
/// before that had nothing of the client left to draw. Here the
/// pane survives as `Content::Leaving`, pinned to where it stood,
/// under `present::close`'s transform -- and is dropped by the
/// frame's own `settle` within a frame of `pane::LEAVING`, with no
/// client event, no `sync_panes` and no layout behind it.
#[test]
fn a_window_that_closes_itself_fades_out_and_is_gone_on_time() {
    let mut desk = Desk::new();
    let opened = desk.open_surface();
    let pane = opened.pane;
    land(&mut desk);
    let stood = desk.state.pane_outer_of(pane).expect("a mapped pane");

    opened.toplevel.destroy();
    desk.pump();

    let left = left_of(&desk.state, pane);
    let since = left.since;
    assert_eq!(left.outer, stood, "it fades from the rectangle it stood at");
    assert!(
        matches!(left.remains, crate::pane::Remains::Lost),
        "no renderer here, so nothing was imported: it is drawn as its frame \
         and a fill, and never as nothing"
    );
    assert!(
        desk.state
            .on_screen()
            .iter()
            .any(|(each, window)| *each == pane && window.is_none()),
        "it is in the walk the renderer draws, and has no client"
    );

    let at = |state: &Solium, after: Duration| state.drawn_id_at(pane, stood, since + after);
    let start = at(&desk.state, Duration::ZERO);
    let half = at(&desk.state, present::CLOSING / 2);
    let end = at(&desk.state, present::CLOSING);
    assert!(
        (start.opacity - 1.0).abs() < 1e-3 && start.rect == stood.to_f64(),
        "it starts as it stood: {start:?}"
    );
    let centre = |rect: Rectangle<f64, Logical>| {
        (
            rect.loc.x + rect.size.w / 2.0,
            rect.loc.y + rect.size.h / 2.0,
        )
    };
    let (cx, cy) = centre(stood.to_f64());
    let (hx, hy) = centre(half.rect);
    assert!(
        half.opacity > 0.1
            && half.opacity < 0.9
            && half.rect.size.w < f64::from(stood.size.w)
            && half.rect.size.h < f64::from(stood.size.h)
            && (hx - cx).abs() < 1.0
            && (hy - cy).abs() < 1.0,
        "half way through it is shrinking and fading where it stood, as \
         `present::close` draws a close: {half:?}"
    );
    assert!(
        !end.shows(),
        "and at the end of the fade it is gone: {end:?}"
    );

    let frame = Duration::from_millis(16);
    assert!(
        desk.state.settle(since + crate::pane::LEAVING - frame),
        "while it fades the frames keep coming: nothing else would ask for one"
    );
    assert!(desk.state.panes.get(pane).is_some(), "and it is still here");
    desk.state.settle(since + crate::pane::LEAVING + frame);
    assert!(
        desk.state.panes.get(pane).is_none(),
        "a frame after its fade it is gone, and only that frame's settle ran"
    );
}

/// **#126: a client that disconnects -- a crash, a `kill` -- goes
/// the same way, and is told gone once.**
///
/// Its objects are destroyed in id order, so its surface goes
/// before its toplevel: `CompositorHandler::destroyed` hears of it
/// first and takes it out of the space, and `toplevel_destroyed`
/// then has nothing to find. Two notices, one `close`.
#[test]
fn a_client_that_disconnects_fades_out_and_is_told_gone_once() {
    let mut desk = Desk::new();
    desk.install(RECORDER);
    let (conn, queue, client) = another_client(&mut desk);
    let qh = queue.handle();
    let (window, toplevel) = open_window(&mut desk.display, &mut desk.state, &conn, &client, &qh);
    desk.state.sync_panes();
    let pane = desk.state.panes.id_of(&window).expect("a pane");
    land(&mut desk);
    let stood = desk.state.pane_outer_of(pane).expect("a mapped pane");

    drop((toplevel, qh, queue, client, conn));
    desk.display
        .dispatch_clients(&mut desk.state)
        .expect("dispatching the disconnect");
    desk.display.flush_clients().expect("flushing");

    let left = left_of(&desk.state, pane);
    assert_eq!(left.outer, stood, "it fades from where it stood");
    assert!(
        !desk.state.space.elements().any(|each| *each == window),
        "and the space has let go of it"
    );
    let since = left.since;
    let half = desk
        .state
        .drawn_id_at(pane, stood, since + present::CLOSING / 2);
    assert!(
        half.opacity > 0.1 && half.opacity < 0.9,
        "half way through its fade: {half:?}"
    );
    let id = pane.get();
    assert_eq!(desk.events(), format!("open {id},close {id}*"));

    desk.state.space.refresh();
    desk.state.sync_panes();
    desk.state
        .settle(since + crate::pane::LEAVING + Duration::from_millis(16));
    assert!(desk.state.panes.get(pane).is_none(), "and gone on time");
    assert_eq!(
        desk.events(),
        format!("open {id},close {id}*"),
        "told once, however many ways it was heard going"
    );
}

/// **#126: several windows of one application going at once each
/// fade, and each is told gone once.**
#[test]
fn every_window_of_a_client_that_disconnects_fades_out() {
    let mut desk = Desk::new();
    desk.install(RECORDER);
    let (conn, queue, client) = another_client(&mut desk);
    let qh = queue.handle();
    let (first, one) = open_window(&mut desk.display, &mut desk.state, &conn, &client, &qh);
    let (second, two) = open_window(&mut desk.display, &mut desk.state, &conn, &client, &qh);
    desk.state.sync_panes();
    let panes = [&first, &second].map(|window| {
        desk.state
            .panes
            .id_of(window)
            .expect("a pane for each window")
    });
    land(&mut desk);

    drop((one, two, qh, queue, client, conn));
    desk.display
        .dispatch_clients(&mut desk.state)
        .expect("dispatching the disconnect");

    for pane in panes {
        assert!(
            desk.state.panes.get(pane).is_some_and(Pane::ghost),
            "each window is fading out"
        );
    }
    let [a, b] = panes.map(crate::pane::PaneId::get);
    assert_eq!(
        desk.events(),
        format!("open {a},open {b},close {a}*,close {b}*"),
        "and each is told gone, once"
    );
}

/// **#126: what the renderer imported is what a window leaves --
/// taken in `toplevel_destroyed`, every surface of its tree, and
/// still there after the surfaces are destroyed.**
///
/// smithay's `DummyRenderer` stands in for the GPU: it imports
/// shared memory into a texture of the buffer's size and files it
/// by its context, as GLES does, with no GPU at all. The client
/// destroys the lot in the order an orderly exit does, in one flush,
/// so by the time anything could look the surfaces are gone -- and
/// smithay's own reset has dropped what it held for them.
#[test]
fn an_orderly_exit_leaves_the_picture_the_renderer_imported() {
    use smithay::backend::renderer::element::surface::{
        WaylandSurfaceRenderElement, render_elements_from_surface_tree,
    };
    use smithay::backend::renderer::{Renderer as _, test::DummyRenderer};

    let mut desk = Desk::new();
    desk.state.textures = Some(crate::remains::Textures::Dummy(DummyRenderer.context_id()));
    let opened = desk.open_surface();
    let compositor = desk.client.compositor.clone().expect("wl_compositor bound");
    let subcompositor = desk
        .client
        .subcompositor
        .clone()
        .expect("wl_subcompositor bound");
    let child = compositor.create_surface(&desk.qh, ());
    let sub = subcompositor.get_subsurface(&child, &opened.surface, &desk.qh, ());
    sub.set_position(8, 8);
    sub.set_desync();
    commit_buffer(&desk.client, &desk.qh, &child, 16, 16);
    opened.surface.commit();
    desk.pump();

    // What a frame does: import every surface of the tree.
    let root = desk
        .state
        .panes
        .get(opened.pane)
        .and_then(Pane::client)
        .and_then(|window| window.wl_surface().map(std::borrow::Cow::into_owned))
        .expect("the window's surface");
    let drawn: Vec<WaylandSurfaceRenderElement<DummyRenderer>> = render_elements_from_surface_tree(
        &mut DummyRenderer,
        &root,
        (0, 0),
        1.0,
        1.0,
        smithay::backend::renderer::element::Kind::Unspecified,
    );
    assert_eq!(drawn.len(), 2, "the premise: two surfaces drawn");

    opened.toplevel.destroy();
    opened.xdg.destroy();
    sub.destroy();
    child.destroy();
    opened.surface.destroy();
    desk.pump();

    assert!(
        smithay::backend::renderer::utils::with_renderer_surface_state(&root, |state| {
            state.buffer().is_none()
        })
        .unwrap_or(true),
        "the premise: smithay has let go of the surface's state"
    );
    let crate::pane::Remains::Picture(picture) = &left_of(&desk.state, opened.pane).remains else {
        panic!("a window with a picture fades out from it");
    };
    assert_eq!(
        picture.dummy_sizes(),
        vec![(16, 16), (64, 64)],
        "both surfaces, topmost first, from the textures imported before \
         the client went"
    );
}

/// **#126: the same for a client that disconnects**, which is heard
/// of at its surface's destruction rather than its toplevel's --
/// before smithay unlinks the surface from its tree or runs the
/// hook that drops what the renderer imported for it.
#[test]
fn a_client_that_disconnects_leaves_the_picture_the_renderer_imported() {
    use smithay::backend::renderer::element::surface::{
        WaylandSurfaceRenderElement, render_elements_from_surface_tree,
    };
    use smithay::backend::renderer::{Renderer as _, test::DummyRenderer};

    let mut desk = Desk::new();
    desk.state.textures = Some(crate::remains::Textures::Dummy(DummyRenderer.context_id()));
    let (conn, mut queue, mut client) = another_client(&mut desk);
    let qh = queue.handle();
    let (window, toplevel, surface) =
        open_surface(&mut desk.display, &mut desk.state, &conn, &client, &qh);
    desk.state.sync_panes();
    let pane = desk.state.panes.id_of(&window).expect("a pane");
    let compositor = client.compositor.clone().expect("wl_compositor bound");
    let subcompositor = client
        .subcompositor
        .clone()
        .expect("wl_subcompositor bound");
    let child = compositor.create_surface(&qh, ());
    let sub = subcompositor.get_subsurface(&child, &surface, &qh, ());
    sub.set_desync();
    commit_buffer(&client, &qh, &child, 16, 16);
    surface.commit();
    pump(
        &mut desk.display,
        &mut desk.state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    let root = window
        .wl_surface()
        .map(std::borrow::Cow::into_owned)
        .expect("the window's surface");
    let drawn: Vec<WaylandSurfaceRenderElement<DummyRenderer>> = render_elements_from_surface_tree(
        &mut DummyRenderer,
        &root,
        (0, 0),
        1.0,
        1.0,
        smithay::backend::renderer::element::Kind::Unspecified,
    );
    assert_eq!(drawn.len(), 2, "the premise: two surfaces drawn");

    drop((sub, child, surface, toplevel, qh, queue, client, conn));
    desk.display
        .dispatch_clients(&mut desk.state)
        .expect("dispatching the disconnect");

    let crate::pane::Remains::Picture(picture) = &left_of(&desk.state, pane).remains else {
        panic!("a window with a picture fades out from it");
    };
    assert_eq!(picture.dummy_sizes(), vec![(16, 16), (64, 64)]);
}

/// **#126's review: a client that disconnects keeps a subsurface
/// whose `wl_surface` is older than its window's.** Its objects go
/// in id order, and ids are recycled, so a video's or a page's
/// surface can be destroyed -- and unlinked, and its pixels dropped
/// -- before the window's own is. The picture is taken at the first
/// surface of the window's tree to go, while all of it is there.
///
/// The subsurface's `wl_surface` is created before the window's, so
/// its id is lower; its `wl_subsurface` after both, so that is
/// destroyed after the window's surface and unlinks nothing first.
/// One whose `wl_subsurface` is the older is
/// `a_client_that_disconnects_keeps_a_subsurface_whose_wl_subsurface_is_older`.
#[test]
fn a_client_that_disconnects_keeps_a_subsurface_older_than_its_window() {
    use smithay::backend::renderer::element::surface::{
        WaylandSurfaceRenderElement, render_elements_from_surface_tree,
    };
    use smithay::backend::renderer::{Renderer as _, test::DummyRenderer};

    let mut desk = Desk::new();
    desk.state.textures = Some(crate::remains::Textures::Dummy(DummyRenderer.context_id()));
    let (conn, mut queue, mut client) = another_client(&mut desk);
    let qh = queue.handle();
    let compositor = client.compositor.clone().expect("wl_compositor bound");
    let child = compositor.create_surface(&qh, ());
    let (window, toplevel, surface) =
        open_surface(&mut desk.display, &mut desk.state, &conn, &client, &qh);
    desk.state.sync_panes();
    let pane = desk.state.panes.id_of(&window).expect("a pane");
    let subcompositor = client
        .subcompositor
        .clone()
        .expect("wl_subcompositor bound");
    let sub = subcompositor.get_subsurface(&child, &surface, &qh, ());
    sub.set_desync();
    commit_buffer(&client, &qh, &child, 16, 16);
    surface.commit();
    pump(
        &mut desk.display,
        &mut desk.state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert!(
        wayland_client::Proxy::id(&child).protocol_id()
            < wayland_client::Proxy::id(&surface).protocol_id(),
        "the premise: the subsurface's surface is the older"
    );
    let root = window
        .wl_surface()
        .map(std::borrow::Cow::into_owned)
        .expect("the window's surface");
    let drawn: Vec<WaylandSurfaceRenderElement<DummyRenderer>> = render_elements_from_surface_tree(
        &mut DummyRenderer,
        &root,
        (0, 0),
        1.0,
        1.0,
        smithay::backend::renderer::element::Kind::Unspecified,
    );
    assert_eq!(drawn.len(), 2, "the premise: two surfaces drawn");

    drop((sub, child, surface, toplevel, qh, queue, client, conn));
    desk.display
        .dispatch_clients(&mut desk.state)
        .expect("dispatching the disconnect");

    let crate::pane::Remains::Picture(picture) = &left_of(&desk.state, pane).remains else {
        panic!("a window with a picture fades out from it");
    };
    assert_eq!(
        picture.dummy_sizes(),
        vec![(16, 16), (64, 64)],
        "the subsurface that went first is not in the picture"
    );
}

/// **#126's second review: and one whose `wl_subsurface` is older
/// than its window's surface, and than its own.** That object is
/// destroyed first, and smithay's destructor for it unlinks the
/// subsurface from its window and resets where it was, telling
/// the compositor nothing -- so by the time any surface of the
/// window was heard going, the page or the video was no longer in
/// it. The window is now taken at that destructor, for a client
/// that has gone, while all of it is there.
///
/// The `wl_subsurface` is given an older id by recycling one: a
/// spare surface made first, before the subsurface's own surface
/// and the window's, and destroyed before the subsurface is made.
#[test]
fn a_client_that_disconnects_keeps_a_subsurface_whose_wl_subsurface_is_older() {
    use smithay::backend::renderer::element::surface::{
        WaylandSurfaceRenderElement, render_elements_from_surface_tree,
    };
    use smithay::backend::renderer::{Renderer as _, test::DummyRenderer};

    let mut desk = Desk::new();
    desk.install(RECORDER);
    desk.state.textures = Some(crate::remains::Textures::Dummy(DummyRenderer.context_id()));
    let (conn, mut queue, mut client) = another_client(&mut desk);
    let qh = queue.handle();
    let compositor = client.compositor.clone().expect("wl_compositor bound");
    let spare = compositor.create_surface(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let (window, toplevel, surface) =
        open_surface(&mut desk.display, &mut desk.state, &conn, &client, &qh);
    desk.state.sync_panes();
    let pane = desk.state.panes.id_of(&window).expect("a pane");
    spare.destroy();
    pump(
        &mut desk.display,
        &mut desk.state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    let subcompositor = client
        .subcompositor
        .clone()
        .expect("wl_subcompositor bound");
    let sub = subcompositor.get_subsurface(&child, &surface, &qh, ());
    sub.set_desync();
    commit_buffer(&client, &qh, &child, 16, 16);
    surface.commit();
    pump(
        &mut desk.display,
        &mut desk.state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    let sub_id = wayland_client::Proxy::id(&sub).protocol_id();
    let child_id = wayland_client::Proxy::id(&child).protocol_id();
    let root_id = wayland_client::Proxy::id(&surface).protocol_id();
    assert!(
        sub_id < child_id && sub_id < root_id,
        "the premise: the wl_subsurface ({sub_id}) is older than the subsurface's \
         surface ({child_id}) and the window's ({root_id})"
    );
    let root = window
        .wl_surface()
        .map(std::borrow::Cow::into_owned)
        .expect("the window's surface");
    let drawn: Vec<WaylandSurfaceRenderElement<DummyRenderer>> = render_elements_from_surface_tree(
        &mut DummyRenderer,
        &root,
        (0, 0),
        1.0,
        1.0,
        smithay::backend::renderer::element::Kind::Unspecified,
    );
    assert_eq!(drawn.len(), 2, "the premise: two surfaces drawn");

    drop((sub, child, surface, toplevel, qh, queue, client, conn));
    desk.display
        .dispatch_clients(&mut desk.state)
        .expect("dispatching the disconnect");

    let crate::pane::Remains::Picture(picture) = &left_of(&desk.state, pane).remains else {
        panic!("a window with a picture fades out from it");
    };
    assert_eq!(
        picture.dummy_sizes(),
        vec![(16, 16), (64, 64)],
        "the subsurface whose wl_subsurface went first is not in the picture"
    );
    let id = pane.get();
    assert_eq!(
        desk.events(),
        format!("open {id},close {id}*"),
        "told gone once, at the wl_subsurface, and not again at its surfaces"
    );
}

/// **#126's review: a client that takes its subsurfaces down
/// before its window, and stays connected, fades from what is left
/// when the window goes.** Pinned rather than fixed: the
/// subsurface is destroyed while the window is still an ordinary
/// one, and keeping it would mean keeping a picture of every
/// subsurface any live window lets go of, on the chance that the
/// window follows. The window's own surface still fades, so what
/// is missing is the part the subsurface covered.
#[test]
fn a_window_whose_subsurface_goes_first_fades_without_it() {
    use smithay::backend::renderer::element::surface::{
        WaylandSurfaceRenderElement, render_elements_from_surface_tree,
    };
    use smithay::backend::renderer::{Renderer as _, test::DummyRenderer};

    let mut desk = Desk::new();
    desk.state.textures = Some(crate::remains::Textures::Dummy(DummyRenderer.context_id()));
    let opened = desk.open_surface();
    let compositor = desk.client.compositor.clone().expect("wl_compositor bound");
    let subcompositor = desk
        .client
        .subcompositor
        .clone()
        .expect("wl_subcompositor bound");
    let child = compositor.create_surface(&desk.qh, ());
    let sub = subcompositor.get_subsurface(&child, &opened.surface, &desk.qh, ());
    sub.set_desync();
    commit_buffer(&desk.client, &desk.qh, &child, 16, 16);
    opened.surface.commit();
    desk.pump();
    let root = desk
        .state
        .panes
        .get(opened.pane)
        .and_then(Pane::client)
        .and_then(|window| window.wl_surface().map(std::borrow::Cow::into_owned))
        .expect("the window's surface");
    drop(render_elements_from_surface_tree::<
        DummyRenderer,
        WaylandSurfaceRenderElement<DummyRenderer>,
    >(
        &mut DummyRenderer,
        &root,
        (0, 0),
        1.0,
        1.0,
        smithay::backend::renderer::element::Kind::Unspecified,
    ));

    sub.destroy();
    child.destroy();
    opened.toplevel.destroy();
    desk.pump();

    let crate::pane::Remains::Picture(picture) = &left_of(&desk.state, opened.pane).remains else {
        panic!("a window with a picture fades out from it");
    };
    assert_eq!(
        picture.dummy_sizes(),
        vec![(64, 64)],
        "the window's own surface, and nothing of the subsurface that went first"
    );
}

/// **#126's second review: a live client that destroys a
/// subsurface's `wl_surface` before its `wl_subsurface` keeps its
/// window.** Legal, and no shipped toolkit is known to do it: the
/// subsurface simply goes inert. `CompositorHandler::destroyed`
/// told a disconnect from this by asking the dying surface for its
/// client, which libwayland answers `None` for every surface being
/// destroyed, alive client or not. So it walked up to the window
/// and ended it: `close` went out, the space let go of a live
/// toplevel, and nothing mapped it again while its application
/// went on running.
///
/// And when that client does go, its window fades out as any
/// window of a client that disconnects does.
#[test]
fn a_live_client_destroying_a_subsurfaces_surface_first_keeps_its_window() {
    let mut desk = Desk::new();
    desk.install(RECORDER);
    let (conn, mut queue, mut client) = another_client(&mut desk);
    let qh = queue.handle();
    let (window, toplevel, surface) =
        open_surface(&mut desk.display, &mut desk.state, &conn, &client, &qh);
    desk.state.sync_panes();
    let pane = desk.state.panes.id_of(&window).expect("a pane");
    let compositor = client.compositor.clone().expect("wl_compositor bound");
    let subcompositor = client
        .subcompositor
        .clone()
        .expect("wl_subcompositor bound");
    let child = compositor.create_surface(&qh, ());
    let sub = subcompositor.get_subsurface(&child, &surface, &qh, ());
    sub.set_desync();
    commit_buffer(&client, &qh, &child, 16, 16);
    surface.commit();
    pump(
        &mut desk.display,
        &mut desk.state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    land(&mut desk);
    let id = pane.get();
    assert_eq!(
        desk.events(),
        format!("open {id}"),
        "the premise: the window opened, and nothing else"
    );

    child.destroy();
    pump(
        &mut desk.display,
        &mut desk.state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    // And the window goes on drawing, as its application does.
    commit_buffer(&client, &qh, &surface, 64, 64);
    pump(
        &mut desk.display,
        &mut desk.state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    land(&mut desk);

    let held = |desk: &Desk| desk.state.panes.get(pane).and_then(Pane::client).cloned();
    assert_eq!(
        held(&desk),
        Some(window.clone()),
        "a live window left its pane"
    );
    assert!(
        desk.state.space.elements().any(|each| *each == window),
        "the space let go of a live window"
    );
    assert_eq!(
        desk.events(),
        format!("open {id}"),
        "scripts were told a live window closed"
    );

    sub.destroy();
    pump(
        &mut desk.display,
        &mut desk.state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    land(&mut desk);
    assert_eq!(
        held(&desk),
        Some(window.clone()),
        "the inert subsurface's own object going took the window with it"
    );

    drop((sub, child, surface, toplevel, qh, queue, client, conn));
    desk.display
        .dispatch_clients(&mut desk.state)
        .expect("dispatching the disconnect");
    assert!(
        desk.state.panes.get(pane).is_some_and(Pane::ghost),
        "when its client goes, the window fades out"
    );
    assert_eq!(
        desk.events(),
        format!("open {id},close {id}*"),
        "and is told gone, once"
    );
}

/// **#126: a window that closed itself keeps the buffer it is drawn
/// from until its fade is over**, and then the client that closed
/// it and stayed is told it may have it back.
///
/// Drawn once through `DummyRenderer` first, so that there is a
/// picture to draw the fade from and so a buffer worth keeping.
#[test]
fn a_window_that_closed_itself_keeps_its_last_buffer_until_its_fade_is_over() {
    use smithay::backend::renderer::element::surface::{
        WaylandSurfaceRenderElement, render_elements_from_surface_tree,
    };
    use smithay::backend::renderer::{Renderer as _, test::DummyRenderer};

    let mut desk = Desk::new();
    desk.state.textures = Some(crate::remains::Textures::Dummy(DummyRenderer.context_id()));
    let opened = desk.open_surface();
    let last = commit_buffer(&desk.client, &desk.qh, &opened.surface, 80, 80);
    desk.pump();
    land(&mut desk);
    desk.pump();
    let root = desk
        .state
        .panes
        .get(opened.pane)
        .and_then(Pane::client)
        .and_then(|window| window.wl_surface().map(std::borrow::Cow::into_owned))
        .expect("the window's surface");
    // Dropped at once, as a frame's elements are: each holds the
    // buffer it was built from, and one kept here would hold it
    // for this test rather than for the fade.
    drop(render_elements_from_surface_tree::<
        DummyRenderer,
        WaylandSurfaceRenderElement<DummyRenderer>,
    >(
        &mut DummyRenderer,
        &root,
        (0, 0),
        1.0,
        1.0,
        smithay::backend::renderer::element::Kind::Unspecified,
    ));
    let last = wayland_client::Proxy::id(&last);
    assert!(
        !desk.client.released.contains(&last),
        "the premise: the buffer on screen is not released"
    );

    opened.toplevel.destroy();
    opened.xdg.destroy();
    opened.surface.destroy();
    desk.pump();
    assert!(
        !desk.client.released.contains(&last),
        "a buffer the fade is drawn from was handed back while it was drawn"
    );

    let since = left_of(&desk.state, opened.pane).since;
    desk.state
        .settle(since + crate::pane::LEAVING + Duration::from_millis(16));
    desk.pump();
    assert!(
        desk.client.released.contains(&last),
        "and handed back once the fade is over"
    );
}

/// **A window that went keeps its app id for its frame**: what its
/// frame is told as `appId` while it fades is what it was told while
/// the window was here, so a bar showing the application's icon fades
/// out with it, as it keeps its title
/// (`a_window_that_left_is_nobodys_to_find`).
#[test]
fn a_window_that_left_keeps_its_app_id_for_its_frame() {
    let mut desk = Desk::new();
    let opened = desk.open_surface();
    opened.toplevel.set_app_id("the-app-that-went".to_owned());
    desk.pump();
    land(&mut desk);
    assert_eq!(
        desk.state.pane_app_id(opened.pane),
        "the-app-that-went",
        "the premise: the frame is told the client's app id"
    );

    opened.toplevel.destroy();
    desk.pump();
    let _ = left_of(&desk.state, opened.pane);
    assert_eq!(
        desk.state.pane_app_id(opened.pane),
        "the-app-that-went",
        "the frame fading out lost its app id"
    );
}

/// **#126: a window whose client has gone is drawn, and is nothing
/// else.**
///
/// Two windows overlapping and no layout, the upper one closing
/// itself. While it fades:
///
/// * it is in no window list a script sees;
/// * the pointer, a press and a resize border under it all reach
///   the window underneath -- a pane with no client that answered
///   "covered" would be the input black hole the loading pane is
///   on purpose, and a titlebar or a border it offered would be a
///   press nothing takes;
/// * the keyboard goes to the window underneath;
/// * `sol.place` does not move it, and `close_pane` does nothing,
///   neither `closing` nor a second `close`.
#[test]
fn a_window_that_left_is_nobodys_to_find() {
    let mut desk = Desk::new();
    let lower = desk.open_surface();
    let upper = desk.open_surface();
    for (opened, at) in [(&lower, (100, 100)), (&upper, (200, 200))] {
        commit_buffer(&desk.client, &desk.qh, &opened.surface, 400, 300);
        desk.pump();
        let window = desk
            .state
            .panes
            .get(opened.pane)
            .and_then(Pane::client)
            .cloned()
            .expect("a window");
        desk.state.map_stacked(window, at, false);
    }
    land(&mut desk);
    let (ghost, below) = (upper.pane.get(), lower.pane.get());
    desk.install(&format!(
        "{RECORDER}\nsol.bind(\"super+x\", function() \
         sol.place({ghost}, {{ x = 0, y = 0, w = 50, h = 50 }}) end)"
    ));
    let lower_window = desk
        .state
        .panes
        .get(lower.pane)
        .and_then(Pane::client)
        .cloned()
        .expect("the lower window");
    let lower_surface = lower_window
        .wl_surface()
        .map(std::borrow::Cow::into_owned)
        .expect("its surface");
    // Inside both, and on the lower window's right-hand border.
    let point: Point<f64, Logical> = (499.0, 300.0).into();
    upper.toplevel.set_title("the window that went".to_owned());
    desk.pump();
    assert!(
        desk.state.looks_focused(upper.pane),
        "the premise: the window going is the focused one"
    );

    upper.toplevel.destroy();
    desk.pump();
    let stood = left_of(&desk.state, upper.pane).outer;
    assert!(
        stood.to_f64().contains(point) && desk.state.drawn(upper.pane, stood).covers(point),
        "the premise: the window that went is drawn over the point"
    );

    assert!(
        desk.state
            .snapshot()
            .windows
            .iter()
            .all(|row| row.id != ghost),
        "a window that went is in a script's window list"
    );
    assert_eq!(
        desk.state.window_under(point).map(|(window, _)| window),
        Some(lower_window),
        "the pointer stopped at a window that went"
    );
    assert_eq!(
        desk.state.surface_under(point).map(|(surface, _)| surface),
        Some(lower_surface),
        "a press went to a window that went"
    );
    assert_eq!(
        desk.state
            .chrome_under(point)
            .map(|under| (under.pane, under.chrome)),
        Some((lower.pane, Chrome::Resize(ResizeEdge::Right))),
        "the border under a window that went is not the lower window's to drag"
    );

    desk.state.sync_panes();
    assert_eq!(
        desk.state
            .focused_window()
            .and_then(|window| desk.state.panes.id_of(&window)),
        Some(lower.pane),
        "the keyboard goes to the window that is still here"
    );
    assert!(
        desk.state.looks_focused(upper.pane)
            && desk.state.pane_title(upper.pane) == "the window that went",
        "and the frame fading out is drawn as it stood: focused, and titled"
    );
    point_at(&mut desk.state, point);
    assert!(
        desk.state.pointer_over(lower.pane) && !desk.state.pointer_over(upper.pane),
        "a frame fading out lights up for the pointer over it, as if a press there \
         would do something"
    );

    let tile = desk.state.panes.get(upper.pane).and_then(Pane::placed);
    assert!(
        desk.state.trigger("super+x"),
        "the premise: the binding ran"
    );
    assert_eq!(
        desk.state.panes.get(upper.pane).and_then(Pane::placed),
        tile,
        "a script placing it moved the tile its picture is cut to"
    );
    assert_eq!(
        left_of(&desk.state, upper.pane).outer,
        stood,
        "a script placing it moved it"
    );
    assert_eq!(
        desk.state.pane_outer_of(upper.pane),
        Some(stood),
        "a script placing it moved where it is drawn"
    );
    desk.state.close_pane(upper.pane);
    assert!(
        desk.state
            .panes
            .get(upper.pane)
            .is_some_and(|pane| pane.closing_at().is_none()),
        "closing it by hand started a close"
    );
    assert_eq!(
        desk.events(),
        format!("close {ghost}*"),
        "and nothing was told anything but `close`, once: {below} stayed"
    );
}

/// **#126: a layout reflows at `close` and the window fades where it
/// stood while its neighbour grows in** -- the picture #128 gives a
/// close the compositor asks for, with no `closing`, because nobody
/// asked for this one. The window is in no arrangement afterwards:
/// an `adopt`, on a `monitors` event, finds nothing to put back.
///
/// It fades in front of every window the layout moved, and
/// everywhere else keeps its place in the stack (#126's review): in
/// scrolling the column on the far side does not move, was over it,
/// and stays over it. See `crate::pane::Left::over`.
#[test]
fn a_window_that_closes_itself_hands_its_space_over_as_it_fades() {
    for layout in LAYOUTS {
        let mut desk = Desk::new();
        let (closed, others) = closing_scene(&mut desk, layout);
        let before: Vec<Rectangle<i32, Logical>> =
            others.iter().map(|other| desk.placed(other.pane)).collect();
        let stood = desk
            .state
            .pane_outer_of(closed.pane)
            .expect("a mapped pane");
        let stacked_over: Vec<bool> = others
            .iter()
            .map(|other| above(&mut desk.state, closed.pane, other.pane))
            .collect();

        closed.toplevel.destroy();
        desk.pump();

        let id = closed.pane.get();
        assert_eq!(
            desk.events()
                .split(',')
                .filter(|event| !event.starts_with("open"))
                .collect::<Vec<_>>(),
            vec![format!("close {id}*")],
            "{layout}: `close`, once, and nothing else"
        );
        assert!(
            others
                .iter()
                .zip(&before)
                .any(|(other, was)| desk.placed(other.pane) != *was),
            "{layout}: the layout closed up at `close`"
        );
        let since = left_of(&desk.state, closed.pane).since;
        let half = desk
            .state
            .drawn_id_at(closed.pane, stood, since + present::CLOSING / 2);
        let (cx, cy) = (
            f64::from(stood.loc.x) + f64::from(stood.size.w) / 2.0,
            f64::from(stood.loc.y) + f64::from(stood.size.h) / 2.0,
        );
        assert!(
            half.shows()
                && (half.rect.loc.x + half.rect.size.w / 2.0 - cx).abs() < 1.0
                && (half.rect.loc.y + half.rect.size.h / 2.0 - cy).abs() < 1.0,
            "{layout}: half way through, it is fading where it stood: {half:?} \
             against {stood:?}"
        );
        let crate::pane::Left { outer, .. } = left_of(&desk.state, closed.pane);
        assert_eq!(*outer, stood, "{layout}: and the layout did not move it");
        for ((other, was), over) in others.iter().zip(&before).zip(&stacked_over) {
            let moved = desk.placed(other.pane) != *was;
            assert_eq!(
                above(&mut desk.state, closed.pane, other.pane),
                moved || *over,
                "{layout}: it fades in front of a window moving in, as a close \
                 the compositor asked for does, and otherwise where it was in the \
                 stack (moved: {moved}, over it before: {over})"
            );
        }

        // `adopt` puts back whatever an arrangement is missing,
        // from `sol.windows()`, and runs on `monitors`.
        let grown: Vec<Rectangle<i32, Logical>> =
            others.iter().map(|other| desk.placed(other.pane)).collect();
        desk.state.trigger_monitors_changed();
        desk.state.space.refresh();
        desk.state.sync_panes();
        assert!(
            desk.state.panes.get(closed.pane).is_some_and(Pane::ghost),
            "{layout}: the premise: it is still fading"
        );
        let now = since + crate::pane::LEAVING + Duration::from_millis(16);
        desk.state.settle(now);
        desk.state.trigger_relayout();
        assert_eq!(
            others
                .iter()
                .map(|other| desk.placed(other.pane))
                .collect::<Vec<_>>(),
            grown,
            "{layout}: adopted and laid out again, the arrangement made room \
             for the window that went"
        );
        assert_eq!(
            desk.events()
                .split(',')
                .filter(|event| !event.starts_with("open"))
                .count(),
            1,
            "{layout}: and it was never told of again"
        );
    }
}

/// **#126: a client that quits part of the way through a close the
/// compositor asked for leaves on that close**: from its transform,
/// with nothing restarted, and gone when that close's fade is over.
/// A client that exits the moment it is asked -- inside `CLOSING`,
/// before the request even goes out, as one closing on a second
/// press of its own shortcut does -- is exactly this.
#[test]
fn a_client_that_quits_during_a_close_leaves_on_that_close() {
    let mut desk = Desk::new();
    desk.install(RECORDER);
    let opened = desk.open_surface();
    let pane = opened.pane;
    land(&mut desk);
    let stood = desk.state.pane_outer_of(pane).expect("a mapped pane");

    desk.state.close_pane(pane);
    let due = desk
        .state
        .panes
        .get(pane)
        .and_then(Pane::closing_at)
        .expect("the premise: a close is playing");
    let began = due - present::CLOSING;
    let fading = desk
        .state
        .drawn_id_at(pane, stood, began + present::CLOSING / 2);

    desk.state.clock.advance(Duration::from_millis(60));
    opened.toplevel.destroy();
    desk.pump();

    let left = left_of(&desk.state, pane);
    assert_eq!(left.since, began, "its fade began at the press");
    assert_eq!(
        desk.state
            .drawn_id_at(pane, stood, began + present::CLOSING / 2),
        fading,
        "and it goes on with that fade rather than starting another"
    );
    let id = pane.get();
    assert_eq!(
        desk.events(),
        format!("open {id},closing {id}*,close {id}*")
    );
    desk.state
        .settle(began + crate::pane::LEAVING + Duration::from_millis(1));
    assert!(
        desk.state.panes.get(pane).is_none(),
        "gone when that close's fade is over"
    );
}

/// **#126: a window that goes keeps the shift its selections had**,
/// however a script rebuilds them after it. `workspaces.lua`
/// rebuilds a desk's membership from `sol.windows()` on every
/// `layout` event, and a window that has gone is not in it -- so a
/// window on a desk carried a screen away would lose its desk's
/// shift on the next one and fade over the desk on screen.
#[test]
fn a_window_that_left_keeps_the_shift_its_desk_had() {
    let mut desk = Desk::new();
    let opened = desk.open_surface();
    let pane = opened.pane;
    let id = pane.get();
    desk.install(&format!(
        "sol.bind(\"super+g\", function() \
            sol.group(\"desk\", {{ windows = {{ {id} }} }}) \
            sol.present_group(\"desk\", {{ x = -500 }}, {{ duration = 1 }}) end)\n\
         sol.bind(\"super+h\", function() \
            sol.group(\"desk\", {{ windows = {{}} }}) end)"
    ));
    assert!(desk.state.trigger("super+g"), "the premise: the desk moved");
    land(&mut desk);
    let stood = desk.state.pane_outer_of(pane).expect("a mapped pane");
    let carried = desk.state.drawn(pane, stood).rect;
    assert!(
        (carried.loc.x - f64::from(stood.loc.x - 500)).abs() < 1.0,
        "the premise: the desk carries it 500 to the left: {carried:?}"
    );

    opened.toplevel.destroy();
    desk.pump();
    assert!(
        desk.state.trigger("super+h"),
        "the premise: the desk was rebuilt"
    );
    let since = left_of(&desk.state, pane).since;
    let drawn = desk.state.drawn_id_at(pane, stood, since).rect;
    assert!(
        (drawn.loc.x - carried.loc.x).abs() < 1.0,
        "a desk rebuilt without the window that went moved it back: \
         {drawn:?}, where the desk had it at {carried:?}"
    );
}

/// **#126's review: a window that goes moves with its desk for as
/// long as it fades**, rather than stopping where the desk had it
/// when it went. The desk is rebuilt without it and carried back,
/// the way `workspaces.lua` switches desks: a window closed with
/// `Ctrl+D` just before a switch used to stay where it was and fade
/// over the desk sliding in, and one that went during a slide
/// stopped half way while the rest of its desk carried on.
///
/// Measured against a window staying on the same desk, whose only
/// transform is the desk's, and at the centre, which a close
/// shrinks about.
#[test]
fn a_window_that_left_moves_with_its_desk() {
    let mut desk = Desk::new();
    let staying = desk.open_surface();
    let going = desk.open_surface();
    let (stays, goes) = (staying.pane.get(), going.pane.get());
    desk.install(&format!(
        "sol.bind(\"super+g\", function() \
            sol.group(\"desk\", {{ windows = {{ {stays}, {goes} }} }}) \
            sol.present_group(\"desk\", {{ x = -500 }}, {{ duration = 1 }}) end)\n\
         sol.bind(\"super+h\", function() \
            sol.group(\"desk\", {{ windows = {{ {stays} }} }}) \
            sol.present_group(\"desk\", {{ x = 0 }}, {{ duration = 100 }}) end)"
    ));
    assert!(desk.state.trigger("super+g"), "the premise: the desk moved");
    land(&mut desk);
    let stood = desk.state.pane_outer_of(going.pane).expect("a mapped pane");
    let beside = desk
        .state
        .pane_outer_of(staying.pane)
        .expect("a mapped pane");

    going.toplevel.destroy();
    desk.pump();
    assert!(left_of(&desk.state, going.pane).since > Duration::ZERO);
    let slid = desk.state.clock.now();
    assert!(
        desk.state.trigger("super+h"),
        "the premise: the desk went back"
    );
    let centre = |rect: Rectangle<f64, Logical>| rect.loc.x + rect.size.w / 2.0;
    let at = slid + Duration::from_millis(50);
    let carried =
        desk.state.drawn_id_at(staying.pane, beside, at).rect.loc.x - f64::from(beside.loc.x);
    assert!(
        carried < -1.0 && carried > -499.0,
        "the premise: half way through, the desk is on its way back: {carried}"
    );
    let ghost = centre(desk.state.drawn_id_at(going.pane, stood, at).rect) - centre(stood.to_f64());
    assert!(
        (ghost - carried).abs() < 1.0,
        "the window that went is carried {ghost} while its desk is carried {carried}"
    );
}

/// The fills `render::remains_elements` draws for a pane fading out,
/// as premultiplied colours, at the start of its fade.
fn fills(state: &Solium, pane: crate::pane::PaneId) -> Vec<[f32; 4]> {
    let left = left_of(state, pane);
    let frame = state.drawn_id_at(pane, left.outer, left.since);
    crate::render::remains_elements(state, pane, &frame, left.outer.size, 1.0)
        .into_iter()
        .filter_map(|element| match element {
            crate::render::Element::Solid(solid) => Some(solid.color().components()),
            _ => None,
        })
        .collect()
}

/// **#126's review: a window that left no picture is filled with a
/// translucent grey, not an opaque white.**
///
/// Common rather than rare: smithay clears a surface's textures on
/// every new buffer and imports only when it draws, so a client
/// that paints and exits inside a frame leaves nothing. The fill
/// was the shipped `Theme.surface`, white, at full opacity -- a
/// white rectangle flashed over a dark terminal as it went.
#[test]
fn a_window_that_left_no_picture_is_filled_with_a_translucent_grey() {
    let mut desk = Desk::new();
    let opened = desk.open_surface();
    land(&mut desk);
    opened.toplevel.destroy();
    desk.pump();
    assert!(
        matches!(
            left_of(&desk.state, opened.pane).remains,
            crate::pane::Remains::Lost
        ),
        "the premise: no renderer, so nothing was imported"
    );
    let fills = fills(&desk.state, opened.pane);
    let [[r, g, b, a]] = fills.as_slice() else {
        panic!("one fill where the client was: {fills:?}");
    };
    assert!(
        *a <= 0.5 + f32::EPSILON,
        "the fill starts at full opacity, over whatever the client showed: {a}"
    );
    assert!(
        (r - g).abs() < f32::EPSILON
            && (g - b).abs() < f32::EPSILON
            && *r <= a / 2.0 + f32::EPSILON,
        "the fill is a light colour rather than a mid grey: {:?}",
        [r, g, b, a]
    );
}

/// **#126's review: a surface that left no picture is filled where
/// it was**, so what did survive never stands in a hole. Asked of
/// the picture as a whole, one surface with pixels meant no fill at
/// all: a root that went unimported left its subsurface -- a page,
/// a video -- standing in the window's frame round a hole, and a
/// subsurface that went unimported left a hole in the root.
///
/// Each case is one surface committing a last buffer after the last
/// frame that drew it, which is what empties smithay's textures for
/// it: Firefox, closed nested, committed its page subsurface 6 ms
/// before destroying its toplevel. `DummyRenderer` draws the frame
/// before that commit, and imports every surface of the tree.
#[test]
fn a_surface_that_left_no_picture_is_filled_where_it_was() {
    use smithay::backend::renderer::element::surface::{
        WaylandSurfaceRenderElement, render_elements_from_surface_tree,
    };
    use smithay::backend::renderer::{Renderer as _, test::DummyRenderer};

    for late in ["root", "subsurface"] {
        let mut desk = Desk::new();
        desk.state.textures = Some(crate::remains::Textures::Dummy(DummyRenderer.context_id()));
        let opened = desk.open_surface();
        let compositor = desk.client.compositor.clone().expect("wl_compositor bound");
        let subcompositor = desk
            .client
            .subcompositor
            .clone()
            .expect("wl_subcompositor bound");
        let child = compositor.create_surface(&desk.qh, ());
        let sub = subcompositor.get_subsurface(&child, &opened.surface, &desk.qh, ());
        sub.set_position(8, 8);
        sub.set_desync();
        commit_buffer(&desk.client, &desk.qh, &child, 16, 16);
        opened.surface.commit();
        desk.pump();
        let root = desk
            .state
            .panes
            .get(opened.pane)
            .and_then(Pane::client)
            .and_then(|window| window.wl_surface().map(std::borrow::Cow::into_owned))
            .expect("the window's surface");
        drop(render_elements_from_surface_tree::<
            DummyRenderer,
            WaylandSurfaceRenderElement<DummyRenderer>,
        >(
            &mut DummyRenderer,
            &root,
            (0, 0),
            1.0,
            1.0,
            smithay::backend::renderer::element::Kind::Unspecified,
        ));
        let (surface, side, hole) = if late == "root" {
            (
                &opened.surface,
                64,
                Rectangle::new((0, 0).into(), (64, 64).into()),
            )
        } else {
            (&child, 16, Rectangle::new((8, 8).into(), (16, 16).into()))
        };
        commit_buffer(&desk.client, &desk.qh, surface, side, side);
        desk.pump();

        opened.toplevel.destroy();
        desk.pump();
        let crate::pane::Remains::Picture(picture) = &left_of(&desk.state, opened.pane).remains
        else {
            panic!("{late}: a window with a picture fades out from it");
        };
        assert_eq!(
            picture.dummy_sizes().len(),
            1,
            "{late}: the premise: one surface has pixels and the other has none"
        );
        let fills: Vec<Rectangle<i32, smithay::utils::Physical>> = picture
            .elements((0, 0).into(), 1.0, 1.0)
            .filter(crate::remains::Surface::filled)
            .map(|surface| {
                smithay::backend::renderer::element::Element::geometry(
                    &surface,
                    smithay::utils::Scale::from(1.0),
                )
            })
            .collect();
        assert_eq!(
            fills,
            vec![hole],
            "{late}: the surface that left no pixels is a hole in what fades"
        );
    }
}

/// **#126's review: a window whose client keeps its surface and
/// gives it a new buffer stops fading there and then.** smithay's
/// GLES renderer uploads a surface's next shared-memory buffer of
/// the same size into the texture it already has for that surface
/// -- the one the fade is drawn from -- so a client hiding a window
/// and showing it again inside the fade by mapping a new role on
/// the same `wl_surface` would have had the fade show the new
/// window's pixels. No client was seen to; see
/// `crate::remains::Picture::holds`. That upload needs a GPU and is
/// read, not driven; what is pinned here is that the fade has ended
/// by the time anything could draw the new buffer.
#[test]
fn a_window_that_went_ends_its_fade_when_its_surface_is_given_a_new_buffer() {
    use smithay::backend::renderer::element::surface::{
        WaylandSurfaceRenderElement, render_elements_from_surface_tree,
    };
    use smithay::backend::renderer::{Renderer as _, test::DummyRenderer};

    let mut desk = Desk::new();
    desk.state.textures = Some(crate::remains::Textures::Dummy(DummyRenderer.context_id()));
    let opened = desk.open_surface();
    let root = desk
        .state
        .panes
        .get(opened.pane)
        .and_then(Pane::client)
        .and_then(|window| window.wl_surface().map(std::borrow::Cow::into_owned))
        .expect("the window's surface");
    drop(render_elements_from_surface_tree::<
        DummyRenderer,
        WaylandSurfaceRenderElement<DummyRenderer>,
    >(
        &mut DummyRenderer,
        &root,
        (0, 0),
        1.0,
        1.0,
        smithay::backend::renderer::element::Kind::Unspecified,
    ));

    opened.toplevel.destroy();
    opened.xdg.destroy();
    desk.pump();
    assert!(
        matches!(
            left_of(&desk.state, opened.pane).remains,
            crate::pane::Remains::Picture(_)
        ),
        "the premise: the window is fading out from its picture"
    );

    // The same surface, a new window: what a hide and a show is.
    let wm_base = desk.client.wm_base.clone().expect("xdg_wm_base bound");
    let xdg = wm_base.get_xdg_surface(&opened.surface, &desk.qh, ());
    let _toplevel = xdg.get_toplevel(&desk.qh, ());
    desk.client.leave_frame(&_toplevel, &desk.qh);
    opened.surface.commit();
    desk.pump();
    assert!(
        desk.state.panes.get(opened.pane).is_some_and(Pane::ghost),
        "a commit with no buffer in it touches no texture, and the fade goes on"
    );
    commit_buffer(&desk.client, &desk.qh, &opened.surface, 64, 64);
    desk.pump();
    assert!(
        desk.state.panes.get(opened.pane).is_none(),
        "the window that went is still fading from the texture its surface's new \
         buffer is about to be uploaded into"
    );
}

/// Three windows of 400x300, mapped overlapping at `(100, 100)`,
/// `(200, 200)` and `(300, 300)`, bottom to top, with no layout.
fn overlapping(desk: &mut Desk) -> [Opened; 3] {
    let opened = [
        desk.open_surface(),
        desk.open_surface(),
        desk.open_surface(),
    ];
    for (each, at) in opened.iter().zip([(100, 100), (200, 200), (300, 300)]) {
        commit_buffer(&desk.client, &desk.qh, &each.surface, 400, 300);
        desk.pump();
        let window = desk
            .state
            .panes
            .get(each.pane)
            .and_then(Pane::client)
            .cloned()
            .expect("a window");
        desk.state.map_stacked(window, at, false);
    }
    land(desk);
    opened
}

/// Every pane, topmost first, after the frame's `sync_panes`.
fn stacked(state: &mut Solium) -> Vec<crate::pane::PaneId> {
    state.space.refresh();
    state.sync_panes();
    state
        .on_screen()
        .into_iter()
        .map(|(pane, _)| pane)
        .collect()
}

/// **#126's review: a window that closes itself behind another one
/// fades behind it**, where it was in the stack, and not in front
/// of the window that was covering it.
///
/// No layout, which is the default, so nothing grows into its
/// space: a terminal behind a browser whose shell exits, a dialog
/// behind its window finishing. #126 put every window that went on
/// top of the stack to fade, so its last picture jumped in front
/// of the window over it and faded there, where before it had
/// vanished unseen.
#[test]
fn a_window_that_closes_itself_behind_another_fades_behind_it() {
    let mut desk = Desk::new();
    let [bottom, middle, top] = overlapping(&mut desk);
    assert_eq!(
        stacked(&mut desk.state),
        vec![top.pane, middle.pane, bottom.pane],
        "the premise: three windows, stacked as they were mapped"
    );

    middle.toplevel.destroy();
    desk.pump();
    assert!(
        desk.state.panes.get(middle.pane).is_some_and(Pane::ghost),
        "the premise: the middle window is fading out"
    );
    for sweep in 0..2 {
        assert_eq!(
            stacked(&mut desk.state),
            vec![top.pane, middle.pane, bottom.pane],
            "sweep {sweep}: the window that went fades where it was in the stack"
        );
    }
}

/// **#126's review, the other half: a window that goes from the
/// top stays over the window the keyboard moves to.** Focusing that
/// window raises it, which is why a window fading out is kept over
/// the panes it was over rather than at a place in the list: kept
/// at a place, it would go behind the first window focused after
/// it, and a terminal closed with `Ctrl+D` over another window
/// would vanish where the two overlap, as it did before #126.
#[test]
fn a_window_that_goes_from_the_top_stays_over_the_window_the_keyboard_moves_to() {
    let mut desk = Desk::new();
    let [bottom, middle, top] = overlapping(&mut desk);
    let window = desk
        .state
        .panes
        .get(top.pane)
        .and_then(Pane::client)
        .cloned()
        .expect("a window");
    desk.state.focus_window(
        &window,
        smithay::utils::SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    // Somewhere the pointer is over the bottom window alone, so the
    // keyboard goes to the one furthest down, and has the most to
    // be raised past.
    point_at(&mut desk.state, (150.0, 150.0).into());

    top.toplevel.destroy();
    desk.pump();
    let order = stacked(&mut desk.state);
    assert_eq!(
        desk.state
            .focused_window()
            .and_then(|window| desk.state.panes.id_of(&window)),
        Some(bottom.pane),
        "the premise: the keyboard went to the window under the pointer"
    );
    assert_eq!(
        stacked(&mut desk.state),
        vec![top.pane, bottom.pane, middle.pane],
        "the window focused is raised, and the window that went stays over it: \
         the sweep before this one had {order:?}"
    );
}

/// **#126: nothing is kept of a window that is not leaving.**
///
/// The picture is taken at the moment a client goes, and never
/// before: a window that stays holds nothing extra, however many
/// frames it draws. `remains::taken` counts pictures taken on this
/// thread, which is the test's own.
#[test]
fn nothing_is_kept_of_a_window_that_stays() {
    use smithay::backend::renderer::{Renderer as _, test::DummyRenderer};

    let mut desk = Desk::new();
    desk.state.textures = Some(crate::remains::Textures::Dummy(DummyRenderer.context_id()));
    let staying = desk.open_surface();
    let going = desk.open_surface();
    let taken = crate::remains::taken();
    for size in 60..90 {
        for opened in [&staying, &going] {
            commit_buffer(&desk.client, &desk.qh, &opened.surface, size, size);
        }
        desk.pump();
        let now = desk.state.clock.now();
        desk.state.settle(now);
        desk.state.space.refresh();
        desk.state.sync_panes();
    }
    assert_eq!(
        crate::remains::taken(),
        taken,
        "a picture was taken of a window nobody closed"
    );
    assert!(desk.state.panes.iter().all(|pane| pane.left().is_none()));

    going.toplevel.destroy();
    desk.pump();
    assert_eq!(
        crate::remains::taken(),
        taken + 1,
        "one, of the window that went"
    );
    assert!(
        desk.state.panes.get(going.pane).is_some_and(Pane::ghost)
            && desk
                .state
                .panes
                .get(staying.pane)
                .is_some_and(|pane| pane.left().is_none()),
        "and only the window that went keeps anything"
    );
}

/// **#126: a window whose application never arrived is told gone
/// while its pane is still here**, as every window is: its row is
/// in `close`'s own snapshot, leaving. It used to be removed first,
/// so `close` was the one event about a window that was not in it.
///
/// A pane with no scene -- building one needs Qt, which a test
/// holding a Wayland client cannot have -- so there is nothing of
/// it on screen to fade, and it goes at once.
#[test]
fn a_window_whose_application_never_came_is_told_gone_while_it_is_here() {
    let mut desk = Desk::new();
    desk.install(RECORDER);
    let area = Rectangle::new((100, 100).into(), (400, 300).into());
    let now = desk.state.clock.now();
    let pane = desk.state.panes.open(Pane::loading(
        "never",
        None,
        area,
        std::path::PathBuf::new(),
        None,
        now,
    ));
    desk.state.trigger_open(pane);
    let patience = desk.state.loading.patience;
    desk.state.settle(now + patience);
    let id = pane.get();
    assert_eq!(desk.events(), format!("open {id},close {id}*"));
    assert!(
        desk.state.panes.get(pane).is_none(),
        "with nothing on screen to fade, it goes at once"
    );
}

mod after_close;

mod keyboard_at_open;

/// **#141 and #142: what is drawn over what, above and below the
/// windows, and whether the one on top is the one the pointer
/// reaches.**
///
/// The picture is asked of [`crate::render::stacked`], which is
/// everything `render::elements` draws below the drag icon, in its
/// order -- `elements` needs a GPU and only turns each entry into
/// elements. The pointer is asked of the hit tests themselves.
/// The workspaces for a shell that is its own program (#60):
/// `ext-workspace-v1` against a real client binding it as a bar does.
mod ext_workspace {
    use super::*;
    use ext_workspace_handle_v1::State;

    /// The desk's monitor, by name.
    fn monitor(desk: &Desk) -> String {
        desk.state
            .space
            .outputs()
            .next()
            .map(|output| output.name())
            .expect("the desk's monitor")
    }

    /// A script declaring one group on the desk's monitor, showing
    /// `showing`, with workspaces "1" and "2", and recording every
    /// `surface` action it hears in `heard`.
    fn declare(desk: &mut Desk, showing: &str) {
        let monitor = monitor(desk);
        desk.install(&format!(
            r#"heard = ""
sol.on("surface", function(surface, action, data)
heard = heard .. surface .. " " .. action .. " " .. tostring(data.id) .. " " .. tostring(data.monitor) .. ";"
end)
sol.workspaces({{ groups = {{ {{ id = "g", monitors = {{ "{monitor}" }}, showing = {{ "{showing}" }},
workspaces = {{ {{ id = "1", name = "one", col = 1, row = 1 }}, {{ id = "2", name = "two", col = 2, row = 1 }} }} }} }} }})"#
        ));
        // What the script declared as it loaded, carried out.
        let outcome = desk
            .state
            .scripts
            .as_mut()
            .map(crate::script::Scripts::startup)
            .unwrap_or_default();
        desk.state.apply(outcome);
        publish(desk);
    }

    fn publish(desk: &mut Desk) {
        desk.pump();
        desk.state.publish_ext_workspaces();
        desk.pump();
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

    fn told(desk: &Desk) -> Vec<WorkspaceTold> {
        desk.client
            .workspaces_told
            .workspaces
            .iter()
            .map(|(_, told)| told.clone())
            .collect()
    }

    /// **A bar is told each declared group and its workspaces, and
    /// which is shown, and again when that changes.**
    #[test]
    fn a_bar_is_told_each_group_and_workspace_and_which_is_shown() {
        let mut desk = Desk::new();
        let _monitor = advertise_monitor(&mut desk);
        declare(&mut desk, "1");
        desk.client.bind_workspaces(&desk.qh);
        publish(&mut desk);
        let groups = &desk.client.workspaces_told.groups;
        assert_eq!(groups.len(), 1, "one group");
        assert_eq!(
            (groups[0].1, groups[0].2.len(), groups[0].3),
            (1, 2, false),
            "(its monitors, its workspaces, removed)"
        );
        let ids_names: Vec<(String, String, Vec<u32>, u32, u32)> = told(&desk)
            .into_iter()
            .map(|told| {
                (
                    told.id,
                    told.name,
                    told.coordinates,
                    told.state,
                    told.capabilities,
                )
            })
            .collect();
        let activate = ext_workspace_handle_v1::WorkspaceCapabilities::Activate.bits();
        assert_eq!(
            ids_names,
            [
                (
                    "g/1".to_owned(),
                    "one".to_owned(),
                    vec![1, 1],
                    State::Active.bits(),
                    activate
                ),
                ("g/2".to_owned(), "two".to_owned(), vec![2, 1], 0, activate),
            ]
        );
        let done = desk.client.workspaces_told.done;
        assert!(done >= 1, "never done");
        publish(&mut desk);
        assert_eq!(
            desk.client.workspaces_told.done, done,
            "nothing changed, nothing told"
        );
        declare(&mut desk, "2");
        assert_eq!(
            told(&desk)
                .iter()
                .map(|told| told.state)
                .collect::<Vec<_>>(),
            [0, State::Active.bits()],
            "the switch"
        );
        assert_eq!(desk.client.workspaces_told.done, done + 1);
    }

    /// **A bar's `activate` reaches Lua at its `commit` as
    /// `workspaces.go` from the surface "ext-workspace"**, with the
    /// workspace's own id and its group's monitor.
    #[test]
    fn activate_reaches_lua_as_workspaces_go_at_commit() {
        let mut desk = Desk::new();
        declare(&mut desk, "1");
        desk.client.bind_workspaces(&desk.qh);
        publish(&mut desk);
        let second = desk.client.workspaces_told.workspaces[1].0.clone();
        second.activate();
        desk.pump();
        let heard = |desk: &Desk| {
            desk.state
                .scripts
                .as_ref()
                .map(|scripts| scripts.evaluate("return heard"))
                .unwrap_or_default()
        };
        assert_eq!(heard(&desk), "", "nothing before the commit");
        desk.client
            .workspaces_told
            .manager
            .clone()
            .expect("bound")
            .commit();
        desk.pump();
        assert_eq!(
            heard(&desk),
            format!("ext-workspace workspaces.go 2 {};", monitor(&desk))
        );
    }

    /// **`workspaces.offered = false` takes the global away**: every
    /// group and workspace removed, the manager finished, and a
    /// client connecting afterwards not offered it.
    #[test]
    fn taking_it_away_finishes_every_manager_and_hides_it() {
        let mut desk = Desk::new();
        declare(&mut desk, "1");
        desk.client.bind_workspaces(&desk.qh);
        publish(&mut desk);
        desk.state.apply(crate::script::Outcome {
            commands: vec![crate::script::Command::WorkspacesOffered(false)],
            ..crate::script::Outcome::default()
        });
        desk.pump();
        let told = &desk.client.workspaces_told;
        assert!(told.finished, "the manager was not finished");
        assert!(told.groups.iter().all(|(.., removed)| *removed));
        assert!(told.workspaces.iter().all(|(_, each)| each.removed));
        let (_conn, _queue, late) = connect(&mut desk.display, &mut desk.state);
        assert_eq!(late.workspace_global, None);
    }
}

mod toplevels;

/// One window as a capture source (#47): a share names a window by
/// its handle on the ext list, and is told the buffer
/// `toplevels.capture` draws it into.
mod window_capture {
    use super::*;
    use crate::toplevels::{Settings, WindowCapture};

    fn publish(desk: &mut Desk) {
        desk.pump();
        desk.state.publish_toplevels();
        desk.pump();
    }

    fn configure(desk: &mut Desk, capture: WindowCapture) {
        desk.state.apply(crate::script::Outcome {
            commands: vec![crate::script::Command::Toplevels(Settings {
                capture,
                ..Settings::default()
            })],
            ..crate::script::Outcome::default()
        });
        publish(desk);
    }

    /// A session on the first window the ext list told of, opened as
    /// a share opens one: the list bound, a source made from the
    /// window's handle, a session on it.
    fn share_first_window(
        desk: &mut Desk,
    ) -> ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1 {
        desk.client.bind_toplevels(&desk.qh);
        publish(desk);
        let registry = desk.client.registry.clone().expect("the registry");
        let name = desk
            .client
            .window_source_global
            .expect("ext_foreign_toplevel_image_capture_source_manager_v1 offered");
        let sources: ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1 =
            registry.bind(name, 1, &desk.qh, ());
        let manager = desk
            .client
            .image_copy
            .clone()
            .expect("ext_image_copy_capture_manager_v1 offered");
        let (handle, _) = desk
            .client
            .ext_told
            .first()
            .cloned()
            .expect("a listed window");
        let source = sources.create_source(&handle, &desk.qh, ());
        let session = manager.create_session(
            &source,
            ext_image_copy_capture_manager_v1::Options::empty(),
            &desk.qh,
            (),
        );
        desk.pump();
        session
    }

    fn announced(size: (i32, i32)) -> [CopyConstraint; 3] {
        #[expect(clippy::cast_sign_loss, reason = "a window's size is positive")]
        let (w, h) = (size.0 as u32, size.1 as u32);
        [
            CopyConstraint::Size(w, h),
            CopyConstraint::Shm(u32::from(wl_shm::Format::Xrgb8888)),
            CopyConstraint::Done,
        ]
    }

    /// **A window is announced at its own size, and again when it
    /// changes**: the client's 64x64, then the 100x80 it commits,
    /// then with `"pane"` the pane's outer rectangle, each told at the
    /// next pass without the share asking, and nothing told when
    /// nothing changed.
    #[test]
    fn a_window_is_announced_at_its_size_and_again_when_it_changes() {
        let mut desk = Desk::new();
        let opened = desk.open_surface();
        let _session = share_first_window(&mut desk);
        assert_eq!(
            desk.client.copy_constraints,
            announced((64, 64)),
            "the client as it drew itself"
        );
        commit_buffer(&desk.client, &desk.qh, &opened.surface, 100, 80);
        publish(&mut desk);
        assert_eq!(
            desk.client.copy_constraints[3..],
            announced((100, 80)),
            "the size it committed"
        );
        configure(&mut desk, WindowCapture::Pane);
        let window = desk
            .state
            .panes
            .get(opened.pane)
            .and_then(Pane::client)
            .cloned()
            .expect("its client");
        let outer = crate::render::flat(&desk.state, &window)
            .expect("its pane")
            .outer;
        assert_eq!(
            desk.client.copy_constraints[6..],
            announced((outer.w, outer.h)),
            "the pane"
        );
        publish(&mut desk);
        assert_eq!(
            desk.client.copy_constraints.len(),
            9,
            "nothing changed, and nothing was announced"
        );
    }

    /// **A frame of a window is queued as that window's**, at its
    /// announced size, for the next pass to draw it alone.
    #[test]
    fn a_frame_of_a_window_is_queued_as_that_windows() {
        let mut desk = Desk::new();
        let opened = desk.open_surface();
        let session = share_first_window(&mut desk);
        let shm = desk.client.shm.clone().expect("wl_shm bound");
        let fd = anon_file(64 * 64 * 4);
        let pool = shm.create_pool(fd.as_fd(), 64 * 64 * 4, &desk.qh, ());
        let buffer = pool.create_buffer(0, 64, 64, 64 * 4, wl_shm::Format::Xrgb8888, &desk.qh, ());
        let frame = session.create_frame(&desk.qh, ());
        frame.attach_buffer(&buffer);
        frame.capture();
        desk.pump();
        let queued: Vec<_> = desk
            .state
            .pending_captures
            .iter()
            .map(|capture| (capture.window, capture.size))
            .collect();
        assert_eq!(
            queued,
            [(
                Some((opened.pane.get(), WindowCapture::Client)),
                (64, 64).into()
            )]
        );
        assert_eq!(desk.client.copy_answers, [], "answered before it was drawn");
    }

    /// **A share of a window stops when the window goes**, and when
    /// `toplevels.capture = false` takes windows away, which also
    /// hides the manager from a client that connects afterwards.
    #[test]
    fn a_share_of_a_window_stops_when_it_goes_or_windows_are_taken_away() {
        let mut desk = Desk::new();
        let opened = desk.open_surface();
        let _kept = desk.open_surface();
        let _session = share_first_window(&mut desk);
        assert_eq!(desk.client.copy_stopped, 0, "the premise: it is running");
        opened.toplevel.destroy();
        opened.xdg.destroy();
        opened.surface.destroy();
        desk.pump();
        desk.state.sync_panes();
        publish(&mut desk);
        assert_eq!(desk.client.copy_stopped, 1, "the window went");

        let (handle, _) = desk.client.ext_told[1].clone();
        let registry = desk.client.registry.clone().expect("the registry");
        let name = desk.client.window_source_global.expect("still offered");
        let sources: ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1 =
            registry.bind(name, 1, &desk.qh, ());
        let manager = desk.client.image_copy.clone().expect("the copy manager");
        let source = sources.create_source(&handle, &desk.qh, ());
        let _second = manager.create_session(
            &source,
            ext_image_copy_capture_manager_v1::Options::empty(),
            &desk.qh,
            (),
        );
        desk.pump();
        configure(&mut desk, WindowCapture::Off);
        assert_eq!(desk.client.copy_stopped, 2, "windows were taken away");
        let (_conn, _queue, late) = connect(&mut desk.display, &mut desk.state);
        assert_eq!(
            late.window_source_global, None,
            "offered with windows taken away"
        );
        assert!(
            late.image_sources.is_some(),
            "monitors are still offered as sources"
        );
    }
}

/// A program that arranges the monitors (#51), against
/// `zwlr-output-management-v1` from a real client.
mod output_management {
    use super::*;

    /// The client binds the manager, as kanshi does, and hears the
    /// heads.
    fn arranger(desk: &mut Desk) -> zwlr_output_manager_v1::ZwlrOutputManagerV1 {
        let registry = desk.client.registry.clone().expect("the registry");
        let name = desk
            .client
            .output_manager_global
            .expect("zwlr_output_manager_v1 offered");
        let manager = registry.bind(name, 4, &desk.qh, ());
        desk.pump();
        manager
    }

    fn serial(desk: &Desk) -> u32 {
        *desk.client.output_dones.last().expect("a done")
    }

    fn head(desk: &Desk) -> zwlr_output_head_v1::ZwlrOutputHeadV1 {
        desk.client.heads_told[0].0.clone()
    }

    /// A configuration of the one monitor at `serial`, on at `at` or
    /// off.
    fn propose(
        desk: &mut Desk,
        manager: &zwlr_output_manager_v1::ZwlrOutputManagerV1,
        serial: u32,
        at: Option<(i32, i32)>,
    ) -> zwlr_output_configuration_v1::ZwlrOutputConfigurationV1 {
        let configuration = manager.create_configuration(serial, &desk.qh, ());
        let head = head(desk);
        match at {
            Some((x, y)) => {
                let on = configuration.enable_head(&head, &desk.qh, ());
                on.set_position(x, y);
            }
            None => configuration.disable_head(&head),
        }
        configuration
    }

    /// **A program is told each monitor, the modes it offers and how
    /// it is driven**, then `done`, and nothing more while nothing
    /// changes.
    #[test]
    fn a_program_is_told_each_monitor_its_modes_and_how_it_is_driven() {
        let mut desk = Desk::new();
        let _manager = arranger(&mut desk);
        let output = desk
            .state
            .space
            .outputs()
            .next()
            .cloned()
            .expect("a monitor");
        let told = &desk.client.heads_told;
        assert_eq!(told.len(), 1, "one monitor, one head");
        let head = &told[0].1;
        assert_eq!(
            (head.name.clone(), head.enabled, head.position, head.scale),
            (output.name(), true, (0, 0), 1.0)
        );
        let mode = output.current_mode().expect("a mode");
        let current = head.current.clone().expect("its mode named");
        let current = desk
            .client
            .modes_told
            .iter()
            .find(|(each, _)| *each == current)
            .map(|(_, told)| *told)
            .expect("one of its modes");
        assert_eq!(
            (current.size, current.refresh),
            ((mode.size.w, mode.size.h), mode.refresh)
        );
        assert_eq!(desk.client.output_dones.len(), 1, "one done");
        desk.state.publish_output_heads();
        desk.pump();
        assert_eq!(desk.client.output_dones.len(), 1, "nothing changed");
    }

    /// **A test is answered at once**: a monitor moved passes, one
    /// switched off with nothing else on fails, and either from an
    /// older picture is cancelled. None of them moves anything.
    #[test]
    fn a_test_is_answered_and_a_stale_or_dark_one_is_not_taken() {
        let mut desk = Desk::new();
        let manager = arranger(&mut desk);
        let now = serial(&desk);
        propose(&mut desk, &manager, now, Some((100, 0))).test();
        propose(&mut desk, &manager, now, None).test();
        propose(&mut desk, &manager, now.wrapping_sub(1), Some((100, 0))).test();
        desk.pump();
        assert_eq!(
            desk.client.configuration_answers,
            ["succeeded", "failed", "cancelled"]
        );
        let output = desk
            .state
            .space
            .outputs()
            .next()
            .cloned()
            .expect("a monitor");
        assert_eq!(output.current_location(), (0, 0).into(), "a test moved it");
    }

    /// **An apply reaches Lua, and the shipped answer arranges the
    /// monitors**: the monitor moves where the program put it, the
    /// program is told it succeeded, and then where it is now.
    /// Without an answer it is told it failed, and nothing moves.
    #[test]
    fn an_apply_is_answered_by_the_configuration_or_fails() {
        let mut desk = Desk::new();
        let manager = arranger(&mut desk);
        let now = serial(&desk);
        propose(&mut desk, &manager, now, Some((300, 50))).apply();
        desk.pump();
        assert_eq!(
            desk.client.configuration_answers,
            ["failed"],
            "with nothing to answer it"
        );

        desk.install(r#"require("output_management")"#);
        let now = serial(&desk);
        propose(&mut desk, &manager, now, Some((300, 50))).apply();
        desk.pump();
        assert_eq!(desk.client.configuration_answers[1..], ["succeeded"]);
        let output = desk
            .state
            .space
            .outputs()
            .next()
            .cloned()
            .expect("a monitor");
        assert_eq!(output.current_location(), (300, 50).into());
        assert_eq!(
            desk.client.heads_told[0].1.position,
            (300, 50),
            "the program was not told where it is now"
        );
    }

    /// **`output_management = false` takes it away**: the manager is
    /// finished, its heads with it, and a client connecting
    /// afterwards is not offered it.
    #[test]
    fn taken_away_the_manager_is_finished_and_hidden() {
        let mut desk = Desk::new();
        let _manager = arranger(&mut desk);
        desk.state.apply(crate::script::Outcome {
            commands: vec![crate::script::Command::OutputManagement(false)],
            ..crate::script::Outcome::default()
        });
        desk.pump();
        assert!(desk.client.output_manager_finished);
        assert!(desk.client.heads_told[0].1.finished, "its head was left");
        let (_conn, _queue, late) = connect(&mut desk.display, &mut desk.state);
        assert_eq!(late.output_manager_global, None);
    }
}

mod framing;

/// What is behind a surface, blurred at its client's asking (#78):
/// `ext-background-effect-v1` against a real client.
mod background_effect {
    use super::*;
    use wayland_client::Proxy as _;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    /// A 300×40 layer surface at the bottom layer, drawn, and its
    /// server side.
    fn panel(
        desk: &mut Desk,
    ) -> (
        wl_surface::WlSurface,
        zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        WlSurface,
    ) {
        use zwlr_layer_surface_v1::Anchor;
        let compositor = desk.client.compositor.clone().expect("wl_compositor bound");
        let shell = desk
            .client
            .layer_shell
            .clone()
            .expect("zwlr_layer_shell_v1 bound");
        let surface = compositor.create_surface(&desk.qh, ());
        let layered = shell.get_layer_surface(
            &surface,
            None,
            zwlr_layer_shell_v1::Layer::Bottom,
            "panel".to_owned(),
            &desk.qh,
            (),
        );
        layered.set_anchor(Anchor::Bottom | Anchor::Left);
        layered.set_size(300, 40);
        surface.commit();
        desk.pump();
        commit_buffer(&desk.client, &desk.qh, &surface, 300, 40);
        desk.pump();
        let ours = desk
            .state
            .space
            .outputs()
            .find_map(|output| {
                smithay::desktop::layer_map_for_output(output)
                    .layers()
                    .find(|layer| {
                        layer.wl_surface().id().protocol_id() == surface.id().protocol_id()
                    })
                    .map(|layer| layer.wl_surface().clone())
            })
            .expect("the panel is mapped");
        (surface, layered, ours)
    }

    fn region(desk: &Desk, rects: &[(i32, i32, i32, i32)]) -> wl_region::WlRegion {
        let compositor = desk.client.compositor.clone().expect("wl_compositor bound");
        let region = compositor.create_region(&desk.qh, ());
        for (x, y, w, h) in rects {
            region.add(*x, *y, *w, *h);
        }
        region
    }

    /// **A client is offered blur, and its region takes effect at
    /// its next commit**: none before it, the region after it, and
    /// none again once it asks for none.
    #[test]
    fn a_region_takes_effect_at_the_next_commit_and_none_takes_it_away() {
        let mut desk = Desk::new();
        desk.pump();
        assert_eq!(
            desk.client.blur_capabilities,
            Some(ext_background_effect_manager_v1::Capability::Blur.bits()),
            "the premise: blur is offered"
        );
        let (surface, _layered, ours) = panel(&mut desk);
        let manager = desk.client.background_effect.clone().expect("bound");
        let effect = manager.get_background_effect(&surface, &desk.qh, ());
        effect.set_blur_region(Some(&region(&desk, &[(0, 0, 300, 40)])));
        desk.pump();
        assert_eq!(
            crate::background_effect::region_of(&ours),
            None,
            "taken before the commit"
        );
        surface.commit();
        desk.pump();
        assert_eq!(
            crate::background_effect::region_of(&ours),
            Some(vec![rect(0, 0, 300, 40)])
        );
        effect.set_blur_region(None);
        surface.commit();
        desk.pump();
        assert_eq!(crate::background_effect::region_of(&ours), None);
    }

    /// **The surface is matched by `blur_region`**, as a layer
    /// surface's fact and a window's in `sol.windows()`.
    #[test]
    fn a_window_that_asked_is_blur_region_in_sol_windows() {
        let mut desk = Desk::new();
        let opened = desk.open_surface();
        let manager = desk.client.background_effect.clone().expect("bound");
        let effect = manager.get_background_effect(&opened.surface, &desk.qh, ());
        effect.set_blur_region(Some(&region(&desk, &[(0, 0, 32, 32)])));
        opened.surface.commit();
        desk.pump();
        let row = desk
            .state
            .snapshot()
            .windows
            .into_iter()
            .find(|window| window.id == opened.pane.get())
            .expect("its row");
        assert!(row.blur_region);
    }

    /// **A backdrop rule on a layer surface is resolved** (#78): the
    /// shipped `client_blur` behind a panel that asked for a blur is
    /// wanted. Resolving it reads which layers are below the panel
    /// (`t2_cut`), and the layer map was held across the walk that
    /// asked, so the compositor waited on its own lock for ever.
    #[test]
    fn a_backdrop_rule_on_a_layer_surface_is_resolved() {
        let mut desk = Desk::new();
        fixture_effects(&mut desk.state);
        let (surface, _layered, ours) = panel(&mut desk);
        let manager = desk.client.background_effect.clone().expect("bound");
        let effect = manager.get_background_effect(&surface, &desk.qh, ());
        effect.set_blur_region(Some(&region(&desk, &[(0, 0, 300, 40)])));
        surface.commit();
        desk.pump();
        desk.state.apply_effects_set(
            Ok(Vec::new()),
            Ok(crate::effect::settings::Settings::default()),
        );
        assert!(
            desk.state.effects.problems().is_empty(),
            "{:?}",
            desk.state.effects.problems()
        );
        let slots = crate::render::build_slots(&mut desk.state);
        assert!(slots.wanted(
            &crate::effect::plan::Owner::LayerShell(ours.id()),
            crate::effect::rules::Slot::Behind
        ));
    }

    /// **With nothing drawing regions the protocol is not offered**:
    /// `effects.client_blur = false` and no rule of the user's
    /// matching `blur_region` or cut by `mask = "region"`.
    #[test]
    fn with_no_rule_drawing_regions_it_is_not_offered() {
        let mut desk = Desk::new();
        let effects = |desk: &mut Desk, client_blur: bool| {
            let settings = crate::effect::settings::Settings {
                client_blur: client_blur.then(|| crate::script::Doc::Text("blur".to_owned())),
                ..crate::effect::settings::Settings::default()
            };
            desk.state.apply(crate::script::Outcome {
                commands: vec![crate::script::Command::Effects {
                    rules: Ok(Vec::new()),
                    settings: Ok(settings),
                }],
                ..crate::script::Outcome::default()
            });
        };
        effects(&mut desk, false);
        let (_conn, _queue, late) = connect(&mut desk.display, &mut desk.state);
        assert!(
            late.background_effect.is_none(),
            "offered with nothing drawing it"
        );
        effects(&mut desk, true);
        let (_conn, _queue, later) = connect(&mut desk.display, &mut desk.state);
        assert!(later.background_effect.is_some(), "not offered again");
    }
}

mod stacking;

mod hosted;

/// **#417: a frame that does more than draw**, against a real client
/// and real QML: what its layers are told of their window, what they
/// send to Lua, the right button and the wheel on the titlebar, and a
/// panel dropped down over the window that holds a grab.
/// A window kept above the others (#424).
mod above {
    use super::*;
    use smithay::backend::input::ButtonState;

    /// Two windows 400 by 300, `a` at (100, 100) and `b` at
    /// (300, 200), overlapping between (300, 200) and (500, 400); `b`
    /// focused last and so on top; a pointer bound; and `super+a`
    /// keeping `a` above, `super+b` toggling it.
    fn two() -> (Desk, Opened, Opened) {
        let mut desk = Desk::new();
        let a = desk.open_surface();
        let b = desk.open_surface();
        for opened in [&a, &b] {
            let _buffer = commit_buffer(&desk.client, &desk.qh, &opened.surface, 400, 300);
        }
        desk.pump();
        for (opened, at) in [(&a, (100, 100)), (&b, (300, 200))] {
            let window = window_of(&desk, opened.pane);
            desk.state.space.map_element(window, at, false);
        }
        desk.state.space.refresh();
        desk.state.profile.focus_follows_mouse = false;
        let id = a.pane.get();
        desk.install(&format!(
            "sol.bind(\"super+a\", function() sol.keep_above({id}) end)\n\
             sol.bind(\"super+b\", function() sol.keep_above({id}, \"toggle\") end)"
        ));
        let _pointer = desk.client.seat_pointer(&desk.qh);
        for opened in [&a, &b] {
            focus(&mut desk, opened.pane);
        }
        // Whatever opening them started, landed.
        desk.state.clock.advance(Duration::from_secs(1));
        let now = desk.state.clock.now();
        desk.state.settle(now);
        desk.state.sync_panes();
        desk.pump();
        (desk, a, b)
    }

    fn window_of(desk: &Desk, pane: crate::pane::PaneId) -> Window {
        desk.state
            .panes
            .get(pane)
            .and_then(Pane::client)
            .cloned()
            .expect("a client")
    }

    /// `pane`'s window given the keyboard, as a click gives it, and
    /// the frame's stack taken.
    fn focus(desk: &mut Desk, pane: crate::pane::PaneId) {
        let window = window_of(desk, pane);
        desk.state.focus_window(
            &window,
            SERIAL_COUNTER.next_serial(),
            crate::script::FocusSource::Script,
        );
        desk.state.sync_panes();
        desk.pump();
    }

    /// Where the two overlap.
    const BOTH: (f64, f64) = (400.0, 300.0);

    /// The pane whose window is under `at`.
    fn under(desk: &Desk, at: (f64, f64)) -> Option<crate::pane::PaneId> {
        desk.state
            .window_under(at.into())
            .and_then(|(window, _)| desk.state.panes.id_of(&window))
    }

    /// Whether `pane`'s row in `sol.windows()` says it is kept above.
    fn row_above(desk: &Desk, pane: crate::pane::PaneId) -> bool {
        desk.state
            .snapshot()
            .windows
            .iter()
            .find(|row| row.id == pane.get())
            .is_some_and(|row| row.above)
    }

    /// A left click at `at`, through the real input path.
    fn click(desk: &mut Desk, at: (f64, f64), time: u64) {
        move_pointer(&mut desk.state, at, time);
        let region = crate::monitor::union(&desk.state.space).expect("a monitor");
        for (state, at) in [
            (ButtonState::Pressed, time + 1),
            (ButtonState::Released, time + 2),
        ] {
            crate::synth::send_button(&mut desk.state, region, 0x110, state, at);
        }
        desk.state.sync_panes();
        desk.pump();
    }

    /// **A kept-above window stays over a window focused after it,
    /// and takes its click** (#424): `sol.keep_above` puts `a` over
    /// `b`, which had been focused last; focusing `b` again raises it
    /// only among the windows not kept above, so `a` is still drawn
    /// and hit where the two overlap, and a click there focuses `a`.
    /// Its row says `above`, and its frame is told `keptAbove`.
    #[test]
    fn a_kept_above_window_stays_over_a_window_focused_after_it_and_takes_its_click() {
        let (mut desk, a, b) = two();
        assert_eq!(under(&desk, BOTH), Some(b.pane), "the premise: b is on top");
        assert!(!row_above(&desk, a.pane));

        assert!(desk.state.trigger("super+a"), "the binding ran");
        desk.state.sync_panes();
        assert_eq!(under(&desk, BOTH), Some(a.pane), "kept above, a is over b");
        assert!(row_above(&desk, a.pane) && !row_above(&desk, b.pane));
        assert!(
            desk.state
                .frame_window(a.pane)
                .is_some_and(|told| told.kept_above),
            "its frame is told keptAbove"
        );

        focus(&mut desk, b.pane);
        assert!(desk.state.is_focused(&window_of(&desk, b.pane)));
        assert_eq!(
            under(&desk, BOTH),
            Some(a.pane),
            "b focused after it is still under it"
        );
        click(&mut desk, BOTH, 10);
        assert!(
            desk.state.is_focused(&window_of(&desk, a.pane)),
            "the click where a covers b is a's"
        );
        click(&mut desk, (650.0, 450.0), 20);
        assert!(
            desk.state.is_focused(&window_of(&desk, b.pane)),
            "a click on b where nothing covers it focuses b"
        );
        assert_eq!(under(&desk, BOTH), Some(a.pane), "and raises it under a");
    }

    /// **Toggled off, a window is stacked as any other** (#424):
    /// `sol.keep_above(id, "toggle")` turns it off, the row says so,
    /// and the next window focused goes over it.
    #[test]
    fn toggled_off_a_window_is_stacked_as_any_other() {
        let (mut desk, a, b) = two();
        assert!(desk.state.trigger("super+b"), "the binding ran");
        desk.state.sync_panes();
        assert!(row_above(&desk, a.pane), "toggled on");
        assert!(desk.state.trigger("super+b"));
        desk.state.sync_panes();
        assert!(!row_above(&desk, a.pane), "and off again");
        assert_eq!(
            under(&desk, BOTH),
            Some(a.pane),
            "off, it stays where it was until something is raised over it"
        );
        focus(&mut desk, b.pane);
        assert_eq!(under(&desk, BOTH), Some(b.pane), "b focused goes over it");
    }

    /// **A focused fullscreen window covers one kept above** (#424),
    /// as in KWin: `b` fullscreen and focused is over `a` everywhere,
    /// and lifted over the bars as a fullscreen window is.
    #[test]
    fn a_focused_fullscreen_window_covers_one_kept_above() {
        let (mut desk, a, b) = two();
        assert!(desk.state.trigger("super+a"));
        desk.state.sync_panes();
        assert_eq!(under(&desk, BOTH), Some(a.pane));
        let window = window_of(&desk, b.pane);
        desk.state.toggle_fullscreen(&window);
        focus(&mut desk, b.pane);
        let _answer = commit_buffer(&desk.client, &desk.qh, &b.surface, 1920, 1080);
        desk.pump();
        desk.state.clock.advance(Duration::from_secs(2));
        let now = desk.state.clock.now();
        desk.state.settle(now);
        desk.state.sync_panes();
        assert_eq!(
            under(&desk, (150.0, 150.0)),
            Some(b.pane),
            "fullscreen and focused, b covers a"
        );
        let screen = Rectangle::new((0, 0).into(), (1920, 1080).into());
        assert_eq!(desk.state.lifted_on(screen), Some(b.pane), "and is lifted");
    }
}

mod frames;

/// **#30: a window minimized, and shown again**, against a real
/// client: the state, what is drawn, what a press and the keyboard
/// reach, the client's own request, focusing, the shipped layouts,
/// `super+m`, the overview and `config.minimize`.
/// What the compositor gives a configuration over a move drag
/// (#452), each on its own and with no maximise in sight: the
/// `drag` event, `sol.drag_hold` and `sol.drag_grip`.
mod drag_primitives {
    use super::*;
    use smithay::backend::input::{ButtonState, KeyState};

    /// Writes down every `drag` and `drop` as `"phase id x,y"`, and
    /// does to the drag what `on_begin` and `on_move` say, `id`,
    /// `x` and `y` in scope.
    fn script(on_begin: &str, on_move: &str) -> String {
        format!(
            r#"
told = {{}}
sol.on("drag", function(id, phase, x, y)
told[#told + 1] = phase .. " " .. id .. " " .. math.floor(x) .. "," .. math.floor(y)
if phase == "begin" then {on_begin} end
if phase == "move" then {on_move} end
end)
sol.on("drop", function(id, x, y)
told[#told + 1] = "drop " .. id .. " " .. math.floor(x) .. "," .. math.floor(y)
end)
"#
        )
    }

    /// A 400 by 300 window at 200,200 with no frame, and `script`.
    fn desk(script: &str) -> (Desk, Window, u64) {
        let mut desk = Desk::new();
        let opened = desk.open_surface();
        commit_buffer(&desk.client, &desk.qh, &opened.surface, 400, 300);
        desk.pump();
        let window = desk
            .state
            .panes
            .get(opened.pane)
            .and_then(Pane::client)
            .cloned()
            .expect("a client");
        desk.state
            .space
            .map_element(window.clone(), (200, 200), false);
        desk.state.space.refresh();
        // Past its opening, so it is under the pointer where it lives.
        desk.state.clock.advance(Duration::from_secs(1));
        let now = desk.state.clock.now();
        desk.state.settle(now);
        desk.install(script);
        (desk, window, opened.pane.get())
    }

    fn told(desk: &Desk) -> String {
        desk.state
            .scripts
            .as_ref()
            .map(|scripts| scripts.evaluate("return table.concat(told, \";\")"))
            .unwrap_or_default()
    }

    fn region(desk: &Desk) -> Rectangle<i32, Logical> {
        crate::monitor::union(&desk.state.space).expect("a monitor")
    }

    /// Super and the left button down at `at`: the modifier's move.
    fn take(desk: &mut Desk, at: (f64, f64)) {
        move_pointer(&mut desk.state, at, 10_000);
        crate::input::key(&mut desk.state, 133_u32.into(), KeyState::Pressed, 11);
        let region = region(desk);
        crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, 12_000);
    }

    fn release(desk: &mut Desk, time: u64) {
        let region = region(desk);
        crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Released, time);
        crate::input::key(&mut desk.state, 133_u32.into(), KeyState::Released, 99);
    }

    fn pass(desk: &mut Desk) {
        crate::input::grab::settle_drags(&mut desk.state, false);
    }

    fn at(desk: &Desk, window: &Window) -> Option<Point<i32, Logical>> {
        desk.state.space.element_location(window)
    }

    /// **`begin` as the drag starts, one `move` a pass wherever the
    /// pointer got to, and `end` before the `drop`**, and none of it
    /// inside the grab: three motions between two passes are one
    /// `move`, and the window follows every one of them.
    #[test]
    fn a_drag_is_told_as_it_begins_moves_once_a_pass_and_ends() {
        let (mut desk, window, id) = desk(&script("", ""));
        take(&mut desk, (250.0, 250.0));
        assert_eq!(
            told(&desk),
            format!("begin {id} 250,250"),
            "told at the press"
        );
        for (step, x) in [(1_u64, 260.0), (2, 270.0), (3, 280.0)] {
            move_pointer(&mut desk.state, (x, 250.0), 12_000 + step * 1_000);
        }
        assert_eq!(
            told(&desk),
            format!("begin {id} 250,250"),
            "a move was told before the pass"
        );
        assert_eq!(
            at(&desk, &window),
            Some((230, 200).into()),
            "the window follows"
        );
        pass(&mut desk);
        pass(&mut desk);
        move_pointer(&mut desk.state, (300.0, 260.0), 20_000);
        release(&mut desk, 21_000);
        assert_eq!(
            told(&desk),
            format!(
                "begin {id} 250,250;move {id} 280,250;move {id} 300,260;\
                 end {id} 300,260;drop {id} 300,260"
            )
        );
        assert!(desk.state.moves.is_empty(), "the drag is over");
    }

    /// **A held window stays where it is while the pointer goes on,
    /// and let go it moves on from where it stands**, with no jump;
    /// a window let go of while it is held is dropped nowhere.
    #[test]
    fn a_held_window_stays_while_the_pointer_moves_and_goes_on_from_where_it_is() {
        let (mut desk, window, id) = desk(&script(
            "sol.drag_hold(id, true)",
            "if x >= 400 then sol.drag_hold(id, false) end",
        ));
        take(&mut desk, (250.0, 250.0));
        move_pointer(&mut desk.state, (350.0, 300.0), 13_000);
        pass(&mut desk);
        assert_eq!(
            at(&desk, &window),
            Some((200, 200).into()),
            "held, it moved"
        );
        move_pointer(&mut desk.state, (420.0, 300.0), 14_000);
        pass(&mut desk);
        assert_eq!(
            at(&desk, &window),
            Some((200, 200).into()),
            "let go, it jumped to the pointer"
        );
        move_pointer(&mut desk.state, (450.0, 320.0), 15_000);
        assert_eq!(
            at(&desk, &window),
            Some((230, 220).into()),
            "let go, it did not move on with the pointer"
        );
        release(&mut desk, 16_000);
        assert!(told(&desk).ends_with(&format!("drop {id} 450,320")));

        // Held to the end: no drop.
        let (mut desk, window, id) = desk_held();
        take(&mut desk, (250.0, 250.0));
        move_pointer(&mut desk.state, (300.0, 250.0), 13_000);
        release(&mut desk, 14_000);
        assert_eq!(at(&desk, &window), Some((200, 200).into()));
        assert_eq!(
            told(&desk),
            format!("begin {id} 250,250;move {id} 300,250;end {id} 300,250"),
            "a window held to the end was dropped"
        );
    }

    fn desk_held() -> (Desk, Window, u64) {
        desk(&script("sol.drag_hold(id, true)", ""))
    }

    /// **A grip puts the window so the point it names is under the
    /// pointer, and the drag goes on from there**: half along and 10
    /// down puts the 400-wide window 200 left of the pointer and 10
    /// above it; a share past the frame is held inside it.
    #[test]
    fn a_grip_puts_the_window_under_the_pointer_and_the_drag_goes_on_from_it() {
        for (grip, gripped) in [
            ("{ x = 0.5, dy = 10 }", (500 - 200, 400 - 10)),
            ("{ x = 2, y = 2 }", (500 - 400, 400 - 299)),
        ] {
            let (mut desk, window, _) = desk(&script(
                "",
                &format!("if not gripped then gripped = true; sol.drag_grip(id, {grip}) end"),
            ));
            take(&mut desk, (210.0, 210.0));
            move_pointer(&mut desk.state, (500.0, 400.0), 13_000);
            pass(&mut desk);
            assert_eq!(
                at(&desk, &window),
                Some(gripped.into()),
                "{grip}: not under the pointer"
            );
            move_pointer(&mut desk.state, (540.0, 420.0), 14_000);
            assert_eq!(
                at(&desk, &window),
                Some((gripped.0 + 40, gripped.1 + 20).into()),
                "{grip}: the drag did not go on from the grip"
            );
            release(&mut desk, 15_000);
        }

        // Let go within the pass of its last motion, as a quick flick
        // is: the motion is told before the `end`, and the grip still
        // holds the window where it was let go, and drops it there.
        let (mut desk, window, id) = desk(&script(
            "",
            "if not gripped then gripped = true; sol.drag_grip(id, { x = 0.5, dy = 10 }) end",
        ));
        take(&mut desk, (210.0, 210.0));
        move_pointer(&mut desk.state, (500.0, 400.0), 13_000);
        release(&mut desk, 14_000);
        assert_eq!(
            at(&desk, &window),
            Some((300, 390).into()),
            "a flick was not gripped"
        );
        assert!(
            told(&desk).ends_with(&format!(
                "move {id} 500,400;end {id} 500,400;drop {id} 500,400"
            )),
            "{}",
            told(&desk)
        );
        assert!(desk.state.moves.is_empty(), "the drag is over");
    }
}

/// The configuration's hold over an edge or modifier resize (#454):
/// `resizing` told as one begins and ends, and `sol.resize_hold`.
mod resize_primitives {
    use super::*;
    use smithay::backend::input::{ButtonState, KeyState};

    const SUPER: u32 = 133;
    const BTN_RIGHT: u32 = 0x111;

    /// Writes down every `resizing` as `"phase id x,y"` and every
    /// `resize` as `"resize id x,y"`, and does what `on_begin` says
    /// when one begins, `id` in scope.
    fn script(on_begin: &str) -> String {
        format!(
            r#"
told = {{}}
sol.on("resizing", function(id, phase, x, y)
told[#told + 1] = phase .. " " .. id .. " " .. math.floor(x) .. "," .. math.floor(y)
if phase == "begin" then {on_begin} end
end)
sol.on("resize", function(id, x, y)
told[#told + 1] = "resize " .. id .. " " .. math.floor(x) .. "," .. math.floor(y)
end)
"#
        )
    }

    /// A 400 by 300 window at 200,200 with no frame, and `script`.
    fn desk(script: &str) -> (Desk, Window, crate::pane::PaneId, u64) {
        let mut desk = Desk::new();
        let opened = desk.open_surface();
        commit_buffer(&desk.client, &desk.qh, &opened.surface, 400, 300);
        desk.pump();
        let window = desk
            .state
            .panes
            .get(opened.pane)
            .and_then(Pane::client)
            .cloned()
            .expect("a client");
        desk.state
            .space
            .map_element(window.clone(), (200, 200), false);
        desk.state.space.refresh();
        desk.state.clock.advance(Duration::from_secs(1));
        let now = desk.state.clock.now();
        desk.state.settle(now);
        desk.install(script);
        (desk, window, opened.pane, opened.pane.get())
    }

    fn desk_held() -> (Desk, Window, crate::pane::PaneId, u64) {
        desk(&script("sol.resize_hold(id, true)"))
    }

    fn told(desk: &Desk) -> String {
        desk.state
            .scripts
            .as_ref()
            .map(|scripts| scripts.evaluate("return table.concat(told, \";\")"))
            .unwrap_or_default()
    }

    fn region(desk: &Desk) -> Rectangle<i32, Logical> {
        crate::monitor::union(&desk.state.space).expect("a monitor")
    }

    /// Super and the right button down at `at`: the modifier's
    /// resize, by the corner of the quarter `at` is in.
    fn take(desk: &mut Desk, at: (f64, f64)) {
        move_pointer(&mut desk.state, at, 10_000);
        crate::input::key(&mut desk.state, SUPER.into(), KeyState::Pressed, 11);
        let region = region(desk);
        crate::synth::send_button(
            &mut desk.state,
            region,
            BTN_RIGHT,
            ButtonState::Pressed,
            12_000,
        );
    }

    fn release(desk: &mut Desk, time: u64) {
        let region = region(desk);
        crate::synth::send_button(
            &mut desk.state,
            region,
            BTN_RIGHT,
            ButtonState::Released,
            time,
        );
        crate::input::key(&mut desk.state, SUPER.into(), KeyState::Released, 99);
    }

    /// A pass: what the drag asked for is offered and done.
    fn pass(desk: &mut Desk) {
        crate::input::grab::settle_drags(&mut desk.state, false);
        desk.state.settle_resize();
    }

    /// The size the window's client was last asked for: what a resize
    /// changes at once, the client drawing it when it can.
    fn asked(window: &Window) -> Option<(i32, i32)> {
        window
            .toplevel()
            .and_then(|toplevel| toplevel.with_pending_state(|state| state.size))
            .map(|size| (size.w, size.h))
    }

    /// **`begin` as the resize is taken, before its first motion is
    /// offered to anyone, and `end` after the release and after the
    /// last motion is offered**, none of it inside the grab: a motion
    /// let go of within its pass is offered, and then the resize ends.
    #[test]
    fn a_resize_is_told_as_it_begins_and_after_the_last_of_it_ends() {
        let (mut desk, window, _, id) = desk(&script(""));
        take(&mut desk, (550.0, 450.0));
        assert_eq!(
            told(&desk),
            format!("begin {id} 550,450"),
            "not told at the press"
        );
        move_pointer(&mut desk.state, (570.0, 460.0), 13_000);
        assert_eq!(
            told(&desk),
            format!("begin {id} 550,450"),
            "a motion was told before its pass"
        );
        pass(&mut desk);
        assert_eq!(asked(&window), Some((420, 310)));
        move_pointer(&mut desk.state, (590.0, 470.0), 14_000);
        release(&mut desk, 15_000);
        assert_eq!(
            told(&desk),
            format!("begin {id} 550,450;resize {id} 620,510"),
            "the end was told before the last motion was offered"
        );
        pass(&mut desk);
        assert_eq!(
            told(&desk),
            format!(
                "begin {id} 550,450;resize {id} 620,510;resize {id} 640,520;\
                 end {id} 590,470"
            )
        );
        assert_eq!(asked(&window), Some((440, 320)));
        assert!(desk.state.resizes.is_empty(), "the resize is not over");
    }

    /// **A held resize leaves the window as it is and asks the
    /// layouts nothing, and let go it goes on from where the pointer
    /// is then**, so nothing jumps; one held to the release ends
    /// where it began.
    #[test]
    fn a_held_resize_keeps_the_window_and_goes_on_from_where_the_pointer_is() {
        let (mut desk, window, _, id) = desk_held();
        let before = asked(&window);
        take(&mut desk, (550.0, 450.0));
        move_pointer(&mut desk.state, (600.0, 500.0), 13_000);
        pass(&mut desk);
        assert_eq!(asked(&window), before, "held, it grew");
        assert_eq!(
            told(&desk),
            format!("begin {id} 550,450"),
            "a held resize was offered to the layouts"
        );
        // Let go, as `sol.resize_hold(id, false)` asks it.
        desk.state.apply(crate::script::Outcome {
            commands: vec![crate::script::Command::ResizeHold { id, held: false }],
            ..Default::default()
        });
        pass(&mut desk);
        assert_eq!(asked(&window), before, "let go, it jumped to the pointer");
        move_pointer(&mut desk.state, (630.0, 520.0), 14_000);
        pass(&mut desk);
        assert_eq!(
            asked(&window),
            Some((430, 320)),
            "let go, it did not go on from where the pointer was"
        );
        release(&mut desk, 15_000);
        pass(&mut desk);
        assert!(
            told(&desk).ends_with(&format!("end {id} 630,520")),
            "{}",
            told(&desk)
        );
        assert!(desk.state.resizes.is_empty());

        // Held to the release: as it was, and over.
        let (mut desk, window_held, _, id) = desk_held();
        let before = asked(&window_held);
        take(&mut desk, (550.0, 450.0));
        move_pointer(&mut desk.state, (650.0, 520.0), 13_000);
        release(&mut desk, 14_000);
        pass(&mut desk);
        assert_eq!(
            asked(&window_held),
            before,
            "held to the end, it was resized"
        );
        assert_eq!(
            desk.state
                .real_geometry(&window_held)
                .map(|geometry| geometry.size),
            Some((400, 300).into()),
            "the client was asked for another size"
        );
        assert_eq!(told(&desk), format!("begin {id} 550,450;end {id} 650,520"));
        assert!(desk.state.resizes.is_empty());
    }
}

/// A maximised window's titlebar (#452): a double click on it
/// still restores it, a click leaves it, and a drag takes it off.
mod drag_off {
    use super::*;
    use smithay::backend::input::ButtonState;
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;

    /// A frame style with a 32-pixel titlebar that a double tap
    /// maximises or restores, as Illia's `rounded-violet` does it.
    fn style() -> std::path::PathBuf {
        let style =
            std::env::temp_dir().join(format!("solium-frame-drag-off-{}", std::process::id()));
        std::fs::create_dir_all(&style).expect("a style folder");
        std::fs::write(
            style.join("Pane.qml"),
            "import Solium\nPaneStyle {\n    insets.top: 32\n    \
             Layer { depth: \"frame\"; name: \"bar\"; source: \"Frame.qml\" }\n}\n",
        )
        .expect("writing the manifest");
        std::fs::write(
            style.join("Frame.qml"),
            "import QtQuick\nItem {\n    id: frame\n    \
             property string action: \"\"\n    \
             TapHandler { onDoubleTapped: frame.action = \"maximize\" }\n}\n",
        )
        .expect("writing the layer");
        style
    }

    fn button(desk: &mut Desk, state: ButtonState, time: u64) {
        let region = crate::monitor::union(&desk.state.space).expect("a monitor");
        crate::synth::send_button(&mut desk.state, region, 0x110, state, time);
    }

    /// The client draws the size it was last told, and a second
    /// goes by.
    fn answer(desk: &mut Desk, opened: &Opened) {
        desk.pump();
        let (w, h) = last_configured(&desk.client, &opened.toplevel)
            .filter(|&(w, h)| w > 0 && h > 0)
            .expect("the window was told a size");
        commit_buffer(&desk.client, &desk.qh, &opened.surface, w, h);
        desk.pump();
        desk.state.clock.advance(Duration::from_secs(2));
        let now = desk.state.clock.now();
        desk.state.settle(now);
    }

    /// **#452, on the titlebar a frame draws**: a double click on a
    /// maximised window's titlebar restores it; a click with a
    /// hand's few pixels of motion in it leaves it maximised where
    /// it is; and a drag past `maximize.restore_after` brings it off
    /// at its old size with the pointer on its titlebar, as far
    /// along it as it was taken, and moves it on with the pointer.
    #[test]
    fn a_maximised_windows_titlebar_takes_a_double_click_a_click_and_a_drag() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            crate::qml::start().expect("Qt starts");
            let style = style();
            let mut desk = Desk::new();
            let opened = desk.open_surface();
            let pane = opened.pane;
            let window = desk
                .state
                .panes
                .get(pane)
                .and_then(Pane::client)
                .cloned()
                .expect("a client");
            desk.install("require(\"floating\")");
            desk.state
                .decorations
                .set_style(&mut desk.state.panes, Some(style.display().to_string()));
            desk.state
                .decorations
                .insert(&mut desk.state.panes, pane, 400, 300);
            commit_buffer(&desk.client, &desk.qh, &opened.surface, 400, 300);
            desk.pump();
            desk.state
                .space
                .map_element(window.clone(), (200, 200), false);
            desk.state.space.refresh();
            desk.state.clock.advance(Duration::from_secs(1));
            let now = desk.state.clock.now();
            desk.state.settle(now);
            let before = desk.state.real_geometry(&window).expect("mapped");
            assert_eq!(
                desk.state.insets_of(pane).top,
                32,
                "the premise: a titlebar"
            );

            desk.state.toggle_maximize(&window);
            desk.pump();
            answer(&mut desk, &opened);
            let maximised = desk.state.pane_outer_of(pane).expect("outer");
            let at = maximised.loc + Point::from((maximised.size.w / 5, 16));
            let at = (f64::from(at.x), f64::from(at.y));
            assert!(
                matches!(
                    desk.state.chrome_under(at.into()),
                    Some(under) if matches!(under.chrome, crate::state::Chrome::Frame)
                ),
                "the premise: the point is on the maximised window's titlebar"
            );

            // A double click.
            move_pointer(&mut desk.state, at, 10_000);
            button(&mut desk, ButtonState::Pressed, 11_000);
            button(&mut desk, ButtonState::Released, 12_000);
            button(&mut desk, ButtonState::Pressed, 13_000);
            button(&mut desk, ButtonState::Released, 14_000);
            assert!(
                !in_state(&window, State::Maximized),
                "the double click did not restore the window"
            );
            answer(&mut desk, &opened);
            assert_eq!(
                desk.state.real_geometry(&window),
                Some(before),
                "the double click did not put it back where it was"
            );

            // A click, with two or three pixels in it.
            desk.state.toggle_maximize(&window);
            desk.pump();
            answer(&mut desk, &opened);
            move_pointer(&mut desk.state, at, 20_000);
            button(&mut desk, ButtonState::Pressed, 21_000);
            move_pointer(&mut desk.state, (at.0 + 3.0, at.1 + 2.0), 22_000);
            crate::input::grab::settle_drags(&mut desk.state, false);
            button(&mut desk, ButtonState::Released, 23_000);
            answer(&mut desk, &opened);
            assert!(
                in_state(&window, State::Maximized),
                "the click restored the window"
            );
            assert_eq!(
                desk.state.pane_outer_of(pane),
                Some(maximised),
                "the click moved the window"
            );

            // A drag.
            move_pointer(&mut desk.state, at, 30_000);
            button(&mut desk, ButtonState::Pressed, 31_000);
            move_pointer(&mut desk.state, (at.0 + 2.0, at.1), 32_000);
            crate::input::grab::settle_drags(&mut desk.state, false);
            assert!(
                in_state(&window, State::Maximized),
                "the first two pixels restored it"
            );
            let to = (at.0 + 50.0, at.1 + 80.0);
            move_pointer(&mut desk.state, to, 33_000);
            crate::input::grab::settle_drags(&mut desk.state, false);
            assert!(
                !in_state(&window, State::Maximized),
                "the drag did not restore the window"
            );
            answer(&mut desk, &opened);
            let off = desk.state.pane_outer_of(pane).expect("outer");
            assert_eq!(
                (off.size.w, off.size.h),
                (before.size.w, before.size.h + 32),
                "it did not come off at the size it had"
            );
            assert_eq!(
                (f64::from(off.loc.x), f64::from(off.loc.y)),
                ((to.0 - f64::from(off.size.w) / 5.0).round(), to.1 - 16.0),
                "it did not come off a fifth of the way along under the \
                 pointer, 16 down its titlebar"
            );
            assert!(
                matches!(
                    desk.state.chrome_under(to.into()),
                    Some(under) if matches!(under.chrome, crate::state::Chrome::Frame)
                ),
                "the pointer is not on its titlebar"
            );
            move_pointer(&mut desk.state, (to.0 + 100.0, to.1 + 50.0), 34_000);
            crate::input::grab::settle_drags(&mut desk.state, false);
            button(&mut desk, ButtonState::Released, 35_000);
            answer(&mut desk, &opened);
            assert_eq!(
                desk.state.pane_outer_of(pane),
                Some(Rectangle::new(off.loc + Point::from((100, 50)), off.size)),
                "it did not follow the pointer once off"
            );
            assert!(!in_state(&window, State::Maximized), "maximised again");
            drop(desk);
            let _ = std::fs::remove_dir_all(&style);
        });
    }
}

mod minimize;
