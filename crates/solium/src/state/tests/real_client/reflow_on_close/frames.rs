use super::*;
use smithay::backend::input::ButtonState;

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

/// The titlebar is the top 32 pixels. A right press on it opens a
/// panel 60 tall below it, over the client, which holds a grab
/// while it is open; the wheel anywhere on the frame sends
/// `volume` with how far it turned, in notches.
const FRAME: &str = r#"
import QtQuick
import Solium

Item {
id: frame
property string action: ""
readonly property bool onButton: false
property int windowId: 0
property bool expanded: false
property int rights: 0
property int wheels: 0
property int panelPresses: 0
property int dismissals: 0
property int escapes: 0
property int slider: -1

MouseArea {
x: 0; y: 0; width: frame.width; height: 32
acceptedButtons: Qt.RightButton
onPressed: {
frame.rights += 1
frame.expanded = true
panel.forceActiveFocus()
}
}
WheelHandler {
target: null
onWheel: (event) => {
frame.wheels += 1
Solium.send("volume", { by: event.angleDelta.y / 120, id: frame.windowId })
}
}
Item {
id: panel
y: 32; width: frame.width; height: 60
visible: frame.expanded
Keys.onEscapePressed: {
frame.escapes += 1
frame.expanded = false
}
MouseArea {
anchors.fill: parent
onPressed: (mouse) => {
    frame.panelPresses += 1
    frame.slider = mouse.x
}
onPositionChanged: (mouse) => frame.slider = mouse.x
}
}
Grab {
active: frame.expanded
target: panel
onDismissed: {
frame.dismissals += 1
frame.expanded = false
}
}
}
"#;

/// A window framed by [`FRAME`] at (600, 500), with the keyboard,
/// a pointer bound, every layer built, and `events` recording what
/// `sol.on("frame")` hears. The style's folder, to remove.
fn framed() -> (Desk, Opened, std::path::PathBuf) {
    framed_by(FRAME)
}

/// A style folder whose one `frame` layer is `layer`, under
/// `insets.top: 32`.
fn style_of(layer: &str) -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let style = std::env::temp_dir().join(format!("solium-frames-{}-{serial}", std::process::id()));
    std::fs::create_dir_all(&style).expect("a style folder");
    std::fs::write(
        style.join("Pane.qml"),
        "import Solium\nPaneStyle {\n    insets.top: 32\n    \
         Layer { depth: \"frame\"; name: \"bar\"; source: \"Frame.qml\" }\n}\n",
    )
    .expect("writing the manifest");
    std::fs::write(style.join("Frame.qml"), layer).expect("writing the layer");
    style
}

/// [`framed`], with `layer` as the frame's one layer.
fn framed_by(layer: &str) -> (Desk, Opened, std::path::PathBuf) {
    let style = style_of(layer);
    let mut desk = Desk::new();
    let opened = desk.open_surface();
    let _buffer = commit_buffer(&desk.client, &desk.qh, &opened.surface, 400, 300);
    desk.pump();
    desk.install(
        "events = {}\n\
         sol.on(\"frame\", function(id, name, data)\n\
             events[#events + 1] = id .. \" \" .. name .. \" \" .. data.by .. \" \" .. data.id\n\
         end)",
    );
    desk.state
        .decorations
        .set_style(&mut desk.state.panes, Some(style.display().to_string()));
    desk.state
        .decorations
        .insert(&mut desk.state.panes, opened.pane, 400, 300);
    let window = desk
        .state
        .panes
        .get(opened.pane)
        .and_then(Pane::client)
        .cloned()
        .expect("a client");
    desk.state
        .space
        .map_element(window.clone(), (600, 500), false);
    desk.state.space.refresh();
    desk.state.sync_panes();
    desk.state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    desk.state.profile.focus_follows_mouse = false;
    let _pointer = desk.client.seat_pointer(&desk.qh);
    desk.state.clock.advance(Duration::from_secs(1));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    desk.pump();
    (desk, opened, style)
}

/// The point `local` of `pane`, on the screen.
fn at(desk: &Desk, pane: crate::pane::PaneId, local: (f64, f64)) -> (f64, f64) {
    let outer = desk.state.pane_outer_of(pane).expect("outer");
    (
        f64::from(outer.loc.x) + local.0,
        f64::from(outer.loc.y) + local.1,
    )
}

fn region(desk: &Desk) -> Rectangle<i32, Logical> {
    crate::monitor::union(&desk.state.space).expect("a monitor")
}

