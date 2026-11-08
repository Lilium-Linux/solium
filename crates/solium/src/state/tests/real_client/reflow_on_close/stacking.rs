use super::*;
use crate::render::{Stacked, Walked};
use crate::scripted::Layer as Scripted;
use smithay::backend::input::ButtonState;
use zwlr_layer_shell_v1::Layer as Client;

/// Something on the monitor, by a number both ends of the
/// fixture agree on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seen {
    /// A client's layer surface, by its `wl_surface`'s id.
    Layer(u32),
    /// A script's surface.
    Script(crate::scripted::SurfaceId),
    /// A pane.
    Pane(crate::pane::PaneId),
    /// An effect's slot beside a surface.
    Effect(crate::effect::rules::Slot),
}

/// The desk's monitor.
fn screen() -> Rectangle<i32, Logical> {
    Rectangle::new((0, 0).into(), (1920, 1080).into())
}

/// What the desk's monitor draws, topmost first.
fn drawn(desk: &Desk) -> Vec<Seen> {
    drawn_by(desk, screen())
}

/// What the monitor at `monitor` draws, topmost first, through
/// the walk `elements` draws with, no slot ready: so every
/// stacking test is the walk's guard that with no slot nothing
/// moves.
fn drawn_by(desk: &Desk, monitor: Rectangle<i32, Logical>) -> Vec<Seen> {
    drawn_with(desk, monitor, &|_, _| false)
}

/// [`drawn_by`], with the slots `ready` says are ready.
fn drawn_with(
    desk: &Desk,
    monitor: Rectangle<i32, Logical>,
    ready: &dyn Fn(&crate::effect::plan::Owner, crate::effect::rules::Slot) -> bool,
) -> Vec<Seen> {
    let output = desk
        .state
        .output_for(monitor)
        .map(|output| output.name())
        .unwrap_or_default();
    let mut drawn = Vec::new();
    crate::render::stacked_walk(
        crate::render::stacked(&desk.state, monitor, desk.state.clock.now()),
        &output,
        ready,
        |walked| match walked {
            Walked::Stacked(Stacked::Layer(surface, _)) => {
                drawn.push(Seen::Layer(surface.wl_surface().id().protocol_id()));
            }
            Walked::Stacked(Stacked::Surface(id, _, _)) => drawn.push(Seen::Script(id)),
            Walked::Stacked(Stacked::Panes(nodes)) => {
                drawn.extend(nodes.into_iter().map(|((pane, ..), _)| Seen::Pane(pane)))
            }
            Walked::Slot(_, slot, ..) => drawn.push(Seen::Effect(slot)),
        },
    );
    drawn
}

/// Whether `top` is drawn over `bottom`, both being drawn.
fn drawn_over(desk: &Desk, top: Seen, bottom: Seen) -> bool {
    drawn_over_by(desk, screen(), top, bottom)
}

/// [`drawn_over`], on the monitor at `monitor`.
fn drawn_over_by(desk: &Desk, monitor: Rectangle<i32, Logical>, top: Seen, bottom: Seen) -> bool {
    let drawn = drawn_by(desk, monitor);
    let at = |seen| drawn.iter().position(|each| *each == seen);
    matches!((at(top), at(bottom)), (Some(top), Some(bottom)) if top < bottom)
}

/// A client's layer surface at `layer`, anchored to the top of
/// the monitor: across its whole width, or `width` wide from its
/// left edge. With a buffer of that size, so it is drawn and can
/// be pointed at.
fn layer_surface(
    desk: &mut Desk,
    layer: Client,
    width: Option<i32>,
    height: i32,
    exclusive: i32,
) -> (
    wl_surface::WlSurface,
    zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
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
        layer,
        "stacking-test".to_string(),
        &desk.qh,
        (),
    );
    match width {
        Some(width) => {
            layered.set_anchor(Anchor::Top | Anchor::Left);
            layered.set_size(width.unsigned_abs(), height.unsigned_abs());
        }
        None => {
            layered.set_anchor(Anchor::Top | Anchor::Left | Anchor::Right);
            layered.set_size(0, height.unsigned_abs());
        }
    }
    layered.set_exclusive_zone(exclusive);
    surface.commit();
    desk.pump();
    commit_buffer(
        &desk.client,
        &desk.qh,
        &surface,
        width.unwrap_or(screen().size.w),
        height,
    );
    desk.pump();
    (surface, layered)
}

fn id(surface: &wl_surface::WlSurface) -> u32 {
    wayland_client::Proxy::id(surface).protocol_id()
}

/// Where the pointer is delivered at a point: the surface, by id.
fn delivered(desk: &Desk, x: f64, y: f64) -> Option<u32> {
    desk.state
        .surface_under((x, y).into())
        .map(|(surface, _)| surface.id().protocol_id())
}

/// A script's interactive surface at `layer`, over `rect`, its
/// scene stood in for by one that takes every press there: these
/// tests are about the order, not about what a scene claims.
fn scripted(
    desk: &mut Desk,
    name: &str,
    layer: Scripted,
    rect: Rectangle<i32, Logical>,
) -> crate::scripted::SurfaceId {
    desk.state
        .declare_surface(crate::scripted::Declaration::for_test(
            name,
            std::path::PathBuf::from("/nonexistent/stacking-test.qml"),
            layer,
            crate::scripted::On::Rect(rect),
        ));
    let id = desk
        .state
        .surfaces
        .named(name)
        .expect("the surface was declared");
    desk.state
        .surfaces
        .get_mut(id)
        .expect("live")
        .stand_in(crate::scripted::Stand::solid());
    id
}

/// The script's surface a press at a point is offered to.
fn claimed(desk: &Desk, x: f64, y: f64) -> Option<crate::scripted::SurfaceId> {
    desk.state
        .surface_claiming(true, (x, y).into(), crate::qml::hosted::Asking::Press)
        .map(|(_, id, _)| id)
}

fn window_of(desk: &Desk, pane: crate::pane::PaneId) -> Window {
    desk.state
        .panes
        .get(pane)
        .and_then(Pane::client)
        .cloned()
        .expect("the pane has its client")
}

/// A window at `at`, sent fullscreen, answering with a buffer
/// the size of the monitor as a client does.
fn fullscreen(desk: &mut Desk, at: (i32, i32)) -> (Opened, Window) {
    fullscreen_on(desk, at, screen())
}

/// [`fullscreen`], on the monitor at `monitor`, which `at` is
/// on.
fn fullscreen_on(
    desk: &mut Desk,
    at: (i32, i32),
    monitor: Rectangle<i32, Logical>,
) -> (Opened, Window) {
    let opened = desk.open_surface();
    let window = window_of(desk, opened.pane);
    desk.state.space.map_element(window.clone(), at, false);
    desk.state.space.refresh();
    opened.toplevel.set_fullscreen(None);
    desk.pump();
    commit_buffer(
        &desk.client,
        &desk.qh,
        &opened.surface,
        monitor.size.w,
        monitor.size.h,
    );
    desk.pump();
    landed(desk);
    assert_eq!(
        desk.state.real_geometry(&window),
        Some(monitor),
        "the premise: the window covers the monitor, bar and all"
    );
    (opened, window)
}

