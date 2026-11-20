//! The handler deadline: a listener that runs past it is stopped, and one
//! stopped three times is taken out.

use mlua::{Lua, Table, Value};

/// How long one handler may run before it is stopped (03 §3.3.3, Ruling 16).
/// `tests::a_listener_that_never_returns_is_stopped_and_the_others_still_run`,
/// `tests::a_binding_that_never_returns_is_stopped`.
pub const HANDLER_DEADLINE: std::time::Duration = std::time::Duration::from_millis(100);
/// How many times a listener may be stopped before it is taken out.
/// `tests::a_listener_stopped_three_times_is_taken_out`.
pub const STRIKES: u32 = 3;
/// What the error a stopped handler is unwound with says.
/// `tests::a_listener_stopped_three_times_is_taken_out`.
pub const STOPPED: &str = "this handler ran for longer than 100 ms and was stopped";

/// When the handler running now started, for the instruction hook; `None`
/// between dispatches.
/// `tests::a_listener_that_never_returns_is_stopped_and_the_others_still_run`,
/// `tests::lua_run_between_dispatches_is_not_stopped`.
#[derive(Debug, Default)]
pub struct Deadline {
    pub started: Option<std::time::Instant>,
    /// Where the handler running now was stopped, as `file:line`, once the
    /// instruction hook has stopped it.
    /// `tests::a_stopped_override_is_logged_where_it_was_stopped`.
    pub stopped_at: Option<String>,
}

/// Start a handler's clock now, with nowhere it was stopped yet.
/// `tests::each_listener_has_the_whole_deadline`,
/// `tests::a_stopped_override_is_logged_where_it_was_stopped`.
pub fn start_clock(lua: &Lua) {
    if let Ok(Some(mut deadline)) = lua.try_app_data_mut::<Deadline>() {
        deadline.started = Some(std::time::Instant::now());
        deadline.stopped_at = None;
    }
}

/// Set the handler clock back to when it started, or stop it with `None`.
/// `tests::lua_run_between_dispatches_is_not_stopped`.
pub fn set_deadline(lua: &Lua, started: Option<std::time::Instant>) {
    if let Ok(Some(mut deadline)) = lua.try_app_data_mut::<Deadline>() {
        deadline.started = started;
    }
}

/// Note `debug`'s function and line as where the handler running now was
/// stopped. `tests::a_stopped_override_is_logged_where_it_was_stopped`,
/// `tests::a_handler_stopped_at_its_focus_direction_is_logged_at_that_call`.
pub fn stopped_here(lua: &Lua, debug: &mlua::debug::Debug) {
    let source = debug.source();
    let file = file_of(source.source.as_deref(), source.short_src.as_deref());
    let line = debug.current_line().unwrap_or_default();
    if let Ok(Some(mut deadline)) = lua.try_app_data_mut::<Deadline>() {
        deadline.stopped_at = Some(format!("{file}:{line}"));
    }
}

/// Where the handler running now was stopped, once it has been.
/// `tests::a_stopped_override_is_logged_where_it_was_stopped`.
pub fn stopped_at(lua: &Lua) -> Option<String> {
    lua.try_app_data_ref::<Deadline>()
        .ok()
        .flatten()
        .and_then(|deadline| deadline.stopped_at.clone())
}

/// A file by its whole path, and a chunk of text by the short name Lua's own
/// messages give it.
/// `tests::a_stopped_listener_is_logged_with_its_file_and_line`,
/// `tests::a_stopped_listener_from_a_loaded_chunk_is_logged_by_its_short_name`.
pub fn file_of(source: Option<&str>, short_src: Option<&str>) -> String {
    source
        .and_then(|source| source.strip_prefix('@'))
        .or(short_src)
        .unwrap_or_default()
        .to_owned()
}

/// When the handler running now started, if one is running.
/// `tests::a_binding_that_loops_on_focus_direction_is_stopped`.
pub fn deadline_started(lua: &Lua) -> Option<std::time::Instant> {
    lua.try_app_data_ref::<Deadline>()
        .ok()
        .flatten()
        .and_then(|deadline| deadline.started)
}

/// Whether the handler running now is past its deadline.
/// `tests::a_listener_that_never_returns_is_stopped_and_the_others_still_run`.
pub fn late(lua: &Lua) -> bool {
    deadline_started(lua).is_some_and(|started| started.elapsed() >= HANDLER_DEADLINE)
}

/// Count a stop at the deadline against `handler`, a listener of `event` or a
/// `done` (`event` is then `"done"`), by function, and log it with the file
/// and line the handler was written at, and the file and line it was stopped
/// at. True from its third stop on, when it is taken out, which is logged too.
/// `tests::a_listener_stopped_three_times_is_taken_out`,
/// `tests::a_done_stopped_three_times_is_not_called_again`,
/// `tests::a_stopped_listener_is_logged_with_its_file_and_line`,
/// `tests::a_stopped_override_is_logged_where_it_was_stopped`.
pub fn strike(
    lua: &Lua,
    strikes: &Table,
    handler: &mlua::Function,
    event: &str,
    err: &mlua::Error,
) -> mlua::Result<bool> {
    let count = strikes.get::<Option<u32>>(handler)?.unwrap_or(0) + 1;
    strikes.set(handler, count)?;
    let info = handler.info();
    let file = file_of(info.source.as_deref(), info.short_src.as_deref());
    let line = info.line_defined.unwrap_or_default();
    let stopped_at = stopped_at(lua);
    tracing::error!(
        %err,
        event,
        file = %file,
        line,
        stopped_at = stopped_at.as_deref().map(tracing::field::display),
        count,
        "a handler ran for longer than 100 ms and was stopped"
    );
    if count >= STRIKES {
        tracing::error!(
            event,
            file = %file,
            line,
            "a handler stopped three times is taken out until the configuration is reloaded"
        );
    }
    Ok(count >= STRIKES)
}