fn click(desk: &mut Desk, button: u32, time: u64) {
    let region = region(desk);
    crate::synth::send_button(&mut desk.state, region, button, ButtonState::Pressed, time);
    crate::synth::send_button(
        &mut desk.state,
        region,
        button,
        ButtonState::Released,
        time + 1,
    );
    desk.pump();
}

/// The frame's integer property `name`.
fn told(desk: &mut Desk, pane: crate::pane::PaneId, name: &str) -> i32 {
    desk.state
        .panes
        .get_mut(pane)
        .and_then(Pane::decoration_mut)
        .expect("the frame")
        .first_layer_int(name)
}

/// The panel opened with a right press on the titlebar.
fn open_panel(desk: &mut Desk, pane: crate::pane::PaneId) {
    let bar = at(desk, pane, (20.0, 16.0));
    move_pointer(&mut desk.state, bar, 10);
    click(desk, BTN_RIGHT, 11);
    assert_eq!(
        desk.state.decorations.holding,
        Some(pane),
        "the premise: the panel holds its grab"
    );
}

/// A layer that grows its window's titlebar by `grow`, at once,
/// and is told what the frame reserves as `insetTop`.
const GROWS: &str = r#"
import QtQuick

Item {
property int grow: 0
property real growTop: grow
property int insetTop: 0
property string action: ""
readonly property bool onButton: false
}
"#;

/// The same, eased over 160 ms, as a titlebar opening its quick
/// settings would.
const EASES: &str = r#"
import QtQuick

Item {
property int grow: 0
property real growTop: grow
Behavior on growTop { NumberAnimation { duration: 160 } }
property int insetTop: 0
property string action: ""
readonly property bool onButton: false
}
"#;

/// Ask every layer of `pane`'s frame to grow by `by`.
fn grow_to(desk: &mut Desk, pane: crate::pane::PaneId, by: i32) {
    desk.state
        .panes
        .get_mut(pane)
        .and_then(Pane::decoration_mut)
        .expect("the frame")
        .set_layers_int("grow", by);
}

/// One frame, `ms` after the last: the QML clock moves, the
/// frames' growth is followed, and the client hears what it was
/// told.
fn frame(desk: &mut Desk, ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
    desk.state.clock.advance(Duration::from_millis(ms));
    crate::render::frame_tick(&mut desk.state);
    let now = desk.state.clock.now();
    desk.state.settle(now);
    desk.pump();
}

/// How many configures this toplevel has been sent.
fn configures(desk: &Desk, toplevel: &xdg_toplevel::XdgToplevel) -> usize {
    let id = wayland_client::Proxy::id(toplevel);
    desk.client
        .configures
        .iter()
        .filter(|(to, _, _)| *to == id)
        .count()
}

/// The client's own rectangle, where the space has it.
fn client_rect(desk: &Desk, pane: crate::pane::PaneId) -> Rectangle<i32, Logical> {
    let window = desk
        .state
        .panes
        .get(pane)
        .and_then(Pane::client)
        .expect("a client");
    desk.state.real_geometry(window).expect("mapped")
}

