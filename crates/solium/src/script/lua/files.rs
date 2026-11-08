//! `sol.read`, `sol.write`, `sol.watch` and `sol.store`.

use std::time::Duration;

use mlua::{Lua, Table, Value};

use crate::script::lua::*;
use crate::script::*;

/// Ids for reads, writes and watches (#473), counting up across reloads as
/// `RUNS` and `TIMERS` do.
pub(crate) static FILES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// `config.files`, through `sol.files` (#473).
#[derive(Debug, Clone, Copy)]
pub(crate) struct FileSettings {
    /// The most a `sol.read` reads, in bytes.
    pub(crate) max_read: usize,
    /// How often the stores are compared with what was last saved.
    pub(crate) store_interval: Duration,
    /// The largest store file that is read, in bytes.
    pub(crate) store_max: u64,
}

impl Default for FileSettings {
    fn default() -> Self {
        Self {
            max_read: crate::files::MAX_READ,
            store_interval: crate::files::STORE_INTERVAL,
            store_max: crate::files::MAX_STORE,
        }
    }
}

/// Set in a Lua state `solium --check` loads (#473): `sol.store` then
/// reads its file and changes nothing, and collects here what it would
/// have set aside, for the check to say. Present means checking.
#[derive(Debug, Default)]
pub(crate) struct CheckOnly(pub(crate) Vec<String>);

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
                pending.push(Command::WatchCancel(this.0));
            });
            Ok(())
        });
    }
}
