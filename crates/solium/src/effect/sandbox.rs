//! One locked-down Lua per effect (Ruling 4).
//!
//! `math`, `table` and `string`, and nothing that reaches a file, the
//! configuration, the compositor or another effect:
//! `tests::effect_lua_reaches_no_sol_io_os_package_or_files`. `pcall` and
//! `xpcall` go with the file functions, because each would catch the
//! budget's stop and loop (milestone 1 Task 11 had to wrap them for the
//! configuration's Lua; an effect has no use for them).
//!
//! Every call runs under an instruction hook installed for that call only,
//! every 10 000 instructions, checking a deadline:
//! `tests::an_effect_that_never_returns_is_stopped_within_its_budget`. Not a
//! standing hook, because Lua 5.4 takes its slow path on every instruction
//! while one is installed (\[16\] §4's 36.5 µs against 18 µs), and a per-frame
//! `mesh` call would pay it whether or not anything runs long.
//!
//! What an effect returned is read with raw gets only, outside any call, so
//! reading it runs none of the effect's Lua: a metatable on a param cannot
//! loop where no budget is installed
//! (`tests::reading_the_table_runs_none_of_its_metamethods`).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mlua::{Lua, LuaOptions, StdLib, Table, Value as LuaValue};
use solium_effects::spec::{self, EffectSpec, Extent, Given, GridSpec, ParamSpec, Rung, Value};

use super::host::Problem;

/// How long a call may run, and how much memory a state may hold.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Budget {
    pub(crate) time: Duration,
    pub(crate) memory: usize,
}

impl Budget {
    /// A load-time call: `effect.lua`, `stages`, `reach`, `bleed` (the
    /// 2026-10-01 "Lua at 100 ms" decision).
    pub(crate) const LOAD: Self = Self {
        time: Duration::from_millis(100),
        memory: 16 << 20,
    };
    /// A per-frame `mesh` call (Task 26).
    #[cfg_attr(test, expect(dead_code, reason = "Task 26's mesh call is its reader"))]
    pub(crate) const MESH: Duration = Duration::from_millis(2);
}

const EVERY: u32 = 10_000;
const STOPPED: &str = "the effect ran past its budget and was stopped";

/// When the call running now must have returned by.
#[derive(Debug, Default)]
struct Due(Option<Instant>);

pub(crate) struct Sandbox {
    lua: Lua,
    effect: String,
    file: PathBuf,
    returned: Option<mlua::RegistryKey>,
    /// Set when a call was stopped by its budget: mlua 0.12.1 leaves the
    /// error object in the stopped frame's locals, so this state runs nothing
    /// more; `Host::revive` (Task 26) makes a new one after the frame, or a
    /// reload does (Ruling 4).
    /// `tests::an_effect_that_never_returns_is_stopped_within_its_budget`.
    poisoned: std::cell::Cell<bool>,
}

impl std::fmt::Debug for Sandbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sandbox")
            .field("effect", &self.effect)
            .field("file", &self.file)
            .finish_non_exhaustive()
    }
}

impl Sandbox {
    pub(crate) fn new(effect: &str, file: &Path) -> Result<Self, Problem> {
        let fail = |err: mlua::Error| {
            Problem::error(
                effect,
                file,
                None,
                format!("could not make the effect's Lua: {err}"),
            )
        };
        let lua = Lua::new_with(
            StdLib::MATH | StdLib::TABLE | StdLib::STRING,
            LuaOptions::new(),
        )
        .map_err(fail)?;
        lua.set_memory_limit(Budget::LOAD.memory).map_err(fail)?;
        harden(&lua, effect, file).map_err(fail)?;
        lua.set_app_data(Due::default());
        Ok(Self {
            lua,
            effect: effect.to_owned(),
            file: file.to_owned(),
            returned: None,
            poisoned: std::cell::Cell::new(false),
        })
    }

