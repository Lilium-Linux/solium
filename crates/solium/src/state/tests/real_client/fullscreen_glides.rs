//! **#49: going fullscreen or maximised, and coming back, is drawn
//! moving**, through the placement path and the transform a layout's
//! glide uses, on the one clock.
//!
//! Each window is 400x300 at 300,200 on a 1920x1080 monitor, with no
//! frames, and the scripts answer `fullscreen` with 260 ms and `maximize`
//! with 220 ms, both linear, so where a frame is drawn is arithmetic. The
//! keys are bound as `modes.lua` binds them. Frames are read through
//! `Solium::drawn_at`, which is what the renderer and the hit tests draw
//! and resolve a pane with, eight of them 45 ms apart as #49's own
//! measurement took them -- which found the window at its destination on
//! frame 0 and on every frame after.

use super::*;

pub(super) const SCRIPT: &str = r#"
    sol.on("fullscreen", function() sol.animate({ duration = 260, easing = "linear" }) end)
    sol.on("maximize", function() sol.animate({ duration = 220, easing = "linear" }) end)
    sol.bind("super+f", function() sol.toggle_fullscreen() end)
    sol.bind("super+shift+m", function() sol.toggle_maximize() end)
"#;

/// Where the window lives before anything is toggled.
pub(super) fn before() -> Rectangle<f64, Logical> {
    Rectangle::new((300, 200).into(), (400, 300).into()).to_f64()
}

/// The monitor, which is also its work area: there is no bar.
pub(super) fn screen() -> Rectangle<f64, Logical> {
    Rectangle::new((0, 0).into(), (1920, 1080).into()).to_f64()
}

/// One monitor, one client's window shown and at rest, focused, and
/// the scripts a test names.
struct Desk {
    display: Display<Solium>,
    state: Solium,
    conn: Connection,
    queue: wayland_client::EventQueue<Client>,
    qh: QueueHandle<Client>,
    client: Client,
    window: Window,
    toplevel: xdg_toplevel::XdgToplevel,
    surface: wl_surface::WlSurface,
    pane: crate::pane::PaneId,
}

impl Desk {
    fn new(name: &str, script: &str) -> Self {
        Self::on(name, script, (300, 200), |state| {
            one_screen(state);
        })
    }

