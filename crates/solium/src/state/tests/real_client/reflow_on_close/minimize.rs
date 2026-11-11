use super::*;
use crate::render::Stacked;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;

/// Writes down every `minimize`, `unminimize` and `focus`, in
/// order, as `"name id"` -- with a `_` when the window's own row
/// in that event's snapshot says `minimized`.
const HEARD: &str = r#"
events = {}
local function row(id)
for _, window in ipairs(sol.windows()) do
if window.id == id then return window end
end
end
for _, name in ipairs({ "minimize", "unminimize", "focus" }) do
sol.on(name, function(id)
local window = row(id)
local mark = (window == nil and "?") or (window.minimized and "_") or ""
events[#events + 1] = name .. " " .. id .. mark
end)
end
"#;

/// How long [`slow`]'s present on a minimized window plays.
const SLOW_MS: u64 = 5000;

/// A present on every window minimized, a fade to half over
/// [`SLOW_MS`]: a test can look at it part way through.
fn slow() -> String {
    format!(
        "sol.on(\"minimize\", function(id) \
         sol.animate({{ duration = {SLOW_MS} }}) \
         sol.present(id, {{ opacity = 0.5 }}) end)"
    )
}

/// [`HEARD`], `super+x` minimizing `pane` and `super+y` showing it
/// again, and `extra`.
fn keys(desk: &mut Desk, pane: crate::pane::PaneId, extra: &str) {
    desk.install(&format!(
        "{HEARD}\n\
         sol.bind(\"super+x\", function() sol.minimize({id}) end)\n\
         sol.bind(\"super+y\", function() sol.unminimize({id}) end)\n\
         {extra}",
        id = pane.get()
    ));
}

/// The panes the desk's monitor draws, topmost first, through
/// the walk the renderer draws with.
fn drawn(desk: &Desk) -> Vec<crate::pane::PaneId> {
    let screen = Rectangle::new((0, 0).into(), (1920, 1080).into());
    crate::render::stacked(&desk.state, screen, desk.state.clock.now())
        .into_iter()
        .flat_map(|stacked| match stacked {
            Stacked::Panes(nodes) => nodes.into_iter().map(|((pane, ..), _)| pane).collect(),
            _ => Vec::new(),
        })
        .collect()
}

/// What `pane`'s row in `sol.windows()` says of `minimized`.
fn minimized(desk: &Desk, pane: crate::pane::PaneId) -> Option<bool> {
    desk.state
        .snapshot()
        .windows
        .iter()
        .find(|row| row.id == pane.get())
        .map(|row| row.minimized)
}

/// `ms` later, with what landed retired.
fn after(desk: &mut Desk, ms: u64) {
    desk.state.clock.advance(Duration::from_millis(ms));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    desk.pump();
}

/// The pane the keyboard is on, if any.
fn focused(desk: &Desk) -> Option<crate::pane::PaneId> {
    desk.state
        .focused_window()
        .and_then(|window| desk.state.panes.id_of(&window))
}

/// `pane`'s client.
fn window_of(desk: &Desk, pane: crate::pane::PaneId) -> Window {
    desk.state
        .panes
        .get(pane)
        .and_then(Pane::client)
        .cloned()
        .expect("the pane has a client")
}

/// Give `pane` the keyboard, as a script would.
fn focus(desk: &mut Desk, pane: crate::pane::PaneId) {
    let window = window_of(desk, pane);
    desk.state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    desk.pump();
}

/// Two windows of 400 by 300 mapped at `at`, the second over
/// the first, every animation landed.
fn two_at(desk: &mut Desk, at: [(i32, i32); 2]) -> (Opened, Opened) {
    let first = desk.open_surface();
    let second = desk.open_surface();
    for (opened, at) in [(&first, at[0]), (&second, at[1])] {
        commit_buffer(&desk.client, &desk.qh, &opened.surface, 400, 300);
        desk.pump();
        let window = window_of(desk, opened.pane);
        desk.state.map_stacked(window, at, false);
    }
    land(desk);
    (first, second)
}

/// **`sol.minimize` minimizes a window**: its row says so in the
/// one `minimize` it fires, and so does the `Windows` model; the
/// client is told it is suspended, and it stays mapped and alive.
#[test]
fn sol_minimize_marks_the_row_fires_once_and_leaves_the_client_mapped() {
    let mut desk = Desk::new();
    let (window, _toplevel, pane) = desk.open();
    keys(&mut desk, pane, "");

    assert!(desk.state.trigger("super+x"), "super+x is bound");
    desk.pump();

    assert_eq!(desk.events(), format!("minimize {}_", pane.get()));
    assert_eq!(minimized(&desk, pane), Some(true));
    let row = crate::models::windows::rows(&desk.state)
        .into_iter()
        .find(|row| row.key == pane.get().to_string())
        .and_then(|row| row.values.get("minimized").cloned());
    assert_eq!(
        row,
        Some(crate::json::Json::Bool(true)),
        "the Windows model does not say the window is minimized"
    );
    assert!(
        in_state(&window, State::Suspended),
        "the client was not told it is suspended"
    );
    assert!(
        desk.state.space.elements().any(|each| *each == window)
            && desk.state.panes.get(pane).and_then(Pane::client) == Some(&window),
        "a minimized window was unmapped, or lost its client"
    );
}

/// **A minimized window is drawn while the present on it plays
/// out, and not once it has landed**, when `sol.windows()` says
/// it is drawn at no opacity.
#[test]
fn a_minimized_window_is_drawn_until_its_present_lands() {
    let mut desk = Desk::new();
    let (_, _, pane) = desk.open();
    land(&mut desk);
    keys(&mut desk, pane, &slow());

    assert!(desk.state.trigger("super+x"));
    after(&mut desk, SLOW_MS / 2);
    assert!(
        drawn(&desk).contains(&pane),
        "a minimized window was not drawn while its present played out"
    );

    after(&mut desk, SLOW_MS);
    assert!(
        !drawn(&desk).contains(&pane),
        "a minimized window is drawn after its present landed"
    );
    let outer = desk.state.pane_outer_of(pane).expect("it keeps its slot");
    assert!(
        !desk.state.drawn(pane, outer).shows(),
        "`sol.windows()` says a minimized window shows"
    );
}

/// **A press on a minimized window reaches what is under it**,
/// pointer, touch and `sol.window_at` alike -- even while the
/// present taking it away is still drawing it there.
#[test]
fn a_press_on_a_minimized_window_reaches_what_is_under_it() {
    let mut desk = Desk::new();
    let (lower, upper) = two_at(&mut desk, [(100, 100), (200, 200)]);
    let point: Point<f64, Logical> = (300.0, 300.0).into();
    let lower_window = window_of(&desk, lower.pane);
    let lower_surface = lower_window
        .wl_surface()
        .map(std::borrow::Cow::into_owned)
        .expect("its surface");
    assert_eq!(
        desk.state.window_under(point).map(|(window, _)| window),
        Some(window_of(&desk, upper.pane)),
        "the premise: the upper window is under the point"
    );
    keys(
        &mut desk,
        upper.pane,
        &format!(
            "{}\nsol.bind(\"super+w\", function() \
             sol.status(tostring(sol.window_at(300, 300))) end)",
            slow()
        ),
    );

    assert!(desk.state.trigger("super+x"));
    after(&mut desk, SLOW_MS / 2);
    assert!(
        drawn(&desk).contains(&upper.pane),
        "the premise: the present is still drawing the window"
    );
    assert_eq!(
        desk.state.window_under(point).map(|(window, _)| window),
        Some(lower_window),
        "the pointer stopped at a minimized window"
    );
    assert_eq!(
        desk.state.surface_under(point).map(|(surface, _)| surface),
        Some(lower_surface.clone()),
        "a press went to a minimized window"
    );
    assert_eq!(
        desk.state.touch_under(point).map(|(surface, _)| surface),
        Some(lower_surface),
        "a touch went to a minimized window"
    );
    assert!(desk.state.trigger("super+w"));
    assert_eq!(
        desk.state.status,
        lower.pane.get().to_string(),
        "`sol.window_at` named a minimized window"
    );
}

/// **The keyboard leaves a minimized window for the window still
/// on screen, and never goes back to it**: not as the window the
/// pointer is over, not as the topmost, not while its present
/// plays and not after.
#[test]
fn the_keyboard_leaves_a_minimized_window_for_the_next_and_never_comes_back_to_it() {
    let mut desk = Desk::new();
    let (first, second) = two_at(&mut desk, [(100, 100), (700, 100)]);
    desk.install(&format!(
        "{HEARD}\n{}\nsol.bind(\"super+x\", function() sol.minimize() end)",
        slow()
    ));
    focus(&mut desk, first.pane);
    point_at(&mut desk.state, (300.0, 250.0).into());
    assert_eq!(focused(&desk), Some(first.pane), "the premise");

    assert!(desk.state.trigger("super+x"));
    assert_eq!(
        focused(&desk),
        Some(second.pane),
        "the keyboard did not go on to the window still on screen"
    );

    assert!(desk.state.trigger("super+x"));
    assert_eq!(
        focused(&desk),
        None,
        "the keyboard went back to a minimized window"
    );
    desk.state.settle_focus();
    assert_eq!(focused(&desk), None, "a settle handed it one, mid-present");
    after(&mut desk, SLOW_MS * 2);
    desk.state.settle_focus();
    assert_eq!(focused(&desk), None, "a settle handed it one, landed");
}

/// **A client asking to be minimized is minimized**:
/// `xdg_toplevel.set_minimized`, as sol.minimize does it.
#[test]
fn a_client_asking_to_be_minimized_is_minimized() {
    let mut desk = Desk::new();
    let (window, toplevel, pane) = desk.open();
    desk.install(HEARD);

    toplevel.set_minimized();
    desk.pump();

    assert_eq!(minimized(&desk, pane), Some(true));
    assert_eq!(desk.events(), format!("minimize {}_", pane.get()));
    assert!(in_state(&window, State::Suspended));
}

/// **A frame's minimize button minimizes its window through the
/// configuration** (#417): a click on a layer that asks for
/// `"minimize"` puts the window away as `sol.minimize` does, so
/// the `minimize` listener hears it, with the window already
/// minimized in its snapshot, and plays it.
#[test]
fn a_frames_minimize_button_minimizes_its_window_through_the_configuration() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let style =
            std::env::temp_dir().join(format!("solium-frame-minimize-{}", std::process::id()));
        std::fs::create_dir_all(&style).expect("a style folder");
        std::fs::write(
            style.join("Pane.qml"),
            "import Solium\nPaneStyle {\n    insets.top: 32\n    \
             Layer { depth: \"frame\"; name: \"bar\"; source: \"Frame.qml\" }\n}\n",
        )
        .expect("writing the manifest");
        // The whole layer is the button.
        std::fs::write(
            style.join("Frame.qml"),
            "import QtQuick\nItem {\n    id: frame\n    \
             property string action: \"\"\n    \
             readonly property bool onButton: true\n    \
             MouseArea { anchors.fill: parent; onClicked: frame.action = \"minimize\" }\n}\n",
        )
        .expect("writing the layer");
        let mut desk = Desk::new();
        let opened = desk.open_surface();
        let pane = opened.pane;
        desk.install(HEARD);
        desk.state
            .decorations
            .set_style(&mut desk.state.panes, Some(style.display().to_string()));
        desk.state
            .decorations
            .insert(&mut desk.state.panes, pane, 64, 96);
        desk.state.clock.advance(Duration::from_secs(1));
        let now = desk.state.clock.now();
        desk.state.settle(now);
        let outer = desk.state.pane_outer_of(pane).expect("outer");
        let at = (f64::from(outer.loc.x) + 10.0, f64::from(outer.loc.y) + 16.0);
        assert!(
            matches!(
                desk.state.chrome_under(at.into()),
                Some(under) if matches!(under.chrome, crate::state::Chrome::Frame)
            ),
            "the premise: the point is on the frame"
        );
        move_pointer(&mut desk.state, at, 10);
        let region = crate::monitor::union(&desk.state.space).expect("a monitor");
        crate::synth::send_button(
            &mut desk.state,
            region,
            0x110,
            smithay::backend::input::ButtonState::Pressed,
            11,
        );
        crate::synth::send_button(
            &mut desk.state,
            region,
            0x110,
            smithay::backend::input::ButtonState::Released,
            12,
        );
        desk.pump();
        assert_eq!(minimized(&desk, pane), Some(true));
        assert!(
            desk.events()
                .ends_with(&format!("minimize {}_", pane.get())),
            "the configuration did not hear it: {}",
            desk.events()
        );
        drop(desk);
        let _ = std::fs::remove_dir_all(&style);
    });
}