    #[cfg_attr(
        test,
        expect(
            dead_code,
            reason = "Task 8's stages and Task 26's mesh call through it"
        )
    )]
    pub(crate) fn lua(&self) -> &Lua {
        &self.lua
    }

    /// Whether a call was stopped by its budget, so this state must not run
    /// again. `tests::an_effect_that_never_returns_is_stopped_within_its_budget`.
    pub(crate) fn poisoned(&self) -> bool {
        self.poisoned.get()
    }

    /// Run `call` under a deadline `limit` from now.
    /// `tests::an_effect_that_never_returns_is_stopped_within_its_budget`.
    pub(crate) fn budgeted<R>(
        &self,
        limit: Duration,
        call: impl FnOnce(&Lua) -> mlua::Result<R>,
    ) -> Result<R, Problem> {
        if self.poisoned.get() {
            return Err(Problem::error(
                &self.effect,
                &self.file,
                None,
                format!("{STOPPED} earlier, and is not run again until a reload"),
            ));
        }
        if let Ok(Some(mut due)) = self.lua.try_app_data_mut::<Due>() {
            due.0 = Some(Instant::now() + limit);
        }
        let hooked = self.lua.set_global_hook(
            mlua::HookTriggers::new().every_nth_instruction(EVERY),
            |lua, _debug| {
                let late = lua
                    .try_app_data_ref::<Due>()
                    .ok()
                    .flatten()
                    .and_then(|due| due.0)
                    .is_some_and(|due| Instant::now() >= due);
                if late {
                    return Err(mlua::Error::runtime(STOPPED));
                }
                Ok(mlua::VmState::Continue)
            },
        );
        let result = hooked.and_then(|()| call(&self.lua));
        self.lua.remove_global_hook();
        if let Ok(Some(mut due)) = self.lua.try_app_data_mut::<Due>() {
            due.0 = None;
        }
        result.map_err(|err| {
            let mut problem = self.problem(&err);
            if problem.message.contains(STOPPED) {
                self.poisoned.set(true);
                problem.message = format!("{STOPPED} ({} ms)", limit.as_millis());
            }
            problem
        })
    }

    fn problem(&self, err: &mlua::Error) -> Problem {
        let (line, message) = located(&err.to_string(), &self.file);
        Problem::error(&self.effect, &self.file, line, message)
    }

    /// Run `effect.lua` and read what it returned.
    pub(crate) fn load_effect(&mut self) -> Result<EffectSpec, Problem> {
        let text = std::fs::read_to_string(&self.file).map_err(|err| {
            Problem::error(
                &self.effect,
                &self.file,
                None,
                format!("cannot read it: {err}"),
            )
        })?;
        let name = format!("@{}", self.file.display());
        let returned: Table = self.budgeted(Budget::LOAD.time, |lua| {
            lua.load(text.as_str()).set_name(name).eval::<Table>()
        })?;
        let spec = read_spec(&returned)
            .map_err(|message| Problem::error(&self.effect, &self.file, None, message))?;
        self.returned = Some(
            self.lua
                .create_registry_value(returned)
                .map_err(|err| self.problem(&err))?,
        );
        Ok(spec)
    }

    /// The table `effect.lua` returned.
    pub(crate) fn returned(&self) -> Result<Table, Problem> {
        let key = self.returned.as_ref().ok_or_else(|| {
            Problem::error(
                &self.effect,
                &self.file,
                None,
                "the effect was never loaded".to_owned(),
            )
        })?;
        self.lua
            .registry_value(key)
            .map_err(|err| self.problem(&err))
    }

    /// The params as the `p` table every function of them is called with.
    /// `tests::reach_as_a_function_is_called_with_the_params`.
    pub(crate) fn params_table(&self, params: &[(String, Value)]) -> Result<Table, Problem> {
        let build = || -> mlua::Result<Table> {
            let table = self.lua.create_table()?;
            for (name, value) in params {
                match value {
                    Value::Number(number) => table.raw_set(name.as_str(), *number)?,
                    Value::Int(int) => table.raw_set(name.as_str(), *int)?,
                    Value::Bool(yes) => table.raw_set(name.as_str(), *yes)?,
                    Value::Vec4(four) => table.raw_set(
                        name.as_str(),
                        self.lua.create_sequence_from(four.iter().copied())?,
                    )?,
                    Value::Word(word) => table.raw_set(name.as_str(), word.as_str())?,
                }
            }
            Ok(table)
        };
        build().map_err(|err| self.problem(&err))
    }

    /// `reach` or `bleed` for these params. `tests::reach_as_a_function_is_called_with_the_params`.
    pub(crate) fn extent(&self, key: &str, params: &[(String, Value)]) -> Result<f64, Problem> {
        let value: LuaValue = self
            .returned()?
            .raw_get(key)
            .map_err(|err| self.problem(&err))?;
        let problem = |message: String| Problem::error(&self.effect, &self.file, None, message);
        match value {
            LuaValue::Nil => Ok(0.0),
            #[expect(clippy::cast_precision_loss, reason = "a number of pixels")]
            LuaValue::Integer(int) => Ok(int as f64),
            LuaValue::Number(number) => Ok(number),
            LuaValue::Function(function) => {
                let p = self.params_table(params)?;
                let got: f64 = self.budgeted(Budget::LOAD.time, |_| function.call(p))?;
                if got.is_finite() && got >= 0.0 {
                    Ok(got)
                } else {
                    Err(problem(format!(
                        "`{key}` gave {got}, not a number of pixels"
                    )))
                }
            }
            _ => Err(problem(format!("`{key}` is not a number or a function"))),
        }
    }
}

