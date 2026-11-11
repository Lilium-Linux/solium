//! The window manager as the compositor holds it: what it loaded with, what
//! it declared, what it was last told, and what it has not answered yet.
//!
//! The window manager answers batches in the order it was sent them. The
//! compositor waits for each answer, so the next event is told to a window
//! manager that knows what the last one came to, but only `wm.ask_ms`, one
//! frame: after that it goes on with its own answer (the entry points in
//! [`crate::wm::events`] say what each is), and the window manager's is
//! applied when it comes, in the order it came. While one is late, nothing
//! after it is waited for either, until the window manager has caught up:
//! a slow one costs a frame once, not a frame an event. An answer nobody is
//! waiting for is kept in [`Scripts::late`], which
//! [`crate::state::Solium::apply`] empties before anything else.
//!
//! A test waits for every answer however long it takes, so what it asserts
//! has happened by the time it looks; `script::tests::waits` keeps the
//! session's time.

use std::collections::VecDeque;
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

use anyhow::{Result, bail};
use solium_wm_protocol::{self as wire, Message};

use crate::script::*;
use crate::wm::events::Answers;
use crate::wm::link::Link;
use crate::wm::stores::StoreReads;
use crate::wm::{Chosen, Settings, Wait};
use solium_wm_protocol::Ask;

/// A batch the window manager has not answered yet.
#[derive(Debug)]
struct Pending {
    serial: u64,
    /// The event the batch was, by its name: what its answer's time is
    /// counted under.
    event: &'static str,
    sent: Instant,
    /// Who answers what the window manager asks while it handles the batch:
    /// the compositor, over the desktop as the batch found it.
    answers: Answers,
    /// A binding fired at the lock screen: its answer is held to the lock's
    /// rule whenever it comes.
    at_the_lock: bool,
    /// A window going fullscreen or maximised, or leaving either.
    change: bool,
}

/// An answer that came with nobody waiting for it.
#[derive(Debug)]
pub(crate) struct Late {
    pub(crate) outcome: Outcome,
    /// It answers a window's fullscreen or maximise, and is applied as one:
    /// a listener that turns the change back is not told it again.
    pub(crate) change: bool,
}

/// What [`Scripts::hear`] reads for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Awaited {
    /// Nothing: whatever has come is kept for later.
    Nothing,
    /// The answer to this batch.
    Answer(u64),
    /// The answer to a reload.
    Reloaded,
}

/// What it read.
enum Heard {
    Answer(Option<String>, Outcome),
    Reloaded(wire::Reloaded),
}

/// What the last answer to each kind of ask said, which is the
/// compositor's own answer to the next one the window manager is late for.
#[derive(Debug, Default)]
pub(crate) struct Took {
    /// A modified wheel turn.
    pub(crate) scroll: bool,
    /// A window's own resize.
    pub(crate) resize: bool,
}

/// The window manager, as the compositor talks to it: over its socket.
#[derive(Debug)]
pub(crate) struct Scripts {
    pub(crate) link: Link,
    /// What the window manager was last told of the desktop, so each batch
    /// tells it only what changed.
    pub(crate) told: crate::wm::diff::Told,
    /// The last batch's serial.
    pub(crate) serial: u64,
    /// The batches sent and not answered yet, oldest first.
    pending: VecDeque<Pending>,
    /// The answers that came with nobody waiting for them, oldest first.
    pub(crate) late: VecDeque<Late>,
    /// How long each kind of message is waited for: `wm`.
    pub(crate) settings: Settings,
    /// Whether every answer is waited for, as a test does.
    #[cfg(test)]
    pub(crate) patient: bool,
    /// The connection failed, or the window manager stopped answering,
    /// and why: nothing more is sent.
    down: Option<String>,
    /// The configuration it was started on, to start it on again.
    config: std::path::PathBuf,
    /// It was started to read nothing of the user's own.
    shipped_only: bool,
    /// What `wm.toml` said when it was started: the command it is, unless
    /// it is the shipped one, and the weight of its scope.
    chosen: Chosen,
    /// What was wrong with `wm.toml` then, a line each.
    chosen_said: Vec<String>,
    pub(crate) took: Took,
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
    /// What the window manager keeps, as it last said: held here for the
    /// one started in its place, should it stop.
    keeps: std::collections::BTreeMap<String, wire::Doc>,
    /// The events the window manager listens to, as it last said: no other
    /// is sent, but for those that need no listening
    /// ([`wire::listen::ALWAYS`]).
    listens: std::collections::BTreeSet<String>,
    /// Each bound combination the window manager wrote in a spelling of
    /// its own, by its one spelling.
    spellings: std::collections::HashMap<String, String>,
    /// How long its answers took.
    timing: crate::wm::timing::Timing,
}

