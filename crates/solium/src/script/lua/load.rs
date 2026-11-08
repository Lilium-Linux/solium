//! Loading a configuration, and what it said: bindings, settings read
//! before the compositor starts, keeps and stores.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use mlua::{Lua, Table, Value};

use crate::script::lua::*;
use crate::script::*;

impl Scripts {
    /// Bytes Lua is holding. Its allocator does not hand memory back to the
    /// system, so this rising while everything else is flat says the growth is
    /// script-side rather than a compositor leak.
    pub(crate) fn used_memory(&self) -> usize {
        self.lua.used_memory()
    }

    /// Where the configuration is, in the order it is looked for.
    ///
    /// The user's own file wins, and the bundled one is the fallback rather
    /// than a default that has to be copied before anything works.
    /// Where a user's own scripts live, whether or not they have any.
    pub(crate) fn user_config_dir() -> Option<std::path::PathBuf> {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config"))
            })
            .map(|base| base.join("solium"))
    }

    /// Every binding the configuration registered, in its canonical spelling.
    ///
    /// Sorted, because this is read by a person comparing one run to the next.
    pub(crate) fn bindings(&self) -> Vec<Binding> {
        let Ok(sol) = self.lua.globals().get::<Table>("sol") else {
            return Vec::new();
        };
        let Ok(bindings) = sol.get::<Table>("_bindings") else {
            return Vec::new();
        };
        let sources = sol.get::<Table>("_binding_sources").ok();
        let mut names: Vec<Binding> = bindings
            .pairs::<String, Value>()
            .filter_map(Result::ok)
            .map(|(combo, _)| {
                let asked = self.binding_options(&combo).unwrap_or_default();
                Binding {
                    note: sources
                        .as_ref()
                        .and_then(|sources| sources.get::<Option<String>>(combo.clone()).ok())
                        .flatten(),
                    combo,
                    repeating: asked.repeating,
                    locked: asked.locked,
                    inhibited: asked.inhibited,
                }
            })
            .collect();
        names.sort_by(|left, right| left.combo.cmp(&right.combo));
        names
    }

    /// What the binding for `combo` asked for, or `None` when nothing binds
    /// it. Side-effect free, as [`Self::has_binding`] is, for the same reason:
    /// the key filter asks it.
    pub(crate) fn binding_options(&self, combo: &str) -> Option<BindingOptions> {
        if !self.has_binding(combo) {
            return None;
        }
        let asked = self
            .lua
            .globals()
            .get::<Table>("sol")
            .and_then(|sol| sol.get::<Table>("_binding_options"))
            .and_then(|options| options.get::<Option<Table>>(normalise_combo(combo)))
            .ok()
            .flatten();
        let flag = |name: &str| {
            asked
                .as_ref()
                .is_some_and(|asked| asked.get::<bool>(name).unwrap_or(false))
        };
        Some(BindingOptions {
            repeating: flag("repeating"),
            locked: flag("locked"),
            inhibited: flag("inhibited"),
        })
    }

    /// Bindings a script deliberately took away.
    ///
    /// A combination somebody said something about and which is now bound to
    /// nothing -- which is exactly `sol.unbind`. Reported separately from the
    /// list above because it is the one thing that list cannot show: a key
    /// that is missing on purpose looks, in a list of what survived, identical
    /// to one that was never there.
    pub(crate) fn unbound(&self) -> Vec<Binding> {
        let Ok(sol) = self.lua.globals().get::<Table>("sol") else {
            return Vec::new();
        };
        let (Ok(bindings), Ok(sources)) = (
            sol.get::<Table>("_bindings"),
            sol.get::<Table>("_binding_sources"),
        ) else {
            return Vec::new();
        };
        let mut gone: Vec<Binding> = sources
            .pairs::<String, String>()
            .filter_map(Result::ok)
            .filter(|(combo, _)| {
                !matches!(bindings.get::<Value>(combo.clone()), Ok(Value::Function(_)))
            })
            .map(|(combo, note)| Binding {
                combo,
                note: Some(note),
                // Gone, so it asks for nothing.
                repeating: false,
                locked: false,
                inhibited: false,
            })
            .collect();
        gone.sort_by(|left, right| left.combo.cmp(&right.combo));
        gone
    }

    /// Settings the configuration wrote that its defaults do not define.
    ///
    /// In the order `config.lua` found them, which is `pairs` order and so is
    /// arbitrary -- sorted here, because the only reader is a person checking a
    /// file. See `sol.unknown`.
    pub(crate) fn unknown_settings(&self) -> Vec<UnknownSetting> {
        let Ok(sol) = self.lua.globals().get::<Table>("sol") else {
            return Vec::new();
        };
        let Ok(unknown) = sol.get::<Table>("_unknown_settings") else {
            return Vec::new();
        };
        let mut found: Vec<UnknownSetting> = unknown
            .sequence_values::<Table>()
            .filter_map(Result::ok)
            .filter_map(|entry| {
                Some(UnknownSetting {
                    key: entry.get::<String>("key").ok()?,
                    meant: entry.get::<Option<String>>("meant").ok().flatten(),
                })
            })
            .collect();
        found.sort_by(|left, right| left.key.cmp(&right.key));
        found
    }

    /// The effect rules the configuration handed over as it loaded, the last
    /// `sol.effects` winning, read without taking them, so `--check` sees
    /// them and [`Self::startup`] still hands them over.
    /// `check::tests::the_effects_a_rule_names_are_checked`.
    pub(crate) fn effects_at_load(
        &self,
    ) -> Option<Result<Vec<crate::effect::rules::Rule>, Vec<crate::effect::rules::RuleError>>> {
        self.lua.app_data_ref::<Pending>().and_then(|pending| {
            pending
                .commands
                .iter()
                .rev()
                .find_map(|command| match command {
                    solium_wm_protocol::Command::Configure((section, nodes))
                        if section == "effects" =>
                    {
                        crate::wm::convert::doc(nodes).ok().map(|options| {
                            crate::script::settings::effects::parse(&options, true).0
                        })
                    }
                    _ => None,
                })
        })
    }

    /// The engine's own keys the configuration handed over as it loaded,
    /// the last `sol.effects` winning, as [`Self::effects_at_load`] reads
    /// its rules: `--check` judges them as the session would.
    /// `check::tests::a_refused_effects_key_fails_the_check`.
    pub(crate) fn effect_settings_at_load(
        &self,
    ) -> Option<Result<crate::effect::settings::Settings, String>> {
        self.lua.app_data_ref::<Pending>().and_then(|pending| {
            pending
                .commands
                .iter()
                .rev()
                .find_map(|command| match command {
                    solium_wm_protocol::Command::Configure((section, nodes))
                        if section == "effects" =>
                    {
                        crate::wm::convert::doc(nodes).ok().map(|options| {
                            crate::script::settings::effects::parse(&options, true).1
                        })
                    }
                    _ => None,
                })
        })
    }

    /// What the configuration said through `sol.qml`, if anything.
    pub(crate) fn qml(&self) -> crate::qml::renderer::Configured {
        self.lua
            .app_data_ref::<crate::qml::renderer::Configured>()
            .map(|configured| configured.clone())
            .unwrap_or_default()
    }

    /// What the configuration said through `sol.session`: the default,
    /// both on, when it said nothing.
    pub(crate) fn session(&self) -> crate::session::Settings {
        self.lua
            .app_data_ref::<crate::session::Settings>()
            .map(|settings| *settings)
            .unwrap_or_default()
    }

    pub(crate) fn config_path() -> std::path::PathBuf {
        if let Some(path) = std::env::var_os("SOLIUM_LUA_INIT") {
            return std::path::PathBuf::from(path);
        }

        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config"))
            });

        if let Some(home) = config_home {
            let path = home.join("solium").join("init.lua");
            if path.is_file() {
                return path;
            }
        }

        crate::assets::lua().join("init.lua")
    }

    /// Load the configuration script and everything it pulls in, cold.
    ///
    /// Nothing is carried, because at startup there is nothing to carry. A
    /// reload goes through [`Self::load_carrying`].
    pub(crate) fn load(config: &Path) -> Result<Self> {
        Self::load_carrying(config, Keep::new())
    }

    /// Everything the running scripts asked the host to keep.
    ///
    /// Read off the *old* `Scripts` before the new ones are built, which is
    /// the only moment both exist. Reading rather than moving: a load that
    /// fails leaves the running configuration alone — a typo costs a log line,
    /// not the session — and that promise only holds if collecting the keep
    /// cannot damage the scripts it was collected from.
    pub(crate) fn kept(&self) -> Keep {
        let Ok(sol) = self.lua.globals().get::<Table>("sol") else {
            return Keep::new();
        };
        let Ok(keeps) = sol.get::<Table>("_keeps") else {
            return Keep::new();
        };
        let keeps = keeps
            .pairs::<String, Value>()
            .filter_map(Result::ok)
            .filter_map(|(name, value)| Kept::read(&value, 0, &name).map(|kept| (name, kept)))
            .collect();
        // A store is carried as the plain data it saves, so a reload gives
        // back what a restart would (#473).
        let stores = self.stores().into_iter().collect();
        Keep { keeps, stores }
    }

    /// Every store, as the plain data it saves (#473): the ones this
    /// configuration asked for, and the ones an earlier configuration did
    /// and this one has not, which are kept for as long as the compositor
    /// runs. What cannot be saved is left out, named in the log once per
    /// key path; a store of more than 65536 values is not saved at all.
    /// Rust only, so it runs between dispatches: no Lua is called.
    /// `tests::a_store_survives_a_reload_as_the_data_it_saves`.
    pub(crate) fn stores(&self) -> Vec<(String, crate::json::Json)> {
        let mut out = Vec::new();
        // A check saves nothing.
        if self.lua.app_data_ref::<CheckOnly>().is_some() {
            return out;
        }
        if let Ok(stores) = self
            .lua
            .globals()
            .get::<Table>("sol")
            .and_then(|sol| sol.get::<Table>("_stores"))
        {
            let mut warned = self
                .lua
                .remove_app_data::<StoreWarnings>()
                .unwrap_or_default();
            for (name, value) in stores.pairs::<String, Value>().filter_map(Result::ok) {
                let mut left = 65_536;
                match crate::files::plain(&value, &name, 0, &mut left, &mut warned.0) {
                    Ok(Some(json)) => out.push((name, json)),
                    Ok(None) => {}
                    Err(why) => {
                        if warned.0.insert(name.clone()) {
                            tracing::warn!(store = name, why, "a store is too big to save");
                        }
                    }
                }
            }
            self.lua.set_app_data(warned);
        }
        if let Some(carried) = self.lua.app_data_ref::<Carried>() {
            for (name, json) in &carried.0.stores {
                if !out.iter().any(|(each, _)| each == name) {
                    out.push((name.clone(), json.clone()));
                }
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// How often the stores are saved: `files.store_interval` (#473).
    pub(crate) fn store_interval(&self) -> Duration {
        self.lua
            .try_app_data_ref::<FileSettings>()
            .ok()
            .flatten()
            .map_or(crate::files::STORE_INTERVAL, |settings| {
                settings.store_interval
            })
    }

    /// Load the configuration for `solium --check`: as [`Self::load`] does,
    /// except that nothing of the user's is changed, so a store's broken
    /// file stays where it is and is said in [`Self::check_notes`] (#473).
    pub(crate) fn load_for_check(config: &Path) -> Result<Self> {
        Self::load_as(config, Keep::new(), true)
    }

    /// What a check found that a session would act on: a store's broken
    /// file, left where it is.
    /// The settings sections the configuration handed over as it loaded
    /// that the compositor refuses, each with why: `--check` fails on them.
    /// `check::tests::a_refused_settings_section_fails_the_check`.
    pub(crate) fn refused_at_load(&self) -> Vec<String> {
        self.lua
            .app_data_ref::<Pending>()
            .map(|pending| {
                pending
                    .commands
                    .iter()
                    .filter_map(crate::wm::convert::refusal)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn check_notes(&self) -> Vec<String> {
        self.lua
            .app_data_ref::<CheckOnly>()
            .map(|check| check.0.clone())
            .unwrap_or_default()
    }

    /// Load the configuration again, handing back what the last one kept.
    ///
    /// ## What a script is entitled to know after a reload
    ///
    /// This is the contract, and it is deliberately short, because everything
    /// in it is something the host has to keep true for ever:
    ///
    ///  1. **Whatever it handed to `sol.keep`, and nothing else.** A reload
    ///     throws the whole Lua state away, so a script's own variables are
    ///     gone by construction. `sol.keep` is the one exception, it is opt-in,
    ///     and it holds plain data and the layouts `sol.layout` makes, nothing
    ///     else — see [`Kept`].
    ///  2. **The world as it now is, re-announced.** `restore`, then
    ///     `monitors`, then `layout` — see [`crate::state::Solium::reload`].
    ///     A script that can rebuild itself from `sol.windows()` and
    ///     `sol.monitors()` needs no keep at all.
    ///
    /// Anything else a script believed is gone, and that is the point: the
    /// alternative is every script inventing its own answer, which is what
    /// `workspaces.lua` and `modes.lua` had each done — differently, and one
    /// of them wrongly. See the comment at the top of `lua/modes.lua`.
    pub(crate) fn load_carrying(config: &Path, carried: Keep) -> Result<Self> {
        Self::load_as(config, carried, false)
    }

    /// [`Self::load_carrying`], or for `--check` (`checking`).
    fn load_as(config: &Path, carried: Keep, checking: bool) -> Result<Self> {
        let lua = Lua::new();
        if checking {
            lua.set_app_data(CheckOnly::default());
        }
        lua.set_app_data(Pending::default());
        lua.set_app_data(Snapshot::default());
        lua.set_app_data(Carried(carried));

        let sol = build_api(&lua).map_err(failed("building the script API"))?;
        lua.globals()
            .set("sol", &sol)
            .map_err(failed("installing `sol`"))?;

        // Every 10 000 instructions, a handler past its deadline is unwound
        // with an error. `tests::a_listener_that_never_returns_is_stopped_and_the_others_still_run`.
        // The global hook rather than a thread's own: a coroutine a handler
        // makes finds no callback for a thread's hook, drops it, and would
        // loop unchecked. `tests::a_listener_that_never_returns_inside_a_coroutine_is_stopped`.
        lua.set_app_data(Deadline::default());
        lua.set_global_hook(
            mlua::HookTriggers::new().every_nth_instruction(10_000),
            |lua, debug| {
                if late(lua) {
                    // Where the time ran out, for the log: inside a wrapper,
                    // such as an `actions.override`, that is the wrapped
                    // function, where the struck listener is the wrapper.
                    // `tests::a_stopped_override_is_logged_where_it_was_stopped`.
                    stopped_here(lua, debug);
                    return Err(mlua::Error::runtime(STOPPED));
                }
                Ok(mlua::VmState::Continue)
            },
        )
        .map_err(failed("installing the handler deadline"))?;
        // `pcall`, `xpcall` and `load` hand the stop on rather than catch
        // it: a handler that calls one in a loop would otherwise catch it
        // every time, and run for ever. They catch every other error as they
        // did. Each is wrapped in Lua, and only the look at its results is
        // Rust, so a coroutine can still yield inside one. `xpcall`'s message
        // handler is not called for the stop: Lua calls it from inside the
        // instruction hook that raised the stop, where no hook runs, so a
        // handler that never returned there would never be stopped.
        // `tests::a_listener_that_retries_with_pcall_is_stopped`,
        // `tests::a_listener_that_retries_with_xpcall_is_stopped`,
        // `tests::a_listener_that_retries_load_is_stopped`,
        // `tests::an_xpcall_handler_that_never_returns_is_not_called_for_the_stop`,
        // `tests::an_xpcall_handler_that_never_returns_is_stopped`,
        // `tests::pcall_still_catches_an_ordinary_error_in_a_handler`,
        // `tests::load_still_answers_a_chunk_or_nil_and_why`,
        // `tests::a_coroutine_can_still_yield_inside_pcall`.
        let hand_on = lua
            .create_function(|lua, results: mlua::MultiValue| {
                // `pcall` and `xpcall` answer `false` for an error they
                // caught, and `load` answers `nil`.
                if matches!(results.front(), Some(Value::Boolean(false) | Value::Nil)) && late(lua)
                {
                    return Err(mlua::Error::runtime(STOPPED));
                }
                Ok(results)
            })
            .map_err(failed("wrapping `pcall`, `xpcall` and `load`"))?;
        let in_time = lua
            .create_function(|lua, (): ()| Ok(!late(lua)))
            .map_err(failed("wrapping `pcall`, `xpcall` and `load`"))?;
        for (name, wrapper) in [
            (
                "pcall",
                "return function(...) return hand_on(pcall(...)) end",
            ),
            ("load", "return function(...) return hand_on(load(...)) end"),
            (
                "xpcall",
                "return function(f, handler, ...)\n\
                     if type(handler) == 'function' then\n\
                         local theirs = handler\n\
                         handler = function(err)\n\
                             if in_time() then return theirs(err) end\n\
                             return err\n\
                         end\n\
                     end\n\
                     return hand_on(xpcall(f, handler, ...))\n\
                 end",
            ),
        ] {
            let original: mlua::Function = lua
                .globals()
                .get(name)
                .map_err(failed("reading `pcall`, `xpcall` and `load`"))?;
            let wrapped: mlua::Function = lua
                .load(format!("local {name}, hand_on, in_time = ...\n{wrapper}"))
                .set_name(format!("={name}"))
                .call((original, hand_on.clone(), in_time.clone()))
                .map_err(failed("wrapping `pcall`, `xpcall` and `load`"))?;
            lua.globals()
                .set(name, wrapped)
                .map_err(failed("wrapping `pcall`, `xpcall` and `load`"))?;
        }

        // So a script can `require` its neighbours.
        if let Some(directory) = config.parent().and_then(|path| path.to_str()) {
            let package: Table = lua
                .globals()
                .get("package")
                .map_err(failed("reading `package`"))?;
            let path: String = package.get("path").unwrap_or_default();
            // The user's directory first, then the config's own, then the
            // shipped set, then Lua's. Ordered this way so dropping a single
            // `config.lua` into ~/.config/solium overrides just that file —
            // copying the whole set to change one number is not
            // configurability.
            //
            // The shipped directory is added *unconditionally*, and that is
            // the fix for the thing this whole model rests on. The path used
            // to be built from the chosen config's own directory, which is
            // the shipped one only while you are using the shipped
            // `init.lua`. Write your own and `require("modes")` stopped
            // resolving — so the documented way to start ("replace the entry
            // point, require everything that ships, add your own") could not
            // work at all. Found by running the example out of the guide.
            //
            // *Where* the shipped directory is stopped being a constant here
            // when the compositor became installable: `crate::assets` answers
            // that once, for the Lua and the QML together. What is local to
            // this function is only the order.
            let shipped_lua = crate::assets::lua();
            let shipped = shipped_lua.display();
            let mut search = format!("{directory}/?.lua;{shipped}/?.lua;{path}");
            if let Some(user) = Self::user_config_dir() {
                search = format!("{}/?.lua;{search}", user.display());
            }
            package
                .set("path", search)
                .map_err(failed("extending package.path"))?;
        }

        let source = std::fs::read_to_string(config)
            .with_context(|| format!("reading {}", config.display()))?;
        // Named `@<path>`, as Lua names a file it loads itself: its messages
        // then say `<path>:<line>:` rather than `[string "<path>"]:<line>:`,
        // a failed reload is put on the overlay at its line, and a stopped
        // listener is logged with the whole path.
        // `tests::a_configuration_error_names_its_file_and_line`,
        // `tests::an_error_in_the_configuration_names_it_as_a_file`,
        // `tests::a_stopped_listener_is_logged_with_its_file_and_line`.
        lua.load(&source)
            .set_name(format!("@{}", config.to_string_lossy()))
            .exec()
            .map_err(failed("running the configuration"))?;

        let bindings: Table = sol.get("_bindings").map_err(failed("reading bindings"))?;
        // Counted outside the macro: inside it, `Value` resolves to tracing's
        // own trait of that name rather than to Lua's type.
        let bound = bindings.pairs::<Value, Value>().count();
        tracing::info!(config = %config.display(), bindings = bound, "scripts loaded");

        Ok(Self {
            lua,
            told: crate::wm::diff::Told::new(),
            mirror: solium_wm_protocol::mirror::Mirror::new(),
            serial: 0,
        })
    }
}