/// Let every animation land: a window opening is drawn fading in,
/// and one drawn at nothing covers nothing.
fn landed(desk: &mut Desk) {
    desk.state.clock.advance(Duration::from_secs(1));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    desk.state.sync_panes();
    desk.pump();
}

fn strip(x: i32, width: i32) -> Rectangle<i32, Logical> {
    Rectangle::new((x, 0).into(), (width, 30).into())
}

/// **An overlay surface mapped before a bar is drawn over it,
/// and takes the press there -- before the edge of the window
/// under both.** The bar was drawn on top because it was mapped
/// last, while the press went to the overlay; and neither beat
/// the window's resize border, which `pointer_button` asked
/// before any client's surface.
#[test]
fn an_overlay_mapped_before_a_bar_is_drawn_over_it_and_takes_the_press() {
    let mut desk = Desk::new();
    let opened = desk.open_surface();
    let window = window_of(&desk, opened.pane);
    desk.state.space.map_element(window, (500, 10), false);
    desk.state.space.refresh();
    landed(&mut desk);
    let edge = Point::<f64, Logical>::from((532.0, 12.0));
    assert_eq!(
        desk.state.claim_under(edge),
        Claim::Chrome(Chrome::Resize(ResizeEdge::Top)),
        "the premise: with nothing over it, a press there drags the window's edge"
    );

    let (overlay, _overlay) = layer_surface(&mut desk, Client::Overlay, None, 30, -1);
    let (bar, _bar) = layer_surface(&mut desk, Client::Top, None, 30, 30);

    assert!(
        drawn_over(&desk, Seen::Layer(id(&overlay)), Seen::Layer(id(&bar)))
            && drawn_over(&desk, Seen::Layer(id(&bar)), Seen::Pane(opened.pane)),
        "the overlay, the bar, then the window: {:?}",
        drawn(&desk)
    );
    assert_eq!(
        (
            delivered(&desk, edge.x, edge.y),
            desk.state.claim_under(edge)
        ),
        (Some(id(&overlay)), Claim::Nothing),
        "(the surface the pointer reaches, what the compositor makes of a press): \
         the overlay is on top and the press is its"
    );
}

/// **The same, for a script's surfaces**: one declared at
/// `overlay` before one at `top` is drawn over it and is the one
/// offered the press.
#[test]
fn a_scripted_overlay_is_drawn_over_a_scripted_bar_and_takes_the_press() {
    let mut desk = Desk::new();
    let overlay = scripted(&mut desk, "overlay", Scripted::Overlay, strip(0, 1920));
    let bar = scripted(&mut desk, "bar", Scripted::Top, strip(0, 1920));
    assert!(
        drawn_over(&desk, Seen::Script(overlay), Seen::Script(bar)),
        "the scripted overlay over the scripted bar: {:?}",
        drawn(&desk)
    );
    assert_eq!(
        (
            claimed(&desk, 100.0, 15.0),
            desk.state.claim_under((100.0, 15.0).into())
        ),
        (Some(overlay), Claim::Surface)
    );
}

/// **A layer-shell `behind` slot is drawn just under its surface,
/// and a scripted surface's `front` just over it.**
#[test]
fn a_layer_shell_behind_slot_is_drawn_just_under_its_surface_and_a_surface_front_slot_just_over_it()
{
    use crate::effect::plan::Owner;
    use crate::effect::rules::Slot;
    let mut desk = Desk::new();
    let (bar, _bar) = layer_surface(&mut desk, Client::Top, None, 30, 30);
    let bar_id = id(&bar);
    let script = scripted(&mut desk, "strip", Scripted::Bottom, strip(0, 1920));
    let ready = |owner: &Owner, slot: Slot| match owner {
        Owner::LayerShell(id) => id.protocol_id() == bar_id && slot == Slot::Behind,
        Owner::Surface(id, _) => *id == script && slot == Slot::Front,
        Owner::Pane(..) => false,
    };
    let drawn = drawn_with(&desk, screen(), &ready);
    let at = |seen| drawn.iter().position(|each| *each == seen).expect("drawn");
    assert_eq!(
        at(Seen::Effect(Slot::Behind)),
        at(Seen::Layer(bar_id)) + 1,
        "{drawn:?}"
    );
    assert_eq!(
        at(Seen::Effect(Slot::Front)) + 1,
        at(Seen::Script(script)),
        "{drawn:?}"
    );
}

/// **Every failure leaves a layer surface and a scripted surface
/// drawn**: the stacked walk with failing rules on both is the
/// walk with none (the owners `every_failure_leaves_the_part_drawn`
/// cannot make).
#[test]
fn every_failure_leaves_layer_and_scripted_surfaces_drawn() {
    use crate::effect::plan::Owner;
    use crate::effect::rules::Slot;
    let mut desk = Desk::new();
    let (bar, _bar) = layer_surface(&mut desk, Client::Top, None, 30, 30);
    let script = scripted(&mut desk, "strip", Scripted::Bottom, strip(0, 1920));
    let plain = drawn(&desk);
    let fixtures = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/effects"
    ));
    desk.state.effects = crate::effect::host::Host::new(crate::effect::host::Library::with(
        Some(fixtures.to_owned()),
        fixtures.join("none"),
    ));
    // No shipped effects, so no shipped `client_blur` to draw either.
    desk.state.effect_settings.client_blur = None;
    let rules = r#"{ { match = "*", part = "layer_shell:*", slot = "behind", effect = "ring" },
                     { match = "*", part = "layer_shell:*", slot = "replace", effect = "fail-compile" },
                     { match = "*", part = "surface:strip", slot = "front", effect = "ring" },
                     { match = "*", part = "surface:strip", slot = "replace", effect = "fail-compile" } }"#;
    desk.state.apply_effects(Ok(
        crate::effect::rules::parse(&tree(rules)).expect("parses")
    ));
    desk.state
        .effects
        .compile_pending(&mut crate::effect::host::tests::Refusing);
    let mut slots = crate::render::build_slots(&mut desk.state);
    let bar_id = id(&bar);
    let mut owners: Vec<(bool, Slot)> = slots
        .wants()
        .map(|(owner, slot, _)| match owner {
            Owner::LayerShell(surface) => (surface.protocol_id() == bar_id, slot),
            Owner::Surface(surface, _) => (*surface == script, slot),
            Owner::Pane(..) => (false, slot),
        })
        .collect();
    owners.sort_by_key(|(_, slot)| format!("{slot:?}"));
    assert_eq!(
        owners.len(),
        4,
        "the premise: both surfaces' two slots are wanted, and nothing else: {owners:?}"
    );
    assert!(owners.iter().all(|(theirs, _)| *theirs), "{owners:?}");
    crate::render::record_boxes(&desk.state, &mut slots);
    let Solium {
        store,
        effects,
        chains,
        memos,
        xrays,
        ..
    } = &mut desk.state;
    let mut cx = crate::render::RunCx {
        store,
        effects: &*effects,
        chains,
        masked: None,
        now: 0.0,
        pass: 1,
        memos,
        xrays: &*xrays,
        motion: (std::time::Duration::ZERO, 0.0),
        waited: false,
        drawing: crate::drawing::Drawing::every(),
    };
    let mut runner = crate::render::tests::NoGpu::default();
    crate::render::run_slots(
        &mut cx,
        &mut crate::pool::Pool::new(0),
        &mut runner,
        &mut slots,
        &crate::effect::store::Drawn::default(),
        crate::render::Nest::Inner,
    );
    assert_eq!(
        runner.runs, 4,
        "the premise: every chain was run, and failed"
    );
    assert_eq!(
        drawn_with(&desk, screen(), &|owner, slot| slots.is_ready(owner, slot)),
        plain
    );
}

