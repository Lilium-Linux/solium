//! The monitors and the workspaces on them.

use mlua::{Lua, Table, Value};

use crate::script::lua::*;
use crate::script::*;

/// Add this area's functions to `sol`.
pub(crate) fn register(lua: &Lua, sol: &Table) -> mlua::Result<()> {
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

            super::settings::configure(
                lua,
                "monitors",
                lua::convert::wire_doc_of(&Value::Table(rows))?,
            )?;
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
                pending.push(solium_wm_protocol::Command::Power((monitor, on)));
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

    // Whether a program that arranges the monitors -- kanshi, wlr-randr, a
    // settings panel -- sees them, through `zwlr-output-management-v1`
    // (#51): `config.output_management`, which the shipped `init.lua` hands
    // over. `tests::sol_output_management_is_a_flag`.
    sol.set(
        "output_management",
        lua.create_function(|lua, offered: bool| {
            with_pending(lua, |pending| {
                pending.push(solium_wm_protocol::Command::Configure((
                    "output_management".to_owned(),
                    vec![solium_wm_protocol::DocNode::Boolean(offered)],
                )));
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
                pending.push(solium_wm_protocol::Command::Configure((
                    "workspaces_offered".to_owned(),
                    vec![solium_wm_protocol::DocNode::Boolean(offered)],
                )));
            })
        })?,
    )?;

    // `sol.workspaces{ arrangement, groups, windows }`: the compositor does
    // not know what a workspace is, so Lua says, each time it changes
    // (03 §3.2.17). `tests::sol_workspaces_declares_groups_and_windows`.
    sol.set(
        "workspaces",
        lua.create_function(|lua, declared: Table| {
            super::settings::configure(
                lua,
                "workspaces",
                lua::convert::wire_doc_of(&Value::Table(declared))?,
            )
        })?,
    )?;

    Ok(())
}
