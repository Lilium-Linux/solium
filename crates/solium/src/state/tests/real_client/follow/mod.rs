//! **#220: the pointer follows the keyboard**, against a real client and
//! the real input path: a key that moves focus takes the pointer to the
//! window it focused at once and flies only its picture there, a click or
//! a bar's chip takes it nowhere, and the warp underneath -- refused while
//! something holds the pointer, ended by the hand on the mouse, and asking
//! again where it lands what the pointer is over (#229).
//!
//! Every key is pressed through the keymap with `us,ru` and the Cyrillic
//! group active (#132).

use super::*;
use crate::keymap::tests::{set_us_ru, tap};
use crate::script::{FocusSource, Interrupt, Warp, WarpTo};
use smithay::backend::input::ButtonState;

/// Super, the arrows and `t`, as xkb keycodes: evdev 125, 105, 106
/// and 20, plus eight.
const SUPER: u32 = 133;
const LEFT: u32 = 113;
const RIGHT: u32 = 114;
const T: u32 = 28;

/// The left button, as evdev names it.
const BUTTON: u32 = 0x110;

/// The shipped layouts, the action vocabulary and the pointer's
/// policy, with a stand-in for a bar's window chip: `chip` is
/// `windows.focus` for the window it names, as a shell's chip sends
/// it.
const SHIPPED: &str = "require(\"actions\")\n\
     require(\"modes\")\n\
     require(\"follow\")\n\
     require(\"workspaces\")\n\
     require(\"tiling\")\n\
     require(\"scrolling\")\n\
     require(\"direction\")\n\
     sol.on(\"surface\", function(_, action, data)\n\
         if action == \"chip\" then sol.act(\"windows.focus\", data) end\n\
     end)\n";

/// One client on `screens` monitors side by side, each 1920x1080, the
/// keyboard on `us,ru` with Russian active, focus not following the
/// mouse and a click focusing.
struct Desk {
    display: Display<Solium>,
    state: Solium,
    conn: Connection,
    queue: wayland_client::EventQueue<Client>,
    qh: QueueHandle<Client>,
    client: Client,
    /// The input clock the synthetic events carry, in microseconds.
    time: u64,
}

/// A window [`Desk::open`] opened: the client's surface, its
/// `xdg_surface` for a menu to hang from, and the server's.
struct Opened {
    window: Window,
    pane: crate::pane::PaneId,
    surface: wl_surface::WlSurface,
    xdg: xdg_surface::XdgSurface,
    server: WlSurface,
}

impl Desk {
    fn new(screens: usize) -> Self {
        let mut display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        // See the #99 test: a Qt scene in a process holding a
        // libwayland connection of its own aborts the test binary.
        state
            .decorations
            .set_style(&mut state.panes, Some("none".to_string()));
        if screens == 2 {
            side_by_side(&mut state, "follow-test");
        } else {
            a_screen(&mut state, "follow-test", (0, 0));
        }
        set_us_ru(&mut state, 1);
        state.profile.focus_follows_mouse = false;
        state.profile.click_to_focus = true;
        let (conn, queue, client) = connect(&mut display, &mut state);
        let qh = queue.handle();
        Self {
            display,
            state,
            conn,
            queue,
            qh,
            client,
            time: 1_000_000,
        }
    }

    /// The scripts `body` with the shipped files on `package.path`,
    /// handed to the compositor. A directory per call, for the
    /// reason the layout harness in `script.rs` gives.
    fn install(&mut self, body: &str) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let directory =
            std::env::temp_dir().join(format!("solium-follow-{}-{serial}", std::process::id()));
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
        self.state.start_scripts(Some(
            Scripts::load(&entry).expect("loading the test scripts"),
        ));
    }

    fn open(&mut self) -> Opened {
        let (window, _toplevel, surface, xdg) = open_xdg(
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
        let server = window
            .wl_surface()
            .map(std::borrow::Cow::into_owned)
            .expect("an xdg window has a surface");
        Opened {
            window,
            pane,
            surface,
            xdg,
            server,
        }
    }

    /// The client opens a menu on `parent` that grabs the pointer,
    /// as a context menu opened by a press does: the grab before the
    /// first commit, with the serial of the keyboard's last enter.
    fn menu(&mut self, parent: &Opened) -> xdg_popup::XdgPopup {
        let compositor = self.client.compositor.clone().expect("wl_compositor bound");
        let wm_base = self.client.wm_base.clone().expect("xdg_wm_base bound");
        let seat = self.client.seat.clone().expect("wl_seat bound");
        let surface = compositor.create_surface(&self.qh, ());
        let xdg = wm_base.get_xdg_surface(&surface, &self.qh, ());
        let positioner = wm_base.create_positioner(&self.qh, ());
        positioner.set_size(32, 32);
        positioner.set_anchor_rect(0, 0, 1, 1);
        let popup = xdg.get_popup(Some(&parent.xdg), &positioner, &self.qh, ());
        popup.grab(&seat, self.client.serial);
        surface.commit();
        self.pump();
        popup
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

    /// Keys pressed in order and let go in reverse, through the keymap.
    fn keys(&mut self, codes: &[u32]) {
        tap(&mut self.state, codes);
        self.pump();
    }

    /// Every transform landed, and the flights with them.
    fn rest(&mut self) {
        self.state.clock.advance(Duration::from_secs(2));
        self.state.sync_panes();
        let now = self.state.clock.now();
        self.state.settle(now);
        self.pump();
    }

    fn region(&self) -> Rectangle<i32, Logical> {
        crate::monitor::union(&self.state.space).expect("a monitor")
    }

    /// The mouse moved by `by`, as a real one reports it.
    fn nudge(&mut self, by: (f64, f64)) {
        self.time += 8_000;
        let region = self.region();
        crate::synth::send_motion(&mut self.state, region, Point::from(by), self.time);
    }

    /// The mouse moved to `to`, as a real one reports it.
    fn mouse_to(&mut self, to: Point<f64, Logical>) {
        let by = to - self.drawn();
        self.nudge((by.x, by.y));
    }

    /// The left button, pressed or let go.
    fn button(&mut self, pressed: bool) {
        self.time += 8_000;
        let region = self.region();
        let state = if pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        };
        crate::synth::send_button(&mut self.state, region, BUTTON, state, self.time);
        self.pump();
    }

    /// Where the pointer is.
    fn at(&self) -> Point<f64, Logical> {
        self.state
            .seat
            .get_pointer()
            .map(|pointer| pointer.current_location())
            .unwrap_or_default()
    }

    /// Where it is drawn.
    fn drawn(&self) -> Point<f64, Logical> {
        self.state.drawn_pointer_at(self.state.clock.now())
    }

    /// The surface the pointer is on, as the seat says it.
    fn under(&self) -> Option<WlSurface> {
        self.state
            .seat
            .get_pointer()
            .and_then(|pointer| pointer.current_focus())
    }

    /// The rectangle the layout last asked for this pane.
    fn placed(&self, pane: crate::pane::PaneId) -> Rectangle<i32, Logical> {
        self.state
            .panes
            .get(pane)
            .and_then(Pane::placed)
            .expect("the layout has placed this pane")
    }

    fn focused(&self) -> Option<Window> {
        self.state.focused_window()
    }

    /// Carry out `commands` as a script's dispatch would.
    fn apply(&mut self, commands: Vec<Command>) {
        self.state.apply(Outcome {
            commands,
            ..Outcome::default()
        });
        self.pump();
    }

    /// A warp, asked for by a script, that the mouse carries on from
    /// where the pointer is.
    fn warp(&mut self, to: WarpTo, motion: Option<AnimationSpec>) {
        self.warp_with(to, motion, Interrupt::Pointer);
    }

    /// A warp, asked for by a script, that the mouse carries on from
    /// as `interrupt` says.
    fn warp_with(&mut self, to: WarpTo, motion: Option<AnimationSpec>, interrupt: Interrupt) {
        self.apply(vec![Command::Warp(Warp {
            to,
            motion,
            interrupt,
        })]);
    }

    /// A scene's action, heard by the `surface` listeners and
    /// applied, as `settle_actions` does.
    fn action(&mut self, action: &str, data: &crate::json::Json) {
        let snapshot = self.state.snapshot();
        let mut scripts = self.state.scripts.take().expect("scripts are installed");
        let outcome = scripts.surface_action("bar", action, data, snapshot);
        self.state.scripts = Some(scripts);
        self.state.apply(outcome);
        self.pump();
    }

    /// What a script holds this window as.
    fn id(&self, opened: &Opened) -> u64 {
        opened.pane.get()
    }

    /// The client fills the tile the layout gave it, as a real one
    /// answers its configure. No frame, so the tile is the client.
    fn answer(&mut self, opened: &Opened) {
        let tile = self.placed(opened.pane);
        commit_buffer(
            &self.client,
            &self.qh,
            &opened.surface,
            tile.size.w,
            tile.size.h,
        );
        self.pump();
    }
}

