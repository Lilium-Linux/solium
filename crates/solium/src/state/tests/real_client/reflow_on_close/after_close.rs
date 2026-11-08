//! **#128, 2026-10-14: a close gives the space back when the
//! configuration says, and the keyboard moves on by itself.**
//!
//! Inside `reflow_on_close` for [`Desk`], the one fixture with the
//! shipped layouts and a real client. Each scene is started the way
//! the compositor starts a configuration, `start_scripts`, so what the
//! shipped `modes.lua` hands over at load -- floating's focus model --
//! is in force before the layout key is pressed, as it is in a
//! session. Keys are pressed through the filter the backends feed,
//! with `us,ru` and Russian active (#132).

use super::*;

use smithay::backend::input::{ButtonState, KeyState};
use smithay::input::keyboard::Keycode;

/// `init.lua`'s `super+q`: close the focused window.
const CLOSE_KEY: &str = r#"
sol.bind("super+q", function()
for _, window in ipairs(sol.windows()) do
if window.focused then
sol.close(window.id)
return
end
end
end)
"#;

/// Super and q, as xkb keycodes: evdev 125 and 16, plus eight.
const SUPER: u32 = 133;
const Q: u32 = 24;

/// The shipped `layout`, started over the windows already open
/// as a session starts its configuration, with `before` ahead of
/// it and [`RECORDER`] after: the scripts' own top level applied
/// (`start_scripts`), then the layout's key, then the arrangement
/// let land. `"floating"` presses nothing.
fn start(desk: &mut Desk, layout: &str, before: &str) {
    let required = if layout == "floating" {
        String::new()
    } else {
        format!("require({layout:?})")
    };
    desk.install(&format!(
        "{before}\n{CLOSE_KEY}\nrequire(\"modes\")\n{required}\n{RECORDER}"
    ));
    let scripts = desk.state.scripts.take();
    desk.state.start_scripts(scripts);
    if layout != "floating" {
        assert!(
            desk.state.trigger(key_of(layout)),
            "{layout}: the layout key was not handled"
        );
    }
    desk.state.clock.advance(Duration::from_secs(1));
    let now = desk.state.clock.now();
    desk.state.settle(now);
    desk.pump();
}

fn key_of(layout: &str) -> &'static str {
    if layout == "tiling" {
        "super+t"
    } else {
        "super+s"
    }
}

/// Press `keys` in order and let them go in reverse, through the
/// key filter, as a keyboard does.
fn press(desk: &mut Desk, keys: &[u32]) {
    for &code in keys {
        crate::input::key(&mut desk.state, Keycode::new(code), KeyState::Pressed, 0);
    }
    for &code in keys.iter().rev() {
        crate::input::key(&mut desk.state, Keycode::new(code), KeyState::Released, 0);
    }
    desk.pump();
}

/// `super+q` on the focused window, with Russian active.
fn super_q(desk: &mut Desk) {
    press(desk, &[SUPER, Q]);
}

fn window_of(desk: &Desk, pane: crate::pane::PaneId) -> Window {
    desk.state
        .panes
        .get(pane)
        .and_then(Pane::client)
        .cloned()
        .expect("the pane has its client")
}

fn give_keyboard(desk: &mut Desk, pane: crate::pane::PaneId) {
    let window = window_of(desk, pane);
    desk.state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    assert_eq!(
        desk.focused(),
        pane,
        "the premise: the window has the keyboard"
    );
}

fn centre(rect: Rectangle<i32, Logical>) -> (f64, f64) {
    (
        f64::from(rect.loc.x) + f64::from(rect.size.w) / 2.0,
        f64::from(rect.loc.y) + f64::from(rect.size.h) / 2.0,
    )
}

