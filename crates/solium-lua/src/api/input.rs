//! The keyboard, the pointer and touch, and whether a mode holds input.

use mlua::{Lua, Table, Value};

use crate::*;

/// Add this area's functions to `sol`.
pub(crate) fn register(lua: &Lua, sol: &Table) -> mlua::Result<()> {
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
                let mirror = mirror(lua);
                let keyboard = mirror
                    .keyboard
                    .clone()
                    .unwrap_or(solium_wm_protocol::Keyboard {
                        layouts: Vec::new(),
                        short_names: Vec::new(),
                        active: 0,
                        caps: false,
                        num: false,
                        repeat_rate: 0,
                        repeat_delay: 0,
                    });
                let live = |names: &[String]| -> String {
                    names
                        .get((keyboard.active as usize).saturating_sub(1))
                        .cloned()
                        .unwrap_or_default()
                };
                let table = lua.create_table()?;
                table.set("shortcuts_inhibited", mirror.shortcuts_owner.is_some())?;
                table.set("shortcuts_owner", mirror.shortcuts_owner)?;
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
                table.set("layout_name", live(&keyboard.layouts))?;
                table.set("layout_short", live(&keyboard.short_names))?;
                table.set("caps", keyboard.caps)?;
                table.set("num", keyboard.num)?;
                table.set("repeat_rate", keyboard.repeat_rate)?;
                table.set("repeat_delay", keyboard.repeat_delay)?;
                return Ok(Value::Table(table));
            };

            super::settings::configure(lua, "keyboard", convert::wire_section(Some(options))?)?;
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
            let mirror = mirror(lua);
            let Some(field) = &mirror.text_field else {
                return Ok(Value::Nil);
            };
            let table = lua.create_table()?;
            table.set("window", field.window)?;
            table.set("framed", field.framed)?;
            if let Some(caret) = &field.caret {
                table.set("x", caret.x)?;
                table.set("y", caret.y)?;
                table.set("w", caret.w)?;
                table.set("h", caret.h)?;
            }
            Ok(Value::Table(table))
        })?,
    )?;

    sol.set(
        "cursor",
        lua.create_function(|lua, ()| {
            let (x, y) = mirror(lua).cursor;
            let table = lua.create_table()?;
            table.set("x", x)?;
            table.set("y", y)?;
            Ok(table)
        })?,
    )?;

    // The shortcuts a window holds, taken back until the keyboard leaves it
    // (#58). See `shortcuts_inhibit`.
    sol.set(
        "take_shortcuts_back",
        lua.create_function(|lua, ()| {
            with_pending(lua, |pending| {
                pending.push(solium_wm_protocol::Command::TakeShortcutsBack);
            })
        })?,
    )?;

    // The pointer a window holds, taken back until it is pressed on (#427).
    // See `state::pointer_hold`.
    sol.set(
        "release_pointer",
        lua.create_function(|lua, ()| {
            with_pending(lua, |pending| {
                pending.push(solium_wm_protocol::Command::ReleasePointer);
            })
        })?,
    )?;

    sol.set(
        "touch",
        lua.create_function(|lua, (slot, options): (i32, Table)| {
            use solium_wm_protocol::TouchAction;
            let action: String = options.get("action")?;
            let action = match action.as_str() {
                "capture" => TouchAction::Capture,
                "move" => TouchAction::Move(options.get("window")?),
                "resize" => {
                    let edges: String = options.get("edges")?;
                    if !solium_wm_protocol::touch::RESIZE_EDGES.contains(&edges.as_str()) {
                        return Err(mlua::Error::runtime("sol.touch: invalid resize edges"));
                    }
                    TouchAction::Resize(solium_wm_protocol::TouchResize {
                        id: options.get("window")?,
                        edges,
                    })
                }
                _ => {
                    return Err(mlua::Error::runtime(
                        "sol.touch: action must be capture, move or resize",
                    ));
                }
            };
            with_pending(lua, |pending| {
                pending.push(solium_wm_protocol::Command::Touch(
                    solium_wm_protocol::TouchClaim { slot, action },
                ));
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
                pending.push(solium_wm_protocol::Command::Click(
                    solium_wm_protocol::Point { x, y },
                ))
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
            let refusal = match mirror(lua).window(id) {
                None => Some("is not a window"),
                Some(window) if window.leaving => Some("is leaving"),
                Some(window) if window.minimized => Some("is minimized"),
                Some(_) => None,
            };
            if let Some(why) = refusal {
                return Err(mlua::Error::runtime(format!("sol.press: {id} {why}")));
            }
            with_pending(lua, |pending| {
                let cause = pending.source;
                pending.push(solium_wm_protocol::Command::Press(
                    solium_wm_protocol::PressOn { id, press, cause },
                ));
            })
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

    Ok(())
}
