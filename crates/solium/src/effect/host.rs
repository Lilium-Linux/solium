//! The effect host: where effect folders are, what was loaded from them, and
//! what went wrong, by file and line.
//!
//! An effect is `effects/<name>/effect.lua` and the files beside it (\[16\] §2).
//! The user's folder shadows the shipped one name by name, as a pane style's
//! does (`style::resolve`): `tests::a_user_folder_shadows_the_shipped_one`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use solium_effects::glsl::{self, Sources};
use solium_effects::spec::{EffectSpec, GridSpec, Rung, Severity, Value};
use solium_effects::stage::{Binding, Plan, Stage};

use super::geometry::{self, Refusal};
use super::sandbox::{Budget, Sandbox};
use super::settings::Caps;

/// Where effect folders are looked for: the user's, then the shipped.
#[derive(Clone, Debug)]
pub(crate) struct Library {
    user: Option<PathBuf>,
    shipped: PathBuf,
}

impl Library {
    /// This machine's: `~/.config/solium/effects/`, then the build's or the
    /// install's (`assets::effects`).
    pub(crate) fn new() -> Self {
        Self::with(
            crate::script::Scripts::user_config_dir().map(|dir| dir.join("effects")),
            crate::assets::effects(),
        )
    }

    pub(crate) fn with(user: Option<PathBuf>, shipped: PathBuf) -> Self {
        Self { user, shipped }
    }

    /// The folder the effect called `name` is in, or `None`.
    ///
    /// A bare name only (`tests::an_effect_name_is_a_bare_name`), and a folder
    /// with no `effect.lua` is skipped so the shipped one still draws
    /// (`tests::a_folder_without_effect_lua_falls_through`).
    pub(crate) fn resolve(&self, name: &str) -> Option<PathBuf> {
        if !is_name(name) {
            return None;
        }
        self.user
            .iter()
            .chain(std::iter::once(&self.shipped))
            .map(|place| place.join(name))
            .find(|folder| folder.join("effect.lua").is_file())
    }

    /// Where a folder's closure (`pixels`, `fallback`, `use`) is looked for when the
    /// user's folders lack it: the shipped folders, everywhere but in tests.
    /// `check::tests::a_user_folder_naming_a_shipped_effect_passes`.
    pub(crate) fn shipped(&self) -> &Path {
        &self.shipped
    }

    /// Every folder in the user's place, sorted: what `--check` reads.
    /// `tests::the_users_folders_are_listed_sorted`.
    pub(crate) fn user_folders(&self) -> Vec<PathBuf> {
        let Some(user) = &self.user else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(user) else {
            return Vec::new();
        };
        let mut folders: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        folders.sort();
        folders
    }
}

/// Whether `name` can name an effect: lower-case letters, digits, `-` and `_`.
/// `tests::an_effect_name_is_a_bare_name`.
pub(crate) fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
}

/// Something wrong with an effect, where it is.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Problem {
    /// The effect's name, `"config"` for the configuration itself, or
    /// `"rules"` for a rule `sol.effects` gave
    /// (`state::tests::a_broken_rule_keeps_the_rules_that_ran`).
    pub(crate) effect: String,
    pub(crate) file: PathBuf,
    pub(crate) line: Option<u32>,
    pub(crate) column: Option<u32>,
    pub(crate) message: String,
    pub(crate) severity: Severity,
}

impl Problem {
    pub(crate) fn error(effect: &str, file: &Path, line: Option<u32>, message: String) -> Self {
        Self {
            effect: effect.to_owned(),
            file: file.to_owned(),
            line,
            column: None,
            message,
            severity: Severity::Error,
        }
    }

    pub(crate) fn warning(effect: &str, file: &Path, message: String) -> Self {
        Self {
            severity: Severity::Warning,
            ..Self::error(effect, file, None, message)
        }
    }
}

/// One effect, loaded: its folder, what its `effect.lua` said, its Lua.
/// `P` is the program type a [`Compiler`] makes: GL's in the compositor, a
/// number in the tests (`tests::a_program_is_compiled_once_per_content`).
#[derive(Debug)]
pub(crate) struct Loaded<P = super::gl::Program> {
    name: String,
    dir: PathBuf,
    spec: EffectSpec,
    sandbox: Sandbox,
    /// The params at their defaults, and the stages `stages` gave for them at
    /// load: a bind at the defaults reads these rather than calling
    /// `stages(p)` again (`tests::a_fallback_naming_another_effect_binds_it_at_load`).
    defaults: Vec<(String, Value)>,
    stages: Vec<Stage>,
    hash: u64,
    /// The effects its `use` stages name at the defaults, loaded with it
    /// (`tests::a_used_effect_is_loaded_with_its_user_and_a_broken_stage_is_refused`).
    used: Vec<String>,
    /// The programs this version needs, by content hash: it swaps in only
    /// once every one compiled
    /// (`tests::a_reload_with_a_broken_effect_keeps_the_one_that_ran`).
    pub(crate) needs: Vec<u64>,
    _program: std::marker::PhantomData<P>,
}

/// An effect bound with a rule's overrides.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Bound {
    pub(crate) params: Vec<(String, Value)>,
    pub(crate) reach: f64,
    pub(crate) bleed: f64,
    pub(crate) warnings: Vec<Problem>,
    /// The configured plan, then one plan per fallback rung, each that
    /// needs a format this GPU lacks dropped ([`Host::bind`]; empty from
    /// [`Loaded::bind`]).
    /// `tests::a_stage_asking_for_rgba16f_where_it_is_missing_takes_the_fallback`.
    pub(crate) plans: Vec<Plan>,
}

impl<P> Loaded<P> {
    /// Load the effect in `dir` into a sandbox of its own, and read its
    /// stages at the defaults: a stage that cannot be read refuses the
    /// version, and the effects its `use`s name are loaded with it.
    /// `tests::the_fixture_effects_load_and_bind_at_their_defaults`,
    /// `tests::a_used_effect_is_loaded_with_its_user_and_a_broken_stage_is_refused`.
    #[cfg(test)]
    pub(crate) fn load(name: &str, dir: &Path) -> Result<Self, Problem> {
        Self::load_with(name, dir, Caps::default())
    }

    /// [`Self::load`], its Lua made with `caps` (`effects.sandbox`):
    /// `tests::a_mesh_stopped_at_load_names_its_budget_and_key`.
    pub(crate) fn load_with(name: &str, dir: &Path, caps: Caps) -> Result<Self, Problem> {
        let file = dir.join("effect.lua");
        let mut sandbox = Sandbox::with_caps(name, &file, caps)?;
        let spec = sandbox.load_effect()?;
        let (defaults, _) = solium_effects::spec::bind(&spec.params, &[])
            .map_err(|message| Problem::error(name, &file, None, message))?;
        let stages = sandbox.stages(&spec, &defaults)?;
        let mut used = Vec::new();
        named_by_use(&stages, &mut used);
        Ok(Self {
            name: name.to_owned(),
            dir: dir.to_owned(),
            spec,
            sandbox,
            defaults,
            stages,
            hash: folder_hash(dir),
            used,
            needs: Vec::new(),
            _program: std::marker::PhantomData,
        })
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    pub(crate) fn spec(&self) -> &EffectSpec {
        &self.spec
    }

    pub(crate) fn sandbox(&self) -> &Sandbox {
        &self.sandbox
    }

    /// The params at their defaults, as a bind with no overrides gives them.
    pub(crate) fn defaults(&self) -> &[(String, Value)] {
        &self.defaults
    }

    pub(crate) fn hash(&self) -> u64 {
        self.hash
    }

    /// The effects its plans splice in: those its `use` stages name at the
    /// defaults, and a `fallback` naming another effect
    /// (`tests::an_effect_changed_under_its_user_gives_its_old_program_back`).
    fn splices(&self) -> impl Iterator<Item = &String> {
        self.used
            .iter()
            .chain(self.spec.fallback.iter().filter_map(|rung| match rung {
                Rung::Effect(name) => Some(name),
                Rung::Params(_) => None,
            }))
    }

    /// The stages for `params` as bound: those read at load when they are
    /// the defaults, else what `stages(p)` gives now. At load and at bind,
    /// never per frame. `tests::a_fallback_naming_another_effect_binds_it_at_load`.
    pub(crate) fn stages(&self, params: &[(String, Value)]) -> Result<Vec<Stage>, Problem> {
        if params == self.defaults.as_slice() {
            return Ok(self.stages.clone());
        }
        self.sandbox.stages(&self.spec, params)
    }

    /// Bind with `overrides`: refused for an unknown param or a wrong kind,
    /// clamped with a warning out of range, then `reach` and `bleed` for them.
    /// `tests::the_fixture_effects_load_and_bind_at_their_defaults`,
    /// `tests::binding_names_the_effect_and_warns_of_a_clamp`.
    pub(crate) fn bind(&self, overrides: &[(String, Value)]) -> Result<Bound, Problem> {
        let file = self.dir.join("effect.lua");
        let (params, warnings) = solium_effects::spec::bind(&self.spec.params, overrides)
            .map_err(|message| Problem::error(&self.name, &file, None, message))?;
        let reach = self.sandbox.extent("reach", &params)?;
        let bleed = self.sandbox.extent("bleed", &params)?;
        let warnings = warnings
            .into_iter()
            .map(|message| Problem::warning(&self.name, &file, message))
            .collect();
        Ok(Bound {
            params,
            reach,
            bleed,
            warnings,
            plans: Vec::new(),
        })
    }
}

/// The effects `use` stages name, anywhere in `stages`, each once
/// (`tests::the_fixture_effects_load_and_bind_at_their_defaults`).
fn named_by_use(stages: &[Stage], into: &mut Vec<String>) {
    for stage in stages {
        match stage {
            Stage::Use { effect, .. } => {
                if !into.contains(effect) {
                    into.push(effect.clone());
                }
            }
            Stage::Repeat { body, .. } | Stage::State { body, .. } => named_by_use(body, into),
            Stage::Pass { .. } | Stage::Save(_) | Stage::Get(_) => {}
        }
    }
}

/// A folder's identity: every file in it, sorted by name, its name and its
/// bytes two parts of `glsl::content_hash`, the hash programs are keyed by.
/// `tests::a_folders_hash_is_its_files`.
pub(crate) fn folder_hash(dir: &Path) -> u64 {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.is_file())
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    let parts: Vec<Vec<u8>> = files
        .iter()
        .flat_map(|file| {
            let name = file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned().into_bytes())
                .unwrap_or_default();
            [name, std::fs::read(file).unwrap_or_default()]
        })
        .collect();
    let slices: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
    solium_effects::glsl::content_hash(&slices)
}

/// What compiles a program: GL in the compositor, a counter in the tests
/// (`tests::Counting`).
pub(crate) trait Compiler {
    type Program: Clone;
    fn compile(&mut self, vertex: &str, sources: &Sources) -> Result<Self::Program, String>;
    fn delete(&mut self, program: Self::Program);
    /// How many lines lower than GLSL ES 1.00's `#line` rule this driver
    /// numbers the user's file (`glsl::line_shift`): asked once, after the
    /// first compile that fails
    /// (`tests::a_driver_numbering_from_0_after_line_is_calibrated_once`).
    fn line_shift(&mut self) -> u32 {
        0
    }
}

/// An effect version: which folder, and which load of it. A reload that
/// changes it gives a new generation, and an old id resolves to nothing;
/// one that reads it as it was, under new caps, keeps the id.
/// `tests::a_new_version_is_a_new_generation`,
/// `tests::new_caps_keep_an_unchanged_folders_id`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct EffectId {
    index: u32,
    generation: u32,
}

impl EffectId {
    /// The id of no effect: a `sol.present` geometry whose folder is
    /// missing carries it, and [`Host::by_id`] answers nothing for it, since
    /// a slot's index starts at 1, so the geometry follows its `failed`
    /// (`state::tests::real_client::a_present_deform_is_a_file`).
    pub(crate) const NONE: Self = Self {
        index: 0,
        generation: 0,
    };

    /// An id a test makes up, naming no slot of any host.
    #[cfg(test)]
    pub(crate) const fn for_test(index: u32) -> Self {
        Self {
            index,
            generation: u32::MAX,
        }
    }
}

/// A program asked for and not compiled yet, and whose log it would be.
#[derive(Debug)]
struct Asked {
    vertex: &'static str,
    sources: Sources,
    effect: String,
    frag: PathBuf,
}

/// One effect's versions: the one that runs and the one waiting to compile.
/// (Not `Slot`: `rules::Slot` is the only one, Ruling 2.)
#[derive(Debug)]
struct Versions<P> {
    index: u32,
    generation: u32,
    current: Option<Rc<Loaded<P>>>,
    /// The programs the running version holds, which `drop_unused` keeps:
    /// its `needs` when it swapped in, bound again when an effect its plans
    /// splice in changed under it
    /// (`tests::an_effect_changed_under_its_user_gives_its_old_program_back`).
    holds: Vec<u64>,
    pending: Option<Loaded<P>>,
    /// [`Host::revive`] gave the running version up: its folder changed
    /// since it loaded, or loading it again failed. It is not tried again,
    /// and its folder not read, until a reload or a new version
    /// (`tests::a_rebuild_that_fails_is_a_problem_and_waits_for_a_reload`).
    given_up: bool,
}

/// Every effect the configuration wants, loaded GPU-free at config load and
/// compiled at the top of the next `prepare` (Ruling 7).
/// `tests::a_cold_effect_is_ready_after_the_first_compile`.
#[derive(Debug)]
pub(crate) struct Host<P = super::gl::Program> {
    library: Library,
    /// Who wants what: `"rules"`, `"on"`, `"style"`, `"present"`, `"check"`.
    wanted: BTreeMap<&'static str, BTreeSet<String>>,
    slots: BTreeMap<String, Versions<P>>,
    next_index: u32,
    asked: HashMap<u64, Asked>,
    programs: HashMap<u64, Result<P, Arc<str>>>,
    /// Programs bound plans hold beyond what each running version holds (a
    /// rule's params can pick other steps; Tasks 9 and 14 add them through
    /// [`Self::hold`]), so `drop_unused` keeps them.
    held: BTreeSet<u64>,
    /// A program may be referenced by nothing now (a version went, or what is
    /// held changed), so the next `compile_pending` sweeps, even with nothing
    /// else to do (`tests::an_effect_no_longer_wanted_gives_its_program_back`).
    sweep: bool,
    /// The compiler's [`Compiler::line_shift`], once a compile has failed.
    line_shift: Option<u32>,
    /// What every effect's Lua is made with, `effects.sandbox`: a version
    /// loaded under other caps is loaded again
    /// (`tests::new_caps_load_every_wanted_folder_again`).
    caps: Caps,
    /// What this GPU renders into: `None` until the first `prepare` probes
    /// it, and then unknown rather than missing, so every rung is kept
    /// (Ruling 11, `tests::a_stage_asking_for_rgba16f_where_it_is_missing_takes_the_fallback`).
    formats: Option<crate::pool::Formats>,
    problems: Vec<Problem>,
    generation: u64,
    /// Loads that read a folder: what `a_reload_loads_each_effect_once` counts.
    #[cfg(test)]
    pub(crate) loads: u32,
}

