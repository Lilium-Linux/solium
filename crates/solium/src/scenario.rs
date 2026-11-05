//! Scenarios: the shipped configuration driven as a person drives it, and
//! checked by what the compositor then holds.
//!
//! A feature written as configuration -- Lua policy, and QML drawing from the
//! data the compositor publishes -- has no Rust of its own, and its tests
//! should not need any either: the compositor is not meant to know the
//! feature exists. So they are written beside it, as data, and this plays
//! them. Each file in `crates/solium/tests/scenarios/` returns a table:
//!
//! ```lua
//! return {
//!     -- Played with a real Wayland client and no Qt, or, with `qt = true`,
//!     -- on the Qt thread with no client: a process holding a raw libwayland
//!     -- connection cannot build a Qt scene (`state::tests`, `scale_resend`).
//!     qt = false,
//!     -- The `user.lua` the shipped `config.lua` merges, and the `init.lua`
//!     -- that is loaded: both found ahead of the shipped files.
//!     user = [[ return { ... } ]],
//!     init = [[ require("...") ]],
//!     steps = { { open = true }, { key = "caps_lock" }, { expect = function(world) ... end } },
//! }
//! ```
//!
//! With a client, on a 1920x1080 monitor and `us,ru` with Russian live (#132).
//! `monitors = { { w, h }, ... }` names the monitors instead: side by side from
//! the origin, top-aligned, `scenario-1` first.
//!
//! | step | |
//! |---|---|
//! | `open = true` | a window of 200x100, given the keyboard; with `app_id = "name"`, of that application |
//! | `pointer = { x, y }` | the pointer moved there through the real input path, which also makes that monitor the one a new window opens on |
//! | `answer = n` | the n-th window opened answers the last size it was configured with, as a client does |
//! | `wait = ms` | that much time, on the compositor's clock, and the frame at the end of it settled |
//! | `move = { x, y }` | the last window opened, put there |
//! | `focus = n` | the n-th window opened, given the keyboard |
//! | `field = { x, y, w, h }` | the client enables a text field with its caret there, in its surface |
//! | `field = true` | the client enables a text field and says nothing about a caret |
//! | `caret = { x, y, w, h }` | the client says its field's caret has moved there, as kitty does after a key, without enabling it again |
//! | `disable = true` | the client disables it |
//! | `framed = n` | the n-th window opened may have a frame again, as one leaving fullscreen may: its frame is on its way, and no Qt here builds it |
//! | `bare = n` | the n-th window opened is drawn bare, as a fullscreen window and one drawing its own decorations are; every window opens bare here, under the style `"none"` |
//! | `key = "combo"` | keys pressed by name through the real input path, as `SOLIUM_KEY_AT` presses them |
//! | `press = { x, y }` | the pointer moved there and the left button pressed, through the real input path; with `super = true`, the drag modifier held first, and with `button = "right"` the right button |
//! | `drag = { x, y }` | the pointer moved there with the button held, and the pass ended as a frame ends one: the `drag` listeners told where it got to, an edge drag settled |
//! | `release = true` | the button let go, and the drag modifier if it was held, and the pass ended |
//!
//! On the Qt thread:
//!
//! | step | |
//! |---|---|
//! | `pane = "name", client = { w, h }` | a frame built from the shipped style of that name, around a client that size |
//! | `tell = { caret = { x, y, w, h } or false, values = { ... } }` | what a frame tells it: the caret, and values merged as `sol.pane_values` merges them; and, kept until told again, `focused` (true until then), `inside` (whether the pointer is over the window, false until then), `title` and `app_id` (each empty until then) |
//! | `point = { x, y }` | the pointer moved there, in the pane's own space, as the compositor hands it to every layer; `point = false`, gone from the pane, which is also no longer `inside` it |
//! | `wait = ms` | that much time, on the clock QML's animations and timers run on |
//! | `scene = "path", size = { w, h }` | a shipped scene, `qml/` and that path, built that size, as a `sol.surface` builds one |
//! | `set = { ... }` | properties written into that scene, as a redeclared surface writes them |
//!
//! And for either, `expect = function(world) end`, which fails the scenario by
//! raising an error, `assert` included. `world` holds what the compositor holds:
//! `surfaces` (by name: `x`, `y`, `w`, `h` for one placed at a rect, and its
//! `properties`), `panes` (what `sol.pane_values` handed every pane), `field`
//! (what `sol.text_input()` answers, `framed` included), `windows` (each
//! window opened: `id`; `x`, `y`, `w`, `h`, where it lives; `drawn` -- the
//! rectangle it is drawn at now, `x`, `y`, `w`, `h` -- `transformed`,
//! whether it holds a presentation transform at all, `maximized`, and
//! `values`, what `sol.frame` handed its frame) and `unknown` (each
//! setting `solium --check` would report: `key`, and the `meant` it
//! suggests); or, on the Qt thread,
//! `pixel(layer, x, y)`, the frame's layer of that name rendered, answering
//! `r, g, b, a` -- the scene is the layer `"scene"` -- and `dormant`, `true`
//! by the name of each of the frame's layers that says it is dormant now, and
//! `opacity`, each layer's `layerOpacity` by its name: how opaque the next
//! frame composites the picture `pixel` reads.
//! `tests::each`, one test per file.