    /// [`Self::new`], with the monitors `screens` maps and the
    /// window at `at`.
    fn on(name: &str, script: &str, at: (i32, i32), screens: impl FnOnce(&mut Solium)) -> Self {
        let mut display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        state
            .decorations
            .set_style(&mut state.panes, Some("none".to_string()));
        screens(&mut state);
        state.start_scripts(Some(script_at(name, script)));

        let (conn, mut queue, mut client) = connect(&mut display, &mut state);
        let qh = queue.handle();
        let (window, toplevel, surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        commit_buffer(&client, &qh, &surface, 400, 300);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        state.space.map_element(window.clone(), at, false);
        state.space.refresh();
        let pane = state.panes.id_of(&window).expect("the window has a pane");
        state.focus_window(
            &window,
            SERIAL_COUNTER.next_serial(),
            crate::script::FocusSource::Script,
        );
        let mut desk = Self {
            display,
            state,
            conn,
            queue,
            qh,
            client,
            window,
            toplevel,
            surface,
            pane,
        };
        desk.land();
        assert!(!desk.transformed(), "the premise: at rest where it lives");
        desk
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

    /// Where the window is drawn at `at`.
    fn drawn(&self, at: Duration) -> Rectangle<f64, Logical> {
        let held = self.state.panes.get(self.pane).expect("the pane is here");
        self.state
            .drawn_at(held, self.state.pane_outer(held), at)
            .rect
    }

    /// Eight frames 45 ms apart, from `from`.
    fn burst(&self, from: Duration) -> Vec<Rectangle<f64, Logical>> {
        (0..8)
            .map(|frame| self.drawn(from + Duration::from_millis(45 * frame)))
            .collect()
    }

    /// The client answering the size it was told, as a client does.
    fn answer(&mut self, width: i32, height: i32) {
        commit_buffer(&self.client, &self.qh, &self.surface, width, height);
        self.pump();
    }

    /// A frame `after` from now, retired as a frame retires it.
    fn frame_after(&mut self, after: Duration) {
        self.state.clock.advance(after);
        let now = self.state.clock.now();
        self.state.settle(now);
        self.state.sync_panes();
    }

    /// Past every animation, retired as a frame retires it.
    fn land(&mut self) {
        self.state.clock.advance(Duration::from_secs(1));
        let now = self.state.clock.now();
        self.state.settle(now);
        self.state.sync_panes();
    }

    fn transformed(&self) -> bool {
        self.state
            .panes
            .get(self.pane)
            .is_some_and(present::transformed)
    }
}

/// `script`, loaded from a file of its own, which is gone again by the
/// time this returns.
pub(super) fn script_at(name: &str, script: &str) -> Scripts {
    let directory =
        std::env::temp_dir().join(format!("solium-glides-{name}-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    let entry = directory.join("init.lua");
    std::fs::write(&entry, script).expect("writing the test script");
    let scripts = Scripts::load(&entry).expect("loading the test script");
    let _ = std::fs::remove_dir_all(&directory);
    scripts
}

/// Whether `rect` is part of the way from `from` to `to`: strictly
/// between them on every edge that moves, and on the others where
/// both are.
pub(super) fn between(
    rect: Rectangle<f64, Logical>,
    from: Rectangle<f64, Logical>,
    to: Rectangle<f64, Logical>,
) -> bool {
    let part = |value: f64, from: f64, to: f64| {
        if (from - to).abs() < 0.5 {
            (value - from).abs() < 0.5
        } else {
            (value - from) * (to - from) > 0.0 && (value - to) * (from - to) > 0.0
        }
    };
    part(rect.loc.x, from.loc.x, to.loc.x)
        && part(rect.loc.y, from.loc.y, to.loc.y)
        && part(rect.size.w, from.size.w, to.size.w)
        && part(rect.size.h, from.size.h, to.size.h)
}

/// The same, with either end allowed.
fn from_to(
    rect: Rectangle<f64, Logical>,
    from: Rectangle<f64, Logical>,
    to: Rectangle<f64, Logical>,
) -> bool {
    rect == from || rect == to || between(rect, from, to)
}

/// That `frames` is a glide from `from` to `to`: frame 0 where it
/// was, frames 1 to `moving` part of the way and each further than
/// the last, and frame 7, at 315 ms, landed. 5 frames move in a
/// 260 ms glide and 4 in a 220 ms one; the frames between the last
/// of those and frame 7 are left out, because the glide starts when
/// the change is made, after the moment the burst counts from, by
/// however long the request took.
fn glides(
    frames: &[Rectangle<f64, Logical>],
    from: Rectangle<f64, Logical>,
    to: Rectangle<f64, Logical>,
    moving: usize,
) {
    assert_eq!(
        frames[0], from,
        "frame 0 is where it was drawn: {frames:#?}"
    );
    for frame in 1..=moving {
        assert!(
            between(frames[frame], from, to),
            "frame {frame} is part of the way from {from:?} to {to:?}: {frames:#?}"
        );
        assert!(
            between(frames[frame], frames[frame - 1], to),
            "frame {frame} is further on than the one before: {frames:#?}"
        );
    }
    assert_eq!(frames[7], to, "and frame 7 has landed: {frames:#?}");
}

/// **A window sent fullscreen grows from where it was to cover the
/// monitor, and shrinks back when it leaves**, as its client asks
/// both through `xdg_toplevel`. Once each lands, and its client has
/// answered, it holds no transform: a plain element, which a
/// fullscreen game or video needs to be scanned out directly.
#[test]
fn a_window_glides_into_fullscreen_and_out_again() {
    let mut desk = Desk::new("in-and-out", SCRIPT);

    let at = desk.state.clock.now();
    desk.toplevel.set_fullscreen(None);
    desk.pump();
    glides(&desk.burst(at), before(), screen(), 5);
    desk.answer(1920, 1080);
    desk.land();
    assert!(
        !desk.transformed(),
        "fullscreen and at rest, it holds no transform"
    );
    assert_eq!(desk.drawn(desk.state.clock.now()), screen());

    let at = desk.state.clock.now();
    desk.toplevel.unset_fullscreen();
    desk.pump();
    glides(&desk.burst(at), screen(), before(), 5);
    desk.answer(400, 300);
    desk.land();
    assert!(
        !desk.transformed(),
        "back and at rest, it holds no transform"
    );
    assert_eq!(desk.drawn(desk.state.clock.now()), before());
    assert_eq!(
        desk.state.panes.get(desk.pane).and_then(Pane::lifted_until),
        None,
        "and keeps nothing of its shrink once that has landed"
    );
}

/// **And maximised, and restored, the same way**, by the key.
#[test]
fn a_window_glides_into_maximised_and_out_again() {
    let mut desk = Desk::new("maximised", SCRIPT);

    let at = desk.state.clock.now();
    assert!(
        desk.state.trigger("super+shift+m"),
        "super+shift+m is bound"
    );
    desk.pump();
    glides(&desk.burst(at), before(), screen(), 4);
    desk.answer(1920, 1080);
    desk.land();
    assert!(!desk.transformed(), "maximised and at rest, no transform");

    let at = desk.state.clock.now();
    assert!(desk.state.trigger("super+shift+m"));
    desk.pump();
    glides(&desk.burst(at), screen(), before(), 4);
    desk.answer(400, 300);
    desk.land();
    assert!(!desk.transformed(), "restored and at rest, no transform");
    assert_eq!(desk.drawn(desk.state.clock.now()), before());
}

/// **The window is told its new size on the toggle**, not when the
/// animation lands, and is drawn where it was while it is told: its
/// old picture is what is stretched until it answers. Both ways.
#[test]
fn the_client_is_told_its_new_size_on_the_toggle() {
    let mut desk = Desk::new("told", SCRIPT);

    let at = desk.state.clock.now();
    assert!(desk.state.trigger("super+f"), "super+f is bound");
    desk.pump();
    assert_eq!(
        last_configured(&desk.client, &desk.toplevel),
        Some((1920, 1080)),
        "told the monitor's size on the key"
    );
    assert_eq!(desk.drawn(at), before(), "while still drawn where it was");
    desk.answer(1920, 1080);
    desk.land();

    let at = desk.state.clock.now();
    assert!(desk.state.trigger("super+f"));
    desk.pump();
    assert_eq!(
        last_configured(&desk.client, &desk.toplevel),
        Some((400, 300)),
        "told the size it had, on the key back"
    );
    assert_eq!(
        desk.drawn(at),
        screen(),
        "while still drawn covering the monitor"
    );
}

/// **A second toggle part of the way through the first starts from
/// where the window is drawn**, not from the monitor it was headed
/// for and not from the slot it left. The first glide, 60 ms on from
/// the moment it is read, bounds how far it can have got by the
/// time the second key is handled.
#[test]
fn a_second_toggle_mid_flight_starts_from_where_the_window_is_drawn() {
    let mut desk = Desk::new("mid-flight", SCRIPT);

    assert!(desk.state.trigger("super+f"));
    desk.pump();
    desk.state.clock.advance(Duration::from_millis(100));
    let at = desk.state.clock.now();
    let mid = desk.drawn(at);
    let soon = desk.drawn(at + Duration::from_millis(60));
    assert!(
        between(mid, before(), screen()) && between(soon, mid, screen()),
        "the premise: part of the way into fullscreen, and still going: {mid:?}, {soon:?}"
    );

    assert!(desk.state.trigger("super+f"));
    desk.pump();
    let from = desk.drawn(at);
    assert!(
        from_to(from, mid, soon),
        "the way back starts where the window was drawn, between {mid:?} and \
         {soon:?}, and not at the monitor or at the slot it left: {from:?}"
    );
    assert!(
        between(desk.drawn(at + Duration::from_millis(130)), from, before()),
        "and it shrinks from there"
    );
    assert_eq!(
        desk.drawn(at + Duration::from_millis(400)),
        before(),
        "back where it lives"
    );
}

/// **A listener that toggles the change straight back is not told
/// that too**: it would answer it again, and again. Told once, the
/// window ends where it started.
#[test]
fn a_listener_that_toggles_the_change_back_is_not_told_it_again() {
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;

    let mut desk = Desk::new(
        "toggled-back",
        r#"
        local told = 0
        sol.on("fullscreen", function(id)
            told = told + 1
            sol.status(tostring(told))
            if told < 5 then sol.toggle_fullscreen(id) end
        end)
        sol.bind("super+f", function() sol.toggle_fullscreen() end)
        "#,
    );

    assert!(desk.state.trigger("super+f"));
    desk.pump();
    assert_eq!(desk.state.status, "1", "told once");
    assert!(
        !in_state(&desk.window, State::Fullscreen),
        "and the listener's toggle back was made"
    );
    desk.land();
    assert_eq!(desk.drawn(desk.state.clock.now()), before());
    assert!(!desk.transformed());
}

/// The right of two monitors, side by side.
fn right() -> Rectangle<f64, Logical> {
    Rectangle::new((1920, 0).into(), (1920, 1080).into()).to_f64()
}

/// **A window on the second monitor grows to cover that one**, the
/// monitor it is on and not the first, and shrinks back to where it
/// was on it.
#[test]
fn a_window_on_the_second_monitor_glides_to_cover_that_one() {
    let mut desk = Desk::on("second-monitor", SCRIPT, (2220, 200), |state| {
        side_by_side(state, "left-test");
    });
    let was = Rectangle::new((2220, 200).into(), (400, 300).into()).to_f64();

    let at = desk.state.clock.now();
    assert!(desk.state.trigger("super+f"));
    desk.pump();
    glides(&desk.burst(at), was, right(), 5);
    desk.answer(1920, 1080);
    desk.land();
    assert!(!desk.transformed());

    let at = desk.state.clock.now();
    assert!(desk.state.trigger("super+f"));
    desk.pump();
    glides(&desk.burst(at), right(), was, 5);
}

/// **A monitor unplugged part of the way through leaves no window
/// transformed**: the window growing on it is brought onto the one
/// left, and is at rest there once that lands.
#[test]
fn a_monitor_unplugged_mid_glide_leaves_the_window_at_rest() {
    let mut gone = None;
    let mut desk = Desk::on("unplugged", SCRIPT, (2220, 200), |state| {
        gone = Some(side_by_side(state, "left-test").1);
    });
    let gone = gone.expect("the right monitor");

    assert!(desk.state.trigger("super+f"));
    desk.pump();
    desk.state.clock.advance(Duration::from_millis(100));
    let was = Rectangle::new((2220, 200).into(), (400, 300).into()).to_f64();
    assert!(
        between(desk.drawn(desk.state.clock.now()), was, right()),
        "the premise: part of the way"
    );
    crate::layer::close_all(&gone);
    desk.state.space.unmap_output(&gone);
    desk.state.settle_monitors();
    desk.land();
    assert!(!desk.transformed(), "at rest once it lands");
    let drawn = desk.drawn(desk.state.clock.now());
    assert!(
        drawn.loc.x < 1920.0,
        "and drawn on the monitor that is left: {drawn:?}"
    );
}

/// **An instant change is drawn as the window is on the next frame**:
/// a listener answering with no length leaves no transform behind,
/// rather than one whose target -- the monitor -- the next frame
/// draws with the old 400x300 picture stretched across it.
#[test]
fn an_instant_change_draws_the_window_as_it_is_on_the_next_frame() {
    let mut desk = Desk::new(
        "instant",
        r#"
        sol.on("fullscreen", function() sol.animate({ duration = 0 }) end)
        sol.bind("super+f", function() sol.toggle_fullscreen() end)
        "#,
    );
    assert!(desk.state.trigger("super+f"));
    desk.pump();
    let next = desk.state.clock.now() + Duration::from_millis(16);
    let drawn = desk.drawn(next);
    assert_eq!(
        (drawn.size.w, drawn.size.h),
        (400.0, 300.0),
        "the first frame draws the committed 400x300 picture, not stretched to the monitor"
    );
    assert!(!desk.transformed(), "and holds no transform");
}

/// **And part of the way through a glide, it holds nothing either**:
/// a window maximising, not yet answered, sent fullscreen at once is
/// drawn as its client has it on the next frame, not with its old
/// picture stretched across the monitor by the hold the maximise
/// left.
#[test]
fn an_instant_change_part_of_the_way_through_a_glide_holds_nothing() {
    let mut desk = Desk::new(
        "instant-mid-glide",
        r#"
        sol.on("maximize", function() sol.animate({ duration = 220, easing = "linear" }) end)
        sol.on("fullscreen", function() sol.animate({ duration = 0 }) end)
        sol.bind("super+f", function() sol.toggle_fullscreen() end)
        sol.bind("super+shift+m", function() sol.toggle_maximize() end)
        "#,
    );
    assert!(desk.state.trigger("super+shift+m"));
    desk.pump();
    desk.state.clock.advance(Duration::from_millis(100));
    assert!(desk.state.trigger("super+f"));
    desk.pump();
    let next = desk.state.clock.now() + Duration::from_millis(16);
    assert_eq!(
        desk.drawn(next),
        Rectangle::new((0, 0).into(), (400, 300).into()).to_f64(),
        "the committed 400x300 picture where it now lives"
    );
    assert!(!desk.transformed(), "with no transform");
    assert!(!desk.state.holding_resize(desk.pane), "and nothing held");
}

/// **A client slower than the glide is held at its new rectangle
/// until it answers**, at the rectangle the glide landed on: the
/// hold an edge drag keeps on a window until its client answers,
/// kept from the toggle. Without it, going in, the window was drawn
/// 400x300 in the monitor's corner from the moment it landed, and
/// coming out, 1920x1080 hanging off the monitor at 300,200.
/// Answered, it is drawn as it is and nothing holds it.
///
/// Its picture meanwhile is fitted as `resize.fill` says, as every
/// glide's is since #233 -- stretched before that, whatever the
/// setting, which then spoke only of a drag. A fill that is not the
/// default, so that the default cannot pass for it.
#[test]
fn a_slow_client_is_held_at_its_new_rectangle_until_it_answers() {
    let mut desk = Desk::new("slow", SCRIPT);
    desk.state.resizing.fill = crate::resizing::Fill::Crop;
    for (size, to) in [((1920, 1080), screen()), ((400, 300), before())] {
        assert!(desk.state.trigger("super+f"));
        desk.pump();
        desk.frame_after(Duration::from_millis(300));
        assert!(!desk.transformed(), "landed, it holds no transform");
        assert_eq!(
            desk.drawn(desk.state.clock.now()),
            to,
            "drawn where it landed while its client has not answered"
        );
        assert_eq!(
            desk.state.resize_fill(desk.pane),
            crate::resizing::Fill::Crop,
            "fitted to it as `resize.fill` says"
        );
        desk.answer(size.0, size.1);
        desk.frame_after(Duration::from_millis(16));
        assert_eq!(desk.drawn(desk.state.clock.now()), to, "and once it has");
        assert!(
            !desk.state.holding_resize(desk.pane),
            "and nothing holds it once it has answered"
        );
        desk.land();
    }
}

/// **A window moved by its titlebar while its change holds it is
/// where it was moved**: the hold keeps the size the change told its
/// client, not the place, so it is drawn under the pointer, and a
/// client that never answers lands there rather than back where the
/// change put it.
#[test]
fn a_window_moved_while_its_change_holds_it_stays_where_it_was_moved() {
    let mut desk = Desk::new("moved-while-held", SCRIPT);
    assert!(desk.state.trigger("super+f"));
    desk.pump();
    desk.answer(1920, 1080);
    desk.land();
    assert!(desk.state.trigger("super+f"));
    desk.pump();
    desk.frame_after(Duration::from_millis(300));
    assert!(desk.state.holding_resize(desk.pane), "the premise: held");
    // What `MoveGrab::motion` does on every motion.
    desk.state
        .space
        .map_element(desk.window.clone(), (600, 400), false);
    desk.frame_after(Duration::from_millis(16));
    let moved = Rectangle::new((600, 400).into(), (400, 300).into()).to_f64();
    assert_eq!(
        desk.drawn(desk.state.clock.now()),
        moved,
        "drawn where it was moved, at the size it was told"
    );
    desk.frame_after(crate::resizing::PATIENCE);
    assert!(!desk.state.holding_resize(desk.pane), "no longer held");
    assert_eq!(
        desk.drawn(desk.state.clock.now()).loc,
        moved.loc,
        "and left there once its patience runs out"
    );
}

/// **#404: a window made fullscreen while it is being dragged covers
/// its monitor, and the drag is over.** `super+f` with super already
/// held for a drag is easy to press by accident; the grab went on
/// moving the window by the pointer, so it ended fullscreen at an
/// offset with the desk showing along two edges. Maximised the same:
/// the work area is where it goes, not the pointer.
#[test]
fn a_window_made_fullscreen_mid_drag_covers_its_monitor_and_the_drag_ends() {
    use smithay::backend::input::ButtonState;
    use smithay::input::pointer::GrabStartData;
    for (name, key, covers) in [
        ("fullscreen-mid-drag", "super+f", screen()),
        ("maximize-mid-drag", "super+shift+m", screen()),
    ] {
        let mut desk = Desk::new(name, SCRIPT);
        let region = Rectangle::new((0, 0).into(), (1920, 1080).into());
        let pointer = desk
            .state
            .seat
            .get_pointer()
            .expect("the seat has a pointer");
        let mut time = 1_000_u64;
        let mut motion = |state: &mut Solium, by: (f64, f64)| {
            time += 8_000;
            crate::synth::send_motion(state, region, by.into(), time);
        };
        let at = pointer.current_location();
        motion(&mut desk.state, (500.0 - at.x, 350.0 - at.y));
        crate::synth::send_button(
            &mut desk.state,
            region,
            0x110,
            ButtonState::Pressed,
            100_000,
        );
        // What a press on a frame, or a client's `xdg_toplevel.move`,
        // starts: the grab that follows the pointer.
        let start = GrabStartData {
            focus: None,
            button: 0x110,
            location: pointer.current_location(),
        };
        crate::input::grab::MoveGrab::start(
            &mut desk.state,
            &pointer,
            start,
            desk.window.clone(),
            (300, 200).into(),
            SERIAL_COUNTER.next_serial(),
        );
        motion(&mut desk.state, (60.0, 40.0));
        assert_eq!(
            desk.state.real_geometry(&desk.window).map(|real| real.loc),
            Some((360, 240).into()),
            "{name}: the premise: the drag moves the window"
        );

        assert!(desk.state.trigger(key), "{key} was not handled");
        desk.pump();
        assert!(!pointer.is_grabbed(), "{name}: the drag is over");
        motion(&mut desk.state, (-170.0, 50.0));
        motion(&mut desk.state, (-90.0, 30.0));
        crate::synth::send_button(
            &mut desk.state,
            region,
            0x110,
            ButtonState::Released,
            200_000,
        );
        desk.answer(1920, 1080);
        desk.land();
        assert_eq!(
            desk.state
                .real_geometry(&desk.window)
                .map(|real| real.loc.to_f64()),
            Some(covers.loc),
            "{name}: where it lives is the corner of what it covers"
        );
        assert_eq!(
            desk.drawn(desk.state.clock.now()),
            covers,
            "{name}: drawn over exactly what it covers"
        );
    }
}

/// **#452: a maximised window dragged off glides out of its
/// maximise into the rectangle under the pointer, as the key's
/// restore glides, and the glide goes with the pointer**: a motion
/// part of the way through moves the picture as far as the pointer
/// went, and it lands where the window is, not where the drag first
/// put it.
#[test]
fn a_window_dragged_off_its_maximise_glides_under_the_pointer() {
    use smithay::backend::input::ButtonState;
    use smithay::input::pointer::GrabStartData;
    // `lua/floating.lua` is what pulls a maximised window off.
    let script = format!(
        "package.path = {shipped:?} .. \"/?.lua\"\nrequire(\"floating\")\n{SCRIPT}",
        shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
    );
    let mut desk = Desk::new("dragged-off", &script);
    assert!(desk.state.trigger("super+shift+m"));
    desk.pump();
    desk.answer(1920, 1080);
    desk.land();
    assert_eq!(desk.drawn(desk.state.clock.now()), screen());

    let region = Rectangle::new((0, 0).into(), (1920, 1080).into());
    let pointer = desk
        .state
        .seat
        .get_pointer()
        .expect("the seat has a pointer");
    let mut time = 1_000_u64;
    let mut motion = |state: &mut Solium, by: (f64, f64)| {
        time += 8_000;
        crate::synth::send_motion(state, region, by.into(), time);
    };
    let at = pointer.current_location();
    motion(&mut desk.state, (960.0 - at.x, 10.0 - at.y));
    crate::synth::send_button(
        &mut desk.state,
        region,
        0x110,
        ButtonState::Pressed,
        100_000,
    );
    let start = GrabStartData {
        focus: None,
        button: 0x110,
        location: pointer.current_location(),
    };
    crate::input::grab::MoveGrab::start(
        &mut desk.state,
        &pointer,
        start,
        desk.window.clone(),
        (0, 0).into(),
        SERIAL_COUNTER.next_serial(),
    );
    crate::input::grab::settle_drags(&mut desk.state, true);
    // Half way along and 10 down, so 200 left of the pointer and 10
    // above it at 400 by 300.
    motion(&mut desk.state, (40.0, 100.0));
    crate::input::grab::settle_drags(&mut desk.state, false);
    let under = Rectangle::new((800, 100).into(), (400, 300).into()).to_f64();
    let first = desk.drawn(desk.state.clock.now());
    assert!(
        (first.loc.x - 0.0).abs() < 1.0
            && (first.loc.y - 0.0).abs() < 1.0
            && (first.size.w - 1920.0).abs() < 1.0
            && (first.size.h - 1080.0).abs() < 1.0,
        "the glide starts from the maximised rectangle: {first:?}"
    );
    desk.frame_after(Duration::from_millis(45));
    let then = desk.state.clock.now();
    let part = desk.drawn(then);
    assert!(
        between(part, screen(), under),
        "part of the way out at 45 ms: {part:?}"
    );
    motion(&mut desk.state, (100.0, 0.0));
    crate::input::grab::settle_drags(&mut desk.state, false);
    let carried = desk.drawn(then);
    assert_eq!(
        (
            (carried.loc.x - part.loc.x - 100.0).abs() < 1e-6,
            (carried.loc.y - part.loc.y).abs() < 1e-6,
            carried.size
        ),
        (true, true, part.size),
        "the glide did not go with the pointer"
    );
    crate::synth::send_button(
        &mut desk.state,
        region,
        0x110,
        ButtonState::Released,
        200_000,
    );
    desk.answer(400, 300);
    desk.land();
    let moved = Rectangle::new((900, 100).into(), (400, 300).into()).to_f64();
    assert_eq!(
        desk.drawn(desk.state.clock.now()),
        moved,
        "it landed somewhere other than under the pointer"
    );
    assert!(!desk.transformed(), "and holds no transform at rest");
}

/// **And one that never answers is held only so long**:
/// `resizing::PATIENCE` past the landing, as a drag's hold is, and
/// then drawn at the size it has, where it lives.
#[test]
fn a_client_that_never_answers_is_drawn_as_it_is_once_its_patience_runs_out() {
    let mut desk = Desk::new("silent", SCRIPT);
    assert!(desk.state.trigger("super+f"));
    desk.pump();
    desk.frame_after(Duration::from_millis(300));
    assert_eq!(
        desk.drawn(desk.state.clock.now()),
        screen(),
        "the premise: held at the monitor"
    );
    desk.frame_after(crate::resizing::PATIENCE);
    assert!(!desk.state.holding_resize(desk.pane), "no longer held");
    assert_eq!(
        desk.drawn(desk.state.clock.now()),
        Rectangle::new((0, 0).into(), (400, 300).into()).to_f64(),
        "drawn at the size its client has, where it lives"
    );
}

/// **An edge drag on a window its change is still holding takes
/// it**: the change's hold is let go rather than landed, so its
/// patience running out part of the way through the drag does not
/// put the window at its client's old size under the pointer.
#[test]
fn an_edge_drag_takes_a_window_its_change_is_holding() {
    let mut desk = Desk::new("dragged", SCRIPT);
    // The pointer's rectangle is how this test sees which hold has the
    // window; the default draws it at its client's size.
    desk.state.resizing.follow = crate::resizing::Follow::Pointer;
    assert!(desk.state.trigger("super+f"));
    desk.pump();
    desk.answer(1920, 1080);
    desk.land();
    assert!(desk.state.trigger("super+f"));
    desk.pump();
    desk.frame_after(Duration::from_millis(300));
    assert!(
        desk.state.holding_resize(desk.pane),
        "the premise: held for its answer"
    );

    let wanted = Rectangle::new((250, 200).into(), (450, 300).into());
    desk.state.pending_resize = Some(ResizeRequest {
        window: desk.window.clone(),
        wanted,
        edge_at: (250.0, 200.0),
        edges: ResizeEdge::Left,
    });
    desk.state.settle_resize();
    desk.frame_after(crate::resizing::PATIENCE);
    assert_eq!(
        desk.state.pane_outer_of(desk.pane),
        Some(wanted),
        "where the drag has it"
    );
}

/// **A window a mode is presenting is left where the mode draws
/// it**: asking for fullscreen while it is a thumbnail, the client
/// is told its new size and the window goes on being drawn as the
/// thumbnail, rather than gliding out of the grid and landing over
/// it while the mode still holds the input. The mode letting go
/// brings it to the monitor.
#[test]
fn a_window_a_mode_presents_stays_where_the_mode_draws_it() {
    let mut desk = Desk::new(
        "presented",
        &format!(
            "{SCRIPT}\n\
             sol.bind(\"super+o\", function()\n\
                 for _, window in ipairs(sol.windows()) do\n\
                     sol.present(window.id, {{ x = 100, y = 100, w = 200, h = 150 }})\n\
                 end\n\
             end)\n\
             sol.bind(\"super+p\", function()\n\
                 for _, window in ipairs(sol.windows()) do\n\
                     sol.present_clear(window.id)\n\
                 end\n\
             end)\n"
        ),
    );
    let thumbnail = Rectangle::new((100, 100).into(), (200, 150).into()).to_f64();
    assert!(desk.state.trigger("super+o"));
    desk.land();
    assert_eq!(
        desk.drawn(desk.state.clock.now()),
        thumbnail,
        "the premise: presented"
    );

    desk.toplevel.set_fullscreen(None);
    desk.pump();
    assert_eq!(
        last_configured(&desk.client, &desk.toplevel),
        Some((1920, 1080)),
        "the client is told its new size all the same"
    );
    desk.answer(1920, 1080);
    desk.land();
    assert_eq!(
        desk.drawn(desk.state.clock.now()),
        thumbnail,
        "and the window is still drawn where the mode draws it"
    );

    assert!(desk.state.trigger("super+p"));
    desk.land();
    assert_eq!(desk.drawn(desk.state.clock.now()), screen());
    assert!(!desk.transformed());
}

/// **Placing a window where it already is keeps its presentation
/// (#289).** A layout places every window on every pass; one that
/// a script had presented elsewhere (a mobile shell hiding an app
/// at Home, its recents cards) glided back to its own rect at full
/// opacity, as if the script had cleared it. Placed somewhere new,
/// it still glides there.
#[test]
fn placing_a_window_where_it_already_is_keeps_its_presentation() {
    let mut desk = Desk::new(
        "placed-in-place",
        &format!(
            "{SCRIPT}\n\
             sol.bind(\"super+o\", function()\n\
                 for _, window in ipairs(sol.windows()) do\n\
                     sol.present(window.id, {{ x = 100, y = 100, w = 200, h = 150, opacity = 0 }})\n\
                 end\n\
             end)\n\
             sol.bind(\"super+l\", function()\n\
                 for _, window in ipairs(sol.windows()) do\n\
                     sol.place(window.id, {{ x = window.x, y = window.y, w = window.w, h = window.h, tile = false }})\n\
                 end\n\
             end)\n"
        ),
    );
    let thumbnail = Rectangle::new((100, 100).into(), (200, 150).into()).to_f64();
    assert!(desk.state.trigger("super+o"));
    desk.land();
    assert_eq!(
        desk.drawn(desk.state.clock.now()),
        thumbnail,
        "the premise: presented"
    );

    assert!(desk.state.trigger("super+l"));
    desk.land();
    assert_eq!(
        desk.drawn(desk.state.clock.now()),
        thumbnail,
        "placed where it already is, it is still drawn where the script presents it"
    );
    assert!(desk.transformed(), "and the presentation is still held");
}

/// **A `sol.present` a listener makes is replaced by the glide**, as
/// the compositor's move comes after the listeners' commands: only a
/// picture a mode was presenting *before* the change is the mode's.
/// A listener presenting the window it is told about does not keep
/// it at its old size, transformed for good.
#[test]
fn a_present_a_listener_makes_is_replaced_by_the_glide() {
    let mut desk = Desk::new(
        "listener-presents",
        r#"
        sol.on("fullscreen", function(id)
            sol.animate({ duration = 260, easing = "linear" })
            sol.present(id, { opacity = 0.5 })
        end)
        sol.bind("super+f", function() sol.toggle_fullscreen() end)
        "#,
    );
    assert!(desk.state.trigger("super+f"));
    desk.pump();
    let midway = desk.state.clock.now() + Duration::from_millis(100);
    assert!(
        between(desk.drawn(midway), before(), screen()),
        "it glides to the monitor: {:?}",
        desk.drawn(midway)
    );
    desk.answer(1920, 1080);
    desk.land();
    assert_eq!(desk.drawn(desk.state.clock.now()), screen());
    assert!(!desk.transformed(), "and at rest it holds no transform");
}

/// **Turning round part of the way out keeps the way back**, for
/// fullscreen and maximised alike: pressed again before the client
/// has drawn at the size it went back to, what is kept is the
/// rectangle it was told, and not the monitor's size it has not
/// yet left -- which the next way out configured it with, so the
/// window lost its size for good.
#[test]
fn turning_round_part_of_the_way_out_keeps_the_way_back() {
    for key in ["super+f", "super+shift+m"] {
        let mut desk = Desk::new("turned-round", SCRIPT);
        assert!(desk.state.trigger(key));
        desk.pump();
        desk.answer(1920, 1080);
        desk.land();

        assert!(desk.state.trigger(key));
        desk.pump();
        desk.state.clock.advance(Duration::from_millis(100));
        assert!(desk.state.trigger(key));
        desk.pump();
        desk.answer(1920, 1080);
        desk.land();

        assert!(desk.state.trigger(key));
        desk.pump();
        assert_eq!(
            last_configured(&desk.client, &desk.toplevel),
            Some((400, 300)),
            "{key}: the way out is to the size it had"
        );
        desk.answer(400, 300);
        desk.land();
        assert_eq!(desk.drawn(desk.state.clock.now()), before(), "{key}");
    }
}

/// **A reload part of the way through leaves no window transformed**:
/// the glide goes on under the new scripts and is released when it
/// lands, entering and leaving alike.
#[test]
fn a_reload_mid_glide_leaves_the_window_at_rest() {
    let mut desk = Desk::new("reloaded", SCRIPT);
    let directory =
        std::env::temp_dir().join(format!("solium-glides-reload-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    let entry = directory.join("init.lua");
    std::fs::write(&entry, SCRIPT).expect("writing the test script");

    for (size, from, to) in [
        ((1920, 1080), before(), screen()),
        ((400, 300), screen(), before()),
    ] {
        assert!(desk.state.trigger("super+f"));
        desk.pump();
        desk.state.clock.advance(Duration::from_millis(100));
        assert!(
            between(desk.drawn(desk.state.clock.now()), from, to),
            "the premise: part of the way"
        );
        desk.state.reload_from(&entry);
        desk.answer(size.0, size.1);
        desk.land();
        assert!(!desk.transformed(), "at rest once it lands");
        assert_eq!(desk.drawn(desk.state.clock.now()), to);
    }
    let _ = std::fs::remove_dir_all(&directory);
}

/// A window placed where `to` says, as a layout places a tile.
fn place(desk: &mut Desk, to: Rectangle<i32, Logical>, duration: Duration) {
    let id = desk.state.window_id(&desk.window);
    let now = desk.state.clock.now();
    desk.state.place(
        id,
        to_rect(to),
        AnimationSpec {
            duration,
            easing: present::Curve::Linear,
        },
        now,
        Standing::Tile,
    );
    desk.pump();
}

/// **A window placed at a new size is drawn in the rectangle it was
/// placed at until its client answers**, as one a fullscreen change
/// moved is: the same hold, kept from the placement. Without it the
/// window was the size its client last drew, 400x300, in the corner of
/// its new 800x600 rectangle, from the frame after a placement of no
/// length and from the frame a glide landed on, until the client drew:
/// a gap beside it, or, placed smaller, over its neighbour. Answered, it
/// is drawn as it is and nothing holds it.
#[test]
fn a_window_placed_at_a_new_size_is_drawn_there_until_its_client_answers() {
    let to = Rectangle::new((100, 50).into(), (800, 600).into());
    for (name, duration) in [
        ("placed-at-once", Duration::ZERO),
        ("placed-by-a-glide", Duration::from_millis(120)),
    ] {
        let mut desk = Desk::new(name, SCRIPT);
        place(&mut desk, to, duration);
        desk.frame_after(duration + Duration::from_millis(16));
        assert!(
            !desk.transformed(),
            "{name}: arrived, it holds no transform"
        );
        assert_eq!(
            desk.window.geometry().size,
            (400, 300).into(),
            "{name}: the premise: its client has not answered"
        );
        assert_eq!(
            desk.drawn(desk.state.clock.now()),
            to.to_f64(),
            "{name}: drawn in the rectangle it was placed at"
        );
        assert!(
            desk.state.holding_resize(desk.pane),
            "{name}: and held there"
        );
        desk.answer(800, 600);
        desk.frame_after(Duration::from_millis(16));
        assert_eq!(
            desk.drawn(desk.state.clock.now()),
            to.to_f64(),
            "{name}: and once it has answered"
        );
        assert!(
            !desk.state.holding_resize(desk.pane),
            "{name}: and nothing holds it once it has"
        );
    }
}

/// **A window placed where it is, at the size it has, is held by
/// nothing**: a layout places every window on every pass, and most of
/// them have not moved.
#[test]
fn a_window_placed_at_the_size_it_has_is_not_held() {
    let mut desk = Desk::new("placed-as-it-is", SCRIPT);
    let elsewhere = Rectangle::new((700, 400).into(), (400, 300).into());
    place(&mut desk, elsewhere, Duration::ZERO);
    assert!(
        !desk.state.holding_resize(desk.pane),
        "moved and not resized: its client has nothing to answer"
    );
    desk.frame_after(Duration::from_millis(16));
    assert_eq!(desk.drawn(desk.state.clock.now()), elsewhere.to_f64());
}

/// **The window manager is told the size an application took**, when it
/// is not the one the window was placed at: a client that never answers
/// a placement is held in its rectangle for `resize.patience`, then
/// drawn at the size it has, and a listener to `size` hears that size.
/// One that answers with the size it was asked for is told nothing.
#[test]
fn a_window_manager_is_told_the_size_an_application_took() {
    const LISTENS: &str = r#"
        sol.on("size", function(id, w, h) sol.status(w .. "x" .. h) end)
    "#;
    let to = Rectangle::new((100, 50).into(), (800, 600).into());

    let mut desk = Desk::new("took-its-own-size", LISTENS);
    place(&mut desk, to, Duration::ZERO);
    desk.frame_after(Duration::from_millis(16));
    assert_eq!(desk.state.status, "", "held, and nothing told yet");
    desk.frame_after(crate::resizing::PATIENCE);
    assert!(!desk.state.holding_resize(desk.pane), "no longer held");
    assert_eq!(
        desk.drawn(desk.state.clock.now()).size,
        (400.0, 300.0).into(),
        "drawn at the size its client has"
    );
    assert_eq!(
        desk.state.status, "400x300",
        "and the window manager is told"
    );

    let mut desk = Desk::new("took-the-size-it-was-given", LISTENS);
    place(&mut desk, to, Duration::ZERO);
    desk.answer(800, 600);
    desk.frame_after(Duration::from_millis(16));
    desk.frame_after(crate::resizing::PATIENCE);
    assert_eq!(
        desk.state.status, "",
        "an application that drew the size it was given took none of its own"
    );
}

/// **A window manager that listens is told a window's new title**, with
/// the window's row: a title is something an application changes when
/// it likes, and no other event says so.
#[test]
fn a_window_manager_that_listens_is_told_a_windows_new_title() {
    let mut desk = Desk::new(
        "renamed",
        r#"
        sol.on("rename", function(id)
            for _, window in ipairs(sol.windows()) do
                if window.id == id then sol.status(window.title) end
            end
        end)
        "#,
    );
    desk.toplevel.set_title("ready".to_owned());
    desk.pump();
    assert_eq!(desk.state.status, "ready");
}