impl<P: Clone> Host<P> {
    pub(crate) fn new(library: Library) -> Self {
        Self {
            library,
            wanted: BTreeMap::new(),
            slots: BTreeMap::new(),
            next_index: 0,
            asked: HashMap::new(),
            programs: HashMap::new(),
            held: BTreeSet::new(),
            sweep: false,
            line_shift: None,
            caps: Caps::default(),
            formats: None,
            problems: Vec::new(),
            generation: 0,
            #[cfg(test)]
            loads: 0,
        }
    }

    /// The programs the bound rules hold, from now until the rules are bound
    /// again, which calls this again: what one set bound is given back when
    /// another replaces it
    /// (`state::tests::a_replaced_rule_set_holds_only_its_own_programs`).
    pub(crate) fn hold(&mut self, keys: impl IntoIterator<Item = u64>) {
        let keys: BTreeSet<u64> = keys.into_iter().collect();
        if keys != self.held {
            self.held = keys;
            self.sweep = true;
        }
        // A program a refused set's binding asked for is held by nothing now,
        // so it is not compiled
        // (`state::tests::a_refused_rule_set_leaves_no_compile_asked_for`).
        self.forget_unheld_asks();
    }

    /// Forget every program asked for that nothing would keep once compiled:
    /// not the bound rules ([`Self::hold`]), not a running version, not a
    /// pending version's needs. Asked for by nobody, it is not compiled
    /// (`tests::a_name_dropped_beside_one_kept_asks_for_nothing_of_its_own`,
    /// `state::tests::a_refused_rule_set_leaves_no_compile_asked_for`).
    fn forget_unheld_asks(&mut self) {
        let kept: BTreeSet<u64> = self
            .held
            .iter()
            .chain(self.slots.values().flat_map(|slot| {
                slot.holds
                    .iter()
                    .chain(slot.pending.iter().flat_map(|pending| pending.needs.iter()))
            }))
            .copied()
            .collect();
        self.asked.retain(|key, _| kept.contains(key));
    }

    /// What [`Self::hold`] holds, for a test to read.
    #[cfg(test)]
    pub(crate) fn held_for_test(&self) -> &BTreeSet<u64> {
        &self.held
    }

    /// Every program asked for and not compiled yet, for a test to read.
    #[cfg(test)]
    pub(crate) fn asked_for_test(&self) -> BTreeSet<u64> {
        self.asked.keys().copied().collect()
    }