/// Three tiles, and which is which: `(whole, closed, grows)` --
/// the window with the right of the screen to itself, and the two
/// that share the left, one of which is closed and the other of
/// which grows into its space when it is.
///
/// Opened one at a time with tiling on and the pointer at the
/// left, so each new window splits the tile there, as it does
/// in use. That puts the whole tile last in the layout's sweep,
/// so it is the topmost of the three once the layout has placed
/// them again -- which is where the keyboard went before #128.
fn three_tiles(
    desk: &mut Desk,
    before: &str,
) -> (
    crate::pane::PaneId,
    crate::pane::PaneId,
    crate::pane::PaneId,
) {
    let first = desk.open().2;
    start(desk, "tiling", before);
    point_at(&mut desk.state, (200.0, 540.0).into());
    let mut panes = vec![first];
    for _ in 0..2 {
        panes.push(desk.open().2);
        desk.state.clock.advance(Duration::from_secs(1));
        let now = desk.state.clock.now();
        desk.state.settle(now);
        desk.pump();
    }
    let whole = panes
        .iter()
        .copied()
        .max_by_key(|pane| desk.placed(*pane).loc.x)
        .expect("three windows");
    let pair: Vec<crate::pane::PaneId> = panes
        .iter()
        .copied()
        .filter(|pane| *pane != whole)
        .collect();
    let (one, other, right) = (
        desk.placed(pair[0]),
        desk.placed(pair[1]),
        desk.placed(whole),
    );
    assert!(
        one.loc.x == other.loc.x
            && one.size.w == other.size.w
            && one.loc.x + one.size.w < right.loc.x
            && right.size.h > one.size.h,
        "the premise: two tiles share the left of the screen, {one:?} and \
         {other:?}, and one has the right, {right:?}"
    );
    (whole, pair[0], pair[1])
}

/// `config.focus.after_close.tiling = list`, written so that a
/// configuration that has no `after_close` -- the compositor
/// before #128 -- still loads, and ignores it.
fn set_after_close(list: &str) -> String {
    format!(
        "local focus = require(\"config\").focus\n\
         focus.after_close = focus.after_close or {{}}\n\
         focus.after_close.tiling = {list}"
    )
}

/// **`reflow_on_close` decides when the others close up**, in
/// each layout: at the press (`"immediate"`, the default), when
/// the close lands (`"when_landed"`), or only when the
/// application has gone (`"when_gone"`). `close_landed` is
/// heard once, as the close lands, whatever the setting.
///
/// Against `0ce2ab8` it fails at the first `close_landed`: nothing
/// told a layout the close had landed, and `"when_landed"` was
/// read as `"immediate"`.
#[test]
fn with_reflow_on_close_when_landed_the_grid_waits_for_the_close_to_land() {
    for layout in LAYOUTS {
        for (setting, at_press, at_landing) in [
            ("immediate", true, true),
            ("when_landed", false, true),
            ("when_gone", false, false),
        ] {
            let mut desk = Desk::new();
            let (dying, survivor) = two(
                &mut desk,
                layout,
                &format!(
                    "require(\"config\").{layout}.reflow_on_close = {setting:?}\n\
                     heard_landed = {{}}\n\
                     sol.on(\"close_landed\", function(id) \
                     heard_landed[#heard_landed + 1] = id end)"
                ),
            );
            let was = desk.placed(survivor);
            desk.state.close_pane(dying);
            desk.pump();
            assert_eq!(
                desk.placed(survivor) != was,
                at_press,
                "{layout}, {setting}: at the press the survivor is at {:?}, and was \
                 at {was:?}",
                desk.placed(survivor)
            );
            desk.ask();
            assert_eq!(
                desk.placed(survivor) != was,
                at_landing,
                "{layout}, {setting}: once the close landed the survivor is at \
                 {:?}, and was at {was:?}",
                desk.placed(survivor)
            );
            let heard = desk
                .state
                .scripts
                .as_ref()
                .map(|scripts| scripts.evaluate("return table.concat(heard_landed, \",\")"))
                .unwrap_or_default();
            assert_eq!(
                heard,
                dying.get().to_string(),
                "{layout}, {setting}: `close_landed` was not heard once, for the \
                 window closed"
            );
        }
    }
}

