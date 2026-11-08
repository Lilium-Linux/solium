//! The windows: listing them, focusing, placing, moving and closing them.

use mlua::{Lua, Table, Value};
use smithay::utils::{Logical, Point};

use crate::script::lua::*;
use crate::script::*;

/// Add this area's functions to `sol`.
pub(crate) fn register(lua: &Lua, sol: &Table) -> mlua::Result<()> {
    // Every window a layout may arrange, topmost first.
    //
    // `modal` and `parent` are here rather than behind a `sol.dialog(id)` of
    // their own, and that is the point: a layout already walks this list once
    // per pass, and a second call per window to ask whether it is a dialog is a
    // second chance for the two answers to come from different moments. The
    // snapshot is one instant by construction — see the module header — and
    // anything a layout has to decide with belongs inside it.
    //
    // `parent` is a number, `false`, or absent; see `Parentage::to_value` for
    // why the middle one exists.
    //
    // `min` and `max` are here for the same reason (#115): `{ w, h }` in the
    // window's own terms, frame included, or absent when the application
    // limited neither side. `cramped` is the layout's own word coming back,
    // from `sol.place`. `app_id` is what `tiling.client_size_ignore` matches.
    // `shown` is whether the window's client has been shown yet; see
    // `WindowInfo::shown`. `fullscreen` and `style` are the two match keys of
    // an effect rule (`effects.rules`) nothing else here says, so every key a
    // rule matches a window by can be read from Lua; `maximized` beside
    // `fullscreen`, for a mode switch (#443). `x11_type`,
    // `accepts_input`, `class` and `instance` are read only from an X11
    // window's own properties (#221) -- `nil` for a Wayland one, except
    // `accepts_input`, which is `true` -- and exist for a window rule (#56) to
    // act on a case the compositor's own default (hiding a window whose
    // `class` or `instance` is in `config.x11.hidden`) does not already decide
    // for it. `content_type` is what the window says it is showing (#71),
    // `"none"` when it said nothing; see `WindowInfo::content_type`. `pid` is
    // the process the window belongs to, absent when it cannot be told; see
    // `WindowInfo::pid`.
    sol.set(
        "windows",
        lua.create_function(|lua, ()| {
            let snapshot = snapshot(lua)?;
            let windows = lua.create_table()?;
            for (index, window) in snapshot.windows.iter().enumerate() {
                let entry = lua.create_table()?;
                entry.set("id", window.id)?;
                entry.set("x", window.rect.x)?;
                entry.set("y", window.rect.y)?;
                entry.set("w", window.rect.w)?;
                entry.set("h", window.rect.h)?;
                let insets = lua.create_table()?;
                insets.set("top", window.insets.top)?;
                insets.set("right", window.insets.right)?;
                insets.set("bottom", window.insets.bottom)?;
                insets.set("left", window.insets.left)?;
                entry.set("insets", insets)?;
                entry.set("title", window.title.clone())?;
                entry.set("focused", window.focused)?;
                entry.set("monitor", window.monitor.clone())?;
                entry.set("modal", window.modal)?;
                entry.set("parent", lua::convert::parentage_value(window.parent, lua)?)?;
                entry.set("leaving", window.leaving)?;
                entry.set("minimized", window.minimized)?;
                entry.set("above", window.above)?;
                if let Some(icon) = window.icon {
                    let rect = lua.create_table()?;
                    rect.set("x", icon.x)?;
                    rect.set("y", icon.y)?;
                    rect.set("w", icon.w)?;
                    rect.set("h", icon.h)?;
                    entry.set("icon", rect)?;
                }
                entry.set("app_id", window.app_id.clone())?;
                entry.set("min", size_table(lua, window.min)?)?;
                entry.set("max", size_table(lua, window.max)?)?;
                entry.set("cramped", window.cramped)?;
                entry.set("shown", window.shown)?;
                entry.set("fullscreen", window.fullscreen)?;
                entry.set("maximized", window.maximized)?;
                entry.set("style", window.style.clone())?;
                entry.set("client_side", window.client_side)?;
                entry.set("blur_region", window.blur_region)?;
                entry.set("x11_type", window.x11_type.clone())?;
                entry.set("accepts_input", window.accepts_input)?;
                entry.set("class", window.class.clone())?;
                entry.set("instance", window.instance.clone())?;
                entry.set("content_type", window.content_type)?;
                entry.set("pid", window.pid)?;
                windows.set(index + 1, entry)?;
            }
            Ok(windows)
        })?,
    )?;

    // Installed, visible applications (03 §3.2.13), by name. A read, the
    // same shape as `sol.windows()` above -- and, like it, only meaningful
    // inside a dispatch: a `.lua` file's top level runs before the first
    // real snapshot arrives, so this reads empty there (QML that needs the
    // list at once reads `Solium.Apps`, live, instead).
    sol.set(
        "apps",
        lua.create_function(|lua, ()| {
            let snapshot = snapshot(lua)?;
            let apps = lua.create_table()?;
            for (index, app) in snapshot.apps.iter().enumerate() {
                let entry = lua.create_table()?;
                entry.set("id", app.id.clone())?;
                entry.set("name", app.name.clone())?;
                entry.set("generic_name", app.generic_name.clone())?;
                entry.set("icon", app.icon.clone())?;
                let categories = lua.create_table()?;
                for (n, category) in app.categories.iter().enumerate() {
                    categories.set(n + 1, category.clone())?;
                }
                entry.set("categories", categories)?;
                apps.set(index + 1, entry)?;
            }
            Ok(apps)
        })?,
    )?;

    // Anything the compositor should draw in QML: a wallpaper, a bar, a dock,
    // a heads-up display.
    //
    //     sol.surface("wallpaper", {
    //         scene = "wallpaper.qml",
    //         layer = "background",
    //         on    = "every-monitor",
    //         properties = { source = "…" },
    //     })
    //
    // `interactive = true` lets the pointer reach it: the scene calls
    // `Solium.send(action, data)`, and a `sol.on("surface", …)` handler is
    // told the surface's name, the action and its data, which is how the
    // tweaks panel works and how a bar's buttons would
    // (`tests::a_surface_action_reaches_lua_with_its_data`,
    // `state::tests::real_client::a_click_on_a_hosted_button_is_acted_on_at_its_release`).
    //
    // `reserve = { bottom = 48 }` takes those edges out of the work area of
    // every monitor it is on, whatever its size, and the scene's own
    // `Solium.surface.reserve` wins for an edge it sets
    // (`tests::sol_surface_reads_the_edges_it_reserves`,
    // `state::tests::real_client::reflow_on_close::hosted::a_scene_reserve_overrides_its_edge_and_reflows_the_layout_once`).
    //
    // `outside_click` is what a press outside an open `Grab` of its scene
    // does once it has dismissed it: `"swallow"`, the default, or `"pass"`
    // to also click what is under it, or a table naming grabs,
    // `{ default = "swallow", ["tray-menu"] = "pass" }`
    // (`tests::sol_surface_reads_outside_click_as_a_word_or_a_table`,
    // `state::tests::real_client::reflow_on_close::hosted::a_press_outside_a_grab_dismisses_it_and_is_swallowed_by_default`,
    // `state::tests::real_client::reflow_on_close::hosted::with_outside_click_pass_the_dismissing_press_reaches_the_window_under_it`).
    //
    // `keyboard = { bindings = "except_claimed" }` says which compositor
    // bindings still work while an item of its scene holds the keyboard:
    // every one but the keys the item claims (the default), `"all"`, or
    // `"none"`, which leaves the scene every key but the escape hatches
    // (`tests::sol_surface_reads_its_keyboard_bindings`,
    // `input::tests::a_claimed_key_reaches_the_scene_and_not_its_binding`,
    // `input::tests::with_bindings_none_even_super_bindings_reach_the_scene`).
    //
    // `sol.surface(name, false)` takes one away. Re-declaring the same name
    // changes the surface in place, writing what changed into its live scene,
    // and only a new scene file replaces it, so running the configuration
    // again keeps what the scene holds
    // (`scripted::tests::a_redeclared_property_is_written_into_the_live_scene`,
    // `scripted::tests::a_new_scene_path_rebuilds_the_surface`).
    //
    // This is the primitive the wallpaper used to be a special case of. See
    // `scripted.rs` for why it is worth having rather than a `Command` each.
    sol.set(
        "surface",
        lua.create_function(|lua, (name, options): (String, Value)| {
            let options = match options {
                Value::Table(options) => options,
                // `false` and `nil` both remove it, so a configuration that
                // stops declaring something and one that declares it off mean
                // the same thing.
                _ => {
                    with_pending(lua, |pending| {
                        pending.push(solium_wm_protocol::Command::SurfaceGone(name.clone()));
                    })?;
                    return Ok(Value::Nil);
                }
            };

            let scene: String = options.get("scene")?;
            let Some(scene) = crate::scripted::find_scene(&scene) else {
                // Named rather than ignored: a scene that is not there is a
                // typo, and a surface that silently does not appear is the
                // hardest kind of configuration mistake to find.
                tracing::error!(surface = name, scene, "no such QML scene");
                return Ok(Value::Nil);
            };

            let layer = options
                .get::<Option<String>>("layer")?
                .as_deref()
                .and_then(crate::scripted::Layer::parse)
                .unwrap_or_default();

            let on = match options.get::<Value>("on")? {
                Value::String(where_) => match where_.to_str()?.as_ref() {
                    "primary" => crate::scripted::On::Primary,
                    "every-monitor" => crate::scripted::On::EveryMonitor,
                    monitor => crate::scripted::On::Monitor(monitor.to_owned()),
                },
                Value::Table(rect) => match rect_from(&rect)? {
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "a rect from a script is screen-sized"
                    )]
                    Some(rect) => crate::scripted::On::Rect(smithay::utils::Rectangle::new(
                        (rect.x.round() as i32, rect.y.round() as i32).into(),
                        (
                            (rect.w.round() as i32).max(1),
                            (rect.h.round() as i32).max(1),
                        )
                            .into(),
                    )),
                    None => crate::scripted::On::EveryMonitor,
                },
                _ => crate::scripted::On::EveryMonitor,
            };

            let properties = match options.get::<Option<Table>>("properties")? {
                Some(table) => {
                    crate::scripted::Properties::new(crate::json::Json::object_from_lua(&table)?)
                }
                None => crate::scripted::Properties::default(),
            };
            let interactive = options.get::<Option<bool>>("interactive")?.unwrap_or(false);
            // `tests::sol_surface_reads_the_edges_it_reserves`,
            // `tests::a_negative_reserve_is_refused`.
            let reserve = match options.get::<Option<Table>>("reserve")? {
                Some(edges) => {
                    let edge = |name: &str| -> mlua::Result<i32> {
                        let value = edges.get::<Option<i32>>(name)?.unwrap_or(0);
                        if value < 0 {
                            return Err(mlua::Error::runtime(format!(
                                "reserve.{name} is {value}; a reserve is never negative"
                            )));
                        }
                        Ok(value)
                    };
                    crate::scripted::Edges {
                        top: edge("top")?,
                        right: edge("right")?,
                        bottom: edge("bottom")?,
                        left: edge("left")?,
                    }
                }
                None => crate::scripted::Edges::default(),
            };
            // `tests::sol_surface_reads_outside_click_as_a_word_or_a_table`,
            // `tests::an_unknown_outside_click_is_refused`.
            let outside = |word: &str| -> mlua::Result<crate::scripted::Outside> {
                match word {
                    "swallow" => Ok(crate::scripted::Outside::Swallow),
                    "pass" => Ok(crate::scripted::Outside::Pass),
                    other => Err(mlua::Error::runtime(format!(
                        "outside_click is \"swallow\" or \"pass\", not {other:?}"
                    ))),
                }
            };
            let outside_click = match options.get::<Value>("outside_click")? {
                Value::Nil => crate::scripted::OutsideClick::default(),
                Value::String(word) => crate::scripted::OutsideClick {
                    default: outside(&word.to_str()?)?,
                    named: std::collections::BTreeMap::new(),
                },
                Value::Table(table) => {
                    let mut policy = crate::scripted::OutsideClick::default();
                    for pair in table.pairs::<String, String>() {
                        let (name, word) = pair?;
                        if name == "default" {
                            policy.default = outside(&word)?;
                        } else {
                            policy.named.insert(name, outside(&word)?);
                        }
                    }
                    policy
                }
                _ => {
                    return Err(mlua::Error::runtime(
                        "outside_click is \"swallow\", \"pass\" or a table of them",
                    ));
                }
            };

            // `tests::sol_surface_reads_its_keyboard_bindings`,
            // `tests::an_unknown_keyboard_bindings_is_refused`.
            let keyboard = match options.get::<Option<Table>>("keyboard")? {
                Some(keyboard) => match keyboard.get::<Value>("bindings")? {
                    Value::Nil => crate::scripted::KeyPolicy::ExceptClaimed,
                    Value::String(word) => match word.to_str()?.as_ref() {
                        "except_claimed" => crate::scripted::KeyPolicy::ExceptClaimed,
                        "all" => crate::scripted::KeyPolicy::All,
                        "none" => crate::scripted::KeyPolicy::NoBindings,
                        other => {
                            return Err(mlua::Error::runtime(format!(
                                "keyboard.bindings is \"except_claimed\", \"all\" or \"none\", not {other:?}"
                            )));
                        }
                    },
                    _ => {
                        return Err(mlua::Error::runtime(
                            "keyboard.bindings is \"except_claimed\", \"all\" or \"none\"",
                        ));
                    }
                },
                None => crate::scripted::KeyPolicy::ExceptClaimed,
            };

            with_pending(lua, |pending| {
                pending.push(solium_wm_protocol::Command::Surface(to_wire::surface(
                    crate::scripted::Declaration {
                        name: name.clone(),
                        scene: scene.clone(),
                        layer,
                        on: on.clone(),
                        properties: properties.clone(),
                        interactive,
                        reserve,
                        outside_click: outside_click.clone(),
                        keyboard,
                    },
                )));
            })?;
            Ok(Value::Nil)
        })?,
    )?;

    // Hit-testing against *drawn* rects, not real ones: in a mode a window is
    // where the script put it, and asking the compositor is what keeps the
    // script from reimplementing the transform to find out.
    //
    // **And asked exactly as the compositor asks it**, through
    // `crate::state::owns` -- the predicate `Solium::window_under` walks with:
    // on a screen that draws the window, and inside what it paints there.
    // Until #134's fourth review this asked only whether the drawn rectangle
    // held the point. With two monitors side by side the left one's hidden
    // workspace is carried over the right one, so a click on empty space
    // there in overview found a window nobody could see and `sol.focus` gave
    // it the keyboard; a window fading out at opacity zero was found over the
    // neighbour that grew into its place, so a drop there went nowhere (#135).
    // `on_two_monitors_sol_window_at_answers_what_the_right_monitor_draws`,
    // `on_two_monitors_an_overview_click_on_the_right_monitor_leaves_the_keyboard_on_screen`
    // and `sol_window_at_over_a_window_fading_out_answers_the_neighbour_in_its_place`.
    sol.set(
        "window_at",
        lua.create_function(|lua, (x, y, skip): (f64, f64, Option<u64>)| {
            let snapshot = snapshot(lua)?;
            let point = Point::<f64, Logical>::from((x, y));
            // `skip` is what makes this usable while dragging. A dragged
            // window follows the cursor, so it is always the topmost thing
            // under it — ask without skipping and the answer is always the
            // window in your hand, which is why dropping one onto another
            // never swapped anything.
            //
            // `false`: no row of a snapshot is what is left of a window whose
            // client has gone -- `Solium::snapshot` lists none, which
            // `a_window_that_left_is_nobodys_to_find` asks.
            //
            // A minimized window is under no point, even while a present on
            // it plays out, as the press walks have it (#30):
            // `real_client::reflow_on_close::minimize::a_press_on_a_minimized_window_reaches_what_is_under_it`.
            Ok(snapshot
                .windows
                .iter()
                .find(|window| {
                    Some(window.id) != skip
                        && !window.minimized
                        && crate::state::owns(
                            window.drawn.slot,
                            false,
                            window.drawn.frame,
                            point,
                            &snapshot.screens,
                        )
                })
                .map(|window| window.id))
        })?,
    )?;

    // `sol.focus(id, { warp = ... })`: the window takes the keyboard, and
    // the pointer goes to it once it has (#220). `warp = true` is the centre
    // with the picture there at once; a table is `sol.warp`'s options. The
    // focus is told with what this handler is answering.
    // `tests::sol_focus_carries_its_source_and_its_warp`.
    sol.set(
        "focus",
        lua.create_function(|lua, (id, options): (u64, Option<Table>)| {
            let warp = match options.as_ref() {
                Some(options) => match options.get::<Value>("warp")? {
                    Value::Nil | Value::Boolean(false) => None,
                    Value::Boolean(true) => Some(Warp {
                        to: WarpTo::Window { id, at: CENTRE },
                        motion: None,
                        interrupt: Interrupt::default(),
                    }),
                    Value::Table(warp) => {
                        let (at, motion, interrupt) = warp_options(Some(&warp), "sol.focus")?;
                        Some(Warp {
                            to: WarpTo::Window {
                                id,
                                at: at.unwrap_or(CENTRE),
                            },
                            motion,
                            interrupt,
                        })
                    }
                    other => {
                        return Err(mlua::Error::runtime(format!(
                            "sol.focus: warp is true, false or a table, not {}",
                            other.type_name()
                        )));
                    }
                },
                None => None,
            };
            with_pending(lua, |pending| {
                let cause = to_wire::focus_cause(pending.source);
                pending.push(solium_wm_protocol::Command::Focus((
                    id,
                    cause,
                    warp.map(to_wire::warp),
                )));
            })
        })?,
    )?;

    // What the handler running now is answering, in the `focus` event's
    // words: what a `sol.focus` made here would be told with (#220). For a
    // policy that acts where no window takes the keyboard, such as a
    // workspace switch to an empty desk (`lua/workspaces.lua`).
    // `tests::the_focus_event_says_what_moved_the_keyboard`.
    sol.set(
        "focus_source",
        lua.create_function(|lua, ()| {
            Ok(lua
                .app_data_ref::<Pending>()
                .map_or(FocusSource::Script, |pending| pending.source)
                .name())
        })?,
    )?;

    // Fullscreen and maximised, from a script (#150). The window named, or
    // the focused one when none is, and nothing when nothing is focused: a
    // binding that names no window means the one being typed into.
    // `sol_toggles_name_the_focused_window_when_given_none`.
    sol.set(
        "toggle_fullscreen",
        lua.create_function(|lua, id: Option<u64>| {
            let Some(id) = id.or_else(|| focused_id(lua)) else {
                return Ok(());
            };
            with_pending(lua, |pending| {
                pending.push(solium_wm_protocol::Command::ToggleFullscreen(id));
            })
        })?,
    )?;

    // Minimize a window, and show it again (#30). The window named, or the
    // focused one when none is, as the toggles above. A minimized window
    // takes no focus, so `sol.unminimize()` with no id names nothing: it is
    // for a window a script can name, from `sol.windows()`.
    // `directions::sol_minimize_names_the_focused_window_when_given_none_and_reaches_past_the_lock`.
    sol.set(
        "minimize",
        lua.create_function(|lua, id: Option<u64>| {
            let Some(id) = id.or_else(|| focused_id(lua)) else {
                return Ok(());
            };
            with_pending(lua, |pending| {
                pending.push(solium_wm_protocol::Command::Minimize(id));
            })
        })?,
    )?;

    sol.set(
        "unminimize",
        lua.create_function(|lua, id: Option<u64>| {
            let Some(id) = id.or_else(|| focused_id(lua)) else {
                return Ok(());
            };
            with_pending(lua, |pending| {
                pending.push(solium_wm_protocol::Command::Unminimize(id));
            })
        })?,
    )?;

    // Show a minimized window anyway, where a present puts it, or stop (#30):
    // what an overview or a recents screen does to draw a minimized window
    // as one of its thumbnails. `shown` is `true` when left out. It changes
    // nothing for a window that is not minimized, and showing it again
    // forgets it. Still no pointer or touch reaches it: the script that
    // shows it knows where it put it.
    // `real_client::reflow_on_close::minimize::the_overview_shows_a_minimized_window_and_choosing_it_shows_it_again`.
    sol.set(
        "show_minimized",
        lua.create_function(|lua, (id, shown): (u64, Option<bool>)| {
            with_pending(lua, |pending| {
                pending.push(solium_wm_protocol::Command::ShowMinimized((
                    id,
                    shown.unwrap_or(true),
                )));
            })
        })?,
    )?;

    // Keep a window above every window that is not, or stop (#424): `on` is
    // `true`, `false` or `"toggle"`, and `true` when left out; the id is the
    // focused window's when it is left out, as the toggles' is. A window kept
    // above stays over the others in what is drawn and in what is clicked,
    // and a click or the keyboard raises a window only among its own kind. A
    // focused fullscreen window still covers it. Anything else for `on` is an
    // error naming what it was given.
    // `real_client::reflow_on_close::above::a_kept_above_window_stays_over_a_window_focused_after_it_and_takes_its_click`.
    sol.set(
        "keep_above",
        lua.create_function(|lua, (id, on): (Option<u64>, mlua::Value)| {
            let on = match on {
                mlua::Value::Nil | mlua::Value::Boolean(true) => Some(true),
                mlua::Value::Boolean(false) => Some(false),
                mlua::Value::String(word) if word.to_string_lossy() == "toggle" => None,
                other => {
                    return Err(mlua::Error::runtime(format!(
                        "sol.keep_above: `on` is true, false or \"toggle\", not {}",
                        other.type_name()
                    )));
                }
            };
            let Some(id) = id.or_else(|| focused_id(lua)) else {
                return Ok(());
            };
            with_pending(lua, |pending| {
                pending.push(solium_wm_protocol::Command::KeepAbove((id, on)));
            })
        })?,
    )?;

    sol.set(
        "toggle_maximize",
        lua.create_function(|lua, id: Option<u64>| {
            let Some(id) = id.or_else(|| focused_id(lua)) else {
                return Ok(());
            };
            with_pending(lua, |pending| {
                pending.push(solium_wm_protocol::Command::ToggleMaximize(id));
            })
        })?,
    )?;

    // Hold a window a move drag has where it is, or let it go on (#452).
    // While held, the pointer goes on and the `drag` listeners go on hearing
    // it; let go, the window moves on from where it stands. A window no drag
    // has is left alone.
    // `real_client::reflow_on_close::drag_primitives::a_held_window_stays_while_the_pointer_moves_and_goes_on_from_where_it_is`.
    sol.set(
        "drag_hold",
        lua.create_function(|lua, (id, held): (u64, bool)| {
            with_pending(lua, |pending| {
                pending.push(solium_wm_protocol::Command::DragHold((id, held)));
            })
        })?,
    )?;

    // Where the pointer holds a window a move drag has (#452): `{ x, y }`
    // as shares of its frame's width and height, and `{ dx, dy }` pixels
    // further, each 0 when left out, from the frame's top-left. The window
    // goes there at once and the drag goes on from there.
    // `real_client::reflow_on_close::drag_primitives::a_grip_puts_the_window_under_the_pointer_and_the_drag_goes_on_from_it`.
    sol.set(
        "drag_grip",
        lua.create_function(|lua, (id, grip): (u64, mlua::Table)| {
            let get = |key: &str| -> mlua::Result<f64> {
                Ok(grip.get::<Option<f64>>(key)?.unwrap_or(0.0))
            };
            let along = (get("x")?, get("y")?);
            let by = (get("dx")?, get("dy")?);
            if ![along.0, along.1, by.0, by.1]
                .iter()
                .all(|value| value.is_finite())
            {
                return Err(mlua::Error::RuntimeError(
                    "sol.drag_grip: x, y, dx and dy are numbers".to_owned(),
                ));
            }
            with_pending(lua, |pending| {
                pending.push(solium_wm_protocol::Command::DragGrip((id, along, by)));
            })
        })?,
    )?;

    // Keep a window a resize has as it is, or let the resize go on (#454):
    // the counterpart of `sol.drag_hold`. While held, the pointer goes on and
    // the window keeps its size and place, and the `resize` listeners are
    // offered nothing; let go, the resize goes on from where the pointer is
    // then. A window no resize has is left alone.
    // `real_client::reflow_on_close::resize_primitives::a_held_resize_keeps_the_window_and_goes_on_from_where_the_pointer_is`.
    sol.set(
        "resize_hold",
        lua.create_function(|lua, (id, held): (u64, bool)| {
            with_pending(lua, |pending| {
                pending.push(solium_wm_protocol::Command::ResizeHold((id, held)));
            })
        })?,
    )?;

    // Focus or move by direction (#150). The compositor knows no more than
    // the verb and the direction: it hands both to every `direction`
    // listener, and the layout in charge answers (`lua/direction.lua`). A
    // direction that is not one of the four is an error rather than nothing,
    // so a misspelt binding says so instead of being a key that does nothing.
    // `sol_direction_reaches_the_listeners_and_a_bad_one_is_an_error`.
    for (name, verb) in [("focus_direction", "focus"), ("move_direction", "move")] {
        sol.set(
            name,
            lua.create_function(move |lua, dir: String| {
                if !["left", "right", "up", "down"].contains(&dir.as_str()) {
                    return Err(mlua::Error::RuntimeError(format!(
                        "sol.{name}: {dir:?} is not a direction; it is left, right, up or down"
                    )));
                }
                // A move is of the focused window, and every placement of it
                // while the listeners run says so. See `Command::Place::moved`,
                // and `a_fullscreen_or_maximised_window_moved_to_another_monitor_is_so_on_that_one`.
                let moving = if verb == "move" {
                    focused_id(lua)
                } else {
                    None
                };
                let mut outer = None;
                // And what a `sol.focus` its listeners make is told with: a
                // direction or a move, whatever called it (#220).
                // `tests::the_focus_event_says_what_moved_the_keyboard`.
                let source = if verb == "move" {
                    FocusSource::Move
                } else {
                    FocusSource::Direction
                };
                let mut outer_source = FocusSource::Script;
                with_pending(lua, |pending| {
                    outer = pending.moving;
                    pending.moving = moving.or(outer);
                    outer_source = pending.source;
                    pending.source = source;
                })?;
                // The clock starts again for each `direction` listener, so
                // the handler that asked gets its own clock back, and is
                // stopped here once that has run out: a loop of these calls
                // is stopped however few of its instructions are its own.
                // `tests::a_binding_that_loops_on_focus_direction_is_stopped`.
                // The listeners' time counts against it, so one stopped at
                // the deadline stops it too.
                // `tests::a_direction_listener_stopped_at_the_deadline_stops_its_caller_too`.
                let started = deadline_started(lua);
                let heard = call_listeners(lua, "direction", (verb, dir));
                set_deadline(lua, started);
                with_pending(lua, |pending| {
                    pending.moving = outer;
                    pending.source = outer_source;
                })?;
                heard?;
                if late(lua) {
                    // Stopped at this call, which is where the log says it
                    // was stopped: a `direction` listener's clock cleared the
                    // place, or left that listener's own.
                    // `tests::a_handler_stopped_at_its_focus_direction_is_logged_at_that_call`.
                    let _ = lua.inspect_stack(1, |caller| stopped_here(lua, caller));
                    return Err(mlua::Error::runtime(STOPPED));
                }
                Ok(())
            })?,
        )?;
    }

    // `place` changes where a window *lives*; `present` changes where it is
    // *drawn*. A layout uses this one — and leaving a mode afterwards restores
    // it to wherever the layout has since put it, which is the correct answer
    // and comes out for free.
    //
    // The rect is a *tile* unless it says `tile = false`: the client is held
    // inside it, cut to it where it commits more than it has (#133). A
    // layout's arrangement wants that and gets it by default; a window a
    // layout places without tiling it — a dialog centred on its parent — says
    // so, and is drawn at whatever it commits.
    //
    // `tile` may instead be the tile itself, `{ x, y, w, h }`, when the window
    // is placed smaller than the tile it has -- `tiling.lua` centring a window
    // at its own maximum (#115). The window lives at the rect; the tile is
    // what holds its client and what an edge drag moves a seam from. See
    // `Command::Place::inside`.
    //
    // `cramped = true` is the layout saying this tile is smaller than the
    // window's own minimum, which `sol.windows()` reports back. Said afresh on
    // every placement, so a placement that does not say it is not cramped.
    sol.set(
        "place",
        lua.create_function(|lua, (id, options): (u64, Table)| {
            let Some(rect) = rect_from(&options)? else {
                return Err(mlua::Error::runtime("sol.place needs a rect"));
            };
            let (tile, inside) = match options.get::<Value>("tile")? {
                Value::Table(within) => match rect_from(&within)? {
                    Some(within) => (true, Some(within)),
                    None => {
                        return Err(mlua::Error::runtime(
                            "sol.place's tile is true, false, or the tile's { x, y, w, h }",
                        ));
                    }
                },
                Value::Boolean(false) => (false, None),
                _ => (true, None),
            };
            let cramped = options.get::<Option<bool>>("cramped")?.unwrap_or(false);
            with_pending(lua, |pending| {
                let animation = pending.animation;
                let moved = pending.moving == Some(id);
                pending.push(solium_wm_protocol::Command::Place(
                    solium_wm_protocol::Place {
                        id,
                        rect: to_wire::rect(rect),
                        motion: to_wire::motion(animation),
                        tile,
                        inside: inside.map(to_wire::rect),
                        cramped,
                        moved,
                    },
                ));
            })
        })?,
    )?;

    // The other half of a tile: let a window go from one. Without it the
    // compositor cannot know a layout has stopped — the mode is the script's —
    // and a window left in the tile it last had would go on being cut to that
    // tile after a switch to floating, the moment it committed anything larger.
    sol.set(
        "unplace",
        lua.create_function(|lua, id: u64| {
            with_pending(lua, |pending| {
                pending.push(solium_wm_protocol::Command::Unplace(id))
            })
        })?,
    )?;

    sol.set(
        "close",
        lua.create_function(|lua, id: u64| {
            with_pending(lua, |pending| {
                pending.push(solium_wm_protocol::Command::Close(id))
            })
        })?,
    )?;

    // `sol.act(action, data, done)`: one of the compositor's verbs, queued
    // like every write here (03 §3.3.2). The id it answers is one no earlier
    // `sol.act` answered, and `done(ok, reason)` runs once, after the command
    // is applied, in a dispatch of its own.
    // `tests::sol_act_returns_an_attempt_and_done_hears_the_outcome_once`,
    // `tests::each_sol_act_answers_an_id_of_its_own_counting_up`,
    // `state::tests::real_client::reflow_on_close::hosted::sol_act_tells_done_once_the_window_was_asked_to_close`.
    static ATTEMPTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    sol.set(
        "act",
        lua.create_function(
            |lua, (action, data, done): (String, Value, Option<mlua::Function>)| {
                let data = crate::json::Json::from_lua(&data)?.unwrap_or(crate::json::Json::Null);
                let attempt = ATTEMPTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                with_pending(lua, |pending| {
                    let cause = to_wire::focus_cause(pending.source);
                    pending.push(solium_wm_protocol::Command::Act((
                        attempt,
                        action,
                        data.render(),
                        cause,
                    )));
                })?;
                if let Some(done) = done {
                    let attempts: Table = lua.globals().get::<Table>("sol")?.get("_attempts")?;
                    attempts.set(attempt, done)?;
                }
                Ok(attempt)
            },
        )?,
    )?;

    Ok(())
}