/// Take out of the base library what reaches files or catches the stop, make
/// `print` a debug line naming the effect, and refuse finalisers.
/// `tests::effect_lua_reaches_no_sol_io_os_package_or_files`.
fn harden(lua: &Lua, effect: &str, file: &Path) -> mlua::Result<()> {
    let globals = lua.globals();
    for name in [
        "dofile",
        "loadfile",
        "load",
        "pcall",
        "xpcall",
        "collectgarbage",
    ] {
        globals.raw_set(name, LuaValue::Nil)?;
    }
    let string: Table = globals.raw_get("string")?;
    string.raw_set("dump", LuaValue::Nil)?;
    // Lua 5.4 runs `__gc` in a GC step a Rust allocation triggers and in
    // `lua_close`, outside any hooked call, where no budget stops a loop; and
    // it marks a table for finalisation only if `__gc` is in its metatable
    // when `setmetatable` is called, so refusing it here is enough. The
    // refusal names the caller's line, as Lua's own errors do.
    // `tests::a_metatable_with_gc_is_refused_and_dropping_the_state_returns`.
    let original: mlua::Function = globals.raw_get("setmetatable")?;
    let path = file.display().to_string();
    let guarded = lua.create_function(move |lua, (table, meta): (Table, Option<Table>)| {
        if let Some(meta) = &meta
            && !meta.raw_get::<LuaValue>("__gc")?.is_nil()
        {
            let at = lua
                .inspect_stack(1, |debug| debug.current_line())
                .flatten()
                .map(|line| format!("{path}:{line}: "))
                .unwrap_or_default();
            return Err(mlua::Error::runtime(format!(
                "{at}an effect may not give a table a `__gc` finaliser: it would run where no budget can stop it"
            )));
        }
        original.call::<Table>((table, meta))
    })?;
    globals.raw_set("setmetatable", guarded)?;
    let named = effect.to_owned();
    let print = lua.create_function(move |_, args: mlua::Variadic<LuaValue>| {
        let words: Vec<String> = args
            .iter()
            .map(|value| value.to_string().unwrap_or_default())
            .collect();
        tracing::debug!(effect = %named, "{}", words.join("\t"));
        Ok(())
    })?;
    globals.raw_set("print", print)
}

/// `<file>:<line>: <message>` split into the line and the message; anything
/// else is the whole first line. Lua writes a chunk name longer than 59 bytes
/// as `...` and its tail, which is found too, so long as the tail names the
/// effect's folder. `tests::a_located_message_is_split_at_the_file`,
/// `tests::a_lua_error_in_a_long_path_names_its_line`.
pub(crate) fn located(text: &str, file: &Path) -> (Option<u32>, String) {
    let first = |text: &str| text.lines().next().unwrap_or(text).to_owned();
    let path = file.display().to_string();
    let whole = text.find(&format!("{path}:")).map(|at| at + path.len() + 1);
    let after = whole.or_else(|| {
        text.match_indices("...").find_map(|(at, dots)| {
            let rest = text.get(at + dots.len()..)?;
            let tail = rest.get(..rest.find(':')?)?;
            (tail.contains('/') && path.ends_with(tail)).then_some(at + dots.len() + tail.len() + 1)
        })
    });
    if let Some(rest) = after.and_then(|at| text.get(at..)) {
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if let (Some(Ok(line)), Some(message)) = (
            rest.get(..digits).map(str::parse::<u32>),
            rest.get(digits..).and_then(|rest| rest.strip_prefix(": ")),
        ) {
            return (Some(line), first(message));
        }
    }
    (None, first(text))
}

