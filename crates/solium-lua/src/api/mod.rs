//! `sol`: the table of functions a configuration calls, one area to a file.

use mlua::{Lua, Table};

mod bindings;
mod input;
mod keep;
mod layout;
mod monitors;
mod present;
mod process;
mod services;
mod settings;
mod windows;

pub use windows::layer_named;

/// Build the `sol` table.
pub fn build_api(lua: &Lua) -> mlua::Result<Table> {
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

    keep::register(lua, &sol)?;
    windows::register(lua, &sol)?;
    monitors::register(lua, &sol)?;
    services::register(lua, &sol)?;
    settings::register(lua, &sol)?;
    present::register(lua, &sol)?;
    input::register(lua, &sol)?;
    bindings::register(lua, &sol)?;
    process::register(lua, &sol)?;
    layout::register(lua, &sol)?;

    Ok(sol)
}