/// **A window that asks inside itself gets its tile back** (#231),
/// whether the others closed up at the press or when the close
/// landed: the application answers the close by drawing its
/// question in the same window -- kitty with a program running
/// -- and stays, and once the grace runs out the window is given
/// back to the tile it left, beside the same neighbour.
///
/// Against `0ce2ab8` it fails at `when_landed`'s press, for the
/// reason the test above does.
#[test]
fn a_window_that_asks_inside_itself_gets_its_tile_back() {
    for layout in LAYOUTS {
        for setting in ["immediate", "when_landed"] {
            let mut desk = Desk::new();
            let first = desk.open_surface();
            let second = desk.open_surface();
            start(
                &mut desk,
                layout,
                &format!("require(\"config\").{layout}.reflow_on_close = {setting:?}"),
            );
            let (asks, beside) = if desk.focused() == first.pane {
                (&first, &second)
            } else {
                (&second, &first)
            };
            let before = (desk.placed(asks.pane), desk.placed(beside.pane));
            russian(&mut desk.state);

            super_q(&mut desk);
            assert_eq!(
                desk.placed(beside.pane) != before.1,
                setting == "immediate",
                "{layout}, {setting}: at the press the neighbour is at {:?}",
                desk.placed(beside.pane)
            );
            desk.ask();
            assert_ne!(
                desk.placed(beside.pane),
                before.1,
                "{layout}, {setting}: the neighbour never took the space"
            );
            // Its question, drawn in the window it was asked to
            // close, and no sign of going.
            desk.answer(asks);
            desk.refuse();

            let id = asks.pane.get();
            assert!(
                desk.events().ends_with(&format!("refused {id}")),
                "{layout}, {setting}: the refusal was not told: {}",
                desk.events()
            );
            let after = (desk.placed(asks.pane), desk.placed(beside.pane));
            match layout {
                "tiling" => assert_eq!(
                    after, before,
                    "tiling, {setting}: the window did not get its tile back"
                ),
                _ => {
                    let (back, other) = after;
                    assert_eq!(
                        before.0.loc.x < before.1.loc.x,
                        back.loc.x < other.loc.x,
                        "scrolling, {setting}: the window came back on the other \
                         side of its neighbour"
                    );
                    let (left, right) = if back.loc.x < other.loc.x {
                        (back, other)
                    } else {
                        (other, back)
                    };
                    assert_eq!(
                        left.loc.x + left.size.w + 12,
                        right.loc.x,
                        "scrolling, {setting}: the two are not neighbours: \
                         {left:?}, {right:?}"
                    );
                }
            }
        }
    }
}