/// A floating window's place: 64 pixels square, the size every
/// client here commits, at `(x, y)`, and there at once unless
/// `millis` says.
fn floated(id: u64, x: f64, y: f64, millis: u64) -> Command {
    Command::Place {
        id,
        rect: Rect {
            x,
            y,
            w: 64.0,
            h: 64.0,
        },
        animation: AnimationSpec {
            duration: Duration::from_millis(millis),
            easing: crate::present::Curve::Linear,
        },
        tile: false,
        inside: None,
        cramped: false,
        moved: false,
    }
}

fn centre(rect: Rectangle<i32, Logical>) -> Point<f64, Logical> {
    (
        f64::from(rect.loc.x) + f64::from(rect.size.w) / 2.0,
        f64::from(rect.loc.y) + f64::from(rect.size.h) / 2.0,
    )
        .into()
}

/// A linear motion of `millis`, so a test can say where the picture
/// is half way.
fn linear(millis: u64) -> Option<AnimationSpec> {
    Some(AnimationSpec {
        duration: Duration::from_millis(millis),
        easing: crate::present::Curve::Linear,
    })
}

fn near(left: Point<f64, Logical>, right: Point<f64, Logical>) -> bool {
    (left.x - right.x).abs() < 1.0 && (left.y - right.y).abs() < 1.0
}

/// Tiling, with three windows, each client filling its tile if
/// `answered`: `(alone, beside, arrow)`, the window alone in its
/// column, the upper of the two in the other, and the arrow key that
/// goes from the second to the first.
fn three_tiled(desk: &mut Desk, answered: bool) -> (Opened, Opened, u32) {
    desk.install(SHIPPED);
    let mut opened: Vec<Opened> = (0..3).map(|_| desk.open()).collect();
    desk.keys(&[SUPER, T]);
    desk.rest();
    if answered {
        for each in &opened {
            desk.answer(each);
        }
        desk.rest();
    }
    opened.sort_by_key(|each| std::cmp::Reverse(desk.placed(each.pane).size.h));
    let alone = opened.remove(0);
    opened.sort_by_key(|each| desk.placed(each.pane).loc.y);
    let beside = opened.remove(0);
    let (a, b) = (desk.placed(alone.pane), desk.placed(beside.pane));
    assert!(
        a.size.h > b.size.h * 3 / 2
            && (a.loc.x + a.size.w <= b.loc.x || b.loc.x + b.size.w <= a.loc.x),
        "the premise: one window alone in a column and two in the other, {a:?} and {b:?}"
    );
    let arrow = if a.loc.x < b.loc.x { LEFT } else { RIGHT };
    (alone, beside, arrow)
}

/// **A key takes the pointer to the window it focuses at once, and
/// flies its picture there**: `super` and an arrow in tiling put the
/// pointer at the centre of the window it focused before anything is
/// drawn,
/// gives that window the pointer, and draws the picture from where
/// it was, part of the way along half way through, and there once
/// it lands, asking for frames until it has. A click on another
/// window, and a bar's chip, focus without taking the pointer
/// anywhere.
#[test]
fn a_key_takes_the_pointer_to_the_window_it_focuses_at_once_and_flies_its_picture() {
    let mut desk = Desk::new(1);
    let (left, right, arrow) = three_tiled(&mut desk, true);
    let was = centre(desk.placed(right.pane));
    desk.mouse_to(was);
    desk.state.focus_window(
        &right.window,
        SERIAL_COUNTER.next_serial(),
        FocusSource::Script,
    );
    assert!(
        desk.state.flight.is_none(),
        "the premise: nothing flies yet"
    );

    let before = desk.state.clock.now();
    desk.keys(&[SUPER, arrow]);
    let to = centre(desk.placed(left.pane));
    assert_eq!(
        (
            desk.focused().as_ref() == Some(&left.window),
            desk.at(),
            desk.under().as_ref() == Some(&left.server),
        ),
        (true, to, true),
        "(the left window has the keyboard, where the pointer is, the left window \
         has the pointer): all of it at once"
    );
    let flight = desk.state.flight.expect("the picture flies");
    assert_eq!(
        (flight.from(), flight.to()),
        (was, to),
        "(from where it was drawn, to where the pointer is)"
    );
    let half = desk
        .state
        .drawn_pointer_at(before + Duration::from_millis(130));
    assert!(
        (half.x - was.x).abs() > 1.0
            && (half.x - to.x).abs() > 1.0
            && (half.x - was.x).signum() != (half.x - to.x).signum(),
        "half way through it is drawn between the two, not at {half:?}: from {was:?} \
         to {to:?}"
    );
    assert_eq!(
        (
            desk.state.drawn_pointer_at(before),
            desk.state.drawn_pointer_at(before + Duration::from_secs(1)),
        ),
        (was, to),
        "(drawn at the start, drawn once it has landed)"
    );
    assert!(
        desk.state.settle_flight(desk.state.clock.now()),
        "while it flies the frames keep coming"
    );
    desk.rest();
    assert_eq!(
        (desk.state.flight.is_none(), desk.drawn()),
        (true, to),
        "(landed and retired, drawn where the pointer is)"
    );

    // A click on the right window, off its centre: the keyboard goes
    // there and the pointer stays where the hand put it.
    let clicked = was + Point::from((40.0, 30.0));
    desk.mouse_to(clicked);
    desk.button(true);
    desk.button(false);
    assert_eq!(
        (
            desk.focused().as_ref() == Some(&right.window),
            desk.at(),
            desk.state.flight.is_none(),
        ),
        (true, clicked, true),
        "a click focused the right window and took the pointer nowhere"
    );

    // A bar's chip for the left window: the same.
    let id = desk.id(&left);
    #[expect(clippy::cast_precision_loss, reason = "a pane id, far below 2^52")]
    let chip = crate::json::Json::Object(std::collections::BTreeMap::from([(
        "id".to_owned(),
        crate::json::Json::Number(id as f64),
    )]));
    desk.action("chip", &chip);
    assert_eq!(
        (
            desk.focused().as_ref() == Some(&left.window),
            desk.at(),
            desk.state.flight.is_none(),
        ),
        (true, clicked, true),
        "a bar's chip focused the left window and took the pointer nowhere"
    );
}