    #[expect(
        dead_code,
        reason = "nothing reads the folders through the host yet: --check makes its own Library"
    )]
    pub(crate) fn library(&self) -> &Library {
        &self.library
    }

    /// What every effect's Lua is made with from now on, `effects.sandbox`;
    /// whether they changed. Every wanted folder loaded under other caps is
    /// loaded again now, so an effect a budget refused can load under a
    /// larger one, and the folders the same `sol.effects` names load under
    /// the new caps (`tests::new_caps_load_every_wanted_folder_again`); an
    /// unchanged folder keeps its id, so a present under way is not ended
    /// (`tests::new_caps_keep_an_unchanged_folders_id`).
    pub(crate) fn set_caps(&mut self, caps: Caps) -> bool {
        if caps == self.caps {
            return false;
        }
        self.caps = caps;
        self.settle_wanted(true);
        true
    }

    /// What every effect's Lua is made with, for a test to read.
    #[cfg(test)]
    pub(crate) fn caps(&self) -> Caps {
        self.caps
    }

    /// Every name any origin wants, and their closure: `pixels`, a
    /// `fallback` naming an effect, and a `use` stage at the defaults
    /// (`tests::a_used_effect_is_loaded_with_its_user_and_a_broken_stage_is_refused`).
    fn all_wanted(&self) -> BTreeSet<String> {
        let mut names: BTreeSet<String> = self.wanted.values().flatten().cloned().collect();
        let mut queue: Vec<String> = names.iter().cloned().collect();
        while let Some(name) = queue.pop() {
            let Some(loaded) = self.latest(&name) else {
                continue;
            };
            for more in loaded.spec().pixels.iter().chain(loaded.splices()).cloned() {
                if names.insert(more.clone()) {
                    queue.push(more);
                }
            }
        }
        names
    }

    /// Every loaded effect but `changed` whose plans splice one of them in,
    /// at any depth: a `use` of a `use` holds the innermost's programs too
    /// (`tests::an_effect_changed_under_its_user_gives_its_old_program_back`).
    fn users_of(&self, changed: &[String]) -> Vec<String> {
        let mut under: BTreeSet<&str> = changed.iter().map(String::as_str).collect();
        let mut users = Vec::new();
        loop {
            let more: Vec<&str> = self
                .slots
                .keys()
                .map(String::as_str)
                .filter(|name| !under.contains(name))
                .filter(|name| {
                    self.latest(name).is_some_and(|loaded| {
                        loaded.splices().any(|each| under.contains(each.as_str()))
                    })
                })
                .collect();
            if more.is_empty() {
                return users;
            }
            under.extend(more.iter().copied());
            users.extend(more.into_iter().map(str::to_owned));
        }
    }

    /// `origin` now wants exactly `names`. A name newly wanted is loaded now,
    /// GPU-free; one nobody wants any more is dropped with its problems,
    /// loaded or not, so a name a rule set named and the next one does not
    /// is not left on the overlay.
    /// `tests::a_program_is_compiled_once_per_content`,
    /// `tests::a_name_no_longer_wanted_takes_its_problems_with_it`.
    pub(crate) fn want(&mut self, origin: &'static str, names: impl IntoIterator<Item = String>) {
        let before = self.all_wanted();
        self.wanted.insert(origin, names.into_iter().collect());
        self.settle_wanted(false);
        let now = self.all_wanted();
        for name in before.difference(&now) {
            self.clear_problems_of(name);
        }
    }

    /// `origin` wants `names` as well as what it wanted: `sol.present`'s, so
    /// a second genie does not drop the first's effect mid-flight.
    /// `tests::present_wants_accumulate_until_a_reload`.
    pub(crate) fn add_wanted(
        &mut self,
        origin: &'static str,
        names: impl IntoIterator<Item = String>,
    ) {
        self.wanted.entry(origin).or_default().extend(names);
        self.settle_wanted(false);
    }

    /// Read every wanted folder again (a configuration reload), and clear the
    /// recorded compile failures once (\[16\] §5). `sol.present`'s names go:
    /// its bindings name them again as they run.
    /// `tests::a_failed_content_is_not_retried_until_it_changes_or_a_reload`,
    /// `tests::present_wants_accumulate_until_a_reload`.
    #[cfg(test)]
    pub(crate) fn reload(&mut self) {
        self.reload_keeping(Vec::new());
    }

    /// [`Self::reload`], `sol.present` keeping `live`, the names a geometry
    /// still draws with: an unchanged one keeps its version and its id
    /// (Ruling 7: an unrelated reload rebuilds nothing), and only one whose
    /// folder changed ends a present under way, by its `on_reload`.
    /// `tests::a_reload_keeps_what_a_live_present_names`,
    /// `state::tests::real_client::a_reload_that_leaves_a_presents_folder_unchanged_keeps_it`.
    pub(crate) fn reload_keeping(&mut self, live: impl IntoIterator<Item = String>) {
        self.programs.retain(|_, program| program.is_ok());
        let live: BTreeSet<String> = live.into_iter().collect();
        if live.is_empty() {
            self.wanted.remove("present");
        } else {
            self.wanted.insert("present", live);
        }
        // And what binding them said (`present:<name>`), for the same reason
        // (`tests::present_wants_accumulate_until_a_reload`).
        self.clear_problems_prefixed("present:");
        self.settle_wanted(true);
    }

    /// The name of the effect an id is a version of, whichever version:
    /// what a reload keeps wanted for a geometry under way.
    /// `tests::a_reload_keeps_what_a_live_present_names`.
    pub(crate) fn name_of(&self, id: EffectId) -> Option<&str> {
        self.slots
            .iter()
            .find(|(_, slot)| slot.index == id.index)
            .map(|(name, _)| name.as_str())
    }

    fn settle_wanted(&mut self, again: bool) {
        // Until nothing new is wanted, since a name's closure is known only
        // once it is loaded; each name is loaded at most once a settle, so a
        // reload reads a changed folder once
        // (`tests::a_reload_loads_each_effect_once`).
        let mut loaded: BTreeSet<String> = BTreeSet::new();
        let mut fresh: Vec<String> = Vec::new();
        loop {
            let next: Vec<String> = self
                .all_wanted()
                .into_iter()
                .filter(|name| !loaded.contains(name) && (again || !self.slots.contains_key(name)))
                .collect();
            if next.is_empty() {
                break;
            }
            for name in next {
                if self.load_one(&name) {
                    fresh.push(name.clone());
                }
                loaded.insert(name);
            }
        }
        // Bound once the whole closure is loaded, since a version's plans
        // splice in the effects it uses and falls back to
        // (`tests::a_fallback_naming_another_effect_binds_it_at_load`).
        for name in &fresh {
            self.programs_of(name);
        }
        let keep = self.all_wanted();
        let gone: Vec<String> = self
            .slots
            .keys()
            .filter(|name| !keep.contains(*name))
            .cloned()
            .collect();
        for name in gone {
            if self
                .slots
                .remove(&name)
                .is_some_and(|slot| slot.current.is_some())
            {
                self.sweep = true;
            }
            self.clear_problems_of(&name);
        }
        // An effect whose own folder is unchanged is bound again when one its
        // plans splice in changed, so it holds that one's new programs and
        // the old version's are given back
        // (`tests::an_effect_changed_under_its_user_gives_its_old_program_back`).
        for name in self.users_of(&fresh) {
            self.programs_of(&name);
        }
        // What was asked for and is no longer wanted is not asked for any
        // more, so with nothing wanted nothing is compiled
        // (`tests::a_name_dropped_before_it_compiled_asks_for_nothing`,
        // `tests::a_name_dropped_beside_one_kept_asks_for_nothing_of_its_own`).
        self.forget_unheld_asks();
    }

    /// Load `name` if its folder changed, or was never loaded: whether a
    /// new version is now pending.
    fn load_one(&mut self, name: &str) -> bool {
        let Some(dir) = self.library.resolve(name) else {
            self.replace_problems(
                name,
                vec![Problem::error(
                    name,
                    Path::new(name),
                    None,
                    format!(
                        "no effect called `{name}` (looked in your effects folder and the shipped one)"
                    ),
                )],
            );
            return false;
        };
        let hash = folder_hash(&dir);
        // A running version its budget stopped is read again even when its
        // folder is unchanged, so a reload rebuilds what `revive` gave up
        // (`tests::a_rebuild_that_fails_is_a_problem_and_waits_for_a_reload`),
        // and so is one loaded under other caps
        // (`tests::new_caps_load_every_wanted_folder_again`).
        if let Some(slot) = self.slots.get(name)
            && slot.pending.is_none()
            && slot.current.as_ref().is_some_and(|current| {
                current.hash() == hash
                    && current.dir() == dir
                    && !current.sandbox().poisoned()
                    && current.sandbox().caps() == self.caps
            })
        {
            return false;
        }
        #[cfg(test)]
        {
            self.loads += 1;
        }
        match Loaded::<P>::load_with(name, &dir, self.caps) {
            Err(problem) => {
                self.replace_problems(name, vec![problem]);
                false
            }
            Ok(loaded) => {
                // A geometry file is held to drawing the window where it is
                // at progress 0 before it is a version (Ruling 19), and a
                // refusal keeps the version that ran:
                // `tests::a_geometry_file_that_moves_the_window_at_rest_is_a_problem_at_its_mesh`.
                if loaded.spec().mesh
                    && let Some(problem) = refused_at_rest(&loaded)
                {
                    self.replace_problems(name, vec![problem]);
                    return false;
                }
                let index = self.slots.get(name).map_or_else(
                    || {
                        self.next_index += 1;
                        self.next_index
                    },
                    |slot| slot.index,
                );
                let slot = self.slots.entry(name.to_owned()).or_insert(Versions {
                    index,
                    generation: 0,
                    current: None,
                    holds: Vec::new(),
                    pending: None,
                    given_up: false,
                });
                slot.pending = Some(loaded);
                self.replace_problems(name, Vec::new());
                true
            }
        }
    }

    /// Bind a pending version at its defaults and record every program its
    /// plans ask for as its `needs`, so it swaps in only once each compiled;
    /// a plan that cannot be made, or a lint error, refuses the version and
    /// keeps the one that ran. With nothing pending, the running version is
    /// bound instead, and holds what its plans ask for now.
    /// `tests::a_stage_effect_swaps_in_only_once_every_step_compiled`,
    /// `tests::a_reload_whose_stage_fails_its_lints_keeps_the_one_that_ran`,
    /// `tests::an_effect_changed_under_its_user_gives_its_old_program_back`.
    fn programs_of(&mut self, name: &str) {
        if !self.has_pending(name) {
            if let Ok((_, holds)) = self.bind_plans(name, &[])
                && let Some(slot) = self.slots.get_mut(name)
            {
                slot.holds = holds;
            }
            return;
        }
        match self.bind_plans(name, &[]) {
            Ok((bound, needs)) => {
                if let Some(slot) = self.slots.get_mut(name) {
                    // With nothing to compile (a geometry file alone) the
                    // version is current at once, so a `sol.present` genie
                    // resolves its id on the pass it is named:
                    // `tests::a_version_with_nothing_to_compile_is_current_at_once`.
                    if needs.is_empty() {
                        if let Some(pending) = slot.pending.take() {
                            swap_in(slot, pending);
                            self.sweep = true;
                        }
                    } else if let Some(pending) = slot.pending.as_mut() {
                        pending.needs = needs;
                    }
                }
                self.add_problems(bound.warnings);
            }
            Err(problems) => {
                // Refused as a load is: on a cold start the effect is absent
                // (`tests::a_stage_effect_swaps_in_only_once_every_step_compiled`).
                let cold = self.slots.get_mut(name).is_some_and(|slot| {
                    slot.pending = None;
                    slot.current.is_none()
                });
                if cold {
                    self.slots.remove(name);
                }
                self.add_problems(problems);
            }
        }
    }

    /// The newest version of `name`: the one waiting to compile if there is
    /// one, else the one that runs. What a bind at config load reads, since
    /// a cold effect is not current until the next `prepare`.
    /// `tests::a_fallback_naming_another_effect_binds_it_at_load`.
    pub(crate) fn latest(&self, name: &str) -> Option<&Loaded<P>> {
        self.slots
            .get(name)
            .and_then(|slot| slot.pending.as_ref().or(slot.current.as_deref()))
    }

    /// Bind `name` with `overrides` into its configured plan and one plan
    /// per fallback rung, dropping any that needs a format this GPU lacks;
    /// every step's program is asked for, and held until the rules' next
    /// [`Self::hold`], which forgets the ask when the rules no longer hold
    /// it (`state::tests::a_refused_rule_set_leaves_no_compile_asked_for`).
    /// A cold effect binds against its pending
    /// version. `tests::a_stage_asking_for_rgba16f_where_it_is_missing_takes_the_fallback`,
    /// `tests::a_fallback_naming_another_effect_binds_it_at_load`.
    pub(crate) fn bind(
        &mut self,
        name: &str,
        overrides: &[(String, Value)],
    ) -> Result<Bound, Problem> {
        // An effect that did not load is refused with why it did not, at its
        // own file and line, so a rule naming it says where to look
        // (`tests::binding_an_effect_that_did_not_load_gives_its_own_problem`).
        if self.latest(name).is_none()
            && let Some(problem) = self
                .problems
                .iter()
                .find(|each| each.effect == name && each.severity == Severity::Error)
        {
            return Err(problem.clone());
        }
        let (bound, keys) = self.bind_plans(name, overrides).map_err(|problems| {
            problems.into_iter().next().unwrap_or_else(|| {
                Problem::error(name, Path::new(name), None, "it did not bind".to_owned())
            })
        })?;
        self.held.extend(keys);
        Ok(bound)
    }

    /// [`Self::bind`]'s plans, and the key of every program they ask for, or
    /// every problem that refused them; nothing is held. Every step's
    /// `.frag` is linted before anything is asked for, so a refused binding
    /// asks for nothing (`tests::a_stage_effect_swaps_in_only_once_every_step_compiled`).
    fn bind_plans(
        &mut self,
        name: &str,
        overrides: &[(String, Value)],
    ) -> Result<(Bound, Vec<u64>), Vec<Problem>> {
        let loaded = self.latest(name).ok_or_else(|| {
            vec![Problem::error(
                name,
                Path::new(name),
                None,
                format!("no effect called `{name}` is loaded"),
            )]
        })?;
        let file = loaded.dir().join("effect.lua");
        let mut bound = loaded.bind(overrides).map_err(|problem| vec![problem])?;
        // Each plan to make: its root effect, its params, and which rung.
        let mut wanted = vec![(name.to_owned(), overrides.to_vec(), None::<String>)];
        for (index, rung) in loaded.spec().fallback.iter().enumerate() {
            let which = Some(format!("its fallback {}", index + 1));
            match rung {
                Rung::Params(more) => {
                    let mut with = overrides.to_vec();
                    with.extend(more.iter().cloned());
                    wanted.push((name.to_owned(), with, which));
                }
                Rung::Effect(other) => wanted.push((other.clone(), Vec::new(), which)),
            }
        }
        let mut plans = Vec::new();
        for (root, with, rung) in wanted {
            let mut resolve =
                |effect: &str, params: &[(String, Value)]| -> Result<Binding, String> {
                    let each = self
                        .latest(effect)
                        .ok_or_else(|| format!("`{effect}` is not loaded"))?;
                    let (params, _) = solium_effects::spec::bind(&each.spec().params, params)?;
                    let stages = each.stages(&params).map_err(|problem| problem.message)?;
                    Ok(Binding {
                        stages,
                        inputs: each.spec().inputs.clone(),
                        params,
                    })
                };
            let plan =
                solium_effects::stage::flatten(&root, &with, &mut resolve).map_err(|message| {
                    let message = match &rung {
                        Some(rung) => format!("{rung}: {message}"),
                        None => message,
                    };
                    vec![Problem::error(name, &file, None, message)]
                })?;
            // Unknown formats (no probe yet) keep the plan: the probe's
            // first answer rebinds (Ruling 11).
            let lacking = self.formats.is_some_and(|formats| !formats.supports(&plan));
            if !lacking {
                plans.push(plan);
            }
        }
        if plans.is_empty() {
            return Err(vec![Problem::error(
                name,
                &file,
                None,
                "no version of this effect can run on this GPU: each draws into rgba16f, which it cannot render into"
                    .to_owned(),
            )]);
        }
        let mut texts: HashMap<PathBuf, String> = HashMap::new();
        let mut asks = Vec::new();
        let mut errors = Vec::new();
        for step in plans.iter().flat_map(|plan| {
            plan.steps
                .iter()
                .chain(plan.states.iter().flat_map(|state| state.steps.iter()))
        }) {
            let Some(dir) = self.latest(&step.effect).map(|each| each.dir().to_owned()) else {
                errors.push(Problem::error(
                    &step.effect,
                    Path::new(&step.effect),
                    None,
                    format!("`{}` is not loaded", step.effect),
                ));
                continue;
            };
            let frag = dir.join(&step.frag);
            if !texts.contains_key(&frag) {
                match std::fs::read_to_string(&frag) {
                    Ok(text) => {
                        texts.insert(frag.clone(), text);
                    }
                    Err(err) => {
                        let problem = Problem::error(
                            &step.effect,
                            &frag,
                            None,
                            format!("cannot read it: {err}"),
                        );
                        if !errors.contains(&problem) {
                            errors.push(problem);
                        }
                        continue;
                    }
                }
            }
            let Some(text) = texts.get(&frag) else {
                continue;
            };
            for lint in glsl::lint(&step.signature, text) {
                let problem = Problem {
                    line: Some(lint.line),
                    severity: lint.severity,
                    ..Problem::error(&step.effect, &frag, None, lint.message)
                };
                let into = match lint.severity {
                    Severity::Error => &mut errors,
                    Severity::Warning => &mut bound.warnings,
                };
                if !into.contains(&problem) {
                    into.push(problem);
                }
            }
            asks.push((
                step.effect.clone(),
                frag,
                glsl::assemble(&step.signature, text),
            ));
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        let mut keys = Vec::with_capacity(asks.len());
        for (effect, frag, sources) in asks {
            keys.push(self.request(&effect, &frag, glsl::PASS_VERTEX, sources));
        }
        let mut at = keys.iter();
        for step in plans.iter_mut().flat_map(|plan| {
            plan.steps.iter_mut().chain(
                plan.states
                    .iter_mut()
                    .flat_map(|state| state.steps.iter_mut()),
            )
        }) {
            if let Some(key) = at.next() {
                step.key = *key;
            }
        }
        keys.sort_unstable();
        keys.dedup();
        bound.plans = plans;
        Ok((bound, keys))
    }

    /// What this GPU renders into, once probed.
    /// `tests::the_formats_are_probed_once_and_only_while_something_is_wanted`.
    pub(crate) fn formats(&self) -> Option<crate::pool::Formats> {
        self.formats
    }

    /// Record what the probe found: whether that changed what was known
    /// (`None` to `Some`, or another answer), which `render::note_formats`
    /// turns into a rebind after the frame.
    /// `tests::a_stage_asking_for_rgba16f_where_it_is_missing_takes_the_fallback`,
    /// `state::tests::the_formats_probe_rebinds_the_rules_after_the_frame`.
    pub(crate) fn set_formats(&mut self, formats: crate::pool::Formats) -> bool {
        let changed = self.formats != Some(formats);
        self.formats = Some(formats);
        changed
    }

    /// Whether `prepare` should probe the formats now: something is wanted
    /// and they were never probed, so a desktop with no effect never makes
    /// the probe's target.
    /// `tests::the_formats_are_probed_once_and_only_while_something_is_wanted`.
    pub(crate) fn wants_formats(&self) -> bool {
        !self.is_idle() && self.formats.is_none()
    }

    /// Ask for a program; its key is the hash of its content, so two effects
    /// with the same program share it
    /// (`tests::a_program_is_compiled_once_per_content`), and one that failed
    /// is not asked for again until a reload clears it
    /// (`tests::a_failed_content_is_not_retried_until_it_changes_or_a_reload`).
    pub(crate) fn request(
        &mut self,
        effect: &str,
        frag: &Path,
        vertex: &'static str,
        sources: Sources,
    ) -> u64 {
        let key = sources.key(vertex);
        if !self.programs.contains_key(&key) {
            self.asked.entry(key).or_insert(Asked {
                vertex,
                sources,
                effect: effect.to_owned(),
                frag: frag.to_owned(),
            });
        }
        key
    }

    /// Compile what was asked for, then swap in every pending version whose
    /// programs all compiled; drop the rest and keep what ran. Between frames,
    /// with the context current. Nothing at all when idle.
    /// `tests::a_reload_with_a_broken_effect_keeps_the_one_that_ran`,
    /// `tests::a_broken_effect_on_a_cold_start_is_absent_not_fatal`,
    /// `tests::an_empty_host_touches_no_gl`.
    pub(crate) fn compile_pending<C: Compiler<Program = P>>(&mut self, compiler: &mut C) {
        if self.is_idle() {
            return;
        }
        for (key, asked) in std::mem::take(&mut self.asked) {
            if self.programs.contains_key(&key) {
                continue;
            }
            match compiler.compile(asked.vertex, &asked.sources) {
                Ok(program) => {
                    self.programs.insert(key, Ok(program));
                }
                Err(log) => {
                    let shift = *self.line_shift.get_or_insert_with(|| compiler.line_shift());
                    let problems =
                        log_problems(&asked.effect, &asked.frag, &asked.sources, &log, shift);
                    self.add_problems(problems);
                    self.programs.insert(key, Err(Arc::from(log.as_str())));
                }
            }
        }
        for slot in self.slots.values_mut() {
            let Some(pending) = slot.pending.take() else {
                continue;
            };
            // A version replaced or refused leaves programs that only it
            // needed (`tests::a_replaced_version_gives_its_program_back`).
            self.sweep = true;
            if pending
                .needs
                .iter()
                .all(|key| matches!(self.programs.get(key), Some(Ok(_))))
            {
                swap_in(slot, pending);
            }
        }
        if std::mem::take(&mut self.sweep) {
            self.drop_unused(compiler);
        }
    }

    fn drop_unused<C: Compiler<Program = P>>(&mut self, compiler: &mut C) {
        let live: BTreeSet<u64> = self
            .slots
            .values()
            .filter(|slot| slot.current.is_some())
            .flat_map(|slot| slot.holds.iter().copied())
            .chain(self.held.iter().copied())
            .collect();
        let dead: Vec<u64> = self
            .programs
            .iter()
            .filter(|(key, program)| program.is_ok() && !live.contains(key))
            .map(|(key, _)| *key)
            .collect();
        for key in dead {
            if let Some(Ok(program)) = self.programs.remove(&key) {
                compiler.delete(program);
            }
        }
    }

    /// Rebuild every effect whose Lua its budget stopped, GPU-free, after
    /// the frame (Ruling 4): the same folder loaded again when its hash is
    /// unchanged, keeping its programs, its `needs` and its `EffectId`, so
    /// only the panes whose call came in the pass that overran fade. A folder
    /// changed since it loaded waits for the reload that reads it.
    /// `geometry::tests::a_stopped_state_is_rebuilt_after_the_frame`,
    /// `tests::a_stopped_state_whose_folder_changed_waits_for_the_reload`.
    pub(crate) fn revive(&mut self) {
        let caps = self.caps;
        self.revive_with(|name, dir| Loaded::<P>::load_with(name, dir, caps));
    }

    /// [`Self::revive`], loading a folder with `load`.
    fn revive_with(&mut self, load: impl Fn(&str, &Path) -> Result<Loaded<P>, Problem>) {
        let mut problems = Vec::new();
        for slot in self.slots.values_mut() {
            if slot.given_up {
                continue;
            }
            let Some(current) = slot
                .current
                .as_ref()
                .filter(|current| current.sandbox().poisoned())
            else {
                continue;
            };
            #[cfg(test)]
            {
                self.loads += 1;
            }
            // Read before, so a changed folder's Lua is not run, and after,
            // so a save during the load is not swapped in under the old id
            // (`tests::a_stopped_state_whose_folder_changed_waits_for_the_reload`,
            // `tests::a_folder_saved_during_a_rebuild_is_not_swapped_in`).
            if folder_hash(current.dir()) != current.hash() {
                slot.given_up = true;
                continue;
            }
            match load(current.name(), current.dir()) {
                Ok(mut fresh) if fresh.hash() == current.hash() => {
                    // `tests::a_rebuilt_state_keeps_what_its_version_needs`.
                    fresh.needs.clone_from(&current.needs);
                    slot.current = Some(Rc::new(fresh));
                }
                Ok(_) => slot.given_up = true,
                // A rebuild that fails is said, once, and not tried again
                // every frame (`tests::a_rebuild_that_fails_is_a_problem_and_waits_for_a_reload`).
                Err(problem) => {
                    slot.given_up = true;
                    problems.push(problem);
                }
            }
        }
        self.add_problems(problems);
    }

    /// The version of `name` that runs, if one compiled.
    /// `tests::a_broken_effect_on_a_cold_start_is_absent_not_fatal`.
    pub(crate) fn effect(&self, name: &str) -> Option<Rc<Loaded<P>>> {
        self.slots.get(name).and_then(|slot| slot.current.clone())
    }

    /// Whether a version of `name` waits for the next compile.
    /// `tests::present_wants_accumulate_until_a_reload`.
    pub(crate) fn has_pending(&self, name: &str) -> bool {
        self.slots
            .get(name)
            .is_some_and(|slot| slot.pending.is_some())
    }

    /// The pending version's folder hash, for a test to compare across a reload.
    #[cfg(test)]
    pub(crate) fn pending_hash(&self, name: &str) -> Option<u64> {
        self.slots
            .get(name)
            .and_then(|slot| slot.pending.as_ref())
            .map(Loaded::hash)
    }

    /// What the running version of `name` holds, for a test to compare
    /// across a reload.
    #[cfg(test)]
    pub(crate) fn holds(&self, name: &str) -> Vec<u64> {
        self.slots
            .get(name)
            .filter(|slot| slot.current.is_some())
            .map(|slot| slot.holds.clone())
            .unwrap_or_default()
    }

    /// Every asked program compiled and no version waiting: the effects'
    /// "ready" on the warm-up list (spec C14, Ruling 7).
    /// `tests::a_cold_effect_is_ready_after_the_first_compile`.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the warm-up list (milestone 1, T2) reads it")
    )]
    pub(crate) fn ready(&self) -> bool {
        self.asked.is_empty() && self.slots.values().all(|slot| slot.pending.is_none())
    }

    /// The running version's id, for a test to compare with what a present
    /// carries ([`Self::upcoming`] is what one is given).
    /// `tests::a_new_version_is_a_new_generation`.
    #[cfg(test)]
    pub(crate) fn id(&self, name: &str) -> Option<EffectId> {
        self.slots
            .get(name)
            .filter(|slot| slot.current.is_some())
            .map(|slot| EffectId {
                index: slot.index,
                generation: slot.generation,
            })
    }

    /// The id the newest version of `name` ([`Self::latest`]) is drawn
    /// under: a pending one's, the id it takes once it compiled, at the top
    /// of the next `prepare` and before any grid is built (Ruling 7); else
    /// the running one's. A geometry presented on a cold start, or after a
    /// reload, before its frag compiled is drawn from that pass on, not by
    /// its `failed`, and is not ended by its own version swapping in.
    /// `state::tests::real_client::a_geometry_with_a_frag_presents_before_it_compiled`.
    pub(crate) fn upcoming(&self, name: &str) -> Option<EffectId> {
        let slot = self.slots.get(name)?;
        // The same folder compiling again under new caps keeps the running
        // id (`tests::new_caps_keep_an_unchanged_folders_id`).
        let generation = match (&slot.pending, &slot.current) {
            (Some(pending), _) if same_folder(slot, pending) => slot.generation,
            (Some(_), _) => slot.generation.checked_add(1)?,
            (None, Some(_)) => slot.generation,
            (None, None) => return None,
        };
        Some(EffectId {
            index: slot.index,
            generation,
        })
    }

    /// The version an id names, while it is the one that runs.
    /// `tests::a_new_version_is_a_new_generation`.
    pub(crate) fn by_id(&self, id: EffectId) -> Option<Rc<Loaded<P>>> {
        self.slots
            .values()
            .find(|slot| slot.index == id.index && slot.generation == id.generation)
            .and_then(|slot| slot.current.clone())
    }

    #[expect(
        dead_code,
        reason = "Task 29's warp draws a folder's frag through it once it compiled"
    )]
    pub(crate) fn program(&self, key: u64) -> Option<&P> {
        self.programs
            .get(&key)
            .and_then(|program| program.as_ref().ok())
    }

    /// A program by key, as `run::preflight` reads it: compiled, failed, or
    /// asked for and not compiled yet. A key nothing asked for is no program
    /// a later compile would bring, so it is a failure, not pending for ever.
    /// `tests::a_programs_lookup_tells_pending_from_failed`.
    pub(crate) fn lookup(&self, key: u64) -> super::run::Lookup<'_, P> {
        use super::run::Lookup;
        match self.programs.get(&key) {
            Some(Ok(program)) => Lookup::Ready(program),
            Some(Err(_)) => Lookup::Failed,
            None if self.asked.contains_key(&key) => Lookup::Pending,
            None => Lookup::Failed,
        }
    }

    pub(crate) fn problems(&self) -> &[Problem] {
        &self.problems
    }

    pub(crate) fn problems_generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn push_problem(&mut self, problem: Problem) {
        self.add_problems(vec![problem]);
    }

    pub(crate) fn clear_problems_of(&mut self, effect: &str) {
        self.replace_problems(effect, Vec::new());
    }

    /// Take every problem of an effect named under `prefix` (every pane
    /// style's, `"style:"`), in one change, and none when there was nothing
    /// to take. `tests::clearing_by_prefix_is_one_change`.
    pub(crate) fn clear_problems_prefixed(&mut self, prefix: &str) {
        let before = self.problems.len();
        self.problems
            .retain(|each| !each.effect.starts_with(prefix));
        if self.problems.len() != before {
            self.generation += 1;
        }
    }

    /// Add what is not already listed: two effects binding one broken
    /// `.frag` name it once (`tests::a_broken_frag_two_effects_bind_is_named_once`).
    fn add_problems(&mut self, problems: Vec<Problem>) {
        let before = self.problems.len();
        for problem in problems {
            if !self.problems.contains(&problem) {
                self.problems.push(problem);
            }
        }
        if self.problems.len() != before {
            self.generation += 1;
        }
    }

    fn replace_problems(&mut self, effect: &str, problems: Vec<Problem>) {
        let before = self.problems.len();
        self.problems.retain(|each| each.effect != effect);
        if self.problems.len() != before || !problems.is_empty() {
            self.problems.extend(problems);
            self.generation += 1;
        }
    }

    /// Nothing wanted, nothing asked, nothing pending and nothing to give
    /// back: `prepare` does nothing (`tests::an_empty_host_touches_no_gl`,
    /// `tests::an_effect_no_longer_wanted_gives_its_program_back`).
    pub(crate) fn is_idle(&self) -> bool {
        self.asked.is_empty()
            && !self.sweep
            && self.slots.values().all(|slot| slot.pending.is_none())
            && self.wanted.values().all(BTreeSet::is_empty)
    }
}