/// **In tiling, the window that takes the closed one's space
/// takes the keyboard** as the close lands, and between two that
/// take it equally, the one used last -- not whichever the
/// layout's sweep happened to stack on top, which is where the
/// compositor gave it before #128.
///
/// Four tiles, opened with the pointer at the top left so each
/// new window splits the tile there: one of them has two side by
/// side over it, and when it is closed with `super+q`, Russian
/// active, both grow into its space. Run twice, each of the two
/// used last once; the keyboard follows that.
/// Against `0ce2ab8` it fails in one of the two runs: the
/// keyboard went to the same window both times.
#[test]
fn in_tiling_the_window_that_takes_the_space_takes_the_keyboard() {
    for used_last in [0, 1] {
        let mut desk = Desk::new();
        let mut panes = vec![desk.open().2];
        start(&mut desk, "tiling", "");
        for at in [(200.0, 540.0), (200.0, 300.0), (200.0, 300.0)] {
            point_at(&mut desk.state, at.into());
            panes.push(desk.open().2);
            desk.state.clock.advance(Duration::from_secs(1));
            let now = desk.state.clock.now();
            desk.state.settle(now);
            desk.pump();
        }
        panes.truncate(4);
        // The window with two side by side across the whole of
        // one of its sides: its space is theirs when it goes.
        let pair_over = |closed: crate::pane::PaneId| {
            let stood = desk.placed(closed);
            let others: Vec<crate::pane::PaneId> = panes
                .iter()
                .copied()
                .filter(|pane| *pane != closed)
                .collect();
            others.iter().copied().find_map(|one| {
                others
                    .iter()
                    .copied()
                    .find(|other| {
                        let (a, b) = (desk.placed(one), desk.placed(*other));
                        a.loc.y == b.loc.y
                            && a.size.h == b.size.h
                            && a.loc.x == stood.loc.x
                            && b.loc.x + b.size.w == stood.loc.x + stood.size.w
                            && a.loc.x < b.loc.x
                    })
                    .map(|other| (one, other))
            })
        };
        let (closed, takers) = panes
            .iter()
            .copied()
            .find_map(|closed| pair_over(closed).map(|pair| (closed, pair)))
            .unwrap_or_else(|| {
                let tiles: Vec<_> = panes.iter().map(|pane| desk.placed(*pane)).collect();
                panic!("the premise: a tile with two side by side over it: {tiles:?}")
            });
        let takers = [takers.0, takers.1];
        russian(&mut desk.state);
        point_at(&mut desk.state, (1.0, 1.0).into());
        give_keyboard(&mut desk, takers[used_last]);
        give_keyboard(&mut desk, closed);
        let stood = desk.placed(closed);

        super_q(&mut desk);
        assert!(
            desk.events()
                .ends_with(&format!("closing {}*", closed.get())),
            "the premise: super+q closed the focused window: {}",
            desk.events()
        );
        assert!(
            takers.iter().all(|pane| desk.placed(*pane).overlaps(stood)),
            "the premise: both grew into where the closed window stood, {stood:?}"
        );
        assert_eq!(
            desk.focused(),
            closed,
            "the keyboard left the closing window before its close landed"
        );
        let history = desk.state.focus_history.clone();
        desk.ask();
        assert_eq!(
            desk.focused(),
            takers[used_last],
            "the keyboard did not go to the window that took the space and was \
             used last: {takers:?}, the one used last {used_last}, closed \
             {closed:?}, focused before {history:?}, placed {:?}",
            takers.map(|pane| desk.placed(pane))
        );
    }
}

/// **In scrolling, the column that takes the closed one's place
/// takes the keyboard**: three columns, the middle one closed,
/// and the one to its right slides into its place. Not the
/// column on the left, used before it and the topmost.
/// Against `0ce2ab8` it fails at the last assertion.
#[test]
fn in_scrolling_the_column_that_takes_the_place_takes_the_keyboard() {
    let mut desk = Desk::new();
    let mut panes = [desk.open().2, desk.open().2, desk.open().2];
    start(&mut desk, "scrolling", "");
    russian(&mut desk.state);
    point_at(&mut desk.state, (1.0, 1.0).into());
    panes.sort_by_key(|pane| desk.placed(*pane).loc.x);
    let [left, middle, right] = panes;
    give_keyboard(&mut desk, left);
    give_keyboard(&mut desk, middle);
    let stood = desk.placed(middle);

    super_q(&mut desk);
    assert!(
        desk.placed(right).overlaps(stood) && !desk.placed(left).overlaps(stood),
        "the premise: the right-hand column slid into the closed one's place, \
         {stood:?}: it is at {:?} and the left one at {:?}",
        desk.placed(right),
        desk.placed(left)
    );
    desk.ask();
    assert_eq!(
        desk.focused(),
        right,
        "the keyboard did not go to the column that took the place"
    );
}

/// **In floating, the window used before the closed one on its
/// monitor takes the keyboard** -- not one on the other monitor,
/// though that was used more recently and is the topmost.
/// Against `0ce2ab8` it fails at the last assertion: the
/// keyboard went across to the other monitor.
#[test]
fn in_floating_the_window_focused_before_it_on_that_monitor_takes_the_keyboard() {
    let mut desk = Desk::side_by_side();
    let before = desk.open().2;
    let across = desk.open().2;
    let closed = desk.open().2;
    start(&mut desk, "floating", "");
    russian(&mut desk.state);
    for (pane, at) in [
        (before, (100, 100)),
        (across, (2020, 100)),
        (closed, (400, 300)),
    ] {
        let window = window_of(&desk, pane);
        desk.state.map_stacked(window, at, false);
    }
    desk.state.space.refresh();
    desk.state.sync_panes();
    point_at(&mut desk.state, (1500.0, 900.0).into());
    give_keyboard(&mut desk, before);
    give_keyboard(&mut desk, across);
    give_keyboard(&mut desk, closed);

    super_q(&mut desk);
    desk.ask();
    assert_eq!(
        desk.focused(),
        before,
        "the keyboard did not go to the window used before on the closed one's \
         monitor"
    );
}