/// **Focusing a minimized window shows it first**: `sol.focus`
/// and a window asking to be brought forward each fire
/// `unminimize` before `focus`, and the window has the keyboard.
#[test]
fn focusing_a_minimized_window_shows_it_first() {
    let mut desk = Desk::new();
    let opened = desk.open_surface();
    let pane = opened.pane;
    let window = window_of(&desk, pane);
    keys(
        &mut desk,
        pane,
        &format!(
            "sol.bind(\"super+f\", function() sol.focus({}) end)",
            pane.get()
        ),
    );
    let id = pane.get();

    assert!(desk.state.trigger("super+x"));
    assert_eq!(focused(&desk), None, "the premise: it gave the keyboard up");
    assert!(desk.state.trigger("super+f"));
    assert_eq!(
        desk.events(),
        format!("minimize {id}_,unminimize {id},focus {id}")
    );
    assert_eq!(minimized(&desk, pane), Some(false));
    assert!(
        !in_state(&window, State::Suspended),
        "the client was not told it is back"
    );
    assert_eq!(focused(&desk), Some(pane));

    assert!(desk.state.trigger("super+x"));
    let (token, _) = desk
        .state
        .activation_state
        .create_external_token(smithay::wayland::xdg_activation::XdgActivationTokenData::default());
    desk.client
        .activation
        .clone()
        .expect("xdg_activation_v1 bound")
        .activate(token.as_str().to_owned(), &opened.surface);
    desk.pump();
    assert_eq!(
        desk.events(),
        format!(
            "minimize {id}_,unminimize {id},focus {id},\
             minimize {id}_,unminimize {id},focus {id}"
        ),
        "an activation did not show the window before focusing it"
    );
    assert_eq!(focused(&desk), Some(pane));
}