/// What a window manager kept, by name, as the compositor holds it.
pub(crate) type Keeps = Vec<(String, wire::Doc)>;

/// How long a test waits for an answer: long enough that only a window
/// manager that is not going to answer misses it.
#[cfg(test)]
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);

impl Scripts {
    /// Load the configuration, cold.
    pub(crate) fn load(config: &Path) -> Result<Self> {
        Self::load_reading(config, StoreReads::default(), false, Vec::new())
    }

    /// Start a window manager on `config` in place of one that stopped,
    /// handing it what that one kept. With `shipped_only` it reads nothing
    /// of the user's own: [`Self::shipped_config`], in place of a
    /// configuration that keeps stopping.
    pub(crate) fn start_again(config: &Path, shipped_only: bool, keeps: Keeps) -> Result<Self> {
        Self::load_reading(config, StoreReads::default(), shipped_only, keeps)
    }

    /// The configuration that ships.
    pub(crate) fn shipped_config() -> std::path::PathBuf {
        crate::assets::lua().join("init.lua")
    }

    /// Load the configuration for `solium --check`: as [`Self::load`] does,
    /// except that nothing of the user's is changed, so a store's broken
    /// file stays where it is and is said in [`Self::check_notes`] (#473).
    pub(crate) fn load_for_check(config: &Path) -> Result<Self> {
        Self::load_reading(config, StoreReads::checking(), false, Vec::new())
    }

    fn load_reading(
        config: &Path,
        stores: StoreReads,
        shipped_only: bool,
        keeps: Keeps,
    ) -> Result<Self> {
        let stores = Rc::new(stores);
        let answers = Answers {
            snapshot: None,
            stores: Rc::clone(&stores),
        };
        let within = Settings::default().load;
        let (chosen, said) = Chosen::read();
        for line in &said {
            tracing::warn!(file = ?Chosen::file(), "{line}");
        }
        let (link, commands) = Link::start(config, shipped_only, &chosen, keeps, &answers, within)?;
        let mut scripts = Self::over(link, commands, stores, config, shipped_only);
        scripts.chosen = chosen;
        scripts.chosen_said = said;
        Ok(scripts)
    }

    /// A window manager that is not the Lua one, run by `run` on a thread
    /// with its end of the socket, as a program `wm.toml` names is run.
    #[cfg(test)]
    pub(crate) fn played(
        run: impl FnOnce(std::os::unix::net::UnixStream) + Send + 'static,
    ) -> Result<Self> {
        let stores = Rc::new(StoreReads::default());
        let answers = Answers {
            snapshot: None,
            stores: Rc::clone(&stores),
        };
        let (link, commands) = Link::played(run, &answers)?;
        Ok(Self::over(
            link,
            commands,
            stores,
            Path::new("/a/window/manager/of/its/own"),
            false,
        ))
    }

    /// The window manager at the other end of `link`, started on `config`,
    /// which answered `hello` with `commands`.
    pub(crate) fn over(
        link: Link,
        commands: Vec<wire::Command>,
        stores: Rc<StoreReads>,
        config: &Path,
        shipped_only: bool,
    ) -> Self {
        let loaded = wire::Answer {
            serial: 0,
            handled: false,
            commands,
            motion: None,
            decision: None,
        };
        let loaded_stores = stores.take_read();
        let mut scripts = Self {
            link,
            told: crate::wm::diff::Told::new(),
            serial: 0,
            pending: VecDeque::new(),
            late: VecDeque::new(),
            settings: Settings::default(),
            #[cfg(test)]
            patient: true,
            down: None,
            config: config.to_owned(),
            shipped_only,
            chosen: Chosen::default(),
            chosen_said: Vec::new(),
            took: Took::default(),
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
            keeps: std::collections::BTreeMap::new(),
            listens: std::collections::BTreeSet::new(),
            spellings: std::collections::HashMap::new(),
            timing: crate::wm::timing::Timing::default(),
        };
        scripts.declarations_in(&loaded);
        scripts.listens_in(&loaded);
        scripts.settings_in(&loaded);
        scripts.loaded = loaded;
        scripts
    }

