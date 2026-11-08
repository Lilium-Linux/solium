//! The compositor's routing to hosted scenes, with a stand-in for each
//! scene for most pointer policy tests. Real Qt touch tests below
//! keep the whole Wayland fixture on Qt's executor thread.

use super::*;
use crate::qml::hosted::PointerKind;
use crate::scripted::{Layer as Scripted, Stand};
use smithay::backend::input::{ButtonState, KeyState};
use wayland_client::Proxy as _;

const SHIFT_L: u32 = 50;
const SUPER_L: u32 = 133;
const BTN_RIGHT: u32 = 0x111;

fn region(desk: &Desk) -> Rectangle<i32, Logical> {
    crate::monitor::union(&desk.state.space).expect("a monitor")
}

fn bar() -> Rectangle<i32, Logical> {
    Rectangle::new((0, 0).into(), (1920, 30).into())
}

/// A desk whose keyboard is `us,ru` with Russian active: every input
/// change is tested with the Cyrillic group active (#132).
fn russian_desk() -> Desk {
    let mut desk = Desk::new();
    russian(&mut desk.state);
    desk
}

/// **A right press on a scene reaches it as the right button, with
/// shift held** (#163).
#[test]
fn a_right_press_on_a_scene_reaches_it_as_the_right_button_with_shift_held() {
    let mut desk = russian_desk();
    let bar = stand_in(&mut desk.state, "bar", Scripted::Top, bar(), Stand::solid());
    move_pointer(&mut desk.state, (100.0, 15.0), 1);
    crate::input::key(&mut desk.state, SHIFT_L.into(), KeyState::Pressed, 2);
    let region = region(&desk);
    crate::synth::send_button(&mut desk.state, region, BTN_RIGHT, ButtonState::Pressed, 3);
    crate::synth::send_button(&mut desk.state, region, BTN_RIGHT, ButtonState::Released, 4);
    let buttons: Vec<(PointerKind, u32, u32)> = scene_events(&desk.state, bar)
        .into_iter()
        .filter(|event| !matches!(event.kind, PointerKind::Motion))
        .map(|event| (event.kind, event.buttons, event.modifiers))
        .collect();
    assert_eq!(
        buttons,
        vec![
            (PointerKind::Press(0x2), 0x2, crate::qml::keys::QT_SHIFT),
            (PointerKind::Release(0x2), 0, crate::qml::keys::QT_SHIFT),
        ],
        "(what, the buttons held after it, the modifiers)"
    );
}

/// **A scene is told when each event happened, on the compositor's
/// clock**, which is what Qt counts a double-click and a
/// `TapHandler`'s taps by: a press a second after the motion
/// before it is told it came a second later.
#[test]
fn a_scene_is_told_when_each_event_happened_on_the_compositors_clock() {
    let mut desk = russian_desk();
    let bar = stand_in(&mut desk.state, "bar", Scripted::Top, bar(), Stand::solid());
    move_pointer(&mut desk.state, (100.0, 15.0), 1);
    desk.state.clock.advance(Duration::from_secs(1));
    let region = region(&desk);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, 2);
    let now = u64::try_from(desk.state.clock.now().as_millis()).unwrap_or(u64::MAX);
    let times: Vec<u64> = scene_events(&desk.state, bar)
        .iter()
        .map(|event| event.time)
        .collect();
    assert!(
        matches!(
            (times.first(), times.last()),
            (Some(motion), Some(press)) if press.saturating_sub(*motion) >= 1000 && *press <= now
        ),
        "the times the scene was told, the last a press a second after the first: \
         {times:?}, now {now}"
    );
}

#[test]
fn the_wheel_over_a_scene_reaches_it() {
    let mut desk = russian_desk();
    let bar = stand_in(&mut desk.state, "bar", Scripted::Top, bar(), Stand::solid());
    move_pointer(&mut desk.state, (100.0, 15.0), 1);
    let region = region(&desk);
    crate::synth::send_axis(&mut desk.state, region, (0.0, 120.0), 2);
    assert!(
        scene_events(&desk.state, bar).iter().any(|event| event.kind
            == PointerKind::Wheel {
                angle: (0.0, -120.0),
                pixels: (0.0, 0.0),
            }),
        "a notch down is a negative angle in Qt's terms: {:?}",
        scene_events(&desk.state, bar)
    );
}

/// **A touchpad scroll reaches a scene in pixels, and its end does
/// not** (Ruling 9): three units of two fingers moving are an
/// `angleDelta` of 24 and a `pixelDelta` of 3, both towards the
/// user; the fingers lifting move nothing, and a scene is not told
/// of that as a wheel turn, which its handlers would read as one
/// the other way.
#[test]
fn a_touchpad_scroll_reaches_a_scene_in_pixels_and_its_end_does_not() {
    let mut desk = russian_desk();
    let bar = stand_in(&mut desk.state, "bar", Scripted::Top, bar(), Stand::solid());
    move_pointer(&mut desk.state, (100.0, 15.0), 1);
    let region = region(&desk);
    crate::synth::send_finger_scroll(&mut desk.state, region, (0.0, 3.0), 2);
    crate::synth::send_finger_scroll(&mut desk.state, region, (0.0, 0.0), 3);
    let wheels: Vec<PointerKind> = scene_events(&desk.state, bar)
        .into_iter()
        .map(|event| event.kind)
        .filter(|kind| matches!(kind, PointerKind::Wheel { .. }))
        .collect();
    assert_eq!(
        wheels,
        vec![PointerKind::Wheel {
            angle: (0.0, -24.0),
            pixels: (0.0, -3.0),
        }],
        "the scroll, and nothing for the fingers lifting"
    );
}

/// **`super` and the wheel stay the compositor's over a scene**: a
/// scrolling layout's viewport is not a shell's to take.
#[test]
fn super_and_the_wheel_stay_the_compositors_over_a_scene() {
    let mut desk = russian_desk();
    desk.install(r#"sol.on("scroll", function(dx, dy) sol.status("scrolled") end)"#);
    let bar = stand_in(&mut desk.state, "bar", Scripted::Top, bar(), Stand::solid());
    move_pointer(&mut desk.state, (100.0, 15.0), 1);
    crate::input::key(&mut desk.state, SUPER_L.into(), KeyState::Pressed, 2);
    let region = region(&desk);
    crate::synth::send_axis(&mut desk.state, region, (0.0, 120.0), 3);
    assert_eq!(desk.state.status, "scrolled");
    assert!(
        !scene_events(&desk.state, bar)
            .iter()
            .any(|event| matches!(event.kind, PointerKind::Wheel { .. }))
    );
}

/// **A device whose natural scroll libinput already set is not
/// flipped a second time by the software fallback** (#157's own
/// central risk): with `profile.natural_scroll` true -- the
/// setting that used to flip every device uniformly -- and the
/// synthetic device registered as handled, the wheel reaches the
/// scene exactly as unflipped, proving `pointer_axis`'s own
/// `Registry::handled` check actually gates the flip, not just
/// the unit-tested `Registry::handled` in isolation.
#[test]
fn pointer_axis_does_not_flip_a_device_whose_natural_scroll_libinput_already_set() {
    let mut desk = russian_desk();
    desk.state.profile.natural_scroll = true;
    desk.state.input.device_seen(
        crate::input::devices::DeviceInfo {
            id: "synthetic".to_owned(),
            name: "synthetic pointer".to_owned(),
            kind: crate::input::devices::DeviceKind::Mouse,
            vendor: None,
            product: None,
            output: None,
        },
        crate::input::devices::Report::test_applied(&["natural_scroll"]),
    );
    let bar = stand_in(&mut desk.state, "bar", Scripted::Top, bar(), Stand::solid());
    move_pointer(&mut desk.state, (100.0, 15.0), 1);
    let region = region(&desk);
    crate::synth::send_axis(&mut desk.state, region, (0.0, 120.0), 2);
    assert!(
        scene_events(&desk.state, bar).iter().any(|event| event.kind
            == PointerKind::Wheel {
                angle: (0.0, -120.0),
                pixels: (0.0, 0.0),
            }),
        "libinput already flipped this device's raw deltas, so the software \
         fallback must leave the direction alone even though the profile asks for \
         natural scroll: {:?}",
        scene_events(&desk.state, bar)
    );
}

/// **A device the natural-scroll mechanism never touched still
/// gets the old software flip** when the profile asks for it --
/// the other half of the same branch: with nothing registered,
/// `Registry::handled` answers false, and `pointer_axis` must
/// fall back to `profile.natural_scroll` exactly as it did
/// before #157.
#[test]
fn pointer_axis_still_flips_a_device_its_natural_scroll_mechanism_never_touched() {
    let mut desk = russian_desk();
    desk.state.profile.natural_scroll = true;
    let bar = stand_in(&mut desk.state, "bar", Scripted::Top, bar(), Stand::solid());
    move_pointer(&mut desk.state, (100.0, 15.0), 1);
    let region = region(&desk);
    crate::synth::send_axis(&mut desk.state, region, (0.0, 120.0), 2);
    assert!(
        scene_events(&desk.state, bar).iter().any(|event| event.kind
            == PointerKind::Wheel {
                angle: (0.0, 120.0),
                pixels: (0.0, 0.0),
            }),
        "no device claimed this natural scroll, so the old blanket flip must still \
         apply: {:?}",
        scene_events(&desk.state, bar)
    );
}

/// A button over 600..664 x 500..540 of the screen, nothing elsewhere.
fn button_over_the_window(at: Point<f64, Logical>) -> crate::qml::hosted::Hit {
    if (600.0..664.0).contains(&at.x) && (500.0..540.0).contains(&at.y) {
        crate::qml::hosted::Hit::Press
    } else {
        crate::qml::hosted::Hit::Nothing
    }
}

/// A panel over the window, rounded by 20 at its top-left corner.
fn rounded_over_the_window(at: Point<f64, Logical>) -> crate::qml::hosted::Hit {
    let (dx, dy) = (20.0 - (at.x - 600.0), 20.0 - (at.y - 500.0));
    let in_corner = dx > 0.0 && dy > 0.0 && dx * dx + dy * dy > 400.0;
    if (600.0..664.0).contains(&at.x) && (500.0..564.0).contains(&at.y) && !in_corner {
        crate::qml::hosted::Hit::Press
    } else {
        crate::qml::hosted::Hit::Nothing
    }
}

fn hover_everywhere(_: Point<f64, Logical>) -> crate::qml::hosted::Hit {
    crate::qml::hosted::Hit::Hover
}

fn screen_wide() -> Rectangle<i32, Logical> {
    Rectangle::new((0, 0).into(), (1920, 1080).into())
}

/// A window at 600,500, and a full-screen scene over it.
fn window_under_a_scene(
    hit: fn(Point<f64, Logical>) -> crate::qml::hosted::Hit,
) -> (Desk, Opened, crate::scripted::SurfaceId) {
    let mut desk = russian_desk();
    let opened = desk.open_surface();
    let window = desk
        .state
        .panes
        .get(opened.pane)
        .and_then(crate::pane::Pane::client)
        .cloned()
        .expect("a client");
    desk.state.space.map_element(window, (600, 500), false);
    desk.state.space.refresh();
    desk.state.clock.advance(Duration::from_secs(1));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    desk.pump();
    let scene = stand_in(
        &mut desk.state,
        "shell",
        Scripted::Top,
        screen_wide(),
        Stand {
            hit,
            ..Stand::solid()
        },
    );
    (desk, opened, scene)
}

/// The surface the pointer is on, by its protocol id.
fn pointer_on(desk: &Desk) -> Option<u32> {
    desk.state
        .surface_under(
            desk.state
                .seat
                .get_pointer()
                .map(|pointer| pointer.current_location())
                .unwrap_or_default(),
        )
        .map(|(surface, _)| surface.id().protocol_id())
}

fn window_id(opened: &Opened) -> u32 {
    wayland_client::Proxy::id(&opened.surface).protocol_id()
}

fn presses(desk: &Desk, id: crate::scripted::SurfaceId) -> usize {
    scene_events(&desk.state, id)
        .iter()
        .filter(|event| matches!(event.kind, PointerKind::Press(_)))
        .count()
}

fn click(desk: &mut Desk, at: (f64, f64), time: u64) {
    move_pointer(&mut desk.state, at, time);
    let region = region(desk);
    crate::synth::send_button(
        &mut desk.state,
        region,
        0x110,
        ButtonState::Pressed,
        time + 1,
    );
    crate::synth::send_button(
        &mut desk.state,
        region,
        0x110,
        ButtonState::Released,
        time + 2,
    );
}

/// **#173: a press where the shell draws nothing reaches the
/// window under it.**
#[test]
fn a_press_where_the_shell_draws_nothing_reaches_the_window_under_it() {
    let (mut desk, opened, shell) = window_under_a_scene(button_over_the_window);
    click(&mut desk, (630.0, 555.0), 10);
    assert_eq!(
        pointer_on(&desk),
        Some(window_id(&opened)),
        "the window does not have the pointer"
    );
    assert_eq!(
        presses(&desk, shell),
        0,
        "the shell took a press where it draws nothing"
    );
}

/// **The wheel where the shell draws nothing is the window's**: the
/// scene is not told of it, and the window under it has the
/// pointer the wheel goes to.
#[test]
fn the_wheel_where_the_shell_draws_nothing_is_the_windows_under_it() {
    let (mut desk, opened, shell) = window_under_a_scene(button_over_the_window);
    move_pointer(&mut desk.state, (630.0, 555.0), 10);
    let region = region(&desk);
    crate::synth::send_axis(&mut desk.state, region, (0.0, 120.0), 11);
    let wheels = scene_events(&desk.state, shell)
        .iter()
        .filter(|event| matches!(event.kind, PointerKind::Wheel { .. }))
        .count();
    assert_eq!(
        (wheels, pointer_on(&desk)),
        (0, Some(window_id(&opened))),
        "(the wheel turns the shell was told, the surface the pointer is on)"
    );
}

/// **#173: a press on a shell button does not reach the window.**
#[test]
fn a_press_on_a_shell_button_does_not_reach_the_window() {
    let (mut desk, _, shell) = window_under_a_scene(button_over_the_window);
    click(&mut desk, (630.0, 520.0), 10);
    assert_eq!(
        pointer_on(&desk),
        None,
        "a window under a shell button has the pointer (Ruling 8)"
    );
    assert_eq!(presses(&desk, shell), 1);
}

/// **#173: a press on a rounded corner's transparent part falls
/// through.**
#[test]
fn a_press_on_a_rounded_corners_transparent_part_falls_through() {
    let (mut desk, opened, shell) = window_under_a_scene(rounded_over_the_window);
    click(&mut desk, (601.0, 501.0), 10);
    assert_eq!(pointer_on(&desk), Some(window_id(&opened)));
    assert_eq!(presses(&desk, shell), 0);
    click(&mut desk, (630.0, 530.0), 20);
    assert_eq!(
        presses(&desk, shell),
        1,
        "inside the rounded shape the press is the shell's"
    );
}

/// **A hover strip hears the pointer and leaves the window its
/// press** (Ruling 8).
#[test]
fn a_hover_strip_hears_the_motion_and_leaves_the_window_its_press() {
    let (mut desk, opened, shell) = window_under_a_scene(hover_everywhere);
    click(&mut desk, (630.0, 530.0), 10);
    assert!(
        scene_events(&desk.state, shell)
            .iter()
            .any(|event| event.kind == PointerKind::Motion),
        "the strip heard no motion"
    );
    assert_eq!(presses(&desk, shell), 0, "a hover strip took a press");
    assert_eq!(pointer_on(&desk), Some(window_id(&opened)));
}

/// **A press a scene took holds the pointer until the buttons are
/// up** (Ruling 7): dragged off the button onto the window, the
/// pointer is still nobody else's, and the release reaches the
/// scene.
#[test]
fn a_release_after_dragging_off_a_shell_button_reaches_the_scene() {
    let (mut desk, _, shell) = window_under_a_scene(button_over_the_window);
    move_pointer(&mut desk.state, (630.0, 520.0), 10);
    let region = region(&desk);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, 11);
    move_pointer(&mut desk.state, (630.0, 555.0), 12);
    assert_eq!(
        pointer_on(&desk),
        None,
        "the window under the drag has the pointer while the scene holds it"
    );
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Released, 13);
    assert!(
        scene_events(&desk.state, shell)
            .iter()
            .any(|event| matches!(event.kind, PointerKind::Release(0x1))),
        "the release went elsewhere"
    );
    assert!(
        desk.state.scene_press.is_none(),
        "the hold outlived the buttons"
    );
}

/// **Focus follows the pointer through a shell only where the
/// shell takes no press** (Ruling 8, as under a client's bar): on
/// a shell button over a window, and dragging off it, the pointer
/// is the shell's and the window under it does not take the
/// keyboard; off the button it does.
#[test]
fn focus_follows_the_mouse_through_a_shell_only_where_it_takes_no_press() {
    let (mut desk, opened, _) = window_under_a_scene(button_over_the_window);
    let other = desk.open_surface();
    let other_window = desk
        .state
        .panes
        .get(other.pane)
        .and_then(crate::pane::Pane::client)
        .cloned()
        .expect("a client");
    desk.state
        .space
        .map_element(other_window.clone(), (100, 100), false);
    desk.state.space.refresh();
    desk.state.focus_window(
        &other_window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    desk.state.profile.focus_follows_mouse = true;

    move_pointer(&mut desk.state, (630.0, 520.0), 10);
    let on_the_button = desk.focused();
    let region = region(&desk);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, 11);
    move_pointer(&mut desk.state, (630.0, 555.0), 12);
    let dragged_off = desk.focused();
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Released, 13);
    move_pointer(&mut desk.state, (630.0, 556.0), 14);
    assert_eq!(
        (on_the_button, dragged_off, desk.focused()),
        (other.pane, other.pane, opened.pane),
        "(the window with the keyboard with the pointer on the shell button, \
         dragged off it onto the window, and moved on the window after the release)"
    );
}

#[test]
fn a_real_touch_button_clicks_without_reaching_or_focusing_its_client() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let mut desk = Desk::new();
        let opened = desk.open_surface();
        let window = desk
            .state
            .panes
            .get(opened.pane)
            .and_then(crate::pane::Pane::client)
            .cloned()
            .expect("a client");
        desk.state.space.map_element(window, (600, 500), false);
        let other = desk.open_surface();
        let other_window = desk
            .state
            .panes
            .get(other.pane)
            .and_then(crate::pane::Pane::client)
            .cloned()
            .expect("another client");
        desk.state
            .space
            .map_element(other_window.clone(), (100, 100), false);
        desk.state.space.refresh();
        desk.state.clock.advance(Duration::from_secs(1));
        let now = desk.state.clock.now();
        desk.state.settle(now);
        desk.state.focus_window(
            &other_window,
            SERIAL_COUNTER.next_serial(),
            crate::script::FocusSource::Script,
        );
        desk.install("require(\"touch\")");
        let directory = std::env::temp_dir().join("solium-real-touch-button");
        std::fs::create_dir_all(&directory).expect("scene directory");
        let path = directory.join("Scene.qml");
        std::fs::write(
            &path,
            r"import QtQuick
            Item {
                property int clicks: 0
                MouseArea { anchors.fill: parent; onClicked: parent.clicks += 1 }
            }",
        )
        .expect("scene source");
        desk.state
            .declare_surface(crate::scripted::Declaration::for_test(
                "touch-button",
                path,
                Scripted::Top,
                crate::scripted::On::Rect(Rectangle::new((600, 500).into(), (64, 64).into())),
            ));
        let id = desk.state.surfaces.named("touch-button").expect("declared");
        let output = desk.state.space.outputs().next().cloned().expect("monitor");
        desk.state
            .surfaces
            .get_mut(id)
            .and_then(|surface| surface.instance_mut(&output))
            .expect("real scene on the Qt thread")
            .scene_for_test()
            .resize(64, 64, 1.0);
        assert_eq!(
            desk.state.surfaces.get(id).expect("scene").hit(
                &output,
                Rectangle::new((600, 500).into(), (64, 64).into()),
                (630.0, 520.0).into()
            ),
            crate::qml::hosted::Hit::Press,
            "the rendered button owns the test point"
        );
        let _touch = desk.client.seat_touch(&desk.qh);
        desk.pump();
        let region = region(&desk);
        crate::synth::send_touch(&mut desk.state, region, (630.0, 520.0), 10_000);
        desk.pump();
        let clicks = desk
            .state
            .surfaces
            .get_mut(id)
            .and_then(|surface| surface.instance_mut(&output))
            .expect("real scene on the Qt thread")
            .scene_for_test()
            .get_int("clicks");
        assert_eq!(clicks, 1);
        assert!(
            desk.client.touches.is_empty(),
            "the client under the scene gets no finger"
        );
        assert_eq!(desk.focused(), other.pane);
        drop(desk);
        let _ = std::fs::remove_dir_all(directory);
    });
}

#[test]
fn a_real_touch_on_the_shipped_frames_close_button_closes_its_client() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let mut desk = Desk::new();
        let opened = desk.open_surface();
        desk.state
            .decorations
            .set_style(&mut desk.state.panes, Some("top".into()));
        desk.state
            .decorations
            .insert(&mut desk.state.panes, opened.pane, 64, 96);
        let values = crate::decoration::Values::default();
        desk.state
            .panes
            .get_mut(opened.pane)
            .and_then(Pane::decoration_mut)
            .expect("shipped frame")
            .tell_as_a_frame_would(
                &crate::decoration::Look {
                    title: "touch",
                    app_id: "",
                    focused: true,
                    pointer_inside: false,
                    caret: None,
                    values: &values,
                    window: None,
                    window_values: crate::decoration::Values::none(),
                },
                64,
                96,
            );
        // A Row places the shipped buttons during polish, as on
        // the first real frame, before any hardware can touch it.
        let layers = desk
            .state
            .panes
            .get_mut(opened.pane)
            .and_then(Pane::decoration_mut)
            .expect("frame")
            .rendered_layers();
        assert!(!layers.is_empty());
        desk.state.clock.advance(Duration::from_secs(1));
        let now = desk.state.clock.now();
        desk.state.settle(now);
        let outer = desk.state.pane_outer_of(opened.pane).expect("outer");
        let at = (
            f64::from(outer.loc.x + outer.size.w) - 18.5,
            f64::from(outer.loc.y) + 16.0,
        );
        assert!(
            matches!(desk.state.chrome_under(at.into()), Some(under) if matches!(under.chrome, crate::state::Chrome::Frame))
        );
        let region = region(&desk);
        crate::synth::send_touch(&mut desk.state, region, at, 10);
        desk.ask();
        assert!(desk.client.closes.contains(&opened.toplevel.id()));
    });
}