/// `pending` swapped in as the version that runs, holding what it needs: a
/// new generation (`tests::a_new_version_is_a_new_generation`), unless it
/// is the running one's folder as it was ([`same_folder`]).
fn swap_in<P>(slot: &mut Versions<P>, pending: Loaded<P>) {
    if !same_folder(slot, &pending) {
        slot.generation += 1;
    }
    slot.holds = pending.needs.clone();
    slot.current = Some(Rc::new(pending));
    slot.given_up = false;
}

/// Whether `pending` is the running version's folder as it was, loaded again
/// only because the caps changed or its state was stopped: the same version,
/// which keeps its id as [`Host::revive`]'s rebuild does, so new
/// `effects.sandbox` caps end no present under way
/// (`tests::new_caps_keep_an_unchanged_folders_id`,
/// `state::tests::real_client::a_reload_that_changes_only_the_sandbox_keeps_a_present`).
fn same_folder<P>(slot: &Versions<P>, pending: &Loaded<P>) -> bool {
    slot.current
        .as_ref()
        .is_some_and(|current| current.hash() == pending.hash() && current.dir() == pending.dir())
}

/// A geometry file held to its rest at its defaults (`geometry::at_rest`),
/// and why not, as a problem at its file: a Lua error at its own line,
/// anything else at its `mesh`'s.
/// `tests::a_geometry_file_that_moves_the_window_at_rest_is_a_problem_at_its_mesh`.
fn refused_at_rest<P>(loaded: &Loaded<P>) -> Option<Problem> {
    let grid = loaded
        .spec()
        .grid
        .unwrap_or(GridSpec::Fixed { cols: 1, rows: 1 });
    let refusal = geometry::at_rest(loaded.sandbox(), grid, loaded.defaults()).err()?;
    Some(at_rest_problem(loaded, refusal))
}

/// What a refusal at load says, and where.
/// `tests::a_mesh_too_slow_for_a_frame_is_a_problem_at_its_mesh`.
fn at_rest_problem<P>(loaded: &Loaded<P>, refusal: Refusal) -> Problem {
    let file = loaded.dir().join("effect.lua");
    let line = loaded.sandbox().mesh_line();
    let message = match refusal {
        Refusal::Error { line, message } => {
            return Problem::error(loaded.name(), &file, line, message);
        }
        Refusal::Budget => format!(
            "its mesh ran past the {} ms its checks at load have (`effects.sandbox.load_ms`; progress 0 and 1, every axis and direction), and was stopped",
            loaded.sandbox().caps().load.as_millis()
        ),
        Refusal::Slow { took } => format!(
            "its mesh takes {:.1} ms a call; a frame gives it {} ms",
            took.as_secs_f64() * 1000.0,
            Budget::MESH.as_millis()
        ),
        Refusal::Count { wanted, got } => {
            format!("its mesh wrote {got} numbers, where its grid has {wanted}: x and y for each point")
        }
        Refusal::NotFinite => "its mesh wrote a number that is not finite".to_owned(),
        Refusal::TooBig => {
            "its mesh put a point more than four monitors' width or height from the window's monitor"
                .to_owned()
        }
        Refusal::MovesAtRest => "its mesh moves the window at progress 0: at rest a geometry draws the window where it is, arriving, leaving and resizing".to_owned(),
    };
    Problem::error(loaded.name(), &file, line, message)
}

/// A configuration error as a problem: the first `<file>.lua:<line>:` in it
/// names where (`tests::a_config_problem_is_at_the_first_lua_file_and_line_in_the_error`,
/// `state::tests::a_failed_reload_is_a_problem_until_one_succeeds`).
pub(crate) fn config_problem(error: &str) -> Problem {
    for (at, _) in error.match_indices(".lua:") {
        let start = error
            .get(..at)
            .and_then(|before| {
                before
                    .char_indices()
                    .rev()
                    .find(|&(_, c)| c.is_whitespace() || c == '"' || c == '\'')
            })
            .map_or(0, |(index, c)| index + c.len_utf8());
        let (Some(file), Some(from)) = (error.get(start..at + 4), error.get(start..)) else {
            continue;
        };
        let (line, message) = super::sandbox::located(from, Path::new(file));
        if line.is_some() {
            return Problem::error("config", Path::new(file), line, message);
        }
    }
    Problem::error(
        "config",
        Path::new("init.lua"),
        None,
        error.lines().next().unwrap_or(error).to_owned(),
    )
}