/// **A tiled window is warped to its tile before its client has
/// filled it**: the clients here never answer their tiles, and an
/// arrow key still puts the pointer at the centre of the tile it
/// focused, where the window is going, and not of the corner its old
/// size takes up there.
#[test]
fn a_tiled_window_is_warped_to_its_tile_before_its_client_has_filled_it() {
    let mut desk = Desk::new(1);
    let (left, right, arrow) = three_tiled(&mut desk, false);
    desk.mouse_to(centre(desk.placed(right.pane)));
    desk.state.focus_window(
        &right.window,
        SERIAL_COUNTER.next_serial(),
        FocusSource::Script,
    );
    desk.keys(&[SUPER, arrow]);
    assert_eq!(
        (desk.focused().as_ref() == Some(&left.window), desk.at(),),
        (true, centre(desk.placed(left.pane))),
        "(focused, where the pointer is)"
    );
}

/// **A key takes the pointer across to a window on the other
/// monitor**: two monitors, a tiled window on each, and `super+right`
/// from the left one puts the pointer at the right one's centre, on
/// the right monitor, at once, with its picture flying over the seam.
#[test]
fn a_key_takes_the_pointer_across_to_a_window_on_the_other_monitor() {
    let mut desk = Desk::new(2);
    desk.install(SHIPPED);
    desk.keys(&[SUPER, T]);
    desk.mouse_to((960.0, 540.0).into());
    let left = desk.open();
    desk.mouse_to((2880.0, 540.0).into());
    let right = desk.open();
    desk.rest();
    let (l, r) = (desk.placed(left.pane), desk.placed(right.pane));
    assert!(
        l.loc.x + l.size.w <= 1920 && r.loc.x >= 1920,
        "the premise: a window on each monitor, {l:?} and {r:?}"
    );
    let was = centre(l);
    desk.mouse_to(was);
    desk.state.focus_window(
        &left.window,
        SERIAL_COUNTER.next_serial(),
        FocusSource::Script,
    );

    let before = desk.state.clock.now();
    desk.keys(&[SUPER, RIGHT]);
    let to = centre(r);
    assert_eq!(
        (
            desk.focused().as_ref() == Some(&right.window),
            desk.at(),
            desk.state.active_output().map(|output| output.name()),
        ),
        (true, to, Some(RIGHT_SCREEN.to_owned())),
        "(the right window has the keyboard, the pointer, its monitor) at once"
    );
    let half = desk
        .state
        .drawn_pointer_at(before + Duration::from_millis(80));
    assert!(
        desk.state.flight.is_some() && half.x > was.x + 1.0 && half.x < to.x - 1.0,
        "its picture flies from {was:?} to {to:?}, and is at {half:?}"
    );
}

/// **A warp goes where the window was moved in the same handler**: a
/// handler that warps to a window and then moves it puts the pointer
/// at the centre of where it moved it, though the window is still
/// drawn on its way there.
#[test]
fn a_warp_goes_where_the_window_was_moved_in_the_same_handler() {
    let mut desk = Desk::new(1);
    desk.install("");
    let opened = desk.open();
    let id = desk.id(&opened);
    desk.apply(vec![floated(id, 100.0, 100.0, 0)]);
    desk.rest();
    desk.apply(vec![
        Command::Warp(Warp {
            to: WarpTo::Window { id, at: (0.5, 0.5) },
            motion: linear(200),
            interrupt: Interrupt::Pointer,
        }),
        floated(id, 1000.0, 500.0, 10_000),
    ]);
    assert_eq!(
        desk.at(),
        Point::from((1032.0, 532.0)),
        "the pointer is at the centre of where the window was moved, not of where it was"
    );
}

/// **What the pointer is over is asked again where its picture
/// lands** (#229): a warp to a window still sliding into place finds
/// nothing under the pointer when it is carried out, and once the
/// picture has landed, with the window there, the window has the
/// pointer -- with no motion to tell it.
#[test]
fn where_the_picture_lands_the_pointer_is_given_to_what_is_there() {
    let mut desk = Desk::new(1);
    desk.install("");
    let opened = desk.open();
    let id = desk.id(&opened);
    desk.apply(vec![floated(id, 100.0, 100.0, 0)]);
    desk.rest();
    desk.mouse_to((1700.0, 900.0).into());
    assert!(
        desk.under().is_none(),
        "the premise: the pointer is over nothing"
    );

    desk.apply(vec![
        floated(id, 1200.0, 100.0, 400),
        Command::Warp(Warp {
            to: WarpTo::Window { id, at: (0.5, 0.5) },
            motion: linear(300),
            interrupt: Interrupt::Pointer,
        }),
    ]);
    assert_eq!(
        (desk.at(), desk.under().is_none()),
        (Point::from((1232.0, 132.0)), true),
        "the pointer is where the window is going, and the window is not drawn there yet"
    );
    desk.rest();
    assert!(
        desk.under().as_ref() == Some(&opened.server),
        "the picture landed with the window there, and the window was not given the \
         pointer"
    );
}

