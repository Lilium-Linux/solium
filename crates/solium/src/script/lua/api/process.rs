//! Programs, timers, files and stores, the lock, reload and quit.

use mlua::{Lua, Table, Value};

use crate::script::lua::*;
use crate::script::*;

/// Add this area's functions to `sol`.
pub(crate) fn register(lua: &Lua, sol: &Table) -> mlua::Result<()> {
    sol.set(
        "reload",
        lua.create_function(|lua, ()| {
            with_pending(lua, |pending| {
                pending.push(solium_wm_protocol::Command::Reload)
            })
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
                pending.push(solium_wm_protocol::Command::LockNow);
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
                pending.push(solium_wm_protocol::Command::LockPeek(progress));
            })
        })?,
    )?;

    // `sol.spawn("foot", "-e", "htop")`. Variadic rather than a table because
    // the common case is one program and no arguments, and that should read
    // like one.
    sol.set(
        "spawn",
        lua.create_function(|lua, args: mlua::Variadic<String>| {
            if args.is_empty() {
                return Err(mlua::Error::runtime("sol.spawn needs a program to run"));
            }
            let argv = args.into_iter().collect();
            with_pending(lua, |pending| {
                pending.push(solium_wm_protocol::Command::Spawn(argv));
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
                pending.push(Command::Run { program, args });
            })
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
                pending.push(Command::FileRead(crate::files::ReadRequest {
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
                    pending.push(Command::FileWrite(crate::files::WriteRequest {
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
                pending.push(Command::Watch {
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
                                pending.push(Command::StoreLoaded { name, text });
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
            with_pending(lua, |pending| {
                pending.push(solium_wm_protocol::Command::Quit)
            })
        })?,
    )?;

    Ok(())
}