/// **`sol.unminimize` shows the window again**: the row, the
/// client and the screen -- and, with nothing else holding it,
/// the keyboard settles on it.
#[test]
fn sol_unminimize_shows_the_window_again() {
    let mut desk = Desk::new();
    let (window, _, pane) = desk.open();
    land(&mut desk);
    keys(&mut desk, pane, &slow());
    assert!(desk.state.trigger("super+x"));
    after(&mut desk, SLOW_MS * 2);
    assert!(!drawn(&desk).contains(&pane), "the premise: put away");

    assert!(desk.state.trigger("super+y"));
    let id = pane.get();
    assert_eq!(
        desk.events(),
        format!("minimize {id}_,unminimize {id},focus {id}")
    );
    assert_eq!(minimized(&desk, pane), Some(false));
    assert!(!in_state(&window, State::Suspended));
    assert!(drawn(&desk).contains(&pane), "it is not drawn again");
}

/// **Minimizing a window twice, or showing one that is not
/// minimized, does nothing**, and tells nothing.
#[test]
fn minimizing_or_unminimizing_twice_does_nothing_the_second_time() {
    let mut desk = Desk::new();
    let (_, _, pane) = desk.open();
    keys(&mut desk, pane, "");

    assert!(desk.state.trigger("super+y"));
    assert!(desk.state.trigger("super+x"));
    assert!(desk.state.trigger("super+x"));
    assert_eq!(minimized(&desk, pane), Some(true));
    assert!(desk.state.trigger("super+y"));
    assert!(desk.state.trigger("super+y"));
    assert_eq!(minimized(&desk, pane), Some(false));
    let id = pane.get();
    assert_eq!(
        desk.events(),
        format!("minimize {id}_,unminimize {id},focus {id}")
    );
}

/// **A placement leaves a minimized window where it is**: its
/// slot, its tile and its picture, so a stray `sol.place` starts
/// no glide over the present taking it away.
#[test]
fn a_placement_leaves_a_minimized_window_where_it_is() {
    let mut desk = Desk::new();
    let (_, _, pane) = desk.open();
    land(&mut desk);
    keys(
        &mut desk,
        pane,
        &format!(
            "sol.bind(\"super+p\", function() \
             sol.place({}, {{ x = 0, y = 0, w = 50, h = 50 }}) end)",
            pane.get()
        ),
    );
    let slot = desk.state.pane_outer_of(pane);
    let tile = desk.state.panes.get(pane).and_then(Pane::placed);

    assert!(desk.state.trigger("super+x"));
    assert!(desk.state.trigger("super+p"));

    assert_eq!(desk.state.pane_outer_of(pane), slot, "it was moved");
    assert_eq!(
        desk.state.panes.get(pane).and_then(Pane::placed),
        tile,
        "its tile was moved"
    );
    assert!(
        !desk.state.panes.get(pane).is_some_and(present::transformed),
        "a glide was started on it"
    );
}