/// **`"pointer"` gives the keyboard to the window under the
/// pointer, and the list decides whether it comes first**: with
/// the shipped tiling list the window under the pointer takes it;
/// with `{ "neighbour", "topmost" }` the window that took the
/// space does, wherever the pointer is.
/// Against `0ce2ab8` it fails at the second list: the
/// configuration was not read and the pointer always won.
#[test]
fn with_after_close_pointer_the_window_under_the_pointer_takes_the_keyboard() {
    for (list, takes) in [
        ("nil", "under the pointer"),
        ("{ \"neighbour\", \"topmost\" }", "grew"),
    ] {
        let mut desk = Desk::new();
        let before = if list == "nil" {
            String::new()
        } else {
            set_after_close(list)
        };
        let (whole, closed, grows) = three_tiles(&mut desk, &before);
        russian(&mut desk.state);
        give_keyboard(&mut desk, closed);
        let (x, y) = centre(
            desk.state
                .pane_outer_of(whole)
                .expect("a mapped pane has a rectangle"),
        );
        point_at(&mut desk.state, (x, y).into());

        super_q(&mut desk);
        desk.ask();
        let expected = if takes == "grew" { grows } else { whole };
        assert_eq!(
            desk.focused(),
            expected,
            "after_close.tiling = {list}: the keyboard did not go to the window \
             {takes}"
        );
    }
}

/// **`"recent"` hands the keyboard back to the window used before
/// the closed one, and an empty list leaves it on nothing** --
/// changing the key changes where it goes. The window used
/// before is not the topmost here: the one that grows is raised
/// over it without being focused.
/// Against `0ce2ab8` it fails at `"recent"`: the topmost took it.
#[test]
fn with_after_close_recent_tiling_hands_the_keyboard_to_the_window_used_before() {
    for list in ["{ \"recent\" }", "{}"] {
        let mut desk = Desk::new();
        let (whole, closed, grows) = three_tiles(&mut desk, &set_after_close(list));
        russian(&mut desk.state);
        point_at(&mut desk.state, (1.0, 1.0).into());
        give_keyboard(&mut desk, whole);
        give_keyboard(&mut desk, closed);
        let raised = window_of(&desk, grows);
        desk.state.space.raise_element(&raised, false);
        desk.state.sync_panes();

        super_q(&mut desk);
        desk.ask();
        let now = desk
            .state
            .focused_window()
            .and_then(|window| desk.state.panes.id_of(&window));
        let expected = if list == "{}" { None } else { Some(whole) };
        assert_eq!(
            now, expected,
            "after_close.tiling = {list}: the keyboard is on {now:?}"
        );
    }
}

/// **A window that closes itself while it has the keyboard hands
/// it to the window that takes its place**, as one the
/// compositor closes does -- `exit` in a terminal is the
/// ordinary way to close one.
/// Against `0ce2ab8` it fails at the last assertion: the topmost
/// took it.
#[test]
fn a_window_that_closes_itself_hands_the_keyboard_to_the_window_that_takes_its_place() {
    let mut desk = Desk::new();
    let opened: Vec<(Window, xdg_toplevel::XdgToplevel, crate::pane::PaneId)> =
        (0..3).map(|_| desk.open()).collect();
    start(&mut desk, "tiling", "");
    let area = |pane| {
        let placed = desk.placed(pane);
        i64::from(placed.size.w) * i64::from(placed.size.h)
    };
    let whole = opened
        .iter()
        .map(|each| each.2)
        .max_by_key(|pane| area(*pane))
        .expect("three windows");
    let mut pair = opened.iter().filter(|each| each.2 != whole);
    let (closing, grows) = match (pair.next(), pair.next()) {
        (Some(closing), Some(grows)) => (closing, grows.2),
        _ => panic!("the premise: three windows"),
    };
    point_at(&mut desk.state, (1.0, 1.0).into());
    give_keyboard(&mut desk, whole);
    give_keyboard(&mut desk, closing.2);

    closing.1.destroy();
    desk.pump();
    desk.state.space.refresh();
    desk.state.sync_panes();

    assert!(
        desk.events()
            .ends_with(&format!("close {}*", closing.2.get())),
        "the premise: the window went on its own: {}",
        desk.events()
    );
    assert_eq!(
        desk.focused(),
        grows,
        "the keyboard did not go to the window that took the space"
    );
}