/// **Real motion mid-flight wins at once**: the mouse moved while a
/// warp's picture is half way ends the flight, and carries on from
/// where the warp put the pointer -- or, for a warp whose
/// `interrupt` is `"picture"`, from where the picture is drawn --
/// and the picture is drawn where the pointer is.
#[test]
fn real_motion_mid_flight_wins_at_once() {
    let mut desk = Desk::new(1);
    desk.install("");
    let mut carried = Vec::new();
    for interrupt in [Interrupt::Pointer, Interrupt::Picture] {
        desk.mouse_to((100.0, 100.0).into());
        desk.warp_with(
            WarpTo::Point {
                x: 1100.0,
                y: 600.0,
            },
            linear(10_000),
            interrupt,
        );
        desk.state.clock.advance(Duration::from_secs(5));
        let half = desk.drawn();
        assert!(
            near(half, (600.0, 350.0).into()),
            "the premise: half way through it is drawn half way, at {half:?}"
        );
        desk.nudge((10.0, 0.0));
        assert_eq!(desk.drawn(), desk.at(), "{interrupt:?}: drawn where it is");
        carried.push((interrupt, desk.state.flight.is_none(), desk.at()));
    }
    let went_on = |at: Point<f64, Logical>, from: (f64, f64)| {
        near(at, Point::from(from) + Point::from((10.0, 0.0)))
    };
    assert!(
        matches!(
            carried.as_slice(),
            [(_, true, pointer), (_, true, picture)]
                if went_on(*pointer, (1100.0, 600.0)) && went_on(*picture, (600.0, 350.0))
        ),
        "(interrupt, the flight over, where the pointer went on to): {carried:?}"
    );
}

/// **With focus following the mouse, a nudge mid-flight keeps the
/// keyboard where the key put it**: a key in tiling focuses the
/// window alone in its column and takes the pointer to it, and the
/// mouse moved a pixel 30 ms into the picture's flight -- with the
/// picture still over the window it left -- carries on from where
/// the pointer is, so focus following the mouse leaves the keyboard
/// on the window the key chose.
#[test]
fn with_focus_following_the_mouse_a_nudge_mid_flight_keeps_the_keyboard_where_the_key_put_it() {
    let mut desk = Desk::new(1);
    let (left, right, arrow) = three_tiled(&mut desk, true);
    desk.state.profile.focus_follows_mouse = true;
    desk.mouse_to(centre(desk.placed(right.pane)));
    assert!(
        desk.focused().as_ref() == Some(&right.window),
        "the premise: focus followed the mouse onto the window beside"
    );
    desk.keys(&[SUPER, arrow]);
    let to = centre(desk.placed(left.pane));
    desk.state.clock.advance(Duration::from_millis(30));
    let drawn = desk.drawn();
    assert!(
        desk.state
            .window_under(drawn)
            .is_some_and(|(window, _)| window == right.window),
        "the premise: 30 ms in, the picture is still over the window it left, at {drawn:?}"
    );
    desk.nudge((1.0, 0.0));
    assert_eq!(
        (
            desk.focused().as_ref() == Some(&left.window),
            desk.at(),
            desk.state.flight.is_none(),
        ),
        (true, to + Point::from((1.0, 0.0)), true),
        "(the keyboard still where the key put it, the pointer, the flight over)"
    );
}

/// **Real motion after a warp reports only its own speed**: the jump
/// a warp makes is no motion of the mouse's, so the mouse moved ten
/// pixels every 8 ms just after one -- carried out at once, or
/// interrupting a flight from the pointer or from the picture -- is
/// 1250 pixels a second along `x`, as `Solium.cursor.velocity` would
/// say it, and not the distance the warp put behind it.
#[test]
fn real_motion_after_a_warp_reports_only_its_own_speed() {
    let mut desk = Desk::new(1);
    desk.install("");
    let mut speeds = Vec::new();
    for (motion, interrupt) in [
        (None, Interrupt::Pointer),
        (linear(10_000), Interrupt::Pointer),
        (linear(10_000), Interrupt::Picture),
    ] {
        desk.mouse_to((100.0, 100.0).into());
        let _ = desk.state.pointer.velocity();
        desk.warp_with(
            WarpTo::Point {
                x: 1100.0,
                y: 600.0,
            },
            motion,
            interrupt,
        );
        desk.state.clock.advance(Duration::from_millis(50));
        desk.nudge((10.0, 0.0));
        desk.nudge((10.0, 0.0));
        let speed = desk.state.pointer.velocity();
        speeds.push((speed.x.round(), speed.y.round()));
    }
    assert_eq!(
        speeds,
        vec![(1250.0, 0.0), (1250.0, 0.0), (1250.0, 0.0)],
        "(after a warp at once, after one whose flight the mouse ended going on from \
         the pointer, and from the picture)"
    );
}

/// **A press mid-flight lands it**: the press goes where the pointer
/// is, and the picture is there with it.
#[test]
fn a_press_mid_flight_lands_it() {
    let mut desk = Desk::new(1);
    desk.install("");
    desk.mouse_to((100.0, 100.0).into());
    desk.warp(
        WarpTo::Point {
            x: 1100.0,
            y: 600.0,
        },
        linear(10_000),
    );
    desk.state.clock.advance(Duration::from_secs(5));
    desk.button(true);
    assert_eq!(
        (desk.state.flight.is_none(), desk.drawn(), desk.at()),
        (true, (1100.0, 600.0).into(), (1100.0, 600.0).into()),
        "(landed, drawn, where the pointer is)"
    );
    desk.button(false);
}

/// **A click that takes the pointer along lands where it was
/// aimed**: with `focus.cursor.follows.click` on, a press off the
/// centre of another window focuses it, and the pointer stays where
/// the hand put it until the button is up -- so the press, and the
/// client's whole click, go where they were aimed -- and only then
/// goes to the window's centre.
#[test]
fn a_click_that_takes_the_pointer_along_lands_where_it_was_aimed() {
    let mut desk = Desk::new(1);
    desk.install(
        "require(\"follow\")\n\
         require(\"config\").focus.cursor.follows.click = true",
    );
    let first = desk.open();
    let second = desk.open();
    desk.apply(vec![
        floated(desk.id(&first), 100.0, 100.0, 0),
        floated(desk.id(&second), 1000.0, 100.0, 0),
    ]);
    desk.rest();
    desk.state.focus_window(
        &first.window,
        SERIAL_COUNTER.next_serial(),
        FocusSource::Script,
    );
    let aimed: Point<f64, Logical> = (1010.0, 110.0).into();
    desk.mouse_to(aimed);
    desk.button(true);
    let pressed = (
        desk.focused().as_ref() == Some(&second.window),
        desk.at(),
        desk.under().as_ref() == Some(&second.server),
    );
    desk.button(false);
    assert_eq!(
        (pressed, desk.at()),
        ((true, aimed, true), (1032.0, 132.0).into()),
        "((the press focused the second window, where the pointer was, on the second \
         window), and once the button is up, where the pointer is)"
    );
}

