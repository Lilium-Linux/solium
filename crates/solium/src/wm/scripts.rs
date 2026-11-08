//! The window manager as the compositor holds it: what it loaded with, what
//! it declared, and what it was last told.

use std::path::Path;

use anyhow::Result;
use solium_wm_protocol as wire;

use std::rc::Rc;

use crate::script::lua::{Ask, Host};
use crate::script::*;
use crate::wm::events::Answers;
use crate::wm::stores::StoreReads;

/// The window manager, as the compositor talks to it: in this process for
/// now, the Lua host.
#[derive(Debug)]
pub(crate) struct Scripts {
    pub(crate) host: Host,
    /// What the window manager was last told of the desktop, so each batch
    /// tells it only what changed.
    pub(crate) told: crate::wm::diff::Told,
    /// The last batch's serial.
    pub(crate) serial: u64,
    /// What the configuration answered its load with: the settings and
    /// requests its top level made, which [`Scripts::startup`] hands over
    /// and the readers of the load (`qml`, `session`, the effect rules)
    /// read.
    pub(crate) loaded: wire::Answer,
    /// Whether [`Scripts::startup`] has handed `loaded` over.
    pub(crate) started: bool,
    /// What the window manager last declared: its bindings, which the key
    /// filter asks about on every key, and what `--check` reports.
    pub(crate) declared: wire::Declarations,
    /// The store files read for the window manager.
    pub(crate) stores: Rc<StoreReads>,
    /// The stores read as it loaded, handed over with [`Self::startup`].
    loaded_stores: Vec<Command>,
}

impl Scripts {
    /// Load the configuration, cold.
    pub(crate) fn load(config: &Path) -> Result<Self> {
        Self::load_reading(config, StoreReads::default())
    }

    /// Load the configuration for `solium --check`: as [`Self::load`] does,
    /// except that nothing of the user's is changed, so a store's broken
    /// file stays where it is and is said in [`Self::check_notes`] (#473).
    pub(crate) fn load_for_check(config: &Path) -> Result<Self> {
        Self::load_reading(config, StoreReads::checking())
    }

    fn load_reading(config: &Path, stores: StoreReads) -> Result<Self> {
        let stores = Rc::new(stores);
        let (host, loaded) = Host::load(
            config,
            Box::new(Answers {
                snapshot: None,
                stores: Rc::clone(&stores),
            }),
        )?;
        let loaded_stores = stores.take_read();
        let mut scripts = Self {
            host,
            told: crate::wm::diff::Told::new(),
            serial: 0,
            loaded: wire::Answer {
                serial: 0,
                handled: false,
                commands: Vec::new(),
                motion: None,
                decision: None,
            },
            started: false,
            declared: wire::Declarations {
                bindings: Vec::new(),
                unbound: Vec::new(),
                unknown: Vec::new(),
                notes: Vec::new(),
            },
            stores,
            loaded_stores,
        };
        scripts.declarations_in(&loaded);
        scripts.loaded = loaded;
        Ok(scripts)
    }

    /// Who answers the window manager's questions: the compositor, over
    /// `snapshot` when a dispatch has one.
    pub(crate) fn answers(&self, snapshot: Option<Snapshot>) -> Box<dyn Ask> {
        Box::new(Answers {
            snapshot,
            stores: Rc::clone(&self.stores),
        })
    }

    /// Load the configuration again, the window manager carrying what it
    /// kept ([`Host::reload`]); [`Self::startup`] then hands over what the
    /// new one asked for. One that fails to load leaves everything as it
    /// was.
    pub(crate) fn reload(&mut self, config: &Path) -> Result<()> {
        let answers = self.answers(None);
        let loaded = self.host.reload(config, answers)?;
        self.loaded_stores = self.stores.take_read();
        self.declared = wire::Declarations {
            bindings: Vec::new(),
            unbound: Vec::new(),
            unknown: Vec::new(),
            notes: Vec::new(),
        };
        self.declarations_in(&loaded);
        self.loaded = loaded;
        self.started = false;
        Ok(())
    }

    /// Where the configuration is. See [`crate::script::lua::config_path`].
    pub(crate) fn config_path() -> std::path::PathBuf {
        crate::script::lua::config_path()
    }

    /// Where a user's own scripts live, whether or not they have any.
    pub(crate) fn user_config_dir() -> Option<std::path::PathBuf> {
        crate::script::lua::user_config_dir()
    }

    /// Bytes the window manager's Lua is holding. See [`Host::used_memory`].
    pub(crate) fn used_memory(&self) -> usize {
        self.host.used_memory()
    }

    /// The window manager's stores, as it would save them.
    #[cfg(test)]
    pub(crate) fn stores(&self) -> Vec<(String, crate::json::Json)> {
        self.host.stores()
    }

    /// What the configuration asked for as it loaded: its top level runs
    /// `sol` functions before any event. Handed over once.
    pub(crate) fn startup(&mut self) -> Outcome {
        // Handed over once: a second call would apply it all again.
        if std::mem::replace(&mut self.started, true) {
            return Outcome::default();
        }
        let mut outcome = crate::wm::events::outcome_of(self.loaded.clone());
        outcome
            .commands
            .splice(0..0, std::mem::take(&mut self.loaded_stores));
        outcome
    }

    /// Keep what an answer declares: the key filter and `--check` ask
    /// these, not the window manager.
    pub(crate) fn declarations_in(&mut self, answer: &wire::Answer) {
        if let Some(declared) = answer
            .commands
            .iter()
            .rev()
            .find_map(|command| match command {
                wire::Command::Declare(declared) => Some(declared),
                _ => None,
            })
        {
            self.declared = declared.clone();
        }
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
        let mut notes = self.declared.notes.clone();
        notes.extend(self.stores.notes());
        notes
    }
}