    /// Who answers the window manager's questions: the compositor, over
    /// `snapshot` when a dispatch has one.
    pub(crate) fn answers(&self, snapshot: Option<Snapshot>) -> Answers {
        Answers {
            snapshot,
            stores: Rc::clone(&self.stores),
        }
    }

    /// Send `batch`, and wait for its answer as `wait` says. `None` when it
    /// did not come in that time: it is applied when it does, out of
    /// [`Self::late`].
    pub(crate) fn send_batch(
        &mut self,
        batch: wire::Batch,
        answers: Answers,
        at_the_lock: bool,
        change: bool,
        wait: Wait,
    ) -> Option<(Option<String>, Outcome)> {
        if self.down() {
            return None;
        }
        let serial = batch.serial;
        let event = batch.events.first().map_or("nothing", wire::Event::case);
        if let Err(err) = self.link.send(&Message::Batch(batch)) {
            self.failed(&err);
            return None;
        }
        let behind = !self.pending.is_empty();
        self.pending.push_back(Pending {
            serial,
            event,
            sent: Instant::now(),
            answers,
            at_the_lock,
            change,
        });
        let within = self.within(wait, behind);
        if within.is_zero() {
            return None;
        }
        match self.hear(Awaited::Answer(serial), Some(Instant::now() + within)) {
            Some(Heard::Answer(decision, outcome)) => Some((decision, outcome)),
            _ => None,
        }
    }

    /// How long an event sent with `wait` is waited for, the window manager
    /// being `behind` when it had not answered the one before.
    fn within(&self, wait: Wait, behind: bool) -> std::time::Duration {
        #[cfg(test)]
        if self.patient {
            return PATIENCE;
        }
        match wait {
            Wait::Frame if behind => std::time::Duration::ZERO,
            Wait::Frame => self.settings.ask,
            Wait::Last => self.settings.hard,
        }
    }

    /// Read what the window manager has sent, answering its questions,
    /// until `awaited` comes or `until` passes; with no `until`, only what
    /// has already come. Every other answer on the way is kept in
    /// [`Self::late`], in the order it came.
    fn hear(&mut self, awaited: Awaited, until: Option<Instant>) -> Option<Heard> {
        loop {
            let read = {
                let nobody = self.answers(None);
                // A question is about the batch being handled, which is the
                // oldest one not answered yet.
                let answers: &dyn Ask = self
                    .pending
                    .front()
                    .map_or(&nobody, |pending| &pending.answers);
                self.link.next(until, answers)
            };
            match read {
                Ok(Some(Message::Answer(answer))) => {
                    let serial = answer.serial;
                    let Some(pending) = self.answered(serial) else {
                        tracing::warn!(
                            serial,
                            "the window manager answered a batch it was not waiting on; left out"
                        );
                        continue;
                    };
                    let (decision, outcome) = self.arrived(&pending, answer);
                    let in_time = awaited == Awaited::Answer(serial);
                    self.timing
                        .answered(pending.event, pending.sent.elapsed(), !in_time);
                    if in_time {
                        return Some(Heard::Answer(decision, outcome));
                    }
                    tracing::debug!(
                        serial,
                        after_ms = pending.sent.elapsed().as_secs_f64() * 1000.0,
                        "an answer nobody waited for"
                    );
                    self.late.push_back(Late {
                        outcome,
                        change: pending.change,
                    });
                }
                Ok(Some(Message::Reloaded(reloaded))) if awaited == Awaited::Reloaded => {
                    return Some(Heard::Reloaded(reloaded));
                }
                Ok(Some(Message::Submit(commands))) => self.submitted(commands),
                Ok(Some(Message::Goodbye(why))) => {
                    self.failed(&anyhow::anyhow!("left: {why}"));
                    return None;
                }
                Ok(Some(other)) => tracing::warn!(
                    ?other,
                    "a message the compositor does not take from the window manager; left out"
                ),
                Ok(None) => return None,
                Err(err) => {
                    self.failed(&err);
                    return None;
                }
            }
        }
    }