/// The table `effect.lua` returned, as an [`EffectSpec`]. The keys are walked
/// in sorted order, so the first error a table has is always the same one.
/// `tests::an_unknown_key_is_refused`, `tests::motion_is_refused_until_tokens_exist`,
/// `tests::params_are_read_with_their_kinds_ranges_and_defaults`.
fn read_spec(table: &Table) -> Result<EffectSpec, String> {
    let mut entries: Vec<(String, LuaValue)> = Vec::new();
    for pair in table.pairs::<LuaValue, LuaValue>() {
        let (key, value) = pair.map_err(|err| err.to_string())?;
        let LuaValue::String(key) = key else {
            return Err("effect.lua's table has a key that is not a name".to_owned());
        };
        entries.push((
            key.to_str().map_err(|err| err.to_string())?.to_string(),
            value,
        ));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut spec = EffectSpec::default();
    for (key, value) in entries {
        match key.as_str() {
            "api" => spec.api = whole(&value).ok_or("`api` must be a whole number")?,
            "inputs" => spec.inputs = words(&value, "inputs")?,
            "params" => spec.params = params(&value)?,
            "reach" => spec.reach = extent(&value, "reach")?,
            "bleed" => spec.bleed = extent(&value, "bleed")?,
            "stages" => {
                spec.stages = match value {
                    LuaValue::Table(_) => Given::List,
                    LuaValue::Function(_) => Given::Function,
                    _ => return Err("`stages` is a list or a function of the params".to_owned()),
                }
            }
            "frag" => spec.frag = Some(word(&value, "frag")?),
            "pixels" => spec.pixels = Some(word(&value, "pixels")?),
            "fallback" => spec.fallback = rungs(&value)?,
            "duration" => {
                spec.duration = Some(float(&value).ok_or("`duration` is a number of milliseconds")?)
            }
            "easing" => {
                let name = word(&value, "easing")?;
                if solium_animation::Curve::from_name(&name).is_none() {
                    return Err(format!(
                        "`easing = \"{name}\"` is not a curve this Solium has"
                    ));
                }
                spec.easing = Some(name);
            }
            "grid" => spec.grid = Some(grid(&value)?),
            "mesh" => {
                let LuaValue::Function(_) = value else {
                    return Err("`mesh` is a function(t, cols, rows, out)".to_owned());
                };
                spec.mesh = true;
            }
            "motion" => {
                return Err(
                    "`motion`: motion tokens arrive with P14; give `duration` and `easing`"
                        .to_owned(),
                );
            }
            other => {
                let meant = spec::nearest(other, &spec::KEYS)
                    .map(|meant| format!("; did you mean `{meant}`?"))
                    .unwrap_or_default();
                return Err(format!(
                    "`{other}` is not a key an effect.lua may return{meant}"
                ));
            }
        }
    }
    spec.check()?;
    Ok(spec)
}

/// An `Integer`, or a `Number` with no fraction, that fits a `u32`.
fn whole(value: &LuaValue) -> Option<u32> {
    match value {
        LuaValue::Integer(int) => u32::try_from(*int).ok(),
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a whole number in u32's range, checked just here"
        )]
        LuaValue::Number(number)
            if number.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(number) =>
        {
            Some(*number as u32)
        }
        _ => None,
    }
}

/// An `Integer`, or a finite `Number`.
fn float(value: &LuaValue) -> Option<f64> {
    match value {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a number from a file, far below 2^53"
        )]
        LuaValue::Integer(int) => Some(*int as f64),
        LuaValue::Number(number) if number.is_finite() => Some(*number),
        _ => None,
    }
}

fn word(value: &LuaValue, key: &str) -> Result<String, String> {
    let LuaValue::String(word) = value else {
        return Err(format!("`{key}` is a string"));
    };
    Ok(word.to_str().map_err(|err| err.to_string())?.to_string())
}

/// A list of strings, and nothing but a list.
fn words(value: &LuaValue, key: &str) -> Result<Vec<String>, String> {
    let LuaValue::Table(list) = value else {
        return Err(format!("`{key}` is a list of names"));
    };
    if list.pairs::<LuaValue, LuaValue>().count() != list.raw_len() {
        return Err(format!("`{key}` is a list of names, with no other keys"));
    }
    list.sequence_values::<LuaValue>()
        .map(|each| word(&each.map_err(|err| err.to_string())?, key))
        .collect()
}

fn extent(value: &LuaValue, key: &str) -> Result<Extent, String> {
    match value {
        LuaValue::Nil => Ok(Extent::Fixed(0.0)),
        LuaValue::Function(_) => Ok(Extent::Function),
        other => float(other)
            .map(Extent::Fixed)
            .ok_or_else(|| format!("`{key}` is a number of pixels, or a function of the params")),
    }
}