/// **Below the windows, by layer too**: a client's background
/// mapped after its bottom surface stays under it, and at each
/// layer the client's surface is over the script's -- and the
/// script's dock has the press only where nothing is drawn over
/// it.
#[test]
fn a_background_mapped_after_a_bottom_surface_stays_under_it() {
    let mut desk = Desk::new();
    let opened = desk.open_surface();
    let window = window_of(&desk, opened.pane);
    desk.state.space.map_element(window, (600, 500), false);
    desk.state.space.refresh();
    landed(&mut desk);
    let script_bottom = scripted(&mut desk, "dock", Scripted::Bottom, strip(0, 1920));
    let script_background = scripted(&mut desk, "wallpaper", Scripted::Background, screen());
    let (bottom, _bottom) = layer_surface(&mut desk, Client::Bottom, Some(200), 30, 0);
    let (background, _background) = layer_surface(&mut desk, Client::Background, None, 30, 0);
    let below: Vec<Seen> = drawn(&desk)
        .into_iter()
        .skip_while(|seen| *seen != Seen::Pane(opened.pane))
        .skip(1)
        .collect();
    assert_eq!(
        below,
        vec![
            Seen::Layer(id(&bottom)),
            Seen::Script(script_bottom),
            Seen::Layer(id(&background)),
            Seen::Script(script_background),
        ],
        "under the window: the client's bottom surface, the script's, the \
         client's background, the script's"
    );
    // The client's bottom surface is drawn over the script's dock,
    // so it has the pointer there and the dock does not have the
    // press (#461). Beside it, the dock is drawn over the
    // client's background, so the dock has the press and no
    // client has the pointer.
    assert_eq!(
        (
            delivered(&desk, 100.0, 15.0),
            claimed_below(&desk, 100.0, 15.0),
            delivered(&desk, 1000.0, 15.0),
            claimed_below(&desk, 1000.0, 15.0),
        ),
        (Some(id(&bottom)), None, None, Some(script_bottom)),
        "(on the client's bottom surface: the surface the pointer reaches, the \
         script's surface offered the press; the same beside it)"
    );
}

/// **A client's surfaces below the windows take input as
/// `layer_shell.input_below` says** (#461). By default each one
/// has the pointer wherever no window and no script's surface
/// over it does, on the bottom layer and on the background, and
/// a window over one keeps its own. `false` passes every one
/// through, so the script's wallpaper under them has the press;
/// a list takes only the namespaces it names.
#[test]
fn a_client_surface_below_the_windows_takes_input_as_layer_shell_input_below_says() {
    let mut desk = Desk::new();
    let opened = desk.open_surface();
    commit_buffer(&desk.client, &desk.qh, &opened.surface, 400, 300);
    desk.pump();
    let window = window_of(&desk, opened.pane);
    desk.state.space.map_element(window, (600, 0), false);
    desk.state.space.refresh();
    landed(&mut desk);
    let wallpaper = scripted(&mut desk, "wallpaper", Scripted::Background, screen());
    let (bottom, _bottom) = layer_surface(&mut desk, Client::Bottom, Some(200), 200, 0);
    let (background, _background) = layer_surface(&mut desk, Client::Background, None, 200, 0);
    let on_the_window = desk
        .state
        .surface_under((700.0, 100.0).into())
        .map(|(surface, _)| surface.id().protocol_id());
    assert_eq!(
        on_the_window,
        Some(id(&opened.surface)),
        "the premise: the window is over both, and has the pointer over them"
    );
    let seen = |desk: &Desk| {
        (
            delivered(desk, 100.0, 100.0),
            delivered(desk, 1500.0, 100.0),
            delivered(desk, 700.0, 100.0),
            claimed_below(desk, 1500.0, 100.0),
        )
    };
    assert_eq!(
        seen(&desk),
        (
            Some(id(&bottom)),
            Some(id(&background)),
            on_the_window,
            None
        ),
        "by default: (the bottom surface, the background beside it, the window \
         over both, the wallpaper under the background)"
    );

    desk.state.layer_shell.input_below = crate::layer::InputBelow::Nothing;
    assert_eq!(
        seen(&desk),
        (None, None, on_the_window, Some(wallpaper)),
        "with `false`, every client surface below the windows is passed through"
    );

    desk.state.layer_shell.input_below =
        crate::layer::InputBelow::Named(vec![crate::effect::rules::Glob::parse("stacking-*")]);
    assert_eq!(
        seen(&desk).0,
        Some(id(&bottom)),
        "a prefix naming the namespace takes it"
    );
    desk.state.layer_shell.input_below =
        crate::layer::InputBelow::Named(vec![crate::effect::rules::Glob::parse("someone-else")]);
    assert_eq!(
        seen(&desk),
        (None, None, on_the_window, Some(wallpaper)),
        "a list that does not name the namespace passes it through"
    );
}

/// **A press on a client's background surface reaches it**
/// (#461), through the real input path: the client is told the
/// pointer entered it and the button went down and up there.
#[test]
fn a_press_on_a_clients_background_surface_reaches_it() {
    let mut desk = Desk::new();
    let _pointer = desk.client.seat_pointer(&desk.qh);
    desk.pump();
    let (background, _background) = layer_surface(&mut desk, Client::Background, None, 30, 0);
    move_pointer(&mut desk.state, (960.0, 15.0), 10);
    click(&mut desk, 11);
    desk.pump();
    assert_eq!(
        (desk.client.buttons.clone(), desk.client.clicked_on.clone()),
        (
            vec![(0x110, true), (0x110, false)],
            vec![Some(id(&background)); 2]
        ),
        "(the buttons the client was told, the surface each arrived on)"
    );
}

/// **A touch on a client's bottom surface reaches it** (#461):
/// touch walks the same layers the pointer does.
#[test]
fn a_touch_on_a_clients_bottom_surface_reaches_it() {
    let mut desk = Desk::new();
    let _touch = desk.client.seat_touch(&desk.qh);
    desk.pump();
    let (bottom, _bottom) = layer_surface(&mut desk, Client::Bottom, Some(200), 30, 0);
    assert_eq!(
        desk.state
            .touch_under((100.0, 15.0).into())
            .map(|(surface, _)| surface.id().protocol_id()),
        Some(id(&bottom))
    );
    let region = crate::monitor::union(&desk.state.space).expect("a monitor");
    crate::synth::touch_down(&mut desk.state, region, 0, (100.0, 15.0), 10_000);
    desk.pump();
    assert!(
        desk.client.touches.iter().any(|event| matches!(
            event,
            wl_touch::Event::Down { surface, .. } if id(surface) == id(&bottom)
        )),
        "the bottom surface was not told of the touch: {:?}",
        desk.client.touches
    );
}