/// **A click that opened a menu takes the pointer nowhere later**:
/// with `focus.cursor.follows.click` on, a press on another window
/// focuses it, and the client answers with a menu that grabs the
/// pointer. When the button comes up the menu still holds the
/// pointer, so the warp the press asked for is refused, as any warp
/// is while the pointer is held -- and forgotten: a click on the
/// desktop that dismisses the menu leaves the pointer where that
/// click was, and does not send it to the window's centre then.
#[test]
fn a_click_that_opened_a_menu_takes_the_pointer_nowhere_later() {
    let mut desk = Desk::new(1);
    desk.install(
        "require(\"follow\")\n\
         require(\"config\").focus.cursor.follows.click = true",
    );
    let first = desk.open();
    let second = desk.open();
    desk.apply(vec![
        floated(desk.id(&first), 100.0, 100.0, 0),
        floated(desk.id(&second), 1000.0, 100.0, 0),
    ]);
    desk.rest();
    desk.state.focus_window(
        &first.window,
        SERIAL_COUNTER.next_serial(),
        FocusSource::Script,
    );
    let aimed: Point<f64, Logical> = (1010.0, 110.0).into();
    desk.mouse_to(aimed);
    desk.button(true);
    let _menu = desk.menu(&second);
    desk.button(false);
    let held = desk
        .state
        .seat
        .get_pointer()
        .is_some_and(|pointer| pointer.is_grabbed());
    assert!(
        held && desk.focused().as_ref() == Some(&second.window) && desk.at() == aimed,
        "the premise: the press focused the second window, and its menu holds the \
         pointer where it was aimed: held {held}, at {:?}",
        desk.at()
    );
    let elsewhere: Point<f64, Logical> = (500.0, 700.0).into();
    desk.mouse_to(elsewhere);
    desk.button(true);
    desk.button(false);
    assert_eq!(
        (desk.at(), desk.state.warp_after_press.is_none()),
        (elsewhere, true),
        "(where the pointer is after the click that dismissed the menu, nothing waits)"
    );
}

/// **A second warp mid-flight starts from where the picture is**, not
/// from where the first was going.
#[test]
fn a_second_warp_mid_flight_starts_from_where_the_picture_is() {
    let mut desk = Desk::new(1);
    desk.install("");
    desk.mouse_to((100.0, 100.0).into());
    desk.warp(
        WarpTo::Point {
            x: 1100.0,
            y: 600.0,
        },
        linear(10_000),
    );
    desk.state.clock.advance(Duration::from_secs(5));
    let half = desk.drawn();
    desk.warp(WarpTo::Point { x: 200.0, y: 900.0 }, linear(10_000));
    let flight = desk.state.flight.expect("the second warp flies");
    assert_eq!(
        (near(flight.from(), half), flight.to(), desk.at()),
        (true, (200.0, 900.0).into(), (200.0, 900.0).into()),
        "(from where the first had got to {half:?}, to the second's point, the pointer \
         there): from {:?}",
        flight.from()
    );
}

/// **A warp takes no focus with it**: with focus following the
/// mouse, a warp onto another window gives that window the pointer
/// and leaves the keyboard where it was.
#[test]
fn a_warp_takes_no_focus_with_it() {
    let mut desk = Desk::new(1);
    desk.install("");
    desk.state.profile.focus_follows_mouse = true;
    let first = desk.open();
    let second = desk.open();
    desk.apply(vec![
        floated(desk.id(&first), 100.0, 100.0, 0),
        floated(desk.id(&second), 1000.0, 100.0, 0),
    ]);
    desk.rest();
    desk.mouse_to((132.0, 132.0).into());
    assert!(
        desk.focused().as_ref() == Some(&first.window),
        "the premise: focus follows the mouse onto the first window"
    );
    desk.warp(
        WarpTo::Window {
            id: desk.id(&second),
            at: (0.5, 0.5),
        },
        None,
    );
    assert_eq!(
        (
            desk.at(),
            desk.under().as_ref() == Some(&second.server),
            desk.focused().as_ref() == Some(&first.window),
        ),
        ((1032.0, 132.0).into(), true, true),
        "(the pointer, on the second window, the keyboard still on the first)"
    );
}

/// **A pointer confined mid-flight stays where the client holds
/// it**: a warp onto a window, the window's client confining the
/// pointer to itself while the picture is still on its way, and the
/// mouse moved: the pointer stays in the window rather than going on
/// from the picture, which is outside it.
#[test]
fn a_pointer_confined_mid_flight_stays_where_the_client_holds_it() {
    use zwp_pointer_constraints_v1::Lifetime;
    let mut desk = Desk::new(1);
    desk.install("");
    let opened = desk.open();
    let id = desk.id(&opened);
    desk.apply(vec![floated(id, 600.0, 200.0, 0)]);
    desk.rest();
    desk.mouse_to((100.0, 900.0).into());
    desk.warp_with(
        WarpTo::Window { id, at: (0.5, 0.5) },
        linear(10_000),
        Interrupt::Picture,
    );
    let pointer = desk.client.seat_pointer(&desk.qh);
    let compositor = desk.client.compositor.clone().expect("wl_compositor bound");
    let region = compositor.create_region(&desk.qh, ());
    region.add(0, 0, 64, 64);
    let _confined = desk
        .client
        .pointer_constraints
        .clone()
        .expect("zwp_pointer_constraints_v1 bound")
        .confine_pointer(
            &opened.surface,
            &pointer,
            Some(&region),
            Lifetime::Persistent,
            &desk.qh,
            (),
        );
    desk.pump();
    desk.state.clock.advance(Duration::from_secs(5));
    let drawn = desk.drawn();
    assert!(
        drawn.x < 500.0,
        "the premise: the picture is still on its way, outside the window, at {drawn:?}"
    );
    desk.nudge((1.0, 0.0));
    assert_eq!(
        (desk.at(), desk.state.flight.is_none()),
        ((632.0, 232.0).into(), true),
        "(where the pointer is, the flight over)"
    );
}

/// **A held pointer is not warped**: while a button is held over a
/// window, and while a client has the pointer locked, a warp moves it
/// nowhere; once the button is up it goes.
#[test]
fn a_held_pointer_is_not_warped() {
    use zwp_pointer_constraints_v1::Lifetime;
    let mut desk = Desk::new(1);
    desk.install("");
    let opened = desk.open();
    let id = desk.id(&opened);
    desk.apply(vec![floated(id, 100.0, 100.0, 0)]);
    desk.rest();
    desk.mouse_to((132.0, 132.0).into());

    desk.button(true);
    desk.warp(
        WarpTo::Point {
            x: 1500.0,
            y: 800.0,
        },
        None,
    );
    let held = desk.at();
    desk.button(false);
    desk.warp(
        WarpTo::Point {
            x: 1500.0,
            y: 800.0,
        },
        None,
    );
    let free = desk.at();

    desk.mouse_to((132.0, 132.0).into());
    let pointer = desk.client.seat_pointer(&desk.qh);
    let constraints = desk
        .client
        .pointer_constraints
        .clone()
        .expect("zwp_pointer_constraints_v1 bound");
    let _locked = constraints.lock_pointer(
        &opened.surface,
        &pointer,
        None,
        Lifetime::Persistent,
        &desk.qh,
        (),
    );
    desk.pump();
    desk.nudge((1.0, 0.0));
    desk.nudge((1.0, 0.0));
    let locked_at = desk.at();
    desk.warp(
        WarpTo::Point {
            x: 1500.0,
            y: 800.0,
        },
        None,
    );
    assert_eq!(
        (held, free, desk.at()),
        ((132.0, 132.0).into(), (1500.0, 800.0).into(), locked_at),
        "(where a warp left it with a button held, with the button up, with the \
         pointer locked)"
    );
}