/// **A floating window's frame grows and its client moves down at
/// its size** (#423): a layer's `growTop` of 40 makes the frame
/// reserve 72 at the top and tells every layer so as `insetTop`;
/// the window's top edge stays where it was, its pane is 40
/// taller, and the client is 40 lower and the size it was,
/// configured with nothing. The 40 under the titlebar is the
/// frame's, so a press there is not the client's. Back to 0, the
/// window is exactly where it began.
#[test]
fn a_floating_windows_frame_grows_and_its_client_moves_down_at_its_size() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let (mut desk, opened, style) = framed_by(GROWS);
        frame(&mut desk, 16);
        let outer = desk.state.pane_outer_of(opened.pane).expect("outer");
        let client = client_rect(&desk, opened.pane);
        let sent = configures(&desk, &opened.toplevel);
        let below_the_bar = Point::<f64, Logical>::from((
            f64::from(outer.loc.x) + 20.0,
            f64::from(outer.loc.y) + 52.0,
        ));
        assert!(
            desk.state
                .chrome_under(below_the_bar)
                .is_none_or(|under| !matches!(under.chrome, crate::state::Chrome::Frame)),
            "the premise: 52 down is the client's before the frame grows"
        );

        grow_to(&mut desk, opened.pane, 40);
        frame(&mut desk, 16);
        let grown = desk.state.pane_outer_of(opened.pane).expect("outer");
        assert_eq!(grown.loc, outer.loc, "the window's top edge stayed");
        assert_eq!(
            (grown.size.w, grown.size.h),
            (outer.size.w, outer.size.h + 40),
            "the pane is 40 taller"
        );
        let moved = client_rect(&desk, opened.pane);
        assert_eq!(
            (moved.loc.x, moved.loc.y),
            (client.loc.x, client.loc.y + 40),
            "the client moved down by 40"
        );
        assert_eq!(moved.size, client.size, "at its size");
        assert_eq!(desk.state.insets_of(opened.pane).top, 72);
        assert_eq!(told(&mut desk, opened.pane, "insetTop"), 72);
        assert!(
            desk.state
                .chrome_under(below_the_bar)
                .is_some_and(|under| under.pane == opened.pane
                    && matches!(under.chrome, crate::state::Chrome::Frame)),
            "52 down is the frame's band now"
        );
        frame(&mut desk, 16);
        frame(&mut desk, 16);
        assert_eq!(
            configures(&desk, &opened.toplevel),
            sent,
            "a floating client is not configured for its frame growing"
        );

        grow_to(&mut desk, opened.pane, 0);
        frame(&mut desk, 16);
        frame(&mut desk, 16);
        assert_eq!(
            desk.state.pane_outer_of(opened.pane),
            Some(outer),
            "back to 0, the window is where it began"
        );
        assert_eq!(client_rect(&desk, opened.pane), client);
        assert_eq!(told(&mut desk, opened.pane, "insetTop"), 32);
        let _ = std::fs::remove_dir_all(&style);
    });
}

/// A window framed by `layer` in a tiling layout's one tile, its
/// client having answered its tile. The style's folder, to remove.
fn tiled_by(layer: &str) -> (Desk, Opened, std::path::PathBuf) {
    let style = style_of(layer);
    let mut desk = Desk::new();
    let opened = desk.open_surface();
    desk.state
        .decorations
        .set_style(&mut desk.state.panes, Some(style.display().to_string()));
    desk.state
        .decorations
        .insert(&mut desk.state.panes, opened.pane, 400, 300);
    desk.arrange("tiling", "");
    desk.answer(&opened);
    frame(&mut desk, 16);
    frame(&mut desk, 16);
    (desk, opened, style)
}

/// **A held window's client is configured smaller once its frame
/// has grown** (#423): in a tile, a `growTop` eased from 0 to 40
/// is followed frame by frame -- the frame reserving every step
/// of the way, the client held in what is left of the tile and
/// told nothing while it moves -- and once it stands still the
/// client is configured once, 40 shorter. Eased back to 0, it is
/// configured once more, to the size it had.
#[test]
fn a_held_windows_client_is_configured_smaller_once_its_frame_has_grown() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let (mut desk, opened, style) = tiled_by(EASES);
        let outer = desk.state.pane_outer_of(opened.pane).expect("outer");
        let tile = desk.placed(opened.pane);
        assert_eq!(outer, tile, "the premise: the window fills its tile");
        let client = client_rect(&desk, opened.pane);
        let sent = configures(&desk, &opened.toplevel);

        grow_to(&mut desk, opened.pane, 40);
        let mut seen = Vec::new();
        for _ in 0..30 {
            frame(&mut desk, 16);
            let top = desk.state.insets_of(opened.pane).top;
            seen.push(top);
            assert_eq!(
                desk.state.pane_outer_of(opened.pane),
                Some(outer),
                "the pane stays its tile while the frame grows"
            );
            assert_eq!(
                configures(&desk, &opened.toplevel),
                sent,
                "the client is told nothing while the frame grows: {seen:?}"
            );
            if top == 72 {
                break;
            }
        }
        assert!(
            seen.iter().any(|top| *top > 32 && *top < 72),
            "the frame followed the ease through its steps: {seen:?}"
        );
        assert_eq!(seen.last(), Some(&72), "and landed on 40 more: {seen:?}");
        frame(&mut desk, 16);
        assert_eq!(
            configures(&desk, &opened.toplevel),
            sent + 1,
            "configured once it stood still"
        );
        assert_eq!(
            last_configured(&desk.client, &opened.toplevel),
            Some((client.size.w, client.size.h - 40)),
            "40 shorter"
        );
        let held = client_rect(&desk, opened.pane);
        assert_eq!(
            (held.loc.x, held.loc.y),
            (client.loc.x, client.loc.y + 40),
            "under the taller frame"
        );
        commit_buffer(
            &desk.client,
            &desk.qh,
            &opened.surface,
            client.size.w,
            client.size.h - 40,
        );
        frame(&mut desk, 16);
        frame(&mut desk, 16);
        assert_eq!(
            configures(&desk, &opened.toplevel),
            sent + 1,
            "and only once"
        );
        assert_eq!(desk.state.pane_outer_of(opened.pane), Some(outer));

        grow_to(&mut desk, opened.pane, 0);
        for _ in 0..30 {
            frame(&mut desk, 16);
            if desk.state.insets_of(opened.pane).top == 32 {
                break;
            }
        }
        assert_eq!(
            configures(&desk, &opened.toplevel),
            sent + 1,
            "nothing told while it shrinks"
        );
        frame(&mut desk, 16);
        assert_eq!(
            last_configured(&desk.client, &opened.toplevel),
            Some((client.size.w, client.size.h)),
            "back to 0, configured to the size it had"
        );
        assert_eq!(configures(&desk, &opened.toplevel), sent + 2);
        commit_buffer(
            &desk.client,
            &desk.qh,
            &opened.surface,
            client.size.w,
            client.size.h,
        );
        frame(&mut desk, 16);
        frame(&mut desk, 16);
        assert_eq!(client_rect(&desk, opened.pane), client);
        assert_eq!(desk.state.pane_outer_of(opened.pane), Some(outer));
        let _ = std::fs::remove_dir_all(&style);
    });
}