#[test]
fn locking_cancels_fingers_already_held_on_a_real_scene_and_client() {
    with_real_touch_scene(
        "lock-held",
        r"import QtQuick
        Item {
            property bool held: finger.pressed
            property int taps: 0
            TapHandler { id: finger; onTapped: parent.taps += 1 }
        }",
        |desk, id, output| {
            let region = region(desk);
            crate::synth::touch_down(&mut desk.state, region, 0, (620.0, 520.0), 10);
            crate::synth::touch_down(&mut desk.state, region, 1, (720.0, 520.0), 11);
            crate::synth::touch_frame(&mut desk.state, region);
            desk.pump();
            assert!(
                desk.state
                    .surfaces
                    .get_mut(id)
                    .and_then(|s| s.instance_mut(output))
                    .expect("scene")
                    .scene_for_test()
                    .get_bool("held")
            );
            let _lock = desk
                .client
                .locks
                .as_ref()
                .expect("manager")
                .lock(&desk.qh, ());
            desk.pump();
            let scene = desk
                .state
                .surfaces
                .get_mut(id)
                .and_then(|s| s.instance_mut(output))
                .expect("scene")
                .scene_for_test();
            assert!(
                !scene.get_bool("held"),
                "lock cancels the scene finger immediately"
            );
            assert_eq!(scene.get_int("taps"), 0, "cancel is never a tap");
            assert!(
                desk.client
                    .touches
                    .iter()
                    .any(|event| matches!(event, wl_touch::Event::Cancel)),
                "the client finger is cancelled too"
            );
            crate::synth::touch_up(&mut desk.state, region, 0, 20);
            crate::synth::touch_up(&mut desk.state, region, 1, 21);
            crate::synth::touch_frame(&mut desk.state, region);
            desk.pump();
            assert!(
                !desk
                    .client
                    .touches
                    .iter()
                    .any(|event| matches!(event, wl_touch::Event::Up { .. })),
                "pre-lock fingers never release into another target"
            );
        },
    );
}

/// **#281: a finger at Solium's own lock reaches its scene, as Qt
/// touch events, and nothing else**: not the window under it, not
/// a scene of the session's over it, and not `sol.on("touch")`,
/// which stays silent while locked. Each of the three is shown to
/// hear the same finger first, unlocked.
#[test]
fn a_finger_at_the_native_lock_reaches_its_scene_and_nothing_else() {
    const TAPS: &str = r"import QtQuick
        Item {
            property int taps: 0
            TapHandler { onTapped: parent.taps += 1 }
        }";
    with_real_touch_scene("native-lock-touch", TAPS, |desk, id, output| {
        desk.install(
            r#"touches = 0
            sol.on("touch", function() touches = touches + 1 end)"#,
        );
        let region = region(desk);
        let (over_scene, over_window) = ((620.0, 520.0), (720.0, 520.0));
        let scene_taps = |desk: &mut Desk| {
            desk.state
                .surfaces
                .get_mut(id)
                .and_then(|s| s.instance_mut(output))
                .expect("the session's scene")
                .scene_for_test()
                .get_int("taps")
        };
        let downs = |desk: &Desk| {
            desk.client
                .touches
                .iter()
                .filter(|event| matches!(event, wl_touch::Event::Down { .. }))
                .count()
        };
        let heard = |desk: &Desk| {
            desk.state
                .scripts
                .as_ref()
                .map(|scripts| scripts.evaluate("return tostring(touches)"))
                .unwrap_or_default()
        };

        crate::synth::send_touch(&mut desk.state, region, over_scene, 10);
        crate::synth::send_touch(&mut desk.state, region, over_window, 20);
        desk.pump();
        assert_eq!(
            scene_taps(desk),
            1,
            "the premise: the session's scene hears a finger"
        );
        assert_eq!(downs(desk), 1, "the premise: the window hears a finger");
        let before = heard(desk);
        assert_ne!(
            before, "0",
            "the premise: `sol.on(\"touch\")` hears a finger"
        );

        // In a directory of its own: a scene's reload watch is
        // its whole directory, so one loose in the temporary
        // directory would be another test's edit.
        let lock_directory =
            std::env::temp_dir().join(format!("solium-native-lock-touch-{}", std::process::id()));
        std::fs::create_dir_all(&lock_directory).expect("a directory for the scene");
        let lock_scene = lock_directory.join("Lock.qml");
        std::fs::write(&lock_scene, TAPS).expect("writing the lock scene");
        desk.state.lock_scenes.use_scene(lock_scene);
        desk.state.lock_natively();
        desk.state
            .lock_scenes
            .instance_for_test(output)
            .expect("the lock scene")
            .scene_for_test()
            .resize(1920, 1080, 1.0);
        desk.pump();
        desk.client.touches.clear();

        crate::synth::send_touch(&mut desk.state, region, over_scene, 30);
        crate::synth::send_touch(&mut desk.state, region, over_window, 40);
        desk.pump();

        let lock_taps = desk
            .state
            .lock_scenes
            .instance_for_test(output)
            .expect("the lock scene")
            .scene_for_test()
            .get_int("taps");
        assert_eq!(
            lock_taps, 2,
            "the native lock's scene did not hear both fingers"
        );
        assert_eq!(
            scene_taps(desk),
            1,
            "a scene of the session heard a finger at the lock"
        );
        assert_eq!(downs(desk), 0, "a window behind the lock heard a finger");
        assert_eq!(
            heard(desk),
            before,
            "`sol.on(\"touch\")` heard a finger at the lock"
        );

        desk.state.unlock_natively();
        assert!(
            desk.state.lock.is_none(),
            "the native unlock did not unlock"
        );
        let _ = std::fs::remove_dir_all(lock_directory);
    });
}

/// **#281: a press at Solium's own lock reaches its scene, as a Qt
/// mouse event, and nothing else**: not the window under it and
/// not a scene of the session's over it. Both are shown to hear
/// the same press first, unlocked, and the lock's scene hears
/// the pointer move as well as press.
#[test]
fn a_press_at_the_native_lock_reaches_its_scene_and_nothing_else() {
    const PRESSES: &str = r"import QtQuick
        Item {
            property int presses: 0
            property int moves: 0
            MouseArea {
                anchors.fill: parent
                hoverEnabled: true
                onPressed: parent.presses += 1
                onPositionChanged: parent.moves += 1
            }
        }";
    with_real_touch_scene("native-lock-pointer", PRESSES, |desk, id, output| {
        let _pointer = desk.client.seat_pointer(&desk.qh);
        desk.pump();
        let (over_scene, over_window) = ((620.0, 520.0), (720.0, 520.0));
        let click = |desk: &mut Desk, at: (f64, f64), time: u64| {
            move_pointer(&mut desk.state, (at.0 - 5.0, at.1), time);
            move_pointer(&mut desk.state, at, time + 1);
            let region = region(desk);
            for (state, offset) in [(ButtonState::Pressed, 2), (ButtonState::Released, 3)] {
                crate::synth::send_button(&mut desk.state, region, 0x110, state, time + offset);
            }
            desk.pump();
        };
        let scene_presses = |desk: &mut Desk| {
            desk.state
                .surfaces
                .get_mut(id)
                .and_then(|s| s.instance_mut(output))
                .expect("the session's scene")
                .scene_for_test()
                .get_int("presses")
        };

        click(desk, over_scene, 10);
        click(desk, over_window, 20);
        assert_eq!(
            scene_presses(desk),
            1,
            "the premise: the session's scene hears a press"
        );
        assert!(
            desk.client.buttons.iter().any(|(_, pressed)| *pressed),
            "the premise: the window hears a press"
        );

        // A directory of its own: a scene file loose in the
        // temporary directory shares its implicit import with
        // whatever other tests left there.
        let lock_directory =
            std::env::temp_dir().join(format!("solium-native-lock-pointer-{}", std::process::id()));
        std::fs::create_dir_all(&lock_directory).expect("a directory for the lock scene");
        let lock_scene = lock_directory.join("Lock.qml");
        std::fs::write(&lock_scene, PRESSES).expect("writing the lock scene");
        desk.state.lock_scenes.use_scene(lock_scene);
        desk.state.lock_natively();
        desk.state
            .lock_scenes
            .instance_for_test(output)
            .expect("the lock scene")
            .scene_for_test()
            .resize(1920, 1080, 1.0);
        desk.pump();
        desk.client.buttons.clear();

        click(desk, over_scene, 30);
        click(desk, over_window, 40);

        let lock = |desk: &mut Desk, name: &str| {
            desk.state
                .lock_scenes
                .instance_for_test(output)
                .expect("the lock scene")
                .scene_for_test()
                .get_int(name)
        };
        assert_eq!(
            desk.state.lock_scenes.shown_on(output),
            crate::lock_scene::Shown::Own,
            "the premise: the lock shows its scene"
        );
        assert_eq!(
            lock(desk, "presses"),
            2,
            "the native lock's scene did not hear both presses"
        );
        assert!(
            lock(desk, "moves") > 0,
            "the native lock's scene did not hear the pointer move"
        );
        assert_eq!(
            scene_presses(desk),
            1,
            "a scene of the session heard a press at the lock"
        );
        assert!(
            desk.client.buttons.is_empty(),
            "a window behind the lock heard a press"
        );

        desk.state.unlock_natively();
        assert!(
            desk.state.lock.is_none(),
            "the native unlock did not unlock"
        );
        let _ = std::fs::remove_dir_all(lock_directory);
    });
}

fn with_touch_window(
    framed: bool,
    run: impl FnOnce(&mut Desk, &Opened, Window, Rectangle<i32, Logical>) + Send + 'static,
) {
    crate::qml::qt_test::on_the_qt_thread(move || {
        crate::qml::start().expect("Qt starts");
        let mut desk = Desk::new();
        let opened = desk.open_surface();
        let window = desk
            .state
            .panes
            .get(opened.pane)
            .and_then(Pane::client)
            .cloned()
            .expect("client");
        let _buffer =
            super::super::commit_buffer(&desk.client, &desk.qh, &opened.surface, 400, 300);
        desk.pump();
        desk.state
            .space
            .map_element(window.clone(), (100, 100), false);
        desk.state.space.refresh();
        if framed {
            desk.state
                .decorations
                .set_style(&mut desk.state.panes, Some("top".into()));
            desk.state
                .decorations
                .insert(&mut desk.state.panes, opened.pane, 400, 300);
            let values = crate::decoration::Values::default();
            let decoration = desk
                .state
                .panes
                .get_mut(opened.pane)
                .and_then(Pane::decoration_mut)
                .expect("shipped frame");
            decoration.tell_as_a_frame_would(
                &crate::decoration::Look {
                    title: "touch",
                    app_id: "",
                    focused: true,
                    pointer_inside: false,
                    caret: None,
                    values: &values,
                    window: None,
                    window_values: crate::decoration::Values::none(),
                },
                400,
                332,
            );
            assert!(!decoration.rendered_layers().is_empty());
        }
        desk.state.clock.advance(Duration::from_secs(1));
        let now = desk.state.clock.now();
        desk.state.settle(now);
        let before = desk.state.pane_outer_of(opened.pane).expect("outer");
        desk.install("require(\"touch\")");
        let _touch = desk.client.seat_touch(&desk.qh);
        desk.pump();
        // These measure the rectangle the finger asks for before
        // the client answers, which is what a window is drawn at
        // under `resize.follow = "pointer"`; the default draws it
        // at its client's size.
        desk.state.resizing.follow = crate::resizing::Follow::Pointer;
        run(&mut desk, &opened, window, before);
    });
}

#[test]
fn a_finger_on_a_border_resizes_the_window_through_client_commit() {
    with_touch_window(false, |desk, opened, _, before| {
        let region = region(desk);
        let at = (
            f64::from(before.loc.x + before.size.w - 1),
            f64::from(before.loc.y + before.size.h / 2),
        );
        assert!(
            matches!(desk.state.chrome_under(at.into()), Some(under) if matches!(under.chrome, crate::state::Chrome::Resize(_)))
        );
        crate::synth::touch_down(&mut desk.state, region, 0, at, 10_000);
        crate::synth::touch_motion(&mut desk.state, region, 0, (at.0 + 100.0, at.1), 20_000);
        crate::synth::touch_up(&mut desk.state, region, 0, 30_000);
        desk.state.settle_resize();
        let wanted = Rectangle::new(before.loc, (before.size.w + 100, before.size.h).into());
        assert_eq!(
            desk.state.pane_outer_of(opened.pane),
            Some(wanted),
            "finger moves the held outer rectangle before the client answers"
        );
        let _buffer = super::super::commit_buffer(
            &desk.client,
            &desk.qh,
            &opened.surface,
            wanted.size.w,
            wanted.size.h,
        );
        desk.pump();
        desk.state.settle_resize();
        assert_eq!(desk.state.pane_outer_of(opened.pane), Some(wanted));
        assert!(
            desk.state.resize_hold.is_none(),
            "the client's answer reconciles the hold"
        );
    });
}

#[test]
fn a_finger_moves_a_titlebar_while_the_pointer_hovers_a_button() {
    with_touch_window(true, |desk, opened, window, before| {
        let region = region(desk);
        move_pointer(
            &mut desk.state,
            (
                f64::from(before.loc.x + before.size.w) - 18.5,
                f64::from(before.loc.y) + 16.0,
            ),
            10_000,
        );
        let at = (
            f64::from(before.loc.x) + 100.0,
            f64::from(before.loc.y) + 16.0,
        );
        let origin = desk
            .state
            .space
            .element_location(&window)
            .expect("window origin");
        crate::synth::touch_down(&mut desk.state, region, 0, at, 20_000);
        crate::synth::touch_motion(
            &mut desk.state,
            region,
            0,
            (at.0 + 100.0, at.1 + 50.0),
            30_000,
        );
        crate::synth::touch_up(&mut desk.state, region, 0, 40_000);
        assert_eq!(
            desk.state.space.element_location(&window),
            Some(origin + Point::from((100, 50)))
        );
        assert!(!desk.state.seat.get_pointer().expect("pointer").is_grabbed());
        desk.ask();
        assert!(!desk.client.closes.contains(&opened.toplevel.id()));
    });
}

/// **#452: a finger on a maximised window's titlebar holds it
/// as the pointer does**: a few pixels leave it maximised where
/// it is, and past `maximize.restore_after` it comes off at its
/// old size under the finger and follows it.
#[test]
fn a_finger_drags_a_maximised_window_off_its_maximise() {
    with_touch_window(true, |desk, opened, window, before| {
        let region = region(desk);
        desk.install("require(\"touch\")\nrequire(\"floating\")");
        desk.state.toggle_maximize(&window);
        desk.pump();
        let (w, h) = last_configured(&desk.client, &opened.toplevel)
            .expect("the window was told the work area");
        let _buffer = super::super::commit_buffer(&desk.client, &desk.qh, &opened.surface, w, h);
        desk.pump();
        let maximised = desk.state.pane_outer_of(opened.pane).expect("outer");
        // A twentieth of the way along: clear of the title, which
        // is a control a finger does not drag by.
        let at = maximised.loc + Point::from((maximised.size.w / 20, 16));
        let at = (f64::from(at.x), f64::from(at.y));
        crate::synth::touch_down(&mut desk.state, region, 0, at, 20_000);
        crate::synth::touch_motion(&mut desk.state, region, 0, (at.0 + 3.0, at.1), 30_000);
        crate::input::grab::settle_drags(&mut desk.state, false);
        assert_eq!(
            (
                crate::state::window_states(&window).maximized,
                desk.state.pane_outer_of(opened.pane)
            ),
            (true, Some(maximised)),
            "three pixels moved or restored it"
        );
        let to = (at.0 + 50.0, at.1 + 80.0);
        crate::synth::touch_motion(&mut desk.state, region, 0, to, 40_000);
        crate::input::grab::settle_drags(&mut desk.state, false);
        assert!(
            !crate::state::window_states(&window).maximized,
            "the finger did not take it off"
        );
        let off = desk.state.real_geometry(&window).expect("mapped");
        let outer = desk.state.pane_outer_of(opened.pane).expect("outer");
        // Placed for its old width, which its client has not
        // drawn yet.
        assert_eq!(
            (f64::from(outer.loc.x), f64::from(outer.loc.y)),
            (
                (to.0 - f64::from(before.size.w) / 20.0).round(),
                to.1 - 16.0
            ),
            "it did not come off under the finger"
        );
        crate::synth::touch_motion(
            &mut desk.state,
            region,
            0,
            (to.0 + 100.0, to.1 + 50.0),
            50_000,
        );
        crate::input::grab::settle_drags(&mut desk.state, false);
        crate::synth::touch_up(&mut desk.state, region, 0, 60_000);
        assert_eq!(
            desk.state.real_geometry(&window).map(|real| real.loc),
            Some(off.loc + Point::from((100, 50))),
            "it did not follow the finger"
        );
    });
}

/// Whether the pointer shows the closed hand a move holds.
fn closed_hand(desk: &mut Desk) -> bool {
    use smithay::input::pointer::{CursorIcon, CursorImageStatus};
    matches!(
        desk.state.pointer.showing(),
        CursorImageStatus::Named(CursorIcon::Grabbing)
    )
}

/// **A window moved by its titlebar shows the closed hand for as
/// long as the drag lasts, and the titlebar's arrow after**
/// (#374): the press starts the move with the hand, the motion
/// keeps it, frame after frame, and the first frame after the
/// release asks again what is under the pointer.
#[test]
fn a_titlebar_move_shows_the_closed_hand_until_it_ends() {
    with_touch_window(true, |desk, _, window, before| {
        use smithay::input::pointer::{CursorIcon, CursorImageStatus};
        let region = region(desk);
        let at = (
            f64::from(before.loc.x) + 100.0,
            f64::from(before.loc.y) + 16.0,
        );
        move_pointer(&mut desk.state, at, 10_000);
        let origin = desk
            .state
            .space
            .element_location(&window)
            .expect("window origin");
        let before_the_press = closed_hand(desk);
        crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, 11_000);
        let pressed = closed_hand(desk);
        move_pointer(&mut desk.state, (at.0 + 100.0, at.1 + 50.0), 12_000);
        desk.state.reassert_cursor();
        let dragging = closed_hand(desk);
        let moved = desk.state.space.element_location(&window);
        crate::synth::send_button(
            &mut desk.state,
            region,
            0x110,
            ButtonState::Released,
            13_000,
        );
        desk.state.reassert_cursor();
        let after = desk.state.pointer.showing();
        assert_eq!(
            (before_the_press, pressed, dragging),
            (false, true, true),
            "(the hand before the press, as it starts the move, while it drags)"
        );
        assert_eq!(
            moved,
            Some(origin + Point::from((100, 50))),
            "the premise: the titlebar drag moved the window"
        );
        assert!(
            matches!(after, CursorImageStatus::Named(CursorIcon::Default)),
            "the titlebar's arrow is not back after the move: {after:?}"
        );
    });
}

/// **A window moved with the modifier shows the closed hand too**,
/// from the press to the release, and not after it (#374).
#[test]
fn a_modifier_move_shows_the_closed_hand_until_it_ends() {
    with_touch_window(false, |desk, _, window, before| {
        let region = region(desk);
        let at = (
            f64::from(before.loc.x) + 200.0,
            f64::from(before.loc.y) + 150.0,
        );
        move_pointer(&mut desk.state, at, 10_000);
        let origin = desk
            .state
            .space
            .element_location(&window)
            .expect("window origin");
        crate::input::key(&mut desk.state, SUPER_L.into(), KeyState::Pressed, 11);
        crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, 12_000);
        let pressed = closed_hand(desk);
        move_pointer(&mut desk.state, (at.0 + 60.0, at.1 + 40.0), 13_000);
        desk.state.reassert_cursor();
        let dragging = closed_hand(desk);
        let moved = desk.state.space.element_location(&window);
        crate::synth::send_button(
            &mut desk.state,
            region,
            0x110,
            ButtonState::Released,
            14_000,
        );
        crate::input::key(&mut desk.state, SUPER_L.into(), KeyState::Released, 15);
        desk.state.reassert_cursor();
        let after = closed_hand(desk);
        assert_eq!(
            moved,
            Some(origin + Point::from((60, 40))),
            "the premise: the modifier drag moved the window"
        );
        assert_eq!(
            (pressed, dragging, after),
            (true, true, false),
            "(the hand as the move starts, while it drags, after it)"
        );
    });
}

#[test]
fn a_finger_on_a_tiled_seam_moves_the_seam() {
    with_touch_window(false, |desk, opened, _, _| {
        let other = desk.open_surface();
        desk.arrange("tiling", "require(\"touch\")");
        desk.answer(opened);
        desk.answer(&other);
        desk.state.clock.advance(Duration::from_secs(1));
        let now = desk.state.clock.now();
        desk.state.settle(now);
        let mut panes = [
            (opened.pane, desk.placed(opened.pane)),
            (other.pane, desk.placed(other.pane)),
        ];
        panes.sort_by_key(|(_, area)| area.loc.x);
        let (left, a) = panes[0];
        let (right, b) = panes[1];
        let at = (
            f64::from(a.loc.x + a.size.w - 1),
            f64::from(a.loc.y + a.size.h / 2),
        );
        assert!(
            matches!(desk.state.chrome_under(at.into()), Some(under) if under.pane == left && matches!(under.chrome, crate::state::Chrome::Resize(_)))
        );
        let region = region(desk);
        crate::synth::touch_down(&mut desk.state, region, 0, at, 10_000);
        crate::synth::touch_motion(&mut desk.state, region, 0, (at.0 + 100.0, at.1), 20_000);
        desk.state.settle_resize();
        crate::synth::touch_up(&mut desk.state, region, 0, 30_000);
        desk.state.settle_resize();
        let (after_a, after_b) = (desk.placed(left), desk.placed(right));
        assert_eq!(after_a.size.w, a.size.w + 100);
        assert_eq!(after_b.loc.x, b.loc.x + 100);
        assert_eq!(after_a.loc.x, a.loc.x);
        assert_eq!(after_b.loc.x + after_b.size.w, b.loc.x + b.size.w);
    });
}

#[test]
fn a_touch_resize_never_grabs_the_pointer_and_ends_when_its_finger_lifts() {
    with_touch_window(false, |desk, opened, _, before| {
        let region = region(desk);
        let at = (
            f64::from(before.loc.x + before.size.w - 1),
            f64::from(before.loc.y + before.size.h / 2),
        );
        crate::synth::touch_down(&mut desk.state, region, 0, at, 10_000);
        crate::synth::touch_motion(&mut desk.state, region, 0, (at.0 + 100.0, at.1), 20_000);
        desk.state.settle_resize();
        let wanted = desk.state.pane_outer_of(opened.pane).expect("outer");
        assert_eq!(wanted.size.w, before.size.w + 100);
        assert!(!desk.state.seat.get_pointer().expect("pointer").is_grabbed());
        move_pointer(&mut desk.state, (1100.0, 800.0), 30_000);
        desk.state.settle_resize();
        assert_eq!(desk.state.pane_outer_of(opened.pane), Some(wanted));
        crate::synth::touch_up(&mut desk.state, region, 0, 40_000);
        crate::synth::touch_motion(&mut desk.state, region, 0, (at.0 + 200.0, at.1), 50_000);
        move_pointer(&mut desk.state, (1200.0, 900.0), 60_000);
        desk.state.settle_resize();
        assert_eq!(desk.state.pane_outer_of(opened.pane), Some(wanted));
    });
}