    /// Read what the window manager has sent since it was last read,
    /// without waiting: its answers are in [`Self::late`] afterwards.
    pub(crate) fn hear_ready(&mut self) {
        if !self.down() {
            self.hear(Awaited::Nothing, None);
        }
    }

    /// Whether the window manager has stopped answering, at `now`: a batch
    /// is waiting, and it has said nothing for `wm.strikes` hard deadlines
    /// running. Counted from the last thing it said, not from when the
    /// batch was sent: one slow answer does not make the batches behind it
    /// late in their turn.
    pub(crate) fn stalled(&mut self, now: Instant) {
        if self.down() {
            return;
        }
        let Some(waiting) = self.pending.front() else {
            return;
        };
        let quiet = now.saturating_duration_since(waiting.sent.max(self.link.heard()));
        let allowed = self.settings.hard.saturating_mul(self.settings.strikes);
        if quiet >= allowed {
            self.failed(&anyhow::anyhow!(
                "answered nothing for {} ms",
                allowed.as_millis()
            ));
        }
    }

    /// The batch `serial` answers, taken off the unanswered. One before it
    /// that was never answered is dropped with it: answers come in order.
    fn answered(&mut self, serial: u64) -> Option<Pending> {
        while let Some(pending) = self.pending.pop_front() {
            if pending.serial == serial {
                return Some(pending);
            }
            if pending.serial > serial {
                self.pending.push_front(pending);
                return None;
            }
            tracing::warn!(
                serial = pending.serial,
                "the window manager never answered a batch"
            );
        }
        None
    }

    /// An answer as the compositor applies it, with what it declares and
    /// sets kept on the way.
    fn arrived(
        &mut self,
        pending: &Pending,
        mut answer: wire::Answer,
    ) -> (Option<String>, Outcome) {
        if pending.at_the_lock {
            crate::wm::events::refuse_past_the_lock(&mut answer);
        }
        self.declarations_in(&answer);
        self.listens_in(&answer);
        self.settings_in(&answer);
        for command in &answer.commands {
            if let wire::Command::Keep((name, doc)) = command {
                self.keeps.insert(name.clone(), doc.clone());
            }
        }
        let decision = answer.decision.take();
        let mut outcome = crate::wm::events::outcome_of(answer);
        outcome.commands.splice(0..0, self.stores.take_read());
        (decision, outcome)
    }

    /// Say in the log how long this window manager's answers took: as a
    /// session ends, and as a window manager is replaced.
    pub(crate) fn say_timing(&self) {
        if let Some(said) = self.timing.said() {
            tracing::info!("window manager: {said}");
        }
    }

    /// The window manager stopped, `err` being how: nothing more is sent,
    /// and what was never answered never will be.
    /// [`crate::state::Solium::restart_wm`] starts another.
    fn failed(&mut self, err: &anyhow::Error) {
        if self.down.is_none() {
            let why = format!("{err:#}");
            tracing::error!("the window manager {why}");
            self.down = Some(why);
            self.link.abandon();
        }
        self.pending.clear();
    }

    /// Whether the window manager has stopped: gone, or not answering.
    pub(crate) fn down(&self) -> bool {
        self.down.is_some()
    }

    /// How it stopped, as what follows "the window manager".
    pub(crate) fn why_down(&self) -> Option<&str> {
        self.down.as_deref()
    }

    /// The configuration it was started on.
    pub(crate) fn config(&self) -> &Path {
        &self.config
    }

    /// Whether it was started to read nothing of the user's own.
    pub(crate) fn shipped_only(&self) -> bool {
        self.shipped_only
    }

    /// What `wm.toml` said when this window manager was started.
    pub(crate) fn chosen(&self) -> &Chosen {
        &self.chosen
    }