/// **#431: a frame that grows while its window is still answering a
/// new tile keeps the new tile.** A window tiled a moment ago folds
/// or unfolds its bar while its client is still drawing the size it
/// had; the frame was kept at that size, so once it stood still the
/// client was told it again and the window left its tile.
#[test]
fn a_frame_grown_while_its_window_answers_a_new_tile_keeps_the_new_tile() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let (mut desk, opened, style) = tiled_by(EASES);
        let tile = desk.placed(opened.pane);
        let client = client_rect(&desk, opened.pane);
        // Drawing a size of its own, smaller than the tile, as a
        // window that was floating a moment ago is.
        commit_buffer(
            &desk.client,
            &desk.qh,
            &opened.surface,
            client.size.w - 120,
            client.size.h - 80,
        );
        desk.pump();
        // And placed in a tile of its own again, larger than that,
        // as a layout switched on does, its client yet to answer.
        let moved = Rectangle::new(tile.loc, (tile.size.w - 20, tile.size.h - 20).into());
        let now = desk.state.clock.now();
        desk.state.place(
            opened.pane.get(),
            crate::state::snapshot::to_rect(moved),
            crate::script::AnimationSpec {
                duration: Duration::ZERO,
                ..crate::script::AnimationSpec::default()
            },
            now,
            crate::state::placement::Standing::Tile,
        );
        desk.pump();
        assert_ne!(
            client_rect(&desk, opened.pane).size,
            crate::state::inner(moved, desk.state.insets_of(opened.pane)).size,
            "the premise: the client has not drawn its tile"
        );

        grow_to(&mut desk, opened.pane, 40);
        for _ in 0..30 {
            frame(&mut desk, 16);
            if desk.state.insets_of(opened.pane).top == 72 {
                break;
            }
        }
        frame(&mut desk, 16);
        let room = crate::state::inner(moved, desk.state.insets_of(opened.pane));
        assert_eq!(
            last_configured(&desk.client, &opened.toplevel),
            Some((room.size.w, room.size.h)),
            "the client is told the room under the grown frame in its new \
             tile, not in the one it was leaving"
        );
        assert_eq!(desk.state.pane_outer_of(opened.pane), Some(moved));
        let _ = std::fs::remove_dir_all(&style);
    });
}