use std::path::{Path, PathBuf};
use std::time::Duration;

use mlua::{Function, Lua, Table, Value};
use smithay::desktop::Window;
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::utils::Rectangle;

use crate::json::Json;
use crate::text_input::tests::{Desk, enable};

/// Every scenario file, sorted.
fn scenarios() -> Vec<PathBuf> {
    let directory = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/scenarios"));
    let mut found: Vec<PathBuf> = std::fs::read_dir(directory)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|end| end == "lua"))
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

/// A scenario file, read into a Lua state of its own.
fn read(lua: &Lua, path: &Path) -> Table {
    let text = std::fs::read_to_string(path).expect("reading a scenario");
    lua.load(&text)
        .set_name(path.display().to_string())
        .eval::<Table>()
        .unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

/// A JSON value as a Lua one.
fn lua_of(lua: &Lua, json: &Json) -> mlua::Result<Value> {
    Ok(match json {
        Json::Null => Value::Nil,
        Json::Bool(yes) => Value::Boolean(*yes),
        Json::Number(number) => Value::Number(*number),
        Json::Text(text) => Value::String(lua.create_string(text)?),
        Json::List(items) => {
            let table = lua.create_table()?;
            for (index, item) in items.iter().enumerate() {
                table.set(index + 1, lua_of(lua, item)?)?;
            }
            Value::Table(table)
        }
        Json::Object(fields) => {
            let table = lua.create_table()?;
            for (key, value) in fields {
                table.set(key.as_str(), lua_of(lua, value)?)?;
            }
            Value::Table(table)
        }
    })
}

/// Four numbers out of a step.
fn rect_of(table: &Table) -> mlua::Result<(i32, i32, i32, i32)> {
    Ok((table.get(1)?, table.get(2)?, table.get(3)?, table.get(4)?))
}

/// Run an `expect` step, failing the scenario with what it raised.
fn expect(path: &Path, index: usize, check: &Function, world: Table) {
    if let Err(err) = check.call::<()>(world) {
        panic!("{} step {index}: {err}", path.display());
    }
}

/// The configuration a scenario names, written where its `require`s find it
/// first, and loaded.
fn configure(state: &mut crate::state::Solium, path: &Path, scenario: &Table) {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("scenario");
    let directory = std::env::temp_dir().join(format!("solium-scenario-{stem}"));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let user: Option<String> = scenario.get("user").expect("`user` is text");
    std::fs::write(
        directory.join("user.lua"),
        user.unwrap_or_else(|| "return {}".to_owned()),
    )
    .expect("writing user.lua");
    let init: Option<String> = scenario.get("init").expect("`init` is text");
    let entry = directory.join("init.lua");
    std::fs::write(&entry, init.unwrap_or_default()).expect("writing init.lua");
    let scripts = crate::script::Scripts::load(&entry)
        .unwrap_or_else(|err| panic!("{}: loading its configuration: {err}", path.display()));
    state.start_scripts(Some(scripts));
    let _ = std::fs::remove_dir_all(&directory);
}

/// The monitors a scenario names, `{ w, h }` each, or one of 1920x1080.
fn sizes_of(scenario: &Table) -> Vec<(i32, i32)> {
    let named: Option<Vec<Table>> = scenario.get("monitors").expect("`monitors` is a list");
    named.map_or_else(
        || vec![(1920, 1080)],
        |monitors| {
            monitors
                .iter()
                .map(|size| (size.get(1).expect("w"), size.get(2).expect("h")))
                .collect()
        },
    )
}

/// Those monitors, side by side from the origin, top-aligned and named
/// `scenario-1`, `scenario-2` and on; and `us,ru` with Russian live.
fn furnish(state: &mut crate::state::Solium, sizes: &[(i32, i32)]) {
    use smithay::input::keyboard::{Layout, XkbConfig};
    let mut x = 0;
    for (index, &(w, h)) in sizes.iter().enumerate() {
        let output = Output::new(
            format!("scenario-{}", index + 1),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "solium".to_owned(),
                model: "scenario".to_owned(),
            },
        );
        output.change_current_state(
            Some(Mode {
                size: (w, h).into(),
                refresh: 60_000,
            }),
            None,
            Some(Scale::Fractional(1.0)),
            None,
        );
        state.space.map_output(&output, (x, 0));
        x += w;
    }
    let keyboard = state.seat.get_keyboard().expect("the seat has a keyboard");
    keyboard
        .set_xkb_config(
            state,
            XkbConfig {
                layout: "us,ru",
                options: Some("grp:alt_shift_toggle".to_owned()),
                ..Default::default()
            },
        )
        .expect("compiling us,ru");
    keyboard.with_xkb_state(state, |mut context| context.set_layout(Layout(1)));
    state.keyboard = crate::keymap::describe(state);
}

