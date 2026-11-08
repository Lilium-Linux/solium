//! `sol.read`, `sol.write`, `sol.watch` and `sol.store`.

use mlua::{Lua, Table, Value};

use crate::json::Json;
use crate::script::lua::convert::describe_value;
use crate::script::lua::*;

/// Ids for reads, writes and watches (#473), counting up across reloads as
/// `RUNS` and `TIMERS` do.
pub(crate) static FILES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How much `sol.read` reads when `files.max_read` does not say: 4 MiB.
pub(crate) const MAX_READ: usize = 4 << 20;
/// The largest store file that is read when `files.store_max` does not
/// say: 1 MiB. A store is settings and small state, read when it is first
/// asked for, so a bigger one is taken for a broken one.
pub(crate) const MAX_STORE: u64 = 1 << 20;

/// What `config.files` says of this window manager's own reads, through
/// `sol.files` (#473). How often the stores are saved is the compositor's.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FileSettings {
    /// The most a `sol.read` reads, in bytes.
    pub(crate) max_read: usize,
    /// The largest store file that is read, in bytes.
    pub(crate) store_max: u64,
}

impl Default for FileSettings {
    fn default() -> Self {
        Self {
            max_read: MAX_READ,
            store_max: MAX_STORE,
        }
    }
}

impl FileSettings {
    /// `sol.files`'s table, read. Each key left out keeps its default, and
    /// one of the wrong kind is named in the log and keeps its default too.
    pub(crate) fn read(options: Option<&Table>) -> mlua::Result<Self> {
        let mut settings = Self::default();
        let Some(options) = options else {
            return Ok(settings);
        };
        match options.get::<Value>("max_read")? {
            Value::Nil => {}
            value => match whole_bytes(&value) {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "checked to be a whole number from 0, and saturating past usize"
                )]
                Some(bytes) => settings.max_read = bytes as usize,
                None => tracing::warn!(
                    value = describe_value(&value),
                    "files.max_read is a whole number of bytes, 0 or more; keeping the \
                     default, 4194304"
                ),
            },
        }
        match options.get::<Value>("store_max")? {
            Value::Nil => {}
            value => match whole_bytes(&value) {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "checked to be a whole number from 0, and saturating past u64"
                )]
                Some(bytes) => settings.store_max = bytes as u64,
                None => tracing::warn!(
                    value = describe_value(&value),
                    "files.store_max is a whole number of bytes, 0 or more; keeping the \
                     default, 1048576"
                ),
            },
        }
        Ok(settings)
    }
}

/// A number of bytes as a setting writes it: a whole number, 0 or more.
pub(crate) fn whole_bytes(value: &Value) -> Option<f64> {
    number(value).filter(|bytes| *bytes >= 0.0 && bytes.fract() == 0.0)
}

/// The key paths of stores already named in the log for holding what
/// cannot be saved, so each is named once (#473).
#[derive(Debug, Default)]
pub(crate) struct StoreWarnings(pub(crate) std::collections::HashSet<String>);