/// Run every listener registered for an event.
///
/// One failing listener is logged and the rest still run: a broken script must
/// not silently disable the others, which is what returning early would do.
/// One stopped at the deadline counts a strike, and is taken out at the third;
/// both are logged with the file and line the listener was written at.
/// `tests::a_listener_stopped_three_times_is_taken_out`,
/// `tests::a_stopped_listener_is_logged_with_its_file_and_line`.
pub fn call_listeners(
    lua: &Lua,
    event: &str,
    args: impl mlua::IntoLuaMulti + Clone,
) -> mlua::Result<bool> {
    let sol: Table = lua.globals().get("sol")?;
    let handlers: Table = sol.get("_handlers")?;
    let Value::Table(listeners) = handlers.get::<Value>(event)? else {
        return Ok(false);
    };
    let strikes: Table = sol.get("_strikes")?;

    let mut called = false;
    let mut out = Vec::new();
    for listener in listeners.sequence_values::<mlua::Function>() {
        let listener = match listener {
            Ok(listener) => listener,
            Err(err) => {
                tracing::error!(%err, event, "a listener failed");
                continue;
            }
        };
        // Each listener has the whole deadline, its clock started here and
        // nowhere a script can reach, so no handler can put its own off and
        // no configuration can take the next one's away.
        // `tests::each_listener_has_the_whole_deadline`,
        // `tests::a_handler_that_calls_sol_deadline_is_still_stopped`,
        // `tests::replacing_sol_deadline_leaves_each_listener_its_own_deadline`.
        start_clock(lua);
        match listener.call::<()>(args.clone()) {
            Ok(()) => called = true,
            Err(err) if err.to_string().contains(STOPPED) => {
                if strike(lua, &strikes, &listener, event, &err)? {
                    out.push(listener);
                }
            }
            Err(err) => tracing::error!(%err, event, "a listener failed"),
        }
    }
    for gone in out {
        let kept: Vec<mlua::Function> = listeners
            .sequence_values::<mlua::Function>()
            .filter_map(Result::ok)
            .filter(|each| *each != gone)
            .collect();
        listeners.clear()?;
        for (index, each) in kept.into_iter().enumerate() {
            listeners.set(index + 1, each)?;
        }
    }
    Ok(called)
}

/// The listeners of a decision hook, each in turn until one returns a
/// table, which is the answer. A listener stopped at the deadline is
/// struck as any other.
pub fn call_deciders(
    lua: &Lua,
    event: &str,
    args: impl mlua::IntoLuaMulti + Clone,
) -> mlua::Result<Option<solium_json::Json>> {
    let sol: Table = lua.globals().get("sol")?;
    let handlers: Table = sol.get("_handlers")?;
    let Value::Table(listeners) = handlers.get::<Value>(event)? else {
        return Ok(None);
    };
    let strikes: Table = sol.get("_strikes")?;
    for listener in listeners.sequence_values::<mlua::Function>() {
        let Ok(listener) = listener else {
            continue;
        };
        start_clock(lua);
        match listener.call::<Value>(args.clone()) {
            Ok(value @ Value::Table(_)) => return solium_json::Json::from_lua(&value),
            Ok(_) => {}
            Err(err) if err.to_string().contains(STOPPED) => {
                let _ = strike(lua, &strikes, &listener, event, &err)?;
            }
            Err(err) => tracing::error!(%err, event, "a listener failed"),
        }
    }
    Ok(None)
}

/// Turn a Lua error into one of ours.
///
/// `mlua::Error` is not `Send + Sync`, so it cannot be an `anyhow` source
/// directly; the message is what matters here anyway, and a Lua error already
/// carries its own traceback in it.
pub fn failed(context: &'static str) -> impl FnOnce(mlua::Error) -> anyhow::Error {
    move |err| anyhow::anyhow!("{context}: {err}")
}

/// Call a run's or a timer's function under the handler deadline. One
/// stopped there is struck as a listener is, by function, and one struck
/// three times is not called again until the next reload: true then.
pub fn call_struck(
    lua: &Lua,
    strikes: &Table,
    run: &mlua::Function,
    args: impl mlua::IntoLuaMulti,
    event: &str,
) -> mlua::Result<bool> {
    if strikes.get::<Option<u32>>(run)?.unwrap_or(0) >= STRIKES {
        return Ok(true);
    }
    start_clock(lua);
    match run.call::<()>(args) {
        Ok(()) => Ok(false),
        Err(err) if err.to_string().contains(STOPPED) => strike(lua, strikes, run, event, &err),
        Err(err) => {
            tracing::error!(%err, event, "a function failed");
            Ok(false)
        }
    }
}