/// What the compositor holds, as a scenario's `expect` reads it.
fn world(lua: &Lua, desk: &Desk, windows: &[Window]) -> mlua::Result<Table> {
    let state = &desk.state;
    let world = lua.create_table()?;
    let surfaces = lua.create_table()?;
    for surface in state.surfaces.iter() {
        let entry = lua.create_table()?;
        if let crate::scripted::On::Rect(rect) = surface.declared.on {
            entry.set("x", rect.loc.x)?;
            entry.set("y", rect.loc.y)?;
            entry.set("w", rect.size.w)?;
            entry.set("h", rect.size.h)?;
        }
        let properties = Json::Object(surface.declared.properties.fields().clone());
        entry.set("properties", lua_of(lua, &properties)?)?;
        surfaces.set(surface.name(), entry)?;
    }
    world.set("surfaces", surfaces)?;
    world.set("panes", lua_of(lua, state.decorations.values().object())?)?;
    if let Some(field) = state.text_field() {
        let entry = lua.create_table()?;
        entry.set("window", field.window)?;
        entry.set("framed", field.framed)?;
        if let Some(caret) = field.caret {
            entry.set("x", caret.loc.x)?;
            entry.set("y", caret.loc.y)?;
            entry.set("w", caret.size.w)?;
            entry.set("h", caret.size.h)?;
        }
        world.set("field", entry)?;
    }
    let opened = lua.create_table()?;
    let snapshot = state.snapshot();
    for (index, window) in windows.iter().enumerate() {
        let entry = lua.create_table()?;
        entry.set(
            "id",
            state
                .panes
                .id_of(window)
                .map_or(0, crate::pane::PaneId::get),
        )?;
        if let Some(real) = state.real_geometry(window) {
            entry.set("x", real.loc.x)?;
            entry.set("y", real.loc.y)?;
            entry.set("w", real.size.w)?;
            entry.set("h", real.size.h)?;
        }
        if let Some(pane) = state.panes.of(window) {
            let drawn = state
                .drawn_at(pane, state.pane_outer(pane), state.clock.now())
                .rect;
            let rect = lua.create_table()?;
            rect.set("x", drawn.loc.x)?;
            rect.set("y", drawn.loc.y)?;
            rect.set("w", drawn.size.w)?;
            rect.set("h", drawn.size.h)?;
            entry.set("drawn", rect)?;
            entry.set("transformed", crate::present::transformed(pane))?;
            entry.set("values", lua_of(lua, pane.frame_values().object())?)?;
        }
        let id = state.panes.id_of(window).map(crate::pane::PaneId::get);
        entry.set(
            "maximized",
            snapshot
                .windows
                .iter()
                .any(|info| Some(info.id) == id && info.maximized),
        )?;
        opened.set(index + 1, entry)?;
    }
    world.set("windows", opened)?;
    // What `solium --check` would call a setting nothing reads, which a
    // scenario's own `user` can be checked against:
    // `tests::each`.
    let unknown = lua.create_table()?;
    if let Some(scripts) = state.scripts.as_ref() {
        for (index, setting) in scripts.unknown_settings().into_iter().enumerate() {
            let entry = lua.create_table()?;
            entry.set("key", setting.key)?;
            entry.set("meant", setting.meant)?;
            unknown.set(index + 1, entry)?;
        }
    }
    world.set("unknown", unknown)?;
    Ok(world)
}