/// Put what a store's file or a reload held into its table (#473): a key
/// there wins over the default, and a table of defaults with no list part is
/// filled key by key, so a default added since the file was saved is
/// still there. A list is taken whole.
/// `tests::a_store_takes_the_files_keys_and_keeps_the_defaults_it_lacks`.
pub(crate) fn fill_store(lua: &Lua, table: &Table, found: &crate::json::Json) -> mlua::Result<()> {
    use crate::json::Json;
    match found {
        Json::Object(fields) => {
            for (key, value) in fields {
                match (table.raw_get::<Value>(key.as_str())?, value) {
                    (Value::Table(inner), Json::Object(_)) if inner.raw_len() == 0 => {
                        fill_store(lua, &inner, value)?;
                    }
                    _ => table.raw_set(key.as_str(), value.to_lua(lua)?)?,
                }
            }
        }
        Json::List(items) => {
            for index in (1..=table.raw_len()).rev() {
                table.raw_set(index, Value::Nil)?;
            }
            for (index, item) in items.iter().enumerate() {
                table.raw_set(index + 1, item.to_lua(lua)?)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// What `sol.watch` hands back: the watch, to cancel (#473).
#[derive(Debug, Clone, Copy)]
pub(crate) struct WatchHandle(pub(crate) u64);

impl mlua::UserData for WatchHandle {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        // As a timer's `cancel`: its function is let go here, so it is not
        // called again whatever events are on their way, and the compositor
        // is told when it can be, or forgets the watch the next time it
        // fires and finds no function.
        // `tests::a_cancelled_watch_is_not_told`.
        methods.add_method("cancel", |lua, this, ()| {
            let watches: Table = lua.globals().get::<Table>("sol")?.get("_watches")?;
            if watches.get::<Value>(this.0)?.is_nil() {
                return Ok(());
            }
            watches.set(this.0, Value::Nil)?;
            let _ = with_pending(lua, |pending| {
                pending.push(solium_wm_protocol::Command::WatchCancel(this.0));
            });
            Ok(())
        });
    }
}

/// Each store's text as the compositor was last given it, loaded or saved,
/// so a `save-stores` answers with only the stores that changed.
#[derive(Debug, Default)]
pub(crate) struct StoresSent(pub(crate) std::collections::HashMap<String, String>);

/// A path a configuration names, made absolute: `~/` is the home
/// directory, and anything that is neither absolute nor under `~/` is
/// refused, since the window manager's working directory is nobody's.
/// `tests::a_path_is_absolute_or_under_home`.
pub(crate) fn expand(path: &str) -> Result<std::path::PathBuf, String> {
    if let Some(rest) = path.strip_prefix("~/") {
        let home = std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .ok_or_else(|| format!("{path}: HOME is not set, so `~/` names nothing"))?;
        return Ok(std::path::PathBuf::from(home).join(rest));
    }
    if path.starts_with('/') {
        return Ok(std::path::PathBuf::from(path));
    }
    Err(format!(
        "{path:?} is neither absolute nor under `~/`: a path is relative to nothing here"
    ))
}

/// A Lua value as the plain data a store keeps: strings, finite numbers,
/// booleans, and tables of them, a sequence as a list and string keys as an
/// object. Anything else is left out, and named in the log once per key
/// path (`warned` holds those already named). `where_it_is` is the path down
/// to `value`, from the store's name. `left` bounds the values read, since
/// the walk is Rust, which the handler deadline cannot stop: past it the
/// whole store is an error and is not saved.
/// `crate::files::tests::only_plain_data_is_stored`.
pub(crate) fn plain(
    value: &mlua::Value,
    where_it_is: &str,
    depth: usize,
    left: &mut usize,
    warned: &mut std::collections::HashSet<String>,
) -> Result<Option<Json>, String> {
    /// As deep as JSON text may nest (`json.rs`).
    const DEEPEST: usize = 64;
    *left = left
        .checked_sub(1)
        .ok_or_else(|| "more than 65536 values".to_owned())?;
    fn leave_out(
        warned: &mut std::collections::HashSet<String>,
        where_it_is: &str,
        why: &str,
    ) -> Result<Option<Json>, String> {
        if warned.insert(where_it_is.to_owned()) {
            tracing::warn!(
                key = where_it_is,
                why,
                "a store keeps only strings, numbers, booleans and tables of them; this is left out"
            );
        }
        Ok(None)
    }
    match value {
        mlua::Value::Nil => Ok(None),
        mlua::Value::Boolean(yes) => Ok(Some(Json::Bool(*yes))),
        #[expect(
            clippy::cast_precision_loss,
            reason = "stored as JSON, whose numbers are doubles"
        )]
        mlua::Value::Integer(number) => Ok(Some(Json::Number(*number as f64))),
        mlua::Value::Number(number) if number.is_finite() => Ok(Some(Json::Number(*number))),
        mlua::Value::Number(_) => leave_out(warned, where_it_is, "a number that is not finite"),
        mlua::Value::String(text) => match text.to_str() {
            Ok(text) => Ok(Some(Json::Text(text.to_owned()))),
            Err(_) => leave_out(warned, where_it_is, "a string that is not UTF-8"),
        },
        mlua::Value::Table(table) => {
            if depth >= DEEPEST {
                return leave_out(
                    warned,
                    where_it_is,
                    "nested deeper than 64, or a table that contains itself",
                );
            }
            let pairs: Vec<(mlua::Value, mlua::Value)> = table
                .pairs::<mlua::Value, mlua::Value>()
                .filter_map(Result::ok)
                .collect();
            let length = table.raw_len();
            let is_list = length > 0
                && pairs.len() == length
                && pairs.iter().all(|(key, _)| {
                    matches!(key, mlua::Value::Integer(index)
                        if usize::try_from(*index).is_ok_and(|index| (1..=length).contains(&index)))
                });
            if is_list {
                let mut items = Vec::with_capacity(length);
                for index in 1..=length {
                    let item = table
                        .raw_get::<mlua::Value>(index)
                        .unwrap_or(mlua::Value::Nil);
                    let named = format!("{where_it_is}.{index}");
                    match plain(&item, &named, depth + 1, left, warned)? {
                        Some(item) => items.push(item),
                        // A list keeps its positions: what cannot be kept
                        // in one is `null`.
                        None => items.push(Json::Null),
                    }
                }
                return Ok(Some(Json::List(items)));
            }
            let mut fields = std::collections::BTreeMap::new();
            for (key, item) in pairs {
                let key = match key {
                    mlua::Value::String(key) => match key.to_str() {
                        Ok(key) => key.to_owned(),
                        Err(_) => continue,
                    },
                    other => {
                        let named = format!("{where_it_is}.{}", key_name(&other));
                        if warned.insert(named.clone()) {
                            tracing::warn!(
                                key = named,
                                "a store's tables are lists or have string keys; this key is left out"
                            );
                        }
                        continue;
                    }
                };
                let named = format!("{where_it_is}.{key}");
                if let Some(item) = plain(&item, &named, depth + 1, left, warned)? {
                    fields.insert(key, item);
                }
            }
            Ok(Some(Json::Object(fields)))
        }
        other => leave_out(warned, where_it_is, other.type_name()),
    }
}

fn key_name(key: &mlua::Value) -> String {
    match key {
        mlua::Value::Integer(number) => number.to_string(),
        mlua::Value::Number(number) => number.to_string(),
        other => format!("<{}>", other.type_name()),
    }
}