/// **A close hands the keyboard on as `"settle"`, and the pointer
/// stays where it is** (#128 with #220). The window that takes
/// the keyboard when a close lands is chosen by
/// `config.focus.after_close` and given it by the compositor's
/// own settle, so `focus` is told `source = "settle"` and
/// `changed = true`, and `lua/follow.lua` takes the pointer there
/// only if `focus.cursor.follows.settle` says so: `false` as
/// shipped, so a close never moves the pointer out from under the
/// hand. With it `true` the pointer goes to the window that took
/// the space, which shows it is the shipped `false` that keeps it
/// still.
/// With `settle_focus` telling `"binding"` instead, it fails at
/// the source, and the pointer flies.
#[test]
fn a_close_hands_the_keyboard_on_as_settle_and_leaves_the_pointer_alone() {
    for follows in [false, true] {
        let mut desk = Desk::new();
        let before = format!(
            "require(\"follow\")\n\
             require(\"config\").focus.cursor.follows.settle = {follows}\n\
             heard = {{}}\n\
             sol.on(\"focus\", function(id, source, changed)\n\
                 heard[#heard + 1] = id .. \" \" .. source .. \" \" .. tostring(changed)\n\
             end)"
        );
        let (_, closed, grows) = three_tiles(&mut desk, &before);
        russian(&mut desk.state);
        point_at(&mut desk.state, (1.0, 1.0).into());
        give_keyboard(&mut desk, closed);
        let evaluate = |desk: &Desk, lua: &str| {
            desk.state
                .scripts
                .as_ref()
                .map(|scripts| scripts.evaluate(lua))
                .unwrap_or_default()
        };
        let _ = evaluate(&desk, "heard = {} return \"\"");

        super_q(&mut desk);
        desk.ask();
        assert_eq!(
            desk.focused(),
            grows,
            "follows.settle = {follows}: the premise: the window that took the \
             space took the keyboard"
        );
        assert_eq!(
            evaluate(&desk, "return table.concat(heard, \",\")"),
            format!("{} settle true", grows.get()),
            "follows.settle = {follows}: the hand-off was not told as the \
             compositor's own settle"
        );
        let at = desk
            .state
            .seat
            .get_pointer()
            .map(|pointer| pointer.current_location())
            .expect("the seat has a pointer");
        if follows {
            let took = desk.placed(grows).to_f64();
            assert!(
                took.contains(at),
                "follows.settle = true: the pointer did not go to the window \
                 that took the space: {at:?}, {took:?}"
            );
        } else {
            assert_eq!(
                (at.x, at.y),
                (1.0, 1.0),
                "follows.settle = false: a close moved the pointer"
            );
        }
    }
}