/// The pane of the n-th window a scenario opened, counted from 1.
fn pane_of(desk: &Desk, windows: &[Window], n: usize) -> crate::pane::PaneId {
    windows
        .get(n.saturating_sub(1))
        .and_then(|window| desk.state.panes.id_of(window))
        .expect("that window, with a pane")
}

/// The pointer moved to `at` through the real input path, from wherever it
/// is: where it ends up.
fn point_to(
    desk: &mut Desk,
    region: Rectangle<i32, smithay::utils::Logical>,
    at: (f64, f64),
    time: u64,
) -> smithay::utils::Point<f64, smithay::utils::Logical> {
    let location = |desk: &Desk| {
        desk.state
            .seat
            .get_pointer()
            .map(|pointer| pointer.current_location())
            .unwrap_or_default()
    };
    let was = location(desk);
    crate::synth::send_motion(
        &mut desk.state,
        region,
        smithay::utils::Point::from(at) - was,
        time,
    );
    location(desk)
}

/// The end of a pass, as each loop ends one: what a drag did told to the
/// `drag` listeners, an edge drag settled, and the client told what that
/// sent.
fn end_pass(desk: &mut Desk) {
    crate::input::grab::settle_drags(&mut desk.state, false);
    desk.state.settle_resize();
    desk.pump();
}

/// The left button, or the right.
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
/// Super, as evdev numbers it plus xkb's eight.
const SUPER: u32 = 133;