#[test]
fn without_lua_policy_touch_neither_moves_nor_resizes_a_window() {
    for framed in [false, true] {
        with_touch_window(framed, move |desk, opened, _, before| {
            desk.install("");
            let region = region(desk);
            let at = if framed {
                (220.0, 116.0)
            } else {
                (499.0, 250.0)
            };
            crate::synth::touch_down(&mut desk.state, region, 0, at, 10_000);
            crate::synth::touch_motion(
                &mut desk.state,
                region,
                0,
                (at.0 + 80.0, at.1 + 50.0),
                20_000,
            );
            crate::synth::touch_up(&mut desk.state, region, 0, 30_000);
            let now = desk.state.clock.now();
            desk.state.settle(now);
            assert_eq!(desk.state.pane_outer_of(opened.pane), Some(before));
            assert!(desk.state.pending_resize.is_none());
        });
    }
}

#[test]
fn lua_can_capture_a_client_finger_before_delivery() {
    with_real_touch_scene("lua-capture", TOUCH_DRAG_SCENE, |desk, _, _| {
        desk.install(
            r#"
            touch_events = {}
            assert(not pcall(sol.touch, 0, { action = "unknown" }))
            assert(not pcall(sol.touch, 0, { action = "move" }))
            assert(not pcall(sol.touch, 0, { action = "resize", window = 1, edges = "sideways" }))
            assert(not pcall(sol.click, math.huge, 0))
            sol.on("touch", function(e)
                touch_events[#touch_events + 1] = e.kind .. ":" .. e.target
                if e.kind == "down" then
                    assert(e.target == "client" and e.window)
                    sol.touch(e.slot, { action = "capture" })
                end
            end)
        "#,
        );
        let region = region(desk);
        crate::synth::touch_down(&mut desk.state, region, 0, (720.0, 520.0), 10_000);
        crate::synth::touch_motion(&mut desk.state, region, 0, (740.0, 540.0), 20_000);
        crate::synth::touch_up(&mut desk.state, region, 0, 30_000);
        crate::synth::touch_frame(&mut desk.state, region);
        desk.pump();
        assert!(
            desk.client.touches.is_empty(),
            "Lua capture precedes client down"
        );
        assert_eq!(
            recorded_touches(desk),
            "down:client,motion:client,up:client"
        );
    });
}

#[test]
fn lua_can_move_from_client_content_instead_of_a_titlebar() {
    with_touch_window(false, |desk, opened, window, _| {
        desk.install(
            r#"
            sol.on("touch", function(e)
                if e.kind == "down" and e.window then
                    sol.touch(e.slot, { action = "move", window = e.window })
                end
            end)
        "#,
        );
        let before = desk.state.space.element_location(&window).expect("mapped");
        let region = region(desk);
        crate::synth::touch_down(&mut desk.state, region, 0, (220.0, 220.0), 10_000);
        crate::synth::touch_motion(&mut desk.state, region, 0, (300.0, 270.0), 20_000);
        crate::synth::touch_up(&mut desk.state, region, 0, 30_000);
        crate::synth::touch_frame(&mut desk.state, region);
        desk.pump();
        assert_eq!(
            desk.state.space.element_location(&window),
            Some(before + Point::from((80, 50)))
        );
        assert!(
            !desk
                .client
                .touches
                .iter()
                .any(|e| matches!(e, wl_touch::Event::Down { .. }))
        );
        assert!(desk.state.pane_outer_of(opened.pane).is_some());
    });
}

#[test]
fn the_first_lua_claim_wins_and_motion_cannot_steal_a_client_sequence() {
    with_touch_window(false, |desk, _, window, _| {
        desk.install(
            r#"
            sol.on("touch", function(e)
                if e.kind == "down" then
                    sol.touch(e.slot, { action = "capture" })
                    sol.touch(e.slot, { action = "move", window = e.window })
                end
            end)
        "#,
        );
        let before = desk.state.space.element_location(&window);
        let region = region(desk);
        crate::synth::touch_down(&mut desk.state, region, 0, (220.0, 220.0), 10_000);
        crate::synth::touch_motion(&mut desk.state, region, 0, (300.0, 270.0), 20_000);
        crate::synth::touch_up(&mut desk.state, region, 0, 30_000);
        assert_eq!(desk.state.space.element_location(&window), before);
        desk.install(
            r#"
            sol.on("touch", function(e)
                if e.kind == "down" then
                    sol.touch(e.slot + 100, { action = "capture" })
                elseif e.kind == "motion" then
                    sol.touch(e.slot, { action = "capture" })
                end
            end)
        "#,
        );
        crate::synth::touch_down(&mut desk.state, region, 0, (220.0, 220.0), 40_000);
        crate::synth::touch_motion(&mut desk.state, region, 0, (230.0, 230.0), 50_000);
        crate::synth::touch_up(&mut desk.state, region, 0, 60_000);
        crate::synth::touch_frame(&mut desk.state, region);
        desk.pump();
        assert!(
            desk.client
                .touches
                .iter()
                .any(|e| matches!(e, wl_touch::Event::Down { .. }))
        );
        assert!(
            desk.client
                .touches
                .iter()
                .any(|e| matches!(e, wl_touch::Event::Motion { .. }))
        );
        assert!(
            desk.client
                .touches
                .iter()
                .any(|e| matches!(e, wl_touch::Event::Up { .. }))
        );
    });
}

#[test]
fn desktop_touch_actions_can_all_be_disabled_by_lua_configuration() {
    for framed in [false, true] {
        with_touch_window(framed, move |desk, opened, _, before| {
            desk.install(r#"
                require("config").touch = { focus = false, move = false, resize = false, click = false }
                require("touch")
            "#);
            let region = region(desk);
            let at = if framed {
                (220.0, 116.0)
            } else {
                (499.0, 250.0)
            };
            crate::synth::touch_down(&mut desk.state, region, 0, at, 10_000);
            crate::synth::touch_motion(
                &mut desk.state,
                region,
                0,
                (at.0 + 80.0, at.1 + 50.0),
                20_000,
            );
            crate::synth::touch_up(&mut desk.state, region, 0, 30_000);
            let now = desk.state.clock.now();
            desk.state.settle(now);
            assert_eq!(desk.state.pane_outer_of(opened.pane), Some(before));
        });
    }
}

const TOUCH_LOGGER: &str = r#"
    touch_events = {}
    sol.on("touch", function(event)
        touch_events[#touch_events + 1] = string.format("%s:%d:%.0f:%.0f:%s",
            event.kind, event.slot, event.x, event.y, event.output)
    end)
"#;

fn recorded_touches(desk: &Desk) -> String {
    desk.state
        .scripts
        .as_ref()
        .expect("scripts")
        .evaluate("return table.concat(touch_events, ',')")
}

/// **Lua hears each finger's velocity, measured by Solium (#284)**,
/// through the real input path: a finger moving 1000 px/s right
/// and 500 px/s up, sampled every 7 ms and stamped in 10 ms steps
/// as the Surface's touchscreen stamps them. `t` is the stamp in
/// seconds; a lift right after the motion keeps the throw.
#[test]
fn lua_hears_each_fingers_velocity_measured_by_solium() {
    let mut desk = Desk::new();
    let _opened = desk.open_surface();
    desk.install(
        r#"
        touch_velocity = {}
        sol.on("touch", function(event)
            touch_velocity[#touch_velocity + 1] = string.format("%s:%.0f:%.0f:%.3f",
                event.kind, event.vx, event.vy, event.t)
        end)
        "#,
    );
    let region = region(&desk);
    let stamp = |real_us: u64| real_us / 10_000 * 10_000;
    crate::synth::touch_down(&mut desk.state, region, 0, (200.0, 400.0), 1_000_000);
    for step in 1..=12_u32 {
        let real_us = 1_000_000 + u64::from(step) * 7_000;
        let moved = f64::from(step) * 7.0;
        crate::synth::touch_motion(
            &mut desk.state,
            region,
            0,
            (200.0 + moved, 400.0 - moved / 2.0),
            stamp(real_us),
        );
    }
    crate::synth::touch_up(&mut desk.state, region, 0, stamp(1_000_000 + 13 * 7_000));
    let log = desk
        .state
        .scripts
        .as_ref()
        .expect("scripts")
        .evaluate("return touch_velocity[#touch_velocity]");
    let fields: Vec<&str> = log.split(':').collect();
    let (kind, vx, vy, t) = (
        fields[0],
        fields[1].parse::<f64>().expect("vx"),
        fields[2].parse::<f64>().expect("vy"),
        fields[3].parse::<f64>().expect("t"),
    );
    assert_eq!(kind, "up", "{log}");
    assert!((vx - 1000.0).abs() < 120.0, "vx at the lift: {log}");
    assert!((vy + 500.0).abs() < 60.0, "vy at the lift: {log}");
    assert!((t - 1.09).abs() < 0.001, "t is the lift's stamp: {log}");
}

#[test]
fn lua_hears_each_fingers_touch_with_its_original_target() {
    with_real_touch_scene(
        "lua-touch",
        r"import QtQuick
        Item { property int taps: 0; TapHandler { onTapped: parent.taps += 1 } }",
        |desk, id, output| {
            desk.install(TOUCH_LOGGER);
            let region = region(desk);
            crate::synth::touch_down(&mut desk.state, region, 0, (620.0, 520.0), 10_000);
            crate::synth::touch_down(&mut desk.state, region, 1, (720.0, 520.0), 11_000);
            crate::synth::touch_motion(&mut desk.state, region, 1, (730.0, 530.0), 20_000);
            crate::synth::touch_up(&mut desk.state, region, 0, 30_000);
            crate::synth::touch_up(&mut desk.state, region, 1, 31_000);
            crate::synth::touch_frame(&mut desk.state, region);
            desk.pump();
            let name = output.name();
            assert_eq!(
                recorded_touches(desk),
                format!(
                    "down:0:620:520:{name},down:1:720:520:{name},motion:1:730:530:{name},up:0:620:520:{name},up:1:730:530:{name}"
                )
            );
            assert_eq!(
                desk.state
                    .surfaces
                    .get_mut(id)
                    .and_then(|s| s.instance_mut(output))
                    .expect("scene")
                    .scene_for_test()
                    .get_int("taps"),
                1
            );
            assert!(
                desk.client
                    .touches
                    .iter()
                    .any(|e| matches!(e, wl_touch::Event::Down { id: 1, .. }))
            );
        },
    );
}

#[test]
fn lua_hears_cancel_at_the_fingers_last_position_and_no_later_release() {
    with_real_touch_scene(
        "lua-cancel",
        r"import QtQuick
        Item { TapHandler {} }",
        |desk, _, output| {
            desk.install(TOUCH_LOGGER);
            let region = region(desk);
            crate::synth::touch_down(&mut desk.state, region, 1, (720.0, 520.0), 10_000);
            crate::synth::touch_motion(&mut desk.state, region, 1, (730.0, 530.0), 20_000);
            crate::synth::touch_frame(&mut desk.state, region);
            crate::synth::touch_cancel(&mut desk.state, region);
            crate::synth::touch_up(&mut desk.state, region, 1, 30_000);
            desk.pump();
            let name = output.name();
            assert_eq!(
                recorded_touches(desk),
                format!("down:1:720:520:{name},motion:1:730:530:{name},cancel:1:730:530:{name}")
            );
            assert!(
                desk.client
                    .touches
                    .iter()
                    .any(|e| matches!(e, wl_touch::Event::Cancel))
            );
        },
    );
}

#[test]
fn locked_touch_is_never_told_to_lua_even_when_unlock_cancels_it() {
    with_real_touch_scene(
        "lua-locked",
        r"import QtQuick
        Item { TapHandler {} }",
        |desk, _, output| {
            desk.install(TOUCH_LOGGER);
            let lock = desk
                .client
                .locks
                .as_ref()
                .expect("manager")
                .lock(&desk.qh, ());
            desk.pump();
            let region = region(desk);
            crate::synth::touch_down(&mut desk.state, region, 0, (620.0, 520.0), 10_000);
            crate::synth::touch_motion(&mut desk.state, region, 0, (630.0, 530.0), 20_000);
            assert_eq!(recorded_touches(desk), "");
            let frame = desk.state.lock_frame();
            desk.state.frame_presented(output, frame);
            desk.pump();
            lock.unlock_and_destroy();
            desk.pump();
            assert!(desk.state.lock.is_none());
            crate::synth::touch_up(&mut desk.state, region, 0, 30_000);
            assert_eq!(
                recorded_touches(desk),
                "",
                "unlock never replays lock input"
            );
        },
    );
}

#[test]
fn a_touch_under_a_script_grab_clicks_the_mode_but_scenes_still_own_their_controls() {
    with_real_touch_scene(
        "script-touch",
        r"import QtQuick
        Item { property int taps: 0; TapHandler { onTapped: parent.taps += 1 } }",
        |desk, id, output| {
            desk.install(
                r#"
                require("touch")
                clicks = {}
                sol.on("click", function(x, y)
                    clicks[#clicks + 1] = string.format("%.0f:%.0f", x, y)
                end)
            "#,
            );
            desk.state.script_grab = true;
            let region = region(desk);
            crate::synth::send_touch(&mut desk.state, region, (720.0, 520.0), 10_000);
            crate::synth::send_touch(&mut desk.state, region, (620.0, 520.0), 20_000);
            desk.pump();
            assert_eq!(
                desk.state
                    .scripts
                    .as_ref()
                    .expect("scripts")
                    .evaluate("return table.concat(clicks, ',')"),
                "720:520"
            );
            assert!(
                desk.client.touches.is_empty(),
                "overview's client receives no finger"
            );
            assert_eq!(
                desk.state
                    .surfaces
                    .get_mut(id)
                    .and_then(|s| s.instance_mut(output))
                    .expect("scene")
                    .scene_for_test()
                    .get_int("taps"),
                1
            );
        },
    );
}

#[test]
fn cancelling_a_framed_client_touch_keeps_its_backend_timestamp() {
    with_real_touch_scene(
        "cancel-time",
        r"import QtQuick
        Item { TapHandler {} }",
        |desk, _, _| {
            let region = region(desk);
            crate::synth::touch_down(&mut desk.state, region, 0, (720.0, 520.0), 8_000_000);
            crate::synth::touch_motion(&mut desk.state, region, 0, (730.0, 530.0), 8_010_000);
            crate::synth::touch_frame(&mut desk.state, region);
            crate::synth::touch_cancel(&mut desk.state, region);
            desk.pump();
            let times: Vec<_> =
                desk.client
                    .touches
                    .iter()
                    .filter_map(|event| match event {
                        wl_touch::Event::Down { time, .. }
                        | wl_touch::Event::Motion { time, .. } => Some(*time),
                        _ => None,
                    })
                    .collect();
            assert_eq!(
                times,
                [8000, 8010, 8010],
                "cancel's stationary motion uses the last backend time, not compositor uptime"
            );
        },
    );
}

#[test]
fn cancellation_frames_one_clients_release_while_another_still_holds_a_finger() {
    with_real_touch_scene(
        "cancel-clients",
        r"import QtQuick
        Item { TapHandler {} }",
        |desk, _, _| {
            let (connection, mut queue, mut client) =
                super::super::connect(&mut desk.display, &mut desk.state);
            let qh = queue.handle();
            let (window, _toplevel) = super::super::open_window(
                &mut desk.display,
                &mut desk.state,
                &connection,
                &client,
                &qh,
            );
            desk.state.space.map_element(window, (800, 500), false);
            desk.state.space.refresh();
            desk.state.clock.advance(Duration::from_secs(1));
            let now = desk.state.clock.now();
            desk.state.settle(now);
            let _touch = client.seat_touch(&qh);
            super::super::pump(
                &mut desk.display,
                &mut desk.state,
                &connection,
                &qh,
                &mut queue,
                &mut client,
            );
            let region = region(desk);
            crate::synth::touch_down(&mut desk.state, region, 0, (720.0, 520.0), 8_000_000);
            // A valid seat grab transition can leave independently
            // focused contacts. Keep both real-client streams live.
            desk.state
                .seat
                .get_touch()
                .expect("touch")
                .unset_grab(&mut desk.state);
            crate::synth::touch_down(&mut desk.state, region, 1, (820.0, 520.0), 8_010_000);
            crate::synth::touch_frame(&mut desk.state, region);
            desk.pump();
            super::super::pump(
                &mut desk.display,
                &mut desk.state,
                &connection,
                &qh,
                &mut queue,
                &mut client,
            );
            assert!(
                desk.client
                    .touches
                    .iter()
                    .any(|e| matches!(e, wl_touch::Event::Down { id: 0, .. }))
            );
            assert!(
                client
                    .touches
                    .iter()
                    .any(|e| matches!(e, wl_touch::Event::Down { id: 1, .. }))
            );
            desk.client.touches.clear();
            client.touches.clear();
            crate::synth::touch_up(&mut desk.state, region, 0, 8_020_000);
            crate::synth::touch_cancel(&mut desk.state, region);
            desk.pump();
            super::super::pump(
                &mut desk.display,
                &mut desk.state,
                &connection,
                &qh,
                &mut queue,
                &mut client,
            );
            assert!(matches!(
                desk.client.touches.first(),
                Some(wl_touch::Event::Up { id: 0, .. })
            ));
            assert!(
                matches!(desk.client.touches.last(), Some(wl_touch::Event::Frame)),
                "the lifted client's up must be framed even while another client is cancelled"
            );
            assert!(
                client
                    .touches
                    .iter()
                    .any(|e| matches!(e, wl_touch::Event::Cancel))
            );
        },
    );
}

fn other_touch_resize_window(desk: &mut Desk) -> Opened {
    let other = desk.open_surface();
    let window = desk
        .state
        .panes
        .get(other.pane)
        .and_then(Pane::client)
        .cloned()
        .expect("client");
    let _buffer = super::super::commit_buffer(&desk.client, &desk.qh, &other.surface, 400, 300);
    desk.pump();
    desk.state.space.map_element(window, (800, 100), false);
    desk.state.space.refresh();
    desk.state.clock.advance(Duration::from_secs(1));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    other
}

#[test]
fn a_second_finger_cannot_take_over_an_active_resize() {
    with_touch_window(false, |desk, opened, _, _| {
        let other = other_touch_resize_window(desk);
        let region = region(desk);
        crate::synth::touch_down(&mut desk.state, region, 0, (499.0, 250.0), 10_000);
        crate::synth::touch_down(&mut desk.state, region, 1, (1199.0, 250.0), 11_000);
        crate::synth::touch_motion(&mut desk.state, region, 0, (599.0, 250.0), 20_000);
        crate::synth::touch_motion(&mut desk.state, region, 1, (1279.0, 250.0), 21_000);
        desk.state.settle_resize();
        assert_eq!(
            desk.state.pane_outer_of(opened.pane).expect("first").size.w,
            500
        );
        assert_eq!(
            desk.state.pane_outer_of(other.pane).expect("second").size.w,
            400
        );
        crate::synth::touch_up(&mut desk.state, region, 1, 30_000);
        crate::synth::touch_motion(&mut desk.state, region, 0, (609.0, 250.0), 31_000);
        desk.state.settle_resize();
        assert_eq!(
            desk.state.pane_outer_of(opened.pane).expect("first").size.w,
            510,
            "lifting the rejected finger does not end the owner"
        );
        crate::synth::touch_up(&mut desk.state, region, 0, 40_000);
    });
}

#[test]
fn a_finger_cannot_take_over_a_pointer_resize() {
    with_touch_window(false, |desk, opened, _, _| {
        let other = other_touch_resize_window(desk);
        let region = region(desk);
        move_pointer(&mut desk.state, (499.0, 250.0), 10_000);
        crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, 11_000);
        assert!(desk.state.seat.get_pointer().expect("pointer").is_grabbed());
        crate::synth::touch_down(&mut desk.state, region, 0, (1199.0, 250.0), 12_000);
        move_pointer(&mut desk.state, (599.0, 250.0), 20_000);
        crate::synth::touch_motion(&mut desk.state, region, 0, (1279.0, 250.0), 21_000);
        desk.state.settle_resize();
        assert_eq!(
            desk.state.pane_outer_of(opened.pane).expect("first").size.w,
            500
        );
        assert_eq!(
            desk.state.pane_outer_of(other.pane).expect("second").size.w,
            400
        );
        crate::synth::touch_up(&mut desk.state, region, 0, 30_000);
        crate::synth::send_button(
            &mut desk.state,
            region,
            0x110,
            ButtonState::Released,
            31_000,
        );
    });
}

#[test]
fn a_pointer_border_or_modifier_press_cannot_take_over_a_finger_resize() {
    for modifier in [false, true] {
        with_touch_window(false, move |desk, opened, _, _| {
            let other = other_touch_resize_window(desk);
            let region = region(desk);
            crate::synth::touch_down(&mut desk.state, region, 0, (499.0, 250.0), 10_000);
            let button = if modifier {
                crate::input::key(&mut desk.state, SUPER_L.into(), KeyState::Pressed, 11);
                move_pointer(&mut desk.state, (1000.0, 250.0), 11_000);
                BTN_RIGHT
            } else {
                move_pointer(&mut desk.state, (1199.0, 250.0), 11_000);
                0x110
            };
            crate::synth::send_button(
                &mut desk.state,
                region,
                button,
                ButtonState::Pressed,
                12_000,
            );
            assert!(
                !desk.state.seat.get_pointer().expect("pointer").is_grabbed(),
                "a rejected resize press must not install a pointer grab"
            );
            crate::synth::touch_motion(&mut desk.state, region, 0, (599.0, 250.0), 20_000);
            move_pointer(&mut desk.state, (1279.0, 250.0), 21_000);
            desk.state.settle_resize();
            assert_eq!(
                desk.state.pane_outer_of(opened.pane).expect("first").size.w,
                500
            );
            assert_eq!(
                desk.state.pane_outer_of(other.pane).expect("second").size.w,
                400
            );
            crate::synth::send_button(
                &mut desk.state,
                region,
                button,
                ButtonState::Released,
                30_000,
            );
            if modifier {
                crate::input::key(&mut desk.state, SUPER_L.into(), KeyState::Released, 31);
            }
            crate::synth::touch_up(&mut desk.state, region, 0, 32_000);
        });
    }
}

// Keep the entire Wayland fixture on Qt's executor: a scene and
// its destructor must run on the thread that owns QGuiApplication.
fn with_real_touch_scene(
    name: &'static str,
    source: &'static str,
    run: impl FnOnce(&mut Desk, crate::scripted::SurfaceId, &Output) + Send + 'static,
) {
    crate::qml::qt_test::on_the_qt_thread(move || {
        crate::qml::start().expect("Qt starts");
        let mut desk = Desk::new();
        let opened = desk.open_surface();
        let window = desk
            .state
            .panes
            .get(opened.pane)
            .and_then(crate::pane::Pane::client)
            .cloned()
            .expect("client");
        desk.state.space.map_element(window, (700, 500), false);
        desk.state.space.refresh();
        desk.state.clock.advance(Duration::from_secs(1));
        let now = desk.state.clock.now();
        desk.state.settle(now);
        let directory = std::env::temp_dir().join(format!("solium-touch-{name}"));
        std::fs::create_dir_all(&directory).expect("directory");
        let path = directory.join("Scene.qml");
        std::fs::write(&path, source).expect("scene source");
        desk.state
            .declare_surface(crate::scripted::Declaration::for_test(
                name,
                path,
                Scripted::Top,
                crate::scripted::On::Rect(Rectangle::new((600, 500).into(), (64, 64).into())),
            ));
        let id = desk.state.surfaces.named(name).expect("declared");
        let output = desk.state.space.outputs().next().cloned().expect("monitor");
        desk.state
            .surfaces
            .get_mut(id)
            .and_then(|s| s.instance_mut(&output))
            .expect("real scene")
            .scene_for_test()
            .resize(64, 64, 1.0);
        let _touch = desk.client.seat_touch(&desk.qh);
        desk.pump();
        run(&mut desk, id, &output);
        drop(desk);
        let _ = std::fs::remove_dir_all(directory);
    });
}

const TOUCH_DRAG_SCENE: &str = r"import QtQuick
    Item {
        id: root
        property real distance: box.x
        property int ended: 0
        Rectangle {
            id: box; width: 64; height: 64
            DragHandler {
                dragThreshold: 0
                onActiveChanged: if (!active) root.ended += 1
            }
        }
    }";

#[test]
fn a_finger_dragging_a_real_scenes_drag_handler_moves_it() {
    with_real_touch_scene("drag", TOUCH_DRAG_SCENE, |desk, id, output| {
        let region = region(desk);
        crate::synth::touch_down(&mut desk.state, region, 0, (620.0, 520.0), 10);
        for (index, x) in [640.0, 660.0, 680.0].into_iter().enumerate() {
            crate::synth::touch_motion(&mut desk.state, region, 0, (x, 520.0), 20 + index as u64);
        }
        crate::synth::touch_up(&mut desk.state, region, 0, 30);
        let scene = desk
            .state
            .surfaces
            .get_mut(id)
            .and_then(|s| s.instance_mut(output))
            .expect("scene")
            .scene_for_test();
        assert_eq!(scene.get_real("distance", -1.0), 60.0);
        assert_eq!(scene.get_int("ended"), 1);
    });
}

#[test]
fn a_finger_keeps_its_real_scene_after_moving_over_a_client() {
    with_real_touch_scene("capture", TOUCH_DRAG_SCENE, |desk, id, output| {
        let region = region(desk);
        crate::synth::touch_down(&mut desk.state, region, 0, (620.0, 520.0), 10);
        crate::synth::touch_motion(&mut desk.state, region, 0, (650.0, 520.0), 20);
        crate::synth::touch_motion(&mut desk.state, region, 0, (720.0, 520.0), 30);
        crate::synth::touch_up(&mut desk.state, region, 0, 40);
        crate::synth::touch_frame(&mut desk.state, region);
        desk.pump();
        let scene = desk
            .state
            .surfaces
            .get_mut(id)
            .and_then(|s| s.instance_mut(output))
            .expect("scene")
            .scene_for_test();
        assert_eq!(scene.get_real("distance", -1.0), 100.0);
        assert_eq!(scene.get_int("ended"), 1);
        assert!(desk.client.touches.is_empty(), "captured touch never leaks");
    });
}

#[test]
fn two_fingers_on_a_real_scene_and_client_reach_their_own_targets() {
    with_real_touch_scene(
        "two-targets",
        r"import QtQuick
        Item { property int taps: 0; TapHandler { onTapped: parent.taps += 1 } }",
        |desk, id, output| {
            let region = region(desk);
            crate::synth::touch_down(&mut desk.state, region, 0, (620.0, 520.0), 10);
            crate::synth::touch_down(&mut desk.state, region, 1, (720.0, 520.0), 11);
            crate::synth::touch_frame(&mut desk.state, region);
            crate::synth::touch_up(&mut desk.state, region, 0, 20);
            crate::synth::touch_up(&mut desk.state, region, 1, 21);
            crate::synth::touch_frame(&mut desk.state, region);
            desk.pump();
            let taps = desk
                .state
                .surfaces
                .get_mut(id)
                .and_then(|s| s.instance_mut(output))
                .expect("scene")
                .scene_for_test()
                .get_int("taps");
            assert_eq!(taps, 1);
            assert!(
                desk.client
                    .touches
                    .iter()
                    .any(|e| matches!(e, wl_touch::Event::Down { id: 1, .. }))
            );
            assert!(
                desk.client
                    .touches
                    .iter()
                    .any(|e| matches!(e, wl_touch::Event::Up { id: 1, .. }))
            );
            assert!(!desk.client.touches.iter().any(|e| matches!(
                e,
                wl_touch::Event::Down { id: 0, .. } | wl_touch::Event::Up { id: 0, .. }
            )));
        },
    );
}

#[test]
fn while_locked_a_touch_over_a_real_scene_reaches_only_the_lock_surface() {
    with_real_touch_scene(
        "locked",
        r"import QtQuick
        Item { property int taps: 0; TapHandler { onTapped: parent.taps += 1 } }",
        |desk, id, output| {
            let _global = output.create_global::<Solium>(&desk.display.handle());
            desk.pump();
            let lock = desk
                .client
                .locks
                .as_ref()
                .expect("lock manager")
                .lock(&desk.qh, ());
            let surface = desk
                .client
                .compositor
                .as_ref()
                .expect("compositor")
                .create_surface(&desk.qh, ());
            let role = lock.get_lock_surface(&surface, &desk.client.outputs[0], &desk.qh, ());
            desk.pump();
            let serial = desk
                .client
                .lock_configures
                .iter()
                .find(|(id, _)| *id == role.id())
                .expect("configure")
                .1;
            role.ack_configure(serial);
            let _buffer = super::super::commit_buffer(&desk.client, &desk.qh, &surface, 1920, 1080);
            desk.pump();
            assert!(desk.state.lock.is_some());
            let region = region(desk);
            crate::synth::send_touch(&mut desk.state, region, (620.0, 520.0), 10);
            desk.pump();
            let taps = desk
                .state
                .surfaces
                .get_mut(id)
                .and_then(|s| s.instance_mut(output))
                .expect("scene")
                .scene_for_test()
                .get_int("taps");
            assert_eq!(taps, 0);
            assert!(desk.client.touches.iter().any(
                |e| matches!(e, wl_touch::Event::Down { surface: target, .. } if target == &surface)
            ));
        },
    );
}

/// Two fingers retain independent slots and surface-local positions
/// through the actual input dispatcher and the wl_touch wire (#181).
#[test]
fn two_synthetic_fingers_reach_a_client_with_motion_and_frames() {
    let mut desk = Desk::new();
    let opened = desk.open_surface();
    let window = desk
        .state
        .panes
        .get(opened.pane)
        .and_then(crate::pane::Pane::client)
        .cloned()
        .expect("a client");
    desk.state.space.map_element(window, (100, 100), false);
    desk.state.space.refresh();
    // Finish the opening transition before targeting the client,
    // as window_under_a_scene does for pointer and touch routing.
    desk.state.clock.advance(Duration::from_secs(1));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    assert!(desk.state.touch_under((110.0, 112.0).into()).is_some());
    let _touch = desk.client.seat_touch(&desk.qh);
    desk.pump();
    let region = region(&desk);
    crate::synth::touch_down(&mut desk.state, region, 0, (110.0, 112.0), 10);
    crate::synth::touch_down(&mut desk.state, region, 1, (125.0, 128.0), 11);
    crate::synth::touch_frame(&mut desk.state, region);
    crate::synth::touch_motion(&mut desk.state, region, 1, (130.0, 135.0), 12);
    crate::synth::touch_frame(&mut desk.state, region);
    crate::synth::touch_up(&mut desk.state, region, 0, 13);
    crate::synth::touch_up(&mut desk.state, region, 1, 14);
    crate::synth::touch_frame(&mut desk.state, region);
    desk.pump();
    let seen: Vec<_> = desk
        .client
        .touches
        .iter()
        .map(|event| match event {
            wl_touch::Event::Down {
                id, surface, x, y, ..
            } => {
                assert_eq!(surface, &opened.surface);
                ("down", *id, *x, *y)
            }
            wl_touch::Event::Motion { id, x, y, .. } => ("motion", *id, *x, *y),
            wl_touch::Event::Up { id, .. } => ("up", *id, 0.0, 0.0),
            wl_touch::Event::Frame => ("frame", -1, 0.0, 0.0),
            _ => ("unexpected", -1, 0.0, 0.0),
        })
        .collect();
    assert_eq!(
        seen,
        [
            ("down", 0, 10.0, 12.0),
            ("down", 1, 25.0, 28.0),
            ("frame", -1, 0.0, 0.0),
            ("motion", 1, 30.0, 35.0),
            ("frame", -1, 0.0, 0.0),
            ("up", 0, 0.0, 0.0),
            ("up", 1, 0.0, 0.0),
            ("frame", -1, 0.0, 0.0),
        ]
    );
}

/// **A touch neither reaches nor focuses the window under a shell
/// button** (Ruling 8, as for the pointer): a touch where the shell
/// takes a press belongs to that scene, and the window with the
/// keyboard keeps it. Beside the button
/// the touch lands on the window under the shell and focuses it,
/// even while the shell holds a press of the mouse's, which is the
/// pointer's and not the touch's.
#[test]
fn a_touch_on_a_shell_button_neither_reaches_nor_focuses_the_window_under_it() {
    let (mut desk, opened, _) = window_under_a_scene(button_over_the_window);
    let other = desk.open_surface();
    let other_window = desk
        .state
        .panes
        .get(other.pane)
        .and_then(crate::pane::Pane::client)
        .cloned()
        .expect("a client");
    desk.state
        .space
        .map_element(other_window.clone(), (100, 100), false);
    desk.state.space.refresh();
    desk.state.focus_window(
        &other_window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    desk.install("require(\"touch\")");
    let region = region(&desk);
    let lands = |desk: &Desk, at: (f64, f64)| {
        desk.state
            .touch_under(at.into())
            .map(|(surface, _)| surface.id().protocol_id())
    };

    crate::synth::send_touch(&mut desk.state, region, (630.0, 520.0), 10);
    let on_the_button = (lands(&desk, (630.0, 520.0)), desk.focused());
    move_pointer(&mut desk.state, (630.0, 520.0), 20);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, 21);
    let beside_while_held = lands(&desk, (630.0, 555.0));
    crate::synth::send_touch(&mut desk.state, region, (630.0, 555.0), 22);
    assert_eq!(
        (on_the_button, beside_while_held, desk.focused()),
        ((None, other.pane), Some(window_id(&opened)), opened.pane),
        "((where a touch on the shell button lands, the window with the keyboard \
         after it), where one beside the button lands while the shell holds a mouse \
         press, the window with the keyboard after that touch)"
    );
}

/// A real full-screen scene on the overlay layer whose only item is
/// a `MouseArea` over all of it, disabled while `catching` is
/// false, and under it a real scene on the bottom layer with a
/// `TapHandler` over 100..164 x 100..164 of the screen: the shell
/// of #322, an overlay that holds nothing but a background meant
/// to catch presses only when it is turned on, over a dock's icon.
fn disabled_mouse_area_over_a_scene(
    name: &'static str,
    run: impl FnOnce(&mut Desk, crate::scripted::SurfaceId, crate::scripted::SurfaceId) + Send + 'static,
) {
    crate::qml::qt_test::on_the_qt_thread(move || {
        crate::qml::start().expect("Qt starts");
        let mut desk = Desk::new();
        let directory = std::env::temp_dir().join(format!("solium-disabled-{name}"));
        let _ = std::fs::remove_dir_all(&directory);
        let output = desk.state.space.outputs().next().cloned().expect("monitor");
        let scenes = [
            (
                "overlay",
                Scripted::Overlay,
                crate::scripted::On::EveryMonitor,
                (1920, 1080),
                r"import QtQuick
                Item {
                    id: root
                    property bool catching: false
                    property int caught: 0
                    MouseArea {
                        anchors.fill: parent
                        enabled: root.catching
                        onPressed: root.caught += 1
                    }
                }",
            ),
            (
                "dock",
                Scripted::Bottom,
                crate::scripted::On::Rect(Rectangle::new((100, 100).into(), (64, 64).into())),
                (64, 64),
                r"import QtQuick
                Item { property int taps: 0; TapHandler { onTapped: parent.taps += 1 } }",
            ),
        ];
        let mut ids = Vec::new();
        for (scene, layer, on, (width, height), source) in scenes {
            let folder = directory.join(scene);
            std::fs::create_dir_all(&folder).expect("directory");
            let path = folder.join("Scene.qml");
            std::fs::write(&path, source).expect("scene source");
            let declared = format!("{name}-{scene}");
            desk.state
                .declare_surface(crate::scripted::Declaration::for_test(
                    &declared, path, layer, on,
                ));
            let id = desk.state.surfaces.named(&declared).expect("declared");
            desk.state
                .surfaces
                .get_mut(id)
                .and_then(|s| s.instance_mut(&output))
                .expect("real scene")
                .scene_for_test()
                .resize(width, height, 1.0);
            ids.push(id);
        }
        let _touch = desk.client.seat_touch(&desk.qh);
        desk.pump();
        run(&mut desk, ids[0], ids[1]);
        drop(desk);
        let _ = std::fs::remove_dir_all(directory);
    });
}

/// An integer property of a real scene's root.
fn scene_int(desk: &mut Desk, id: crate::scripted::SurfaceId, name: &str) -> i32 {
    let output = desk.state.space.outputs().next().cloned().expect("monitor");
    desk.state
        .surfaces
        .get_mut(id)
        .and_then(|s| s.instance_mut(&output))
        .expect("scene")
        .scene_for_test()
        .get_int(name)
}

/// **A disabled `MouseArea` over a scene leaves its touch and its
/// click to the scene under it** (#322, Ruling 6): a disabled
/// handler takes nothing, and a `MouseArea`'s `enabled` is its
/// own, not the item's, so a full-screen overlay holding only a
/// disabled one is not in the way of the dock's icon under it, for
/// a finger or for the mouse.
#[test]
fn a_disabled_mouse_area_over_a_scene_leaves_its_touch_and_its_click_to_the_scene_under_it() {
    disabled_mouse_area_over_a_scene("off", |desk, overlay, dock| {
        let region = region(desk);
        crate::synth::send_touch(&mut desk.state, region, (130.0, 130.0), 10);
        desk.pump();
        let after_the_touch = scene_int(desk, dock, "taps");
        click(desk, (130.0, 130.0), 10_000);
        desk.pump();
        assert_eq!(
            (
                after_the_touch,
                scene_int(desk, dock, "taps"),
                scene_int(desk, overlay, "caught")
            ),
            (1, 2, 0),
            "(the dock's taps after the touch, after the click too, the presses \
             the overlay caught)"
        );
    });
}

/// **A `MouseArea` enabled again over a scene takes its touch and
/// its click again**: the overlay of the test before, turned on,
/// catches both presses, and the dock's icon under it hears
/// neither.
#[test]
fn a_mouse_area_enabled_again_over_a_scene_takes_its_touch_and_its_click_again() {
    disabled_mouse_area_over_a_scene("on", |desk, overlay, dock| {
        let output = desk.state.space.outputs().next().cloned().expect("monitor");
        desk.state
            .surfaces
            .get_mut(overlay)
            .and_then(|s| s.instance_mut(&output))
            .expect("scene")
            .scene_for_test()
            .set_bool("catching", true);
        let region = region(desk);
        crate::synth::send_touch(&mut desk.state, region, (130.0, 130.0), 10);
        desk.pump();
        let after_the_touch = scene_int(desk, overlay, "caught");
        click(desk, (130.0, 130.0), 10_000);
        desk.pump();
        assert_eq!(
            (
                after_the_touch,
                scene_int(desk, overlay, "caught"),
                scene_int(desk, dock, "taps")
            ),
            (1, 2, 0),
            "(the presses the overlay caught after the touch, after the click too, \
             the dock's taps)"
        );
    });
}

/// **A scene hears the pointer leave when it moves off its
/// items**, so what it hovered is hovered no longer.
#[test]
fn the_scene_hears_the_pointer_leave_when_it_moves_off_its_items() {
    let (mut desk, _, shell) = window_under_a_scene(button_over_the_window);
    move_pointer(&mut desk.state, (630.0, 520.0), 10);
    move_pointer(&mut desk.state, (630.0, 555.0), 11);
    let left = desk
        .state
        .surfaces
        .get(shell)
        .and_then(crate::scripted::Surface::stand)
        .map(|stand| stand.left);
    assert_eq!(left, Some(1));
}

/// **A window's frame is kept from the pointer only where a shell
/// takes a press over it, or holds one** (Ruling 8): on a shell
/// button over the window and while a press the shell took is
/// held, the frame under it is not what the pointer is on, as the
/// window is not; beside the button, or under a scene that takes
/// only hover, it is.
#[test]
fn the_frames_are_kept_from_the_pointer_only_where_a_shell_takes_a_press() {
    let (mut desk, _, _) = window_under_a_scene(button_over_the_window);
    let kept = |desk: &Desk, at: (f64, f64)| desk.state.frames_kept_from(at.into());
    let on_the_button = kept(&desk, (630.0, 520.0));
    let beside_it = kept(&desk, (630.0, 555.0));
    move_pointer(&mut desk.state, (630.0, 520.0), 10);
    let region = region(&desk);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, 11);
    let held = kept(&desk, (630.0, 555.0));
    let (strip, _, _) = window_under_a_scene(hover_everywhere);
    assert_eq!(
        (on_the_button, beside_it, held, kept(&strip, (630.0, 530.0))),
        (true, false, true, false),
        "(on a shell button over the window, beside it, beside it while the shell \
         holds a press, under a hover strip)"
    );
}

/// **A frame the pointer hovered is told it left when the pointer
/// moves onto a shell button drawn over it**: the shell takes that
/// motion, and the frame under it still hears that it is no longer
/// what the pointer is on. A built frame needs Qt, which cannot
/// run beside this real client (the #99 test), so the frame's
/// hover is set by hand, as the pointer over its titlebar sets it.
#[test]
fn a_hovered_frame_hears_the_pointer_leave_onto_a_shell_button_over_it() {
    let (mut desk, opened, _) = window_under_a_scene(button_over_the_window);
    move_pointer(&mut desk.state, (630.0, 555.0), 10);
    desk.state.hovered_frame = Some(opened.pane);
    move_pointer(&mut desk.state, (630.0, 520.0), 11);
    assert_eq!(
        desk.state.hovered_frame, None,
        "the frame under the shell button is still hovered"
    );
}

/// **A scene pressed under a pointer that has not moved still
/// hears it leave**: the scene came under a still pointer, took
/// the press, and was dragged off and let go of; the next motion
/// off it tells it.
#[test]
fn a_scene_pressed_under_a_still_pointer_hears_it_leave() {
    let mut desk = russian_desk();
    move_pointer(&mut desk.state, (100.0, 15.0), 1);
    let bar = stand_in(&mut desk.state, "bar", Scripted::Top, bar(), Stand::solid());
    let region = region(&desk);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, 2);
    move_pointer(&mut desk.state, (100.0, 500.0), 3);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Released, 4);
    move_pointer(&mut desk.state, (100.0, 501.0), 5);
    let left = desk
        .state
        .surfaces
        .get(bar)
        .and_then(crate::scripted::Surface::stand)
        .map(|stand| stand.left);
    assert_eq!(
        (presses(&desk, bar), left),
        (1, Some(1)),
        "(the presses the scene took, how many times it heard the pointer leave)"
    );
}