/// **The scenes phase renders what a wanted cut holds and
/// nothing else** (Ruling F31): nothing with no cut wanted; the
/// script's wallpaper alone for a wallpaper cut on the desk's
/// monitor; the dock on `bottom` too for a bottom cut; never the
/// bar on `top`.
#[test]
fn the_scenes_phase_renders_only_what_a_wanted_cut_holds() {
    use crate::effect::xray::Cut;
    use crate::render::SceneRef;
    let mut desk = Desk::new();
    let _bar = scripted(&mut desk, "bar", Scripted::Top, strip(0, 1920));
    let dock = scripted(&mut desk, "dock", Scripted::Bottom, strip(0, 1920));
    let wallpaper = scripted(&mut desk, "wallpaper", Scripted::Background, screen());
    assert!(crate::render::scenes_needed(&desk.state).is_empty());
    let monitor = desk
        .state
        .output_for(screen())
        .map(|output| output.name())
        .expect("the desk's monitor");
    desk.state.xrays.want(&monitor, Cut::Background);
    assert_eq!(
        crate::render::scenes_needed(&desk.state),
        vec![SceneRef::Surface(wallpaper, monitor.clone())]
    );
    desk.state.xrays.clear_wants();
    desk.state.xrays.want(&monitor, Cut::Bottom);
    assert_eq!(
        crate::render::scenes_needed(&desk.state),
        vec![
            SceneRef::Surface(dock, monitor.clone()),
            SceneRef::Surface(wallpaper, monitor)
        ]
    );
}

/// **A T2 rule on the background wants no slot** (Ruling F29):
/// nothing lies below a part on `background`, so its blur reads
/// nothing and is no problem either; the same rule on a `bottom`
/// surface, which reads the wallpaper, wants its slot.
#[test]
fn a_t2_rule_on_the_background_wants_no_slot() {
    use crate::effect::plan::Owner;
    use crate::effect::rules::Slot;
    let mut desk = Desk::new();
    let dock = scripted(&mut desk, "dock", Scripted::Bottom, strip(0, 1920));
    let wallpaper = scripted(&mut desk, "wallpaper", Scripted::Background, screen());
    desk.state
        .apply_effects(Ok(crate::effect::rules::parse(&tree(
            r#"{ { match = "*", part = "surface:dock", slot = "behind", effect = "blur" },
             { match = "*", part = "surface:wallpaper", slot = "behind", effect = "blur" } }"#,
        ))
        .expect("parses")));
    assert!(
        desk.state.effects.problems().is_empty(),
        "{:?}",
        desk.state.effects.problems()
    );
    let monitor = desk
        .state
        .output_for(screen())
        .map(|output| output.name())
        .expect("the desk's monitor");
    let slots = crate::render::build_slots(&mut desk.state);
    assert!(
        slots.wanted(&Owner::Surface(dock, monitor.clone()), Slot::Behind),
        "the dock reads the wallpaper"
    );
    assert!(
        !slots.wanted(&Owner::Surface(wallpaper, monitor), Slot::Behind),
        "the wallpaper has nothing below it to read"
    );
}

/// **xray holds the cut's bands and nothing else**: the
/// wallpaper and, with `bottom`, the desktop's widgets, the
/// client's over the script's, in `stacked`'s order; never a
/// pane, a `top` or `overlay` surface, or the pointer. And
/// `band_of` names each entry's band.
#[test]
fn xray_holds_the_cuts_bands_and_nothing_else() {
    use crate::effect::xray::{Cut, SessionLayers, XraySource as _};
    use crate::stack::{Band, Owner as Whose};
    let mut desk = Desk::new();
    let opened = desk.open_surface();
    let window = window_of(&desk, opened.pane);
    desk.state.space.map_element(window, (600, 500), false);
    desk.state.space.refresh();
    landed(&mut desk);
    let _bar = scripted(&mut desk, "bar", Scripted::Top, strip(0, 1920));
    let dock = scripted(&mut desk, "dock", Scripted::Bottom, strip(0, 1920));
    let wallpaper = scripted(&mut desk, "wallpaper", Scripted::Background, screen());
    let (_top, _top_layer) = layer_surface(&mut desk, Client::Top, None, 30, 0);
    let (_overlay, _overlay_layer) = layer_surface(&mut desk, Client::Overlay, Some(200), 30, 0);
    let (widgets, _widgets) = layer_surface(&mut desk, Client::Bottom, Some(200), 30, 0);
    let (backdrop, _backdrop) = layer_surface(&mut desk, Client::Background, None, 30, 0);
    let output = desk.state.output_for(screen()).expect("the desk's monitor");
    let seen = |entries: &[Stacked]| -> Vec<Seen> {
        entries
            .iter()
            .flat_map(|each| match each {
                Stacked::Layer(surface, _) => {
                    vec![Seen::Layer(surface.wl_surface().id().protocol_id())]
                }
                Stacked::Surface(id, ..) => vec![Seen::Script(*id)],
                Stacked::Panes(nodes) => nodes
                    .iter()
                    .map(|((pane, ..), _)| Seen::Pane(*pane))
                    .collect(),
            })
            .collect()
    };
    let background = SessionLayers.entries(&desk.state, &output, Cut::Background);
    let bottom = SessionLayers.entries(&desk.state, &output, Cut::Bottom);
    assert_eq!(
        seen(&background),
        vec![Seen::Layer(id(&backdrop)), Seen::Script(wallpaper)],
        "the wallpaper alone, the client's over the script's"
    );
    assert_eq!(
        seen(&bottom),
        vec![
            Seen::Layer(id(&widgets)),
            Seen::Script(dock),
            Seen::Layer(id(&backdrop)),
            Seen::Script(wallpaper),
        ],
        "and the desktop's widgets over it"
    );
    assert!(background.iter().all(|each| matches!(
        crate::render::band_of(&desk.state, each),
        Band::Layer(Scripted::Background, _)
    )));
    assert!(bottom.iter().all(|each| !matches!(each, Stacked::Panes(_))));
    let all = crate::render::stacked(&desk.state, screen(), desk.state.clock.now());
    let order: Vec<&Stacked> = all.iter().filter(|each| bottom.contains(each)).collect();
    assert_eq!(
        order,
        bottom.iter().collect::<Vec<_>>(),
        "not stacked's order"
    );
    let bands: Vec<Band> = all
        .iter()
        .map(|each| crate::render::band_of(&desk.state, each))
        .collect();
    assert_eq!(
        bands,
        vec![
            Band::Layer(Scripted::Overlay, Whose::Client),
            Band::Layer(Scripted::Top, Whose::Client),
            Band::Layer(Scripted::Top, Whose::Script),
            Band::Windows,
            Band::Layer(Scripted::Bottom, Whose::Client),
            Band::Layer(Scripted::Bottom, Whose::Script),
            Band::Layer(Scripted::Background, Whose::Client),
            Band::Layer(Scripted::Background, Whose::Script),
        ],
        "the band of each of stacked's entries"
    );
}