/// Play a scenario with a real client.
fn with_a_client(path: &Path) {
    let lua = Lua::new();
    let scenario = read(&lua, path);
    let mut desk = Desk::new();
    furnish(&mut desk.state, &sizes_of(&scenario));
    let region = crate::monitor::union(&desk.state.space).expect("a monitor");
    configure(&mut desk.state, path, &scenario);
    let mut windows: Vec<Window> = Vec::new();
    // Each window's surface and toplevel, in the order opened, for `answer`.
    let mut clients = Vec::new();
    let mut text_input = None;
    // The button a `press` holds, and whether with the drag modifier, until
    // its `release`; and the time each pointer event is stamped with.
    let mut held: Option<(u32, bool)> = None;
    let mut time: u64 = 1_000;
    let steps: Vec<Table> = scenario.get("steps").expect("`steps` is a list");
    for (index, step) in steps.iter().enumerate() {
        let index = index + 1;
        time += 1_000;
        if step.get::<Option<bool>>("open").ok().flatten() == Some(true) {
            let app_id: Option<String> = step.get("app_id").expect("`app_id` is text");
            let (window, surface, _, toplevel) = desk.window_of(app_id.as_deref());
            desk.focus(&window);
            windows.push(window);
            clients.push((surface, toplevel));
        } else if let Ok(Some(n)) = step.get::<Option<usize>>("answer") {
            let (surface, toplevel) = clients
                .get(n.saturating_sub(1))
                .cloned()
                .expect("that window");
            desk.answer(&surface, &toplevel);
        } else if let Ok(Some(ms)) = step.get::<Option<u64>>("wait") {
            desk.state.clock.advance(Duration::from_millis(ms));
            let now = desk.state.clock.now();
            desk.state.settle(now);
            desk.state.sync_panes();
        } else if let Ok(Some(at)) = step.get::<Option<Table>>("move") {
            let (x, y): (i32, i32) = (at.get(1).expect("x"), at.get(2).expect("y"));
            let window = windows.last().cloned().expect("a window to move");
            desk.state.space.map_element(window, (x, y), false);
        } else if let Ok(Some(n)) = step.get::<Option<usize>>("focus") {
            let window = windows
                .get(n.saturating_sub(1))
                .cloned()
                .expect("that window");
            desk.focus(&window);
        } else if let Ok(Some(value)) = step.get::<Option<Value>>("field") {
            let input = text_input.get_or_insert_with(|| desk.text_input()).clone();
            match value {
                Value::Table(caret) => enable(&input, rect_of(&caret).expect("x, y, w, h")),
                _ => {
                    input.enable();
                    input.commit();
                }
            }
            desk.pump();
        } else if let Ok(Some(caret)) = step.get::<Option<Table>>("caret") {
            let (x, y, w, h) = rect_of(&caret).expect("x, y, w, h");
            let input = text_input.as_ref().expect("a field to move the caret of");
            input.set_cursor_rectangle(x, y, w, h);
            input.commit();
            desk.pump();
        } else if let Ok(Some(n)) = step.get::<Option<usize>>("framed") {
            let id = pane_of(&desk, &windows, n);
            desk.state.decorations.unset_bare(&mut desk.state.panes, id);
            desk.pump();
        } else if let Ok(Some(n)) = step.get::<Option<usize>>("bare") {
            let id = pane_of(&desk, &windows, n);
            desk.state.decorations.set_bare(&mut desk.state.panes, id);
            desk.pump();
        } else if step.get::<Option<bool>>("disable").ok().flatten() == Some(true) {
            if let Some(input) = text_input.as_ref() {
                input.disable();
                input.commit();
            }
            desk.pump();
        } else if let Ok(Some(at)) = step.get::<Option<Table>>("press") {
            let point: (f64, f64) = (at.get(1).expect("x"), at.get(2).expect("y"));
            let modifier = at
                .get::<Option<bool>>("super")
                .expect("`super` is true or false");
            let button: Option<String> = at.get("button").expect("`button` is text");
            let button = if button.as_deref() == Some("right") {
                BTN_RIGHT
            } else {
                BTN_LEFT
            };
            let modifier = modifier == Some(true);
            point_to(&mut desk, region, point, time);
            desk.pump();
            if modifier {
                crate::input::key(
                    &mut desk.state,
                    SUPER.into(),
                    smithay::backend::input::KeyState::Pressed,
                    u32::try_from(time / 1_000).unwrap_or(u32::MAX),
                );
            }
            crate::synth::send_button(
                &mut desk.state,
                region,
                button,
                smithay::backend::input::ButtonState::Pressed,
                time + 1,
            );
            held = Some((button, modifier));
            end_pass(&mut desk);
        } else if let Ok(Some(at)) = step.get::<Option<Table>>("drag") {
            let point: (f64, f64) = (at.get(1).expect("x"), at.get(2).expect("y"));
            assert!(
                held.is_some(),
                "{} step {index}: a drag with no button held",
                path.display()
            );
            point_to(&mut desk, region, point, time);
            end_pass(&mut desk);
        } else if step.get::<Option<bool>>("release").ok().flatten() == Some(true) {
            let (button, modifier) = held.take().unwrap_or_else(|| {
                panic!(
                    "{} step {index}: a release with no button held",
                    path.display()
                )
            });
            crate::synth::send_button(
                &mut desk.state,
                region,
                button,
                smithay::backend::input::ButtonState::Released,
                time,
            );
            if modifier {
                crate::input::key(
                    &mut desk.state,
                    SUPER.into(),
                    smithay::backend::input::KeyState::Released,
                    u32::try_from(time / 1_000).unwrap_or(u32::MAX),
                );
            }
            end_pass(&mut desk);
        } else if let Ok(Some(at)) = step.get::<Option<Table>>("pointer") {
            let at: (f64, f64) = (at.get(1).expect("x"), at.get(2).expect("y"));
            let now = point_to(&mut desk, region, at, 0);
            desk.pump();
            assert!(
                (now.x - at.0).abs() < 0.5 && (now.y - at.1).abs() < 0.5,
                "{} step {index}: the pointer did not reach {at:?}: it is at {now:?}",
                path.display()
            );
        } else if let Ok(Some(combo)) = step.get::<Option<String>>("key") {
            assert!(
                crate::synth::key(&mut desk.state, region, &combo, 0),
                "{} step {index}: no key types {combo}",
                path.display()
            );
            desk.pump();
        } else if let Ok(Some(check)) = step.get::<Option<Function>>("expect") {
            expect(
                path,
                index,
                &check,
                world(&lua, &desk, &windows).expect("the world"),
            );
        } else {
            panic!(
                "{} step {index}: not a step a client scenario has",
                path.display()
            );
        }
    }
}