/// **A scene pressed over another one the pointer was hovering
/// takes the hover from it**: the hover strip under it hears the
/// pointer leave at the press, and the scene pressed hears it
/// leave when the pointer moves off after the release.
#[test]
fn a_scene_pressed_over_another_hovered_one_takes_the_hover() {
    let mut desk = russian_desk();
    let strip = stand_in(
        &mut desk.state,
        "strip",
        Scripted::Top,
        bar(),
        Stand {
            hit: hover_everywhere,
            ..Stand::solid()
        },
    );
    move_pointer(&mut desk.state, (100.0, 15.0), 1);
    let panel = stand_in(
        &mut desk.state,
        "panel",
        Scripted::Overlay,
        bar(),
        Stand::solid(),
    );
    let region = region(&desk);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, 2);
    move_pointer(&mut desk.state, (100.0, 500.0), 3);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Released, 4);
    move_pointer(&mut desk.state, (100.0, 501.0), 5);
    let left = |id| {
        desk.state
            .surfaces
            .get(id)
            .and_then(crate::scripted::Surface::stand)
            .map(|stand| stand.left)
    };
    assert_eq!(
        (presses(&desk, panel), left(strip), left(panel)),
        (1, Some(1), Some(1)),
        "(the presses the panel took, the strip heard the pointer leave, the panel heard it leave)"
    );
}

/// **The pointer keeps the shape of a press a scene holds**
/// (Ruling 8, #108): dragged off a shell button onto a window's
/// resize border, the press is still the scene's, so the pointer
/// does not offer a resize.
#[test]
fn a_press_a_scene_holds_keeps_its_shape_over_a_resize_border() {
    let (mut desk, _, _) = window_under_a_scene(button_over_the_window);
    let corner = Point::<f64, Logical>::from((663.0, 563.0));
    assert!(
        desk.state.claim_under(corner).cursor().is_some(),
        "the premise: the window's corner offers a resize, got {:?}",
        desk.state.claim_under(corner)
    );
    move_pointer(&mut desk.state, (630.0, 520.0), 10);
    let region = region(&desk);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, 11);
    move_pointer(&mut desk.state, (corner.x, corner.y), 12);
    assert!(
        !desk.state.pointer.assert(None),
        "the pointer offered a resize for a press the scene holds"
    );
}

/// **The pointer is the shape the scene under it asks for**
/// (#373): over a shell button whose scene asks for the pointing
/// hand, the hand; off the button, over the window under the
/// scene, not the scene's; back on it with the scene asking for
/// the arrow, not the hand either, and that without the pointer
/// moving, from the frame's own check.
#[test]
fn the_pointer_is_the_shape_the_scene_under_it_asks_for() {
    use smithay::input::pointer::{CursorIcon, CursorImageStatus};
    let (mut desk, _, shell) = window_under_a_scene(button_over_the_window);
    let ask = |desk: &mut Desk, icon: Option<CursorIcon>| {
        if let Some(stand) = desk
            .state
            .surfaces
            .get_mut(shell)
            .and_then(crate::scripted::Surface::stand_mut)
        {
            stand.cursor = icon;
        }
    };
    let hand = |desk: &mut Desk| {
        matches!(
            desk.state.pointer.showing(),
            CursorImageStatus::Named(CursorIcon::Pointer)
        )
    };
    ask(&mut desk, Some(CursorIcon::Pointer));
    move_pointer(&mut desk.state, (630.0, 520.0), 10);
    let on_the_button = hand(&mut desk);
    move_pointer(&mut desk.state, (630.0, 560.0), 11);
    let off_it = hand(&mut desk);
    move_pointer(&mut desk.state, (630.0, 520.0), 12);
    let back = hand(&mut desk);
    ask(&mut desk, None);
    desk.state.reassert_cursor();
    let arrow = hand(&mut desk);
    assert_eq!(
        (on_the_button, off_it, back, arrow),
        (true, false, true, false),
        "(the hand over the button, over the window beside it, over the button \
         again, once the scene asks for the arrow)"
    );
}