/// **A `growTop` of 0 is no change at all** (#423): a layer that
/// declares it and never moves it leaves a held window and a
/// floating one where they were, their clients configured with
/// nothing.
#[test]
fn a_grow_of_nothing_moves_and_tells_nothing() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        for tiled in [true, false] {
            let (mut desk, opened, style) = if tiled {
                tiled_by(GROWS)
            } else {
                framed_by(GROWS)
            };
            let outer = desk.state.pane_outer_of(opened.pane);
            let client = client_rect(&desk, opened.pane);
            let sent = configures(&desk, &opened.toplevel);
            grow_to(&mut desk, opened.pane, 0);
            for _ in 0..4 {
                frame(&mut desk, 16);
            }
            assert_eq!(
                desk.state.pane_outer_of(opened.pane),
                outer,
                "tiled {tiled}"
            );
            assert_eq!(client_rect(&desk, opened.pane), client, "tiled {tiled}");
            assert_eq!(
                configures(&desk, &opened.toplevel),
                sent,
                "tiled {tiled}: nothing configured"
            );
            assert_eq!(desk.state.insets_of(opened.pane).top, 32);
            let _ = std::fs::remove_dir_all(&style);
        }
    });
}

/// **#438: a floating window whose frame grows while it glides
/// arrives at the size it has.** The frame makes it taller as it
/// grows; a glide still aimed at the rectangle it had drew it
/// landing there and then jumping to its real size as it let go.
#[test]
fn a_window_whose_frame_grows_while_it_glides_is_aimed_at_its_grown_size() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let (mut desk, opened, style) = framed_by(GROWS);
        let pane = opened.pane;
        let outer = desk.state.pane_outer_of(pane).expect("outer");
        let away = Rectangle::new((outer.loc.x + 200, outer.loc.y + 100).into(), outer.size);
        let now = desk.state.clock.now();
        desk.state.place(
            pane.get(),
            crate::state::snapshot::to_rect(away),
            crate::script::AnimationSpec {
                duration: Duration::from_millis(320),
                ..crate::script::AnimationSpec::default()
            },
            now,
            crate::state::placement::Standing::Free,
        );
        frame(&mut desk, 16);
        let aimed = |desk: &Desk| {
            desk.state
                .panes
                .get(pane)
                .and_then(crate::present::course)
                .map(|(_, to)| to)
        };
        assert!(aimed(&desk).is_some(), "the premise: it is gliding");

        grow_to(&mut desk, pane, 31);
        frame(&mut desk, 16);
        let grown = desk.state.pane_outer_of(pane).expect("outer");
        assert_eq!(
            grown.size.h,
            away.size.h + 31,
            "the premise: the frame grew it"
        );
        assert_eq!(
            aimed(&desk),
            Some(Rectangle::new(
                (f64::from(grown.loc.x), f64::from(grown.loc.y)).into(),
                (f64::from(grown.size.w), f64::from(grown.size.h)).into(),
            )),
            "the glide arrives where the window is, its frame grown"
        );
        let _ = std::fs::remove_dir_all(&style);
    });
}

/// **A frame is told its window and its state** (#417): its id
/// and its process, and a maximise, a fullscreen and a minimize
/// each as the compositor set it, and whether a layout holds it in
/// a tile.
#[test]
fn a_frame_is_told_its_window_and_its_state() {
    let mut desk = Desk::new();
    let opened = desk.open_surface();
    let window = desk
        .state
        .panes
        .get(opened.pane)
        .and_then(Pane::client)
        .cloned()
        .expect("a client");
    let told = desk.state.frame_window(opened.pane).expect("a window");
    assert_eq!(told.id, opened.pane.get());
    assert_eq!(
        told.pid,
        Some(std::process::id()),
        "this process is the client"
    );
    assert!(told.floating, "no layout holds it in a tile");
    assert!(!told.maximized && !told.fullscreen && !told.minimized);
    desk.state.toggle_maximize(&window);
    assert!(
        desk.state
            .frame_window(opened.pane)
            .is_some_and(|told| told.maximized)
    );
    desk.state.minimize(opened.pane);
    assert!(
        desk.state
            .frame_window(opened.pane)
            .is_some_and(|told| told.minimized)
    );
}

