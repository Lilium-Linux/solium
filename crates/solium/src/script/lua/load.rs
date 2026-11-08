//! Loading a configuration, and what it said: bindings, settings read
//! before the compositor starts, keeps and stores.

use std::path::Path;

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

    /// Every binding the window manager declared, in its canonical
    /// spelling, sorted.
    pub(crate) fn bindings(&self) -> Vec<Binding> {
        self.declared.bindings.clone()
    }

    /// Whether a combination is bound, in any spelling.
    pub(crate) fn has_binding(&self, combo: &str) -> bool {
        self.binding(combo).is_some()
    }

    /// What the binding for `combo` asked for, or `None` when nothing binds
    /// it.
    pub(crate) fn binding_options(&self, combo: &str) -> Option<BindingOptions> {
        self.binding(combo).map(|binding| BindingOptions {
            repeating: binding.repeating,
            locked: binding.locked,
            inhibited: binding.inhibited,
        })
    }

    /// The declared binding for `combo`. The key filter asks on every key,
    /// so the sorted list is searched rather than walked.
    fn binding(&self, combo: &str) -> Option<&Binding> {
        let combo = normalise_combo(combo);
        let bindings = &self.declared.bindings;
        bindings
            .binary_search_by(|binding| binding.combo.as_str().cmp(&combo))
            .ok()
            .and_then(|at| bindings.get(at))
    }

    /// Bindings the window manager deliberately took away, with what it
    /// said about each. Reported apart from [`Self::bindings`] because a key
    /// missing on purpose looks, in a list of what survived, identical to
    /// one that was never there.
    pub(crate) fn unbound(&self) -> Vec<Binding> {
        self.declared.unbound.clone()
    }

    /// Settings the configuration wrote that its defaults do not define,
    /// sorted by key, because the only reader is a person checking a file.
    pub(crate) fn unknown_settings(&self) -> Vec<UnknownSetting> {
        self.declared.unknown.clone()
    }

    /// The last settings section of this name the configuration handed over
    /// as it loaded, as the document it was.
    fn loaded_section(&self, name: &str) -> Option<Doc> {
        self.loaded
            .commands
            .iter()
            .rev()
            .find_map(|command| match command {
                solium_wm_protocol::Command::Configure((section, nodes)) if section == name => {
                    crate::wm::convert::doc(nodes).ok()
                }
                _ => None,
            })
    }

    /// The effect rules the configuration handed over as it loaded, the last
    /// `sol.effects` winning, read without taking them, so `--check` sees
    /// them and [`Self::startup`] still hands them over.
    /// `check::tests::the_effects_a_rule_names_are_checked`.
    pub(crate) fn effects_at_load(
        &self,
    ) -> Option<Result<Vec<crate::effect::rules::Rule>, Vec<crate::effect::rules::RuleError>>> {
        self.loaded_section("effects")
            .map(|options| crate::script::settings::effects::parse(&options, true).0)
    }

    /// The engine's own keys the configuration handed over as it loaded,
    /// the last `sol.effects` winning, as [`Self::effects_at_load`] reads
    /// its rules: `--check` judges them as the session would.
    /// `check::tests::a_refused_effects_key_fails_the_check`.
    pub(crate) fn effect_settings_at_load(
        &self,
    ) -> Option<Result<crate::effect::settings::Settings, String>> {
        self.loaded_section("effects")
            .map(|options| crate::script::settings::effects::parse(&options, true).1)
    }

    /// What the configuration said through `sol.qml` as it loaded, if
    /// anything: read before any scene starts Qt.
    pub(crate) fn qml(&self) -> crate::qml::renderer::Configured {
        self.loaded_section("qml")
            .map(|options| crate::script::settings::qml::parse(&options))
            .unwrap_or_default()
    }

    /// What the configuration said through `sol.session` as it loaded: the
    /// default, both on, when it said nothing.
    pub(crate) fn session(&self) -> crate::session::Settings {
        self.loaded_section("session")
            .map(|options| crate::script::settings::session::parse(&options))
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
        stores_of(&self.lua)
    }

    /// Load the configuration for `solium --check`: as [`Self::load`] does,
    /// except that nothing of the user's is changed, so a store's broken
    /// file stays where it is and is said in [`Self::check_notes`] (#473).
    pub(crate) fn load_for_check(config: &Path) -> Result<Self> {
        Self::load_as(config, Keep::new(), true)
    }

    /// The settings sections the configuration handed over as it loaded
    /// that the compositor refuses, each with why: `--check` fails on them.
    /// `check::tests::a_refused_settings_section_fails_the_check`.
    pub(crate) fn refused_at_load(&self) -> Vec<String> {
        self.loaded
            .commands
            .iter()
            .filter_map(crate::wm::convert::refusal)
            .collect()
    }

    /// What a check found that a session would act on, such as a store's
    /// broken file, left where it is.
    pub(crate) fn check_notes(&self) -> Vec<String> {
        self.declared.notes.clone()
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
        lua.set_app_data(StoresSent::default());
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

        // The load's answer: what the top level asked for, and the
        // declarations whole, whether or not anything was declared.
        let pending = lua.remove_app_data::<Pending>().unwrap_or_default();
        lua.set_app_data(Pending::default());
        let mut loaded = solium_wm_protocol::Answer {
            serial: 0,
            handled: false,
            commands: Vec::new(),
            motion: None,
            decision: None,
        };
        super::dispatch::answered(&mut loaded, pending);
        lua.remove_app_data::<Undeclared>();
        let declared = declarations(&lua);
        tracing::info!(
            config = %config.display(),
            bindings = declared.bindings.len(),
            "scripts loaded"
        );
        loaded
            .commands
            .push(solium_wm_protocol::Command::Declare(declared));

        let mut scripts = Self {
            lua,
            told: crate::wm::diff::Told::new(),
            mirror: solium_wm_protocol::mirror::Mirror::new(),
            serial: 0,
            loaded: solium_wm_protocol::Answer {
                serial: 0,
                handled: false,
                commands: Vec::new(),
                motion: None,
                decision: None,
            },
            started: false,
            declared: solium_wm_protocol::Declarations {
                bindings: Vec::new(),
                unbound: Vec::new(),
                unknown: Vec::new(),
                notes: Vec::new(),
            },
        };
        scripts.declarations_in(&loaded);
        scripts.loaded = loaded;
        Ok(scripts)
    }
}

/// The stores as they would be saved: each `sol.store` table as the plain
/// data it holds, by name. Nothing in a check, which saves nothing.
pub(crate) fn stores_of(lua: &Lua) -> Vec<(String, crate::json::Json)> {
    let mut out = Vec::new();
    // A check saves nothing.
    if lua.app_data_ref::<CheckOnly>().is_some() {
        return out;
    }
    if let Ok(stores) = lua
        .globals()
        .get::<Table>("sol")
        .and_then(|sol| sol.get::<Table>("_stores"))
    {
        let mut warned = lua.remove_app_data::<StoreWarnings>().unwrap_or_default();
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
        lua.set_app_data(warned);
    }
    if let Some(carried) = lua.app_data_ref::<Carried>() {
        for (name, json) in &carried.0.stores {
            if !out.iter().any(|(each, _)| each == name) {
                out.push((name.clone(), json.clone()));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}