/// A window at 600,500 under a shell with a button over it, and
/// another window, which has the keyboard; click-to-focus on and
/// focus-follows-mouse off, so only a press moves the keyboard.
fn the_keyboard_beside_a_shell_button() -> (Desk, Opened, Opened, crate::scripted::SurfaceId) {
    let (mut desk, opened, shell) = window_under_a_scene(button_over_the_window);
    let other = desk.open_surface();
    let other_window = desk
        .state
        .panes
        .get(other.pane)
        .and_then(crate::pane::Pane::client)
        .cloned()
        .expect("a client");
    desk.state
        .space
        .map_element(other_window.clone(), (100, 100), false);
    desk.state.space.refresh();
    desk.state.focus_window(
        &other_window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    desk.state.profile.click_to_focus = true;
    desk.state.profile.focus_follows_mouse = false;
    (desk, opened, other, shell)
}

/// **A button Qt has no name for, pressed on a shell button, is
/// swallowed** (Ruling 9): the scene is not told of it, and it
/// does not pass through the scene to the window under it, which
/// is neither focused nor sent it.
#[test]
fn a_button_qt_has_no_name_for_is_swallowed_where_a_shell_takes_a_press() {
    let (mut desk, opened, other, shell) = the_keyboard_beside_a_shell_button();
    move_pointer(&mut desk.state, (630.0, 520.0), 10);
    let region = region(&desk);
    crate::synth::send_button(&mut desk.state, region, 0x100, ButtonState::Pressed, 11);
    crate::synth::send_button(&mut desk.state, region, 0x100, ButtonState::Released, 12);
    assert_eq!(
        (desk.focused(), presses(&desk, shell), pointer_on(&desk)),
        (other.pane, 0, None),
        "(the window with the keyboard, the presses the scene was told, the surface \
         the pointer is on) after BTN_0 on a shell button over window {}",
        window_id(&opened)
    );
}

/// **A button Qt has no name for, pressed while a scene holds a
/// press, is none of the compositor's** (Ruling 7): dragged off
/// the shell button onto the window, a side button there neither
/// focuses the window nor starts a drag of it; it goes on as it
/// is, as the pointer is nobody else's until every button is up.
#[test]
fn a_button_qt_has_no_name_for_during_a_scenes_press_is_not_the_compositors() {
    let (mut desk, _, other, shell) = the_keyboard_beside_a_shell_button();
    move_pointer(&mut desk.state, (630.0, 520.0), 10);
    let region = region(&desk);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, 11);
    move_pointer(&mut desk.state, (630.0, 545.0), 12);
    crate::synth::send_button(&mut desk.state, region, 0x120, ButtonState::Pressed, 13);
    assert_eq!(
        (desk.focused(), presses(&desk, shell)),
        (other.pane, 1),
        "(the window with the keyboard, the presses the scene was told)"
    );
}

/// **A grab smithay starts during a scene's press leaves the scene
/// its wheel and its release** (Ruling 7): a button Qt has no name
/// for, pressed during the hold, starts a click grab, and the
/// hold still has the pointer.
#[test]
fn a_grab_started_during_a_scenes_press_leaves_it_the_wheel_and_the_release() {
    let (mut desk, _, shell) = window_under_a_scene(button_over_the_window);
    move_pointer(&mut desk.state, (630.0, 520.0), 10);
    let region = region(&desk);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, 11);
    crate::synth::send_button(&mut desk.state, region, 0x120, ButtonState::Pressed, 12);
    assert!(
        desk.state
            .seat
            .get_pointer()
            .is_some_and(|pointer| pointer.is_grabbed()),
        "the premise: smithay grabbed the pointer"
    );
    crate::synth::send_axis(&mut desk.state, region, (0.0, 120.0), 13);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Released, 14);
    let events = scene_events(&desk.state, shell);
    assert_eq!(
        (
            events
                .iter()
                .any(|event| matches!(event.kind, PointerKind::Wheel { .. })),
            events
                .iter()
                .any(|event| matches!(event.kind, PointerKind::Release(0x1))),
            desk.state.scene_press.is_none(),
        ),
        (true, true, true),
        "(the scene heard the wheel, it heard the release, the hold let go)"
    );
}

/// **The release of a button Qt has no name for, which a shell
/// swallowed the press of, is swallowed wherever it lands**: pressed
/// on a shell button over a window and let go of over the window,
/// the window is told of neither, so it never holds a release for a
/// press it was not sent.
#[test]
fn a_swallowed_unnamed_press_swallows_its_release_off_the_scene() {
    let (mut desk, _, _, shell) = the_keyboard_beside_a_shell_button();
    let _pointer = desk.client.seat_pointer(&desk.qh);
    desk.pump();
    move_pointer(&mut desk.state, (630.0, 520.0), 10);
    let region = region(&desk);
    crate::synth::send_button(&mut desk.state, region, 0x100, ButtonState::Pressed, 11);
    move_pointer(&mut desk.state, (630.0, 555.0), 12);
    let on_the_window = pointer_on(&desk);
    crate::synth::send_button(&mut desk.state, region, 0x100, ButtonState::Released, 13);
    desk.pump();
    assert_eq!(
        (
            on_the_window.is_some(),
            desk.client.buttons.clone(),
            presses(&desk, shell),
            desk.state.swallowed.is_empty(),
        ),
        (true, Vec::new(), 0, true),
        "(the window had the pointer at the release, the buttons it was told, \
         the presses the scene was told, the swallowed presses all let go)"
    );
}

/// **The lock forgets the presses swallowed before it** (Ruling 12):
/// a release behind the lock is the lock screen's, so a press the
/// shell swallowed is never let go of through the shell, and after
/// the lock the same button clicked on a window reaches it whole.
#[test]
fn the_lock_forgets_the_presses_a_shell_swallowed() {
    let (mut desk, _, _, _) = the_keyboard_beside_a_shell_button();
    let _pointer = desk.client.seat_pointer(&desk.qh);
    desk.pump();
    move_pointer(&mut desk.state, (630.0, 520.0), 10);
    let region = region(&desk);
    crate::synth::send_button(&mut desk.state, region, 0x100, ButtonState::Pressed, 11);
    let swallowed = desk.state.swallowed.contains(&0x100);
    desk.state.release_grabs();
    move_pointer(&mut desk.state, (630.0, 555.0), 12);
    crate::synth::send_button(&mut desk.state, region, 0x100, ButtonState::Pressed, 13);
    crate::synth::send_button(&mut desk.state, region, 0x100, ButtonState::Released, 14);
    desk.pump();
    assert_eq!(
        (swallowed, desk.client.buttons.clone()),
        (true, vec![(0x100, true), (0x100, false)]),
        "(the shell swallowed the first press, what the window was told of the click after the lock)"
    );
}

fn inside_the_menu(at: Point<f64, Logical>) -> bool {
    (100.0..300.0).contains(&at.x) && (30.0..230.0).contains(&at.y)
}

/// A full-screen scene holding the grab `tray-menu`, whose target
/// is 100..300 x 30..230, with `policy`, over a window at 600,500;
/// focus follows clicks only, so a window focused is one pressed.
fn grabbing(policy: crate::scripted::OutsideClick) -> (Desk, Opened, crate::scripted::SurfaceId) {
    let (mut desk, opened, _) = window_under_a_scene(button_over_the_window);
    desk.state.profile.focus_follows_mouse = false;
    desk.state.remove_surface("shell");
    let mut declared = crate::scripted::Declaration::for_test(
        "menu",
        std::path::PathBuf::from("/nonexistent/hosted-test.qml"),
        Scripted::Top,
        crate::scripted::On::Rect(screen_wide()),
    );
    declared.outside_click = policy;
    desk.state.declare_surface(declared);
    let id = desk.state.surfaces.named("menu").expect("declared");
    desk.state
        .surfaces
        .get_mut(id)
        .expect("live")
        .stand_in(Stand {
            hit: |_| crate::qml::hosted::Hit::Nothing,
            grab: Some(crate::qml::hosted::GrabReport::Held("tray-menu".to_owned())),
            inside: inside_the_menu,
            ..Stand::solid()
        });
    desk.state.settle_scenes();
    assert!(desk.state.hosted_grab.is_some(), "the grab was not taken");
    (desk, opened, id)
}

fn dismissed(desk: &Desk, id: crate::scripted::SurfaceId) -> u32 {
    desk.state
        .surfaces
        .get(id)
        .and_then(crate::scripted::Surface::stand)
        .map_or(0, |stand| stand.dismissed)
}

/// Report `grab` from the stand-in for surface `id`, and settle.
fn report(desk: &mut Desk, id: crate::scripted::SurfaceId, grab: crate::qml::hosted::GrabReport) {
    if let Some(stand) = desk
        .state
        .surfaces
        .get_mut(id)
        .and_then(crate::scripted::Surface::stand_mut)
    {
        stand.grab = Some(grab);
    }
    desk.state.settle_scenes();
}

/// **A press outside a grab dismisses it and is swallowed, by
/// default** (Q2), and so is its release, wherever it lands: the
/// window under it neither takes the keyboard nor is told of a
/// button.
#[test]
fn a_press_outside_a_grab_dismisses_it_and_is_swallowed_by_default() {
    let (mut desk, _, menu) = grabbing(crate::scripted::OutsideClick::default());
    let _pointer = desk.client.seat_pointer(&desk.qh);
    desk.pump();
    desk.state.give_keyboard(None, SERIAL_COUNTER.next_serial());
    move_pointer(&mut desk.state, (630.0, 530.0), 10);
    let region = region(&desk);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, 11);
    move_pointer(&mut desk.state, (631.0, 531.0), 12);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Released, 13);
    desk.pump();
    assert_eq!(
        (
            dismissed(&desk, menu),
            desk.state.hosted_grab.is_none(),
            presses(&desk, menu),
            desk.state.focused_window().is_none(),
            desk.client.buttons.clone(),
        ),
        (1, true, 0, true, Vec::new()),
        "(the grab's dismissals, the grab let go, the presses its scene was told, \
         no window focused, the buttons the window was told)"
    );
}

/// **With `outside_click = "pass"`, the dismissing press reaches
/// what is under it**: the window is clicked, press and release.
#[test]
fn with_outside_click_pass_the_dismissing_press_reaches_the_window_under_it() {
    let policy = crate::scripted::OutsideClick {
        default: crate::scripted::Outside::Pass,
        ..Default::default()
    };
    let (mut desk, opened, menu) = grabbing(policy);
    let _pointer = desk.client.seat_pointer(&desk.qh);
    desk.pump();
    desk.state.give_keyboard(None, SERIAL_COUNTER.next_serial());
    click(&mut desk, (630.0, 530.0), 10);
    desk.pump();
    let focused = desk.state.focused_window().and_then(|window| {
        window
            .wl_surface()
            .map(|surface| surface.id().protocol_id())
    });
    assert_eq!(
        (dismissed(&desk, menu), focused, desk.client.buttons.clone()),
        (
            1,
            Some(window_id(&opened)),
            vec![(0x110, true), (0x110, false)]
        ),
        "(the grab's dismissals, the window focused, the buttons it was told)"
    );
}

/// **A policy named for the grab beats the default.**
#[test]
fn a_policy_named_for_the_grab_beats_the_default() {
    let policy = crate::scripted::OutsideClick {
        default: crate::scripted::Outside::Swallow,
        named: std::collections::BTreeMap::from([(
            "tray-menu".to_owned(),
            crate::scripted::Outside::Pass,
        )]),
    };
    let (mut desk, opened, _) = grabbing(policy);
    desk.state.give_keyboard(None, SERIAL_COUNTER.next_serial());
    click(&mut desk, (630.0, 530.0), 10);
    let focused = desk.state.focused_window().and_then(|window| {
        window
            .wl_surface()
            .map(|surface| surface.id().protocol_id())
    });
    assert_eq!(focused, Some(window_id(&opened)));
}

/// **A press inside the grab's target reaches its scene**, where
/// no item of it takes one, and holds the pointer for it until
/// the release.
#[test]
fn a_press_inside_the_grab_target_reaches_the_scene() {
    let (mut desk, _, menu) = grabbing(crate::scripted::OutsideClick::default());
    click(&mut desk, (150.0, 100.0), 10);
    let released = scene_events(&desk.state, menu)
        .iter()
        .any(|event| matches!(event.kind, PointerKind::Release(0x1)));
    assert_eq!(
        (
            dismissed(&desk, menu),
            presses(&desk, menu),
            released,
            desk.state.hosted_grab.is_some(),
            desk.state.scene_press.is_none(),
        ),
        (0, 1, true, true, true),
        "(the grab's dismissals, the presses its scene was told, the release reached it, \
         the grab still held, the press let go)"
    );
}

/// **A button Qt has no name for, pressed inside a grab, swallows
/// its release**: the scene is not told of it, and when the grab
/// has gone by the release, the window the release lands on is not
/// told of it either.
#[test]
fn an_unnamed_button_pressed_in_a_grab_swallows_its_release() {
    let (mut desk, _, menu) = grabbing(crate::scripted::OutsideClick::default());
    let _pointer = desk.client.seat_pointer(&desk.qh);
    desk.pump();
    move_pointer(&mut desk.state, (150.0, 100.0), 10);
    let region = region(&desk);
    crate::synth::send_button(&mut desk.state, region, 0x100, ButtonState::Pressed, 11);
    report(&mut desk, menu, crate::qml::hosted::GrabReport::Released);
    move_pointer(&mut desk.state, (630.0, 530.0), 12);
    let on_the_window = pointer_on(&desk);
    crate::synth::send_button(&mut desk.state, region, 0x100, ButtonState::Released, 13);
    desk.pump();
    assert_eq!(
        (
            on_the_window.is_some(),
            presses(&desk, menu),
            dismissed(&desk, menu),
            desk.client.buttons.clone(),
        ),
        (true, 0, 0, Vec::new()),
        "(the window had the pointer at the release, the presses the scene was told, \
         the grab's dismissals, the buttons the window was told)"
    );
}

/// **A press a scene holds when a grab begins keeps the pointer
/// until its release** (Ruling 7): a bar button whose press opens
/// a menu hears the motion and the release, the menu's scene hears
/// neither and is not dismissed, and the motion after the release
/// is the menu's.
#[test]
fn a_press_held_when_a_grab_begins_keeps_the_pointer_until_its_release() {
    let (mut desk, _, shell) = window_under_a_scene(button_over_the_window);
    let menu = stand_in(
        &mut desk.state,
        "menu",
        Scripted::Overlay,
        screen_wide(),
        Stand {
            hit: |_| crate::qml::hosted::Hit::Nothing,
            inside: inside_the_menu,
            ..Stand::solid()
        },
    );
    move_pointer(&mut desk.state, (630.0, 520.0), 10);
    let region = region(&desk);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, 11);
    report(
        &mut desk,
        menu,
        crate::qml::hosted::GrabReport::Held("tray-menu".to_owned()),
    );
    move_pointer(&mut desk.state, (150.0, 100.0), 12);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Released, 13);
    let menu_heard_before = scene_events(&desk.state, menu).len();
    move_pointer(&mut desk.state, (151.0, 101.0), 14);
    let shell_heard: Vec<PointerKind> = scene_events(&desk.state, shell)
        .into_iter()
        .map(|event| event.kind)
        .collect();
    assert_eq!(
        (
            shell_heard
                .iter()
                .filter(|kind| matches!(kind, PointerKind::Motion))
                .count(),
            shell_heard.last().copied(),
            menu_heard_before,
            dismissed(&desk, menu),
            desk.state.hosted_grab.is_some(),
            scene_events(&desk.state, menu)
                .iter()
                .any(|event| event.kind == PointerKind::Motion),
        ),
        (2, Some(PointerKind::Release(0x1)), 0, 0, true, true),
        "(the motions the bar heard, the last thing it heard, what the menu heard \
         before the release, the menu's dismissals, the grab still held, the menu \
         heard the motion after the release)"
    );
}

/// **While a grab is held the pointer is the scene's**: it hears
/// the motion anywhere, and no window has the pointer.
#[test]
fn while_a_grab_is_held_the_pointer_is_the_scenes() {
    let (mut desk, _, menu) = grabbing(crate::scripted::OutsideClick::default());
    move_pointer(&mut desk.state, (630.0, 530.0), 10);
    let focus = desk
        .state
        .seat
        .get_pointer()
        .and_then(|pointer| pointer.current_focus());
    assert_eq!(
        (
            pointer_on(&desk),
            focus.is_none(),
            scene_events(&desk.state, menu)
                .iter()
                .any(|event| event.kind == PointerKind::Motion),
        ),
        (None, true, true),
        "(the surface under the pointer, the seat's pointer on nothing, the scene heard the motion)"
    );
}

/// **While a grab is held no window takes the keyboard, the frame
/// or the cursor from the pointer** (Ruling 8): with focus
/// following the mouse the window under the pointer is not
/// focused, its frame is kept from it, and its corner offers no
/// resize, since a press there would dismiss the grab.
#[test]
fn while_a_grab_is_held_no_window_takes_focus_frame_or_cursor_from_the_pointer() {
    let (mut desk, _, _) = grabbing(crate::scripted::OutsideClick::default());
    desk.state.give_keyboard(None, SERIAL_COUNTER.next_serial());
    desk.state.profile.focus_follows_mouse = true;
    let corner = Point::<f64, Logical>::from((663.0, 563.0));
    move_pointer(&mut desk.state, (corner.x, corner.y), 10);
    assert_eq!(
        (
            desk.state.focused_window().is_none(),
            desk.state.frames_kept_from(corner),
            desk.state.claim_under(corner).cursor().is_none(),
        ),
        (true, true, true),
        "(no window focused, the frame kept from the pointer, no resize offered)"
    );
}

/// **While a grab is held the wheel is the scene's**, over the
/// window too.
#[test]
fn while_a_grab_is_held_the_wheel_is_the_scenes() {
    let (mut desk, _, menu) = grabbing(crate::scripted::OutsideClick::default());
    move_pointer(&mut desk.state, (630.0, 530.0), 10);
    let region = region(&desk);
    crate::synth::send_axis(&mut desk.state, region, (0.0, 120.0), 11);
    assert!(
        scene_events(&desk.state, menu)
            .iter()
            .any(|event| matches!(event.kind, PointerKind::Wheel { .. })),
        "the grab's scene did not hear the wheel"
    );
}

/// **One grab is held at a time** (Ruling 12): a grab another
/// surface's scene takes dismisses the one held.
#[test]
fn a_grab_another_scene_takes_dismisses_the_one_held() {
    let (mut desk, _, menu) = grabbing(crate::scripted::OutsideClick::default());
    let search = stand_in(
        &mut desk.state,
        "search",
        Scripted::Overlay,
        screen_wide(),
        Stand {
            hit: |_| crate::qml::hosted::Hit::Nothing,
            ..Stand::solid()
        },
    );
    report(
        &mut desk,
        search,
        crate::qml::hosted::GrabReport::Held("search".to_owned()),
    );
    let held = desk
        .state
        .hosted_grab
        .as_ref()
        .map(|grab| (grab.surface, grab.name.clone()));
    assert_eq!(
        (dismissed(&desk, menu), held),
        (1, Some((search, "search".to_owned()))),
        "(the first grab's dismissals, the grab held)"
    );
}

/// **A grab follows its scene to where it is drawn now**: the menu's
/// surface declared again further right hears the pointer in its
/// new coordinates.
#[test]
fn a_grab_follows_its_scene_to_where_it_is_drawn() {
    let (mut desk, _, menu) = grabbing(crate::scripted::OutsideClick::default());
    let mut moved = crate::scripted::Declaration::for_test(
        "menu",
        std::path::PathBuf::from("/nonexistent/hosted-test.qml"),
        Scripted::Top,
        crate::scripted::On::Rect(Rectangle::new((100, 0).into(), (1820, 1080).into())),
    );
    moved.outside_click = crate::scripted::OutsideClick::default();
    desk.state.declare_surface(moved);
    move_pointer(&mut desk.state, (150.0, 100.0), 10);
    let last = desk
        .state
        .surfaces
        .get(menu)
        .and_then(crate::scripted::Surface::stand)
        .and_then(|stand| stand.seen.last().map(|(at, _)| *at));
    assert_eq!(
        last,
        Some(Point::from((50.0, 100.0))),
        "where in its own coordinates the moved scene heard the pointer"
    );
}

/// **A surface taken away lets go of its grab**: the window under
/// where it was is clicked again.
#[test]
fn a_surface_taken_away_lets_go_of_its_grab() {
    let (mut desk, opened, _) = grabbing(crate::scripted::OutsideClick::default());
    desk.state.remove_surface("menu");
    desk.state.give_keyboard(None, SERIAL_COUNTER.next_serial());
    click(&mut desk, (630.0, 530.0), 10);
    let focused = desk.state.focused_window().and_then(|window| {
        window
            .wl_surface()
            .map(|surface| surface.id().protocol_id())
    });
    assert_eq!(
        (desk.state.hosted_grab.is_none(), focused),
        (true, Some(window_id(&opened))),
        "(the grab let go, the window the click focused)"
    );
}

/// The surface the seat's pointer is on, the one told `enter`
/// last, by its protocol id.
fn pointer_focus(desk: &Desk) -> Option<u32> {
    desk.state
        .seat
        .get_pointer()
        .and_then(|pointer| pointer.current_focus())
        .map(|surface| surface.id().protocol_id())
}

/// Press and let go of the left button where the pointer is, with
/// no motion before it.
fn click_here(desk: &mut Desk, time: u64) {
    let region = region(desk);
    for (button_state, at) in [
        (ButtonState::Pressed, time),
        (ButtonState::Released, time + 1),
    ] {
        crate::synth::send_button(&mut desk.state, region, 0x110, button_state, at);
    }
}

/// **A swallowed outside press gives the pointer back to the
/// window under it at once** (Ruling 12): the window has the
/// pointer while the button is still down, and the next click
/// there, with no motion before it, reaches it, as a second click
/// after the one that closed a menu does.
#[test]
fn a_swallowed_outside_press_gives_the_pointer_back_to_the_window_under_it() {
    let (mut desk, opened, menu) = grabbing(crate::scripted::OutsideClick::default());
    let _pointer = desk.client.seat_pointer(&desk.qh);
    desk.pump();
    move_pointer(&mut desk.state, (630.0, 530.0), 10);
    let held = pointer_focus(&desk);
    let region = region(&desk);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, 11);
    let swallowed = pointer_focus(&desk);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Released, 12);
    click_here(&mut desk, 13);
    desk.pump();
    assert_eq!(
        (
            held,
            dismissed(&desk, menu),
            swallowed,
            desk.client.buttons.clone()
        ),
        (
            None,
            1,
            Some(window_id(&opened)),
            vec![(0x110, true), (0x110, false)]
        ),
        "(the pointer's surface while the grab was held, the grab's dismissals, \
         its surface once the press was swallowed, the buttons the window was told)"
    );
}