    /// What was wrong with `wm.toml` when this window manager was started,
    /// a line each.
    pub(crate) fn chosen_said(&self) -> &[String] {
        &self.chosen_said
    }

    /// Whether `wm.toml` now names another program than the one running,
    /// so that a reload is a new window manager and not this one loading
    /// again.
    pub(crate) fn chosen_another(&self) -> bool {
        !self.shipped_only && Chosen::read().0.command != self.chosen.command
    }

    /// The window manager's process, when it is one.
    pub(crate) fn pid(&self) -> Option<u32> {
        self.link.pid()
    }

    /// The file that says which window manager this is, for a problem
    /// about it to name: `wm.toml` when that named its program, and the
    /// configuration it was started on otherwise.
    pub(crate) fn named_in(&self) -> std::path::PathBuf {
        Self::named_by(&self.chosen, self.shipped_only, &self.config)
    }

    /// [`Self::named_in`], of a window manager started on `config` as
    /// `chosen` says.
    pub(crate) fn named_by(
        chosen: &Chosen,
        shipped_only: bool,
        config: &Path,
    ) -> std::path::PathBuf {
        match (&chosen.command, Chosen::file()) {
            (Some(_), Some(file)) if !shipped_only => file,
            _ => config.to_owned(),
        }
    }

    /// Load the configuration again, the window manager carrying what it
    /// kept; [`Self::startup`] then hands over what the new one asked for.
    /// One that fails to load leaves everything as it was.
    pub(crate) fn reload(&mut self, config: &Path) -> Result<()> {
        if self.down() {
            bail!("the window manager has stopped");
        }
        let path = config.to_string_lossy().into_owned();
        self.link.send(&Message::Reload(path))?;
        let within = {
            #[cfg(test)]
            {
                self.settings.load.max(PATIENCE)
            }
            #[cfg(not(test))]
            {
                self.settings.load
            }
        };
        let loaded = match self.hear(Awaited::Reloaded, Some(Instant::now() + within)) {
            Some(Heard::Reloaded(wire::Reloaded::Loaded(loaded))) => loaded,
            Some(Heard::Reloaded(wire::Reloaded::Failed(why))) => bail!(why),
            _ => bail!("the window manager did not load the configuration again within {within:?}"),
        };
        self.loaded_stores = self.stores.take_read();
        self.declare(wire::Declarations {
            bindings: Vec::new(),
            unbound: Vec::new(),
            unknown: Vec::new(),
            notes: Vec::new(),
        });
        self.declarations_in(&loaded);
        self.listens.clear();
        self.listens_in(&loaded);
        self.settings = Settings::default();
        self.settings_in(&loaded);
        self.loaded = loaded;
        self.started = false;
        // The new configuration says what it keeps when it is next asked:
        // what the old one kept under a name this one never asks for is
        // nobody's.
        self.keeps.clear();
        Ok(())
    }

    /// What the window manager keeps, as it last said, taken from it: for
    /// the one started in its place.
    pub(crate) fn take_keeps(&mut self) -> Keeps {
        std::mem::take(&mut self.keeps).into_iter().collect()
    }

    /// Where the configuration is. See [`crate::wm::paths::config_path`].
    pub(crate) fn config_path() -> std::path::PathBuf {
        crate::wm::paths::config_path(&crate::assets::lua())
    }

    /// Where a user's own scripts live, whether or not they have any.
    pub(crate) fn user_config_dir() -> Option<std::path::PathBuf> {
        crate::wm::paths::user_config_dir()
    }