/// **A frame sends a named action with its data to Lua** (#417):
/// a layer's `Solium.send("volume", { by, id })` reaches
/// `sol.on("frame", id, name, data)` with the window's id; and
/// `Solium.send("minimize")` is the built-in action, which Lua
/// hears as the window minimized and not as a frame action.
#[test]
fn a_frame_sends_a_named_action_with_its_data_to_lua() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let (mut desk, opened, style) = framed();
        let id = opened.pane.get();
        // What every frame is told as it is drawn.
        let window = desk.state.frame_window(opened.pane);
        if let Some(decoration) = desk
            .state
            .panes
            .get_mut(opened.pane)
            .and_then(Pane::decoration_mut)
        {
            decoration.tell_as_a_frame_would(
                &crate::decoration::Look {
                    title: "",
                    app_id: "",
                    focused: true,
                    pointer_inside: true,
                    caret: None,
                    values: crate::decoration::Values::none(),
                    window,
                    window_values: crate::decoration::Values::none(),
                },
                400,
                300,
            );
        }
        assert_eq!(
            told(&mut desk, opened.pane, "windowId"),
            i32::try_from(id).expect("small")
        );
        let bar = at(&desk, opened.pane, (20.0, 16.0));
        move_pointer(&mut desk.state, bar, 10);
        let region = region(&desk);
        crate::synth::send_axis(&mut desk.state, region, (0.0, 120.0), 11);
        desk.pump();
        assert_eq!(desk.events(), format!("{id} volume -1 {id}"));
        drop(desk);
        let _ = std::fs::remove_dir_all(&style);
    });
}

/// **A right press and the wheel on the titlebar reach the frame,
/// and not the client** (#417): the right press is the right
/// button to the frame's QML, and moves nothing; the wheel turn
/// reaches its `WheelHandler`; the client is told neither.
#[test]
fn a_right_press_and_the_wheel_on_the_titlebar_reach_the_frame_and_not_the_client() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let (mut desk, opened, style) = framed();
        let before = desk.state.pane_outer_of(opened.pane);
        let bar = at(&desk, opened.pane, (20.0, 16.0));
        move_pointer(&mut desk.state, bar, 10);
        let region = region(&desk);
        crate::synth::send_button(&mut desk.state, region, BTN_RIGHT, ButtonState::Pressed, 11);
        move_pointer(&mut desk.state, (bar.0 + 40.0, bar.1 + 3.0), 12);
        crate::synth::send_button(
            &mut desk.state,
            region,
            BTN_RIGHT,
            ButtonState::Released,
            13,
        );
        crate::synth::send_axis(&mut desk.state, region, (0.0, 120.0), 14);
        desk.pump();
        assert_eq!(
            (
                told(&mut desk, opened.pane, "rights"),
                told(&mut desk, opened.pane, "wheels"),
                desk.state.pane_outer_of(opened.pane) == before,
                desk.client.buttons.clone(),
            ),
            (1, 1, true, Vec::new()),
            "(right presses the frame heard, wheel turns it heard, \
             the window where it was, the buttons the client was told)"
        );
        drop(desk);
        let _ = std::fs::remove_dir_all(&style);
    });
}

/// **A frame holding a grab takes the press over the client, and
/// is dismissed outside** (#417): opened, the panel over the top
/// of the client takes a press there, which the client is not
/// told; a press on the client below the panel dismisses the grab
/// and is swallowed, with its release; and the next press there
/// is the client's.
#[test]
fn a_frame_holding_a_grab_takes_the_press_over_the_client_and_is_dismissed_outside() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let (mut desk, opened, style) = framed();
        let panel = at(&desk, opened.pane, (30.0, 50.0));
        let below = at(&desk, opened.pane, (30.0, 200.0));
        assert!(
            desk.state.surface_under(panel.into()).is_some(),
            "the premise: the client is under where the panel opens"
        );
        open_panel(&mut desk, opened.pane);
        move_pointer(&mut desk.state, panel, 20);
        assert!(
            desk.state.surface_under(panel.into()).is_none(),
            "the client has the pointer under the open panel"
        );
        click(&mut desk, BTN_LEFT, 21);
        move_pointer(&mut desk.state, below, 22);
        click(&mut desk, BTN_LEFT, 23);
        let after_dismissal = desk.client.buttons.clone();
        click(&mut desk, BTN_LEFT, 25);
        assert_eq!(
            (
                told(&mut desk, opened.pane, "panelPresses"),
                told(&mut desk, opened.pane, "dismissals"),
                told(&mut desk, opened.pane, "slider"),
                after_dismissal,
                desk.state.decorations.holding,
                desk.client.buttons.clone(),
            ),
            (
                1,
                1,
                30,
                Vec::new(),
                None,
                vec![(BTN_LEFT, true), (BTN_LEFT, false)]
            ),
            "(presses the panel took, times it was dismissed, where its slider is, \
             what the client was told until then, the frame holding, \
             what the client was told of the next click)"
        );
        drop(desk);
        let _ = std::fs::remove_dir_all(&style);
    });
}