/// Two floated windows, `left` at 100,100 and `right` at 600,100,
/// the keyboard on `left` and the pointer in its middle, locked there
/// by its client (#427). Returns both, and the lock, kept alive.
fn a_game_holding_the_pointer(
    desk: &mut Desk,
) -> (Opened, Opened, zwp_locked_pointer_v1::ZwpLockedPointerV1) {
    use zwp_pointer_constraints_v1::Lifetime;
    desk.install("");
    let left = desk.open();
    let right = desk.open();
    let (l, r) = (desk.id(&left), desk.id(&right));
    desk.apply(vec![
        floated(l, 100.0, 100.0, 0),
        floated(r, 600.0, 100.0, 0),
    ]);
    desk.rest();
    desk.state.focus_window(
        &left.window,
        SERIAL_COUNTER.next_serial(),
        FocusSource::Click,
    );
    desk.mouse_to((132.0, 132.0).into());
    let pointer = desk.client.seat_pointer(&desk.qh);
    let locked = desk
        .client
        .pointer_constraints
        .clone()
        .expect("zwp_pointer_constraints_v1 bound")
        .lock_pointer(
            &left.surface,
            &pointer,
            None,
            Lifetime::Persistent,
            &desk.qh,
            (),
        );
    desk.pump();
    desk.nudge((1.0, 0.0));
    desk.nudge((40.0, 0.0));
    (left, right, locked)
}

/// **#427: a key that focuses another window lets go of the pointer
/// the window it leaves had locked**: only the window with the
/// keyboard holds the pointer, so a game cannot keep it once a
/// binding has moved the keyboard elsewhere, and it is not granted
/// again over the game while the keyboard is on the other window.
#[test]
fn a_key_focusing_another_window_lets_go_of_the_pointer_a_game_locked() {
    let mut desk = Desk::new(1);
    let (left, right, _locked) = a_game_holding_the_pointer(&mut desk);
    let locked_at = desk.at();
    assert_eq!(
        locked_at,
        (132.0, 132.0).into(),
        "the premise: the lock holds the pointer still"
    );
    desk.state.focus_window(
        &right.window,
        SERIAL_COUNTER.next_serial(),
        FocusSource::Binding,
    );
    desk.pump();
    desk.nudge((10.0, 0.0));
    desk.nudge((10.0, 0.0));
    assert_eq!(
        (desk.focused().as_ref() == Some(&right.window), desk.at()),
        (true, (152.0, 132.0).into()),
        "(the keyboard on the other window, the pointer moving over the game)"
    );
    let _ = left;
}

/// **#427: `sol.release_pointer` takes the pointer back from the
/// window holding it, until that window is pressed on**: the pointer
/// moves with the keyboard still on the game, and a click on the game
/// gives it the pointer to lock again.
#[test]
fn release_pointer_lets_go_until_the_window_is_pressed_on() {
    let mut desk = Desk::new(1);
    let (left, _right, _locked) = a_game_holding_the_pointer(&mut desk);
    assert_eq!(desk.at(), (132.0, 132.0).into(), "the premise: locked");
    desk.state.release_pointer();
    desk.pump();
    desk.nudge((10.0, 0.0));
    desk.nudge((10.0, 0.0));
    let released = desk.at();
    assert_eq!(
        desk.focused().as_ref(),
        Some(&left.window),
        "a Wayland window keeps the keyboard; only an X11 one is \
         taken off it"
    );
    desk.button(true);
    desk.button(false);
    desk.nudge((1.0, 0.0));
    desk.nudge((10.0, 0.0));
    let pressed = desk.at();
    assert_eq!(
        (released, pressed),
        ((152.0, 132.0).into(), (152.0, 132.0).into()),
        "(where the pointer went once taken back, and where a press \
         on the game left it locked again)"
    );
}

/// **A warp to nothing goes nowhere, and one off every screen is kept
/// on one**: a window that is not there moves the pointer nowhere,
/// and a point past the monitor's corner is its last pixel.
#[test]
fn a_warp_to_nothing_goes_nowhere_and_one_off_every_screen_is_kept_on_one() {
    let mut desk = Desk::new(1);
    desk.install("");
    desk.mouse_to((100.0, 100.0).into());
    desk.warp(
        WarpTo::Window {
            id: 4242,
            at: (0.5, 0.5),
        },
        linear(200),
    );
    let nowhere = (desk.at(), desk.state.flight.is_none());
    desk.warp(
        WarpTo::Point {
            x: 5000.0,
            y: 5000.0,
        },
        None,
    );
    assert_eq!(
        (nowhere, desk.at()),
        (((100.0, 100.0).into(), true), (1919.0, 1079.0).into()),
        "(a window that is not there, a point off every screen)"
    );
}

/// **`sol.focus` with a warp takes the pointer to the window it
/// focuses**, at its centre for `warp = true`, with its picture there
/// at once.
#[test]
fn sol_focus_with_a_warp_takes_the_pointer_to_the_window_it_focuses() {
    let mut desk = Desk::new(1);
    desk.install("");
    let opened = desk.open();
    let id = desk.id(&opened);
    desk.apply(vec![floated(id, 600.0, 200.0, 0)]);
    desk.rest();
    desk.mouse_to((100.0, 900.0).into());
    desk.install(&format!(
        "sol.bind(\"super+w\", function() sol.focus({id}, {{ warp = true }}) end)"
    ));
    assert!(desk.state.trigger("super+w"), "super+w is bound");
    assert_eq!(
        (
            desk.focused().as_ref() == Some(&opened.window),
            desk.at(),
            desk.state.flight.is_none(),
        ),
        (true, (632.0, 232.0).into(), true),
        "(focused, the pointer at its centre, drawn there at once)"
    );
}