/// **A popup that closes by itself gives the pointer back to the
/// window under it at once** (Ruling 12): with the pointer resting
/// over the window, the scene letting go of its grab, as Escape or
/// a timer closing the popup does, gives the window the pointer,
/// and the next click there, with no motion before it, reaches it.
#[test]
fn a_popup_that_closes_by_itself_gives_the_pointer_back_to_the_window_under_it() {
    let (mut desk, opened, menu) = grabbing(crate::scripted::OutsideClick::default());
    let _pointer = desk.client.seat_pointer(&desk.qh);
    desk.pump();
    move_pointer(&mut desk.state, (630.0, 530.0), 10);
    let held = pointer_focus(&desk);
    report(&mut desk, menu, crate::qml::hosted::GrabReport::Released);
    let closed = pointer_focus(&desk);
    click_here(&mut desk, 11);
    desk.pump();
    assert_eq!(
        (
            held,
            closed,
            dismissed(&desk, menu),
            desk.client.buttons.clone()
        ),
        (
            None,
            Some(window_id(&opened)),
            0,
            vec![(0x110, true), (0x110, false)]
        ),
        "(the pointer's surface while the grab was held, its surface once the \
         scene let go, the grab's dismissals, the buttons the window was told)"
    );
}

/// A grab's target over the window at 600,500.
fn inside_over_the_window(at: Point<f64, Logical>) -> bool {
    (600.0..664.0).contains(&at.x) && (500.0..564.0).contains(&at.y)
}

/// **A popup closed during a press inside it gives the pointer
/// back at the release** (Ruling 12): with the pointer resting
/// over the window, a grab the scene lets go of while it holds a
/// press, as a popup closed from its own `onPressed` is, leaves
/// the pointer with the scene until the button is up, and then
/// gives it to the window, so the next click there, with no motion
/// before it, reaches it.
#[test]
fn a_popup_closed_during_a_press_inside_it_gives_the_pointer_back_at_the_release() {
    let (mut desk, opened, menu) = grabbing(crate::scripted::OutsideClick::default());
    if let Some(stand) = desk
        .state
        .surfaces
        .get_mut(menu)
        .and_then(crate::scripted::Surface::stand_mut)
    {
        stand.inside = inside_over_the_window;
    }
    let _pointer = desk.client.seat_pointer(&desk.qh);
    desk.pump();
    move_pointer(&mut desk.state, (630.0, 530.0), 10);
    let region = region(&desk);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, 11);
    report(&mut desk, menu, crate::qml::hosted::GrabReport::Released);
    let pressed = pointer_focus(&desk);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Released, 12);
    let released = pointer_focus(&desk);
    click_here(&mut desk, 13);
    desk.pump();
    assert_eq!(
        (
            presses(&desk, menu),
            pressed,
            released,
            desk.state.hosted_grab.is_none(),
            desk.client.buttons.clone()
        ),
        (
            1,
            None,
            Some(window_id(&opened)),
            true,
            vec![(0x110, true), (0x110, false)]
        ),
        "(the presses the scene took, the pointer's surface while its press was \
         held, its surface after the release, the grab let go, the buttons the \
         window was told)"
    );
}

/// **A surface taken away gives the pointer back to the window
/// under it at once**, with the grab it held, before the pointer
/// moves.
#[test]
fn a_surface_taken_away_gives_the_pointer_back_to_the_window_under_it() {
    let (mut desk, opened, _) = grabbing(crate::scripted::OutsideClick::default());
    move_pointer(&mut desk.state, (630.0, 530.0), 10);
    let held = pointer_focus(&desk);
    desk.state.remove_surface("menu");
    assert_eq!(
        (held, desk.state.hosted_grab.is_none(), pointer_focus(&desk)),
        (None, true, Some(window_id(&opened))),
        "(the pointer's surface while the grab was held, the grab let go, its \
         surface after)"
    );
}

fn window(desk: &Desk, opened: &Opened) -> Window {
    desk.state
        .panes
        .get(opened.pane)
        .and_then(crate::pane::Pane::client)
        .cloned()
        .expect("a client")
}

/// The surface the keyboard is on, by its protocol id.
fn keyboard_on(desk: &Desk) -> Option<u32> {
    desk.state
        .seat
        .get_keyboard()
        .and_then(|keyboard| keyboard.current_focus())
        .map(|surface| surface.id().protocol_id())
}

/// Report `wants` from the stand-in for surface `id`, and settle.
fn want(
    desk: &mut Desk,
    id: crate::scripted::SurfaceId,
    wants: crate::qml::hosted::KeyboardReport,
) {
    if let Some(stand) = desk
        .state
        .surfaces
        .get_mut(id)
        .and_then(crate::scripted::Surface::stand_mut)
    {
        stand.keyboard = Some(wants);
    }
    desk.state.settle_scenes();
}

fn let_go(desk: &Desk, id: crate::scripted::SurfaceId) -> u32 {
    desk.state
        .surfaces
        .get(id)
        .and_then(crate::scripted::Surface::stand)
        .map_or(0, |stand| stand.let_go)
}

/// **The window loses the keyboard while the shell holds it, still
/// reads as focused, and gets the keyboard back when the shell lets
/// go** (Ruling 14).
#[test]
fn the_window_gets_the_keyboard_back_when_the_shell_lets_go() {
    let (mut desk, opened, shell) = window_under_a_scene(button_over_the_window);
    let window = window(&desk, &opened);
    desk.state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    want(
        &mut desk,
        shell,
        crate::qml::hosted::KeyboardReport::Wanted(vec!["Escape".to_owned()]),
    );
    let held = (
        keyboard_on(&desk),
        desk.state.focused_window() == Some(window.clone()),
        desk.state.looks_focused(opened.pane),
    );
    want(&mut desk, shell, crate::qml::hosted::KeyboardReport::LetGo);
    assert_eq!(
        (
            held,
            keyboard_on(&desk),
            desk.state.hosted_keyboard.is_none()
        ),
        ((None, true, true), Some(window_id(&opened)), true),
        "((the keyboard while held, the window read as focused, drawn focused), \
         the keyboard after, the hold over)"
    );
}

/// **Clicking a window ends the shell's hold** (Ruling 14): the
/// scene is told to let go, and the window clicked has the
/// keyboard.
#[test]
fn clicking_a_window_ends_the_shells_hold() {
    let (mut desk, opened, shell) = window_under_a_scene(button_over_the_window);
    desk.state.profile.focus_follows_mouse = false;
    want(
        &mut desk,
        shell,
        crate::qml::hosted::KeyboardReport::Wanted(Vec::new()),
    );
    let held = desk.state.hosted_keyboard.is_some();
    click(&mut desk, (630.0, 555.0), 10);
    assert_eq!(
        (
            held,
            desk.state.hosted_keyboard.is_none(),
            let_go(&desk, shell),
            keyboard_on(&desk)
        ),
        (true, true, 1, Some(window_id(&opened))),
        "(held, the hold over, the times the scene was told to let go, the keyboard)"
    );
}

/// **Focus does not follow the mouse away from a scene holding
/// the keyboard** (Ruling 14): with focus following the mouse, as
/// a desktop has it, the pointer crossing another window leaves
/// the keyboard with the scene, the scene is not told to let go,
/// and the window the keyboard came from still reads as focused.
/// A click is what takes it back
/// (`clicking_a_window_ends_the_shells_hold`).
#[test]
fn the_pointer_crossing_a_window_does_not_end_the_shells_hold() {
    let (mut desk, _, shell) = window_under_a_scene(button_over_the_window);
    let other = desk.open_surface();
    let other_window = window(&desk, &other);
    desk.state
        .space
        .map_element(other_window.clone(), (100, 100), false);
    desk.state.space.refresh();
    desk.state.focus_window(
        &other_window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    desk.state.profile.focus_follows_mouse = true;
    want(
        &mut desk,
        shell,
        crate::qml::hosted::KeyboardReport::Wanted(Vec::new()),
    );
    move_pointer(&mut desk.state, (630.0, 555.0), 10);
    assert_eq!(
        (
            desk.state.hosted_keyboard.is_some(),
            let_go(&desk, shell),
            keyboard_on(&desk),
            desk.focused(),
        ),
        (true, 0, None, other.pane),
        "(still held, the times the scene was told to let go, the keyboard, the \
         window read as focused)"
    );
}

/// **`sol.focus` ends the shell's hold**, as every explicit focus
/// does (Ruling 14), and the window it focuses has the keyboard.
#[test]
fn sol_focus_ends_the_shells_hold() {
    let (mut desk, opened, shell) = window_under_a_scene(button_over_the_window);
    desk.install(r#"sol.bind("super+f", function() sol.focus(sol.windows()[1].id) end)"#);
    want(
        &mut desk,
        shell,
        crate::qml::hosted::KeyboardReport::Wanted(Vec::new()),
    );
    assert!(desk.state.trigger("super+f"), "the binding was not handled");
    assert_eq!(
        (
            desk.state.hosted_keyboard.is_none(),
            let_go(&desk, shell),
            keyboard_on(&desk)
        ),
        (true, 1, Some(window_id(&opened))),
        "(the hold over, the times the scene was told to let go, the keyboard)"
    );
}

/// **A scene that takes the keyboard from another gives it back to
/// the window the first took it from**: the first scene is told to
/// let go, and the window is not forgotten on the way.
#[test]
fn a_hold_another_scene_takes_returns_to_the_window_the_first_took_it_from() {
    let (mut desk, opened, menu) = window_under_a_scene(button_over_the_window);
    let window = window(&desk, &opened);
    desk.state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    let search = stand_in(
        &mut desk.state,
        "search",
        Scripted::Top,
        screen_wide(),
        Stand {
            hit: |_| crate::qml::hosted::Hit::Nothing,
            ..Stand::solid()
        },
    );
    want(
        &mut desk,
        menu,
        crate::qml::hosted::KeyboardReport::Wanted(Vec::new()),
    );
    want(
        &mut desk,
        search,
        crate::qml::hosted::KeyboardReport::Wanted(Vec::new()),
    );
    let holder = desk.state.hosted_keyboard.as_ref().map(|held| held.surface);
    want(&mut desk, search, crate::qml::hosted::KeyboardReport::LetGo);
    assert_eq!(
        (holder, let_go(&desk, menu), keyboard_on(&desk)),
        (Some(search), 1, Some(window_id(&opened))),
        "(the holder, the first scene told to let go, the keyboard at the end)"
    );
}

/// **Nothing settles the keyboard onto a window while the shell
/// holds it**: with no window to give it back to, the keyboard is
/// the scene's until it lets go, whatever looks for a window to
/// focus meanwhile; then the window under the pointer has it.
#[test]
fn nothing_settles_the_keyboard_onto_a_window_while_the_shell_holds_it() {
    let (mut desk, opened, shell) = window_under_a_scene(button_over_the_window);
    desk.state.give_keyboard(None, SERIAL_COUNTER.next_serial());
    move_pointer(&mut desk.state, (630.0, 555.0), 10);
    desk.state.give_keyboard(None, SERIAL_COUNTER.next_serial());
    want(
        &mut desk,
        shell,
        crate::qml::hosted::KeyboardReport::Wanted(Vec::new()),
    );
    desk.state.settle_focus();
    let held = (desk.state.hosted_keyboard.is_some(), keyboard_on(&desk));
    want(&mut desk, shell, crate::qml::hosted::KeyboardReport::LetGo);
    assert_eq!(
        (held, keyboard_on(&desk)),
        ((true, None), Some(window_id(&opened))),
        "((held after the settle, the keyboard then), the keyboard once let go)"
    );
}

/// **A surface taken away while its scene holds the keyboard gives
/// it back**: the window it came from has it again.
#[test]
fn a_surface_taken_away_gives_the_keyboard_back() {
    let (mut desk, opened, shell) = window_under_a_scene(button_over_the_window);
    let window = window(&desk, &opened);
    desk.state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    want(
        &mut desk,
        shell,
        crate::qml::hosted::KeyboardReport::Wanted(Vec::new()),
    );
    desk.state.remove_surface("shell");
    assert_eq!(
        (desk.state.hosted_keyboard.is_none(), keyboard_on(&desk)),
        (true, Some(window_id(&opened))),
        "(the hold over, the keyboard)"
    );
}

/// **A surface the pointer does not reach holds no keyboard**, as
/// it holds no grab: nothing could click it to take the keyboard
/// back, so the window keeps it, on Russian (#132).
#[test]
fn a_surface_the_pointer_does_not_reach_holds_no_keyboard() {
    let (mut desk, opened, _) = window_under_a_scene(button_over_the_window);
    let window = window(&desk, &opened);
    desk.state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    let mut declared = crate::scripted::Declaration::for_test(
        "osd",
        std::path::PathBuf::from("/nonexistent/hosted-test.qml"),
        Scripted::Overlay,
        crate::scripted::On::Rect(screen_wide()),
    );
    declared.interactive = false;
    desk.state.declare_surface(declared);
    let osd = desk.state.surfaces.named("osd").expect("declared");
    desk.state
        .surfaces
        .get_mut(osd)
        .expect("live")
        .stand_in(Stand {
            hit: |_| crate::qml::hosted::Hit::Nothing,
            ..Stand::solid()
        });
    want(
        &mut desk,
        osd,
        crate::qml::hosted::KeyboardReport::Wanted(vec!["Escape".to_owned()]),
    );
    assert_eq!(
        (desk.state.hosted_keyboard.is_none(), keyboard_on(&desk)),
        (true, Some(window_id(&opened))),
        "(no hold, the keyboard)"
    );
}

/// **A surface declared again out of the pointer's reach while its
/// scene holds the keyboard gives it back**: the scene is told to
/// let go, and the window it came from has the keyboard again.
#[test]
fn a_surface_declared_again_out_of_the_pointers_reach_gives_the_keyboard_back() {
    let (mut desk, opened, shell) = window_under_a_scene(button_over_the_window);
    let window = window(&desk, &opened);
    desk.state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    want(
        &mut desk,
        shell,
        crate::qml::hosted::KeyboardReport::Wanted(Vec::new()),
    );
    let held = desk.state.hosted_keyboard.is_some();
    let mut declared = desk
        .state
        .surfaces
        .get(shell)
        .expect("live")
        .declared
        .clone();
    declared.interactive = false;
    desk.state.declare_surface(declared);
    assert_eq!(
        (
            held,
            desk.state.hosted_keyboard.is_none(),
            let_go(&desk, shell),
            keyboard_on(&desk)
        ),
        (true, true, 1, Some(window_id(&opened))),
        "(held, the hold over, the times the scene was told to let go, the keyboard)"
    );
}

/// **A search popup holding the pointer and the keyboard at once
/// gives both back when it is dismissed** (Rulings 12 to 14), on
/// Russian (#132): the field types Cyrillic, Escape is the field's
/// and not its binding's, a press outside dismisses the popup and
/// is swallowed while the field still holds the keyboard, the
/// window has the pointer at once and the keyboard once the field
/// lets go, and the next click there, with no motion before it,
/// reaches it.
#[test]
fn a_search_popup_holding_the_pointer_and_the_keyboard_gives_both_back_when_dismissed() {
    const Q: u32 = 24;
    const ESCAPE: u32 = 9;
    let (mut desk, opened, menu) = grabbing(crate::scripted::OutsideClick::default());
    desk.install(r#"sol.bind("Escape", function() sol.status("bound") end)"#);
    let _pointer = desk.client.seat_pointer(&desk.qh);
    desk.pump();
    let window = window(&desk, &opened);
    desk.state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    want(
        &mut desk,
        menu,
        crate::qml::hosted::KeyboardReport::Wanted(vec!["Escape".to_owned()]),
    );
    move_pointer(&mut desk.state, (630.0, 530.0), 10);
    for (code, key_state, time) in [
        (Q, KeyState::Pressed, 11),
        (Q, KeyState::Released, 12),
        (ESCAPE, KeyState::Pressed, 13),
        (ESCAPE, KeyState::Released, 14),
    ] {
        crate::input::key(&mut desk.state, code.into(), key_state, time);
    }
    let told: Vec<(u32, i32)> = desk
        .state
        .scene_keys
        .iter()
        .filter(|key| key.pressed)
        .map(|key| (key.code, key.qt_key))
        .collect();
    let typed = desk
        .state
        .scene_keys
        .first()
        .map(|key| key.text.clone())
        .unwrap_or_default();
    let held = (
        desk.state.hosted_grab.is_some(),
        desk.state.hosted_keyboard.is_some(),
        keyboard_on(&desk),
        pointer_focus(&desk),
    );
    let region = region(&desk);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, 15);
    let pressed = (
        dismissed(&desk, menu),
        desk.state.hosted_grab.is_none(),
        desk.state.hosted_keyboard.is_some(),
        pointer_focus(&desk),
    );
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Released, 16);
    want(&mut desk, menu, crate::qml::hosted::KeyboardReport::LetGo);
    let closed = keyboard_on(&desk);
    click_here(&mut desk, 17);
    desk.pump();
    let window = Some(window_id(&opened));
    assert_eq!(
        (
            told,
            typed,
            std::mem::take(&mut desk.state.status),
            held,
            pressed,
            closed,
            desk.client.buttons.clone(),
        ),
        (
            vec![(Q, 0x0419), (ESCAPE, 0x0100_0000)],
            "й".to_owned(),
            String::new(),
            (true, true, None, None),
            (1, true, true, window),
            window,
            vec![(0x110, true), (0x110, false)],
        ),
        "(the keys the field was told, by keycode and Qt key, what it typed, what \
         the binding left, (the grab held, the keyboard held, the keyboard's surface, \
         the pointer's) while open, (the dismissals, the grab let go, the keyboard \
         still held, the pointer's surface) at the outside press, the keyboard's \
         surface once the field let go, the buttons the window was told)"
    );
}

/// **A monitor unplugged while its scene holds the keyboard gives
/// it back**: the hold goes with its monitor, though the surface
/// is still declared, and the window it came from has the
/// keyboard again.
#[test]
fn a_monitor_unplugged_while_its_scene_holds_the_keyboard_gives_it_back() {
    let (mut desk, opened, _) = window_under_a_scene(button_over_the_window);
    let window = window(&desk, &opened);
    desk.state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    let right = a_screen(&mut desk.state, "hold-right", (1920, 0));
    desk.state.settle_monitors();
    let search = stand_in(
        &mut desk.state,
        "search",
        Scripted::Top,
        Rectangle::new((1920, 0).into(), (1920, 1080).into()),
        Stand {
            hit: |_| crate::qml::hosted::Hit::Nothing,
            ..Stand::solid()
        },
    );
    want(
        &mut desk,
        search,
        crate::qml::hosted::KeyboardReport::Wanted(Vec::new()),
    );
    let held = desk
        .state
        .hosted_keyboard
        .as_ref()
        .map(|held| held.output.name());
    desk.state.space.unmap_output(&right);
    desk.state.settle_monitors();
    desk.state.settle_scenes();
    assert_eq!(
        (
            held,
            desk.state.hosted_keyboard.is_none(),
            keyboard_on(&desk)
        ),
        (
            Some("hold-right".to_owned()),
            true,
            Some(window_id(&opened))
        ),
        "(the monitor the hold was on, the hold over, the keyboard)"
    );
}

/// **The bindings policy is the surface's as it is declared now**:
/// one redeclared while its scene holds the keyboard applies from
/// the next key, so `bindings = "none"` gives the scene even a
/// `super` binding's key, on Russian (#132).
#[test]
fn a_bindings_policy_redeclared_while_the_shell_holds_the_keyboard_applies_at_once() {
    const Q: u32 = 24;
    let (mut desk, _, shell) = window_under_a_scene(button_over_the_window);
    desk.install(r#"sol.bind("super+q", function() sol.status("super+q") end)"#);
    want(
        &mut desk,
        shell,
        crate::qml::hosted::KeyboardReport::Wanted(Vec::new()),
    );
    desk.state.declare_surface(crate::scripted::Declaration {
        keyboard: crate::scripted::KeyPolicy::NoBindings,
        ..crate::scripted::Declaration::for_test(
            "shell",
            std::path::PathBuf::from("/nonexistent/hosted-test.qml"),
            Scripted::Top,
            crate::scripted::On::Rect(screen_wide()),
        )
    });
    for (code, key_state, time) in [
        (SUPER_L, KeyState::Pressed, 20),
        (Q, KeyState::Pressed, 21),
        (Q, KeyState::Released, 22),
        (SUPER_L, KeyState::Released, 23),
    ] {
        crate::input::key(&mut desk.state, code.into(), key_state, time);
    }
    assert_eq!(
        (
            desk.state.hosted_keyboard.is_some(),
            std::mem::take(&mut desk.state.status),
            desk.state
                .scene_keys
                .iter()
                .any(|key| key.pressed && key.code == Q),
        ),
        (true, String::new(), true),
        "(still held, what the binding left, the key told the scene)"
    );
}

/// **A window that leaves while the shell holds the keyboard is not
/// given it back**: its close was asked for, and when the shell
/// lets go the keyboard goes where it would have gone at the close,
/// to nothing, since no other window is open.
#[test]
fn a_window_closed_while_the_shell_holds_the_keyboard_is_not_given_it_back() {
    let (mut desk, opened, shell) = window_under_a_scene(button_over_the_window);
    let window = window(&desk, &opened);
    desk.state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    want(
        &mut desk,
        shell,
        crate::qml::hosted::KeyboardReport::Wanted(Vec::new()),
    );
    desk.state.close_pane(opened.pane);
    let _ = desk.ask();
    want(&mut desk, shell, crate::qml::hosted::KeyboardReport::LetGo);
    assert_eq!(
        (desk.state.hosted_keyboard.is_none(), keyboard_on(&desk)),
        (true, None),
        "(the hold over, the keyboard)"
    );
}

/// **Locking the session dismisses a hosted grab**, as every grab is
/// released at the lock.
#[test]
fn locking_the_session_dismisses_a_hosted_grab() {
    let (mut desk, _, menu) = grabbing(crate::scripted::OutsideClick::default());
    desk.state.release_grabs();
    assert_eq!(
        (dismissed(&desk, menu), desk.state.hosted_grab.is_none()),
        (1, true),
        "(the grab's dismissals, the grab let go)"
    );
}

/// **A grab suspends a client's pointer lock, and the lock comes
/// back once the grab is gone** (Ruling 12): the screenshot UI over
/// a game needs the pointer (02, primitive 5). A lock is granted
/// at the motion after the one that entered the window, and holds
/// the pointer where that motion found it.
#[test]
fn a_grab_suspends_a_pointer_lock_and_the_lock_comes_back_after() {
    use zwp_pointer_constraints_v1::Lifetime;
    let (mut desk, opened, menu) = grabbing(crate::scripted::OutsideClick::default());
    report(&mut desk, menu, crate::qml::hosted::GrabReport::Released);
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
    let at = |desk: &Desk| {
        desk.state
            .seat
            .get_pointer()
            .map(|pointer| pointer.current_location())
            .unwrap_or_default()
    };
    move_pointer(&mut desk.state, (630.0, 530.0), 20);
    move_pointer(&mut desk.state, (631.0, 531.0), 21);
    move_pointer(&mut desk.state, (700.0, 700.0), 22);
    let locked = at(&desk);

    report(
        &mut desk,
        menu,
        crate::qml::hosted::GrabReport::Held("shot".to_owned()),
    );
    move_pointer(&mut desk.state, (700.0, 700.0), 23);
    let grabbed = at(&desk);

    report(&mut desk, menu, crate::qml::hosted::GrabReport::Released);
    move_pointer(&mut desk.state, (640.0, 540.0), 24);
    move_pointer(&mut desk.state, (641.0, 541.0), 25);
    move_pointer(&mut desk.state, (800.0, 800.0), 26);
    assert_eq!(
        (locked, grabbed, at(&desk)),
        (
            Point::from((630.0, 530.0)),
            Point::from((700.0, 700.0)),
            Point::from((640.0, 540.0)),
        ),
        "(where the lock held the pointer, where it went while a grab was held, \
         where the lock held it after the grab)"
    );
}

/// **A grab that begins while a button is held on a game whose
/// pointer is locked has the pointer once that button is let go
/// of** (Ruling 12): a screenshot key pressed while firing. The
/// press is the game's, so its release is too, and from the
/// motion after it the pointer moves and is the grab's scene's.
#[test]
fn a_grab_begun_during_a_press_on_a_locked_window_has_the_pointer_after_the_release() {
    use zwp_pointer_constraints_v1::Lifetime;
    let (mut desk, opened, menu) = grabbing(crate::scripted::OutsideClick::default());
    report(&mut desk, menu, crate::qml::hosted::GrabReport::Released);
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
    let at = |desk: &Desk| {
        desk.state
            .seat
            .get_pointer()
            .map(|pointer| pointer.current_location())
            .unwrap_or_default()
    };
    move_pointer(&mut desk.state, (630.0, 530.0), 20);
    move_pointer(&mut desk.state, (631.0, 531.0), 21);
    let locked = at(&desk);
    let region = region(&desk);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, 22);
    report(
        &mut desk,
        menu,
        crate::qml::hosted::GrabReport::Held("shot".to_owned()),
    );
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Released, 23);
    desk.pump();
    let before = scene_events(&desk.state, menu).len();
    move_pointer(&mut desk.state, (700.0, 700.0), 24);
    move_pointer(&mut desk.state, (701.0, 701.0), 25);
    let heard = scene_events(&desk.state, menu)
        .iter()
        .skip(before)
        .filter(|event| event.kind == PointerKind::Motion)
        .count();
    desk.pump();
    assert_eq!(
        (
            locked,
            desk.client.buttons.clone(),
            at(&desk),
            heard,
            desk.state.hosted_grab.is_some(),
        ),
        (
            Point::from((630.0, 530.0)),
            vec![(0x110, true), (0x110, false)],
            Point::from((701.0, 701.0)),
            2,
            true,
        ),
        "(where the lock held the pointer, the buttons the game was told, where the \
         pointer is after the release, the motions the grab's scene heard after it, \
         the grab still held)"
    );
}