/// `{ along = a, across = b }`, which turns with the axis, or `{ cols, rows }`.
fn grid(value: &LuaValue) -> Result<GridSpec, String> {
    const SHAPE: &str =
        "`grid` is { along = <n>, across = <n> } or { <cols>, <rows> }, each 1 or more";
    let LuaValue::Table(table) = value else {
        return Err(SHAPE.to_owned());
    };
    let count = |got: mlua::Result<LuaValue>| -> Result<u32, String> {
        whole(&got.map_err(|err| err.to_string())?)
            .filter(|&n| n >= 1)
            .ok_or_else(|| SHAPE.to_owned())
    };
    if table.pairs::<LuaValue, LuaValue>().count() != 2 {
        return Err(SHAPE.to_owned());
    }
    if table.raw_len() == 0 {
        Ok(GridSpec::Turning {
            along: count(table.raw_get("along"))?,
            across: count(table.raw_get("across"))?,
        })
    } else {
        Ok(GridSpec::Fixed {
            cols: count(table.raw_get(1))?,
            rows: count(table.raw_get(2))?,
        })
    }
}

/// A param's value as a rung or a default gives it: a number, a whole number,
/// a boolean, a word, or a table of four numbers. `what` names it in an error.
fn value_of(value: &LuaValue, what: &str) -> Result<Value, String> {
    match value {
        LuaValue::Integer(int) => Ok(Value::Int(*int)),
        LuaValue::Number(number) => Ok(Value::Number(*number)),
        LuaValue::Boolean(yes) => Ok(Value::Bool(*yes)),
        LuaValue::String(word) => Ok(Value::Word(
            word.to_str().map_err(|err| err.to_string())?.to_string(),
        )),
        LuaValue::Table(four) => {
            let numbers: Vec<f64> = four
                .sequence_values::<f64>()
                .collect::<mlua::Result<_>>()
                .map_err(|err| err.to_string())?;
            let [a, b, c, d] = numbers[..] else {
                return Err(format!("{what}: a table is four numbers"));
            };
            Ok(Value::Vec4([a, b, c, d]))
        }
        _ => Err(format!(
            "{what} is a number, a boolean, a word or four numbers"
        )),
    }
}

/// `fallback`: cheaper rungs, each a table of param overrides or another
/// effect's name, in order.
fn rungs(value: &LuaValue) -> Result<Vec<Rung>, String> {
    const SHAPE: &str =
        "`fallback` is a list of rungs, each { <param> = <value>, … } or an effect's name";
    let LuaValue::Table(list) = value else {
        return Err(SHAPE.to_owned());
    };
    if list.pairs::<LuaValue, LuaValue>().count() != list.raw_len() {
        return Err(SHAPE.to_owned());
    }
    let mut read = Vec::new();
    for rung in list.sequence_values::<LuaValue>() {
        match rung.map_err(|err| err.to_string())? {
            LuaValue::String(name) => read.push(Rung::Effect(
                name.to_str().map_err(|err| err.to_string())?.to_string(),
            )),
            LuaValue::Table(overrides) => {
                let mut params = Vec::new();
                for pair in overrides.pairs::<LuaValue, LuaValue>() {
                    let (name, value) = pair.map_err(|err| err.to_string())?;
                    let name = word(&name, "a rung's param").map_err(|_| SHAPE.to_owned())?;
                    let value = value_of(&value, &format!("`fallback`'s `{name}`"))?;
                    params.push((name, value));
                }
                params.sort_by(|a, b| a.0.cmp(&b.0));
                read.push(Rung::Params(params));
            }
            _ => return Err(SHAPE.to_owned()),
        }
    }
    Ok(read)
}

/// The keys a param's table may have besides its default, `[1]`.
const PARAM_KEYS: [&str; 3] = ["min", "max", "int"];