/// **The compositor's own changes of focus say what they are**: a
/// window opening is `"open"`, a press `"click"`, the pointer moving
/// onto a window with focus following it `"hover"`, a touch
/// `"touch"`, an application asking `"activate"`, a window found
/// after the one with the keyboard went `"settle"`, and a scene's
/// `windows.focus` `"surface"`.
#[test]
fn the_compositors_own_focus_changes_say_what_they_are() {
    let mut desk = Desk::new(1);
    desk.install(
        "heard = {}\n\
         sol.on(\"touch\", function(e) if e.kind == \"down\" and e.window then sol.focus(e.window) end end)\n\
         sol.on(\"focus\", function(_, source) heard[#heard + 1] = source end)\n\
         sol.on(\"surface\", function(_, action, data)\n\
             if action == \"chip\" then sol.act(\"windows.focus\", data) end\n\
         end)",
    );
    let first = desk.open();
    let second = desk.open();
    desk.apply(vec![
        floated(desk.id(&first), 100.0, 100.0, 0),
        floated(desk.id(&second), 1000.0, 100.0, 0),
    ]);
    desk.rest();
    let heard = |desk: &Desk| {
        desk.state
            .scripts
            .as_ref()
            .map(|scripts| scripts.evaluate("return table.concat(heard, \",\")"))
            .unwrap_or_default()
    };
    let opened = heard(&desk);

    // A press on the first window.
    desk.mouse_to((132.0, 132.0).into());
    desk.button(true);
    desk.button(false);
    // The pointer onto the second, with focus following it.
    desk.state.profile.focus_follows_mouse = true;
    desk.mouse_to((1032.0, 132.0).into());
    desk.state.profile.focus_follows_mouse = false;
    // A touch on the first.
    // The Lua policy above chooses touch-to-focus.
    let region = desk.region();
    crate::synth::send_touch(&mut desk.state, region, (132.0, 132.0), 9);
    desk.pump();
    // The second asks to be brought forward.
    let token = {
        let (token, _) = desk
            .state
            .activation_state
            .create_external_token(XdgActivationTokenData::default());
        token.as_str().to_owned()
    };
    desk.client
        .activation
        .clone()
        .expect("xdg_activation_v1 bound")
        .activate(token, &second.surface);
    desk.pump();
    // The keyboard left with nowhere to be, and found a window.
    desk.state.give_keyboard(None, SERIAL_COUNTER.next_serial());
    desk.state.settle_focus();
    // A bar's chip for the first.
    #[expect(clippy::cast_precision_loss, reason = "a pane id, far below 2^52")]
    let chip = crate::json::Json::Object(std::collections::BTreeMap::from([(
        "id".to_owned(),
        crate::json::Json::Number(desk.id(&first) as f64),
    )]));
    desk.action("chip", &chip);

    let all = heard(&desk);
    let after = all
        .strip_prefix(opened.as_str())
        .map(|rest| rest.trim_start_matches(','))
        .unwrap_or(all.as_str());
    assert_eq!(
        (opened.split(',').next_back(), after),
        (Some("open"), "click,hover,touch,activate,settle,surface"),
        "(what the second window opening was told as, what came after it)"
    );
}

/// **The `focus` event says whether the keyboard moved**: a window
/// focused from another is told `changed = true`, and the window that
/// has the keyboard focused again `false` -- a layout focusing its
/// window again after a key that only resized it, which is how the
/// shipped policy knows to leave the pointer alone.
#[test]
fn the_focus_event_says_whether_the_keyboard_moved() {
    let mut desk = Desk::new(1);
    desk.install(
        "heard = {}\n\
         sol.on(\"focus\", function(_, _, changed) heard[#heard + 1] = tostring(changed) end)",
    );
    let first = desk.open();
    let second = desk.open();
    let focus = |opened: &Opened| Command::Focus {
        id: opened.pane.get(),
        source: FocusSource::Script,
        warp: None,
    };
    desk.apply(vec![focus(&second)]);
    let evaluate = |desk: &Desk, code: &str| {
        desk.state
            .scripts
            .as_ref()
            .map(|scripts| scripts.evaluate(code))
            .unwrap_or_default()
    };
    assert_eq!(evaluate(&desk, "heard = {} return \"\""), "");
    desk.apply(vec![focus(&first)]);
    desk.apply(vec![focus(&first)]);
    desk.apply(vec![focus(&second)]);
    assert_eq!(
        evaluate(&desk, "return table.concat(heard, \",\")"),
        "true,false,true",
        "(the first from the second, the first again, the second from the first)"
    );
}

/// `sol.press`: a button or a key delivered to a window by a script,
/// as a device would deliver it. See `state/press.rs`.
mod press {
    use super::*;

    /// `BTN_SIDE`, a mouse's back button, as evdev names it.
    const SIDE: u32 = 0x113;
    /// Escape, the left Alt and the left arrow, as evdev names them.
    const ESCAPE: u32 = 1;
    const ALT: u32 = 56;
    const ARROW: u32 = 105;

    /// A surface action runs its name as Lua, inside a dispatch as
    /// a scene's action is, and says how it went in `said`.
    const RUNS: &str = "said = \"\"\n\
         sol.on(\"surface\", function(_, action)\n\
             local ok, err = pcall(load(action))\n\
             said = ok and \"ok\" or tostring(err)\n\
         end)\n";

    /// Two floating windows of the one client, `first` at 100,100
    /// and `second` at 1000,100, with the client holding a pointer,
    /// and `first` focused by a click on it.
    fn two_windows(desk: &mut Desk) -> (Opened, Opened, wl_pointer::WlPointer) {
        desk.install(RUNS);
        let first = desk.open();
        let second = desk.open();
        desk.apply(vec![
            floated(desk.id(&first), 100.0, 100.0, 0),
            floated(desk.id(&second), 1000.0, 100.0, 0),
        ]);
        desk.rest();
        let pointer = desk.client.seat_pointer(&desk.qh);
        desk.pump();
        desk.mouse_to((132.0, 132.0).into());
        desk.button(true);
        desk.button(false);
        assert!(
            desk.focused().as_ref() == Some(&first.window),
            "the premise: a click focused the first window"
        );
        desk.client.buttons.clear();
        desk.client.clicked_on.clear();
        (first, second, pointer)
    }

    /// Run `code` as a scene's action would, and say how it went.
    fn run(desk: &mut Desk, code: &str) -> String {
        desk.action(code, &crate::json::Json::Null);
        desk.state
            .scripts
            .as_ref()
            .map(|scripts| scripts.evaluate("return said"))
            .unwrap_or_default()
    }

    fn surface_id(opened: &Opened) -> u32 {
        wayland_client::Proxy::id(&opened.surface).protocol_id()
    }