/// **A grab is let go of once no scene of its surface is on its
/// monitor** (Ruling 12), though the surface is still declared
/// there: the window under where the menu was is clicked again.
#[test]
fn a_scene_that_goes_from_a_monitor_lets_go_of_its_grab() {
    let (mut desk, opened, menu) = grabbing(crate::scripted::OutsideClick::default());
    if let Some(stand) = desk
        .state
        .surfaces
        .get_mut(menu)
        .and_then(crate::scripted::Surface::stand_mut)
    {
        stand.hosts = false;
    }
    desk.state.settle_scenes();
    desk.state.give_keyboard(None, SERIAL_COUNTER.next_serial());
    click(&mut desk, (630.0, 530.0), 10);
    let focused = desk.state.focused_window().and_then(|window| {
        window
            .wl_surface()
            .map(|surface| surface.id().protocol_id())
    });
    assert_eq!(
        (desk.state.hosted_grab.is_none(), focused),
        (true, Some(window_id(&opened))),
        "(the grab let go, the window the click focused)"
    );
}

/// **A surface the pointer does not reach holds no grab**, nor
/// does its grab dismiss the one held: `interactive = false` is
/// "the pointer reaches it not at all", a `Grab` in its scene too.
#[test]
fn a_surface_the_pointer_does_not_reach_holds_no_grab() {
    let (mut desk, _, menu) = grabbing(crate::scripted::OutsideClick::default());
    let mut declared = crate::scripted::Declaration::for_test(
        "osd",
        std::path::PathBuf::from("/nonexistent/hosted-test.qml"),
        Scripted::Overlay,
        crate::scripted::On::Rect(screen_wide()),
    );
    declared.interactive = false;
    desk.state.declare_surface(declared);
    let osd = desk.state.surfaces.named("osd").expect("declared");
    desk.state
        .surfaces
        .get_mut(osd)
        .expect("live")
        .stand_in(Stand {
            hit: |_| crate::qml::hosted::Hit::Nothing,
            ..Stand::solid()
        });
    report(
        &mut desk,
        osd,
        crate::qml::hosted::GrabReport::Held("osd".to_owned()),
    );
    let held = desk.state.hosted_grab.as_ref().map(|grab| grab.surface);
    assert_eq!(
        (held, dismissed(&desk, menu), dismissed(&desk, osd)),
        (Some(menu), 0, 0),
        "(whose grab is held, the menu's dismissals, the osd's)"
    );
}

/// **A surface declared again out of the pointer's reach dismisses
/// the grab it held**: the menu is closed, not left open in a
/// scene whose grab the compositor no longer holds and that will
/// not be told of again, which a later press outside it would then
/// not close.
#[test]
fn a_surface_declared_again_out_of_the_pointers_reach_dismisses_the_grab_it_held() {
    let (mut desk, _, menu) = grabbing(crate::scripted::OutsideClick::default());
    let mut declared = desk
        .state
        .surfaces
        .get(menu)
        .expect("live")
        .declared
        .clone();
    declared.interactive = false;
    desk.state.declare_surface(declared);
    assert_eq!(
        (desk.state.hosted_grab.is_none(), dismissed(&desk, menu)),
        (true, 1),
        "(the grab let go, the menu's dismissals)"
    );
}

/// **A grab that begins while a button is held on a game whose
/// pointer is locked lets go of the lock at once** (Ruling 12):
/// the press keeps the pointer on the game until the release, and
/// the pointer moves meanwhile, so the game is told it is unlocked
/// before it is told of that motion, and after the release it has
/// the pointer no longer.
#[test]
fn a_grab_begun_during_a_press_on_a_locked_window_lets_go_of_the_lock_at_once() {
    use smithay::wayland::pointer_constraints::with_pointer_constraint;
    use zwp_pointer_constraints_v1::Lifetime;
    let (mut desk, opened, menu) = grabbing(crate::scripted::OutsideClick::default());
    report(&mut desk, menu, crate::qml::hosted::GrabReport::Released);
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
    move_pointer(&mut desk.state, (630.0, 530.0), 20);
    move_pointer(&mut desk.state, (631.0, 531.0), 21);
    let region = region(&desk);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, 22);
    let lock_of = |desk: &Desk| {
        let pointer = desk.state.seat.get_pointer().expect("a pointer");
        pointer.current_focus().map(|surface| {
            (
                surface.id().protocol_id(),
                with_pointer_constraint(&surface, &pointer, |constraint| {
                    constraint.is_some_and(|constraint| constraint.is_active())
                }),
            )
        })
    };
    let before = lock_of(&desk);
    report(
        &mut desk,
        menu,
        crate::qml::hosted::GrabReport::Held("shot".to_owned()),
    );
    move_pointer(&mut desk.state, (700.0, 700.0), 23);
    let during = lock_of(&desk);
    let moved = desk
        .state
        .seat
        .get_pointer()
        .map(|pointer| pointer.current_location())
        .unwrap_or_default();
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Released, 24);
    move_pointer(&mut desk.state, (710.0, 710.0), 25);
    let after = lock_of(&desk);
    let game = window_id(&opened);
    assert_eq!(
        (before, during, moved, after),
        (
            Some((game, true)),
            Some((game, false)),
            Point::from((700.0, 700.0)),
            None
        ),
        "(the game's lock before the grab, during the press after the grab began, \
         where the pointer moved to meanwhile, after the release)"
    );
}

/// A surface on every monitor reserving `reserve`, its scene stood
/// in for by `stand`.
fn reserving(
    desk: &mut Desk,
    name: &str,
    reserve: crate::scripted::Edges,
    stand: Stand,
) -> crate::scripted::SurfaceId {
    let mut declared = crate::scripted::Declaration::for_test(
        name,
        std::path::PathBuf::from("/nonexistent/hosted-test.qml"),
        Scripted::Top,
        crate::scripted::On::EveryMonitor,
    );
    declared.reserve = reserve;
    desk.state.declare_surface(declared);
    let id = desk.state.surfaces.named(name).expect("declared");
    desk.state
        .surfaces
        .get_mut(id)
        .expect("live")
        .stand_in(stand);
    id
}

fn the_monitor(desk: &Desk) -> Output {
    desk.state
        .space
        .outputs()
        .next()
        .cloned()
        .expect("a monitor")
}

fn bottom(edge: i32) -> crate::scripted::Edges {
    crate::scripted::Edges {
        bottom: edge,
        ..Default::default()
    }
}

/// The stand-in for surface `id`, to change what it reports.
fn stand_of(desk: &mut Desk, id: crate::scripted::SurfaceId) -> &mut Stand {
    desk.state
        .surfaces
        .get_mut(id)
        .and_then(crate::scripted::Surface::stand_mut)
        .expect("a stand-in")
}

/// **#162: a declared reserve takes its edge out of the work area**,
/// and Lua's `sol.monitors()` and the scene's `Solium.monitor.area`
/// see the same work area. Ruling 10.
#[test]
fn a_declared_reserve_takes_its_edge_out_of_the_work_area() {
    let mut desk = Desk::new();
    reserving(&mut desk, "bar", bottom(48), Stand::solid());
    let output = the_monitor(&desk);
    let area = desk.state.work_area_on(&output).expect("a work area");
    assert_eq!((area.loc.y, area.size.h), (0, 1032));
    let lua = desk
        .state
        .snapshot()
        .monitors
        .first()
        .map(|monitor| monitor.area.h);
    assert_eq!(
        lua,
        Some(1032.0),
        "sol.monitors() disagrees with the compositor"
    );
    let row = crate::models::monitors::rows(&desk.state)
        .first()
        .and_then(|row| row.values.get("area").cloned());
    assert_eq!(
        row,
        Some(crate::models::monitors::rect(area)),
        "Solium.monitor.area disagrees with the compositor"
    );
}

/// **Two surfaces reserving one edge take both** (Ruling 10): a
/// bar and a dock under it, on the bottom edge.
#[test]
fn two_surfaces_reserving_one_edge_take_both() {
    let mut desk = Desk::new();
    reserving(&mut desk, "bar", bottom(48), Stand::solid());
    reserving(&mut desk, "dock", bottom(20), Stand::solid());
    let output = the_monitor(&desk);
    let area = desk.state.work_area_on(&output).expect("a work area");
    assert_eq!((area.loc.y, area.size.h), (0, 1012));
}

/// **A reserve a scene changes on its own is read at the frame's
/// settle** (Ruling 11): a timer in the scene hides the bar with
/// nothing delivered to it, and the frame after re-flows the
/// layout once.
#[test]
fn a_reserve_a_scene_changes_on_its_own_is_read_after_the_frame() {
    let mut desk = Desk::new();
    desk.install(
        r#"layouts = 0
sol.on("layout", function() layouts = layouts + 1; sol.status(tostring(layouts)) end)"#,
    );
    let bar = reserving(&mut desk, "bar", bottom(48), Stand::solid());
    let before: u32 = desk.state.status.parse().unwrap_or(0);
    stand_of(&mut desk, bar).reserve = Some(crate::scripted::SceneReserve {
        bottom: Some(0),
        ..Default::default()
    });
    let now = desk.state.clock.now();
    desk.state.settle(now);
    let output = the_monitor(&desk);
    assert_eq!(
        (
            desk.state.work_area_on(&output).map(|area| area.size.h),
            desk.state.status.parse::<u32>().unwrap_or(0) - before,
        ),
        (Some(1080), 1),
        "(the work area's height, the layout passes) after the frame"
    );
}

/// **A reserving surface is drawn across its whole monitor**: its
/// own reserve does not shrink it.
#[test]
fn a_reserving_surface_is_drawn_across_the_whole_monitor() {
    let mut desk = Desk::new();
    let bar = reserving(&mut desk, "bar", bottom(48), Stand::solid());
    let screen = Rectangle::new((0, 0).into(), (1920, 1080).into());
    let drawn: Vec<Rectangle<i32, Logical>> =
        crate::render::stacked(&desk.state, screen, desk.state.clock.now())
            .into_iter()
            .filter_map(|each| match each {
                crate::render::Stacked::Surface(id, area, _) if id == bar => Some(area),
                _ => None,
            })
            .collect();
    assert_eq!(drawn, vec![screen]);
}

/// **A reserve the scene sets wins for that edge, and runs `layout`
/// once** (Ruling 10, Ruling 11).
#[test]
fn a_scene_reserve_overrides_its_edge_and_reflows_the_layout_once() {
    let mut desk = Desk::new();
    desk.install(
        r#"layouts = 0
sol.on("layout", function() layouts = layouts + 1; sol.status(tostring(layouts)) end)"#,
    );
    let bar = reserving(
        &mut desk,
        "bar",
        crate::scripted::Edges {
            bottom: 48,
            top: 10,
            ..Default::default()
        },
        Stand::solid(),
    );
    let before: u32 = desk.state.status.parse().unwrap_or(0);
    stand_of(&mut desk, bar).reserve = Some(crate::scripted::SceneReserve {
        bottom: Some(0),
        ..Default::default()
    });
    desk.state.settle_scenes();
    let output = the_monitor(&desk);
    let area = desk.state.work_area_on(&output).expect("a work area");
    assert_eq!(
        (area.loc.y, area.size.h),
        (10, 1070),
        "the scene's bottom must win and the declared top stay"
    );
    let after: u32 = desk.state.status.parse().unwrap_or(0);
    assert_eq!(after, before + 1, "layout must run exactly once");
    desk.state.settle_scenes();
    assert_eq!(
        desk.state.status.parse::<u32>().unwrap_or(0),
        after,
        "an unchanged reserve ran layout again"
    );
}

/// Counts every `layout` pass, ahead of the tiling layout.
const LAYOUTS: &str = r#"layouts = 0
sol.on("layout", function() layouts = layouts + 1 end)"#;

/// How many `layout` passes [`LAYOUTS`] has counted.
fn layouts(desk: &Desk) -> u32 {
    desk.state
        .scripts
        .as_ref()
        .map(|scripts| scripts.evaluate("return tostring(layouts)"))
        .and_then(|said| said.parse().ok())
        .unwrap_or(0)
}

/// A bar across the bottom 48 of the monitor, and nothing else.
fn bottom_bar(at: Point<f64, Logical>) -> crate::qml::hosted::Hit {
    if at.y >= 1032.0 {
        crate::qml::hosted::Hit::Press
    } else {
        crate::qml::hosted::Hit::Nothing
    }
}

/// That bar, and quick settings grown out of it over the windows:
/// 600..1300 x 532..1032.
fn bottom_bar_and_its_panel(at: Point<f64, Logical>) -> crate::qml::hosted::Hit {
    let panel = (600.0..1300.0).contains(&at.x) && (532.0..1032.0).contains(&at.y);
    if panel {
        crate::qml::hosted::Hit::Press
    } else {
        bottom_bar(at)
    }
}

/// Two windows tiled under a canvas on every monitor whose bar
/// reserves the bottom 48, every glide landed, with Russian the
/// active group and `before` run ahead of the tiling layout.
fn tiled_under_a_bar(
    before: &str,
    stand: Stand,
) -> (Desk, [crate::pane::PaneId; 2], crate::scripted::SurfaceId) {
    let mut desk = russian_desk();
    let (_, _, first) = desk.open();
    let (_, _, second) = desk.open();
    desk.arrange("tiling", before);
    let bar = reserving(&mut desk, "bar", bottom(48), stand);
    desk.state.clock.advance(Duration::from_secs(2));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    (desk, [first, second], bar)
}

/// **A reserve the scene changes at a press re-flows the tiled
/// windows once, in the dispatch that delivered the press, from
/// that instant on the compositor's clock** (Ruling 11, Q4): the
/// bar hides, and the windows glide down into the room it gave
/// back with the layout's own motion, starting where they were.
/// Russian is the active group (#132).
#[test]
fn a_reserve_the_scene_changes_at_a_press_reflows_the_tiled_windows_once_from_that_instant() {
    let (mut desk, panes, bar) = tiled_under_a_bar(LAYOUTS, Stand::solid());
    let placed = panes.map(|pane| desk.placed(pane));
    assert!(
        placed.iter().all(|rect| rect.loc.y + rect.size.h <= 1032),
        "the premise: the windows are tiled above the bar: {placed:?}"
    );
    let passes = layouts(&desk);
    let at_rest = desk.state.clock.now();
    let shown = panes.map(|pane| drawn_now(&desk.state, pane, at_rest).rect.size.h);
    move_pointer(&mut desk.state, (960.0, 1050.0), 10);
    stand_of(&mut desk, bar).reserve = Some(crate::scripted::SceneReserve {
        bottom: Some(0),
        ..Default::default()
    });
    let region = region(&desk);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, 11);
    let now = desk.state.clock.now();
    let started = panes.map(|pane| drawn_now(&desk.state, pane, now).rect.size.h);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Released, 12);
    let reflowed = panes.map(|pane| desk.placed(pane));
    let midway = panes.map(|pane| {
        drawn_now(&desk.state, pane, now + Duration::from_millis(60))
            .rect
            .size
            .h
    });
    assert_eq!(
        layouts(&desk) - passes,
        1,
        "the reserve the press changed must run the layout exactly once"
    );
    assert_eq!(
        reflowed.map(|rect| rect.size.h - 48),
        placed.map(|rect| rect.size.h),
        "the windows did not take the room the bar gave back"
    );
    assert!(
        started
            .iter()
            .zip(&shown)
            .all(|(drawn, was)| (drawn - was).abs() < 1.0),
        "the glide must start where the windows were drawn, at the dispatch's \
         instant: drawn {started:?}, before the press {shown:?}"
    );
    assert!(
        midway
            .iter()
            .zip(shown.iter().zip(&reflowed))
            .all(|(drawn, (was, now))| {
                *drawn > was + 1.0 && *drawn < f64::from(now.size.h) - 1.0
            }),
        "60 ms on, the windows must be on their way, on the clock the press was \
         delivered on: drawn {midway:?}, from {shown:?} to {reflowed:?}"
    );
}

/// **A panel growing out of the bar moves no window** (Illia's
/// requirement, primitive 3): a press on the bar opens quick
/// settings over the windows, inside the same canvas, and the
/// work area, the layout and every window stay as they were,
/// while the panel takes the presses over the window under it.
/// Russian is the active group (#132).
#[test]
fn a_panel_growing_out_of_the_bar_moves_no_window() {
    let (mut desk, panes, bar) = tiled_under_a_bar(
        LAYOUTS,
        Stand {
            hit: bottom_bar,
            ..Stand::solid()
        },
    );
    let passes = layouts(&desk);
    let placed = panes.map(|pane| desk.placed(pane));
    let output = the_monitor(&desk);
    let area = desk.state.work_area_on(&output);
    let region = region(&desk);
    move_pointer(&mut desk.state, (960.0, 1050.0), 10);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, 11);
    stand_of(&mut desk, bar).hit = bottom_bar_and_its_panel;
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Released, 12);
    move_pointer(&mut desk.state, (800.0, 700.0), 13);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, 14);
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Released, 15);
    let pressed_on_the_panel = desk
        .state
        .surfaces
        .get(bar)
        .and_then(crate::scripted::Surface::stand)
        .is_some_and(|stand| {
            stand.seen.iter().any(|(at, event)| {
                let off = *at - Point::from((800.0, 700.0));
                matches!(event.kind, PointerKind::Press(0x1))
                    && off.x.abs() < 0.5
                    && off.y.abs() < 0.5
            })
        });
    assert_eq!(
        (
            layouts(&desk) - passes,
            panes.map(|pane| desk.placed(pane)),
            desk.state.work_area_on(&output),
            pressed_on_the_panel,
        ),
        (0, placed, area, true),
        "(layout passes, the windows, the work area, whether the panel took the \
         press over the window) once quick settings opened"
    );
}

/// **A reserve declared again, or taken away with its surface,
/// re-flows the windows once each**, in the dispatch of the
/// binding that said it: a key with Russian the active group
/// (#132) doubles the bar, and another takes it away.
#[test]
fn a_reserve_declared_again_or_taken_away_reflows_the_windows_once_each() {
    const KEY_B: u32 = 56;
    const KEY_N: u32 = 57;
    let (mut desk, panes, _) = tiled_under_a_bar(
        &format!(
            r#"{LAYOUTS}
sol.bind("super+b", function()
sol.surface("bar", {{ scene = "/nonexistent/hosted-test.qml", layer = "top", interactive = true, reserve = {{ bottom = 96 }} }})
end)
sol.bind("super+n", function() sol.surface("bar", false) end)"#
        ),
        Stand::solid(),
    );
    let heights = |desk: &Desk| panes.map(|pane| desk.placed(pane).size.h);
    let chord = |desk: &mut Desk, key: u32, time: u32| {
        for (code, state, at) in [
            (SUPER_L, KeyState::Pressed, time),
            (key, KeyState::Pressed, time + 1),
            (key, KeyState::Released, time + 2),
            (SUPER_L, KeyState::Released, time + 3),
        ] {
            crate::input::key(&mut desk.state, code.into(), state, at);
        }
    };
    let (passes, tiled) = (layouts(&desk), heights(&desk));
    chord(&mut desk, KEY_B, 10);
    let (doubled, after_doubling) = (heights(&desk), layouts(&desk));
    chord(&mut desk, KEY_N, 20);
    let (gone, after_going) = (heights(&desk), layouts(&desk));
    assert_eq!(
        (
            after_doubling - passes,
            doubled.map(|h| h + 48),
            after_going - after_doubling,
            gone.map(|h| h - 96),
        ),
        (1, tiled, 1, doubled),
        "(layout passes, heights + 48) once the bar doubled, and \
         (layout passes, heights - 96) once it went"
    );
}

