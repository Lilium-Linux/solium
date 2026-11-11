//! `sol.run`, `sol.after` and `sol.every`: programs run for their output,
//! and timers.

use std::time::Duration;

use mlua::{Lua, Table, Value};

use crate::*;

/// Ids for `sol.run{}`'s runs and for timers, counting up across reloads,
/// so an old configuration's id never names a new configuration's run or
/// timer. A timer's is also its place among timers due at the same moment.
pub static RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static TIMERS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How long a child may run when nothing says, in milliseconds.
pub const TIMEOUT_MS: u64 = 30_000;
/// How much of each stream is kept when nothing says, in bytes.
pub const MAX_OUTPUT: usize = 1 << 20;

/// What a `sol.run{}` takes when its options leave a key out: `config.run`,
/// through `sol.run_defaults`, or its own.
#[derive(Debug, Clone, Copy)]
pub struct RunDefaults {
    pub timeout: Duration,
    pub max_output: usize,
}

impl Default for RunDefaults {
    fn default() -> Self {
        Self {
            timeout: Duration::from_millis(TIMEOUT_MS),
            max_output: MAX_OUTPUT,
        }
    }
}

impl RunDefaults {
    /// `sol.run_defaults`'s table, read. Each key left out keeps its
    /// default, and one of the wrong kind is named in the log and keeps its
    /// default too.
    pub fn read(options: Option<&Table>) -> mlua::Result<Self> {
        let mut defaults = Self::default();
        let Some(options) = options else {
            return Ok(defaults);
        };
        match options.get::<Value>("timeout")? {
            Value::Nil => {}
            value => match number(&value)
                .filter(|ms| *ms > 0.0)
                .and_then(|ms| Duration::try_from_secs_f64(ms / 1000.0).ok())
            {
                Some(timeout) => defaults.timeout = timeout,
                None => tracing::warn!(
                    value = describe_value(&value),
                    "run.timeout is a number of milliseconds above 0; keeping the \
                     default, 30000"
                ),
            },
        }
        match options.get::<Value>("max_output")? {
            Value::Nil => {}
            value => match whole_bytes(&value) {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "checked to be a whole number from 0, and saturating past usize"
                )]
                Some(bytes) => defaults.max_output = bytes as usize,
                None => tracing::warn!(
                    value = describe_value(&value),
                    "run.max_output is a whole number of bytes, 0 or more; keeping the \
                     default, 1048576"
                ),
            },
        }
        Ok(defaults)
    }
}

/// The options `sol.run{}` reads, for its error about any other.
pub const RUN_OPTIONS: [&str; 6] = ["cwd", "env", "timeout", "max_output", "lines", "done"];