/// **A slider dragged off the frame follows the pointer** (#417):
/// a press in the panel holds the pointer for the frame until it
/// is let go of, wherever it goes, so the slider is dragged to
/// where the pointer is and the release dismisses nothing.
#[test]
fn a_slider_dragged_off_the_frame_follows_the_pointer() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let (mut desk, opened, style) = framed();
        open_panel(&mut desk, opened.pane);
        let panel = at(&desk, opened.pane, (30.0, 50.0));
        move_pointer(&mut desk.state, panel, 20);
        let region = region(&desk);
        crate::synth::send_button(&mut desk.state, region, BTN_LEFT, ButtonState::Pressed, 21);
        move_pointer(&mut desk.state, (panel.0 + 70.0, panel.1 + 300.0), 22);
        let dragged = told(&mut desk, opened.pane, "slider");
        crate::synth::send_button(&mut desk.state, region, BTN_LEFT, ButtonState::Released, 23);
        desk.pump();
        assert_eq!(
            (
                dragged,
                told(&mut desk, opened.pane, "dismissals"),
                desk.state.decorations.holding,
                desk.client.buttons.clone(),
            ),
            (100, 0, Some(opened.pane), Vec::new()),
            "(where the slider was dragged to, times the panel was dismissed, \
             the frame holding, the buttons the client was told)"
        );
        drop(desk);
        let _ = std::fs::remove_dir_all(&style);
    });
}

/// **Locking the session dismisses a frame's grab and its press**
/// (#417): the panel hears it is dismissed, no frame holds the
/// pointer or the keys, and a press the frame was given is
/// forgotten, its release being the lock screen's.
#[test]
fn locking_the_session_dismisses_a_frames_grab_and_its_press() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let (mut desk, opened, style) = framed();
        open_panel(&mut desk, opened.pane);
        let panel = at(&desk, opened.pane, (30.0, 50.0));
        move_pointer(&mut desk.state, panel, 20);
        let region = region(&desk);
        crate::synth::send_button(&mut desk.state, region, BTN_LEFT, ButtonState::Pressed, 21);
        let pressed = desk.state.decorations.pressed;
        desk.state.release_grabs();
        assert_eq!(
            (
                pressed,
                told(&mut desk, opened.pane, "dismissals"),
                desk.state.decorations.holding,
                desk.state.decorations.pressed,
            ),
            (Some(opened.pane), 1, None, None),
            "(the press the frame was given, times the panel was dismissed, \
             the frame holding, the press held after)"
        );
        drop(desk);
        let _ = std::fs::remove_dir_all(&style);
    });
}

/// **A frame holding a grab takes the keys no binding takes**
/// (#417), on a `us,ru` keymap with Russian active: Escape
/// reaches the item with the frame's keyboard focus, and a letter
/// reaches it too, not the window; a binding still runs. Once the
/// panel has closed, a letter is the window's again.
#[test]
fn a_frame_holding_a_grab_takes_the_keys_no_binding_takes() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let (mut desk, opened, style) = framed();
        russian(&mut desk.state);
        desk.install("sol.bind(\"super+q\", function() sol.status(\"bound\") end)");
        open_panel(&mut desk, opened.pane);
        desk.pump();
        desk.client.key_events.clear();
        let region = region(&desk);
        crate::synth::key(&mut desk.state, region, "Cyrillic_ef", 30);
        crate::synth::key(&mut desk.state, region, "super+Cyrillic_shorti", 31);
        let bound = desk.state.status.clone();
        crate::synth::key(&mut desk.state, region, "Escape", 32);
        desk.pump();
        // Presses only: the release of a key a binding took goes
        // on to the window, as it always has.
        let pressed = |desk: &Desk| {
            desk.client
                .key_events
                .iter()
                .filter(|(_, pressed)| *pressed)
                .count()
        };
        let while_open = pressed(&desk);
        crate::synth::key(&mut desk.state, region, "Cyrillic_ef", 33);
        desk.pump();
        assert_eq!(
            (
                while_open,
                bound.as_str(),
                told(&mut desk, opened.pane, "escapes"),
                desk.state.decorations.holding,
                pressed(&desk),
            ),
            (0, "bound", 1, None, 1),
            "(the window was told no key while the panel was open, the binding \
             ran, the Escapes the panel heard, the frame holding, \
             the window told the letter after)"
        );
        drop(desk);
        let _ = std::fs::remove_dir_all(&style);
    });
}