/// Play a scenario on the Qt thread.
fn on_the_qt_thread(path: &Path) {
    use crate::decoration::{Decoration, Look, Values};

    let lua = Lua::new();
    let scenario = read(&lua, path);
    crate::qml::start().expect("Qt starts");
    if crate::qml::on_gpu() {
        // Built at 1x1 into a dmabuf, with no image to read back.
        return;
    }
    // QML's clock, which its animations run on: the compositor's, handed to
    // it once a frame, from a moment of this scenario's own. Moved a frame at
    // a time, and in real time too, because a Timer with no animation beside
    // it is Qt's own real timer (`MainContext` in `qml/host.cpp`), which only
    // the frame's draining of Qt's events delivers.
    let mut clock = Duration::from_secs(1_000_000);
    let mut tick = |by: Duration| {
        let until = clock + by;
        while clock < until {
            std::thread::sleep(Duration::from_millis(16));
            clock += Duration::from_millis(16);
            crate::qml::tick(clock);
        }
    };
    let mut decoration: Option<(Decoration, (i32, i32))> = None;
    let mut scene: Option<crate::qml::Scene> = None;
    let mut values = Values::default();
    let mut caret = None;
    let mut focused = true;
    let mut inside = false;
    let mut title = String::new();
    let mut app_id = String::new();
    let steps: Vec<Table> = scenario.get("steps").expect("`steps` is a list");
    for (index, step) in steps.iter().enumerate() {
        let index = index + 1;
        if let Ok(Some(name)) = step.get::<Option<String>>("pane") {
            let client: Table = step.get("client").expect("`client = { w, h }`");
            let (w, h): (i32, i32) = (client.get(1).expect("w"), client.get(2).expect("h"));
            let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/qml/panes")).join(&name);
            let style = crate::style::load(&dir).expect("a shipped style loads");
            let size = (w + style.insets.horizontal(), h + style.insets.vertical());
            let built = Decoration::from_style(&style, w, h).expect("the style builds");
            decoration = Some((built, size));
        } else if let Ok(Some(tell)) = step.get::<Option<Table>>("tell") {
            match tell.get::<Value>("caret").expect("a caret, or false") {
                Value::Table(at) => {
                    let (x, y, w, h) = rect_of(&at).expect("x, y, w, h");
                    caret = Some(Rectangle::new((x, y).into(), (w, h).into()));
                }
                Value::Boolean(false) => caret = None,
                _ => {}
            }
            if let Ok(Some(handed)) = tell.get::<Option<Table>>("values") {
                values.merge(Json::object_from_lua(&handed).expect("values"));
            }
            if let Ok(Some(told)) = tell.get::<Option<bool>>("focused") {
                focused = told;
            }
            if let Ok(Some(told)) = tell.get::<Option<bool>>("inside") {
                inside = told;
            }
            if let Ok(Some(told)) = tell.get::<Option<String>>("title") {
                title = told;
            }
            if let Ok(Some(told)) = tell.get::<Option<String>>("app_id") {
                app_id = told;
            }
            let (built, size) = decoration.as_mut().expect("a `pane` step first");
            built.tell_as_a_frame_would(
                &Look {
                    title: &title,
                    app_id: &app_id,
                    focused,
                    pointer_inside: inside,
                    caret,
                    values: &values,
                    window: None,
                    window_values: crate::decoration::Values::none(),
                },
                size.0,
                size.1,
            );
            tick(Duration::from_millis(16));
        } else if let Ok(Some(name)) = step.get::<Option<String>>("scene") {
            let size: Table = step.get("size").expect("`size = { w, h }`");
            let (w, h): (i32, i32) = (size.get(1).expect("w"), size.get(2).expect("h"));
            let file = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/qml")).join(&name);
            scene = Some(crate::qml::Scene::for_host(&file, w, h, None).expect("the scene builds"));
            tick(Duration::from_millis(16));
        } else if let Ok(Some(set)) = step.get::<Option<Table>>("set") {
            let built = scene.as_mut().expect("a `scene` step first");
            for (key, value) in Json::object_from_lua(&set).expect("properties") {
                assert!(
                    built.set_json(&key, &value),
                    "{} step {index}: the scene has no property {key}",
                    path.display()
                );
            }
            tick(Duration::from_millis(16));
        } else if let Ok(point @ (Value::Table(_) | Value::Boolean(false))) =
            step.get::<Value>("point")
        {
            let (built, _) = decoration.as_mut().expect("a `pane` step first");
            match point {
                Value::Table(at) => {
                    let (x, y): (f64, f64) = (at.get(1).expect("x"), at.get(2).expect("y"));
                    built.pointer(x, y, None);
                }
                _ => {
                    built.pointer_left();
                    inside = false;
                }
            }
            tick(Duration::from_millis(16));
        } else if let Ok(Some(ms)) = step.get::<Option<u64>>("wait") {
            tick(Duration::from_millis(ms));
        } else if let Ok(Some(check)) = step.get::<Option<Function>>("expect") {
            let dormant = lua.create_table().expect("a table");
            for name in decoration
                .as_ref()
                .map(|(built, _)| built.dormant_layers())
                .unwrap_or_default()
            {
                dormant.set(name, true).expect("a layer's name");
            }
            let mut layers = decoration
                .as_mut()
                .map(|(built, _)| built.rendered_layers())
                .unwrap_or_default();
            if let Some(built) = scene.as_mut()
                && let Ok(rendered) = built.render()
            {
                layers.push((
                    "scene".to_owned(),
                    rendered.pixels.to_vec(),
                    rendered.stride,
                ));
            }
            let pixel = lua
                .create_function(move |_, (name, x, y): (String, usize, usize)| {
                    let Some((_, pixels, stride)) =
                        layers.iter().find(|(layer, ..)| *layer == name)
                    else {
                        return Err(mlua::Error::runtime(format!("no layer named {name}")));
                    };
                    let at = y * stride + x * 4;
                    let read = pixels.get(at..at + 4).ok_or_else(|| {
                        mlua::Error::runtime(format!("({x}, {y}) is outside {name}"))
                    })?;
                    Ok((read[2], read[1], read[0], read[3]))
                })
                .expect("pixel");
            let opacity = lua.create_table().expect("a table");
            for (name, alpha) in decoration
                .as_ref()
                .map(|(built, _)| built.layer_opacities())
                .unwrap_or_default()
            {
                opacity.set(name, alpha).expect("a layer's opacity");
            }
            let world = lua.create_table().expect("a table");
            world.set("pixel", pixel).expect("pixel");
            world.set("dormant", dormant).expect("dormant");
            world.set("opacity", opacity).expect("opacity");
            expect(path, index, &check, world);
        } else {
            panic!(
                "{} step {index}: not a step a Qt scenario has",
                path.display()
            );
        }
    }
    // The frame and every animation and Timer in it go with it, so nothing
    // is left running on the clock this moved.
    drop(decoration);
    drop(scene);
    tick(Duration::from_millis(16));
}