    /// The window manager's stores, as it would save them, each as its
    /// JSON text.
    #[cfg(test)]
    pub(crate) fn stores(&self) -> Vec<(String, String)> {
        self.link
            .run(
                Box::new(|wm| {
                    wm.host()
                        .stores()
                        .into_iter()
                        .map(|(name, json)| format!("{name}\t{}", json.render()))
                        .collect::<Vec<_>>()
                        .join("\n")
                }),
                &self.answers(None),
            )
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .map(|(name, json)| (name.to_owned(), json.to_owned()))
            .collect()
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
            self.declare(declared.clone());
        }
    }

    /// Keep `declared` as the key filter searches it: each combination in
    /// its one spelling, sorted. A window manager writes a combination as
    /// it likes, `super+shift+e` or `Shift+Super+E`, in any order, and is
    /// told of the key in the spelling it used ([`Self::spelled`]).
    fn declare(&mut self, mut declared: wire::Declarations) {
        self.spellings.clear();
        for binding in &mut declared.bindings {
            let canonical = normalise_combo(&binding.combo);
            if canonical != binding.combo {
                let written = std::mem::replace(&mut binding.combo, canonical.clone());
                self.spellings.entry(canonical).or_insert(written);
            }
        }
        for binding in &mut declared.unbound {
            binding.combo = normalise_combo(&binding.combo);
        }
        // Stable, so of one combination declared twice the first is kept.
        declared
            .bindings
            .sort_by(|one, other| one.combo.cmp(&other.combo));
        declared.bindings.dedup_by(|later, first| {
            let twice = later.combo == first.combo;
            if twice {
                tracing::warn!(
                    combo = later.combo,
                    "the window manager bound a key twice; the first is kept"
                );
            }
            twice
        });
        self.declared = declared;
    }

    /// `combo` as the window manager wrote it when it bound it, which is
    /// how it is told the key was pressed.
    pub(crate) fn spelled(&self, combo: &str) -> String {
        let canonical = normalise_combo(combo);
        self.spellings.get(&canonical).cloned().unwrap_or(canonical)
    }

    /// Keep the events an answer says the window manager listens to: the
    /// last `listen` in it, whole. A name that is no event's is said and
    /// left out.
    fn listens_in(&mut self, answer: &wire::Answer) {
        let Some(names) = answer
            .commands
            .iter()
            .rev()
            .find_map(|command| match command {
                wire::Command::Listen(names) => Some(names),
                _ => None,
            })
        else {
            return;
        };
        self.listens.clear();
        for name in names {
            if wire::Event::CASES.contains(&name.as_str()) {
                self.listens.insert(name.clone());
            } else {
                tracing::warn!(
                    name,
                    "the window manager listens to an event there is none of; left out"
                );
            }
        }
    }

    /// Whether the window manager is sent `event`: it listens to it, or it
    /// is one that needs no listening.
    pub(crate) fn hears(&self, event: &wire::Event) -> bool {
        event.always() || self.listens.contains(event.case())
    }

    /// Commands the window manager sent of its own accord, asked for by no
    /// batch: applied as a late answer is, in the order they came among
    /// those.
    fn submitted(&mut self, commands: Vec<wire::Command>) {
        let answer = wire::Answer {
            serial: 0,
            handled: false,
            commands,
            motion: None,
            decision: None,
        };
        self.declarations_in(&answer);
        self.listens_in(&answer);
        self.settings_in(&answer);
        for command in &answer.commands {
            if let wire::Command::Keep((name, doc)) = command {
                self.keeps.insert(name.clone(), doc.clone());
            }
        }
        let outcome = crate::wm::events::outcome_of(answer);
        self.late.push_back(Late {
            outcome,
            change: false,
        });
    }

    /// Keep the waits an answer sets: the last `wm` section in it.
    fn settings_in(&mut self, answer: &wire::Answer) {
        if let Some(waits) = answer
            .commands
            .iter()
            .rev()
            .find_map(|command| match command {
                wire::Command::Configure((section, nodes)) if section == "wm" => {
                    crate::wm::convert::doc(nodes).ok()
                }
                _ => None,
            })
        {
            self.settings = crate::script::settings::wm::parse(&waits);
        }
    }

    /// Every binding the window manager declared, in its canonical
    /// spelling, sorted.
    pub(crate) fn bindings(&self) -> Vec<wire::Binding> {
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
    fn binding(&self, combo: &str) -> Option<&wire::Binding> {
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
    pub(crate) fn unbound(&self) -> Vec<wire::Binding> {
        self.declared.unbound.clone()
    }

    /// Settings the configuration wrote that its defaults do not define,
    /// sorted by key, because the only reader is a person checking a file.
    pub(crate) fn unknown_settings(&self) -> Vec<wire::UnknownSetting> {
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