/// `params`: name = { default, min =, max =, int = }, sorted by name, so their
/// uniform order and their `p` table never depend on Lua's table order.
/// `tests::params_are_read_with_their_kinds_ranges_and_defaults`,
/// `tests::a_params_unknown_key_is_refused`.
fn params(value: &LuaValue) -> Result<Vec<(String, ParamSpec)>, String> {
    const SHAPE: &str = "`params` is a table of name = { default, min =, max =, int = }";
    let LuaValue::Table(table) = value else {
        return Err(SHAPE.to_owned());
    };
    let mut read = Vec::new();
    for pair in table.pairs::<LuaValue, LuaValue>() {
        let (name, param) = pair.map_err(|err| err.to_string())?;
        let (LuaValue::String(name), LuaValue::Table(param)) = (name, param) else {
            return Err(format!("{SHAPE}: each is name = {{ <default>, … }}"));
        };
        let name = name.to_str().map_err(|err| err.to_string())?.to_string();
        for pair in param.pairs::<LuaValue, LuaValue>() {
            let (key, _) = pair.map_err(|err| err.to_string())?;
            match key {
                LuaValue::Integer(1) => {}
                LuaValue::String(key) => {
                    let key = key.to_str().map_err(|err| err.to_string())?.to_string();
                    if !PARAM_KEYS.contains(&key.as_str()) {
                        let meant = spec::nearest(&key, &PARAM_KEYS)
                            .map(|meant| format!("; did you mean `{meant}`?"))
                            .unwrap_or_default();
                        return Err(format!(
                            "param `{name}`: `{key}` is not min, max or int{meant}"
                        ));
                    }
                }
                _ => {
                    return Err(format!(
                        "param `{name}`: one default, then min =, max = and int ="
                    ));
                }
            }
        }
        let field = |key: &str| -> Result<LuaValue, String> {
            param
                .raw_get::<LuaValue>(key)
                .map_err(|err| err.to_string())
        };
        let int = match field("int")? {
            LuaValue::Nil => false,
            LuaValue::Boolean(yes) => yes,
            _ => return Err(format!("param `{name}`: `int` is true or false")),
        };
        let first: LuaValue = param.raw_get(1).map_err(|err| err.to_string())?;
        let default = match first {
            LuaValue::Integer(whole) if int => Value::Int(whole),
            #[expect(clippy::cast_precision_loss, reason = "a param, far below 2^53")]
            LuaValue::Integer(whole) => Value::Number(whole as f64),
            LuaValue::Number(number) if !int => Value::Number(number),
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a whole number checked just here"
            )]
            LuaValue::Number(number) if number.fract() == 0.0 => Value::Int(number as i64),
            LuaValue::Number(_) => {
                return Err(format!(
                    "param `{name}`: an `int` param's default is a whole number"
                ));
            }
            LuaValue::Nil => {
                return Err(format!(
                    "param `{name}` has no default: write `{name} = {{ <default> }}`"
                ));
            }
            other => value_of(&other, &format!("param `{name}`"))?,
        };
        let bound = |key: &str| -> Result<Option<f64>, String> {
            match field(key)? {
                LuaValue::Nil => Ok(None),
                other => float(&other)
                    .map(Some)
                    .ok_or_else(|| format!("param `{name}`: `{key}` is a number")),
            }
        };
        let (min, max) = (bound("min")?, bound("max")?);
        read.push((name, ParamSpec { default, min, max }));
    }
    read.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(read)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use solium_effects::spec::{Given, Value};

    use super::{Budget, Sandbox, located};
    use crate::effect::host::tests::{folder, scratch};

    fn sandbox(name: &str, lua: &str) -> (Sandbox, std::path::PathBuf) {
        let place = scratch(&format!("sandbox-{name}"));
        let dir = folder(
            &place,
            name,
            lua,
            &[(
                "effect.frag",
                "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n",
            )],
        );
        (
            Sandbox::new(name, &dir.join("effect.lua")).expect("a sandbox"),
            place,
        )
    }

    /// **`effect.lua` reaches no `sol`, no files and no way to catch its own
    /// stop.** Every one of these names is `nil` inside, and `math`, `table`
    /// and `string` are there.
    #[test]
    fn effect_lua_reaches_no_sol_io_os_package_or_files() {
        let probe = r#"
            local gone = {}
            for _, name in ipairs({ "sol", "io", "os", "package", "require", "dofile", "loadfile",
                                    "load", "pcall", "xpcall", "collectgarbage", "debug", "coroutine" }) do
                if _G[name] ~= nil then gone[#gone + 1] = name end
            end
            assert(#gone == 0, "reachable: " .. table.concat(gone, ", "))
            assert(string.dump == nil, "string.dump")
            assert(math.sin and table.insert and string.format)
            local m = setmetatable({}, { __index = function() return 1 end })
            assert(m.anything == 1, "a metatable without __gc is the effect's own business")
            return { api = 1, frag = "effect.frag" }
        "#;
        let (mut sandbox, place) = sandbox("reach", probe);
        sandbox.load_effect().expect("nothing reachable");
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A finaliser cannot be installed.** Lua 5.4 runs `__gc` in a GC step
    /// that a Rust-side allocation triggers and in `lua_close` when a state is
    /// dropped (at every reload that replaces a version), outside any hooked
    /// call, where no budget can stop a loop in it. So `setmetatable` refuses
    /// a metatable carrying `__gc`, and dropping the state returns at once.
    #[test]
    fn a_metatable_with_gc_is_refused_and_dropping_the_state_returns() {
        let lua = "setmetatable({}, { __gc = function() while true do end end })\nreturn { api = 1, frag = 'effect.frag' }";
        let (mut state, place) = sandbox("finaliser", lua);
        let problem = state.load_effect().expect_err("refused");
        assert!(problem.message.contains("__gc"), "{problem:?}");
        assert_eq!(problem.line, Some(1), "the line that asked: {problem:?}");
        let started = Instant::now();
        drop(state);
        assert!(
            started.elapsed() < Budget::LOAD.time + Duration::from_millis(400),
            "{:?}",
            started.elapsed()
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// **An effect that never returns is stopped within its budget**, plus one
    /// hook interval, and is a problem rather than a hang. The state it was
    /// stopped in runs nothing more: mlua 0.12.1 leaves the stop's error in
    /// the stopped frame's locals (Ruling 4).
    #[test]
    fn an_effect_that_never_returns_is_stopped_within_its_budget() {
        let (mut sandbox, place) = sandbox("forever", "while true do end");
        let started = Instant::now();
        let problem = sandbox.load_effect().expect_err("stopped");
        assert!(
            started.elapsed() < Budget::LOAD.time + Duration::from_millis(400),
            "{:?}",
            started.elapsed()
        );
        assert!(problem.message.contains("budget"), "{problem:?}");
        assert!(sandbox.poisoned(), "a stopped state is poisoned");
        let again = sandbox
            .budgeted(Budget::LOAD.time, |_| Ok(()))
            .expect_err("nothing more runs in it");
        assert!(again.message.contains("not run again"), "{again:?}");
        let _ = std::fs::remove_dir_all(place);
    }

    /// **An effect that allocates without bound is stopped** by the memory
    /// limit, as an error and not an abort.
    #[test]
    fn an_effect_that_allocates_without_bound_is_stopped() {
        let (mut sandbox, place) = sandbox(
            "greedy",
            "local s = string.rep('x', 1 << 30) return { api = 1 }",
        );
        let problem = sandbox.load_effect().expect_err("stopped");
        assert!(problem.message.contains("memory"), "{problem:?}");
        assert!(!sandbox.poisoned(), "only a stop by the clock poisons");
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A Lua error names the file and the line**, so the overlay and
    /// `--check` can say `…/effect.lua:3: …`.
    #[test]
    fn a_lua_error_names_the_file_and_line() {
        let (mut sandbox, place) = sandbox("broken", "return {\n  api = 1,\n  frag = = 'x',\n}\n");
        let problem = sandbox.load_effect().expect_err("a syntax error");
        assert_eq!(problem.line, Some(3), "{problem:?}");
        assert!(problem.file.ends_with("broken/effect.lua"), "{problem:?}");
        let _ = std::fs::remove_dir_all(place);
    }

    /// **The line survives a long path.** Lua shortens a chunk name longer
    /// than 59 bytes to `...` and its tail in every message, so an
    /// `effect.lua` deep in a configuration directory reads
    /// `...ects/broken/effect.lua:3: …`, which still names its line.
    #[test]
    fn a_lua_error_in_a_long_path_names_its_line() {
        let place = scratch("sandbox-a-configuration-directory-a-long-way-down");
        let deep = place
            .join("home")
            .join("someone")
            .join("a-long-way-down")
            .join("solium");
        let dir = folder(
            &deep,
            "broken",
            "return {\n  api = 1,\n  frag = = 'x',\n}\n",
            &[],
        );
        let file = dir.join("effect.lua");
        assert!(file.display().to_string().len() > 59, "{}", file.display());
        let problem = Sandbox::new("broken", &file)
            .expect("a sandbox")
            .load_effect()
            .expect_err("a syntax error");
        assert_eq!(problem.line, Some(3), "{problem:?}");
        assert!(!problem.message.contains("..."), "{problem:?}");
        let _ = std::fs::remove_dir_all(place);
    }

    #[test]
    fn an_api_other_than_1_is_refused() {
        let (mut sandbox, place) = sandbox("future", "return { api = 2, frag = 'effect.frag' }");
        assert!(
            sandbox
                .load_effect()
                .expect_err("refused")
                .message
                .contains("api")
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// **An unknown key is refused by name**, with what was probably meant:
    /// a typo must not silently mean "default".
    #[test]
    fn an_unknown_key_is_refused() {
        let (mut sandbox, place) = sandbox("typo", "return { api = 1, stagse = {} }");
        let problem = sandbox.load_effect().expect_err("refused");
        assert!(
            problem.message.contains("stagse") && problem.message.contains("stages"),
            "{problem:?}"
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// `motion` is refused until P14, saying so (Ruling 21).
    #[test]
    fn motion_is_refused_until_tokens_exist() {
        let (mut sandbox, place) = sandbox(
            "token",
            "return { api = 1, frag = 'effect.frag', motion = 'emphasized' }",
        );
        assert!(
            sandbox
                .load_effect()
                .expect_err("refused")
                .message
                .contains("P14")
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// A key in a param's table other than its default, `min`, `max` and
    /// `int` is refused by name, as an unknown key of the file's is: `mn = 1`
    /// must not silently mean "no minimum".
    #[test]
    fn a_params_unknown_key_is_refused() {
        let (mut sandbox, place) = sandbox(
            "param-typo",
            "return { api = 1, frag = 'effect.frag', params = { passes = { 3, mn = 1 } } }",
        );
        let problem = sandbox.load_effect().expect_err("refused");
        assert!(
            problem.message.contains("`mn`") && problem.message.contains("`min`"),
            "{problem:?}"
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// **Reading what an effect returned runs none of its Lua.** It is read
    /// outside any budgeted call, so a metamethod run there could loop with
    /// no hook to stop it: every read is raw, and an `__index` that would
    /// fail is never reached, in a param's table or in the returned one.
    #[test]
    fn reading_the_table_runs_none_of_its_metamethods() {
        let lua = "local trap = { __index = function() error('a metamethod ran') end }
            return setmetatable({ api = 1, frag = 'effect.frag',
                params = { passes = setmetatable({ 3 }, trap) } }, trap)";
        let (mut sandbox, place) = sandbox("raw", lua);
        let spec = sandbox.load_effect().expect("read raw");
        assert_eq!(spec.params[0].1.default, Value::Number(3.0));
        assert_eq!(sandbox.extent("reach", &[]).expect("read raw"), 0.0);
        let _ = std::fs::remove_dir_all(place);
    }

    #[test]
    fn params_are_read_with_their_kinds_ranges_and_defaults() {
        let lua = "return { api = 1, frag = 'effect.frag', params = {
            passes = { 3, min = 1, max = 6, int = true }, offset = { 3 }, tint = { { 1, 0, 0, 1 } },
            on = { true }, mode = { 'soft' } } }";
        let (mut sandbox, place) = sandbox("kinds", lua);
        let spec = sandbox.load_effect().expect("loads");
        let kind = |name: &str| {
            spec.params
                .iter()
                .find(|(each, _)| each == name)
                .map(|(_, param)| param.default.clone())
        };
        assert_eq!(kind("passes"), Some(Value::Int(3)));
        assert_eq!(kind("offset"), Some(Value::Number(3.0)));
        assert_eq!(kind("tint"), Some(Value::Vec4([1.0, 0.0, 0.0, 1.0])));
        assert_eq!(kind("on"), Some(Value::Bool(true)));
        assert_eq!(kind("mode"), Some(Value::Word("soft".to_owned())));
        assert_eq!(spec.stages, Given::Absent);
        let _ = std::fs::remove_dir_all(place);
    }

    /// `reach` as a function of the params is called with them, at bind.
    #[test]
    fn reach_as_a_function_is_called_with_the_params() {
        let lua = "return { api = 1, frag = 'effect.frag', params = { passes = { 3, int = true }, offset = { 3 } },
            reach = function(p) return math.ceil(p.offset * 2 ^ (p.passes + 1)) end }";
        let (mut sandbox, place) = sandbox("reach-fn", lua);
        sandbox.load_effect().expect("loads");
        let reach = sandbox
            .extent(
                "reach",
                &[
                    ("passes".to_owned(), Value::Int(3)),
                    ("offset".to_owned(), Value::Number(3.0)),
                ],
            )
            .expect("called");
        assert!((reach - 48.0).abs() < f64::EPSILON, "{reach}");
        let _ = std::fs::remove_dir_all(place);
    }

    #[test]
    fn a_located_message_is_split_at_the_file() {
        let file = std::path::Path::new("/x/blur/effect.lua");
        assert_eq!(
            located(
                "runtime error: /x/blur/effect.lua:12: attempt to index nil",
                file
            ),
            (Some(12), "attempt to index nil".to_owned())
        );
        assert_eq!(
            located("memory error: not enough memory", file),
            (None, "memory error: not enough memory".to_owned())
        );
        let long =
            std::path::Path::new("/home/someone/a/long/way/down/solium/effects/blur/effect.lua");
        assert_eq!(
            located(
                "syntax error: ...g/way/down/solium/effects/blur/effect.lua:3: unexpected symbol near '='",
                long
            ),
            (Some(3), "unexpected symbol near '='".to_owned()),
            "Lua's shortened chunk name"
        );
        assert_eq!(
            located("runtime error: ...other/effect.lua:3: no", long),
            (None, "runtime error: ...other/effect.lua:3: no".to_owned()),
            "a tail of another file is not this one's"
        );
    }
}