#[cfg(test)]
mod tests {
    use mlua::Lua;

    /// Whether a scenario file is played on the Qt thread.
    fn on_qt(path: &std::path::Path) -> bool {
        let lua = Lua::new();
        super::read(&lua, path)
            .get::<Option<bool>>("qt")
            .ok()
            .flatten()
            .unwrap_or(false)
    }

    /// Play one scenario file: on the Qt thread when it says `qt = true`,
    /// otherwise against a client.
    ///
    /// A client scenario is skipped on a machine whose own `~/.config/solium`
    /// holds Lua: that comes first on `package.path`, so `require("user")`
    /// would find the developer's settings and not the scenario's, as
    /// `script::tests::shipped_init_with_user` skips.
    fn play(file: &str) {
        let path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/scenarios"))
            .join(file);
        if on_qt(&path) {
            crate::qml::qt_test::on_the_qt_thread(move || super::on_the_qt_thread(&path));
            return;
        }
        let own = crate::script::Scripts::user_config_dir()
            .and_then(|own| std::fs::read_dir(own).ok())
            .is_some_and(|entries| {
                entries
                    .filter_map(Result::ok)
                    .any(|entry| entry.path().extension().is_some_and(|kind| kind == "lua"))
            });
        if own {
            eprintln!(
                "skipped: ~/.config/solium holds Lua of its own, which a scenario would load"
            );
            return;
        }
        super::with_a_client(&path);
    }