/// A driver's log as problems: string 1 at the `.frag`'s line; a string-0
/// line past the prelude's length is the user's too, from a driver that
/// ignored `#line` and counted across the joined strings (Ruling 6); the rest
/// of the prelude's, or the epilogue's, one problem naming the engine. A
/// string-1 line is `shift` lower than the user's on a driver that applies
/// GLSL ES 3.00's `#line` rule ([`Compiler::line_shift`]).
/// `tests::a_log_numbered_across_the_strings_is_mapped_back`,
/// `tests::a_driver_numbering_from_0_after_line_is_calibrated_once`.
pub(crate) fn log_problems(
    effect: &str,
    frag: &Path,
    sources: &Sources,
    log: &str,
    shift: u32,
) -> Vec<Problem> {
    let prelude = u32::try_from(sources.prelude.lines().count()).unwrap_or(u32::MAX);
    let user = u32::try_from(sources.user.lines().count()).unwrap_or(u32::MAX);
    let found: Vec<Problem> = glsl::compile_log(log)
        .into_iter()
        .map(|diagnostic| match (diagnostic.string, diagnostic.line) {
            (Some(1), line) => Problem {
                line: line.map(|line| line + shift),
                column: diagnostic.column,
                ..Problem::error(effect, frag, None, diagnostic.message)
            },
            (Some(0), Some(line)) if line > prelude && line - prelude <= user => Problem {
                line: Some(line - prelude),
                column: diagnostic.column,
                ..Problem::error(effect, frag, None, diagnostic.message)
            },
            (Some(_), _) => Problem::error(
                effect,
                frag,
                None,
                format!(
                    "the engine's own GLSL did not compile here, which is a Solium bug: {}",
                    diagnostic.message
                ),
            ),
            (None, _) => Problem::error(effect, frag, None, diagnostic.message),
        })
        .collect();
    if found.is_empty() {
        vec![Problem::error(
            effect,
            frag,
            None,
            "the program did not compile".to_owned(),
        )]
    } else {
        found
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::{Path, PathBuf};

    use super::Library;

    /// A temporary directory of this test's own, emptied first, as
    /// `style::tests::scratch` makes them.
    pub(crate) fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("solium-effect-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temporary directory");
        dir
    }

    /// An effect folder `name` in `place`: its `effect.lua` and any other files.
    pub(crate) fn folder(
        place: &Path,
        name: &str,
        effect_lua: &str,
        files: &[(&str, &str)],
    ) -> PathBuf {
        let dir = place.join(name);
        std::fs::create_dir_all(&dir).expect("an effect folder");
        std::fs::write(dir.join("effect.lua"), effect_lua).expect("writing effect.lua");
        for (file, text) in files {
            std::fs::write(dir.join(file), text).expect("writing an effect's file");
        }
        dir
    }

    /// **A user's folder shadows the shipped one of the same name** ([16] §2),
    /// name by name, as a pane style's does.
    #[test]
    fn a_user_folder_shadows_the_shipped_one() {
        let (user, shipped) = (scratch("user-shadow"), scratch("shipped-shadow"));
        folder(&shipped, "blur", "return { api = 1 }", &[]);
        let mine = folder(&user, "blur", "return { api = 1 }", &[]);
        let library = Library::with(Some(user.clone()), shipped.clone());
        assert_eq!(library.resolve("blur"), Some(mine));
        let _ = (
            std::fs::remove_dir_all(&user),
            std::fs::remove_dir_all(&shipped),
        );
    }

    /// **A folder with no `effect.lua` falls through** to the shipped one: an
    /// empty `~/.config/solium/effects/blur/` made and not yet filled does not
    /// cost the blur, as an empty style folder does not cost the frame
    /// (`style::tests::a_bare_name_needs_a_manifest_and_not_merely_a_folder`).
    #[test]
    fn a_folder_without_effect_lua_falls_through() {
        let (user, shipped) = (scratch("user-empty"), scratch("shipped-empty"));
        let theirs = folder(&shipped, "blur", "return { api = 1 }", &[]);
        std::fs::create_dir_all(user.join("blur")).expect("an empty folder");
        let library = Library::with(Some(user.clone()), shipped.clone());
        assert_eq!(library.resolve("blur"), Some(theirs));
        let _ = (
            std::fs::remove_dir_all(&user),
            std::fs::remove_dir_all(&shipped),
        );
    }

    /// **An effect's name is a bare name**: it is written in rules, in `use`
    /// and in `fallback`, where a path means nothing, so `""`, `"a/b"` and
    /// `".."` resolve to nothing even when such a directory exists. Each one
    /// here would find an `effect.lua` if it were joined on as a path.
    #[test]
    fn an_effect_name_is_a_bare_name() {
        let root = scratch("names");
        let (user, shipped) = (root.join("user"), root.join("shipped"));
        folder(&shipped, "ok-name_2", "return { api = 1 }", &[]);
        let elsewhere = folder(&root, "elsewhere", "return { api = 1 }", &[]);
        folder(&user.join("a"), "b", "return { api = 1 }", &[]);
        folder(&user, "Blur", "return { api = 1 }", &[]);
        folder(&user, "blur ", "return { api = 1 }", &[]);
        std::fs::write(user.join("effect.lua"), "return { api = 1 }").expect("\"\" as a path");
        std::fs::write(root.join("effect.lua"), "return { api = 1 }").expect("\"..\" as a path");
        let library = Library::with(Some(user.clone()), shipped.clone());
        let absolute = elsewhere.display().to_string();
        for bad in ["", "a/b", "..", "Blur", "blur ", "/tmp", absolute.as_str()] {
            assert_eq!(library.resolve(bad), None, "{bad:?} is not a name");
        }
        assert!(library.resolve("ok-name_2").is_some());
        assert_eq!(library.resolve("nowhere"), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `--check` reads every folder in the user's place, sorted, whether or
    /// not it has an `effect.lua` (that it lacks one is what it reports).
    #[test]
    fn the_users_folders_are_listed_sorted() {
        let (user, shipped) = (scratch("user-list"), scratch("shipped-list"));
        // Made in an order that is neither sorted nor sorted backwards, so a
        // directory listed in the order it was written fails.
        folder(&user, "fade", "return { api = 1 }", &[]);
        folder(&user, "zoom", "return { api = 1 }", &[]);
        std::fs::create_dir_all(user.join("blur")).expect("a folder");
        std::fs::create_dir_all(user.join("kawase")).expect("a folder");
        std::fs::write(user.join("notes.txt"), "not a folder").expect("a file");
        let library = Library::with(Some(user.clone()), shipped.clone());
        let names = ["blur", "fade", "kawase", "zoom"];
        assert_eq!(
            library.user_folders(),
            names.map(|name| user.join(name)).to_vec()
        );
        assert!(
            Library::with(None, shipped.clone())
                .user_folders()
                .is_empty()
        );
        let _ = (
            std::fs::remove_dir_all(&user),
            std::fs::remove_dir_all(&shipped),
        );
    }

    /// **The fixture folders load**, each into a sandbox of its own, their
    /// stages read, and their defaults bind.
    #[test]
    fn the_fixture_effects_load_and_bind_at_their_defaults() {
        let fixtures = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/effects"
        ));
        for name in [
            "identity",
            "kawase",
            "tint",
            "frost",
            "three",
            "jump",
            "state-count",
            "ring",
        ] {
            let dir = fixtures.join(name);
            let loaded = super::Loaded::<u32>::load(name, &dir)
                .unwrap_or_else(|problem| panic!("{name}: {problem:?}"));
            let bound = loaded
                .bind(&[])
                .unwrap_or_else(|problem| panic!("{name} at its defaults: {problem:?}"));
            assert_eq!(
                (loaded.name(), loaded.dir(), loaded.hash()),
                (name, dir.as_path(), super::folder_hash(&dir))
            );
            assert_eq!(loaded.spec().api, 1, "{name}");
            assert!(!loaded.sandbox().poisoned() && loaded.needs.is_empty());
            if name == "frost" {
                // its stages(p) at the defaults use both.
                assert_eq!(loaded.used, ["kawase", "tint"]);
            }
            if matches!(name, "kawase" | "frost") {
                // kawase's is a function of its defaults, 3 * 2^(3 + 1);
                // frost's a number.
                assert!(
                    (bound.reach - 48.0).abs() < f64::EPSILON,
                    "{name}: {bound:?}"
                );
            }
        }
    }

    /// Binding through a loaded effect names it: a refused override is an
    /// error at its `effect.lua`, and a clamp a warning there.
    #[test]
    fn binding_names_the_effect_and_warns_of_a_clamp() {
        use solium_effects::spec::{Severity, Value};
        let place = scratch("bind");
        let lua = "return { api = 1, frag = 'effect.frag', params = { amount = { 0.5, min = 0, max = 1 } } }";
        let dir = folder(&place, "soft", lua, &[("effect.frag", "x")]);
        let loaded = super::Loaded::<()>::load("soft", &dir).expect("loads");
        let bound = loaded
            .bind(&[("amount".to_owned(), Value::Number(2.0))])
            .expect("clamped");
        assert_eq!(
            bound.params,
            vec![("amount".to_owned(), Value::Number(1.0))]
        );
        assert_eq!(bound.warnings.len(), 1, "{bound:?}");
        let warning = &bound.warnings[0];
        assert_eq!(
            (warning.effect.as_str(), warning.severity, &warning.file),
            ("soft", Severity::Warning, &dir.join("effect.lua"))
        );
        let refused = loaded
            .bind(&[("amout".to_owned(), Value::Number(0.2))])
            .expect_err("a typo");
        assert_eq!(
            (refused.effect.as_str(), refused.severity),
            ("soft", Severity::Error)
        );
        assert!(refused.message.contains("`amount`"), "{refused:?}");
        let _ = std::fs::remove_dir_all(place);
    }

    /// Two folders with the same files have the same hash, and one changed
    /// byte gives another: what Task 4's "unchanged is not reloaded" rests on.
    /// The hash is the one content hash programs are keyed by, over each
    /// file's name and bytes in name order.
    #[test]
    fn a_folders_hash_is_its_files() {
        let place = scratch("hash");
        let lua = "return { api = 1, frag = 'effect.frag' }";
        let a = folder(&place, "a", lua, &[("effect.frag", "x")]);
        let b = folder(&place, "b", lua, &[("effect.frag", "x")]);
        let c = folder(&place, "c", lua, &[("effect.frag", "y")]);
        let hash = |dir: &Path| super::folder_hash(dir);
        assert_eq!(hash(&a), hash(&b));
        assert_ne!(hash(&a), hash(&c));
        assert_eq!(
            hash(&a),
            solium_effects::glsl::content_hash(&[
                b"effect.frag",
                b"x",
                b"effect.lua",
                lua.as_bytes()
            ]),
            "each file's name and bytes, in name order, through glsl::content_hash"
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// The shipped folders sit beside the shipped QML and Lua, wherever those
    /// were found, so an install that has one has the others.
    #[test]
    fn the_shipped_effects_are_beside_the_qml_and_the_lua() {
        assert_eq!(
            crate::assets::effects().parent(),
            crate::assets::qml().parent()
        );
        assert!(crate::assets::effects().ends_with("effects"));
        assert!(
            crate::assets::effects().join("README.md").is_file(),
            "the reference ships with them"
        );
    }

    /// A compiler that counts what it was asked, fails a source containing
    /// `FAIL` with a Mesa-shaped log at the line it is on (`shift` lower, as
    /// a driver applying GLSL ES 3.00's `#line` rule numbers it), and records
    /// deletes and how often its line rule was asked.
    #[derive(Debug, Default)]
    pub(crate) struct Counting {
        pub(crate) compiled: Vec<u64>,
        pub(crate) deleted: Vec<u32>,
        pub(crate) shift: u32,
        pub(crate) probed: u32,
        next: u32,
    }

    impl super::Compiler for Counting {
        type Program = u32;
        fn compile(
            &mut self,
            vertex: &str,
            sources: &solium_effects::glsl::Sources,
        ) -> Result<u32, String> {
            self.compiled.push(sources.key(vertex));
            if let Some(index) = sources.user.lines().position(|line| line.contains("FAIL")) {
                let line = u32::try_from(index).expect("a short source") + 1 - self.shift;
                return Err(format!("1:{line}(1): error: FAIL is not GLSL"));
            }
            self.next += 1;
            Ok(self.next)
        }
        fn delete(&mut self, program: u32) {
            self.deleted.push(program);
        }
        fn line_shift(&mut self) -> u32 {
            self.probed += 1;
            self.shift
        }
    }

    /// A compiler for a `Solium`'s own host, with no GPU: it refuses a
    /// source whose user string contains `FAIL` with a Mesa-shaped log at
    /// that line, as [`Counting`] does, and answers a program no test ever
    /// draws otherwise (`gl::Program::for_test`).
    /// `state::tests::real_client::every_failure_leaves_the_part_drawn`.
    #[derive(Debug)]
    pub(crate) struct Refusing;

    impl super::Compiler for Refusing {
        type Program = crate::effect::gl::Program;
        fn compile(
            &mut self,
            _vertex: &str,
            sources: &solium_effects::glsl::Sources,
        ) -> Result<crate::effect::gl::Program, String> {
            if let Some(index) = sources.user.lines().position(|line| line.contains("FAIL")) {
                let line = u32::try_from(index).expect("a short source") + 1;
                return Err(format!("1:{line}(1): error: FAIL is not GLSL"));
            }
            Ok(crate::effect::gl::Program::for_test())
        }
        fn delete(&mut self, _program: crate::effect::gl::Program) {}
    }

    /// A `frag` effect's signature at its defaults, as its one pass is
    /// flattened: its params' kinds, no `uses`, its texture inputs known.
    fn default_signature(
        spec: &solium_effects::spec::EffectSpec,
        host: solium_effects::glsl::Host,
    ) -> solium_effects::glsl::Signature {
        solium_effects::glsl::Signature {
            host,
            params: spec
                .params
                .iter()
                .filter_map(|(name, param)| {
                    solium_effects::glsl::kind_of(&param.default).map(|kind| (name.clone(), kind))
                })
                .collect(),
            uses: Vec::new(),
            known: spec
                .inputs
                .iter()
                .filter(|input| *input != "shape")
                .map(|input| input.trim_start_matches("state:").to_owned())
                .collect(),
        }
    }

    const ONE_PASS: &str = "return { api = 1, inputs = { 'self' }, frag = 'effect.frag' }";
    const FRAG: &str = "vec4 sol_effect(vec2 uv) {\n  return sol_tex(uv);\n}\n";

    fn host_with(place: &Path) -> super::Host<u32> {
        super::Host::new(Library::with(
            Some(place.to_owned()),
            place.join("no-shipped"),
        ))
    }

    /// **A program is compiled once per content**: two effects sharing a
    /// `.frag` and a signature compile it once.
    #[test]
    fn a_program_is_compiled_once_per_content() {
        let place = scratch("once-per-content");
        folder(&place, "a", ONE_PASS, &[("effect.frag", FRAG)]);
        folder(&place, "b", ONE_PASS, &[("effect.frag", FRAG)]);
        let mut host = host_with(&place);
        host.want("rules", ["a".to_owned(), "b".to_owned()]);
        let mut compiler = Counting::default();
        host.compile_pending(&mut compiler);
        assert_eq!(compiler.compiled.len(), 1, "{:?}", compiler.compiled);
        assert!(host.effect("a").is_some() && host.effect("b").is_some());
        let _ = std::fs::remove_dir_all(place);
    }

    /// **An unchanged effect is not recompiled or rebuilt on reload**: the
    /// same `Loaded` survives, so an unrelated reload rebuilds nothing.
    #[test]
    fn an_unchanged_effect_is_not_recompiled_or_rebuilt_on_reload() {
        let place = scratch("unchanged");
        folder(&place, "a", ONE_PASS, &[("effect.frag", FRAG)]);
        let mut host = host_with(&place);
        host.want("rules", ["a".to_owned()]);
        let mut compiler = Counting::default();
        host.compile_pending(&mut compiler);
        let before = host.effect("a").expect("loaded");
        host.reload();
        host.compile_pending(&mut compiler);
        assert_eq!(compiler.compiled.len(), 1);
        assert!(std::rc::Rc::ptr_eq(
            &before,
            &host.effect("a").expect("still loaded")
        ));
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A reload with a broken effect keeps the one that ran**, and a
    /// problem names the new version's line.
    #[test]
    fn a_reload_with_a_broken_effect_keeps_the_one_that_ran() {
        let place = scratch("keeps");
        let dir = folder(&place, "a", ONE_PASS, &[("effect.frag", FRAG)]);
        let mut host = host_with(&place);
        host.want("rules", ["a".to_owned()]);
        let mut compiler = Counting::default();
        host.compile_pending(&mut compiler);
        let first = host.effect("a").expect("v1");
        std::fs::write(
            dir.join("effect.frag"),
            "vec4 sol_effect(vec2 uv) {\n  FAIL\n}\n",
        )
        .expect("v2");
        host.reload();
        host.compile_pending(&mut compiler);
        assert!(
            std::rc::Rc::ptr_eq(&first, &host.effect("a").expect("still v1")),
            "the broken v2 replaced v1"
        );
        let problem = host
            .problems()
            .iter()
            .find(|each| each.effect == "a")
            .expect("a problem");
        assert_eq!(
            (problem.file.clone(), problem.line),
            (dir.join("effect.frag"), Some(2)),
            "{problem:?}"
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A broken effect on a cold start is absent, not fatal**: no version,
    /// a problem, and the host serves the others.
    #[test]
    fn a_broken_effect_on_a_cold_start_is_absent_not_fatal() {
        let place = scratch("cold");
        folder(&place, "bad", ONE_PASS, &[("effect.frag", "FAIL\n")]);
        folder(&place, "good", ONE_PASS, &[("effect.frag", FRAG)]);
        let mut host = host_with(&place);
        host.want("rules", ["bad".to_owned(), "good".to_owned()]);
        host.compile_pending(&mut Counting::default());
        assert!(host.effect("bad").is_none());
        assert!(host.effect("good").is_some());
        assert!(host.problems().iter().any(|each| each.effect == "bad"));
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A failed content is not retried until it changes, or a reload.**
    #[test]
    fn a_failed_content_is_not_retried_until_it_changes_or_a_reload() {
        let place = scratch("not-retried");
        folder(&place, "bad", ONE_PASS, &[("effect.frag", "FAIL\n")]);
        let mut host = host_with(&place);
        host.want("rules", ["bad".to_owned()]);
        let mut compiler = Counting::default();
        host.compile_pending(&mut compiler);
        host.compile_pending(&mut compiler);
        assert_eq!(
            compiler.compiled.len(),
            1,
            "tried again with nothing changed"
        );
        host.reload();
        host.compile_pending(&mut compiler);
        assert_eq!(
            compiler.compiled.len(),
            2,
            "a reload clears the failures once"
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// **An empty host touches no GL**: nothing wanted, no compile, no
    /// delete, and `is_idle` says so, which is what `prepare` asks first.
    #[test]
    fn an_empty_host_touches_no_gl() {
        let place = scratch("empty");
        let mut host = host_with(&place);
        assert!(host.is_idle());
        let mut compiler = Counting::default();
        host.compile_pending(&mut compiler);
        assert!(compiler.compiled.is_empty() && compiler.deleted.is_empty());
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A name dropped before it compiled asks for nothing**: its program
    /// is no longer asked for, so a host with nothing wanted is idle and
    /// `prepare` compiles nothing (Ruling 7, spec §8.4).
    #[test]
    fn a_name_dropped_before_it_compiled_asks_for_nothing() {
        let place = scratch("dropped-cold");
        folder(&place, "a", ONE_PASS, &[("effect.frag", FRAG)]);
        let mut host = host_with(&place);
        host.want("rules", ["a".to_owned()]);
        host.want("rules", []);
        assert!(
            host.is_idle(),
            "nothing is wanted, but a compile is still asked for"
        );
        let mut compiler = Counting::default();
        host.compile_pending(&mut compiler);
        assert!(compiler.compiled.is_empty(), "{:?}", compiler.compiled);
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A name dropped before it compiled asks for nothing of its own**
    /// while another stays wanted: only the one still wanted is compiled.
    #[test]
    fn a_name_dropped_beside_one_kept_asks_for_nothing_of_its_own() {
        let place = scratch("dropped-beside-kept");
        folder(&place, "a", ONE_PASS, &[("effect.frag", FRAG)]);
        folder(
            &place,
            "b",
            ONE_PASS,
            &[(
                "effect.frag",
                "vec4 sol_effect(vec2 uv) {\n  return sol_tex(uv) * 0.5;\n}\n",
            )],
        );
        let mut host = host_with(&place);
        host.want("rules", ["a".to_owned(), "b".to_owned()]);
        host.want("rules", ["a".to_owned()]);
        let mut compiler = Counting::default();
        host.compile_pending(&mut compiler);
        assert_eq!(
            compiler.compiled.len(),
            1,
            "a name nobody wants was still compiled: {:?}",
            compiler.compiled
        );
        assert!(host.effect("a").is_some(), "the premise: a still runs");
        let _ = std::fs::remove_dir_all(place);
    }

    /// **An effect no longer wanted gives its program back** at the next
    /// compile, even with nothing else wanted, and the host is idle after
    /// it: programs nothing references are deleted between frames (Ruling 7).
    #[test]
    fn an_effect_no_longer_wanted_gives_its_program_back() {
        let place = scratch("given-back");
        folder(&place, "a", ONE_PASS, &[("effect.frag", FRAG)]);
        let mut host = host_with(&place);
        host.want("rules", ["a".to_owned()]);
        let mut compiler = Counting::default();
        host.compile_pending(&mut compiler);
        assert!(
            compiler.deleted.is_empty(),
            "the premise: a's program is live"
        );
        host.want("rules", []);
        assert!(
            !host.is_idle(),
            "a program nothing references is still held"
        );
        host.compile_pending(&mut compiler);
        assert_eq!(compiler.deleted, vec![1], "a's program was not deleted");
        assert!(host.is_idle());
        let _ = std::fs::remove_dir_all(place);
    }

    /// A Lua error in a new version keeps the one that ran too, and a name
    /// nobody ships is a problem saying where it was looked for.
    #[test]
    fn a_lua_error_or_a_missing_folder_is_a_problem_and_nothing_else() {
        let place = scratch("lua-error");
        let dir = folder(&place, "a", ONE_PASS, &[("effect.frag", FRAG)]);
        let mut host = host_with(&place);
        host.want("rules", ["a".to_owned(), "nowhere".to_owned()]);
        host.compile_pending(&mut Counting::default());
        assert!(
            host.problems()
                .iter()
                .any(|each| each.effect == "nowhere" && each.message.contains("no effect"))
        );
        let first = host.effect("a").expect("v1");
        std::fs::write(dir.join("effect.lua"), "return {").expect("v2");
        host.reload();
        host.compile_pending(&mut Counting::default());
        assert!(std::rc::Rc::ptr_eq(
            &first,
            &host.effect("a").expect("still v1")
        ));
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A name no longer wanted takes its problems with it**, loaded or
    /// not: a name nobody ships, or one whose checks refused it on a cold
    /// start, is listed while it is wanted and not after.
    #[test]
    fn a_name_no_longer_wanted_takes_its_problems_with_it() {
        let place = scratch("unwanted-problems");
        folder(
            &place,
            "lint",
            ONE_PASS,
            &[(
                "effect.frag",
                "vec4 sol_effect(vec2 uv) {\n  return sol_tex(uv) * p_nothing;\n}\n",
            )],
        );
        let mut host = host_with(&place);
        host.want("rules", ["nowhere".to_owned(), "lint".to_owned()]);
        let named: std::collections::BTreeSet<String> = host
            .problems()
            .iter()
            .map(|each| each.effect.clone())
            .collect();
        assert_eq!(
            named,
            std::collections::BTreeSet::from(["lint".to_owned(), "nowhere".to_owned()]),
            "the premise"
        );
        host.want("rules", []);
        assert_eq!(host.problems(), &[], "a name nobody wants is still listed");
        let _ = std::fs::remove_dir_all(place);
    }

    /// **Every pane style's problems are cleared at once, in one change**:
    /// each effect named under the prefix and nothing else, with one bump of
    /// the generation the `problems` event is told by, and none when there
    /// was nothing to clear.
    #[test]
    fn clearing_by_prefix_is_one_change() {
        let place = scratch("prefixed-problems");
        let mut host = host_with(&place);
        for effect in ["style:a", "style:b", "rules"] {
            host.push_problem(super::Problem::error(
                effect,
                Path::new("effects.lua"),
                None,
                "wrong".to_owned(),
            ));
        }
        let before = host.problems_generation();
        host.clear_problems_prefixed("style:");
        let left: Vec<&str> = host
            .problems()
            .iter()
            .map(|each| each.effect.as_str())
            .collect();
        assert_eq!(left, ["rules"]);
        assert_eq!(host.problems_generation(), before + 1);
        host.clear_problems_prefixed("style:");
        assert_eq!(
            host.problems_generation(),
            before + 1,
            "nothing to clear is no change"
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// **Binding an effect that did not load is refused with why it did
    /// not**, at its own file and line, rather than as "not loaded".
    #[test]
    fn binding_an_effect_that_did_not_load_gives_its_own_problem() {
        let place = scratch("bind-unloaded");
        let dir = folder(
            &place,
            "lint",
            ONE_PASS,
            &[(
                "effect.frag",
                "vec4 sol_effect(vec2 uv) {\n  return sol_tex(uv) * p_nothing;\n}\n",
            )],
        );
        let mut host = host_with(&place);
        host.want("rules", ["lint".to_owned(), "nowhere".to_owned()]);
        let refused = host.bind("lint", &[]).expect_err("refused");
        assert_eq!(
            (refused.file, refused.line),
            (dir.join("effect.frag"), Some(2)),
            "{}",
            refused.message
        );
        let refused = host.bind("nowhere", &[]).expect_err("refused");
        assert!(refused.message.contains("looked in"), "{}", refused.message);
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A cold effect is current after the first compile** (spec C14): what
    /// `prepare` runs before it resolves a slot, so a wanted effect is never
    /// absent from the first frame for want of a compile (Ruling 7).
    #[test]
    fn a_cold_effect_is_ready_after_the_first_compile() {
        let place = scratch("warm");
        folder(&place, "a", ONE_PASS, &[("effect.frag", FRAG)]);
        let mut host = host_with(&place);
        host.want("rules", ["a".to_owned()]);
        assert!(
            !host.ready() && host.effect("a").is_none(),
            "the premise: cold and pending"
        );
        host.compile_pending(&mut Counting::default());
        assert!(host.ready());
        assert!(host.effect("a").is_some());
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A program asked for and not compiled yet looks pending**, a compiled
    /// one ready, a failed one failed: what `run::preflight` reads. A key
    /// nothing asked for is no program a later compile would bring, so it is
    /// a failure, not pending for ever.
    #[test]
    fn a_programs_lookup_tells_pending_from_failed() {
        use super::super::run::Lookup;
        let place = scratch("lookup");
        folder(&place, "a", ONE_PASS, &[("effect.frag", FRAG)]);
        folder(
            &place,
            "bad",
            ONE_PASS,
            &[("effect.frag", "vec4 sol_effect(vec2 uv) {\n  FAIL\n}\n")],
        );
        let mut host = host_with(&place);
        host.want("rules", ["a".to_owned(), "bad".to_owned()]);
        let a = host.bind("a", &[]).expect("binds").plans[0].steps[0].key;
        let bad = host.bind("bad", &[]).expect("binds").plans[0].steps[0].key;
        assert!(matches!(host.lookup(a), Lookup::Pending));
        assert!(
            matches!(host.lookup(0), Lookup::Failed),
            "asked for by nothing"
        );
        host.compile_pending(&mut Counting::default());
        assert!(matches!(host.lookup(a), Lookup::Ready(_)));
        assert!(matches!(host.lookup(bad), Lookup::Failed));
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A used effect is loaded with its user**, as a `pixels` or a
    /// `fallback` effect is: named by a `use` in a stage list, or in what
    /// `stages(p)` gives at the defaults. Reading the stages at load refuses
    /// a stage that cannot be read, as a problem at its file, and the
    /// version with it.
    #[test]
    fn a_used_effect_is_loaded_with_its_user_and_a_broken_stage_is_refused() {
        let place = scratch("used");
        folder(
            &place,
            "listed",
            "return { api = 1, inputs = { 'self' }, stages = { { 'use', 'plain' } } }",
            &[],
        );
        folder(
            &place,
            "called",
            "return { api = 1, inputs = { 'self' }, params = { n = { 1, int = true } },
                stages = function(p) return { { 'use', p.n == 1 and 'other' or 'never' } } end }",
            &[],
        );
        folder(&place, "plain", ONE_PASS, &[("effect.frag", FRAG)]);
        folder(&place, "other", ONE_PASS, &[("effect.frag", FRAG)]);
        folder(
            &place,
            "broken",
            "return { api = 1, stages = { { 'pass', 'a.frag', scal = 0.5 } } }",
            &[],
        );
        let mut host = host_with(&place);
        host.want("rules", ["listed", "called", "broken"].map(str::to_owned));
        assert!(
            host.has_pending("plain") && host.has_pending("other"),
            "a used effect was not loaded: {:?}",
            host.problems()
        );
        assert!(
            !host
                .problems()
                .iter()
                .any(|problem| problem.effect == "never"),
            "only the defaults' stages are followed: {:?}",
            host.problems()
        );
        assert!(!host.has_pending("broken"), "a broken stage was loaded");
        assert!(
            host.problems()
                .iter()
                .any(|problem| problem.effect == "broken" && problem.message.contains("scal")),
            "{:?}",
            host.problems()
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A reload reads a changed effect once**: the closure is settled until
    /// nothing new is wanted, each name loaded at most once a settle.
    #[test]
    fn a_reload_loads_each_effect_once() {
        let place = scratch("load-once");
        let dir = folder(&place, "a", ONE_PASS, &[("effect.frag", FRAG)]);
        let mut host = host_with(&place);
        host.want("rules", ["a".to_owned()]);
        host.compile_pending(&mut Counting::default());
        std::fs::write(
            dir.join("effect.frag"),
            "vec4 sol_effect(vec2 uv) { return vec4(1.0); }\n",
        )
        .expect("v2");
        let before = host.loads;
        host.reload();
        assert_eq!(
            host.loads - before,
            1,
            "a changed effect was loaded more than once by one reload"
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// **New caps load every wanted folder again**, and only then: an
    /// effect the default 16 MiB refused loads once `effects.sandbox` gives
    /// it 64, one compiled and unchanged is loaded again under them (its
    /// frame-time memory cap is the new one too), and the same caps again
    /// load nothing.
    #[test]
    fn new_caps_load_every_wanted_folder_again() {
        let place = scratch("new-caps");
        folder(
            &place,
            "big",
            "local big = string.rep('x', 20 * 1024 * 1024)\nreturn { api = 1, frag = 'effect.frag' }",
            &[("effect.frag", FRAG)],
        );
        folder(&place, "small", ONE_PASS, &[("effect.frag", FRAG)]);
        let mut host = host_with(&place);
        host.want("rules", ["big".to_owned(), "small".to_owned()]);
        assert!(host.latest("big").is_none(), "20 MiB loaded under 16");
        // Compiled, so `small` is current and nothing of it pending: only
        // the caps can make its unchanged folder load again.
        host.compile_pending(&mut Counting::default());
        assert!(
            host.effect("small").is_some() && !host.has_pending("small"),
            "the premise: small runs, and nothing of it waits"
        );
        // A long budget beside it, so a busy machine building the string
        // slowly is not what is tested.
        let big = crate::effect::settings::Caps {
            memory: 64 << 20,
            load: std::time::Duration::from_millis(5000),
        };
        assert!(host.set_caps(big));
        assert_eq!(host.caps(), big);
        assert!(
            host.latest("big").is_some(),
            "not loaded again under 64 MiB: {:?}",
            host.problems()
        );
        assert!(
            host.problems().iter().all(|each| each.effect != "big"),
            "{:?}",
            host.problems()
        );
        assert_eq!(
            host.latest("small").map(|loaded| loaded.sandbox().caps()),
            Some(big),
            "an unchanged folder kept the caps it loaded under"
        );
        host.compile_pending(&mut Counting::default());
        assert_eq!(
            host.effect("small").map(|loaded| loaded.sandbox().caps()),
            Some(big),
            "the version that runs is not the one loaded under the new caps"
        );
        let before = host.loads;
        assert!(!host.set_caps(big), "the same caps are no change");
        assert_eq!(host.loads, before, "the same caps loaded a folder again");
        let _ = std::fs::remove_dir_all(place);
    }

    /// **New caps keep an unchanged folder's id**: what loads again only
    /// because `effects.sandbox` changed is the same version under new caps,
    /// so a geometry swapped in at once and one whose frag compiles first
    /// both keep the id a present carries, and the id they take while
    /// compiling ([`Host::upcoming`]) is that one too; an edit is still a
    /// new generation (`a_new_version_is_a_new_generation`).
    #[test]
    fn new_caps_keep_an_unchanged_folders_id() {
        let place = scratch("new-caps-id");
        folder(&place, "flat", crate::effect::geometry::tests::FLAT, &[]);
        folder(&place, "frag", ONE_PASS, &[("effect.frag", FRAG)]);
        let mut host = host_with(&place);
        host.want("rules", ["flat".to_owned(), "frag".to_owned()]);
        host.compile_pending(&mut Counting::default());
        let (flat, frag) = (
            host.id("flat").expect("flat runs"),
            host.id("frag").expect("frag runs"),
        );
        let caps = crate::effect::settings::Caps {
            memory: 32 << 20,
            load: std::time::Duration::from_millis(5000),
        };
        assert!(host.set_caps(caps));
        assert!(
            host.has_pending("frag") && !host.has_pending("flat"),
            "the premise: both loaded again, and only the frag waits"
        );
        assert_eq!(host.id("flat"), Some(flat), "a geometry lost its id");
        assert_eq!(
            host.upcoming("frag"),
            Some(frag),
            "a frag compiling again under new caps was given a new id"
        );
        host.compile_pending(&mut Counting::default());
        assert_eq!(host.id("frag"), Some(frag), "a frag lost its id");
        for (name, id) in [("flat", flat), ("frag", frag)] {
            assert_eq!(
                host.by_id(id).map(|loaded| loaded.sandbox().caps()),
                Some(caps),
                "{name}'s id does not name the version under the new caps"
            );
        }
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A mesh stopped at load names its budget and its key**: under
    /// `effects.sandbox.load_ms = 250` a `mesh` that never returns is one
    /// problem, saying 250 ms and the key.
    #[test]
    fn a_mesh_stopped_at_load_names_its_budget_and_key() {
        let place = scratch("stopped-at-load");
        folder(
            &place,
            "endless",
            "return { api = 1, grid = { 1, 1 }, mesh = function(t, cols, rows, out) while true do end end }",
            &[],
        );
        let mut host = host_with(&place);
        host.set_caps(crate::effect::settings::Caps {
            load: std::time::Duration::from_millis(250),
            ..crate::effect::settings::Caps::default()
        });
        host.want("present", ["endless".to_owned()]);
        let problems: Vec<_> = host
            .problems()
            .iter()
            .filter(|each| each.effect == "endless")
            .collect();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].message.contains("250 ms")
                && problems[0].message.contains("effects.sandbox.load_ms"),
            "{problems:?}"
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A version with nothing to compile is current at once**: a
    /// geometry-only effect `sol.present` names resolves its id on the pass
    /// it is named, with no compile between.
    #[test]
    fn a_version_with_nothing_to_compile_is_current_at_once() {
        let place = scratch("current-at-once");
        folder(&place, "flat", crate::effect::geometry::tests::FLAT, &[]);
        let mut host = host_with(&place);
        host.add_wanted("present", ["flat".to_owned()]);
        assert!(
            host.id("flat").is_some() && !host.has_pending("flat"),
            "a geometry-only effect waited for a compile"
        );
        assert!(host.ready());
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A geometry file that moves the window at rest is refused at load**,
    /// a problem at its `mesh`'s line, and no version at all.
    #[test]
    fn a_geometry_file_that_moves_the_window_at_rest_is_a_problem_at_its_mesh() {
        let fixtures = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/effects"
        ));
        let mut host: super::Host<u32> = super::Host::new(Library::with(None, fixtures.to_owned()));
        host.want("present", ["mover".to_owned()]);
        assert!(
            host.latest("mover").is_none(),
            "a version that moves at rest was loaded"
        );
        let problem = host
            .problems()
            .iter()
            .find(|each| each.effect == "mover")
            .expect("a problem");
        assert_eq!(problem.line, Some(7), "{problem:?}");
        assert!(problem.message.contains("progress 0"), "{problem:?}");
        assert!(problem.file.ends_with("mover/effect.lua"), "{problem:?}");
    }

    /// **A refusal at load says why, at its `mesh`'s line**: a mesh too
    /// slow for a frame says how long a call took and what a frame gives;
    /// one the load budget stopped says it was the checks' budget.
    #[test]
    fn a_mesh_too_slow_for_a_frame_is_a_problem_at_its_mesh() {
        let place = scratch("slow-problem");
        let dir = folder(&place, "flat", crate::effect::geometry::tests::FLAT, &[]);
        let loaded = super::Loaded::<u32>::load("flat", &dir).expect("loads");
        let slow = super::at_rest_problem(
            &loaded,
            super::Refusal::Slow {
                took: std::time::Duration::from_micros(3400),
            },
        );
        assert_eq!(
            slow.message,
            "its mesh takes 3.4 ms a call; a frame gives it 2 ms"
        );
        assert_eq!(slow.line, Some(1), "{slow:?}");
        let stopped = super::at_rest_problem(&loaded, super::Refusal::Budget);
        assert!(
            stopped.message.contains("100 ms its checks at load have"),
            "{stopped:?}"
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// A host wanting `spin` (`geometry::tests::SPIN`) in `place`, its
    /// running version stopped by its budget: what `revive` is for.
    fn stopped_in(place: &Path) -> (super::Host<u32>, std::rc::Rc<super::Loaded<u32>>) {
        let mut host = host_with(place);
        host.want("rules", ["spin".to_owned()]);
        host.compile_pending(&mut Counting::default());
        let stopped = host.effect("spin").expect("current");
        stopped
            .sandbox()
            .lua()
            .globals()
            .set("spin", true)
            .expect("set");
        assert_eq!(
            crate::effect::geometry::mesh(
                stopped.sandbox(),
                &crate::effect::geometry::tests::ask(),
                1,
                1
            ),
            Err(super::Refusal::Budget)
        );
        (host, stopped)
    }

    /// **A stopped state whose folder changed waits for the reload** (Ruling
    /// 4): the changed folder's Lua is not run, nothing is swapped in under
    /// the old version's id, and the reload reads it as a new version.
    #[test]
    fn a_stopped_state_whose_folder_changed_waits_for_the_reload() {
        let place = scratch("revive-changed");
        let dir = folder(&place, "spin", crate::effect::geometry::tests::SPIN, &[]);
        let (mut host, _stopped) = stopped_in(&place);
        let id = host.id("spin");
        std::fs::write(dir.join("effect.lua"), crate::effect::geometry::tests::FLAT)
            .expect("a save");
        let read = std::cell::Cell::new(0);
        host.revive_with(|name, dir| {
            read.set(read.get() + 1);
            super::Loaded::load(name, dir)
        });
        assert_eq!(read.get(), 0, "a changed folder's Lua was run");
        assert!(
            host.effect("spin").expect("loaded").sandbox().poisoned(),
            "a changed folder was swapped in under the old version"
        );
        assert_eq!(host.id("spin"), id);
        host.reload();
        assert!(!host.effect("spin").expect("loaded").sandbox().poisoned());
        assert_ne!(host.id("spin"), id, "the reload's is a new version");
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A folder saved during a rebuild is not swapped in**: the hash is
    /// read again after the load, so what the load read is the folder the
    /// version was, or nothing changes (here the load reads another folder,
    /// as a save between the two reads would make it).
    #[test]
    fn a_folder_saved_during_a_rebuild_is_not_swapped_in() {
        let place = scratch("revive-saved");
        folder(&place, "spin", crate::effect::geometry::tests::SPIN, &[]);
        let other = folder(&place, "other", crate::effect::geometry::tests::FLAT, &[]);
        let (mut host, _stopped) = stopped_in(&place);
        host.revive_with(|name, _| super::Loaded::load(name, &other));
        assert!(
            host.effect("spin").expect("loaded").sandbox().poisoned(),
            "a load of other files was swapped in"
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A rebuilt state keeps what its version needs** (Ruling 4): its
    /// programs stay its own, and `needs` says which.
    #[test]
    fn a_rebuilt_state_keeps_what_its_version_needs() {
        let place = scratch("revive-needs");
        let lua = crate::effect::geometry::tests::SPIN
            .replace("grid = { 1, 1 },", "grid = { 1, 1 }, frag = 'effect.frag',");
        folder(
            &place,
            "spin",
            &lua,
            &[(
                "effect.frag",
                "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n",
            )],
        );
        let (mut host, stopped) = stopped_in(&place);
        assert!(!stopped.needs.is_empty(), "its frag is a program it needs");
        host.revive();
        let fresh = host.effect("spin").expect("loaded");
        assert!(!fresh.sandbox().poisoned());
        assert_eq!(fresh.needs, stopped.needs);
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A rebuild that fails is a problem, and waits for a reload**: it is
    /// named on the overlay once, the next frame's `revive` reads nothing,
    /// and a reload of the unchanged folder builds it again and takes the
    /// problem away.
    #[test]
    fn a_rebuild_that_fails_is_a_problem_and_waits_for_a_reload() {
        let place = scratch("revive-fails");
        folder(&place, "spin", crate::effect::geometry::tests::SPIN, &[]);
        let (mut host, _stopped) = stopped_in(&place);
        let failed = "it did not load again";
        host.revive_with(|name, dir| {
            Err(super::Problem::error(
                name,
                &dir.join("effect.lua"),
                None,
                failed.to_owned(),
            ))
        });
        assert!(
            host.problems()
                .iter()
                .any(|each| each.effect == "spin" && each.message == failed),
            "{:?}",
            host.problems()
        );
        assert!(host.effect("spin").expect("loaded").sandbox().poisoned());
        let before = host.loads;
        host.revive();
        assert_eq!(
            host.loads, before,
            "a failed rebuild was tried again before a reload"
        );
        host.reload();
        assert!(
            !host.effect("spin").expect("loaded").sandbox().poisoned(),
            "a reload of the unchanged folder left it stopped"
        );
        assert!(
            host.problems().iter().all(|each| each.message != failed),
            "{:?}",
            host.problems()
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// **`sol.present`'s names accumulate until a reload**: a second genie
    /// naming another effect does not drop the first's mid-flight.
    #[test]
    fn present_wants_accumulate_until_a_reload() {
        let place = scratch("present-wants");
        folder(&place, "a", ONE_PASS, &[("effect.frag", FRAG)]);
        folder(&place, "b", ONE_PASS, &[("effect.frag", FRAG)]);
        let mut host = host_with(&place);
        host.add_wanted("present", ["a".to_owned()]);
        host.add_wanted("present", ["b".to_owned()]);
        assert!(
            host.has_pending("a") && host.has_pending("b"),
            "the second genie dropped the first's effect"
        );
        host.push_problem(super::Problem::error(
            "present:a",
            Path::new("a"),
            None,
            "sol.present: too many params".to_owned(),
        ));
        host.reload();
        assert!(
            !host.has_pending("a") && host.effect("a").is_none(),
            "a reload keeps sol.present's names"
        );
        assert!(
            host.problems()
                .iter()
                .all(|each| each.effect != "present:a"),
            "a reload keeps what binding a present said: {:?}",
            host.problems()
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A reload keeps what a live present names**: of two names presents
    /// wanted, the one a geometry under way still names stays, with its id
    /// while its folder is unchanged, and the other goes; once that folder
    /// changes, a reload gives it a new version, and the old id answers
    /// nothing.
    #[test]
    fn a_reload_keeps_what_a_live_present_names() {
        let flat = crate::effect::geometry::tests::FLAT;
        let place = scratch("present-live");
        folder(&place, "a", flat, &[]);
        folder(&place, "b", flat, &[]);
        let mut host = host_with(&place);
        host.add_wanted("present", ["a".to_owned(), "b".to_owned()]);
        let id = host.id("a").expect("a geometry file is current at once");
        assert_eq!(host.name_of(id), Some("a"));
        host.reload_keeping(["a".to_owned()]);
        assert_eq!(
            host.id("a"),
            Some(id),
            "an unchanged folder a present names lost its id"
        );
        assert!(host.effect("b").is_none(), "a name nothing draws was kept");
        std::fs::write(place.join("a/effect.lua"), format!("{flat}\n-- edited\n")).expect("edited");
        host.reload_keeping(["a".to_owned()]);
        assert!(
            host.id("a").is_some_and(|now| now != id) && host.by_id(id).is_none(),
            "a changed folder kept the version a present began with"
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A log numbered across the strings is mapped back** (Ruling 6): a
    /// driver that ignored `#line` reports string 0 at the prelude's length
    /// plus the user's line; a line inside the prelude is the engine's.
    #[test]
    fn a_log_numbered_across_the_strings_is_mapped_back() {
        let spec = solium_effects::spec::EffectSpec {
            api: 1,
            frag: Some("effect.frag".to_owned()),
            ..Default::default()
        };
        let sources = solium_effects::glsl::assemble(
            &default_signature(&spec, solium_effects::glsl::Host::Pass),
            FRAG,
        );
        let prelude = sources.prelude.lines().count();
        let frag = Path::new("/x/a/effect.frag");
        let found = super::log_problems(
            "a",
            frag,
            &sources,
            &format!("0:{}(5): error: `x' undeclared", prelude + 2),
            0,
        );
        assert_eq!(found[0].line, Some(2), "{found:?}");
        assert!(!found[0].message.contains("Solium bug"), "{found:?}");
        let engine = super::log_problems("a", frag, &sources, "0:3(1): error: y", 0);
        assert!(engine[0].message.contains("Solium bug"), "{engine:?}");
    }

    /// **A replaced version gives its program back**: once the new one
    /// swaps in, the program only the old one needed is deleted between
    /// frames (Ruling 7).
    #[test]
    fn a_replaced_version_gives_its_program_back() {
        let place = scratch("replaced");
        let dir = folder(&place, "a", ONE_PASS, &[("effect.frag", FRAG)]);
        let mut host = host_with(&place);
        host.want("rules", ["a".to_owned()]);
        let mut compiler = Counting::default();
        host.compile_pending(&mut compiler);
        std::fs::write(
            dir.join("effect.frag"),
            "vec4 sol_effect(vec2 uv) { return vec4(1.0); }\n",
        )
        .expect("v2");
        host.reload();
        host.compile_pending(&mut compiler);
        assert_eq!(compiler.compiled.len(), 2, "the premise: v2 compiled");
        assert_eq!(compiler.deleted, vec![1], "v1's program was kept");
        let _ = std::fs::remove_dir_all(place);
    }

    /// **An effect changed under its user gives its old program back**: a
    /// reload that changes only the folder of an effect others splice in,
    /// through a `use` (at any depth) or a `fallback`, binds those others
    /// again, so each holds the new version's program and the old one is
    /// deleted between frames (Ruling 7).
    #[test]
    fn an_effect_changed_under_its_user_gives_its_old_program_back() {
        let place = scratch("changed-under");
        folder(
            &place,
            "outer",
            "return { api = 1, inputs = { 'self' }, stages = { { 'use', 'user' } } }",
            &[],
        );
        folder(
            &place,
            "user",
            "return { api = 1, inputs = { 'self' }, stages = { { 'use', 'plain' } } }",
            &[],
        );
        folder(
            &place,
            "rich",
            "return { api = 1, inputs = { 'self' }, frag = 'rich.frag', fallback = { 'plain' } }",
            &[(
                "rich.frag",
                "vec4 sol_effect(vec2 uv) { return sol_tex(uv).bgra; }\n",
            )],
        );
        let plain = folder(&place, "plain", ONE_PASS, &[("effect.frag", FRAG)]);
        let mut host = host_with(&place);
        host.want("rules", ["outer".to_owned(), "rich".to_owned()]);
        let mut compiler = Counting::default();
        host.compile_pending(&mut compiler);
        let old = host.holds("plain");
        assert_eq!(old.len(), 1, "the premise: plain runs");
        assert!(
            host.holds("outer") == old && host.holds("user") == old,
            "the premise: its users hold plain's program"
        );
        assert!(host.holds("rich").contains(&old[0]));
        std::fs::write(
            plain.join("effect.frag"),
            "vec4 sol_effect(vec2 uv) { return vec4(1.0); }\n",
        )
        .expect("v2");
        host.reload();
        host.compile_pending(&mut compiler);
        let new = host.holds("plain");
        assert!(new.len() == 1 && new != old, "the premise: plain's v2 runs");
        assert_eq!(host.holds("user"), new, "a use still holds plain's v1");
        assert_eq!(
            host.holds("outer"),
            new,
            "a use of a use still holds plain's v1"
        );
        let rich = host.holds("rich");
        assert!(
            rich.contains(&new[0]) && !rich.contains(&old[0]),
            "a fallback still holds plain's v1: {rich:?}"
        );
        let v1 = compiler
            .compiled
            .iter()
            .position(|key| *key == old[0])
            .expect("plain's v1 compiled");
        assert_eq!(
            compiler.deleted,
            vec![u32::try_from(v1).expect("a short list") + 1],
            "plain's v1 program was kept"
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A driver that numbers the user's file from 0 is calibrated once**
    /// (Ruling 6; NVIDIA does, wirecheck case 12a): its log's line is one
    /// lower than the user's, the host asks the compiler's line rule once,
    /// and not before a compile fails, and every problem is at its own line.
    #[test]
    fn a_driver_numbering_from_0_after_line_is_calibrated_once() {
        let place = scratch("shifted");
        folder(&place, "good", ONE_PASS, &[("effect.frag", FRAG)]);
        let two = "vec4 sol_effect(vec2 uv) {\n  FAIL\n}\n";
        let three = "vec4 sol_effect(vec2 uv) {\n  vec4 c = vec4(0.0);\n  FAIL\n}\n";
        folder(&place, "a", ONE_PASS, &[("effect.frag", two)]);
        folder(&place, "b", ONE_PASS, &[("effect.frag", three)]);
        let mut host = host_with(&place);
        let mut compiler = Counting {
            shift: 1,
            ..Counting::default()
        };
        host.want("rules", ["good".to_owned()]);
        host.compile_pending(&mut compiler);
        assert_eq!(compiler.probed, 0, "asked with nothing failed");
        host.want("rules", ["good".to_owned(), "a".to_owned(), "b".to_owned()]);
        host.compile_pending(&mut compiler);
        let line = |name: &str| {
            host.problems()
                .iter()
                .find(|each| each.effect == name)
                .and_then(|each| each.line)
        };
        assert_eq!((line("a"), line("b")), (Some(2), Some(3)));
        assert_eq!(compiler.probed, 1, "the driver's rule is asked once");
        let _ = std::fs::remove_dir_all(place);
    }

    /// An id outlives nothing: a new version is a new generation, and the
    /// old id resolves to nothing, as an unresolved anchor does.
    #[test]
    fn a_new_version_is_a_new_generation() {
        let place = scratch("generation");
        let dir = folder(&place, "a", ONE_PASS, &[("effect.frag", FRAG)]);
        let mut host = host_with(&place);
        host.want("rules", ["a".to_owned()]);
        host.compile_pending(&mut Counting::default());
        let old = host.id("a").expect("an id");
        std::fs::write(
            dir.join("effect.frag"),
            "vec4 sol_effect(vec2 uv) { return vec4(1.0); }\n",
        )
        .expect("v2");
        host.reload();
        host.compile_pending(&mut Counting::default());
        assert!(host.by_id(old).is_none());
        assert!(host.id("a").is_some_and(|new| new != old));
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A configuration error is put where it is**: the first
    /// `<file>.lua:<line>:` in the error names the file and the line, a path
    /// Lua cut short with `...` included, and an error naming none is the
    /// configuration's, with no line.
    #[test]
    fn a_config_problem_is_at_the_first_lua_file_and_line_in_the_error() {
        let problem = super::config_problem(
            "running the configuration: syntax error: /home/u/.config/solium/init.lua:6: \
             syntax error near 'is'",
        );
        assert_eq!(
            (
                problem.effect.as_str(),
                problem.file.as_path(),
                problem.line,
                problem.message.as_str()
            ),
            (
                "config",
                Path::new("/home/u/.config/solium/init.lua"),
                Some(6),
                "syntax error near 'is'"
            )
        );
        let module = super::config_problem(
            "running the configuration: runtime error: ...ong/way/down/solium/lua/tiling.lua:12: \
             attempt to index a nil value\nstack traceback:\n\t[C]: in ?",
        );
        assert_eq!(
            (module.file.as_path(), module.line, module.message.as_str()),
            (
                Path::new("...ong/way/down/solium/lua/tiling.lua"),
                Some(12),
                "attempt to index a nil value"
            )
        );
        let nowhere = super::config_problem("reading /x/init.lua: No such file or directory");
        assert_eq!(
            (
                nowhere.file.as_path(),
                nowhere.line,
                nowhere.message.as_str()
            ),
            (
                Path::new("init.lua"),
                None,
                "reading /x/init.lua: No such file or directory"
            )
        );
    }

    /// **A stage asking for `rgba16f` where it is missing takes the
    /// fallback**: the configured plan is dropped at load and the first rung
    /// needing only `rgba8` comes first.
    #[test]
    fn a_stage_asking_for_rgba16f_where_it_is_missing_takes_the_fallback() {
        let place = scratch("formats");
        let frag = "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n";
        folder(
            &place,
            "fine",
            "return { api = 1, inputs = { 'self' }, params = { field = { 1, int = true } }, fallback = { { field = 0 } },
            stages = function(p) if p.field == 1 then return { { 'pass', 'a.frag', format = 'rgba16f' } } end return { { 'pass', 'a.frag' } } end }",
            &[("a.frag", frag)],
        );
        let mut host = host_with(&place);
        host.want("rules", ["fine".to_owned()]);
        assert_eq!(
            host.bind("fine", &[]).expect("binds").plans.len(),
            2,
            "formats not probed yet are unknown, not missing: every rung is kept"
        );
        assert!(
            host.set_formats(crate::pool::Formats { rgba16f: false }),
            "the first answer changes what is known"
        );
        let bound = host.bind("fine", &[]).expect("binds");
        assert_eq!(bound.plans.len(), 1, "the rgba16f plan was dropped");
        assert_eq!(
            bound.plans[0].steps[0].format,
            solium_effects::stage::Format::Rgba8
        );
        assert!(
            !host.set_formats(crate::pool::Formats { rgba16f: false }),
            "the same answer again asks for no rebind"
        );
        assert!(host.set_formats(crate::pool::Formats { rgba16f: true }));
        assert_eq!(host.bind("fine", &[]).expect("binds").plans.len(), 2);
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A fallback naming another effect binds it at load**, and its `use`
    /// closure with it; and **`stages` runs once per rung at bind**, never
    /// again (its counter does not move over a hundred lookups).
    #[test]
    fn a_fallback_naming_another_effect_binds_it_at_load() {
        let place = scratch("fallback-effect");
        let frag = "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n";
        folder(
            &place,
            "rich",
            "calls = 0 return { api = 1, inputs = { 'self' }, fallback = { 'plain' }, stages = function(p) calls = calls + 1 return { { 'use', 'plain' } } end }",
            &[],
        );
        folder(
            &place,
            "plain",
            "return { api = 1, inputs = { 'self' }, frag = 'a.frag' }",
            &[("a.frag", frag)],
        );
        let mut host = host_with(&place);
        host.want("rules", ["rich".to_owned()]);
        let bound = host.bind("rich", &[]).expect("binds");
        assert_eq!(bound.plans.len(), 2);
        assert!(
            host.effect("plain").is_some() || host.has_pending("plain"),
            "loaded with its user"
        );
        let rich = host.latest("rich").expect("loaded");
        for _ in 0..100 {
            let _ = host.latest("rich");
        }
        let calls: i64 = rich
            .sandbox()
            .lua()
            .globals()
            .get("calls")
            .expect("the counter");
        assert_eq!(calls, 1, "stages ran per lookup, not once at bind");
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A many-pass effect swaps in only once every step compiled**, each
    /// step's program asked for at load: one `.frag` of two failing keeps the
    /// effect absent and names that file at its line, and a stage's `.frag`
    /// that fails its lints refuses the version at load, asking for nothing.
    #[test]
    fn a_stage_effect_swaps_in_only_once_every_step_compiled() {
        let place = scratch("every-step");
        let stages = "return { api = 1, inputs = { 'self' }, stages = { { 'pass', 'a.frag', scale = 0.5 }, { 'pass', 'b.frag', scale = 2 } } }";
        let failing = "vec4 sol_effect(vec2 uv) {\n  FAIL\n}\n";
        let linted = "vec4 sol_effect(vec2 uv) {\n  return sol_tex(uv) * p_nothing;\n}\n";
        let lone = "vec4 sol_effect(vec2 uv) {\n  return sol_tex(uv).bgra;\n}\n";
        folder(
            &place,
            "two",
            stages,
            &[("a.frag", FRAG), ("b.frag", failing)],
        );
        folder(
            &place,
            "linted",
            stages,
            &[("a.frag", lone), ("b.frag", linted)],
        );
        let mut host = host_with(&place);
        host.want("rules", ["two".to_owned(), "linted".to_owned()]);
        assert!(
            !host.has_pending("linted"),
            "a stage's lint error was loaded"
        );
        let at = |effect: &str, file: &str, problems: &[super::Problem]| {
            problems.iter().any(|problem| {
                problem.effect == effect && problem.file.ends_with(file) && problem.line == Some(2)
            })
        };
        assert!(
            at("linted", "linted/b.frag", host.problems()),
            "{:?}",
            host.problems()
        );
        let mut compiler = Counting::default();
        host.compile_pending(&mut compiler);
        assert_eq!(
            compiler.compiled.len(),
            2,
            "each of two's steps asked for, and nothing of the refused version"
        );
        assert!(
            host.effect("two").is_none(),
            "swapped in with a step that did not compile"
        );
        assert!(
            at("two", "two/b.frag", host.problems()),
            "{:?}",
            host.problems()
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A reload whose stage fails its lints keeps the one that ran**: the
    /// new version is refused at load, at the `.frag`'s line, and the old
    /// one still runs.
    #[test]
    fn a_reload_whose_stage_fails_its_lints_keeps_the_one_that_ran() {
        let place = scratch("stage-lint-reload");
        let dir = folder(
            &place,
            "a",
            "return { api = 1, inputs = { 'self' }, stages = { { 'pass', 'a.frag', scale = 0.5 } } }",
            &[("a.frag", FRAG)],
        );
        let mut host = host_with(&place);
        host.want("rules", ["a".to_owned()]);
        let mut compiler = Counting::default();
        host.compile_pending(&mut compiler);
        let first = host.effect("a").expect("v1");
        std::fs::write(
            dir.join("a.frag"),
            "vec4 sol_effect(vec2 uv) {\n  return sol_tex(uv) * p_nothing;\n}\n",
        )
        .expect("v2");
        host.reload();
        assert!(!host.has_pending("a"), "a stage's lint error was loaded");
        host.compile_pending(&mut compiler);
        assert!(
            std::rc::Rc::ptr_eq(&first, &host.effect("a").expect("still v1")),
            "the refused v2 replaced v1"
        );
        assert!(
            host.problems()
                .iter()
                .any(|each| each.file == dir.join("a.frag") && each.line == Some(2)),
            "{:?}",
            host.problems()
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A broken `.frag` two effects bind is named once**: an effect using
    /// another reads its steps at bind, and both refuse at the one line.
    #[test]
    fn a_broken_frag_two_effects_bind_is_named_once() {
        let place = scratch("named-once");
        let broken = "vec4 sol_effect(vec2 uv) {\n  return sol_tex(uv) * p_nothing;\n}\n";
        folder(
            &place,
            "user",
            "return { api = 1, inputs = { 'self' }, stages = { { 'use', 'plain' } } }",
            &[],
        );
        folder(&place, "plain", ONE_PASS, &[("effect.frag", broken)]);
        let mut host = host_with(&place);
        host.want("rules", ["user".to_owned()]);
        let named: Vec<_> = host
            .problems()
            .iter()
            .filter(|problem| problem.file.ends_with("plain/effect.frag"))
            .collect();
        assert_eq!(named.len(), 1, "{:?}", host.problems());
        assert!(!host.has_pending("user") && !host.has_pending("plain"));
        let _ = std::fs::remove_dir_all(place);
    }

    /// **The formats are probed once, and only while something is wanted**:
    /// `prepare` asks this before it makes a 1×1 target of each format, so a
    /// desktop with no effect configured never touches GL for it.
    #[test]
    fn the_formats_are_probed_once_and_only_while_something_is_wanted() {
        let place = scratch("probe-once");
        folder(&place, "a", ONE_PASS, &[("effect.frag", FRAG)]);
        let mut host = host_with(&place);
        assert!(!host.wants_formats(), "probed with nothing wanted");
        host.want("rules", ["a".to_owned()]);
        assert!(host.wants_formats());
        assert!(host.set_formats(crate::pool::Formats { rgba16f: true }));
        assert_eq!(host.formats(), Some(crate::pool::Formats { rgba16f: true }));
        assert!(!host.wants_formats(), "probed twice");
        let _ = std::fs::remove_dir_all(place);
    }

    fn shipped() -> PathBuf {
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/effects")).to_path_buf()
    }

    /// **The shipped blur loads and binds**: six steps at its defaults (three
    /// down, three up), its rungs four and two, its reach 48 px.
    #[test]
    fn the_shipped_blur_loads_and_binds_at_every_rung() {
        let mut host: super::Host<u32> = super::Host::new(Library::with(None, shipped()));
        host.want("rules", ["blur".to_owned()]);
        let bound = host.bind("blur", &[]).expect("binds");
        let steps: Vec<usize> = bound.plans.iter().map(|plan| plan.steps.len()).collect();
        assert_eq!(steps, [6, 4, 2]);
        assert!((bound.reach - 48.0).abs() < f64::EPSILON, "{}", bound.reach);
        assert!(
            bound.plans[0].reads.backdrop,
            "its input is the backdrop, rebound by a rule's source"
        );
    }

    /// **A user's `blur` folder shadows the shipped one**, which is there to
    /// be shadowed.
    #[test]
    fn a_users_blur_folder_shadows_the_shipped_one() {
        assert_eq!(
            Library::with(None, shipped()).resolve("blur"),
            Some(shipped().join("blur")),
            "no shipped blur to shadow"
        );
        let user = scratch("user-blur");
        let mine = folder(
            &user,
            "blur",
            "return { api = 1, inputs = { 'backdrop' }, frag = 'effect.frag' }",
            &[(
                "effect.frag",
                "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n",
            )],
        );
        assert_eq!(
            Library::with(Some(user.clone()), shipped()).resolve("blur"),
            Some(mine)
        );
        let _ = std::fs::remove_dir_all(user);
    }
}