/// Whether `layout` holds window `pane` in its arrangement, as
/// the number of trees or strips that have it.
fn held_by(desk: &Desk, layout: &str, pane: crate::pane::PaneId) -> String {
    let (table, kind) = if layout == "tiling" {
        ("trees", "tree")
    } else {
        ("views", "view")
    };
    let chunk = format!(
        "local n = 0 \
         for _, {kind} in pairs(require({layout:?}).{table}) do \
         if {kind}:contains({id}) then n = n + 1 end end \
         return tostring(n)",
        id = pane.get()
    );
    desk.state
        .scripts
        .as_ref()
        .map(|scripts| scripts.evaluate_in(desk.state.snapshot(), &chunk))
        .unwrap_or_default()
}

/// **The shipped layouts leave a minimized window out, and take
/// it back where it was** when it is shown again: tiling into
/// the very tile, scrolling beside the same neighbour. An
/// `adopt` in between -- a reload's, a monitor's -- does not put
/// it back early.
#[test]
fn the_layouts_leave_a_minimized_window_out_and_take_it_back_where_it_was() {
    const BEFORE: &str = r#"
sol.on("minimize", function(id) last = id end)
sol.bind("super+x", function() sol.minimize() end)
sol.bind("super+y", function() sol.unminimize(last) end)
"#;
    for layout in LAYOUTS {
        let mut desk = Desk::new();
        let (away, stays) = two(&mut desk, layout, BEFORE);
        let before = (desk.placed(away), desk.placed(stays));

        assert!(desk.state.trigger("super+x"));
        land(&mut desk);
        assert_eq!(minimized(&desk, away), Some(true), "{layout}: the premise");
        assert_eq!(
            held_by(&desk, layout, away),
            "0",
            "{layout}: a minimized window is still in the arrangement"
        );
        if layout == "tiling" {
            assert!(
                desk.placed(stays).size.w > before.1.size.w,
                "tiling: the window beside it did not grow into its space"
            );
        }
        let adopt = format!("require({layout:?}).adopt() return \"adopted\"");
        let adopted = desk
            .state
            .scripts
            .as_ref()
            .map(|scripts| scripts.evaluate_in(desk.state.snapshot(), &adopt));
        assert_eq!(
            adopted.as_deref(),
            Some("adopted"),
            "{layout}: the premise: adopt ran"
        );
        assert_eq!(
            held_by(&desk, layout, away),
            "0",
            "{layout}: an adopt put a minimized window back"
        );

        assert!(desk.state.trigger("super+y"));
        land(&mut desk);
        assert_eq!(
            held_by(&desk, layout, away),
            "1",
            "{layout}: the window shown again is not in the arrangement"
        );
        let after = (desk.placed(away), desk.placed(stays));
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

/// **`super+m` minimizes the focused window through the shipped
/// configuration**, `minimize.lua`, which `init.lua` loads: the
/// genie plays, and the window is gone once it lands.
#[test]
fn super_m_minimizes_the_focused_window_through_the_shipped_configuration() {
    let mut desk = Desk::new();
    let (_, _, pane) = desk.open();
    land(&mut desk);
    focus(&mut desk, pane);
    desk.install(&format!("require(\"minimize\")\n{HEARD}"));
    assert!(
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/lua/init.lua"))
            .is_ok_and(|init| init.contains("require(\"minimize\")")),
        "init.lua does not load minimize.lua"
    );

    assert!(desk.state.trigger("super+m"), "super+m is bound");
    assert_eq!(minimized(&desk, pane), Some(true));
    assert_eq!(desk.events(), format!("minimize {}_", pane.get()));
    after(&mut desk, 300);
    assert!(drawn(&desk).contains(&pane), "the genie is not playing");
    after(&mut desk, 600);
    assert!(
        !drawn(&desk).contains(&pane),
        "it is drawn once the genie landed"
    );
}

/// **`sol.act("windows.minimize")` and `"windows.unminimize"`
/// do what `sol.minimize` and `sol.unminimize` do**, and `done`
/// hears that they were done.
#[test]
fn sol_act_minimizes_and_unminimizes_a_window() {
    let mut desk = Desk::new();
    let (_, _, pane) = desk.open();
    let id = pane.get();
    keys(
        &mut desk,
        pane,
        &format!(
            r#"answers = ""
local function told(ok, reason) answers = answers .. tostring(ok) .. ";"; sol.status(answers) end
sol.bind("super+a", function() sol.act("windows.minimize", {{ id = {id} }}, told) end)
sol.bind("super+b", function() sol.act("windows.unminimize", {{ id = {id} }}, told) end)"#
        ),
    );

    assert!(desk.state.trigger("super+a"));
    assert_eq!(minimized(&desk, pane), Some(true));
    assert!(desk.state.trigger("super+b"));
    assert_eq!(minimized(&desk, pane), Some(false));
    assert_eq!(desk.state.status, "true;true;");
    assert_eq!(
        desk.events(),
        format!("minimize {id}_,unminimize {id},focus {id}")
    );
}

/// **The overview shows a minimized window, and choosing it
/// shows it again** with the keyboard: drawn as a thumbnail
/// while the overview is up, and a click on the thumbnail
/// focuses it, which shows it.
#[test]
fn the_overview_shows_a_minimized_window_and_choosing_it_shows_it_again() {
    let mut desk = Desk::new();
    let (_, _, other) = desk.open();
    let (_, _, chosen) = desk.open();
    land(&mut desk);
    focus(&mut desk, chosen);
    desk.install(&format!(
        "require(\"minimize\")\nrequire(\"overview\")\n{HEARD}"
    ));
    assert!(desk.state.trigger("super+m"));
    after(&mut desk, 1000);
    assert!(!drawn(&desk).contains(&chosen), "the premise: put away");

    assert!(
        desk.state.trigger("super+space"),
        "the overview key is bound"
    );
    after(&mut desk, 1000);
    assert!(
        drawn(&desk).contains(&chosen) && drawn(&desk).contains(&other),
        "the overview does not show the minimized window beside the other"
    );
    let thumbnail = desk
        .state
        .panes
        .get(chosen)
        .and_then(present::target_of)
        .expect("the overview presents the minimized window")
        .rect;
    let centre = (
        thumbnail.loc.x + thumbnail.size.w / 2.0,
        thumbnail.loc.y + thumbnail.size.h / 2.0,
    );
    desk.state.trigger_click(centre.0, centre.1);
    desk.pump();

    assert_eq!(
        minimized(&desk, chosen),
        Some(false),
        "it is still minimized"
    );
    assert_eq!(
        focused(&desk),
        Some(chosen),
        "it does not have the keyboard"
    );
    after(&mut desk, 1000);
    assert!(
        drawn(&desk).contains(&chosen),
        "it is not drawn once the overview left"
    );
}

/// Window `pane` minimized through the shipped configuration,
/// with `before` run first, and what `minimize.lua` presented,
/// recorded by a `sol.present` that writes down its options.
fn presented_with(before: &str) -> (Desk, String) {
    let mut desk = Desk::new();
    let (_, _, pane) = desk.open();
    land(&mut desk);
    focus(&mut desk, pane);
    desk.install(&format!(
        r#"seen = nil
local present = sol.present
sol.present = function(id, options) seen = options; return present(id, options) end
{before}
require("minimize")"#
    ));
    assert!(desk.state.trigger("super+m"));
    let said = desk
        .state
        .scripts
        .as_ref()
        .map(|scripts| {
            scripts.evaluate(
                "local d = seen.deform \
                 return string.format(\"%g %g %g %g %g\", \
                 d.spread, d.to.x, d.to.y, d.to.w, d.to.h)",
            )
        })
        .unwrap_or_default();
    (desk, said)
}

/// **`config.minimize.motion` is how long the genie plays**:
/// over by 700 ms at the shipped 520, still playing then at 1000.
#[test]
fn config_minimize_motion_is_how_long_the_genie_plays() {
    for (set, playing) in [
        ("", false),
        (
            "require(\"config\").minimize.motion = { duration = 1000, easing = \"inOutCubic\" }",
            true,
        ),
    ] {
        let (mut desk, _) = presented_with(set);
        let pane = desk
            .state
            .snapshot()
            .windows
            .first()
            .map(|row| row.id)
            .and_then(|id| desk.state.panes.by_script_id(id).map(Pane::id))
            .expect("a window");
        after(&mut desk, 700);
        assert_eq!(
            drawn(&desk).contains(&pane),
            playing,
            "with {set:?}, the genie is {} playing at 700 ms",
            if playing { "not" } else { "still" }
        );
    }
}

/// **`config.minimize.spread` is how much of the window trails
/// behind it**, handed to the genie as its `spread`.
#[test]
fn config_minimize_spread_is_the_genies_spread() {
    let (_, shipped) = presented_with("");
    let (_, set) = presented_with("require(\"config\").minimize.spread = 2.5");
    assert!(shipped.starts_with("1.4 "), "{shipped:?}");
    assert!(set.starts_with("2.5 "), "{set:?}");
}

/// **`config.minimize.slot` is the strip the window goes into**,
/// centred on the bottom edge of the monitor's work area.
#[test]
fn config_minimize_slot_is_the_strip_the_window_goes_into() {
    let (_, shipped) = presented_with("");
    let (_, set) = presented_with("require(\"config\").minimize.slot = { w = 300, h = 40 }");
    assert_eq!(shipped, "1.4 900 1056 120 24");
    assert_eq!(set, "1.4 810 1040 300 40");
}

/// The screen-space pointer, as the seat holds it.
fn pointer_at(desk: &Desk) -> Point<f64, Logical> {
    desk.state
        .seat
        .get_pointer()
        .map(|pointer| pointer.current_location())
        .unwrap_or_default()
}

/// Press and let go of the left button where the pointer is, with
/// no motion before it.
fn click_where_it_is(desk: &mut Desk, time: u64) {
    let region = crate::monitor::union(&desk.state.space).expect("a monitor");
    for (button, at) in [
        (smithay::backend::input::ButtonState::Pressed, time),
        (smithay::backend::input::ButtonState::Released, time + 1),
    ] {
        crate::synth::send_button(&mut desk.state, region, 0x110, button, at);
    }
}

/// **A minimized window on a monitor that goes comes back on the
/// one left**: the rescue moves it as it moves every window, so
/// focusing it afterwards shows it there, drawn, with the
/// keyboard -- not on a monitor nobody has.
#[test]
fn a_minimized_window_on_a_monitor_that_goes_comes_back_on_the_one_left() {
    let mut desk = Desk::side_by_side();
    let opened = desk.open_surface();
    commit_buffer(&desk.client, &desk.qh, &opened.surface, 400, 300);
    desk.pump();
    let window = window_of(&desk, opened.pane);
    desk.state.map_stacked(window, (2400, 300), false);
    land(&mut desk);
    let pane = opened.pane;
    keys(
        &mut desk,
        pane,
        &format!(
            "sol.bind(\"super+f\", function() sol.focus({}) end)",
            pane.get()
        ),
    );
    assert!(desk.state.trigger("super+x"));
    let right = desk
        .state
        .space
        .outputs()
        .find(|output| output.name() == RIGHT_SCREEN)
        .cloned()
        .expect("the right monitor");
    desk.state.space.unmap_output(&right);
    desk.state.settle_monitors();
    desk.pump();

    let left = Rectangle::new((0, 0).into(), (1920, 1080).into());
    assert!(
        desk.state
            .pane_outer_of(pane)
            .is_some_and(|outer| left.contains_rect(outer)),
        "the minimized window was left on the monitor that went: {:?}",
        desk.state.pane_outer_of(pane)
    );
    assert!(desk.state.trigger("super+f"));
    land(&mut desk);
    assert_eq!(minimized(&desk, pane), Some(false));
    assert_eq!(focused(&desk), Some(pane));
    assert!(
        drawn(&desk).contains(&pane),
        "it is not drawn on the monitor left"
    );
}

/// **Showing a window that is on no monitor brings it onto one**
/// first, as the rescue would have.
#[test]
fn unminimizing_a_window_on_no_monitor_brings_it_onto_one() {
    let mut desk = Desk::new();
    let (window, _, pane) = desk.open();
    land(&mut desk);
    keys(&mut desk, pane, "");
    assert!(desk.state.trigger("super+x"));
    desk.state.map_stacked(window, (-5000, 300), false);
    desk.state.sync_panes();
    assert!(
        desk.state
            .pane_outer_of(pane)
            .is_some_and(|outer| !desk.state.on_any_output(outer)),
        "the premise: the window is on no monitor"
    );

    assert!(desk.state.trigger("super+y"));
    assert!(
        desk.state
            .pane_outer_of(pane)
            .is_some_and(|outer| desk.state.on_any_output(outer)),
        "a window shown again is on no monitor: {:?}",
        desk.state.pane_outer_of(pane)
    );
}

/// **A press with no motion after a client minimizes itself
/// reaches the window under the pointer**, and its client hears
/// it; the client that minimized hears nothing. Two clients, so
/// which one is told is what is asked.
#[test]
fn a_press_with_no_motion_after_a_minimize_reaches_the_window_under_it() {
    let mut desk = Desk::new();
    let lower = desk.open_surface();
    commit_buffer(&desk.client, &desk.qh, &lower.surface, 400, 300);
    desk.pump();
    let lower_window = window_of(&desk, lower.pane);
    desk.state
        .map_stacked(lower_window.clone(), (100, 100), false);
    let (conn, mut queue, mut other) = another_client(&mut desk);
    let qh = queue.handle();
    let (upper, toplevel, surface, _) =
        open_xdg(&mut desk.display, &mut desk.state, &conn, &other, &qh);
    commit_buffer(&other, &qh, &surface, 400, 300);
    let _mine = desk.client.seat_pointer(&desk.qh);
    let _theirs = other.seat_pointer(&qh);
    pump(
        &mut desk.display,
        &mut desk.state,
        &conn,
        &qh,
        &mut queue,
        &mut other,
    );
    desk.pump();
    desk.state.map_stacked(upper.clone(), (200, 200), false);
    land(&mut desk);
    move_pointer(&mut desk.state, (300.0, 300.0), 10);
    let focus = |desk: &Desk| {
        desk.state
            .seat
            .get_pointer()
            .and_then(|pointer| pointer.current_focus())
    };
    assert_eq!(
        focus(&desk),
        upper.wl_surface().map(std::borrow::Cow::into_owned),
        "the premise: the pointer is on the upper window"
    );

    toplevel.set_minimized();
    pump(
        &mut desk.display,
        &mut desk.state,
        &conn,
        &qh,
        &mut queue,
        &mut other,
    );
    click_where_it_is(&mut desk, 20);
    pump(
        &mut desk.display,
        &mut desk.state,
        &conn,
        &qh,
        &mut queue,
        &mut other,
    );
    desk.pump();

    assert_eq!(
        (
            focus(&desk),
            desk.client.buttons.clone(),
            other.buttons.clone()
        ),
        (
            lower_window.wl_surface().map(std::borrow::Cow::into_owned),
            vec![(0x110, true), (0x110, false)],
            Vec::new()
        ),
        "(the pointer's surface, what the window under it heard, what the \
         minimized window heard)"
    );
}

/// **A minimized window lets go of the pointer it locked**: a
/// game put away does not keep the cursor still.
#[test]
fn a_minimized_window_lets_go_of_the_pointer_it_locked() {
    use zwp_pointer_constraints_v1::Lifetime;
    let mut desk = Desk::new();
    let opened = desk.open_surface();
    commit_buffer(&desk.client, &desk.qh, &opened.surface, 400, 300);
    desk.pump();
    let window = window_of(&desk, opened.pane);
    desk.state.map_stacked(window, (100, 100), false);
    land(&mut desk);
    keys(&mut desk, opened.pane, "");
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
    move_pointer(&mut desk.state, (300.0, 300.0), 10);
    move_pointer(&mut desk.state, (301.0, 301.0), 11);
    move_pointer(&mut desk.state, (700.0, 700.0), 12);
    assert_eq!(
        pointer_at(&desk),
        Point::from((300.0, 300.0)),
        "the premise: the lock holds the pointer"
    );

    assert!(desk.state.trigger("super+x"));
    move_pointer(&mut desk.state, (700.0, 700.0), 13);
    assert_eq!(
        pointer_at(&desk),
        Point::from((700.0, 700.0)),
        "a minimized window kept the pointer locked"
    );
}

/// **A close a minimized window refuses leaves it put away**: it
/// is drawn at no point of the close -- not while it would fade,
/// not when it is asked, not when it is given back -- and the
/// present that put it away is still what it is held by.
#[test]
fn a_close_a_minimized_window_refuses_leaves_it_put_away() {
    let mut desk = Desk::new();
    let (_, _, pane) = desk.open();
    land(&mut desk);
    keys(&mut desk, pane, &slow());
    assert!(desk.state.trigger("super+x"));
    after(&mut desk, SLOW_MS * 2);
    let held = |desk: &Desk| {
        desk.state
            .panes
            .get(pane)
            .and_then(present::target_of)
            .map(|frame| (frame.rect, frame.opacity))
    };
    let away = held(&desk);
    assert!(away.is_some(), "the premise: the present holds it");

    desk.state.close_pane(pane);
    after(&mut desk, 50);
    assert!(!drawn(&desk).contains(&pane), "drawn while it is closed");
    desk.ask();
    assert!(
        !drawn(&desk).contains(&pane),
        "drawn when it is asked to go"
    );
    desk.refuse();
    assert!(!drawn(&desk).contains(&pane), "drawn when it is given back");
    after(&mut desk, 100);
    assert!(
        !drawn(&desk).contains(&pane),
        "drawn after it is given back"
    );
    assert_eq!(minimized(&desk, pane), Some(true));
    assert_eq!(
        held(&desk),
        away,
        "the present that put it away was replaced"
    );
}

/// **A close refused while minimized leaves the window out of
/// the layouts**, and `unminimize` takes it back where it was.
#[test]
fn a_close_refused_while_minimized_leaves_the_window_out_of_the_layouts() {
    const BEFORE: &str = r#"
sol.on("minimize", function(id) last = id end)
sol.bind("super+x", function() sol.minimize() end)
sol.bind("super+y", function() sol.unminimize(last) end)
"#;
    for layout in LAYOUTS {
        let mut desk = Desk::new();
        let (away, stays) = two(&mut desk, layout, BEFORE);
        let before = (desk.placed(away), desk.placed(stays));
        assert!(desk.state.trigger("super+x"));
        land(&mut desk);

        desk.state.close_pane(away);
        desk.ask();
        desk.refuse();
        land(&mut desk);
        assert_eq!(minimized(&desk, away), Some(true), "{layout}: the premise");
        assert_eq!(
            held_by(&desk, layout, away),
            "0",
            "{layout}: a refusal put a minimized window back in the arrangement"
        );

        assert!(desk.state.trigger("super+y"));
        land(&mut desk);
        assert_eq!(
            held_by(&desk, layout, away),
            "1",
            "{layout}: shown again, it is not in the arrangement"
        );
        if layout == "tiling" {
            assert_eq!(
                (desk.placed(away), desk.placed(stays)),
                before,
                "tiling: the window did not come back where it was"
            );
        }
    }
}

/// **A window shown again while the overview is up stays in its
/// grid**: a mode's picture is the mode's.
#[test]
fn a_window_shown_again_during_the_overview_stays_in_its_grid() {
    let mut desk = Desk::new();
    let (_, _, _other) = desk.open();
    let (_, _, pane) = desk.open();
    land(&mut desk);
    focus(&mut desk, pane);
    desk.install(&format!(
        "require(\"minimize\")\nrequire(\"overview\")\n{HEARD}\n\
         sol.bind(\"super+y\", function() sol.unminimize({}) end)",
        pane.get()
    ));
    assert!(desk.state.trigger("super+m"));
    after(&mut desk, 1000);
    assert!(desk.state.trigger("super+space"));
    after(&mut desk, 1000);
    let thumbnail = |desk: &Desk| {
        desk.state
            .panes
            .get(pane)
            .and_then(present::target_of)
            .map(|frame| frame.rect)
    };
    let grid = thumbnail(&desk);
    assert!(grid.is_some(), "the premise: the overview presents it");

    assert!(desk.state.trigger("super+y"));
    after(&mut desk, 1000);
    assert_eq!(minimized(&desk, pane), Some(false));
    assert_eq!(
        thumbnail(&desk),
        grid,
        "it was pulled out of the overview's grid"
    );
    assert!(drawn(&desk).contains(&pane));
}

/// **A window shown again with nothing focused takes the
/// keyboard**, by the compositor's own settle, rather than
/// leaving it on nothing.
#[test]
fn a_window_shown_again_with_nothing_focused_takes_the_keyboard() {
    let mut desk = Desk::new();
    let (_, _, pane) = desk.open();
    land(&mut desk);
    keys(&mut desk, pane, "");
    assert!(desk.state.trigger("super+x"));
    assert_eq!(
        focused(&desk),
        None,
        "the premise: nothing has the keyboard"
    );
    assert!(desk.state.trigger("super+y"));
    assert_eq!(focused(&desk), Some(pane));
}

/// **A modal dialog goes with its window**: minimizing the
/// window minimizes the dialog, showing it shows the dialog, and
/// minimizing the dialog -- by a script or by its own client --
/// minimizes the window, and showing the dialog shows both.
#[test]
fn a_modal_dialog_is_minimized_and_shown_with_its_window() {
    let mut desk = Desk::new();
    let (_, parent_toplevel, parent) = desk.open();
    let dialog_window = desk.open_dialog_for(&parent_toplevel);
    desk.state.sync_panes();
    desk.pump();
    let dialog = desk.state.panes.id_of(&dialog_window).expect("a pane");
    desk.install(&format!(
        "sol.bind(\"super+a\", function() sol.minimize({p}) end)\n\
         sol.bind(\"super+b\", function() sol.unminimize({p}) end)\n\
         sol.bind(\"super+c\", function() sol.minimize({d}) end)\n\
         sol.bind(\"super+e\", function() sol.unminimize({d}) end)",
        p = parent.get(),
        d = dialog.get()
    ));
    let both = |desk: &Desk| (minimized(desk, parent), minimized(desk, dialog));

    assert!(desk.state.trigger("super+a"));
    assert_eq!(
        both(&desk),
        (Some(true), Some(true)),
        "minimizing the window"
    );
    assert!(desk.state.trigger("super+b"));
    assert_eq!(
        both(&desk),
        (Some(false), Some(false)),
        "showing the window"
    );
    assert!(desk.state.trigger("super+c"));
    assert_eq!(
        both(&desk),
        (Some(true), Some(true)),
        "minimizing the dialog"
    );
    assert!(desk.state.trigger("super+e"));
    assert_eq!(
        both(&desk),
        (Some(false), Some(false)),
        "showing the dialog"
    );
    let toplevel = dialog_window.toplevel().cloned().expect("an xdg dialog");
    smithay::wayland::shell::xdg::XdgShellHandler::minimize_request(&mut desk.state, toplevel);
    assert_eq!(
        both(&desk),
        (Some(true), Some(true)),
        "the dialog's client asking"
    );
}

/// **The Developer Tweaks genie does nothing without
/// `minimize.lua`**, and does not load it: a configuration that
/// left it out has no `super+m`.
#[test]
fn the_tweaks_genie_does_not_load_minimize_lua() {
    let mut desk = Desk::new();
    let (_, _, pane) = desk.open();
    focus(&mut desk, pane);
    desk.install(
        "sol.bind(\"super+z\", function() require(\"tweaks\").handle(\"effect:genie\") end)",
    );
    assert!(desk.state.trigger("super+z"));
    assert!(
        !desk.state.trigger("super+m"),
        "the tweaks genie bound super+m by loading minimize.lua"
    );
}

/// **The overview sends the minimized windows it showed back on
/// leaving**, drawn while the genie takes each back and put away
/// once it lands, and no longer shown anyway.
#[test]
fn the_overview_sends_the_minimized_windows_it_showed_back_on_leaving() {
    let mut desk = Desk::new();
    let (_, _, _other) = desk.open();
    let (_, _, pane) = desk.open();
    land(&mut desk);
    focus(&mut desk, pane);
    desk.install("require(\"minimize\")\nrequire(\"overview\")");
    assert!(desk.state.trigger("super+m"));
    after(&mut desk, 1000);
    assert!(desk.state.trigger("super+space"));
    after(&mut desk, 1000);
    assert!(
        drawn(&desk).contains(&pane),
        "the premise: shown in the overview"
    );

    assert!(desk.state.trigger("escape"), "escape leaves the overview");
    after(&mut desk, 200);
    assert!(drawn(&desk).contains(&pane), "not drawn on its way back");
    assert!(
        !desk
            .state
            .panes
            .get(pane)
            .is_some_and(Pane::shown_minimized),
        "still shown anyway after the overview left"
    );
    after(&mut desk, 1000);
    assert_eq!(minimized(&desk, pane), Some(true));
    assert!(!drawn(&desk).contains(&pane), "drawn once it is back");
}

/// **A window put away is not drawn when the selection it is in
/// changes**: the displacement that keeps a window looking where
/// it was would be a transform playing, which draws it.
#[test]
fn a_put_away_window_is_not_drawn_when_its_selection_changes() {
    let mut desk = Desk::new();
    let (_, _, pane) = desk.open();
    let id = pane.get();
    keys(
        &mut desk,
        pane,
        &format!(
            "sol.bind(\"super+g\", function() \
                sol.group(\"desk\", {{ windows = {{ {id} }} }}) \
                sol.present_group(\"desk\", {{ x = -500 }}, {{ duration = 1 }}) end)\n\
             sol.bind(\"super+h\", function() \
                sol.animate({{ duration = 1000 }}) \
                sol.group(\"desk\", {{ windows = {{}} }}) end)"
        ),
    );
    assert!(desk.state.trigger("super+g"));
    land(&mut desk);
    assert!(desk.state.trigger("super+x"));
    land(&mut desk);

    assert!(
        desk.state.trigger("super+h"),
        "the premise: the desk was rebuilt"
    );
    after(&mut desk, 100);
    assert!(!drawn(&desk).contains(&pane), "a selection change drew it");
}

/// **`sol.show_minimized` draws a minimized window until told
/// not to**, and showing the window again forgets it: minimized
/// once more, it is put away.
#[test]
fn show_minimized_draws_a_window_until_told_not_to_and_is_forgotten_when_it_is_shown() {
    let mut desk = Desk::new();
    let (_, _, pane) = desk.open();
    land(&mut desk);
    keys(
        &mut desk,
        pane,
        &format!(
            "sol.bind(\"super+s\", function() sol.show_minimized({id}) end)\n\
             sol.bind(\"super+d\", function() sol.show_minimized({id}, false) end)",
            id = pane.get()
        ),
    );
    assert!(desk.state.trigger("super+x"));
    assert!(!drawn(&desk).contains(&pane), "the premise: put away");
    assert!(desk.state.trigger("super+s"));
    assert!(
        drawn(&desk).contains(&pane),
        "shown anyway, it is not drawn"
    );
    assert!(desk.state.trigger("super+d"));
    assert!(!drawn(&desk).contains(&pane), "told not to, it is drawn");
    assert!(desk.state.trigger("super+s"));
    assert!(desk.state.trigger("super+y"));
    assert!(
        !desk
            .state
            .panes
            .get(pane)
            .is_some_and(Pane::shown_minimized),
        "showing it again did not forget it"
    );
    assert!(desk.state.trigger("super+x"));
    assert!(
        !drawn(&desk).contains(&pane),
        "minimized again, it is drawn"
    );
}

/// **`workspaces.visible` leaves a minimized window out**, and
/// gives it with the rest when asked for minimized windows too,
/// as the overview asks.
#[test]
fn workspaces_visible_leaves_minimized_windows_out_unless_asked() {
    let mut desk = Desk::new();
    let (_, _, _other) = desk.open();
    let (_, _, pane) = desk.open();
    keys(&mut desk, pane, "require(\"workspaces\")");
    assert!(desk.state.trigger("super+x"));
    let said = desk.state.scripts.as_ref().map(|scripts| {
        scripts.evaluate_in(
            desk.state.snapshot(),
            "local w = require(\"workspaces\") \
             return #w.visible() .. \" \" .. #w.visible(nil, true)",
        )
    });
    assert_eq!(said.as_deref(), Some("1 2"));
}
