//! The Lua scripting surface.
//!
//! Modes — overview, the app switcher, peek, the icon→window genie — are
//! scripts over the presentation transform, not features with their own
//! renderers. This module is the whole boundary between them and the
//! compositor, and it is deliberately narrow: a script enumerates windows,
//! sets targets, and binds triggers. **Nothing here mentions Wayland.** A mode
//! that had to know a window was a `wl_surface` would be a mode that could not
//! be written by anyone but us.
//!
//! ## How a call works
//!
//! Scripts never hold compositor state. Each dispatch is:
//!
//! 1. Rust builds a [`Snapshot`] — windows, work area, cursor — and hands it to
//!    Lua as app data.
//! 2. The handler runs. Reads come from the snapshot; writes are pushed onto a
//!    queue as [`Command`]s.
//! 3. Rust drains the queue and applies it.
//!
//! Which is the "commands are not state" rule (see CONTRIBUTING.md) made
//! structural: a script cannot mutate the compositor directly, so there is no
//! way for its idea of a window's geometry to drift from the compositor's. It
//! also means no borrow of `Solium` is alive while Lua runs, which is what
//! stops a script calling back into the compositor mid-dispatch and deadlocking
//! on the seat.

use std::{path::Path, time::Duration};

use anyhow::{Context, Result, anyhow};
use mlua::{Lua, Table, Value};

mod command;
pub(crate) mod doc;
mod keep;
mod lua;
mod settings;
mod types;

pub(crate) use command::*;
pub(crate) use doc::{Doc, Table as DocTable};
pub(crate) use keep::*;
pub(crate) use lua::convert::doc_of_plain;
pub(crate) use types::*;

use lua::convert::describe_value as describe;
use lua::*;

/// The Lua runtime and the scripts loaded into it.
#[derive(Debug)]
pub(crate) struct Scripts {
    lua: Lua,
}

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
                    Command::Effects { rules, .. } => Some(rules.clone()),
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
                    Command::Effects { settings, .. } => Some(settings.clone()),
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

        Ok(Self { lua })
    }

    /// Whether any script has bound this key combination.
    ///
    /// Deliberately side-effect free, and separate from dispatching it: the
    /// keyboard filter runs while the seat holds its own lock, so the decision
    /// to intercept is made there and the handler runs afterwards. Calling a
    /// script from inside the filter would let it call back into the seat and
    /// deadlock.
    pub(crate) fn has_binding(&self, combo: &str) -> bool {
        let Ok(sol) = self.lua.globals().get::<Table>("sol") else {
            return false;
        };
        let Ok(bindings) = sol.get::<Table>("_bindings") else {
            return false;
        };
        bindings
            .get::<Value>(normalise_combo(combo))
            .is_ok_and(|value| matches!(value, Value::Function(_)))
    }

    /// Run the handler bound to a key combination.
    pub(crate) fn key(&mut self, combo: &str, snapshot: Snapshot) -> Outcome {
        self.dispatch_as(FocusSource::Binding, snapshot, |lua, sol| {
            let bindings: Table = sol.get("_bindings")?;
            let handler: Value = bindings.get(normalise_combo(combo))?;
            match handler {
                Value::Function(function) => match function.call::<()>(()) {
                    Ok(()) => Ok(true),
                    // Logged where it was written and where it was stopped,
                    // as a stopped listener is, and unhandled, as a binding
                    // that fails is.
                    // `tests::a_stopped_binding_is_logged_where_it_was_written_and_stopped`.
                    Err(err) if err.to_string().contains(STOPPED) => {
                        let info = function.info();
                        let file = file_of(info.source.as_deref(), info.short_src.as_deref());
                        let line = info.line_defined.unwrap_or_default();
                        let stopped_at = stopped_at(lua);
                        tracing::error!(
                            %err,
                            event = "binding",
                            combo,
                            file = %file,
                            line,
                            stopped_at = stopped_at.as_deref().map(tracing::field::display),
                            "a handler ran for longer than 100 ms and was stopped"
                        );
                        Ok(false)
                    }
                    Err(err) => Err(err),
                },
                _ => Ok(false),
            }
        })
    }

    /// Tell scripts a window has appeared.
    ///
    /// Every listener runs. Returns whether any of them did: nothing happening
    /// is a valid answer, and the compositor then uses its own plain animation
    /// rather than leaving a window to pop into existence.
    pub(crate) fn opened(&mut self, id: u64, snapshot: Snapshot) -> Outcome {
        self.dispatch_as(FocusSource::Open, snapshot, move |lua, _| {
            call_listeners(lua, "open", id)
        })
    }

    /// A window went fullscreen, or left it: `(id, entering)`.
    ///
    /// Told after the change is made: the client has been sent its new size,
    /// and the window lives at its new rectangle. What is left is moving the
    /// picture there, which the compositor does once every listener has run,
    /// from where the window is drawn, with the timing the listeners set with
    /// `sol.animate` -- [`Outcome::motion`] -- and at once when none set one.
    /// `tests::a_change_is_answered_with_the_motion_sol_animate_set`.
    pub(crate) fn fullscreen(&mut self, id: u64, entering: bool, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| {
            call_listeners(lua, "fullscreen", (id, entering))
        })
    }

    /// A window was maximised, or restored: `(id, entering)`. Answered the
    /// way [`Self::fullscreen`] is, and told on its own, so the two can be
    /// given different motions.
    /// `tests::a_change_is_answered_with_the_motion_sol_animate_set`.
    pub(crate) fn maximize(&mut self, id: u64, entering: bool, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| {
            call_listeners(lua, "maximize", (id, entering))
        })
    }

    /// Run the handler for a pointer press, while a mode owns input.
    pub(crate) fn click(&mut self, x: f64, y: f64, snapshot: Snapshot) -> Outcome {
        self.dispatch_as(FocusSource::Click, snapshot, move |lua, _| {
            call_listeners(lua, "click", (x, y))
        })
    }

    /// Down runs before delivery so Lua can capture; always outside seat locks.
    pub(crate) fn touch(&mut self, event: TouchEvent, snapshot: Snapshot) -> Outcome {
        self.dispatch_as(FocusSource::Touch, snapshot, move |lua, _| {
            let table = lua.create_table()?;
            table.set("kind", event.kind)?;
            table.set("slot", event.slot)?;
            table.set("x", event.x)?;
            table.set("y", event.y)?;
            table.set("output", event.output)?;
            table.set("target", event.target)?;
            table.set("window", event.window)?;
            table.set("control", event.control)?;
            table.set("edges", event.edges)?;
            table.set("time", event.time)?;
            table.set("t", event.t)?;
            table.set("vx", event.velocity.0)?;
            table.set("vy", event.velocity.1)?;
            call_listeners(lua, "touch", table)
        })
    }

    /// Whatever the configuration asked for while it was being read.
    ///
    /// A script's top level runs once, at load, and anything it requests there
    /// — the dock's contents, a starting mode — lands in the same pending
    /// buffer an event handler would use. Without draining it that work is
    /// simply dropped, silently, which is what happened to the first dock: no
    /// error, no items, nothing drawn.
    pub(crate) fn startup(&mut self) -> Outcome {
        // Takes the buffer the configuration filled, rather than dispatching:
        // `dispatch` installs a fresh `Pending` before it calls anything, so
        // routing this through it would throw away the very requests it is
        // here to collect.
        let pending = self.lua.remove_app_data::<Pending>().unwrap_or_default();
        self.lua.set_app_data(Pending::default());
        Outcome {
            handled: false,
            commands: pending.commands,
            grab: pending.grab,
            status: pending.status,
            motion: pending.animated.then_some(pending.animation),
        }
    }

    /// Focus moved to a window: `(id, source, changed)`, `source` the word
    /// for what moved it, and `changed` whether the keyboard came from
    /// another window or none, `false` for the window that had it focused
    /// again (#220). `tests::the_focus_event_says_what_moved_the_keyboard`,
    /// `state::tests::real_client::follow::the_focus_event_says_whether_the_keyboard_moved`.
    pub(crate) fn focused(
        &mut self,
        id: u64,
        source: FocusSource,
        changed: bool,
        snapshot: Snapshot,
    ) -> Outcome {
        self.dispatch(snapshot, move |lua, _| {
            call_listeners(lua, "focus", (id, source.name(), changed))
        })
    }

    /// The keyboard's layout, Caps Lock or Num Lock changed: `(state,
    /// changed)`, `state` being what `sol.keyboard()` answers and `changed`
    /// `"layout"`, `"caps"` or `"num"`.
    /// `keyboard_change::tests::a_layout_switch_and_a_caps_toggle_by_key_are_told_once_each_with_russian_active`.
    pub(crate) fn keyboard_changed(
        &mut self,
        changed: &'static str,
        snapshot: Snapshot,
    ) -> Outcome {
        self.dispatch(snapshot, move |lua, sol| {
            let state: Value = sol.get::<mlua::Function>("keyboard")?.call(())?;
            call_listeners(lua, "keyboard", (state, changed))
        })
    }

    /// The session's lock changed: `(state)`, `"locking"` while Solium's own
    /// lock comes down over the desktop, `"locked"`, `"unlocking"` while it
    /// goes, and `"unlocked"`. Nothing else of the lock: not who holds it,
    /// nothing of what is typed at it.
    /// `lock_auth::tests::the_scripts_hear_the_lock_come_and_go`.
    pub(crate) fn lock_changed(&mut self, state: &'static str, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| call_listeners(lua, "lock", state))
    }

    /// The focused text field changed: `(field, why)`, `field` being what
    /// `sol.text_input()` answers and `why` `"field"`, `"caret"` or
    /// `"framed"`, as `text_input::Why` names it.
    /// `text_input::tests::text_input_is_told_when_a_field_is_enabled_and_when_it_is_focused`,
    /// `text_input::tests::text_input_is_told_when_the_caret_moves_once_a_pass`.
    pub(crate) fn text_input(&mut self, why: &'static str, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, sol| {
            let field: Value = sol.get::<mlua::Function>("text_input")?.call(())?;
            call_listeners(lua, "text_input", (field, why))
        })
    }

    /// A window asked to be brought forward, and the compositor has answered
    /// as it does: focused it, or refused it on a workspace nobody is looking
    /// at or while it is being closed. `(id, why)`.
    ///
    /// `why` is `"launch"` for an application that was already running and
    /// answered a launch with a window it had, the launch's own window having
    /// dissolved (#177), and `"request"` for anything else: a token nothing
    /// was launched with, which any client can make for itself. Never for a
    /// launch's window arriving, which is `open` and `focus`.
    /// `relaunch::relaunching_an_application_tells_the_scripts_no_close_for_its_window`,
    /// `relaunch::a_window_opened_for_a_relaunch_still_arrives_in_the_window_opened_for_the_launch`
    /// and `relaunch::a_genuine_activation_is_told_as_a_request_and_the_view_stays_where_it_is`,
    /// in `state/tests.rs`.
    pub(crate) fn activated(&mut self, id: u64, why: &'static str, snapshot: Snapshot) -> Outcome {
        self.dispatch_as(FocusSource::Activate, snapshot, move |lua, _| {
            call_listeners(lua, "activate", (id, why))
        })
    }

    /// An edge was dragged.
    ///
    /// Offered to layouts before the compositor resizes anything, so a tiled
    /// window can move its seam instead of growing over its neighbour.
    ///
    /// `(id, edge_x, edge_y, horizontal_side, vertical_side)`.
    ///
    /// This signature line has been wrong twice, so it is written out rather
    /// than summarised. It said `(id, x, y, horizontal, vertical)` until now —
    /// the names of the *pre-#120* booleans, which that issue replaced with
    /// sides in the code and forgot to replace here — and the prose under it
    /// called the first pair "the delta" for the whole life of the event and
    /// then "where the pointer is", neither of which it is any more.
    ///
    /// `edge_x, edge_y` is **where the dragged edge should come to rest** on
    /// each axis, in the coordinates `sol.place` and `tree:layout` already
    /// speak. A position and not a delta, so handling the same drag twice gives
    /// the same layout; derived from the drag's own rectangle rather than from
    /// the seat, so the edge moves *with* the pointer instead of jumping to it.
    /// That is #124, and the difference is most of a window's width on a
    /// `super`+right-button drag, which begins in the middle of one. See
    /// [`crate::input::resize::dragged_edge`].
    ///
    /// On an axis this drag does not move there is no such edge, and the
    /// pointer's own coordinate is passed through there instead. A handler that
    /// checks its side before using the coordinate — which is what the side is
    /// for — never sees it.
    ///
    /// `horizontal_side, vertical_side` are the *side* of the window being
    /// dragged on each axis: `"left"`/`"right"`, `"top"`/`"bottom"`, or nil
    /// where that axis is not in play. They were a pair of booleans until #120;
    /// a script that only tested them for truthiness still reads the same,
    /// because a side is truthy and nil is not, but one that passes them on now
    /// passes on something a layout can choose a seam from. See
    /// [`crate::input::resize::sides`].
    pub(crate) fn resized(
        &mut self,
        id: u64,
        edge_at: (f64, f64),
        sides: (Option<&'static str>, Option<&'static str>),
        snapshot: Snapshot,
    ) -> Outcome {
        self.dispatch(snapshot, move |lua, _| {
            call_listeners(lua, "resize", (id, edge_at.0, edge_at.1, sides.0, sides.1))
        })
    }

    /// A window went away.
    ///
    /// Needed by any layout that keeps state of its own: a tree cannot drop a
    /// node it is never told about, and diffing the window list on every pass
    /// — which is what the layouts did instead — cannot tell "closed" from
    /// "moved to another workspace".
    pub(crate) fn closed(&mut self, id: u64, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| call_listeners(lua, "close", id))
    }

    /// The compositor has started closing a window: its leaving animation is
    /// playing, and the client has not been asked yet.
    ///
    /// Not `close`, and not a replacement for it. The client may still refuse
    /// -- an unsaved-changes prompt -- and a script that treated this as the
    /// window being gone would forget a window that is about to come back.
    /// `close` still fires when it really goes; `refused` fires instead when it
    /// does not. This is the moment a layout may reflow if it wants to, which
    /// is why it exists: the compositor answers a close at once, the way it
    /// answers an open or a resize, and the client catches up inside that
    /// answer (#128).
    pub(crate) fn closing(&mut self, id: u64, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| call_listeners(lua, "closing", id))
    }

    /// A close has landed: its animation has played, the window has faded
    /// out of sight, and its client has just been asked to go (an X11 one
    /// that let the last request go unanswered, ended). Between `closing` and
    /// `close` or `refused`, once per close the compositor asked for; never
    /// for a window that went, or answered with a dialog, before its close
    /// landed, nor for one whose client never came.
    ///
    /// The moment a layout set to wait for the animation reflows
    /// (`reflow_on_close = "when_landed"`, #128): later than `closing`, so the
    /// other windows stay put while the window plays out, and earlier than
    /// `close`, so they do not wait on the application.
    /// `with_reflow_on_close_when_landed_the_grid_waits_for_the_close_to_land`
    /// in `state/tests.rs`.
    pub(crate) fn close_landed(&mut self, id: u64, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| {
            call_listeners(lua, "close_landed", id)
        })
    }

    /// A window that was closing is staying: its client declined, and the
    /// window is back on screen.
    ///
    /// Once per refusal, on the frame the window is given back, and never
    /// after `close` for the same window. A layout that took the window out
    /// on `closing` puts it back here. Not `open`: the window never went, and
    /// an arrival would run `open.lua`'s animation on it a second time.
    pub(crate) fn refused(&mut self, id: u64, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| call_listeners(lua, "refused", id))
    }

    /// A window has just been minimized (#30): by `sol.minimize`, or by its
    /// own client asking. Fired once, before anything else reacts, with the
    /// window already `minimized` in the snapshot, so a configuration starts
    /// its present -- the shipped one plays the genie into its slot -- in the
    /// same dispatch, and a layout takes the window out of its arrangement.
    /// The window stays drawn while that present plays out.
    /// A notification arrived (#346): the `notification` hooks decide what
    /// happens to it, the first that returns a table deciding (`banner`,
    /// `keep`), each under the handler deadline. `None` when none answered,
    /// so the shipped rule applies.
    /// `tests::the_first_notification_hook_that_answers_decides`.
    pub(crate) fn notification_decision(
        &mut self,
        arrival: &crate::json::Json,
        snapshot: Snapshot,
    ) -> (Option<crate::json::Json>, Outcome) {
        let data = arrival.to_lua(&self.lua).unwrap_or(Value::Nil);
        let decided = std::rc::Rc::new(std::cell::RefCell::new(None));
        let slot = decided.clone();
        let outcome = self.dispatch(snapshot, move |lua, _| {
            *slot.borrow_mut() = call_deciders(lua, "notification", data.clone())?;
            Ok(true)
        });
        let decided = decided.borrow_mut().take();
        (decided, outcome)
    }

    /// Something happened to a notification (#346): `added`, `replaced`,
    /// `closed`, `invoked`, `dnd` or `owner`, for `sol.on("notifications")`.
    /// `tests::notification_events_reach_their_listeners_with_their_origin`.
    pub(crate) fn notifications_event(
        &mut self,
        event: &crate::json::Json,
        snapshot: Snapshot,
    ) -> Outcome {
        let data = event.to_lua(&self.lua).unwrap_or(Value::Nil);
        self.dispatch(snapshot, move |lua, _| {
            call_listeners(lua, "notifications", data.clone())
        })
    }

    /// Something changed in audio (#347): `volume`, `mute`, `default`,
    /// `added` or `removed`, with who did it, for `sol.on("audio")`.
    /// `tests::audio_events_reach_their_listeners_with_their_origin`.
    pub(crate) fn audio_event(&mut self, event: &crate::json::Json, snapshot: Snapshot) -> Outcome {
        let data = event.to_lua(&self.lua).unwrap_or(Value::Nil);
        self.dispatch(snapshot, move |lua, _| {
            call_listeners(lua, "audio", data.clone())
        })
    }

    /// Something changed in the tray (#363): an item `added`, `changed` or
    /// `removed`, its `menu` opened, read or closed, or the `owner` of the
    /// watcher's name, for `sol.on("tray")`.
    /// `tests::tray_events_reach_their_listeners_with_their_origin`.
    pub(crate) fn tray_event(&mut self, event: &crate::json::Json, snapshot: Snapshot) -> Outcome {
        let data = event.to_lua(&self.lua).unwrap_or(Value::Nil);
        self.dispatch(snapshot, move |lua, _| {
            call_listeners(lua, "tray", data.clone())
        })
    }

    /// Something changed in media (#379): a player `added`, `changed` or
    /// `removed`, or another player `active`, for `sol.on("media")`.
    /// `tests::media_events_reach_their_listeners_with_the_player`.
    pub(crate) fn media_event(&mut self, event: &crate::json::Json, snapshot: Snapshot) -> Outcome {
        let data = event.to_lua(&self.lua).unwrap_or(Value::Nil);
        self.dispatch(snapshot, move |lua, _| {
            call_listeners(lua, "media", data.clone())
        })
    }

    /// `sol.notify`'s notifications that closed without an action: their
    /// functions are let go, so a shell that posts many does not keep them
    /// all. `tests::sol_notify_queues_one_of_the_shells_own_with_its_actions`.
    pub(crate) fn notify_forget(&mut self, tokens: &[u64]) {
        let forgotten = self
            .lua
            .globals()
            .get::<Table>("sol")
            .and_then(|sol| sol.get::<Table>("_notify_actions"))
            .and_then(|kept| {
                for token in tokens {
                    kept.set(*token, Value::Nil)?;
                }
                Ok(())
            });
        if let Err(err) = forgotten {
            tracing::debug!(%err, "sol.notify's functions were not let go");
        }
    }

    /// An action of one of `sol.notify`'s own notifications was invoked: its
    /// function runs, and the notification's functions are let go.
    /// `tests::sol_notify_queues_one_of_the_shells_own_with_its_actions`.
    pub(crate) fn notify_action(&mut self, token: u64, key: &str, snapshot: Snapshot) -> Outcome {
        let key = key.to_owned();
        self.dispatch(snapshot, move |lua, sol| {
            let kept: Table = sol.get("_notify_actions")?;
            let Some(functions) = kept.get::<Option<Table>>(token)? else {
                return Ok(false);
            };
            kept.set(token, Value::Nil)?;
            let Some(run) = functions.get::<Option<mlua::Function>>(key.as_str())? else {
                return Ok(false);
            };
            start_clock(lua);
            match run.call::<()>(()) {
                Ok(()) => Ok(true),
                Err(err) => {
                    tracing::error!(%err, key, "a sol.notify action failed");
                    Ok(false)
                }
            }
        })
    }

    pub(crate) fn minimized(&mut self, id: u64, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| call_listeners(lua, "minimize", id))
    }

    /// A minimized window is shown again (#30): by `sol.unminimize`, by its
    /// client, or by being focused. A layout takes it back where it was, and
    /// a configuration presents it back from wherever it put it.
    pub(crate) fn unminimized(&mut self, id: u64, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| {
            call_listeners(lua, "unminimize", id)
        })
    }

    /// Something the compositor owns changed the space windows get.
    ///
    /// A decoration reserving a different amount is the case that needs it:
    /// the slots are the same but what fits in them is not, and a layout that
    /// is never told goes on believing its own last arithmetic.
    /// The set of monitors changed: one arrived, or one went away.
    ///
    /// Separate from `layout` because it is a different question. `layout`
    /// asks a mode to arrange the windows it already knows about; this says
    /// the screens themselves are not the screens they were, so a mode holding
    /// per-monitor state -- which both shipped layouts do, one tree or one
    /// view per screen -- has to re-home the windows whose monitor is gone
    /// before arranging anything.
    ///
    /// Without it a window on an unplugged monitor is in a tree that nothing
    /// iterates, because `tiling.apply` walks the monitors that *exist*. It
    /// keeps its old slot, which is now off every screen, and it reappears
    /// there when the monitor comes back -- which is exactly what a hardware
    /// test found.
    pub(crate) fn monitors_changed(&mut self, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| call_listeners(lua, "monitors", ()))
    }

    /// The problems changed: an effect broke or was mended, or a reload failed.
    /// `tests::the_problems_event_reaches_its_listeners`.
    pub(crate) fn problems_changed(&mut self, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| call_listeners(lua, "problems", ()))
    }

    /// These scripts have replaced a running session's, rather than started one.
    ///
    /// The first half of the reload contract on [`Self::load_carrying`], and
    /// the half a keep cannot cover. `sol.keep` gives a script its *data* back;
    /// this is the moment it may act on it — after every script has loaded, so
    /// a mode restored here can be sure the layout it names has registered
    /// itself, which it cannot be at its own top level.
    ///
    /// It does not fire at startup, and that asymmetry is the whole meaning of
    /// the event: a cold start has nothing to restore, and a script that
    /// listens for this is saying "this is what I do differently when I am not
    /// the first configuration this session has had".
    pub(crate) fn restored(&mut self, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| call_listeners(lua, "restore", ()))
    }

    /// A hosted scene sent an action, with data: the `surface` listeners
    /// hear the surface's name, the action and the data, a table, a value or
    /// `nil` (Ruling 15).
    /// `tests::a_surface_action_reaches_lua_with_its_data`,
    /// `tests::an_action_with_no_data_reaches_lua_as_nil`.
    /// A window frame's layer sent something that is not a built-in action
    /// (#417): `sol.on("frame", function(id, name, data) ... end)`, with the
    /// window's id, the name it sent and its data, as a scene's `surface`
    /// listeners hear theirs. `tests::frame_listeners_hear_the_window_the_name_and_the_data`.
    pub(crate) fn frame_action(
        &mut self,
        id: u64,
        name: &str,
        data: &crate::json::Json,
        snapshot: Snapshot,
    ) -> Outcome {
        let data = data.to_lua(&self.lua).unwrap_or_else(|err| {
            tracing::warn!(%err, name, "a frame's data did not reach Lua");
            Value::Nil
        });
        let name = name.to_owned();
        self.dispatch_as(FocusSource::Surface, snapshot, move |lua, _| {
            call_listeners(lua, "frame", (id, name.clone(), data.clone()))
        })
    }

    pub(crate) fn surface_action(
        &mut self,
        name: &str,
        action: &str,
        data: &crate::json::Json,
        snapshot: Snapshot,
    ) -> Outcome {
        let data = data.to_lua(&self.lua).unwrap_or_else(|err| {
            tracing::warn!(%err, action, "an action's data did not reach Lua");
            Value::Nil
        });
        let (name, action) = (name.to_owned(), action.to_owned());
        self.dispatch_as(FocusSource::Surface, snapshot, move |lua, _| {
            call_listeners(lua, "surface", (name.clone(), action.clone(), data.clone()))
        })
    }

    /// Attempts whose outcomes are known: each one's `done` is called once,
    /// and forgotten, in a dispatch of its own, each under the whole handler
    /// deadline.
    /// `tests::sol_act_returns_an_attempt_and_done_hears_the_outcome_once`,
    /// `tests::a_done_that_never_returns_is_stopped_and_the_next_done_still_hears_its_outcome`.
    ///
    /// A `done` stopped at the deadline is struck as a listener is, by
    /// function, and one stopped three times is not called again until the
    /// next reload. What it asked for in the run that was stopped is
    /// dropped, the attempts it started forgotten with it, so a retry that
    /// acts again and then never returns is not told again, round after
    /// round.
    /// `tests::a_done_stopped_three_times_is_not_called_again`,
    /// `tests::what_a_stopped_done_asked_for_is_dropped`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_done_that_acts_again_and_never_returns_does_not_stall_every_dispatch`.
    pub(crate) fn attempts_settled(&mut self, settled: &[Settled], snapshot: Snapshot) -> Outcome {
        let settled = settled.to_vec();
        self.dispatch(snapshot, move |lua, sol| {
            let attempts: Table = sol.get("_attempts")?;
            let strikes: Table = sol.get("_strikes")?;
            for each in &settled {
                let done: Option<mlua::Function> = attempts.get(each.attempt)?;
                attempts.set(each.attempt, Value::Nil)?;
                let Some(done) = done else { continue };
                if strikes.get::<Option<u32>>(&done)?.unwrap_or(0) >= STRIKES {
                    continue;
                }
                let asked = lua
                    .try_app_data_ref::<Pending>()
                    .ok()
                    .flatten()
                    .map_or(0, |pending| pending.commands.len());
                start_clock(lua);
                match done.call::<()>((each.ok, each.reason)) {
                    Ok(()) => {}
                    Err(err) if err.to_string().contains(STOPPED) => {
                        let dropped = lua
                            .try_app_data_mut::<Pending>()
                            .ok()
                            .flatten()
                            .map(|mut pending| {
                                let at = asked.min(pending.commands.len());
                                pending.commands.split_off(at)
                            })
                            .unwrap_or_default();
                        for command in dropped {
                            if let Command::Act { attempt, .. } = command {
                                attempts.set(attempt, Value::Nil)?;
                            }
                        }
                        let _ = strike(lua, &strikes, &done, "done", &err)?;
                    }
                    Err(err) => tracing::error!(%err, "an attempt's done failed"),
                }
            }
            Ok(true)
        })
    }

    /// What `sol.run{}`'s children said and how they ended (#472), in the
    /// order it was heard, in one dispatch: each line to its run's `lines`
    /// as `(line, stream)`, and each end to its `done`, once, with the
    /// result, after which the run's functions are let go. Each call has the
    /// whole handler deadline, and a function stopped there three times is
    /// not called again until the next reload.
    /// `tests::sol_run_with_a_table_hands_its_output_to_done`,
    /// `tests::sol_run_tells_lines_then_done`.
    pub(crate) fn runs_heard(
        &mut self,
        heard: &[crate::runs::Heard],
        snapshot: Snapshot,
    ) -> Outcome {
        use crate::runs::Heard;
        self.dispatch(snapshot, move |lua, sol| {
            let runs: Table = sol.get("_runs")?;
            let strikes: Table = sol.get("_strikes")?;
            for each in heard {
                match each {
                    Heard::Line { id, stream, line } => {
                        let Some(kept) = runs.get::<Option<Table>>(*id)? else {
                            continue;
                        };
                        let Some(lines) = kept.get::<Option<mlua::Function>>("lines")? else {
                            continue;
                        };
                        let line = lua.create_string(line)?;
                        call_struck(lua, &strikes, &lines, (line, stream.name()), "lines")?;
                    }
                    Heard::Done { id, ended } => {
                        let Some(kept) = runs.get::<Option<Table>>(*id)? else {
                            continue;
                        };
                        runs.set(*id, Value::Nil)?;
                        let Some(done) = kept.get::<Option<mlua::Function>>("done")? else {
                            continue;
                        };
                        let result = lua.create_table()?;
                        result.set("code", ended.code)?;
                        result.set("signal", ended.signal)?;
                        result.set("timed_out", ended.timed_out)?;
                        result.set("truncated", ended.truncated)?;
                        result.set("stdout", lua.create_string(&ended.stdout)?)?;
                        result.set("stderr", lua.create_string(&ended.stderr)?)?;
                        result.set("error", ended.error.as_deref())?;
                        call_struck(lua, &strikes, &done, result, "done")?;
                    }
                }
            }
            Ok(true)
        })
    }

    /// Timers due (#472), in the order they fell due, in one dispatch: each
    /// one's function, under the whole handler deadline. A `sol.after`'s is
    /// let go before it runs. A timer with no function left, and one whose
    /// function was stopped three times, is cancelled.
    /// `tests::timers_run_in_the_order_they_fall_due`,
    /// `tests::a_timer_stopped_three_times_is_cancelled`.
    pub(crate) fn timers_due(&mut self, due: &[(u64, bool)], snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, sol| {
            let timers: Table = sol.get("_timers")?;
            let strikes: Table = sol.get("_strikes")?;
            let mut gone = Vec::new();
            for &(id, last) in due {
                let Some(run) = timers.get::<Option<mlua::Function>>(id)? else {
                    gone.push(id);
                    continue;
                };
                if last {
                    timers.set(id, Value::Nil)?;
                }
                if call_struck(lua, &strikes, &run, (), "timer")? && !last {
                    timers.set(id, Value::Nil)?;
                    gone.push(id);
                }
            }
            if !gone.is_empty() {
                with_pending(lua, |pending| {
                    pending
                        .commands
                        .extend(gone.into_iter().map(Command::TimerCancel));
                })?;
            }
            Ok(true)
        })
    }

    /// What the configuration's reads, writes and watches came to (#473),
    /// in the order heard, in one dispatch: each read's function with
    /// `(data, nil)` or `(nil, err)`, each write's with `err` or `nil`, after
    /// which they are let go, and each watch's with its event. Each call has
    /// the whole handler deadline; a watch whose function was stopped three
    /// times, and one with no function left, is cancelled.
    /// `tests::sol_read_hands_the_file_to_its_function`,
    /// `tests::a_cancelled_watch_is_not_told`.
    pub(crate) fn files_heard(
        &mut self,
        heard: &[crate::files::Heard],
        snapshot: Snapshot,
    ) -> Outcome {
        use crate::files::Heard;
        self.dispatch(snapshot, move |lua, sol| {
            let files: Table = sol.get("_files")?;
            let watches: Table = sol.get("_watches")?;
            let strikes: Table = sol.get("_strikes")?;
            let mut gone = Vec::new();
            for each in heard {
                match each {
                    Heard::Read { id, result } => {
                        let Some(done) = files.get::<Option<mlua::Function>>(*id)? else {
                            continue;
                        };
                        files.set(*id, Value::Nil)?;
                        match result {
                            Ok(data) => {
                                let data = lua.create_string(data)?;
                                call_struck(lua, &strikes, &done, (data, Value::Nil), "read")?;
                            }
                            Err(err) => {
                                call_struck(
                                    lua,
                                    &strikes,
                                    &done,
                                    (Value::Nil, err.as_str()),
                                    "read",
                                )?;
                            }
                        }
                    }
                    Heard::Wrote { id, error } => {
                        let Some(done) = files.get::<Option<mlua::Function>>(*id)? else {
                            continue;
                        };
                        files.set(*id, Value::Nil)?;
                        call_struck(lua, &strikes, &done, error.as_deref(), "write")?;
                    }
                    Heard::WatchEnded { id } => {
                        watches.set(*id, Value::Nil)?;
                    }
                    Heard::Watched { id, kind, path } => {
                        let Some(run) = watches.get::<Option<mlua::Function>>(*id)? else {
                            gone.push(*id);
                            continue;
                        };
                        let event = lua.create_table()?;
                        event.set("kind", kind.name())?;
                        event.set("path", path.as_str())?;
                        if call_struck(lua, &strikes, &run, event, "watch")? {
                            watches.set(*id, Value::Nil)?;
                            gone.push(*id);
                        }
                    }
                }
            }
            if !gone.is_empty() {
                gone.sort_unstable();
                gone.dedup();
                with_pending(lua, |pending| {
                    pending
                        .commands
                        .extend(gone.into_iter().map(Command::WatchCancel));
                })?;
            }
            Ok(true)
        })
    }

    pub(crate) fn relayout(&mut self, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| call_listeners(lua, "layout", ()))
    }

    /// A move drag of a window began, moved or ended (#452): `(id, phase,
    /// x, y)`, told by `crate::input::grab::settle_drags`.
    pub(crate) fn dragged(
        &mut self,
        id: u64,
        phase: &'static str,
        x: f64,
        y: f64,
        snapshot: Snapshot,
    ) -> Outcome {
        self.dispatch_as(FocusSource::Drop, snapshot, move |lua, _| {
            call_listeners(lua, "drag", (id, phase, x, y))
        })
    }

    /// An edge or modifier resize of a window began or ended (#454): `(id,
    /// phase, x, y)`, told by `crate::input::grab::settle_drags`.
    pub(crate) fn resizing(
        &mut self,
        id: u64,
        phase: &'static str,
        x: f64,
        y: f64,
        snapshot: Snapshot,
    ) -> Outcome {
        self.dispatch(snapshot, move |lua, _| {
            call_listeners(lua, "resizing", (id, phase, x, y))
        })
    }

    /// A window was dragged and let go.
    ///
    /// The event a layout needs and could not have: without it a drag in a
    /// tiled layout leaves the window wherever the cursor stopped, because
    /// nothing ever tells the layout to think again.
    pub(crate) fn dropped(&mut self, id: u64, x: f64, y: f64, snapshot: Snapshot) -> Outcome {
        self.dispatch_as(FocusSource::Drop, snapshot, move |lua, _| {
            call_listeners(lua, "drop", (id, x, y))
        })
    }

    /// The pointer wheel turned, with the compositor's modifier held.
    ///
    /// Only offered to scripts when Super is down, so an unmodified wheel keeps
    /// belonging to whatever is under the cursor. A scrolling layout that ate
    /// every wheel event would make every terminal in it unusable.
    pub(crate) fn scrolled(&mut self, dx: f64, dy: f64, snapshot: Snapshot) -> Outcome {
        self.dispatch_as(FocusSource::Scroll, snapshot, move |lua, _| {
            call_listeners(lua, "scroll", (dx, dy))
        })
    }

    /// The shared shape of every dispatch: snapshot in, commands out.
    fn dispatch(
        &mut self,
        snapshot: Snapshot,
        call: impl FnOnce(&Lua, &Table) -> mlua::Result<bool>,
    ) -> Outcome {
        self.dispatch_as(FocusSource::Script, snapshot, call)
    }

    /// [`Self::dispatch`], for a handler answering `source`: what a
    /// `sol.focus` it makes is told with (#220).
    /// `tests::the_focus_event_says_what_moved_the_keyboard`.
    fn dispatch_as(
        &mut self,
        source: FocusSource,
        snapshot: Snapshot,
        call: impl FnOnce(&Lua, &Table) -> mlua::Result<bool>,
    ) -> Outcome {
        let at_the_lock = source == FocusSource::Binding && snapshot.locked;
        self.lua.set_app_data(snapshot);
        self.lua.set_app_data(Pending {
            source,
            at_the_lock,
            ..Pending::default()
        });
        start_clock(&self.lua);

        let handled = match self.lua.globals().get::<Table>("sol") {
            Ok(sol) => match call(&self.lua, &sol) {
                Ok(handled) => handled,
                Err(err) => {
                    // A script erroring must not take the compositor with it,
                    // and must not leave the trigger swallowed either: an
                    // unhandled key belongs to the focused client.
                    tracing::error!(%err, "a script failed");
                    false
                }
            },
            Err(err) => {
                tracing::error!(%err, "`sol` is missing");
                false
            }
        };
        set_deadline(&self.lua, None);

        let pending = self.lua.remove_app_data::<Pending>().unwrap_or_default();

        Outcome {
            handled,
            commands: pending.commands,
            grab: pending.grab,
            status: pending.status,
            motion: pending.animated.then_some(pending.animation),
        }
    }
}

/// Whether a program can be found on `PATH`.
///
/// A plain lookup rather than a spawn: asking costs nothing, and finding out by
/// running it means finding out *after* something has already failed. A desktop
/// cannot assume any particular terminal or editor is installed, and a script
/// that picks the first one that *is* beats one that silently does nothing on a
/// machine set up differently from the author's.
pub(crate) fn which(program: &str) -> bool {
    if program.contains('/') {
        return std::path::Path::new(program).is_file();
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| directory.join(program).is_file())
}

/// Put a key combination into one canonical form.
///
/// So that `shift+super+q` and `super+shift+q` are the same binding. Done in
/// one place and used both when binding and when dispatching, because two
/// spellings of the same key is a bug that only shows up for whoever spelled it
/// the other way.
pub(crate) fn normalise_combo(combo: &str) -> String {
    let mut modifiers = Vec::new();
    let mut key = String::new();

    for part in combo.split('+') {
        let part = part.trim().to_ascii_lowercase();
        match part.as_str() {
            "" => {}
            "ctrl" | "control" => modifiers.push("ctrl"),
            "alt" | "meta" => modifiers.push("alt"),
            "shift" => modifiers.push("shift"),
            "super" | "logo" | "win" | "cmd" => modifiers.push("super"),
            _ => key = part,
        }
    }

    // A fixed order, and each modifier once.
    let mut canonical = String::new();
    for name in ["ctrl", "alt", "shift", "super"] {
        if modifiers.contains(&name) {
            canonical.push_str(name);
            canonical.push('+');
        }
    }
    canonical.push_str(&key);
    canonical
}

// The generated reference pages check their sources against the code here.
// Declared right above the tests, never higher: two tests read this file's
// production code as everything before its first `#[cfg(test)]`.
#[cfg(test)]
use crate::present::Curve;
#[cfg(test)]
pub(crate) use lua::deform_for_test;
#[cfg(test)]
mod reference;
#[cfg(test)]
mod source;

/// The one test helper other modules' tests share.
/// `state::tests::real_client::reflow_on_close::hosted::an_unknown_action_is_warned_of_the_first_time_only`.
#[cfg(test)]
pub(crate) use tests::logged_while;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::devices::DeviceInfo;
    use crate::input::devices::{DeviceKind, InputConfig};
    use crate::present::Frame;
    use smithay::utils::Logical;
    use smithay::utils::Rectangle;

    /// **A rect with a negative size, or a side that is not a number, is
    /// refused by name (#282)**, rather than reaching the renderer, which
    /// panicked on a negative size. Zero is a size.
    #[test]
    fn a_rect_with_a_negative_or_non_finite_side_is_refused() {
        let lua = mlua::Lua::new();
        let rect = |source: &str| {
            let table: mlua::Table = lua.load(source).eval().expect("a table");
            super::rect_from(&table)
        };
        assert!(rect("return { x = 0, y = 0, w = 0, h = 10 }").is_ok());
        for bad in [
            "return { x = 0, y = 0, w = -18, h = 13 }",
            "return { x = 0, y = 0, w = 10, h = -1 }",
            "return { x = 0/0, y = 0, w = 10, h = 10 }",
            "return { x = 0, y = 0, w = 1/0, h = 10 }",
        ] {
            let refused = rect(bad).expect_err(bad).to_string();
            assert!(
                refused.contains("a size of zero or more"),
                "{bad}: {refused}"
            );
        }
    }

    /// **`motion` times a present's properties apart, and anything else
    /// under it is refused by name** (#293). A property left out, and a
    /// duration or easing left out of one, is left for the batch's own.
    #[test]
    fn a_present_motion_names_its_properties_and_a_bad_one_is_refused() {
        let lua = mlua::Lua::new();
        let motion = |source: &str| {
            let table: mlua::Table = lua.load(source).eval().expect("a table");
            super::present_motion_from(&table)
        };
        assert_eq!(motion("return {}").expect("no motion"), None);
        let timed = motion(
            "return { motion = { x = { duration = 300, easing = 'linear' }, \
             opacity = { duration = 450 } } }",
        )
        .expect("a motion")
        .expect("one was given");
        assert_eq!(
            timed.x,
            (Some(Duration::from_millis(300)), Some(Curve::Linear))
        );
        assert_eq!(timed.y, (None, None), "left out, so the batch's");
        assert_eq!(timed.size, (None, None));
        assert_eq!(timed.opacity, (Some(Duration::from_millis(450)), None));
        assert_eq!(timed.client_opacity, (None, None));
        for (bad, says) in [
            ("return { motion = 3 }", "a table of timings by property"),
            ("return { motion = { colour = {} } }", "not colour"),
            ("return { motion = { [1] = {} } }", "a key of type integer"),
            (
                "return { motion = { x = 300 } }",
                "`motion.x` is { duration, easing }",
            ),
        ] {
            let refused = motion(bad).expect_err(bad).to_string();
            assert!(refused.contains(says), "{bad}: {refused}");
        }
    }

    /// **A present's motion is resolved against the batch's own timing**
    /// (#293): what a property leaves out is `sol.animate`'s, and a present
    /// with no `motion` carries none.
    #[test]
    fn a_present_motion_fills_what_it_leaves_out_from_the_batch() {
        let directory = std::env::temp_dir().join("solium-script-test-present-motion");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("super+1", function()
                sol.animate({ duration = 300, easing = "outBack" })
                sol.present(1, { x = 0, y = 0, w = 10, h = 10,
                                 motion = { opacity = { duration = 500 },
                                            size = { easing = "linear" } } })
                sol.present(2, { x = 0, y = 0, w = 10, h = 10 })
                sol.present_from(3, { x = 0, y = 0, w = 10, h = 10,
                                      motion = { x = { duration = 120 } } })
            end)
            "#,
        )
        .expect("writing the test script");
        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let outcome = scripts.key("super+1", Snapshot::default());
        let channels: Vec<_> = outcome
            .commands
            .iter()
            .map(|command| match command {
                Command::Present { channels, .. } | Command::PresentFrom { channels, .. } => {
                    channels.as_deref().copied()
                }
                other => panic!("expected a present command, got {other:?}"),
            })
            .collect();
        let batch = AnimationSpec {
            duration: Duration::from_millis(300),
            easing: Curve::OutBack,
        };
        assert_eq!(
            channels,
            vec![
                Some(crate::present::Channels {
                    x: batch,
                    y: batch,
                    size: AnimationSpec {
                        duration: Duration::from_millis(300),
                        easing: Curve::Linear,
                    },
                    opacity: AnimationSpec {
                        duration: Duration::from_millis(500),
                        easing: Curve::OutBack,
                    },
                    client_opacity: batch,
                }),
                None,
                Some(crate::present::Channels {
                    x: AnimationSpec {
                        duration: Duration::from_millis(120),
                        easing: Curve::OutBack,
                    },
                    y: batch,
                    size: batch,
                    opacity: batch,
                    client_opacity: batch,
                }),
            ]
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **`client_opacity` is a number from 0 to 1, and anything else is
    /// refused by name** (#296), with the call that was given it.
    #[test]
    fn a_client_opacity_is_a_number_from_0_to_1_and_anything_else_is_refused() {
        let lua = mlua::Lua::new();
        let read = |source: &str| {
            let table: mlua::Table = lua.load(source).eval().expect("a table");
            super::client_opacity_from(&table, "sol.present")
        };
        assert_eq!(read("return {}").expect("none"), None);
        assert_eq!(
            read("return { client_opacity = 0.25 }").expect("a quarter"),
            Some(0.25)
        );
        assert_eq!(
            read("return { client_opacity = 0 }").expect("an integer"),
            Some(0.0)
        );
        assert_eq!(
            read("return { client_opacity = 1 }").expect("the end"),
            Some(1.0)
        );
        for (bad, says) in [
            ("return { client_opacity = 'half' }", "got a string"),
            ("return { client_opacity = {} }", "got a table"),
            ("return { client_opacity = 1.5 }", "got 1.5"),
            ("return { client_opacity = -0.1 }", "got -0.1"),
            ("return { client_opacity = 0/0 }", "a number from 0 to 1"),
        ] {
            let refused = read(bad).expect_err(bad).to_string();
            assert!(
                refused.contains("sol.present: `client_opacity` is a number from 0 to 1")
                    && refused.contains(says),
                "{bad}: {refused}"
            );
        }
    }

    /// **Both calls carry `client_opacity`, and `motion` times it** (#296):
    /// `sol.present`'s is the client's opacity to land on, `present_from`'s
    /// the one to start from; a bad one fails the call that was given it.
    #[test]
    fn both_presents_carry_a_client_opacity_and_motion_times_it() {
        let directory = std::env::temp_dir().join("solium-script-test-client-opacity");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("super+1", function()
                sol.animate({ duration = 300, easing = "outBack" })
                sol.present(1, { client_opacity = 0,
                                 motion = { client_opacity = { duration = 450 } } })
                sol.present_from(2, { x = 0, y = 0, w = 10, h = 10, client_opacity = 0.5 })
                sol.present(3, {})
            end)
            sol.bind("super+2", function()
                sol.present_from(2, { x = 0, y = 0, w = 10, h = 10, client_opacity = 2 })
            end)
            "#,
        )
        .expect("writing the test script");
        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let outcome = scripts.key("super+1", Snapshot::default());
        let read: Vec<_> = outcome
            .commands
            .iter()
            .map(|command| match command {
                Command::Present {
                    client_opacity,
                    channels,
                    ..
                }
                | Command::PresentFrom {
                    client_opacity,
                    channels,
                    ..
                } => (
                    *client_opacity,
                    channels.as_deref().map(|channels| channels.client_opacity),
                ),
                other => panic!("expected a present command, got {other:?}"),
            })
            .collect();
        assert_eq!(
            read,
            vec![
                (
                    Some(0.0),
                    Some(AnimationSpec {
                        duration: Duration::from_millis(450),
                        easing: Curve::OutBack,
                    })
                ),
                (Some(0.5), None),
                (None, None),
            ]
        );
        let mut refused = None;
        let log = logged_while(|| {
            refused = Some(scripts.key("super+2", Snapshot::default()));
        });
        assert!(
            refused.is_some_and(|outcome| outcome.commands.is_empty())
                && log.contains("sol.present_from")
                && log.contains("client_opacity")
                && log.contains("got 2"),
            "a client_opacity of 2 was not refused by name:\n{log}"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **`sol.present_from` reads a deform for the frame it starts in**,
    /// checked as `sol.present`'s is.
    #[test]
    fn present_from_carries_the_deform_it_starts_in() {
        let directory = std::env::temp_dir().join("solium-script-test-present-from-deform");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("super+1", function()
                sol.present_from(1, { x = 0, y = 0, w = 60, h = 60,
                    deform = { effect = "genie", to = { x = 0, y = 0, w = 60, h = 60 } } })
                sol.present_from(2, { x = 0, y = 0, w = 60, h = 60 })
            end)
            "#,
        )
        .expect("writing the test script");
        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let outcome = scripts.key("super+1", Snapshot::default());
        let effects: Vec<Option<String>> = outcome
            .commands
            .iter()
            .map(|command| match command {
                Command::PresentFrom { deform, .. } => {
                    deform.as_ref().map(|deform| deform.effect.clone())
                }
                other => panic!("expected a present_from command, got {other:?}"),
            })
            .collect();
        assert_eq!(effects, vec![Some("genie".to_owned()), None]);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn combos_normalise_to_one_spelling() {
        assert_eq!(normalise_combo("super+space"), "super+space");
        assert_eq!(normalise_combo("Super+Space"), "super+space");
        assert_eq!(normalise_combo("shift+super+q"), "shift+super+q");
        // The whole point: written the other way round, still the same binding.
        assert_eq!(
            normalise_combo("super+shift+q"),
            normalise_combo("shift+super+q")
        );
        assert_eq!(normalise_combo("win+d"), "super+d");
        assert_eq!(normalise_combo("escape"), "escape");
    }

    #[test]
    fn a_repeated_modifier_is_not_repeated() {
        assert_eq!(normalise_combo("ctrl+ctrl+c"), "ctrl+c");
    }

    /// What `sol.window_at` asks of a window, [`crate::state::owns`], which
    /// was `Rect::contains` on the drawn rectangle until #134's fourth review.
    #[test]
    fn a_drawn_window_covers_its_own_top_left_but_not_its_bottom_right() {
        let drawn = Rectangle::<f64, Logical>::new((10.0, 20.0).into(), (100.0, 50.0).into());
        let slot = drawn.to_i32_round();
        let frame = Frame {
            rect: drawn,
            ..Frame::real(slot)
        };
        let covers = |x: f64, y: f64| crate::state::owns(slot, false, frame, (x, y).into(), &[]);
        assert!(covers(10.0, 20.0));
        assert!(covers(109.0, 69.0));
        // Exclusive, so adjacent thumbnails cannot both claim the same pixel.
        assert!(!covers(110.0, 70.0));
        assert!(!covers(9.0, 20.0));
    }

    /// The whole round trip, without a compositor: a script binds a key, the
    /// binding runs, and its transforms come back as commands.
    #[test]
    fn a_script_binds_a_key_and_its_commands_come_back() {
        let directory = std::env::temp_dir().join("solium-script-test");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("Super+Space", function()
                sol.animate({ duration = 300, easing = "outBack" })
                for _, window in ipairs(sol.windows()) do
                    sol.present(window.id, { x = 0, y = 0, w = 100, h = 50 })
                end
                sol.grab_input(true)
                sol.status("test")
            end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        assert!(scripts.has_binding("super+space"));
        // Normalisation is not just for the binding side.
        assert!(scripts.has_binding("SUPER+space"));
        assert!(!scripts.has_binding("super+tab"));

        let snapshot = Snapshot {
            locked: false,
            notifications: Default::default(),
            audio: Default::default(),
            tray: Default::default(),
            media: Default::default(),
            keyboard: crate::keymap::State::initial(),
            shortcuts_owner: None,
            windows: vec![WindowInfo {
                icon: None,
                client_side: false,
                blur_region: false,
                insets: crate::decoration::Insets::default(),
                id: 7,
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 800.0,
                    h: 600.0,
                },
                drawn: Drawn::default(),
                title: "a window".to_owned(),
                focused: true,
                monitor: "test-1".to_owned(),
                modal: false,
                parent: Parentage::None,
                leaving: false,
                minimized: false,
                above: false,
                app_id: String::new(),
                min: None,
                max: None,
                cramped: false,
                shown: true,
                fullscreen: false,
                maximized: false,
                style: String::new(),
                x11_type: None,
                accepts_input: true,
                class: None,
                instance: None,
                content_type: "none",
                pid: None,
            }],
            monitors: vec![MonitorInfo {
                name: "test-1".to_owned(),
                area: Rect {
                    x: 0.0,
                    y: 34.0,
                    w: 1600.0,
                    h: 866.0,
                },
                whole: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1600.0,
                    h: 900.0,
                },
                scale: 1.0,
                focused: true,
                primary: true,
                transform: "normal".to_owned(),
                off: false,
            }],
            work_area: Rect {
                x: 0.0,
                y: 34.0,
                w: 1600.0,
                h: 866.0,
            },
            cursor: (0.0, 0.0),
            screens: Vec::new(),
            text_input: None,
            problems: Vec::new(),
            apps: std::rc::Rc::default(),
        };

        let outcome = scripts.key("super+space", snapshot);
        assert!(outcome.handled);
        assert_eq!(outcome.grab, Some(true));
        assert_eq!(outcome.status.as_deref(), Some("test"));
        assert_eq!(outcome.commands.len(), 1);

        match &outcome.commands[0] {
            Command::Present {
                id,
                rect,
                animation,
                ..
            } => {
                assert_eq!(*id, 7);
                assert_eq!(rect.map(|rect| rect.w), Some(100.0));
                // `animate` applied to the batch queued after it.
                assert_eq!(animation.duration, Duration::from_millis(300));
                assert_eq!(animation.easing, Curve::OutBack);
            }
            other => panic!("expected a present command, got {other:?}"),
        }
    }

    /// `sol.pane_build` hands `config.pane_build` over as a duration, whole
    /// or fractional milliseconds, and a value that is no time at all changes
    /// nothing (#226).
    #[test]
    fn sol_pane_build_hands_the_budget_over() {
        let directory = std::env::temp_dir().join("solium-script-test-pane-build");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("Super+A", function() sol.pane_build(5) end)
            sol.bind("Super+B", function() sol.pane_build(0.5) end)
            sol.bind("Super+C", function() sol.pane_build(-1) end)
            sol.bind("Super+D", function() sol.pane_build("soon") end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let mut budget = |combo: &str| {
            let outcome = scripts.key(combo, empty_snapshot());
            assert!(outcome.handled, "{combo} was not handled");
            match outcome.commands.as_slice() {
                [Command::PaneBuild(budget)] => Some(*budget),
                [] => None,
                other => panic!("expected at most a budget from {combo}, got {other:?}"),
            }
        };
        assert_eq!(budget("super+a"), Some(Duration::from_millis(5)));
        assert_eq!(budget("super+b"), Some(Duration::from_micros(500)));
        assert_eq!(budget("super+c"), None, "a time before now is no budget");
        assert_eq!(budget("super+d"), None, "nor is a word");
    }

    /// **`sol.decoration` is still `sol.pane`.**
    ///
    /// The setting was renamed when a style stopped being one QML file and
    /// became a folder. `sol.decoration` stays because an `init.lua` someone
    /// wrote before that calls it by name, and Lua gives no warning for a call
    /// to a nil field -- it raises, the script that raised is abandoned, and a
    /// configuration that used to work comes up with no bindings and no
    /// layouts. One line to keep, and this is what says the line is there.
    ///
    /// Both are driven, and the *same* command has to come back from each: an
    /// alias bound to some other function would pass a test that only checked
    /// `sol.decoration` was callable.
    #[test]
    fn the_old_name_for_sol_pane_still_works() {
        let directory = std::env::temp_dir().join("solium-script-test-pane-alias");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("Super+P", function() sol.pane("border") end)
            sol.bind("Super+D", function() sol.decoration("border") end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let named = |scripts: &mut Scripts, combo: &str| {
            let outcome = scripts.key(combo, empty_snapshot());
            assert!(outcome.handled, "{combo} was not handled");
            match outcome.commands.as_slice() {
                [Command::Decoration { name }] => name.clone(),
                other => panic!("expected one style command from {combo}, got {other:?}"),
            }
        };

        assert_eq!(named(&mut scripts, "super+p").as_deref(), Some("border"));
        assert_eq!(
            named(&mut scripts, "super+d").as_deref(),
            Some("border"),
            "the old name reaches the same command as the new one"
        );
    }

    /// **A `user.lua` written before the rename still chooses a style.**
    ///
    /// The migration path the `sol.decoration` alias does *not* cover, and the
    /// one that would have broken quietly. `config.lua` merges the user's table
    /// over its defaults key by key, so a file setting the old `decoration`
    /// leaves `pane` at `"top"` -- their choice read, merged, and thrown away,
    /// with the shipped default on screen and nothing anywhere saying why.
    ///
    /// It runs the **shipped** `config.lua`, not a copy: the temporary
    /// directory holds only `init.lua` and `user.lua`, and `Scripts::load` puts
    /// that directory ahead of the shipped one on `package.path` -- so
    /// `require("user")` finds the fixture and `require("config")` finds the
    /// real thing. That is what makes this a test of the file under review
    /// rather than of a restatement of it.
    ///
    /// Both halves, because they are two edits with one purpose:
    ///
    /// * `config.pane` is the old key's value, which is the read-across;
    /// * `config.decoration` is `nil`, which is the stale key not surviving the
    ///   merge into the table the compositor reads.
    ///
    /// Guarded on a real `~/.config/solium`, which comes *first* on that path:
    /// a developer with their own `user.lua` would have it answer instead of
    /// the fixture, and the assertion would then be about their configuration.
    #[test]
    fn a_user_file_using_the_old_key_still_chooses_a_style() {
        let Some(own) = Scripts::user_config_dir() else {
            return;
        };
        if own.join("user.lua").exists() || own.join("config.lua").exists() {
            // This machine's own configuration would decide it, not the fixture.
            return;
        }

        let directory = std::env::temp_dir().join("solium-script-test-old-key");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        std::fs::write(
            directory.join("user.lua"),
            "return { decoration = \"border\" }\n",
        )
        .expect("writing the fixture user.lua");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            local config = require("config")
            sol.bind("Super+P", function()
                sol.pane(config.pane)
                sol.status(tostring(config.decoration))
            end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let outcome = scripts.key("super+p", empty_snapshot());
        assert!(outcome.handled);

        match outcome.commands.as_slice() {
            [Command::Decoration { name }] => assert_eq!(
                name.as_deref(),
                Some("border"),
                "a user.lua setting the old `decoration` key still names the style"
            ),
            other => panic!("expected one style command, got {other:?}"),
        }
        assert_eq!(
            outcome.status.as_deref(),
            Some("nil"),
            "and the old key does not survive into the configuration table"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// A fixture `user.lua` in a directory of its own, and the *shipped*
    /// `init.lua` loaded against it.
    ///
    /// The shipped entry point rather than a stand-in, because the thing under
    /// test in every caller below is whether the file people actually run
    /// honours the configuration -- a copy that requires the right modules in
    /// the right order would pass while the real one did not, which is the
    /// class of bug being fixed.
    ///
    /// `None` when this machine has any Lua of its own under
    /// `~/.config/solium`: that directory comes *first* on `package.path`, so
    /// one file there answers `require` ahead of the fixture or the shipped
    /// script it shadows, and the assertions below would then be about
    /// somebody's own configuration. Any `.lua` at all rather than a list of
    /// names, because the list would have to grow every time a script is added
    /// and would fail open when somebody forgot. The same guard, for the same
    /// reason, as `a_user_file_using_the_old_key_still_chooses_a_style`.
    fn shipped_init_with_user(name: &str, user: &str) -> Option<(std::path::PathBuf, Scripts)> {
        let (directory, loaded) = shipped_init_and_user(name, user)?;
        Some((directory, loaded.expect("loading the shipped init.lua")))
    }

    /// [`shipped_init_with_user`], with what the load answered, for a test of
    /// a `user.lua` that should be refused.
    fn shipped_init_and_user(
        name: &str,
        user: &str,
    ) -> Option<(std::path::PathBuf, anyhow::Result<Scripts>)> {
        let own = Scripts::user_config_dir()?;
        if let Ok(entries) = std::fs::read_dir(&own)
            && entries
                .filter_map(Result::ok)
                .any(|entry| entry.path().extension().is_some_and(|kind| kind == "lua"))
        {
            return None;
        }

        let directory = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).ok()?;
        std::fs::write(directory.join("user.lua"), user).ok()?;
        // Copied rather than required in place: `Scripts::load` puts the
        // config's *own* directory on `package.path` ahead of the shipped one,
        // and that is how the fixture `user.lua` beside it gets found at all.
        let shipped = crate::assets::lua().join("init.lua");
        let config = directory.join("init.lua");
        std::fs::copy(&shipped, &config).ok()?;

        let loaded = Scripts::load(&config);
        Some((directory, loaded))
    }

    /// Every spawn a key produced, as `(program, args)`.
    fn spawns(outcome: &Outcome) -> Vec<(String, Vec<String>)> {
        outcome
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::Spawn { program, args } => Some((program.clone(), args.clone())),
                _ => None,
            })
            .collect()
    }

    /// **A `user.lua` can add, replace and remove a binding.**
    ///
    /// The fault this is pinned against: bindings were `sol.bind` calls in
    /// `init.lua` and `user.lua` was a settings table, so the two could not
    /// meet. Binding `super+b` to a browser meant either editing a file the
    /// next update overwrites or writing your own `init.lua` -- two hundred
    /// lines of layout wiring, mode setup and terminal detection adopted in
    /// order to add one key (#117).
    ///
    /// Three assertions, because `config.bindings` makes three promises and
    /// only the first is the obvious one:
    ///
    ///  1. A combination nothing else uses is bound, and runs what it says.
    ///  2. A combination `init.lua` already bound is **replaced** -- the
    ///     decision recorded in `config.lua`'s comment, and the one that makes
    ///     the section worth having: refusing a clash would permit adding a
    ///     binding and forbid changing one, and changing one is what people
    ///     come here for.
    ///  3. `false` removes a shipped binding outright, so the compositor stops
    ///     swallowing the key rather than binding it to a handler that does
    ///     nothing.
    ///
    /// It runs the shipped `init.lua`, which is what makes it a test of the
    /// ordering as well as the mechanism: `require("bindings")` is the last
    /// line of that file precisely so the user's call to `sol.bind` is the
    /// later one, and moving it up would break assertion 2 alone.
    #[test]
    fn a_user_file_can_add_replace_and_remove_a_binding() {
        let Some((directory, mut scripts)) = shipped_init_with_user(
            "solium-script-test-user-bindings",
            r#"
            return {
                bindings = {
                    -- Added: nothing in the shipped configuration binds this.
                    ["super+f5"] = "fixture-browser",
                    -- Replaced: `init.lua` binds super+q to closing a window.
                    ["Super+Q"] = { "fixture-editor", "a file.txt" },
                    -- Removed: `init.lua` binds super+g to the tilt demo.
                    ["super+g"] = false,
                },
            }
            "#,
        ) else {
            return;
        };

        let added = scripts.key("super+f5", empty_snapshot());
        assert!(
            added.handled,
            "a binding from user.lua did not reach sol.bind"
        );
        assert_eq!(
            spawns(&added),
            vec![("fixture-browser".to_owned(), Vec::new())],
            "a string binding is a command line"
        );

        // Written `Super+Q` in the fixture and pressed as `super+q`: the
        // compositor normalises, so a configuration is not obliged to guess the
        // spelling `init.lua` happened to use.
        let replaced = scripts.key("super+q", empty_snapshot());
        assert!(replaced.handled);
        assert_eq!(
            spawns(&replaced),
            vec![("fixture-editor".to_owned(), vec!["a file.txt".to_owned()])],
            "the user's super+q did not win: a list keeps its spaces, and the shipped \
             binding closes a window rather than spawning anything"
        );
        assert!(
            !replaced
                .commands
                .iter()
                .any(|command| matches!(command, Command::Close { .. })),
            "the shipped super+q ran as well as the user's, which means both handlers \
             are registered and only one of them can be reached"
        );

        assert!(
            !scripts.has_binding("super+g"),
            "`false` left the combination bound, so the compositor still swallows the key"
        );

        // And `--check` says so, which is the other half of the decision: a
        // binding may replace a shipped one, but not quietly.
        let reported = scripts.bindings();
        let note = |combo: &str| {
            reported
                .iter()
                .find(|binding| binding.combo == combo)
                .unwrap_or_else(|| panic!("{combo} is not in what --check would print"))
                .note
                .clone()
        };
        assert_eq!(note("super+f5").as_deref(), Some("config.bindings"));
        assert_eq!(
            note("super+q").as_deref(),
            Some("config.bindings, replacing a shipped binding"),
            "--check must be able to tell a replacement from an addition"
        );
        let removed = scripts.unbound();
        assert_eq!(
            removed
                .iter()
                .map(|binding| binding.combo.as_str())
                .collect::<Vec<_>>(),
            vec!["super+g"],
            "a binding removed on purpose looks, in a list of what survived, exactly \
             like one that was never there -- so it is reported separately"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A list in `config.bindings` can ask to repeat and to fire at the
    /// lock** (#224), with the named fields beside the command and not in it;
    /// and a binding there replaces the shipped one whole, so a volume key
    /// rebound as a string asks for nothing.
    #[test]
    fn a_user_file_binding_can_ask_to_repeat_and_to_fire_at_the_lock() {
        let Some((directory, mut scripts)) = shipped_init_with_user(
            "solium-script-test-user-binding-options",
            r#"
            return {
                bindings = {
                    ["XF86AudioRaiseVolume"] = {
                        "fixture-mixer", "up", repeating = true, locked = true,
                    },
                    ["XF86AudioLowerVolume"] = "fixture-mixer down",
                    ["super+f5"] = { "fixture-browser" },
                },
            }
            "#,
        ) else {
            return;
        };

        let raised = scripts.key("xf86audioraisevolume", empty_snapshot());
        assert_eq!(
            spawns(&raised),
            vec![("fixture-mixer".to_owned(), vec!["up".to_owned()])],
            "the options became part of the command line"
        );
        let asked = |combo: &str| {
            scripts
                .bindings()
                .into_iter()
                .find(|binding| binding.combo == combo)
                .map(|binding| (binding.repeating, binding.locked))
        };
        assert_eq!(
            [
                asked("xf86audioraisevolume"),
                asked("xf86audiolowervolume"),
                asked("super+f5"),
            ],
            [
                Some((true, true)),
                Some((false, false)),
                Some((false, false))
            ],
            "(repeating, locked) for: the list that asked, the string that replaced a \
             shipped repeating key, the list that asked for nothing"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **An option a `config.bindings` list does not have is refused**, naming
    /// the binding, as a binding of any other wrong shape is: one that asks for
    /// something it does not get looks exactly like one that is broken.
    #[test]
    fn a_user_file_binding_with_an_option_it_does_not_have_is_refused() {
        for (case, entry, says) in [
            (
                "repeat",
                r#"{ "fixture-mixer", ["repeat"] = true }"#,
                "repeating",
            ),
            ("unknown", r#"{ "fixture-mixer", loud = true }"#, "loud"),
            (
                "not-a-boolean",
                r#"{ "fixture-mixer", locked = "yes" }"#,
                "locked",
            ),
        ] {
            let Some((directory, loaded)) = shipped_init_and_user(
                &format!("solium-script-test-user-binding-refused-{case}"),
                &format!("return {{ bindings = {{ [\"super+f5\"] = {entry} }} }}\n"),
            ) else {
                return;
            };
            let error = loaded
                .err()
                .map(|error| format!("{error:#}"))
                .unwrap_or_default();
            assert!(
                error.contains("super+f5") && error.contains(says),
                "{case}: not refused naming the binding and {says:?}: {error:?}"
            );
            let _ = std::fs::remove_dir_all(&directory);
        }
    }

    /// **`open.motion` and `open.scale` decide the open animation.**
    ///
    /// Both were documented in `config.lua` and read by nothing: `open.lua`
    /// held `220 / outBack` and `0.88` as local constants while the
    /// configuration advertised `200 / outCubic` and `0.92`, so editing either
    /// setting did nothing and there was no way to tell that apart from having
    /// misunderstood what it meant (#117).
    ///
    /// The numbers in the fixture are deliberately unlike both pairs, so this
    /// cannot pass by agreeing with whichever set of constants happens to be in
    /// the file.
    #[test]
    fn the_open_animation_is_the_one_the_configuration_names() {
        let Some((directory, mut scripts)) = shipped_init_with_user(
            "solium-script-test-open-animation",
            r#"
            return {
                open = {
                    motion = { duration = 777, easing = "linear" },
                    scale = 0.5,
                },
            }
            "#,
        ) else {
            return;
        };

        // 800x600 at the origin, from `one_screen`. At a scale of 0.5 the
        // window starts 400x300 about its own centre, so at 200,150.
        let outcome = scripts.opened(7, one_screen(&[7]));
        let from = outcome
            .commands
            .iter()
            .find_map(|command| match command {
                Command::PresentFrom {
                    id: 7,
                    rect,
                    animation,
                    ..
                } => Some((*rect, *animation)),
                _ => None,
            })
            .expect("the open animation did not run");

        assert_eq!(
            from.1.duration,
            Duration::from_millis(777),
            "open.motion.duration is not what the window arrives with"
        );
        assert_eq!(
            from.1.easing,
            Curve::Linear,
            "open.motion.easing is not what the window arrives with"
        );
        assert!(
            (from.0.w - 400.0).abs() < 0.5 && (from.0.h - 300.0).abs() < 0.5,
            "open.scale did not decide how small the window starts: {:?}",
            from.0
        );
        assert!(
            (from.0.x - 200.0).abs() < 0.5 && (from.0.y - 150.0).abs() < 0.5,
            "and it is still shrunk about its own centre: {:?}",
            from.0
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **`scrolling.widths` and `scrolling.default_width` decide how wide a
    /// column opens.**
    ///
    /// The other two settings that were documented and read by nothing: the
    /// strip used the constant list in `crates/layout/src/scroller.rs`, so a
    /// configuration asking for quarters got thirds (#117).
    ///
    /// Driven through the shipped `scrolling.lua` rather than by calling
    /// `sol.layout.scroller` from a fixture, because the missing link was the
    /// *call site* -- the constructor took no argument, and no test of the
    /// layout crate could have seen that. 0.8 of a 1600-wide screen is
    /// unmistakably neither of the shipped presets near it.
    #[test]
    fn the_configured_widths_are_what_a_column_opens_at() {
        let Some((directory, mut scripts)) = shipped_init_with_user(
            "solium-script-test-scrolling-widths",
            r#"
            return {
                gap = 0,
                scrolling = { widths = { 0.25, 0.8 }, default_width = 2 },
            }
            "#,
        ) else {
            return;
        };

        scripts.monitors_changed(one_screen(&[]));
        // super+s is the shipped binding that puts the scrolling layout in
        // charge; without it the strip is built but nothing places anything.
        scripts.key("super+s", one_screen(&[]));
        let outcome = scripts.opened(7, one_screen(&[7]));

        let placed = outcome
            .commands
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::Place { id: 7, rect, .. } => Some(*rect),
                _ => None,
            })
            .expect("the scrolling layout placed nothing");
        assert!(
            (placed.w - 1280.0).abs() < 1.0,
            "a new column did not open at `default_width` of `widths` -- 0.8 of a \
             1600-wide view is 1280, and the shipped presets would give 533, 800 or \
             1066: {placed:?}"
        );

        // And the cycle is over the configured list, not the shipped one: one
        // step from the second of two wraps to the first.
        let cycled = scripts.key("super+r", one_screen(&[7]));
        let after = cycled
            .commands
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::Place { id: 7, rect, .. } => Some(*rect),
                _ => None,
            })
            .expect("super+r placed nothing");
        assert!(
            (after.w - 400.0).abs() < 1.0,
            "super+r did not cycle through the configured widths: 0.25 of 1600 is 400, \
             and a lap of a two-entry list is two steps: {after:?}"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A key the defaults do not define is reported, not swallowed.**
    ///
    /// `config.lua`'s `merge` validated nothing, so `tilling = { ... }` was
    /// merged in as a new section, read by nothing, for ever -- and `solium
    /// --check`, the one command whose job is answering "did my configuration
    /// work", said it had loaded fine (#117). It had. That was never the
    /// question.
    ///
    /// Four fixtures, and the last two are the ones that make this worth
    /// having: a check that reports real typos and also reports working
    /// settings is a check nobody will keep running.
    #[test]
    fn an_unrecognised_setting_is_reported_rather_than_merged_in_silence() {
        let Some((directory, scripts)) = shipped_init_with_user(
            "solium-script-test-unknown-settings",
            r#"
            return {
                -- A typo at the top level, and one inside a section.
                tilling = { split = 0.6 },
                open = { scal = 0.5 },
                -- A near miss on the *deprecated* spelling of `pane`. Reported,
                -- because it is not a key anything reads -- and reported with no
                -- suggestion, because the only word within two edits of it is
                -- `decoration`, and answering a typo with a deprecated spelling
                -- walks somebody past the key that actually works (#117 review).
                decoraton = "border",
                -- Not typos, and must not be reported: `keyboard` leaves its
                -- xkb names out on purpose, so they cannot be checked against
                -- the defaults, and a binding combination is whatever you
                -- press. `active`
                -- belongs with a dual layout and is exactly the pair that used
                -- to be called a typo.
                keyboard = { layout = "us,ua", active = 2 },
                bindings = { ["super+f6"] = "fixture-nothing" },
            }
            "#,
        ) else {
            return;
        };

        let found = scripts.unknown_settings();
        let reported: Vec<(&str, Option<&str>)> = found
            .iter()
            .map(|setting| (setting.key.as_str(), setting.meant.as_deref()))
            .collect();
        assert_eq!(
            reported,
            vec![
                ("decoraton", None),
                ("open.scal", Some("open.scale")),
                ("tilling", Some("tiling"))
            ],
            "the unrecognised keys are wrong: a near miss inside a section must be \
             reported by its full path, a near miss on a deprecated name must not be \
             answered with that name, and neither the `keyboard` section nor a binding \
             combination may be called a typo"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// The `sol.surface` declarations named `shell`, and whether one was taken
    /// away, out of a batch of commands.
    fn shell_surfaces(commands: &[Command]) -> (Vec<crate::scripted::Declaration>, bool) {
        let declared = commands
            .iter()
            .filter_map(|command| match command {
                Command::Surface(declared) if declared.name == "shell" => {
                    Some((**declared).clone())
                }
                _ => None,
            })
            .collect();
        let removed = commands
            .iter()
            .any(|command| matches!(command, Command::SurfaceGone(name) if name == "shell"));
        (declared, removed)
    }

    /// The shipped configuration, then the screens arriving, as `shell` sees
    /// it: what a cold start and a reload both do.
    fn shell_after_monitors(name: &str, user: &str) -> Option<(Scripts, Vec<Command>)> {
        let (directory, mut scripts) = shipped_init_with_user(name, user)?;
        let mut commands = scripts.startup().commands;
        commands.extend(scripts.monitors_changed(one_screen(&[])).commands);
        let _ = std::fs::remove_dir_all(&directory);
        Some((scripts, commands))
    }

    /// **With nothing configured, no shell is hosted**, and the shipped
    /// configuration says so every time it runs rather than by staying quiet.
    ///
    /// The second half is what makes taking a shell out of `user.lua` work on
    /// `super+shift+r`: a surface outlives a reload until something removes it
    /// by name, and a reload runs the configuration again, so `shell.lua`
    /// removes `shell` whenever none is configured.
    #[test]
    fn the_shipped_configuration_hosts_no_shell_of_its_own() {
        // The environment has already chosen a shell for this process, and
        // that is what it is for; there is nothing for this test to say.
        if std::env::var_os("SOLIUM_SHELL_SCENE").is_some() {
            return;
        }
        let Some((_, commands)) = shell_after_monitors("solium-script-test-no-shell", "return {}")
        else {
            return;
        };
        let (declared, removed) = shell_surfaces(&commands);
        assert!(
            declared.is_empty(),
            "the shipped configuration hosted a shell: {declared:?}"
        );
        assert!(
            removed,
            "with no shell configured, `shell` must be taken away, or one taken out of \
             user.lua stays on screen after a reload"
        );
    }

    /// **`properties` arrive as values, keys sorted**, so a configuration
    /// that declares the same table twice declares the same surface twice.
    #[test]
    fn a_surfaces_properties_reach_the_declaration_as_sorted_values() {
        let directory = std::env::temp_dir().join("solium-script-test-properties");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"sol.surface("bar", { scene = "/solium-fixture/bar.qml", properties = { b = 2, a = { y = 1, x = "s" } } })"#,
        )
        .expect("writing the test script");
        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let declared: Vec<String> = scripts
            .startup()
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::Surface(declared) => Some(declared.properties.render()),
                _ => None,
            })
            .collect();
        assert_eq!(declared, vec![r#"{"a":{"x":"s","y":1},"b":2}"#.to_owned()]);
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **`reserve` names the edges a surface takes out of the work area**,
    /// an edge it leaves out reserving nothing (#162, Ruling 10).
    #[test]
    fn sol_surface_reads_the_edges_it_reserves() {
        let directory = std::env::temp_dir().join("solium-script-test-reserve");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"sol.surface("bar", { scene = "/solium-fixture/bar.qml", reserve = { bottom = 48, left = 4 } })"#,
        )
        .expect("writing the test script");
        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let reserves: Vec<crate::scripted::Edges> = scripts
            .startup()
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::Surface(declared) => Some(declared.reserve),
                _ => None,
            })
            .collect();
        assert_eq!(
            reserves,
            vec![crate::scripted::Edges {
                bottom: 48,
                left: 4,
                ..Default::default()
            }]
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A negative reserve fails the load**, as a half rect does (Ruling 10).
    #[test]
    fn a_negative_reserve_is_refused() {
        let directory = std::env::temp_dir().join("solium-script-test-reserve-negative");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"sol.surface("bar", { scene = "/solium-fixture/bar.qml", reserve = { top = -1 } })"#,
        )
        .expect("writing the test script");
        assert!(
            Scripts::load(&config).is_err(),
            "a negative reserve must fail the load, as a half rect does"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **`outside_click` is a word or a table of them** (Ruling 13): a word
    /// is every grab's policy, and a table names grabs, with `default` for
    /// the rest; a surface that says nothing swallows.
    #[test]
    fn sol_surface_reads_outside_click_as_a_word_or_a_table() {
        let directory = std::env::temp_dir().join("solium-script-test-outside");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.surface("a", { scene = "/solium-fixture/a.qml", outside_click = "pass" })
            sol.surface("b", { scene = "/solium-fixture/b.qml", outside_click = { default = "swallow", ["tray-menu"] = "pass" } })
            sol.surface("c", { scene = "/solium-fixture/c.qml" })
            "#,
        )
        .expect("writing the test script");
        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let policies: Vec<crate::scripted::OutsideClick> = scripts
            .startup()
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::Surface(declared) => Some(declared.outside_click.clone()),
                _ => None,
            })
            .collect();
        let _ = std::fs::remove_dir_all(&directory);
        use crate::scripted::Outside;
        let [a, b, c] = policies.as_slice() else {
            panic!("expected three surfaces, got {policies:?}");
        };
        assert_eq!(
            [
                a.for_grab("anything"),
                b.for_grab("tray-menu"),
                b.for_grab("search"),
                c.for_grab("anything"),
            ],
            [
                Outside::Pass,
                Outside::Pass,
                Outside::Swallow,
                Outside::Swallow
            ],
            "[a word, a grab the table names, one it does not, a surface that says nothing]"
        );
    }

    /// **`keyboard.bindings` says which bindings still work while the scene
    /// holds the keyboard** (Q3, Ruling 14): every one but the claimed keys
    /// when it says nothing, as when it says `"except_claimed"`.
    #[test]
    fn sol_surface_reads_its_keyboard_bindings() {
        let directory = std::env::temp_dir().join("solium-script-test-keyboard-bindings");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.surface("a", { scene = "/solium-fixture/a.qml", keyboard = { bindings = "all" } })
            sol.surface("b", { scene = "/solium-fixture/b.qml", keyboard = { bindings = "none" } })
            sol.surface("c", { scene = "/solium-fixture/c.qml", keyboard = { bindings = "except_claimed" } })
            sol.surface("d", { scene = "/solium-fixture/d.qml", keyboard = {} })
            sol.surface("e", { scene = "/solium-fixture/e.qml" })
            "#,
        )
        .expect("writing the test script");
        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let policies: Vec<crate::scripted::KeyPolicy> = scripts
            .startup()
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::Surface(declared) => Some(declared.keyboard),
                _ => None,
            })
            .collect();
        let _ = std::fs::remove_dir_all(&directory);
        use crate::scripted::KeyPolicy;
        assert_eq!(
            policies,
            [
                KeyPolicy::All,
                KeyPolicy::NoBindings,
                KeyPolicy::ExceptClaimed,
                KeyPolicy::ExceptClaimed,
                KeyPolicy::ExceptClaimed,
            ],
            "[all, none, except_claimed, an empty table, nothing said]"
        );
    }

    /// **A `keyboard.bindings` that is none of the three fails the load**, as
    /// an unknown `outside_click` does.
    #[test]
    fn an_unknown_keyboard_bindings_is_refused() {
        let directory = std::env::temp_dir().join("solium-script-test-keyboard-unknown");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        let mut refused = Vec::new();
        for options in [
            r#"keyboard = { bindings = "some" }"#,
            r#"keyboard = "all""#,
            "keyboard = { bindings = true }",
        ] {
            std::fs::write(
                &config,
                format!(
                    r#"sol.surface("bar", {{ scene = "/solium-fixture/bar.qml", {options} }})"#
                ),
            )
            .expect("writing the test script");
            refused.push(Scripts::load(&config).is_err());
        }
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            refused,
            [true, true, true],
            "[a word it does not know, a word, a bool]"
        );
    }

    /// **An `outside_click` that is neither word fails the load**, as a
    /// negative reserve does, rather than quietly swallowing.
    #[test]
    fn an_unknown_outside_click_is_refused() {
        let directory = std::env::temp_dir().join("solium-script-test-outside-unknown");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        let mut refused = Vec::new();
        for options in [
            r#"outside_click = "through""#,
            r#"outside_click = { ["tray-menu"] = "through" }"#,
            "outside_click = 1",
        ] {
            std::fs::write(
                &config,
                format!(
                    r#"sol.surface("bar", {{ scene = "/solium-fixture/bar.qml", {options} }})"#
                ),
            )
            .expect("writing the test script");
            refused.push(Scripts::load(&config).is_err());
        }
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            refused,
            [true, true, true],
            "[an unknown word, an unknown word for a named grab, not a word or a table]"
        );
    }

    /// **A `properties` table with a list part keeps its named keys**: the
    /// bag is always read as an object, so `{ "x", label = "y" }` loses
    /// nothing, as it did not before properties became values.
    #[test]
    fn a_surfaces_properties_keep_their_named_keys_beside_a_list_part() {
        let directory = std::env::temp_dir().join("solium-script-test-properties-list-part");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"sol.surface("bar", { scene = "/solium-fixture/bar.qml", properties = { "x", label = "y" } })"#,
        )
        .expect("writing the test script");
        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let declared: Vec<String> = scripts
            .startup()
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::Surface(declared) => Some(declared.properties.render()),
                _ => None,
            })
            .collect();
        assert_eq!(declared, vec![r#"{"1":"x","label":"y"}"#.to_owned()]);
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **`shell.scene` in `user.lua` hosts that scene**: over the windows,
    /// taking the pointer, on every monitor, with `~` expanded, and told
    /// nothing by hand about the screen it is on -- and `shell.scene` is a
    /// setting, not a typo.
    #[test]
    fn the_shell_scene_is_read_from_the_configuration() {
        if std::env::var_os("SOLIUM_SHELL_SCENE").is_some() {
            return;
        }
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        let Some((scripts, commands)) = shell_after_monitors(
            "solium-script-test-shell-scene",
            r#"return { shell = { scene = "~/solium-fixture-shell/shell.qml" } }"#,
        ) else {
            return;
        };

        let unknown: Vec<String> = scripts
            .unknown_settings()
            .into_iter()
            .map(|setting| setting.key)
            .collect();
        assert!(
            unknown.is_empty(),
            "`shell.scene` was reported as unrecognised: {unknown:?}"
        );

        let (declared, _) = shell_surfaces(&commands);
        let [shell] = declared.as_slice() else {
            panic!("expected one `shell` surface, got {declared:?}");
        };
        assert_eq!(
            shell.scene,
            std::path::PathBuf::from(home).join("solium-fixture-shell/shell.qml"),
            "the scene is not the configured one with `~` expanded"
        );
        assert_eq!(shell.layer, crate::scripted::Layer::Top);
        assert!(shell.interactive, "a shell must take the pointer");
        assert_eq!(
            shell.on,
            crate::scripted::On::EveryMonitor,
            "the shell is not on every monitor"
        );
        assert!(
            shell.properties.get("screenInfo").is_none(),
            "the shell was handed `screenInfo`, which `Solium.monitor` replaces: {:?}",
            shell.properties
        );
    }

    /// **The shell is on every monitor, unless the configuration names one**:
    /// `shell.on` is a setting, `"every-monitor"` by default (#161, Ruling 5).
    #[test]
    fn the_shell_is_on_every_monitor_unless_the_configuration_names_one() {
        if std::env::var_os("SOLIUM_SHELL_SCENE").is_some() {
            return;
        }
        for (user, expected) in [
            (
                r#"return { shell = { scene = "/solium-fixture/shell.qml" } }"#,
                crate::scripted::On::EveryMonitor,
            ),
            (
                r#"return { shell = { scene = "/solium-fixture/shell.qml", on = "primary" } }"#,
                crate::scripted::On::Primary,
            ),
        ] {
            let Some((scripts, commands)) =
                shell_after_monitors("solium-script-test-shell-on", user)
            else {
                return;
            };
            assert!(
                scripts.unknown_settings().is_empty(),
                "`shell.on` was reported as unrecognised"
            );
            let (declared, _) = shell_surfaces(&commands);
            let placed: Vec<&crate::scripted::On> =
                declared.iter().map(|shell| &shell.on).collect();
            assert!(
                !placed.is_empty() && placed.iter().all(|on| **on == expected),
                "{user}: {placed:?}"
            );
        }
    }

    /// **The hosted shell's outside press is the user's to configure**
    /// (Q2, Ruling 13): `shell.outside_click` in `config.lua` is the shell's
    /// `outside_click`, a word or a table naming grabs, and swallows by
    /// default.
    #[test]
    fn the_shell_takes_its_outside_click_from_the_configuration() {
        if std::env::var_os("SOLIUM_SHELL_SCENE").is_some() {
            return;
        }
        use crate::scripted::Outside;
        for (user, tray_menu, search) in [
            (
                r#"return { shell = { scene = "/solium-fixture/shell.qml" } }"#,
                Outside::Swallow,
                Outside::Swallow,
            ),
            (
                r#"return { shell = { scene = "/solium-fixture/shell.qml", outside_click = "pass" } }"#,
                Outside::Pass,
                Outside::Pass,
            ),
            (
                r#"return { shell = { scene = "/solium-fixture/shell.qml", outside_click = { default = "swallow", ["tray-menu"] = "pass" } } }"#,
                Outside::Pass,
                Outside::Swallow,
            ),
        ] {
            let Some((scripts, commands)) =
                shell_after_monitors("solium-script-test-shell-outside", user)
            else {
                return;
            };
            assert!(
                scripts.unknown_settings().is_empty(),
                "`shell.outside_click` was reported as unrecognised"
            );
            let (declared, _) = shell_surfaces(&commands);
            let policies: Vec<(Outside, Outside)> = declared
                .iter()
                .map(|shell| {
                    (
                        shell.outside_click.for_grab("tray-menu"),
                        shell.outside_click.for_grab("search"),
                    )
                })
                .collect();
            assert!(
                !policies.is_empty()
                    && policies.iter().all(|policy| *policy == (tray_menu, search)),
                "{user}: {policies:?}"
            );
        }
    }

    /// **Which bindings still work while the hosted shell holds the keyboard
    /// is the user's to configure** (Q3): `shell.keyboard.bindings` in
    /// `config.lua` is the shell's `keyboard.bindings`, and every binding but
    /// the claimed keys works by default.
    #[test]
    fn the_shell_takes_its_keyboard_bindings_from_the_configuration() {
        if std::env::var_os("SOLIUM_SHELL_SCENE").is_some() {
            return;
        }
        use crate::scripted::KeyPolicy;
        for (user, expected) in [
            (
                r#"return { shell = { scene = "/solium-fixture/shell.qml" } }"#,
                KeyPolicy::ExceptClaimed,
            ),
            (
                r#"return { shell = { scene = "/solium-fixture/shell.qml", keyboard = { bindings = "none" } } }"#,
                KeyPolicy::NoBindings,
            ),
            (
                r#"return { shell = { scene = "/solium-fixture/shell.qml", keyboard = { bindings = "all" } } }"#,
                KeyPolicy::All,
            ),
        ] {
            let Some((scripts, commands)) =
                shell_after_monitors("solium-script-test-shell-keyboard", user)
            else {
                return;
            };
            assert!(
                scripts.unknown_settings().is_empty(),
                "`shell.keyboard.bindings` was reported as unrecognised"
            );
            let (declared, _) = shell_surfaces(&commands);
            let policies: Vec<KeyPolicy> = declared.iter().map(|shell| shell.keyboard).collect();
            assert!(
                !policies.is_empty() && policies.iter().all(|policy| *policy == expected),
                "{user}: {policies:?}"
            );
        }
    }

    /// **`SOLIUM_SHELL_SCENE` wins over `shell.scene`**, because it is set per
    /// run -- `dev/run-shell.sh` sets it to the scene being worked on.
    ///
    /// In a child process, because the environment is the process's and the
    /// tests share one: setting the variable here would change what every
    /// other test running beside this one reads. The child is this binary,
    /// running [`shell_scene_under_the_environment`] alone with the variable
    /// set.
    #[test]
    fn the_environment_overrides_the_configured_shell_scene() {
        let binary = std::env::current_exe().expect("this test binary");
        let output = std::process::Command::new(binary)
            .args([
                "--exact",
                "script::tests::shell_scene_under_the_environment",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("SOLIUM_SHELL_SCENE", ENVIRONMENT_SHELL)
            .env_remove("RUST_LOG")
            .output()
            .expect("running the child");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "the child failed:\n{stdout}\n{stderr}"
        );
        if stdout.contains("shell-override: skipped") {
            return;
        }
        assert!(
            stdout.contains("shell-override: checked"),
            "the child did not run the check:\n{stdout}\n{stderr}"
        );
    }

    /// What the child is given, and what its configuration says instead.
    const ENVIRONMENT_SHELL: &str = "/solium-fixture/from-the-environment/shell.qml";
    const CONFIGURED_SHELL: &str = "/solium-fixture/from-the-configuration/shell.qml";

    /// The child half of `the_environment_overrides_the_configured_shell_scene`.
    #[test]
    #[ignore = "run by the_environment_overrides_the_configured_shell_scene, with the variable set"]
    fn shell_scene_under_the_environment() {
        // `cargo test -- --include-ignored` runs this in the test process
        // itself, without the variable. Nothing to check there, and the line
        // printed is neither marker that
        // `the_environment_overrides_the_configured_shell_scene` accepts.
        if std::env::var("SOLIUM_SHELL_SCENE").as_deref() != Ok(ENVIRONMENT_SHELL) {
            println!("shell-override: not run by the parent, nothing checked");
            return;
        }
        let Some((_, commands)) = shell_after_monitors(
            "solium-script-test-shell-override",
            &format!("return {{ shell = {{ scene = \"{CONFIGURED_SHELL}\" }} }}"),
        ) else {
            println!("shell-override: skipped");
            return;
        };
        let (declared, _) = shell_surfaces(&commands);
        let scenes: Vec<&std::path::Path> =
            declared.iter().map(|shell| shell.scene.as_path()).collect();
        assert_eq!(
            scenes,
            vec![std::path::Path::new(ENVIRONMENT_SHELL)],
            "the configured scene won over SOLIUM_SHELL_SCENE"
        );
        println!("shell-override: checked");
    }

    /// A configured cursor theme and size reach the compositor as written.
    ///
    /// The wiring between `sol.cursor_theme` and `Command::Cursor`, which is the
    /// one part of the cursor work that no test in `cursor.rs` can reach: that
    /// module's tests start from a `Configured` that they built themselves.
    #[test]
    fn a_configured_cursor_theme_reaches_the_compositor() {
        let directory = std::env::temp_dir().join("solium-script-test-cursor");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("Super+C", function()
                sol.cursor_theme({ theme = "Fixture", size = 40 })
            end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let outcome = scripts.key("super+c", empty_snapshot());
        assert!(outcome.handled);
        match outcome.commands.as_slice() {
            [Command::Cursor(configured)] => {
                assert_eq!(configured.theme.as_deref(), Some("Fixture"));
                assert_eq!(configured.size, Some(40));
            }
            other => panic!("expected one cursor command, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **`cursor.scene` reaches the compositor as written**, for the pointer to
    /// look for as the shell's scene is looked for; a value that is not a
    /// string is no scene, with a line in the log, rather than a configuration
    /// that fails to load.
    #[test]
    fn a_configured_cursor_scene_reaches_the_compositor() {
        let directory = std::env::temp_dir().join("solium-script-test-cursor-scene");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("Super+C", function()
                sol.cursor_theme({ scene = "~/.config/solium/cursor/Cursor.qml", size = 32 })
            end)
            sol.bind("Super+V", function()
                sol.cursor_theme({ scene = 5 })
            end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let scenes: Vec<_> = ["super+c", "super+v"]
            .into_iter()
            .map(
                |key| match scripts.key(key, empty_snapshot()).commands.as_slice() {
                    [Command::Cursor(configured)] => configured.scene.clone(),
                    other => panic!("expected one cursor command, got {other:?}"),
                },
            )
            .collect();
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            scenes,
            [Some("~/.config/solium/cursor/Cursor.qml".to_owned()), None],
            "[a path, a number]"
        );
    }

    /// **The shipped configuration turns the screens off after ten minutes**,
    /// the same number the compositor holds for a configuration that says
    /// nothing, and a `user.lua` can say 0. See `crate::idle::Settings`.
    #[test]
    fn the_shipped_configuration_turns_the_screens_off_after_ten_minutes() {
        for (user, after) in [
            ("return {}", Duration::from_secs(600)),
            (
                "return { idle = { screens_off_after = 0 } }",
                Duration::ZERO,
            ),
        ] {
            let Some((directory, mut scripts)) =
                shipped_init_with_user("solium-script-test-idle-default", user)
            else {
                return;
            };
            let idle: Vec<crate::idle::Settings> = scripts
                .startup()
                .commands
                .into_iter()
                .filter_map(|command| match command {
                    Command::Idle(settings) => Some(settings),
                    _ => None,
                })
                .collect();
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                idle,
                [crate::idle::Settings {
                    screens_off_after: after,
                    ..crate::idle::Settings::default()
                }],
                "{user}: the shipped init.lua did not hand `config.idle` over as it says"
            );
            assert_eq!(
                idle.first().map(|settings| settings.off_frame_interval),
                Some(Duration::from_secs(1))
            );
        }
    }

    /// **The shipped configuration locks after five minutes**, the number the
    /// compositor holds for a configuration that says nothing; a `user.lua`
    /// can say 0, and one that says something that is not a number of 0 or
    /// more keeps the five minutes. See `crate::idle::Settings`.
    #[test]
    fn the_shipped_configuration_locks_after_five_minutes() {
        for (user, after) in [
            ("return {}", Duration::from_secs(300)),
            ("return { idle = { lock_after = 0 } }", Duration::ZERO),
            (
                "return { idle = { lock_after = 90 } }",
                Duration::from_secs(90),
            ),
            (
                "return { idle = { lock_after = \"soon\" } }",
                Duration::from_secs(300),
            ),
            (
                "return { idle = { lock_after = -1 } }",
                Duration::from_secs(300),
            ),
        ] {
            let Some((directory, mut scripts)) =
                shipped_init_with_user("solium-script-test-idle-lock", user)
            else {
                return;
            };
            let after_seen: Vec<Duration> = scripts
                .startup()
                .commands
                .into_iter()
                .filter_map(|command| match command {
                    Command::Idle(settings) => Some(settings.lock_after),
                    _ => None,
                })
                .collect();
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                after_seen,
                [after],
                "{user}: the shipped init.lua did not hand `idle.lock_after` over as it says"
            );
        }
        assert_eq!(
            crate::idle::Settings::default().lock_after,
            Duration::from_secs(300)
        );
    }

    #[test]
    fn shortcuts_policy_and_escape_from_user_lua_reach_the_shipped_configuration() {
        let (directory, mut scripts) = shipped_init_with_user(
            "solium-script-test-shortcuts-user",
            r#"return { input = { shortcuts_inhibit = {
                allow = "fullscreen", escape = "super+x", apps = { deny = { "blocked*" } }
            } } }"#,
        )
        .expect("shipped configuration");
        assert!(
            scripts
                .bindings()
                .iter()
                .any(|binding| binding.combo == "super+x" && binding.inhibited)
        );
        let settings = scripts
            .startup()
            .commands
            .into_iter()
            .find_map(|command| match command {
                Command::ShortcutsInhibit(settings) => Some(settings),
                _ => None,
            })
            .expect("shipped init hands over shortcut policy");
        assert_eq!(settings.allow, crate::shortcuts_inhibit::Allow::Fullscreen);
        assert_eq!(settings.escape, "super+x");
        assert!(
            settings
                .deny
                .iter()
                .any(|pattern| pattern.matches("blocked.viewer"))
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn shortcuts_policy_validates_permission_escape_and_application_patterns() {
        use crate::shortcuts_inhibit::Allow;
        let lua = Lua::new();
        for (value, allowed) in [
            ("true", Allow::Always),
            ("false", Allow::Never),
            ("'fullscreen'", Allow::Fullscreen),
        ] {
            let table: Table = lua
                .load(format!(
                    "return {{allow={value}, escape='Super+X', apps={{deny={{'blocked*'}}}}}}"
                ))
                .eval()
                .expect("table");
            let settings = settings::input::shortcuts(
                &lua::convert::doc_of(&Value::Table(table)).expect("a document"),
            )
            .expect("valid policy");
            assert_eq!(settings.allow, allowed);
            assert_eq!(settings.escape, "super+x");
            assert!(settings.deny[0].matches("blocked.viewer"));
            assert!(!settings.deny[0].matches("trusted.viewer"));
        }
        for spec in [
            "{allow='always'}",
            "{escape=''}",
            "{escape='bogus+q'}",
            "{escape='super+no_such_key'}",
            "{apps={deny={'a*b'}}}",
            "{apps={deny={''}}}",
        ] {
            let table: Table = lua.load(format!("return {spec}")).eval().expect("table");
            assert!(
                settings::input::shortcuts(
                    &lua::convert::doc_of(&Value::Table(table)).expect("a document")
                )
                .is_err(),
                "accepted invalid policy {spec}"
            );
        }
    }

    fn input_config(outcome: Outcome) -> Option<InputConfig> {
        outcome
            .commands
            .into_iter()
            .find_map(|command| match command {
                Command::Input(config) => Some(config),
                _ => None,
            })
    }

    /// **The shipped configuration taps and scrolls naturally on a touchpad,
    /// and does neither on a mouse** -- `config.lua`'s own `input` section,
    /// read through `sol.input` exactly as `idle` is. A `user.lua` overriding
    /// one field of `touchpad` keeps the other.
    #[test]
    fn the_shipped_configuration_configures_a_touchpad_and_a_mouse() {
        let Some((directory, mut scripts)) =
            shipped_init_with_user("solium-script-test-input-default", "return {}")
        else {
            return;
        };
        let config = input_config(scripts.startup()).expect("`init.lua` calls `sol.input`");
        let _ = std::fs::remove_dir_all(&directory);

        let touchpad = DeviceInfo {
            id: "touchpad".into(),
            name: "Synaptics TouchPad".into(),
            kind: DeviceKind::Touchpad,
            vendor: None,
            product: None,
            output: None,
        };
        let mouse = DeviceInfo {
            id: "mouse".into(),
            name: "Any Mouse".into(),
            kind: DeviceKind::Mouse,
            vendor: None,
            product: None,
            output: None,
        };
        assert_eq!(config.resolve(&touchpad).tap, Some(true));
        assert_eq!(config.resolve(&touchpad).natural_scroll, Some(true));
        assert_eq!(
            config.resolve(&mouse).natural_scroll,
            None,
            "a mouse is not a surface your fingers are on; the shipped config says nothing \
             about it, leaving libinput's own default"
        );
    }

    /// **A `devices` entry matches the device `config.lua` names, overriding
    /// only the fields it sets.** The integration half of
    /// `devices::tests::a_name_matched_override_changes_only_the_fields_it_names`:
    /// this proves the override actually reaches `Command::Input` from real
    /// Lua syntax, parenthesis and all.
    #[test]
    fn a_devices_entry_in_user_lua_overrides_one_field_of_the_matched_device() {
        let Some((directory, mut scripts)) = shipped_init_with_user(
            "solium-script-test-input-override",
            r#"
            return {
                input = {
                    devices = {
                        { name = "Synaptics", natural_scroll = false },
                    },
                },
            }
            "#,
        ) else {
            return;
        };
        let config = input_config(scripts.startup()).expect("`init.lua` calls `sol.input`");
        let _ = std::fs::remove_dir_all(&directory);

        let resolved = config.resolve(&DeviceInfo {
            id: "touchpad".into(),
            name: "Synaptics TouchPad".into(),
            kind: DeviceKind::Touchpad,
            vendor: None,
            product: None,
            output: None,
        });
        assert_eq!(
            resolved.natural_scroll,
            Some(false),
            "the override's own field"
        );
        assert_eq!(
            resolved.tap,
            Some(true),
            "the override named nothing about tap, so the touchpad default stands"
        );
    }

    #[test]
    fn an_absolute_devices_output_in_user_lua_reaches_the_input_settings() {
        let (directory, mut scripts) = shipped_init_with_user(
            "solium-script-test-input-output",
            r#"return { input = {
                touchscreen = { output = "eDP-1" },
                tablet_tool = { output = "DP-2" },
                devices = { { name = "External", output = "HDMI-A-1" } },
            } }"#,
        )
        .expect("shipped initialization");
        let config = input_config(scripts.startup()).expect("init calls sol.input");
        let mut panel = DeviceInfo {
            id: "panel".into(),
            name: "Panel".into(),
            kind: DeviceKind::Touchscreen,
            vendor: None,
            product: None,
            output: None,
        };
        assert_eq!(config.resolve(&panel).output.as_deref(), Some("eDP-1"));
        panel.kind = DeviceKind::TabletTool;
        assert_eq!(config.resolve(&panel).output.as_deref(), Some("DP-2"));
        panel.name = "External tablet".into();
        assert_eq!(config.resolve(&panel).output.as_deref(), Some("HDMI-A-1"));
        let _ = std::fs::remove_dir_all(directory);
    }

    /// **`input.touchscreen.velocity_window_ms` reaches the compositor, and
    /// a value that is not a whole number from 1 to 1000 is named and left
    /// unset (#284).**
    #[test]
    fn a_touchscreens_velocity_window_is_read_and_a_bad_one_is_named() {
        for (window, expected) in [
            ("40", Some(40)),
            ("1000", Some(1000)),
            ("0", None),
            ("1.5", None),
            ("2000", None),
            ("\"fast\"", None),
        ] {
            let log = logged_while(|| {
                let (directory, mut scripts) = shipped_init_with_user(
                    "solium-script-test-input-velocity-window",
                    &format!(
                        "return {{ input = {{ touchscreen = {{ velocity_window_ms = {window} }} }} }}"
                    ),
                )
                .expect("shipped initialization");
                let config = input_config(scripts.startup()).expect("init calls sol.input");
                assert_eq!(
                    config
                        .default_for(DeviceKind::Touchscreen)
                        .and_then(|settings| settings.velocity_window_ms),
                    expected,
                    "velocity_window_ms = {window}"
                );
                let _ = std::fs::remove_dir_all(directory);
            });
            assert_eq!(
                log.contains("input.velocity_window_ms"),
                expected.is_none(),
                "velocity_window_ms = {window}: {log}"
            );
        }
    }

    #[test]
    fn an_absolute_devices_output_of_the_wrong_type_is_logged_and_kept_unset() {
        let log = logged_while(|| {
            let (directory, mut scripts) = shipped_init_with_user(
                "solium-script-test-input-output-invalid",
                "return { input = { touchscreen = { output = 3 } } }",
            )
            .expect("shipped initialization");
            let config = input_config(scripts.startup()).expect("init calls sol.input");
            let panel = DeviceInfo {
                id: "panel".into(),
                name: "Panel".into(),
                kind: DeviceKind::Touchscreen,
                vendor: None,
                product: None,
                output: Some("eDP-1".into()),
            };
            let settings = config.resolve(&panel);
            assert_eq!(settings.output, None);
            assert_eq!(
                crate::input::devices::bound_output(&panel, &settings),
                Some("eDP-1"),
                "invalid configuration preserves the device binding"
            );
            let _ = std::fs::remove_dir_all(directory);
        });
        assert!(log.contains("input.output"), "{log}");
        assert!(log.contains("touchscreen"), "{log}");
    }

    /// **A value of the wrong shape for its field is named in the log and
    /// left unset, not fatal.** `accel_profile` only has two sensible
    /// spellings, and a third must not take the rest of `input` down with it
    /// -- the same policy `idle.dbus_inhibit` already has
    /// (`idle_dbus_inhibit_false_owns_nothing`).
    #[test]
    fn sol_input_warns_on_an_unrecognised_option_value_and_keeps_it_unset() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-input-bad-value",
            r#"sol.input({ mouse = { accel_profile = "fastest" } })"#,
        );
        let config = input_config(scripts.startup()).expect("the call ran");
        let _ = std::fs::remove_dir_all(&directory);

        let mouse = DeviceInfo {
            id: "mouse".into(),
            name: "Any Mouse".into(),
            kind: DeviceKind::Mouse,
            vendor: None,
            product: None,
            output: None,
        };
        assert_eq!(
            config.resolve(&mouse).accel_profile,
            None,
            "\"fastest\" is not \"flat\" or \"adaptive\"; it must not be guessed at"
        );
    }

    /// Every `Command::FocusMode` a run of commands carries, as `(click,
    /// follow, clear_on_empty_click)`, in order.
    fn focus_modes(commands: Vec<Command>) -> Vec<(Option<bool>, Option<bool>, Option<bool>)> {
        commands
            .into_iter()
            .filter_map(|command| match command {
                Command::FocusMode {
                    click,
                    follow,
                    clear_on_empty_click,
                    ..
                } => Some((click, follow, clear_on_empty_click)),
                _ => None,
            })
            .collect()
    }

    /// Every `Command::FocusMode`'s `after_close` a run of commands carries,
    /// in order.
    fn after_closes(commands: Vec<Command>) -> Vec<Option<Vec<crate::successor::Successor>>> {
        commands
            .into_iter()
            .filter_map(|command| match command {
                Command::FocusMode { after_close, .. } => Some(after_close),
                _ => None,
            })
            .collect()
    }

    /// **`after_close` is read word by word** (#128): a word, a list in
    /// order, a word it does not know left out with the rest kept, an empty
    /// list kept empty -- the keyboard left on nothing -- and `nil` or
    /// anything else read as `None`, the compositor's own rule.
    #[test]
    fn sol_focus_mode_reads_after_close_word_by_word() {
        use crate::successor::Successor::{Neighbour, Pointer, Recent, Topmost};
        let directory = std::env::temp_dir().join("solium-script-test-after-close-words");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.focus_mode({ after_close = { "recent", "neighbor", "pointer", 7 } })
            sol.focus_mode({ after_close = "topmost" })
            sol.focus_mode({ after_close = { "neighbour" } })
            sol.focus_mode({ after_close = {} })
            sol.focus_mode({})
            sol.focus_mode({ after_close = 42 })
            "#,
        )
        .expect("writing the test script");
        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let words = after_closes(scripts.startup().commands);
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            words,
            [
                Some(vec![Recent, Pointer]),
                Some(vec![Topmost]),
                Some(vec![Neighbour]),
                Some(vec![]),
                None,
                None,
            ]
        );
    }

    /// **Each shipped mode hands over its own focus model** (#128): floating
    /// click-only, tiling and scrolling following the pointer, each with its
    /// own `focus.after_close` -- floating's at load, tiling's and
    /// scrolling's as each is switched to. A shipped `"follow"` is handed
    /// over as `None`, the machine's own answer, so a tablet or a phone,
    /// with no pointer to hover, keeps its own (#159);
    /// `on_a_tablet_the_shipped_layouts_leave_hover_to_the_machine` in
    /// `state/tests.rs` is that half.
    #[test]
    fn the_shipped_modes_each_hand_over_their_own_focus_model() {
        use crate::successor::Successor::{Neighbour, Pointer, Recent, Topmost};
        let Some((directory, mut scripts)) =
            shipped_init_with_user("solium-script-test-after-close-shipped", "return {}")
        else {
            return;
        };
        let commands = [
            scripts.startup().commands,
            scripts.key("super+t", empty_snapshot()).commands,
            scripts.key("super+s", empty_snapshot()).commands,
        ];
        let _ = std::fs::remove_dir_all(&directory);
        let [loaded, tiling, scrolling] =
            commands.map(|commands| (focus_modes(commands.clone()), after_closes(commands)));
        assert_eq!(
            loaded,
            (
                vec![(None, Some(false), Some(true))],
                vec![Some(vec![Recent, Topmost])]
            ),
            "floating, at load"
        );
        let layout = (
            vec![(None, None, Some(true))],
            vec![Some(vec![Pointer, Neighbour, Topmost])],
        );
        assert_eq!(tiling, layout, "tiling");
        assert_eq!(scrolling, layout, "scrolling");
    }

    /// **Whether the pointer focuses what it moves over is your entry for
    /// the mode, then your `focus.follow`, then the shipped entry, then the
    /// machine's own answer** (#128 review, the ranking of 2026-10-14). So
    /// `focus = { follow = false }` never follows, in any mode, unless you
    /// also name a mode; merged over the shipped table, the shipped
    /// `tiling = "follow"` used to win over it. Each mode is switched to in
    /// turn, `custom` -- a mode no table names -- by a binding of the
    /// user's own, and the `follow` each hands over is read; `None` is the
    /// machine's own answer, which a shipped `"follow"` defers to.
    /// An entry of yours that is neither word counts as none.
    /// Against the ranking before it -- any entry in `focus.modes`, then
    /// `focus.follow` -- it fails at `{ follow = false }`: tiling's shipped
    /// `"follow"` won. With a shipped `"follow"` read as yours it fails at
    /// the row whose `tiling` is no word, which falls through to the shipped
    /// entry: tiling handed over `true`, which would override a tablet's own
    /// answer.
    #[test]
    fn your_mode_entry_then_your_focus_follow_then_the_shipped_entry_then_the_machine() {
        const CUSTOM: &str = r#"bindings = {
            ["super+alt+F12"] = function()
                local modes = require("modes")
                modes.register("custom", {})
                modes.use("custom")
            end,
        }"#;
        // Floating, tiling, scrolling and `custom`, in the order switched to.
        for (row, (focus, expected)) in [
            ("{ follow = false }", [Some(false); 4]),
            ("{ follow = true }", [Some(true); 4]),
            (
                r#"{ follow = false, modes = { tiling = "follow" } }"#,
                [Some(false), Some(true), Some(false), Some(false)],
            ),
            (
                r#"{ follow = true, modes = { floating = "click" } }"#,
                [Some(false), Some(true), Some(true), Some(true)],
            ),
            (
                r#"{ modes = { scrolling = "click", custom = "follow", tiling = "hover" } }"#,
                [Some(false), None, Some(false), Some(true)],
            ),
            ("{}", [Some(false), None, None, None]),
        ]
        .into_iter()
        .enumerate()
        {
            let Some((directory, mut scripts)) = shipped_init_with_user(
                &format!("solium-script-test-focus-ranking-{row}"),
                &format!("return {{ focus = {focus}, {CUSTOM} }}"),
            ) else {
                return;
            };
            let commands = [
                scripts.startup().commands,
                scripts.key("super+t", empty_snapshot()).commands,
                scripts.key("super+s", empty_snapshot()).commands,
                scripts.key("super+alt+F12", empty_snapshot()).commands,
            ];
            let _ = std::fs::remove_dir_all(&directory);
            let follows = commands.map(|commands| {
                let modes = focus_modes(commands);
                assert_eq!(modes.len(), 1, "focus = {focus}: one hand-over per mode");
                modes.first().and_then(|(_, follow, _)| *follow)
            });
            assert_eq!(
                follows, expected,
                "focus = {focus}: floating, tiling, scrolling and a mode no table names"
            );
        }
    }

    /// **A key `sol.focus_mode` is not given is left to the machine** (#219):
    /// unlike `sol.idle`, there is no fixed default for the script to fill
    /// in, because a mode calling it knows nothing about the machine's own
    /// form factor. So the command says `None`, and the compositor reads
    /// that as the form factor's answer (`Command::FocusMode`).
    #[test]
    fn sol_focus_mode_leaves_a_key_out_to_the_machine() {
        let directory = std::env::temp_dir().join("solium-script-test-focus-mode");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.focus_mode({ follow = false })
            "#,
        )
        .expect("writing the test script");
        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let modes = focus_modes(scripts.startup().commands);
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            modes,
            [(None, Some(false), None)],
            "a table naming only `follow` must leave `click` and \
             `clear_on_empty_click` as `None`, to the machine"
        );
    }

    /// **The shipped configuration's default mode, floating, focuses on
    /// click and not on hover** (#219): the desktop's windows are free to
    /// overlap, and one you were not aiming for sitting under the pointer on
    /// the way to the one you want should not steal the keyboard.
    #[test]
    fn the_shipped_configuration_defaults_floating_to_click_only_focus() {
        let Some((directory, mut scripts)) =
            shipped_init_with_user("solium-script-test-focus-mode-default", "return {}")
        else {
            return;
        };
        let modes = focus_modes(scripts.startup().commands);
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            modes,
            [(None, Some(false), Some(true))],
            "the shipped init.lua did not hand `config.focus` over for the \
             default (floating) mode as it says"
        );
    }

    /// **`config.focus.modes` overrides a mode's own default** from
    /// `user.lua`: the spelling that exists works. `nearest` does *not*
    /// catch a typo here (`focus.modes.folating` merges in silently) --
    /// `open_sections["focus.modes"] = true` accepts any name, the same as
    /// `bindings`, because mode names are open-ended too.
    #[test]
    fn a_user_lua_can_turn_floatings_focus_back_to_follow() {
        let Some((directory, mut scripts)) = shipped_init_with_user(
            "solium-script-test-focus-mode-user",
            r#"return { focus = { modes = { floating = "follow" } } }"#,
        ) else {
            return;
        };
        let modes = focus_modes(scripts.startup().commands);
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            modes,
            [(None, Some(true), Some(true))],
            "a user.lua naming `focus.modes.floating` did not reach `sol.focus_mode`"
        );
    }

    /// **A reload keeps the focus model the session was already in** (#219),
    /// the same defect #116 found in `workspaces.lua` and `modes.lua` fixed
    /// for the mode itself: `modes.lua` runs fresh on every load, so without
    /// `sol.keep` this file could not tell a reload from a fresh start, and
    /// `super+shift+r` after switching to tiling would answer `follow =
    /// false` -- floating's click-only override -- instead of tiling's own
    /// `follow`, every time. Since #128 `config.focus.modes` names tiling's
    /// `"follow"`, which as shipped is the machine's own answer: `None`.
    #[test]
    fn a_reload_keeps_the_focus_model_tiling_was_switched_to() {
        let Some((directory, mut scripts)) =
            shipped_init_with_user("solium-script-test-focus-mode-reload", "return {}")
        else {
            return;
        };
        let _ = scripts.startup();
        let switched = scripts.key("super+t", empty_snapshot());
        assert!(
            switched.handled,
            "super+t: tiling's binding was not reached"
        );
        assert_eq!(
            focus_modes(switched.commands),
            [(None, None, Some(true))],
            "switching to tiling did not hand over its own `follow`"
        );

        let config = directory.join("init.lua");
        let mut reloaded =
            Scripts::load_carrying(&config, scripts.kept()).expect("reloading the test scripts");
        let modes = focus_modes(reloaded.startup().commands);
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            modes,
            [(None, None, Some(true))],
            "a reload forgot tiling was in charge and fell back to floating's click-only \
             override (`follow = false`)"
        );
    }

    /// `sol.qml` is answered straight after the configuration is read, before
    /// `startup` hands anything it asked for to the compositor.
    #[test]
    fn a_configured_qml_renderer_is_read_before_qt_starts() {
        let directory = std::env::temp_dir().join("solium-script-test-qml");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        let configured = |source: &str| {
            std::fs::write(&config, source).expect("writing the test script");
            Scripts::load(&config)
                .expect("loading the test script")
                .qml()
        };

        assert_eq!(
            configured(r#"sol.qml({ renderer = "software", probe_timeout = 1500 })"#),
            crate::qml::renderer::Configured {
                renderer: Some("software".to_owned()),
                probe_timeout: Some(Duration::from_millis(1500)),
                buffers: None,
            }
        );
        // `qml.buffers`, each key on its own, the other left at its default.
        let buffers = |step, shrink| crate::qml::buffers::Buffers::new(step, shrink);
        for (source, wanted) in [
            (
                "sol.qml({ buffers = { step = 256, shrink = 3 } })",
                buffers(256, 3),
            ),
            (
                "sol.qml({ buffers = { step = 0 } })",
                buffers(0, crate::qml::buffers::SHRINK),
            ),
            (
                "sol.qml({ buffers = { shrink = 5 } })",
                buffers(crate::qml::buffers::STEP, 5),
            ),
            // Out of bounds, or not whole numbers: warned about, and the
            // defaults kept.
            ("sol.qml({ buffers = { step = 2000 } })", None),
            ("sol.qml({ buffers = { shrink = 1 } })", None),
            ("sol.qml({ buffers = { step = 64.5 } })", None),
            ("sol.qml({ buffers = 128 })", None),
        ] {
            assert_eq!(configured(source).buffers, wanted, "{source}");
        }
        // Said nothing, three ways.
        for source in ["", "sol.qml(nil)", "sol.qml({})"] {
            assert_eq!(
                configured(source),
                crate::qml::renderer::Configured::default(),
                "{source:?}"
            );
        }
        // Not a string and not a timeout: warned about, and left unsaid.
        assert_eq!(
            configured(r#"sol.qml({ renderer = true, probe_timeout = 0 })"#),
            crate::qml::renderer::Configured::default()
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_configured_session_is_read_by_the_backend() {
        let directory = std::env::temp_dir().join("solium-script-test-session");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        let configured = |source: &str| {
            std::fs::write(&config, source).expect("writing the test script");
            Scripts::load(&config)
                .expect("loading the test script")
                .session()
        };
        let settings = |systemd, autostart| crate::session::Settings {
            systemd,
            autostart,
            ..crate::session::Settings::default()
        };

        assert_eq!(
            configured("sol.session({ systemd = false })"),
            settings(false, true)
        );
        assert_eq!(
            configured("sol.session({ autostart = false })"),
            settings(true, false)
        );
        assert_eq!(
            configured("sol.session({ stop_timeout = 1500 })"),
            crate::session::Settings {
                stop_timeout: Duration::from_millis(1500),
                ..settings(true, true)
            }
        );
        // Said nothing, three ways, and said something of the wrong kind.
        for source in [
            "",
            "sol.session(nil)",
            "sol.session({})",
            r#"sol.session({ systemd = "no", autostart = 0 })"#,
            "sol.session({ stop_timeout = 0 })",
            "sol.session({ stop_timeout = -5 })",
            "sol.session({ stop_timeout = 2.5 })",
            r#"sol.session({ stop_timeout = "soon" })"#,
        ] {
            assert_eq!(configured(source), settings(true, true), "{source:?}");
        }

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// `sol.lock`'s own parsing: a string `command`, a boolean
    /// `before_sleep` and `logind_unlock`, an absent table keeping the
    /// defaults, and a value of
    /// the wrong kind named in the log with the default kept. See
    /// `crate::logind::Settings`.
    #[test]
    fn sol_lock_parses_command_and_before_sleep() {
        let directory = std::env::temp_dir().join("solium-script-test-lock");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        let configured = |source: &str| {
            std::fs::write(&config, source).expect("writing the test script");
            let mut scripts = Scripts::load(&config).expect("loading the test script");
            scripts
                .startup()
                .commands
                .into_iter()
                .find_map(|command| match command {
                    Command::Lock(settings) => Some(settings),
                    _ => None,
                })
                .expect("one Command::Lock")
        };

        assert_eq!(
            configured(r#"sol.lock({ command = "swaylock -f" })"#),
            crate::logind::Settings {
                command: Some("swaylock -f".to_owned()),
                before_sleep: true,
                logind_unlock: true,
            }
        );
        assert_eq!(
            configured("sol.lock({ before_sleep = false })"),
            crate::logind::Settings {
                command: None,
                before_sleep: false,
                logind_unlock: true,
            }
        );
        assert_eq!(
            configured("sol.lock({ logind_unlock = false })"),
            crate::logind::Settings {
                command: None,
                before_sleep: true,
                logind_unlock: false,
            }
        );
        // Said nothing, two ways, and said something of the wrong kind.
        for source in [
            "sol.lock()",
            "sol.lock({})",
            "sol.lock({ command = 5, before_sleep = \"no\", logind_unlock = 1 })",
        ] {
            assert_eq!(
                configured(source),
                crate::logind::Settings::default(),
                "{source:?}"
            );
        }

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// `sol.lock`'s settings for Solium's own lock (#281): every one read,
    /// one alone keeping the others' defaults, a `max_seconds` under its
    /// `seconds` taken as `seconds`, and a value of the wrong kind named in
    /// the log with the default kept. See `crate::lock_auth::Settings`.
    #[test]
    fn sol_lock_parses_the_native_locks_settings() {
        use crate::lock_auth::{Backoff, Keypad, Lockout, SecretLayout, Settings};
        let directory = std::env::temp_dir().join("solium-script-test-lock-auth");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        let configured = |source: &str| {
            std::fs::write(&config, source).expect("writing the test script");
            let mut scripts = Scripts::load(&config).expect("loading the test script");
            scripts
                .startup()
                .commands
                .into_iter()
                .find_map(|command| match command {
                    Command::LockAuth(settings) => Some(settings),
                    _ => None,
                })
                .expect("one Command::LockAuth")
        };

        assert_eq!(
            configured(
                r#"sol.lock({
                    lockout = { attempts = 5, seconds = 10, max_seconds = 40 },
                    secret_layout = "current",
                    helper_timeout_ms = 5000,
                    retry = { seconds = 1, max_seconds = 8, attempts = 3 },
                    keypad = "keyboard",
                })"#
            ),
            Settings {
                lockout: Lockout {
                    attempts: 5,
                    wait: Backoff {
                        seconds: 10,
                        max_seconds: 40,
                    },
                },
                secret_layout: SecretLayout::Current,
                helper_timeout: std::time::Duration::from_millis(5000),
                retry: Backoff {
                    seconds: 1,
                    max_seconds: 8,
                },
                retries: 3,
                keypad: Keypad::Keyboard,
                pin: crate::pin::Settings::default(),
            }
        );
        assert_eq!(
            configured("sol.lock({ lockout = { attempts = 4 }, keypad = \"letters\" })"),
            Settings {
                lockout: Lockout {
                    attempts: 4,
                    ..Lockout::default()
                },
                keypad: Keypad::Letters,
                ..Settings::default()
            }
        );
        assert_eq!(
            configured("sol.lock({ retry = { seconds = 50, max_seconds = 10 } })").retry,
            Backoff {
                seconds: 50,
                max_seconds: 50,
            }
        );
        // Said nothing, two ways, and said everything of the wrong kind.
        for source in [
            "sol.lock()",
            "sol.lock({})",
            r#"sol.lock({
                lockout = 3,
                secret_layout = "dvorak",
                helper_timeout_ms = 5,
                retry = { seconds = 0, max_seconds = "a minute", attempts = -1 },
                keypad = 7,
            })"#,
            "sol.lock({ lockout = { attempts = 0, seconds = 1.5 } })",
        ] {
            assert_eq!(configured(source), Settings::default(), "{source:?}");
        }

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// `lock.pin` (#281): every key read, one alone keeping the others'
    /// defaults, a `max_length` under `min_length` taken as `min_length`, and
    /// a value of the wrong kind or out of bounds (a PIN shorter than four
    /// digits, no attempts, no hours) named in the log with the default kept.
    /// See `crate::pin::Settings`.
    #[test]
    fn sol_lock_parses_the_pins_settings() {
        use crate::pin::Settings;
        use std::time::Duration;
        let directory = std::env::temp_dir().join("solium-script-test-lock-pin");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        let configured = |source: &str| {
            std::fs::write(&config, source).expect("writing the test script");
            let mut scripts = Scripts::load(&config).expect("loading the test script");
            scripts
                .startup()
                .commands
                .into_iter()
                .find_map(|command| match command {
                    Command::LockAuth(settings) => Some(settings.pin),
                    _ => None,
                })
                .expect("one Command::LockAuth")
        };
        assert_eq!(
            configured(
                r#"sol.lock({ pin = {
                    enabled = false, min_length = 6, max_length = 10,
                    attempts = 3, password_after_hours = 12,
                } })"#
            ),
            Settings {
                enabled: false,
                min_length: 6,
                max_length: 10,
                attempts: 3,
                password_after: Duration::from_secs(12 * 3600),
            }
        );
        assert_eq!(
            configured("sol.lock({ pin = { attempts = 9 } })"),
            Settings {
                attempts: 9,
                ..Settings::default()
            }
        );
        let longer = configured("sol.lock({ pin = { min_length = 9 } })");
        assert_eq!((longer.min_length, longer.max_length), (9, 9));
        // Past the caps, the caps: ten wrong PINs, three days.
        let loose =
            configured("sol.lock({ pin = { attempts = 50, password_after_hours = 1000 } })");
        assert_eq!(
            (loose.attempts, loose.password_after),
            (10, Duration::from_secs(72 * 3600))
        );
        let edge = configured("sol.lock({ pin = { attempts = 10, password_after_hours = 72 } })");
        assert_eq!(
            (edge.attempts, edge.password_after),
            (10, Duration::from_secs(72 * 3600))
        );
        for source in [
            "sol.lock({})",
            "sol.lock({ pin = true })",
            r#"sol.lock({ pin = {
                enabled = "yes", min_length = 3, max_length = 65,
                attempts = 0, password_after_hours = 0.5,
            } })"#,
            "sol.lock({ pin = { password_after_hours = -1, attempts = \"five\" } })",
        ] {
            assert_eq!(configured(source), Settings::default(), "{source:?}");
        }
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **The shipped configuration's native lock settings match
    /// `lock_auth::Settings`'s own default**, as its locker settings match
    /// `logind::Settings`'s below.
    #[test]
    fn the_shipped_configuration_s_native_lock_settings_match_the_rust_default() {
        let Some((directory, mut scripts)) =
            shipped_init_with_user("solium-script-test-lock-auth-default", "return {}")
        else {
            return;
        };
        let settings: Vec<crate::lock_auth::Settings> = scripts
            .startup()
            .commands
            .into_iter()
            .filter_map(|command| match command {
                Command::LockAuth(settings) => Some(settings),
                _ => None,
            })
            .collect();
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            settings,
            [crate::lock_auth::Settings::default()],
            "the shipped init.lua did not hand config.lock over as it says"
        );
    }

    /// `sol.lock`'s settings for what Solium's own lock shows (#281): every
    /// one read, `native` true by default and false by default once
    /// `command` names a locker, `enter_ms` and `exit_ms` held to their
    /// maximums, and a value of the wrong kind named in the log with the
    /// default kept. See `crate::lock_scene::Settings`.
    #[test]
    fn sol_lock_parses_what_the_native_lock_shows() {
        use crate::lock_scene::{ENTER_MAX_MS, EXIT_MAX_MS, PromptOn, Settings};
        use std::time::Duration;
        let directory = std::env::temp_dir().join("solium-script-test-lock-scene");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        let configured = |source: &str| {
            std::fs::write(&config, source).expect("writing the test script");
            let mut scripts = Scripts::load(&config).expect("loading the test script");
            scripts
                .startup()
                .commands
                .into_iter()
                .find_map(|command| match command {
                    Command::LockScene(settings) => Some(settings),
                    _ => None,
                })
                .expect("one Command::LockScene")
        };

        assert_eq!(
            configured(
                r#"sol.lock({
                    native = false,
                    scene = "~/my-lock.qml",
                    prompt_on = "DP-2",
                    enter_ms = 500,
                    exit_ms = 400,
                    keypad_margin = 0,
                })"#
            ),
            Settings {
                native: false,
                scene: "~/my-lock.qml".to_owned(),
                prompt_on: PromptOn::Monitor("DP-2".to_owned()),
                enter: Duration::from_millis(500),
                exit: Duration::from_millis(400),
                keypad_margin: 0,
            }
        );
        assert_eq!(
            configured(r#"sol.lock({ prompt_on = "primary", enter_ms = 0, exit_ms = 0 })"#),
            Settings {
                prompt_on: PromptOn::Primary,
                enter: Duration::ZERO,
                exit: Duration::ZERO,
                ..Settings::default()
            }
        );
        assert_eq!(
            configured("sol.lock({ exit_ms = 5000 })").exit,
            Duration::from_millis(u64::from(EXIT_MAX_MS)),
            "an exit past the maximum was not held to it"
        );
        assert_eq!(
            configured("sol.lock({ enter_ms = 5000 })").enter,
            Duration::from_millis(u64::from(ENTER_MAX_MS)),
            "an entry past the maximum was not held to it"
        );
        assert!(
            !configured(r#"sol.lock({ command = "swaylock -f" })"#).native,
            "a locker named, and Solium's own lock is still the default"
        );
        assert!(
            configured(r#"sol.lock({ command = "swaylock -f", native = true })"#).native,
            "`native = true` beside a locker was not taken"
        );
        // Said nothing, two ways, and said everything of the wrong kind.
        for source in [
            "sol.lock()",
            "sol.lock({})",
            r#"sol.lock({ native = "yes", scene = "", prompt_on = 3, enter_ms = -1, exit_ms = -5, keypad_margin = -1 })"#,
            "sol.lock({ scene = 7, enter_ms = \"slow\", exit_ms = 12.5, keypad_margin = \"wide\" })",
        ] {
            assert_eq!(configured(source), Settings::default(), "{source:?}");
        }

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **The shipped configuration's settings for what the native lock shows
    /// match `lock_scene::Settings`'s own default.**
    #[test]
    fn the_shipped_configuration_s_lock_scene_settings_match_the_rust_default() {
        let Some((directory, mut scripts)) =
            shipped_init_with_user("solium-script-test-lock-scene-default", "return {}")
        else {
            return;
        };
        let settings: Vec<crate::lock_scene::Settings> = scripts
            .startup()
            .commands
            .into_iter()
            .filter_map(|command| match command {
                Command::LockScene(settings) => Some(settings),
                _ => None,
            })
            .collect();
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            settings,
            [crate::lock_scene::Settings::default()],
            "the shipped init.lua did not hand config.lock over as it says"
        );
    }

    /// **The shipped configuration's `lock` table matches `logind::Settings`'s
    /// own default**, the same parity
    /// `the_shipped_configuration_turns_the_screens_off_after_ten_minutes`
    /// holds for `idle`: a future edit to `config.lua`'s `lock.before_sleep`
    /// or `command` without a matching change to `logind::Settings::default`,
    /// or the other way around, must fail a test rather than ship a mismatch.
    /// See `crate::logind::Settings`.
    #[test]
    fn the_shipped_configuration_s_lock_table_matches_the_rust_default() {
        let Some((directory, mut scripts)) =
            shipped_init_with_user("solium-script-test-lock-default", "return {}")
        else {
            return;
        };
        let lock: Vec<crate::logind::Settings> = scripts
            .startup()
            .commands
            .into_iter()
            .filter_map(|command| match command {
                Command::Lock(settings) => Some(settings),
                _ => None,
            })
            .collect();
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            lock,
            [crate::logind::Settings::default()],
            "the shipped init.lua did not hand config.lock over as it says"
        );
    }

    /// `sol.auto_reload`'s own parsing (#223): a boolean `automatic`, a
    /// non-negative `quiet_ms`, an absent table keeping both defaults, and a
    /// value of the wrong kind named in the log with the default kept. See
    /// `crate::autoreload::Settings`.
    #[test]
    fn sol_auto_reload_parses_automatic_and_quiet_ms() {
        let directory = std::env::temp_dir().join("solium-script-test-auto-reload");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        let configured = |source: &str| {
            std::fs::write(&config, source).expect("writing the test script");
            let mut scripts = Scripts::load(&config).expect("loading the test script");
            scripts
                .startup()
                .commands
                .into_iter()
                .find_map(|command| match command {
                    Command::AutoReload(settings) => Some(settings),
                    _ => None,
                })
                .expect("one Command::AutoReload")
        };

        assert_eq!(
            configured("sol.auto_reload({ automatic = false })"),
            crate::autoreload::Settings {
                automatic: false,
                ..crate::autoreload::Settings::default()
            }
        );
        assert_eq!(
            configured("sol.auto_reload({ quiet_ms = 50 })"),
            crate::autoreload::Settings {
                quiet_ms: 50,
                ..crate::autoreload::Settings::default()
            }
        );
        // Said nothing, two ways, and said something of the wrong kind.
        for source in [
            "sol.auto_reload()",
            "sol.auto_reload({})",
            r#"sol.auto_reload({ automatic = "yes", quiet_ms = "soon" })"#,
        ] {
            assert_eq!(
                configured(source),
                crate::autoreload::Settings::default(),
                "{source:?}"
            );
        }

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// `sol.x11`'s own parsing (#221): a table of strings for `hidden`, an
    /// absent table or key keeping the default, and a value of the wrong
    /// kind named in the log with the default kept, the same contract
    /// `sol_lock_parses_command_and_before_sleep` checks for `sol.lock`.
    #[test]
    fn sol_x11_parses_hidden_as_a_list_of_strings() {
        let directory = std::env::temp_dir().join("solium-script-test-x11");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        let configured = |source: &str| {
            std::fs::write(&config, source).expect("writing the test script");
            let mut scripts = Scripts::load(&config).expect("loading the test script");
            scripts
                .startup()
                .commands
                .into_iter()
                .find_map(|command| match command {
                    Command::X11(settings) => Some(settings),
                    _ => None,
                })
                .expect("one Command::X11")
        };

        assert_eq!(
            configured(r#"sol.x11({ hidden = { "my-tray-helper", "another-one" } })"#),
            crate::xwayland::Settings {
                hidden: vec!["my-tray-helper".to_owned(), "another-one".to_owned()],
            },
            "a configured list replaces the default rather than adding to it"
        );
        assert_eq!(
            configured(r#"sol.x11({ hidden = {} })"#),
            crate::xwayland::Settings { hidden: vec![] },
            "an empty list is a real answer: hide nothing at all"
        );
        // Said nothing, three ways, and said something of the wrong kind.
        for source in [
            "sol.x11()",
            "sol.x11({})",
            r#"sol.x11({ hidden = "xwaylandvideobridge" })"#,
            "sol.x11({ hidden = { 5, 6 } })",
        ] {
            assert_eq!(
                configured(source),
                crate::xwayland::Settings::default(),
                "{source:?}"
            );
        }

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **The shipped configuration's `reload` table matches
    /// `autoreload::Settings`'s own default**, the same parity
    /// `the_shipped_configuration_s_lock_table_matches_the_rust_default` holds
    /// for `lock`: a future edit to `config.lua`'s `reload.automatic` or
    /// `reload.quiet_ms` without a matching change to
    /// `autoreload::Settings::default`, or the other way around, must fail a
    /// test rather than ship a mismatch.
    #[test]
    fn the_shipped_configuration_s_reload_table_matches_the_rust_default() {
        let Some((directory, mut scripts)) =
            shipped_init_with_user("solium-script-test-auto-reload-default", "return {}")
        else {
            return;
        };
        let reload: Vec<crate::autoreload::Settings> = scripts
            .startup()
            .commands
            .into_iter()
            .filter_map(|command| match command {
                Command::AutoReload(settings) => Some(settings),
                _ => None,
            })
            .collect();
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            reload,
            [crate::autoreload::Settings::default()],
            "the shipped init.lua did not hand config.reload over as it says"
        );
    }

    /// **The shipped configuration's `x11.hidden` matches
    /// `xwayland::Settings`'s own default** (#221), the same parity
    /// `the_shipped_configuration_s_lock_table_matches_the_rust_default`
    /// holds for `lock`: a future edit to one without the other must fail a
    /// test rather than ship a mismatch.
    #[test]
    fn sol_output_management_is_a_flag() {
        let directory = std::env::temp_dir().join("solium-script-test-output-management");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        let offered = |source: &str| {
            std::fs::write(&config, source).expect("writing the test script");
            let mut scripts = Scripts::load(&config).expect("loading the test script");
            scripts
                .startup()
                .commands
                .into_iter()
                .find_map(|command| match command {
                    Command::OutputManagement(offered) => Some(offered),
                    _ => None,
                })
        };
        assert_eq!(offered("sol.output_management(false)"), Some(false));
        assert_eq!(offered("sol.output_management(true)"), Some(true));
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **The shipped configuration answers `monitors.apply` with
    /// `sol.monitors`** (#51): a program's rows become the arrangement, mode,
    /// rotation, scale and place, with `primary` kept from
    /// `config.monitors`, and the protocol is offered.
    #[test]
    fn the_shipped_configuration_answers_monitors_apply_with_sol_monitors() {
        use crate::json::Json;
        let Some((directory, mut scripts)) = shipped_init_with_user(
            "solium-script-test-monitors-apply",
            r#"return { monitors = { { name = "DP-1", primary = true } } }"#,
        ) else {
            return;
        };
        let startup = scripts.startup();
        assert!(
            startup
                .commands
                .iter()
                .any(|command| matches!(command, Command::OutputManagement(true))),
            "the shipped init.lua did not offer it"
        );
        let object = |pairs: Vec<(&str, Json)>| {
            Json::Object(
                pairs
                    .into_iter()
                    .map(|(key, value)| (key.to_owned(), value))
                    .collect(),
            )
        };
        let row = object(vec![
            ("name", Json::Text("DP-1".to_owned())),
            ("enabled", Json::Bool(true)),
            ("x", Json::Number(1920.0)),
            ("y", Json::Number(0.0)),
            (
                "mode",
                object(vec![
                    ("w", Json::Number(1280.0)),
                    ("h", Json::Number(720.0)),
                    ("refresh", Json::Number(60.0)),
                ]),
            ),
            ("scale", Json::Number(1.5)),
            ("transform", Json::Text("90".to_owned())),
        ]);
        let off = object(vec![
            ("name", Json::Text("DP-2".to_owned())),
            ("enabled", Json::Bool(false)),
        ]);
        let asked = object(vec![("monitors", Json::List(vec![row, off]))]);
        let outcome = scripts.surface_action(
            crate::output_management::SURFACE,
            "monitors.apply",
            &asked,
            Snapshot::default(),
        );
        let _ = std::fs::remove_dir_all(&directory);
        let arrangement = outcome
            .commands
            .into_iter()
            .find_map(|command| match command {
                Command::Monitors(arrangement) => Some(arrangement),
                _ => None,
            })
            .expect("monitors.apply was not answered with sol.monitors");
        assert_eq!(
            arrangement.lit_as(),
            [
                (
                    "DP-1".to_owned(),
                    true,
                    crate::monitor::Wanted::Exact {
                        width: 1280,
                        height: 720,
                        refresh: Some(60)
                    },
                    Some(smithay::utils::Transform::_90),
                    None
                ),
                (
                    "DP-2".to_owned(),
                    false,
                    crate::monitor::Wanted::Best,
                    None,
                    None
                )
            ]
        );
        assert_eq!(
            arrangement.scale("DP-1"),
            crate::monitor::Scaling::Fixed(1.5)
        );
        assert_eq!(arrangement.primary(), Some("DP-1"), "primary kept");
        let placed = arrangement.place(&[("DP-1".to_owned(), (720, 1280).into())]);
        assert_eq!(placed.at, [(1920, 0).into()]);
    }

    #[test]
    fn sol_workspaces_offered_is_a_flag() {
        let directory = std::env::temp_dir().join("solium-script-test-workspaces-offered");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        let offered = |source: &str| {
            std::fs::write(&config, source).expect("writing the test script");
            let mut scripts = Scripts::load(&config).expect("loading the test script");
            scripts
                .startup()
                .commands
                .into_iter()
                .find_map(|command| match command {
                    Command::WorkspacesOffered(offered) => Some(offered),
                    _ => None,
                })
        };
        assert_eq!(offered("sol.workspaces_offered(false)"), Some(false));
        assert_eq!(offered("sol.workspaces_offered(true)"), Some(true));
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn the_shipped_workspaces_offer_ext_workspace() {
        let Some((directory, mut scripts)) =
            shipped_init_with_user("solium-script-test-workspaces-offered-default", "return {}")
        else {
            return;
        };
        let offered: Vec<bool> = scripts
            .startup()
            .commands
            .into_iter()
            .filter_map(|command| match command {
                Command::WorkspacesOffered(offered) => Some(offered),
                _ => None,
            })
            .collect();
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            offered,
            [true],
            "the shipped workspaces.lua did not offer them"
        );
    }

    #[test]
    fn sol_toplevels_reads_each_protocol_as_a_flag() {
        use crate::toplevels::{Settings, WindowCapture};
        let directory = std::env::temp_dir().join("solium-script-test-toplevels");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        let configured = |source: &str| {
            std::fs::write(&config, source).expect("writing the test script");
            let mut scripts = Scripts::load(&config).expect("loading the test script");
            scripts
                .startup()
                .commands
                .into_iter()
                .find_map(|command| match command {
                    Command::Toplevels(settings) => Some(settings),
                    _ => None,
                })
                .expect("one Command::Toplevels")
        };
        assert_eq!(
            configured("sol.toplevels({ list = false, manage = true })"),
            Settings {
                list: false,
                manage: true,
                ..Settings::default()
            }
        );
        assert_eq!(
            configured("sol.toplevels({ manage = false })"),
            Settings {
                list: true,
                manage: false,
                ..Settings::default()
            },
            "one left out keeps its default"
        );
        for (written, capture) in [
            ("\"pane\"", WindowCapture::Pane),
            ("\"client\"", WindowCapture::Client),
            ("false", WindowCapture::Off),
            ("\"frame\"", WindowCapture::Client),
            ("true", WindowCapture::Client),
        ] {
            assert_eq!(
                configured(&format!("sol.toplevels({{ capture = {written} }})")).capture,
                capture,
                "capture = {written}"
            );
        }
        assert_eq!(
            configured(r#"sol.toplevels({ list = "no", manage = 0 })"#),
            Settings::default(),
            "a word or a number is no flag"
        );
        assert_eq!(configured("sol.toplevels()"), Settings::default());
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn the_shipped_configuration_s_toplevels_table_matches_the_rust_default() {
        let Some((directory, mut scripts)) =
            shipped_init_with_user("solium-script-test-toplevels-default", "return {}")
        else {
            return;
        };
        let toplevels: Vec<crate::toplevels::Settings> = scripts
            .startup()
            .commands
            .into_iter()
            .filter_map(|command| match command {
                Command::Toplevels(settings) => Some(settings),
                _ => None,
            })
            .collect();
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            toplevels,
            [crate::toplevels::Settings::default()],
            "the shipped init.lua did not hand config.toplevels over as it says"
        );
    }

    #[test]
    fn sol_frames_reads_the_policy_and_its_rules() {
        use crate::effect::rules::Glob;
        use crate::framing::{Policy, Rule, Settings};
        let directory = std::env::temp_dir().join("solium-script-test-frames");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        let configured = |source: &str| {
            std::fs::write(&config, source).expect("writing the test script");
            let mut scripts = Scripts::load(&config).expect("loading the test script");
            scripts
                .startup()
                .commands
                .into_iter()
                .find_map(|command| match command {
                    Command::Frames(settings) => Some(settings),
                    _ => None,
                })
                .expect("one Command::Frames")
        };

        assert_eq!(
            configured(
                r#"sol.frames({ policy = "force", rules = {
                    { match = { app_id = "org.gnome.*" }, policy = "prefer" },
                    { match = { app_id = "firefox" }, policy = "allow" },
                } })"#
            ),
            Settings {
                policy: Policy::Force,
                rules: vec![
                    Rule {
                        app_id: Glob::parse("org.gnome.*"),
                        policy: Policy::Prefer,
                    },
                    Rule {
                        app_id: Glob::parse("firefox"),
                        policy: Policy::Allow,
                    },
                ],
            }
        );
        assert_eq!(
            configured(r#"sol.frames({ policy = "allow" })"#),
            Settings {
                policy: Policy::Allow,
                rules: Vec::new(),
            }
        );
        for broken in [
            r#"sol.frames({ policy = "server" })"#,
            "sol.frames({ policy = 1 })",
            r#"sol.frames({ rules = "force" })"#,
            r#"sol.frames({ rules = { { policy = "force" } } })"#,
            r#"sol.frames({ rules = { { match = { app_id = "x" } } } })"#,
            r#"sol.frames({ rules = { { match = { app_id = 5 }, policy = "force" } } })"#,
            r#"sol.frames({ policy = "force", rules = { { match = { app_id = "x" }, policy = "both" } } })"#,
        ] {
            assert_eq!(
                configured(broken),
                Settings::default(),
                "{broken} was not refused whole"
            );
        }
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn the_shipped_configuration_s_frames_table_matches_the_rust_default() {
        let Some((directory, mut scripts)) =
            shipped_init_with_user("solium-script-test-frames-default", "return {}")
        else {
            return;
        };
        let frames: Vec<crate::framing::Settings> = scripts
            .startup()
            .commands
            .into_iter()
            .filter_map(|command| match command {
                Command::Frames(settings) => Some(settings),
                _ => None,
            })
            .collect();
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            frames,
            [crate::framing::Settings::default()],
            "the shipped init.lua did not hand config.frames over as it says"
        );
    }

    #[test]
    fn sol_layer_shell_reads_input_below_as_a_flag_or_a_list_of_namespaces() {
        use crate::effect::rules::Glob;
        use crate::layer::{InputBelow, Settings};
        let directory = std::env::temp_dir().join("solium-script-test-layer-shell");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        let configured = |source: &str| {
            std::fs::write(&config, source).expect("writing the test script");
            let mut scripts = Scripts::load(&config).expect("loading the test script");
            scripts
                .startup()
                .commands
                .into_iter()
                .find_map(|command| match command {
                    Command::LayerShell(settings) => Some(settings),
                    _ => None,
                })
                .expect("one Command::LayerShell")
        };
        let below = |input_below| Settings { input_below };

        assert_eq!(
            configured("sol.layer_shell({ input_below = true })"),
            below(InputBelow::Every)
        );
        assert_eq!(
            configured("sol.layer_shell({ input_below = false })"),
            below(InputBelow::Nothing)
        );
        assert_eq!(
            configured(r#"sol.layer_shell({ input_below = { "desktop", "dock*", "*" } })"#),
            below(InputBelow::Named(vec![
                Glob::parse("desktop"),
                Glob::parse("dock*"),
                Glob::Any,
            ])),
            "each name is a namespace, a prefix or every one"
        );
        assert_eq!(
            configured("sol.layer_shell({ input_below = {} })"),
            below(InputBelow::Named(Vec::new())),
            "an empty list names none"
        );
        for broken in [
            "sol.layer_shell({ input_below = 5 })",
            r#"sol.layer_shell({ input_below = { "desktop", 5 } })"#,
            "sol.layer_shell({})",
            "sol.layer_shell()",
        ] {
            assert_eq!(
                configured(broken),
                Settings::default(),
                "{broken} kept the default"
            );
        }
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn the_shipped_configuration_s_layer_shell_table_matches_the_rust_default() {
        let Some((directory, mut scripts)) =
            shipped_init_with_user("solium-script-test-layer-shell-default", "return {}")
        else {
            return;
        };
        let layer_shell: Vec<crate::layer::Settings> = scripts
            .startup()
            .commands
            .into_iter()
            .filter_map(|command| match command {
                Command::LayerShell(settings) => Some(settings),
                _ => None,
            })
            .collect();
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            layer_shell,
            [crate::layer::Settings::default()],
            "the shipped init.lua did not hand config.layer_shell over as it says"
        );
    }

    #[test]
    fn the_shipped_configuration_s_x11_table_matches_the_rust_default() {
        let Some((directory, mut scripts)) =
            shipped_init_with_user("solium-script-test-x11-default", "return {}")
        else {
            return;
        };
        let x11: Vec<crate::xwayland::Settings> = scripts
            .startup()
            .commands
            .into_iter()
            .filter_map(|command| match command {
                Command::X11(settings) => Some(settings),
                _ => None,
            })
            .collect();
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            x11,
            [crate::xwayland::Settings::default()],
            "the shipped init.lua did not hand config.x11 over as it says"
        );
    }

    /// **The shipped configuration runs a moving window's effects on every
    /// pass** (`effects.motion_hz = 0`, 2026-10-16), and that is the Rust
    /// default too: the glass is read where the window is on every frame of
    /// a glide, a drag or a resize, and a lower rate is a choice made in
    /// `config.lua` for a weak GPU. The same parity
    /// `the_shipped_configuration_s_lock_table_matches_the_rust_default`
    /// holds for `lock`.
    #[test]
    fn the_shipped_configuration_runs_a_moving_windows_effects_on_every_pass() {
        let Some((directory, scripts)) =
            shipped_init_with_user("solium-script-test-motion-hz-default", "return {}")
        else {
            return;
        };
        let settings = scripts.effect_settings_at_load();
        let _ = std::fs::remove_dir_all(&directory);
        let Some(Ok(settings)) = settings else {
            panic!("the shipped init.lua handed over no effects settings: {settings:?}");
        };
        let default = crate::effect::settings::Settings::default().motion_hz;
        assert!(
            settings.motion_hz.abs() < f64::EPSILON && default.abs() < f64::EPSILON,
            "the shipped effects.motion_hz is {} and the Rust default {default}, not both 0",
            settings.motion_hz
        );
    }

    /// The shipped `config.lua` and `init.lua` tell the session and start
    /// autostart, and `--check` knows both keys. Shipped files only: the
    /// entry points `package.path` at them, past the developer's own
    /// `~/.config/solium`.
    #[test]
    fn the_shipped_configuration_tells_the_session_and_starts_autostart() {
        let shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua");
        let directory = std::env::temp_dir().join("solium-script-test-session-shipped");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let entry = directory.join("init.lua");
        let load = |source: String| {
            std::fs::write(&entry, source).expect("writing the entry point");
            Scripts::load(&entry).expect("loading the shipped scripts")
        };

        let scripts = load(format!(
            "package.path = {shipped:?} .. \"/?.lua\"\ndofile({shipped:?} .. \"/init.lua\")\n"
        ));
        assert_eq!(scripts.session(), crate::session::Settings::default());

        std::fs::write(
            directory.join("user.lua"),
            "return { session = { systemd = false, autostart = false, stop_timeout = 900, \
             autostrat = true } }",
        )
        .expect("writing the user file");
        let scripts = load(format!(
            "package.path = {here:?} .. \"/?.lua;\" .. {shipped:?} .. \"/?.lua\"\n\
             sol.session(require(\"config\").session)\n",
            here = directory.to_string_lossy(),
        ));
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            scripts.session(),
            crate::session::Settings {
                systemd: false,
                autostart: false,
                stop_timeout: Duration::from_millis(900),
            }
        );
        let unknown: Vec<String> = scripts
            .unknown_settings()
            .into_iter()
            .map(|setting| setting.key)
            .collect();
        assert_eq!(unknown, vec!["session.autostrat".to_owned()]);
    }

    /// The shipped `config.lua` asks for `auto` and a five second probe, through
    /// the shipped `init.lua`'s own call.
    #[test]
    fn the_shipped_configuration_asks_for_auto() {
        let Some(own) = Scripts::user_config_dir() else {
            return;
        };
        if own.join("user.lua").exists() || own.join("config.lua").exists() {
            return;
        }
        let directory = std::env::temp_dir().join("solium-script-test-qml-default");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            local config = require("config")
            sol.qml(config.qml)
            "#,
        )
        .expect("writing the test script");

        let scripts = Scripts::load(&config).expect("loading the test script");
        assert_eq!(
            scripts.qml(),
            crate::qml::renderer::Configured {
                renderer: Some("auto".to_owned()),
                probe_timeout: Some(crate::qml::renderer::PROBE_TIMEOUT),
                buffers: Some(crate::qml::buffers::Buffers::default()),
            }
        );
        let shipped = std::fs::read_to_string(crate::assets::lua().join("init.lua"))
            .expect("the shipped init.lua");
        assert!(
            shipped.contains("sol.qml(config.qml)"),
            "the shipped init.lua does not hand qml to the compositor"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **The shipped `config.lua` says nothing about the cursor, and that is
    /// the setting.**
    ///
    /// The one property of this configuration surface that a plausible-looking
    /// edit would destroy without failing anything else. `cursor = {}` is not
    /// an empty placeholder waiting to be filled in with sensible defaults:
    /// writing `cursor = { size = 24 }` there would make the configuration
    /// *always* have an opinion, `XCURSOR_SIZE` would never be reached, and
    /// the precedence tests in `cursor::theme` would all still pass because
    /// they never read this file.
    ///
    /// Runs the shipped `config.lua` rather than a copy, the same way the
    /// old-key test above does and with the same guard on a real
    /// `~/.config/solium`, which would otherwise answer first.
    #[test]
    fn the_shipped_configuration_leaves_the_cursor_to_the_environment() {
        let Some(own) = Scripts::user_config_dir() else {
            return;
        };
        if own.join("user.lua").exists() || own.join("config.lua").exists() {
            return;
        }

        let directory = std::env::temp_dir().join("solium-script-test-cursor-default");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            local config = require("config")
            sol.bind("Super+C", function()
                sol.cursor_theme(config.cursor)
            end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let outcome = scripts.key("super+c", empty_snapshot());
        assert!(outcome.handled);
        match outcome.commands.as_slice() {
            [Command::Cursor(configured)] => assert_eq!(
                configured,
                &crate::cursor::theme::Configured::default(),
                "the shipped config.lua names a cursor theme or size, so XCURSOR_THEME and \
                 XCURSOR_SIZE can never win"
            ),
            other => panic!("expected one cursor command, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A configuration with no `cursor` key at all still loads.**
    ///
    /// The documented way to change one setting is a single
    /// `~/.config/solium/config.lua`, and shipped `init.lua` calls
    /// `sol.cursor_theme(config.cursor)` against it. A copy written before this
    /// setting existed — which is every copy made before #81 — has no `cursor`
    /// key, so that call is `sol.cursor_theme(nil)`. Taking a `Table` made that
    /// an error at load, and an error at load is not a pointer that falls back:
    /// it is *no configuration*, no layouts, no bindings and no decorations,
    /// over a setting the user never heard of.
    ///
    /// The same class of break as `two_functions_cannot_share_one_name` below,
    /// and the same shape of guard. `sol.keyboard` and `sol.monitors` take an
    /// optional table for exactly this reason.
    ///
    /// Asserted at load *and* in a binding, because those are two different
    /// call sites with two different consequences: the one in `init.lua` runs
    /// while the configuration is being built, and the one under a key runs
    /// after.
    #[test]
    fn a_configuration_that_says_nothing_about_the_cursor_still_loads() {
        let directory = std::env::temp_dir().join("solium-script-test-cursor-nil");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            -- A user's own config.lua, written before the cursor setting
            -- existed: no `cursor` key, so this is sol.cursor_theme(nil).
            local config = { pane = {} }
            sol.cursor_theme(config.cursor)
            sol.bind("Super+C", function()
                sol.cursor_theme(config.cursor)
                sol.status("alive")
            end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config)
            .expect("a configuration with no cursor key failed to load at all");
        let outcome = scripts.key("super+c", empty_snapshot());
        assert!(outcome.handled);
        assert_eq!(outcome.status.as_deref(), Some("alive"));
        match outcome.commands.as_slice() {
            [Command::Cursor(configured)] => assert_eq!(
                configured,
                &crate::cursor::theme::Configured::default(),
                "no table is not 'reset it': it means the configuration did not say, so \
                 XCURSOR_THEME and XCURSOR_SIZE still get their turn"
            ),
            other => panic!("expected one cursor command, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// And a `size` that is not a whole number is ignored rather than fatal.
    ///
    /// `Settings::resolve` is the one place that decides what a size *out of
    /// range* means, and it is handed the number to decide about. A value that
    /// is not a number is a different thing: mlua answers an `Option<i32>` with
    /// a conversion *error* for `"big"` and for `24.5`, and a `?` on it would
    /// take the whole configuration down — the same failure as the missing
    /// table above, reached through a typo in one field. The `theme` key eight
    /// lines above has always read this way; this makes the pair agree.
    #[test]
    fn a_cursor_size_that_is_not_a_number_is_ignored_and_not_fatal() {
        let directory = std::env::temp_dir().join("solium-script-test-cursor-size");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("Super+C", function()
                sol.cursor_theme({ theme = "Fixture", size = "big" })
            end)
            sol.bind("Super+D", function()
                sol.cursor_theme({ theme = "Fixture", size = 24.5 })
            end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        for key in ["super+c", "super+d"] {
            let outcome = scripts.key(key, empty_snapshot());
            assert!(
                outcome.handled,
                "{key}: the listener failed, so the whole configuration went with it"
            );
            match outcome.commands.as_slice() {
                [Command::Cursor(configured)] => {
                    assert_eq!(
                        configured.theme.as_deref(),
                        Some("Fixture"),
                        "{key}: the rest of the table was lost with the bad size"
                    );
                    assert_eq!(
                        configured.size, None,
                        "{key}: a size that is not a whole number reached the compositor"
                    );
                }
                other => panic!("{key}: expected one cursor command, got {other:?}"),
            }
        }

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// `sol.cursor` is still where the pointer is, and setting the theme is a
    /// different name.
    ///
    /// **The regression this exists for shipped, and was invisible to the
    /// gate.** #81 registered the theme setter under `sol.cursor`, which was
    /// already the getter that answers with the pointer's position. `sol.set`
    /// replaces without complaining, so the getter simply stopped existing.
    /// What broke was `tiling.lua`, which asks where the pointer is on every
    /// window open to decide which pane the new window splits: every
    /// `sol.on("open")` failed with "error converting Lua nil to table", and
    /// no window opened in a tiled layout was placed. Nothing caught it — the
    /// compositor starts, `--check` passes, and each function's own test
    /// passes because each calls its own name. It took running the thing.
    ///
    /// So this asserts the pair, in one script, in one dispatch: the position
    /// comes back as the numbers the snapshot was built with, *and* the theme
    /// reaches the compositor as a command. Either name overwriting the other
    /// fails this, whichever way round it is done.
    #[test]
    fn two_functions_cannot_share_one_name() {
        let directory = std::env::temp_dir().join("solium-script-test-cursor-names");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("Super+C", function()
                local at = sol.cursor()
                sol.status(string.format("%d,%d", at.x, at.y))
                sol.cursor_theme({ theme = "Fixture" })
            end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let mut snapshot = empty_snapshot();
        snapshot.cursor = (12.0, 34.0);
        let outcome = scripts.key("super+c", snapshot);
        assert!(
            outcome.handled,
            "the listener failed, which is exactly how the collision showed up"
        );
        assert_eq!(
            outcome.status.as_deref(),
            Some("12,34"),
            "sol.cursor() did not answer with the pointer's position"
        );
        match outcome.commands.as_slice() {
            [Command::Cursor(configured)] => {
                assert_eq!(configured.theme.as_deref(), Some("Fixture"));
            }
            other => panic!("sol.cursor_theme() did not reach the compositor: {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **The three answers about a parent survive the trip into Lua.**
    ///
    /// The whole value of [`Parentage`] is that "no parent" and "a parent I
    /// cannot point at" stay apart, and the only place that can be lost is the
    /// one line that turns it into a Lua value. A script that has to centre a
    /// dialog asks `if window.parent then`, so both of the negative answers
    /// have to be falsey -- and a script that wants to tell them apart asks
    /// `== false`, so they have to be different values. Both halves are checked
    /// here, because getting `Unknown` wrong in the other direction -- `nil` --
    /// costs nothing today and quietly deletes the distinction.
    ///
    /// `modal` rides along: it is set on the same line and read by the same
    /// scripts, and a boolean that arrives as nil reads as false.
    #[test]
    fn a_parent_reaches_a_script_as_a_number_false_or_nothing() {
        let directory = std::env::temp_dir().join("solium-script-test-parentage");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("Super+P", function()
                for _, window in ipairs(sol.windows()) do
                    -- Reported as a place, because `sol.place` is the only way
                    -- out of a dispatch that carries four numbers.
                    sol.place(window.id, {
                        x = window.parent == nil and -1
                            or (window.parent == false and -2 or window.parent),
                        y = window.modal and 1 or 0,
                        w = 1,
                        h = 1,
                    })
                end
            end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let mut snapshot = empty_snapshot();
        let window = |id: u64, modal: bool, parent: Parentage| WindowInfo {
            icon: None,
            client_side: false,
            blur_region: false,
            insets: crate::decoration::Insets::default(),
            id,
            rect: Rect::default(),
            drawn: Drawn::default(),
            title: String::new(),
            focused: false,
            monitor: "test-1".to_owned(),
            modal,
            parent,
            leaving: false,
            minimized: false,
            above: false,
            app_id: String::new(),
            min: None,
            max: None,
            cramped: false,
            shown: true,
            fullscreen: false,
            maximized: false,
            style: String::new(),
            x11_type: None,
            accepts_input: true,
            class: None,
            instance: None,
            content_type: "none",
            pid: None,
        };
        snapshot.windows = vec![
            window(1, false, Parentage::None),
            window(2, true, Parentage::Unknown),
            window(3, true, Parentage::Window(1)),
        ];

        let outcome = scripts.key("super+p", snapshot);
        assert!(outcome.handled);
        let mut seen = Vec::new();
        for command in &outcome.commands {
            if let Command::Place { id, rect, .. } = command {
                seen.push((*id, rect.x, rect.y));
            }
        }
        assert_eq!(
            seen,
            vec![
                // No parent at all: absent from the table, so `nil`.
                (1, -1.0, 0.0),
                // A parent that was named and cannot be pointed at: `false`,
                // which is falsey for `if window.parent then` and still tells
                // a script that asks that something was named.
                (2, -2.0, 1.0),
                // A parent that can be pointed at: its id, as a number.
                (3, 1.0, 1.0),
            ],
            "the parent of a window did not survive the trip into Lua"
        );
    }

    /// **A configuration that fails to load names its file and line** as
    /// `<path>:<line>:`, the form every other Lua error in Solium takes, so
    /// a failed reload can be put on the overlay where it is
    /// (`effect::host::config_problem`). Named as a bare string, the chunk
    /// read `[string "<path>"]:2:`.
    #[test]
    fn a_configuration_error_names_its_file_and_line() {
        let directory =
            std::env::temp_dir().join(format!("solium-config-error-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(&config, "sol.pane('none')\nthis is not lua\n")
            .expect("writing the test script");
        let error = Scripts::load(&config)
            .err()
            .map(|err| format!("{err:#}"))
            .expect("a configuration that is not Lua fails to load");
        let _ = std::fs::remove_dir_all(&directory);
        assert!(
            error.contains(&format!("{}:2: ", config.display())),
            "{error}"
        );
    }

    /// **A broken effect reaches `sol.problems()` with its file and line.**
    #[test]
    fn a_broken_effect_reaches_sol_problems_with_its_file_and_line() {
        let directory =
            std::env::temp_dir().join(format!("solium-problems-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("Super+P", function()
                local row = sol.problems()[1]
                sol.status(string.format("%s %s %d %s", row.effect, row.file, row.line, row.message))
            end)
        "#,
        )
        .expect("writing the test script");
        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let mut snapshot = empty_snapshot();
        snapshot.problems = vec![super::ProblemRow {
            effect: "blur".to_owned(),
            file: "/x/blur/down.frag".to_owned(),
            line: Some(2),
            column: None,
            message: "`p_ofset` is not a param".to_owned(),
            severity: "error",
        }];
        let outcome = scripts.key("super+p", snapshot);
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            outcome.status.as_deref(),
            Some("blur /x/blur/down.frag 2 `p_ofset` is not a param")
        );
    }

    /// **The `problems` event fires once per change**, to every listener.
    #[test]
    fn the_problems_event_reaches_its_listeners() {
        let directory =
            std::env::temp_dir().join(format!("solium-problems-event-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            "local n = 0\nsol.on('problems', function() n = n + 1; sol.status(tostring(n)) end)\n",
        )
        .expect("writing");
        let mut scripts = Scripts::load(&config).expect("loading");
        let outcome = scripts.problems_changed(empty_snapshot());
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(outcome.status.as_deref(), Some("1"));
    }

    /// **The shipped configuration shows the problems on the primary
    /// monitor**, in its top-right corner as `file:line: message` rows, at
    /// most eight and then a count, taking no input; and takes the overlay
    /// away when none are left.
    #[test]
    fn the_shipped_overlay_lists_the_problems_on_the_primary_monitor() {
        let Some((directory, mut scripts)) =
            shipped_init_with_user("solium-problems-overlay", "return {}")
        else {
            return;
        };
        let mut snapshot = one_screen(&[]);
        snapshot.problems = (1..=10)
            .map(|line| super::ProblemRow {
                effect: "blur".to_owned(),
                file: "/x/blur/down.frag".to_owned(),
                line: Some(line),
                column: None,
                message: "`p_ofset` is not a param".to_owned(),
                severity: if line == 1 { "warning" } else { "error" },
            })
            .collect();
        let shown = scripts.problems_changed(snapshot).commands;
        let gone = scripts.problems_changed(one_screen(&[])).commands;
        let _ = std::fs::remove_dir_all(&directory);

        let declared: Vec<&crate::scripted::Declaration> = shown
            .iter()
            .filter_map(|command| match command {
                Command::Surface(declared) if declared.name == "problems" => Some(&**declared),
                _ => None,
            })
            .collect();
        let [overlay] = declared.as_slice() else {
            panic!("one overlay, not {}: {shown:?}", declared.len());
        };
        assert!(
            overlay.scene.ends_with("problems.qml"),
            "{:?}",
            overlay.scene
        );
        assert_eq!(overlay.layer, crate::scripted::Layer::Overlay);
        assert!(!overlay.interactive, "the overlay takes no input");
        assert_eq!(
            overlay.on,
            crate::scripted::On::Rect(smithay::utils::Rectangle::new(
                (840, 0).into(),
                (760, 240).into()
            )),
            "the top-right corner of the 1600-wide primary monitor"
        );
        let properties = overlay.properties.render();
        assert!(
            properties.contains(
                r#"{"severity":"warning","text":"/x/blur/down.frag:1: `p_ofset` is not a param"}"#
            ),
            "{properties}"
        );
        assert!(
            properties.contains(r#""text":"/x/blur/down.frag:8: "#),
            "{properties}"
        );
        assert!(
            !properties.contains(r#""text":"/x/blur/down.frag:9: "#),
            "{properties}"
        );
        assert!(
            properties.contains(r#""text":"and 2 more""#),
            "{properties}"
        );
        assert!(
            gone.iter()
                .any(|command| matches!(command, Command::SurfaceGone(name) if name == "problems")),
            "the overlay stays with nothing left to show: {gone:?}"
        );
    }

    /// A snapshot with nothing in it, for the tests that only want a call made.
    fn empty_snapshot() -> Snapshot {
        Snapshot {
            locked: false,
            notifications: Default::default(),
            audio: Default::default(),
            tray: Default::default(),
            media: Default::default(),
            keyboard: crate::keymap::State::initial(),
            shortcuts_owner: None,
            windows: Vec::new(),
            monitors: Vec::new(),
            work_area: Rect::default(),
            cursor: (0.0, 0.0),
            screens: Vec::new(),
            text_input: None,
            problems: Vec::new(),
            apps: std::rc::Rc::default(),
        }
    }

    /// **A script names an effect folder, and its params and anchor come
    /// through as it wrote them.**
    ///
    /// The join `script::shipped` cannot see: a deform's effect is a folder's
    /// name, resolved by the effect host where the command is applied, its
    /// params every key but the deform's own, and its anchor.
    ///
    /// The anchor is the half worth pinning. `{ window = 9 }` must survive as
    /// an *identity* all the way to the command, because the moment it becomes
    /// a rectangle here it is a rectangle measured when the key was pressed.
    #[test]
    fn a_script_names_an_effect_and_the_engine_answers() {
        use solium_effects::spec::Value as Param;
        let directory = std::env::temp_dir().join("solium-script-test-deform");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("super+1", function()
                sol.present(1, { deform = { effect = "genie", axis = "left",
                                            spread = 2.5,
                                            to = { x = 10, y = 20, w = 30, h = 40 } } })
                sol.present(2, { deform = { effect = "genie", axis = "auto", progress = 0.5,
                                            seed = 0.25, failed = "hide", on_reload = "keep",
                                            to = { window = 9 } } })
                sol.present(3, { deform = { effect = "nonsense", to = { window = 9 } } })
                sol.present(4, {})
                sol.present(5, { deform = { effect = "genie", tint = { 1, 0, 0, 1 }, on = true,
                                            to = { surface = "dock" } } })
            end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let outcome = scripts.key("super+1", Snapshot::default());
        let deforms: Vec<Option<DeformSpec>> = outcome
            .commands
            .iter()
            .map(|command| match command {
                Command::Present { deform, .. } => deform.clone(),
                other => panic!("expected a present command, got {other:?}"),
            })
            .collect();
        assert_eq!(deforms.len(), 5);

        let spec = |effect: &str, params: Vec<(String, Param)>, aim: Aim| DeformSpec {
            effect: effect.to_owned(),
            params,
            axis: None,
            // Not named, so all the way in, which is what one animates
            // towards.
            progress: 1.0,
            aim,
            seed: None,
            failed: None,
            on_reload: None,
        };
        assert_eq!(
            deforms[0],
            Some(DeformSpec {
                axis: Some("left".to_owned()),
                ..spec(
                    "genie",
                    vec![("spread".to_owned(), Param::Number(2.5))],
                    Aim::Rect(Rect {
                        x: 10.0,
                        y: 20.0,
                        w: 30.0,
                        h: 40.0
                    }),
                )
            })
        );
        assert_eq!(
            deforms[1],
            Some(DeformSpec {
                axis: Some("auto".to_owned()),
                progress: 0.5,
                seed: Some(0.25),
                failed: Some(crate::effect::settings::PresentFailed::Hide),
                on_reload: Some(crate::effect::settings::PresentReload::Keep),
                ..spec("genie", Vec::new(), Aim::Window(9))
            })
        );
        // A folder nobody has is the effect host's to say, where the command
        // is applied: here it is only a name.
        assert_eq!(
            deforms[2],
            Some(spec("nonsense", Vec::new(), Aim::Window(9)))
        );
        assert_eq!(deforms[3], None);
        // **A surface survives this far as a name.** It becomes a
        // `scripted::SurfaceId` in `Solium::aimed`, which is the first place
        // that can see whether there is a surface by that name -- and the id is
        // what keeps `present::Frame` `Copy`. Params of every kind, sorted.
        assert_eq!(
            deforms[4],
            Some(spec(
                "genie",
                vec![
                    ("on".to_owned(), Param::Bool(true)),
                    ("tint".to_owned(), Param::Vec4([1.0, 0.0, 0.0, 1.0])),
                ],
                Aim::Surface("dock".to_owned()),
            ))
        );
    }

    /// **A deform's own keys are refused outside their words**, each naming
    /// the key: `seed` in [0, 1), `failed`, `on_reload`, `axis`, and a param
    /// that is no param's value.
    #[test]
    fn a_deforms_own_keys_are_refused_outside_their_words() {
        let lua = Lua::new();
        let cases = [
            ("seed = 1", "`seed` is a number from 0 up to 1"),
            ("seed = -0.5", "`seed` is a number from 0 up to 1"),
            ("failed = 'fade'", "`failed` is \"flat\" or \"hide\""),
            (
                "on_reload = 'switch'",
                "`on_reload` is \"flat\" or \"keep\"",
            ),
            ("axis = 'sideways'", "`axis` is \"auto\" or \"down\""),
            ("progress = 0/0", "`progress` is a finite number"),
            ("spread = { 1, 2 }", "`spread` is not a param's value"),
        ];
        for (key, says) in cases {
            let options: Table = lua
                .load(format!(
                    "return {{ deform = {{ effect = 'genie', {key}, to = {{ window = 2 }} }} }}"
                ))
                .eval()
                .expect("the test's table");
            let error = deform_from(&options).expect_err(key).to_string();
            assert!(error.contains(says), "{key}: {error}");
        }
        let options: Table = lua
            .load("return { deform = { effect = 'genie', seed = 0, failed = 'flat', on_reload = 'flat', to = { window = 2 } } }")
            .eval()
            .expect("the test's table");
        assert!(deform_from(&options).is_ok_and(|deform| deform.is_some()));
    }

    /// **A script says how deep a window is drawn and what it turns about.**
    ///
    /// All of that pair's surface in one script: both keys read, a table
    /// mentioning neither producing exactly the frame every window has had
    /// until now, and each pivot axis defaulting on its own -- `pivot_x = 0`
    /// means the left edge and says nothing about the vertical, so a script
    /// naming one axis must not be given two.
    ///
    /// **The two pivot numbers are deliberately different, and neither is a
    /// default.** `(0.5, 0.5)` and `(0.0, 0.0)` are each their own transpose,
    /// so a fixture built from either cannot tell `pivot_x` read into the
    /// wrong half of the pair from the code being right. `(0.25, 1.0)` can,
    /// and the two one-axis cases pin the same swap from the other side:
    /// naming only `pivot_x` must move the *first* number and only that one.
    #[test]
    fn a_script_says_how_deep_a_window_is_and_what_it_turns_about() {
        let directory = std::env::temp_dir().join("solium-script-test-depth");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("super+3", function()
                sol.present(1, { z = 2.5, pivot_x = 0.25, pivot_y = 1.0 })
                -- All four, because `rect_from` refuses half a rect: a table
                -- that mentions neither new key still has to be a table a
                -- script could really write.
                sol.present(2, { x = 10, y = 20, w = 300, h = 200 })
                sol.present(3, { pivot_x = 0.25 })
                sol.present(4, { pivot_y = 1.0 })
                sol.present(5)
            end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let outcome = scripts.key("super+3", Snapshot::default());
        let drawn = presented(&outcome.commands);
        assert_eq!(drawn.len(), 5);

        assert!(
            (drawn[0].0 - 2.5).abs() < f32::EPSILON,
            "the depth the script asked for, got {}",
            drawn[0].0
        );
        assert_eq!(drawn[0].1, (0.25, 1.0), "x into x, y into y");

        // A table mentioning neither is the frame every window on the machine
        // has had until now: depth zero, which ties with every other window
        // and so keeps the order the stack gave them, turning about its own
        // centre.
        assert!((drawn[1].0 - 0.0).abs() < f32::EPSILON);
        assert_eq!(drawn[1].1, (0.5, 0.5));

        // One axis named leaves the other in the middle. Asserted from both
        // sides, because one of them alone is satisfied by a reader that
        // defaults the pair together on whichever axis it was given.
        assert_eq!(drawn[2].1, (0.25, 0.5), "only the horizontal moved");
        assert_eq!(drawn[3].1, (0.5, 1.0), "only the vertical moved");

        // And no options table at all answers the same as a table that says
        // nothing -- `sol.present(id)` clears a window back to its own
        // geometry, and it must not sort or hinge differently for it.
        assert!((drawn[4].0 - 0.0).abs() < f32::EPSILON);
        assert_eq!(drawn[4].1, (0.5, 0.5));

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A number no window can be drawn with is dropped, per key and per
    /// axis.**
    ///
    /// One division reaches NaN from a script, and the two fields answer it
    /// very differently if nothing stops it here.
    ///
    /// A **pivot** is the dangerous one. `warp.rs` computes
    /// `loc + size * pivot` for the point the matrix turns about, so a
    /// non-finite one makes that centre non-finite and every vertex of the
    /// mesh with it -- and nothing further down declines to draw the result,
    /// because `Mat4::project_with_w` guards with `out_w <= 1e-6` and every
    /// comparison against a NaN is false. The window disappears and its damage
    /// rectangle is nonsense, from a typo.
    ///
    /// A **depth** is quieter to look at and no safer. `render::by_depth`
    /// answers `Equal` when `partial_cmp` declines, so a NaN ties with 1.0 and
    /// with 2.0 while those two do not tie with each other -- not a total
    /// order, and `slice::sort_by` handed one of those **may panic**. That is
    /// the render walk, so the failure is the session going black.
    ///
    /// So both fall back to the default and say so in the log -- the answer
    /// `deform_from` gives an effect this build does not have, and
    /// `easing_from` an easing nobody wrote: the script loses the key it
    /// mistyped and keeps its window. An **infinite depth is kept**, in both
    /// directions, because it orders perfectly well: `z = math.huge` is a
    /// legible spelling of "above everything" and `-math.huge` of "behind
    /// everything". Both are asserted, because a guard written
    /// `z.is_nan() || z == f32::NEG_INFINITY` passes every other case here.
    #[test]
    fn a_depth_or_a_pivot_that_cannot_be_drawn_with_falls_back() {
        let directory = std::env::temp_dir().join("solium-script-test-nonfinite");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("super+4", function()
                sol.present(1, { z = 0/0, pivot_x = 0.25 })
                sol.present(2, { pivot_x = 0/0, pivot_y = 1.0 })
                sol.present(3, { pivot_x = 0.25, pivot_y = 1/0 })
                sol.present(4, { pivot_y = -1/0 })
                sol.present(5, { z = 1/0 })
                sol.present(6, { z = -1/0 })
            end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let outcome = scripts.key("super+4", Snapshot::default());
        let drawn = presented(&outcome.commands);
        assert_eq!(drawn.len(), 6);

        // The key that was wrong is the only key that loses anything: the
        // pivot beside the NaN depth is the one the script wrote.
        assert!(
            (drawn[0].0 - 0.0).abs() < f32::EPSILON,
            "a NaN depth is zero"
        );
        assert_eq!(drawn[0].1, (0.25, 0.5));

        // And the axis that was wrong is the only axis. This is the second
        // reading of the transpose: a NaN given as `pivot_x` must come back as
        // a centred *first* number beside the 1.0 that was given as `pivot_y`.
        assert_eq!(drawn[1].1, (0.5, 1.0), "the horizontal fell back, alone");
        assert_eq!(
            drawn[2].1,
            (0.25, 0.5),
            "and an infinity no less than a NaN"
        );
        assert_eq!(drawn[3].1, (0.5, 0.5), "in either direction");

        // A depth is not a coordinate and an infinite one sorts, so it is the
        // one non-finite number here that survives -- **in both directions**.
        // Asserted from each end because a guard that refused only one of them
        // (`z.is_nan() || z == f32::NEG_INFINITY`, the plausible one: NaN and
        // the negative infinity are what a division by zero yields when the
        // numerator went wrong) passes every other case in this test.
        assert!(
            drawn[4].0.is_infinite() && drawn[4].0 > 0.0,
            "an infinite depth orders, so it is kept, got {}",
            drawn[4].0
        );
        assert!(
            drawn[5].0.is_infinite() && drawn[5].0 < 0.0,
            "and so does a negative one, which means behind everything, got {}",
            drawn[5].0
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// The depth and pivot of each `Present` in a batch, in order.
    fn presented(commands: &[Command]) -> Vec<(f32, (f32, f32))> {
        commands
            .iter()
            .map(|command| match command {
                Command::Present { z, pivot, .. } => (*z, *pivot),
                other => panic!("expected a present command, got {other:?}"),
            })
            .collect()
    }

    /// **A script names a selection, and both halves of it come back.**
    ///
    /// The round trip for the other primitive this file gained: who is in a
    /// group, and what carrying it means. The displacement is the part worth
    /// pinning -- `x` on `sol.present_group` is a *delta* where `x` on
    /// `sol.present` is a destination, because a selection has no rectangle of
    /// its own to be moved to, and reading it as a destination would put every
    /// member of every group in the same place.
    #[test]
    fn a_script_names_a_selection_and_says_where_to_carry_it() {
        let directory = std::env::temp_dir().join("solium-script-test-group");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("super+2", function()
                sol.animate({ duration = 300, easing = "inOutQuad" })
                sol.group("desk-2", {
                    windows = { 3, 7 },
                    surfaces = { "wallpaper-2" },
                    monitors = { "DP-1" },
                    monitor = "DP-1",
                })
                sol.present_group("desk-2", { x = -2560, opacity = 0.5 })
                sol.present_group("desk-1", { y = 40 }, { duration = 90 })
                sol.present_group_clear("desk-3")
                sol.group("desk-4", false)
            end)
            "#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let outcome = scripts.key("super+2", Snapshot::default());
        assert_eq!(outcome.commands.len(), 5);

        match &outcome.commands[0] {
            Command::Group {
                name,
                selection: Some(selection),
                animation,
            } => {
                assert_eq!(name, "desk-2");
                assert_eq!(selection.windows, vec![3, 7]);
                assert_eq!(selection.surfaces, vec!["wallpaper-2".to_owned()]);
                assert_eq!(selection.monitors, vec!["DP-1".to_owned()]);
                assert_eq!(selection.on.as_deref(), Some("DP-1"));
                // A membership change is animated too, and by the ambient
                // setting: that is how long a window takes to get back to where
                // it was looking when it changes desks.
                assert_eq!(animation.duration, Duration::from_millis(300));
            }
            other => panic!("expected a group command, got {other:?}"),
        }

        match &outcome.commands[1] {
            Command::PresentGroup {
                name,
                to,
                animation,
            } => {
                assert_eq!(name, "desk-2");
                assert_eq!(to.offset(), (-2560.0, 0.0));
                assert!((to.opacity - 0.5).abs() < f32::EPSILON);
                assert_eq!(animation.duration, Duration::from_millis(300));
                assert_eq!(animation.easing, Curve::InOutQuad);
            }
            other => panic!("expected a present_group command, got {other:?}"),
        }

        // Its own table overrides the ambient duration and keeps the easing.
        match &outcome.commands[2] {
            Command::PresentGroup { animation, to, .. } => {
                assert_eq!(to.offset(), (0.0, 40.0));
                assert_eq!(animation.duration, Duration::from_millis(90));
                assert_eq!(animation.easing, Curve::InOutQuad);
            }
            other => panic!("expected a present_group command, got {other:?}"),
        }

        assert!(
            matches!(&outcome.commands[3], Command::ClearGroup { name, .. } if name == "desk-3")
        );
        // `false` removes a selection, the same spelling `sol.surface` takes.
        assert!(matches!(
            &outcome.commands[4],
            Command::Group {
                name,
                selection: None,
                ..
            } if name == "desk-4"
        ));
    }

    #[test]
    fn a_failing_script_does_not_swallow_the_key() {
        let directory = std::env::temp_dir().join("solium-script-test-error");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"sol.bind("super+e", function() error("deliberate") end)"#,
        )
        .expect("writing the test script");

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let outcome = scripts.key("super+e", Snapshot::default());
        // Not handled: the key goes to the focused client rather than
        // disappearing into a broken script.
        assert!(!outcome.handled);
        assert!(outcome.commands.is_empty());
    }

    /// Who each named selection holds, after a batch of commands.
    ///
    /// Folded rather than indexed, because `regroup` re-declares every desk on
    /// every event and a test that asserted on `commands[4]` would be pinning
    /// the order of a loop over monitors.
    fn membership(commands: &[Command]) -> std::collections::HashMap<String, Vec<u64>> {
        let mut out = std::collections::HashMap::new();
        for command in commands {
            if let Command::Group {
                name,
                selection: Some(selection),
                ..
            } = command
            {
                out.insert(name.clone(), selection.windows.clone());
            }
        }
        out
    }

    /// Where each named selection was last asked to sit, after a batch.
    fn carried(commands: &[Command]) -> std::collections::HashMap<String, (f64, f64)> {
        let mut out = std::collections::HashMap::new();
        for command in commands {
            match command {
                Command::PresentGroup { name, to, .. } => {
                    out.insert(name.clone(), to.offset());
                }
                Command::ClearGroup { name, .. } => {
                    out.insert(name.clone(), (0.0, 0.0));
                }
                _ => {}
            }
        }
        out
    }

    /// One 1600x900 screen and three windows on it, for the reload test.
    fn one_screen(windows: &[u64]) -> Snapshot {
        Snapshot {
            locked: false,
            notifications: Default::default(),
            audio: Default::default(),
            tray: Default::default(),
            media: Default::default(),
            keyboard: crate::keymap::State::initial(),
            shortcuts_owner: None,
            windows: windows
                .iter()
                .map(|id| WindowInfo {
                    icon: None,
                    client_side: false,
                    blur_region: false,
                    insets: crate::decoration::Insets::default(),
                    id: *id,
                    rect: Rect {
                        x: 0.0,
                        y: 0.0,
                        w: 800.0,
                        h: 600.0,
                    },
                    drawn: Drawn::default(),
                    title: String::new(),
                    focused: false,
                    monitor: "test-1".to_owned(),
                    // Ordinary windows: this fixture is about what a reload
                    // keeps, and #72's modality plays no part in it. Spelled
                    // out rather than defaulted so that a third field added to
                    // `WindowInfo` fails here and is decided for this test
                    // rather than silently inherited.
                    modal: false,
                    parent: Parentage::None,
                    leaving: false,
                    minimized: false,
                    above: false,
                    app_id: String::new(),
                    min: None,
                    max: None,
                    cramped: false,
                    shown: true,
                    fullscreen: false,
                    maximized: false,
                    style: String::new(),
                    x11_type: None,
                    accepts_input: true,
                    class: None,
                    instance: None,
                    content_type: "none",
                    pid: None,
                })
                .collect(),
            monitors: vec![MonitorInfo {
                name: "test-1".to_owned(),
                area: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1600.0,
                    h: 900.0,
                },
                whole: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1600.0,
                    h: 900.0,
                },
                scale: 1.0,
                focused: true,
                primary: true,
                transform: "normal".to_owned(),
                off: false,
            }],
            work_area: Rect {
                x: 0.0,
                y: 0.0,
                w: 1600.0,
                h: 900.0,
            },
            cursor: (0.0, 0.0),
            screens: Vec::new(),
            text_input: None,
            problems: Vec::new(),
            apps: std::rc::Rc::default(),
        }
    }

    /// **A reload does not lose the session, which is the whole of issue #116.**
    ///
    /// The bug as it was reported: `super+shift+r` on workspace 3, and the
    /// desktop is drawn two screen-widths off-stage with no key that brings it
    /// back. Underneath it are three separate losses, and this drives all
    /// three through the same sequence `Solium::reload` runs -- `kept` and
    /// `load_carrying`, then `restore`, `monitors`, `layout`:
    ///
    ///  * which workspace each monitor is showing, so the desks are carried
    ///    relative to the one in view and not to desk 1;
    ///  * which workspace each *window* is on, so three windows on two desks
    ///    are still on two desks rather than swept onto one;
    ///  * which layout is in charge, because a tiled session that comes back
    ///    believing it is floating toggles the wrong way on the next key.
    ///
    /// It runs the **shipped** `workspaces.lua`, `modes.lua` and `tiling.lua`
    /// rather than copies, which is what makes it a test of the files under
    /// review. That also means a developer's own `~/.config/solium` would
    /// answer `require` ahead of them -- it comes first on `package.path` --
    /// so the run is skipped there rather than asserting about somebody's
    /// configuration. The same guard, for the same reason, as
    /// `a_user_file_using_the_old_key_still_chooses_a_style`.
    #[test]
    fn a_reload_keeps_the_workspace_the_windows_and_the_layout() {
        if let Some(own) = Scripts::user_config_dir()
            && (own.join("config.lua").exists()
                || own.join("user.lua").exists()
                || own.join("workspaces.lua").exists()
                || own.join("modes.lua").exists())
        {
            return;
        }

        let directory = std::env::temp_dir().join("solium-script-test-reload");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            local workspaces = require("workspaces")
            local modes = require("modes")
            local tiling = require("tiling")

            sol.bind("super+f1", function()
                modes.use("tiling")
                workspaces.go(3)
            end)

            -- What the session believes about itself, in one string, so the
            -- assertions read the scripts' own answer rather than a
            -- reconstruction of it.
            sol.bind("super+f2", function()
                sol.status(string.format(
                    "%d %s", workspaces.on("test-1"), tostring(tiling.active)))
            end)
            "#,
        )
        .expect("writing the test script");

        let first = &[7_u64, 8];
        let all = &[7_u64, 8, 9];

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        // The screens arrive, which is the moment the desks can be built at
        // all -- a script's top level runs before any output is placed.
        scripts.monitors_changed(one_screen(first));
        // Two windows open on workspace 1, then the session moves to 3 with
        // tiling on, and a third window opens there.
        scripts.opened(7, one_screen(first));
        scripts.opened(8, one_screen(first));
        scripts.key("super+f1", one_screen(first));
        scripts.opened(9, one_screen(all));

        let before = scripts.key("super+f2", one_screen(all));
        assert_eq!(
            before.status.as_deref(),
            Some("3 true"),
            "the session was not set up: this is before any reload"
        );

        // Exactly what `Solium::reload` does, in the order it does it.
        let carried_over = scripts.kept();
        let mut scripts =
            Scripts::load_carrying(&config, carried_over).expect("reloading the test script");
        let restored = scripts.restored(one_screen(all));
        let monitors = scripts.monitors_changed(one_screen(all));
        let layout = scripts.relayout(one_screen(all));

        let after = scripts.key("super+f2", one_screen(all));
        assert_eq!(
            after.status.as_deref(),
            Some("3 true"),
            "after the reload the scripts believe they are somewhere else: the workspace in \
             view and the layout in charge are both part of the session, not of the file"
        );

        let mut commands = restored.commands;
        commands.extend(monitors.commands);
        commands.extend(layout.commands);

        let who = membership(&commands);
        assert_eq!(
            who.get("desk-1@test-1").map(Vec::as_slice),
            Some([7, 8].as_slice()),
            "the windows that were on workspace 1 are not on desk 1 any more; \
             memberships found: {who:?}"
        );
        assert_eq!(
            who.get("desk-3@test-1").map(Vec::as_slice),
            Some([9].as_slice()),
            "the window that was on workspace 3 is not on desk 3 any more; \
             memberships found: {who:?}"
        );

        // And the desks sit relative to the one in view. Desk 3 is what is
        // being looked at, so it is carried nowhere; desk 1 is two cells to
        // its left, at 1600 x 1.06 each. Off by this is the off-stage
        // desktop the issue was reported as.
        let where_they_sit = carried(&commands);
        assert_eq!(
            where_they_sit.get("desk-3@test-1"),
            Some(&(0.0, 0.0)),
            "the workspace in view is not at the origin; offsets: {where_they_sit:?}"
        );
        let (dx, dy) = where_they_sit
            .get("desk-1@test-1")
            .copied()
            .unwrap_or_default();
        assert!(
            (dx - (-2.0 * 1600.0 * 1.06)).abs() < 0.5 && dy == 0.0,
            "desk 1 sits at {dx},{dy}, which is not two screens to the left of desk 3"
        );
    }

    /// **A reload that shortens the arrangement still leaves every window
    /// somewhere you can reach.**
    ///
    /// The failure the fix for #116 introduced, which the bug it fixed did not
    /// have. `workspaces.of` is now carried across the reload, and the
    /// `restore` sweep forgot the abandoned *desks* without touching the
    /// entries naming them -- so `columns = 4` edited to `columns = 2` left a
    /// window remembering workspace 4, in no group at all:
    ///
    ///   * `regroup` loops `1..count`, so nothing ever names it -- it is drawn
    ///     over whatever workspace is in view, on top of windows that belong
    ///     there;
    ///   * `visible()` filters it out, so no layout arranges it;
    ///   * and `go()` clamps to `count()`, so no key switches to where it
    ///     thinks it is.
    ///
    /// Before #116 `of` was wiped on every reload and the window came back on
    /// the workspace in view. The same is true of `showing`, and worse: a
    /// session looking at workspace 4 when the arrangement shrinks to two
    /// carries *every* surviving desk off to the left of a workspace that no
    /// longer exists, which is the off-stage desktop the issue was reported as,
    /// reached by the other door.
    ///
    /// Same shape as the reload test above, with the configuration edited
    /// between the two loads -- which is what a reload is for.
    #[test]
    fn a_reload_with_fewer_workspaces_leaves_no_window_on_a_desk_that_is_gone() {
        if let Some(own) = Scripts::user_config_dir()
            && (own.join("config.lua").exists()
                || own.join("user.lua").exists()
                || own.join("workspaces.lua").exists()
                || own.join("modes.lua").exists())
        {
            return;
        }

        let directory = std::env::temp_dir().join("solium-script-test-reload-shorter");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            local workspaces = require("workspaces")

            sol.bind("super+f1", function() workspaces.go(3) end)
            sol.bind("super+f2", function() workspaces.go(4) end)
            sol.bind("super+f3", function()
                sol.status(string.format("%d/%d",
                    workspaces.on("test-1"), workspaces.count()))
            end)
            "#,
        )
        .expect("writing the test script");

        let first = &[7_u64];
        let all = &[7_u64, 9];

        let mut scripts = Scripts::load(&config).expect("loading the test script");
        scripts.monitors_changed(one_screen(first));
        // One window on workspace 1, then to workspace 3, a window opens there,
        // and the session is left looking at workspace 4.
        scripts.opened(7, one_screen(first));
        scripts.key("super+f1", one_screen(first));
        scripts.opened(9, one_screen(all));
        scripts.key("super+f2", one_screen(all));

        let before = scripts.key("super+f3", one_screen(all));
        assert_eq!(
            before.status.as_deref(),
            Some("4/4"),
            "the session was not set up: this is before any reload"
        );

        // The edit. `require` searches the configuration's own directory first,
        // so the *second* load finds this and the first did not.
        std::fs::write(
            directory.join("user.lua"),
            "return { workspaces = { columns = 2 } }\n",
        )
        .expect("writing the fixture user.lua");

        let carried_over = scripts.kept();
        let mut scripts =
            Scripts::load_carrying(&config, carried_over).expect("reloading the test script");
        let restored = scripts.restored(one_screen(all));
        let monitors = scripts.monitors_changed(one_screen(all));
        let layout = scripts.relayout(one_screen(all));

        let after = scripts.key("super+f3", one_screen(all));
        assert_eq!(
            after.status.as_deref(),
            Some("2/2"),
            "after the reload the scripts are still looking at a workspace the \
             arrangement no longer has, and `go` clamps to the count -- so no key \
             switches away from it"
        );

        let mut commands = restored.commands;
        commands.extend(monitors.commands);
        commands.extend(layout.commands);

        let who = membership(&commands);
        assert_eq!(
            who.get("desk-1@test-1").map(Vec::as_slice),
            Some([7].as_slice()),
            "the window on workspace 1 is not on desk 1 any more; memberships: {who:?}"
        );
        assert_eq!(
            who.get("desk-2@test-1").map(Vec::as_slice),
            Some([9].as_slice()),
            "the window kept on workspace 3 is in no selection at all: nothing names \
             it, so no layout arranges it and no key reaches it; memberships: {who:?}"
        );

        // And the desk in view is the one in view, rather than one cell to the
        // left of a workspace that no longer exists.
        let where_they_sit = carried(&commands);
        assert_eq!(
            where_they_sit.get("desk-2@test-1"),
            Some(&(0.0, 0.0)),
            "every surviving desk is carried off-stage, relative to a workspace the \
             arrangement no longer has; offsets: {where_they_sit:?}"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A keyboard section that says what layout to start on is not a typo.**
    ///
    /// `config.lua` restates `sol.keyboard`'s key set, because `keyboard`
    /// leaves the xkb names out on purpose and so cannot be the list. The
    /// restatement left `active` out, and `sol.keyboard` reads it -- so a
    /// dual-layout configuration, the only kind that has an `active` to name,
    /// was told its working setting is read by nothing and `solium --check`
    /// exited 1 (#117 review).
    ///
    /// That is worse than the silence #117 replaced. `--check` answers "did my
    /// configuration work", and a check that is wrong about a setting people
    /// really write is a check they stop running. See
    /// `every_key_a_section_accepts_is_one_the_compositor_reads`, which is what
    /// stops the list drifting again.
    #[test]
    fn a_keyboard_section_naming_its_layouts_and_which_is_live_is_not_a_typo() {
        let Some((directory, scripts)) = shipped_init_with_user(
            "solium-script-test-keyboard-active",
            r#"
            return {
                keyboard = { layout = "us,ru", active = 2 },
            }
            "#,
        ) else {
            return;
        };

        let reported: Vec<String> = scripts
            .unknown_settings()
            .into_iter()
            .map(|setting| setting.key)
            .collect();
        assert!(
            reported.is_empty(),
            "a configuration naming two layouts and which of them is live is reported \
             as read by nothing, and `--check` exits 1 over a setting that works: \
             {reported:?}"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **What `sol.keyboard()` reads, handed back to `sol.keyboard{ ... }`,
    /// compiles no keymap**, with `us,ru` and Russian live: the live layout's
    /// names come back as `layout_name` and `layout_short`, and `layout` is
    /// only ever the xkb names a keymap is compiled from.
    #[test]
    fn sol_keyboard_handed_back_what_it_read_compiles_no_keymap() {
        let directory = std::env::temp_dir().join("solium-script-test-keyboard-round-trip");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("Super+P", function()
                local read = sol.keyboard()
                sol.status(tostring(read.layout_name) .. " " .. tostring(read.layout_short))
                sol.keyboard(read)
            end)
            "#,
        )
        .expect("writing the test script");
        let mut snapshot = empty_snapshot();
        snapshot.keyboard = crate::keymap::State {
            layouts: vec!["English (US)".to_owned(), "Russian".to_owned()],
            short: vec!["EN".to_owned(), "RU".to_owned()],
            active: 2,
            ..crate::keymap::State::initial()
        };
        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let outcome = scripts.key("super+p", snapshot);
        let _ = std::fs::remove_dir_all(&directory);

        assert_eq!(
            outcome.status.as_deref(),
            Some("Russian RU"),
            "the live layout's names"
        );

        let requests: Vec<&crate::keymap::Request> = outcome
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::Keyboard(request) => Some(request),
                _ => None,
            })
            .collect();
        assert_eq!(
            requests.len(),
            1,
            "what was read is handed back as a request"
        );
        assert_eq!(
            requests[0].keymap, None,
            "and it names no keymap to compile: `layout` is not the live layout's name"
        );
    }

    /// **The shipped configuration frames every window in `glass`**: the
    /// shipped `init.lua` hands the compositor `pane = "glass"` when a
    /// `user.lua` says nothing, and the style a `user.lua` names when it
    /// names one, which is how to go back to `top`.
    #[test]
    fn the_shipped_configuration_frames_windows_in_glass() {
        for (user, wanted) in [("return {}", "glass"), ("return { pane = \"top\" }", "top")] {
            let Some((directory, mut scripts)) =
                shipped_init_with_user("solium-script-test-pane", user)
            else {
                return;
            };
            let handed: Vec<Option<String>> = scripts
                .startup()
                .commands
                .into_iter()
                .filter_map(|command| match command {
                    Command::Decoration { name } => Some(name),
                    _ => None,
                })
                .collect();
            assert_eq!(handed, vec![Some(wanted.to_owned())], "{user}");
            let _ = std::fs::remove_dir_all(&directory);
        }
    }

    /// **`fullscreen.covers` reaches the compositor from the shipped
    /// `init.lua`**: the default when a `user.lua` says nothing, `"none"` when
    /// it says that, and the default kept for a word that is neither.
    #[test]
    fn the_shipped_fullscreen_setting_reaches_the_compositor() {
        for (user, wanted) in [
            ("return {}", crate::stack::Covers::Top),
            (
                "return { fullscreen = { covers = \"none\" } }",
                crate::stack::Covers::Nothing,
            ),
            (
                "return { fullscreen = { covers = \"everything\" } }",
                crate::stack::Covers::Top,
            ),
        ] {
            let Some((directory, mut scripts)) =
                shipped_init_with_user("solium-script-test-fullscreen", user)
            else {
                return;
            };
            let handed: Vec<crate::stack::Covers> = scripts
                .startup()
                .commands
                .into_iter()
                .filter_map(|command| match command {
                    Command::Fullscreen(covers) => Some(covers),
                    _ => None,
                })
                .collect();
            assert_eq!(handed, vec![wanted], "{user}");
            let _ = std::fs::remove_dir_all(&directory);
        }
    }

    /// **The shipped `resize` setting reaches the compositor (#233)**: the
    /// default is `anchor` on `#1c1c1c`, and each key a user changes changes
    /// what the compositor is told -- a name that is not one keeps the
    /// default, as `fill`'s always did. `patience` is milliseconds from 0 to
    /// `resizing::PATIENCE_MAX`, anything else keeping the quarter second.
    #[test]
    fn the_shipped_resize_setting_reaches_the_compositor() {
        use crate::resizing::{BACKGROUND, EVERY_FRAME, Fill, Follow, PATIENCE, SYNC, Settings};
        let waits = |millis| Settings {
            patience: std::time::Duration::from_millis(millis),
            ..Settings::default()
        };
        let syncs = |millis| Settings {
            sync: std::time::Duration::from_millis(millis),
            ..Settings::default()
        };
        let tells = |hz: f64| Settings {
            tell: std::time::Duration::from_secs_f64(1.0 / hz),
            ..Settings::default()
        };
        for (user, wanted) in [
            (
                "return {}",
                Settings {
                    fill: Fill::Crop,
                    background: BACKGROUND,
                    patience: PATIENCE,
                    tell: EVERY_FRAME,
                    follow: Follow::App,
                    sync: SYNC,
                    overlap: true,
                },
            ),
            (
                "return { resize = { fill = \"stretch\" } }",
                Settings {
                    fill: Fill::Stretch,
                    background: BACKGROUND,
                    patience: PATIENCE,
                    tell: EVERY_FRAME,
                    follow: Follow::App,
                    sync: SYNC,
                    overlap: true,
                },
            ),
            (
                "return { resize = { fill = \"crop\", background = \"#10203080\" } }",
                Settings {
                    fill: Fill::Crop,
                    background: crate::resizing::Background::Colour([0x10, 0x20, 0x30, 0x80]),
                    patience: PATIENCE,
                    tell: EVERY_FRAME,
                    follow: Follow::App,
                    sync: SYNC,
                    overlap: true,
                },
            ),
            (
                "return { resize = { background = \"none\" } }",
                Settings {
                    fill: Fill::Crop,
                    background: crate::resizing::Background::None,
                    patience: PATIENCE,
                    tell: EVERY_FRAME,
                    follow: Follow::App,
                    sync: SYNC,
                    overlap: true,
                },
            ),
            (
                "return { resize = { fill = \"hold\", background = \"dark\" } }",
                Settings {
                    fill: Fill::Crop,
                    background: BACKGROUND,
                    patience: PATIENCE,
                    tell: EVERY_FRAME,
                    follow: Follow::App,
                    sync: SYNC,
                    overlap: true,
                },
            ),
            (
                "return { resize = { background = \"#1c1c1c\" } }",
                Settings {
                    background: crate::resizing::Background::Colour([0x1c, 0x1c, 0x1c, 0xff]),
                    ..Settings::default()
                },
            ),
            (
                "return { resize = { background = \"edge\" } }",
                Settings::default(),
            ),
            ("return { resize = { patience = 600 } }", waits(600)),
            ("return { resize = { patience = 0 } }", waits(0)),
            ("return { resize = { patience = 5000 } }", waits(5000)),
            ("return { resize = { patience = 5001 } }", waits(250)),
            ("return { resize = { patience = -1 } }", waits(250)),
            ("return { resize = { patience = \"long\" } }", waits(250)),
            ("return { resize = { sync = 0 } }", syncs(0)),
            ("return { resize = { sync = 50 } }", syncs(50)),
            ("return { resize = { sync = 200 } }", syncs(200)),
            ("return { resize = { sync = 5000 } }", syncs(5000)),
            ("return { resize = { sync = 5001 } }", syncs(0)),
            ("return { resize = { sync = -1 } }", syncs(0)),
            ("return { resize = { sync = \"long\" } }", syncs(0)),
            (
                "return { resize = { overlap = false } }",
                Settings {
                    overlap: false,
                    ..Settings::default()
                },
            ),
            (
                "return { resize = { overlap = \"no\" } }",
                Settings::default(),
            ),
            ("return { resize = { tell_hz = 30 } }", tells(30.0)),
            ("return { resize = { tell_hz = 1000 } }", tells(1000.0)),
            ("return { resize = { tell_hz = 0 } }", Settings::default()),
            (
                "return { resize = { tell_hz = 1001 } }",
                Settings::default(),
            ),
            ("return { resize = { tell_hz = 0.5 } }", Settings::default()),
            ("return { resize = { tell_hz = -1 } }", Settings::default()),
            (
                "return { resize = { tell_hz = \"fast\" } }",
                Settings::default(),
            ),
            (
                "return { resize = { follow = \"pointer\" } }",
                Settings {
                    follow: Follow::Pointer,
                    ..Settings::default()
                },
            ),
            (
                "return { resize = { follow = \"app\" } }",
                Settings::default(),
            ),
            (
                "return { resize = { follow = \"edge\" } }",
                Settings::default(),
            ),
        ] {
            let Some((directory, mut scripts)) =
                shipped_init_with_user("solium-script-test-resize", user)
            else {
                return;
            };
            let handed: Vec<Settings> = scripts
                .startup()
                .commands
                .into_iter()
                .filter_map(|command| match command {
                    Command::Resize(settings) => Some(settings),
                    _ => None,
                })
                .collect();
            assert_eq!(handed, vec![wanted], "{user}");
            let _ = std::fs::remove_dir_all(&directory);
        }
    }

    /// **Each mode fits a resize by its own `resize.modes` entry, or by
    /// `resize.fill`, the one for every mode (#233)**, handed over as the
    /// focus model is: once at load for the mode the session opens in, and
    /// again at each switch, with the background and the patience beside it.
    /// A mode the table does not name takes `fill`; a table naming no mode
    /// hands nothing over past `init.lua`'s one call, so the shipped
    /// configuration says it once.
    #[test]
    fn each_mode_fits_a_resize_by_its_own_fill_or_the_one_for_every_mode() {
        use crate::resizing::{BACKGROUND, Fill, Settings};
        let last = |commands: Vec<Command>| {
            commands
                .into_iter()
                .filter_map(|command| match command {
                    Command::Resize(settings) => Some(settings),
                    _ => None,
                })
                .next_back()
        };
        let fill = |fill: Fill| {
            Some(Settings {
                fill,
                background: BACKGROUND,
                ..Settings::default()
            })
        };
        let patient = |fill: Fill| {
            Some(Settings {
                fill,
                patience: std::time::Duration::from_millis(600),
                ..Settings::default()
            })
        };
        let capped = |fill: Fill| {
            Some(Settings {
                fill,
                tell: std::time::Duration::from_secs_f64(1.0 / 30.0),
                ..Settings::default()
            })
        };
        // At load (floating), then tiling, then scrolling.
        for (resize, expected) in [
            (
                r#"{ fill = "anchor", modes = { tiling = "stretch" } }"#,
                [fill(Fill::Anchor), fill(Fill::Stretch), fill(Fill::Anchor)],
            ),
            (
                r#"{ modes = { floating = "stretch", scrolling = "anchor" } }"#,
                [fill(Fill::Stretch), fill(Fill::Crop), fill(Fill::Anchor)],
            ),
            (
                r#"{ background = "none", modes = { scrolling = "stretch" } }"#,
                [
                    Some(Settings {
                        fill: Fill::Crop,
                        background: crate::resizing::Background::None,
                        ..Settings::default()
                    }),
                    Some(Settings {
                        fill: Fill::Crop,
                        background: crate::resizing::Background::None,
                        ..Settings::default()
                    }),
                    Some(Settings {
                        fill: Fill::Stretch,
                        background: crate::resizing::Background::None,
                        ..Settings::default()
                    }),
                ],
            ),
            (
                r#"{ patience = 600, modes = { tiling = "stretch" } }"#,
                [
                    patient(Fill::Crop),
                    patient(Fill::Stretch),
                    patient(Fill::Crop),
                ],
            ),
            // `tell_hz` goes over with every switch, as the patience does: a
            // switch that dropped it would put the drag back to every frame.
            (
                r#"{ tell_hz = 30, modes = { tiling = "stretch" } }"#,
                [
                    capped(Fill::Crop),
                    capped(Fill::Stretch),
                    capped(Fill::Crop),
                ],
            ),
            ("{}", [fill(Fill::Crop), None, None]),
        ] {
            let Some((directory, mut scripts)) = shipped_init_with_user(
                "solium-script-test-resize-modes",
                &format!("return {{ resize = {resize} }}"),
            ) else {
                return;
            };
            let handed = [
                last(scripts.startup().commands),
                last(scripts.key("super+t", empty_snapshot()).commands),
                last(scripts.key("super+s", empty_snapshot()).commands),
            ];
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                handed, expected,
                "resize = {resize}: at load, then tiling, then scrolling"
            );
        }
    }

    /// **`sol.effects` reaches the compositor as rules**, parsed.
    #[test]
    fn sol_effects_reaches_the_compositor_as_rules() {
        let directory =
            std::env::temp_dir().join(format!("solium-effects-rules-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"sol.effects({ rules = { { match = { app_id = "mpv" }, part = "client", slot = "replace", effect = { "blur", source = "self" } } } })"#,
        )
        .expect("writing");
        let mut scripts = Scripts::load(&config).expect("loading");
        let commands = scripts.startup().commands;
        let _ = std::fs::remove_dir_all(&directory);
        let rules = commands.iter().find_map(|command| match command {
            Command::Effects { rules, .. } => Some(rules.clone()),
            _ => None,
        });
        assert_eq!(rules.expect("sent").expect("parsed").len(), 1);
    }

    /// **`sol.effects` reaches the compositor with its settings**: the
    /// engine's keys beside the rules, parsed; a key out of its bounds is
    /// the set's error; no options at all are the defaults.
    #[test]
    fn sol_effects_reaches_the_compositor_with_its_settings() {
        let directory =
            std::env::temp_dir().join(format!("solium-effects-settings-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            "sol.effects({ rules = {}, sandbox = { load_ms = 250 }, present = { failed = 'hide' } })\n\
             sol.effects({ limits = { params = 99 } })\nsol.effects()\n",
        )
        .expect("writing");
        let mut scripts = Scripts::load(&config).expect("loading");
        let commands = scripts.startup().commands;
        let _ = std::fs::remove_dir_all(&directory);
        let sets: Vec<_> = commands
            .into_iter()
            .filter_map(|command| match command {
                Command::Effects { settings, .. } => Some(settings),
                _ => None,
            })
            .collect();
        let [set, refused, none] = sets.as_slice() else {
            panic!("three sets: {sets:?}");
        };
        let set = set.as_ref().expect("parsed");
        assert_eq!(set.sandbox.load, std::time::Duration::from_millis(250));
        assert_eq!(
            set.present.failed,
            crate::effect::settings::PresentFailed::Hide
        );
        assert!(
            refused
                .as_ref()
                .is_err_and(|error| error.contains("effects.limits.params")),
            "{refused:?}"
        );
        assert_eq!(none, &Ok(crate::effect::settings::Settings::default()));
    }

    /// **`sol.effects` reads `xray` and `memo`** (Rulings F29, F30; amended
    /// 2026-10-08, configurable): both reach the compositor in its settings,
    /// parsed; a word outside `split`'s words is the set's error, naming it;
    /// and a number that is not finite is refused by name rather than
    /// dropped for the default, as `effect::settings::tree_of` keeps it in
    /// both tables and in their lists, so `budget_mib = 1/0` is not "no
    /// cap", `scale = 0/0` is not 1, and `layers = { "background", 0/0 }`
    /// is not the wallpaper alone.
    #[test]
    fn sol_effects_reads_xray_and_memo() {
        let directory =
            std::env::temp_dir().join(format!("solium-effects-xray-memo-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            "sol.effects({ xray = { keep_ms = 0 }, memo = { split = 'off' } })\n\
             sol.effects({ memo = { split = 'short' } })\n\
             sol.effects({ memo = { budget_mib = 1/0 } })\n\
             sol.effects({ xray = { scale = 0/0 } })\n\
             sol.effects({ xray = { keep_ms = -1/0 } })\n\
             sol.effects({ xray = 1/0 })\n\
             sol.effects({ xray = { layers = { 0/0 } } })\n\
             sol.effects({ xray = { layers = { 'background', 0/0 } } })\n",
        )
        .expect("writing");
        let mut scripts = Scripts::load(&config).expect("loading");
        let commands = scripts.startup().commands;
        let _ = std::fs::remove_dir_all(&directory);
        let sets: Vec<_> = commands
            .into_iter()
            .filter_map(|command| match command {
                Command::Effects { settings, .. } => Some(settings),
                _ => None,
            })
            .collect();
        let [read, refused @ ..] = sets.as_slice() else {
            panic!("eight sets: {sets:?}");
        };
        let read = read.as_ref().expect("parsed");
        assert_eq!(
            (read.xray.keep_ms, read.memo.split),
            (0, solium_effects::stage::Split::Off)
        );
        assert_eq!(
            (read.xray.scale, read.memo.budget_mib),
            (1.0, 0),
            "what was not said is the default"
        );
        let says = [
            "`effects.memo.split`",
            "`effects.memo.budget_mib`",
            "`effects.xray.scale`",
            "`effects.xray.keep_ms`",
            "`effects.xray` is a table",
            "`effects.xray.layers` is a list of words",
            "`effects.xray.layers` is a list of words",
        ];
        assert_eq!(refused.len(), says.len(), "{refused:?}");
        for (refused, says) in refused.iter().zip(says) {
            assert!(
                refused.as_ref().is_err_and(|error| error.contains(says)),
                "{says}: {refused:?}"
            );
        }
    }

    /// **`sol.startup` reaches the compositor**: `first_frame` read and
    /// checked into `Command::Startup`, with no table the default, and a
    /// refused value, a number that is not finite or an unknown key an
    /// `Err` naming what is wrong.
    #[test]
    fn sol_startup_reaches_the_compositor() {
        let directory = std::env::temp_dir().join(format!("solium-startup-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            "sol.startup({ first_frame = { max_ms = 500 } })\n\
             sol.startup()\n\
             sol.startup({ first_frame = { max_ms = -1 } })\n\
             sol.startup({ first_frame = { max_ms = 1/0 } })\n\
             sol.startup({ frist_frame = {} })\n\
             sol.startup({ first_frame = 5 })\n",
        )
        .expect("writing");
        let mut scripts = Scripts::load(&config).expect("loading");
        let commands = scripts.startup().commands;
        let _ = std::fs::remove_dir_all(&directory);
        let read: Vec<_> = commands
            .into_iter()
            .filter_map(|command| match command {
                Command::Startup(first_frame) => Some(first_frame),
                _ => None,
            })
            .collect();
        let [given, none, refused @ ..] = read.as_slice() else {
            panic!("six calls: {read:?}");
        };
        assert_eq!(
            given,
            &Ok(crate::warm::FirstFrame {
                max_ms: 500,
                ..crate::warm::FirstFrame::default()
            })
        );
        assert_eq!(none, &Ok(crate::warm::FirstFrame::default()));
        let says = [
            "`startup.first_frame.max_ms`",
            "`startup.first_frame.max_ms`",
            "did you mean `first_frame`?",
            "`startup.first_frame` is a table",
        ];
        assert_eq!(refused.len(), says.len(), "{refused:?}");
        for (refused, says) in refused.iter().zip(says) {
            assert!(
                refused.as_ref().is_err_and(|error| error.contains(says)),
                "{says}: {refused:?}"
            );
        }
    }

    /// **A broken rule reaches the compositor as its errors**, so the
    /// compositor can keep the rules that ran and list them; and
    /// `sol.effects{}` with no `rules` is an empty set, not an error.
    #[test]
    fn a_broken_rule_reaches_the_compositor_as_its_errors() {
        let directory =
            std::env::temp_dir().join(format!("solium-effects-broken-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            "sol.effects({ rules = { { match = '*', part = 'cleint', slot = 'behind', effect = false } } })\nsol.effects({})\n",
        )
        .expect("writing");
        let mut scripts = Scripts::load(&config).expect("loading");
        let commands = scripts.startup().commands;
        let _ = std::fs::remove_dir_all(&directory);
        let sets: Vec<_> = commands
            .into_iter()
            .filter_map(|command| match command {
                Command::Effects { rules, .. } => Some(rules),
                _ => None,
            })
            .collect();
        let [broken, empty] = sets.as_slice() else {
            panic!("two sets: {sets:?}");
        };
        let errors = broken.as_ref().expect_err("refused");
        assert_eq!((errors[0].rule, errors[0].key), (1, "part"));
        assert_eq!(empty, &Ok(Vec::new()));
    }

    /// **The shipped configuration declares no rules**, and hands that over,
    /// so a reload that removes a user's rules takes them away; a `user.lua`'s
    /// rules replace the empty list whole.
    #[test]
    fn the_shipped_configuration_declares_no_rules() {
        for (user, wanted) in [
            ("return {}", 0),
            (
                "return { effects = { rules = { { match = '*', part = 'client', slot = 'behind', effect = 'glow' } } } }",
                1,
            ),
        ] {
            let Some((directory, mut scripts)) =
                shipped_init_with_user("solium-effects-none", user)
            else {
                return;
            };
            let unknown = scripts.unknown_settings().len();
            let rules = scripts
                .startup()
                .commands
                .into_iter()
                .find_map(|command| match command {
                    Command::Effects { rules, .. } => Some(rules),
                    _ => None,
                });
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(
                rules.expect("sent").expect("parsed").len(),
                wanted,
                "{user}"
            );
            assert_eq!(unknown, 0, "{user}: `effects` is a setting");
        }
    }

    /// **The shipped configuration writes out the engine's defaults**:
    /// `config.lua`'s `effects.sandbox`, `limits` and `present` are the
    /// numbers Rust defaults to, so the two cannot drift; and a `user.lua`
    /// that raises one reaches the compositor.
    #[test]
    fn the_shipped_configuration_writes_out_the_engines_defaults() {
        for (user, wanted) in [
            ("return {}", crate::effect::settings::Settings::default()),
            (
                "return { effects = { sandbox = { load_ms = 300 } } }",
                crate::effect::settings::Settings {
                    sandbox: crate::effect::settings::Caps {
                        load: std::time::Duration::from_millis(300),
                        ..crate::effect::settings::Caps::default()
                    },
                    ..crate::effect::settings::Settings::default()
                },
            ),
        ] {
            let Some((directory, mut scripts)) =
                shipped_init_with_user("solium-effects-settings", user)
            else {
                return;
            };
            let unknown = scripts.unknown_settings().len();
            let settings =
                scripts
                    .startup()
                    .commands
                    .into_iter()
                    .find_map(|command| match command {
                        Command::Effects { settings, .. } => Some(settings),
                        _ => None,
                    });
            let _ = std::fs::remove_dir_all(&directory);
            assert_eq!(settings, Some(Ok(wanted)), "{user}");
            assert_eq!(unknown, 0, "{user}: every key is a setting");
        }
    }

    /// Every key `config.lua`'s `open_sections` accepts for one section.
    ///
    /// Scraped from the shipped file rather than restated here: a third copy of
    /// a list whose second copy is the defect under test would be the same
    /// mistake with more steps.
    fn accepted_keys(section: &str) -> Vec<String> {
        let path = crate::assets::lua().join("config.lua");
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let Some(table) = text.split("local open_sections = {").nth(1) else {
            return Vec::new();
        };
        let Some(rest) = table.split(&format!("{section} = {{")).nth(1) else {
            return Vec::new();
        };
        // Comments stripped before anything else is looked for, because a line
        // documenting `sol.keyboard{ active = 2 }` holds both a closing brace
        // and an `=` -- so leaving them in ends the table early and drops the
        // very key the comment is about. No string in this table holds a `--`.
        let without_comments: String = rest
            .lines()
            .map(|line| line.split_once("--").map_or(line, |(code, _)| code))
            .collect::<Vec<&str>>()
            .join("\n");
        let Some(body) = without_comments.split('}').next() else {
            return Vec::new();
        };
        let mut found: Vec<String> = body
            .split(',')
            .filter_map(|entry| entry.split_once('='))
            .filter(|(_, value)| value.trim() == "true")
            .map(|(key, _)| key.trim().to_owned())
            .collect();
        found.sort();
        found
    }

    /// The keys a section's parser reads out of the document it is handed.
    ///
    /// Asked of the parser through the document's own record of what was
    /// looked up in it, so this cannot be a restatement of the reads and
    /// cannot drift from them however they are spelled -- `keyboard` reads
    /// five of its keys through a closure and three directly, and a scan for
    /// `get(` would see only three.
    fn keys_read_by(name: &str) -> Vec<String> {
        doc::tests::keys_read(|| match name {
            "cursor_theme" => {
                let _ = settings::cursor::parse(&Doc::table());
            }
            "keyboard" => {
                let _ = settings::keyboard::parse(&Doc::table());
            }
            other => panic!("no parser named for sol.{other}"),
        })
    }

    /// **The key sets `config.lua` restates are the ones the compositor reads.**
    ///
    /// `keyboard` and `cursor` default to `{}` -- empty *means* "whatever the
    /// session already said" -- so the defaults cannot double as the list of
    /// what exists, and `config.lua` writes the list out. #117 said in a comment
    /// that a reader growing a key would make this report that key as a typo,
    /// and called that loud. It was not loud: the list shipped already missing
    /// `active`, and nothing failed.
    ///
    /// So the two halves are compared here instead, and in both directions. A
    /// key the compositor reads and the list omits is a working setting called
    /// a typo, which exits `--check` non-zero over nothing; a key the list
    /// accepts and nothing reads is #117's original fault, read from the other
    /// side -- a real typo waved through.
    #[test]
    fn every_key_a_section_accepts_is_one_the_compositor_reads() {
        for (section, function) in [("keyboard", "keyboard"), ("cursor", "cursor_theme")] {
            let read = keys_read_by(function);
            let accepted = accepted_keys(section);
            // Either scrape coming back empty would make this agree with
            // anything, and both have a way of doing that quietly: a renamed
            // `open_sections`, or a `sol.` function that stopped being reached.
            assert!(
                read.len() >= 2 && accepted.len() >= 2,
                "sol.{function} was seen reading {read:?} and config.lua's \
                 open_sections.{section} accepts {accepted:?}; one of the two walks \
                 is broken, not the lists"
            );
            assert_eq!(
                accepted, read,
                "config.lua's open_sections.{section} and the keys sol.{function} \
                 actually reads have drifted apart: a key only the compositor reads \
                 is a working setting `--check` calls a typo, and a key only the \
                 list has is a typo `--check` waves through"
            );
        }
    }

    fn loaded(name: &str, script: &str) -> (std::path::PathBuf, Scripts) {
        let directory = std::env::temp_dir().join(name);
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(&config, script).expect("writing the test script");
        let scripts = Scripts::load(&config).expect("loading the test script");
        (directory, scripts)
    }

    /// **A scene's action reaches Lua with its surface's name and its data.**
    #[test]
    fn a_surface_action_reaches_lua_with_its_data() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-action-data",
            r#"sol.on("surface", function(surface, action, data) sol.status(surface .. " " .. action .. " " .. data.id) end)"#,
        );
        let data = crate::json::Json::parse(r#"{"id":7}"#).expect("valid JSON");
        let outcome = scripts.surface_action("shell", "windows.focus", &data, one_screen(&[]));
        assert_eq!(outcome.status.as_deref(), Some("shell windows.focus 7"));
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A frame's listeners hear the window, the name and the data**
    /// (#417): `sol.on("frame", function(id, name, data))`.
    #[test]
    fn frame_listeners_hear_the_window_the_name_and_the_data() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-frame-action",
            r#"sol.on("frame", function(id, name, data) sol.status(id .. " " .. name .. " " .. data.by) end)"#,
        );
        let data = crate::json::Json::parse(r#"{"by":0.05}"#).expect("valid JSON");
        let outcome = scripts.frame_action(7, "volume", &data, one_screen(&[]));
        assert_eq!(outcome.status.as_deref(), Some("7 volume 0.05"));
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **`sol.frame` hands one window its values** (#417): the window's id
    /// and the table, as a command, and nothing for anything not a table.
    #[test]
    fn sol_frame_hands_one_window_its_values() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-frame-values",
            r#"sol.frame(7, { opacity = 0.5, label = "a" })
               sol.status(tostring(pcall(sol.frame, 7, 1)))"#,
        );
        let outcome = scripts.startup();
        let handed: Vec<(u64, String)> = outcome
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::FrameValues { id, fields } => {
                    Some((*id, crate::json::Json::Object(fields.clone()).render()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(handed, [(7, r#"{"label":"a","opacity":0.5}"#.to_owned())]);
        assert_eq!(outcome.status.as_deref(), Some("false"));
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn an_action_with_no_data_reaches_lua_as_nil() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-action-nil",
            r#"sol.on("surface", function(surface, action, data) sol.status(tostring(data)) end)"#,
        );
        let outcome = scripts.surface_action(
            "tweaks",
            "pane:border",
            &crate::json::Json::Null,
            one_screen(&[]),
        );
        assert_eq!(outcome.status.as_deref(), Some("nil"));
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A window going fullscreen or maximised is told to the scripts, each
    /// change by its own event, and answered with the timing `sol.animate`
    /// set** (#49): the listener hears `(id, entering)`, and what it set is
    /// the outcome's `motion`. A dispatch that called nothing answers `None`,
    /// which the compositor reads as "at once", and is not the 220 ms a
    /// command queued with no `sol.animate` is given.
    #[test]
    fn a_change_is_answered_with_the_motion_sol_animate_set() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-change-motion",
            r#"
            sol.on("fullscreen", function(id, entering)
                sol.status("fullscreen " .. id .. " " .. tostring(entering))
                sol.animate({ duration = 260, easing = "linear" })
            end)
            sol.on("maximize", function(id, entering)
                sol.status("maximize " .. id .. " " .. tostring(entering))
                if entering then sol.animate({ duration = 120 }) end
            end)
            "#,
        );

        let entering = scripts.fullscreen(7, true, one_screen(&[7]));
        assert_eq!(entering.status.as_deref(), Some("fullscreen 7 true"));
        let motion = entering.motion.expect("the listener set a motion");
        assert_eq!(
            (motion.duration, motion.easing),
            (Duration::from_millis(260), Curve::Linear)
        );

        let leaving = scripts.fullscreen(7, false, one_screen(&[7]));
        assert_eq!(leaving.status.as_deref(), Some("fullscreen 7 false"));

        let maximised = scripts.maximize(7, true, one_screen(&[7]));
        assert_eq!(maximised.status.as_deref(), Some("maximize 7 true"));
        let motion = maximised.motion.expect("the listener set a motion");
        assert_eq!(
            (motion.duration, motion.easing),
            (Duration::from_millis(120), Curve::OutCubic),
            "the easing it did not name is the default's"
        );

        let restored = scripts.maximize(7, false, one_screen(&[7]));
        assert_eq!(restored.status.as_deref(), Some("maximize 7 false"));
        assert!(
            restored.motion.is_none(),
            "nothing called sol.animate, so there is no motion: {:?}",
            restored.motion
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **`sol.act` queues the action as a command and answers an attempt id;
    /// `done` hears the outcome once** (03 §3.3.2).
    #[test]
    fn sol_notifications_sets_the_service_and_answers_its_rows() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-notifications",
            r#"sol.notifications{ serve = true, timeouts = { low = 1000, normal = 9000, critical = 0 }, history = { max = 5 } }"#,
        );
        let configs: Vec<crate::services::notifications::Config> = scripts
            .startup()
            .commands
            .into_iter()
            .filter_map(|command| match command {
                Command::Notifications(config) => Some(config),
                _ => None,
            })
            .collect();
        let [config] = configs.as_slice() else {
            panic!("one sol.notifications: {configs:?}")
        };
        assert!(config.serve);
        assert_eq!(
            config.settings.timeouts.low,
            Some(Duration::from_millis(1000))
        );
        assert_eq!(
            config.settings.timeouts.normal,
            Some(Duration::from_millis(9000))
        );
        assert_eq!(config.settings.timeouts.critical, None);
        assert_eq!(config.settings.history_max, 5);

        let mut snapshot = one_screen(&[]);
        snapshot.notifications = std::rc::Rc::new(crate::services::notifications::View {
            available: true,
            serving: true,
            dnd: false,
            owner: None,
            banners: vec![std::collections::BTreeMap::from([
                ("id", crate::json::Json::Number(7.0)),
                ("summary", crate::json::Json::Text("New mail".to_owned())),
            ])],
            history: Vec::new(),
        });
        assert_eq!(
            scripts.evaluate_in(
                snapshot,
                "local n = sol.notifications(); return n.banners[1].summary .. ' ' .. n.banners[1].id .. ' ' .. tostring(n.serving) .. ' ' .. #n.history"
            ),
            "New mail 7 true 0"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn the_first_notification_hook_that_answers_decides() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-notification-hook",
            r#"sol.on("notification", function(n) seen = n.summary end)
               sol.on("notification", function(n) if n.urgency == "low" then return { banner = false } end end)
               sol.on("notification", function(n) return { banner = true, keep = false } end)"#,
        );
        let arrival = |urgency: &str| {
            crate::json::Json::Object(std::collections::BTreeMap::from([
                (
                    "summary".to_owned(),
                    crate::json::Json::Text("Hi".to_owned()),
                ),
                (
                    "urgency".to_owned(),
                    crate::json::Json::Text(urgency.to_owned()),
                ),
            ]))
        };
        let (low, _) = scripts.notification_decision(&arrival("low"), one_screen(&[]));
        assert_eq!(
            low.map(|table| table.render()),
            Some(r#"{"banner":false}"#.to_owned())
        );
        let (normal, _) = scripts.notification_decision(&arrival("normal"), one_screen(&[]));
        assert_eq!(
            normal.map(|table| table.render()),
            Some(r#"{"banner":true,"keep":false}"#.to_owned())
        );
        assert_eq!(scripts.evaluate("return seen"), "Hi");
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn notification_events_reach_their_listeners_with_their_origin() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-notification-events",
            r#"sol.on("notifications", function(e) sol.status(e.what .. " " .. e.origin .. " " .. e.id .. " " .. tostring(e.reason)) end)"#,
        );
        let event = crate::json::Json::Object(std::collections::BTreeMap::from([
            (
                "what".to_owned(),
                crate::json::Json::Text("closed".to_owned()),
            ),
            (
                "origin".to_owned(),
                crate::json::Json::Text("scene".to_owned()),
            ),
            ("id".to_owned(), crate::json::Json::Number(7.0)),
            (
                "reason".to_owned(),
                crate::json::Json::Text("dismissed".to_owned()),
            ),
        ]));
        let outcome = scripts.notifications_event(&event, one_screen(&[]));
        assert_eq!(outcome.status.as_deref(), Some("closed scene 7 dismissed"));
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn sol_audio_sets_its_limits_and_answers_its_endpoints() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-audio",
            r#"sol.audio{ max = 1.2, step = 0.02 }"#,
        );
        let settings: Vec<Option<crate::services::audio::model::Settings>> = scripts
            .startup()
            .commands
            .into_iter()
            .filter_map(|command| match command {
                Command::Audio(settings) => Some(settings),
                _ => None,
            })
            .collect();
        assert_eq!(
            settings,
            [Some(crate::services::audio::model::Settings {
                max: 1.2,
                step: 0.02
            })]
        );
        let mut snapshot = one_screen(&[]);
        snapshot.audio = std::rc::Rc::new(crate::services::audio::View {
            available: true,
            ready: true,
            output: Some(std::collections::BTreeMap::from([
                ("label", crate::json::Json::Text("Speakers".to_owned())),
                ("volume", crate::json::Json::Number(0.5)),
            ])),
            input: None,
            outputs: Vec::new(),
            inputs: Vec::new(),
            streams: vec![std::collections::BTreeMap::from([
                ("id", crate::json::Json::Number(41.0)),
                ("app", crate::json::Json::Text("Firefox".to_owned())),
                (
                    "windows",
                    crate::json::Json::List(vec![crate::json::Json::Number(7.0)]),
                ),
            ])],
            windows: std::collections::BTreeMap::from([(
                7,
                std::collections::BTreeMap::from([
                    ("volume", crate::json::Json::Number(0.25)),
                    ("count", crate::json::Json::Number(1.0)),
                ]),
            )]),
        });
        assert_eq!(
            scripts.evaluate_in(
                snapshot,
                "local a = sol.audio(); return a.output.label .. ' ' .. a.output.volume .. ' ' .. tostring(a.input) .. ' ' .. #a.outputs \
                 .. ' ' .. a.streams[1].app .. ' ' .. a.streams[1].windows[1] .. ' ' .. a.windows[7].volume"
            ),
            "Speakers 0.5 nil 0 Firefox 7 0.25"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn audio_events_reach_their_listeners_with_their_origin() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-audio-events",
            r#"sol.on("audio", function(e) sol.status(e.what .. " " .. e.origin .. " " .. e.bus .. " " .. tostring(e.output)) end)"#,
        );
        let event = crate::services::audio::event_json(&crate::services::audio::model::Event {
            what: "mute",
            id: 3,
            direction: crate::services::audio::model::Direction::Output,
            output: true,
            value: 0.4,
            muted: true,
            origin: crate::services::audio::volume::Origin::External,
            bus: crate::services::audio::model::Kind::Bluetooth,
            stream: false,
        });
        let outcome = scripts.audio_event(&event, one_screen(&[]));
        assert_eq!(
            outcome.status.as_deref(),
            Some("mute external bluetooth true")
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn sol_tray_sets_the_service_and_answers_its_items() {
        let (directory, mut scripts) =
            loaded("solium-script-test-tray", r#"sol.tray{ serve = true }"#);
        let configs: Vec<crate::services::tray::Config> = scripts
            .startup()
            .commands
            .into_iter()
            .filter_map(|command| match command {
                Command::Tray(config) => Some(config),
                _ => None,
            })
            .collect();
        assert_eq!(configs, [crate::services::tray::Config { serve: true }]);
        let mut snapshot = one_screen(&[]);
        snapshot.tray = std::rc::Rc::new(crate::services::tray::View {
            available: true,
            serving: true,
            owner: None,
            items: vec![std::collections::BTreeMap::from([
                ("id", crate::json::Json::Number(4.0)),
                ("title", crate::json::Json::Text("Steam".to_owned())),
            ])],
        });
        assert_eq!(
            scripts.evaluate_in(
                snapshot,
                "local t = sol.tray(); return t.items[1].title .. ' ' .. t.items[1].id .. ' ' .. tostring(t.serving) .. ' ' .. tostring(t.owner)"
            ),
            "Steam 4 true nil"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn tray_events_reach_their_listeners_with_their_origin() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-tray-events",
            r#"sol.on("tray", function(e) sol.status(e.what .. " " .. e.origin .. " " .. e.id .. " " .. tostring(e.open) .. " " .. e.item.title) end)"#,
        );
        let view = crate::services::tray::View {
            items: vec![std::collections::BTreeMap::from([
                ("id", crate::json::Json::Number(4.0)),
                ("title", crate::json::Json::Text("Steam".to_owned())),
                ("menuOpen", crate::json::Json::Bool(true)),
            ])],
            ..crate::services::tray::View::default()
        };
        let event = crate::services::tray::Event {
            what: "menu",
            id: 4,
            origin: crate::services::notifications::Origin::Scene,
        }
        .to_json(&view);
        let outcome = scripts.tray_event(&event, one_screen(&[]));
        assert_eq!(outcome.status.as_deref(), Some("menu scene 4 true Steam"));
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn sol_media_sets_the_service_and_answers_its_players() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-media",
            r#"sol.media{ watch = true, ignore = { "kdeconnect" }, position_interval_ms = 250 }"#,
        );
        let configs: Vec<crate::services::media::Config> = scripts
            .startup()
            .commands
            .into_iter()
            .filter_map(|command| match command {
                Command::Media(config) => Some(config),
                _ => None,
            })
            .collect();
        assert_eq!(
            configs,
            [crate::services::media::Config {
                watch: true,
                ignore: vec!["kdeconnect".to_owned()],
                position_interval_ms: 250,
            }]
        );
        let mut snapshot = one_screen(&[]);
        let row = std::collections::BTreeMap::from([
            ("id", crate::json::Json::Number(3.0)),
            ("title", crate::json::Json::Text("Song".to_owned())),
            ("active", crate::json::Json::Bool(true)),
            ("playing", crate::json::Json::Bool(true)),
            ("rate", crate::json::Json::Number(1.0)),
            ("position", crate::json::Json::Number(10.0)),
            ("positionAt", crate::json::Json::Number(1000.0)),
        ]);
        snapshot.media = crate::services::media::Seen {
            view: std::rc::Rc::new(crate::services::media::View {
                available: true,
                active: 3,
                position_interval_ms: 1000,
                players: vec![row],
            }),
            now: std::time::Duration::from_secs(4),
        };
        assert_eq!(
            scripts.evaluate_in(
                snapshot,
                "local m = sol.media(); return m.active.title .. ' ' .. m.players[1].id .. ' ' .. m.active.position .. ' ' .. tostring(m.available)"
            ),
            "Song 3 13 true"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn media_events_reach_their_listeners_with_the_player() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-media-events",
            r#"sol.on("media", function(e) sol.status(e.what .. " " .. e.id .. " " .. e.player.title) end)"#,
        );
        let seen = crate::services::media::Seen {
            view: std::rc::Rc::new(crate::services::media::View {
                players: vec![std::collections::BTreeMap::from([
                    ("id", crate::json::Json::Number(3.0)),
                    ("title", crate::json::Json::Text("Song".to_owned())),
                ])],
                ..crate::services::media::View::default()
            }),
            now: std::time::Duration::ZERO,
        };
        let event = crate::services::media::Event {
            what: "active",
            id: 3,
        }
        .to_json(&seen);
        let outcome = scripts.media_event(&event, one_screen(&[]));
        assert_eq!(outcome.status.as_deref(), Some("active 3 Song"));
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_tray_action_is_refused_from_a_binding_at_the_lock() {
        let act = |action: &str| Command::Act {
            attempt: 0,
            action: action.to_owned(),
            data: crate::json::Json::Null,
            source: FocusSource::Binding,
        };
        assert!(act("tray.activate").reaches_past_the_lock());
        assert!(act("tray.open_menu").reaches_past_the_lock());
        assert!(!act("audio.mute").reaches_past_the_lock());
        // A media key plays and pauses at the lock: it reaches no window.
        assert!(!act("media.play_pause").reaches_past_the_lock());
    }

    #[test]
    fn sol_notify_queues_one_of_the_shells_own_with_its_actions() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-notify",
            r#"sol.on("surface", function()
                   sol.notify{ summary = "Battery low", body = "10% left", icon = "battery-caution", urgency = "critical",
                               actions = { { key = "open", label = "Settings", fn = function() sol.status("opened") end } } }
               end)"#,
        );
        let outcome =
            scripts.surface_action("shell", "go", &crate::json::Json::Null, one_screen(&[]));
        let owns: Vec<crate::services::notifications::Own> = outcome
            .commands
            .into_iter()
            .filter_map(|command| match command {
                Command::Notify(own) => Some(own),
                _ => None,
            })
            .collect();
        let [own] = owns.as_slice() else {
            panic!("one sol.notify: {owns:?}")
        };
        assert_eq!(own.summary, "Battery low");
        assert_eq!(own.icon, "battery-caution");
        assert_eq!(
            own.urgency,
            crate::services::notifications::model::Urgency::Critical
        );
        assert_eq!(own.actions, [("open".to_owned(), "Settings".to_owned())]);
        assert_ne!(own.token, 0);
        let ran = scripts.notify_action(own.token, "open", one_screen(&[]));
        assert_eq!(ran.status.as_deref(), Some("opened"));
        // Run once: the functions are let go after it.
        let again = scripts.notify_action(own.token, "open", one_screen(&[]));
        assert_eq!(again.status, None);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn sol_act_returns_an_attempt_and_done_hears_the_outcome_once() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-act",
            r#"sol.on("surface", function()
                   attempt = sol.act("windows.focus", { id = 9 }, function(ok, reason) sol.status(tostring(ok) .. " " .. tostring(reason)) end)
               end)"#,
        );
        let outcome =
            scripts.surface_action("shell", "go", &crate::json::Json::Null, one_screen(&[]));
        let acts: Vec<(u64, String, String)> = outcome
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::Act {
                    attempt,
                    action,
                    data,
                    ..
                } => Some((*attempt, action.clone(), data.render())),
                _ => None,
            })
            .collect();
        let [(attempt, action, data)] = acts.as_slice() else {
            panic!("one act: {acts:?}")
        };
        assert_eq!(
            scripts.evaluate("return tostring(attempt)"),
            attempt.to_string(),
            "sol.act answered another id than its command carries"
        );
        assert_eq!(
            (action.as_str(), data.as_str()),
            ("windows.focus", r#"{"id":9}"#)
        );
        let settled = [Settled {
            attempt: *attempt,
            ok: false,
            reason: Some("unknown-window"),
        }];
        assert_eq!(
            scripts
                .attempts_settled(&settled, one_screen(&[]))
                .status
                .as_deref(),
            Some("false unknown-window")
        );
        assert_eq!(
            scripts.attempts_settled(&settled, one_screen(&[])).status,
            None,
            "done was called twice"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// The `sol.run{}` requests among `outcome`'s commands.
    fn run_requests(outcome: &Outcome) -> Vec<crate::runs::Request> {
        outcome
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::RunRead(request) => Some(request.clone()),
                _ => None,
            })
            .collect()
    }

    /// Start `requests` as the compositor would, read them until each is
    /// done, and tell `scripts` what was heard after every read: the status
    /// each telling left, in order.
    fn run_to_the_end(scripts: &mut Scripts, requests: Vec<crate::runs::Request>) -> Vec<String> {
        let mut runs = crate::runs::Runs::default();
        let count = requests.len();
        for request in requests {
            let mut process = crate::launch::command(&request.argv[0]);
            process.args(&request.argv[1..]);
            runs.start(request, process, Duration::ZERO);
        }
        let started = std::time::Instant::now();
        let mut done = 0;
        let mut statuses = Vec::new();
        while done < count {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "the runs never ended"
            );
            let heard = runs.pump(Duration::ZERO);
            done += heard
                .iter()
                .filter(|each| matches!(each, crate::runs::Heard::Done { .. }))
                .count();
            if !heard.is_empty() {
                statuses.extend(scripts.runs_heard(&heard, one_screen(&[])).status);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        statuses
    }

    /// **`sol.run` with a table hands its output to `done`** (#472): the
    /// program's stdout, stderr, code and the rest, once, and its functions
    /// are let go. The variadic form is still fire and forget.
    #[test]
    fn sol_run_with_a_table_hands_its_output_to_done() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-run-done",
            r#"sol.run({ "sh", "-c", "echo out; echo err >&2; exit 4" }, {
                   done = function(r)
                       sol.status(r.stdout .. r.stderr .. tostring(r.code) .. " "
                           .. tostring(r.signal) .. " " .. tostring(r.timed_out) .. " "
                           .. tostring(r.truncated) .. " " .. tostring(r.error))
                   end,
               })
               sol.run("true")"#,
        );
        let outcome = scripts.startup();
        assert!(
            outcome.commands.iter().any(|command| matches!(
                command,
                Command::Run { program, args } if program == "true" && args.is_empty()
            )),
            "the variadic form changed: {:?}",
            outcome.commands
        );
        let requests = run_requests(&outcome);
        let [request] = requests.as_slice() else {
            panic!("one run: {requests:?}")
        };
        assert_eq!(
            (request.timeout, request.max_output, request.lines),
            (Duration::from_secs(30), 1 << 20, false),
            "the defaults"
        );
        assert_eq!(
            run_to_the_end(&mut scripts, requests),
            ["out\nerr\n4 nil false false nil"]
        );
        assert_eq!(
            scripts.evaluate("return tostring(next(sol._runs))"),
            "nil",
            "a done run's functions were kept"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **`sol.run` tells each line, then `done`** (#472): `(line, stream)`
    /// for every whole line, the last unterminated one at the end, and the
    /// options it was given reach the request.
    #[test]
    fn sol_run_tells_lines_then_done() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-run-lines",
            r#"local seen = {}
               sol.run({ "sh", "-c", "printf 'a\nb'; echo c >&2" }, {
                   timeout = 5000,
                   max_output = 4096,
                   cwd = "/",
                   env = { SOLIUM_A = "1", SOLIUM_B = false },
                   lines = function(line, stream) seen[#seen + 1] = stream .. ":" .. line end,
                   done = function(r) sol.status(table.concat(seen, ",") .. "|" .. r.stdout) end,
               })"#,
        );
        let requests = run_requests(&scripts.startup());
        let [request] = requests.as_slice() else {
            panic!("one run: {requests:?}")
        };
        assert_eq!(
            (
                request.timeout,
                request.max_output,
                request.lines,
                request.cwd.as_deref()
            ),
            (Duration::from_secs(5), 4096, true, Some("/"))
        );
        assert_eq!(
            request.env,
            [
                ("SOLIUM_A".to_owned(), Some("1".to_owned())),
                ("SOLIUM_B".to_owned(), None)
            ]
        );
        let statuses = run_to_the_end(&mut scripts, requests);
        assert_eq!(
            statuses.last().map(String::as_str),
            Some("stdout:a,stderr:c,stdout:b|a\nb")
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **`sol.run` takes the configured defaults, and a call overrides them**
    /// (#472): `config.run`'s `timeout` and `max_output`, through
    /// `sol.run_defaults`, are what a call that names neither gets; a value
    /// of the wrong kind keeps the built-in default.
    #[test]
    fn sol_run_takes_the_configured_defaults_and_a_call_overrides_them() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-run-defaults",
            r#"sol.run({ "true" })
               sol.run_defaults({ timeout = 5000, max_output = 10 })
               sol.run({ "true" })
               sol.run({ "true" }, { timeout = 7000, max_output = 20 })
               sol.run_defaults({ timeout = 0, max_output = "lots" })
               sol.run({ "true" })"#,
        );
        let taken: Vec<(Duration, usize)> = run_requests(&scripts.startup())
            .iter()
            .map(|request| (request.timeout, request.max_output))
            .collect();
        assert_eq!(
            taken,
            [
                (Duration::from_secs(30), 1 << 20),
                (Duration::from_secs(5), 10),
                (Duration::from_secs(7), 20),
                (Duration::from_secs(30), 1 << 20),
            ]
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **`sol.run` refuses what it cannot do**: an option it does not know, a
    /// timeout that is not above 0, an empty argv.
    #[test]
    fn sol_run_refuses_an_option_it_does_not_know() {
        let (directory, scripts) = loaded("solium-script-test-run-refuses", "");
        for (chunk, says) in [
            (
                r#"sol.run({ "true" }, { timout = 5 })"#,
                "no option `timout`",
            ),
            (r#"sol.run({ "true" }, { timeout = 0 })"#, "above 0"),
            (r#"sol.run({ "true" }, { max_output = -1 })"#, "max_output"),
            (r#"sol.run({}, {})"#, "needs a program"),
            (r#"sol.every(0, function() end)"#, "above 0"),
        ] {
            let answer = scripts.evaluate(&format!(
                "local ok, err = pcall(function() {chunk} end) return tostring(ok) .. ' ' .. tostring(err)"
            ));
            assert!(
                answer.starts_with("false ") && answer.contains(says),
                "{chunk}: {answer}"
            );
        }
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// Play `outcome`'s timer commands into `timers` at `now`.
    fn play_timers(timers: &mut crate::timers::Timers, outcome: &Outcome, now: Duration) {
        for command in &outcome.commands {
            match command {
                Command::Timer { id, after, period } => timers.add(*id, now, *after, *period),
                Command::TimerCancel(id) => timers.cancel(*id),
                _ => {}
            }
        }
    }

    /// Move a clock from 0 to `until` in `step`s, telling `scripts` the
    /// timers due at each, as `timers::settle` does.
    fn run_timers(
        scripts: &mut Scripts,
        timers: &mut crate::timers::Timers,
        until: u64,
        step: u64,
    ) {
        let mut at = 0;
        while at <= until {
            let now = Duration::from_millis(at);
            let due = timers.due(now);
            if !due.is_empty() {
                let outcome = scripts.timers_due(&due, one_screen(&[]));
                play_timers(timers, &outcome, now);
            }
            at += step;
        }
    }

    /// **Timers run in the order they fall due** (#472), and by creation
    /// among those due at once; `sol.after`'s once, `sol.every`'s each
    /// period.
    #[test]
    fn timers_run_in_the_order_they_fall_due() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-timers-order",
            r#"order = {}
               local function note(name) return function() order[#order + 1] = name end end
               sol.after(50, note("b"))
               sol.after(10, note("a"))
               sol.after(50, note("c"))
               sol.every(20, note("e"))"#,
        );
        let mut timers = crate::timers::Timers::default();
        play_timers(&mut timers, &scripts.startup(), Duration::ZERO);
        run_timers(&mut scripts, &mut timers, 60, 10);
        assert_eq!(
            scripts.evaluate("return table.concat(order, ',')"),
            "a,e,e,b,c,e"
        );
        assert_eq!(timers.len(), 1, "only the every is still set");
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A cancelled timer does not run** (#472): cancelled from a handler,
    /// or from another timer due at the same moment; and cancelling one that
    /// has run changes nothing.
    #[test]
    fn a_cancelled_timer_does_not_run() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-timers-cancel",
            r#"ran = {}
               local every = sol.every(10, function() ran[#ran + 1] = "every" end)
               local other
               sol.after(30, function() other:cancel() end)
               other = sol.after(30, function() ran[#ran + 1] = "other" end)
               local once = sol.after(5, function() ran[#ran + 1] = "once" end)
               sol.on("surface", function() every:cancel() once:cancel() end)"#,
        );
        let mut timers = crate::timers::Timers::default();
        play_timers(&mut timers, &scripts.startup(), Duration::ZERO);
        run_timers(&mut scripts, &mut timers, 20, 5);
        let outcome =
            scripts.surface_action("shell", "go", &crate::json::Json::Null, one_screen(&[]));
        play_timers(&mut timers, &outcome, Duration::from_millis(20));
        assert!(
            outcome
                .commands
                .iter()
                .all(|command| matches!(command, Command::TimerCancel(_))),
            "{:?}",
            outcome.commands
        );
        run_timers(&mut scripts, &mut timers, 100, 5);
        assert_eq!(
            scripts.evaluate("return table.concat(ran, ',')"),
            "once,every,every"
        );
        assert_eq!(timers.len(), 0);
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A timer stopped three times is cancelled** (#472): its function ran
    /// past the handler deadline each time, and the compositor is told to
    /// forget it.
    #[test]
    fn a_timer_stopped_three_times_is_cancelled() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-timers-stopped",
            "sol.every(10, function() while true do end end)",
        );
        let mut timers = crate::timers::Timers::default();
        play_timers(&mut timers, &scripts.startup(), Duration::ZERO);
        let mut cancelled = Vec::new();
        for round in 1..=3_u64 {
            let due = timers.due(Duration::from_millis(10 * round));
            assert_eq!(due.len(), 1, "round {round}");
            let outcome = scripts.timers_due(&due, one_screen(&[]));
            cancelled.push(
                outcome
                    .commands
                    .iter()
                    .any(|command| matches!(command, Command::TimerCancel(_))),
            );
            play_timers(&mut timers, &outcome, Duration::from_millis(10 * round));
        }
        assert_eq!(cancelled, [false, false, true]);
        assert_eq!(timers.len(), 0);
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// Play `outcome`'s file commands into `files`, as the compositor does.
    fn play_files(files: &mut crate::files::Files, outcome: &Outcome) {
        for command in &outcome.commands {
            match command.clone() {
                Command::FileRead(request) => files.read(request),
                Command::FileWrite(request) => files.write(request),
                Command::Watch { id, path, shown } => files.watch(id, &path, shown),
                Command::WatchCancel(id) => files.cancel_watch(id),
                Command::StoreLoaded { name, text } => files.loaded_store(name, text),
                _ => {}
            }
        }
    }

    /// Pump `files`, telling `scripts` what was heard and playing back what
    /// that asks for, until the script's global `seen` holds `count`
    /// entries: those, sorted.
    fn pump_files(
        scripts: &mut Scripts,
        files: &mut crate::files::Files,
        count: usize,
    ) -> Vec<String> {
        let started = std::time::Instant::now();
        let seen = |scripts: &Scripts| -> Vec<String> {
            let mut seen: Vec<String> = scripts
                .evaluate("return table.concat(seen, '\\n')")
                .split('\n')
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect();
            seen.sort();
            seen
        };
        while seen(scripts).len() < count {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "only {:?}",
                seen(scripts)
            );
            let heard = files.pump();
            if !heard.is_empty() {
                let outcome = scripts.files_heard(&heard, one_screen(&[]));
                play_files(files, &outcome);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        seen(scripts)
    }

    /// **`sol.read` hands the file to its function** (#473): a `/proc` file,
    /// whose size says 0, whole; a missing one, `nil` and the system's
    /// words; and the function is let go once told.
    #[test]
    fn sol_read_hands_the_file_to_its_function() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-read",
            r#"seen = {}
               sol.read("/proc/self/status", function(data, err)
                   seen[#seen + 1] = "proc " .. tostring(data:sub(1, 5)) .. " " .. tostring(err)
               end)
               sol.read("/nonexistent-solium-file", function(data, err)
                   seen[#seen + 1] = "missing " .. tostring(data) .. " " .. err
               end)"#,
        );
        let mut files = crate::files::Files::default();
        let outcome = scripts.startup();
        assert!(
            outcome.commands.iter().any(|command| matches!(
                command,
                Command::FileRead(request) if request.max == crate::files::MAX_READ
            )),
            "the default cap: {:?}",
            outcome.commands
        );
        play_files(&mut files, &outcome);
        assert_eq!(
            pump_files(&mut scripts, &mut files, 2),
            [
                "missing nil No such file or directory (os error 2)",
                "proc Name: nil"
            ]
        );
        assert_eq!(scripts.evaluate("return tostring(next(sol._files))"), "nil");
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **`sol.read` stops at `files.max_read`** (#473): the setting
    /// `sol.files` hands over is the cap, and past it the function is told
    /// so; a value of the wrong kind keeps the default.
    #[test]
    fn sol_read_stops_at_files_max_read() {
        let directory = std::env::temp_dir().join("solium-script-test-read-cap");
        let _ = std::fs::create_dir_all(&directory);
        std::fs::write(directory.join("ten"), "0123456789").expect("writing");
        let ten = directory.join("ten");
        let (_, mut scripts) = loaded(
            "solium-script-test-read-cap",
            &format!(
                r#"seen = {{}}
                   local function note(data, err) seen[#seen + 1] = tostring(data) .. " " .. tostring(err) end
                   sol.files({{ max_read = 4 }})
                   sol.read("{ten}", note)
                   sol.files({{ max_read = 10 }})
                   sol.read("{ten}", note)
                   sol.files({{ max_read = "lots" }})
                   sol.read("{ten}", function() end)"#,
                ten = ten.display()
            ),
        );
        let outcome = scripts.startup();
        let caps: Vec<usize> = outcome
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::FileRead(request) => Some(request.max),
                _ => None,
            })
            .collect();
        assert_eq!(caps, [4, 10, crate::files::MAX_READ]);
        let mut files = crate::files::Files::default();
        play_files(&mut files, &outcome);
        assert_eq!(
            pump_files(&mut scripts, &mut files, 2),
            ["0123456789 nil", "nil longer than files.max_read"]
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **`sol.write` writes, and tells its function**: `nil` when the file
    /// is written, the error when its directory is missing; one without a
    /// function is still written, and kept nowhere.
    #[test]
    fn sol_write_writes_and_tells_its_function() {
        let directory = std::env::temp_dir().join("solium-script-test-write");
        let _ = std::fs::remove_dir_all(&directory);
        let (_, mut scripts) = loaded(
            "solium-script-test-write",
            &format!(
                r#"seen = {{}}
                   sol.write("{dir}/a", "first", function(err) seen[#seen + 1] = "a " .. tostring(err) end)
                   sol.write("{dir}/b", "quiet")
                   sol.write("{dir}/no/c", "x", function(err) seen[#seen + 1] = "c " .. tostring(err) end)"#,
                dir = directory.display()
            ),
        );
        let outcome = scripts.startup();
        assert_eq!(
            scripts.evaluate(
                "local n = 0 for _ in pairs(sol._files) do n = n + 1 end return tostring(n)"
            ),
            "2",
            "a write with no function was kept"
        );
        let mut files = crate::files::Files::default();
        play_files(&mut files, &outcome);
        assert_eq!(
            pump_files(&mut scripts, &mut files, 2),
            ["a nil", "c No such file or directory (os error 2)"]
        );
        let started = std::time::Instant::now();
        while files.writing() > 0 {
            assert!(started.elapsed() < Duration::from_secs(10));
            files.pump();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            std::fs::read_to_string(directory.join("a")).ok().as_deref(),
            Some("first")
        );
        assert_eq!(
            std::fs::read_to_string(directory.join("b")).ok().as_deref(),
            Some("quiet")
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **The file functions refuse what they cannot do** (#473): a relative
    /// path, a watch on a missing directory, a store's bad name.
    #[test]
    fn the_file_functions_refuse_what_they_cannot_do() {
        let (directory, scripts) = loaded("solium-script-test-files-refuse", "");
        for (chunk, says) in [
            (
                r#"sol.read("relative", function() end)"#,
                "neither absolute",
            ),
            (r#"sol.write("./x", "y")"#, "neither absolute"),
            (r#"sol.watch("x", function() end)"#, "neither absolute"),
            (
                r#"sol.watch("/nonexistent-solium-dir/x", function() end)"#,
                "is not a directory",
            ),
            (r#"sol.store("../up", {})"#, "not a store's name"),
            (r#"sol.store("", {})"#, "not a store's name"),
        ] {
            let answer = scripts.evaluate(&format!(
                "local ok, err = pcall(function() {chunk} end) return tostring(ok) .. ' ' .. tostring(err)"
            ));
            assert!(
                answer.starts_with("false ") && answer.contains(says),
                "{chunk}: {answer}"
            );
        }
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A watch tells its function, and a cancelled one is not told**
    /// (#473): its event carries the kind and the path as it was given, and
    /// after `cancel` the compositor is told to forget it.
    #[test]
    fn a_cancelled_watch_is_not_told() {
        let directory = std::env::temp_dir().join("solium-script-test-watch");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a directory");
        let (_, mut scripts) = loaded(
            "solium-script-test-watch-config",
            &format!(
                r#"seen = {{}}
                   watch = sol.watch("{dir}/f", function(event)
                       seen[#seen + 1] = event.kind .. " " .. event.path
                   end)
                   sol.on("surface", function() watch:cancel() watch:cancel() end)"#,
                dir = directory.display()
            ),
        );
        let mut files = crate::files::Files::default();
        play_files(&mut files, &scripts.startup());
        assert_eq!(files.watching(), 1);
        std::fs::write(directory.join("f"), "1").expect("writing");
        pump_files(&mut scripts, &mut files, 2);
        assert_eq!(
            scripts.evaluate("return table.concat(seen, ',')"),
            format!("created {dir}/f,changed {dir}/f", dir = directory.display())
        );
        let outcome =
            scripts.surface_action("shell", "go", &crate::json::Json::Null, one_screen(&[]));
        assert!(
            matches!(&outcome.commands[..], [Command::WatchCancel(_)]),
            "{:?}",
            outcome.commands
        );
        play_files(&mut files, &outcome);
        assert_eq!(files.watching(), 0);
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A watch whose directory goes is told `removed` and let go at once**
    /// (#473): its function leaves `sol._watches` without waiting for a
    /// reload.
    #[test]
    fn a_watch_whose_directory_goes_is_told_removed_and_let_go() {
        let directory = std::env::temp_dir().join("solium-script-test-watch-gone");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a directory");
        let (config, mut scripts) = loaded(
            "solium-script-test-watch-gone-config",
            &format!(
                r#"seen = {{}}
                   sol.watch("{dir}/f", function(event) seen[#seen + 1] = event.kind end)"#,
                dir = directory.display()
            ),
        );
        let mut files = crate::files::Files::default();
        play_files(&mut files, &scripts.startup());
        std::fs::remove_dir_all(&directory).expect("removing the directory");
        assert_eq!(pump_files(&mut scripts, &mut files, 1), ["removed"]);
        let started = std::time::Instant::now();
        while scripts.evaluate("return tostring(next(sol._watches))") != "nil" {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the function was kept"
            );
            let heard = files.pump();
            if !heard.is_empty() {
                play_files(&mut files, &scripts.files_heard(&heard, one_screen(&[])));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(files.watching(), 0);
        assert_eq!(
            scripts.evaluate("return table.concat(seen, ',')"),
            "removed"
        );
        let _ = std::fs::remove_dir_all(&config);
    }

    /// **`sol.files` reads the store settings** (#473): `store_interval` is
    /// what the settle asks for, and `store_max` caps a store file; values
    /// of the wrong kind keep the defaults.
    #[test]
    fn sol_files_reads_the_store_settings() {
        let state = crate::files::tests::scratch("script-store-settings");
        crate::files::tests::use_state_dir(&state);
        std::fs::write(state.join("big.json"), r#"{"a": "0123456789"}"#).expect("writing");
        let (directory, scripts) = loaded(
            "solium-script-test-store-settings",
            r#"sol.files({ store_interval = 5000, store_max = 10 })
               big = sol.store("big", { a = "default" })"#,
        );
        assert_eq!(scripts.store_interval(), Duration::from_secs(5));
        assert_eq!(scripts.evaluate("return big.a"), "default");
        assert!(
            state.join("big.json.broken").exists(),
            "past store_max was read"
        );
        scripts.evaluate("sol.files({ store_interval = 0, store_max = -1 }) return ''");
        assert_eq!(scripts.store_interval(), crate::files::STORE_INTERVAL);
        let _ = std::fs::remove_dir_all(&directory);
        let _ = std::fs::remove_dir_all(&state);
    }

    /// **A store takes the file's keys and keeps the defaults it lacks**
    /// (#473), inside nested tables too, takes a list whole, and is the same
    /// table when asked for twice; the text it loaded is handed over so it
    /// is not saved again unchanged.
    #[test]
    fn a_store_takes_the_files_keys_and_keeps_the_defaults_it_lacks() {
        let state = crate::files::tests::scratch("script-store-fill");
        crate::files::tests::use_state_dir(&state);
        std::fs::write(
            state.join("prefs.json"),
            r#"{"volume": 70, "ui": {"dark": true}, "recent": ["a"], "gone": 1}"#,
        )
        .expect("writing");
        let (directory, mut scripts) = loaded(
            "solium-script-test-store-fill",
            r#"local s = sol.store("prefs", { volume = 50, muted = false,
                   ui = { dark = false, scale = 2 }, recent = { "x", "y", "z" } })
               same = sol.store("prefs") == s
               shown = string.format("%d %s %s %d %d %s %d", s.volume, tostring(s.muted),
                   tostring(s.ui.dark), s.ui.scale, #s.recent, s.recent[1], s.gone)"#,
        );
        assert_eq!(scripts.evaluate("return tostring(same)"), "true");
        assert_eq!(scripts.evaluate("return shown"), "70 false true 2 1 a 1");
        let outcome = scripts.startup();
        assert!(
            outcome.commands.iter().any(|command| matches!(
                command,
                Command::StoreLoaded { name, text }
                    if name == "prefs" && text == r#"{"gone":1,"recent":["a"],"ui":{"dark":true},"volume":70}"#
            )),
            "{:?}",
            outcome.commands
        );
        let _ = std::fs::remove_dir_all(&directory);
        let _ = std::fs::remove_dir_all(&state);
    }

    /// **A store survives a reload as the data it saves** (#473): carried
    /// as its plain data, not read from the file again, with a function in
    /// it left out; and one the new configuration does not ask for is still
    /// carried, for a later one that does.
    #[test]
    fn a_store_survives_a_reload_as_the_data_it_saves() {
        let state = crate::files::tests::scratch("script-store-reload");
        crate::files::tests::use_state_dir(&state);
        let (directory, scripts) = loaded(
            "solium-script-test-store-reload",
            r#"local s = sol.store("prefs", { volume = 50 })
               s.volume = 65
               s.run = function() end
               sol.store("other", {}).kept = "yes""#,
        );
        assert_eq!(
            scripts
                .stores()
                .iter()
                .map(|(name, json)| format!("{name}={}", json.render()))
                .collect::<Vec<_>>(),
            [r#"other={"kept":"yes"}"#, r#"prefs={"volume":65}"#]
        );
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"local s = sol.store("prefs", { volume = 50 })
               shown = tostring(s.volume) .. " " .. tostring(s.run)"#,
        )
        .expect("writing");
        let scripts = Scripts::load_carrying(&config, scripts.kept()).expect("reloading");
        assert_eq!(scripts.evaluate("return shown"), "65 nil");
        std::fs::write(&config, r#"shown = sol.store("other", {}).kept"#).expect("writing");
        let scripts = Scripts::load_carrying(&config, scripts.kept()).expect("reloading");
        assert_eq!(scripts.evaluate("return shown"), "yes");
        assert!(
            std::fs::read_dir(&state)
                .map(|mut dir| dir.next().is_none())
                .unwrap_or(true),
            "a reload read or wrote the state directory itself"
        );
        let _ = std::fs::remove_dir_all(&directory);
        let _ = std::fs::remove_dir_all(&state);
    }

    /// **`sol.workspaces` declares groups and windows**, every field as Lua
    /// wrote it.
    #[test]
    fn sol_workspaces_declares_groups_and_windows() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-workspaces",
            r#"sol.workspaces({
                arrangement = { kind = "grid", columns = 3, rows = 2 },
                groups = { { id = "DP-1", monitors = { "DP-1" }, showing = { "2" },
                             workspaces = { { id = "1", name = "1", col = 1, row = 1 }, { id = "2", name = "web", col = 2, row = 1 },
                                            { id = "scratch", hidden = true } } } },
                windows = { [42] = { "2" } },
            })"#,
        );
        let declared: Vec<crate::models::workspaces::Declared> = scripts
            .startup()
            .commands
            .into_iter()
            .filter_map(|command| match command {
                Command::Workspaces(declared) => Some(declared),
                _ => None,
            })
            .collect();
        let [declared] = declared.as_slice() else {
            panic!("one declaration: {declared:?}")
        };
        assert_eq!(
            (
                declared.arrangement.kind.as_str(),
                declared.arrangement.columns,
                declared.arrangement.rows
            ),
            ("grid", 3, 2)
        );
        assert_eq!(declared.groups[0].showing, vec!["2".to_owned()]);
        assert_eq!(declared.groups[0].workspaces[1].name, "web");
        assert_eq!(
            declared.groups[0].workspaces[2],
            crate::models::workspaces::Workspace {
                id: "scratch".to_owned(),
                name: "scratch".to_owned(),
                col: 1,
                row: 1,
                hidden: true,
            },
            "a workspace declared with only its id and hidden"
        );
        assert_eq!(declared.windows.get(&42), Some(&vec!["2".to_owned()]));
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **The shipped `workspaces.lua` declares its workspaces**: one group per
    /// monitor when workspaces are per monitor, showing what each monitor shows.
    #[test]
    fn the_shipped_workspaces_declare_what_each_monitor_shows() {
        let Some((scripts, commands)) = shell_after_monitors(
            "solium-script-test-shipped-workspaces",
            "return { workspaces = { per_monitor = true } }",
        ) else {
            return;
        };
        let _ = scripts;
        let last = commands.iter().rev().find_map(|command| match command {
            Command::Workspaces(declared) => Some(declared.clone()),
            _ => None,
        });
        let declared = last.expect("workspaces.lua declared nothing");
        assert_eq!(
            declared
                .groups
                .iter()
                .map(|group| group.id.as_str())
                .collect::<Vec<_>>(),
            vec!["test-1"]
        );
        assert_eq!(declared.groups[0].showing, vec!["1".to_owned()]);
    }

    /// **With workspaces not per monitor, one group has every monitor**, and
    /// shows what every monitor shows.
    #[test]
    fn with_workspaces_together_one_group_has_every_monitor() {
        let Some((directory, mut scripts)) = shipped_init_with_user(
            "solium-script-test-workspaces-together",
            "return { workspaces = { per_monitor = false } }",
        ) else {
            return;
        };
        let _ = scripts.startup();
        let mut two = one_screen(&[]);
        let mut right = two.monitors[0].clone();
        right.name = "test-2".to_owned();
        right.whole.x = 1600.0;
        right.area.x = 1600.0;
        right.focused = false;
        two.monitors.push(right);
        let outcome = scripts.monitors_changed(two);
        let groups = outcome
            .commands
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::Workspaces(declared) => Some(
                    declared
                        .groups
                        .iter()
                        .map(|group| (group.monitors.clone(), group.showing.clone()))
                        .collect::<Vec<_>>(),
                ),
                _ => None,
            });
        assert_eq!(
            groups,
            Some(vec![(
                vec!["test-1".to_owned(), "test-2".to_owned()],
                vec!["1".to_owned()]
            )]),
            "(monitors, showing) of each group"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **`workspaces.go` from a scene switches the monitor it names**, through
    /// `actions.lua` to `workspaces.lua`, which then declares the new state.
    /// An id that is no workspace's number, `2.5` or none at all, switches
    /// nothing and is logged.
    #[test]
    fn a_workspaces_go_from_a_scene_switches_the_monitor_it_names() {
        let Some((directory, mut scripts)) = shipped_init_with_user(
            "solium-script-test-workspaces-go",
            "return { workspaces = { per_monitor = true } }",
        ) else {
            return;
        };
        // Two monitors, the pointer on the left one: the right one is named,
        // so it is the one that switches.
        let two = || {
            let mut snapshot = one_screen(&[]);
            let mut right = snapshot.monitors[0].clone();
            right.name = "test-2".to_owned();
            right.whole.x = 1600.0;
            right.area.x = 1600.0;
            right.focused = false;
            snapshot.monitors.push(right);
            snapshot
        };
        let _ = scripts.startup();
        let _ = scripts.monitors_changed(two());
        let go = |scripts: &mut Scripts, data: &str| {
            let data = crate::json::Json::parse(data).expect("valid JSON");
            scripts
                .surface_action("shell", "workspaces.go", &data, two())
                .commands
                .iter()
                .rev()
                .find_map(|command| match command {
                    Command::Workspaces(declared) => Some(
                        declared
                            .groups
                            .iter()
                            .map(|group| (group.id.clone(), group.showing.clone()))
                            .collect::<Vec<_>>(),
                    ),
                    _ => None,
                })
        };
        let mut others = Vec::new();
        let log = logged_while(|| {
            for data in [
                r#"{"id":2.5,"monitor":"test-2"}"#,
                r#"{"monitor":"test-2"}"#,
            ] {
                others.push(go(&mut scripts, data));
            }
        });
        let showing = go(&mut scripts, r#"{"id":"2","monitor":"test-2"}"#);
        assert_eq!(
            (
                showing,
                others,
                log.matches("workspaces.go: answered only as").count()
            ),
            (
                Some(vec![
                    ("test-1".to_owned(), vec!["1".to_owned()]),
                    ("test-2".to_owned(), vec!["2".to_owned()])
                ]),
                vec![None, None],
                2
            ),
            "(group, showing) after the monitor named switches, not the one in front; \
             declared after each other form; lines logged"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A window that opens is declared on the workspace its monitor
    /// shows**, at the layout that places it, so a hosted shell counts it
    /// at once.
    #[test]
    fn a_window_that_opens_is_declared_on_the_workspace_its_monitor_shows() {
        let Some((directory, mut scripts)) = shipped_init_with_user(
            "solium-script-test-workspaces-layout",
            "return { workspaces = { per_monitor = true } }",
        ) else {
            return;
        };
        let _ = scripts.startup();
        let _ = scripts.monitors_changed(one_screen(&[]));
        let _ = scripts.opened(9, one_screen(&[9]));
        let outcome = scripts.relayout(one_screen(&[9]));
        let windows = outcome
            .commands
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::Workspaces(declared) => Some(declared.windows.clone()),
                _ => None,
            });
        assert_eq!(
            windows,
            Some(std::collections::BTreeMap::from([(
                9,
                vec!["1".to_owned()]
            )]))
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A window that opens while floating (the default mode) is declared
    /// on the workspace its monitor shows too, with no `relayout` to ask for
    /// it**: `"open"` is the only event a floating window ever sees, since
    /// nothing arranges it, so a hosted shell reading `Workspaces`/
    /// `WindowList` must learn its workspace from `"open"` alone, not only
    /// from the `"layout"` a tiling mode would have fired next (a bar that
    /// read `Workspaces.showing(monitor).id` never saw a window opened this
    /// way).
    #[test]
    fn a_window_that_opens_while_floating_is_declared_with_no_relayout_to_ask_for_it() {
        let Some((directory, mut scripts)) = shipped_init_with_user(
            "solium-script-test-workspaces-floating-open",
            "return { workspaces = { per_monitor = true } }",
        ) else {
            return;
        };
        let _ = scripts.startup();
        let _ = scripts.monitors_changed(one_screen(&[]));
        // Only `opened`, exactly what a floating window gets: nothing here
        // asks for a layout.
        let outcome = scripts.opened(9, one_screen(&[9]));
        let windows = outcome
            .commands
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::Workspaces(declared) => Some(declared.windows.clone()),
                _ => None,
            });
        assert_eq!(
            windows,
            Some(std::collections::BTreeMap::from([(
                9,
                vec!["1".to_owned()]
            )])),
            "a floating window's open must declare its workspace by itself"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **The declared arrangement is the shape the workspaces make**: the
    /// shipped row of four is four by one, not the grid's `rows`; a column
    /// is one wide; and a row of none is the one workspace there is.
    #[test]
    fn the_declared_arrangement_is_the_shape_of_its_workspaces() {
        let mut shapes = Vec::new();
        for (name, user) in [
            ("row", "return {}"),
            (
                "column",
                r#"return { workspaces = { arrangement = "vertical", rows = 3 } }"#,
            ),
            ("none", "return { workspaces = { columns = 0 } }"),
        ] {
            let Some((_scripts, commands)) =
                shell_after_monitors(&format!("solium-script-test-workspaces-shape-{name}"), user)
            else {
                return;
            };
            let declared = commands
                .iter()
                .rev()
                .find_map(|command| match command {
                    Command::Workspaces(declared) => Some(declared.clone()),
                    _ => None,
                })
                .expect("workspaces.lua declared nothing");
            let last = declared.groups[0]
                .workspaces
                .last()
                .map(|workspace| (workspace.col, workspace.row));
            shapes.push((
                declared.arrangement.kind,
                declared.arrangement.columns,
                declared.arrangement.rows,
                last,
            ));
        }
        assert_eq!(
            shapes,
            vec![
                ("horizontal".to_owned(), 4, 1, Some((4, 1))),
                ("vertical".to_owned(), 1, 3, Some((1, 3))),
                ("horizontal".to_owned(), 1, 1, Some((1, 1))),
            ],
            "(kind, columns, rows, the last workspace's cell)"
        );
    }

    /// **`windows.send` from a scene moves the window it names**, not the
    /// focused one, through `actions.lua` to `workspaces.lua`, which then
    /// declares where every window is.
    #[test]
    fn a_windows_send_from_a_scene_moves_the_window_it_names() {
        let Some((directory, mut scripts)) = shipped_init_with_user(
            "solium-script-test-windows-send",
            "return { workspaces = { per_monitor = true } }",
        ) else {
            return;
        };
        let _ = scripts.startup();
        let _ = scripts.monitors_changed(one_screen(&[7, 8]));
        let data = crate::json::Json::parse(r#"{"id":7,"workspace":"3"}"#).expect("valid JSON");
        let outcome = scripts.surface_action("shell", "windows.send", &data, one_screen(&[7, 8]));
        let windows = outcome
            .commands
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::Workspaces(declared) => Some(declared.windows.clone()),
                _ => None,
            });
        assert_eq!(
            windows,
            Some(std::collections::BTreeMap::from([
                (7, vec!["3".to_owned()]),
                (8, vec!["1".to_owned()])
            ])),
            "window 7 sent to 3, window 8 left on the one its monitor shows"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **`windows.send` names its window by number or by digits, and logs
    /// any other form**: `{ id: "8" }` sends window 8 as `{ id: 8 }` would,
    /// while `{ id, monitor }`, a window that is not open, no id at all and
    /// a workspace that is no workspace's number, `2.5`, each move nothing
    /// and say so.
    #[test]
    fn a_windows_send_names_its_window_by_number_or_digits_and_logs_any_other_form() {
        let Some((directory, mut scripts)) = shipped_init_with_user(
            "solium-script-test-windows-send-forms",
            "return { workspaces = { per_monitor = true } }",
        ) else {
            return;
        };
        let _ = scripts.startup();
        let _ = scripts.monitors_changed(one_screen(&[7, 8]));
        let send = |scripts: &mut Scripts, data: &str| {
            let data = crate::json::Json::parse(data).expect("valid JSON");
            scripts
                .surface_action("shell", "windows.send", &data, one_screen(&[7, 8]))
                .commands
                .iter()
                .rev()
                .find_map(|command| match command {
                    Command::Workspaces(declared) => Some(declared.windows.clone()),
                    _ => None,
                })
        };
        let digits = send(&mut scripts, r#"{"id":"8","workspace":"2"}"#);
        let mut others = Vec::new();
        let log = logged_while(|| {
            for data in [
                r#"{"id":7,"monitor":"test-1"}"#,
                r#"{"id":99,"workspace":"2"}"#,
                r#"{"workspace":"2"}"#,
                r#"{"id":7,"workspace":2.5}"#,
            ] {
                others.push(send(&mut scripts, data));
            }
        });
        assert_eq!(
            (
                digits,
                others,
                log.matches("windows.send: answered only as").count()
            ),
            (
                Some(std::collections::BTreeMap::from([
                    (7, vec!["1".to_owned()]),
                    (8, vec!["2".to_owned()])
                ])),
                vec![None, None, None, None],
                4
            ),
            "(declared after an id in digits, declared after each other form, lines logged)"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **`workspaces.lua` does not route a scene's actions by itself**: a
    /// configuration without `actions.lua` routes them its own way, and
    /// keeping the workspaces does not route each one a second time.
    #[test]
    fn workspaces_lua_does_not_route_a_scenes_actions_by_itself() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-workspaces-no-actions",
            &format!(
                r#"package.path = {shipped:?} .. "/?.lua"
require("workspaces")
sol.on("surface", function(_, action, data) sol.act(action, data) end)"#,
                shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
            ),
        );
        let data = crate::json::Json::parse(r#"{"id":4}"#).expect("valid JSON");
        let outcome = scripts.surface_action("shell", "windows.close", &data, one_screen(&[4]));
        let acts = outcome
            .commands
            .iter()
            .filter(|command| matches!(command, Command::Act { .. }))
            .count();
        assert_eq!(acts, 1, "windows.close was routed more than once, or never");
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **`actions.lua` routes the vocabulary and leaves the rest alone**: a
    /// `windows.*` action becomes a `sol.act`, one a file overrides is
    /// answered in Lua instead, and one outside the vocabulary, a tweak's
    /// id, is nobody's but its surface's listener's.
    #[test]
    fn actions_lua_routes_the_vocabulary_and_leaves_the_rest_alone() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-actions-lua",
            &format!(
                r#"package.path = {shipped:?} .. "/?.lua"
local actions = require("actions")
actions.override("windows.focus", function(data, surface) sol.status("mine " .. data.id .. " " .. surface) end)"#,
                shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
            ),
        );
        let acts = |outcome: &Outcome| -> Vec<(String, String)> {
            outcome
                .commands
                .iter()
                .filter_map(|command| match command {
                    Command::Act { action, data, .. } => Some((action.clone(), data.render())),
                    _ => None,
                })
                .collect()
        };
        let id =
            |id: u64| crate::json::Json::parse(&format!(r#"{{"id":{id}}}"#)).expect("valid JSON");
        let closed = scripts.surface_action("shell", "windows.close", &id(4), one_screen(&[]));
        let focused = scripts.surface_action("shell", "windows.focus", &id(3), one_screen(&[]));
        let tweaked = scripts.surface_action(
            "tweaks",
            "pane:border",
            &crate::json::Json::Null,
            one_screen(&[]),
        );
        assert_eq!(
            (
                acts(&closed),
                (acts(&focused), focused.status.as_deref()),
                acts(&tweaked)
            ),
            (
                vec![("windows.close".to_owned(), r#"{"id":4}"#.to_owned())],
                (Vec::new(), Some("mine 3 shell")),
                Vec::new()
            ),
            "(the vocabulary's act, (an overridden one's acts, what its handler said), \
             a tweak's acts)"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **An override stopped three times is taken out alone**: it is a
    /// `surface` listener of its own, so the stops are its and not the
    /// router's. Every other action is still routed, and its own action
    /// goes to the compositor again.
    #[test]
    fn an_override_stopped_three_times_is_taken_out_alone() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-actions-override-stopped",
            &format!(
                r#"package.path = {shipped:?} .. "/?.lua"
local actions = require("actions")
runs = 0
actions.override("windows.close", function() runs = runs + 1; while true do end end)"#,
                shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
            ),
        );
        let acts = |outcome: &Outcome| -> Vec<(String, String)> {
            outcome
                .commands
                .iter()
                .filter_map(|command| match command {
                    Command::Act { action, data, .. } => Some((action.clone(), data.render())),
                    _ => None,
                })
                .collect()
        };
        let id =
            |id: u64| crate::json::Json::parse(&format!(r#"{{"id":{id}}}"#)).expect("valid JSON");
        let stopped: Vec<Vec<(String, String)>> = (0..3)
            .map(|_| {
                acts(&scripts.surface_action("shell", "windows.close", &id(4), one_screen(&[])))
            })
            .collect();
        let focused = scripts.surface_action("shell", "windows.focus", &id(3), one_screen(&[]));
        let closed = scripts.surface_action("shell", "windows.close", &id(5), one_screen(&[]));
        assert_eq!(
            (
                stopped,
                acts(&focused),
                acts(&closed),
                scripts.evaluate("return tostring(runs)")
            ),
            (
                vec![Vec::new(), Vec::new(), Vec::new()],
                vec![("windows.focus".to_owned(), r#"{"id":3}"#.to_owned())],
                vec![("windows.close".to_owned(), r#"{"id":5}"#.to_owned())],
                "3".to_owned()
            ),
            "(the acts of the three stopped closes, another action's acts, \
             the same action's acts once the override is out, how many times it ran)"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **An override of `nil` gives its action back to the compositor**, as
    /// it did when the overrides were a table: no error, and `sol.act` hears
    /// the action.
    #[test]
    fn an_override_of_nil_gives_its_action_back_to_the_compositor() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-actions-override-nil",
            &format!(
                r#"package.path = {shipped:?} .. "/?.lua"
local actions = require("actions")
actions.override("windows.close", function() heard = true end)
actions.override("windows.close", nil)"#,
                shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
            ),
        );
        let data = crate::json::Json::parse(r#"{"id":4}"#).expect("valid JSON");
        let mut outcome = None;
        let log = logged_while(|| {
            outcome =
                Some(scripts.surface_action("shell", "windows.close", &data, one_screen(&[])));
        });
        let acts: Vec<(String, String)> = outcome
            .iter()
            .flat_map(|outcome| outcome.commands.iter())
            .filter_map(|command| match command {
                Command::Act { action, data, .. } => Some((action.clone(), data.render())),
                _ => None,
            })
            .collect();
        assert_eq!(
            (
                scripts.evaluate("return tostring(heard)"),
                acts,
                log.contains("failed")
            ),
            (
                "nil".to_owned(),
                vec![("windows.close".to_owned(), r#"{"id":4}"#.to_owned())],
                false
            ),
            "(whether the replaced override heard it, the acts queued, whether a listener failed):\n{log}"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A stopped override is logged where it was stopped**: the listener
    /// struck is `actions.lua`'s own, so the log also names the file and line
    /// the handler's time ran out at, which is the override's.
    #[test]
    fn a_stopped_override_is_logged_where_it_was_stopped() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-actions-override-where",
            &format!(
                r#"package.path = {shipped:?} .. "/?.lua"
local actions = require("actions")
actions.override("windows.close", function()
    while true do end
end)"#,
                shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
            ),
        );
        let data = crate::json::Json::parse(r#"{"id":4}"#).expect("valid JSON");
        let log = logged_while(|| {
            let _ = scripts.surface_action("shell", "windows.close", &data, one_screen(&[]));
        });
        let at = format!("stopped_at={}:4", directory.join("init.lua").display());
        assert!(
            log.lines()
                .any(|line| line.contains("was stopped") && line.contains(&at)),
            "the stop names {at}:\n{log}"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A stopped binding is logged where it was written and where it was
    /// stopped**, as a stopped listener is, and is left unhandled, as a
    /// binding that fails is.
    #[test]
    fn a_stopped_binding_is_logged_where_it_was_written_and_stopped() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-binding-where",
            "sol.bind(\"super+x\", function()\n    while true do end\nend)",
        );
        let mut handled = None;
        let log = logged_while(|| {
            handled = Some(scripts.key("super+x", one_screen(&[])).handled);
        });
        let init = directory.join("init.lua");
        let (written, stopped) = (
            format!("file={} line=1", init.display()),
            format!("stopped_at={}:2", init.display()),
        );
        assert_eq!(
            (
                handled,
                log.lines().any(|line| line.contains("was stopped")
                    && line.contains("event=\"binding\"")
                    && line.contains(&written)
                    && line.contains(&stopped)),
            ),
            (Some(false), true),
            "(whether the key was handled, whether the stop names {written} and {stopped}):\n{log}"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A handler stopped when its `sol.focus_direction` returns is logged
    /// at that call**, not with no place, nor at a `direction` listener's
    /// line.
    #[test]
    fn a_handler_stopped_at_its_focus_direction_is_logged_at_that_call() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-direction-where",
            "sol.on(\"direction\", function() end)\nsol.on(\"layout\", function()\n    while true do\n        sol.focus_direction(\"left\")\n    end\nend)",
        );
        let log = logged_while(|| {
            let _ = scripts.relayout(one_screen(&[]));
        });
        // The error carries a traceback here, so the fields are on a line
        // of their own.
        let at = format!("stopped_at={}:4", directory.join("init.lua").display());
        assert!(
            log.contains("was stopped")
                && log
                    .lines()
                    .any(|line| line.contains("event=\"layout\"") && line.contains(&at)),
            "the stop names {at}:\n{log}"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A later override of the same name replaces the earlier one**: only
    /// the last answers, and the action does not also go to the compositor.
    #[test]
    fn a_later_override_of_the_same_name_replaces_the_earlier_one() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-actions-override-replaced",
            &format!(
                r#"package.path = {shipped:?} .. "/?.lua"
local actions = require("actions")
heard = ""
actions.override("windows.close", function() heard = heard .. "first;" end)
actions.override("windows.close", function() heard = heard .. "second;" end)"#,
                shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
            ),
        );
        let data = crate::json::Json::parse(r#"{"id":4}"#).expect("valid JSON");
        let outcome = scripts.surface_action("shell", "windows.close", &data, one_screen(&[]));
        assert_eq!(
            (
                scripts.evaluate("return heard"),
                format!("{:?}", outcome.commands)
            ),
            ("second;".to_owned(), "[]".to_owned()),
            "(which overrides answered, the commands queued)"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **Each `sol.act` answers an id of its own, counting up**, with or
    /// without a `done`, and data that is no table reaches the command as
    /// it is.
    #[test]
    fn each_sol_act_answers_an_id_of_its_own_counting_up() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-act-ids",
            r#"sol.on("surface", function()
                   first = sol.act("windows.close", { id = 1 })
                   second = sol.act("windows.close", 2)
               end)"#,
        );
        let outcome =
            scripts.surface_action("shell", "go", &crate::json::Json::Null, one_screen(&[]));
        let acts: Vec<(u64, String)> = outcome
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::Act { attempt, data, .. } => Some((*attempt, data.render())),
                _ => None,
            })
            .collect();
        let said = |name: &str| -> u64 {
            scripts
                .evaluate(&format!("return tostring({name})"))
                .parse()
                .unwrap_or(0)
        };
        let (first, second) = (said("first"), said("second"));
        assert_eq!(
            (second > first, acts),
            (
                true,
                vec![(first, r#"{"id":1}"#.to_owned()), (second, "2".to_owned())]
            ),
            "(the second id after the first, the commands with their ids and data)"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **Data that contains itself is an error in the handler, not a crash
    /// of the compositor**: `sol.act`, `sol.surface`'s properties and
    /// `sol.pane_values` each raise an ordinary Lua error, which the handler
    /// can catch, and nothing is queued.
    #[test]
    fn data_that_contains_itself_is_an_error_in_the_handler() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-data-contains-itself",
            r#"sol.on("surface", function()
                   local t = {} t.t = t
                   local said = {}
                   for _, call in ipairs({
                       function() sol.act("windows.focus", t) end,
                       function() sol.surface("bar", { scene = "/nonexistent/bar.qml", properties = t }) end,
                       function() sol.pane_values(t) end,
                   }) do
                       local ok, err = pcall(call)
                       said[#said + 1] = tostring(ok) .. " " .. tostring(tostring(err):find("contains itself", 1, true) ~= nil)
                   end
                   sol.status(table.concat(said, ";"))
               end)"#,
        );
        let outcome =
            scripts.surface_action("shell", "go", &crate::json::Json::Null, one_screen(&[]));
        assert_eq!(
            (outcome.status.as_deref(), format!("{:?}", outcome.commands)),
            (Some("false true;false true;false true"), "[]".to_owned()),
            "(each call's pcall and whether it said why, the commands queued)"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A listener that never returns is stopped, and the others still run**
    /// (03 §3.3.3): the compositor answers within the deadline.
    #[test]
    fn a_listener_that_never_returns_is_stopped_and_the_others_still_run() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline",
            r#"
            sol.on("layout", function() while true do end end)
            sol.on("layout", function() sol.status("ran") end)
            "#,
        );
        let started = std::time::Instant::now();
        let outcome = scripts.relayout(one_screen(&[]));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "took {:?}",
            started.elapsed()
        );
        assert_eq!(outcome.status.as_deref(), Some("ran"));
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_binding_that_never_returns_is_stopped() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-binding",
            r#"sol.bind("super+x", function() while true do end end)"#,
        );
        let started = std::time::Instant::now();
        let _ = scripts.key("super+x", one_screen(&[]));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "took {:?}",
            started.elapsed()
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A listener stopped three times is taken out until the next reload.**
    #[test]
    fn a_listener_stopped_three_times_is_taken_out() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-strikes",
            r#"
            runs = 0
            sol.on("layout", function() runs = runs + 1; sol.status(tostring(runs)); while true do end end)
            "#,
        );
        for _ in 0..4 {
            let _ = scripts.relayout(one_screen(&[]));
        }
        let outcome = scripts.relayout(one_screen(&[]));
        assert_eq!(
            (outcome.status, scripts.evaluate("return tostring(runs)")),
            (None, "3".to_owned()),
            "(what a fifth call said, how many times the listener ran)"
        );
        let mut reloaded =
            Scripts::load(&directory.join("init.lua")).expect("loading the test script again");
        assert_eq!(
            reloaded.relayout(one_screen(&[])).status.as_deref(),
            Some("1"),
            "a reload did not put the listener back"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **Each listener has the whole deadline**: the clock starts again for
    /// the next listener, so one that needs a few milliseconds still runs
    /// after another was stopped.
    #[test]
    fn each_listener_has_the_whole_deadline() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-each",
            r#"
            sol.on("layout", function() while true do end end)
            sol.on("layout", function() for _ = 1, 200000 do end sol.status("whole") end)
            "#,
        );
        let outcome = scripts.relayout(one_screen(&[]));
        assert_eq!(outcome.status.as_deref(), Some("whole"));
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A handler cannot put its own deadline off**: nothing the
    /// configuration can call starts the clock again, so a loop that tries
    /// to through `sol._deadline` is stopped like any other. It gives up
    /// after two seconds, so the test ends either way.
    #[test]
    fn a_handler_that_calls_sol_deadline_is_still_stopped() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-restart-called",
            r#"sol.on("layout", function()
                   local started = os.time()
                   while os.time() - started < 2 do
                       if type(sol._deadline) == "function" then sol._deadline() end
                   end
                   sol.status("gave up")
               end)"#,
        );
        let outcome = scripts.relayout(one_screen(&[]));
        assert_eq!(outcome.status, None, "the handler was not stopped");
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A configuration that replaces `sol._deadline` still leaves each
    /// listener its own deadline**: the compositor starts each listener's
    /// clock itself, so the one after a stopped listener still runs.
    #[test]
    fn replacing_sol_deadline_leaves_each_listener_its_own_deadline() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-restart-replaced",
            r#"
            sol._deadline = function() end
            sol.on("layout", function() while true do end end)
            sol.on("layout", function() for _ = 1, 200000 do end sol.status("whole") end)
            "#,
        );
        let outcome = scripts.relayout(one_screen(&[]));
        assert_eq!(outcome.status.as_deref(), Some("whole"));
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A loop inside a coroutine is stopped too**: a coroutine the handler
    /// makes is under the same deadline as the handler.
    #[test]
    fn a_listener_that_never_returns_inside_a_coroutine_is_stopped() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-coroutine",
            r#"
            sol.on("layout", function() coroutine.wrap(function() while true do end end)() end)
            sol.on("layout", function() sol.status("ran") end)
            "#,
        );
        let started = std::time::Instant::now();
        let outcome = scripts.relayout(one_screen(&[]));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "took {:?}",
            started.elapsed()
        );
        assert_eq!(outcome.status.as_deref(), Some("ran"));
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A `done` that never returns is stopped, and the next one still hears
    /// its outcome**, with the whole deadline of its own.
    #[test]
    fn a_done_that_never_returns_is_stopped_and_the_next_done_still_hears_its_outcome() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-done",
            r#"sol.on("surface", function()
                   sol.act("windows.focus", { id = 1 }, function() while true do end end)
                   sol.act("windows.focus", { id = 2 }, function(ok) for _ = 1, 200000 do end sol.status("second " .. tostring(ok)) end)
               end)"#,
        );
        let outcome =
            scripts.surface_action("shell", "go", &crate::json::Json::Null, one_screen(&[]));
        let settled: Vec<Settled> = outcome
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::Act { attempt, .. } => Some(Settled {
                    attempt: *attempt,
                    ok: true,
                    reason: None,
                }),
                _ => None,
            })
            .collect();
        assert_eq!(settled.len(), 2, "two acts: {:?}", outcome.commands);
        let started = std::time::Instant::now();
        let status = scripts.attempts_settled(&settled, one_screen(&[])).status;
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "took {:?}",
            started.elapsed()
        );
        assert_eq!(status.as_deref(), Some("second true"));
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// Every `sol.act` in `outcome`, settled as done.
    fn all_done(outcome: &Outcome) -> Vec<Settled> {
        outcome
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::Act { attempt, .. } => Some(Settled {
                    attempt: *attempt,
                    ok: true,
                    reason: None,
                }),
                _ => None,
            })
            .collect()
    }

    /// **A `done` stopped three times is not called again** until the next
    /// reload: its stops are counted by function, as a listener's are.
    #[test]
    fn a_done_stopped_three_times_is_not_called_again() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-done-strikes",
            r#"runs = 0
               local function done() runs = runs + 1; while true do end end
               sol.on("surface", function() sol.act("windows.focus", { id = 1 }, done) end)"#,
        );
        for _ in 0..4 {
            let outcome =
                scripts.surface_action("shell", "go", &crate::json::Json::Null, one_screen(&[]));
            let settled = all_done(&outcome);
            assert_eq!(settled.len(), 1, "one act: {:?}", outcome.commands);
            let _ = scripts.attempts_settled(&settled, one_screen(&[]));
        }
        assert_eq!(scripts.evaluate("return tostring(runs)"), "3");
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A `done` written inline is a new function at each `sol.act`**, so its
    /// stops do not add up: it is stopped every time, and never taken out.
    #[test]
    fn an_inline_done_is_stopped_each_time_and_never_taken_out() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-done-inline",
            r#"runs = 0
               sol.on("surface", function()
                   sol.act("windows.focus", { id = 1 }, function() runs = runs + 1; while true do end end)
               end)"#,
        );
        for _ in 0..4 {
            let outcome =
                scripts.surface_action("shell", "go", &crate::json::Json::Null, one_screen(&[]));
            let settled = all_done(&outcome);
            assert_eq!(settled.len(), 1, "one act: {:?}", outcome.commands);
            let _ = scripts.attempts_settled(&settled, one_screen(&[]));
        }
        assert_eq!(scripts.evaluate("return tostring(runs)"), "4");
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **What a `done` asked for before it was stopped is dropped**: its
    /// commands are not applied, and the attempt it started is forgotten,
    /// so a retry that acts again and then never returns is not told again.
    #[test]
    fn what_a_stopped_done_asked_for_is_dropped() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-done-drops",
            r#"sol.on("surface", function()
                   sol.act("windows.focus", { id = 1 }, function()
                       sol.act("windows.focus", { id = 2 }, function() end)
                       sol.close(2)
                       while true do end
                   end)
               end)"#,
        );
        let outcome =
            scripts.surface_action("shell", "go", &crate::json::Json::Null, one_screen(&[]));
        let told = scripts.attempts_settled(&all_done(&outcome), one_screen(&[]));
        assert_eq!(
            (
                format!("{:?}", told.commands),
                scripts.evaluate(
                    "local n = 0 for _ in pairs(sol._attempts) do n = n + 1 end return tostring(n)"
                )
            ),
            ("[]".to_owned(), "0".to_owned()),
            "(the commands the stopped done left, the attempts still waiting)"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **The clock stops when the dispatch ends**: Lua run between
    /// dispatches is no handler's, and is not stopped however long after one
    /// it runs.
    #[test]
    fn lua_run_between_dispatches_is_not_stopped() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-between",
            r#"sol.on("layout", function() end)"#,
        );
        let _ = scripts.relayout(one_screen(&[]));
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert_eq!(
            scripts.evaluate("for _ = 1, 200000 do end return 'finished'"),
            "finished"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A binding that keeps calling `sol.focus_direction` is still
    /// stopped**: the clock each `direction` listener starts is not the
    /// binding's.
    #[test]
    fn a_binding_that_loops_on_focus_direction_is_stopped() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-direction",
            r#"
            sol.on("direction", function() end)
            sol.bind("super+x", function() while true do sol.focus_direction("left") end end)
            "#,
        );
        let started = std::time::Instant::now();
        let _ = scripts.key("super+x", one_screen(&[]));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "took {:?}",
            started.elapsed()
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A `direction` listener stopped at the deadline stops the handler
    /// that called `sol.focus_direction` too**: the listeners' time counts
    /// against that handler, which is what keeps a loop of these calls
    /// stoppable.
    #[test]
    fn a_direction_listener_stopped_at_the_deadline_stops_its_caller_too() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-direction-caller",
            r#"
            sol.on("direction", function() while true do end end)
            sol.on("layout", function()
                sol.status("asked")
                sol.focus_direction("left")
                sol.status("went on")
            end)
            "#,
        );
        assert_eq!(
            scripts.relayout(one_screen(&[])).status.as_deref(),
            Some("asked")
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A loop that catches the stop with `pcall` is still stopped**, and
    /// the stop counts against the listener.
    #[test]
    fn a_listener_that_retries_with_pcall_is_stopped() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-pcall",
            r#"
            sol.on("layout", function() repeat until pcall(function() while true do end end) end)
            sol.on("layout", function() sol.status("ran") end)
            "#,
        );
        let started = std::time::Instant::now();
        let outcome = scripts.relayout(one_screen(&[]));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "took {:?}",
            started.elapsed()
        );
        assert_eq!(
            (
                outcome.status.as_deref(),
                scripts.evaluate(
                    "local n = 0 for _, count in pairs(sol._strikes) do n = n + count end return tostring(n)"
                )
            ),
            (Some("ran"), "1".to_owned()),
            "(what the next listener said, the strikes counted)"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A loop that catches the stop with `xpcall` is still stopped.**
    #[test]
    fn a_listener_that_retries_with_xpcall_is_stopped() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-xpcall",
            r#"
            sol.on("layout", function()
                repeat until xpcall(function() while true do end end, function(err) return err end)
            end)
            sol.on("layout", function() sol.status("ran") end)
            "#,
        );
        let started = std::time::Instant::now();
        let outcome = scripts.relayout(one_screen(&[]));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "took {:?}",
            started.elapsed()
        );
        assert_eq!(outcome.status.as_deref(), Some("ran"));
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **`pcall` and `xpcall` still catch an ordinary error in a handler**,
    /// and still hand back every value.
    #[test]
    fn pcall_still_catches_an_ordinary_error_in_a_handler() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-pcall-ordinary",
            r#"
            sol.on("layout", function()
                local ok, err = pcall(error, "mine", 0)
                local good, a, b = pcall(function() return 1, 2 end)
                local xok, xerr = xpcall(function() error("theirs", 0) end, function(e) return "handled " .. e end)
                sol.status(table.concat({ tostring(ok), err, tostring(good), a, b, tostring(xok), xerr }, " "))
            end)
            "#,
        );
        assert_eq!(
            scripts.relayout(one_screen(&[])).status.as_deref(),
            Some("false mine true 1 2 false handled theirs")
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A coroutine can still yield inside `pcall` and `xpcall`**, as it
    /// can in plain Lua 5.4.
    #[test]
    fn a_coroutine_can_still_yield_inside_pcall() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-pcall-yield",
            r#"
            sol.on("layout", function()
                local co = coroutine.wrap(function()
                    local _, first = pcall(function() return coroutine.yield("one") end)
                    local _, second = xpcall(function() return coroutine.yield("two") end, function(e) return e end)
                    return first .. " " .. second
                end)
                sol.status(table.concat({ co(), co("a"), co("b") }, " "))
            end)
            "#,
        );
        assert_eq!(
            scripts.relayout(one_screen(&[])).status.as_deref(),
            Some("one two a b")
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **`xpcall`'s message handler is not called for the stop**: Lua calls
    /// it from inside the instruction hook that raised the stop, where no hook
    /// runs, so one that never returned there would never be stopped.
    #[test]
    fn an_xpcall_handler_that_never_returns_is_not_called_for_the_stop() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-xpcall-handler",
            r#"
            sol.on("layout", function()
                xpcall(function() while true do end end, function() while true do end end)
            end)
            sol.on("layout", function() sol.status("ran") end)
            "#,
        );
        let started = std::time::Instant::now();
        let outcome = scripts.relayout(one_screen(&[]));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "took {:?}",
            started.elapsed()
        );
        assert_eq!(outcome.status.as_deref(), Some("ran"));
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **An `xpcall` message handler that never returns is stopped**, when
    /// the error it handles is an ordinary one.
    #[test]
    fn an_xpcall_handler_that_never_returns_is_stopped() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-xpcall-handler-ordinary",
            r#"
            sol.on("layout", function()
                xpcall(function() error("mine") end, function() while true do end end)
            end)
            sol.on("layout", function() sol.status("ran") end)
            "#,
        );
        let started = std::time::Instant::now();
        let outcome = scripts.relayout(one_screen(&[]));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "took {:?}",
            started.elapsed()
        );
        assert_eq!(outcome.status.as_deref(), Some("ran"));
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A loop that catches the stop in `load`'s reader is still stopped.**
    #[test]
    fn a_listener_that_retries_load_is_stopped() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-load",
            r#"
            sol.on("layout", function() repeat until load(function() while true do end end) end)
            sol.on("layout", function() sol.status("ran") end)
            "#,
        );
        let started = std::time::Instant::now();
        let outcome = scripts.relayout(one_screen(&[]));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "took {:?}",
            started.elapsed()
        );
        assert_eq!(outcome.status.as_deref(), Some("ran"));
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **`load` still answers a chunk, or `nil` and why**, and still takes a
    /// reader, a name, a mode and an environment.
    #[test]
    fn load_still_answers_a_chunk_or_nil_and_why() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-load-ordinary",
            r#"
            sol.on("layout", function()
                local bad, why = load("return (")
                local good = load("return x", "=mine", "t", { x = 5 })
                local pieces, at = { "return ", "7" }, 0
                local read = load(function() at = at + 1 return pieces[at] end)
                sol.status(table.concat({ tostring(bad), type(why), good(), read() }, " "))
            end)
            "#,
        );
        assert_eq!(
            scripts.relayout(one_screen(&[])).status.as_deref(),
            Some("nil string 5 7")
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **The deadline cannot reach a `__close` run by a stop**: the
    /// to-be-closed variables of the function a stop interrupts are closed
    /// inside the instruction hook, where no hook runs, so one runs to its end
    /// however late it is. Bounded here, so the test ends.
    #[test]
    fn a_close_run_after_a_stop_is_not_under_the_deadline() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-close",
            r#"
            sol.on("layout", function()
                local guard <close> = setmetatable({}, { __close = function()
                    for _ = 1, 1e6 do end
                    closed = true
                end })
                while true do end
            end)
            "#,
        );
        let _ = scripts.relayout(one_screen(&[]));
        assert_eq!(scripts.evaluate("return tostring(closed)"), "true");
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **The deadline cannot reach a `__gc` finalizer**: Lua runs one with no
    /// hook, so it runs to its end however long it takes. Bounded here, so
    /// the test ends.
    #[test]
    fn a_gc_finalizer_is_not_under_the_deadline() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-gc",
            r#"
            sol.on("layout", function()
                setmetatable({}, { __gc = function()
                    for _ = 1, 1e8 do end
                    finalized = true
                end })
                collectgarbage()
            end)
            "#,
        );
        let started = std::time::Instant::now();
        let _ = scripts.relayout(one_screen(&[]));
        assert!(
            started.elapsed() >= HANDLER_DEADLINE,
            "the finalizer ended within the deadline, so it shows nothing: {:?}",
            started.elapsed()
        );
        assert_eq!(scripts.evaluate("return tostring(finalized)"), "true");
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **The stops are counted by function**: one function listening for two
    /// events is taken out at its third stop, wherever the stops were, of the
    /// event it was stopped in, and of the other at its next stop there.
    #[test]
    fn the_stops_are_counted_by_function_across_events() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-strikes-by-function",
            r#"
            local function loop() while true do end end
            sol.on("layout", loop)
            sol.on("monitors", loop)
            "#,
        );
        let _ = scripts.relayout(one_screen(&[]));
        let _ = scripts.relayout(one_screen(&[]));
        let _ = scripts.monitors_changed(one_screen(&[]));
        let count = "return #sol._handlers.layout .. ' ' .. #sol._handlers.monitors";
        assert_eq!(
            scripts.evaluate(count),
            "1 0",
            "(layout listeners, monitors listeners)"
        );
        let _ = scripts.relayout(one_screen(&[]));
        assert_eq!(
            scripts.evaluate(count),
            "0 0",
            "(layout listeners, monitors listeners)"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// What `tracing` logs on this thread while `run` runs.
    pub(crate) fn logged_while(run: impl FnOnce()) -> String {
        #[derive(Clone, Default)]
        struct Lines(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for Lines {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0
                    .lock()
                    .map_err(|_| std::io::Error::other("the log lock was poisoned"))?
                    .extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        // A global subscriber that wants nothing, set once. With this
        // thread's subscriber the only one registered, `tracing` asks only
        // the thread that first reaches a call site whether it is wanted,
        // and caches the answer for every thread: a test on another thread
        // reaching "was stopped" first turned it off here, and `script::`
        // failed every time on two threads. With two registered, each event
        // asks the subscriber of its own thread.
        // `tests::a_stopped_listener_is_logged_with_its_file_and_line`.
        static QUIET: std::sync::Once = std::sync::Once::new();
        QUIET.call_once(|| {
            let _ = tracing::subscriber::set_global_default(
                tracing::subscriber::NoSubscriber::default(),
            );
        });
        let lines = Lines::default();
        let writer = lines.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, run);
        let bytes = lines
            .0
            .lock()
            .map(|bytes| bytes.clone())
            .unwrap_or_default();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// **An error in the configuration names it as Lua names a file**,
    /// `<path>:<line>:`, not as a string chunk, `[string "<path>"]:<line>:`.
    /// Lua shortens a long path from the front, so only its end is asserted.
    #[test]
    fn an_error_in_the_configuration_names_it_as_a_file() {
        let directory = std::env::temp_dir().join("solium-script-test-config-error-where");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        let _ = std::fs::write(&config, "\nerror(\"mine\")\n");
        let said = Scripts::load(&config)
            .err()
            .map(|err| err.to_string())
            .unwrap_or_default();
        assert!(
            said.contains("init.lua:2: mine") && !said.contains("[string"),
            "{said}"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A listener written in a chunk of text is logged by the chunk's
    /// short name**, as Lua's own messages name it: an `=name` chunk as
    /// `name`, and one with no name by its first line, not its whole text.
    #[test]
    fn a_stopped_listener_from_a_loaded_chunk_is_logged_by_its_short_name() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-where-chunk",
            r#"
            sol.on("layout", load("return function() while true do end end", "=mine")())
            sol.on("layout", load("return function()\n while true do end\n end")())
            "#,
        );
        let log = logged_while(|| {
            let _ = scripts.relayout(one_screen(&[]));
        });
        for at in [
            "file=mine line=1",
            r#"file=[string "return function()..."] line=1"#,
        ] {
            assert!(
                log.lines().any(|line| line.contains(at)),
                "no line names {at}:\n{log}"
            );
        }
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **A stopped listener is logged with its file and line**, and so is
    /// one taken out.
    #[test]
    fn a_stopped_listener_is_logged_with_its_file_and_line() {
        let (directory, mut scripts) = loaded(
            "solium-script-test-deadline-where",
            "\nsol.on(\"layout\", function() while true do end end)\n",
        );
        let log = logged_while(|| {
            for _ in 0..3 {
                let _ = scripts.relayout(one_screen(&[]));
            }
        });
        let at = format!("file={} line=2", directory.join("init.lua").display());
        let named: Vec<&str> = log.lines().filter(|line| line.contains(&at)).collect();
        assert!(
            named.len() == 4 && named.iter().any(|line| line.contains("taken out")),
            "three stops and one take-out, each naming {at}:\n{log}"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }
}

/// **The configuration that ships may only ask for things the compositor has.**
///
/// `init.lua` asked for an easing called `inOutCubic` and the engine had no
/// such curve. `parse_easing` warned, fell back to the default, and the session
/// carried on -- so the genie was quietly the wrong animation for as long as it
/// took somebody to read a hardware log and notice five identical warnings in
/// it. Nothing in the gate was looking, because nothing in the gate reads Lua
/// for anything but syntax: `solium --check` loads the scripts, and a name
/// inside a binding's body is not looked up until the binding runs.
///
/// That is a class rather than an incident. A script names things -- easings,
/// events, layers, QML scenes, decorations, keys -- and every one of those
/// lookups either warns and carries on or, in the case of an event and a key,
/// misses in complete silence. So this walks `lua/*.lua` and resolves each name
/// through the same function the compositor uses at run time.
///
/// **What it cannot see.** Only names written as literals. `shell.lua` takes
/// its scene from the configuration or the environment and `workspaces.lua`
/// builds `"super+" .. index` in a loop; a name assembled at run time is
/// outside this and outside any static check. Lua comments are stripped, so a
/// documented example that is deliberately a placeholder -- a path into
/// somebody's home directory -- does not fail a build.
#[cfg(test)]
mod shipped {
    use super::{Curve, Scripts, normalise_combo};

    /// The Lua the compositor ships, as `(file, text)`.
    ///
    /// From `CARGO_MANIFEST_DIR` rather than a path relative to the process,
    /// because a test's working directory is the workspace root and this file
    /// should not have to know that.
    fn scripts() -> Vec<(String, String)> {
        let directory = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/lua"));
        let Ok(entries) = std::fs::read_dir(directory) else {
            return Vec::new();
        };
        let mut found: Vec<(String, String)> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|end| end == "lua"))
            .filter_map(|path| {
                let name = path.file_name()?.to_str()?.to_owned();
                Some((name, std::fs::read_to_string(&path).ok()?))
            })
            .collect();
        found.sort();
        // A check that walks an empty directory passes, which is the one way
        // this could be green and mean nothing at all.
        assert!(
            found.len() >= 8,
            "found {} shipped scripts in {}; the walk is broken, not the scripts",
            found.len(),
            directory.display()
        );
        found
    }

    /// One line with its Lua comment removed.
    ///
    /// `--` outside a string starts a comment. Tracked rather than searched for
    /// because `config.lua` has a `"module 'user' not found"` in it and a naive
    /// cut would one day land inside a string like that one.
    fn code(line: &str) -> &str {
        let bytes = line.as_bytes();
        let mut quoted = false;
        let mut index = 0;
        while index < bytes.len() {
            match bytes[index] {
                b'\\' if quoted => index += 1,
                b'"' => quoted = !quoted,
                b'-' if !quoted && bytes.get(index + 1) == Some(&b'-') => return &line[..index],
                _ => {}
            }
            index += 1;
        }
        line
    }

    /// Every double-quoted literal that follows `marker`, with where it was.
    ///
    /// `marker` is matched literally and the literal must come next, with only
    /// spaces between: `easing =` finds `easing = "outCubic"` and `sol.on(`
    /// finds `sol.on("open", ...)`. Anything else after the marker -- a
    /// variable, a table, a concatenation -- is skipped rather than guessed at.
    /// That is the limit this module states up front, and it is why
    /// `shell.lua`'s `scene = chosen` never appears here.
    fn named(text: &str, marker: &str) -> Vec<(usize, String)> {
        // An empty marker matches at every position and consumes none of them,
        // so the walk below would never move. Refused here rather than left to
        // a caller, because the symptom is a test run that never finishes --
        // which is how this was found.
        assert!(!marker.is_empty(), "a marker has to be something");
        let mut found = Vec::new();
        for (number, line) in text.lines().enumerate() {
            let line = code(line);
            let mut from = 0;
            while let Some(at) = line[from..].find(marker) {
                let after = from + at + marker.len();
                from = after;
                let rest = line[after..].trim_start_matches(' ');
                let Some(rest) = rest.strip_prefix('"') else {
                    continue;
                };
                let Some(end) = rest.find('"') else {
                    continue;
                };
                found.push((number + 1, rest[..end].to_owned()));
            }
        }
        found
    }

    /// Every double-quoted literal in a chunk of Lua, in order.
    ///
    /// For a list rather than an assignment: `{ "top", "left", ... }` has no
    /// marker in front of each item, only in front of the whole thing.
    fn quoted(text: &str) -> Vec<String> {
        let mut found = Vec::new();
        for line in text.lines() {
            let mut rest = code(line);
            while let Some(at) = rest.find('"') {
                let after = &rest[at + 1..];
                let Some(end) = after.find('"') else {
                    break;
                };
                found.push(after[..end].to_owned());
                rest = &after[end + 1..];
            }
        }
        found
    }

    /// Everything in the shipped Lua that follows `marker`, for every file.
    fn everywhere(marker: &str) -> Vec<(String, usize, String)> {
        scripts()
            .into_iter()
            .flat_map(|(file, text)| {
                named(&text, marker)
                    .into_iter()
                    .map(move |(line, value)| (file.clone(), line, value))
            })
            .collect()
    }

    /// Where the QML that ships lives.
    fn shipped_qml() -> std::path::PathBuf {
        std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/qml"))
    }

    /// **Every easing the shipped configuration asks for exists.**
    ///
    /// The one that did not: `sol.animate({ duration = 520, easing =
    /// "inOutCubic" })`, in `init.lua` for `super+m` and again in `tweaks.lua`
    /// for the genie tweak. `Curve::from_name` knew five names and that was not
    /// one of them, so both fell back to `outCubic` -- a different animation,
    /// chosen by nobody, announced only in a log line.
    #[test]
    fn motion_274_lua_springs_carry_physical_release_velocity() {
        use crate::present::Curve;
        use crate::script::easing_from;
        use mlua::{Lua, Table};
        let lua = Lua::new();
        let options: Table = lua.load("return { easing = { spring = { stiffness = 220, damping = 24, mass = 1.2, initial_velocity = -3.5, epsilon = 0.002 } } }").eval().expect("options");
        let Curve::Spring(spring) = easing_from(&options)
            .expect("spring options")
            .expect("curve")
        else {
            panic!("physical spring required")
        };
        assert_eq!(
            (
                spring.stiffness,
                spring.damping,
                spring.mass,
                spring.initial_velocity,
                spring.epsilon
            ),
            (220.0, 24.0, 1.2, -3.5, 0.002)
        );
        assert!((spring.value_at(0.0000001) / 0.0000001 + 3.5).abs() < 0.001);
        for invalid in [
            "mass=0",
            "mass=1e-30",
            "stiffness=-1",
            "damping=-1",
            "epsilon=0",
            "initial_velocity=1/0",
            "mass=0/0",
        ] {
            let options: Table = lua
                .load(format!(
                    "return {{ easing = {{ spring = {{ {invalid} }} }} }}"
                ))
                .eval()
                .expect("options");
            assert!(easing_from(&options).is_err(), "{invalid}");
        }
    }

    #[test]
    fn every_easing_named_is_one_the_engine_has() {
        let asked = everywhere("easing =");
        assert!(!asked.is_empty(), "no easings found; the scan is broken");
        for (file, line, name) in asked {
            assert!(
                Curve::from_name(&name).is_some(),
                "{file}:{line} asks for easing {name:?}, which Curve::from_name cannot read. \
                 Names: {:?}",
                Curve::all().map(|(known, _)| known)
            );
        }
    }

    /// **Every effect the shipped configuration asks for is a shipped
    /// folder.**
    ///
    /// The easing case above, one layer over. `sol.present` takes `deform = {
    /// effect = "..." }` and the name is an effect folder's, resolved by the
    /// effect host where the command is applied -- which also means a
    /// misspelling is not a Lua error but a problem on the overlay and an
    /// undeformed window, so it is caught here before it ships.
    #[test]
    fn every_effect_named_is_one_the_engine_has() {
        let asked = everywhere("effect =");
        assert!(!asked.is_empty(), "no effects found; the scan is broken");
        let shipped = crate::assets::effects();
        for (file, line, name) in asked {
            assert!(
                shipped.join(&name).join("effect.lua").is_file(),
                "{file}:{line} asks for effect {name:?}, which is no folder with an effect.lua in {}",
                shipped.display()
            );
        }
    }

    /// **And every axis, which is worse when it is wrong.**
    ///
    /// A deform's `axis` is refused outside its words when the binding runs,
    /// which is a binding that does nothing, seen by nobody who was not
    /// reading the log. `"auto"` is one of them: the side the target lies
    /// on, picked when the flight starts.
    #[test]
    fn every_axis_named_is_one_the_engine_has() {
        let asked = everywhere("axis =");
        assert!(!asked.is_empty(), "no axes found; the scan is broken");
        for (file, line, name) in asked {
            assert!(
                name == "auto" || solium_effects::Axis::from_name(&name).is_some(),
                "{file}:{line} asks for axis {name:?}, which Axis::from_name cannot read. \
                 Names: \"auto\" and {:?}",
                solium_effects::Axis::all().map(|(known, _)| known)
            );
        }
    }

    /// **Every event a shipped script listens for is one the compositor sends.**
    ///
    /// Worse than an unknown easing, because there is no warning at all:
    /// `sol.on` puts the handler in a table under whatever name it was given,
    /// and a name nothing dispatches is a handler that is simply never called.
    /// A layout that misspells `"monitors"` does not fail -- it stops
    /// rearranging when a screen is unplugged, which reads as a compositor bug.
    ///
    /// The vocabulary is read out of `script/` itself rather than listed
    /// here, because a list is a second copy and two copies of a vocabulary
    /// drifting apart is the defect this whole module exists for.
    #[test]
    fn every_event_listened_for_is_one_that_is_sent() {
        // The production half of the module, each file cut at its first `#[cfg(test)]`
        // because everything below it -- including this test -- talks *about*
        // `call_listeners` and would otherwise be counted as calling it.
        let production = super::source::production();
        // Decision hooks (`sol.on("notification")`) are dispatched by
        // `call_deciders`, and are events like any other.
        let dispatched: Vec<String> = named(production, "call_listeners(lua,")
            .into_iter()
            .chain(named(production, "call_deciders(lua,"))
            .map(|(_, name)| name)
            .collect();

        // The scrape has to see every dispatch there is, or it narrows the
        // vocabulary in silence and this test starts agreeing with anything.
        // One `call_listeners` is the definition; the rest are calls.
        let calls = production.matches("call_listeners").count() - 1
            + production
                .matches("call_deciders")
                .count()
                .saturating_sub(1);
        assert_eq!(
            dispatched.len(),
            calls,
            "{calls} dispatches in script/ but only {} are `call_listeners(lua, \"name\")`; \
             one of them is written some other way and this check cannot see it",
            dispatched.len()
        );
        assert!(
            calls >= 8,
            "only {calls} dispatches found; the scan is broken"
        );

        for (file, line, event) in everywhere("sol.on(") {
            assert!(
                dispatched.contains(&event),
                "{file}:{line} listens for {event:?}, which nothing dispatches. \
                 Events: {dispatched:?}"
            );
        }
    }

    /// **Every layer a shipped surface names is one that exists.**
    ///
    /// `Layer::parse` answers `None` and `unwrap_or_default` turns that into
    /// `Background`, with no warning anywhere. A bar that asked for `"Top"` and
    /// got the background is a bar drawn underneath every window on the screen,
    /// and the symptom is that the bar has vanished.
    #[test]
    fn every_layer_named_is_one_that_exists() {
        let asked = everywhere("layer =");
        assert!(!asked.is_empty(), "no layers found; the scan is broken");
        for (file, line, name) in asked {
            assert!(
                crate::scripted::Layer::parse(&name).is_some(),
                "{file}:{line} puts a surface on layer {name:?}, which is not one of \
                 background, bottom, top, overlay"
            );
        }
    }

    /// **Every QML scene a shipped script names is one that ships.**
    ///
    /// Three lookups, because there are three kinds of scene and a script
    /// writes them the same way: a scripted surface's, found on the QML search
    /// path by `scripted::find_scene`; a loading scene's, which is a name under
    /// `qml/loading`; and the lock's (`lock.scene`), a name under `qml/lock`
    /// (`lock_scene::find_scene`). A scene that is not there is an `ERROR` and
    /// a hole in the picture -- `sol.surface` says "no such QML scene" and
    /// draws nothing, and a lock shows its stand-in.
    ///
    /// The resolved path has to land inside the shipped QML directory. Without
    /// that, a developer with their own `~/.config/solium/qml/wallpaper.qml`
    /// would have a green test for a file that does not ship.
    #[test]
    fn every_scene_named_is_one_that_ships() {
        let shipped = shipped_qml();
        let asked = everywhere("scene =");
        assert!(!asked.is_empty(), "no scenes found; the scan is broken");
        for (file, line, name) in asked {
            let surface = crate::scripted::find_scene(&name)
                .is_some_and(|path| path.starts_with(&shipped) && path.is_file());
            let loading = shipped
                .join("loading")
                .join(format!("{name}.qml"))
                .is_file();
            let lock = crate::lock_scene::find_scene(&name, None, &shipped.join("lock"))
                .is_some_and(|path| path.starts_with(&shipped) && path.is_file());
            assert!(
                surface || loading || lock,
                "{file}:{line} names scene {name:?}, and there is no {name} in \
                 {}, its loading directory or its lock directory",
                shipped.display()
            );
        }
    }

    /// **Every surface a shipped selection names is one a shipped script
    /// declares.**
    ///
    /// `sol.group` is the third place a script writes a surface's name down, and
    /// the quietest of the three. A name nothing has declared is not an error:
    /// the selection simply does not contain it, so the group is declared, the
    /// transform is applied, and the wallpaper stays exactly where it was —
    /// which is the bug this whole item exists to remove, wearing a typo.
    ///
    /// Both halves are read as literals, so what this can see is bounded the way
    /// the module header says. The shipped desks build their surface names from
    /// a workspace index (`wallpaper.for_desk`), and a name assembled at run time
    /// is outside this and outside any static check — so the *membership* side
    /// may legitimately be empty. What is asserted non-empty is the declaration
    /// side, which proves the scan itself still works on this corpus; a literal
    /// written into a group tomorrow is checked against it from that day on.
    #[test]
    fn every_surface_a_selection_names_is_one_that_is_declared() {
        let declared: Vec<String> = everywhere("sol.surface(")
            .into_iter()
            .map(|(_, _, name)| name)
            .collect();
        assert!(
            !declared.is_empty(),
            "no `sol.surface` declarations found; the scan is broken"
        );

        // Every literal inside a `surfaces = { ... }` list, with its file.
        let mut asked: Vec<(String, usize, String)> = Vec::new();
        for (file, text) in scripts() {
            for (line, rest) in surface_lists(&text) {
                for name in quoted(&rest) {
                    asked.push((file.clone(), line, name));
                }
            }
        }
        for (file, line, name) in asked {
            assert!(
                declared.contains(&name),
                "{file}:{line} puts surface {name:?} in a selection, and nothing                  declares one by that name. Declared: {declared:?}"
            );
        }
    }

    /// The text of each `surfaces = { ... }` list in a chunk of Lua, with the
    /// line it starts on.
    ///
    /// A list rather than an assignment, so [`named`] cannot see it: there is no
    /// marker in front of each item, only in front of the whole thing. Single
    /// line only, which is how every one of them is written and what the whole
    /// of this module can see.
    fn surface_lists(text: &str) -> Vec<(usize, String)> {
        let mut found = Vec::new();
        for (number, line) in text.lines().enumerate() {
            let line = code(line);
            let Some(at) = line.find("surfaces = {") else {
                continue;
            };
            let rest = &line[at..];
            let end = rest.find('}').unwrap_or(rest.len());
            found.push((number + 1, rest[..end].to_owned()));
        }
        found
    }

    /// **Every pane style the shipped configuration names is one that ships.**
    ///
    /// A name that is nowhere is a frame that fails to build once per window --
    /// every window undecorated, and a log line each.
    ///
    /// Resolved through `decoration::ships` rather than by joining a path onto
    /// the name here, which is this module's own rule -- "resolves each name
    /// through the same function the compositor uses at run time" -- and which
    /// this test was the one exception to. It matters rather than being a
    /// tidy-up, and the move to style folders is where it would have bitten:
    /// the eight names `config.lua` has always carried stopped being files
    /// under `qml/decorations` and became folders under `qml/panes` on one
    /// commit, and a hand-joined `<name>.qml` would have gone red for all eight
    /// while the compositor drew them perfectly. Through the resolver there was
    /// nothing to remember.
    ///
    /// Both markers, because the setting is `pane` and `decoration` is still
    /// read: a user.lua written before the rename still names a style, and a
    /// name that has to resolve is a name this has to check. `everywhere` sees
    /// only the shipped scripts, so what this really pins is that the alias in
    /// `config.lua` cannot be left pointing at a style that was deleted.
    #[test]
    fn every_pane_style_named_is_one_that_ships() {
        let ships = crate::decoration::ships();
        // The walk before anything is decided by it: a catalogue that came back
        // empty would pass every name below by having no opinion, which is the
        // one way this can be green and mean nothing.
        assert!(
            ships.len() >= 8,
            "this build offers {} styles, which is fewer than the eight that \
             `config.lua` documents -- the walk is broken, not the configuration",
            ships.len()
        );
        let mut asked = everywhere("pane =");
        asked.extend(everywhere("decoration ="));
        assert!(
            !asked.is_empty(),
            "no pane styles found; the scan is broken"
        );
        for (file, line, name) in asked {
            // `none` is a real setting and draws no frame at all, deliberately.
            assert!(
                name == "none" || ships.iter().any(|offered| offered.name == name),
                "{file}:{line} asks for pane style {name:?}, and this build ships no style \
                 of that name -- it ships {:?}",
                ships
                    .iter()
                    .map(|offered| offered.name.as_str())
                    .collect::<Vec<_>>()
            );
        }
    }

    /// **Every key a shipped script binds is spelled the way a key arrives.**
    ///
    /// A binding is a table lookup on a string. `input::combo_for` builds that
    /// string from `xkb::keysym_get_name` and `normalise_combo` lowercases it,
    /// so a binding whose key is not that exact spelling is not a binding at
    /// all -- it is an entry in a table nothing will ever look up, with no
    /// warning at load and nothing at the press but a `no script has bound
    /// this` at info.
    ///
    /// Round-tripped rather than merely resolved: `keysym_from_name` is
    /// forgiving and `keysym_get_name` is not, and it is the second one that
    /// decides what a key is called at run time.
    ///
    /// Only the bindings written as literals after `sol.bind(`. `workspaces.lua`
    /// builds nine of them from a loop counter and they cannot be read from the
    /// text -- see this module's header -- and `scrolling.lua` binds through a
    /// local `bind` helper this marker does not match.
    ///
    /// And it checks the *spelling*, not that the key can be reached. Those are
    /// not the same question: `super+shift+1` is spelled perfectly, and until
    /// #121 no key on a `us` keyboard produced it, because `shift+1` arrives as
    /// `exclam` and that was the only name a press was looked up by.
    /// Reachability is `every_shipped_binding_is_reachable_on_us` below, which
    /// reads the bindings from the loaded scripts rather than from the text and
    /// so sees the loop-built and helper-built ones this one skips.
    #[test]
    fn every_key_bound_is_spelled_the_way_it_arrives() {
        use smithay::input::keyboard::xkb;

        let bound = everywhere("sol.bind(");
        assert!(!bound.is_empty(), "no bindings found; the scan is broken");
        for (file, line, combo) in bound {
            let combo = normalise_combo(&combo);
            let Some(key) = combo.rsplit('+').next() else {
                continue;
            };
            // A combo built by concatenation -- `"super+" .. index` -- reaches
            // this as a bare `super+` with nothing after it. There is no key
            // here to check; the header says so.
            if key.is_empty() {
                continue;
            }
            let keysym = xkb::keysym_from_name(key, xkb::KEYSYM_CASE_INSENSITIVE);
            assert!(
                keysym != xkb::keysyms::KEY_NoSymbol.into(),
                "{file}:{line} binds {combo:?}, and xkb has no key called {key:?}"
            );
            let canonical = xkb::keysym_get_name(keysym).to_ascii_lowercase();
            assert_eq!(
                canonical, key,
                "{file}:{line} binds {combo:?}, but that key arrives called {canonical:?} -- \
                 the binding would never fire"
            );
        }
    }

    /// A `us` keymap, compiled from the real rules, and its modifier indices.
    ///
    /// Real rather than described, because the question these tests ask is
    /// what xkb does and not what anyone believes it does.
    fn us_keymap() -> (smithay::input::keyboard::xkb::Keymap, UsModifiers) {
        use smithay::input::keyboard::xkb;

        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let Some(keymap) = xkb::Keymap::new_from_names(
            &context,
            "",
            "",
            "us",
            "",
            None,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        ) else {
            // No xkb rules on this machine means no keymap to ask, and a test
            // that invented an answer here would be worse than one that says
            // it could not look.
            panic!("no `us` keymap; xkb data is missing, so this proves nothing");
        };
        let modifiers = UsModifiers {
            shift: keymap.mod_get_index(xkb::MOD_NAME_SHIFT),
            ctrl: keymap.mod_get_index(xkb::MOD_NAME_CTRL),
            alt: keymap.mod_get_index(xkb::MOD_NAME_ALT),
            logo: keymap.mod_get_index(xkb::MOD_NAME_LOGO),
        };
        (keymap, modifiers)
    }

    /// Where `us_keymap` keeps each modifier the combination syntax can name.
    struct UsModifiers {
        shift: u32,
        ctrl: u32,
        alt: u32,
        logo: u32,
    }

    /// Every name a press of xkb keycode `code` answers to while `held` is
    /// down -- the list `input::keyboard` tries bindings in.
    ///
    /// Built the way that filter builds it: `modified` is `key_get_one_sym`
    /// under the held modifiers, which is what smithay's `modified_sym` calls,
    /// and `raw` is the key's first keysym at level 0 of its layout, which is
    /// what the filter's `raw_latin_sym_or_raw_current_sym` comes to on a
    /// keymap with one layout -- there is no other to look in, and `us_keymap`
    /// makes no other kind. The filter itself, on two layouts, is
    /// `input::tests::every_shipped_binding_fires_on_both_groups_of_us_ru`.
    /// Then the same
    /// `input::combos_for`, so the string compared below is the string the
    /// compositor looks up and not a second opinion of it.
    fn names_for(
        keymap: &smithay::input::keyboard::xkb::Keymap,
        indices: &UsModifiers,
        code: u32,
        held: &smithay::input::keyboard::ModifiersState,
    ) -> Vec<String> {
        use smithay::input::keyboard::xkb;

        let mut mask = 0;
        for (on, index) in [
            (held.shift, indices.shift),
            (held.ctrl, indices.ctrl),
            (held.alt, indices.alt),
            (held.logo, indices.logo),
        ] {
            if on {
                mask |= 1 << index;
            }
        }
        let mut state = xkb::State::new(keymap);
        state.update_mask(mask, 0, 0, 0, 0, 0);
        let code = xkb::Keycode::new(code);
        let modified = state.key_get_one_sym(code);
        let raw = keymap
            .key_get_syms_by_level(code, state.key_get_layout(code), 0)
            .first()
            .copied();
        crate::input::combos_for(held, modified, raw)
    }

    /// The modifiers a canonical combination names, as a keyboard would hold
    /// them.
    fn held_for(combo: &str) -> smithay::input::keyboard::ModifiersState {
        let named: Vec<&str> = combo.split('+').collect();
        let modifiers = named
            .split_last()
            .map(|(_, modifiers)| modifiers)
            .unwrap_or_default();
        smithay::input::keyboard::ModifiersState {
            ctrl: modifiers.contains(&"ctrl"),
            alt: modifiers.contains(&"alt"),
            shift: modifiers.contains(&"shift"),
            logo: modifiers.contains(&"super"),
            ..Default::default()
        }
    }

    /// **The two height binds in `tiling.lua` are combinations a `us` keyboard
    /// can actually produce.**
    ///
    /// Reachability, which the spelling test above deliberately does not check:
    /// it asks whether a key is *spelled* the way xkb names it, and
    /// `underscore` passes that while being a combination no one can press
    /// with the modifiers the binding also names.
    ///
    /// So this goes the other way round. It starts from the physical keys --
    /// `AE11` and `AE12`, the `-` and `=` of the top row, which are evdev 12
    /// and 13 and so xkb keycodes 20 and 21 -- presses them under the modifiers
    /// the bindings name, and asks `names_for` what that press is called.
    ///
    /// These were bound on ctrl because until #121 the shift spelling was
    /// dead: shift+`-` arrives as `underscore`, and that was the only name a
    /// press had. Ctrl selects no shift level, so it is right before that fix
    /// and after it. The second half pins the fix itself for this key: the
    /// spelling #120 started with is now one a press answers to, so the next
    /// reader who tidies these back to shift gets a working binding rather
    /// than a dead one.
    #[test]
    fn every_height_bind_is_a_key_that_arrives() {
        use smithay::input::keyboard::ModifiersState;

        let (keymap, indices) = us_keymap();
        let minus = 20;
        let equal = 21;
        let with_ctrl = ModifiersState {
            ctrl: true,
            logo: true,
            ..Default::default()
        };
        let with_shift = ModifiersState {
            shift: true,
            logo: true,
            ..Default::default()
        };

        // What `tiling.lua` binds, and what the keyboard sends. These have to
        // be the same string or the binding is an entry in a table nothing
        // looks up.
        for (code, bound) in [(minus, "super+ctrl+minus"), (equal, "super+ctrl+equal")] {
            let sent = names_for(&keymap, &indices, code, &with_ctrl);
            assert!(
                sent.contains(&normalise_combo(bound)),
                "`tiling.lua` binds {bound:?}, and that keypress answers only to {sent:?}"
            );
        }

        // And the spelling it started as, which #121 made reachable. Shift
        // still changes the keysym -- the press is called `underscore` first
        // -- but it also answers to the key it was.
        for (code, shifted, bound) in [
            (minus, "super+shift+underscore", "super+shift+minus"),
            (equal, "super+shift+plus", "super+shift+equal"),
        ] {
            let sent = names_for(&keymap, &indices, code, &with_shift);
            assert_eq!(
                sent,
                [normalise_combo(shifted), normalise_combo(bound)],
                "shift on this key should arrive as {shifted:?} and fall back to \
                 {bound:?}, modified spelling first"
            );
        }
    }

    /// **The held volume and brightness keys repeat, and the laptop keys fire
    /// at the lock -- and nothing else that ships does either** (#224).
    ///
    /// Mute, the media keys and Print do not repeat: a toggle or a skip
    /// repeating at 25 a second is a key that cannot be pressed once. Print
    /// does not fire at the lock, which has nothing on screen worth keeping.
    /// The second half is the one that matters most: every binding allowed at
    /// the lock is one more thing a lock screen does, so the list of them is
    /// pinned whole, and a shipped binding that starts firing there fails this
    /// until it is added on purpose.
    #[test]
    fn the_held_volume_keys_repeat_and_only_the_laptop_keys_fire_at_the_lock() {
        let shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua");
        let directory = std::env::temp_dir().join("solium-shipped-binding-options");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("creating the entry directory");
        let entry = directory.join("init.lua");
        std::fs::write(
            &entry,
            format!(
                "package.path = {shipped:?} .. \"/?.lua\"\ndofile({shipped:?} .. \"/init.lua\")\n"
            ),
        )
        .expect("writing the entry point");
        let scripts = Scripts::load(&entry).expect("loading the shipped init.lua");
        let bindings = scripts.bindings();
        let with = |asked: fn(&super::Binding) -> bool| -> Vec<&str> {
            bindings
                .iter()
                .filter(|binding| asked(binding))
                .map(|binding| binding.combo.as_str())
                .collect()
        };
        assert_eq!(
            with(|binding| binding.repeating),
            vec![
                "shift+xf86audiolowervolume",
                "shift+xf86audioraisevolume",
                "xf86audiolowervolume",
                "xf86audioraisevolume",
                "xf86monbrightnessdown",
                "xf86monbrightnessup",
            ],
            "the shipped bindings that repeat while held"
        );
        assert_eq!(
            with(|binding| binding.locked),
            vec![
                "shift+xf86audiolowervolume",
                "shift+xf86audioraisevolume",
                "xf86audiolowervolume",
                "xf86audiomicmute",
                "xf86audiomute",
                "xf86audionext",
                "xf86audiopause",
                "xf86audioplay",
                "xf86audioprev",
                "xf86audioraisevolume",
                "xf86monbrightnessdown",
                "xf86monbrightnessup",
            ],
            "the shipped bindings that fire at the lock"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **The shipped media keys act on Solium's own media, and spawn
    /// `playerctl` only when it follows no players** (#379).
    #[test]
    fn the_shipped_media_keys_act_on_the_active_player_and_fall_back_to_playerctl() {
        let shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua");
        let directory = std::env::temp_dir().join("solium-shipped-media-keys");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("creating the entry directory");
        let entry = directory.join("init.lua");
        std::fs::write(
            &entry,
            format!(
                "package.path = {shipped:?} .. \"/?.lua\"\ndofile({shipped:?} .. \"/init.lua\")\n"
            ),
        )
        .expect("writing the entry point");
        let mut scripts = Scripts::load(&entry).expect("loading the shipped init.lua");
        let following = super::Snapshot {
            media: crate::services::media::Seen {
                view: std::rc::Rc::new(crate::services::media::View {
                    available: true,
                    ..crate::services::media::View::default()
                }),
                now: std::time::Duration::ZERO,
            },
            ..super::Snapshot::default()
        };
        let acted = |outcome: &super::Outcome| -> Vec<String> {
            outcome
                .commands
                .iter()
                .filter_map(|command| match command {
                    super::Command::Act { action, .. } => Some(action.clone()),
                    _ => None,
                })
                .collect()
        };
        let spawned = |outcome: &super::Outcome| -> Vec<(String, Vec<String>)> {
            outcome
                .commands
                .iter()
                .filter_map(|command| match command {
                    super::Command::Spawn { program, args } => {
                        Some((program.clone(), args.clone()))
                    }
                    _ => None,
                })
                .collect()
        };
        for (key, action) in [
            ("XF86AudioPlay", "media.play_pause"),
            ("XF86AudioPause", "media.play_pause"),
            ("XF86AudioNext", "media.next"),
            ("XF86AudioPrev", "media.previous"),
        ] {
            let outcome = scripts.key(key, following.clone());
            assert_eq!(acted(&outcome), [action], "{key}");
            assert!(spawned(&outcome).is_empty(), "{key} spawned nothing");
        }
        let alone = scripts.key("XF86AudioNext", super::Snapshot::default());
        assert!(acted(&alone).is_empty());
        assert_eq!(
            spawned(&alone),
            [("playerctl".to_owned(), vec!["next".to_owned()])]
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// **Every key a shipped script binds can be pressed on a `us` keyboard.**
    ///
    /// The question the spelling test cannot ask. This one loads the shipped
    /// `init.lua` -- so the bindings are what the running compositor has, the
    /// nine `super+shift+N` built by a loop in `workspaces.lua` and the keys
    /// `scrolling.lua` binds through its own helper included -- and for each,
    /// looks for a physical key that, held under the modifiers the binding
    /// names, answers to that exact string.
    ///
    /// Eleven failed this before #121: all nine send-to-workspace keys and both
    /// `super+shift+bracketleft`/`bracketright`, because shift turned the key
    /// into `exclam` or `braceleft` and that was the only name looked up.
    ///
    /// `package.path` is written by the entry rather than left to
    /// `Scripts::load`, which prepends the developer's own `~/.config/solium`
    /// and would make this a test of somebody's configuration -- the trap the
    /// layout harness below documents.
    #[test]
    fn every_shipped_binding_is_reachable_on_us() {
        let shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua");
        let directory = std::env::temp_dir().join("solium-reachable");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("creating the entry directory");
        let entry = directory.join("init.lua");
        std::fs::write(
            &entry,
            format!(
                "package.path = {shipped:?} .. \"/?.lua\"\ndofile({shipped:?} .. \"/init.lua\")\n"
            ),
        )
        .expect("writing the entry point");
        let scripts = Scripts::load(&entry).expect("loading the shipped init.lua");
        let bound: Vec<String> = scripts
            .bindings()
            .into_iter()
            .map(|binding| binding.combo)
            .collect();

        // The ones this is for, by name, so a load that quietly bound nothing
        // -- or a scan that lost the loop -- cannot pass by having nothing to
        // check.
        let expected = (1..=9).map(|index| format!("shift+super+{index}")).chain([
            "shift+super+bracketleft".to_owned(),
            "shift+super+bracketright".to_owned(),
            // #150's, from `direction.lua` and `modes.lua` by way of `init.lua`.
            "super+left".to_owned(),
            "super+l".to_owned(),
            // #281's lock key, from `init.lua`.
            "ctrl+super+l".to_owned(),
            "shift+super+down".to_owned(),
            "shift+super+h".to_owned(),
            "alt+super+k".to_owned(),
            // #151's laptop keys, from `init.lua`: every key a laptop's
            // function row has, so on a `us` keyboard each must be a key.
            "xf86audioraisevolume".to_owned(),
            "xf86audiolowervolume".to_owned(),
            "shift+xf86audioraisevolume".to_owned(),
            "xf86audiomute".to_owned(),
            "xf86audiomicmute".to_owned(),
            "xf86monbrightnessup".to_owned(),
            "xf86monbrightnessdown".to_owned(),
            "xf86audioplay".to_owned(),
            "xf86audionext".to_owned(),
            "xf86audioprev".to_owned(),
            "print".to_owned(),
            "shift+print".to_owned(),
            "super+f".to_owned(),
            "shift+super+m".to_owned(),
            "shift+super+space".to_owned(),
        ]);
        for combo in expected {
            assert!(
                bound.contains(&combo),
                "the shipped scripts no longer bind {combo:?}; they bind {bound:?}"
            );
        }

        // Every one that fails, not the first: eleven at once is a different
        // bug from one, and a report that stops at the first cannot say which.
        let (keymap, indices) = us_keymap();
        let codes = keymap.min_keycode().raw()..=keymap.max_keycode().raw();
        let dead: Vec<&String> = bound
            .iter()
            .filter(|combo| {
                let combo = normalise_combo(combo);
                let held = held_for(&combo);
                !codes
                    .clone()
                    .any(|code| names_for(&keymap, &indices, code, &held).contains(&combo))
            })
            .collect();
        assert!(
            dead.is_empty(),
            "shipped scripts bind {dead:?}, and for each no key on a `us` keyboard, held \
             under the modifiers it names, answers to that name -- they can never fire"
        );
    }
}

/// The layouts, against the real `tiling.lua` and `scrolling.lua`.
///
/// Issue #72's harder half is not the protocol plumbing -- that is one state, a
/// handler and a delegate macro, and it is exercised by the compositor starting
/// at all. It is what the *layouts* do with a modal dialog, and that is Lua, so
/// a test of it has to run Lua.
///
/// So this runs the shipped scripts, unmodified, resolved through the same
/// `package.path` the compositor sets, against a snapshot of synthetic windows
/// -- and reads the `sol.place` commands that come back. That is the whole of
/// what a layout does, so it is the whole of what there is to check.
///
/// **Both layouts are driven by the same assertions, from one list.** A modal
/// that is right in tiling and wrong in scrolling is the failure this is most
/// likely to have, and two near-identical test functions is how it would be
/// missed: somebody fixes one and copies the test, and the copy asserts about
/// the layout it was copied from.
///
/// What is *not* here: `set_modal` arriving over the wire. That needs a Wayland
/// client, a display and a socket, and there is nothing in this crate's tests
/// that can build one -- so the step from "a client called `set_modal`" to
/// "`WindowInfo::modal` is true" is covered by the type system and by running
/// the compositor, and not by this. The snapshot is built by hand here, which
/// is honest about where the seam is.
#[cfg(test)]
mod dialogs {
    use super::{Command, Drawn, MonitorInfo, Parentage, Rect, Scripts, Snapshot, WindowInfo};
    use crate::present::Frame;
    use smithay::utils::{Logical, Rectangle};

    /// One screen, no bar, round numbers so a wrong answer reads as a wrong
    /// place rather than as arithmetic.
    const AREA: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 2560.0,
        h: 1440.0,
    };

    /// The second screen, where there are two: the same size, immediately to the
    /// right of the first. See [`second_monitor`].
    const BESIDE: Rect = Rect {
        x: 2560.0,
        y: 0.0,
        w: 2560.0,
        h: 1440.0,
    };

    /// Both layouts, by the module that defines them and the key that turns
    /// them on. Every assertion below runs against both; see the module note.
    const LAYOUTS: [(&str, &str); 2] = [("tiling", "super+t"), ("scrolling", "super+s")];

    /// The shipped scripts, with one layout required and nothing else.
    ///
    /// `package.path` is written by the script rather than left to
    /// `Scripts::load`, which prepends the *developer's* `~/.config/solium` --
    /// so on a machine with a user configuration this would silently test that
    /// instead. The same trap `group.rs`'s `desk` fixture documents.
    ///
    /// No `config.lua` of its own: the shipped defaults are what users get, and
    /// a dialog that is only centred under a hand-written configuration is a
    /// dialog that is not centred.
    ///
    /// A directory per *call*, not per layout, and the serial number is not
    /// decoration. `cargo test` runs these on several threads and more than one
    /// test arranges the same layout, so a path derived from the layout name is
    /// two threads writing and reading one `init.lua` at once. The symptom is
    /// not a wrong configuration -- every test writes the same bytes -- it is a
    /// truncated read, and it surfaces as "the layout key was not handled" in
    /// whichever test lost the race. Observed while mutation-testing this
    /// module: a change that should have failed one assertion failed a
    /// different test's, in a different file. `group.rs`'s `desk` fixture
    /// documents the same trap one step less far.
    fn scripts(name: &str, layout: &str) -> Scripts {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "solium-dialogs-{name}-{}-{serial}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&directory);
        let entry = directory.join("init.lua");
        std::fs::write(
            &entry,
            format!(
                "package.path = {shipped:?} .. \"/?.lua\"\n\
                 require(\"modes\")\n\
                 require({layout:?})\n",
                shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
            ),
        )
        .expect("writing the entry point");
        Scripts::load(&entry).expect("loading the shipped layout")
    }

    fn monitor() -> MonitorInfo {
        MonitorInfo {
            name: "DP-1".to_owned(),
            area: AREA,
            whole: AREA,
            scale: 1.0,
            focused: true,
            primary: true,
            transform: "normal".to_owned(),
            off: false,
        }
    }

    /// The screen to the right of [`monitor`], in the one global coordinate
    /// space the compositor has: same size, offset by its whole width.
    ///
    /// Not focused and not primary, because the point of it is to be the screen
    /// nothing falls back to. Every wrong answer about where a dialog goes lands
    /// on `DP-1` — it is the monitor at the origin, where every toplevel is
    /// mapped, and the one `sol.monitor()` answers — so an assertion that a
    /// dialog is on `DP-2` cannot pass by accident.
    fn second_monitor() -> MonitorInfo {
        MonitorInfo {
            name: "DP-2".to_owned(),
            area: BESIDE,
            whole: BESIDE,
            scale: 1.0,
            focused: false,
            primary: false,
            transform: "normal".to_owned(),
            off: false,
        }
    }

    /// A window the compositor has decided is on this monitor.
    ///
    /// Both halves, because the compositor sets both and a layout reads them
    /// from different places: `monitor` is what `monitors.each` groups by, and
    /// the rect is what decides the answer again on the next pass
    /// (`Solium::output_of`). A fixture that moved one without the other would
    /// be a state the compositor never produces.
    fn on(monitor: &MonitorInfo, mut window: WindowInfo) -> WindowInfo {
        window.monitor.clone_from(&monitor.name);
        window.rect.x = monitor.area.x;
        window.rect.y = monitor.area.y;
        window
    }

    /// A window as it is before any layout has had a say: the size its client
    /// chose, in the corner a Wayland toplevel is mapped at.
    fn window(id: u64, w: f64, h: f64) -> WindowInfo {
        WindowInfo {
            icon: None,
            client_side: false,
            blur_region: false,
            insets: crate::decoration::Insets::default(),
            id,
            rect: Rect {
                x: 0.0,
                y: 0.0,
                w,
                h,
            },
            drawn: Drawn::default(),
            title: format!("window {id}"),
            focused: false,
            monitor: "DP-1".to_owned(),
            modal: false,
            parent: Parentage::None,
            leaving: false,
            minimized: false,
            above: false,
            app_id: String::new(),
            min: None,
            max: None,
            cramped: false,
            shown: true,
            fullscreen: false,
            maximized: false,
            style: String::new(),
            x11_type: None,
            accepts_input: true,
            class: None,
            instance: None,
            content_type: "none",
            pid: None,
        }
    }

    /// Drawn at `rect`, where it lives: a window at rest, as the compositor
    /// hands it to a script that has just placed it -- so `sol.window_at`
    /// finds it there, which is what makes
    /// `a_dropped_window_goes_to_the_tile_it_was_let_go_over` a drop onto
    /// window 1. No screens in these snapshots, so no screen refuses it.
    fn drawn(rect: Rect) -> Drawn {
        let rect = Rectangle::<f64, Logical>::new((rect.x, rect.y).into(), (rect.w, rect.h).into());
        let slot = rect.to_i32_round();
        Drawn {
            slot,
            frame: Frame {
                rect,
                ..Frame::real(slot)
            },
        }
    }

    fn modal(id: u64, w: f64, h: f64, parent: Parentage) -> WindowInfo {
        WindowInfo {
            icon: None,
            client_side: false,
            blur_region: false,
            insets: crate::decoration::Insets::default(),
            modal: true,
            parent,
            ..window(id, w, h)
        }
    }

    /// What the compositor looked like, with these monitors and these windows.
    ///
    /// The monitor list is a parameter rather than a constant because it was a
    /// constant, and a one-screen fixture cannot see a whole class of bug: every
    /// window is mapped at (0, 0), so with one monitor every window is on the
    /// right monitor no matter what a layout does with the answer. `work_area`
    /// follows the focused screen, which is what `sol.monitor()` reports.
    fn snapshot_on(monitors: Vec<MonitorInfo>, windows: Vec<WindowInfo>) -> Snapshot {
        let work_area = monitors
            .iter()
            .find(|monitor| monitor.focused)
            .or_else(|| monitors.first())
            .map_or(AREA, |monitor| monitor.area);
        Snapshot {
            windows,
            monitors,
            work_area,
            ..Snapshot::default()
        }
    }

    fn snapshot(windows: Vec<WindowInfo>) -> Snapshot {
        snapshot_on(vec![monitor()], windows)
    }

    /// Where each window was told to go, by id.
    ///
    /// A layout may place the same window twice in one pass; the last one is
    /// what the compositor applies, so it is what this reports.
    fn placed(commands: &[Command]) -> std::collections::HashMap<u64, Rect> {
        let mut out = std::collections::HashMap::new();
        for command in commands {
            if let Command::Place { id, rect, .. } = command {
                out.insert(*id, *rect);
            }
        }
        out
    }

    /// **`sol.windows()` says what a window's frame takes on each side**
    /// (#435): what a configuration putting a window back needs to tell the
    /// frame from the client.
    #[test]
    fn sol_windows_says_what_a_windows_frame_takes() {
        let directory = std::env::temp_dir().join("solium-script-test-window-insets");
        let _ = std::fs::create_dir_all(&directory);
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("Super+I", function()
                local w = sol.windows()[1]
                sol.place(w.id, {
                    x = w.insets.left, y = w.insets.top,
                    w = w.insets.right, h = w.insets.bottom,
                })
            end)
            "#,
        )
        .expect("writing the test script");
        let mut scripts = Scripts::load(&config).expect("loading the test script");
        let framed = WindowInfo {
            icon: None,
            client_side: false,
            blur_region: false,
            insets: crate::decoration::Insets {
                top: 32,
                right: 1,
                bottom: 2,
                left: 3,
            },
            ..window(7, 400.0, 300.0)
        };
        let outcome = scripts.key("super+i", snapshot(vec![framed]));
        assert!(outcome.handled);
        let rect = placed(&outcome.commands).get(&7).copied().expect("placed");
        assert_eq!(
            (rect.x, rect.y, rect.w, rect.h),
            (3.0, 32.0, 1.0, 2.0),
            "left, top, right and bottom, as the compositor has them"
        );
    }

    /// Rects out of Lua are `f64` through a divide or two, so they are compared
    /// with a tolerance rather than exactly. A pixel is far below anything
    /// these assertions are about and far above the error.
    fn about(left: f64, right: f64) -> bool {
        (left - right).abs() < 1.0
    }

    fn centre(rect: Rect) -> (f64, f64) {
        (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0)
    }

    fn overlap(left: Rect, right: Rect) -> bool {
        left.x < right.x + right.w
            && right.x < left.x + left.w
            && left.y < right.y + right.h
            && right.y < left.y + left.h
    }

    /// Turn a layout on with these windows, and report where everything went.
    ///
    /// Switching the mode on runs `started`, which adopts the windows and lays
    /// them out -- the same path a user pressing the key takes.
    fn arrange_on(
        name: &str,
        monitors: Vec<MonitorInfo>,
        windows: Vec<WindowInfo>,
    ) -> std::collections::HashMap<u64, Rect> {
        let (module, key) = LAYOUTS
            .iter()
            .find(|(module, _)| *module == name)
            .copied()
            .expect("a layout this module knows");
        let mut scripts = scripts(name, module);
        let outcome = scripts.key(key, snapshot_on(monitors, windows));
        assert!(outcome.handled, "{name}: the layout key was not handled");
        placed(&outcome.commands)
    }

    fn arrange(name: &str, windows: Vec<WindowInfo>) -> std::collections::HashMap<u64, Rect> {
        arrange_on(name, vec![monitor()], windows)
    }

    /// Whether each window's placements in `commands` were tiles, by id, in
    /// the order they were sent. See `Command::Place::tile`.
    fn tiles(commands: &[Command]) -> std::collections::HashMap<u64, Vec<bool>> {
        let mut out: std::collections::HashMap<u64, Vec<bool>> = std::collections::HashMap::new();
        for command in commands {
            if let Command::Place { id, tile, .. } = command {
                out.entry(*id).or_default().push(*tile);
            }
        }
        out
    }

    /// **#133: a layout's arrangement is a tile, and a dialog it centres is
    /// not.**
    ///
    /// The compositor holds a tile's client inside it. That is what an
    /// arrangement wants -- a window that will not shrink as far as its slot
    /// is cut to the slot rather than drawn over its neighbour -- and it is
    /// wrong for a modal, which `dialogs.lua` centres over its parent at the
    /// size it had and which may grow afterwards. So `sol.place` defaults to a
    /// tile and `dialogs.place` says `tile = false`. Both layouts, since both
    /// go through `dialogs.place`.
    #[test]
    fn a_layouts_arrangement_is_a_tile_and_a_dialog_it_centres_is_not() {
        for (name, key) in LAYOUTS {
            let mut scripts = scripts(name, name);
            let outcome = scripts.key(
                key,
                snapshot(vec![
                    window(1, 800.0, 600.0),
                    window(2, 800.0, 600.0),
                    modal(3, 600.0, 400.0, Parentage::Window(1)),
                ]),
            );
            assert!(outcome.handled, "{name}: the layout key was not handled");
            let placed = tiles(&outcome.commands);
            for id in [1, 2] {
                assert!(
                    placed
                        .get(&id)
                        .is_some_and(|each| !each.is_empty() && each.iter().all(|tile| *tile)),
                    "{name}: window {id} is in the arrangement and was not placed as a tile: {placed:?}"
                );
            }
            assert!(
                placed
                    .get(&3)
                    .is_some_and(|each| !each.is_empty() && each.iter().all(|tile| !*tile)),
                "{name}: the dialog was placed as a tile, so a dialog that grows is cut to \
                 the size it was centred at: {placed:?}"
            );
        }
    }

    /// **A layout that stops lets every window out of its tile.**
    ///
    /// The mode is the script's, so the compositor cannot see a switch to
    /// floating happen: `modes.use` tells it, with `sol.unplace` for every
    /// window. Pressing a layout's key a second time is the switch to
    /// floating, and asserted here as the one that must send it for every
    /// window and place none. Turning a layout on sends it too -- the layout
    /// being replaced may have been another -- and then places everything,
    /// which the first half holds to: each window's last word is a tile.
    #[test]
    fn a_layout_that_stops_lets_every_window_out_of_its_tile() {
        for (name, key) in LAYOUTS {
            let mut scripts = scripts(name, name);
            let windows = || snapshot(vec![window(1, 800.0, 600.0), window(2, 800.0, 600.0)]);

            let on = scripts.key(key, windows());
            assert!(on.handled, "{name}: the layout key was not handled");
            for id in [1_u64, 2] {
                let last = on.commands.iter().rev().find_map(|command| match command {
                    Command::Place {
                        id: placed, tile, ..
                    } if *placed == id => Some(*tile),
                    Command::Unplace { id: unplaced } if *unplaced == id => Some(false),
                    _ => None,
                });
                assert_eq!(
                    last,
                    Some(true),
                    "{name}: turning the layout on left window {id} out of a tile"
                );
            }

            let off = scripts.key(key, windows());
            let mut unplaced: Vec<u64> = off
                .commands
                .iter()
                .filter_map(|command| match command {
                    Command::Unplace { id } => Some(*id),
                    _ => None,
                })
                .collect();
            unplaced.sort_unstable();
            assert_eq!(
                unplaced,
                vec![1, 2],
                "{name}: switching to floating did not let every window go"
            );
            assert!(
                tiles(&off.commands).is_empty(),
                "{name}: nothing is laid out once the layout has stopped"
            );
        }
    }

    /// **A modal dialog takes no share of the screen.**
    ///
    /// Stated as "the other windows are laid out exactly as they would be if
    /// the dialog were not there", which is the strongest form of it and the
    /// one that cannot pass by accident: a dialog that is merely *also* placed
    /// on top of its parent, while still holding a slot in the tree, gives the
    /// other windows different rects and fails here.
    #[test]
    fn a_modal_does_not_take_a_share_of_the_screen() {
        for (name, _) in LAYOUTS {
            let without = arrange(name, vec![window(1, 800.0, 600.0), window(2, 800.0, 600.0)]);
            let with = arrange(
                name,
                vec![
                    window(1, 800.0, 600.0),
                    window(2, 800.0, 600.0),
                    modal(3, 600.0, 400.0, Parentage::Window(1)),
                ],
            );

            for id in [1, 2] {
                let before = without.get(&id).copied().unwrap_or_default();
                let after = with.get(&id).copied().unwrap_or_default();
                assert!(
                    about(before.x, after.x)
                        && about(before.y, after.y)
                        && about(before.w, after.w)
                        && about(before.h, after.h),
                    "{name}: window {id} was given {after:?} with a dialog on screen and \
                     {before:?} without one -- the dialog is taking a share of the screen"
                );
            }
            assert!(
                with.contains_key(&3),
                "{name}: the dialog was left out of the arrangement and then never \
                 placed at all, which for a Wayland toplevel is the top-left corner"
            );
        }
    }

    /// **And it sits over the window waiting on it.**
    ///
    /// Centred on its parent's *new* slot, not on the rect the snapshot
    /// carried: the same pass that places the dialog has just moved the parent,
    /// so the snapshot rect is stale by the time it is read. Asserted against
    /// `placed[1]` for exactly that reason -- comparing with the input rect
    /// would pass whichever answer the script gave.
    #[test]
    fn a_modal_is_centred_on_its_parent() {
        for (name, _) in LAYOUTS {
            let out = arrange(
                name,
                vec![
                    window(1, 800.0, 600.0),
                    window(2, 800.0, 600.0),
                    modal(3, 600.0, 400.0, Parentage::Window(1)),
                ],
            );
            let parent = out.get(&1).copied().expect("the parent was placed");
            let dialog = out.get(&3).copied().expect("the dialog was placed");

            assert!(
                about(centre(parent).0, centre(dialog).0)
                    && about(centre(parent).1, centre(dialog).1),
                "{name}: the dialog is at {dialog:?}, centred on {:?}, but its parent is \
                 at {parent:?}, centred on {:?}",
                centre(dialog),
                centre(parent)
            );
            // Its own size, kept. A dialog knows how big it needs to be and a
            // layout that resizes it wraps the question onto four lines.
            assert!(
                about(dialog.w, 600.0) && about(dialog.h, 400.0),
                "{name}: the dialog was resized to {}x{}",
                dialog.w,
                dialog.h
            );
            // The other window is elsewhere, so this is a dialog over its own
            // parent rather than a dialog in the middle of the screen that
            // happens to be over everything.
            let other = out.get(&2).copied().expect("the other window was placed");
            assert!(
                !about(centre(parent).0, centre(other).0)
                    || !about(centre(parent).1, centre(other).1),
                "{name}: both windows are in the same place, so centring on either \
                 proves nothing"
            );
        }
    }

    /// **And it follows its parent onto the other screen.**
    ///
    /// The case the single-monitor fixture could not see, and the reason the
    /// fixture takes a monitor list now. Every toplevel is mapped at (0, 0) and
    /// the compositor decides which screen a window is on from the centre of its
    /// rect, so a dialog for a window on `DP-2` *arrives* belonging to `DP-1` —
    /// this is the state the compositor really hands a layout, not a contrived
    /// one. Clamp it into its own screen's work area and it is pinned to that
    /// screen's edge; its centre stays there, so the next pass reads back the
    /// same monitor and clamps it identically. It never converges, which is why
    /// a second pass could exist for exactly this case and not deliver it.
    ///
    /// Asserted two ways on purpose. Centred on its parent is the property; on
    /// `DP-2` at all is the one that fails loudly when the clamp is taken from
    /// the wrong screen, because the wrong answer is not a few pixels out — it
    /// is a whole monitor away, against the edge nearest the parent.
    #[test]
    fn a_modal_is_centred_on_its_parents_monitor_and_not_its_own() {
        for (name, _) in LAYOUTS {
            let screens = vec![monitor(), second_monitor()];
            let out = arrange_on(
                name,
                screens,
                vec![
                    // The parent, on the second screen.
                    on(&second_monitor(), window(1, 800.0, 600.0)),
                    // An unrelated window on the first, so the dialog cannot
                    // land on an empty screen and look right by default.
                    on(&monitor(), window(2, 800.0, 600.0)),
                    // The dialog, where the compositor puts a new toplevel: the
                    // origin, which is the *first* screen.
                    modal(3, 600.0, 400.0, Parentage::Window(1)),
                ],
            );

            let parent = out.get(&1).copied().expect("the parent was placed");
            let dialog = out.get(&3).copied().expect("the dialog was placed");

            assert!(
                parent.x >= BESIDE.x,
                "{name}: the parent was laid out at {parent:?}, which is not on DP-2 \
                 ({BESIDE:?}) -- the fixture is wrong before the dialog is even asked \
                 about"
            );
            assert!(
                about(centre(parent).0, centre(dialog).0)
                    && about(centre(parent).1, centre(dialog).1),
                "{name}: the dialog is at {dialog:?}, centred on {:?}, and its parent is \
                 on the other screen at {parent:?}, centred on {:?}",
                centre(dialog),
                centre(parent)
            );
            assert!(
                dialog.x >= BESIDE.x
                    && dialog.y >= BESIDE.y
                    && dialog.x + dialog.w <= BESIDE.x + BESIDE.w
                    && dialog.y + dialog.h <= BESIDE.y + BESIDE.h,
                "{name}: the dialog at {dialog:?} is not on DP-2 ({BESIDE:?}) at all -- it \
                 was clamped into the work area of the screen it was mapped on rather \
                 than its parent's"
            );
        }
    }

    /// **A modal on a modal is centred on where that modal ended up.**
    ///
    /// A file chooser is modal for the document and its "Replace?" prompt is
    /// modal for the chooser: two dialogs, one waiting on the other, and nothing
    /// arranges either of them. So the chooser's rect in this pass is one only
    /// this pass knows — which is what `placed` is for, and a dialog has to be
    /// written into it like any other placement.
    ///
    /// The prompt is listed *first*, which is the order that matters: the
    /// snapshot is topmost first and a prompt is above the chooser that opened
    /// it, so the naive pass reaches the child before the parent exists
    /// anywhere. Left out of `placed`, the child falls through to the snapshot
    /// — where its parent is still at the (0, 0) every toplevel is mapped at —
    /// and lands in the top-left corner.
    #[test]
    fn a_modal_waiting_on_a_modal_is_centred_on_it() {
        for (name, _) in LAYOUTS {
            let out = arrange_on(
                name,
                vec![monitor()],
                vec![
                    modal(3, 300.0, 200.0, Parentage::Window(2)),
                    modal(2, 600.0, 400.0, Parentage::Window(1)),
                    window(1, 800.0, 600.0),
                    window(4, 800.0, 600.0),
                ],
            );

            let document = out.get(&1).copied().expect("the document was placed");
            let chooser = out.get(&2).copied().expect("the chooser was placed");
            let prompt = out.get(&3).copied().expect("the prompt was placed");

            assert!(
                about(centre(document).0, centre(chooser).0)
                    && about(centre(document).1, centre(chooser).1),
                "{name}: the chooser at {chooser:?} is not over the document at \
                 {document:?}, so nothing below proves anything"
            );
            assert!(
                about(centre(chooser).0, centre(prompt).0)
                    && about(centre(chooser).1, centre(prompt).1),
                "{name}: the prompt is at {prompt:?}, centred on {:?}, and the chooser it \
                 is waiting on is at {chooser:?}, centred on {:?} -- the prompt was \
                 centred on where the chooser was in the snapshot rather than on where \
                 this pass put it",
                centre(prompt),
                centre(chooser)
            );
        }
    }

    /// **A modal whose parent cannot be found still floats.**
    ///
    /// Both ways of having nothing to centre on: the client named a parent this
    /// compositor cannot point at (`Unknown` -- not mapped, not ours, or gone
    /// while the dialog was up) and the client named none at all (`None`).
    ///
    /// The two things that must not happen are "tiled" and "lost": it keeps its
    /// size, it does not push the other windows around, and every edge of it is
    /// on the screen. A dialog at the origin passes "not tiled" and is still
    /// under the panel in the corner, so the second half is the one that
    /// matters.
    #[test]
    fn a_modal_with_no_parent_to_find_is_centred_on_the_screen() {
        for (name, _) in LAYOUTS {
            for parent in [Parentage::Unknown, Parentage::None] {
                let without = arrange(name, vec![window(1, 800.0, 600.0)]);
                let out = arrange(
                    name,
                    vec![window(1, 800.0, 600.0), modal(3, 600.0, 400.0, parent)],
                );
                let dialog = out.get(&3).copied().expect("the dialog was placed");

                let before = without.get(&1).copied().unwrap_or_default();
                let after = out.get(&1).copied().unwrap_or_default();
                assert!(
                    about(before.w, after.w) && about(before.h, after.h),
                    "{name}/{parent:?}: the window lost room to a dialog with no parent"
                );

                assert!(
                    about(centre(dialog).0, centre(AREA).0)
                        && about(centre(dialog).1, centre(AREA).1),
                    "{name}/{parent:?}: a parentless dialog went to {dialog:?} rather than \
                     the middle of the screen"
                );
                assert!(
                    dialog.x >= AREA.x
                        && dialog.y >= AREA.y
                        && dialog.x + dialog.w <= AREA.x + AREA.w
                        && dialog.y + dialog.h <= AREA.y + AREA.h,
                    "{name}/{parent:?}: the dialog at {dialog:?} hangs off the work area \
                     {AREA:?}"
                );
            }
        }
    }

    /// **`unset_modal` puts it back in the arrangement.**
    ///
    /// The compositor turns `unset_modal` into a relayout -- see
    /// `XdgDialogHandler::modal_changed` -- so the second pass here is exactly
    /// what the compositor does: the same scripts, the same windows, one of
    /// them no longer modal.
    ///
    /// Rejoining is asserted two ways, because either alone is weak. The
    /// arrangement resizes it -- a floating dialog keeps the size its client
    /// chose, and this one no longer has it; and nothing overlaps anything,
    /// which is a property an arrangement has and a floating window does not,
    /// since a modal overlaps its parent by construction.
    ///
    /// What is deliberately *not* asserted is "the other windows gave up room".
    /// It is true of tiling and false of scrolling on purpose: a column's width
    /// is a share of the view, so a third column makes the strip longer rather
    /// than the first two thinner. That is the first paragraph of
    /// `scrolling.lua` and the whole reason a scroller is not a row of windows
    /// squeezed to fit, so a test that demanded it would be a test demanding
    /// the wrong layout.
    #[test]
    fn unset_modal_returns_a_dialog_to_the_arrangement() {
        for (name, key) in LAYOUTS {
            let mut scripts = scripts(&format!("{name}-unset"), name);
            let outcome = scripts.key(
                key,
                snapshot(vec![
                    window(1, 800.0, 600.0),
                    window(2, 800.0, 600.0),
                    modal(3, 600.0, 400.0, Parentage::Window(1)),
                ]),
            );
            assert!(outcome.handled, "{name}: the layout key was not handled");
            let floating = placed(&outcome.commands);

            // The client called `unset_modal`; everything else is unchanged.
            let outcome = scripts.relayout(snapshot(vec![
                window(1, 800.0, 600.0),
                window(2, 800.0, 600.0),
                window(3, 600.0, 400.0),
            ]));
            let tiled = placed(&outcome.commands);

            let was = floating.get(&3).copied().expect("the dialog was placed");
            let now = tiled.get(&3).copied().expect("the dialog is still placed");
            assert!(
                about(was.w, 600.0) && about(was.h, 400.0),
                "{name}: the dialog was {was:?} while modal, so it was never floating \
                 at its own size and this test proves nothing about it stopping"
            );
            assert!(
                !about(now.w, 600.0) || !about(now.h, 400.0),
                "{name}: the dialog still has the size its client chose ({now:?}) after \
                 `unset_modal`, so it is floating rather than arranged"
            );

            let rects: Vec<(u64, Rect)> = [1, 2, 3]
                .into_iter()
                .map(|id| {
                    (
                        id,
                        tiled
                            .get(&id)
                            .copied()
                            .unwrap_or_else(|| panic!("{name}: window {id} was not placed")),
                    )
                })
                .collect();
            for (index, (id, rect)) in rects.iter().enumerate() {
                for (other, against) in rects.iter().skip(index + 1) {
                    assert!(
                        !overlap(*rect, *against),
                        "{name}: {id} at {rect:?} overlaps {other} at {against:?}, so one \
                         of them is still floating rather than arranged"
                    );
                }
            }
        }
    }

    /// **#128: a layout reflows the moment a close is asked for.**
    ///
    /// Against the shipped `tiling.lua` and `scrolling.lua`, the same way the
    /// dialog tests above run them: synthetic snapshots in, `sol.place`
    /// commands out. What the compositor does around these events -- when it
    /// sends them, and what a window being closed looks like meanwhile -- is
    /// `state.rs`'s `reflow_on_close` module; this is what the layouts do with
    /// them.
    mod reflow_on_close {
        use super::*;
        use std::collections::HashMap;

        /// The shipped layout, with `before` run ahead of it -- a setting,
        /// usually -- and the entry point, which a reload needs to read again.
        ///
        /// A directory per call, for the reason [`scripts`] gives.
        fn layout_with(layout: &str, before: &str) -> (Scripts, std::path::PathBuf) {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let directory = std::env::temp_dir().join(format!(
                "solium-reflow-{layout}-{}-{serial}",
                std::process::id()
            ));
            let _ = std::fs::create_dir_all(&directory);
            let entry = directory.join("init.lua");
            std::fs::write(
                &entry,
                format!(
                    "package.path = {shipped:?} .. \"/?.lua\"\n\
                     {before}\n\
                     require(\"modes\")\n\
                     require({layout:?})\n",
                    shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
                ),
            )
            .expect("writing the entry point");
            let scripts = Scripts::load(&entry).expect("loading the shipped layout");
            (scripts, entry)
        }

        fn when_gone(layout: &str) -> String {
            format!("require(\"config\").{layout}.reflow_on_close = \"when_gone\"")
        }

        /// Windows `ids`, topmost first, each where `placed` put it -- so a
        /// layout reading the snapshot sees the arrangement it made -- and the
        /// ones in `leaving` marked as closing.
        fn desk(placed: &HashMap<u64, Rect>, ids: &[u64], leaving: &[u64]) -> Snapshot {
            snapshot(
                ids.iter()
                    .map(|&id| {
                        let rect = placed.get(&id).copied().unwrap_or_default();
                        WindowInfo {
                            icon: None,
                            client_side: false,
                            blur_region: false,
                            insets: crate::decoration::Insets::default(),
                            rect,
                            drawn: drawn(rect),
                            leaving: leaving.contains(&id),
                            ..window(id, rect.w, rect.h)
                        }
                    })
                    .collect(),
            )
        }

        fn key_of(layout: &str) -> &'static str {
            LAYOUTS
                .iter()
                .find(|(module, _)| *module == layout)
                .map(|(_, key)| *key)
                .expect("a layout this module knows")
        }

        /// The layout switched on over three windows, and where it put them.
        fn arranged(scripts: &mut Scripts, layout: &str) -> HashMap<u64, Rect> {
            arranged_over(scripts, layout, 3)
        }

        /// The same over windows `1..=count`.
        fn arranged_over(scripts: &mut Scripts, layout: &str, count: u64) -> HashMap<u64, Rect> {
            let outcome = scripts.key(
                key_of(layout),
                snapshot((1..=count).map(|id| window(id, 800.0, 600.0)).collect()),
            );
            assert!(outcome.handled, "{layout}: the layout key was not handled");
            let out = placed(&outcome.commands);
            assert_eq!(
                out.len(),
                usize::try_from(count).unwrap_or_default(),
                "{layout}: not every window was placed: {out:?}"
            );
            out
        }

        /// A monitor arriving, as the compositor announces one: `monitors`,
        /// which is where a layout adopts, then `layout`, which is where it
        /// places -- `Solium::settle_monitors`, in that order. What comes back
        /// is where the second put everything.
        fn adopt(scripts: &mut Scripts, snapshot: Snapshot) -> HashMap<u64, Rect> {
            let _ = scripts.monitors_changed(snapshot.clone());
            placed(&scripts.relayout(snapshot).commands)
        }

        fn same(left: Rect, right: Rect) -> bool {
            about(left.x, right.x)
                && about(left.y, right.y)
                && about(left.w, right.w)
                && about(left.h, right.h)
        }

        /// **`closing` gives the survivors exactly what `close` would have, a
        /// whole client's lifetime sooner.**
        ///
        /// Compared against the same layout set to `"when_gone"` and told
        /// `close`, which is what every close did before #128 -- so the claim is
        /// not "something moved" but "the arrangement the user would have got
        /// is the one they get, at the moment they asked". And the window being
        /// closed is placed by neither: it is fading where it stood, and a slot
        /// for it would be a slot for nothing.
        #[test]
        fn closing_reflows_the_survivors_as_close_would_have() {
            for (layout, _) in LAYOUTS {
                let mut moved = false;
                for dying in [1, 2, 3] {
                    let survivors: Vec<u64> =
                        [1, 2, 3].into_iter().filter(|id| *id != dying).collect();

                    let (mut immediate, _) = layout_with(layout, "");
                    let before = arranged(&mut immediate, layout);
                    let early = placed(
                        &immediate
                            .closing(dying, desk(&before, &[1, 2, 3], &[dying]))
                            .commands,
                    );

                    let (mut waiting, _) = layout_with(layout, &when_gone(layout));
                    let was = arranged(&mut waiting, layout);
                    let told = waiting.closing(dying, desk(&was, &[1, 2, 3], &[dying]));
                    assert!(
                        placed(&told.commands).is_empty(),
                        "{layout}/{dying}: waiting for the client, `closing` placed something: {:?}",
                        told.commands
                    );
                    let late = placed(
                        &waiting
                            .closed(dying, desk(&was, &[1, 2, 3], &[dying]))
                            .commands,
                    );

                    assert!(
                        !early.contains_key(&dying),
                        "{layout}/{dying}: the window being closed was given a slot: {:?}",
                        early.get(&dying)
                    );
                    for id in &survivors {
                        let now = early.get(id).copied();
                        let then = late.get(id).copied();
                        assert!(
                            now.zip(then).is_some_and(|(now, then)| same(now, then)),
                            "{layout}/{dying}: window {id} got {now:?} when the close was asked \
                             for, and {then:?} when the client went -- the reflow is not the \
                             same one"
                        );
                    }
                    moved |= survivors.iter().any(|id| {
                        early
                            .get(id)
                            .zip(before.get(id))
                            .is_some_and(|(now, was)| !same(*now, *was))
                    });
                }
                // Rules out a layout that placed everything where it was: equal
                // to `close`'s answer only means something if that answer moves
                // a window. Across the three rather than for each, because this
                // is only here to keep the comparison from being empty; which
                // closes move which windows is the layout's own business.
                assert!(
                    moved,
                    "{layout}: no close moved anything, so this compared nothing"
                );
            }
        }

        /// **A refused close in tiling comes back where it was.**
        ///
        /// `closing` keeps the centre of where the window stood, and `refused`
        /// splits whichever window covers it now. For a window whose sibling was
        /// a single window, that is the sibling, grown into the whole of their
        /// parent's box -- so the axis and the side both come back and the
        /// arrangement is the one it was, rect for rect. Windows 1 and 3 are
        /// that case in the three-window arrangement `adopt` builds (1 over 3,
        /// both beside 2).
        ///
        /// Window 2's sibling was the pair, so it comes back splitting one of
        /// them: accepted in the design, and asserted only as "back in the
        /// arrangement, overlapping nothing".
        #[test]
        fn a_refused_close_in_tiling_comes_back_where_it_was() {
            for dying in [1, 3] {
                let (mut scripts, _) = layout_with("tiling", "");
                let before = arranged(&mut scripts, "tiling");
                let closed = placed(
                    &scripts
                        .closing(dying, desk(&before, &[1, 2, 3], &[dying]))
                        .commands,
                );
                let mut now = before.clone();
                now.extend(closed);
                let back = placed(&scripts.refused(dying, desk(&now, &[1, 2, 3], &[])).commands);
                for id in [1, 2, 3] {
                    let was = before.get(&id).copied();
                    let is = back.get(&id).copied();
                    assert!(
                        was.zip(is).is_some_and(|(was, is)| same(was, is)),
                        "window {id} was at {was:?} before {dying} was closed, and is at {is:?} \
                         after the close was refused"
                    );
                }
            }

            let (mut scripts, _) = layout_with("tiling", "");
            let before = arranged(&mut scripts, "tiling");
            let closed = placed(&scripts.closing(2, desk(&before, &[1, 2, 3], &[2])).commands);
            let mut now = before.clone();
            now.extend(closed);
            let back = placed(&scripts.refused(2, desk(&now, &[1, 2, 3], &[])).commands);
            let rects: Vec<Rect> = [1, 2, 3]
                .iter()
                .filter_map(|id| back.get(id).copied())
                .collect();
            assert_eq!(
                rects.len(),
                3,
                "a refused window is back in the arrangement: {back:?}"
            );
            for (index, rect) in rects.iter().enumerate() {
                for other in rects.iter().skip(index + 1) {
                    assert!(!overlap(*rect, *other), "{rect:?} overlaps {other:?}");
                }
            }

            // And a refusal for a window the tree still holds -- one whose
            // `closing` never reached this layout -- inserts nothing a second
            // time, which is the insert the handler exists to guard.
            let told = scripts.refused(1, desk(&back, &[1, 2, 3], &[]));
            assert!(
                placed(&told.commands).is_empty(),
                "a window that is in the tree was put in it again: {:?}",
                told.commands
            );
        }

        /// **A refused close in scrolling comes back in its place in the
        /// strip.**
        ///
        /// Order and adjacency rather than exact rects, because the strip's
        /// *view* follows its focused column, and putting a window back focuses
        /// it: the columns are where they were relative to each other, and the
        /// whole strip may be scrolled. Every position is asserted, the first
        /// included, because the first has no window on its left to go after
        /// and is where a plain `insert` would put it on the far side of the
        /// focused column instead.
        #[test]
        fn a_refused_close_in_scrolling_comes_back_in_its_place() {
            for dying in [1, 2, 3] {
                let (mut scripts, _) = layout_with("scrolling", "");
                let before = arranged(&mut scripts, "scrolling");
                let mut order: Vec<u64> = vec![1, 2, 3];
                order.sort_by(|a, b| before[a].x.total_cmp(&before[b].x));

                let closed = placed(
                    &scripts
                        .closing(dying, desk(&before, &[1, 2, 3], &[dying]))
                        .commands,
                );
                let mut now = before.clone();
                now.extend(closed);
                let back = placed(&scripts.refused(dying, desk(&now, &[1, 2, 3], &[])).commands);

                let mut returned: Vec<u64> = vec![1, 2, 3];
                assert!(
                    returned.iter().all(|id| back.contains_key(id)),
                    "{dying}: not every window was placed after the refusal: {back:?}"
                );
                returned.sort_by(|a, b| back[a].x.total_cmp(&back[b].x));
                assert_eq!(
                    returned, order,
                    "the strip read {order:?} before {dying} was closed and {returned:?} after the \
                     close was refused"
                );
                for pair in returned.windows(2) {
                    let (left, right) = (back[&pair[0]], back[&pair[1]]);
                    assert!(
                        about(left.x + left.w + 12.0, right.x),
                        "{dying}: {} ends at {} and {} starts at {}, so they are not neighbours",
                        pair[0],
                        left.x + left.w,
                        pair[1],
                        right.x
                    );
                }
            }

            // The window after it closes too, before the refusal: the left
            // neighbour is the only one left to go after, and a strip that
            // looked for the right one would fall through to a plain `insert`
            // and put the window after the focused column instead.
            let (mut scripts, _) = layout_with("scrolling", "");
            let before = arranged_over(&mut scripts, "scrolling", 4);
            let mut order: Vec<u64> = vec![1, 2, 3, 4];
            order.sort_by(|a, b| before[a].x.total_cmp(&before[b].x));
            let (dying, next) = (order[1], order[2]);
            let mut now = before.clone();
            now.extend(placed(
                &scripts
                    .closing(dying, desk(&now, &[1, 2, 3, 4], &[dying]))
                    .commands,
            ));
            now.extend(placed(
                &scripts
                    .closing(next, desk(&now, &[1, 2, 3, 4], &[dying, next]))
                    .commands,
            ));
            let back = placed(
                &scripts
                    .refused(dying, desk(&now, &[1, 2, 3, 4], &[next]))
                    .commands,
            );
            let mut returned: Vec<u64> = order.iter().copied().filter(|id| *id != next).collect();
            returned.sort_by(|a, b| back[a].x.total_cmp(&back[b].x));
            let expected: Vec<u64> = order.iter().copied().filter(|id| *id != next).collect();
            assert_eq!(
                returned, expected,
                "with the window after it gone as well, {dying} did not come back after the one                  before it"
            );
        }

        /// **Set to `"when_gone"`, `closing` and `refused` do nothing, and
        /// `adopt` treats a window being closed as the ordinary window it
        /// always was.**
        ///
        /// The first half is in `closing_reflows_the_survivors_as_close_would_have`,
        /// which compares against this very setting's `close`. This is the rest:
        /// a refusal has nothing to undo, and a `monitors` event during the
        /// close still places the window being closed -- its tile stays reserved
        /// until `close`, which is what waiting means.
        #[test]
        fn when_gone_waits_for_the_client_as_every_close_used_to() {
            for (layout, _) in LAYOUTS {
                let (mut scripts, _) = layout_with(layout, &when_gone(layout));
                let before = arranged(&mut scripts, layout);
                let told = scripts.closing(3, desk(&before, &[1, 2, 3], &[3]));
                assert!(
                    told.commands.is_empty(),
                    "{layout}: `closing` asked for {:?}",
                    told.commands
                );
                let adopted = adopt(&mut scripts, desk(&before, &[1, 2, 3], &[3]));
                assert!(
                    adopted.contains_key(&3),
                    "{layout}: waiting for the client, a monitors event dropped the window being \
                     closed out of the arrangement: {adopted:?}"
                );
                let told = scripts.refused(3, desk(&before, &[1, 2, 3], &[]));
                assert!(
                    told.commands.is_empty(),
                    "{layout}: `refused` asked for {:?}",
                    told.commands
                );
            }
        }

        /// **A setting spelt some other way reads as the default.** A typo in a
        /// value is not something `config.lua`'s unknown-key report can see, so
        /// what it costs is decided here: the default, rather than the old
        /// behaviour.
        #[test]
        fn a_misspelt_reflow_setting_reads_as_immediate() {
            for (layout, _) in LAYOUTS {
                let (mut scripts, _) = layout_with(
                    layout,
                    &format!("require(\"config\").{layout}.reflow_on_close = \"when-gone\""),
                );
                let before = arranged(&mut scripts, layout);
                let early = placed(&scripts.closing(3, desk(&before, &[1, 2, 3], &[3])).commands);
                assert!(
                    !early.is_empty() && !early.contains_key(&3),
                    "{layout}: a misspelt setting did not reflow at once: {early:?}"
                );
            }
        }

        /// **`adopt` during a close does not put the window being closed back.**
        ///
        /// `monitors` and `restore` are the two events that run `adopt` on a
        /// live session -- a screen plugged in, `super+shift+r` -- and `adopt`
        /// exists to put back whatever the arrangement is missing. A window
        /// `closing` took out is missing on purpose.
        ///
        /// Three cases. After `closing`, it stays out. With no `closing` at all
        /// -- an event some route failed to deliver -- `adopt` takes it out
        /// itself, which is the backstop `adopt` has always been for a missed
        /// event. And after a reload, which carries the arrangement across
        /// without it, it is not put back in.
        #[test]
        fn adopt_during_a_close_leaves_the_window_out() {
            for (layout, _) in LAYOUTS {
                let (mut scripts, entry) = layout_with(layout, "");
                let before = arranged(&mut scripts, layout);
                let closed = placed(&scripts.closing(3, desk(&before, &[1, 2, 3], &[3])).commands);
                let adopted = adopt(&mut scripts, desk(&before, &[1, 2, 3], &[3]));
                assert!(
                    !adopted.contains_key(&3),
                    "{layout}: a monitors event during the close put the window being closed \
                     back: {adopted:?}"
                );
                for id in [1, 2] {
                    assert!(
                        adopted
                            .get(&id)
                            .zip(closed.get(&id))
                            .is_some_and(|(now, then)| same(*now, *then)),
                        "{layout}: window {id} moved on a monitors event that changed nothing"
                    );
                }

                // A reload: a fresh Lua state, the layout still current.
                let mut reloaded = Scripts::load_carrying(&entry, scripts.kept())
                    .expect("reloading the shipped layout");
                let restored = placed(&reloaded.restored(desk(&before, &[1, 2, 3], &[3])).commands);
                assert!(
                    restored.contains_key(&1) && !restored.contains_key(&3),
                    "{layout}: a reload during the close put the window being closed back \
                     into the arrangement: {restored:?}"
                );

                // `closing` never arrived.
                let (mut missed, _) = layout_with(layout, "");
                let before = arranged(&mut missed, layout);
                let adopted = adopt(&mut missed, desk(&before, &[1, 2, 3], &[3]));
                assert!(
                    !adopted.contains_key(&3) && adopted.contains_key(&1),
                    "{layout}: with no `closing`, a monitors event kept a slot for the window \
                     being closed: {adopted:?}"
                );
            }
        }

        /// **`view:windows()` is the strip in reading order**: columns left to
        /// right, and top to bottom inside a column. What `scrolling.lua` finds
        /// a closing window's neighbours in.
        #[test]
        fn a_strip_lists_its_windows_in_reading_order() {
            let (scripts, _) = layout_with("scrolling", "");
            let order = scripts.evaluate(
                "local view = sol.layout.scroller()\n\
                 local area = { x = 0, y = 0, w = 2560, h = 1440, gap = 12 }\n\
                 for id = 1, 4 do view:insert(id, area) end\n\
                 -- Focus the first column and pull the second's window into it.\n\
                 view:focus_window(1, area)\n\
                 view:consume()\n\
                 -- Then carry the last column to the front.\n\
                 view:focus_window(4, area)\n\
                 view:move_column(-1, area)\n\
                 view:move_column(-1, area)\n\
                 return table.concat(view:windows(), \",\")",
            );
            assert_eq!(
                order, "4,1,2,3",
                "the strip is [4] [1 over 2] [3], and read as {order}"
            );
        }

        /// Everything a layout holds, as text: every tree's or strip's
        /// arrangement of one screen, in key order, and for a strip which
        /// column it has focused. What a test compares to say a layout was
        /// left exactly as it was, since a layout that is not in charge places
        /// nothing and its commands say nothing.
        fn structure(scripts: &Scripts, layout: &str) -> String {
            let (table, extra) = if layout == "tiling" {
                ("trees", "")
            } else {
                ("views", " .. \" focus=\" .. tostring(each:focused())")
            };
            scripts.evaluate(&format!(
                "local layout = require({layout:?})\n\
                 local keys = {{}}\n\
                 for key in pairs(layout.{table}) do keys[#keys + 1] = key end\n\
                 table.sort(keys)\n\
                 local area = {{ x = 0, y = 0, w = 2560, h = 1440, gap = 12, split = 0.5 }}\n\
                 local out = {{}}\n\
                 for _, key in ipairs(keys) do\n\
                     local each = layout.{table}[key]\n\
                     local slots = {{}}\n\
                     for _, slot in ipairs(each:layout(area)) do\n\
                         slots[#slots + 1] = string.format(\"%d@%.0f,%.0f,%.0fx%.0f\",\n\
                             slot.id, slot.x, slot.y, slot.w, slot.h)\n\
                     end\n\
                     out[#out + 1] = key .. \"=\" .. table.concat(slots, \" \"){extra}\n\
                 end\n\
                 return table.concat(out, \" | \")"
            ))
        }

        /// Three windows opened while `layout` is not in charge, which is
        /// every session's start: floating is the default mode. Each layout
        /// hears `open` whether it is in charge or not and keeps the window in
        /// its structure, so there is something there to be disturbed.
        fn opened_while_floating(scripts: &mut Scripts) -> Vec<WindowInfo> {
            let windows: Vec<WindowInfo> = (1..=3_u32)
                .map(|n| {
                    let mut each = window(u64::from(n), 700.0, 500.0);
                    each.rect.x = f64::from(n) * 300.0;
                    each.rect.y = f64::from(n) * 150.0;
                    each
                })
                .collect();
            for count in 1..=windows.len() {
                let id = windows[count - 1].id;
                let _ = scripts.opened(id, snapshot(windows[..count].to_vec()));
            }
            windows
        }

        fn leaving(windows: &[WindowInfo], ids: &[u64]) -> Vec<WindowInfo> {
            windows
                .iter()
                .cloned()
                .map(|window| WindowInfo {
                    icon: None,
                    client_side: false,
                    blur_region: false,
                    insets: crate::decoration::Insets::default(),
                    leaving: ids.contains(&window.id),
                    ..window
                })
                .collect()
        }

        /// **A layout that is not in charge is left exactly as it was by a
        /// close and its refusal.**
        ///
        /// Floating is the default mode, and both layouts listen all the same.
        /// `closing` used to take the window out of a tree or strip nobody was
        /// using and record where it stood in some other mode's geometry, and
        /// `refused` put it back from that: at the configured split rather
        /// than its old one, or with the inactive strip's focus moved onto it
        /// -- which `scrolling.started` then hands the keyboard to when the
        /// user switches to it. Whichever layout the user had set to
        /// `"when_gone"`, the other still did it.
        #[test]
        fn a_layout_not_in_charge_is_left_as_it_was_by_a_refused_close() {
            for (layout, _) in LAYOUTS {
                let (mut scripts, _) = layout_with(layout, "");
                let windows = opened_while_floating(&mut scripts);
                let before = structure(&scripts, layout);
                assert!(
                    before.contains("2@"),
                    "{layout}: the premise is a structure holding the window: {before}"
                );

                let told = scripts.closing(2, snapshot(leaving(&windows, &[2])));
                assert!(
                    placed(&told.commands).is_empty(),
                    "{layout}: a layout not in charge placed something: {:?}",
                    told.commands
                );
                assert_eq!(
                    structure(&scripts, layout),
                    before,
                    "{layout}: `closing` rearranged a layout that is not in charge"
                );
                let _ = scripts.refused(2, snapshot(windows.clone()));
                assert_eq!(
                    structure(&scripts, layout),
                    before,
                    "{layout}: `refused` rearranged a layout that is not in charge"
                );

                // And a window it never held -- one open before the scripts
                // were -- is not put into it by a refusal either.
                let mut more = windows.clone();
                more.push(window(4, 700.0, 500.0));
                let _ = scripts.closing(4, snapshot(leaving(&more, &[4])));
                let _ = scripts.refused(4, snapshot(more));
                assert_eq!(
                    structure(&scripts, layout),
                    before,
                    "{layout}: `refused` put a window into a layout that is not in charge"
                );
            }
        }

        /// **Switched on during a close, a layout still puts a refused window
        /// back.** Not in charge at `closing`, it took nothing out -- but
        /// `adopt`, when the user switches to it, leaves a window being closed
        /// out of the arrangement, so `refused` has to put back a window it did
        /// not itself take out. The rule is "missing from a layout that is in
        /// charge", not only "taken out by `closing`".
        #[test]
        fn a_layout_switched_on_during_a_close_puts_the_refused_window_back() {
            for (layout, key) in LAYOUTS {
                let (mut scripts, _) = layout_with(layout, "");
                let windows = opened_while_floating(&mut scripts);
                let _ = scripts.closing(3, snapshot(leaving(&windows, &[3])));
                let on = placed(&scripts.key(key, snapshot(leaving(&windows, &[3]))).commands);
                assert!(
                    on.contains_key(&1) && !on.contains_key(&3),
                    "{layout}: switched on during the close, the layout placed the window being \
                     closed, or nothing: {on:?}"
                );
                let back = placed(&scripts.refused(3, snapshot(windows.clone())).commands);
                assert!(
                    back.contains_key(&3),
                    "{layout}: the refused window was left out of the arrangement: {back:?}"
                );
            }
        }

        /// **A refused window goes back whatever the setting says now.**
        ///
        /// The setting is read at each event, so a script can change it while
        /// the session runs -- including between a `closing` that took the
        /// window out and the `refused` that has to put it back. `refused`
        /// decided from the setting as it was then, and returned: a live
        /// window, out of every tree and strip, drawn over the neighbour that
        /// had grown into its space until some later event rebuilt the layout.
        #[test]
        fn a_refused_window_goes_back_whatever_the_setting_says_now() {
            for (layout, _) in LAYOUTS {
                let (mut scripts, _) = layout_with(layout, "");
                let before = arranged(&mut scripts, layout);
                let closed = placed(&scripts.closing(3, desk(&before, &[1, 2, 3], &[3])).commands);
                assert!(
                    !closed.is_empty() && !closed.contains_key(&3),
                    "{layout}: the premise is a layout that closed up at once: {closed:?}"
                );
                let _ = scripts.evaluate(&format!("{}\nreturn \"\"", when_gone(layout)));
                let mut now = before.clone();
                now.extend(closed);
                let back = placed(&scripts.refused(3, desk(&now, &[1, 2, 3], &[])).commands);
                assert!(
                    back.contains_key(&3),
                    "{layout}: the setting changed during the close and the refused window was \
                     left out of the arrangement: {back:?}"
                );
            }
        }

        /// **A refused window goes back onto the screen it is on now.**
        ///
        /// A monitor can go during the grace period. Its windows are on one
        /// that is still there, `monitors` is announced, and `adopt` re-homes
        /// them -- except the one being closed, which it leaves out on purpose.
        /// `refused` then put that one back into the tree or strip it had been
        /// in, which is keyed by a monitor that no longer exists and which
        /// `apply` never walks: a window on screen that nothing lays out.
        #[test]
        fn a_refused_window_goes_back_onto_the_screen_it_is_on_now() {
            for (layout, _) in LAYOUTS {
                let (mut scripts, _) = layout_with(layout, "");
                let before = arranged(&mut scripts, layout);
                let _ = scripts.closing(3, desk(&before, &[1, 2, 3], &[3]));

                let moved = |leaving: bool| {
                    snapshot_on(
                        vec![second_monitor()],
                        (1..=3)
                            .map(|id| WindowInfo {
                                icon: None,
                                client_side: false,
                                blur_region: false,
                                insets: crate::decoration::Insets::default(),
                                leaving: leaving && id == 3,
                                ..on(&second_monitor(), window(id, 800.0, 600.0))
                            })
                            .collect(),
                    )
                };
                let adopted = adopt(&mut scripts, moved(true));
                assert!(
                    adopted.contains_key(&1) && !adopted.contains_key(&3),
                    "{layout}: the premise is the other windows re-homed onto the screen that is \
                     left, without the window being closed: {adopted:?}"
                );

                let back = placed(&scripts.refused(3, moved(false)).commands);
                assert!(
                    back.get(&3).is_some_and(|rect| rect.x >= BESIDE.x),
                    "{layout}: the refused window was not put back onto the screen it is on: \
                     {back:?}"
                );
            }
        }
    }

    /// **#134: a configurable minimum tile, and a new window with no room
    /// overflowing -- by default to the next empty workspace.**
    ///
    /// Against the shipped `tiling.lua` and `workspaces.lua`, as the modules
    /// above run them. The chain is `tiling.lua`'s and the tree steps it calls
    /// are `solium_layout::tree::minimum_tests`'; this is the chain.
    ///
    /// Most tests set a minimum of 1300x800 on a 2560x1440 screen at the
    /// shipped 12px gap. The first window takes the whole 2536x1416 tile, and
    /// no split of that has room -- side by side is 1262 wide, one above the
    /// other is 702 high -- so a workspace holds one window unless something
    /// allows a second, and where each window goes is never in doubt.
    mod overflow {
        use super::super::Outcome;
        use super::*;

        /// [`AREA`] less the shipped gap all round: the one tile a workspace
        /// with one window has.
        const FULL: Rect = Rect {
            x: 12.0,
            y: 12.0,
            w: 2536.0,
            h: 1416.0,
        };

        /// The shipped layouts in the order `init.lua` requires them, `before`
        /// run ahead of them, and the entry point for a reload. A directory
        /// per call, for the reason [`scripts`] gives.
        ///
        /// `scrolling.lua` too, although no test here switches it on: every
        /// layout hears `open` whether or not it is in charge, and until #134's
        /// review this harness left it out -- which is how a strip that was
        /// not in charge handing the keyboard to a window parked on another
        /// workspace went unseen. See
        /// `with_follow_overflow_off_the_window_opens_there_and_the_view_stays`.
        fn tiling_with(before: &str) -> (Scripts, std::path::PathBuf) {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let directory = std::env::temp_dir()
                .join(format!("solium-overflow-{}-{serial}", std::process::id()));
            let _ = std::fs::create_dir_all(&directory);
            let entry = directory.join("init.lua");
            std::fs::write(
                &entry,
                format!(
                    "package.path = {shipped:?} .. \"/?.lua\"\n\
                     {before}\n\
                     require(\"modes\")\n\
                     require(\"workspaces\")\n\
                     require(\"tiling\")\n\
                     require(\"scrolling\")\n",
                    shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
                ),
            )
            .expect("writing the entry point");
            let scripts = Scripts::load(&entry).expect("loading the shipped layout");
            (scripts, entry)
        }

        /// Everything `sol.log` is handed, kept in `_G.logged` as well as
        /// logged. `tiling.lua` looks `sol.log` up when it speaks, so a
        /// replacement installed before it loads is the one it calls.
        const CAPTURE: &str = "_G.logged = {}\n\
             local log = sol.log\n\
             sol.log = function(message)\n\
                 _G.logged[#_G.logged + 1] = message\n\
                 log(message)\n\
             end";

        fn logged(scripts: &Scripts) -> String {
            scripts.evaluate("return table.concat(_G.logged or {}, \"\\n\")")
        }

        fn minimum(w: u32, h: u32) -> String {
            format!("require(\"config\").tiling.minimum = {{ w = {w}, h = {h} }}")
        }

        /// The windows `ids`, where a toplevel maps, with the pointer at
        /// `cursor`. Nothing here is drawn anywhere, so `sol.window_at`
        /// finds nothing and the tree's own hit test is what picks the tile
        /// under the pointer -- the same tile, from the same rectangles, that
        /// the layout placed.
        fn desk_at(ids: &[u64], cursor: (f64, f64)) -> Snapshot {
            Snapshot {
                cursor,
                ..snapshot(ids.iter().map(|&id| window(id, 800.0, 600.0)).collect())
            }
        }

        /// Windows `ids`, each drawn and placed where `placed` says, the ones
        /// in `leaving` being closed. What a layout reading the snapshot after
        /// its own pass would see.
        fn desk(placed: &HashMap<u64, Rect>, ids: &[u64], leaving: &[u64]) -> Snapshot {
            snapshot(
                ids.iter()
                    .map(|&id| {
                        let rect = placed.get(&id).copied().unwrap_or_default();
                        WindowInfo {
                            icon: None,
                            client_side: false,
                            blur_region: false,
                            insets: crate::decoration::Insets::default(),
                            rect,
                            drawn: drawn(rect),
                            leaving: leaving.contains(&id),
                            ..window(id, rect.w, rect.h)
                        }
                    })
                    .collect(),
            )
        }

        type HashMap<K, V> = std::collections::HashMap<K, V>;

        /// Tiling switched on over an empty desk, so every window after this
        /// arrives through `open`, as a window does in a session.
        fn switched_on(scripts: &mut Scripts) {
            let outcome = scripts.key("super+t", desk_at(&[], (0.0, 0.0)));
            assert!(outcome.handled, "the tiling key was not handled");
        }

        /// The workspace a window belongs to, as `workspaces.at` answers.
        fn workspace_of(scripts: &Scripts, id: u64) -> String {
            scripts.evaluate(&format!(
                "return tostring(require(\"workspaces\").at({id}, \"DP-1\"))"
            ))
        }

        /// The workspace `DP-1` is showing.
        fn showing(scripts: &Scripts) -> String {
            scripts.evaluate("return tostring(require(\"workspaces\").on(\"DP-1\"))")
        }

        fn last_focus(commands: &[Command]) -> Option<u64> {
            commands.iter().rev().find_map(|command| match command {
                Command::Focus { id, .. } => Some(*id),
                _ => None,
            })
        }

        fn same(left: Rect, right: Rect) -> bool {
            about(left.x, right.x)
                && about(left.y, right.y)
                && about(left.w, right.w)
                && about(left.h, right.h)
        }

        /// Every rect `commands` placed window `id` at, in order.
        fn places_of(commands: &[Command], id: u64) -> Vec<Rect> {
            commands
                .iter()
                .filter_map(|command| match command {
                    Command::Place {
                        id: placed, rect, ..
                    } if *placed == id => Some(*rect),
                    _ => None,
                })
                .collect()
        }

        /// **A window with no room opens on the next empty workspace, and the
        /// view goes with it, keyboard included.**
        ///
        /// Window 2 has no room beside window 1 (see the module note), and the
        /// default list is largest, workspace, allow: no tile has room, so it
        /// is workspace 2. The view switches there as `super+2` would -- the
        /// status line says so -- the keyboard is handed to the new window,
        /// and the window is placed once, at its final tile. Window 1 stays
        /// where it was, on workspace 1.
        #[test]
        fn a_window_with_no_room_opens_on_the_next_empty_workspace_and_the_view_follows() {
            let (mut scripts, _) = tiling_with(&minimum(1300, 800));
            switched_on(&mut scripts);
            let first = open(&mut scripts, 1, &[1]);
            assert!(
                places_of(&first.commands, 1)
                    .last()
                    .is_some_and(|rect| same(*rect, FULL)),
                "the premise: window 1 takes the whole screen: {:?}",
                first.commands
            );

            let told = open(&mut scripts, 2, &[1, 2]);
            assert_eq!(workspace_of(&scripts, 2), "2", "window 2 was not sent on");
            assert_eq!(workspace_of(&scripts, 1), "1", "window 1 moved workspace");
            assert_eq!(showing(&scripts), "2", "the view did not follow");
            assert_eq!(told.status.as_deref(), Some("workspace 2"));
            assert_eq!(
                last_focus(&told.commands),
                Some(2),
                "the keyboard was left on a workspace nobody is looking at"
            );
            let placed = places_of(&told.commands, 2);
            assert!(
                !placed.is_empty() && placed.iter().all(|rect| same(*rect, FULL)),
                "window 2 was placed somewhere on its way to its tile: {placed:?}"
            );
            assert!(
                places_of(&told.commands, 1).is_empty(),
                "window 1 was placed again, from a workspace that is not in view"
            );
        }

        /// **`follow_overflow = false`: the window opens over there, and the
        /// view stays.** It is still placed in its tile, and grouped onto its
        /// desk in the same dispatch, so it is never drawn over this one.
        ///
        /// And no script hands it the keyboard. `scrolling.lua`, which was not
        /// in charge, did: its `open` handler inserted the window into a strip
        /// and focused the strip's new column, so the keyboard followed the
        /// window to a desk nobody could see (#134 review). This checks the
        /// scripts; that the compositor does not give it the keyboard either is
        /// `a_window_that_overflows_to_a_hidden_workspace_does_not_take_the_keyboard`
        /// in `state.rs`, which types and sees where the key lands.
        #[test]
        fn with_follow_overflow_off_the_window_opens_there_and_the_view_stays() {
            let (mut scripts, _) = tiling_with(&format!(
                "{}\nrequire(\"config\").tiling.follow_overflow = false",
                minimum(1300, 800)
            ));
            switched_on(&mut scripts);
            let _ = open(&mut scripts, 1, &[1]);
            let told = open(&mut scripts, 2, &[1, 2]);

            assert_eq!(workspace_of(&scripts, 2), "2", "window 2 was not sent on");
            assert_eq!(showing(&scripts), "1", "the view went with it");
            assert_eq!(
                last_focus(&told.commands),
                None,
                "the keyboard was moved: {:?}",
                told.commands
            );
            assert!(
                places_of(&told.commands, 2)
                    .last()
                    .is_some_and(|rect| same(*rect, FULL)),
                "window 2 was not placed in its tile over there: {:?}",
                told.commands
            );
            let grouped = told.commands.iter().any(|command| {
                matches!(
                    command,
                    Command::Group { name, selection: Some(selection), .. }
                        if name == "desk-2@DP-1" && selection.windows.contains(&2)
                )
            });
            assert!(
                grouped,
                "window 2 was not put on its own desk in the same dispatch: {:?}",
                told.commands
            );
        }

        /// **With every workspace taken, `"workspace"` does nothing and
        /// `"allow"` places the window here, under the minimum.** The
        /// workspaces are a fixed four, so there is none to make. `"allow"`
        /// is a step the list names, so this is not a fall-through and is not
        /// logged as one.
        #[test]
        fn with_every_workspace_taken_the_window_is_allowed_under_the_minimum() {
            let (mut scripts, _) = tiling_with(&format!("{CAPTURE}\n{}", minimum(1300, 800)));
            switched_on(&mut scripts);
            let _ = scripts.evaluate(
                "local workspaces = require(\"workspaces\")\n\
                 workspaces.of[12] = 2\n\
                 workspaces.of[13] = 3\n\
                 workspaces.of[14] = 4\n\
                 return \"\"",
            );
            assert_eq!(
                scripts.evaluate("return tostring(require(\"workspaces\").count())"),
                "4"
            );
            let _ = open(&mut scripts, 1, &[1, 12, 13, 14]);
            let told = open(&mut scripts, 2, &[1, 2, 12, 13, 14]);

            assert_eq!(workspace_of(&scripts, 2), "1");
            assert_eq!(showing(&scripts), "1");
            let placed = placed(&told.commands);
            let (one, two) = (placed.get(&1).copied(), placed.get(&2).copied());
            assert!(
                one.zip(two).is_some_and(|(one, two)| !overlap(one, two)
                    && one.w < 1300.0
                    && two.w < 1300.0),
                "the two were not split under the minimum: {one:?} {two:?}"
            );
            assert!(
                !logged(&scripts).contains("below tiling.minimum"),
                "an \"allow\" the list names was reported as a fall-through: {}",
                logged(&scripts)
            );
        }

        /// **A list with no `"allow"` still places the window, and says so.**
        /// `{ "largest" }` finds no tile with room, and the list is over; the
        /// window splits the tile under the pointer all the same, on this
        /// workspace, and the log says it went under the minimum.
        #[test]
        fn a_list_without_allow_still_places_the_window_and_says_so() {
            let (mut scripts, _) = tiling_with(&format!(
                "{CAPTURE}\n{}\nrequire(\"config\").tiling.overflow = {{ \"largest\" }}",
                minimum(1300, 800)
            ));
            switched_on(&mut scripts);
            let _ = open(&mut scripts, 1, &[1]);
            let told = open(&mut scripts, 2, &[1, 2]);

            assert_eq!(
                workspace_of(&scripts, 2),
                "1",
                "a list with no workspace step moved it"
            );
            let placed = placed(&told.commands);
            assert!(
                placed.get(&2).is_some_and(|rect| rect.w < 1300.0) && placed.contains_key(&1),
                "window 2 was not placed beside window 1: {placed:?}"
            );
            let said = logged(&scripts);
            assert!(
                said.contains("window 2") && said.contains("below tiling.minimum"),
                "the fall-through was silent: {said:?}"
            );
        }

        /// Four windows at 600x750, each opened with the pointer at the left
        /// edge. 2 goes left of 1, 1262 wide each. 3 has no room one above the
        /// other in 2 -- 702 high -- and so turns, and goes beside it at 625.
        /// 4 has room in 3 neither way, while 1, the largest tile, still has
        /// room across.
        fn four_at_the_left_edge(before: &str) -> (Scripts, Outcome) {
            let (mut scripts, _) = tiling_with(&format!("{}\n{before}", minimum(600, 750)));
            switched_on(&mut scripts);
            let at = (100.0, 700.0);
            let _ = scripts.opened(1, desk_at(&[1], at));
            let _ = scripts.opened(2, desk_at(&[1, 2], at));
            let third = scripts.opened(3, desk_at(&[1, 2, 3], at));
            let placed = placed(&third.commands);
            assert!(
                placed
                    .get(&3)
                    .is_some_and(|rect| about(rect.w, 625.0) && about(rect.h, 1416.0))
                    && placed.get(&2).is_some_and(|rect| about(rect.w, 625.0)),
                "the premise: window 3 turned and went beside window 2: {placed:?}"
            );
            let fourth = scripts.opened(4, desk_at(&[1, 2, 3, 4], at));
            (scripts, fourth)
        }

        /// **The steps are tried in the order written.** Window 4 has no room
        /// under the pointer and window 1 has room across (see
        /// [`four_at_the_left_edge`]): the shipped order puts it there, and
        /// `workspace` ahead of `largest` sends it to workspace 2 instead.
        #[test]
        fn the_steps_are_tried_in_the_order_written() {
            let (shipped, told) = four_at_the_left_edge("");
            assert_eq!(
                workspace_of(&shipped, 4),
                "1",
                "largest comes first by default"
            );
            let one = placed(&told.commands).get(&1).copied();
            let four = placed(&told.commands).get(&4).copied();
            assert!(
                one.zip(four).is_some_and(|(one, four)| about(one.w, 625.0)
                    && about(four.w, 625.0)
                    && four.x > one.x
                    && one.x > 1200.0),
                "window 4 did not split window 1, the largest tile with room: {one:?} {four:?}"
            );

            let (reordered, told) = four_at_the_left_edge(
                "require(\"config\").tiling.overflow = { \"workspace\", \"largest\", \"allow\" }",
            );
            assert_eq!(
                workspace_of(&reordered, 4),
                "2",
                "workspace was written first"
            );
            assert!(
                placed(&told.commands)
                    .get(&4)
                    .is_some_and(|rect| same(*rect, FULL)),
                "window 4 is alone on workspace 2 and has the whole of it: {:?}",
                told.commands
            );
        }

        /// **A window being closed is room for the next one** (#128). The
        /// layout takes it out of its tree at `closing`, and its neighbour
        /// grows into the space -- so the next window has room there and stays
        /// on this workspace, where a tile still held for the closing window
        /// would have sent it to the next.
        ///
        /// And a workspace whose only window is being closed is empty: the
        /// next window with no room goes there, and it is that window the
        /// keyboard goes to, not the one fading out ahead of it in the list.
        #[test]
        fn a_window_being_closed_is_room_for_the_next_one() {
            let (mut scripts, _) = tiling_with(&minimum(1200, 800));
            switched_on(&mut scripts);
            let _ = open(&mut scripts, 1, &[1]);
            let second = open(&mut scripts, 2, &[1, 2]);
            let before = placed(&second.commands);
            assert!(
                before
                    .get(&1)
                    .zip(before.get(&2))
                    .is_some_and(|(one, two)| about(one.w, 1262.0) && about(two.w, 1262.0)),
                "the premise: 1 and 2 side by side, with no room left: {before:?}"
            );
            let _ = scripts.closing(2, desk(&before, &[1, 2], &[2]));
            let _ = scripts.opened(
                3,
                Snapshot {
                    cursor: (500.0, 500.0),
                    ..desk(&before, &[1, 2, 3], &[2])
                },
            );
            assert_eq!(
                workspace_of(&scripts, 3),
                "1",
                "the closing window's tile was counted as taken"
            );

            let (mut scripts, _) = tiling_with(&minimum(1300, 800));
            switched_on(&mut scripts);
            let _ = scripts.evaluate("require(\"workspaces\").of[5] = 2\nreturn \"\"");
            let _ = open(&mut scripts, 1, &[5, 1]);
            let mut windows = vec![
                window(5, 800.0, 600.0),
                window(1, 800.0, 600.0),
                window(3, 800.0, 600.0),
            ];
            windows[0].leaving = true;
            let told = scripts.opened(
                3,
                Snapshot {
                    cursor: (500.0, 500.0),
                    ..snapshot(windows)
                },
            );
            assert_eq!(
                workspace_of(&scripts, 3),
                "2",
                "a workspace holding only a closing window was counted as taken"
            );
            assert_eq!(last_focus(&told.commands), Some(3));
        }

        /// **A layout not in charge sends nothing to another workspace.**
        /// Tiling hears `open` while floating is the mode and keeps the window
        /// in its tree for later. That tree is full, and the window goes into
        /// it anyway -- under the minimum -- rather than off to workspace 2
        /// with the view behind it, on a desktop tiling is not arranging.
        #[test]
        fn a_layout_not_in_charge_sends_nothing_to_another_workspace() {
            let (mut scripts, _) = tiling_with(&minimum(1300, 800));
            let _ = open(&mut scripts, 1, &[1]);
            let told = open(&mut scripts, 2, &[1, 2]);
            assert_eq!(workspace_of(&scripts, 2), "1");
            assert_eq!(showing(&scripts), "1");
            assert_eq!(last_focus(&told.commands), None);
            let held = scripts.evaluate(
                "local trees = require(\"tiling\").trees\n\
                 local out = {}\n\
                 for key, tree in pairs(trees) do\n\
                     local ids = tree:windows()\n\
                     table.sort(ids)\n\
                     if #ids > 0 then out[#out + 1] = key .. \"=\" .. table.concat(ids, \",\") end\n\
                 end\n\
                 table.sort(out)\n\
                 return table.concat(out, \" \")",
            );
            assert_eq!(held, "1@DP-1=1,2", "the trees tiling keeps for later");
        }

        /// **Nothing already open is sent to another workspace**, by any route
        /// that puts one back into a tree -- only a window being opened
        /// overflows.
        ///
        /// Four windows at a minimum where a workspace holds one, so every
        /// route below has three windows with no room and three empty
        /// workspaces to send them to. Each must keep every window on
        /// workspace 1 of its screen and place it there:
        ///
        ///   * `adopt`, by switching tiling on;
        ///   * `adopt`, on a monitor change -- every window now on `DP-2`,
        ///     missing from that screen's tree;
        ///   * `restore`, a reload, with every window back on `DP-1` -- whose
        ///     tree the monitor change emptied, so it is built again from
        ///     nothing;
        ///   * `refused`, a close the application declined;
        ///   * a dialog that stops being modal and rejoins its tree.
        #[test]
        fn nothing_already_open_is_sent_to_another_workspace() {
            let (mut scripts, entry) = tiling_with(&minimum(1300, 800));
            let ids = [1, 2, 3, 4];
            let everyone_home = |scripts: &Scripts, monitor: &str, route: &str| {
                let away = scripts.evaluate(&format!(
                    "local workspaces = require(\"workspaces\")\n\
                     local out = {{}}\n\
                     for id = 1, 5 do\n\
                         local at = workspaces.at(id, {monitor:?})\n\
                         if at ~= 1 then out[#out + 1] = id .. \"@\" .. tostring(at) end\n\
                     end\n\
                     out[#out + 1] = \"showing=\" .. tostring(workspaces.on({monitor:?}))\n\
                     return table.concat(out, \" \")"
                ));
                assert_eq!(
                    away, "showing=1",
                    "{route} sent a window to another workspace"
                );
            };

            let switched = scripts.key("super+t", desk_at(&ids, (500.0, 500.0)));
            let placed_on = placed(&switched.commands);
            assert!(
                ids.iter().all(|id| placed_on.contains_key(id)),
                "switching tiling on left a window out: {placed_on:?}"
            );
            everyone_home(&scripts, "DP-1", "switching tiling on");

            let moved = snapshot_on(
                vec![second_monitor()],
                ids.iter()
                    .map(|&id| on(&second_monitor(), window(id, 800.0, 600.0)))
                    .collect(),
            );
            let _ = scripts.monitors_changed(moved.clone());
            let after = placed(&scripts.relayout(moved).commands);
            assert!(
                ids.iter()
                    .all(|id| after.get(id).is_some_and(|rect| rect.x >= BESIDE.x)),
                "a monitor change left a window out of the screen it is on: {after:?}"
            );
            everyone_home(&scripts, "DP-2", "a monitor change");

            let mut scripts = Scripts::load_carrying(&entry, scripts.kept()).expect("reloading");
            let restored = placed(&scripts.restored(desk_at(&ids, (500.0, 500.0))).commands);
            assert!(
                ids.iter().all(|id| restored.contains_key(id)),
                "a reload left a window out: {restored:?}"
            );
            everyone_home(&scripts, "DP-1", "a reload");

            let _ = scripts.closing(4, desk(&restored, &ids, &[4]));
            let back = placed(&scripts.refused(4, desk(&restored, &ids, &[])).commands);
            assert!(
                back.contains_key(&4),
                "the refused window was not put back: {back:?}"
            );
            everyone_home(&scripts, "DP-1", "a refused close");

            let mut with_dialog: Vec<WindowInfo> =
                ids.iter().map(|&id| window(id, 800.0, 600.0)).collect();
            with_dialog.push(modal(5, 600.0, 400.0, Parentage::Window(1)));
            let _ = scripts.relayout(snapshot(with_dialog.clone()));
            if let Some(dialog) = with_dialog.last_mut() {
                dialog.modal = false;
                dialog.parent = Parentage::None;
            }
            let rejoined = placed(&scripts.relayout(snapshot(with_dialog)).commands);
            assert!(
                rejoined.get(&5).is_some_and(|rect| rect.w < 1300.0),
                "the dialog that stopped being modal did not rejoin the arrangement: {rejoined:?}"
            );
            everyone_home(&scripts, "DP-1", "a dialog rejoining");
        }

        /// 1 over 3 on the left, 2 on the right, at a 700x800 minimum; then
        /// the seam between them dragged left to 1136, through the `resize`
        /// event, so the left column is 1112 wide and window 2 is 1412.
        ///
        /// `adopt` builds it: 2 has room beside 1, and 3 has none anywhere --
        /// window 1 splits to 702 high or 625 wide -- so it is allowed in over
        /// window 1. After the drag the left column has no room either way,
        /// and window 2 has room across: two of 700.
        fn lopsided() -> (Scripts, HashMap<u64, Rect>) {
            let (mut scripts, _) = tiling_with(&minimum(700, 800));
            let switched = scripts.key("super+t", desk_at(&[1, 2, 3], (500.0, 500.0)));
            let first = placed(&switched.commands);
            let dragged = scripts.resized(
                2,
                (1136.0, 700.0),
                (Some("left"), None),
                desk(&first, &[1, 2, 3], &[]),
            );
            let mut now = first;
            now.extend(placed(&dragged.commands));
            assert!(
                now.get(&1)
                    .is_some_and(|rect| about(rect.w, 1112.0) && about(rect.h, 702.0))
                    && now.get(&3).is_some_and(|rect| about(rect.w, 1112.0))
                    && now.get(&2).is_some_and(|rect| about(rect.w, 1412.0)),
                "the fixture is not the one described: {now:?}"
            );
            (scripts, now)
        }

        /// **A refused window under the minimum comes back where it was**,
        /// not into the largest tile. Window 3's tile was under the minimum
        /// before it was closed, so the tile it goes back to has no room
        /// either -- and window 2's does. Taking that would move a window the
        /// user never moved; the arrangement comes back rect for rect.
        #[test]
        fn a_refused_window_under_the_minimum_comes_back_where_it_was() {
            let (mut scripts, before) = lopsided();
            let closed = placed(&scripts.closing(3, desk(&before, &[1, 2, 3], &[3])).commands);
            let mut now = before.clone();
            now.extend(closed);
            let back = placed(&scripts.refused(3, desk(&now, &[1, 2, 3], &[])).commands);
            for id in [1, 2, 3] {
                let was = before.get(&id).copied();
                let is = back.get(&id).copied();
                assert!(
                    was.zip(is).is_some_and(|(was, is)| same(was, is)),
                    "window {id} was at {was:?} before 3 was closed and is at {is:?} after the \
                     close was refused"
                );
            }
            assert_eq!(workspace_of(&scripts, 3), "1");
        }

        /// **A dropped window goes to the tile it was let go over**, under the
        /// minimum if it must, and not to the largest tile. Window 3 is dropped
        /// on window 1, whose tile has no room; window 2's has.
        #[test]
        fn a_dropped_window_goes_to_the_tile_it_was_let_go_over() {
            let (mut scripts, before) = lopsided();
            let one = before.get(&1).copied().unwrap_or_default();
            let (x, y) = (one.x + one.w / 2.0, one.y + one.h * 0.75);
            let dropped = placed(
                &scripts
                    .dropped(3, x, y, desk(&before, &[1, 2, 3], &[]))
                    .commands,
            );
            let (one, two, three) = (
                dropped.get(&1).copied(),
                dropped.get(&2).copied(),
                dropped.get(&3).copied(),
            );
            assert!(
                two.zip(before.get(&2).copied())
                    .is_some_and(|(now, was)| same(now, was)),
                "window 2's tile was split for a window dropped somewhere else: {two:?}"
            );
            assert!(
                one.zip(three)
                    .is_some_and(|(one, three)| about(one.x + one.w, 1124.0)
                        && about(three.x + three.w, 1124.0)
                        && !overlap(one, three)),
                "window 3 did not go back into the left column it was dropped on: {one:?} \
                 {three:?}"
            );
            assert_eq!(workspace_of(&scripts, 3), "1");
        }

        /// **The seams stop at the minimum, by the drag and by the keyboard.**
        /// The shipped 160: window 1's right edge dragged to x=50 stops where
        /// window 1 is 160 wide, where 0.05..0.95 alone stopped it at 126; and
        /// `super+minus` pressed until nothing changes stops at 160 too.
        #[test]
        fn the_shipped_seams_stop_at_the_minimum() {
            let (mut scripts, _) = tiling_with("");
            let switched = scripts.key("super+t", desk_at(&[1, 2], (500.0, 500.0)));
            let first = placed(&switched.commands);
            let dragged = placed(
                &scripts
                    .resized(
                        1,
                        (50.0, 700.0),
                        (Some("right"), None),
                        desk(&first, &[1, 2], &[]),
                    )
                    .commands,
            );
            assert!(
                dragged.get(&1).is_some_and(|rect| about(rect.w, 160.0)),
                "the drag went under the minimum: {dragged:?}"
            );

            let (mut scripts, _) = tiling_with("");
            let switched = scripts.key("super+t", desk_at(&[1, 2], (500.0, 500.0)));
            let mut now = placed(&switched.commands);
            for _ in 0..12 {
                let mut focused = desk(&now, &[1, 2], &[]);
                for window in &mut focused.windows {
                    window.focused = window.id == 1;
                }
                now.extend(placed(&scripts.key("super+minus", focused).commands));
            }
            assert!(
                now.get(&1).is_some_and(|rect| about(rect.w, 160.0)),
                "the keyboard went under the minimum: {now:?}"
            );
        }

        /// **A minimum that is not a size is named once, and ignored on that
        /// side only.** `w = "wide"` is no minimum across, so window 3 turns
        /// and goes beside window 1 when one above the other would take it
        /// under the 800 that `h` still asks for. Three dispatches, one line.
        #[test]
        fn a_minimum_that_is_not_a_size_is_named_once_and_ignored() {
            let (mut scripts, _) = tiling_with(&format!(
                "{CAPTURE}\nrequire(\"config\").tiling.minimum = {{ w = \"wide\", h = 800 }}"
            ));
            let switched = scripts.key("super+t", desk_at(&[1, 2, 3], (500.0, 500.0)));
            let _ = scripts.relayout(desk_at(&[1, 2, 3], (500.0, 500.0)));
            let _ = scripts.relayout(desk_at(&[1, 2, 3], (500.0, 500.0)));
            let placed = placed(&switched.commands);
            assert!(
                placed
                    .get(&1)
                    .zip(placed.get(&3))
                    .is_some_and(|(one, three)| about(one.y, three.y)
                        && about(one.h, 1416.0)
                        && about(three.h, 1416.0)),
                "window 3 was not put beside window 1: {placed:?}"
            );
            let said = logged(&scripts);
            assert_eq!(
                said.matches("tiling.minimum.w").count(),
                1,
                "the bad side was not named exactly once: {said:?}"
            );
            assert!(
                !said.contains("tiling.minimum.h"),
                "the good side was named: {said:?}"
            );
        }

        /// **An empty workspace is given whole once the window on it was sent
        /// away.** Window 7, opened on workspace 2 and sent to 3, leaves
        /// workspace 2 empty, and window 2, overflowing to it, gets the whole
        /// of it rather than half.
        #[test]
        fn a_workspace_left_empty_by_a_send_is_given_whole() {
            let (mut scripts, _) = tiling_with(&minimum(1300, 800));
            switched_on(&mut scripts);
            let _ = scripts.key("super+2", desk_at(&[], (0.0, 0.0)));
            let _ = open(&mut scripts, 7, &[7]);
            let mut sending = desk_at(&[7], (0.0, 0.0));
            for window in &mut sending.windows {
                window.focused = true;
            }
            let _ = scripts.key("super+shift+3", sending);
            let _ = scripts.key("super+1", desk_at(&[7], (0.0, 0.0)));
            assert_eq!(workspace_of(&scripts, 7), "3");
            assert_eq!(showing(&scripts), "1");

            let _ = open(&mut scripts, 1, &[7, 1]);
            let told = open(&mut scripts, 2, &[7, 1, 2]);
            assert_eq!(workspace_of(&scripts, 2), "2");
            assert!(
                places_of(&told.commands, 2)
                    .last()
                    .is_some_and(|rect| same(*rect, FULL)),
                "window 2 shared workspace 2 with a window that is not there: {:?}",
                told.commands
            );
        }

        /// **The next empty workspace is after the one in view, and round
        /// again**; a window being closed does not take one; and with
        /// workspaces not per monitor, a window on any screen does.
        #[test]
        fn the_next_empty_workspace_is_after_the_one_in_view_and_round_again() {
            let vacant = |per_monitor: bool, windows: Vec<WindowInfo>| {
                let (mut scripts, _) = tiling_with(&format!(
                    "require(\"config\").workspaces.per_monitor = {per_monitor}"
                ));
                let _ = scripts.evaluate(
                    "local workspaces = require(\"workspaces\")\n\
                     workspaces.showing[\"DP-1\"] = 3\n\
                     workspaces.showing[\"*all*\"] = 3\n\
                     workspaces.of[21] = 4\n\
                     workspaces.of[22] = 2\n\
                     workspaces.of[23] = 1\n\
                     workspaces.of[24] = 1\n\
                     return \"\"",
                );
                let _ = scripts.relayout(snapshot_on(vec![monitor(), second_monitor()], windows));
                scripts.evaluate("return tostring(require(\"workspaces\").vacant(\"DP-1\"))")
            };
            let closing = |id| WindowInfo {
                icon: None,
                client_side: false,
                blur_region: false,
                insets: crate::decoration::Insets::default(),
                leaving: true,
                ..window(id, 800.0, 600.0)
            };
            let taken = |id| window(id, 800.0, 600.0);

            // Showing 3, with 4 and 2 taken: round to 1, whose only window is
            // being closed.
            assert_eq!(vacant(true, vec![taken(21), taken(22), closing(23)]), "1");
            // The same with 1 taken for real: nothing left.
            assert_eq!(vacant(true, vec![taken(21), taken(22), taken(23)]), "nil");
            // Per monitor, a window on the other screen is its own screen's.
            let elsewhere = on(&second_monitor(), window(24, 800.0, 600.0));
            assert_eq!(
                vacant(true, vec![taken(21), taken(22), elsewhere.clone()]),
                "1"
            );
            // Together, the same window takes workspace 1 on every screen.
            assert_eq!(vacant(false, vec![taken(21), taken(22), elsewhere]), "nil");
            // And a window that belongs to no workspace in particular is on
            // every one, since the view carries it along: 25 was never given
            // one, so no workspace is empty (#134 review).
            assert_eq!(
                vacant(true, vec![taken(21), window(25, 800.0, 600.0)]),
                "nil"
            );
        }

        /// **A tree handed a minimum that is not a size has none on that
        /// side.** `tiling.lua` cleans `config.tiling.minimum` before handing
        /// it over, so this is the binding's own answer for a script that does
        /// not: `0/0` and a string are no floor, where a real 160x150 on the
        /// same 300x200 tile refuses the split both ways.
        #[test]
        fn a_tree_handed_a_minimum_that_is_not_a_size_has_none_on_that_side() {
            let (scripts, _) = tiling_with("");
            let answer = scripts.evaluate(
                "local function try(minimum)\n\
                     local area = { x = 0, y = 0, w = 300, h = 200, gap = 0, split = 0.5,\n\
                         minimum = minimum }\n\
                     local tree = sol.layout.tree()\n\
                     tree:insert(1, nil, nil, nil, area)\n\
                     return tostring(tree:insert_fitting(2, nil, 150, 100, area))\n\
                 end\n\
                 return try({ w = 0/0, h = \"tall\" }) .. \" \" .. try({ w = 160, h = 150 })",
            );
            assert_eq!(answer, "true false");
        }

        /// **With `follow_new_windows` off, no workspace is empty.** A window
        /// that belongs to no workspace in particular is on every one -- the
        /// view carries it along, which is what `workspaces.at` says -- so
        /// window 1 is on workspace 2 as much as on this one. Sending window 2
        /// there moved the view, left window 1 on screen, and laid window 2
        /// over the whole of it from workspace 2's empty tree (#134 review).
        /// So the step does nothing and `"allow"` puts window 2 beside window
        /// 1, here.
        #[test]
        fn with_follow_new_windows_off_no_workspace_is_empty() {
            let (mut scripts, _) = tiling_with(&format!(
                "{}\nrequire(\"config\").workspaces.follow_new_windows = false",
                minimum(1300, 800)
            ));
            switched_on(&mut scripts);
            let _ = open(&mut scripts, 1, &[1]);
            let told = open(&mut scripts, 2, &[1, 2]);

            assert_eq!(
                showing(&scripts),
                "1",
                "the view went to a workspace window 1 is on as well"
            );
            assert_eq!(
                scripts.evaluate("return tostring(require(\"workspaces\").of[2])"),
                "nil",
                "window 2 was given a workspace of its own"
            );
            let placed = placed(&told.commands);
            let (one, two) = (placed.get(&1).copied(), placed.get(&2).copied());
            assert!(
                one.zip(two).is_some_and(|(one, two)| !overlap(one, two)),
                "window 2 was not placed beside window 1: {one:?} {two:?}"
            );
        }

        /// **With `reflow_on_close = "when_gone"` or `"when_landed"`, a
        /// window being closed still takes its workspace.** Those settings
        /// keep a closing window's tile while it fades, and "when_gone" until
        /// its application has gone, so a workspace whose only window is
        /// closing is not empty to them: window 3 goes on to workspace 3,
        /// rather than into a fresh tree on workspace 2 that has it cover
        /// window 5 while window 5 fades (#134 review). Closing up at once,
        /// the same workspace is empty -- that half is
        /// `a_window_being_closed_is_room_for_the_next_one`. Against `0ce2ab8`
        /// `"when_landed"` fails: it was read as "immediate" (#128).
        #[test]
        fn waiting_to_reflow_a_closing_window_still_takes_its_workspace() {
            for setting in ["when_gone", "when_landed"] {
                let (mut scripts, _) = tiling_with(&format!(
                    "{}\nrequire(\"config\").tiling.reflow_on_close = {setting:?}",
                    minimum(1300, 800)
                ));
                switched_on(&mut scripts);
                let _ = scripts.evaluate("require(\"workspaces\").of[5] = 2\nreturn \"\"");
                let _ = open(&mut scripts, 1, &[5, 1]);
                let mut windows = vec![
                    window(5, 800.0, 600.0),
                    window(1, 800.0, 600.0),
                    window(3, 800.0, 600.0),
                ];
                windows[0].leaving = true;
                let _ = scripts.opened(
                    3,
                    Snapshot {
                        cursor: (500.0, 500.0),
                        ..snapshot(windows)
                    },
                );
                assert_eq!(
                    workspace_of(&scripts, 3),
                    "3",
                    "{setting}: a workspace whose only window keeps its tile while it closes was \
                 counted as empty"
                );
            }
        }

        /// **With `follow_overflow` off, each window with no room takes
        /// another empty workspace**, whether or not the one the window
        /// before went to has room.
        ///
        /// At 1000x1000, 1 and 2 share workspace 1 side by side at 1262x1416,
        /// and neither of those halves has room either way. The view stays on
        /// workspace 1, so 3, 4 and 5 each open against those tiles, and each
        /// finds the workspace the last one went to no longer empty: 3 goes to
        /// 2, 4 to 3, 5 to 4. With none left empty, 6 is allowed in on
        /// workspace 1 under the minimum -- although workspace 2, holding
        /// window 3 alone, could still have taken it beside window 3. That is
        /// the step as written, "the next empty workspace"; `config.lua` says
        /// so beside `follow_overflow`.
        #[test]
        fn with_follow_overflow_off_each_window_with_no_room_takes_another_empty_workspace() {
            let (mut scripts, _) = tiling_with(&format!(
                "{}\nrequire(\"config\").tiling.follow_overflow = false",
                minimum(1000, 1000)
            ));
            switched_on(&mut scripts);
            for id in 1..=6 {
                let ids: Vec<u64> = (1..=id).collect();
                let _ = open(&mut scripts, id, &ids);
            }
            let went: Vec<String> = (1..=6).map(|id| workspace_of(&scripts, id)).collect();
            assert_eq!(went, ["1", "1", "2", "3", "4", "1"]);
            assert_eq!(showing(&scripts), "1");
            assert_eq!(
                scripts.evaluate(
                    "local area = { x = 0, y = 0, w = 2560, h = 1440, gap = 12, split = 0.5,\n\
                         minimum = { w = 1000, h = 1000 } }\n\
                     local tree = require(\"tiling\").trees[\"2@DP-1\"]\n\
                     return tostring(tree:insert_fitting(99, nil, nil, nil, area))"
                ),
                "true",
                "the premise: workspace 2 had room for window 6"
            );
        }

        fn open(scripts: &mut Scripts, id: u64, ids: &[u64]) -> Outcome {
            scripts.opened(id, desk_at(ids, (500.0, 500.0)))
        }
    }

    /// **#115: what a window's own application says about its size**, and
    /// what the shipped layouts do with it.
    ///
    /// Against the shipped `tiling.lua`, `scrolling.lua` and `sizes.lua`, as
    /// `init.lua` loads them, on one 2560x1440 screen at the shipped 12px gap
    /// and 160x96 minimum. Windows 1 and 2 side by side take 1262 each of the
    /// 2536 inside the gap, which is the arrangement most of these start from.
    /// The limits are what `sol.windows()` carries -- frame included, as the
    /// compositor adds it -- so nothing here converts anything.
    mod client_sizes {
        use super::super::Outcome;
        use super::*;
        use smithay::utils::Size;

        /// The shipped layouts with `before` run ahead of them, and every
        /// `sol.log` line kept in `_G.logged`. A directory per call, for the
        /// reason [`scripts`] gives.
        fn layouts_with(before: &str) -> Scripts {
            Scripts::load(&entry_with(before)).expect("loading the shipped layouts")
        }

        /// The entry point [`layouts_with`] loads, for a test that loads it
        /// again.
        fn entry_with(before: &str) -> std::path::PathBuf {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let directory =
                std::env::temp_dir().join(format!("solium-sizes-{}-{serial}", std::process::id()));
            let _ = std::fs::create_dir_all(&directory);
            let entry = directory.join("init.lua");
            std::fs::write(
                &entry,
                format!(
                    "package.path = {shipped:?} .. \"/?.lua\"\n\
                     _G.logged = {{}}\n\
                     local log = sol.log\n\
                     sol.log = function(message)\n\
                         _G.logged[#_G.logged + 1] = message\n\
                         log(message)\n\
                     end\n\
                     {before}\n\
                     require(\"modes\")\n\
                     require(\"workspaces\")\n\
                     require(\"tiling\")\n\
                     require(\"scrolling\")\n",
                    shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
                ),
            )
            .expect("writing the entry point");
            entry
        }

        fn logged(scripts: &Scripts) -> String {
            scripts.evaluate("return table.concat(_G.logged or {}, \"\\n\")")
        }

        fn size(w: i32, h: i32) -> Option<Size<i32, Logical>> {
            Some(Size::from((w, h)))
        }

        /// Window `id` with nothing to say about its size.
        fn plain(id: u64) -> WindowInfo {
            window(id, 800.0, 600.0)
        }

        /// Window `id`, whose application says it cannot go under `min`.
        fn at_least(id: u64, w: i32, h: i32) -> WindowInfo {
            WindowInfo {
                icon: None,
                client_side: false,
                blur_region: false,
                insets: crate::decoration::Insets::default(),
                min: size(w, h),
                ..plain(id)
            }
        }

        /// Window `id`, whose application says it cannot go over `max`.
        fn at_most(id: u64, w: i32, h: i32) -> WindowInfo {
            WindowInfo {
                icon: None,
                client_side: false,
                blur_region: false,
                insets: crate::decoration::Insets::default(),
                max: size(w, h),
                ..plain(id)
            }
        }

        fn with_app(app_id: &str, window: WindowInfo) -> WindowInfo {
            WindowInfo {
                icon: None,
                client_side: false,
                blur_region: false,
                insets: crate::decoration::Insets::default(),
                app_id: app_id.to_owned(),
                ..window
            }
        }

        /// These windows, with the pointer at `cursor`.
        fn desk(windows: Vec<WindowInfo>, cursor: (f64, f64)) -> Snapshot {
            Snapshot {
                cursor,
                ..snapshot(windows)
            }
        }

        /// A layout switched on over an empty desk, so every window after
        /// this arrives through `open`.
        fn switched_on(scripts: &mut Scripts, key: &str) {
            assert!(
                scripts.key(key, desk(Vec::new(), (0.0, 0.0))).handled,
                "{key} was not handled"
            );
        }

        /// Windows 1 and 2 side by side under tiling: 2 opened with the
        /// pointer over the right half of 1, so it takes the right.
        fn side_by_side(scripts: &mut Scripts) {
            switched_on(scripts, "super+t");
            let _ = scripts.opened(1, desk(vec![plain(1)], (2000.0, 700.0)));
            let _ = scripts.opened(2, desk(vec![plain(1), plain(2)], (2000.0, 700.0)));
        }

        /// The last placement of window `id` in `commands`: the rect, the tile
        /// it was placed inside when that is not the rect, and whether the
        /// layout said it is cramped.
        fn last_place(commands: &[Command], id: u64) -> Option<(Rect, Option<Rect>, bool)> {
            commands.iter().rev().find_map(|command| match command {
                Command::Place {
                    id: placed,
                    rect,
                    inside,
                    cramped,
                    ..
                } if *placed == id => Some((*rect, *inside, *cramped)),
                _ => None,
            })
        }

        fn width_of(outcome: &Outcome, id: u64) -> f64 {
            last_place(&outcome.commands, id)
                .map(|(rect, _, _)| rect.w)
                .unwrap_or_else(|| panic!("window {id} was not placed: {:?}", outcome.commands))
        }

        /// **`sol.windows()` says how small and how large each window can
        /// be**, which application it is, and whether the layout said it is
        /// cramped -- the four as a layout reads them, in one pass. Absent is
        /// `nil`, so `if window.min then` is the question to ask.
        #[test]
        fn sol_windows_says_how_small_and_how_large_each_window_can_be() {
            let mut scripts = layouts_with(
                "_G.seen = {}\n\
                 sol.on(\"layout\", function()\n\
                     for _, window in ipairs(sol.windows()) do\n\
                         local function size(of)\n\
                             return of and string.format(\"%dx%d\", of.w, of.h) or \"nil\"\n\
                         end\n\
                         _G.seen[#_G.seen + 1] = string.format(\"%d %s min=%s max=%s cramped=%s\",\n\
                             window.id, window.app_id, size(window.min), size(window.max),\n\
                             tostring(window.cramped))\n\
                     end\n\
                 end)",
            );
            let windows = vec![
                WindowInfo {
                    icon: None,
                    client_side: false,
                    blur_region: false,
                    insets: crate::decoration::Insets::default(),
                    max: size(1200, 0),
                    cramped: true,
                    ..with_app("firefox", at_least(1, 820, 0))
                },
                with_app("kitty", plain(2)),
            ];
            let _ = scripts.relayout(desk(windows, (0.0, 0.0)));
            assert_eq!(
                scripts.evaluate("return table.concat(_G.seen, \", \")"),
                "1 firefox min=820x0 max=1200x0 cramped=true, \
                 2 kitty min=nil max=nil cramped=false"
            );
        }

        /// **A window whose minimum is wider than its tile is given the room**,
        /// taken from the window beside it -- which is the whole of #115: the
        /// tile no longer lies about what the client will draw.
        ///
        /// Window 1 says 1600 wide; it had 1262. Window 2 gives up the
        /// difference and is left 924, well over the minimum. Neither is
        /// cramped: the room was there.
        #[test]
        fn a_window_with_a_minimum_wider_than_its_tile_is_given_the_room() {
            let mut scripts = layouts_with("");
            side_by_side(&mut scripts);
            let told = scripts.relayout(desk(vec![at_least(1, 1600, 0), plain(2)], (0.0, 0.0)));
            let (one, _, cramped) = last_place(&told.commands, 1).expect("window 1 placed");
            let (two, _, _) = last_place(&told.commands, 2).expect("window 2 placed");
            assert!(
                about(one.w, 1600.0),
                "window 1 was not given its minimum: {one:?}"
            );
            assert!(
                about(two.w, 924.0) && about(two.x, 12.0 + 1600.0 + 12.0),
                "{two:?}"
            );
            assert!(!cramped, "a window with the room it needs is not cramped");
        }

        /// **`"ignore"` and `client_size_ignore` are the arrangement before
        /// #115**, for the minimum and the maximum alike: window 1 keeps its
        /// 1262 however wide it says it must be, and a window no wider than
        /// 800 fills its tile from the corner rather than being centred.
        ///
        /// `client_size_ignore` by the application's name, and only that
        /// application: `firefox` is not believed, `kitty` beside it still is.
        #[test]
        fn ignore_and_client_size_ignore_lay_out_what_was_laid_out_before() {
            let wide = |app: &str| with_app(app, at_least(1, 1600, 0));
            for (before, app, says) in [
                (
                    "require(\"config\").tiling.client_minimum = \"ignore\"",
                    "kitty",
                    "ignore",
                ),
                (
                    "require(\"config\").tiling.client_size_ignore = { \"firefox\" }",
                    "firefox",
                    "client_size_ignore",
                ),
            ] {
                let mut scripts = layouts_with(before);
                side_by_side(&mut scripts);
                let told = scripts.relayout(desk(vec![wide(app), plain(2)], (0.0, 0.0)));
                assert!(
                    about(width_of(&told, 1), 1262.0),
                    "{says}: the minimum was still heard: {:?}",
                    told.commands
                );
            }
            let mut scripts =
                layouts_with("require(\"config\").tiling.client_size_ignore = { \"firefox\" }");
            side_by_side(&mut scripts);
            let told = scripts.relayout(desk(vec![wide("kitty"), plain(2)], (0.0, 0.0)));
            assert!(
                about(width_of(&told, 1), 1600.0),
                "an application not on the list was not heard: {:?}",
                told.commands
            );

            for before in [
                "require(\"config\").tiling.client_maximum = \"ignore\"",
                "require(\"config\").tiling.client_size_ignore = { \"firefox\" }",
            ] {
                let mut scripts = layouts_with(before);
                switched_on(&mut scripts, "super+t");
                let told = scripts.opened(
                    1,
                    desk(vec![with_app("firefox", at_most(1, 800, 600))], (0.0, 0.0)),
                );
                let (rect, inside, _) = last_place(&told.commands, 1).expect("placed");
                assert!(
                    about(rect.w, 2536.0) && inside.is_none(),
                    "{before}: the maximum was still heard: {rect:?} in {inside:?}"
                );
            }
        }

        /// **A setting that is not one of its words is read as the default**,
        /// and a `client_size_ignore` that is not a list believes every
        /// application -- so a typo in either is the shipped behaviour and not
        /// the old one.
        #[test]
        fn a_setting_that_is_not_one_of_its_words_is_read_as_the_default() {
            for before in [
                "require(\"config\").tiling.client_minimum = \"respekt\"",
                "require(\"config\").tiling.client_size_ignore = \"firefox\"",
            ] {
                let mut scripts = layouts_with(before);
                side_by_side(&mut scripts);
                let told = scripts.relayout(desk(
                    vec![with_app("firefox", at_least(1, 1600, 0)), plain(2)],
                    (0.0, 0.0),
                ));
                assert!(
                    about(width_of(&told, 1), 1600.0),
                    "{before}: the minimum was not heard: {:?}",
                    told.commands
                );
            }
            let mut scripts =
                layouts_with("require(\"config\").tiling.client_maximum = \"centre\"");
            switched_on(&mut scripts, "super+t");
            let told = scripts.opened(1, desk(vec![at_most(1, 800, 600)], (0.0, 0.0)));
            let (rect, inside, _) = last_place(&told.commands, 1).expect("placed");
            assert!(
                about(rect.w, 800.0) && inside.is_some(),
                "\"centre\" was not read as \"center\": {rect:?} in {inside:?}"
            );
        }

        /// **`options.floors` takes sizes, and anything else is no floor**: a
        /// side that is not a size is none on that side, an entry that is not
        /// a table is none at all. The one real floor still moves the seam,
        /// and one the tree cannot meet comes back `cramped`.
        #[test]
        fn floors_that_are_not_sizes_are_no_floors() {
            let scripts = layouts_with("");
            let lay_out = |floors: &str| {
                scripts.evaluate(&format!(
                    "local area = {{ x = 0, y = 0, w = 2000, h = 1000, gap = 0, split = 0.5,\n\
                         floors = {floors} }}\n\
                     local tree = sol.layout.tree()\n\
                     tree:insert(1, nil, nil, nil, area)\n\
                     tree:insert(2, 1, nil, nil, area)\n\
                     local out = {{}}\n\
                     for _, slot in ipairs(tree:layout(area)) do\n\
                         out[#out + 1] = string.format(\"%d=%.0f%s\", slot.id, slot.w,\n\
                             slot.cramped and \" cramped\" or \"\")\n\
                     end\n\
                     return table.concat(out, \",\")"
                ))
            };
            assert_eq!(
                lay_out("{ [1] = { w = 1500, h = \"tall\" }, [2] = \"wide\", [3] = { w = 0/0 } }"),
                "1=1500,2=500"
            );
            assert_eq!(lay_out("{ [1] = { w = 2500 } }"), "1=1900 cramped,2=100");
        }

        /// **A window whose maximum is smaller than its tile is its maximum,
        /// in the middle of the tile** -- and the tile goes with it as `tile`,
        /// so the compositor holds the client inside the tile and a dragged
        /// edge moves the tile's seam.
        #[test]
        fn a_window_whose_maximum_is_smaller_than_its_tile_is_centred_in_it() {
            let mut scripts = layouts_with("");
            switched_on(&mut scripts, "super+t");
            let told = scripts.opened(1, desk(vec![at_most(1, 800, 600)], (0.0, 0.0)));
            let (rect, inside, _) = last_place(&told.commands, 1).expect("window 1 placed");
            let tile = inside.expect("the tile was not handed over with the window");
            assert!(
                about(tile.x, 12.0) && about(tile.w, 2536.0) && about(tile.h, 1416.0),
                "{tile:?}"
            );
            assert!(about(rect.w, 800.0) && about(rect.h, 600.0), "{rect:?}");
            assert!(
                about(rect.x, 12.0 + (2536.0 - 800.0) / 2.0)
                    && about(rect.y, 12.0 + (1416.0 - 600.0) / 2.0),
                "not in the middle of its tile: {rect:?}"
            );
        }

        /// **A window that would crowd a minimum goes where `overflow`
        /// says.** Window 1 needs 2450x1350 of the 2536x1416 it has: beside it
        /// is 74 wide and below it 54 high, both under the minimum, so window 2
        /// has no room either way and the shipped chain sends it to workspace
        /// 2. Before #115 it split window 1's tile, and window 1 drew over it.
        #[test]
        fn a_window_that_would_crowd_a_minimum_goes_where_overflow_says() {
            let mut scripts = layouts_with("");
            switched_on(&mut scripts, "super+t");
            let _ = scripts.opened(1, desk(vec![at_least(1, 2450, 1350)], (2000.0, 700.0)));
            let _ = scripts.opened(
                2,
                desk(vec![at_least(1, 2450, 1350), plain(2)], (2000.0, 700.0)),
            );
            assert_eq!(
                scripts.evaluate("return tostring(require(\"workspaces\").at(2, \"DP-1\"))"),
                "2",
                "window 2 was squeezed in beside a window that has no room to give"
            );
        }

        /// **A window that cannot have its minimum is cramped, and the log
        /// says so once, with the numbers.** Window 1 says 2500 wide; the most
        /// window 2 can give leaves it 2364, window 2 at the 160 minimum. The
        /// layout says `cramped` on every placement and names it in the log on
        /// the first; a window that has room again stops being cramped, and
        /// one that loses it again is named again.
        #[test]
        fn a_window_that_cannot_have_its_minimum_is_cramped_and_said_once() {
            let mut scripts = layouts_with("");
            side_by_side(&mut scripts);
            let crowded = || desk(vec![at_least(1, 2500, 0), plain(2)], (0.0, 0.0));

            let told = scripts.relayout(crowded());
            let (one, _, cramped) = last_place(&told.commands, 1).expect("window 1 placed");
            assert!(about(one.w, 2364.0), "{one:?}");
            assert!(cramped, "window 1 is under its minimum and not said to be");
            assert!(
                !last_place(&told.commands, 2).is_some_and(|(_, _, cramped)| cramped),
                "window 2, at the minimum, is not cramped"
            );
            let said = logged(&scripts);
            assert!(
                said.contains("window 1 needs at least 2500 wide")
                    && said.contains("its tile is 2364x1416"),
                "{said}"
            );

            let told = scripts.relayout(crowded());
            assert!(last_place(&told.commands, 1).is_some_and(|(_, _, cramped)| cramped));
            assert_eq!(
                logged(&scripts).matches("is cramped").count(),
                1,
                "said again for a window that was cramped already"
            );

            let told = scripts.relayout(desk(vec![plain(1), plain(2)], (0.0, 0.0)));
            assert!(!last_place(&told.commands, 1).is_some_and(|(_, _, cramped)| cramped));
            let _ = scripts.relayout(crowded());
            assert_eq!(logged(&scripts).matches("is cramped").count(), 2);
        }

        /// **A seam dragged towards a window stops at its minimum.** Window 2
        /// needs 1500, so window 1 has 1024; dragging window 1's right edge out
        /// to 2000 leaves both where they are.
        #[test]
        fn a_seam_dragged_towards_a_window_stops_at_its_minimum() {
            let mut scripts = layouts_with("");
            side_by_side(&mut scripts);
            let desk_now = || desk(vec![plain(1), at_least(2, 1500, 0)], (0.0, 0.0));
            let told = scripts.relayout(desk_now());
            assert!(
                about(width_of(&told, 1), 1024.0),
                "the premise: {:?}",
                told.commands
            );

            let told = scripts.resized(1, (2000.0, 700.0), (Some("right"), None), desk_now());
            assert!(
                about(width_of(&told, 1), 1024.0) && about(width_of(&told, 2), 1500.0),
                "the drag went past window 2's minimum: {:?}",
                told.commands
            );
        }

        /// **A launched window whose minimum does not fit the tile it was
        /// given is placed again, once, before it is shown -- and never
        /// after.**
        ///
        /// Window 1 fills the screen and window 2 is launched beside it, with
        /// nothing said yet. Its application then says 2450x1350 before its
        /// first frame: beside window 1 there is no room either way, so it goes
        /// where the shipped chain sends it, workspace 2, as it would have at
        /// an `open` that knew. The same minimum arriving after the first
        /// frame is a window already on screen, which is rebalanced and left
        /// cramped where it is. And a window whose desk the user has switched
        /// away from is left on it, in its own desk's tree and no other.
        #[test]
        fn a_launched_window_is_placed_again_by_its_minimum_only_before_it_is_shown() {
            for (shown, away) in [(false, false), (true, false), (false, true)] {
                let mut scripts = layouts_with("");
                switched_on(&mut scripts, "super+t");
                let _ = scripts.opened(1, desk(vec![plain(1)], (2000.0, 700.0)));
                let launched = WindowInfo {
                    icon: None,
                    client_side: false,
                    blur_region: false,
                    insets: crate::decoration::Insets::default(),
                    shown: false,
                    ..plain(2)
                };
                let _ = scripts.opened(2, desk(vec![plain(1), launched], (2000.0, 700.0)));
                let workspace = |scripts: &Scripts| {
                    scripts.evaluate("return tostring(require(\"workspaces\").of[2])")
                };
                assert_eq!(workspace(&scripts), "1", "the premise");

                let arrived = WindowInfo {
                    icon: None,
                    client_side: false,
                    blur_region: false,
                    insets: crate::decoration::Insets::default(),
                    shown,
                    ..at_least(2, 2450, 1350)
                };
                if away {
                    assert!(
                        scripts
                            .key("super+2", desk(vec![plain(1), arrived.clone()], (0.0, 0.0)))
                            .handled,
                        "super+2 was not handled"
                    );
                }
                let told = scripts.relayout(desk(vec![plain(1), arrived], (0.0, 0.0)));
                if away {
                    assert_eq!(
                        workspace(&scripts),
                        "1",
                        "a window on a desk out of view was moved"
                    );
                    assert_eq!(
                        scripts.evaluate(
                            "local tree = require(\"tiling\").trees[require(\"monitors\").key(2, \"DP-1\")]\n\
                             return tostring(tree ~= nil and tree:contains(2))"
                        ),
                        "false",
                        "the window is in the tree of a desk it is not on"
                    );
                } else if shown {
                    assert_eq!(workspace(&scripts), "1", "a window on screen was sent away");
                    assert!(
                        last_place(&told.commands, 2).is_some_and(|(_, _, cramped)| cramped),
                        "{:?}",
                        told.commands
                    );
                } else {
                    assert_eq!(
                        workspace(&scripts),
                        "2",
                        "the launched window stayed in a tile that cannot hold it"
                    );
                    let again = WindowInfo {
                        icon: None,
                        client_side: false,
                        blur_region: false,
                        insets: crate::decoration::Insets::default(),
                        shown: false,
                        ..at_least(2, 2450, 1350)
                    };
                    let _ = scripts.relayout(desk(vec![plain(1), again], (0.0, 0.0)));
                    assert_eq!(workspace(&scripts), "2", "placed a second time");
                }
            }
        }

        /// **What a floating drag believes reaches the compositor**:
        /// `sizes.lua` hands `floating.client_limits` and
        /// `tiling.client_size_ignore` over as it loads, and the shipped
        /// settings are the compositor's own default.
        #[test]
        fn the_floating_setting_and_the_ignored_applications_reach_the_compositor() {
            let handed = |before: &str| {
                layouts_with(before)
                    .startup()
                    .commands
                    .into_iter()
                    .find_map(|command| match command {
                        Command::ClientSizes(sizes) => Some(sizes),
                        _ => None,
                    })
            };
            assert_eq!(handed(""), Some(super::super::ClientSizes::default()));
            for before in [
                "require(\"config\").floating.client_limits = \"respekt\"",
                "require(\"config\").tiling.client_size_ignore = \"firefox\"",
            ] {
                assert_eq!(
                    handed(before),
                    Some(super::super::ClientSizes::default()),
                    "{before}: not read as the default"
                );
            }
            assert_eq!(
                handed(
                    "local config = require(\"config\")\n\
                     config.floating.client_limits = \"ignore\"\n\
                     config.tiling.client_size_ignore = { \"firefox\", 7 }"
                ),
                Some(super::super::ClientSizes {
                    floating: false,
                    ignored: vec!["firefox".to_owned()],
                })
            );
        }

        /// **A reload does not name a cramped window again**: it is the same
        /// window in the same tile, and the log said so once already.
        #[test]
        fn a_reload_does_not_name_a_cramped_window_again() {
            let entry = entry_with("");
            let mut scripts = Scripts::load(&entry).expect("loading the shipped layouts");
            side_by_side(&mut scripts);
            let crowded = || desk(vec![at_least(1, 2500, 0), plain(2)], (0.0, 0.0));
            let _ = scripts.relayout(crowded());
            assert_eq!(
                logged(&scripts).matches("is cramped").count(),
                1,
                "the premise"
            );

            let mut fresh =
                Scripts::load_carrying(&entry, scripts.kept()).expect("reloading the scripts");
            let _ = fresh.restored(crowded());
            let _ = fresh.monitors_changed(crowded());
            let told = fresh.relayout(crowded());
            assert!(
                last_place(&told.commands, 1).is_some_and(|(_, _, cramped)| cramped),
                "the premise: still cramped after the reload"
            );
            assert_eq!(
                logged(&fresh).matches("is cramped").count(),
                0,
                "named again after a reload"
            );
        }

        /// **A scrolling column is never narrower than its window's
        /// minimum.** A column opens at a third of the 2536 inside the gap,
        /// 845; a window that needs 1500 has a 1500 column.
        #[test]
        fn a_scrolling_column_is_as_wide_as_its_windows_minimum() {
            let mut scripts = layouts_with("");
            switched_on(&mut scripts, "super+s");
            let told = scripts.opened(1, desk(vec![at_least(1, 1500, 0)], (0.0, 0.0)));
            assert!(
                about(width_of(&told, 1), 1500.0),
                "the column is narrower than its window: {:?}",
                told.commands
            );
        }
    }

    /// **#118 and #129: a reload is a change of configuration, not of
    /// arrangement.**
    ///
    /// Every layout on every desk comes back from `super+shift+r` exactly as it
    /// was: the same windows in the same tiles at the same split ratios, and the
    /// same columns in the same order at the same widths with the view where it
    /// was. Driven through the sequence `Solium::reload` runs -- the keep read
    /// off the running scripts, the configuration read again, then `restore`,
    /// `monitors` and `layout` -- against the shipped layouts, as the modules
    /// above run them.
    mod reload {
        use super::*;
        use std::collections::HashMap;
        use std::path::{Path, PathBuf};

        /// [`AREA`] less the shipped gap all round: the one tile a workspace
        /// with one window has.
        const FULL: Rect = Rect {
            x: 12.0,
            y: 12.0,
            w: 2536.0,
            h: 1416.0,
        };

        /// The shipped layouts in the order `init.lua` requires them, with
        /// `before` run ahead of them. Written again between two loads, it is
        /// a configuration edited between them -- which is what a reload is
        /// for.
        fn write(entry: &Path, before: &str) {
            std::fs::write(
                entry,
                format!(
                    "package.path = {shipped:?} .. \"/?.lua\"\n\
                     {before}\n\
                     require(\"modes\")\n\
                     require(\"workspaces\")\n\
                     require(\"tiling\")\n\
                     require(\"scrolling\")\n",
                    shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
                ),
            )
            .expect("writing the entry point");
        }

        /// The session, and the entry point a reload reads again. A directory
        /// per call, for the reason [`scripts`] gives.
        fn session(before: &str) -> (Scripts, PathBuf) {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let directory =
                std::env::temp_dir().join(format!("solium-reload-{}-{serial}", std::process::id()));
            let _ = std::fs::create_dir_all(&directory);
            let entry = directory.join("init.lua");
            write(&entry, before);
            let scripts = Scripts::load(&entry).expect("loading the shipped layouts");
            (scripts, entry)
        }

        /// Exactly what `Solium::reload` does, in its order, with the world
        /// looking like `now` throughout. The new scripts, and every command
        /// the three announcements produced.
        fn reload(scripts: &Scripts, entry: &Path, now: &Snapshot) -> (Scripts, Vec<Command>) {
            let mut fresh =
                Scripts::load_carrying(entry, scripts.kept()).expect("reloading the scripts");
            let mut commands = fresh.restored(now.clone()).commands;
            commands.extend(fresh.monitors_changed(now.clone()).commands);
            commands.extend(fresh.relayout(now.clone()).commands);
            (fresh, commands)
        }

        /// The windows `ids` on `DP-1`, topmost first as the compositor lists
        /// them, each where `rects` says it was placed, with the pointer at
        /// `cursor`. A window `rects` has nothing for is one being mapped: the
        /// size its client chose, at the origin.
        fn desk(rects: &HashMap<u64, Rect>, ids: &[u64], cursor: (f64, f64)) -> Snapshot {
            Snapshot {
                cursor,
                ..snapshot(ids.iter().map(|&id| at_rest(rects, id)).collect())
            }
        }

        fn at_rest(rects: &HashMap<u64, Rect>, id: u64) -> WindowInfo {
            match rects.get(&id) {
                Some(rect) => WindowInfo {
                    icon: None,
                    client_side: false,
                    blur_region: false,
                    insets: crate::decoration::Insets::default(),
                    rect: *rect,
                    ..window(id, rect.w, rect.h)
                },
                None => window(id, 800.0, 600.0),
            }
        }

        /// Window `id` opens on top of `below` with the pointer at `cursor`,
        /// and wherever that placed anything is recorded in `rects`.
        fn open(
            scripts: &mut Scripts,
            rects: &mut HashMap<u64, Rect>,
            id: u64,
            below: &[u64],
            cursor: (f64, f64),
        ) {
            let mut ids = vec![id];
            ids.extend_from_slice(below);
            let outcome = scripts.opened(id, desk(rects, &ids, cursor));
            rects.extend(placed(&outcome.commands));
        }

        /// `combo`, pressed over the windows `ids`, with what it placed
        /// recorded in `rects`.
        fn press(
            scripts: &mut Scripts,
            rects: &mut HashMap<u64, Rect>,
            ids: &[u64],
            combo: &str,
            cursor: (f64, f64),
        ) {
            let outcome = scripts.key(combo, desk(rects, ids, cursor));
            assert!(outcome.handled, "{combo} was not handled");
            rects.extend(placed(&outcome.commands));
        }

        /// One tree's or strip's arrangement of a 2560x1440 screen, as text:
        /// every slot it lays out, in its own order, and for a strip the window
        /// it has focused. The columns' order and widths and where the view sits
        /// are all in the slots. `nil` when the layout holds nothing under
        /// `key`, which is what a hidden desk's tree was after a reload before
        /// #129.
        fn arrangement(scripts: &Scripts, layout: &str, key: &str) -> String {
            let (table, focus) = if layout == "tiling" {
                ("trees", "")
            } else {
                (
                    "views",
                    "out[#out + 1] = \"focus=\" .. tostring(each:focused())",
                )
            };
            scripts.evaluate(&format!(
                "local each = require({layout:?}).{table}[{key:?}]\n\
                 if not each then return \"nil\" end\n\
                 local area = {{ x = 0, y = 0, w = 2560, h = 1440, gap = 12 }}\n\
                 local out = {{}}\n\
                 for _, slot in ipairs(each:layout(area)) do\n\
                     out[#out + 1] = string.format(\"%d@%.2f,%.2f,%.2fx%.2f\",\n\
                         slot.id, slot.x, slot.y, slot.w, slot.h)\n\
                 end\n\
                 {focus}\n\
                 return table.concat(out, \" \")"
            ))
        }

        /// The windows tiling holds under `key`, sorted, or `nil` for no tree.
        fn held(scripts: &Scripts, key: &str) -> String {
            scripts.evaluate(&format!(
                "local tree = require(\"tiling\").trees[{key:?}]\n\
                 if not tree then return \"nil\" end\n\
                 local ids = tree:windows()\n\
                 table.sort(ids)\n\
                 return table.concat(ids, \",\")"
            ))
        }

        fn same(left: Rect, right: Rect) -> bool {
            about(left.x, right.x)
                && about(left.y, right.y)
                && about(left.w, right.w)
                && about(left.h, right.h)
        }

        /// Every place `commands` sent, in order.
        fn places(commands: &[Command]) -> Vec<(u64, Rect)> {
            commands
                .iter()
                .filter_map(|command| match command {
                    Command::Place { id, rect, .. } => Some((*id, *rect)),
                    _ => None,
                })
                .collect()
        }

        /// Every place a reload sent is where that window already was, and
        /// every window in `ids` was sent one: the reload re-stated the
        /// arrangement rather than making another.
        fn nothing_moved(commands: &[Command], rects: &HashMap<u64, Rect>, ids: &[u64]) {
            let sent = places(commands);
            for id in ids {
                assert!(
                    sent.iter().any(|(placed, _)| placed == id),
                    "the reload did not lay out window {id}: {sent:?}"
                );
            }
            for (id, rect) in sent {
                let was = rects.get(&id).copied().unwrap_or_default();
                assert!(
                    same(rect, was),
                    "the reload moved window {id} from {was:?} to {rect:?}"
                );
            }
        }

        /// **#118: a reload leaves a tiled desk exactly as it was, dragged
        /// seam and all.**
        ///
        /// Three windows opened in order -- 2 beside 1 with the pointer on 1's
        /// right half, 3 under 2 with it on 2's lower half -- and then the seam
        /// between the two columns dragged left to x=900, a split nothing would
        /// give a window by itself. The reload's snapshot lists the windows as
        /// the compositor does, topmost first, and 1 is on top, having just
        /// been dragged: not the order they were opened in.
        ///
        /// Before #118 the tree died with the Lua state and `adopt` built
        /// another from that list: 1, then 3 beside it, then 2 under 1, every
        /// split at the configured 0.5.
        #[test]
        fn a_reload_leaves_a_tiled_desk_exactly_as_it_was() {
            let (mut scripts, entry) = session("");
            let mut rects = HashMap::new();
            press(&mut scripts, &mut rects, &[], "super+t", (1280.0, 720.0));
            open(&mut scripts, &mut rects, 1, &[], (1280.0, 720.0));
            open(&mut scripts, &mut rects, 2, &[1], (1900.0, 720.0));
            open(&mut scripts, &mut rects, 3, &[2, 1], (1900.0, 1100.0));
            let dragged = scripts.resized(
                1,
                (900.0, 720.0),
                (Some("right"), None),
                desk(&rects, &[1, 3, 2], (900.0, 720.0)),
            );
            rects.extend(placed(&dragged.commands));
            assert!(
                rects.get(&1).is_some_and(|rect| about(rect.w, 888.0))
                    && rects
                        .get(&2)
                        .zip(rects.get(&3))
                        .is_some_and(|(two, three)| about(two.x, 912.0) && three.y > two.y),
                "the fixture is not the one described: {rects:?}"
            );
            let tree = arrangement(&scripts, "tiling", "1@DP-1");

            let (reloaded, commands) =
                reload(&scripts, &entry, &desk(&rects, &[1, 3, 2], (900.0, 720.0)));
            assert_eq!(
                arrangement(&reloaded, "tiling", "1@DP-1"),
                tree,
                "the reload came back with a different tree"
            );
            nothing_moved(&commands, &rects, &[1, 2, 3]);
            // And the scripts it was read from still have theirs: a reload
            // that fails goes on running them, so reading the keep copies.
            assert_eq!(
                arrangement(&scripts, "tiling", "1@DP-1"),
                tree,
                "reading the keep took the tree away from the running scripts"
            );
        }

        /// **#118 as it was reported: on workspace 3, Firefox and then kitty,
        /// a reload, and the two traded places.**
        ///
        /// Firefox takes the screen; kitty opens with the pointer on Firefox's
        /// right half and takes that half. kitty is the window in use, so it is
        /// topmost when the reload comes and the compositor lists it first.
        /// Rebuilt from that list, kitty went in first and took the screen, and
        /// Firefox split it with no pointer to go by -- onto the far side, the
        /// right.
        #[test]
        fn firefox_and_kitty_on_workspace_3_keep_their_places_through_a_reload() {
            let (firefox, kitty) = (1, 2);
            let (mut scripts, entry) = session("");
            let mut rects = HashMap::new();
            press(&mut scripts, &mut rects, &[], "super+t", (1280.0, 720.0));
            press(&mut scripts, &mut rects, &[], "super+3", (1280.0, 720.0));
            open(&mut scripts, &mut rects, firefox, &[], (1280.0, 720.0));
            open(&mut scripts, &mut rects, kitty, &[firefox], (1900.0, 720.0));
            assert!(
                rects
                    .get(&firefox)
                    .zip(rects.get(&kitty))
                    .is_some_and(|(left, right)| left.x < right.x && about(left.w, right.w)),
                "the fixture is Firefox on the left and kitty on the right: {rects:?}"
            );

            let (scripts, commands) = reload(
                &scripts,
                &entry,
                &desk(&rects, &[kitty, firefox], (1900.0, 720.0)),
            );
            nothing_moved(&commands, &rects, &[firefox, kitty]);
            let desks = scripts.evaluate(
                "local workspaces = require(\"workspaces\")\n\
                 return string.format(\"%d %d %d\", workspaces.at(1, \"DP-1\"),\n\
                     workspaces.at(2, \"DP-1\"), workspaces.on(\"DP-1\"))",
            );
            assert_eq!(desks, "3 3 3", "Firefox, kitty, and the workspace in view");
        }

        /// **#129: a desk nobody is looking at keeps its arrangement through a
        /// reload, and through a `monitors` event**, and a window opened there
        /// afterwards splits it rather than covering the screen.
        ///
        /// Two windows side by side on workspace 1, and the view on workspace
        /// 2 when it happens. `adopt` runs on both -- `restore` switches tiling
        /// back on, and `monitors` follows on every reload as on every hotplug
        /// -- and it measured every tree against the windows on the desks *in
        /// view*. So workspace 1's tree lost both windows to a `monitors`
        /// event, and after a reload did not exist at all. Back on workspace 1
        /// the windows were still in their tiles, where the compositor had left
        /// them, and the layout did not know they were there: the next window
        /// took the whole screen, over the top of both.
        #[test]
        fn a_hidden_desk_keeps_its_arrangement_through_a_reload_and_a_monitors_event() {
            for route in ["a reload", "a monitors event"] {
                let (mut scripts, entry) = session("");
                let mut rects = HashMap::new();
                press(&mut scripts, &mut rects, &[], "super+t", (1280.0, 720.0));
                open(&mut scripts, &mut rects, 1, &[], (1280.0, 720.0));
                open(&mut scripts, &mut rects, 2, &[1], (1900.0, 720.0));
                let tree = arrangement(&scripts, "tiling", "1@DP-1");
                press(
                    &mut scripts,
                    &mut rects,
                    &[2, 1],
                    "super+2",
                    (1280.0, 720.0),
                );

                let away = desk(&rects, &[2, 1], (1280.0, 720.0));
                let mut scripts = if route == "a reload" {
                    reload(&scripts, &entry, &away).0
                } else {
                    let _ = scripts.monitors_changed(away.clone());
                    let _ = scripts.relayout(away);
                    scripts
                };
                press(
                    &mut scripts,
                    &mut rects,
                    &[2, 1],
                    "super+1",
                    (1900.0, 1100.0),
                );
                assert_eq!(
                    arrangement(&scripts, "tiling", "1@DP-1"),
                    tree,
                    "after {route} on workspace 2, workspace 1's arrangement is not the one it had"
                );

                let (left, right) = (
                    rects.get(&1).copied().unwrap_or_default(),
                    rects.get(&2).copied().unwrap_or_default(),
                );
                open(&mut scripts, &mut rects, 3, &[2, 1], (1900.0, 1100.0));
                let third = rects.get(&3).copied().unwrap_or_default();
                assert!(
                    !same(third, FULL),
                    "after {route}, window 3 was given the whole screen, over windows 1 and 2"
                );
                let now: Vec<Rect> = [1, 2, 3]
                    .iter()
                    .filter_map(|id| rects.get(id).copied())
                    .collect();
                assert!(
                    rects.get(&1).is_some_and(|rect| same(*rect, left))
                        && [2, 3].iter().all(|id| {
                            rects.get(id).is_some_and(|rect| {
                                rect.x >= right.x - 1.0
                                    && rect.x + rect.w <= right.x + right.w + 1.0
                            })
                        })
                        && now.iter().enumerate().all(|(index, one)| {
                            now.iter()
                                .skip(index + 1)
                                .all(|other| !overlap(*one, *other))
                        }),
                    "after {route}, window 3 did not split window 2's tile: {rects:?}"
                );
            }
        }

        /// **A reload leaves every scrolling strip as it was, in view or not**:
        /// which windows share a column, the columns' order and widths, which
        /// one is focused and where the view sits. And so does a `monitors`
        /// event, which every reload also announces.
        ///
        /// Workspace 1: three columns, the third widened with `super+r` and
        /// carried to the middle, then focus back on the first, which scrolls
        /// the view. Workspace 2, in view throughout: two windows stacked in
        /// one column, widened. Before, `monitors` emptied the strip out of
        /// view (#129); and a reload rebuilt each strip in view from the
        /// snapshot -- a column per window, in stacking order, at the width a
        /// column opens at -- and a strip out of view not at all (#118).
        #[test]
        fn a_reload_leaves_every_scrolling_strip_as_it_was() {
            let cursor = (1280.0, 720.0);
            let (mut scripts, entry) = session("");
            let mut rects = HashMap::new();
            press(&mut scripts, &mut rects, &[], "super+s", cursor);
            open(&mut scripts, &mut rects, 1, &[], cursor);
            open(&mut scripts, &mut rects, 2, &[1], cursor);
            open(&mut scripts, &mut rects, 3, &[2, 1], cursor);
            let first = [3, 2, 1];
            press(&mut scripts, &mut rects, &first, "super+r", cursor);
            press(
                &mut scripts,
                &mut rects,
                &first,
                "super+ctrl+bracketleft",
                cursor,
            );
            press(
                &mut scripts,
                &mut rects,
                &first,
                "super+bracketleft",
                cursor,
            );
            let hidden = arrangement(&scripts, "scrolling", "1@DP-1");
            assert!(
                hidden.ends_with("focus=1")
                    && rects
                        .get(&3)
                        .zip(rects.get(&2))
                        .is_some_and(|(three, two)| three.x < two.x && three.w > two.w),
                "the fixture is [1] [3, widened] [2] with 1 focused: {hidden}"
            );

            press(&mut scripts, &mut rects, &first, "super+2", cursor);
            open(&mut scripts, &mut rects, 4, &[3, 2, 1], cursor);
            open(&mut scripts, &mut rects, 5, &[4, 3, 2, 1], cursor);
            let all = [5, 4, 3, 2, 1];
            press(&mut scripts, &mut rects, &all, "super+bracketleft", cursor);
            press(&mut scripts, &mut rects, &all, "super+comma", cursor);
            press(&mut scripts, &mut rects, &all, "super+r", cursor);
            let shown = arrangement(&scripts, "scrolling", "2@DP-1");
            assert!(
                rects
                    .get(&4)
                    .zip(rects.get(&5))
                    .is_some_and(|(four, five)| about(four.x, five.x)
                        && about(four.w, 1268.0)
                        && five.y > four.y),
                "the fixture is 4 over 5 in one column, half the view wide: {rects:?}"
            );

            let now = desk(&rects, &all, cursor);
            let _ = scripts.monitors_changed(now.clone());
            let _ = scripts.relayout(now.clone());
            assert_eq!(
                [
                    arrangement(&scripts, "scrolling", "1@DP-1"),
                    arrangement(&scripts, "scrolling", "2@DP-1")
                ],
                [hidden.clone(), shown.clone()],
                "a monitors event changed a strip, out of view or in it"
            );

            let (scripts, commands) = reload(&scripts, &entry, &now);
            assert_eq!(
                arrangement(&scripts, "scrolling", "2@DP-1"),
                shown,
                "the strip in view came back different"
            );
            assert_eq!(
                arrangement(&scripts, "scrolling", "1@DP-1"),
                hidden,
                "the strip out of view came back different"
            );
            nothing_moved(&commands, &rects, &[4, 5]);
        }

        /// **And a reload still changes the configuration**: a strip keeps its
        /// columns, and takes the widths the file now names.
        ///
        /// `scrolling.widths` is read where a strip is made (#117), and a strip
        /// that is carried across a reload is not made again. Left at that, it
        /// would go on opening columns at, and cycling through, the widths the
        /// *last* file named: an edited setting that nothing reads. So the
        /// columns already there keep the widths they have, a new one opens at
        /// the new list's first, and `super+r` walks the new list.
        #[test]
        fn a_reload_keeps_the_strip_and_takes_the_widths_the_file_now_names() {
            let cursor = (1280.0, 720.0);
            let (mut scripts, entry) = session("");
            let mut rects = HashMap::new();
            press(&mut scripts, &mut rects, &[], "super+s", cursor);
            open(&mut scripts, &mut rects, 1, &[], cursor);
            open(&mut scripts, &mut rects, 2, &[1], cursor);
            press(&mut scripts, &mut rects, &[2, 1], "super+r", cursor);
            let before = arrangement(&scripts, "scrolling", "1@DP-1");
            assert!(
                rects
                    .get(&1)
                    .is_some_and(|rect| about(rect.w, 2536.0 / 3.0))
                    && rects.get(&2).is_some_and(|rect| about(rect.w, 1268.0)),
                "the fixture is a third and a half: {rects:?}"
            );

            write(
                &entry,
                "require(\"config\").scrolling.widths = { 0.25, 0.75 }",
            );
            let (mut scripts, _) = reload(&scripts, &entry, &desk(&rects, &[2, 1], cursor));
            assert_eq!(
                arrangement(&scripts, "scrolling", "1@DP-1"),
                before,
                "the columns did not keep their widths through the reload"
            );
            open(&mut scripts, &mut rects, 3, &[2, 1], cursor);
            assert!(
                rects.get(&3).is_some_and(|rect| about(rect.w, 634.0)),
                "a column opened after the reload is not the new first width, a quarter: {rects:?}"
            );
            press(&mut scripts, &mut rects, &[3, 2, 1], "super+r", cursor);
            assert!(
                rects.get(&3).is_some_and(|rect| about(rect.w, 1902.0)),
                "super+r after the reload did not go to the new list's next width: {rects:?}"
            );
        }

        /// **A monitor that goes away still hands its windows to the one that
        /// is left** -- the job the `monitors` event was added for.
        ///
        /// `DP-2` holds 3 and 4 on the workspace it shows and 5 on its
        /// workspace 2, out of view. It is unplugged: every window is on `DP-1`
        /// now, and `monitors` is announced. 3 and 4 join workspace 1's tree
        /// there beside 1 and 2, and are laid out on `DP-1`. 5 joins workspace
        /// 2's -- the workspace it was on, on the screen it is on now -- where
        /// before #129 it was taken out of `DP-2`'s tree and put into none,
        /// because `adopt` only put back a window on a desk in view. No tree of
        /// the screen that went holds anything.
        #[test]
        fn a_monitor_that_goes_away_still_hands_its_windows_on() {
            let (mut scripts, _) = session("");
            let both = || vec![monitor(), second_monitor()];
            let two = |rects: &HashMap<u64, Rect>, ids: &[u64], cursor: (f64, f64)| Snapshot {
                cursor,
                ..snapshot_on(
                    both(),
                    ids.iter()
                        .map(|&id| {
                            let each = at_rest(rects, id);
                            if id >= 3 && !rects.contains_key(&id) {
                                on(&second_monitor(), each)
                            } else if id >= 3 {
                                WindowInfo {
                                    icon: None,
                                    client_side: false,
                                    blur_region: false,
                                    insets: crate::decoration::Insets::default(),
                                    monitor: "DP-2".to_owned(),
                                    ..each
                                }
                            } else {
                                each
                            }
                        })
                        .collect(),
                )
            };
            let mut rects: HashMap<u64, Rect> = HashMap::new();
            let outcome = scripts.key("super+t", two(&rects, &[], (1280.0, 720.0)));
            assert!(outcome.handled, "the tiling key was not handled");
            let opening = |scripts: &mut Scripts,
                           rects: &mut HashMap<u64, Rect>,
                           id: u64,
                           below: &[u64],
                           cursor: (f64, f64)| {
                let mut ids = vec![id];
                ids.extend_from_slice(below);
                let outcome = scripts.opened(id, two(rects, &ids, cursor));
                rects.extend(placed(&outcome.commands));
            };
            opening(&mut scripts, &mut rects, 1, &[], (1280.0, 720.0));
            opening(&mut scripts, &mut rects, 2, &[1], (1900.0, 720.0));
            opening(&mut scripts, &mut rects, 3, &[2, 1], (3840.0, 720.0));
            opening(&mut scripts, &mut rects, 4, &[3, 2, 1], (4460.0, 720.0));
            let _ = scripts.evaluate_in(
                two(&rects, &[4, 3, 2, 1], (4460.0, 720.0)),
                "require(\"workspaces\").go(2, \"DP-2\") return \"\"",
            );
            opening(&mut scripts, &mut rects, 5, &[4, 3, 2, 1], (3840.0, 720.0));
            let _ = scripts.evaluate_in(
                two(&rects, &[5, 4, 3, 2, 1], (3840.0, 720.0)),
                "require(\"workspaces\").go(1, \"DP-2\") return \"\"",
            );
            assert_eq!(
                [
                    held(&scripts, "1@DP-1"),
                    held(&scripts, "1@DP-2"),
                    held(&scripts, "2@DP-2")
                ],
                ["1,2", "3,4", "5"],
                "the fixture is two screens, and 5 on DP-2's workspace 2"
            );

            // Unplugged: the compositor has already moved every window onto
            // the screen that is left, by the width of the one that went.
            let gone = snapshot(
                [5, 4, 3, 2, 1]
                    .iter()
                    .map(|&id| {
                        let mut each = at_rest(&rects, id);
                        if id >= 3 {
                            each.rect.x -= BESIDE.x;
                        }
                        each
                    })
                    .collect(),
            );
            let _ = scripts.monitors_changed(gone.clone());
            let after = placed(&scripts.relayout(gone).commands);
            let shown: Vec<Rect> = [1, 2, 3, 4]
                .iter()
                .filter_map(|id| after.get(id).copied())
                .collect();
            assert!(
                shown.len() == 4
                    && shown.iter().all(|rect| rect.x + rect.w <= AREA.w + 1.0)
                    && shown.iter().enumerate().all(|(index, one)| {
                        shown
                            .iter()
                            .skip(index + 1)
                            .all(|other| !overlap(*one, *other))
                    }),
                "the windows on the workspace in view are not all laid out on DP-1: {after:?}"
            );
            assert_eq!(held(&scripts, "1@DP-1"), "1,2,3,4");
            assert_eq!(
                held(&scripts, "2@DP-1"),
                "5",
                "the window on DP-2's hidden workspace is in no tree of the screen it is on"
            );
            assert_eq!(
                [held(&scripts, "1@DP-2"), held(&scripts, "2@DP-2")],
                ["", ""],
                "a tree of the screen that went still holds a window"
            );
        }
    }
}

#[cfg(test)]
impl Scripts {
    /// Run a chunk and hand back what it returns, as text.
    ///
    /// For a test to read what a script recorded or computed, which is
    /// otherwise invisible from out here: a dispatch returns commands, and a
    /// table a script keeps for itself is not one. Test-only, and after the
    /// first `#[cfg(test)]` on purpose -- `every_event_listened_for_is_one_that_is_sent`
    /// reads this file's production half by cutting at the first one.
    pub(crate) fn evaluate(&self, chunk: &str) -> String {
        self.lua
            .load(chunk)
            .eval::<String>()
            .unwrap_or_else(|err| format!("error: {err}"))
    }

    /// [`Self::evaluate`], with `snapshot` as what the compositor looks like:
    /// so a chunk that asks `sol.window_at` is answered from the snapshot a
    /// handler dispatched at that instant would be handed, rather than from
    /// whichever the last dispatch left behind. For a test that asks from Lua
    /// what the compositor asks in Rust --
    /// `on_two_monitors_sol_window_at_answers_what_the_right_monitor_draws`.
    pub(crate) fn evaluate_in(&self, snapshot: Snapshot, chunk: &str) -> String {
        self.lua.set_app_data(snapshot);
        self.evaluate(chunk)
    }
}

/// Focus and move by direction (#150), against the shipped scripts.
///
/// A compositor small enough to read: each window's rect and which one has the
/// keyboard. `sol.place` moves a window, and its monitor with it by where its
/// centre lands, as `Solium::output_of` decides; `sol.focus` moves the keyboard
/// and raises the window. Every press is dispatched from the desk as the last
/// one left it, which is what the compositor hands a script.
#[cfg(test)]
mod directions {
    use super::{
        AnimationSpec, Command, Drawn, FocusSource, Interrupt, MonitorInfo, Parentage, Rect,
        Scripts, Snapshot, Warp, WarpTo, WindowInfo,
    };
    use crate::present::Curve;
    use std::path::PathBuf;

    /// A landscape screen at the origin.
    const WIDE: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 2560.0,
        h: 1440.0,
    };

    /// A portrait screen to its right, so a column beside it is taller than
    /// it is wide and splits one above the other.
    const TALL: Rect = Rect {
        x: 2560.0,
        y: 0.0,
        w: 1440.0,
        h: 2560.0,
    };

    fn screen(name: &str, area: Rect) -> MonitorInfo {
        MonitorInfo {
            name: name.to_owned(),
            area,
            whole: area,
            scale: 1.0,
            focused: name == "DP-1",
            primary: name == "DP-1",
            transform: "normal".to_owned(),
            off: false,
        }
    }

    fn one_screen() -> Vec<MonitorInfo> {
        vec![screen("DP-1", WIDE)]
    }

    fn two_screens() -> Vec<MonitorInfo> {
        vec![screen("DP-1", WIDE), screen("DP-2", TALL)]
    }

    /// A smaller landscape screen top-aligned to the right of `WIDE`: a
    /// laptop's panel beside a monitor, the shape where the one reaches
    /// lower than the other and neither is above it.
    const BESIDE: Rect = Rect {
        x: 2560.0,
        y: 0.0,
        w: 1920.0,
        h: 1080.0,
    };

    /// A screen touching `WIDE` only at its bottom-right corner.
    const CORNER: Rect = Rect {
        x: 2560.0,
        y: 1440.0,
        w: 1920.0,
        h: 1080.0,
    };

    /// A smaller landscape screen whose right edge meets `TALL`'s left, so
    /// the bottom half of the portrait screen is level with nothing on it.
    const LEFT_OF_TALL: Rect = Rect {
        x: 640.0,
        y: 0.0,
        w: 1920.0,
        h: 1080.0,
    };

    /// A screen under `BESIDE`, so the two stand stacked to the right of
    /// `WIDE`: the upper level with its top, this one with its bottom.
    const UNDER_BESIDE: Rect = Rect {
        x: 2560.0,
        y: 1080.0,
        w: 1920.0,
        h: 1080.0,
    };

    /// A portrait screen with `ABOVE_LEFT` and `BELOW_LEFT` stacked on its
    /// left, the upper level with its top half and the lower with its bottom.
    const PORTRAIT: Rect = Rect {
        x: 1920.0,
        y: 0.0,
        w: 1440.0,
        h: 2560.0,
    };

    const ABOVE_LEFT: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 1920.0,
        h: 1080.0,
    };

    const BELOW_LEFT: Rect = Rect {
        x: 0.0,
        y: 1080.0,
        w: 1920.0,
        h: 1080.0,
    };

    fn window(id: u64, monitor: &str, rect: Rect) -> WindowInfo {
        WindowInfo {
            icon: None,
            client_side: false,
            blur_region: false,
            insets: crate::decoration::Insets::default(),
            id,
            rect,
            drawn: Drawn::default(),
            title: format!("window {id}"),
            focused: false,
            monitor: monitor.to_owned(),
            modal: false,
            parent: Parentage::None,
            leaving: false,
            minimized: false,
            above: false,
            app_id: String::new(),
            min: None,
            max: None,
            cramped: false,
            shown: true,
            fullscreen: false,
            maximized: false,
            style: String::new(),
            x11_type: None,
            accepts_input: true,
            class: None,
            instance: None,
            content_type: "none",
            pid: None,
        }
    }

    fn about(left: f64, right: f64) -> bool {
        (left - right).abs() < 1.0
    }

    fn same(left: Rect, right: Rect) -> bool {
        about(left.x, right.x)
            && about(left.y, right.y)
            && about(left.w, right.w)
            && about(left.h, right.h)
    }

    fn inside(rect: Rect, area: Rect) -> bool {
        rect.x >= area.x - 1.0
            && rect.y >= area.y - 1.0
            && rect.x + rect.w <= area.x + area.w + 1.0
            && rect.y + rect.h <= area.y + area.h + 1.0
    }

    /// The window ids `commands` handed the keyboard to, in order.
    fn focuses(commands: &[Command]) -> Vec<u64> {
        commands
            .iter()
            .filter_map(|command| match command {
                Command::Focus { id, .. } => Some(*id),
                _ => None,
            })
            .collect()
    }

    /// The window ids `commands` placed.
    fn places(commands: &[Command]) -> Vec<u64> {
        commands
            .iter()
            .filter_map(|command| match command {
                Command::Place { id, .. } => Some(*id),
                _ => None,
            })
            .collect()
    }

    struct Desk {
        scripts: Scripts,
        entry: PathBuf,
        monitors: Vec<MonitorInfo>,
        windows: Vec<WindowInfo>,
        cursor: (f64, f64),
    }

    impl Desk {
        /// The shipped scripts in the order `init.lua` requires them, with
        /// `before` run ahead of them, on these screens and no windows.
        /// `package.path` is the entry's own and the directory is one per call,
        /// for the reasons `dialogs::scripts` gives.
        fn new(before: &str, monitors: Vec<MonitorInfo>) -> Self {
            Self::new_with(before, "", monitors)
        }

        /// As `new`, with `after` run once every layout script this harness
        /// loads has been required -- the one hook a test needing
        /// `require("bindings")` has, since the real `init.lua` requires it
        /// *last* (`bindings.lua`'s own comment) and this harness otherwise
        /// has no module after `floating` at all.
        /// `a_config_bindings_override_on_an_arrow_survives_leaving_and_returning_to_floating`.
        fn new_with(before: &str, after: &str, monitors: Vec<MonitorInfo>) -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let directory = std::env::temp_dir()
                .join(format!("solium-directions-{}-{serial}", std::process::id()));
            let _ = std::fs::create_dir_all(&directory);
            let entry = directory.join("init.lua");
            std::fs::write(
                &entry,
                format!(
                    "package.path = {shipped:?} .. \"/?.lua\"\n\
                     {before}\n\
                     require(\"modes\")\n\
                     require(\"workspaces\")\n\
                     require(\"tiling\")\n\
                     require(\"scrolling\")\n\
                     require(\"direction\")\n\
                     require(\"floating\")\n\
                     {after}\n",
                    shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
                ),
            )
            .expect("writing the entry point");
            let scripts = Scripts::load(&entry).expect("loading the shipped layouts");
            Self {
                scripts,
                entry,
                monitors,
                windows: Vec::new(),
                cursor: (0.0, 0.0),
            }
        }

        /// Floating windows where `windows` says, and no layout in charge.
        fn floating(monitors: Vec<MonitorInfo>, windows: &[(u64, &str, Rect)]) -> Self {
            let mut desk = Self::new("", monitors);
            for &(id, monitor, rect) in windows {
                desk.windows.push(window(id, monitor, rect));
            }
            desk
        }

        fn snapshot(&self) -> Snapshot {
            let work_area = self
                .monitors
                .iter()
                .find(|monitor| monitor.focused)
                .map_or(WIDE, |monitor| monitor.area);
            Snapshot {
                windows: self.windows.clone(),
                monitors: self.monitors.clone(),
                work_area,
                cursor: self.cursor,
                ..Snapshot::default()
            }
        }

        fn apply(&mut self, commands: &[Command]) {
            for command in commands {
                match command {
                    Command::Place { id, rect, .. } => {
                        let (x, y) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
                        let covering = self
                            .monitors
                            .iter()
                            .find(|monitor| {
                                let whole = monitor.whole;
                                x >= whole.x
                                    && x < whole.x + whole.w
                                    && y >= whole.y
                                    && y < whole.y + whole.h
                            })
                            .map(|monitor| monitor.name.clone());
                        if let Some(window) =
                            self.windows.iter_mut().find(|window| window.id == *id)
                        {
                            window.rect = *rect;
                            if let Some(name) = covering {
                                window.monitor = name;
                            }
                        }
                    }
                    Command::Focus { id, .. } => self.focus(*id),
                    _ => {}
                }
            }
        }

        /// Press a bound key, and apply what it asked for.
        fn press(&mut self, combo: &str) -> Vec<Command> {
            let outcome = self.scripts.key(combo, self.snapshot());
            assert!(
                outcome.handled,
                "{combo} is not bound, or its handler failed"
            );
            self.apply(&outcome.commands);
            outcome.commands
        }

        /// Window `id` opens on `monitor` with the pointer at `cursor`, and has
        /// the keyboard once it is placed, as a new window does.
        fn open(&mut self, id: u64, monitor: &str, cursor: (f64, f64)) {
            let origin = self
                .monitors
                .iter()
                .find(|each| each.name == monitor)
                .map_or(WIDE, |each| each.area);
            self.windows.insert(
                0,
                window(
                    id,
                    monitor,
                    Rect {
                        w: 800.0,
                        h: 600.0,
                        ..origin
                    },
                ),
            );
            self.cursor = cursor;
            let outcome = self.scripts.opened(id, self.snapshot());
            self.apply(&outcome.commands);
            self.focus(id);
        }

        /// The keyboard to window `id`, which comes to the front.
        fn focus(&mut self, id: u64) {
            for window in &mut self.windows {
                window.focused = window.id == id;
            }
            if let Some(at) = self.windows.iter().position(|window| window.id == id) {
                let raised = self.windows.remove(at);
                self.windows.insert(0, raised);
            }
        }

        fn rect(&self, id: u64) -> Rect {
            self.windows
                .iter()
                .find(|window| window.id == id)
                .map(|window| window.rect)
                .unwrap_or_default()
        }

        fn monitor(&self, id: u64) -> String {
            self.windows
                .iter()
                .find(|window| window.id == id)
                .map(|window| window.monitor.clone())
                .unwrap_or_default()
        }

        fn rects(&self) -> Vec<(u64, Rect)> {
            let mut out: Vec<(u64, Rect)> = self
                .windows
                .iter()
                .map(|window| (window.id, window.rect))
                .collect();
            out.sort_by_key(|(id, _)| *id);
            out
        }

        fn workspace_of(&self, id: u64) -> String {
            self.scripts.evaluate(&format!(
                "return tostring(require(\"workspaces\").of[{id}])"
            ))
        }

        /// The desks whose arrangement in `module` holds window `id`, by the
        /// key each is kept under in its `field`, sorted and joined by commas:
        /// `holding("tiling", "trees", 4)` is the trees window 4 is a leaf of.
        fn holding(&self, module: &str, field: &str, id: u64) -> String {
            self.scripts.evaluate(&format!(
                "local out = {{}}\n\
                 for key, each in pairs(require(\"{module}\").{field}) do\n\
                     if each:contains({id}) then out[#out + 1] = key end\n\
                 end\n\
                 table.sort(out)\n\
                 return table.concat(out, \",\")"
            ))
        }

        /// Window `id` dragged until its centre is at `at`, on `monitor`, and
        /// let go there with the pointer on it: the `drop` a move grab ends
        /// with, and what it asked for applied.
        fn drop(&mut self, id: u64, monitor: &str, at: (f64, f64)) {
            if let Some(window) = self.windows.iter_mut().find(|window| window.id == id) {
                window.rect.x = at.0 - window.rect.w / 2.0;
                window.rect.y = at.1 - window.rect.h / 2.0;
                window.monitor = monitor.to_owned();
            }
            self.cursor = at;
            let outcome = self.scripts.dropped(id, at.0, at.1, self.snapshot());
            self.apply(&outcome.commands);
        }

        /// What `Solium::reload` does, with the desk as it is.
        fn reload(&mut self) {
            let mut fresh = Scripts::load_carrying(&self.entry, self.scripts.kept())
                .expect("reloading the scripts");
            let mut commands = fresh.restored(self.snapshot()).commands;
            commands.extend(fresh.monitors_changed(self.snapshot()).commands);
            commands.extend(fresh.relayout(self.snapshot()).commands);
            self.scripts = fresh;
            self.apply(&commands);
        }
    }

    /// Tiling on, and four windows opened into a two-by-two grid on `DP-1`:
    ///
    /// ```text
    /// 1 | 2
    /// --+--
    /// 3 | 4
    /// ```
    ///
    /// Each opened with the pointer where it splits the tile it lands in the
    /// way the picture says, and the picture checked, so a test reading
    /// directions off it cannot pass on some other arrangement.
    fn grid(before: &str, monitors: Vec<MonitorInfo>) -> Desk {
        let mut desk = Desk::new(before, monitors);
        desk.press("super+t");
        desk.open(1, "DP-1", (1280.0, 720.0));
        desk.open(2, "DP-1", (2000.0, 720.0));
        desk.open(3, "DP-1", (640.0, 1200.0));
        desk.open(4, "DP-1", (1920.0, 1200.0));
        let [one, two, three, four] = [1, 2, 3, 4].map(|id| desk.rect(id));
        assert!(
            one.x + one.w < two.x
                && three.x + three.w < four.x
                && one.y + one.h < three.y
                && two.y + two.h < four.y
                && about(one.x, three.x)
                && about(one.y, two.y),
            "the premise: a two-by-two grid, 1 2 over 3 4; it is {:?}",
            desk.rects()
        );
        desk
    }

    /// **In tiling, focus and move reach the neighbouring tile in each
    /// direction, on the arrows and on h, j, k and l.** #150.
    ///
    /// Every focus key from the corner that has a neighbour that way; every
    /// move key trades the window with that neighbour, and the opposite move
    /// puts both back exactly. At the edge of the only monitor nothing
    /// happens: no focus, and nothing placed anywhere else.
    #[test]
    fn in_tiling_focus_and_move_reach_the_neighbour_in_each_direction() {
        let mut desk = grid("", one_screen());
        for (from, key, to) in [
            (1, "super+right", 2),
            (1, "super+down", 3),
            (4, "super+left", 3),
            (4, "super+up", 2),
            (2, "super+h", 1),
            (2, "super+j", 4),
            (3, "super+k", 1),
            (3, "super+l", 4),
        ] {
            desk.focus(from);
            let commands = desk.press(key);
            assert_eq!(focuses(&commands), [to], "{key} from window {from}");
        }

        let start = desk.rects();
        for (from, key, with, back) in [
            (1, "super+shift+right", 2, "super+shift+h"),
            (1, "super+shift+down", 3, "super+alt+k"),
            (4, "super+shift+left", 3, "super+shift+l"),
            (4, "super+shift+up", 2, "super+shift+j"),
        ] {
            desk.focus(from);
            let (was, other) = (desk.rect(from), desk.rect(with));
            desk.press(key);
            assert!(
                same(desk.rect(from), other) && same(desk.rect(with), was),
                "{key} from window {from} did not trade tiles with window {with}: {:?}",
                desk.rects()
            );
            let commands = desk.press(back);
            assert!(
                focuses(&commands).is_empty(),
                "a move does not move the keyboard"
            );
            let now = desk.rects();
            assert!(
                start
                    .iter()
                    .zip(&now)
                    .all(|((_, before), (_, after))| same(*before, *after)),
                "{back} after {key} is not the desk it started as: {start:?} and now {now:?}"
            );
        }

        for (from, key) in [
            (1, "super+left"),
            (1, "super+up"),
            (4, "super+right"),
            (4, "super+down"),
        ] {
            desk.focus(from);
            assert!(
                focuses(&desk.press(key)).is_empty(),
                "{key} from window {from}, at the edge of the only monitor"
            );
        }
        for (from, key) in [(1, "super+shift+left"), (4, "super+shift+down")] {
            desk.focus(from);
            desk.press(key);
            let now = desk.rects();
            assert!(
                start
                    .iter()
                    .zip(&now)
                    .all(|((_, before), (_, after))| same(*before, *after)),
                "{key} from window {from} at the edge of the only monitor moved something: {now:?}"
            );
        }
    }

    /// **At a monitor's edge, tiling goes on to the next monitor, and the desk
    /// it is showing.** #150.
    ///
    /// The grid on the landscape screen; on the portrait one to its right,
    /// showing workspace 2, window 5 above window 6. Focus crosses both ways
    /// to the tile level with the window it left, the nearer when two are.
    /// A move crosses too: window 2 leaves the grid, whose other windows close
    /// up, and goes in beside window 5 on the side it arrived from, on
    /// workspace 2. Moved back, it splits the tile it arrives at, level with
    /// where it was -- which is the grid again. Into a monitor with nothing on
    /// it, a window takes the whole screen.
    #[test]
    fn at_a_monitors_edge_tiling_crosses_to_the_desk_the_next_monitor_shows() {
        let mut desk = grid("", two_screens());
        let start = desk.rects();
        assert_eq!(
            desk.scripts
                .evaluate("require(\"workspaces\").showing[\"DP-2\"] = 2 return \"\""),
            ""
        );
        desk.open(5, "DP-2", (3280.0, 1280.0));
        desk.open(6, "DP-2", (3280.0, 2400.0));
        let (five, six) = (desk.rect(5), desk.rect(6));
        assert!(
            five.y + five.h < six.y && inside(five, TALL) && inside(six, TALL),
            "the premise: 5 above 6 on the portrait screen: {:?}",
            desk.rects()
        );
        assert_eq!(
            desk.workspace_of(5),
            "2",
            "the premise: DP-2 is showing workspace 2"
        );

        for (from, key, to) in [
            (2, "super+right", 5),
            (4, "super+right", 5),
            (6, "super+left", 4),
            (5, "super+left", 2),
        ] {
            desk.focus(from);
            assert_eq!(focuses(&desk.press(key)), [to], "{key} from window {from}");
        }

        desk.focus(2);
        desk.press("super+shift+right");
        let (two, beside, four) = (desk.rect(2), desk.rect(5), desk.rect(4));
        assert_eq!(
            desk.monitor(2),
            "DP-2",
            "window 2 did not cross: {:?}",
            desk.rects()
        );
        assert!(
            inside(two, TALL) && two.x + two.w < beside.x && two.y < beside.y + beside.h,
            "window 2 did not go in on the left of window 5: {:?}",
            desk.rects()
        );
        assert!(same(desk.rect(6), six), "window 6 was not beside the move");
        assert!(
            four.y < WIDE.y + 20.0 && four.h > WIDE.h - 40.0,
            "the grid did not close up over window 2's tile: {:?}",
            desk.rects()
        );
        assert_eq!(
            desk.workspace_of(2),
            "2",
            "window 2 is on the desk DP-2 shows"
        );

        desk.press("super+shift+left");
        assert_eq!(
            desk.monitor(2),
            "DP-1",
            "window 2 did not come back: {:?}",
            desk.rects()
        );
        let now = desk.rects();
        assert!(
            start.iter().all(|(id, was)| now
                .iter()
                .any(|(other, rect)| other == id && same(*rect, *was))),
            "window 2 moved back is not the grid again: {start:?} and now {now:?}"
        );
        assert!(
            same(desk.rect(5), five),
            "window 5 did not take its tile back"
        );
        assert_eq!(desk.workspace_of(2), "1");

        let mut desk = Desk::new("", two_screens());
        desk.press("super+t");
        desk.open(1, "DP-1", (1280.0, 720.0));
        desk.open(2, "DP-1", (2000.0, 720.0));
        desk.press("super+shift+right");
        let whole = |area: Rect| Rect {
            x: area.x + 12.0,
            y: area.y + 12.0,
            w: area.w - 24.0,
            h: area.h - 24.0,
        };
        assert!(
            same(desk.rect(2), whole(TALL)) && same(desk.rect(1), whole(WIDE)),
            "a window moved onto a monitor with nothing on it takes the screen, and the \
             one it left takes the other: {:?}",
            desk.rects()
        );
    }

    /// **With `tiling.move = "split"`, a move goes into the neighbour's split
    /// instead** (#150): window 4 moved left leaves the bottom-right tile to
    /// window 2 and splits window 3's, on the far side, where a swap would
    /// have traded the two. And a value that is neither is named once, and
    /// swaps.
    #[test]
    fn with_tiling_move_split_a_move_goes_into_the_neighbours_split() {
        let mut desk = grid("require(\"config\").tiling.move = \"split\"", one_screen());
        let three = desk.rect(3);
        desk.focus(4);
        desk.press("super+shift+left");
        let (two, now_three, four) = (desk.rect(2), desk.rect(3), desk.rect(4));
        assert!(
            two.h > WIDE.h - 40.0,
            "window 2 did not take the right-hand column: {:?}",
            desk.rects()
        );
        assert!(
            inside(four, three) && inside(now_three, three) && four.x + four.w < now_three.x,
            "window 4 did not go into window 3's tile on its left: {:?}",
            desk.rects()
        );

        let logged = "_G.logged = {}\n\
                      local log = sol.log\n\
                      sol.log = function(message) _G.logged[#_G.logged + 1] = message log(message) end\n\
                      require(\"config\").tiling.move = \"sideways\"";
        let mut desk = grid(logged, one_screen());
        let (three, four) = (desk.rect(3), desk.rect(4));
        desk.focus(4);
        desk.press("super+shift+left");
        desk.press("super+shift+right");
        desk.press("super+shift+left");
        assert!(
            same(desk.rect(4), three) && same(desk.rect(3), four),
            "an unknown tiling.move did not swap: {:?}",
            desk.rects()
        );
        let said = desk
            .scripts
            .evaluate("return table.concat(_G.logged, \"\\n\")");
        assert_eq!(
            said.matches("tiling.move is sideways").count(),
            1,
            "an unknown tiling.move is named once, not per move: {said:?}"
        );
    }

    /// The whole of `area` but the shipped gap round it: the tile a window
    /// alone on a screen is given.
    fn whole(area: Rect) -> Rect {
        Rect {
            x: area.x + 12.0,
            y: area.y + 12.0,
            w: area.w - 24.0,
            h: area.h - 24.0,
        }
    }

    /// Whether `left` and `right` share any pixel.
    fn overlap(left: Rect, right: Rect) -> bool {
        left.x < right.x + right.w
            && right.x < left.x + left.w
            && left.y < right.y + right.h
            && right.y < left.y + left.h
    }

    /// **A window sent to another workspace leaves one grid closed up and is a
    /// tile of the other** (#400), on one monitor and on two.
    ///
    /// Window 4 sent from the grid to workspace 2: window 2 takes the right
    /// column, and 4 is the whole of workspace 2, a leaf of that desk's tree
    /// and of no other. Window 3 sent after it shares workspace 2 with it.
    /// With a second monitor, a send from it leaves the first's grid alone
    /// and its own desk closes up the same way.
    #[test]
    fn a_window_sent_to_another_workspace_leaves_one_grid_closed_up_and_is_a_tile_of_the_other() {
        let mut desk = grid("", one_screen());
        desk.focus(4);
        desk.press("super+shift+2");
        assert_eq!(desk.workspace_of(4), "2");
        assert_eq!(desk.holding("tiling", "trees", 4), "2@DP-1");
        let (one, two) = (desk.rect(1), desk.rect(2));
        assert!(
            about(two.y, one.y) && two.h > WIDE.h - 40.0,
            "workspace 1's grid did not close up over window 4's tile: {:?}",
            desk.rects()
        );
        assert!(
            same(desk.rect(4), whole(WIDE)),
            "window 4 is not the whole of workspace 2: {:?}",
            desk.rects()
        );

        desk.focus(3);
        desk.press("super+shift+2");
        let (three, four) = (desk.rect(3), desk.rect(4));
        assert_eq!(desk.holding("tiling", "trees", 3), "2@DP-1");
        assert!(
            inside(three, WIDE)
                && inside(four, WIDE)
                && !overlap(three, four)
                && three.w * three.h + four.w * four.h > 0.9 * WIDE.w * WIDE.h,
            "windows 3 and 4 do not share workspace 2 between them: {:?}",
            desk.rects()
        );
        assert!(
            desk.rect(1).h > WIDE.h - 40.0,
            "workspace 1's grid did not close up over window 3's tile: {:?}",
            desk.rects()
        );

        let mut desk = grid("", two_screens());
        desk.open(5, "DP-2", (3280.0, 1280.0));
        desk.open(6, "DP-2", (3280.0, 2400.0));
        let start: Vec<Rect> = [1, 2, 3, 4].map(|id| desk.rect(id)).to_vec();
        desk.focus(6);
        desk.press("super+shift+3");
        assert_eq!(desk.workspace_of(6), "3");
        assert_eq!(desk.holding("tiling", "trees", 6), "3@DP-2");
        assert_eq!(
            desk.monitor(6),
            "DP-2",
            "a send keeps the window on its monitor"
        );
        assert!(
            same(desk.rect(5), whole(TALL)) && same(desk.rect(6), whole(TALL)),
            "DP-2's desk did not close up, or window 6 is not the whole of workspace 3: {:?}",
            desk.rects()
        );
        assert!(
            [1, 2, 3, 4]
                .iter()
                .zip(&start)
                .all(|(id, was)| same(desk.rect(*id), *was)),
            "a send on DP-2 moved DP-1's grid: {:?}",
            desk.rects()
        );
    }

    /// **A window dropped on another workspace belongs to it, and its old
    /// grid closes up** (#401): across monitors showing different
    /// workspaces, and on one monitor switched to another workspace mid-drag.
    /// Either way the workspace it belongs to is the one whose tree holds it.
    #[test]
    fn a_window_dropped_on_another_workspace_belongs_to_it_and_leaves_its_old_grid_closed_up() {
        let mut desk = grid("", two_screens());
        assert_eq!(
            desk.scripts
                .evaluate("require(\"workspaces\").showing[\"DP-2\"] = 3 return \"\""),
            ""
        );
        desk.drop(2, "DP-2", (3280.0, 1280.0));
        assert_eq!(desk.workspace_of(2), "3");
        assert_eq!(desk.holding("tiling", "trees", 2), "3@DP-2");
        assert!(
            same(desk.rect(2), whole(TALL)),
            "window 2 is not the whole of the desk it was dropped on: {:?}",
            desk.rects()
        );
        let four = desk.rect(4);
        assert!(
            about(four.y, whole(WIDE).y) && four.h > WIDE.h - 40.0,
            "the grid did not close up over window 2's tile: {:?}",
            desk.rects()
        );

        let mut desk = grid("", one_screen());
        let three = desk.rect(3);
        desk.press("super+2");
        desk.drop(1, "DP-1", (900.0, 500.0));
        assert_eq!(desk.workspace_of(1), "2");
        assert_eq!(desk.holding("tiling", "trees", 1), "2@DP-1");
        assert!(
            same(desk.rect(1), whole(WIDE)),
            "window 1 is not the whole of the workspace it was dropped on: {:?}",
            desk.rects()
        );
        let now = desk.rect(3);
        assert!(
            about(now.x, three.x) && about(now.y, whole(WIDE).y) && now.h > WIDE.h - 40.0,
            "workspace 1's grid, out of view, did not close up over window 1's tile: {:?}",
            desk.rects()
        );
    }

    /// **A window sent from a scrolling strip leaves it for the other
    /// workspace's** (#400): a column of its own there, and in no other strip.
    #[test]
    fn a_window_sent_from_a_scrolling_strip_leaves_it_for_the_other_workspaces() {
        let mut desk = Desk::new("", one_screen());
        desk.press("super+s");
        desk.open(1, "DP-1", (1280.0, 720.0));
        desk.open(2, "DP-1", (1280.0, 720.0));
        desk.focus(2);
        desk.press("super+shift+2");
        assert_eq!(desk.workspace_of(2), "2");
        assert_eq!(desk.holding("scrolling", "views", 2), "2@DP-1");
        assert_eq!(desk.holding("scrolling", "views", 1), "1@DP-1");
    }

    /// **A floating window dropped on the other monitor's workspace belongs
    /// to it** (#401): with no layout in charge the drop moves it all the
    /// same, so it is shown where it was let go.
    #[test]
    fn a_floating_window_dropped_on_the_other_monitors_workspace_belongs_to_it() {
        let mut desk = Desk::floating(
            two_screens(),
            &[(
                1,
                "DP-1",
                Rect {
                    x: 100.0,
                    y: 100.0,
                    w: 800.0,
                    h: 600.0,
                },
            )],
        );
        assert_eq!(
            desk.scripts.evaluate(
                "local workspaces = require(\"workspaces\")\n\
                 workspaces.of[1] = 1\n\
                 workspaces.showing[\"DP-2\"] = 3\n\
                 return \"\""
            ),
            ""
        );
        desk.drop(1, "DP-2", (3280.0, 1280.0));
        assert_eq!(desk.workspace_of(1), "3");
    }

    /// **With no layout in charge, focus and move reach the nearest window
    /// each way.** #150.
    ///
    /// Window 1 in the middle, one window off each side of it. From 1, each
    /// focus key finds that one. From window 4, above, nothing is level with
    /// it on the left, and the nearest centre that way is window 2's. From 2
    /// going right the nearer facing edge wins, 1's over 3's. A move trades
    /// places with the window focus would go to, each keeping its own size,
    /// and the opposite move puts both back. At the monitor's edge nothing
    /// happens. Nor is a window being closed one to go to.
    #[test]
    fn in_floating_focus_and_move_reach_the_nearest_window_each_way() {
        let at = |x: f64, y: f64, w: f64, h: f64| Rect { x, y, w, h };
        let mut desk = Desk::floating(
            one_screen(),
            &[
                (1, "DP-1", at(1000.0, 500.0, 500.0, 400.0)),
                (2, "DP-1", at(200.0, 550.0, 400.0, 300.0)),
                (3, "DP-1", at(1800.0, 450.0, 500.0, 400.0)),
                (4, "DP-1", at(1050.0, 50.0, 400.0, 300.0)),
                (5, "DP-1", at(950.0, 1000.0, 600.0, 300.0)),
            ],
        );
        // h, j, k and l rather than the plain arrows: floating is #222's
        // desktop mode, and the arrows snap there now (`floating.lua`). h, j,
        // k and l still focus by direction, exactly as before.
        for (from, key, to) in [
            (1, "super+h", 2),
            (1, "super+l", 3),
            (1, "super+k", 4),
            (1, "super+j", 5),
            (4, "super+h", 2),
            (2, "super+l", 1),
            (5, "super+k", 1),
            (4, "super+j", 1),
        ] {
            desk.focus(from);
            assert_eq!(focuses(&desk.press(key)), [to], "{key} from window {from}");
        }

        let start = desk.rects();
        for (key, with, back) in [
            ("super+shift+right", 3, "super+shift+h"),
            ("super+shift+down", 5, "super+alt+k"),
            ("super+shift+left", 2, "super+shift+l"),
            ("super+shift+up", 4, "super+shift+j"),
        ] {
            desk.focus(1);
            let (was, other) = (desk.rect(1), desk.rect(with));
            let commands = desk.press(key);
            let (one, them) = (desk.rect(1), desk.rect(with));
            assert!(
                about(one.x, other.x)
                    && about(one.y, other.y)
                    && about(one.w, was.w)
                    && about(them.x, was.x)
                    && about(them.y, was.y)
                    && about(them.w, other.w),
                "{key} did not trade places with window {with}: {:?}",
                desk.rects()
            );
            assert!(
                commands.iter().all(|command| match command {
                    Command::Place { tile, .. } => !tile,
                    _ => true,
                }),
                "a floating window was placed as a tile: {commands:?}"
            );
            desk.press(back);
            let now = desk.rects();
            assert!(
                start
                    .iter()
                    .zip(&now)
                    .all(|((_, before), (_, after))| same(*before, *after)),
                "{back} after {key} is not the desk it started as: {now:?}"
            );
        }

        // A window being closed is fading where it stood, and is not one to
        // go to.
        for window in &mut desk.windows {
            window.leaving = window.id == 2;
        }
        desk.focus(1);
        assert!(
            focuses(&desk.press("super+h")).is_empty(),
            "focus went to a window being closed"
        );
        for window in &mut desk.windows {
            window.leaving = false;
        }

        desk.focus(3);
        let commands = desk.press("super+l");
        assert!(
            focuses(&commands).is_empty(),
            "nothing is right of window 3"
        );
        let commands = desk.press("super+shift+right");
        assert!(
            places(&commands).is_empty(),
            "nothing is right of window 3 and no monitor is either: {commands:?}"
        );
    }

    /// **At a monitor's edge, floating goes on to the next monitor.** #150.
    ///
    /// Focus crosses both ways. A move takes the window across, keeping its
    /// size, as far across the portrait screen as it was across the
    /// landscape one: a half and a quarter of the way there is a half and a
    /// quarter of the way here -- and kept on it, where that would not be.
    #[test]
    fn at_a_monitors_edge_floating_crosses_to_the_next_monitor() {
        let at = |x: f64, y: f64, w: f64, h: f64| Rect { x, y, w, h };
        let mut desk = Desk::floating(
            two_screens(),
            &[
                (1, "DP-1", at(1280.0, 360.0, 400.0, 300.0)),
                (2, "DP-2", at(2800.0, 300.0, 600.0, 500.0)),
            ],
        );
        // l and h, not the plain arrows: floating is #222's desktop mode, and
        // the arrows snap there now (`floating.lua`). l and h still focus by
        // direction, and `super+shift+right` below is still a move, which
        // the arrows keep meaning everywhere.
        desk.focus(1);
        assert_eq!(focuses(&desk.press("super+l")), [2]);
        assert_eq!(focuses(&desk.press("super+h")), [1]);

        desk.press("super+shift+right");
        assert_eq!(
            desk.monitor(1),
            "DP-2",
            "window 1 did not cross: {:?}",
            desk.rects()
        );
        assert!(
            same(desk.rect(1), at(2560.0 + 720.0, 640.0, 400.0, 300.0)),
            "window 1 is not as far across DP-2 as it was across DP-1: {:?}",
            desk.rects()
        );
        assert!(
            same(desk.rect(2), at(2800.0, 300.0, 600.0, 500.0)),
            "window 2 was not part of the move"
        );

        // As far across would put window 3 past DP-2's right edge, so it is
        // kept on the screen, against that edge.
        desk.windows
            .push(window(3, "DP-1", at(2100.0, 900.0, 400.0, 300.0)));
        desk.focus(3);
        desk.press("super+shift+right");
        assert!(
            same(desk.rect(3), at(4000.0 - 400.0, 1600.0, 400.0, 300.0)),
            "window 3 was not kept on DP-2: {:?}",
            desk.rects()
        );
    }

    /// **The next monitor that way is one wholly past this one's edge.**
    /// #150's review.
    ///
    /// The portrait screen stands to the right of the landscape one and
    /// reaches below it, and was taken for the one below it: `super+down`
    /// from the grid's bottom row focused the portrait screen's lower window,
    /// and `super+shift+down` moved a window onto it. Now down from the
    /// bottom row, and up from the portrait screen's top, is nothing at all.
    /// The same for a smaller screen top-aligned beside a bigger one, the
    /// usual laptop and monitor, where up from the big one and down from the
    /// small one each reached the other -- while right and left still cross.
    /// And a screen touching this one only at a corner is past its edge both
    /// ways, so right and down each reach it.
    #[test]
    fn at_a_monitors_edge_the_next_monitor_is_one_wholly_that_way() {
        let mut desk = grid("", two_screens());
        desk.open(5, "DP-2", (3280.0, 1280.0));
        desk.open(6, "DP-2", (3280.0, 2400.0));
        let (five, six) = (desk.rect(5), desk.rect(6));
        assert!(
            five.y + five.h < six.y && inside(five, TALL) && inside(six, TALL),
            "the premise: 5 above 6 on the portrait screen: {:?}",
            desk.rects()
        );
        let start = desk.rects();
        for (from, key) in [
            (3, "super+down"),
            (4, "super+down"),
            (5, "super+up"),
            (3, "super+j"),
            (5, "super+k"),
        ] {
            desk.focus(from);
            assert!(
                focuses(&desk.press(key)).is_empty(),
                "{key} from window {from} went to the monitor beside it"
            );
        }
        for (from, key) in [
            (3, "super+shift+down"),
            (4, "super+shift+down"),
            (5, "super+shift+up"),
        ] {
            desk.focus(from);
            let commands = desk.press(key);
            assert!(
                places(&commands).is_empty(),
                "{key} from window {from} moved something onto the monitor beside it: {commands:?}"
            );
        }
        let now = desk.rects();
        assert!(
            start
                .iter()
                .zip(&now)
                .all(|((_, before), (_, after))| same(*before, *after)),
            "the desk changed: {start:?} and now {now:?}"
        );

        let at = |x: f64, y: f64, w: f64, h: f64| Rect { x, y, w, h };
        let mut desk = Desk::floating(
            vec![screen("DP-1", WIDE), screen("DP-2", BESIDE)],
            &[
                (1, "DP-1", at(1000.0, 1000.0, 400.0, 300.0)),
                (2, "DP-2", at(3000.0, 300.0, 600.0, 400.0)),
            ],
        );
        // k, j, l and h, not the plain arrows: floating is #222's desktop
        // mode, and the arrows snap there now (`floating.lua`). k, j, l and h
        // still focus by direction, and the shift combos below are still a
        // move, which the arrows keep meaning everywhere.
        desk.focus(1);
        assert!(
            focuses(&desk.press("super+k")).is_empty(),
            "up from the big screen reached the small one beside it"
        );
        assert!(
            places(&desk.press("super+shift+up")).is_empty(),
            "a move up from the big screen went onto the small one beside it"
        );
        desk.focus(2);
        assert!(
            focuses(&desk.press("super+j")).is_empty(),
            "down from the small screen reached the big one beside it"
        );
        assert!(
            places(&desk.press("super+shift+down")).is_empty(),
            "a move down from the small screen went onto the big one beside it"
        );
        desk.focus(1);
        assert_eq!(focuses(&desk.press("super+l")), [2]);
        assert_eq!(focuses(&desk.press("super+h")), [1]);

        let mut desk = Desk::floating(
            vec![screen("DP-1", WIDE), screen("DP-3", CORNER)],
            &[
                (1, "DP-1", at(1000.0, 500.0, 400.0, 300.0)),
                (3, "DP-3", at(3000.0, 1700.0, 400.0, 300.0)),
            ],
        );
        for (from, key, to) in [
            (1, "super+j", 3),
            (1, "super+l", 3),
            (3, "super+k", 1),
            (3, "super+h", 1),
        ] {
            desk.focus(from);
            assert_eq!(
                focuses(&desk.press(key)),
                [to],
                "{key} from window {from}, to the screen off on a corner"
            );
        }
    }

    /// **In tiling, a tile off on a diagonal is not that way.** #150's review.
    ///
    /// On the portrait screen, window 1 above 2 and 3 side by side, and
    /// window 9 on the smaller screen to its left. Left of window 1 was
    /// window 2, below it, by the nearest centre; right of it, window 3.
    /// `super+shift+left` then traded 1 with 2 and `super+shift+right` 1 with
    /// 3, leaving a desk the opposite key did not put back. Now left of 1 is
    /// the next monitor, window 9, and right of it nothing: the move right
    /// changes nothing, and the move left takes 1 across. Between tiles that
    /// are level a move trades them, and the opposite move puts both back.
    /// And from window 2, lower than anything on the screen to its left, left
    /// is still window 9: on the next monitor, the nearest centre counts.
    #[test]
    fn in_tiling_a_tile_off_on_a_diagonal_is_not_that_way() {
        let mut desk = Desk::new("", vec![screen("DP-1", LEFT_OF_TALL), screen("DP-2", TALL)]);
        desk.press("super+t");
        desk.open(9, "DP-1", (1600.0, 540.0));
        desk.open(1, "DP-2", (3280.0, 640.0));
        desk.open(2, "DP-2", (3000.0, 2000.0));
        desk.open(3, "DP-2", (3800.0, 2000.0));
        let [one, two, three] = [1, 2, 3].map(|id| desk.rect(id));
        assert!(
            one.y + one.h < two.y
                && about(two.y, three.y)
                && two.x + two.w < three.x
                && inside(one, TALL)
                && inside(two, TALL)
                && inside(three, TALL)
                && inside(desk.rect(9), LEFT_OF_TALL),
            "the premise: 1 above 2 | 3 on the portrait screen, 9 on the other: {:?}",
            desk.rects()
        );

        for (from, key, to) in [
            (1, "super+left", Some(9)),
            (1, "super+h", Some(9)),
            (1, "super+right", None),
            (1, "super+l", None),
            (2, "super+up", Some(1)),
            (3, "super+up", Some(1)),
            (2, "super+right", Some(3)),
            (3, "super+left", Some(2)),
            (2, "super+left", Some(9)),
        ] {
            desk.focus(from);
            assert_eq!(
                focuses(&desk.press(key)),
                to.into_iter().collect::<Vec<_>>(),
                "{key} from window {from}"
            );
        }

        let start = desk.rects();
        desk.focus(1);
        let commands = desk.press("super+shift+right");
        assert!(
            places(&commands).is_empty(),
            "window 1 moved right, with nothing level with it that way: {commands:?}"
        );
        desk.focus(2);
        desk.press("super+shift+right");
        assert!(
            same(desk.rect(2), three) && same(desk.rect(3), two),
            "2 and 3 did not trade tiles: {:?}",
            desk.rects()
        );
        desk.press("super+shift+left");
        let now = desk.rects();
        assert!(
            start
                .iter()
                .zip(&now)
                .all(|((_, before), (_, after))| same(*before, *after)),
            "the opposite move did not put both back: {start:?} and now {now:?}"
        );

        desk.focus(1);
        desk.press("super+shift+left");
        assert_eq!(
            desk.monitor(1),
            "DP-1",
            "window 1 did not cross to the screen on its left: {:?}",
            desk.rects()
        );
        assert!(
            desk.rect(2).h > TALL.h - 40.0 && desk.rect(3).h > TALL.h - 40.0,
            "2 and 3 did not take the portrait screen: {:?}",
            desk.rects()
        );
    }

    /// **Of two monitors that way, the one level with the window is next.**
    /// #150's second review.
    ///
    /// The next monitor was ranked from the rectangle of the monitor the
    /// window is on, so of two screens stacked beside it the same one was
    /// next from every window: right of the big screen's lower tile was the
    /// upper of the two screens beside it, and left of a portrait screen's
    /// upper tile the lower of the two on its left. It is ranked from the
    /// window now, as wlroots ranks from a point in the focused window: in
    /// tiling for focus and for a move, and in scrolling past the end of the
    /// strip.
    #[test]
    fn of_two_monitors_that_way_the_one_level_with_the_window_is_next() {
        let mut desk = Desk::new(
            "",
            vec![
                screen("DP-1", WIDE),
                screen("DP-2", BESIDE),
                screen("DP-3", UNDER_BESIDE),
            ],
        );
        desk.press("super+t");
        desk.open(1, "DP-1", (1280.0, 720.0));
        desk.open(2, "DP-1", (2000.0, 720.0));
        desk.open(3, "DP-1", (1920.0, 1200.0));
        desk.open(5, "DP-2", (3520.0, 540.0));
        desk.open(6, "DP-3", (3520.0, 1620.0));
        // Window 2 taller, so window 3 under it is level with DP-3 alone.
        desk.focus(2);
        for _ in 0..6 {
            desk.press("super+ctrl+equal");
        }
        let (upper, lower) = (desk.rect(2), desk.rect(3));
        assert!(
            about(upper.x, lower.x)
                && upper.y + upper.h < lower.y
                && lower.y > UNDER_BESIDE.y
                && upper.y + upper.h / 2.0 < BESIDE.y + BESIDE.h
                && inside(desk.rect(5), BESIDE)
                && inside(desk.rect(6), UNDER_BESIDE),
            "the premise: 2 above 3 on DP-1's right, 3 level with DP-3 alone, \
             5 on DP-2 and 6 on DP-3: {:?}",
            desk.rects()
        );
        for (from, key, to) in [
            (2, "super+right", 5),
            (2, "super+l", 5),
            (3, "super+right", 6),
            (3, "super+l", 6),
        ] {
            desk.focus(from);
            assert_eq!(focuses(&desk.press(key)), [to], "{key} from window {from}");
        }
        desk.focus(3);
        desk.press("super+shift+right");
        assert_eq!(
            desk.monitor(3),
            "DP-3",
            "super+shift+right from window 3 took it to the other screen: {:?}",
            desk.rects()
        );

        let stacked_left = || {
            vec![
                screen("DP-1", PORTRAIT),
                screen("DP-2", ABOVE_LEFT),
                screen("DP-3", BELOW_LEFT),
            ]
        };
        let mut desk = Desk::new("", stacked_left());
        desk.press("super+t");
        desk.open(7, "DP-2", (960.0, 540.0));
        desk.open(8, "DP-3", (960.0, 1620.0));
        desk.open(1, "DP-1", (2640.0, 640.0));
        desk.open(2, "DP-1", (2640.0, 2000.0));
        let (one, two) = (desk.rect(1), desk.rect(2));
        assert!(
            one.y + one.h < two.y
                && inside(one, PORTRAIT)
                && inside(two, PORTRAIT)
                && one.y + one.h / 2.0 < ABOVE_LEFT.y + ABOVE_LEFT.h
                && two.y > BELOW_LEFT.y,
            "the premise: 1 above 2 on the portrait screen, 2 level with DP-3 alone: {:?}",
            desk.rects()
        );
        for (from, key, to) in [
            (1, "super+left", 7),
            (1, "super+h", 7),
            (2, "super+left", 8),
            (2, "super+h", 8),
        ] {
            desk.focus(from);
            assert_eq!(focuses(&desk.press(key)), [to], "{key} from window {from}");
        }
        desk.focus(1);
        desk.press("super+shift+left");
        assert_eq!(
            desk.monitor(1),
            "DP-2",
            "super+shift+left from window 1 took it to the lower screen: {:?}",
            desk.rects()
        );

        let mut desk = Desk::new("", stacked_left());
        for (id, monitor, area) in [
            (7, "DP-2", ABOVE_LEFT),
            (8, "DP-3", BELOW_LEFT),
            (1, "DP-1", PORTRAIT),
            (2, "DP-1", PORTRAIT),
        ] {
            desk.windows.push(window(
                id,
                monitor,
                Rect {
                    w: 400.0,
                    h: 300.0,
                    ..area
                },
            ));
        }
        desk.press("super+s");
        // Window 2 stacked under window 1: `super+comma` from column 1, on
        // DP-1 because the strip's own keys act on the monitor in front.
        desk.focus(2);
        desk.press("super+h");
        desk.press("super+comma");
        let (one, two) = (desk.rect(1), desk.rect(2));
        assert!(
            about(one.x, two.x)
                && one.y + one.h < two.y
                && inside(one, PORTRAIT)
                && inside(two, PORTRAIT)
                && one.y + one.h / 2.0 < ABOVE_LEFT.y + ABOVE_LEFT.h
                && two.y > BELOW_LEFT.y,
            "the premise: scrolling, 1 above 2 in the portrait screen's one column: {:?}",
            desk.rects()
        );
        for (from, key, to) in [(1, "super+left", 7), (2, "super+left", 8)] {
            desk.focus(from);
            assert_eq!(
                focuses(&desk.press(key)),
                [to],
                "scrolling: {key} from window {from}"
            );
        }
    }

    /// **`direction.beside` with no window, or one whose centre is on no
    /// screen, ranks from the monitor.** #150's third review.
    ///
    /// Round two made the window its third argument, and a mode of your own
    /// calling it with the two it had before raised an error. Now, left of
    /// the portrait screen with two stacked on its left, the one level with
    /// the portrait screen's centre is next with no window, and with a window
    /// whose centre is past both screens that way, which has nothing further
    /// that way to rank -- where before that was no monitor at all. With a
    /// window near the top it is still the upper screen.
    #[test]
    fn direction_beside_with_no_window_or_one_on_no_screen_ranks_from_the_monitor() {
        let desk = Desk::new(
            "",
            vec![
                screen("DP-1", PORTRAIT),
                screen("DP-2", ABOVE_LEFT),
                screen("DP-3", BELOW_LEFT),
            ],
        );
        for (from, next) in [
            ("", "DP-3"),
            (", { x = -5000, y = 0, w = 100, h = 100 }", "DP-3"),
            (", { x = 2000, y = 100, w = 800, h = 600 }", "DP-2"),
        ] {
            let answer = desk.scripts.evaluate_in(
                desk.snapshot(),
                &format!(
                    "local next = require(\"direction\").beside(\"DP-1\", \"left\"{from})\n\
                     return next and next.name or \"no monitor\""
                ),
            );
            assert_eq!(answer, next, "left of DP-1{from}");
        }
    }

    /// **A window that belongs to no workspace belongs to none after a move
    /// across.** #150's review.
    ///
    /// With `follow_new_windows` off a window is on no workspace in
    /// particular, so on every one, and a move onto the next monitor pinned
    /// it to that monitor's: in tiling, in scrolling and with no layout.
    #[test]
    fn a_window_on_no_workspace_is_on_none_after_crossing() {
        let off = "require(\"config\").workspaces.follow_new_windows = false";

        let mut desk = Desk::new(off, two_screens());
        desk.press("super+t");
        desk.open(1, "DP-1", (1280.0, 720.0));
        desk.open(2, "DP-1", (2000.0, 720.0));
        assert_eq!(desk.workspace_of(2), "nil", "the premise");
        desk.press("super+shift+right");
        assert_eq!(desk.monitor(2), "DP-2", "tiling: window 2 did not cross");
        assert_eq!(desk.workspace_of(2), "nil", "tiling");

        let mut desk = Desk::new(off, two_screens());
        for id in [1, 2] {
            desk.windows.push(window(
                id,
                "DP-1",
                Rect {
                    w: 400.0,
                    h: 300.0,
                    ..WIDE
                },
            ));
        }
        desk.press("super+s");
        desk.focus(2);
        desk.press("super+shift+right");
        assert_eq!(desk.monitor(2), "DP-2", "scrolling: window 2 did not cross");
        assert_eq!(desk.workspace_of(2), "nil", "scrolling");

        let mut desk = Desk::new(off, two_screens());
        desk.windows.push(window(
            1,
            "DP-1",
            Rect {
                x: 1280.0,
                y: 360.0,
                w: 400.0,
                h: 300.0,
            },
        ));
        desk.focus(1);
        desk.press("super+shift+right");
        assert_eq!(desk.monitor(1), "DP-2", "floating: window 1 did not cross");
        assert_eq!(desk.workspace_of(1), "nil", "floating");
    }

    /// **A window moved by key onto the other monitor is declared on the
    /// workspace that monitor shows**, by the press that moves it, in
    /// tiling, in scrolling and with no layout: nothing runs `layout` after
    /// a move, so without it a hosted shell would count the window on the
    /// workspace it left.
    #[test]
    fn a_window_moved_onto_the_other_monitor_is_declared_on_the_workspace_it_shows() {
        let declared = |commands: &[Command]| {
            commands.iter().rev().find_map(|command| match command {
                Command::Workspaces(declared) => Some(declared.windows.get(&2).cloned()),
                _ => None,
            })
        };
        let mut moved = Vec::new();
        for layout in ["super+t", "super+s", ""] {
            let mut desk = Desk::new("", two_screens());
            assert_eq!(
                desk.scripts
                    .evaluate("require(\"workspaces\").showing[\"DP-2\"] = 2 return \"\""),
                ""
            );
            if !layout.is_empty() {
                desk.press(layout);
            }
            desk.open(1, "DP-1", (1280.0, 720.0));
            desk.open(2, "DP-1", (2000.0, 720.0));
            desk.focus(2);
            let commands = desk.press("super+shift+right");
            moved.push((desk.monitor(2), declared(&commands)));
        }
        let there = ("DP-2".to_owned(), Some(Some(vec!["2".to_owned()])));
        assert_eq!(
            moved,
            vec![there.clone(), there.clone(), there],
            "(window 2's monitor, where the press declared it) in tiling, scrolling and floating"
        );
    }

    /// **A window on no workspace moved by key onto the other monitor is
    /// declared on the workspace that monitor shows**, by the press that moves
    /// it: with `follow_new_windows` off it belongs to whatever its monitor
    /// shows, and that monitor is now the other one.
    #[test]
    fn a_window_on_no_workspace_moved_across_is_declared_on_what_that_monitor_shows() {
        let off = "require(\"config\").workspaces.follow_new_windows = false";
        let declared = |commands: &[Command]| {
            commands.iter().rev().find_map(|command| match command {
                Command::Workspaces(declared) => Some(declared.windows.get(&2).cloned()),
                _ => None,
            })
        };
        let mut moved = Vec::new();
        for layout in ["super+t", "super+s", ""] {
            let mut desk = Desk::new(off, two_screens());
            assert_eq!(
                desk.scripts
                    .evaluate("require(\"workspaces\").showing[\"DP-2\"] = 2 return \"\""),
                ""
            );
            if !layout.is_empty() {
                desk.press(layout);
            }
            desk.open(1, "DP-1", (1280.0, 720.0));
            desk.open(2, "DP-1", (2000.0, 720.0));
            desk.focus(2);
            assert_eq!(desk.workspace_of(2), "nil", "the premise");
            let commands = desk.press("super+shift+right");
            moved.push((desk.monitor(2), desk.workspace_of(2), declared(&commands)));
        }
        let there = (
            "DP-2".to_owned(),
            "nil".to_owned(),
            Some(Some(vec!["2".to_owned()])),
        );
        assert_eq!(
            moved,
            vec![there.clone(), there.clone(), there],
            "(window 2's monitor, its `of`, where the press declared it) in tiling, scrolling and floating"
        );
    }

    /// **In scrolling, the directions are the strip's own keys.** #150.
    ///
    /// Left and right are the columns and up and down the windows in one,
    /// for focus and for a move; past the end of the strip, focus goes to the
    /// window the next monitor's strip has focused, and a move takes the
    /// window into a column of its own there.
    #[test]
    fn in_scrolling_the_directions_are_the_strips_own_keys() {
        let mut desk = Desk::new("", two_screens());
        for id in [1, 2, 3, 5] {
            let monitor = if id == 5 { "DP-2" } else { "DP-1" };
            desk.windows.push(window(
                id,
                monitor,
                Rect {
                    w: 400.0,
                    h: 300.0,
                    ..if id == 5 { TALL } else { WIDE }
                },
            ));
        }
        desk.press("super+s");
        let x = |desk: &Desk, id| desk.rect(id).x;
        assert!(
            x(&desk, 1) < x(&desk, 2) && x(&desk, 2) < x(&desk, 3) && desk.monitor(3) == "DP-1",
            "the premise: columns 1, 2, 3 on DP-1: {:?}",
            desk.rects()
        );

        desk.focus(2);
        assert_eq!(focuses(&desk.press("super+left")), [1]);
        desk.focus(2);
        assert_eq!(focuses(&desk.press("super+l")), [3]);

        // Window 2 stacked under window 1: `super+comma` from column 1.
        desk.focus(2);
        desk.press("super+h");
        desk.press("super+comma");
        let (one, two) = (desk.rect(1), desk.rect(2));
        assert!(
            about(one.x, two.x) && one.y < two.y,
            "the premise: 1 above 2 in one column: {:?}",
            desk.rects()
        );
        desk.focus(2);
        assert_eq!(focuses(&desk.press("super+up")), [1]);
        assert_eq!(focuses(&desk.press("super+j")), [2]);

        desk.press("super+alt+k");
        assert!(
            desk.rect(2).y < desk.rect(1).y,
            "super+alt+k did not move window 2 above window 1: {:?}",
            desk.rects()
        );
        desk.press("super+shift+down");
        assert!(desk.rect(1).y < desk.rect(2).y, "and back down");

        desk.press("super+shift+right");
        assert!(
            x(&desk, 3) < x(&desk, 2) && about(x(&desk, 1), x(&desk, 2)),
            "the column did not move right past window 3's: {:?}",
            desk.rects()
        );
        desk.press("super+shift+h");
        assert!(x(&desk, 2) < x(&desk, 3), "and back left");

        desk.focus(3);
        assert_eq!(
            focuses(&desk.press("super+right")),
            [5],
            "past the end of the strip, the next monitor's"
        );
        desk.focus(3);
        desk.press("super+shift+right");
        assert_eq!(
            desk.monitor(3),
            "DP-2",
            "window 3 did not cross: {:?}",
            desk.rects()
        );
        assert!(
            inside(desk.rect(3), TALL) && x(&desk, 5) < x(&desk, 3),
            "window 3 is not in a column of its own beside window 5: {:?}",
            desk.rects()
        );
        assert_eq!(
            desk.workspace_of(3),
            "nil",
            "window 3 was on no workspace, and is on none there either"
        );
    }

    /// **`super+shift+space` floats a tiled window, and tiles it again.**
    /// #150.
    ///
    /// Window 2 leaves its tile, window 1 takes the screen, and 2 is centred
    /// on it at the size it had, not as a tile; pressed again, the two share
    /// the screen. With no layout in charge the key changes nothing, now or
    /// when tiling is turned on afterwards.
    #[test]
    fn super_shift_space_floats_a_tiled_window_and_tiles_it_again() {
        let mut desk = Desk::new("", one_screen());
        desk.press("super+t");
        desk.open(1, "DP-1", (1280.0, 720.0));
        desk.open(2, "DP-1", (2000.0, 720.0));
        let (one, two) = (desk.rect(1), desk.rect(2));

        let commands = desk.press("super+shift+space");
        let full = Rect {
            x: 12.0,
            y: 12.0,
            w: 2536.0,
            h: 1416.0,
        };
        let floated = desk.rect(2);
        assert!(
            same(desk.rect(1), full),
            "window 1 did not take the screen: {:?}",
            desk.rects()
        );
        assert!(
            about(floated.w, two.w)
                && about(floated.x + floated.w / 2.0, 1280.0)
                && about(floated.y + floated.h / 2.0, 720.0),
            "window 2 is not centred at the size it had: {floated:?}"
        );
        assert!(
            commands.iter().any(|command| matches!(
                command,
                Command::Place {
                    id: 2,
                    tile: false,
                    ..
                }
            )),
            "window 2 was placed as a tile: {commands:?}"
        );

        desk.press("super+shift+space");
        assert!(
            same(desk.rect(1), one) && same(desk.rect(2), two),
            "pressed again, the two did not share the screen: {:?}",
            desk.rects()
        );

        let mut desk = Desk::new("", one_screen());
        desk.press("super+t");
        desk.open(1, "DP-1", (1280.0, 720.0));
        desk.open(2, "DP-1", (2000.0, 720.0));
        let (one, two) = (desk.rect(1), desk.rect(2));
        desk.press("super+t");
        let commands = desk.press("super+shift+space");
        assert!(
            places(&commands).is_empty(),
            "floating mode placed something: {commands:?}"
        );
        desk.press("super+t");
        assert!(
            same(desk.rect(1), one) && same(desk.rect(2), two),
            "super+shift+space in floating mode floated window 2 for later: {:?}",
            desk.rects()
        );
    }

    /// **A window the layout does not arrange gets the floating answer.**
    /// #150.
    ///
    /// Window 3 floated over tiling's `1 | 2`, and over scrolling's strip:
    /// focus from it finds the nearest window that way among everything on
    /// screen, and a move does nothing -- trading places with a tile would
    /// hand the tile a place its layout takes straight back. Closed, the
    /// window is forgotten as floated.
    #[test]
    fn a_window_the_layout_does_not_arrange_gets_the_floating_answer() {
        for (layout, key) in [("tiling", "super+t"), ("scrolling", "super+s")] {
            let mut desk = Desk::new("", one_screen());
            desk.press(key);
            desk.open(1, "DP-1", (1280.0, 720.0));
            desk.open(2, "DP-1", (2000.0, 720.0));
            desk.open(3, "DP-1", (2000.0, 1200.0));
            // Over to window 1 and back, so a strip is scrolled to its start
            // rather than to wherever opening window 3 left it.
            desk.press("super+left");
            desk.press("super+left");
            desk.focus(3);
            desk.press("super+shift+space");
            assert_eq!(
                focuses(&desk.press("super+left")),
                [1],
                "{layout}: focus left from the floated window"
            );
            if layout == "tiling" {
                desk.focus(3);
                assert_eq!(
                    focuses(&desk.press("super+right")),
                    [2],
                    "{layout}: focus right from the floated window"
                );
            }
            desk.focus(3);
            let commands = desk.press("super+shift+left");
            assert!(
                places(&commands).is_empty(),
                "{layout}: the floated window moved, or moved something: {commands:?}"
            );
            let _ = desk.scripts.closed(3, desk.snapshot());
            assert_eq!(
                desk.scripts
                    .evaluate("return tostring(require(\"dialogs\").lifted[3])"),
                "nil",
                "{layout}: a closed window is still marked floated"
            );
        }
    }

    /// **A window the user floated stays out through a reload.** #150.
    #[test]
    fn a_window_floated_by_the_user_stays_out_through_a_reload() {
        let mut desk = Desk::new("", one_screen());
        desk.press("super+t");
        desk.open(1, "DP-1", (1280.0, 720.0));
        desk.open(2, "DP-1", (2000.0, 720.0));
        desk.press("super+shift+space");
        let (one, two) = (desk.rect(1), desk.rect(2));
        desk.reload();
        assert!(
            same(desk.rect(1), one) && same(desk.rect(2), two),
            "the reload put the floated window back in a tile: {:?}",
            desk.rects()
        );
        desk.press("super+shift+space");
        assert!(
            desk.rect(1).w < 1400.0,
            "and it can still be tiled again: {:?}",
            desk.rects()
        );
    }

    /// A script of `text` alone, in a directory of its own.
    fn plain(name: &str, text: &str) -> Scripts {
        let directory =
            std::env::temp_dir().join(format!("solium-directions-{name}-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&directory);
        let entry = directory.join("init.lua");
        std::fs::write(&entry, text).expect("writing the test script");
        Scripts::load(&entry).expect("loading the test script")
    }

    /// **`sol.focus_direction` and `sol.move_direction` reach every
    /// `direction` listener with the verb and the direction, and a direction
    /// that is not one of the four is an error.** #150.
    #[test]
    fn sol_direction_reaches_the_listeners_and_a_bad_one_is_an_error() {
        let mut scripts = plain(
            "event",
            r#"
            sol.on("direction", function(verb, dir) sol.status(verb .. " " .. dir) end)
            sol.bind("super+a", function() sol.focus_direction("left") end)
            sol.bind("super+b", function() sol.move_direction("down") end)
            sol.bind("super+c", function()
                local ok, err = pcall(sol.focus_direction, "west")
                sol.status(tostring(ok) .. ": " .. tostring(err))
            end)
            "#,
        );
        let status = |scripts: &mut Scripts, combo: &str| {
            scripts
                .key(combo, Snapshot::default())
                .status
                .unwrap_or_default()
        };
        assert_eq!(status(&mut scripts, "super+a"), "focus left");
        assert_eq!(status(&mut scripts, "super+b"), "move down");
        let refused = status(&mut scripts, "super+c");
        assert!(
            refused.starts_with("false: ") && refused.contains("\"west\" is not a direction"),
            "{refused:?}"
        );
    }

    /// **`sol.toggle_fullscreen` and `sol.toggle_maximize` name the focused
    /// window when given none**, the window given when there is one, and
    /// nothing when there is neither. #150.
    #[test]
    fn sol_toggles_name_the_focused_window_when_given_none() {
        let mut scripts = plain(
            "toggles",
            r#"
            sol.bind("super+a", function() sol.toggle_fullscreen() end)
            sol.bind("super+b", function() sol.toggle_maximize() end)
            sol.bind("super+c", function() sol.toggle_fullscreen(9) end)
            "#,
        );
        let desk = |focused: bool| Snapshot {
            windows: vec![
                window(4, "DP-1", WIDE),
                WindowInfo {
                    icon: None,
                    client_side: false,
                    blur_region: false,
                    insets: crate::decoration::Insets::default(),
                    focused,
                    ..window(3, "DP-1", WIDE)
                },
            ],
            ..Snapshot::default()
        };
        let asked = |commands: Vec<Command>| -> Vec<String> {
            commands
                .iter()
                .map(|command| format!("{command:?}"))
                .collect()
        };
        assert_eq!(
            asked(scripts.key("super+a", desk(true)).commands),
            [format!("{:?}", Command::ToggleFullscreen { id: 3 })]
        );
        assert_eq!(
            asked(scripts.key("super+b", desk(true)).commands),
            [format!("{:?}", Command::ToggleMaximize { id: 3 })]
        );
        assert_eq!(
            asked(scripts.key("super+c", desk(true)).commands),
            [format!("{:?}", Command::ToggleFullscreen { id: 9 })]
        );
        assert!(scripts.key("super+a", desk(false)).commands.is_empty());
        assert!(scripts.key("super+b", desk(false)).commands.is_empty());
    }

    /// **`sol.press` names a button or a key, and refuses what it cannot
    /// reach**: a button by its name and a key as `sol.bind` spells it, on
    /// the window named or the focused one, are pressed; a minimized, leaving
    /// or unknown window, and a spec that names no button or key, or both,
    /// or something else, are errors and ask nothing. With nothing focused
    /// and no id, nothing. And it reaches past the lock, so a binding allowed
    /// there cannot ask for it.
    #[test]
    fn sol_press_names_a_button_or_a_key_and_refuses_what_it_cannot_reach() {
        use smithay::input::keyboard::Keysym;
        let mut scripts = plain(
            "press",
            r#"
            said = {}
            local function try(...)
                local ok, err = pcall(sol.press, ...)
                said[#said + 1] = ok and "ok" or tostring(err):match("[^\n]*")
            end
            sol.bind("super+a", function()
                try(1, { button = "side" })
                try(nil, { key = "alt+Left" })
                try(1, { key = "Escape" })
                try(2, { button = "left" })
                try(3, { key = "Escape" })
                try(9, { button = "left" })
                try(1, { button = "back" })
                try(1, { key = "no_such_key" })
                try(1, { key = "alt+" })
                try(1, { button = "left", key = "Escape" })
                try(1, {})
                try(1, { button = "left", at = 3 })
                try(1, { button = 4 })
            end)
            sol.bind("super+b", function() try(nil, { button = "left" }) end)
            "#,
        );
        let desk = |focused: bool| Snapshot {
            windows: vec![
                WindowInfo {
                    icon: None,
                    client_side: false,
                    blur_region: false,
                    insets: crate::decoration::Insets::default(),
                    focused,
                    ..window(1, "DP-1", WIDE)
                },
                WindowInfo {
                    icon: None,
                    client_side: false,
                    blur_region: false,
                    insets: crate::decoration::Insets::default(),
                    minimized: true,
                    above: false,
                    ..window(2, "DP-1", WIDE)
                },
                WindowInfo {
                    icon: None,
                    client_side: false,
                    blur_region: false,
                    insets: crate::decoration::Insets::default(),
                    leaving: true,
                    ..window(3, "DP-1", WIDE)
                },
            ],
            ..Snapshot::default()
        };
        let press = |press: super::Press| Command::Press {
            id: 1,
            press,
            source: FocusSource::Binding,
        };
        let asked = |commands: Vec<Command>| -> Vec<String> {
            commands
                .iter()
                .map(|command| format!("{command:?}"))
                .collect()
        };
        assert_eq!(
            asked(scripts.key("super+a", desk(true)).commands),
            [
                press(super::Press::Button(0x113)),
                press(super::Press::Keys(vec![Keysym::Alt_L, Keysym::Left])),
                press(super::Press::Keys(vec![Keysym::Escape])),
            ]
            .map(|command| format!("{command:?}"))
        );
        let said = scripts.evaluate("return table.concat(said, \"\\n\")");
        let said: Vec<&str> = said.lines().collect();
        let wanted = [
            "ok",
            "ok",
            "ok",
            "2 is minimized",
            "3 is leaving",
            "9 is not a window",
            "no button is called `back`",
            "`no_such_key` names no key",
            "`alt+` names no key",
            "a button and a key at once",
            "it names nothing",
            "`at` is not in it",
            "a button or a key is a string",
        ];
        assert_eq!(said.len(), wanted.len(), "every call answered: {said:?}");
        for (said, wanted) in said.iter().zip(wanted) {
            assert!(said.contains(wanted), "{said:?} is not {wanted:?}");
        }
        assert!(
            scripts.key("super+b", desk(false)).commands.is_empty(),
            "no id and nothing focused presses nothing"
        );
        assert!(
            press(super::Press::Button(0x110)).reaches_past_the_lock(),
            "a press is allowed to a binding at the lock"
        );
    }

    /// **`sol.minimize` names the focused window when given none**, the
    /// window given when there is one, and nothing when there is neither;
    /// `sol.unminimize` and `sol.show_minimized` name the window given. And
    /// all three, with `sol.act`'s two verbs, reach past the lock, so a
    /// binding allowed there cannot ask for any of them (#30, #224).
    #[test]
    fn sol_minimize_names_the_focused_window_when_given_none_and_reaches_past_the_lock() {
        let mut scripts = plain(
            "minimize",
            r#"
            sol.bind("super+a", function() sol.minimize() end)
            sol.bind("super+b", function() sol.unminimize(9) end)
            sol.bind("super+c", function() sol.show_minimized(9) end)
            sol.bind("super+d", function() sol.show_minimized(9, false) end)
            "#,
        );
        let desk = |focused: bool| Snapshot {
            windows: vec![
                window(4, "DP-1", WIDE),
                WindowInfo {
                    client_side: false,
                    blur_region: false,
                    insets: crate::decoration::Insets::default(),
                    focused,
                    ..window(3, "DP-1", WIDE)
                },
            ],
            ..Snapshot::default()
        };
        let asked = |commands: Vec<Command>| -> Vec<String> {
            commands
                .iter()
                .map(|command| format!("{command:?}"))
                .collect()
        };
        assert_eq!(
            asked(scripts.key("super+a", desk(true)).commands),
            [format!("{:?}", Command::Minimize { id: 3 })]
        );
        assert!(scripts.key("super+a", desk(false)).commands.is_empty());
        assert_eq!(
            asked(scripts.key("super+b", desk(true)).commands),
            [format!("{:?}", Command::Unminimize { id: 9 })]
        );
        assert_eq!(
            asked(scripts.key("super+c", desk(true)).commands),
            [format!(
                "{:?}",
                Command::ShowMinimized { id: 9, shown: true }
            )]
        );
        assert_eq!(
            asked(scripts.key("super+d", desk(true)).commands),
            [format!(
                "{:?}",
                Command::ShowMinimized {
                    id: 9,
                    shown: false
                }
            )]
        );
        let act = |action: &str| Command::Act {
            attempt: 1,
            action: action.to_owned(),
            data: crate::json::Json::Null,
            source: FocusSource::Script,
        };
        for command in [
            Command::Minimize { id: 3 },
            Command::Unminimize { id: 3 },
            Command::ShowMinimized { id: 3, shown: true },
            act("windows.minimize"),
            act("windows.unminimize"),
        ] {
            assert!(
                command.reaches_past_the_lock(),
                "{command:?} is allowed to a binding at the lock"
            );
        }
    }

    /// **Desktop mode: the arrows snap the focused window to each half, and a
    /// second press toward the edge it is already at crosses to the next
    /// monitor that way.** #222.
    #[test]
    fn desktop_mode_the_arrows_snap_to_each_half_and_cross_at_the_edge() {
        let mut desk = Desk::new("", two_screens());
        desk.open(1, "DP-1", (1280.0, 720.0));

        desk.press("super+left");
        let left = desk.rect(1);
        assert!(
            about(left.x, WIDE.x)
                && about(left.w, WIDE.w / 2.0)
                && about(left.h, WIDE.h)
                && about(left.y, WIDE.y),
            "super+left did not snap to the left half of DP-1: {left:?}"
        );

        // Already as far left as it goes: DP-1 has no monitor to its left.
        let commands = desk.press("super+left");
        assert!(
            places(&commands).is_empty(),
            "a second super+left with no monitor further left placed something: {commands:?}"
        );
        assert!(
            same(desk.rect(1), left),
            "the window moved with nowhere to go: {:?}",
            desk.rect(1)
        );

        desk.press("super+right");
        let right = desk.rect(1);
        assert!(
            about(right.x + right.w, WIDE.x + WIDE.w) && about(right.w, WIDE.w / 2.0),
            "super+right did not snap to the right half of DP-1: {right:?}"
        );

        // Already at DP-1's right half: a second press crosses to DP-2.
        desk.press("super+right");
        assert_eq!(
            desk.monitor(1),
            "DP-2",
            "a second super+right did not cross to DP-2: {:?}",
            desk.rects()
        );
        let crossed = desk.rect(1);
        assert!(
            about(crossed.x + crossed.w, TALL.x + TALL.w) && about(crossed.w, TALL.w / 2.0),
            "window 1 is not the right half of DP-2: {crossed:?}"
        );
    }

    /// **Desktop mode: restoring puts a snapped window back exactly where it
    /// was, and does nothing once there is nothing to restore.** #222.
    #[test]
    fn desktop_mode_restore_puts_a_snapped_window_back_where_it_was() {
        let mut desk = Desk::new("", one_screen());
        desk.open(1, "DP-1", (1280.0, 720.0));
        let original = desk.rect(1);

        desk.press("super+left");
        assert!(
            !same(desk.rect(1), original),
            "the premise: super+left actually moved the window"
        );
        desk.press("super+down");
        assert!(
            same(desk.rect(1), original),
            "super+down did not restore the pre-snap rectangle: {:?}",
            desk.rects()
        );

        let commands = desk.press("super+down");
        assert!(
            places(&commands).is_empty(),
            "super+down with nothing to restore placed something: {commands:?}"
        );
    }

    /// **Desktop mode: maximise asks the compositor rather than placing the
    /// window itself, restore un-maximises a maximised window, and maximise
    /// from a half goes to that half's top quarter instead.** #222.
    ///
    /// The harness does not simulate what `sol.toggle_maximize` does to a
    /// window's rectangle -- that is the compositor's, in production -- so
    /// this asks only that the right command is sent, and stands in for the
    /// compositor's own answer with `scripts.maximize`, exactly as
    /// `sol_toggles_name_the_focused_window_when_given_none` does.
    #[test]
    fn desktop_mode_maximize_asks_the_compositor_and_a_half_goes_to_its_quarter() {
        let mut desk = Desk::new("", one_screen());
        desk.open(1, "DP-1", (1280.0, 720.0));
        let original = desk.rect(1);

        let commands = desk.press("super+up");
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::ToggleMaximize { id: 1 })),
            "super+up did not ask to maximise: {commands:?}"
        );
        let _ = desk.scripts.maximize(1, true, desk.snapshot());

        let commands = desk.press("super+down");
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::ToggleMaximize { id: 1 })),
            "super+down did not un-maximise a maximised window: {commands:?}"
        );
        let _ = desk.scripts.maximize(1, false, desk.snapshot());
        assert!(
            same(desk.rect(1), original),
            "the window did not keep its pre-maximise rectangle: {:?}",
            desk.rects()
        );

        desk.press("super+left");
        let left = desk.rect(1);
        let commands = desk.press("super+up");
        assert_eq!(
            places(&commands),
            vec![1],
            "super+up from a half did not place the window: {commands:?}"
        );
        let quarter = desk.rect(1);
        assert!(
            about(quarter.x, left.x)
                && about(quarter.w, left.w)
                && about(quarter.y, left.y)
                && about(quarter.h, left.h / 2.0),
            "super+up from the left half is not its top quarter: half {left:?} quarter {quarter:?}"
        );

        let commands = desk.press("super+up");
        assert!(
            places(&commands).is_empty(),
            "a second super+up from the quarter placed something: {commands:?}"
        );
    }

    /// **Desktop mode: a new window with no size of its own gets a share of
    /// the work area, smaller than it on both axes.** #222.
    #[test]
    fn desktop_mode_a_window_with_no_size_of_its_own_gets_a_share_of_the_work_area() {
        let mut desk = Desk::new("", one_screen());
        let mut unshown = window(
            1,
            "DP-1",
            Rect {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            },
        );
        unshown.shown = false;
        desk.windows.push(unshown);
        let outcome = desk.scripts.opened(1, desk.snapshot());
        desk.apply(&outcome.commands);
        let rect = desk.rect(1);
        assert!(
            rect.w > 0.0 && rect.w < WIDE.w && rect.h > 0.0 && rect.h < WIDE.h,
            "a window that has shown no frame of its own did not get a share of the work \
             area smaller than it: {rect:?}"
        );
    }

    /// **Desktop mode: three windows opened in a row land at different
    /// spots, not stacked on top of each other.** #222.
    #[test]
    fn desktop_mode_places_three_new_windows_without_stacking_them() {
        let mut desk = Desk::new("", one_screen());
        desk.open(1, "DP-1", (1280.0, 720.0));
        desk.open(2, "DP-1", (1280.0, 720.0));
        desk.open(3, "DP-1", (1280.0, 720.0));
        let rects = [desk.rect(1), desk.rect(2), desk.rect(3)];
        assert!(
            !same(rects[0], rects[1]) && !same(rects[1], rects[2]) && !same(rects[0], rects[2]),
            "three new windows landed on top of each other: {rects:?}"
        );
    }

    /// The work area #240 was measured on: the nested 1600x900 output.
    const NESTED: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 1600.0,
        h: 900.0,
    };

    /// Window `id` opens on `DP-1` before its application has shown a frame
    /// -- every window `sol.spawn` opens, `super+return`'s terminal among
    /// them -- and has the keyboard once it is placed.
    fn open_unshown(desk: &mut Desk, id: u64) {
        let mut unshown = window(
            id,
            "DP-1",
            Rect {
                w: 0.0,
                h: 0.0,
                ..NESTED
            },
        );
        unshown.shown = false;
        desk.windows.insert(0, unshown);
        let outcome = desk.scripts.opened(id, desk.snapshot());
        desk.apply(&outcome.commands);
        desk.focus(id);
    }

    /// The atrium off: these check where floating placement puts windows on
    /// the whole work area, which the atrium's strip would narrow.
    const NO_ATRIUM: &str = "require(\"config\").floating.atrium.enabled = false\n";

    /// Window `id` opens on `DP-1` already `w` by `h`: an application that
    /// has shown a frame of its own size, which placement keeps.
    fn open_shown(desk: &mut Desk, id: u64, w: f64, h: f64) {
        desk.windows
            .insert(0, window(id, "DP-1", Rect { w, h, ..NESTED }));
        let outcome = desk.scripts.opened(id, desk.snapshot());
        desk.apply(&outcome.commands);
        desk.focus(id);
    }

    /// Fails unless every window on `desk` is inside `area` and no two share
    /// a spot: top-left corners nearer than half the shipped `cascade` step
    /// (16) on both axes, `floating.lua`'s `taken`.
    fn spread_inside(desk: &Desk, area: Rect, why: &str) {
        let rects = desk.rects();
        for (at, (id, rect)) in rects.iter().enumerate() {
            assert!(
                inside(*rect, area),
                "{why}: window {id} is not inside the work area: {rects:?}"
            );
            for (other, earlier) in &rects[..at] {
                assert!(
                    (rect.x - earlier.x).abs() >= 16.0 || (rect.y - earlier.y).abs() >= 16.0,
                    "{why}: windows {other} and {id} share a spot: {rects:?}"
                );
            }
        }
    }

    /// **Desktop mode: ten windows opened before their first frame, as
    /// `super+return` opens them, land centred and then one `cascade` step
    /// apart, each `placement.size` of the work area; the seventh, which one
    /// more step would take past the bottom edge, starts a new diagonal at
    /// the work area's top-left plus one step (`wrap = "corner"`).** #240,
    /// measured nested on 1600x900: the second window landed on the work
    /// area's bottom-right corner, and the third and fourth exactly on top of
    /// it.
    #[test]
    fn desktop_mode_ten_spawned_windows_step_by_cascade_and_wrap_at_the_edge() {
        let mut desk = Desk::new(NO_ATRIUM, vec![screen("DP-1", NESTED)]);
        for id in 1..=10 {
            open_unshown(&mut desk, id);
        }
        let wanted = [
            (320.0, 180.0),
            (352.0, 212.0),
            (384.0, 244.0),
            (416.0, 276.0),
            (448.0, 308.0),
            (480.0, 340.0),
            (32.0, 32.0),
            (64.0, 64.0),
            (96.0, 96.0),
            (128.0, 128.0),
        ];
        for (id, (x, y)) in (1..=10).zip(wanted) {
            let rect = desk.rect(id);
            assert!(
                about(rect.x, x)
                    && about(rect.y, y)
                    && about(rect.w, 960.0)
                    && about(rect.h, 540.0),
                "window {id} is not 960x540 at {x},{y}: {:?}",
                desk.rects()
            );
        }
    }

    /// **Desktop mode: a spot a window was dragged a few pixels off is still
    /// taken**: nearer than half a `cascade` step on both axes, the next
    /// window steps on rather than landing all but on top of it. #240.
    #[test]
    fn desktop_mode_a_spot_a_window_was_nudged_off_is_still_taken() {
        let mut desk = Desk::new(NO_ATRIUM, vec![screen("DP-1", NESTED)]);
        open_unshown(&mut desk, 1);
        for each in &mut desk.windows {
            each.rect.x += 5.0;
            each.rect.y += 5.0;
        }
        open_unshown(&mut desk, 2);
        let second = desk.rect(2);
        assert!(
            about(second.x, 352.0) && about(second.y, 212.0),
            "window 2 did not step past the centre window 1 was nudged 5 pixels off: {:?}",
            desk.rects()
        );
    }

    /// **Desktop mode: a `cascade` that is not a step of a pixel or more on
    /// each axis is read as the shipped (32, 32)**, rather than a step of
    /// nothing that would stack every window on the first. #240.
    #[test]
    fn desktop_mode_a_cascade_of_no_step_is_read_as_the_shipped_one() {
        let mut desk = Desk::new(
            "require(\"config\").floating.atrium.enabled = false\n\
             local config = require(\"config\")\n\
             config.floating.placement.cascade = { x = 0, y = \"far\" }\n",
            vec![screen("DP-1", NESTED)],
        );
        open_unshown(&mut desk, 1);
        open_unshown(&mut desk, 2);
        let second = desk.rect(2);
        assert!(
            about(second.x, 352.0) && about(second.y, 212.0),
            "a cascade of (0, \"far\") did not step by (32, 32): {:?}",
            desk.rects()
        );
    }

    /// **Desktop mode: with no spot free, a window lands where its policy
    /// starts, inside the work area**: a window the size of the work area has
    /// only the one spot, so the second lands on the first rather than the
    /// walk looking for a spot that is not there. #240.
    #[test]
    fn desktop_mode_with_no_free_spot_a_window_lands_where_its_policy_starts() {
        let mut desk = Desk::new(
            "require(\"config\").floating.atrium.enabled = false\n\
             local config = require(\"config\")\n\
             config.floating.placement.size = 1\n",
            vec![screen("DP-1", NESTED)],
        );
        open_unshown(&mut desk, 1);
        open_unshown(&mut desk, 2);
        for id in [1, 2] {
            assert!(
                same(desk.rect(id), NESTED),
                "window {id} is not the whole work area: {:?}",
                desk.rects()
            );
        }
    }

    /// **Desktop mode: windows nearly as tall as the work area still get a
    /// spot each**: with less room below the first than one `cascade` step,
    /// the corner diagonals start as far down as the window leaves room for,
    /// rather than not at all. #240's review: two 1200x880 windows on
    /// 1600x900 both landed on the centre, since the corner's first spot,
    /// (32, 32), did not fit and the walk gave up.
    #[test]
    fn desktop_mode_windows_nearly_the_work_area_tall_get_spots_of_their_own() {
        let mut desk = Desk::new(NO_ATRIUM, vec![screen("DP-1", NESTED)]);
        for id in 1..=4 {
            open_shown(&mut desk, id, 1200.0, 880.0);
        }
        spread_inside(&desk, NESTED, "1200x880, wrap = \"corner\"");
        let wanted = [(200.0, 10.0), (32.0, 20.0), (64.0, 20.0), (96.0, 20.0)];
        for (id, (x, y)) in (1..=4).zip(wanted) {
            let rect = desk.rect(id);
            assert!(
                about(rect.x, x) && about(rect.y, y),
                "window {id} is not at {x},{y}: {:?}",
                desk.rects()
            );
        }
    }

    /// **Desktop mode: once every diagonal its `wrap` starts is spent, a new
    /// window takes the first free spot of the `cascade` grid over the work
    /// area rather than landing on another.** `wrap = "center"` with
    /// 1200x880 windows on 1600x900 has room for seven diagonals of one spot
    /// each, (200, 10) to (392, 10); the eighth and ninth windows used to
    /// land on the first. #240's review.
    #[test]
    fn desktop_mode_once_every_diagonal_is_spent_a_window_takes_a_free_spot_of_the_grid() {
        let mut desk = Desk::new(
            "require(\"config\").floating.atrium.enabled = false\n\
             local config = require(\"config\")\n\
             config.floating.placement.wrap = \"center\"\n",
            vec![screen("DP-1", NESTED)],
        );
        for id in 1..=9 {
            open_shown(&mut desk, id, 1200.0, 880.0);
        }
        spread_inside(&desk, NESTED, "1200x880, wrap = \"center\"");
        let diagonals = [200.0, 232.0, 264.0, 296.0, 328.0, 360.0, 392.0];
        for (id, x) in (1..=7).zip(diagonals) {
            let rect = desk.rect(id);
            assert!(
                about(rect.x, x) && about(rect.y, 10.0),
                "window {id} is not at {x},10: {:?}",
                desk.rects()
            );
        }
        for (id, (x, y)) in [(8, (0.0, 0.0)), (9, (32.0, 0.0))] {
            let rect = desk.rect(id);
            assert!(
                about(rect.x, x) && about(rect.y, y),
                "window {id} is not at {x},{y}: {:?}",
                desk.rects()
            );
        }
    }

    /// **Desktop mode: windows nearly as wide as the work area get a spot
    /// each, whichever `wrap`**: no diagonal has room for even its first
    /// spot, so each window after the first takes the next free spot of the
    /// `cascade` grid, down the work area's left edge. #240's review: 1590x400
    /// windows on 1600x900 all landed on the centre under both wraps.
    #[test]
    fn desktop_mode_windows_nearly_the_work_area_wide_get_spots_of_their_own() {
        for wrap in ["corner", "center"] {
            let mut desk = Desk::new(
                &format!(
                    "require(\"config\").floating.atrium.enabled = false\n\
                     local config = require(\"config\")\n\
                     config.floating.placement.wrap = {wrap:?}\n"
                ),
                vec![screen("DP-1", NESTED)],
            );
            for id in 1..=4 {
                open_shown(&mut desk, id, 1590.0, 400.0);
            }
            spread_inside(&desk, NESTED, &format!("1590x400, wrap = {wrap:?}"));
            let wanted = [(5.0, 250.0), (0.0, 0.0), (0.0, 32.0), (0.0, 64.0)];
            for (id, (x, y)) in (1..=4).zip(wanted) {
                let rect = desk.rect(id);
                assert!(
                    about(rect.x, x) && about(rect.y, y),
                    "wrap = {wrap:?}: window {id} is not at {x},{y}: {:?}",
                    desk.rects()
                );
            }
        }
    }

    /// **Tiling keeps the plain arrows for focus by direction, and desktop
    /// mode gets them back the moment nothing else is in charge.** #222's
    /// whole reason for going through `modes.watch` rather than binding the
    /// four keys for good.
    #[test]
    fn tiling_keeps_the_arrows_for_focus_and_desktop_mode_gets_them_back() {
        let mut desk = grid("", one_screen());
        desk.focus(2);
        assert_eq!(
            focuses(&desk.press("super+left")),
            [1],
            "tiling: super+left still focuses the neighbouring tile"
        );

        // Self-toggle: tiling off is floating, #222's desktop mode.
        desk.press("super+t");
        desk.focus(1);
        let tiled = desk.rect(1);
        desk.press("super+left");
        let snapped = desk.rect(1);
        assert!(
            about(snapped.h, WIDE.h) && snapped.h > tiled.h + 100.0,
            "super+left did not snap once floating took over: tiled {tiled:?} snapped {snapped:?}"
        );
    }

    /// **A `config.bindings` override on a plain arrow survives leaving and
    /// returning to desktop mode**: it must win once, at load (the premise
    /// below), and go on winning every time `super+t`/`super+s` hands the
    /// arrows back, not just until the first one (#222's review).
    ///
    /// `require("bindings")` runs as `new_with`'s `after`, exactly where the
    /// real `init.lua` has it -- last -- so `super+left`'s override wins the
    /// same way it would in production, with no help from this file's own
    /// fix: that part was never broken. What was broken is the *second*
    /// assertion, once a mode switch has run `unbind_keys` at least once.
    #[test]
    fn a_config_bindings_override_on_an_arrow_survives_leaving_and_returning_to_floating() {
        let mut desk = Desk::new_with(
            "local config = require(\"config\")\n\
             config.bindings[\"super+left\"] = function() sol.status(\"custom-left\") end\n",
            "require(\"bindings\")\n",
            one_screen(),
        );

        let status = |desk: &mut Desk, combo: &str| {
            desk.scripts
                .key(combo, desk.snapshot())
                .status
                .unwrap_or_default()
        };
        assert_eq!(
            status(&mut desk, "super+left"),
            "custom-left",
            "the premise: the override wins while floating is in charge"
        );

        // Leave floating (tiling) and come back (self-toggle): both are
        // `unbind_keys`, the first into tiling and the second out of it.
        desk.press("super+t");
        assert_eq!(
            status(&mut desk, "super+left"),
            "custom-left",
            "leaving floating silently reverted super+left to the shipped default"
        );
        desk.press("super+t");
        assert_eq!(
            status(&mut desk, "super+left"),
            "custom-left",
            "returning to floating silently reverted super+left to the shipped default"
        );
    }

    /// **A snap key `config.floating.snap` has moved off its plain arrow is
    /// not left bound once floating lets go.** `config.floating.snap.left =
    /// "super+z"` moves what `super+left` used to mean off the arrow
    /// entirely; leaving floating must give `super+z` back to nothing, not
    /// leave it calling `snap("left")` in tiling for the rest of the session
    /// (#222's review).
    #[test]
    fn a_snap_key_moved_off_the_plain_arrows_is_not_left_bound_once_floating_lets_go() {
        let mut desk = Desk::new(
            "local config = require(\"config\")\n\
             config.floating.snap.left = \"super+z\"\n",
            one_screen(),
        );
        desk.open(1, "DP-1", (1280.0, 720.0));

        assert!(
            desk.scripts.has_binding("super+z"),
            "the premise: super+z is bound while floating is in charge"
        );
        let commands = desk.press("super+z");
        assert_eq!(
            places(&commands),
            vec![1],
            "the premise: super+z snaps the focused window"
        );

        desk.press("super+t");
        assert!(
            !desk.scripts.has_binding("super+z"),
            "super+z is still bound after floating let go of it, to whatever it last did \
             with it -- it is not one of the four plain arrows, so nothing ever gives it \
             back to anything"
        );
    }

    /// **Desktop mode: crossing to the next monitor on a snap carries the
    /// window's explicit workspace with it**, exactly as `direction.lua`'s own
    /// cross-monitor `floating.move` does (`super+shift+left`), so a window
    /// sent to the workspace its monitor happens to show does not end up
    /// grouped onto a desk the monitor it lands on is not showing (#222's
    /// review).
    #[test]
    fn desktop_mode_a_cross_monitor_snap_carries_the_window_to_the_workspace_its_new_monitor_shows()
    {
        let mut desk = Desk::new("", two_screens());
        desk.open(1, "DP-1", (1280.0, 720.0));

        // Explicitly assigned to workspace 1 -- which is also what DP-1,
        // unconfigured, already shows -- so the assignment changes nothing
        // visible yet, only `workspaces.of[1]`.
        desk.press("super+shift+1");
        assert_eq!(
            desk.workspace_of(1),
            "1",
            "the premise: window 1 has an explicit workspace"
        );

        // DP-2 shows workspace 2, the same fakery
        // `at_a_monitors_edge_tiling_crosses_to_the_desk_the_next_monitor_shows`
        // uses, since nothing in this harness switches a real workspace.
        assert_eq!(
            desk.scripts
                .evaluate("require(\"workspaces\").showing[\"DP-2\"] = 2 return \"\""),
            ""
        );

        desk.press("super+right");
        assert!(
            desk.monitor(1) == "DP-1",
            "the premise: one super+right only reaches DP-1's right half: {:?}",
            desk.rects()
        );
        desk.press("super+right");
        assert_eq!(
            desk.monitor(1),
            "DP-2",
            "a second super+right did not cross to DP-2: {:?}",
            desk.rects()
        );
        assert_eq!(
            desk.workspace_of(1),
            "2",
            "window 1 crossed to DP-2 but was not carried to the workspace it shows -- the \
             next `layout` would group it onto a desk DP-2 is not showing"
        );
    }

    /// **Desktop mode: snapping a window maximised through another door --
    /// `super+shift+m`, or the frame's button -- does not capture a bogus
    /// `before_snap` rectangle.** Lua reads `sol.windows()` before the
    /// un-maximise this same key asks for has landed, so a window maximised
    /// outside `floating.lua` has no valid pre-maximise rectangle this file
    /// can capture at that point -- only the compositor remembers it, and a
    /// snap straight after throws that memory away by placing the window
    /// somewhere else. Restoring afterwards must not place the stale,
    /// still-maximised-looking rectangle it would otherwise have recorded
    /// (#222's review).
    #[test]
    fn desktop_mode_snap_after_an_external_maximize_does_not_capture_a_bogus_before_snap_rect() {
        let mut desk = Desk::new("", one_screen());
        desk.open(1, "DP-1", (1280.0, 720.0));

        let commands = desk.press("super+shift+m");
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::ToggleMaximize { id: 1 })),
            "the premise: super+shift+m asks to maximise: {commands:?}"
        );
        let _ = desk.scripts.maximize(1, true, desk.snapshot());
        // What the compositor would have done to the window's own rectangle,
        // in production -- the harness does not simulate it, the same reason
        // `desktop_mode_maximize_asks_the_compositor_and_a_half_goes_to_its_quarter`
        // gives.
        for each in &mut desk.windows {
            if each.id == 1 {
                each.rect = WIDE;
            }
        }

        let commands = desk.press("super+left");
        assert_eq!(
            places(&commands),
            vec![1],
            "super+left did not un-maximise and snap: {commands:?}"
        );
        let left = desk.rect(1);

        let commands = desk.press("super+down");
        assert!(
            places(&commands).is_empty(),
            "restore after an externally-maximised window's first snap placed a bogus \
             rectangle rather than doing nothing: {commands:?}"
        );
        assert!(
            same(desk.rect(1), left),
            "restore moved the window with nothing valid to put it back to: {:?}",
            desk.rects()
        );
    }

    // ## The pointer follows the keyboard (#220)
    //
    // `lua/follow.lua` against the shipped layouts: which focus changes ask
    // for a warp, to where, and how, by what moved the keyboard, and the
    // two that tell no `focus` -- a move, and a switch to an empty
    // workspace. The warp itself is the compositor's, and tested there:
    // `state::tests::real_client::follow`.

    /// The warps `commands` asked for, in order.
    fn warps(commands: &[Command]) -> Vec<Warp> {
        commands
            .iter()
            .filter_map(|command| match command {
                Command::Warp(warp) => Some(*warp),
                _ => None,
            })
            .collect()
    }

    /// A warp to window `id`'s `at`, along `motion`, that the mouse carries
    /// on from where the pointer is.
    fn to_window(id: u64, at: (f64, f64), motion: Option<AnimationSpec>) -> Warp {
        Warp {
            to: WarpTo::Window { id, at },
            motion,
            interrupt: Interrupt::Pointer,
        }
    }

    /// The shipped `focus.cursor.motion`.
    fn shipped_motion() -> Option<AnimationSpec> {
        Some(AnimationSpec {
            duration: std::time::Duration::from_millis(260),
            easing: Curve::OutCubic,
        })
    }

    const CENTRE: (f64, f64) = (0.5, 0.5);

    impl Desk {
        /// Press a bound key, and tell `focus` for every window it focused,
        /// with what it was answering, as the compositor does once the window
        /// has the keyboard. Everything asked for, in order.
        ///
        /// Each `focus` says whether the keyboard came from another window,
        /// as `Solium::focus_window` does: a layout focusing again the window
        /// that has it is told `changed = false`.
        fn press_told(&mut self, combo: &str) -> Vec<Command> {
            let mut had = self
                .windows
                .iter()
                .find(|window| window.focused)
                .map(|window| window.id);
            let mut commands = self.press(combo);
            let focused: Vec<(u64, FocusSource, bool)> = commands
                .iter()
                .filter_map(|command| match command {
                    Command::Focus { id, source, .. } => {
                        let changed = had != Some(*id);
                        had = Some(*id);
                        Some((*id, *source, changed))
                    }
                    _ => None,
                })
                .collect();
            for (id, source, changed) in focused {
                commands.extend(self.told(id, source, changed));
            }
            commands
        }

        /// Window `id` takes the keyboard, moved by `source`, from another
        /// window if `changed`, and `focus` is told. What its listeners asked
        /// for.
        fn told(&mut self, id: u64, source: FocusSource, changed: bool) -> Vec<Command> {
            self.focus(id);
            let commands = self
                .scripts
                .focused(id, source, changed, self.snapshot())
                .commands;
            self.apply(&commands);
            commands
        }
    }

    /// **The pointer follows a key and not a click**: `super+left` in tiling
    /// focuses the tile to the left, and its `focus` asks for a warp to that
    /// window's centre along the shipped motion; any other key binding's
    /// focus does too; a click, a bar's chip, a script, focus following the
    /// mouse, a touch, the wheel, a drop, a window opening, an application
    /// asking and the keyboard settling after a close ask for none.
    #[test]
    fn the_pointer_follows_a_key_and_not_a_click() {
        let mut desk = grid("", one_screen());
        desk.focus(4);
        let commands = desk.press_told("super+left");
        assert_eq!(
            (focuses(&commands), warps(&commands)),
            (vec![3], vec![to_window(3, CENTRE, shipped_motion())]),
            "(focused, warped): {commands:?}"
        );
        assert_eq!(
            warps(&desk.told(1, FocusSource::Binding, true)),
            vec![to_window(1, CENTRE, shipped_motion())],
            "a key binding's focus"
        );
        for source in [
            FocusSource::Click,
            FocusSource::Surface,
            FocusSource::Script,
            FocusSource::Hover,
            FocusSource::Touch,
            FocusSource::Scroll,
            FocusSource::Drop,
            FocusSource::Open,
            FocusSource::Activate,
            FocusSource::Settle,
        ] {
            assert_eq!(warps(&desk.told(2, source, true)), Vec::new(), "{source:?}");
        }
    }

    /// **Which focus changes take the pointer along, where to and how is
    /// configured**: with `focus.cursor.follows.direction` off and `click`
    /// on, `super+left` asks for no warp and a click does; `to` is where on
    /// the window, `motion` how the picture flies, `interrupt` where the
    /// mouse carries on from mid-flight, and `motion = false` puts it there
    /// at once.
    #[test]
    fn which_focus_changes_take_the_pointer_along_where_to_and_how_is_configured() {
        let configured = "local cursor = require(\"config\").focus.cursor\n\
                          cursor.follows.direction = false\n\
                          cursor.follows.click = true\n\
                          cursor.to = { x = 0.5, y = 0.25 }\n\
                          cursor.motion = { duration = 100, easing = \"linear\" }\n\
                          cursor.interrupt = \"picture\"";
        let mut desk = grid(configured, one_screen());
        desk.focus(4);
        let commands = desk.press_told("super+left");
        assert_eq!(
            (focuses(&commands), warps(&commands)),
            (vec![3], Vec::new()),
            "direction off: (focused, warped)"
        );
        assert_eq!(
            warps(&desk.told(2, FocusSource::Click, true)),
            vec![Warp {
                interrupt: Interrupt::Picture,
                ..to_window(
                    2,
                    (0.5, 0.25),
                    Some(AnimationSpec {
                        duration: std::time::Duration::from_millis(100),
                        easing: Curve::Linear,
                    }),
                )
            }],
            "click on, to the top quarter, in 100 ms linear, the mouse going on from the picture"
        );

        let mut desk = grid(
            "require(\"config\").focus.cursor.motion = false",
            one_screen(),
        );
        desk.focus(4);
        assert_eq!(
            warps(&desk.press_told("super+left")),
            vec![to_window(3, CENTRE, None)],
            "motion = false: there at once"
        );
    }

    /// **A move takes the pointer to where the window went**: no `focus` is
    /// told for a move, which keeps the keyboard on the window that moved,
    /// so the `direction` listener asks for the warp to it -- after the
    /// layout has placed it, so the compositor finds it where it went --
    /// unless `focus.cursor.follows.move` is off.
    #[test]
    fn a_move_takes_the_pointer_to_where_the_window_went() {
        let mut desk = grid("", one_screen());
        desk.focus(4);
        let commands = desk.press_told("super+shift+left");
        let placed = commands
            .iter()
            .rposition(|command| matches!(command, Command::Place { id: 4, .. }));
        let warped = commands
            .iter()
            .position(|command| matches!(command, Command::Warp(_)));
        assert_eq!(
            (focuses(&commands), warps(&commands)),
            (Vec::new(), vec![to_window(4, CENTRE, shipped_motion())]),
            "(focused, warped): {commands:?}"
        );
        assert!(
            placed.is_some() && warped > placed,
            "the warp is asked for once the window is placed: {commands:?}"
        );

        let mut desk = grid(
            "require(\"config\").focus.cursor.follows.move = false",
            one_screen(),
        );
        desk.focus(4);
        assert_eq!(
            warps(&desk.press_told("super+shift+left")),
            Vec::new(),
            "follows.move off"
        );
    }

    /// **In scrolling, a key takes the pointer along only if it moves the
    /// keyboard or the window that has it**: every key the strip binds ends
    /// with the strip focusing its window again, and `focus` says
    /// `changed = false` for the window that already had the keyboard. From
    /// the first of three columns, `super+r`, which only widens the column,
    /// asks for no warp; `super+comma` stacks the next column's window under
    /// the first and focuses it, which takes the pointer to it;
    /// `super+period` pushes it back out into a column of its own and
    /// `super+ctrl+bracketleft` moves a column, and each takes the pointer
    /// after the window it moved, as a move by direction does, unless
    /// `focus.cursor.follows.move` is off; `focus.cursor.refocus = true`
    /// takes the pointer along for `super+r` too.
    #[test]
    fn in_scrolling_a_key_takes_the_pointer_along_only_if_it_moves_the_keyboard_or_its_window() {
        let strip = |before: &str| {
            let mut desk = Desk::new(before, one_screen());
            for id in [1, 2, 3] {
                desk.windows.push(window(
                    id,
                    "DP-1",
                    Rect {
                        w: 400.0,
                        h: 300.0,
                        ..WIDE
                    },
                ));
            }
            desk.press("super+s");
            desk.press("super+bracketleft");
            desk.press("super+bracketleft");
            let x = |desk: &Desk, id| desk.rect(id).x;
            assert!(
                x(&desk, 1) < x(&desk, 2)
                    && x(&desk, 2) < x(&desk, 3)
                    && desk
                        .windows
                        .iter()
                        .any(|window| window.id == 1 && window.focused),
                "the premise: columns 1, 2, 3, and the first has the keyboard: {:?}",
                desk.rects()
            );
            desk
        };
        let to = |id| vec![to_window(id, CENTRE, shipped_motion())];

        let mut desk = strip("");
        let widened = desk.press_told("super+r");
        assert_eq!(
            (focuses(&widened), warps(&widened)),
            (vec![1], Vec::new()),
            "super+r: (focused again, warped)"
        );
        assert_eq!(warps(&desk.press_told("super+comma")), to(2), "super+comma");
        assert_eq!(
            warps(&desk.press_told("super+period")),
            to(2),
            "super+period"
        );
        assert_eq!(
            warps(&desk.press_told("super+bracketright")),
            to(3),
            "super+bracketright"
        );
        assert_eq!(
            warps(&desk.press_told("super+ctrl+bracketleft")),
            to(3),
            "super+ctrl+bracketleft"
        );

        let mut desk = strip("require(\"config\").focus.cursor.follows.move = false");
        assert_eq!(
            warps(&desk.press_told("super+comma")),
            to(2),
            "move off: super+comma"
        );
        assert_eq!(
            [
                warps(&desk.press_told("super+period")),
                warps(&desk.press_told("super+ctrl+bracketleft")),
            ],
            [Vec::new(), Vec::new()],
            "move off: [super+period, super+ctrl+bracketleft]"
        );

        let mut desk = strip("require(\"config\").focus.cursor.refocus = true");
        assert_eq!(
            warps(&desk.press_told("super+r")),
            to(1),
            "refocus on: super+r"
        );
    }

    /// **A switch to an empty workspace takes the pointer to the middle, or
    /// leaves it**: `super+2` onto a workspace with no window asks for a warp
    /// to the middle of the monitor that switched -- the one the pointer is
    /// on, with two -- and `super+1` back focuses the window the keyboard
    /// stayed on, and takes the pointer to it although its `focus` says the
    /// keyboard did not move. `focus.cursor.empty_workspace =
    /// "stay"` leaves it, and so does a key binding no longer taking it along.
    #[test]
    fn a_switch_to_an_empty_workspace_takes_the_pointer_to_the_middle_or_leaves_it() {
        let middle = |area: Rect| WarpTo::Point {
            x: area.x + area.w / 2.0,
            y: area.y + area.h / 2.0,
        };
        let mut desk = grid("", one_screen());
        let commands = desk.press_told("super+2");
        assert_eq!(
            (focuses(&commands), warps(&commands)),
            (
                Vec::new(),
                vec![Warp {
                    to: middle(WIDE),
                    motion: shipped_motion(),
                    interrupt: Interrupt::Pointer,
                }]
            ),
            "onto an empty workspace: (focused, warped)"
        );
        let commands = desk.press_told("super+1");
        let back = focuses(&commands);
        assert_eq!(
            warps(&commands),
            back.iter()
                .map(|id| to_window(*id, CENTRE, shipped_motion()))
                .collect::<Vec<_>>(),
            "back: the window focused takes the pointer, {back:?}"
        );
        assert_eq!(back.len(), 1, "back: one window focused");

        for configured in [
            "require(\"config\").focus.cursor.empty_workspace = \"stay\"",
            "require(\"config\").focus.cursor.follows.binding = false",
        ] {
            let mut desk = grid(configured, one_screen());
            assert_eq!(
                warps(&desk.press_told("super+2")),
                Vec::new(),
                "{configured}"
            );
        }

        let mut desk = Desk::new("", two_screens());
        for monitor in &mut desk.monitors {
            monitor.focused = monitor.name == "DP-2";
        }
        assert_eq!(
            warps(&desk.press_told("super+2")),
            vec![Warp {
                to: middle(TALL),
                motion: shipped_motion(),
                interrupt: Interrupt::Pointer,
            }],
            "two monitors: the middle of the one the pointer is on"
        );
    }

    /// **The `focus` event says what moved the keyboard**, and
    /// `sol.focus_source()` says the same in the handler: a key binding's
    /// `sol.focus` is `"binding"`, one in `sol.focus_direction`'s or
    /// `sol.move_direction`'s listeners `"direction"` or `"move"` -- and the
    /// binding's again once that returns --, a scene's action `"surface"`, a
    /// click while a script holds input `"click"`, and the `open`,
    /// `activate`, `scroll` and `drop` listeners theirs; anything else is
    /// `"script"`. The compositor's own are told as it says them.
    #[test]
    fn the_focus_event_says_what_moved_the_keyboard() {
        let mut scripts = plain(
            "focus-source",
            r#"
            heard = {}
            local function note(id) sol.focus(id) heard[#heard + 1] = sol.focus_source() end
            sol.on("focus", function(id, source) heard[#heard + 1] = id .. " " .. source end)
            sol.on("direction", function(verb) note(verb == "focus" and 2 or 3) end)
            sol.bind("super+a", function() note(1) end)
            sol.bind("super+d", function() sol.focus_direction("left") note(4) end)
            sol.bind("super+m", function() sol.move_direction("left") end)
            sol.on("surface", function() note(5) end)
            sol.on("click", function() note(6) end)
            sol.on("open", function(id) note(id) end)
            sol.on("activate", function(id) note(id) end)
            sol.on("scroll", function() note(8) end)
            sol.on("drop", function(id) note(id) end)
            sol.on("layout", function() note(9) end)
            "#,
        );
        let sources = |outcome: super::Outcome| -> Vec<(u64, &'static str)> {
            outcome
                .commands
                .iter()
                .filter_map(|command| match command {
                    Command::Focus { id, source, .. } => Some((*id, source.name())),
                    _ => None,
                })
                .collect()
        };
        let none = Snapshot::default;
        assert_eq!(
            [
                sources(scripts.key("super+a", none())),
                sources(scripts.key("super+d", none())),
                sources(scripts.key("super+m", none())),
                sources(scripts.surface_action("bar", "go", &crate::json::Json::Null, none())),
                sources(scripts.click(1.0, 1.0, none())),
                sources(scripts.opened(7, none())),
                sources(scripts.activated(7, "request", none())),
                sources(scripts.scrolled(0.0, 1.0, none())),
                sources(scripts.dropped(7, 0.0, 0.0, none())),
                sources(scripts.relayout(none())),
            ],
            [
                vec![(1, "binding")],
                vec![(2, "direction"), (4, "binding")],
                vec![(3, "move")],
                vec![(5, "surface")],
                vec![(6, "click")],
                vec![(7, "open")],
                vec![(7, "activate")],
                vec![(8, "scroll")],
                vec![(7, "drop")],
                vec![(9, "script")],
            ],
            "what each `sol.focus` was asked with"
        );
        let _ = scripts.focused(7, FocusSource::Hover, true, none());
        let _ = scripts.focused(7, FocusSource::Settle, false, none());
        assert_eq!(
            scripts.evaluate("return table.concat(heard, \",\")"),
            "binding,direction,binding,move,surface,click,open,activate,scroll,drop,script,\
             7 hover,7 settle",
            "what `sol.focus_source()` said in each handler, then two `focus` events"
        );
    }

    /// **`sol.focus` carries its source and its warp**: a warp table is
    /// `sol.warp`'s options aimed at the window focused, `true` is its centre
    /// at once, none is none, a motion with nothing in it is the default
    /// one, and a warp of any other kind is an error.
    #[test]
    fn sol_focus_carries_its_source_and_its_warp() {
        let mut scripts = plain(
            "focus-warp",
            r#"
            sol.bind("super+a", function()
                sol.focus(3, { warp = { to = { x = 0.25, y = 0.75 }, motion = { duration = 300, easing = "linear" } } })
            end)
            sol.bind("super+b", function() sol.focus(4, { warp = true }) end)
            sol.bind("super+c", function() sol.focus(5) end)
            sol.bind("super+d", function() sol.focus(6, { warp = { motion = {} } }) end)
            sol.bind("super+e", function()
                local ok, err = pcall(sol.focus, 7, { warp = "yes" })
                sol.status(tostring(ok) .. ": " .. tostring(err))
            end)
            "#,
        );
        let focus = |scripts: &mut Scripts, combo: &str| {
            scripts
                .key(combo, Snapshot::default())
                .commands
                .into_iter()
                .filter_map(|command| match command {
                    Command::Focus { id, source, warp } => Some((id, source, warp)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            [
                focus(&mut scripts, "super+a"),
                focus(&mut scripts, "super+b"),
                focus(&mut scripts, "super+c"),
                focus(&mut scripts, "super+d"),
            ],
            [
                vec![(
                    3,
                    FocusSource::Binding,
                    Some(to_window(
                        3,
                        (0.25, 0.75),
                        Some(AnimationSpec {
                            duration: std::time::Duration::from_millis(300),
                            easing: Curve::Linear,
                        }),
                    )),
                )],
                vec![(4, FocusSource::Binding, Some(to_window(4, CENTRE, None)))],
                vec![(5, FocusSource::Binding, None)],
                vec![(
                    6,
                    FocusSource::Binding,
                    Some(to_window(6, CENTRE, Some(AnimationSpec::default()))),
                )],
            ]
        );
        let refused = scripts
            .key("super+e", Snapshot::default())
            .status
            .unwrap_or_default();
        assert!(
            refused.starts_with("false: ") && refused.contains("warp is true, false or a table"),
            "{refused:?}"
        );
    }

    /// **`sol.warp` names a window or a point, and a bad one is an error**:
    /// a window's id is its centre unless `to` says, a point is a point with
    /// a motion's duration and the default easing, a spring is a spring, and
    /// the mouse carries on from the pointer mid-flight unless `interrupt`
    /// says `"picture"`; anything but an id or a point, half a point, `to`
    /// with a point, a `to` that is neither `"center"` nor a share, half a
    /// share, a motion that is not a table, a negative id and an `interrupt`
    /// that is neither `"pointer"` nor `"picture"` are errors, and ask for
    /// nothing.
    #[test]
    fn sol_warp_names_a_window_or_a_point_and_a_bad_one_is_an_error() {
        let mut scripts = plain(
            "warp",
            r#"
            sol.bind("super+a", function() sol.warp(5) end)
            sol.bind("super+b", function() sol.warp({ x = 10, y = 20 }, { motion = { duration = 100 } }) end)
            sol.bind("super+c", function() sol.warp(5, { to = "center", motion = { easing = "spring" } }) end)
            sol.bind("super+d", function()
                sol.warp(5, { interrupt = "picture" })
                sol.warp(6, { interrupt = "pointer" })
            end)
            sol.bind("super+e", function()
                local said = {}
                for _, call in ipairs({
                    function() sol.warp("five") end,
                    function() sol.warp({ x = 1 }) end,
                    function() sol.warp({ x = 1, y = 2 }, { to = "center" }) end,
                    function() sol.warp(5, { to = "middle" }) end,
                    function() sol.warp(5, { to = { x = 0.5 } }) end,
                    function() sol.warp(5, { motion = 3 }) end,
                    function() sol.warp(-1) end,
                    function() sol.warp(5, { interrupt = "wherever" }) end,
                }) do
                    said[#said + 1] = tostring((pcall(call)))
                end
                sol.status(table.concat(said, ","))
            end)
            "#,
        );
        let warped = |scripts: &mut Scripts, combo: &str| {
            warps(&scripts.key(combo, Snapshot::default()).commands)
        };
        assert_eq!(
            [
                warped(&mut scripts, "super+a"),
                warped(&mut scripts, "super+b"),
                warped(&mut scripts, "super+c"),
                warped(&mut scripts, "super+d"),
            ],
            [
                vec![to_window(5, CENTRE, None)],
                vec![Warp {
                    to: WarpTo::Point { x: 10.0, y: 20.0 },
                    motion: Some(AnimationSpec {
                        duration: std::time::Duration::from_millis(100),
                        easing: Curve::OutCubic,
                    }),
                    interrupt: Interrupt::Pointer,
                }],
                vec![to_window(
                    5,
                    CENTRE,
                    Curve::from_name("spring").map(|easing| AnimationSpec {
                        easing,
                        ..AnimationSpec::default()
                    }),
                )],
                vec![
                    Warp {
                        interrupt: Interrupt::Picture,
                        ..to_window(5, CENTRE, None)
                    },
                    to_window(6, CENTRE, None),
                ],
            ]
        );
        let outcome = scripts.key("super+e", Snapshot::default());
        assert_eq!(
            (outcome.status.unwrap_or_default(), warps(&outcome.commands)),
            (
                "false,false,false,false,false,false,false,false".to_owned(),
                Vec::new()
            ),
            "every bad call is an error, and none asks for a warp"
        );
    }

    /// Atrium's stage and strip (#449), on `WIDE`, whose strip is the left
    /// 220 pixels with its one card's slot at (22, 657), 176 by 126.
    mod atrium {
        use super::{Command, Desk, FocusSource, Rect, Snapshot, one_screen};
        use std::time::Duration;

        /// [`desk`], with the strip hiding as `hide` says.
        fn desk_hiding(hide: &str) -> Desk {
            let mut desk = desk();
            desk.scripts.evaluate(&format!(
                "require(\"config\").floating.atrium.hide = \"{hide}\"\nreturn \"\""
            ));
            let snapshot = desk.snapshot();
            act(&mut desk, "leave", "{}", snapshot);
            desk
        }

        /// Window 2 on the stage clear of the strip, window 1 the card.
        fn desk() -> Desk {
            let at = |x: f64| Rect {
                x,
                y: 100.0,
                w: 800.0,
                h: 600.0,
            };
            let mut desk = Desk::floating(
                one_screen(),
                &[(2, "DP-1", at(600.0)), (1, "DP-1", at(700.0))],
            );
            let snapshot = desk.snapshot();
            act(&mut desk, "leave", "{}", snapshot);
            desk
        }

        fn act(
            desk: &mut Desk,
            action: &str,
            data: &str,
            snapshot: Snapshot,
        ) -> super::super::Outcome {
            let data = crate::json::Json::parse(data).expect("valid JSON");
            desk.scripts
                .surface_action("atrium@DP-1", action, &data, snapshot)
        }

        fn set_rect(desk: &mut Desk, id: u64, rect: Rect) {
            if let Some(window) = desk.windows.iter_mut().find(|window| window.id == id) {
                window.rect = rect;
            }
        }

        /// Window `id`'s last present in `commands`: its rect, opacity, how
        /// long it moves and whether it named a timing per property.
        fn presented(
            commands: &[Command],
            id: u64,
        ) -> Option<(Option<Rect>, Option<f32>, Duration, bool)> {
            commands.iter().rev().find_map(|command| match command {
                Command::Present {
                    id: each,
                    rect,
                    opacity,
                    animation,
                    channels,
                    ..
                } if *each == id => Some((*rect, *opacity, animation.duration, channels.is_some())),
                _ => None,
            })
        }

        /// **A stage window maximised over the strip sends the strip away,
        /// and its restore brings it back** -- the cards with their icons:
        /// the cards are drawn under the stage but the icons over every
        /// window, so a strip left out under a maximised window showed its
        /// icons on top of it. Answered without `sol.animate`, which in a
        /// `maximize` listener would time the maximise itself.
        #[test]
        fn a_stage_window_maximised_over_the_strip_sends_the_strip_away() {
            let mut desk = desk_hiding("overlap");
            let restored = desk.rect(2);
            set_rect(&mut desk, 2, super::WIDE);
            let outcome = desk.scripts.maximize(2, true, desk.snapshot());
            let card = presented(&outcome.commands, 1).expect("the card is drawn again");
            assert_eq!(card.1, Some(0.0), "the strip slid away: {card:?}");
            assert!(card.3, "the card names its own timing");
            assert!(
                outcome.motion.is_none(),
                "the maximise keeps its own timing: {:?}",
                outcome.motion
            );

            set_rect(&mut desk, 2, restored);
            let outcome = desk.scripts.maximize(2, false, desk.snapshot());
            let card = presented(&outcome.commands, 1).expect("the card is drawn again");
            assert_eq!(card.1, Some(1.0), "the strip is back: {card:?}");
            assert!(outcome.motion.is_none());
        }

        /// **An edge dragged over the strip sends it away**, and an edge
        /// dragged on while it is away draws nothing again.
        #[test]
        fn a_stage_window_resized_over_the_strip_sends_the_strip_away() {
            let mut desk = desk_hiding("overlap");
            let wider = Rect {
                x: 100.0,
                y: 100.0,
                w: 1300.0,
                h: 600.0,
            };
            set_rect(&mut desk, 2, wider);
            let outcome =
                desk.scripts
                    .resized(2, (100.0, 400.0), (Some("left"), None), desk.snapshot());
            let card = presented(&outcome.commands, 1).expect("the card is drawn again");
            assert_eq!(card.1, Some(0.0), "the strip slid away: {card:?}");

            set_rect(
                &mut desk,
                2,
                Rect {
                    x: 90.0,
                    w: 1310.0,
                    ..wider
                },
            );
            let outcome =
                desk.scripts
                    .resized(2, (90.0, 400.0), (Some("left"), None), desk.snapshot());
            assert_eq!(
                presented(&outcome.commands, 1),
                None,
                "nothing changed for the strip"
            );
        }

        /// The held card 1 dragged to `x`, 720, as the strip's scene says,
        /// and ticks of its frame clock, each `dt` seconds; its front window
        /// as last drawn.
        fn ticks(desk: &mut Desk, x: f64, dt: f64, count: usize) -> (Rect, Duration, bool) {
            let snapshot = desk.snapshot();
            act(
                desk,
                "drag",
                &format!(r#"{{"card":1,"x":{x},"y":720}}"#),
                snapshot,
            );
            let mut last = None;
            for _ in 0..count {
                let snapshot = desk.snapshot();
                let outcome = act(desk, "tick", &format!(r#"{{"dt":{dt}}}"#), snapshot);
                last = presented(&outcome.commands, 1).or(last);
            }
            let (rect, _, duration, channels) = last.expect("the held card is drawn each frame");
            (rect.expect("a rect"), duration, channels)
        }

        /// **A card dragged out of the strip is drawn once a frame, on the
        /// pointer while it is flat and easing after it while it is turned**,
        /// as macOS has it: the strip's tick draws it where the scene last
        /// said the drag was -- not at each of the scene's pointer events,
        /// which arrive unevenly between frames -- and no
        /// spring is put on top -- a present's spring starts again from rest
        /// at each new target, and trailed the pointer.
        #[test]
        fn a_dragged_card_follows_the_pointer_flat_and_eases_after_it_turned() {
            let mut desk = desk();
            desk.cursor = (100.0, 720.0);
            let snapshot = desk.snapshot();
            act(
                &mut desk,
                "drag",
                r#"{"card":1,"x":100,"y":720,"px":100,"py":712}"#,
                snapshot,
            );
            let snapshot = desk.snapshot();
            let again = act(
                &mut desk,
                "drag",
                r#"{"card":1,"x":300,"y":720,"px":100,"py":712}"#,
                snapshot,
            );
            assert_eq!(
                presented(&again.commands, 1),
                None,
                "a pointer event draws nothing"
            );

            // Pulled well past the strip: a window at its own size, carried
            // by its centre, under the pointer.
            let (window, duration, channels) = ticks(&mut desk, 900.0, 0.05, 20);
            assert_eq!(
                (duration, channels),
                (Duration::ZERO, false),
                "drawn, not animated"
            );
            assert_eq!((window.w, window.h), (800.0, 600.0));
            assert!(
                (window.x + 400.0 - 900.0).abs() < 0.5 && (window.y + 300.0 - 720.0).abs() < 0.5,
                "carried by its centre: {window:?}"
            );
            // Flat, it eases after the pointer only by `steady`: part of the
            // way in one 4 ms frame, the rest within a few more.
            let (step, ..) = ticks(&mut desk, 960.0, 0.004, 1);
            assert!(
                step.x > window.x + 5.0 && step.x < window.x + 60.0,
                "{window:?} then {step:?}"
            );
            let (moved, ..) = ticks(&mut desk, 960.0, 0.05, 5);
            assert!(
                (moved.x - window.x - 60.0).abs() < 0.5,
                "{window:?} then {moved:?}"
            );

            // Back towards the strip, turning: one frame later it is still
            // nearly where it was, and it gets there over the frames after.
            let (eased, ..) = ticks(&mut desk, 400.0, 0.004, 1);
            assert!(
                eased.w > 790.0 && eased.x > moved.x - 200.0,
                "eased after the pointer: {eased:?}"
            );
            let (arrived, ..) = ticks(&mut desk, 400.0, 0.05, 40);
            assert!(
                arrived.w < 700.0,
                "turned the way the pointer's place says: {arrived:?}"
            );
        }

        /// **A hovered card picked up does not move at all**: lifted, out
        /// towards the stage and flat, held by the point the pointer is on
        /// as the drag begins -- not snapped back to its slot's size, nor
        /// hopping by the drag threshold to where the press began.
        #[test]
        fn a_hovered_card_picked_up_does_not_jump() {
            let mut desk = desk();
            // The strip waits off the edge while the stage holds a window, so
            // a card can only be hovered once the pointer has brought it out.
            let snapshot = desk.snapshot();
            act(&mut desk, "edge", "{}", snapshot);
            let snapshot = desk.snapshot();
            let hovered = act(&mut desk, "hover", r#"{"card":1}"#, snapshot);
            let lifted = presented(&hovered.commands, 1)
                .expect("the hovered card is drawn")
                .0
                .expect("a rect");
            desk.cursor = (108.0, 720.0);
            let snapshot = desk.snapshot();
            let picked = act(
                &mut desk,
                "drag",
                r#"{"card":1,"x":108,"y":720,"px":100,"py":720}"#,
                snapshot,
            );
            let held = presented(&picked.commands, 1)
                .expect("the picked card is drawn")
                .0
                .expect("a rect");
            for (was, is) in [
                (lifted.x, held.x),
                (lifted.y, held.y),
                (lifted.w, held.w),
                (lifted.h, held.h),
            ] {
                assert!((was - is).abs() < 0.5, "lifted {lifted:?}, held {held:?}");
            }
        }

        /// **While the stage holds a window the strip waits off the edge**,
        /// comes out over the windows while the pointer is at the edge, and
        /// slides away again on its softer spring once the pointer has left
        /// it (the scene sends `leave` half a second after).
        #[test]
        fn the_strip_waits_at_the_edge_while_the_stage_holds_a_window() {
            let mut desk = desk();
            let snapshot = desk.snapshot();
            let away = act(&mut desk, "leave", "{}", snapshot);
            let card = presented(&away.commands, 1).expect("the card is drawn");
            assert_eq!(
                (card.1, card.3),
                (Some(0.0), true),
                "off the edge, on its own spring: {card:?}"
            );
            let snapshot = desk.snapshot();
            let out = act(&mut desk, "edge", "{}", snapshot);
            let card = presented(&out.commands, 1).expect("the card is drawn");
            assert_eq!(card.1, Some(1.0), "out at the edge: {card:?}");
            let z = out.commands.iter().rev().find_map(|command| match command {
                Command::Present { id: 1, z, .. } => Some(*z),
                _ => None,
            });
            assert!(
                z.is_some_and(|z| z > 0.0),
                "over the stage's windows: {z:?}"
            );
        }

        /// **A card dropped onto another card joins its group**, in front,
        /// settling over its windows, and the stage is left as it is. Two cards, so their places
        /// are 562 and 752 down, 176 by 126 at 22 across.
        #[test]
        fn a_card_dropped_onto_another_card_joins_its_group() {
            let at = |x: f64| Rect {
                x,
                y: 100.0,
                w: 800.0,
                h: 600.0,
            };
            let mut desk = Desk::floating(
                one_screen(),
                &[
                    (3, "DP-1", at(600.0)),
                    (2, "DP-1", at(700.0)),
                    (1, "DP-1", at(800.0)),
                ],
            );
            let snapshot = desk.snapshot();
            act(&mut desk, "leave", "{}", snapshot);
            assert_eq!(groups(&desk), "3|2|1");

            desk.cursor = (100.0, 620.0);
            let snapshot = desk.snapshot();
            act(&mut desk, "drag", r#"{"card":1,"x":100,"y":620}"#, snapshot);
            let snapshot = desk.snapshot();
            act(&mut desk, "drag", r#"{"card":1,"x":100,"y":800}"#, snapshot);
            let snapshot = desk.snapshot();
            let over = act(&mut desk, "tick", r#"{"dt":0.004}"#, snapshot);
            assert!(
                presented(&over.commands, 1).is_some(),
                "the card it would join comes forward: {:?}",
                over.commands
            );
            let snapshot = desk.snapshot();
            act(&mut desk, "drop", r#"{"card":1,"x":100,"y":800}"#, snapshot);
            assert_eq!(
                groups(&desk),
                "3|2,1",
                "one group, the dropped card in front, the stage as it was"
            );
        }

        /// **A stage window dropped onto a card joins that card's group**, in
        /// front, the stage left bare behind it -- not a card of its own.
        /// One card, so its place is 657 down, 176 by 126 at 22 across.
        #[test]
        fn a_stage_window_dropped_onto_a_card_joins_its_group() {
            let mut desk = desk();
            desk.drop(2, "DP-1", (100.0, 700.0));
            assert_eq!(groups(&desk), "|2,1", "a bare stage and one card of both");
        }

        /// **A stage window dropped on the strip away from every card is a
        /// card of its own**, just under the stage.
        #[test]
        fn a_stage_window_dropped_between_cards_is_a_card_of_its_own() {
            let mut desk = desk();
            desk.drop(2, "DP-1", (100.0, 300.0));
            assert_eq!(groups(&desk), "|2|1");
        }

        /// **A window let go into a card stays where it was let go until
        /// it comes back.** Moved out of the strip's way there and then,
        /// while it was drawn as a card, it glided to that place at its own
        /// size and its card was undone; it is moved clear of the strip as
        /// it comes back to the stage instead, on the glide that brings it
        /// from its card, just past the strip's inner edge.
        #[test]
        fn a_window_let_go_into_a_card_is_moved_clear_of_the_strip_as_it_comes_back() {
            let mut desk = desk();
            desk.drop(2, "DP-1", (100.0, 700.0));
            assert_eq!(groups(&desk), "|2,1");
            let dropped = desk.rect(2);
            assert_eq!(
                (dropped.x, dropped.y),
                (-300.0, 400.0),
                "where it was let go"
            );

            let snapshot = desk.snapshot();
            let outcome = act(&mut desk, "pick", r#"{"card":1}"#, snapshot);
            desk.apply(&outcome.commands);
            assert!(groups(&desk).starts_with("2,1"), "{}", groups(&desk));
            let back = desk.rect(2);
            assert_eq!(
                (back.x, back.y, back.w, back.h),
                (244.0, 400.0, 800.0, 600.0),
                "clear of the strip, at its own size"
            );
            assert_eq!(desk.rect(1).x, 700.0, "the other was clear of it");
        }

        /// **A card turns no faster than `drag.turn` lets it**: pulled far
        /// out of the strip in one 50 ms frame it has turned a third of the
        /// way into its windows, where it would be all the way, the eased
        /// point already past `drag.reach`; and with `turn = 0`, it is.
        #[test]
        fn a_card_pulled_out_in_one_frame_turns_over_several() {
            let pulled = |turn: Option<u32>| {
                let mut desk = desk();
                if let Some(turn) = turn {
                    desk.scripts.evaluate(&format!(
                        "require(\"config\").floating.atrium.drag.turn = {turn}\nreturn \"\""
                    ));
                }
                desk.cursor = (100.0, 720.0);
                let snapshot = desk.snapshot();
                act(&mut desk, "drag", r#"{"card":1,"x":100,"y":720}"#, snapshot);
                ticks(&mut desk, 1200.0, 0.05, 1).0
            };
            let shipped = pulled(None);
            assert!(
                shipped.w > 300.0 && shipped.w < 500.0,
                "a third of the way: {shipped:?}"
            );
            assert_eq!(pulled(Some(0)).w, 800.0, "all of the way at once");
        }

        /// [`desk`], with the strip a row along the bottom, which on `WIDE`
        /// is six cards' places, 230 apart, from 590 to 1970 across, its
        /// inner edge 1220 down.
        fn desk_along_the_bottom(hide: &str) -> Desk {
            let mut desk = desk();
            desk.scripts.evaluate(&format!(
                "local atrium = require(\"config\").floating.atrium\n\
                 atrium.side = \"bottom\"\n\
                 atrium.hide = \"{hide}\"\n\
                 return \"\""
            ));
            let snapshot = desk.snapshot();
            act(&mut desk, "leave", "{}", snapshot);
            desk
        }

        /// **Along the bottom a card stands in the order its group was last
        /// used, the most recent on the left**, the row centred: a card
        /// coming or going only slides the others along. From the middle
        /// out, one side and then the other, a card leaving sent every card
        /// after it across to the other side, through the cards there.
        #[test]
        fn along_the_bottom_the_most_recent_card_is_on_the_left() {
            let at = |x: f64| Rect {
                x,
                y: 100.0,
                w: 800.0,
                h: 600.0,
            };
            let mut desk = Desk::floating(
                one_screen(),
                &[
                    (4, "DP-1", at(600.0)),
                    (3, "DP-1", at(700.0)),
                    (2, "DP-1", at(800.0)),
                    (1, "DP-1", at(900.0)),
                ],
            );
            desk.scripts.evaluate(
                "local atrium = require(\"config\").floating.atrium\n\
                 atrium.side = \"bottom\"\n\
                 atrium.hide = \"never\"\n\
                 return \"\"",
            );
            let snapshot = desk.snapshot();
            let outcome = act(&mut desk, "leave", "{}", snapshot);
            assert_eq!(groups(&desk), "4|3|2|1");
            let card = |id: u64| {
                presented(&outcome.commands, id)
                    .and_then(|drawn| drawn.0)
                    .expect("the card is drawn")
            };
            let (three, two, one) = (card(3), card(2), card(1));
            assert!(
                three.x < two.x && two.x < one.x,
                "{three:?} {two:?} {one:?}"
            );
            let middle = (three.x + one.x + one.w) / 2.0;
            assert!((middle - 1280.0).abs() < 1.0, "centred: {middle}");
        }

        /// **A window dragged beside the row along the bottom is not turned
        /// into a card**: past the row's ends the way along counts as well
        /// as the way up, so a window dragged to a side or a bottom corner
        /// to snap there stays a window, and one dragged down onto the row
        /// turns. Measured up from the strip alone, a window dragged to the
        /// right edge to snap half the screen leaned back as it got there.
        #[test]
        fn a_window_dragged_beside_the_row_along_the_bottom_is_not_turned() {
            let desk = desk_along_the_bottom("never");
            let closeness = |x: f64, y: f64| -> f64 {
                desk.scripts
                    .evaluate(&format!(
                        "local area = require(\"monitors\").named(\"DP-1\")\n\
                         return tostring(require(\"atrium\").closeness(area, {x}, {y}))"
                    ))
                    .parse()
                    .expect("a number")
            };
            let over = closeness(1280.0, 1120.0);
            assert!(over > 0.6 && over < 0.8, "over the row: {over}");
            assert_eq!(closeness(2555.0, 1120.0), 0.0, "at the right edge");
            assert_eq!(closeness(2500.0, 1435.0), 0.0, "in the bottom corner");
            assert_eq!(closeness(5.0, 1300.0), 0.0, "at the left edge");
        }

        /// **Along the bottom too, a hovered card picked up does not move**:
        /// it is lifted up out of the row about its bottom edge, and held as
        /// it is drawn. It was held as the strip on the left lifts one,
        /// sideways, and hopped there as the drag began.
        #[test]
        fn a_hovered_card_picked_up_along_the_bottom_does_not_jump() {
            let mut desk = desk_along_the_bottom("stage");
            let snapshot = desk.snapshot();
            act(&mut desk, "edge", "{}", snapshot);
            let snapshot = desk.snapshot();
            let hovered = act(&mut desk, "hover", r#"{"card":1}"#, snapshot);
            let lifted = presented(&hovered.commands, 1)
                .expect("the hovered card is drawn")
                .0
                .expect("a rect");
            // The strip's scene begins `strip` and `drag.reach` above the
            // bottom: 1440 - 220 - 320.
            let (x, y) = (lifted.x + lifted.w / 2.0, lifted.y + lifted.h / 2.0);
            desk.cursor = (x, y);
            let snapshot = desk.snapshot();
            let picked = act(
                &mut desk,
                "drag",
                &format!(r#"{{"card":1,"x":{x},"y":{}}}"#, y - 900.0),
                snapshot,
            );
            let held = presented(&picked.commands, 1)
                .expect("the picked card is drawn")
                .0
                .expect("a rect");
            for (was, is) in [
                (lifted.x, held.x),
                (lifted.y, held.y),
                (lifted.w, held.w),
                (lifted.h, held.h),
            ] {
                assert!((was - is).abs() < 0.5, "lifted {lifted:?}, held {held:?}");
            }
        }

        /// The stage's group and the cards behind it, as `"3,2|1"`: each
        /// group's windows front first, the stage first.
        fn groups(desk: &Desk) -> String {
            desk.scripts.evaluate(
                "local out = {}\n\
                 for _, groups in pairs(sol.keep(\"atrium\", {}).desks) do\n\
                     for _, group in ipairs(groups) do out[#out + 1] = table.concat(group, \",\") end\n\
                 end\n\
                 return table.concat(out, \"|\")",
            )
        }

        fn closing(desk: &mut Desk, id: u64) {
            if let Some(window) = desk.windows.iter_mut().find(|window| window.id == id) {
                window.leaving = true;
            }
            let snapshot = desk.snapshot();
            let outcome = desk.scripts.closing(id, snapshot);
            desk.apply(&outcome.commands);
        }

        /// **A window closed on the stage leaves the stage as it was**: the
        /// keyboard the close hands to a card's window (floating's
        /// `after_close` is the window used before, most often a card's)
        /// goes to the stage's next window instead, and no card comes
        /// forward.
        #[test]
        fn a_window_closed_on_the_stage_keeps_the_stage() {
            let mut desk = desk();
            desk.windows.insert(
                0,
                super::window(
                    3,
                    "DP-1",
                    Rect {
                        x: 640.0,
                        y: 140.0,
                        w: 800.0,
                        h: 600.0,
                    },
                ),
            );
            for window in &mut desk.windows {
                if window.id != 1 {
                    window.app_id = "editor".to_owned();
                }
            }
            let snapshot = desk.snapshot();
            act(&mut desk, "leave", "{}", snapshot);
            assert_eq!(
                groups(&desk),
                "2,3|1",
                "the editor's two windows on the stage"
            );

            closing(&mut desk, 2);
            let snapshot = desk.snapshot();
            let outcome = desk.scripts.focused(1, FocusSource::Settle, true, snapshot);
            assert!(
                outcome
                    .commands
                    .iter()
                    .any(|command| matches!(command, Command::Focus { id: 3, .. })),
                "the keyboard goes to the stage's next window: {:?}",
                outcome.commands
            );
            assert_eq!(groups(&desk), "3|1", "the card stays a card");
        }

        /// **The stage's last window closed leaves the stage bare**, the
        /// desktop showing, until a card is picked or a window opens -- not
        /// the next card brought forward in its place.
        #[test]
        fn the_stages_last_window_closed_leaves_it_bare() {
            let mut desk = desk();
            closing(&mut desk, 2);
            assert_eq!(groups(&desk), "|1", "a bare stage and the card behind it");
            let snapshot = desk.snapshot();
            desk.scripts.focused(1, FocusSource::Settle, true, snapshot);
            assert_eq!(
                groups(&desk),
                "|1",
                "the keyboard handed over brings nothing forward"
            );
        }
    }
}

/// `sol.bind`'s options (#224): the third argument is a note, or a table that
/// holds the note and what else the binding asks for.
#[cfg(test)]
mod binding_options {
    use super::Scripts;

    /// `text` loaded as a configuration of its own, or what refused it.
    fn load(name: &str, text: &str) -> anyhow::Result<Scripts> {
        let directory = std::env::temp_dir().join(format!(
            "solium-binding-options-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&directory);
        let entry = directory.join("init.lua");
        std::fs::write(&entry, text).expect("writing the test script");
        let loaded = Scripts::load(&entry);
        let _ = std::fs::remove_dir_all(&directory);
        loaded
    }

    /// **A binding takes its note and what it asks for as one table**, a
    /// plain note as it always has, and a binding bound again with nothing
    /// asked asks for nothing: the later call wins whole.
    #[test]
    fn a_binding_takes_its_note_and_what_it_asks_for_as_a_table() {
        let scripts = load(
            "table",
            r#"
            local function nothing() end
            sol.bind("XF86AudioRaiseVolume", nothing,
                { note = "laptop keys", repeating = true, locked = true })
            sol.bind("XF86AudioMute", nothing, { locked = true })
            sol.bind("super+q", nothing, "a plain note")
            sol.bind("super+w", nothing)
            sol.bind("XF86AudioLowerVolume", nothing, { repeating = true, locked = true })
            sol.bind("XF86AudioLowerVolume", nothing)
            "#,
        )
        .expect("loading the bindings");
        let reported: Vec<(String, Option<String>, bool, bool)> = scripts
            .bindings()
            .into_iter()
            .map(|binding| {
                (
                    binding.combo,
                    binding.note,
                    binding.repeating,
                    binding.locked,
                )
            })
            .collect();
        let row = |combo: &str, note: Option<&str>, repeating, locked| {
            (combo.to_owned(), note.map(str::to_owned), repeating, locked)
        };
        assert_eq!(
            reported,
            vec![
                row("super+q", Some("a plain note"), false, false),
                row("super+w", None, false, false),
                row("xf86audiolowervolume", None, false, false),
                row("xf86audiomute", None, false, true),
                row("xf86audioraisevolume", Some("laptop keys"), true, true),
            ],
            "(combination, note, repeating, locked)"
        );
    }

    /// **An option a binding does not have is refused, by name**, and so is
    /// one that is not true or false. `repeat` is Lua's own word and can only
    /// be written as `["repeat"]`, which is the likeliest slip of all, so its
    /// refusal names the spelling that works.
    #[test]
    fn an_option_a_binding_does_not_have_is_refused_by_name() {
        for (case, options, says) in [
            ("repeat", r#"{ ["repeat"] = true }"#, "repeating"),
            ("unknown", "{ loud = true }", "loud"),
            ("not-a-boolean", r#"{ repeating = "yes" }"#, "repeating"),
            ("note-not-text", "{ note = 3 }", "note"),
        ] {
            let error = load(
                case,
                &format!("sol.bind(\"super+q\", function() end, {options})\n"),
            )
            .err()
            .map(|error| format!("{error:#}"))
            .unwrap_or_default();
            assert!(
                error.contains(says),
                "{case}: the configuration was not refused naming {says:?}: {error:?}"
            );
        }
    }
}