/// `sol.run({ argv }, options)` (#472): queue the run, and keep its `lines`
/// and `done` under its id until it is done.
/// `tests::sol_run_with_a_table_hands_its_output_to_done`.
pub fn run_read(lua: &Lua, argv: &Table, options: Option<&Table>) -> mlua::Result<()> {
    let argv = argv
        .sequence_values::<String>()
        .collect::<mlua::Result<Vec<String>>>()?;
    if argv.is_empty() {
        return Err(mlua::Error::runtime("sol.run needs a program to run"));
    }
    let empty = lua.create_table()?;
    let options = options.unwrap_or(&empty);
    for pair in options.pairs::<Value, Value>() {
        let (key, _) = pair?;
        let key = match &key {
            Value::String(key) => key.to_string_lossy(),
            other => format!("{other:?}"),
        };
        if !RUN_OPTIONS.contains(&key.as_str()) {
            return Err(mlua::Error::runtime(format!(
                "sol.run has no option `{key}`; it takes {}",
                RUN_OPTIONS.join(", ")
            )));
        }
    }
    let cwd: Option<String> = options.get("cwd")?;
    let mut env = Vec::new();
    if let Some(table) = options.get::<Option<Table>>("env")? {
        for pair in table.pairs::<String, Value>() {
            let (name, value) = pair?;
            let value = match value {
                Value::Boolean(false) => None,
                Value::String(value) => Some(value.to_string_lossy()),
                Value::Integer(value) => Some(value.to_string()),
                Value::Number(value) => Some(value.to_string()),
                _ => {
                    return Err(mlua::Error::runtime(format!(
                        "sol.run's env.{name} is a string, or false to remove it"
                    )));
                }
            };
            env.push((name, value));
        }
        env.sort();
    }
    let defaults = lua
        .try_app_data_ref::<RunDefaults>()
        .ok()
        .flatten()
        .map(|defaults| *defaults)
        .unwrap_or_default();
    let timeout = match options.get::<Option<f64>>("timeout")? {
        None => defaults.timeout,
        Some(ms) if ms > 0.0 => Duration::try_from_secs_f64(ms / 1000.0)
            .map_err(|_| mlua::Error::runtime("sol.run's timeout is too long"))?,
        Some(_) => {
            return Err(mlua::Error::runtime(
                "sol.run's timeout is a number of milliseconds above 0",
            ));
        }
    };
    let max_output = match options.get::<Option<f64>>("max_output")? {
        None => defaults.max_output,
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "checked to be a whole number from 0, and saturating past usize"
        )]
        Some(bytes) if bytes >= 0.0 && bytes.fract() == 0.0 => bytes as usize,
        Some(_) => {
            return Err(mlua::Error::runtime(
                "sol.run's max_output is a whole number of bytes, 0 or more",
            ));
        }
    };
    let lines: Option<mlua::Function> = options.get("lines")?;
    let done: Option<mlua::Function> = options.get("done")?;
    let id = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let wants_lines = lines.is_some();
    with_pending(lua, |pending| {
        pending.push(solium_wm_protocol::Command::Run(solium_wm_protocol::Run {
            id,
            argv,
            cwd,
            env: env
                .into_iter()
                .map(|(name, value)| solium_wm_protocol::EnvChange { name, value })
                .collect(),
            timeout_ms: to_wire::millis(timeout),
            max_output: u64::try_from(max_output).unwrap_or(u64::MAX),
            lines: wants_lines,
        }));
    })?;
    if lines.is_some() || done.is_some() {
        let kept = lua.create_table()?;
        kept.set("lines", lines)?;
        kept.set("done", done)?;
        let runs: Table = lua.globals().get::<Table>("sol")?.get("_runs")?;
        runs.set(id, kept)?;
    }
    Ok(())
}

/// `sol.after` and `sol.every` (#472): keep `run` under a new timer's id,
/// queue the timer, and hand back its handle.
/// `tests::timers_run_in_the_order_they_fall_due`.
pub fn set_timer(
    lua: &Lua,
    ms: f64,
    run: mlua::Function,
    every: bool,
) -> mlua::Result<TimerHandle> {
    let name = if every { "sol.every" } else { "sol.after" };
    if ms.is_nan() || (every && ms <= 0.0) {
        return Err(mlua::Error::runtime(if every {
            "sol.every's period is a number of milliseconds above 0"
        } else {
            "sol.after's delay is a number of milliseconds"
        }));
    }
    let after = Duration::try_from_secs_f64(ms.max(0.0) / 1000.0)
        .map_err(|_| mlua::Error::runtime(format!("{name}: {ms} ms is too long")))?;
    let id = TIMERS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    with_pending(lua, |pending| {
        pending.push(solium_wm_protocol::Command::Timer(
            solium_wm_protocol::Timer {
                id,
                after_ms: to_wire::millis(after),
                period_ms: every.then_some(to_wire::millis(after)),
            },
        ));
    })?;
    let timers: Table = lua.globals().get::<Table>("sol")?.get("_timers")?;
    timers.set(id, run)?;
    Ok(TimerHandle(id))
}

/// What `sol.after` and `sol.every` hand back: the timer, to cancel.
#[derive(Debug, Clone, Copy)]
pub struct TimerHandle(pub u64);

impl mlua::UserData for TimerHandle {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        // Its function is let go here, so it cannot run again whatever the
        // compositor has queued; the compositor is told when it can be. A
        // timer cancelled where no command can be queued is forgotten by the
        // compositor the next time it falls due and finds no function.
        // `tests::a_cancelled_timer_does_not_run`.
        methods.add_method("cancel", |lua, this, ()| {
            let timers: Table = lua.globals().get::<Table>("sol")?.get("_timers")?;
            if timers.get::<Value>(this.0)?.is_nil() {
                return Ok(());
            }
            timers.set(this.0, Value::Nil)?;
            let _ = with_pending(lua, |pending| {
                pending.push(solium_wm_protocol::Command::TimerCancel(this.0));
            });
            Ok(())
        });
    }
}
