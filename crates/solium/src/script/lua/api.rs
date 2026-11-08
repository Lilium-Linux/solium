//! `sol`: the table of functions a configuration calls.

use std::time::Duration;

use mlua::{Lua, Table, Value};
use smithay::utils::{Logical, Point};

use crate::script::lua::*;
use crate::script::*;

/// Build the `sol` table.
pub(crate) fn build_api(lua: &Lua) -> mlua::Result<Table> {
    let sol = lua.create_table()?;
    sol.set("_bindings", lua.create_table()?)?;
    sol.set("_handlers", lua.create_table()?)?;
    // Each `sol.act`'s `done`, by attempt id, until its outcome is known.
    // `tests::sol_act_returns_an_attempt_and_done_hears_the_outcome_once`.
    sol.set("_attempts", lua.create_table()?)?;
    // `sol.notify`'s action functions, by token, until one runs (#346).
    sol.set("_notify_actions", lua.create_table()?)?;
    // Each `sol.run{}`'s `lines` and `done`, by run id, until it is done,
    // and each timer's function, by timer id, until it is cancelled or, for
    // `sol.after`, has run (#472). A reload starts with neither, so no
    // function of the old configuration runs into the new one.
    sol.set("_runs", lua.create_table()?)?;
    sol.set("_timers", lua.create_table()?)?;
    // Each `sol.read`'s and `sol.write`'s function, by id, until it is
    // told; each watch's, by id, until it is cancelled; and each store's
    // table, by name (#473). A reload starts with none of the functions, so
    // none of the old configuration's runs into the new one; the stores
    // come back through the keep.
    sol.set("_files", lua.create_table()?)?;
    sol.set("_watches", lua.create_table()?)?;
    sol.set("_stores", lua.create_table()?)?;
    sol.set("_keeps", lua.create_table()?)?;
    // How many times each listener or `done` was stopped at the deadline, by
    // function. `tests::a_listener_stopped_three_times_is_taken_out`,
    // `tests::the_stops_are_counted_by_function_across_events`,
    // `tests::a_done_stopped_three_times_is_not_called_again`.
    sol.set("_strikes", lua.create_table()?)?;
    // Where a binding came from, for the combinations a script chose to say.
    // Keyed the same way `_bindings` is -- the canonical spelling -- so the two
    // can be read together, and holding entries for combinations `_bindings`
    // does *not* have, which is how `sol.unbind` reports a shipped binding
    // somebody deliberately took away. See `Scripts::bindings`.
    sol.set("_binding_sources", lua.create_table()?)?;
    // What each binding asked for beyond its handler, keyed as `_bindings` is,
    // and only for one that asked for something. See `sol.bind` and
    // `Scripts::binding_options`.
    sol.set("_binding_options", lua.create_table()?)?;
    // Settings a configuration wrote that the defaults do not define. Filled by
    // `lua/config.lua`, whose `merge` is the only thing that knows both halves
    // of that comparison, and read by `solium --check`. See `sol.unknown`.
    sol.set("_unknown_settings", lua.create_table()?)?;

    // State that outlives `super+shift+r`.
    //
    //     local state = sol.keep("workspaces", { showing = {}, of = {} })
    //
    // Answers the table the last configuration had under this name, or the
    // defaults on the first load. Mutate it in place and the next reload gets
    // what you left in it; see [`Kept`] for what may be in one -- plain data,
    // and the trees and strips `sol.layout` makes, which is how `tiling.lua`
    // and `scrolling.lua` keep their arrangement (#118).
    //
    // **Why this is the host's job and not a script's.** Two shipped scripts
    // had already invented an answer to surviving a reload, and neither could
    // have got it right, because only the host knows when the Lua state dies.
    // `workspaces.lua` reasoned about the *compositor's* lifetime instead —
    // a selection outlives a reload, so it swept sixteen desk names once per
    // session to clear ones a shorter arrangement had abandoned. `modes.lua`
    // reasoned about nothing at all: it declared `current = "floating"` at its
    // top level, which is true at startup and a lie after every reload, with
    // every window still sitting in the tile a layout put it in.
    //
    // Those are the same defect. A script cannot see the seam it is being cut
    // at, so the seam is where the mechanism belongs.
    sol.set(
        "keep",
        lua.create_function(|lua, (name, defaults): (String, Table)| {
            let sol: Table = lua.globals().get("sol")?;
            let keeps: Table = sol.get("_keeps")?;
            // Asked for twice under one name — two scripts sharing it, or one
            // module required from two places — is the same table both times.
            // Handing out a second would make whichever was harvested last the
            // only one kept, which is a loss nothing would report.
            if let Value::Table(already) = keeps.get::<Value>(name.as_str())? {
                return Ok(already);
            }
            // Cloned out, and the borrow of the app data dropped, before any
            // table is built: `into_value` calls back into Lua.
            let carried = lua
                .app_data_ref::<Carried>()
                .and_then(|carried| carried.0.keeps.get(&name).cloned());
            let table = match carried {
                // Only a table is claimable. A keep that came back as a number
                // would mean the script changed shape between reloads, and the
                // defaults it just passed are the better answer.
                Some(kept @ Kept::Table(_)) => match kept.into_value(lua)? {
                    Value::Table(table) => table,
                    _ => defaults,
                },
                _ => defaults,
            };
            keeps.set(name, &table)?;
            Ok(table)
        })?,
    )?;

    // What is wrong in an effect, a rule or the configuration, with its file
    // and line: `tests::a_broken_effect_reaches_sol_problems_with_its_file_and_line`.
    sol.set(
        "problems",
        lua.create_function(|lua, ()| {
            let snapshot = snapshot(lua)?;
            let rows = lua.create_table()?;
            for (index, problem) in snapshot.problems.iter().enumerate() {
                let row = lua.create_table()?;
                row.set("effect", problem.effect.clone())?;
                row.set("file", problem.file.clone())?;
                row.set("line", problem.line)?;
                row.set("column", problem.column)?;
                row.set("message", problem.message.clone())?;
                row.set("severity", problem.severity)?;
                rows.set(index + 1, row)?;
            }
            Ok(rows)
        })?,
    )?;

    // Effects on parts of windows: `sol.effects{ rules = { … } }`, parsed
    // here and applied whole, or not at all
    // (`state::tests::a_broken_rule_keeps_the_rules_that_ran`), with the
    // engine's own keys beside them (Ruling 28,
    // `tests::sol_effects_reaches_the_compositor_with_its_settings`).
    sol.set(
        "effects",
        lua.create_function(|lua, options: Option<Table>| {
            let given = options.is_some();
            let (rules, settings) =
                settings::effects::parse(&lua::convert::section(options)?, given);
            with_pending(lua, |pending| {
                pending.commands.push(Command::Effects { rules, settings });
            })
        })?,
    )?;

    // How the compositor starts: the first desktop frame's gate
    // (`warm::Warm`), `startup.first_frame`, read and checked here and
    // applied at once (`tests::sol_startup_reaches_the_compositor`).
    sol.set(
        "startup",
        lua.create_function(|lua, options: Option<Table>| {
            let read = match options {
                None => Ok(crate::warm::FirstFrame::default()),
                Some(table) => crate::warm::startup(&lua::convert::doc_of(&Value::Table(table))?),
            };
            with_pending(lua, |pending| {
                pending.commands.push(Command::Startup(read));
            })
        })?,
    )?;

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
                        pending.commands.push(Command::SurfaceGone(name.clone()));
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
                pending
                    .commands
                    .push(Command::Surface(Box::new(crate::scripted::Declaration {
                        name: name.clone(),
                        scene: scene.clone(),
                        layer,
                        on: on.clone(),
                        properties: properties.clone(),
                        interactive,
                        reserve,
                        outside_click: outside_click.clone(),
                        keyboard,
                    })));
            })?;
            Ok(Value::Nil)
        })?,
    )?;

    // The keyboard: which layouts exist, which is live, how keys repeat.
    //
    // Reads with no argument and configures with a table, the same shape as
    // `sol.monitors` below. Switching layout is `sol.keyboard{ active = 2 }`
    // rather than a second function, because "which layout is live" is a
    // property of the keyboard and not a different subject.
    //
    // Every key is optional and an absent one is left alone -- so a binding
    // that switches layout does not quietly reset the repeat rate somebody
    // configured, and a configuration that says nothing about the keyboard
    // leaves the `XKB_DEFAULT_*` environment in force.
    sol.set(
        "keyboard",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            let Some(options) = options else {
                let snapshot = snapshot(lua)?;
                let keyboard = snapshot.keyboard;
                let table = lua.create_table()?;
                table.set("shortcuts_inhibited", snapshot.shortcuts_owner.is_some())?;
                table.set("shortcuts_owner", snapshot.shortcuts_owner)?;
                let layouts = lua.create_table()?;
                for (index, name) in keyboard.layouts.iter().enumerate() {
                    layouts.set(index + 1, name.clone())?;
                }
                table.set("layouts", layouts)?;
                table.set("active", keyboard.active)?;
                // The live layout's names, as `layout_name` and `layout_short`
                // and not as `layout`, which is the xkb names a keymap is
                // compiled from: a table read here and handed back to
                // `sol.keyboard{ ... }` compiles no keymap.
                // `tests::sol_keyboard_handed_back_what_it_read_compiles_no_keymap`,
                // `keyboard_change::tests::a_layout_switch_and_a_caps_toggle_by_key_are_told_once_each_with_russian_active`.
                table.set("layout_name", keyboard.layout())?;
                table.set("layout_short", keyboard.short_name())?;
                table.set("caps", keyboard.caps)?;
                table.set("num", keyboard.num)?;
                table.set("repeat_rate", keyboard.repeat_rate)?;
                table.set("repeat_delay", keyboard.repeat_delay)?;
                return Ok(Value::Table(table));
            };

            let request = settings::keyboard::parse(&lua::convert::section(Some(options))?)
                .map_err(mlua::Error::runtime)?;
            if request == crate::keymap::Request::default() {
                return Ok(Value::Nil);
            }
            with_pending(lua, |pending| {
                pending.commands.push(Command::Keyboard(request.clone()));
            })?;
            Ok(Value::Nil)
        })?,
    )?;

    // The focused text field: the window it is in, whether that window is
    // framed, and its caret in the global space once the client has said where
    // that is. Nothing when no window has a text field enabled.
    // `text_input::tests::an_enabled_field_has_its_caret_in_the_global_space`,
    // `text_input::tests::a_field_says_whether_its_window_is_framed`.
    sol.set(
        "text_input",
        lua.create_function(|lua, ()| {
            let Some(field) = snapshot(lua)?.text_input else {
                return Ok(Value::Nil);
            };
            let table = lua.create_table()?;
            table.set("window", field.window)?;
            table.set("framed", field.framed)?;
            if let Some(caret) = field.caret {
                table.set("x", caret.loc.x)?;
                table.set("y", caret.loc.y)?;
                table.set("w", caret.size.w)?;
                table.set("h", caret.size.h)?;
            }
            Ok(Value::Table(table))
        })?,
    )?;

    // Reads with no argument, configures with a table. One name for one
    // subject: `sol.monitors()` asks where the monitors are, `sol.monitors{…}`
    // says. The same shape as `sol.monitor()`, which answers about one.
    sol.set(
        "monitors",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            let Some(rows) = options else {
                let monitors = lua.create_table()?;
                for (index, monitor) in snapshot(lua)?.monitors.iter().enumerate() {
                    let entry = lua::convert::rect_table(monitor.area, lua)?;
                    entry.set("name", monitor.name.clone())?;
                    entry.set("scale", monitor.scale)?;
                    entry.set("focused", monitor.focused)?;
                    entry.set("primary", monitor.primary)?;
                    entry.set("transform", monitor.transform.clone())?;
                    // Read, never guessed at from anything else: an off
                    // monitor is still here, the same size, in the same place.
                    // `sol_monitor_power_turns_a_monitor_off_and_on_and_says_so`.
                    entry.set("power", if monitor.off { "off" } else { "on" })?;
                    // The whole monitor as well as the usable part: a
                    // wallpaper and a fullscreen window want the one a bar has
                    // not taken a bite out of.
                    entry.set("whole", lua::convert::rect_table(monitor.whole, lua)?)?;
                    monitors.set(index + 1, entry)?;
                }
                return Ok(Value::Table(monitors));
            };

            let places = settings::monitors::parse(&lua::convert::doc_of(&Value::Table(rows))?)
                .map_err(mlua::Error::runtime)?;
            with_pending(lua, |pending| {
                pending
                    .commands
                    .push(Command::Monitors(crate::monitor::Arrangement::new(
                        places.clone(),
                    )));
            })?;
            Ok(Value::Nil)
        })?,
    )?;

    // Turn a monitor off or on: `sol.monitor_power("DP-1", "off")`, or `"all"`
    // for every one. For a binding -- none ships, since which key turns the
    // screens off is a matter of taste and of which keyboard. The monitor
    // stays where it is and keeps its windows; `sol.monitors()` says which
    // are off. Any key, click or motion turns them all back on. See
    // `power.rs`.
    //
    // A mode that is not `"on"` or `"off"` is an error rather than a guess:
    // this runs from a binding, and a binding that silently did nothing is
    // one nobody can debug. A name no monitor has is a line in the log.
    // `sol_monitor_power_turns_a_monitor_off_and_on_and_says_so`.
    sol.set(
        "monitor_power",
        lua.create_function(|lua, (which, mode): (String, String)| {
            let on = match mode.as_str() {
                "on" => true,
                "off" => false,
                other => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "sol.monitor_power: the mode is \"on\" or \"off\", not {other:?}"
                    )));
                }
            };
            let monitor = (which != "all").then_some(which);
            with_pending(lua, |pending| {
                pending.commands.push(Command::Power { monitor, on });
            })
        })?,
    )?;

    // The idle lock, the idle blank and what a window on a dark screen is
    // told: `config.idle`, handed over by `init.lua`. `lock_after` and
    // `screens_off_after` are in seconds and `off_frame_interval` in
    // milliseconds; 0 is "never" for each. An absent table or key keeps the default, for the reason
    // `sol.resize` gives, and a value that is not a number of zero or more is
    // named in the log and the default kept.
    // `the_idle_blank_turns_every_screen_off_after_the_timeout`.
    // `dbus_inhibit` is true or false, and anything else is named and the
    // default kept: `idle_dbus_inhibit_false_owns_nothing`.
    sol.set(
        "idle",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            let idle = settings::idle::parse(&lua::convert::section(options)?);
            with_pending(lua, |pending| {
                pending.commands.push(Command::Idle(idle));
            })
        })?,
    )?;

    // Notifications (03 §3.2.11, #346). `sol.notifications{ serve = true,
    // timeouts = { low = 5000, normal = 7000, critical = 0 }, history = { max
    // = 200 } }` sets them, each key left out keeping its default and one of
    // the wrong kind named in the log; a timeout of 0 is never. With no
    // table, `sol.notifications()` answers what they are: `available`,
    // `serving`, `dnd`, `owner`, `banners` and `history`, newest first.
    // `tests::sol_notifications_sets_the_service_and_answers_its_rows`.
    sol.set(
        "notifications",
        lua.create_function(|lua, options: Option<mlua::Table>| -> mlua::Result<Value> {
            let Some(options) = options else {
                let snapshot = snapshot(lua)?;
                let view = &snapshot.notifications;
                let table = lua.create_table()?;
                table.set("available", view.available)?;
                table.set("serving", view.serving)?;
                table.set("dnd", view.dnd)?;
                for (key, rows) in [("banners", &view.banners), ("history", &view.history)] {
                    let list = lua.create_table()?;
                    for (index, row) in rows.iter().enumerate() {
                        list.set(
                            index + 1,
                            crate::services::notifications::object(row).to_lua(lua)?,
                        )?;
                    }
                    table.set(key, list)?;
                }
                if let Some(owner) = &view.owner {
                    let row = lua.create_table()?;
                    row.set("name", owner.name.clone())?;
                    row.set("pid", owner.pid)?;
                    row.set("program", owner.program.clone())?;
                    table.set("owner", row)?;
                }
                return Ok(Value::Table(table));
            };
            let config = settings::services::notifications(&lua::convert::section(Some(options))?);
            with_pending(lua, |pending| {
                pending.commands.push(Command::Notifications(config));
            })?;
            Ok(Value::Nil)
        })?,
    )?;

    // The tray (#363). `sol.tray{ serve = true }` makes Solium the session's
    // `org.kde.StatusNotifierWatcher`, so applications' tray icons come to
    // it; with no table, `sol.tray()` answers what it holds: `available`,
    // `serving`, `owner` and `items`, in the order they first answered. Its
    // actions go through `sol.act`: `tray.activate { id, x, y }` and the
    // rest.
    // `tests::sol_tray_sets_the_service_and_answers_its_items`.
    sol.set(
        "tray",
        lua.create_function(|lua, options: Option<mlua::Table>| -> mlua::Result<Value> {
            let Some(options) = options else {
                let snapshot = snapshot(lua)?;
                let view = &snapshot.tray;
                let table = lua.create_table()?;
                table.set("available", view.available)?;
                table.set("serving", view.serving)?;
                let items = lua.create_table()?;
                for (index, row) in view.items.iter().enumerate() {
                    items.set(
                        index + 1,
                        crate::services::notifications::object(row).to_lua(lua)?,
                    )?;
                }
                table.set("items", items)?;
                if let Some(owner) = &view.owner {
                    let row = lua.create_table()?;
                    row.set("name", owner.name.clone())?;
                    row.set("pid", owner.pid)?;
                    row.set("program", owner.program.clone())?;
                    table.set("owner", row)?;
                }
                return Ok(Value::Table(table));
            };
            let config = settings::services::tray(&lua::convert::section(Some(options))?);
            with_pending(lua, |pending| {
                pending.commands.push(Command::Tray(config));
            })?;
            Ok(Value::Nil)
        })?,
    )?;

    // Media (#379). `sol.media{ watch = true }` follows the session's MPRIS
    // players; with no table, `sol.media()` answers what it holds:
    // `available`, `players` in the order they appeared, and `active`, each
    // with its `position` where it is now. Its actions go through `sol.act`:
    // `media.play_pause` and the rest, on the active player with no `id`.
    // `tests::sol_media_sets_the_service_and_answers_its_players`.
    sol.set(
        "media",
        lua.create_function(|lua, options: Option<mlua::Table>| -> mlua::Result<Value> {
            let Some(options) = options else {
                let snapshot = snapshot(lua)?;
                return snapshot.media.to_json().to_lua(lua);
            };
            let config = settings::services::media(&lua::convert::section(Some(options))?);
            with_pending(lua, |pending| {
                pending.commands.push(Command::Media(config));
            })?;
            Ok(Value::Nil)
        })?,
    )?;

    // Audio (03 §3.2.5, #347). `sol.audio{ max = 1.0, step = 0.05 }` sets its
    // limits; `sol.audio()` answers what it is: `available`, `ready`,
    // `output`, `input`, `outputs` and `inputs`, every application's playback
    // stream as `streams`, and each window's streams as one, by the window's
    // id, as `windows` (#416). Either starts it. Volume and mute go through
    // `sol.act`: `audio.volume { target | window, value | by }`,
    // `audio.mute { target | window, value }`, `audio.default { endpoint }`.
    // `tests::sol_audio_sets_its_limits_and_answers_its_endpoints`.
    sol.set(
        "audio",
        lua.create_function(|lua, options: Option<mlua::Table>| -> mlua::Result<Value> {
            let Some(options) = options else {
                with_pending(lua, |pending| pending.commands.push(Command::Audio(None)))?;
                let snapshot = snapshot(lua)?;
                let view = &snapshot.audio;
                let table = lua.create_table()?;
                table.set("available", view.available)?;
                table.set("ready", view.ready)?;
                for (key, row) in [("output", &view.output), ("input", &view.input)] {
                    if let Some(row) = row {
                        table.set(
                            key,
                            crate::services::notifications::object(row).to_lua(lua)?,
                        )?;
                    }
                }
                for (key, rows) in [
                    ("outputs", &view.outputs),
                    ("inputs", &view.inputs),
                    ("streams", &view.streams),
                ] {
                    let list = lua.create_table()?;
                    for (index, row) in rows.iter().enumerate() {
                        list.set(
                            index + 1,
                            crate::services::notifications::object(row).to_lua(lua)?,
                        )?;
                    }
                    table.set(key, list)?;
                }
                let windows = lua.create_table()?;
                for (id, row) in &view.windows {
                    windows.set(
                        *id,
                        crate::services::notifications::object(row).to_lua(lua)?,
                    )?;
                }
                table.set("windows", windows)?;
                return Ok(Value::Table(table));
            };
            let settings = settings::services::audio(&lua::convert::section(Some(options))?);
            with_pending(lua, |pending| {
                pending.commands.push(Command::Audio(Some(settings)))
            })?;
            Ok(Value::Nil)
        })?,
    )?;

    // `sol.notify{ summary, body, icon, urgency, timeout, actions = { { key,
    // label, fn } } }`: one of the shell's own notifications, with no bus
    // behind it, through the same decision and model as any other (03
    // §3.2.11). An action's `fn` runs when it is invoked; `key = "default"`
    // is a click on the body.
    // `tests::sol_notify_queues_one_of_the_shells_own_with_its_actions`.
    static NOTIFY_TOKENS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    sol.set(
        "notify",
        lua.create_function(|lua, options: mlua::Table| {
            let text = |key: &str| -> String {
                match options.get::<Value>(key) {
                    Ok(Value::String(text)) => text.to_string_lossy(),
                    _ => String::new(),
                }
            };
            let summary = text("summary");
            if summary.is_empty() {
                return Err(mlua::Error::runtime("sol.notify needs a summary"));
            }
            let urgency = match text("urgency").as_str() {
                "low" => crate::services::notifications::model::Urgency::Low,
                "critical" => crate::services::notifications::model::Urgency::Critical,
                _ => crate::services::notifications::model::Urgency::Normal,
            };
            let expire_timeout = match options.get::<Value>("timeout") {
                Ok(value) => number(&value)
                    .filter(|ms| *ms >= 0.0 && *ms <= f64::from(i32::MAX))
                    .map_or(-1, |ms| ms as i32),
                Err(_) => -1,
            };
            let mut actions = Vec::new();
            let functions = lua.create_table()?;
            if let Ok(Value::Table(list)) = options.get::<Value>("actions") {
                for entry in list.sequence_values::<mlua::Table>() {
                    let entry = entry?;
                    let key: String = entry.get::<Option<String>>("key")?.unwrap_or_default();
                    if key.is_empty() {
                        continue;
                    }
                    let label: String = entry.get::<Option<String>>("label")?.unwrap_or_default();
                    if let Ok(Value::Function(run)) = entry.get::<Value>("fn") {
                        functions.set(key.clone(), run)?;
                    }
                    actions.push((key, label));
                }
            }
            let token = if functions.is_empty() {
                0
            } else {
                let token = NOTIFY_TOKENS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                let sol: Table = lua.globals().get("sol")?;
                let kept: Table = sol.get("_notify_actions")?;
                kept.set(token, functions)?;
                token
            };
            let own = crate::services::notifications::Own {
                summary,
                body: text("body"),
                icon: text("icon"),
                urgency,
                actions,
                token,
                expire_timeout,
            };
            with_pending(lua, |pending| pending.commands.push(Command::Notify(own)))
        })?,
    )?;
    // Automatic reload (#223): `config.reload`, handed over by `init.lua`.
    // `automatic` is true or false, default true; `quiet_ms` is milliseconds
    // of zero or more, default 300 -- long enough that an editor's own
    // "write a temp file, rename it over the original" lands in one quiet
    // period, short enough that a single save still feels instant. Either
    // left out, or of the wrong kind, keeps the default, named in the log --
    // the same contract every other `sol.<setting>` here keeps. Applied at
    // once: `state/commands.rs`'s `configure_autoreload` arms or disarms the
    // watch to match, so toggling this from a binding of your own works
    // mid-session and not only from `config.lua`.
    sol.set(
        "auto_reload",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            let settings = settings::reload::parse(&lua::convert::section(options)?);
            with_pending(lua, |pending| {
                pending.commands.push(Command::AutoReload(settings));
            })
        })?,
    )?;

    // libinput device settings (#157): a default per device type, and
    // overrides matched by name or by vendor/product. `config.lua`'s `input`
    // section, handed over as `sol.input(config.input)`.
    //
    // Each device-type key -- `touchpad`, `mouse`, `keyboard`, `touchscreen`,
    // `tablet_tool`, `tablet_pad`, `switch` -- is a table of the options
    // listed on `crate::input::devices::Settings`; left out, or not a table,
    // every device of that type keeps libinput's own default. `devices` is a
    // list of overrides, each a match -- `name` (a case-insensitive
    // substring), `vendor` and `product` (as `libinput list-devices` or
    // `lsusb` print them), at least one of the three -- plus any of the same
    // options. Overrides apply in list order, over the type default and over
    // each other, field by field: a later one changes only the fields it
    // names, and a field none of them names keeps the type default. Applied
    // to every device already connected and to each one `DeviceAdded` hands
    // over later, and reapplied, from whatever the configuration now says,
    // on every reload -- the live side of this is `tty.rs` and
    // `crate::input::devices::Registry`, since only a backend has real
    // devices to apply it to.
    // `the_shipped_configuration_configures_a_touchpad_and_a_mouse`,
    // `a_devices_entry_in_user_lua_overrides_one_field_of_the_matched_device`,
    // `sol_input_warns_on_an_unrecognised_option_value_and_keeps_it_unset`.
    sol.set(
        "input",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            let (config, shortcuts) = settings::input::parse(&lua::convert::section(options)?)
                .map_err(mlua::Error::runtime)?;
            with_pending(lua, |pending| {
                pending.commands.push(Command::Input(config));
                pending.commands.push(Command::ShortcutsInhibit(shortcuts));
            })
        })?,
    )?;

    // logind's `Lock` and sleep signals (#153): `config.lock`, handed over by
    // `init.lua`. `command` is a program and its arguments, split on
    // whitespace with no quoting, like an autostart `Exec=` line; absent or
    // not a string, nothing runs on `Lock` or before sleep, which is the
    // default — see `logind.rs` for why. `before_sleep` and `logind_unlock`
    // are true or false, default true; anything else is named in the log and
    // the default kept.
    // The rest is how Solium's own lock checks a password (#281),
    // `lock_auth_settings`, and what it shows, `lock_scene_settings`.
    sol.set(
        "lock",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            let (settings, auth, scene) = settings::lock::parse(&lua::convert::section(options)?);
            with_pending(lua, |pending| {
                pending.commands.push(Command::Lock(settings));
                pending.commands.push(Command::LockAuth(auth));
                pending.commands.push(Command::LockScene(scene));
            })
        })?,
    )?;

    // Which of a client's layer surfaces below the windows take the pointer
    // and touch (#461): `config.layer_shell`, handed over by `init.lua`.
    // `input_below` is `true` (every one), `false` (none), or a list of
    // namespaces, each exact, `*`, or a prefix ending in `*`. Anything else
    // keeps the default, every one, and says so, the same as `sol.x11` keeps
    // its list -- see `layer::Settings::default`.
    // `tests::sol_layer_shell_reads_input_below_as_a_flag_or_a_list_of_namespaces`.
    sol.set(
        "layer_shell",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            let settings = settings::layer_shell::parse(&lua::convert::section(options)?);
            with_pending(lua, |pending| {
                pending.commands.push(Command::LayerShell(settings));
            })
        })?,
    )?;

    // Who draws a Wayland window's frame (#462): `config.frames`, handed over
    // by `init.lua`. `policy` is "prefer", "force" or "allow"; `rules` is a
    // list of `{ match = { app_id = ... }, policy = ... }`, each `app_id`
    // exact, `*`, or a prefix ending in `*`, the last matching one winning.
    // Anything else is refused whole and the default kept, with the reason
    // logged: half a policy is a frame on the windows nobody meant.
    // `tests::sol_frames_reads_the_policy_and_its_rules`.
    sol.set(
        "frames",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            let settings = match settings::frames::parse(&lua::convert::section(options)?) {
                Ok(settings) => settings,
                Err(reason) => {
                    tracing::warn!(reason, "frames: keeping the default");
                    crate::framing::Settings::default()
                }
            };
            with_pending(lua, |pending| {
                pending.commands.push(Command::Frames(settings));
            })
        })?,
    )?;

    // Which window protocols a shell that is its own program is offered
    // (#50): `config.toplevels`, handed over by `init.lua`. `list` and
    // `manage` are each true or false; one left out, or not a boolean, keeps
    // its default, true, and a wrong type says so. `capture` (#47) is
    // `"client"`, `"pane"` or `false`, and anything else keeps `"client"`.
    // `tests::sol_toplevels_reads_each_protocol_as_a_flag`.
    sol.set(
        "toplevels",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            let settings = settings::toplevels::parse(&lua::convert::section(options)?);
            with_pending(lua, |pending| {
                pending.commands.push(Command::Toplevels(settings));
            })
        })?,
    )?;

    // Which `WM_CLASS` names are refused a tile, decoration or bar entry
    // outright (#221): `config.x11`, handed over by `init.lua`. `hidden` is a
    // list of strings, matched case-insensitively against either `WM_CLASS`
    // field (`X11Surface::class()` or `X11Surface::instance()`); absent, or
    // not a table of strings, keeps the default list rather than emptying
    // it, the same as `sol.resize` and `sol.idle` keep theirs — see
    // `xwayland::Settings::default`.
    sol.set(
        "x11",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            let settings = settings::x11::parse(&lua::convert::section(options)?);
            with_pending(lua, |pending| {
                pending.commands.push(Command::X11(settings));
            })
        })?,
    )?;

    // The input profile's focus policy, overridden from Lua (#219):
    // `config.focus`, which `lua/modes.lua` hands over on every mode change
    // as well as at load, same as `sol.idle` above. Unlike `sol.idle`, a key
    // left out -- or the whole table left out -- is not a fixed default but
    // the machine's own answer, from its form factor (`SOLIUM_FORM_FACTOR`,
    // #159), which the compositor works out when it applies the command. And
    // never the answer the call before left, which is what a mode naming
    // nothing used to inherit (#128): see `Command::FocusMode`.
    // `sol_focus_mode_leaves_a_key_out_to_the_machine`.
    sol.set(
        "focus_mode",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            let settings::focus::FocusMode {
                click,
                follow,
                clear_on_empty_click,
                after_close,
            } = settings::focus::parse(&lua::convert::section(options)?);
            with_pending(lua, |pending| {
                pending.commands.push(Command::FocusMode {
                    click,
                    follow,
                    clear_on_empty_click,
                    after_close,
                });
            })
        })?,
    )?;

    // The monitor a window is on, or the active one when asked about nothing.
    //
    // Both answers are a work-area rect, so every script written when there
    // was one monitor still reads correctly: `sol.monitor()` meant "the screen"
    // and still does — it is just no longer the only one.
    sol.set(
        "monitor",
        lua.create_function(|lua, id: Option<u64>| {
            let snapshot = snapshot(lua)?;
            let named = id
                .and_then(|id| snapshot.windows.iter().find(|window| window.id == id))
                .and_then(|window| {
                    snapshot
                        .monitors
                        .iter()
                        .find(|monitor| monitor.name == window.monitor)
                })
                .map(|monitor| monitor.area);
            lua::convert::rect_table(named.unwrap_or(snapshot.work_area), lua)
        })?,
    )?;

    sol.set(
        "cursor",
        lua.create_function(|lua, ()| {
            let (x, y) = snapshot(lua)?.cursor;
            let table = lua.create_table()?;
            table.set("x", x)?;
            table.set("y", y)?;
            Ok(table)
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

    sol.set(
        "present",
        lua.create_function(|lua, (id, options): (u64, Option<Table>)| {
            let (rect, opacity, client_opacity, matrix, deform) = match options.as_ref() {
                Some(options) => (
                    rect_from(options)?,
                    options.get::<Option<f32>>("opacity")?,
                    client_opacity_from(options, "sol.present")?,
                    transform_from(options)?,
                    deform_from(options)?,
                ),
                None => (None, None, None, None, None),
            };
            // Outside the match, because these two resolve to a value where
            // the five above resolve to "said nothing": `sol.present(id)` and
            // `sol.present(id, {})` have to produce the same depth and the
            // same pivot, and one function answering for both tables is how
            // the two answers cannot drift apart.
            let z = depth_from(options.as_ref())?;
            let pivot = pivot_from(options.as_ref())?;
            let motion = options
                .as_ref()
                .map(present_motion_from)
                .transpose()?
                .flatten();
            with_pending(lua, |pending| {
                let animation = pending.animation;
                let channels = resolved(motion, animation);
                pending.commands.push(Command::Present {
                    id,
                    rect,
                    opacity,
                    client_opacity,
                    matrix,
                    deform,
                    z,
                    pivot,
                    animation,
                    channels,
                });
            })
        })?,
    )?;

    // The other direction from `present`: this says where a window comes *from*
    // and lets it land where it belongs. A dock icon's rectangle here is the
    // macOS-style genie, and the compositor needs no idea that is what it is.
    sol.set(
        "present_from",
        lua.create_function(|lua, (id, options): (u64, Table)| {
            let Some(rect) = rect_from(&options)? else {
                return Err(mlua::Error::runtime(
                    "sol.present_from needs a rect to come from",
                ));
            };
            let opacity = options.get::<Option<f32>>("opacity")?;
            let client_opacity = client_opacity_from(&options, "sol.present_from")?;
            let motion = present_motion_from(&options)?;
            let deform = deform_from(&options)?;
            with_pending(lua, |pending| {
                let animation = pending.animation;
                let channels = resolved(motion, animation);
                pending.commands.push(Command::PresentFrom {
                    id,
                    rect,
                    opacity,
                    client_opacity,
                    animation,
                    channels,
                    deform,
                });
            })
        })?,
    )?;

    // Which QML file frames every window. A name is one of the decorations
    // that ship, or one of the user's own in ~/.config/solium/qml/decorations;
    // a path is anyone's. Takes effect immediately -- every frame is rebuilt.
    // Read the configuration again, in place. Bound to a key, this is the
    // difference between trying an idea and committing to it.
    // What the Developer Tweaks panel offers, and what to do when one is
    // pressed. Both live in Lua so the panel is a list of whatever the
    // configuration says rather than a menu built into the compositor.
    // Whether the compositor was started with `--debug-mode`.
    //
    // A script's gate rather than the compositor's: the Developer Tweaks panel
    // used to be gated in Rust, and now `lua/tweaks.lua` declines to declare
    // itself. Anything else that only belongs in a development session can do
    // the same.
    sol.set(
        "debug_mode",
        lua.create_function(|_, ()| Ok(crate::dev::debug_mode()))?,
    )?;

    sol.set(
        "reload",
        lua.create_function(|lua, ()| {
            with_pending(lua, |pending| pending.commands.push(Command::Reload))
        })?,
    )?;

    // Lock the session now, the way `config.lock` says: Solium's own lock, or
    // `lock.command`'s locker (#281). It takes nothing and only ever locks;
    // nothing in Lua unlocks (`no_lua_function_unlocks_a_native_lock`).
    // `sol_lock_now_locks_and_never_unlocks`.
    sol.set(
        "lock_now",
        lua.create_function(|lua, ()| {
            with_pending(lua, |pending| {
                pending.commands.push(Command::LockNow);
            })
        })?,
    )?;

    // Pull Solium's own lock scenes down over the unlocked desktop, following
    // a finger, as an iPhone's lock does: a number from 0 to 1 (held to
    // that), or `false` or nothing to let them go back up (#281). A picture
    // and nothing more: it never locks, never unlocks, and at a lock does
    // nothing; `sol.lock_now()` takes the lock from it. Anything else is an
    // error, and a number that is not one (NaN, an infinity) is ignored.
    // `state::tests::real_client::lock_focus::native::peek`,
    // `no_lua_function_unlocks_a_native_lock`.
    sol.set(
        "lock_peek",
        lua.create_function(|lua, progress: Value| {
            let progress = match progress {
                Value::Nil | Value::Boolean(false) => None,
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "a whole number of the way down is 0 or 1 once held"
                )]
                Value::Integer(number) => Some(number as f64),
                Value::Number(number) if number.is_finite() => Some(number),
                Value::Number(number) => {
                    tracing::warn!("sol.lock_peek takes a number from 0 to 1; ignoring {number}");
                    return Ok(());
                }
                other => {
                    return Err(mlua::Error::runtime(format!(
                        "sol.lock_peek takes a number from 0 to 1, or false, not {}",
                        describe(&other)
                    )));
                }
            };
            let progress = progress.map(|progress| progress.clamp(0.0, 1.0));
            with_pending(lua, |pending| {
                pending.commands.push(Command::LockPeek(progress));
            })
        })?,
    )?;

    // The shortcuts a window holds, taken back until the keyboard leaves it
    // (#58). See `shortcuts_inhibit`.
    sol.set(
        "take_shortcuts_back",
        lua.create_function(|lua, ()| {
            with_pending(lua, |pending| {
                pending.commands.push(Command::TakeShortcutsBack);
            })
        })?,
    )?;

    // The pointer a window holds, taken back until it is pressed on (#427).
    // See `state::pointer_hold`.
    sol.set(
        "release_pointer",
        lua.create_function(|lua, ()| {
            with_pending(lua, |pending| {
                pending.commands.push(Command::ReleasePointer);
            })
        })?,
    )?;

    sol.set(
        "pane",
        lua.create_function(|lua, name: Option<String>| {
            with_pending(lua, |pending| {
                pending.commands.push(Command::Decoration { name });
            })
        })?,
    )?;

    // How long a pass may spend building the frames of a new pane style, in
    // milliseconds (#226): `config.pane_build`, handed over by `init.lua`.
    // Nil says nothing; anything but a number of 0 or more is named in the
    // log and changes nothing. `tests::sol_pane_build_hands_the_budget_over`.
    sol.set(
        "pane_build",
        lua.create_function(|lua, millis: Value| {
            let Some(budget) = settings::pane::build_budget(&lua::convert::doc_of(&millis)?) else {
                return Ok(());
            };
            with_pending(lua, |pending| {
                pending.commands.push(Command::PaneBuild(budget));
            })
        })?,
    )?;

    // Values for every pane's layers, read there as one object, `values`: the
    // configuration's way to hand its own settings to the QML that draws by
    // them, whatever they are. Merged by key.
    // `decoration::tests::pane_values_merge_by_key_and_count_only_changes`.
    sol.set(
        "pane_values",
        lua.create_function(|lua, values: mlua::Table| {
            let fields = crate::json::Json::object_from_lua(&values)?;
            with_pending(lua, |pending| {
                pending.commands.push(Command::PaneValues(fields));
            })
        })?,
    )?;

    // Values for one window's frame alone, read there as `windowValues`
    // (#417): what the configuration keeps for that window and the frame
    // shows, its opacity for one. Merged by key, as `sol.pane_values` is,
    // and gone with the window.
    // `tests::sol_frame_hands_one_window_its_values`.
    sol.set(
        "frame",
        lua.create_function(|lua, (id, values): (u64, mlua::Table)| {
            let fields = crate::json::Json::object_from_lua(&values)?;
            with_pending(lua, |pending| {
                pending.commands.push(Command::FrameValues { id, fields });
            })
        })?,
    )?;

    // The old name for `sol.pane`. A style used to be a single QML file and
    // is now a folder; the name changed with it. Kept so no configuration
    // written before the change breaks, and cheap enough to keep until there
    // is a reason to remove it.
    sol.set("decoration", sol.get::<mlua::Function>("pane")?)?;

    // Everything `sol.pane` could be handed, discovered from the
    // directories the compositor actually resolves against rather than from a
    // list kept in Lua. Same principle as `parse_easing`: the names come from
    // the machinery, so a style someone writes is offerable the moment it
    // exists and `lua/tweaks.lua` has nothing to edit.
    //
    // `{ name = "...", kind = "bundle" | "file" }` per entry, sorted, bundles
    // first, and each name appearing once -- the shadowing is already applied,
    // so a script can offer the list as it stands without implying a choice
    // the compositor would not make.
    sol.set(
        "decorations",
        lua.create_function(|lua, ()| {
            let list = lua.create_table()?;
            for (index, offered) in crate::decoration::available().into_iter().enumerate() {
                let entry = lua.create_table()?;
                entry.set("name", offered.name)?;
                entry.set("kind", offered.kind.as_str())?;
                list.set(index + 1, entry)?;
            }
            Ok(list)
        })?,
    )?;

    // A table, so adding a setting later does not change the call. Anything
    // left out keeps its default rather than being reset -- `config.lua` is
    // merged from the user's own file and may well carry only one key.
    sol.set(
        "loading",
        lua.create_function(|lua, options: mlua::Table| {
            let loading = settings::pane::loading(&lua::convert::doc_of(&Value::Table(options))?)
                .map_err(mlua::Error::runtime)?;
            with_pending(lua, |pending| {
                pending.commands.push(Command::Loading(loading.clone()));
            })
        })?,
    )?;

    // Whose own size limits a floating drag is held to (#115), from
    // `sizes.lua` as it loads: `{ floating = "respect" | "ignore", ignore = {
    // app_id, ... } }`. A table so that adding a setting later does not change
    // the call; anything left out is the default, which believes every
    // application, and so is a `floating` that is not "ignore" and an `ignore`
    // that is not a list -- the words `sizes.lua` reads the layouts' settings
    // by. An entry of the list that is not a name is skipped. See
    // `ClientSizes`, and
    // `the_floating_setting_and_the_ignored_applications_reach_the_compositor`.
    sol.set(
        "client_sizes",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            let sizes = settings::client_sizes::parse(&lua::convert::section(options)?);
            with_pending(lua, |pending| {
                pending.commands.push(Command::ClientSizes(sizes.clone()));
            })
        })?,
    )?;

    // How a window's picture is fitted while its rectangle and its client
    // disagree about its size -- a drag ahead of its client, a glide between
    // two tiles (#233) -- and what fills the part it does not cover.
    //
    // `Option<Table>` and not `Table`, for the reason `sol.cursor_theme` below
    // spells out at length: the documented way to override the configuration is
    // one `~/.config/solium/config.lua`, and a copy written before this setting
    // existed has no `resize` key at all. A `Table` parameter would fail the
    // *whole configuration* over a setting nobody asked for -- no layouts, no
    // bindings, no windows placed.
    sol.set(
        "resize",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            let resize = settings::resize::parse(&lua::convert::section(options)?);
            with_pending(lua, |pending| {
                pending.commands.push(Command::Resize(resize));
            })
        })?,
    )?;

    // What a fullscreen window covers: `{ covers = "top" | "none" }`, from
    // `init.lua` with `config.fullscreen`. An optional table for `sol.resize`'s
    // reason, and a name that is neither is named in the log and the default
    // kept, for `resize.fill`'s.
    // `the_shipped_fullscreen_setting_reaches_the_compositor`.
    sol.set(
        "fullscreen",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            let covers = settings::fullscreen::parse(&lua::convert::section(options)?);
            with_pending(lua, |pending| {
                pending.commands.push(Command::Fullscreen(covers));
            })
        })?,
    )?;

    // The pointer's XCursor theme and its size, in logical pixels.
    //
    // An optional table, the same shape as `sol.loading` above and for the
    // same reason: a setting added later does not change the call. What is
    // *different* is what an absent key means. `sol.loading` leaves the
    // compositor's default standing; here an absent key -- or no table at
    // all, which is the same answer -- means "the configuration did not say", and
    // the environment gets its turn -- `XCURSOR_THEME` and `XCURSOR_SIZE` are
    // what every other application on this machine follows, so a compositor
    // that overwrote them with defaults of its own would be the one thing on
    // screen drawing a different pointer. The order is resolved in
    // `cursor::theme::Settings::resolve`, which is where it is written down.
    //
    // Takes effect immediately, so `super+shift+r` is how a theme is tried.
    //
    // **`cursor_theme` and not `cursor`, and that name is a bug fix rather
    // than a preference.** `sol.cursor` was already taken: it is the getter
    // above that returns where the pointer *is*, and `tiling.lua` calls it on
    // every window open to decide which pane the new window splits. Registering
    // a second function under the same key silently replaced it, so every
    // `sol.on("open")` failed with "error converting Lua nil to table" and no
    // window opened in a tiled layout was ever placed. Nothing caught it:
    // `sol.set` overwrites without complaint, the compositor starts, `--check`
    // passes, and the unit tests for each of the two functions pass
    // individually because each calls its own. Only running it showed it. See
    // `two_functions_cannot_share_one_name` below, which is now the guard.
    sol.set(
        "cursor_theme",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            // `Option<Table>`, because a copy of `config.lua` written before
            // this setting existed has no `cursor` key, and shipped `init.lua`
            // calls `sol.cursor_theme(nil)`: a required table would fail the
            // whole configuration over a setting nobody asked for.
            let configured = settings::cursor::parse(&lua::convert::section(options)?);
            with_pending(lua, |pending| {
                pending.commands.push(Command::Cursor(configured.clone()));
            })
        })?,
    )?;

    // Which scene graph QML renders on, and how long the GPU probe may take.
    //
    // Kept aside rather than queued, so `Scripts::qml` can read it as soon as
    // the configuration has loaded, before anything it asked for is applied
    // and before any scene starts Qt. An absent table or key means the
    // configuration said nothing. See
    // `a_configured_qml_renderer_is_read_before_qt_starts`.
    sol.set(
        "qml",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            let configured = settings::qml::parse(&lua::convert::section(options)?);
            lua.set_app_data(configured);
            Ok(())
        })?,
    )?;

    // Whether this session is announced to systemd and D-Bus activation,
    // whether that starts XDG autostart, and how long a signal to end waits
    // for a clean stop: `config.session`, handed over by `init.lua`. Kept
    // aside like `sol.qml`, for the backend to read once the configuration
    // has loaded. An absent table or key keeps the default; a value of the
    // wrong kind is named in the log and the default kept.
    // `a_configured_session_is_read_by_the_backend`.
    sol.set(
        "session",
        lua.create_function(|lua, options: Option<mlua::Table>| {
            let settings = settings::session::parse(&lua::convert::section(options)?);
            lua.set_app_data(settings);
            Ok(())
        })?,
    )?;

    sol.set(
        "present_clear",
        lua.create_function(|lua, id: u64| {
            with_pending(lua, |pending| {
                let animation = pending.animation;
                pending.commands.push(Command::Clear { id, animation });
            })
        })?,
    )?;

    // Name a selection: windows, surfaces and whole monitors, under one name.
    //
    //     sol.group("desk-2", {
    //         windows  = { 3, 7 },
    //         surfaces = { "wallpaper-2" },
    //         monitor  = "DP-1",
    //     })
    //     sol.present_group("desk-2", { x = -2560 }, { duration = 300 })
    //
    // **A transform names a selection, and selections compose.** The wallpaper
    // travels because it is in the selection, not because the compositor knows
    // what a wallpaper is -- it does not, and `sol.surface` stays one primitive
    // doing five jobs. A member's own `sol.present` composes with the group's
    // rather than being replaced by it, so a window tilted inside a moving desk
    // stays tilted within it.
    //
    // `sol.group(name, false)` takes one away. Re-declaring the same name
    // replaces the membership and keeps the transform, so a mode that rebuilds
    // its groups every time it runs -- which is every mode -- does not restart
    // its own animation.
    sol.set(
        "group",
        lua.create_function(|lua, (name, options): (String, Value)| {
            let selection = match options {
                Value::Table(options) => Some(selection_from(&options)?),
                // `false` and `nil` both remove it, the same way `sol.surface`
                // reads them, so one spelling works for both primitives.
                _ => None,
            };
            with_pending(lua, |pending| {
                let animation = pending.animation;
                pending.commands.push(Command::Group {
                    name: name.clone(),
                    selection: selection.clone(),
                    animation,
                });
            })
        })?,
    )?;

    sol.set(
        "present_group",
        lua.create_function(
            |lua, (name, options, motion): (String, Option<Table>, Option<Table>)| {
                let to = match options.as_ref() {
                    Some(options) => shift_from(options)?,
                    None => crate::group::Shift::NONE,
                };
                let (duration, easing) = motion_from(motion.as_ref())?;
                with_pending(lua, |pending| {
                    let animation = pending.animation.with(duration, easing);
                    pending.commands.push(Command::PresentGroup {
                        name: name.clone(),
                        to,
                        animation,
                    });
                })
            },
        )?,
    )?;

    sol.set(
        "present_group_clear",
        lua.create_function(|lua, (name, motion): (String, Option<Table>)| {
            let (duration, easing) = motion_from(motion.as_ref())?;
            with_pending(lua, |pending| {
                let animation = pending.animation.with(duration, easing);
                pending.commands.push(Command::ClearGroup {
                    name: name.clone(),
                    animation,
                });
            })
        })?,
    )?;

    // Applies to everything queued after it, so a mode sets the feel of a batch
    // once instead of repeating it per window — and every window in that batch
    // is animated by the same clock over the same interval.
    sol.set(
        "animate",
        lua.create_function(|lua, options: Table| {
            let duration = options
                .get::<Option<u64>>("duration")?
                .map(Duration::from_millis);
            let easing = easing_from(&options)?;
            with_pending(lua, |pending| {
                if let Some(duration) = duration {
                    pending.animation.duration = duration;
                }
                if let Some(easing) = easing {
                    pending.animation.easing = easing;
                }
                pending.animated = true;
            })
        })?,
    )?;

    sol.set(
        "touch",
        lua.create_function(|lua, (slot, options): (i32, Table)| {
            use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge;
            let action: String = options.get("action")?;
            let action = match action.as_str() {
                "capture" => TouchAction::Capture,
                "move" => TouchAction::Move(options.get("window")?),
                "resize" => {
                    let edges: String = options.get("edges")?;
                    let edges = match edges.as_str() {
                        "top" => ResizeEdge::Top,
                        "bottom" => ResizeEdge::Bottom,
                        "left" => ResizeEdge::Left,
                        "right" => ResizeEdge::Right,
                        "top_left" => ResizeEdge::TopLeft,
                        "top_right" => ResizeEdge::TopRight,
                        "bottom_left" => ResizeEdge::BottomLeft,
                        "bottom_right" => ResizeEdge::BottomRight,
                        _ => return Err(mlua::Error::runtime("sol.touch: invalid resize edges")),
                    };
                    TouchAction::Resize(options.get("window")?, edges)
                }
                _ => {
                    return Err(mlua::Error::runtime(
                        "sol.touch: action must be capture, move or resize",
                    ));
                }
            };
            with_pending(lua, |pending| {
                pending.commands.push(Command::Touch { slot, action })
            })
        })?,
    )?;
    sol.set(
        "click",
        lua.create_function(|lua, (x, y): (f64, f64)| {
            if !x.is_finite() || !y.is_finite() {
                return Err(mlua::Error::runtime(
                    "sol.click: coordinates must be finite",
                ));
            }
            with_pending(lua, |pending| {
                pending.commands.push(Command::Click { x, y })
            })
        })?,
    )?;

    // `sol.press(id, spec)`: one button or key, pressed and let go on a
    // window, as a device would. The window named, or the focused one when
    // none is, and nothing when there is neither, as the toggles below. A
    // window that is not in `sol.windows()` -- gone, or a menu or tooltip
    // that is not a window -- or one leaving or minimized is an error: no
    // device could reach it. What the press does once it is carried out is
    // `Solium::press`'s.
    // `directions::sol_press_names_a_button_or_a_key_and_refuses_what_it_cannot_reach`.
    sol.set(
        "press",
        lua.create_function(|lua, (id, spec): (Option<u64>, Table)| {
            let press = press_from(&spec)?;
            let Some(id) = id.or_else(|| focused_id(lua)) else {
                return Ok(());
            };
            let refusal =
                lua.app_data_ref::<Snapshot>()
                    .map_or(Some("is not a window"), |snapshot| {
                        match snapshot.windows.iter().find(|window| window.id == id) {
                            None => Some("is not a window"),
                            Some(window) if window.leaving => Some("is leaving"),
                            Some(window) if window.minimized => Some("is minimized"),
                            Some(_) => None,
                        }
                    });
            if let Some(why) = refusal {
                return Err(mlua::Error::runtime(format!("sol.press: {id} {why}")));
            }
            with_pending(lua, |pending| {
                pending.commands.push(Command::Press {
                    id,
                    press,
                    source: pending.source,
                });
            })
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
                pending.commands.push(Command::Focus {
                    id,
                    source: pending.source,
                    warp,
                });
            })
        })?,
    )?;

    // Take the pointer to a window or a point (#220): the pointer goes at
    // once, and its picture flies along `motion`, or is there at once with
    // none. Carried out once every command of this dispatch is applied, so
    // a window this handler has just moved is warped to where it was moved.
    // `tests::sol_warp_names_a_window_or_a_point_and_a_bad_one_is_an_error`.
    sol.set(
        "warp",
        lua.create_function(|lua, (target, options): (Value, Option<Table>)| {
            let (at, motion, interrupt) = warp_options(options.as_ref(), "sol.warp")?;
            let to = match target {
                Value::Integer(id) => WarpTo::Window {
                    id: u64::try_from(id).map_err(|_| {
                        mlua::Error::runtime(format!("sol.warp: {id} is no window"))
                    })?,
                    at: at.unwrap_or(CENTRE),
                },
                Value::Table(point) => {
                    if at.is_some() {
                        return Err(mlua::Error::runtime(
                            "sol.warp: `to` is where on a window; a point is already a place",
                        ));
                    }
                    match (
                        point.get::<Option<f64>>("x")?,
                        point.get::<Option<f64>>("y")?,
                    ) {
                        (Some(x), Some(y)) if x.is_finite() && y.is_finite() => {
                            WarpTo::Point { x, y }
                        }
                        _ => {
                            return Err(mlua::Error::runtime(
                                "sol.warp: a point is { x = <number>, y = <number> }",
                            ));
                        }
                    }
                }
                other => {
                    return Err(mlua::Error::runtime(format!(
                        "sol.warp: a window's id or a point {{ x, y }}, not {}",
                        other.type_name()
                    )));
                }
            };
            with_pending(lua, |pending| {
                pending.commands.push(Command::Warp(Warp {
                    to,
                    motion,
                    interrupt,
                }));
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
                pending.commands.push(Command::ToggleFullscreen { id });
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
                pending.commands.push(Command::Minimize { id });
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
                pending.commands.push(Command::Unminimize { id });
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
                pending.commands.push(Command::ShowMinimized {
                    id,
                    shown: shown.unwrap_or(true),
                });
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
                pending.commands.push(Command::KeepAbove { id, on });
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
                pending.commands.push(Command::ToggleMaximize { id });
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
                pending.commands.push(Command::DragHold { id, held });
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
                pending.commands.push(Command::DragGrip { id, along, by });
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
                pending.commands.push(Command::ResizeHold { id, held });
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
                pending.commands.push(Command::Place {
                    id,
                    rect,
                    animation,
                    tile,
                    inside,
                    cramped,
                    moved,
                });
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
                pending.commands.push(Command::Unplace { id })
            })
        })?,
    )?;

    sol.set(
        "close",
        lua.create_function(|lua, id: u64| {
            with_pending(lua, |pending| pending.commands.push(Command::Close { id }))
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
                    pending.commands.push(Command::Act {
                        attempt,
                        action,
                        data,
                        source: pending.source,
                    });
                })?;
                if let Some(done) = done {
                    let attempts: Table = lua.globals().get::<Table>("sol")?.get("_attempts")?;
                    attempts.set(attempt, done)?;
                }
                Ok(attempt)
            },
        )?,
    )?;

    // `sol.spawn("foot", "-e", "htop")`. Variadic rather than a table because
    // the common case is one program and no arguments, and that should read
    // like one.
    sol.set(
        "spawn",
        lua.create_function(|lua, mut args: mlua::Variadic<String>| {
            if args.is_empty() {
                return Err(mlua::Error::runtime("sol.spawn needs a program to run"));
            }
            let program = args.remove(0);
            let args = args.into_iter().collect();
            with_pending(lua, |pending| {
                pending.commands.push(Command::Spawn { program, args });
            })
        })?,
    )?;

    // `sol.run("wpctl", "set-volume", "@DEFAULT_AUDIO_SINK@", "0.5")`: a
    // program that shows no window, so no window is opened for it (#319).
    // With a table first, `sol.run({ "nmcli", "-t", "radio" }, { done = fn })`,
    // its output and how it ended come back (#472): see `run_read`.
    sol.set(
        "run",
        lua.create_function(|lua, args: mlua::MultiValue| {
            if let Some(Value::Table(argv)) = args.front() {
                let argv = argv.clone();
                let options = match args.get(1) {
                    None | Some(Value::Nil) => None,
                    Some(Value::Table(options)) => Some(options.clone()),
                    Some(_) => {
                        return Err(mlua::Error::runtime(
                            "sol.run's second argument is a table of options",
                        ));
                    }
                };
                return run_read(lua, &argv, options.as_ref());
            }
            let mut args =
                <mlua::Variadic<String> as mlua::FromLuaMulti>::from_lua_multi(args, lua)?;
            if args.is_empty() {
                return Err(mlua::Error::runtime("sol.run needs a program to run"));
            }
            let program = args.remove(0);
            let args = args.into_iter().collect();
            with_pending(lua, |pending| {
                pending.commands.push(Command::Run { program, args });
            })
        })?,
    )?;

    // `config.run`, handed over by `init.lua` (#472): the timeout and the
    // output cap a `sol.run{}` takes when its own options leave them out.
    // Either left out, or of the wrong kind, keeps the default, named in the
    // log, as every other `sol.<setting>` here does. Kept in this Lua state,
    // where `run_read` builds each request, so a reload starts from the
    // defaults until the new configuration hands them over again.
    // `tests::sol_run_takes_the_configured_defaults_and_a_call_overrides_them`.
    sol.set(
        "run_defaults",
        lua.create_function(|lua, options: Option<Table>| {
            let defaults = settings::run::parse(&lua::convert::section(options)?);
            lua.set_app_data(defaults);
            Ok(())
        })?,
    )?;

    // `sol.after(ms, fn)` and `sol.every(ms, fn)` (#472): `fn` on the
    // compositor's clock, once or every `ms`, until the handle's `cancel`.
    // See `crate::timers`.
    sol.set(
        "after",
        lua.create_function(|lua, (ms, run): (f64, mlua::Function)| {
            set_timer(lua, ms, run, false)
        })?,
    )?;
    sol.set(
        "every",
        lua.create_function(|lua, (ms, run): (f64, mlua::Function)| set_timer(lua, ms, run, true))?,
    )?;

    // `sol.read(path, fn(data, err))` (#473): the whole file, read on a
    // thread of its own up to `files.max_read`, to `fn`. See `crate::files`.
    sol.set(
        "read",
        lua.create_function(|lua, (path, done): (String, mlua::Function)| {
            let absolute = crate::files::expand(&path)
                .map_err(|why| mlua::Error::runtime(format!("sol.read: {why}")))?;
            let max = lua
                .try_app_data_ref::<FileSettings>()
                .ok()
                .flatten()
                .map_or(crate::files::MAX_READ, |settings| settings.max_read);
            let id = FILES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            with_pending(lua, |pending| {
                pending
                    .commands
                    .push(Command::FileRead(crate::files::ReadRequest {
                        id,
                        path: absolute,
                        max,
                    }));
            })?;
            let files: Table = lua.globals().get::<Table>("sol")?.get("_files")?;
            files.set(id, done)?;
            Ok(())
        })?,
    )?;

    // `sol.write(path, data, fn(err))` (#473): atomic, on a thread of its
    // own, in order with the other writes to `path`. See `crate::files`.
    sol.set(
        "write",
        lua.create_function(
            |lua, (path, data, done): (String, mlua::LuaString, Option<mlua::Function>)| {
                let absolute = crate::files::expand(&path)
                    .map_err(|why| mlua::Error::runtime(format!("sol.write: {why}")))?;
                let id = FILES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                with_pending(lua, |pending| {
                    pending
                        .commands
                        .push(Command::FileWrite(crate::files::WriteRequest {
                            id,
                            path: absolute,
                            data: data.as_bytes().to_vec(),
                            report: done.is_some(),
                        }));
                })?;
                if let Some(done) = done {
                    let files: Table = lua.globals().get::<Table>("sol")?.get("_files")?;
                    files.set(id, done)?;
                }
                Ok(())
            },
        )?,
    )?;

    // `sol.watch(path, fn(event))` (#473): the file followed through its
    // directory, which must exist, until the handle's `cancel` or a reload.
    // See `crate::files`.
    sol.set(
        "watch",
        lua.create_function(|lua, (path, run): (String, mlua::Function)| {
            let absolute = crate::files::expand(&path)
                .map_err(|why| mlua::Error::runtime(format!("sol.watch: {why}")))?;
            let (Some(directory), Some(_)) = (absolute.parent(), absolute.file_name()) else {
                return Err(mlua::Error::runtime(format!(
                    "sol.watch: {path} names no file in a directory"
                )));
            };
            if !directory.is_dir() {
                return Err(mlua::Error::runtime(format!(
                    "sol.watch: {} is not a directory, so {path} cannot be watched",
                    directory.display()
                )));
            }
            let id = FILES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            with_pending(lua, |pending| {
                pending.commands.push(Command::Watch {
                    id,
                    path: absolute,
                    shown: path,
                });
            })?;
            let watches: Table = lua.globals().get::<Table>("sol")?.get("_watches")?;
            watches.set(id, run)?;
            Ok(WatchHandle(id))
        })?,
    )?;

    // `sol.store(name, defaults)` (#473): `sol.keep`'s table, also saved to
    // `$XDG_STATE_HOME/solium/state/<name>.json`. Loaded here, at the call,
    // because the configuration needs its values at once and the file is
    // small and local; saved by the compositor. See `crate::files`.
    sol.set(
        "store",
        lua.create_function(|lua, (name, defaults): (String, Option<Table>)| {
            if !crate::files::store_name_ok(&name) {
                return Err(mlua::Error::runtime(format!(
                    "sol.store: {name:?} is not a store's name, which is letters, digits, `_` \
                     and `-`"
                )));
            }
            let stores: Table = lua.globals().get::<Table>("sol")?.get("_stores")?;
            // Asked for twice, the same table, as `sol.keep` answers.
            if let Value::Table(already) = stores.get::<Value>(name.as_str())? {
                return Ok(already);
            }
            let table = match defaults {
                Some(defaults) => defaults,
                None => lua.create_table()?,
            };
            // Within one session the store a reload carried is the truth;
            // the file is read the first time it is asked for.
            let carried = lua
                .app_data_ref::<Carried>()
                .and_then(|carried| carried.0.stores.get(&name).cloned());
            let found = match carried {
                Some(carried) => Some(carried),
                None => {
                    let max = lua
                        .try_app_data_ref::<FileSettings>()
                        .ok()
                        .flatten()
                        .map_or(crate::files::MAX_STORE, |settings| settings.store_max);
                    match crate::files::read_store(&name, max) {
                        crate::files::StoreFile::Missing => {
                            tracing::info!(store = name, "no store file yet; starting from the defaults");
                            None
                        }
                        crate::files::StoreFile::Found(loaded) => {
                            let text = loaded.render();
                            let name = name.clone();
                            with_pending(lua, |pending| {
                                pending.commands.push(Command::StoreLoaded { name, text });
                            })?;
                            Some(loaded)
                        }
                        crate::files::StoreFile::Broken { path, why } => {
                            tracing::warn!(store = name, path = %path.display(), why, "the store file cannot be used; starting from the defaults");
                            // `--check` changes nothing of the user's: it
                            // says so instead, and the session sets the
                            // file aside. `check::tests::a_check_leaves_a_broken_store_file_as_it_was`.
                            match lua.app_data_mut::<CheckOnly>() {
                                Some(mut check) => check.0.push(format!(
                                    "store `{name}`: {} is {why}; a session would start from the \
                                     defaults and move it to {name}.json.broken",
                                    path.display()
                                )),
                                None => crate::files::set_aside(&name, &path),
                            }
                            None
                        }
                    }
                }
            };
            if let Some(found) = found {
                fill_store(lua, &table, &found)?;
            }
            stores.set(name, &table)?;
            Ok(table)
        })?,
    )?;

    // `config.files`, handed over by `init.lua` (#473): how much a
    // `sol.read` reads, how often the stores are saved, and how large a
    // store file is read. Left out, or of the wrong kind, keeps the default,
    // named in the log, as `sol.run_defaults` does; kept in this Lua state,
    // where `sol.read` and `sol.store` read them and the settle asks
    // `Scripts::store_interval`.
    // `tests::sol_read_stops_at_files_max_read`,
    // `tests::sol_files_reads_the_store_settings`.
    sol.set(
        "files",
        lua.create_function(|lua, options: Option<Table>| {
            let settings = settings::files::parse(&lua::convert::section(options)?);
            lua.set_app_data(settings);
            Ok(())
        })?,
    )?;

    // Whether a program exists to be run. A desktop cannot assume any
    // particular terminal, editor or browser is installed, and a script that
    // picks the first one that *is* there beats one that fails silently on a
    // machine set up differently from the author's.
    sol.set(
        "which",
        lua.create_function(|_, program: String| Ok(which(&program)))?,
    )?;

    // Ending the session, as a command a script issues rather than a key the
    // compositor keeps for itself. Ctrl+Alt+Backspace still works and always
    // will — that one has to survive a broken config — but a compositor whose
    // only way out is hardcoded is not a configurable one.
    sol.set(
        "quit",
        lua.create_function(|lua, ()| {
            with_pending(lua, |pending| pending.commands.push(Command::Quit))
        })?,
    )?;

    sol.set(
        "grab_input",
        lua.create_function(|lua, grabbed: bool| {
            with_pending(lua, |pending| pending.grab = Some(grabbed))
        })?,
    )?;

    sol.set(
        "status",
        lua.create_function(|lua, text: String| {
            with_pending(lua, |pending| pending.status = Some(text))
        })?,
    )?;

    // Whether a program that arranges the monitors -- kanshi, wlr-randr, a
    // settings panel -- sees them, through `zwlr-output-management-v1`
    // (#51): `config.output_management`, which the shipped `init.lua` hands
    // over. `tests::sol_output_management_is_a_flag`.
    sol.set(
        "output_management",
        lua.create_function(|lua, offered: bool| {
            with_pending(lua, |pending| {
                pending.commands.push(Command::OutputManagement(offered));
            })
        })?,
    )?;

    // Whether a shell that is its own program sees the workspaces, through
    // `ext-workspace-v1` (#60): `config.workspaces.offered`, which the shipped
    // `workspaces.lua` hands over. `tests::sol_workspaces_offered_is_a_flag`.
    sol.set(
        "workspaces_offered",
        lua.create_function(|lua, offered: bool| {
            with_pending(lua, |pending| {
                pending.commands.push(Command::WorkspacesOffered(offered));
            })
        })?,
    )?;

    // `sol.workspaces{ arrangement, groups, windows }`: the compositor does
    // not know what a workspace is, so Lua says, each time it changes
    // (03 §3.2.17). `tests::sol_workspaces_declares_groups_and_windows`.
    sol.set(
        "workspaces",
        lua.create_function(|lua, declared: Table| {
            let declared =
                settings::workspaces::parse(&lua::convert::doc_of(&Value::Table(declared))?)
                    .map_err(mlua::Error::runtime)?;
            with_pending(lua, |pending| {
                pending.commands.push(Command::Workspaces(declared));
            })
        })?,
    )?;

    // The later call wins, and always has: two scripts binding one combination
    // is how `init.lua` and `scrolling.lua` coexist, and the alternative --
    // refusing the second -- would make the order files happen to be required
    // in decide which of them works.
    //
    // The third argument is optional and is a *note*, not a name: a short
    // phrase saying where this binding came from, which `solium --check` prints
    // beside the combination. It exists because that "later call wins" rule is
    // the whole mechanism behind `config.bindings` replacing a shipped binding
    // (#117), and a replacement nobody can see is indistinguishable from a key
    // that mysteriously stopped working. Shipped bindings pass nothing, so
    // `--check` stays a plain list until a configuration has something to say.
    //
    // Or the third argument is a table, which holds the note and what else the
    // binding asks for (#224): `repeating`, `locked`, and `inhibited` (#58).
    // What a binding asks for
    // is replaced with it, so a later call that asks for nothing asks for
    // nothing -- a configuration rebinding a shipped repeating key to launch
    // something does not inherit the repeat.
    sol.set(
        "bind",
        lua.create_function(
            |lua, (combo, handler, said): (String, mlua::Function, Option<Value>)| {
                let (note, asked) = binding_note_and_options(said)?;
                let sol: Table = lua.globals().get("sol")?;
                let bindings: Table = sol.get("_bindings")?;
                let combo = normalise_combo(&combo);
                bindings.set(combo.clone(), handler)?;
                let options: Table = sol.get("_binding_options")?;
                if asked == BindingOptions::default() {
                    options.set(combo.clone(), Value::Nil)?;
                } else {
                    let table = lua.create_table()?;
                    table.set("repeating", asked.repeating)?;
                    table.set("locked", asked.locked)?;
                    table.set("inhibited", asked.inhibited)?;
                    options.set(combo.clone(), table)?;
                }
                if let Some(note) = note {
                    let sources: Table = sol.get("_binding_sources")?;
                    sources.set(combo, note)?;
                }
                Ok(())
            },
        )?,
    )?;

    // Whether a combination is already bound, in its canonical spelling.
    //
    // Asking is the only way a script can tell that it is about to replace
    // somebody's binding rather than add one, and normalising is why it cannot
    // do this by reading `_bindings` itself: `Super+Q` and `shift+super+q` are
    // spellings, not keys, and only this side knows which spelling won.
    sol.set(
        "bound",
        lua.create_function(|lua, combo: String| {
            let sol: Table = lua.globals().get("sol")?;
            let bindings: Table = sol.get("_bindings")?;
            Ok(matches!(
                bindings.get::<Value>(normalise_combo(&combo))?,
                Value::Function(_)
            ))
        })?,
    )?;

    // Take a binding away.
    //
    // The counterpart to a configuration being able to replace one. Without it
    // a shipped binding is unremovable short of replacing `init.lua` -- and
    // binding it to a function that does nothing is not the same thing, because
    // the compositor would still swallow the key and `--check` would still list
    // it as bound.
    //
    // The note is kept even though the binding is gone, and that is the point
    // of keeping it: `--check` reads the combinations in `_binding_sources`
    // that are no longer in `_bindings` and reports them as deliberately
    // removed, so an absence is stated rather than merely being an absence.
    sol.set(
        "unbind",
        lua.create_function(|lua, (combo, note): (String, Option<String>)| {
            let sol: Table = lua.globals().get("sol")?;
            let bindings: Table = sol.get("_bindings")?;
            let combo = normalise_combo(&combo);
            bindings.set(combo.clone(), Value::Nil)?;
            let options: Table = sol.get("_binding_options")?;
            options.set(combo.clone(), Value::Nil)?;
            if let Some(note) = note {
                let sources: Table = sol.get("_binding_sources")?;
                sources.set(combo, note)?;
            }
            Ok(())
        })?,
    )?;

    // A setting a configuration wrote that the defaults do not define.
    //
    // `lua/config.lua` merges a `user.lua` over its own table key by key, and
    // until #117 a key the defaults had never heard of was merged in exactly
    // like one they had: `tilling = { split = 0.6 }` became a new section that
    // nothing would ever read, and `--check` -- the one command whose job is
    // answering "did what I wrote take effect" -- said the configuration loaded
    // fine. It did. It just did not do anything.
    //
    // Reported from Lua rather than found from here because the comparison
    // needs both halves, and only `config.lua` has them: the defaults are its
    // table, and the user's file is something it alone reads. `meant` is a
    // near-miss suggestion when there is one worth making, and is optional
    // because most typos are not near misses of anything.
    sol.set(
        "unknown",
        lua.create_function(|lua, (key, meant): (String, Option<String>)| {
            let sol: Table = lua.globals().get("sol")?;
            let unknown: Table = sol.get("_unknown_settings")?;
            let entry = lua.create_table()?;
            entry.set("key", key.clone())?;
            if let Some(meant) = &meant {
                entry.set("meant", meant.clone())?;
            }
            unknown.push(entry)?;
            // Also in the log, because most of these are met in a running
            // session rather than in front of `--check`, and a reload that
            // quietly ignores half a file is the failure this is here to end.
            match meant {
                Some(meant) => tracing::warn!(
                    target: "solium::script",
                    setting = %key,
                    "the configuration sets something nothing reads; did you mean {meant}?"
                ),
                None => tracing::warn!(
                    target: "solium::script",
                    setting = %key,
                    "the configuration sets something nothing reads"
                ),
            }
            Ok(())
        })?,
    )?;

    // Appended, never replaced. Two scripts caring about the same event is
    // ordinary — a tiling script and an open animation both want to know a
    // window appeared — and the second one silently unhooking the first is the
    // kind of bug that gets blamed on the compositor.
    sol.set(
        "on",
        lua.create_function(|lua, (event, handler): (String, mlua::Function)| {
            let sol: Table = lua.globals().get("sol")?;
            let handlers: Table = sol.get("_handlers")?;
            let listeners: Table = match handlers.get::<Value>(event.clone())? {
                Value::Table(existing) => existing,
                _ => {
                    let created = lua.create_table()?;
                    handlers.set(event, &created)?;
                    created
                }
            };
            listeners.push(handler)?;
            Ok(())
        })?,
    )?;

    // The standard arrangements, from the same crate the preview calls. A
    // script picks which and when — and may compute its own rectangles instead,
    // which is why these are offered rather than imposed.
    sol.set("layout", layouts(lua)?)?;

    sol.set(
        "log",
        lua.create_function(|_, message: String| {
            tracing::info!(target: "solium::script", "{message}");
            Ok(())
        })?,
    )?;

    Ok(sol)
}
