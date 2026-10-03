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
//! With a client, on a 1920x1080 monitor and `us,ru` with Russian live (#132):
//!
//! | step | |
//! |---|---|
//! | `open = true` | a window of 200x100, given the keyboard |
//! | `move = { x, y }` | the last window opened, put there |
//! | `focus = n` | the n-th window opened, given the keyboard |
//! | `field = { x, y, w, h }` | the client enables a text field with its caret there, in its surface |
//! | `field = true` | the client enables a text field and says nothing about a caret |
//! | `caret = { x, y, w, h }` | the client says its field's caret has moved there, as kitty does after a key, without enabling it again |
//! | `disable = true` | the client disables it |
//! | `framed = n` | the n-th window opened may have a frame again, as one leaving fullscreen may: its frame is on its way, and no Qt here builds it |
//! | `bare = n` | the n-th window opened is drawn bare, as a fullscreen window and one drawing its own decorations are; every window opens bare here, under the style `"none"` |
//! | `key = "combo"` | keys pressed by name through the real input path, as `SOLIUM_KEY_AT` presses them |
//!
//! On the Qt thread:
//!
//! | step | |
//! |---|---|
//! | `pane = "name", client = { w, h }` | a frame built from the shipped style of that name, around a client that size |
//! | `tell = { caret = { x, y, w, h } or false, values = { ... } }` | what a frame tells it: the caret, and values merged as `sol.pane_values` merges them |
//! | `wait = ms` | that much time, on the clock QML's animations and timers run on |
//! | `scene = "path", size = { w, h }` | a shipped scene, `qml/` and that path, built that size, as a `sol.surface` builds one |
//! | `set = { ... }` | properties written into that scene, as a redeclared surface writes them |
//!
//! And for either, `expect = function(world) end`, which fails the scenario by
//! raising an error, `assert` included. `world` holds what the compositor holds:
//! `surfaces` (by name: `x`, `y`, `w`, `h` for one placed at a rect, and its
//! `properties`), `panes` (what `sol.pane_values` handed every pane), `field`
//! (what `sol.text_input()` answers, `framed` included), `windows` (each
//! window opened: `id`, `x`, `y`) and `unknown` (each setting `solium --check`
//! would report: `key`, and the `meant` it suggests); or, on the Qt thread,
//! `pixel(layer, x, y)`, the frame's layer of that name rendered, answering
//! `r, g, b, a` -- the scene is the layer `"scene"` -- and `dormant`, `true`
//! by the name of each of the frame's layers that says it is dormant now.
//! `tests::every_scenario_with_a_client_passes`,
//! `tests::every_scenario_on_the_qt_thread_passes`.

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

/// A monitor of 1920x1080 at the origin, and `us,ru` with Russian live.
fn furnish(state: &mut crate::state::Solium) {
    use smithay::input::keyboard::{Layout, XkbConfig};
    let output = Output::new(
        "scenario-1".to_owned(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "solium".to_owned(),
            model: "scenario".to_owned(),
        },
    );
    output.change_current_state(
        Some(Mode {
            size: (1920, 1080).into(),
            refresh: 60_000,
        }),
        None,
        Some(Scale::Fractional(1.0)),
        None,
    );
    state.space.map_output(&output, (0, 0));
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
        }
        opened.set(index + 1, entry)?;
    }
    world.set("windows", opened)?;
    // What `solium --check` would call a setting nothing reads, which a
    // scenario's own `user` can be checked against:
    // `tests::every_scenario_with_a_client_passes`.
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

/// Play a scenario with a real client.
fn with_a_client(path: &Path) {
    let lua = Lua::new();
    let scenario = read(&lua, path);
    let mut desk = Desk::new();
    furnish(&mut desk.state);
    configure(&mut desk.state, path, &scenario);
    let mut windows: Vec<Window> = Vec::new();
    let mut text_input = None;
    let steps: Vec<Table> = scenario.get("steps").expect("`steps` is a list");
    for (index, step) in steps.iter().enumerate() {
        let index = index + 1;
        if step.get::<Option<bool>>("open").ok().flatten() == Some(true) {
            let (window, _, _) = desk.window();
            desk.focus(&window);
            windows.push(window);
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
        } else if let Ok(Some(combo)) = step.get::<Option<String>>("key") {
            let region = Rectangle::from_size((1920, 1080).into());
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
            let (built, size) = decoration.as_mut().expect("a `pane` step first");
            built.tell_as_a_frame_would(
                &Look {
                    title: "",
                    focused: true,
                    pointer_inside: false,
                    caret,
                    values: &values,
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
            let world = lua.create_table().expect("a table");
            world.set("pixel", pixel).expect("pixel");
            world.set("dormant", dormant).expect("dormant");
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

    /// **Every scenario with a client passes.**
    ///
    /// Not on a machine whose own `~/.config/solium` holds Lua: that comes
    /// first on `package.path`, so `require("user")` would find the
    /// developer's settings and not the scenario's. Skipped there, as
    /// `script::tests::shipped_init_with_user` skips.
    #[test]
    fn every_scenario_with_a_client_passes() {
        let found: Vec<_> = super::scenarios()
            .into_iter()
            .filter(|path| !on_qt(path))
            .collect();
        assert!(!found.is_empty(), "no scenarios found; the walk is broken");
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
        for path in found {
            super::with_a_client(&path);
        }
    }

    /// **Every scenario on the Qt thread passes.**
    #[test]
    fn every_scenario_on_the_qt_thread_passes() {
        let found: Vec<_> = super::scenarios()
            .into_iter()
            .filter(|path| on_qt(path))
            .collect();
        assert!(!found.is_empty(), "no scenarios found; the walk is broken");
        crate::qml::qt_test::on_the_qt_thread(move || {
            for path in found {
                super::on_the_qt_thread(&path);
            }
        });
    }
}
