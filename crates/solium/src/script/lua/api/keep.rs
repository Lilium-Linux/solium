//! What outlives a reload, what is wrong, and what the log hears.

use mlua::{Lua, Table, Value};

use crate::script::lua::*;
use crate::script::*;

/// Add this area's functions to `sol`.
pub(crate) fn register(lua: &Lua, sol: &Table) -> mlua::Result<()> {
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
            let mirror = mirror(lua);
            let rows = lua.create_table()?;
            for (index, problem) in mirror.problems.iter().enumerate() {
                let row = lua.create_table()?;
                row.set("effect", problem.effect.clone())?;
                row.set("file", problem.file.clone())?;
                row.set("line", problem.line)?;
                row.set("column", problem.column)?;
                row.set("message", problem.message.clone())?;
                row.set("severity", problem.severity.clone())?;
                rows.set(index + 1, row)?;
            }
            Ok(rows)
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
            undeclared(lua);
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

    sol.set(
        "log",
        lua.create_function(|_, message: String| {
            tracing::info!(target: "solium::script", "{message}");
            Ok(())
        })?,
    )?;

    Ok(())
}