/// **A hand-off the session puts off goes by the compositor's own
/// rule, not by a close it no longer answers** (#128 review). A
/// lock, a key still down from an unlock, or a scene holding the
/// keyboard each keeps `settle_focus` from handing the keyboard
/// on as a close lands; the settle that comes later -- here the
/// key coming up -- is the one after an unlock, which goes to the
/// window under the pointer and then the topmost. Tiling's
/// `after_close = {}` would leave the keyboard on nothing, so a
/// window with the keyboard shows the close was not carried over.
/// With the closed window taken only after those early returns, as
/// it was, it fails: the stale close's `{}` left the keyboard on
/// nothing.
#[test]
fn a_settle_put_off_by_a_held_key_goes_by_the_compositors_own_rule() {
    let mut desk = Desk::new();
    let (_, closed, _) = three_tiles(&mut desk, &set_after_close("{}"));
    russian(&mut desk.state);
    point_at(&mut desk.state, (1.0, 1.0).into());
    give_keyboard(&mut desk, closed);
    super_q(&mut desk);
    // The session has just unlocked with a key still down, and
    // the keyboard waits for it to come up.
    desk.state.refocus_on_release = true;
    desk.ask();
    assert!(
        desk.state.focused_window().is_none(),
        "the premise: the keyboard waits for the key to come up"
    );

    // The key comes up, as `input::key` sees it.
    desk.state.refocus_on_release = false;
    desk.state.settle_focus();
    assert!(
        desk.state.focused_window().is_some(),
        "the settle after the key came up went by the close it put off \
         (`after_close = {{}}`), not by the compositor's own rule"
    );
}

/// **Hovering a window focuses it in tiling and scrolling, and
/// not in floating; a click on empty desktop still clears it**
/// (#219). The shipped `focus.modes`, as a session starts them on
/// a desktop: floating's click-only is in force first, and
/// switching to the layout takes its own answer at once. A
/// tablet's half is
/// `on_a_tablet_the_shipped_layouts_leave_hover_to_the_machine`.
/// Against `0ce2ab8` it fails at the first hover: tiling named
/// nothing, and kept floating's click-only.
#[test]
fn the_shipped_focus_follows_the_pointer_in_tiling_and_scrolling_and_not_in_floating() {
    for layout in LAYOUTS {
        let mut desk = Desk::new();
        assert!(
            desk.state.profile.focus_follows_mouse,
            "the premise: a desktop, whose own answer is to follow"
        );
        let (_, _, first) = desk.open();
        let (_, _, second) = desk.open();
        start(&mut desk, layout, "");
        russian(&mut desk.state);
        let focused = desk.focused();
        let other = if focused == first { second } else { first };

        // Over the window itself: this client never answers a
        // configure, so it is the size it opened at, at the
        // corner of its tile.
        let over = centre(
            desk.state
                .pane_outer_of(other)
                .expect("a mapped pane has a rectangle"),
        );
        move_pointer(&mut desk.state, over, 10);
        assert_eq!(
            desk.focused(),
            other,
            "{layout}: the window under the pointer did not take the keyboard"
        );

        // The gap at the corner of the screen, which is empty
        // desktop in both layouts.
        move_pointer(&mut desk.state, (3.0, 3.0), 20);
        let region = crate::monitor::union(&desk.state.space).expect("a monitor");
        crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Pressed, 21);
        crate::synth::send_button(&mut desk.state, region, 0x110, ButtonState::Released, 22);
        assert!(
            desk.state.focused_window().is_none(),
            "{layout}: a click on empty desktop left a window focused"
        );

        // Back to floating, which follows nothing.
        assert!(
            desk.state.trigger(key_of(layout)),
            "{layout}: the key back to floating was not handled"
        );
        let stood = desk
            .state
            .pane_outer_of(focused)
            .expect("a mapped pane has a rectangle");
        let over = centre(stood);
        move_pointer(&mut desk.state, over, 30);
        assert!(
            desk.state.focused_window().is_none(),
            "{layout}: back in floating, hovering a window focused it"
        );
    }
}