    /// One test per scenario file, named after it, so the scenarios run in
    /// parallel (each in a process of its own, under nextest) and a failure
    /// names the scenario that failed. A new file is added here;
    /// `every_scenario_file_has_its_test` fails until it is.
    macro_rules! each {
        ($($name:ident => $file:literal,)*) => {
            /// The files the tests below play.
            const LISTED: &[&str] = &[$($file),*];

            mod each {
                $(
                    #[test]
                    fn $name() {
                        super::play($file);
                    }
                )*
            }
        };
    }

    each! {
        desktop_placement_cascade => "desktop-placement-cascade.lua",
        desktop_placement_center => "desktop-placement-center.lua",
        desktop_placement_pointer => "desktop-placement-pointer.lua",
        desktop_placement_tall_and_wide => "desktop-placement-tall-and-wide.lua",
        desktop_placement_two_monitors => "desktop-placement-two-monitors.lua",
        desktop_placement_wrap_center => "desktop-placement-wrap-center.lua",
        desktop_placement_wrap_corner => "desktop-placement-wrap-corner.lua",
        desktop_snap_divider => "desktop-snap-divider.lua",
        desktop_snap_drag => "desktop-snap-drag.lua",
        desktop_snap_keys => "desktop-snap-keys.lua",
        desktop_snap_off => "desktop-snap-off.lua",
        desktop_snap_settings => "desktop-snap-settings.lua",
        desktop_snap_tiling => "desktop-snap-tiling.lua",
        desktop_snap_two_monitors => "desktop-snap-two-monitors.lua",
        fullscreen_and_maximize_timed_apart => "fullscreen-and-maximize-timed-apart.lua",
        fullscreen_animate_false => "fullscreen-animate-false.lua",
        fullscreen_animate_true => "fullscreen-animate-true.lua",
        fullscreen_glides => "fullscreen-glides.lua",
        fullscreen_instant => "fullscreen-instant.lua",
        fullscreen_tiled => "fullscreen-tiled.lua",
        keyboard_check => "keyboard-check.lua",
        keyboard_follow => "keyboard-follow.lua",
        keyboard_indicator_false => "keyboard-indicator-false.lua",
        keyboard_off => "keyboard-off.lua",
        keyboard_pane_bare => "keyboard-pane-bare.lua",
        keyboard_pane_drawn => "keyboard-pane-drawn.lua",
        keyboard_pane => "keyboard-pane.lua",
        keyboard_surface_drawn => "keyboard-surface-drawn.lua",
        keyboard_surface => "keyboard-surface.lua",
        maximize_animate_false => "maximize-animate-false.lua",
        pane_bars_opaque => "pane-bars-opaque.lua",
        pane_buttons_drawn => "pane-buttons-drawn.lua",
        pane_glass_drawn => "pane-glass-drawn.lua",
        pane_proximity_drawn => "pane-proximity-drawn.lua",
        pane_pulse_drawn => "pane-pulse-drawn.lua",
        pane_top_drawn => "pane-top-drawn.lua",
        snap_drawn => "snap-drawn.lua",
    }

    /// **Every scenario file has its test**, and every test its file.
    #[test]
    fn every_scenario_file_has_its_test() {
        let found: Vec<String> = super::scenarios()
            .iter()
            .filter_map(|path| path.file_name()?.to_str().map(str::to_owned))
            .collect();
        assert!(!found.is_empty(), "no scenarios found; the walk is broken");
        for file in &found {
            assert!(
                LISTED.contains(&file.as_str()),
                "tests/scenarios/{file} has no test: add it to `each!` in scenario.rs"
            );
        }
        for file in LISTED {
            assert!(
                found.iter().any(|found| found == file),
                "`each!` in scenario.rs names {file}, which tests/scenarios does not have"
            );
        }
    }
}