/// **A live cut holds what is drawn below its part, and nothing
/// of it or above it** (#209): under a bar on `top`, both windows
/// and every layer below them; under the upper window, the lower
/// one and those layers, not the bar or itself.
#[test]
fn a_live_cut_holds_what_is_below_its_part_and_nothing_above() {
    use crate::effect::plan::Owner as Part;
    use crate::effect::xray::beneath;
    use smithay::reexports::wayland_server::Resource as _;
    let mut desk = Desk::new();
    let lower = desk.open_surface();
    let upper = desk.open_surface();
    landed(&mut desk);
    let wallpaper = scripted(&mut desk, "wallpaper", Scripted::Background, screen());
    let (bar, _bar) = layer_surface(&mut desk, Client::Top, None, 30, 0);
    let (widgets, _widgets) = layer_surface(&mut desk, Client::Bottom, Some(200), 30, 0);
    let output = desk.state.output_for(screen()).expect("the desk's monitor");
    let all = crate::render::stacked(&desk.state, screen(), desk.state.clock.now());
    let panes: Vec<crate::pane::PaneId> = all
        .iter()
        .filter_map(|each| match each {
            Stacked::Panes(nodes) => Some(
                nodes
                    .iter()
                    .map(|((pane, ..), _)| *pane)
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(
        panes,
        [upper.pane, lower.pane],
        "the premise: the one opened last is on top"
    );
    let seen = |entries: &[Stacked]| -> Vec<Seen> {
        entries
            .iter()
            .flat_map(|each| match each {
                Stacked::Layer(surface, _) => {
                    vec![Seen::Layer(surface.wl_surface().id().protocol_id())]
                }
                Stacked::Surface(id, ..) => vec![Seen::Script(*id)],
                Stacked::Panes(nodes) => nodes
                    .iter()
                    .map(|((pane, ..), _)| Seen::Pane(*pane))
                    .collect(),
            })
            .collect()
    };
    let bar_object = smithay::desktop::layer_map_for_output(&output)
        .layers()
        .find(|surface| surface.wl_surface().id().protocol_id() == id(&bar))
        .map(|surface| surface.wl_surface().id())
        .expect("the bar on the layer map");
    let monitor = output.name();
    let under_bar = beneath(all.clone(), &Part::LayerShell(bar_object), &monitor);
    assert_eq!(
        seen(&under_bar),
        vec![
            Seen::Pane(upper.pane),
            Seen::Pane(lower.pane),
            Seen::Layer(id(&widgets)),
            Seen::Script(wallpaper),
        ],
        "under the bar"
    );
    let under_upper = beneath(
        all,
        &Part::Pane(upper.pane, crate::effect::plan::PaneSlot::Client),
        &monitor,
    );
    assert_eq!(
        seen(&under_upper),
        vec![
            Seen::Pane(lower.pane),
            Seen::Layer(id(&widgets)),
            Seen::Script(wallpaper),
        ],
        "under the upper window"
    );
}

/// The script's surface below the windows a press at a point is
/// offered to.
fn claimed_below(desk: &Desk, x: f64, y: f64) -> Option<crate::scripted::SurfaceId> {
    desk.state
        .surface_claiming(false, (x, y).into(), crate::qml::hosted::Asking::Press)
        .map(|(_, id, _)| id)
}

/// **A window over a script's dock keeps the press, and so do
/// its menu reaching past it and a client's bar.** The dock was
/// offered every press below the windows whatever was drawn over
/// it: click-to-focus focused the window, and the press went to
/// the dock.
#[test]
fn a_window_over_a_scripted_dock_keeps_the_press() {
    let mut desk = Desk::new();
    let dock = scripted(&mut desk, "dock", Scripted::Bottom, screen());
    let (bar, _bar) = layer_surface(&mut desk, Client::Top, Some(200), 30, 0);
    let opened = desk.open_surface();
    commit_buffer(&desk.client, &desk.qh, &opened.surface, 400, 300);
    desk.pump();
    let window = window_of(&desk, opened.pane);
    desk.state
        .space
        .map_element(window.clone(), (100, 100), false);
    desk.state.space.refresh();
    landed(&mut desk);
    let menu = drawn_popup(
        &mut desk.display,
        &mut desk.state,
        &desk.conn,
        &desk.qh,
        &mut desk.queue,
        &mut desk.client,
        &opened.xdg,
        (390, 50),
        (120, 80),
    );
    let on_the_menu = Point::<f64, Logical>::from((560.0, 190.0));
    assert!(
        desk.state
            .real_geometry(&window)
            .is_some_and(|real| !real.to_f64().contains(on_the_menu))
            && delivered(&desk, on_the_menu.x, on_the_menu.y) == Some(id(&menu)),
        "the premise: the menu reaches past its window, and has the pointer there"
    );

    assert_eq!(
        (
            desk.state
                .window_under((300.0, 250.0).into())
                .map(|(under, _)| under),
            claimed_below(&desk, 300.0, 250.0),
            claimed_below(&desk, on_the_menu.x, on_the_menu.y),
            (
                delivered(&desk, 100.0, 15.0),
                claimed_below(&desk, 100.0, 15.0)
            ),
            claimed_below(&desk, 1000.0, 700.0),
        ),
        (Some(window), None, None, (Some(id(&bar)), None), Some(dock)),
        "(the window a press on it focuses, the dock offered that press, the dock \
         offered one on the menu, the surface the pointer reaches on the bar and \
         the dock offered a press there, the dock offered one where nothing is \
         over it)"
    );
}

/// **Within one layer, the order each kind already had**: the
/// client's surface mapped last is on top and has the pointer,
/// and the script's declared first is on top and has the press.
#[test]
fn within_a_layer_the_order_is_the_one_each_kind_had() {
    let mut desk = Desk::new();
    let (first, _first) = layer_surface(&mut desk, Client::Top, None, 30, 0);
    let (second, _second) = layer_surface(&mut desk, Client::Top, None, 30, 0);
    let below_the_bars = Rectangle::new((0, 100).into(), (1920, 30).into());
    let one = scripted(&mut desk, "one", Scripted::Top, below_the_bars);
    let two = scripted(&mut desk, "two", Scripted::Top, below_the_bars);
    assert!(
        drawn_over(&desk, Seen::Layer(id(&second)), Seen::Layer(id(&first)))
            && drawn_over(&desk, Seen::Script(one), Seen::Script(two)),
        "{:?}",
        drawn(&desk)
    );
    assert_eq!(
        (delivered(&desk, 100.0, 15.0), claimed(&desk, 100.0, 115.0)),
        (Some(id(&second)), Some(one))
    );
}

/// **A fullscreen window covers the bar, the client's and the
/// script's, and takes the press where the bar was.**
#[test]
fn a_fullscreen_window_covers_a_bar_and_takes_the_press_where_it_was() {
    let mut desk = Desk::new();
    let (bar, _bar) = layer_surface(&mut desk, Client::Top, None, 30, 30);
    let shell = scripted(&mut desk, "shell", Scripted::Top, strip(0, 1920));
    let (opened, window) = fullscreen(&mut desk, (10, 40));

    assert!(
        drawn_over(&desk, Seen::Pane(opened.pane), Seen::Layer(id(&bar)))
            && drawn_over(&desk, Seen::Pane(opened.pane), Seen::Script(shell)),
        "the fullscreen window over both bars: {:?}",
        drawn(&desk)
    );
    let at = Point::<f64, Logical>::from((100.0, 15.0));
    assert_eq!(
        (
            delivered(&desk, at.x, at.y),
            claimed(&desk, at.x, at.y),
            desk.state.window_under(at).map(|(under, _)| under),
        ),
        (Some(surface_id(&window)), None, Some(window.clone())),
        "(the surface the pointer reaches, the scripted bar being offered the \
         press, the window a press focuses)"
    );
}

/// **An overlay surface stays over a fullscreen window and keeps
/// its presses**, the client's and the script's -- and the bar,
/// mapped after the client's overlay, stays under both.
#[test]
fn an_overlay_stays_over_a_fullscreen_window_and_keeps_its_presses() {
    let mut desk = Desk::new();
    let (overlay, _overlay) = layer_surface(&mut desk, Client::Overlay, Some(200), 30, -1);
    let (bar, _bar) = layer_surface(&mut desk, Client::Top, None, 30, 30);
    let osd = scripted(&mut desk, "osd", Scripted::Overlay, strip(300, 200));
    let (opened, window) = fullscreen(&mut desk, (10, 40));

    let pane = Seen::Pane(opened.pane);
    assert!(
        drawn_over(&desk, Seen::Layer(id(&overlay)), pane)
            && drawn_over(&desk, Seen::Script(osd), pane)
            && drawn_over(&desk, pane, Seen::Layer(id(&bar))),
        "both overlays, the fullscreen window, the bar: {:?}",
        drawn(&desk)
    );
    assert_eq!(
        (
            delivered(&desk, 100.0, 15.0),
            delivered(&desk, 1000.0, 15.0),
            claimed(&desk, 400.0, 15.0),
            desk.state.claim_under((400.0, 15.0).into()),
        ),
        (
            Some(id(&overlay)),
            Some(surface_id(&window)),
            Some(osd),
            Claim::Surface
        ),
        "(on the client's overlay, on the bar beside it, on the script's \
         overlay, what a press there is)"
    );
}

/// **Leaving fullscreen puts the bar back on top.**
#[test]
fn leaving_fullscreen_puts_the_bar_back_on_top() {
    let mut desk = Desk::new();
    let (bar, _bar) = layer_surface(&mut desk, Client::Top, None, 30, 30);
    let (opened, window) = fullscreen(&mut desk, (10, 5));
    let pane = Seen::Pane(opened.pane);
    assert!(
        drawn_over(&desk, pane, Seen::Layer(id(&bar)))
            && delivered(&desk, 20.0, 15.0) == Some(surface_id(&window)),
        "the premise: fullscreen, the window covers the bar"
    );

    opened.toplevel.unset_fullscreen();
    desk.pump();
    commit_buffer(&desk.client, &desk.qh, &opened.surface, 64, 64);
    desk.pump();
    landed(&mut desk);
    assert_eq!(
        desk.state.real_geometry(&window),
        Some(Rectangle::new((10, 5).into(), (64, 64).into())),
        "the premise: back where it was, under the bar's strip"
    );
    assert!(
        drawn_over(&desk, Seen::Layer(id(&bar)), pane),
        "the bar is back over the window: {:?}",
        drawn(&desk)
    );
    assert_eq!(delivered(&desk, 20.0, 15.0), Some(id(&bar)));
}

/// **#49: a window going fullscreen is lifted over the bar as it
/// starts to grow, and one leaving goes back under the bar only
/// once it has finished shrinking** -- drawn and pressed alike.
/// Lifted, it covers the bar only where it is drawn: at the
/// start of growing that is the corner it was in.
#[test]
fn a_window_is_lifted_as_it_starts_to_grow_and_dropped_once_it_has_shrunk() {
    let mut desk = Desk::new();
    desk.install(r#"sol.on("fullscreen", function() sol.animate({ duration = 260 }) end)"#);
    let (strip, _strip) = layer_surface(&mut desk, Client::Top, None, 30, 30);
    let opened = desk.open_surface();
    let window = window_of(&desk, opened.pane);
    desk.state.space.map_element(window.clone(), (10, 5), false);
    desk.state.space.refresh();
    landed(&mut desk);
    let pane = Seen::Pane(opened.pane);
    let bar = Seen::Layer(id(&strip));
    assert!(
        drawn_over(&desk, bar, pane),
        "the premise: the bar over the window: {:?}",
        drawn(&desk)
    );

    opened.toplevel.set_fullscreen(None);
    desk.pump();
    assert!(
        drawn_over(&desk, pane, bar),
        "lifted over the bar as it starts to grow: {:?}",
        drawn(&desk)
    );
    assert_eq!(
        (delivered(&desk, 20.0, 15.0), delivered(&desk, 1500.0, 15.0)),
        (Some(surface_id(&window)), Some(id(&strip))),
        "(a press on the window over the bar, one on the bar where the \
         window is not drawn yet)"
    );
    commit_buffer(&desk.client, &desk.qh, &opened.surface, 1920, 1080);
    desk.pump();
    landed(&mut desk);

    opened.toplevel.unset_fullscreen();
    desk.pump();
    assert!(
        drawn_over(&desk, pane, bar),
        "still over the bar while it shrinks: {:?}",
        drawn(&desk)
    );
    assert_eq!(
        delivered(&desk, 1500.0, 15.0),
        Some(surface_id(&window)),
        "and the press where it still covers the bar is the window's"
    );
    commit_buffer(&desk.client, &desk.qh, &opened.surface, 64, 64);
    desk.pump();
    landed(&mut desk);
    assert!(
        drawn_over(&desk, bar, pane),
        "back under the bar once it has shrunk: {:?}",
        drawn(&desk)
    );
    assert_eq!(delivered(&desk, 20.0, 15.0), Some(id(&strip)));
}

/// **A fullscreen window on a workspace that is not shown does
/// not hide the bar.**
#[test]
fn a_fullscreen_window_on_a_workspace_not_shown_leaves_the_bar_on_top() {
    let mut desk = Desk::new();
    desk.install(
        "require(\"modes\")\n\
         require(\"workspaces\")\n\
         require(\"tiling\")\n\
         require(\"scrolling\")",
    );
    let (bar, _bar) = layer_surface(&mut desk, Client::Top, None, 30, 30);
    let (opened, window) = fullscreen(&mut desk, (10, 40));
    let says = |desk: &Desk, chunk: &str| {
        desk.state
            .scripts
            .as_ref()
            .map(|scripts| scripts.evaluate(chunk))
            .unwrap_or_default()
    };
    let showing = "return tostring(require(\"workspaces\").on(\"reflow-test\"))";
    let pane = Seen::Pane(opened.pane);
    assert!(
        says(&desk, showing) == "1"
            && drawn_over(&desk, pane, Seen::Layer(id(&bar)))
            && delivered(&desk, 100.0, 15.0) == Some(surface_id(&window)),
        "the premise: on the workspace in view, the window covers the bar"
    );

    assert!(desk.state.trigger("super+2"), "super+2 was not handled");
    landed(&mut desk);
    assert_eq!(
        (
            says(&desk, showing),
            says(
                &desk,
                &format!(
                    "return tostring(require(\"workspaces\").of[{}])",
                    opened.pane.get()
                )
            )
        ),
        ("2".to_owned(), "1".to_owned()),
        "the premise: workspace 2 in view, the window on 1"
    );
    assert!(
        drawn_over(&desk, Seen::Layer(id(&bar)), pane),
        "the bar is over the window on the workspace not shown: {:?}",
        drawn(&desk)
    );
    assert_eq!(delivered(&desk, 100.0, 15.0), Some(id(&bar)));
}

/// **`fullscreen.covers = "none"` keeps the bar over a
/// fullscreen window**, set the way `init.lua` sets it.
#[test]
fn with_fullscreen_covers_none_the_bar_stays_over_a_fullscreen_window() {
    let mut desk = Desk::new();
    let (bar, _bar) = layer_surface(&mut desk, Client::Top, None, 30, 30);
    let (opened, window) = fullscreen(&mut desk, (10, 40));
    let pane = Seen::Pane(opened.pane);
    assert!(
        drawn_over(&desk, pane, Seen::Layer(id(&bar)))
            && delivered(&desk, 100.0, 15.0) == Some(surface_id(&window)),
        "the premise: by default the window covers the bar"
    );

    desk.install(
        "local config = require(\"config\")\n\
         config.fullscreen.covers = \"none\"\n\
         sol.fullscreen(config.fullscreen)",
    );
    let scripts = desk.state.scripts.take();
    desk.state.start_scripts(scripts);
    landed(&mut desk);
    assert!(
        drawn_over(&desk, Seen::Layer(id(&bar)), pane),
        "the bar is over the fullscreen window: {:?}",
        drawn(&desk)
    );
    assert_eq!(delivered(&desk, 100.0, 15.0), Some(id(&bar)));
}

/// **A window closing in front of a fullscreen one fades out
/// over it, and the fullscreen window covers the bar only once
/// it is gone.** It was lifted the moment the client left, so the
/// fade played hidden behind it and the bar vanished with it.
#[test]
fn a_window_closing_in_front_of_a_fullscreen_one_fades_out_over_it() {
    let mut desk = Desk::new();
    let (bar_surface, _bar) = layer_surface(&mut desk, Client::Top, None, 30, 30);
    let (video, video_window) = fullscreen(&mut desk, (10, 40));
    let front = desk.open_surface();
    let front_window = window_of(&desk, front.pane);
    desk.state
        .space
        .map_element(front_window, (300, 300), false);
    desk.state.space.refresh();
    landed(&mut desk);
    let (bar, video, front_pane) = (
        Seen::Layer(id(&bar_surface)),
        Seen::Pane(video.pane),
        Seen::Pane(front.pane),
    );
    assert!(
        drawn_over(&desk, bar, front_pane) && drawn_over(&desk, front_pane, video),
        "the premise: a window in front of the fullscreen one, so nothing is \
         lifted: {:?}",
        drawn(&desk)
    );

    front.toplevel.destroy();
    desk.pump();
    let since = left_of(&desk.state, front.pane).since;
    assert!(
        drawn_over(&desk, bar, front_pane) && drawn_over(&desk, front_pane, video),
        "what is left of the window fades over the fullscreen one, under the bar, \
         as it stood: {:?}",
        drawn(&desk)
    );
    assert_eq!(
        delivered(&desk, 100.0, 15.0),
        Some(id(&bar_surface)),
        "and the bar keeps its clicks while it does"
    );

    desk.state
        .settle(since + crate::pane::LEAVING + Duration::from_millis(16));
    assert!(
        desk.state.panes.get(front.pane).is_none(),
        "the premise: the fade is over"
    );
    assert!(
        drawn_over(&desk, video, bar),
        "and the fullscreen window covers the bar: {:?}",
        drawn(&desk)
    );
    assert_eq!(
        delivered(&desk, 100.0, 15.0),
        Some(surface_id(&video_window))
    );
}

/// **Focus follows the pointer to what is on top**: over a
/// client's bar, the window under the bar is not what the
/// pointer is on, and does not take the keyboard.
#[test]
fn focus_follows_mouse_does_not_reach_through_a_bar() {
    let mut desk = Desk::new();
    let under = desk.open_surface();
    let under_window = window_of(&desk, under.pane);
    desk.state.space.map_element(under_window, (500, 10), false);
    let other = desk.open_surface();
    let other_window = window_of(&desk, other.pane);
    desk.state
        .space
        .map_element(other_window.clone(), (800, 300), false);
    desk.state.space.refresh();
    landed(&mut desk);
    let (_bar, _layered) = layer_surface(&mut desk, Client::Top, None, 30, 30);
    desk.state.focus_window(
        &other_window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    desk.state.profile.focus_follows_mouse = true;
    let on_the_bar = Point::<f64, Logical>::from((532.0, 15.0));
    assert!(
        desk.focused() == other.pane
            && desk.state.client_above(on_the_bar)
            && desk.state.window_under(on_the_bar).is_some(),
        "the premise: the other window has the keyboard, and the bar is over the \
         window under it"
    );

    crate::input::follow_pointer(&mut desk.state, on_the_bar, false);
    let over_the_bar = desk.focused();
    crate::input::follow_pointer(&mut desk.state, (532.0, 50.0).into(), false);
    assert_eq!(
        (over_the_bar, desk.focused()),
        (other.pane, under.pane),
        "(the window with the keyboard with the pointer on the bar, and on the \
         window below it)"
    );
}

/// Press the left button at the pointer's current location, and
/// release it: a plain click, through the real input path.
fn click(desk: &mut Desk, time: u64) {
    let region = crate::monitor::union(&desk.state.space).expect("a monitor");
    crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, time);
    crate::synth::send_button(
        &mut desk.state,
        region,
        0x110,
        ButtonState::Released,
        time + 1,
    );
}

/// **A click on empty desktop clears keyboard focus** (#219): the
/// window that held it before the click no longer does, and
/// `sol.windows()`'s `focused` -- which `desk.focused()` reads
/// the same way `focused_window` does -- agrees.
#[test]
fn a_click_on_empty_desktop_clears_keyboard_focus() {
    let mut desk = Desk::new();
    let opened = desk.open_surface();
    let window = window_of(&desk, opened.pane);
    desk.state
        .space
        .map_element(window.clone(), (100, 100), false);
    desk.state.space.refresh();
    landed(&mut desk);
    desk.state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    let empty = (1700.0, 950.0);
    assert!(
        desk.focused() == opened.pane && desk.state.window_under(empty.into()).is_none(),
        "the premise: the window has the keyboard, and {empty:?} is empty desktop"
    );

    move_pointer(&mut desk.state, empty, 10);
    click(&mut desk, 11);

    assert!(
        desk.state.focused_window().is_none(),
        "a click on empty desktop left a window focused"
    );
}

/// **A click on empty desktop with nothing already focused does
/// nothing**: `Solium::clear_focus` declines before it ever asks
/// `give_keyboard` for a serial nobody needed, so this is a click
/// with no window to lose and no window gained, on a desk that
/// never had one.
#[test]
fn a_click_on_empty_desktop_with_nothing_focused_does_nothing() {
    let mut desk = Desk::new();
    assert!(
        desk.state.focused_window().is_none(),
        "the premise: nothing has the keyboard"
    );

    move_pointer(&mut desk.state, (1700.0, 950.0), 10);
    click(&mut desk, 11);

    assert!(
        desk.state.focused_window().is_none(),
        "a click on empty desktop focused a window that was never there"
    );
}

/// **A window with `click_to_focus` off keeps whatever already
/// has the keyboard, including on a press over a *different*
/// window** -- the fallback that clears focus for an unclaimed
/// press (#219) must not fire just because this press was not
/// click-to-focus's to answer.
#[test]
fn a_click_on_a_window_with_click_to_focus_off_leaves_focus_alone() {
    let mut desk = Desk::new();
    let under = desk.open_surface();
    let under_window = window_of(&desk, under.pane);
    desk.state
        .space
        .map_element(under_window, (100, 100), false);
    let other = desk.open_surface();
    let other_window = window_of(&desk, other.pane);
    desk.state
        .space
        .map_element(other_window.clone(), (800, 300), false);
    desk.state.space.refresh();
    landed(&mut desk);
    desk.state.focus_window(
        &other_window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    desk.state.profile.click_to_focus = false;
    desk.state.profile.focus_follows_mouse = false;
    let on_under = (150.0, 150.0);
    assert!(
        desk.focused() == other.pane && desk.state.window_under(on_under.into()).is_some(),
        "the premise: the other window has the keyboard, and {on_under:?} is on \
         the first window"
    );

    move_pointer(&mut desk.state, on_under, 10);
    click(&mut desk, 11);

    assert_eq!(
        desk.focused(),
        other.pane,
        "a press that click-to-focus declined to answer cleared focus instead of \
         leaving it alone"
    );
}

/// **A click on a real client's bar is none of #219's business**:
/// the bar is not empty desktop, so the fallback that clears
/// focus for an unclaimed press must not reach past it, however
/// the bar's own client answers the press.
#[test]
fn the_bar_above_the_windows_is_left_alone() {
    let mut desk = Desk::new();
    let opened = desk.open_surface();
    let window = window_of(&desk, opened.pane);
    desk.state
        .space
        .map_element(window.clone(), (100, 100), false);
    desk.state.space.refresh();
    landed(&mut desk);
    let (_bar, _layered) = layer_surface(&mut desk, Client::Top, None, 30, 30);
    desk.state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    let on_the_bar = (960.0, 15.0);
    assert!(
        desk.focused() == opened.pane && desk.state.client_above(on_the_bar.into()),
        "the premise: the window has the keyboard, and the bar is over {on_the_bar:?}"
    );

    move_pointer(&mut desk.state, on_the_bar, 10);
    click(&mut desk, 11);

    assert_eq!(
        desk.focused(),
        opened.pane,
        "a click on the bar cleared the window's keyboard focus"
    );
}

/// **A click on a registered wallpaper surface that claims
/// nothing still clears keyboard focus** (#219): a wallpaper is
/// not empty space the way the plain empty-desktop test above
/// has it -- it is a real, interactive scripted surface at the
/// background layer, and `surface_claiming` walks it and calls
/// its `hit`, which here answers `Hit::Nothing` -- so this walks
/// a different path through `surface_pointer`/`surface_claiming`
/// than a point with no surface declared at all, and must land
/// on the same fallback.
#[test]
fn a_click_on_the_wallpaper_clears_keyboard_focus() {
    let mut desk = Desk::new();
    let opened = desk.open_surface();
    let window = window_of(&desk, opened.pane);
    desk.state
        .space
        .map_element(window.clone(), (100, 100), false);
    desk.state.space.refresh();
    landed(&mut desk);
    desk.state
        .declare_surface(crate::scripted::Declaration::for_test(
            "wallpaper",
            std::path::PathBuf::from("/nonexistent/wallpaper-test.qml"),
            Scripted::Background,
            crate::scripted::On::Rect(screen()),
        ));
    let wallpaper = desk
        .state
        .surfaces
        .named("wallpaper")
        .expect("the surface was declared");
    desk.state
        .surfaces
        .get_mut(wallpaper)
        .expect("live")
        .stand_in(crate::scripted::Stand {
            hit: |_| crate::qml::hosted::Hit::Nothing,
            ..crate::scripted::Stand::solid()
        });
    desk.state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    let empty = (1700.0, 950.0);
    assert!(
        desk.focused() == opened.pane
            && desk.state.window_under(empty.into()).is_none()
            && !desk.state.surface_pointer(false, empty.into(), None),
        "the premise: the window has the keyboard, {empty:?} is over the \
         wallpaper, and the wallpaper claims nothing there"
    );

    move_pointer(&mut desk.state, empty, 10);
    click(&mut desk, 11);

    assert!(
        desk.state.focused_window().is_none(),
        "a click on a non-claiming wallpaper left a window focused"
    );
}

/// **A fullscreen window's menu is drawn over the bar with it,
/// and takes the press there.** Its popups are drawn in its own
/// walk, which is the band it was lifted into.
#[test]
fn a_fullscreen_windows_menu_over_the_bar_takes_the_press() {
    let mut desk = Desk::new();
    let (bar, _bar) = layer_surface(&mut desk, Client::Top, None, 30, 30);
    let (video, _window) = fullscreen(&mut desk, (10, 40));
    let menu = drawn_popup(
        &mut desk.display,
        &mut desk.state,
        &desk.conn,
        &desk.qh,
        &mut desk.queue,
        &mut desk.client,
        &video.xdg,
        (100, 5),
        (120, 20),
    );
    assert!(
        drawn_over(&desk, Seen::Pane(video.pane), Seen::Layer(id(&bar))),
        "the fullscreen window and its menu over the bar: {:?}",
        drawn(&desk)
    );
    assert_eq!(delivered(&desk, 150.0, 15.0), Some(id(&menu)));
}

/// **On two monitors, a fullscreen window covers its own
/// monitor's bar and no other.** Lifted per monitor: the bar on
/// the left one stays over the left one's windows and keeps its
/// presses.
#[test]
fn on_two_monitors_a_fullscreen_window_covers_only_its_own_monitors_bar() {
    let mut desk = Desk::side_by_side();
    let right = Rectangle::new((1920, 0).into(), (1920, 1080).into());
    let bar = scripted(
        &mut desk,
        "shell",
        Scripted::Top,
        Rectangle::new((0, 0).into(), (3840, 30).into()),
    );
    let left = desk.open_surface();
    let left_window = window_of(&desk, left.pane);
    desk.state
        .space
        .map_element(left_window.clone(), (500, 10), false);
    desk.state.space.refresh();
    landed(&mut desk);
    let (video, window) = fullscreen_on(&mut desk, (1930, 40), right);

    let (bar_seen, video_seen, left_seen) = (
        Seen::Script(bar),
        Seen::Pane(video.pane),
        Seen::Pane(left.pane),
    );
    assert!(
        drawn_over_by(&desk, right, video_seen, bar_seen)
            && drawn_over_by(&desk, screen(), bar_seen, left_seen),
        "the right monitor: the fullscreen window over the bar {:?}; the left \
         monitor: the bar over the window {:?}",
        drawn_by(&desk, right),
        drawn_by(&desk, screen())
    );
    assert_eq!(
        (
            claimed(&desk, 2020.0, 15.0),
            delivered(&desk, 2020.0, 15.0),
            claimed(&desk, 532.0, 15.0),
            desk.state.claim_under((532.0, 15.0).into()),
        ),
        (None, Some(surface_id(&window)), Some(bar), Claim::Surface),
        "(on the right monitor's bar strip: the bar offered the press, the surface \
         the pointer reaches; on the left one's over its window: the bar offered \
         the press, what the press is)"
    );
}