/// **A mode the configuration names nowhere gets the machine's
/// own answer, not the mode before it** -- for hover, and for
/// `after_close`, which goes back to the compositor's own rule,
/// the window under the pointer and then the topmost. A
/// `user.lua` written for the shipped file before #128 named only
/// floating's click-only; switching to tiling then kept it,
/// because the compositor read "not named" as "leave it as it
/// is". On this (desktop) profile the machine's answer is to
/// follow.
/// Against `0ce2ab8` it fails: the pointer was not followed. With
/// `after_close` kept when a mode names none (`if let Some(list) =
/// after_close { self.after_close = list }`) it fails at the last
/// assertion.
#[test]
fn a_mode_that_names_nothing_gets_the_machines_own_focus_not_the_last_modes() {
    use crate::successor::Successor;
    let mut desk = Desk::new();
    start(
        &mut desk,
        "floating",
        "local focus = require(\"config\").focus\n\
         focus.modes = { floating = \"click\" }\n\
         focus.after_close = { floating = { \"recent\" } }\n\
         require(\"tiling\")",
    );
    assert_eq!(
        (
            desk.state.profile.focus_follows_mouse,
            desk.state.after_close.clone()
        ),
        (false, vec![Successor::Recent]),
        "the premise: floating's own answers, at load"
    );

    assert!(
        desk.state.trigger("super+t"),
        "the layout key was not handled"
    );
    let machine = crate::input::profile::Profile::default();
    assert_eq!(
        desk.state.profile.focus_follows_mouse, machine.focus_follows_mouse,
        "a mode named nowhere kept the last mode's focus model"
    );
    assert_eq!(
        desk.state.after_close,
        Successor::UNCONFIGURED.to_vec(),
        "a mode with no `after_close` kept the last mode's"
    );
}

/// Two windows in `layout`, started with `before`, and whether the
/// pointer moving onto the one without the keyboard gives it the
/// keyboard. With `us,ru` and Russian active, as every input test.
fn hover_focuses(desk: &mut Desk, layout: &str, before: &str) -> bool {
    let (_, _, first) = desk.open();
    let (_, _, second) = desk.open();
    start(desk, layout, before);
    russian(&mut desk.state);
    let focused = desk.focused();
    let other = if focused == first { second } else { first };
    // Over the window itself: this client never answers a
    // configure, so it is the size it opened at, at the corner of
    // its tile.
    let over = centre(
        desk.state
            .pane_outer_of(other)
            .expect("a mapped pane has a rectangle"),
    );
    move_pointer(&mut desk.state, over, 10);
    desk.focused() == other
}

/// **Your own `focus.follow = false` keeps every mode from
/// following the pointer, unless you name a mode** (#128 review,
/// the ranking of 2026-10-14). Yours outranks the shipped
/// `tiling = "follow"` and `scrolling = "follow"`, which a merge
/// had left winning over it, and an entry of yours for the mode
/// outranks your `follow`.
/// Against the ranking before it -- any entry in `focus.modes`,
/// then `focus.follow` -- it fails at the first hover: the shipped
/// "follow" won.
#[test]
fn your_focus_follow_false_keeps_every_mode_from_following_unless_you_name_one() {
    for layout in LAYOUTS {
        for (named, follows) in [(false, false), (true, true)] {
            let mut before = String::from("require(\"config\").focus.follow = false");
            if named {
                before.push_str(&format!(
                    "\nrequire(\"config\").focus.modes.{layout} = \"follow\""
                ));
            }
            let mut desk = Desk::new();
            assert_eq!(
                hover_focuses(&mut desk, layout, &before),
                follows,
                "{layout}, `focus.follow = false`, the mode named by you: {named}"
            );
        }
    }
}

/// **On a tablet the shipped layouts leave hover to the machine**
/// (#128 review, with #159): a shipped `"follow"` is the
/// machine's own answer, and a tablet's, with no pointer to
/// hover, is not to follow. An entry of your own still follows,
/// on any machine.
/// With a shipped `"follow"` read as yours, it fails at the first
/// hover: the tablet followed the pointer.
#[test]
fn on_a_tablet_the_shipped_layouts_leave_hover_to_the_machine() {
    use crate::input::profile::{FormFactor, Profile};
    for layout in LAYOUTS {
        for (yours, follows) in [(false, false), (true, true)] {
            let before = if yours {
                format!("require(\"config\").focus.modes.{layout} = \"follow\"")
            } else {
                String::new()
            };
            let mut desk = Desk::new();
            desk.state.profile = Profile::for_form_factor(FormFactor::Tablet);
            assert_eq!(
                hover_focuses(&mut desk, layout, &before),
                follows,
                "{layout} on a tablet, `follow` named by you: {yours}"
            );
        }
    }
}