/// **A property a `layout` handler writes, which moves the
/// scene's reserve, re-flows the windows against the new one**:
/// the scenes are read once the handler's whole dispatch is
/// applied, not part way through it, where the places the same
/// pass asked for afterwards would put the windows back where
/// the old reserve left room.
#[test]
fn a_property_a_layout_handler_writes_reflows_the_windows_against_the_reserve_it_moved() {
    let mut desk = Desk::new();
    let (_, _, pane) = desk.open();
    desk.arrange(
        "tiling",
        r#"passes = 0
sol.on("layout", function()
passes = passes + 1
sol.surface("bar", { scene = "/nonexistent/hosted-test.qml", layer = "top", interactive = true, properties = { pass = passes } })
end)"#,
    );
    desk.state.trigger_relayout();
    let whole = desk.placed(pane);
    let bar = desk
        .state
        .surfaces
        .named("bar")
        .expect("the handler declared it");
    desk.state
        .surfaces
        .get_mut(bar)
        .expect("live")
        .stand_in(Stand {
            reserve: Some(crate::scripted::SceneReserve {
                bottom: Some(48),
                ..Default::default()
            }),
            ..Stand::solid()
        });
    desk.state.trigger_relayout();
    assert_eq!(
        desk.placed(pane).size.h,
        whole.size.h - 48,
        "the window was placed against the reserve the scene had before"
    );
}

/// Queue `actions`, each with its data as JSON, on the stand-in
/// for surface `id`.
fn queue(desk: &mut Desk, id: crate::scripted::SurfaceId, actions: &[(&str, &str)]) {
    if let Some(stand) = desk
        .state
        .surfaces
        .get_mut(id)
        .and_then(crate::scripted::Surface::stand_mut)
    {
        for (action, data) in actions {
            stand.actions.push((
                (*action).to_owned(),
                crate::json::Json::parse(data).expect("valid JSON"),
            ));
        }
    }
}

/// **Two actions from one frame both reach Lua, in order**, at
/// the next settle (Ruling 15).
#[test]
fn two_actions_from_one_frame_both_reach_lua_in_order() {
    let (mut desk, _, shell) = window_under_a_scene(button_over_the_window);
    desk.install(
        r#"heard = ""
sol.on("surface", function(surface, action) heard = heard .. action .. ";"; sol.status(heard) end)"#,
    );
    queue(&mut desk, shell, &[("first", "null"), ("second", "null")]);
    desk.state.settle_scenes();
    assert_eq!(desk.state.status, "first;second;");
}

/// **`windows.focus` from a scene focuses the window**, through
/// the shipped `actions.lua` and `sol.act`.
#[test]
fn windows_focus_from_a_scene_focuses_the_window() {
    let (mut desk, opened, shell) = window_under_a_scene(button_over_the_window);
    desk.install(r#"require("actions")"#);
    desk.state.give_keyboard(None, SERIAL_COUNTER.next_serial());
    let id = desk
        .state
        .snapshot()
        .windows
        .first()
        .map(|window| window.id)
        .expect("a window");
    queue(
        &mut desk,
        shell,
        &[("windows.focus", &format!(r#"{{"id":{id}}}"#))],
    );
    desk.state.settle_scenes();
    assert_eq!(keyboard_on(&desk), Some(window_id(&opened)));
}

/// **An action naming a window that is not there is answered
/// `unknown-window`**, one the compositor does not know
/// `unknown-action`, and one with no window's id `bad-data`, each
/// once its batch is applied, in order.
#[test]
fn sol_act_answers_why_it_could_not() {
    let (mut desk, _, shell) = window_under_a_scene(button_over_the_window);
    desk.install(
        r#"answers = ""
sol.on("surface", function(surface, action, data)
sol.act(action, data, function(ok, reason) answers = answers .. tostring(reason) .. ";"; sol.status(answers) end)
end)"#,
    );
    queue(
        &mut desk,
        shell,
        &[
            ("windows.close", r#"{"id":4242}"#),
            ("windows.fly", r#"{"id":1}"#),
            ("windows.maximize", r#"{"id":"1"}"#),
        ],
    );
    desk.state.settle_scenes();
    assert_eq!(desk.state.status, "unknown-window;unknown-action;bad-data;");
}

/// **`done` hears that an action was done, after it was**: a
/// window closed from a scene starts closing, `done(true)` is
/// told once after that, and the client is asked once the window
/// has faded.
#[test]
fn sol_act_tells_done_once_the_window_was_asked_to_close() {
    let (mut desk, opened, shell) = window_under_a_scene(button_over_the_window);
    desk.install(
        r#"answers = ""
sol.on("closing", function(id) answers = answers .. "closing;"; sol.status(answers) end)
sol.on("surface", function(surface, action, data)
sol.act(action, data, function(ok, reason) answers = answers .. tostring(ok) .. ";"; sol.status(answers) end)
end)"#,
    );
    let id = desk
        .state
        .snapshot()
        .windows
        .first()
        .map(|window| window.id)
        .expect("a window");
    queue(
        &mut desk,
        shell,
        &[("windows.close", &format!(r#"{{"id":{id}}}"#))],
    );
    desk.state.settle_scenes();
    let heard = desk.state.status.clone();
    let faded = desk.state.clock.now() + crate::present::CLOSING + Duration::from_millis(10);
    desk.state.settle_closing(faded);
    desk.pump();
    assert_eq!(
        (
            heard.as_str(),
            desk.client
                .closes
                .contains(&wayland_client::Proxy::id(&opened.toplevel))
        ),
        ("closing;true;", true),
        "(what the listeners heard, in order, whether the client was asked to close)"
    );
}

/// The first window's id, as `sol.windows()` gives it.
fn first_window(desk: &Desk) -> u64 {
    desk.state
        .snapshot()
        .windows
        .first()
        .map(|window| window.id)
        .expect("a window")
}

/// **`done` is told once the whole batch that ran its `sol.act`
/// is applied** (03 §3.3.2), not part way through it, where a
/// later command of the batch runs listeners of its own: both
/// focus events come before it.
#[test]
fn done_is_told_after_every_command_of_the_batch_that_ran_its_act() {
    let (mut desk, _, shell) = window_under_a_scene(button_over_the_window);
    let id = first_window(&desk);
    desk.install(&format!(
        r#"heard = ""
sol.on("focus", function() heard = heard .. "focus;"; sol.status(heard) end)
sol.on("surface", function()
sol.act("windows.fly", nil, function() heard = heard .. "done;"; sol.status(heard) end)
sol.focus({id})
sol.focus({id})
end)"#
    ));
    queue(&mut desk, shell, &[("go", "null")]);
    desk.state.settle_scenes();
    assert_eq!(desk.state.status, "focus;focus;done;");
}

/// **A `sol.act` a hotplug's `monitors` handler runs hears its
/// `done` in the hotplug's dispatch**, though the hotplug holds
/// its handlers as one dispatch.
#[test]
fn a_sol_act_in_a_hotplugs_handler_hears_done_in_the_hotplugs_dispatch() {
    let (mut desk, _, _) = window_under_a_scene(button_over_the_window);
    desk.install(
        r#"sol.on("monitors", function()
sol.act("windows.fly", nil, function(ok, reason) sol.status("done " .. tostring(reason)) end)
end)"#,
    );
    desk.state.settle_monitors();
    assert_eq!(desk.state.status, "done unknown-action");
}

/// **A `sol.act` at the top of a reloaded configuration hears
/// its `done` in the reload's dispatch**, though the reload holds
/// its handlers as one dispatch.
#[test]
fn a_sol_act_in_a_reloaded_configuration_hears_done_in_the_reloads_dispatch() {
    let (mut desk, _, _) = window_under_a_scene(button_over_the_window);
    let directory = std::env::temp_dir().join(format!("solium-reload-act-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    let entry = directory.join("init.lua");
    std::fs::write(
        &entry,
        r#"sol.act("windows.fly", nil, function(ok, reason) sol.status("done " .. tostring(reason)) end)"#,
    )
    .expect("writing the entry point");
    desk.state.reload_from(&entry);
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(desk.state.status, "done unknown-action");
}

/// **A `done` that acts again each time it is told costs rounds,
/// not the session**: a retry that never succeeds is told
/// `ATTEMPT_ROUNDS` times in the dispatch that started it, the
/// attempt left over is told at the next dispatch, and nothing
/// runs the stack out.
#[test]
fn a_done_that_acts_again_each_time_it_is_told_costs_rounds_not_the_session() {
    let (mut desk, _, shell) = window_under_a_scene(button_over_the_window);
    desk.install(
        r#"tries = 0
local function again()
tries = tries + 1
sol.status(tostring(tries))
sol.act("windows.focus", { id = 4242 }, again)
end
sol.on("surface", function(surface, action) if action == "go" then again() end end)"#,
    );
    let rounds = crate::state::commands::ATTEMPT_ROUNDS;
    queue(&mut desk, shell, &[("go", "null")]);
    desk.state.settle_scenes();
    let first = (desk.state.status.clone(), desk.state.settled_attempts.len());
    queue(&mut desk, shell, &[("other", "null")]);
    desk.state.settle_scenes();
    assert_eq!(
        (first, desk.state.status.clone()),
        (((1 + rounds).to_string(), 1), (1 + 2 * rounds).to_string()),
        "((the tries in the first dispatch, the attempts left), the tries after the next)"
    );
}

/// **A `done` that acts again and then never returns is stopped
/// once, not every round**: what it asked for before the stop is
/// dropped, so no round follows it and no attempt is left over
/// for the next dispatch to wait on.
#[test]
fn a_done_that_acts_again_and_never_returns_does_not_stall_every_dispatch() {
    let (mut desk, _, shell) = window_under_a_scene(button_over_the_window);
    desk.install(
        r#"tries = 0
local function again()
tries = tries + 1
sol.status(tostring(tries))
sol.act("windows.focus", { id = 4242 }, again)
while true do end
end
sol.on("surface", function(surface, action)
if action == "go" then sol.act("windows.focus", { id = 4242 }, again) end
end)"#,
    );
    queue(&mut desk, shell, &[("go", "null")]);
    desk.state.settle_scenes();
    queue(&mut desk, shell, &[("other", "null")]);
    desk.state.settle_scenes();
    assert_eq!(
        (
            desk.state.status.as_str(),
            desk.state.settled_attempts.len()
        ),
        ("1", 0),
        "(how many times the done was told, the attempts left)"
    );
}

/// **A window still loading is not a window to focus, send
/// fullscreen or maximise**: those three answer `unknown-window`
/// for a pane with no client yet, though `sol.windows()` lists it,
/// and `windows.close` closes it.
#[test]
fn sol_act_on_a_window_still_loading_answers_unknown_window_but_closes_it() {
    let (mut desk, _, shell) = window_under_a_scene(button_over_the_window);
    desk.install(
        r#"answers = ""
sol.on("surface", function(surface, action, data)
sol.act(action, data, function(ok, reason) answers = answers .. tostring(reason) .. ";"; sol.status(answers) end)
end)"#,
    );
    let source = crate::pane::loading_source(None);
    let loading = desk.state.open_loading("app", None, source, None);
    let data = format!(r#"{{"id":{}}}"#, loading.get());
    queue(
        &mut desk,
        shell,
        &[
            ("windows.focus", &data),
            ("windows.fullscreen", &data),
            ("windows.maximize", &data),
            ("windows.close", &data),
        ],
    );
    desk.state.settle_scenes();
    assert_eq!(
        (
            desk.state.status.as_str(),
            desk.state
                .panes
                .get(loading)
                .is_some_and(crate::pane::Pane::leaving)
        ),
        ("unknown-window;unknown-window;unknown-window;nil;", true),
        "(what the dones heard, whether the loading window is closing)"
    );
}

/// **An action the compositor does not know is warned of the
/// first time only**: asked for twice, the log says so once, and
/// both `done`s still hear `unknown-action`.
#[test]
fn an_unknown_action_is_warned_of_the_first_time_only() {
    let (mut desk, _, shell) = window_under_a_scene(button_over_the_window);
    desk.install(
        r#"answers = ""
sol.on("surface", function(surface, action, data)
sol.act(action, data, function(ok, reason) answers = answers .. tostring(reason) .. ";"; sol.status(answers) end)
end)"#,
    );
    queue(
        &mut desk,
        shell,
        &[("windows.fly", "null"), ("windows.fly", "null")],
    );
    let log = crate::script::logged_while(|| desk.state.settle_scenes());
    assert_eq!(
        (
            desk.state.status.as_str(),
            log.matches("sol.act: no such action").count()
        ),
        ("unknown-action;unknown-action;", 1),
        "(what the dones heard, how many times the log warned of it):\n{log}"
    );
}

fn window_row(desk: &Desk, id: u64) -> std::collections::BTreeMap<&'static str, crate::json::Json> {
    crate::models::windows::rows(&desk.state)
        .into_iter()
        .find(|row| row.key == id.to_string())
        .map(|row| row.values)
        .expect("a row for the window")
}

/// **A window's row is where it lives, its focus and its state**,
/// built from its pane.
#[test]
fn a_window_row_carries_where_it_lives_and_its_focus() {
    let (mut desk, opened, _) = window_under_a_scene(button_over_the_window);
    let window = window(&desk, &opened);
    desk.state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    let id = opened.pane.get();
    let row = window_row(&desk, id);
    use crate::json::Json;
    assert_eq!(row.get("state"), Some(&Json::Text("shown".to_owned())));
    assert_eq!(row.get("focused"), Some(&Json::Bool(true)));
    assert_eq!(row.get("focusOrder"), Some(&Json::Number(0.0)));
    assert_eq!(row.get("onStage"), Some(&Json::Bool(true)));
    assert_eq!(row.get("xwayland"), Some(&Json::Bool(false)));
    assert!(matches!(row.get("monitor"), Some(Json::Text(name)) if !name.is_empty()));
    assert_eq!(
        row.get("workspace"),
        Some(&Json::Text(String::new())),
        "no workspaces are declared, so the window is on none"
    );
}

/// **Workspace rows count their windows and say which is shown**,
/// from what Lua declared, joined with the windows; and the
/// window's own row says which workspace it is on.
#[test]
fn workspace_rows_count_their_windows_and_say_which_is_shown() {
    let (mut desk, opened, _) = window_under_a_scene(button_over_the_window);
    let monitor = the_monitor(&desk).name();
    let id = opened.pane.get();
    desk.install(&format!(
        r#"sol.workspaces({{ arrangement = {{ kind = "horizontal", columns = 2, rows = 1 }},
            groups = {{ {{ id = "{monitor}", monitors = {{ "{monitor}" }}, showing = {{ "1" }},
                         workspaces = {{ {{ id = "1", name = "1", col = 1, row = 1 }}, {{ id = "2", name = "2", col = 2, row = 1 }} }} }} }},
            windows = {{ [{id}] = {{ "2" }} }} }})"#
    ));
    let outcome = desk
        .state
        .scripts
        .as_mut()
        .map(crate::script::Scripts::startup)
        .unwrap_or_default();
    desk.state.apply(outcome);
    let rows = crate::models::workspaces::rows(&desk.state);
    let get = |key: &str, role: &str| {
        rows.iter()
            .find(|row| row.key == key)
            .and_then(|row| row.values.get(role).cloned())
    };
    use crate::json::Json;
    assert_eq!(
        get(&format!("{monitor}/1"), "active"),
        Some(Json::Bool(true))
    );
    assert_eq!(
        (
            get(&format!("{monitor}/1"), "focused"),
            get(&format!("{monitor}/2"), "focused")
        ),
        (Some(Json::Bool(true)), Some(Json::Bool(false))),
        "the workspace the monitor in front shows is the focused one"
    );
    assert_eq!(
        get(&format!("{monitor}/2"), "occupied"),
        Some(Json::Number(1.0))
    );
    assert_eq!(
        get(&format!("{monitor}/1"), "occupied"),
        Some(Json::Number(0.0))
    );
    assert_eq!(
        window_row(&desk, id).get("workspace"),
        Some(&Json::Text("2".to_owned()))
    );

    // Fullscreen and urgent, the window makes its workspace so,
    // and is in its `windows`; the other is neither.
    desk.state.apply(crate::script::Outcome {
        commands: vec![crate::script::Command::ToggleFullscreen { id }],
        ..crate::script::Outcome::default()
    });
    desk.state.urgent.insert(id);
    let rows = crate::models::workspaces::rows(&desk.state);
    let roles = |key: String| {
        let row = rows.iter().find(|row| row.key == key);
        ["urgent", "hasFullscreen", "windows"]
            .map(|role| row.and_then(|row| row.values.get(role).cloned()))
    };
    #[expect(clippy::cast_precision_loss, reason = "a window id, far below 2^53")]
    let number = id as f64;
    assert_eq!(
        (roles(format!("{monitor}/2")), roles(format!("{monitor}/1"))),
        (
            [
                Some(Json::Bool(true)),
                Some(Json::Bool(true)),
                Some(Json::List(vec![Json::Number(number)]))
            ],
            [
                Some(Json::Bool(false)),
                Some(Json::Bool(false)),
                Some(Json::List(Vec::new()))
            ]
        ),
        "(urgent, hasFullscreen, windows) of workspace 2, with the window, and 1"
    );
}

/// **A reload into a configuration that stops calling
/// `sol.workspaces` publishes none**, not what the previous
/// session last declared.
#[test]
fn a_reload_that_stops_declaring_workspaces_publishes_none() {
    let (mut desk, _opened, _) = window_under_a_scene(button_over_the_window);
    let monitor = the_monitor(&desk).name();
    desk.install(&format!(
        r#"sol.workspaces({{ arrangement = {{ kind = "horizontal", columns = 1, rows = 1 }},
            groups = {{ {{ id = "{monitor}", monitors = {{ "{monitor}" }}, showing = {{ "1" }},
                         workspaces = {{ {{ id = "1", name = "1", col = 1, row = 1 }} }} }} }} }})"#
    ));
    let outcome = desk
        .state
        .scripts
        .as_mut()
        .map(crate::script::Scripts::startup)
        .unwrap_or_default();
    desk.state.apply(outcome);
    assert!(
        desk.state.workspaces.is_some(),
        "the premise: a declaration is held"
    );
    assert!(
        !crate::models::workspaces::rows(&desk.state).is_empty(),
        "the premise: it publishes rows"
    );

    let directory = std::env::temp_dir().join(format!(
        "solium-reload-drops-workspaces-{}",
        std::process::id()
    ));
    let _ = std::fs::create_dir_all(&directory);
    let entry = directory.join("init.lua");
    std::fs::write(
        &entry,
        "-- a configuration that never calls sol.workspaces\n",
    )
    .expect("writing the entry point");
    desk.state.reload_from(&entry);
    let _ = std::fs::remove_dir_all(&directory);

    assert!(
        desk.state.workspaces.is_none(),
        "the old declaration stayed held after a reload that never declared one"
    );
    assert!(
        crate::models::workspaces::rows(&desk.state).is_empty(),
        "the old rows stayed published after a reload that never declared any"
    );
}

/// **`focusOrder` is most recent first** (Ruling 18).
#[test]
fn focus_order_is_most_recent_first() {
    let (mut desk, first, _) = window_under_a_scene(button_over_the_window);
    let second = desk.open_surface();
    let (one, two) = (window(&desk, &first), window(&desk, &second));
    desk.state.focus_window(
        &one,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    desk.state.focus_window(
        &two,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    use crate::json::Json;
    assert_eq!(
        window_row(&desk, second.pane.get()).get("focusOrder"),
        Some(&Json::Number(0.0))
    );
    assert_eq!(
        window_row(&desk, first.pane.get()).get("focusOrder"),
        Some(&Json::Number(1.0))
    );
}

/// **A window is listed from the moment it is launched**, before
/// its application draws: `loading`, titled with the program,
/// with no process to name yet, and, never focused, last in
/// `focusOrder` (Ruling 18).
#[test]
fn a_window_still_loading_is_listed_as_loading() {
    let (mut desk, opened, _) = window_under_a_scene(button_over_the_window);
    let focused = window(&desk, &opened);
    desk.state.focus_window(
        &focused,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    let source = crate::pane::loading_source(None);
    let loading = desk.state.open_loading("app", None, source, None);
    let row = window_row(&desk, loading.get());
    use crate::json::Json;
    assert_eq!(
        (
            row.get("state"),
            row.get("title"),
            row.get("pid"),
            row.get("focusOrder")
        ),
        (
            Some(&Json::Text("loading".to_owned())),
            Some(&Json::Text("app".to_owned())),
            Some(&Json::Number(-1.0)),
            Some(&Json::Number(1.0))
        ),
        "(state, title, pid, focusOrder)"
    );
}

/// **A window that closes leaves no gap in `focusOrder`**
/// (Ruling 18): the windows left still count up from 0, so the
/// one focused before the one in front is still 1; and the next
/// focus forgets it, urgent as it was.
#[test]
fn a_closed_window_leaves_no_gap_in_focus_order() {
    let (mut desk, first, _) = window_under_a_scene(button_over_the_window);
    let second = desk.open_surface();
    let third = desk.open_surface();
    let (one, two, three) = (
        window(&desk, &first),
        window(&desk, &second),
        window(&desk, &third),
    );
    desk.state.focus_window(
        &three,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    desk.state.focus_window(
        &two,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    desk.state.focus_window(
        &one,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    desk.state.urgent.insert(second.pane.get());
    second.toplevel.destroy();
    desk.pump();
    use crate::json::Json;
    assert!(
        crate::models::windows::rows(&desk.state)
            .iter()
            .all(|row| row.key != second.pane.get().to_string()),
        "the premise: the closed window is not listed"
    );
    assert_eq!(
        [&first, &third].map(|opened| window_row(&desk, opened.pane.get())
            .get("focusOrder")
            .cloned()),
        [Some(Json::Number(0.0)), Some(Json::Number(1.0))],
        "[the window in front, the one focused before the closed one]"
    );
    desk.state.focus_window(
        &three,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    assert_eq!(
        (desk.state.focus_history.clone(), desk.state.urgent.len()),
        (vec![third.pane.get(), first.pane.get()], 0),
        "the next focus forgets the window that closed: (the focus history, how \
         many windows are urgent)"
    );
}

/// **A window reads `maximized` the moment the compositor
/// maximises it**, before its client has answered, and still
/// once it has: nothing waits for a client (Section 2, rule 3;
/// Ruling 18).
#[test]
fn a_maximised_window_reads_maximized_at_once() {
    let (mut desk, opened, _) = window_under_a_scene(button_over_the_window);
    let window = window(&desk, &opened);
    desk.state.toggle_maximize(&window);
    desk.pump();
    use crate::json::Json;
    let asked = window_row(&desk, opened.pane.get())
        .get("maximized")
        .cloned();
    let id = wayland_client::Proxy::id(&opened.xdg);
    let serial = desk
        .client
        .surface_configures
        .iter()
        .rev()
        .find(|(to, _)| *to == id)
        .map(|&(_, serial)| serial)
        .expect("the window was configured");
    opened.xdg.ack_configure(serial);
    commit_buffer(&desk.client, &desk.qh, &opened.surface, 64, 64);
    desk.pump();
    let agreed = window_row(&desk, opened.pane.get())
        .get("maximized")
        .cloned();
    assert_eq!(
        (asked, agreed),
        (Some(Json::Bool(true)), Some(Json::Bool(true))),
        "(maximized once the compositor maximised it, once the client committed it)"
    );
}