    /// **A window hears the side button with the pointer elsewhere,
    /// and nothing else moves**: `sol.press(id, { button = "side" })`
    /// on the second window, with the pointer over nothing and then
    /// over the first, reaches the second as a press and a release,
    /// on its own surface; the pointer does not move, its picture
    /// neither moves nor changes, the keyboard stays on the first,
    /// and the pointer is back on what is under it.
    #[test]
    fn a_window_hears_the_side_button_with_the_pointer_elsewhere_and_nothing_else_moves() {
        let mut desk = Desk::new(1);
        let (first, second, _pointer) = two_windows(&mut desk);
        let press = format!("sol.press({}, {{ button = \"side\" }})", desk.id(&second));
        for over in [(600.0, 700.0), (132.0, 132.0)] {
            desk.mouse_to(over.into());
            desk.rest();
            desk.client.buttons.clear();
            desk.client.clicked_on.clear();
            let (at, drawn, showing) = (desk.at(), desk.drawn(), desk.state.pointer.said());
            let under = desk.under();
            let said = run(&mut desk, &press);
            assert_eq!(
                (
                    said.as_str(),
                    desk.client.buttons.clone(),
                    desk.client.clicked_on.clone(),
                    desk.at(),
                    desk.drawn(),
                    desk.state.pointer.said() == showing,
                    desk.under() == under,
                    desk.focused().as_ref() == Some(&first.window),
                    desk.state.flight.is_none(),
                ),
                (
                    "ok",
                    vec![(SIDE, true), (SIDE, false)],
                    vec![Some(surface_id(&second)); 2],
                    at,
                    drawn,
                    true,
                    true,
                    true,
                    true,
                ),
                "over {over:?}: (said, the buttons, on which surface, the pointer, its \
                 picture, its shape kept, what it is on kept, the keyboard on the first, \
                 nothing flying)"
            );
        }
        assert_eq!(
            desk.client.pointer_on,
            Some(surface_id(&first)),
            "and the client was told the pointer is back on the first window"
        );
    }

    /// **A key reaches the window, and gives it the keyboard**:
    /// `Escape` and then `alt+Left` on the second window, with the
    /// first focused and Russian active, reach the second as presses
    /// and releases in order, on its own surface; the second has the
    /// keyboard, and the pointer has not moved.
    #[test]
    fn a_key_reaches_the_window_and_gives_it_the_keyboard() {
        let mut desk = Desk::new(1);
        let (_first, second, _pointer) = two_windows(&mut desk);
        desk.mouse_to((600.0, 700.0).into());
        let at = desk.at();
        desk.client.key_events.clear();
        desk.client.typed.clear();
        let id = desk.id(&second);
        let escape = run(
            &mut desk,
            &format!("sol.press({id}, {{ key = \"Escape\" }})"),
        );
        let back = run(
            &mut desk,
            &format!("sol.press({id}, {{ key = \"alt+Left\" }})"),
        );
        desk.pump();
        assert_eq!(
            (
                escape.as_str(),
                back.as_str(),
                desk.client.key_events.clone(),
                desk.client
                    .typed
                    .iter()
                    .all(|(on, _)| *on == Some(surface_id(&second))),
                desk.focused().as_ref() == Some(&second.window),
                desk.at(),
            ),
            (
                "ok",
                "ok",
                vec![
                    (ESCAPE, true),
                    (ESCAPE, false),
                    (ALT, true),
                    (ARROW, true),
                    (ARROW, false),
                    (ALT, false),
                ],
                true,
                true,
                at,
            ),
            "(said for Escape, for alt+Left, the keys the client heard, all on the \
             second window, the second focused, the pointer)"
        );
    }

    /// **A key is found in the active layout first**: with Russian
    /// active, `period` is the key Russian puts `.` on (evdev's
    /// slash), not English's (evdev's dot, `ю` on Russian), and the
    /// `t` of `ctrl+shift+t`, which Russian has nowhere, is the key
    /// English has it on, as a person presses the shortcut.
    #[test]
    fn a_key_is_found_in_the_active_layout_first() {
        /// The left Control and Shift, `t` and `/`, as evdev names them.
        const CTRL: u32 = 29;
        const SHIFT: u32 = 42;
        const T: u32 = 20;
        const SLASH: u32 = 53;
        let mut desk = Desk::new(1);
        let (_first, second, _pointer) = two_windows(&mut desk);
        desk.client.key_events.clear();
        let id = desk.id(&second);
        let period = run(
            &mut desk,
            &format!("sol.press({id}, {{ key = \"period\" }})"),
        );
        let shortcut = run(
            &mut desk,
            &format!("sol.press({id}, {{ key = \"ctrl+shift+t\" }})"),
        );
        desk.pump();
        assert_eq!(
            (
                period.as_str(),
                shortcut.as_str(),
                desk.client.key_events.clone()
            ),
            (
                "ok",
                "ok",
                vec![
                    (SLASH, true),
                    (SLASH, false),
                    (CTRL, true),
                    (SHIFT, true),
                    (T, true),
                    (T, false),
                    (SHIFT, false),
                    (CTRL, false),
                ],
            ),
            "(said for period, for ctrl+shift+t, the keys the client heard)"
        );
    }

    /// **A minimized window is pressed nowhere**: asked from Lua it
    /// is an error, and a press already asked for when it was
    /// minimized reaches it neither as a button nor as a key, nor
    /// gives it the keyboard.
    #[test]
    fn a_minimized_window_is_pressed_nowhere() {
        let mut desk = Desk::new(1);
        let (first, second, _pointer) = two_windows(&mut desk);
        let id = desk.id(&second);
        desk.apply(vec![Command::Minimize { id }]);
        desk.client.key_events.clear();
        let said = run(
            &mut desk,
            &format!("sol.press({id}, {{ button = \"side\" }})"),
        );
        desk.apply(vec![
            Command::Press {
                id,
                press: crate::script::Press::Button(SIDE),
                source: FocusSource::Script,
            },
            Command::Press {
                id,
                press: crate::script::Press::Keys(vec![smithay::input::keyboard::Keysym::Escape]),
                source: FocusSource::Script,
            },
        ]);
        assert_eq!(
            (
                said.contains("is minimized"),
                desk.client.buttons.clone(),
                desk.client.key_events.clone(),
                desk.focused().as_ref() == Some(&first.window),
            ),
            (true, Vec::new(), Vec::new(), true),
            "(an error from Lua, the buttons, the keys, the keyboard still on the first): {said}"
        );
    }

    /// **`sol.windows()` rows carry the client's pid**: the client
    /// here is this process, on the other end of a socketpair, so
    /// the kernel says its pid is ours.
    #[test]
    fn sol_windows_rows_carry_the_clients_pid() {
        let mut desk = Desk::new(1);
        desk.install(RUNS);
        let _opened = desk.open();
        let said = run(&mut desk, "seen = tostring(sol.windows()[1].pid)");
        let seen = desk
            .state
            .scripts
            .as_ref()
            .map(|scripts| scripts.evaluate("return seen"))
            .unwrap_or_default();
        assert_eq!(
            (said.as_str(), seen),
            ("ok", std::process::id().to_string()),
            "(said, the pid the row carries)"
        );
    }
}
