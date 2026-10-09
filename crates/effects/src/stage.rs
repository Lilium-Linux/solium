//! The stage model (\[14\], \[16\] §2): an effect's stages, and every effect
//! they `use`, flattened at load into one linear plan, so nothing is decided
//! per frame. Saved names are local to the effect that saved them
//! (`tests::a_saved_name_inside_a_use_does_not_leak`), a cycle of `use`s is
//! refused (`tests::a_use_inside_a_use_is_flattened_and_a_cycle_refused`),
//! and a used effect reads what it was given where it names its first input,
//! as a chain's later link does
//! (`tests::a_used_effect_naming_its_first_input_reads_what_it_was_given`).

use std::collections::BTreeMap;

use crate::glsl::{self, Host, ParamKind, Signature};
use crate::spec::{self, Value};

/// The most passes one plan runs, its states' included. A six-pass blur is
/// twelve; a plan past this is a mistake (a `repeat` over a long list, or
/// effects each using the next many times), refused rather than built.
/// `tests::a_plan_runs_at_most_256_passes`.
const MOST_STEPS: usize = 256;

/// The most textures one pass reads: `sol_tex` on unit 0 and its `uses` on
/// the next, GLES 2's guaranteed minimum of fragment texture units, so a pass
/// runs on every GPU (Ruling 10). `tests::a_pass_reads_at_most_eight_textures`.
pub const MOST_TEXTURES: usize = 8;

/// What a target holds: 8 bits a channel, or half floats (`rgba16f`, which a
/// GPU may lack; Task 9 probes it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Format {
    #[default]
    Rgba8,
    Rgba16f,
}

/// What a `state` is made again on: the part's shape, the bound params, a
/// commit of the part's own surface, or a region's published outline, which
/// waits for P15 and is refused
/// (`tests::a_state_is_a_sub_plan_read_by_its_name`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Depends {
    Shape,
    Params,
    SelfCommit,
    Region,
}

/// One stage, as an `effect.lua` wrote it.
#[derive(Clone, Debug, PartialEq)]
pub enum Stage {
    /// One `.frag` into a target `scale` times the size of what it reads,
    /// reading the last result (or the one `input` names) and the `uses`.
    Pass {
        frag: String,
        scale: f64,
        format: Format,
        uses: Vec<String>,
        input: Option<String>,
        /// `per_part = true`: never memoised, whatever the frag reads
        /// (Ruling F30, `tests::a_pass_marked_per_part_stays_in_the_suffix`).
        per_part: bool,
    },
    /// `body` once for each number in `over`, with `p_<as_name>` set to it
    /// (`tests::repeat_runs_its_stages_n_times_with_p_jump_set`).
    Repeat {
        over: Vec<f64>,
        as_name: String,
        body: Vec<Stage>,
    },
    /// Names the last result (`tests::save_and_get_name_results`).
    Save(String),
    /// Makes a named result the last one.
    Get(String),
    /// Another effect's stages with these params, spliced in at load
    /// (`tests::use_is_flattened_at_load`).
    Use {
        effect: String,
        params: Vec<(String, Value)>,
    },
    /// A texture kept between runs, made by `body` and made again only when
    /// what it `depends` on changes; later stages read it by `name`.
    State {
        name: String,
        format: Format,
        scale: f64,
        depends: Depends,
        body: Vec<Stage>,
    },
}

/// Where a step's texture comes from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Feed {
    /// One of the part's inputs, by name: `self`, `backdrop`, `old`.
    Input(String),
    /// An earlier step's result: one of [`Plan::steps`], or inside a state
    /// one of that state's own [`StatePlan::steps`].
    Step(usize),
    /// A state's texture, by its index in [`Plan::states`].
    State(usize),
}

/// How big a step's target is: its feed's size times `scale` (rounded up,
/// Task 9), or exactly its feed's, which is how an up pass returns to the
/// level it came from (Ruling 11,
/// `tests::an_up_pass_is_sized_like_the_input_of_the_down_pass_it_undoes`).
#[derive(Clone, Debug, PartialEq)]
pub enum Size {
    Scaled { of: Feed, scale: f64 },
    Like(Feed),
}

/// One pass of a flattened plan.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    /// The effect whose folder holds `frag`.
    pub effect: String,
    pub frag: String,
    /// What the pass's program is compiled against.
    pub signature: Signature,
    pub size: Size,
    pub format: Format,
    /// What `sol_tex` reads.
    pub first: Feed,
    /// What each name the pass `uses` reads, in its order: units 1 onwards.
    pub uses: Vec<(String, Feed)>,
    /// Each `p_<name>`'s value: the effect's bound params, and a `repeat`'s.
    pub uniforms: Vec<(String, Value)>,
    /// The program's content key: 0 until the host sets it at bind (Task 9).
    pub key: u64,
    /// Whether the pass is the part's own: its stage says `per_part = true`,
    /// or its `.frag` reads a name only a part has (`glsl::PER_PART`), which
    /// the host finds at bind, beside `key`. What ends a memo's prefix
    /// (`tests::a_step_reading_a_per_part_name_ends_the_prefix`).
    pub per_part: bool,
}

/// A state's own steps. A `Feed::Step` in them is one of these steps, a
/// `Feed::State` an earlier state, and a `Feed::Input` the part's input: a
/// state is kept between runs, so it never reads a step of the plan
/// (`tests::a_state_reads_inputs_and_earlier_states_and_a_chain_renumbers_them`).
#[derive(Clone, Debug, PartialEq)]
pub struct StatePlan {
    pub name: String,
    pub format: Format,
    /// Scales the box its steps start from (Task 9's `Plan::sizes`).
    pub scale: f64,
    pub depends: Depends,
    pub steps: Vec<Step>,
}

/// What a plan reads of the frame, which decides its tier (Ruling 14):
/// the part's own pixels, the backdrop, its shape, the picture from before.
/// `tests::what_a_plan_reads_is_recorded`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reads {
    pub own: bool,
    pub backdrop: bool,
    pub shape: bool,
    pub old: bool,
}

/// An effect flattened, or a chain of them: every pass in the order it runs.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub steps: Vec<Step>,
    /// Made before the steps, in this order, since a step may read one.
    pub states: Vec<StatePlan>,
    pub reads: Reads,
    /// The input the first pass reads, which a rule's `source` rebinds.
    pub first_input: String,
}

impl Plan {
    /// Every step's size in pixels from the padded box, the main steps' and
    /// each state's, states first because a step may read one: a `Scaled`
    /// step is its feed's size times its scale, rounded up; a `Like` step is
    /// its feed's size, so an up pass returns to its level (Ruling 11); a
    /// state's steps start from the box times the state's scale, and a step
    /// reading a state is the size of the state's last step.
    /// `tests::a_three_pass_blur_returns_to_its_odd_size`,
    /// `tests::a_states_steps_start_from_its_scaled_box`.
    #[expect(
        clippy::type_complexity,
        reason = "the main steps' sizes and each state's, written out where they are made"
    )]
    pub fn sizes(&self, padded: (u32, u32)) -> (Vec<(u32, u32)>, Vec<Vec<(u32, u32)>>) {
        fn scaled(size: (u32, u32), scale: f64) -> (u32, u32) {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a texture side, positive and far below u32::MAX"
            )]
            let side = |n: u32| ((f64::from(n) * scale).ceil() as u32).max(1);
            (side(size.0), side(size.1))
        }
        fn walk(steps: &[Step], padded: (u32, u32), states: &[(u32, u32)]) -> Vec<(u32, u32)> {
            let mut sizes: Vec<(u32, u32)> = Vec::with_capacity(steps.len());
            for step in steps {
                let of = |feed: &Feed, sizes: &[(u32, u32)]| match feed {
                    Feed::Input(_) => padded,
                    Feed::Step(k) => sizes.get(*k).copied().unwrap_or(padded),
                    Feed::State(k) => states.get(*k).copied().unwrap_or(padded),
                };
                let size = match &step.size {
                    Size::Scaled { of: feed, scale } => scaled(of(feed, &sizes), *scale),
                    Size::Like(feed) => of(feed, &sizes),
                };
                sizes.push(size);
            }
            sizes
        }
        let mut state_sizes = Vec::with_capacity(self.states.len());
        let mut state_last = Vec::with_capacity(self.states.len());
        for state in &self.states {
            let start = scaled(padded, state.scale);
            let sizes = walk(&state.steps, start, &state_last);
            state_last.push(sizes.last().copied().unwrap_or(start));
            state_sizes.push(sizes);
        }
        (walk(&self.steps, padded, &state_last), state_sizes)
    }

    /// Every format the plan draws into, once each, `rgba8` first: its
    /// steps', its states' steps' and its states' own. What a GPU must
    /// render into for the plan to run (Ruling 11).
    /// `tests::a_plan_lists_every_format_it_draws_into_once`.
    pub fn formats(&self) -> Vec<Format> {
        let mut formats: Vec<Format> = self
            .steps
            .iter()
            .chain(self.states.iter().flat_map(|state| state.steps.iter()))
            .map(|step| step.format)
            .chain(self.states.iter().map(|state| state.format))
            .collect();
        formats.sort_by_key(|format| *format as u8);
        formats.dedup();
        formats
    }

    /// For each step, the index of the last step that reads it, through
    /// `sol_tex` or `uses`, if any does: the plan's steps, or one state's.
    /// `tests::a_saved_result_keeps_its_slot_until_its_last_reader`.
    pub fn last_readers(steps: &[Step]) -> Vec<Option<usize>> {
        let mut last = vec![None; steps.len()];
        for (index, step) in steps.iter().enumerate() {
            for feed in std::iter::once(&step.first).chain(step.uses.iter().map(|(_, feed)| feed)) {
                if let Feed::Step(k) = feed
                    && let Some(reader) = last.get_mut(*k)
                {
                    *reader = Some(index);
                }
            }
        }
        last
    }

    /// A target slot for each step, and how many slots: a step takes the
    /// first slot of its size and format whose step no step from this one
    /// on reads, else a new one, so a step never draws into what it reads
    /// and a jump flood ping-pongs between two. The last step may take a
    /// freed slot too: nothing draws after it in the run, and the next run
    /// draws a new result. `sizes` are [`Plan::sizes`]' for these steps.
    /// `tests::a_repeated_pass_ping_pongs_between_two_targets`,
    /// `tests::a_saved_result_keeps_its_slot_until_its_last_reader`,
    /// `tests::a_slot_is_shared_only_at_one_size_and_format`.
    pub fn slots(steps: &[Step], sizes: &[(u32, u32)]) -> (Vec<usize>, usize) {
        let readers = Self::last_readers(steps);
        let mut slots = Vec::with_capacity(steps.len());
        // Per slot: its size, its format, and the step whose result it holds.
        let mut held: Vec<((u32, u32), Format, usize)> = Vec::new();
        for (index, step) in steps.iter().enumerate() {
            let size = sizes.get(index).copied().unwrap_or((1, 1));
            let free = held.iter().position(|&(each, format, holder)| {
                each == size
                    && format == step.format
                    && readers
                        .get(holder)
                        .copied()
                        .flatten()
                        .is_none_or(|reader| reader < index)
            });
            let slot = match free {
                Some(slot) => {
                    if let Some(entry) = held.get_mut(slot) {
                        entry.2 = index;
                    }
                    slot
                }
                None => {
                    held.push((size, step.format, index));
                    held.len() - 1
                }
            };
            slots.push(slot);
        }
        (slots, held.len())
    }

    /// Chained content keys: `keys()[k]` covers the first input and steps
    /// `0..=k` (each one's program, uniforms, size, format and feeds), so two
    /// plans that begin alike share their keys as far as they agree, and two
    /// steps with one key draw the same pixels from the same inputs.
    /// `tests::the_keys_are_chained_so_a_shared_head_has_one_key`.
    pub fn keys(&self) -> Vec<u64> {
        let mut last = glsl::content_hash(&[self.first_input.as_bytes()]);
        self.steps
            .iter()
            .map(|step| {
                let text = format!(
                    "{last}{:?}",
                    (
                        step.key,
                        &step.uniforms,
                        &step.size,
                        step.format,
                        &step.first,
                        &step.uses
                    )
                );
                last = glsl::content_hash(&[text.as_bytes()]);
                last
            })
            .collect()
    }

    /// The whole plan's key: its last chained key, or its first input's
    /// when it has no step.
    /// `tests::the_prefix_key_is_its_steps_and_not_the_suffixs`.
    pub fn key(&self) -> u64 {
        self.keys()
            .last()
            .copied()
            .unwrap_or_else(|| glsl::content_hash(&[self.first_input.as_bytes()]))
    }

    /// Steps `0..=last` alone: what a memo of step `last`'s result runs.
    /// `tests::the_keys_are_chained_so_a_shared_head_has_one_key`.
    pub fn upto(&self, last: usize) -> Plan {
        Plan {
            steps: self
                .steps
                .iter()
                .take(last.saturating_add(1))
                .cloned()
                .collect(),
            states: self.states.clone(),
            reads: self.reads,
            first_input: self.first_input.clone(),
        }
    }
}

/// Whether a step writes a target the size of its first input, as a tint
/// or a colour matrix does.
/// `tests::a_trailing_same_size_step_stays_per_part`.
fn same_size(step: &Step) -> bool {
    match &step.size {
        Size::Scaled { of, scale } => *of == step.first && (*scale - 1.0).abs() < f64::EPSILON,
        Size::Like(of) => *of == step.first,
    }
}

/// One prefix result the suffix reads (or, with an empty suffix, the
/// memo's result): its step, its input name there, and its size as a share
/// of the padded box.
/// `tests::a_suffix_step_sized_from_the_prefix_is_sized_from_the_padded_box_at_its_scale`.
#[derive(Clone, Debug, PartialEq)]
pub struct Export {
    pub step: usize,
    pub name: String,
    pub scale: f64,
}

/// A plan split for the memo (Ruling F30).
/// `tests::glass_rect_splits_after_its_blur`.
#[derive(Clone, Debug, PartialEq)]
pub struct Memo {
    /// The steps that read only the backdrop, run once per monitor and cut.
    pub prefix: Option<Plan>,
    /// The rest, run per part, reading the prefix's results as `memo:<k>`.
    pub suffix: Plan,
    pub exports: Vec<Export>,
}

/// How far a memo's prefix runs (`effects.memo.split`, Ruling F30): to its
/// last step that changes size, as far as it reads only the backdrop, or
/// not at all.
/// `tests::the_split_policy_chooses_how_far_the_prefix_runs`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Split {
    #[default]
    Resize,
    Longest,
    Off,
}

/// A prefix result's input name in the suffix.
/// `tests::glass_rect_splits_after_its_blur`.
pub fn memo_name(step: usize) -> String {
    format!("memo:{step}")
}

/// Prefix step `k`'s result as the suffix reads it: the input `memo:<k>`,
/// exported once, at its share of the box.
/// `tests::glass_rect_splits_after_its_blur`.
fn export(k: usize, scales: &[f64], exports: &mut Vec<Export>) -> Feed {
    if !exports.iter().any(|each| each.step == k) {
        exports.push(Export {
            step: k,
            name: memo_name(k),
            scale: scales.get(k).copied().unwrap_or(1.0),
        });
    }
    Feed::Input(memo_name(k))
}

/// A suffix step's feed: a prefix step's result is its export, a later
/// step's moves down by the `len` steps of the prefix, and an input or a
/// state is itself.
/// `tests::glass_rect_splits_after_its_blur`.
fn renumber(feed: &Feed, len: usize, scales: &[f64], exports: &mut Vec<Export>) -> Feed {
    match feed {
        Feed::Step(k) if *k < len => export(*k, scales, exports),
        Feed::Step(k) => Feed::Step(k - len),
        other => other.clone(),
    }
}

/// Split `plan` into the longest run of steps from its start that read only
/// the backdrop or earlier such steps, read no state, and are no part's own
/// (`Step::per_part`), cut back to its last step that changes size under
/// `Split::Resize`, and the rest; under `Split::Off` the whole plan is the
/// suffix. Pure: it knows no effect.
/// `tests::the_split_policy_chooses_how_far_the_prefix_runs`,
/// `tests::a_blur_over_the_backdrop_is_all_prefix_and_its_suffix_is_empty`,
/// `tests::glass_rect_splits_after_its_blur`,
/// `tests::a_step_reading_a_per_part_name_ends_the_prefix`,
/// `tests::a_step_reading_self_or_a_state_ends_the_prefix`,
/// `tests::a_plan_rebound_to_self_is_never_split`,
/// `tests::a_trailing_same_size_step_stays_per_part`,
/// `tests::a_plan_with_no_resizing_step_has_no_prefix`.
pub fn split_memo(plan: &Plan, split: Split) -> Memo {
    let whole = || Memo {
        prefix: None,
        suffix: plan.clone(),
        exports: Vec::new(),
    };
    if split == Split::Off || plan.first_input != "backdrop" {
        return whole();
    }
    let mut len = 0;
    for (index, step) in plan.steps.iter().enumerate() {
        let shared = std::iter::once(&step.first)
            .chain(step.uses.iter().map(|(_, feed)| feed))
            .all(|feed| match feed {
                Feed::Input(name) => name == "backdrop",
                Feed::Step(k) => *k < index,
                Feed::State(_) => false,
            });
        if !shared || step.per_part {
            break;
        }
        len = index + 1;
    }
    // A trailing step the size of what it reads (a tint, a colour matrix)
    // costs per pixel it writes: per part it covers only the parts, holds no
    // monitor-sized result per distinct value, and re-runs alone when only
    // its uniforms move. What is worth sharing ends at the last resize,
    // unless the configuration asks for the longest prefix
    // (`tests::a_trailing_same_size_step_stays_per_part`).
    while split == Split::Resize && len > 0 && plan.steps.get(len - 1).is_some_and(same_size) {
        len -= 1;
    }
    if len == 0 {
        return whole();
    }
    let (shared, rest) = plan.steps.split_at(len);
    // Each prefix step's size as a share of the box: the product of the
    // scales from the backdrop to it; a `Like` step is its feed's
    // (`tests::a_suffix_step_sized_from_the_prefix_is_sized_from_the_padded_box_at_its_scale`).
    let mut scales: Vec<f64> = Vec::with_capacity(len);
    for step in shared {
        let of = |feed: &Feed| match feed {
            Feed::Step(k) => scales.get(*k).copied().unwrap_or(1.0),
            Feed::Input(_) | Feed::State(_) => 1.0,
        };
        let scale = match &step.size {
            Size::Scaled { of: feed, scale } => of(feed) * scale,
            Size::Like(feed) => of(feed),
        };
        scales.push(scale);
    }
    let share = |k: usize| scales.get(k).copied().unwrap_or(1.0);
    let backdrop = || Feed::Input("backdrop".to_owned());
    let mut exports: Vec<Export> = Vec::new();
    let mut steps = Vec::with_capacity(rest.len());
    for step in rest {
        let mut step = step.clone();
        step.size = match &step.size {
            Size::Scaled {
                of: Feed::Step(k),
                scale,
            } if *k < len => Size::Scaled {
                of: backdrop(),
                scale: share(*k) * scale,
            },
            Size::Like(Feed::Step(k)) if *k < len => Size::Scaled {
                of: backdrop(),
                scale: share(*k),
            },
            Size::Scaled { of, scale } => Size::Scaled {
                of: renumber(of, len, &scales, &mut exports),
                scale: *scale,
            },
            Size::Like(of) => Size::Like(renumber(of, len, &scales, &mut exports)),
        };
        step.first = renumber(&step.first, len, &scales, &mut exports);
        step.uses = step
            .uses
            .iter()
            .map(|(name, feed)| (name.clone(), renumber(feed, len, &scales, &mut exports)))
            .collect();
        steps.push(step);
    }
    if steps.is_empty() {
        export(len - 1, &scales, &mut exports);
    }
    let first_input = match steps.first().map(|step| &step.first) {
        Some(Feed::Input(name)) => name.clone(),
        _ => plan.first_input.clone(),
    };
    Memo {
        prefix: Some(Plan {
            steps: shared.to_vec(),
            states: Vec::new(),
            reads: Reads {
                backdrop: true,
                ..Reads::default()
            },
            first_input: "backdrop".to_owned(),
        }),
        suffix: Plan {
            steps,
            states: plan.states.clone(),
            reads: plan.reads,
            first_input,
        },
        exports,
    }
}

/// One effect bound by name: its stages for the bound params, the inputs it
/// declares and the params as bound.
#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    pub stages: Vec<Stage>,
    pub inputs: Vec<String>,
    pub params: Vec<(String, Value)>,
}

/// One effect's scope while it is flattened: its saved names and states, and
/// the stack of sizes its down passes left (Ruling 11).
#[derive(Debug, Default)]
struct Scope {
    saved: BTreeMap<String, Feed>,
    levels: Vec<Feed>,
}

impl Scope {
    /// A state's scope: the inputs saved under a name and the states made
    /// before it, never a step's result.
    /// `tests::a_state_reads_inputs_and_earlier_states_and_a_chain_renumbers_them`.
    fn for_state(&self) -> Self {
        Self {
            saved: self
                .saved
                .iter()
                .filter(|(_, feed)| !matches!(feed, Feed::Step(_)))
                .map(|(name, feed)| (name.clone(), feed.clone()))
                .collect(),
            levels: Vec::new(),
        }
    }
}

/// What a walk of one effect's stages reads from that effect.
#[derive(Clone, Copy, Debug)]
struct Context<'a> {
    effect: &'a str,
    params: &'a [(String, ParamKind)],
    uniforms: &'a [(String, Value)],
    known: &'a [String],
    inputs: &'a [String],
    /// The effect's first input's name, `self` if it declares none.
    first: &'a str,
    /// What that name reads: the result a `use` was given, which at the root
    /// is the input itself; `None` inside a state, which reads the part's.
    given: Option<&'a Feed>,
}

impl Context<'_> {
    /// A name a stage reads: a saved result or state of this effect, or one
    /// of its inputs. `tests::save_and_get_name_results`,
    /// `tests::a_used_effect_naming_its_first_input_reads_what_it_was_given`.
    fn feed(&self, name: &str, scope: &Scope) -> Result<Feed, String> {
        if let Some(feed) = scope.saved.get(name) {
            return Ok(feed.clone());
        }
        if self
            .inputs
            .iter()
            .any(|input| input.strip_prefix("state:") == Some(name))
        {
            return Err(format!(
                "effect `{}` reads state `{name}` before a `state` stage makes it",
                self.effect
            ));
        }
        if name == self.first
            && let Some(given) = self.given
        {
            return Ok(given.clone());
        }
        if self.inputs.iter().any(|input| input == name) {
            return Ok(Feed::Input(name.to_owned()));
        }
        Err(format!(
            "effect `{}` reads `{name}`, which it neither saved nor declared as an input",
            self.effect
        ))
    }
}

/// Mark what reading `feed` reads of the frame.
/// `tests::what_a_plan_reads_is_recorded`.
fn mark(reads: &mut Reads, feed: &Feed) {
    if let Feed::Input(name) = feed {
        match name.as_str() {
            "self" => reads.own = true,
            "backdrop" => reads.backdrop = true,
            "shape" => reads.shape = true,
            "old" => reads.old = true,
            _ => {}
        }
    }
}

/// Names saved or made a state anywhere in `stages`, a state's own stages
/// included: what a pass may read, and so what its prelude declares
/// (`tests::a_state_reads_inputs_and_earlier_states_and_a_chain_renumbers_them`).
fn names_in(stages: &[Stage], into: &mut Vec<String>) {
    for stage in stages {
        match stage {
            Stage::Save(name) => {
                if !into.contains(name) {
                    into.push(name.clone());
                }
            }
            Stage::State { name, body, .. } => {
                if !into.contains(name) {
                    into.push(name.clone());
                }
                names_in(body, into);
            }
            Stage::Repeat { body, .. } => names_in(body, into),
            Stage::Pass { .. } | Stage::Get(_) | Stage::Use { .. } => {}
        }
    }
}

/// Refuse a name in `known` that the prelude cannot declare as
/// `sol_<name>`: no GLSL name, one of the prelude's own, or another known
/// name's `_box` or `_sampler`. Checked once for every name an effect saves
/// or makes a state anywhere, so one no pass reads is caught too, at load
/// rather than as a compile error in Solium's own source string
/// (`tests::a_saved_name_is_a_new_glsl_name`).
fn check_known(effect: &str, known: &[String]) -> Result<(), String> {
    for name in known {
        if !spec::is_identifier(name) {
            return Err(format!(
                "effect `{effect}` names a result or state \"{name}\": a name is lower-case letters, digits and `_`"
            ));
        }
        if glsl::VOCABULARY.contains(&name.as_str()) {
            return Err(format!(
                "effect `{effect}` names a result or state \"{name}\", which is the engine's own `sol_{name}`"
            ));
        }
        if let Some(other) = known.iter().find(|other| {
            [glsl::sampler(other), glsl::box_of(other)]
                .iter()
                .any(|declared| declared.strip_prefix("sol_") == Some(name.as_str()))
        }) {
            return Err(format!(
                "effect `{effect}` names a result or state \"{name}\", which is `{other}`'s own `sol_{name}`"
            ));
        }
    }
    Ok(())
}

/// One flatten: the resolver, and what the walk has made so far.
struct Flattener<'r> {
    #[expect(
        clippy::type_complexity,
        reason = "flatten's resolver, written out as flatten takes it"
    )]
    resolve: &'r mut dyn FnMut(&str, &[(String, Value)]) -> Result<Binding, String>,
    /// The effects being flattened, outermost first: a cycle's witness
    /// (`tests::a_use_inside_a_use_is_flattened_and_a_cycle_refused`).
    stack: Vec<String>,
    states: Vec<StatePlan>,
    reads: Reads,
    /// Passes made so far, the states' included
    /// (`tests::a_plan_runs_at_most_256_passes`).
    count: usize,
}

impl std::fmt::Debug for Flattener<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Flattener")
            .field("stack", &self.stack)
            .field("count", &self.count)
            .finish_non_exhaustive()
    }
}

impl Flattener<'_> {
    /// Flatten effect `name` bound with `overrides`, given `first`, into
    /// `out`; the feed of its last result.
    /// `tests::a_use_inside_a_use_is_flattened_and_a_cycle_refused`.
    fn effect(
        &mut self,
        name: &str,
        overrides: &[(String, Value)],
        first: Feed,
        out: &mut Vec<Step>,
    ) -> Result<Feed, String> {
        if let Some(at) = self.stack.iter().position(|each| each == name) {
            let mut cycle = self.stack[at..].join(" uses ");
            cycle.push_str(&format!(" uses {name}"));
            return Err(format!("a cycle of effects: {cycle}"));
        }
        let binding = (self.resolve)(name, overrides).map_err(|err| match self.stack.last() {
            Some(user) => format!("`{user}` uses `{name}`: {err}"),
            None => err,
        })?;
        self.bound(name, &binding, first, out)
    }

    /// Flatten an effect already bound: its own scope, its params and the
    /// names its passes may read. `tests::use_is_flattened_at_load`.
    fn bound(
        &mut self,
        name: &str,
        binding: &Binding,
        first: Feed,
        out: &mut Vec<Step>,
    ) -> Result<Feed, String> {
        self.stack.push(name.to_owned());
        let params: Vec<(String, ParamKind)> = binding
            .params
            .iter()
            .filter_map(|(each, value)| glsl::kind_of(value).map(|kind| (each.clone(), kind)))
            .collect();
        let uniforms: Vec<(String, Value)> = binding
            .params
            .iter()
            .filter(|(_, value)| glsl::kind_of(value).is_some())
            .cloned()
            .collect();
        let mut known: Vec<String> = Vec::new();
        for input in binding.inputs.iter().filter(|input| *input != "shape") {
            let input = input.trim_start_matches("state:").to_owned();
            if !known.contains(&input) {
                known.push(input);
            }
        }
        names_in(&binding.stages, &mut known);
        check_known(name, &known)?;
        let cx = Context {
            effect: name,
            params: &params,
            uniforms: &uniforms,
            known: &known,
            inputs: &binding.inputs,
            first: binding.inputs.first().map_or("self", String::as_str),
            given: Some(&first),
        };
        let mut scope = Scope::default();
        let last = self.walk(&cx, &binding.stages, first.clone(), &mut scope, &[], out)?;
        self.stack.pop();
        Ok(last)
    }

    fn walk(
        &mut self,
        cx: &Context<'_>,
        stages: &[Stage],
        mut last: Feed,
        scope: &mut Scope,
        extra: &[(String, f64)],
        out: &mut Vec<Step>,
    ) -> Result<Feed, String> {
        for stage in stages {
            match stage {
                Stage::Pass {
                    frag,
                    scale,
                    format,
                    uses,
                    input,
                    per_part,
                } => {
                    if self.count >= MOST_STEPS {
                        return Err(format!(
                            "the stages run more than {MOST_STEPS} passes, the most one effect may run"
                        ));
                    }
                    self.count += 1;
                    let first = match input {
                        Some(name) => cx.feed(name, scope)?,
                        None => last.clone(),
                    };
                    let size = if *scale > 1.0 {
                        scope.levels.pop().map_or_else(
                            || Size::Scaled {
                                of: first.clone(),
                                scale: *scale,
                            },
                            Size::Like,
                        )
                    } else {
                        if *scale < 1.0 {
                            scope.levels.push(first.clone());
                        }
                        Size::Scaled {
                            of: first.clone(),
                            scale: *scale,
                        }
                    };
                    let mut bound = Vec::with_capacity(uses.len());
                    for name in uses {
                        // The analytic shape is no texture: `sol_shape` is
                        // always declared (Ruling 6,
                        // `tests::what_a_plan_reads_is_recorded`).
                        if name == "shape" {
                            self.reads.shape = true;
                            continue;
                        }
                        let feed = cx.feed(name, scope)?;
                        mark(&mut self.reads, &feed);
                        bound.push((name.clone(), feed));
                    }
                    // `tests::a_pass_reads_at_most_eight_textures`.
                    if bound.len() + 1 > MOST_TEXTURES {
                        return Err(format!(
                            "`{frag}` reads {} textures, `sol_tex` and {} in `uses`: a pass reads at most {MOST_TEXTURES}",
                            bound.len() + 1,
                            bound.len()
                        ));
                    }
                    mark(&mut self.reads, &first);
                    let mut params = cx.params.to_vec();
                    let mut uniforms = cx.uniforms.to_vec();
                    for (name, value) in extra {
                        params.push((name.clone(), ParamKind::Float));
                        uniforms.push((name.clone(), Value::Number(*value)));
                    }
                    let signature = Signature {
                        host: Host::Pass,
                        params,
                        uses: bound.iter().map(|(name, _)| name.clone()).collect(),
                        known: cx.known.to_vec(),
                    };
                    out.push(Step {
                        effect: cx.effect.to_owned(),
                        frag: frag.clone(),
                        signature,
                        size,
                        format: *format,
                        first,
                        uses: bound,
                        uniforms,
                        key: 0,
                        per_part: *per_part,
                    });
                    last = Feed::Step(out.len() - 1);
                }
                Stage::Repeat {
                    over,
                    as_name,
                    body,
                } => {
                    // `p_<as>` is declared beside the params, so a name that
                    // is one already, or that GLSL cannot carry, would fail
                    // in the prelude (`tests::a_repeats_name_is_a_new_glsl_name`).
                    if !spec::is_identifier(as_name) {
                        return Err(format!(
                            "`repeat`'s `as = \"{as_name}\"`: a name is lower-case letters, digits and `_`"
                        ));
                    }
                    if cx
                        .params
                        .iter()
                        .map(|(name, _)| name)
                        .chain(extra.iter().map(|(name, _)| name))
                        .any(|name| name == as_name)
                    {
                        return Err(format!(
                            "`repeat`'s `as = \"{as_name}\"` is already a param of `{}`",
                            cx.effect
                        ));
                    }
                    for value in over {
                        let mut more = extra.to_vec();
                        more.push((as_name.clone(), *value));
                        last = self.walk(cx, body, last, scope, &more, out)?;
                    }
                }
                Stage::Save(name) => {
                    scope.saved.insert(name.clone(), last.clone());
                }
                Stage::Get(name) => last = cx.feed(name, scope)?,
                Stage::Use { effect, params } => last = self.effect(effect, params, last, out)?,
                Stage::State {
                    name,
                    format,
                    scale,
                    depends,
                    body,
                } => {
                    if *depends == Depends::Region {
                        return Err(format!(
                            "state `{name}`: depends = \"region\" needs P15's Solium.region, which this Solium does not have yet"
                        ));
                    }
                    let inside = Context { given: None, ..*cx };
                    let mut inner = scope.for_state();
                    let mut steps = Vec::new();
                    let first = Feed::Input(cx.first.to_owned());
                    self.walk(&inside, body, first, &mut inner, extra, &mut steps)?;
                    self.states.push(StatePlan {
                        name: name.clone(),
                        format: *format,
                        scale: *scale,
                        depends: *depends,
                        steps,
                    });
                    scope
                        .saved
                        .insert(name.clone(), Feed::State(self.states.len() - 1));
                }
            }
        }
        Ok(last)
    }
}

/// Flatten `root` bound with `overrides`. `resolve` binds an effect by name
/// (the host's sandbox calls `stages(p)` there, at load).
#[expect(
    clippy::type_complexity,
    reason = "the resolver's signature, written out where it is taken"
)]
pub fn flatten(
    root: &str,
    overrides: &[(String, Value)],
    resolve: &mut dyn FnMut(&str, &[(String, Value)]) -> Result<Binding, String>,
) -> Result<Plan, String> {
    let binding = resolve(root, overrides)?;
    let first_input = binding
        .inputs
        .first()
        .cloned()
        .unwrap_or_else(|| "self".to_owned());
    let mut flattener = Flattener {
        resolve,
        stack: Vec::new(),
        states: Vec::new(),
        reads: Reads::default(),
        count: 0,
    };
    let mut steps = Vec::new();
    flattener.bound(root, &binding, Feed::Input(first_input.clone()), &mut steps)?;
    Ok(Plan {
        steps,
        states: flattener.states,
        reads: flattener.reads,
        first_input,
    })
}

/// Move a step's feeds by `by`.
fn shift(step: &mut Step, by: &dyn Fn(&Feed) -> Feed) {
    step.first = by(&step.first);
    for (_, feed) in &mut step.uses {
        *feed = by(feed);
    }
    step.size = match &step.size {
        Size::Scaled { of, scale } => Size::Scaled {
            of: by(of),
            scale: *scale,
        },
        Size::Like(of) => Size::Like(by(of)),
    };
}

/// Mark what a step's textures read of the frame.
fn mark_step(reads: &mut Reads, step: &Step) {
    mark(reads, &step.first);
    for (_, feed) in &step.uses {
        mark(reads, feed);
    }
}

/// A chain: each link's steps after the last, its first input rebound to the
/// previous link's last step, and its states after the last's. What a later
/// link reads is read again once rebound, so the first input it was given
/// reads nothing of the frame and any other it names still does
/// (`tests::a_chain_feeds_each_links_result_to_the_next_ones_first_input`,
/// `tests::a_chain_reads_what_its_links_read_once_rebound`).
pub fn chain(links: Vec<Plan>) -> Plan {
    let mut links = links.into_iter();
    let Some(mut chained) = links.next() else {
        return Plan {
            steps: Vec::new(),
            states: Vec::new(),
            reads: Reads::default(),
            first_input: "self".to_owned(),
        };
    };
    for link in links {
        let (step_base, state_base) = (chained.steps.len(), chained.states.len());
        let previous = step_base
            .checked_sub(1)
            .map_or_else(|| Feed::Input(chained.first_input.clone()), Feed::Step);
        // A state's steps keep their own numbers and read the part's inputs,
        // so only the states it reads move
        // (`tests::a_state_reads_inputs_and_earlier_states_and_a_chain_renumbers_them`).
        let in_state = |feed: &Feed| match feed {
            Feed::State(k) => Feed::State(k + state_base),
            other => other.clone(),
        };
        let in_main = |feed: &Feed| match feed {
            Feed::Step(k) => Feed::Step(k + step_base),
            Feed::Input(name) if *name == link.first_input => previous.clone(),
            other => in_state(other),
        };
        for mut state in link.states {
            for step in &mut state.steps {
                shift(step, &in_state);
                mark_step(&mut chained.reads, step);
            }
            chained.states.push(state);
        }
        for mut step in link.steps {
            shift(&mut step, &in_main);
            mark_step(&mut chained.reads, &step);
            chained.steps.push(step);
        }
        chained.reads.shape |= link.reads.shape;
    }
    chained
}

#[cfg(test)]
mod tests {
    use super::{
        Binding, Depends, Export, Feed, Format, Plan, Reads, Size, Split, Stage, Step, chain,
        flatten, memo_name, split_memo,
    };
    use crate::spec::Value;

    fn pass(frag: &str, scale: f64) -> Stage {
        Stage::Pass {
            frag: frag.to_owned(),
            scale,
            format: Format::Rgba8,
            uses: Vec::new(),
            input: None,
            per_part: false,
        }
    }

    fn pass_using(frag: &str, uses: &[&str]) -> Stage {
        Stage::Pass {
            frag: frag.to_owned(),
            scale: 1.0,
            format: Format::Rgba8,
            uses: uses.iter().map(|name| (*name).to_owned()).collect(),
            input: None,
            per_part: false,
        }
    }

    fn binding(inputs: &[&str], stages: Vec<Stage>) -> Binding {
        Binding {
            stages,
            inputs: inputs.iter().map(|name| (*name).to_owned()).collect(),
            params: Vec::new(),
        }
    }

    /// The fixtures as flatten sees them: `blur` (three down, three up),
    /// `tint`, `frost` (`use`s both), `a`/`b` for cycles.
    fn library(name: &str, params: &[(String, Value)]) -> Result<Binding, String> {
        let passes = params
            .iter()
            .find(|(each, _)| each == "passes")
            .and_then(|(_, value)| {
                if let Value::Int(n) = value {
                    Some(*n)
                } else {
                    None
                }
            })
            .unwrap_or(3);
        let stages = match name {
            "blur" => {
                let mut stages: Vec<Stage> = (0..passes).map(|_| pass("down.frag", 0.5)).collect();
                stages.extend((0..passes).map(|_| pass("up.frag", 2.0)));
                stages.push(Stage::Save("x".to_owned()));
                stages
            }
            "tint" => vec![pass("tint.frag", 1.0)],
            "frost" => vec![
                Stage::Use {
                    effect: "blur".to_owned(),
                    params: vec![("passes".to_owned(), Value::Int(2))],
                },
                Stage::Save("x".to_owned()),
                Stage::Use {
                    effect: "tint".to_owned(),
                    params: Vec::new(),
                },
                Stage::Get("x".to_owned()),
                pass("final.frag", 1.0),
            ],
            "a" => vec![Stage::Use {
                effect: "b".to_owned(),
                params: Vec::new(),
            }],
            "b" => vec![Stage::Use {
                effect: "a".to_owned(),
                params: Vec::new(),
            }],
            "keeps" => vec![
                Stage::Save("x".to_owned()),
                Stage::Use {
                    effect: "blur".to_owned(),
                    params: Vec::new(),
                },
                Stage::Get("x".to_owned()),
                pass("final.frag", 1.0),
            ],
            "peeks" => vec![
                Stage::Save("mine".to_owned()),
                Stage::Use {
                    effect: "peek".to_owned(),
                    params: Vec::new(),
                },
            ],
            "peek" => vec![Stage::Get("mine".to_owned()), pass("a.frag", 1.0)],
            other => return Err(format!("no effect called {other}")),
        };
        let params = match name {
            "blur" => vec![
                ("passes".to_owned(), Value::Int(passes)),
                ("offset".to_owned(), Value::Number(3.0)),
            ],
            _ => Vec::new(),
        };
        Ok(Binding {
            stages,
            inputs: vec!["backdrop".to_owned()],
            params,
        })
    }

    #[test]
    fn use_is_flattened_at_load() {
        let plan = flatten("frost", &[], &mut library).expect("flattens");
        let frags: Vec<&str> = plan.steps.iter().map(|step| step.frag.as_str()).collect();
        assert_eq!(
            frags,
            [
                "down.frag",
                "down.frag",
                "up.frag",
                "up.frag",
                "tint.frag",
                "final.frag"
            ]
        );
        assert!(
            plan.steps[0]
                .uniforms
                .contains(&("offset".to_owned(), Value::Number(3.0))),
            "blur's own default"
        );
        assert_eq!(plan.steps[0].effect, "blur");
        assert_eq!(plan.steps[0].first, Feed::Input("backdrop".to_owned()));
    }

    #[test]
    fn a_use_inside_a_use_is_flattened_and_a_cycle_refused() {
        let refused = flatten("a", &[], &mut library).expect_err("a cycle");
        assert!(refused.contains('a') && refused.contains('b'), "{refused}");
    }

    /// **A saved name inside a `use` does not leak**, either way: `blur`
    /// saving `x` and `frost` getting its own `x` read different steps, and
    /// a used effect cannot get a name its user saved.
    #[test]
    fn a_saved_name_inside_a_use_does_not_leak() {
        let plan = flatten("frost", &[], &mut library).expect("flattens");
        // frost saved x after blur's last step (3), then tint (4), then got x back.
        assert_eq!(
            plan.steps[5].first,
            Feed::Step(3),
            "frost's x is the step blur ended on, not anything blur named x"
        );
        // keeps saved x before blur ran, and blur's own x is its step 5.
        let plan = flatten("keeps", &[], &mut library).expect("flattens");
        assert_eq!(
            plan.steps[6].first,
            Feed::Input("backdrop".to_owned()),
            "blur's x leaked into keeps"
        );
        let refused = flatten("peeks", &[], &mut library).expect_err("peek gets mine");
        assert!(refused.contains("mine"), "{refused}");
    }

    #[test]
    fn repeat_runs_its_stages_n_times_with_p_jump_set() {
        let mut jump = |name: &str, _: &[(String, Value)]| -> Result<Binding, String> {
            assert_eq!(name, "jump");
            Ok(Binding {
                stages: vec![Stage::Repeat {
                    over: vec![64.0, 32.0, 16.0, 8.0, 4.0, 2.0, 1.0, 1.0],
                    as_name: "jump".to_owned(),
                    body: vec![pass("jump.frag", 1.0)],
                }],
                inputs: vec!["self".to_owned()],
                params: Vec::new(),
            })
        };
        let plan = flatten("jump", &[], &mut jump).expect("flattens");
        let jumps: Vec<Value> = plan
            .steps
            .iter()
            .map(|step| {
                step.uniforms
                    .iter()
                    .find(|(name, _)| name == "jump")
                    .expect("p_jump")
                    .1
                    .clone()
            })
            .collect();
        assert_eq!(jumps.len(), 8);
        assert_eq!(jumps[0], Value::Number(64.0));
        assert_eq!(jumps[7], Value::Number(1.0));
        assert!(
            plan.steps[0]
                .signature
                .params
                .iter()
                .any(|(name, _)| name == "jump")
        );
    }

    #[test]
    fn save_and_get_name_results() {
        let mut lib = |_: &str, _: &[(String, Value)]| -> Result<Binding, String> {
            Ok(Binding {
                stages: vec![
                    Stage::Save("sharp".to_owned()),
                    pass("soft.frag", 0.5),
                    Stage::Get("sharp".to_owned()),
                    Stage::Pass {
                        frag: "both.frag".to_owned(),
                        scale: 1.0,
                        format: Format::Rgba8,
                        uses: vec!["sharp".to_owned()],
                        input: None,
                        per_part: false,
                    },
                ],
                inputs: vec!["backdrop".to_owned()],
                params: Vec::new(),
            })
        };
        let plan = flatten("x", &[], &mut lib).expect("flattens");
        assert_eq!(
            plan.steps[1].first,
            Feed::Input("backdrop".to_owned()),
            "after get, the first input is what was saved"
        );
        assert_eq!(
            plan.steps[1].uses,
            vec![("sharp".to_owned(), Feed::Input("backdrop".to_owned()))]
        );
        assert!(plan.steps[1].signature.uses.contains(&"sharp".to_owned()));
    }

    /// `state` is a sub-plan readable afterwards as its name; `depends =
    /// "region"` is refused, naming P15.
    #[test]
    fn a_state_is_a_sub_plan_read_by_its_name() {
        let mut lib = |_: &str, _: &[(String, Value)]| -> Result<Binding, String> {
            Ok(Binding {
                stages: vec![
                    Stage::State {
                        name: "field".to_owned(),
                        format: Format::Rgba16f,
                        scale: 1.0,
                        depends: Depends::Shape,
                        body: vec![pass("seed.frag", 1.0)],
                    },
                    Stage::Pass {
                        frag: "use.frag".to_owned(),
                        scale: 1.0,
                        format: Format::Rgba8,
                        uses: vec!["field".to_owned()],
                        input: None,
                        per_part: false,
                    },
                ],
                inputs: vec!["self".to_owned()],
                params: Vec::new(),
            })
        };
        let plan = flatten("x", &[], &mut lib).expect("flattens");
        assert_eq!(plan.states.len(), 1);
        assert_eq!(
            plan.steps[0].uses,
            vec![("field".to_owned(), Feed::State(0))]
        );
        let mut region = |_: &str, _: &[(String, Value)]| -> Result<Binding, String> {
            Ok(Binding {
                stages: vec![Stage::State {
                    name: "f".to_owned(),
                    format: Format::Rgba8,
                    scale: 1.0,
                    depends: Depends::Region,
                    body: vec![pass("a.frag", 1.0)],
                }],
                inputs: vec!["self".to_owned()],
                params: Vec::new(),
            })
        };
        assert!(
            flatten("x", &[], &mut region)
                .expect_err("refused")
                .contains("P15")
        );
    }

    /// **An up pass returns to the level it came from** (Ruling 11).
    #[test]
    fn an_up_pass_is_sized_like_the_input_of_the_down_pass_it_undoes() {
        let plan = flatten("blur", &[], &mut library).expect("flattens");
        assert_eq!(plan.steps[3].size, Size::Like(Feed::Step(1)));
        assert_eq!(plan.steps[4].size, Size::Like(Feed::Step(0)));
        assert_eq!(
            plan.steps[5].size,
            Size::Like(Feed::Input("backdrop".to_owned()))
        );
        assert_eq!(
            plan.steps[0].size,
            Size::Scaled {
                of: Feed::Input("backdrop".to_owned()),
                scale: 0.5
            }
        );
    }

    #[test]
    fn a_chain_feeds_each_links_result_to_the_next_ones_first_input() {
        let blur = flatten("blur", &[], &mut library).expect("flattens");
        let tint = flatten("tint", &[], &mut library).expect("flattens");
        let chained = chain(vec![blur, tint]);
        assert_eq!(chained.steps.len(), 7);
        assert_eq!(
            chained.steps[6].first,
            Feed::Step(5),
            "tint reads blur's last step"
        );
        assert_eq!(chained.first_input, "backdrop");
    }

    #[test]
    fn what_a_plan_reads_is_recorded() {
        let plan = flatten("blur", &[], &mut library).expect("flattens");
        assert!(plan.reads.backdrop && !plan.reads.own && !plan.reads.shape);
        // `shape` in `uses` is read, and is no texture: `sol_shape` is
        // always declared (Ruling 6).
        let mut lib = |_: &str, _: &[(String, Value)]| -> Result<Binding, String> {
            Ok(binding(
                &["self"],
                vec![pass_using("ring.frag", &["shape"])],
            ))
        };
        let plan = flatten("ring", &[], &mut lib).expect("flattens");
        assert!(plan.reads.shape && plan.reads.own, "{:?}", plan.reads);
        assert!(plan.steps[0].uses.is_empty() && plan.steps[0].signature.uses.is_empty());
    }

    /// **A used effect naming its first input reads what it was given**, as
    /// a chain's later link does: `inner`, used on `outer`'s first result,
    /// mixing in its `backdrop` mixes in that result, and `outer`, which
    /// reads only `self`, reads no backdrop.
    #[test]
    fn a_used_effect_naming_its_first_input_reads_what_it_was_given() {
        let mut lib = |name: &str, _: &[(String, Value)]| -> Result<Binding, String> {
            match name {
                "outer" => Ok(binding(
                    &["self"],
                    vec![
                        pass("a.frag", 1.0),
                        Stage::Use {
                            effect: "inner".to_owned(),
                            params: Vec::new(),
                        },
                    ],
                )),
                "inner" => Ok(binding(
                    &["backdrop"],
                    vec![pass_using("mix.frag", &["backdrop"])],
                )),
                other => Err(format!("no effect called {other}")),
            }
        };
        let plan = flatten("outer", &[], &mut lib).expect("flattens");
        assert_eq!(
            plan.steps[1].uses,
            vec![("backdrop".to_owned(), Feed::Step(0))]
        );
        assert!(plan.reads.own && !plan.reads.backdrop, "{:?}", plan.reads);
    }

    /// **A chain reads what its links read once each is rebound**: a later
    /// link's first input is the previous link's result, which reads nothing
    /// of the frame, and any other input it names is still read. So a tint
    /// after a generated ring reads no `self` (it needs no capture), and a
    /// mix after it that also names `backdrop` reads the backdrop.
    #[test]
    fn a_chain_reads_what_its_links_read_once_rebound() {
        let mut lib = |name: &str, _: &[(String, Value)]| -> Result<Binding, String> {
            match name {
                "ring" => Ok(binding(&["shape"], vec![pass("ring.frag", 1.0)])),
                "tint" => Ok(binding(&["self"], vec![pass("tint.frag", 1.0)])),
                "mix" => Ok(binding(
                    &["self", "backdrop"],
                    vec![pass_using("mix.frag", &["backdrop"])],
                )),
                other => Err(format!("no effect called {other}")),
            }
        };
        let mut plan = |name: &str| flatten(name, &[], &mut lib).expect("flattens");
        let (ring, tint, mix) = (plan("ring"), plan("tint"), plan("mix"));
        let tinted = chain(vec![ring.clone(), tint]);
        assert!(
            !tinted.reads.own && tinted.reads.shape,
            "{:?}",
            tinted.reads
        );
        let mixed = chain(vec![ring, mix]);
        assert!(
            mixed.reads.backdrop && !mixed.reads.own,
            "{:?}",
            mixed.reads
        );
        assert_eq!(
            mixed.steps[1].uses,
            vec![("backdrop".to_owned(), Feed::Input("backdrop".to_owned()))]
        );
    }

    /// **A state reads the part's inputs and earlier states, never a step's
    /// result**, since it is kept between runs: its first pass reads the
    /// effect's first input even after other passes, a later state reads an
    /// earlier one by name, and a chain renumbers both, inside the states
    /// and out. A state read before its stage makes it is refused.
    #[test]
    fn a_state_reads_inputs_and_earlier_states_and_a_chain_renumbers_them() {
        let mut lib = |name: &str, _: &[(String, Value)]| -> Result<Binding, String> {
            let state = |name: &str, body: Vec<Stage>| Stage::State {
                name: name.to_owned(),
                format: Format::Rgba8,
                scale: 1.0,
                depends: Depends::Params,
                body,
            };
            match name {
                "fields" => Ok(binding(
                    &["self"],
                    vec![
                        pass("first.frag", 1.0),
                        state(
                            "a",
                            vec![
                                pass("a.frag", 1.0),
                                Stage::Save("inner".to_owned()),
                                pass_using("a2.frag", &["inner"]),
                            ],
                        ),
                        state("b", vec![pass_using("b.frag", &["a"])]),
                        pass_using("c.frag", &["b"]),
                    ],
                )),
                "early" => Ok(binding(
                    &["self", "state:a"],
                    vec![
                        pass_using("x.frag", &["a"]),
                        state("a", vec![pass("a.frag", 1.0)]),
                    ],
                )),
                other => Err(format!("no effect called {other}")),
            }
        };
        let fields = flatten("fields", &[], &mut lib).expect("flattens");
        assert_eq!(
            fields.states[0].steps[0].first,
            Feed::Input("self".to_owned()),
            "not the main pass before it"
        );
        assert_eq!(
            fields.states[1].steps[0].uses,
            vec![("a".to_owned(), Feed::State(0))]
        );
        assert!(
            fields.states[0].steps[1]
                .signature
                .known
                .contains(&"inner".to_owned()),
            "a name saved inside a state is declared for its passes"
        );
        let chained = chain(vec![fields.clone(), fields]);
        assert_eq!(
            chained.states[3].steps[0].uses,
            vec![("a".to_owned(), Feed::State(2))]
        );
        assert_eq!(
            chained.steps[3].uses,
            vec![("b".to_owned(), Feed::State(3))]
        );
        let refused = flatten("early", &[], &mut lib).expect_err("read before it is made");
        assert!(refused.contains("before"), "{refused}");
    }

    /// **A `repeat`'s name is a new param**: it becomes `p_<as>` beside the
    /// effect's own, so one that is already a param, or is no GLSL name, is
    /// refused here rather than by the driver inside the prelude.
    #[test]
    fn a_repeats_name_is_a_new_glsl_name() {
        let repeat = |as_name: &str| Stage::Repeat {
            over: vec![1.0, 2.0],
            as_name: as_name.to_owned(),
            body: vec![pass("jump.frag", 1.0)],
        };
        for (as_name, says) in [("offset", "offset"), ("Jump", "Jump"), ("2x", "2x")] {
            let mut lib = |_: &str, _: &[(String, Value)]| -> Result<Binding, String> {
                Ok(Binding {
                    stages: vec![repeat(as_name)],
                    inputs: vec!["self".to_owned()],
                    params: vec![("offset".to_owned(), Value::Number(1.0))],
                })
            };
            let refused = flatten("x", &[], &mut lib).expect_err(as_name);
            assert!(refused.contains(says), "{refused}");
        }
        let mut nested = |_: &str, _: &[(String, Value)]| -> Result<Binding, String> {
            Ok(binding(
                &["self"],
                vec![Stage::Repeat {
                    over: vec![1.0],
                    as_name: "jump".to_owned(),
                    body: vec![repeat("jump")],
                }],
            ))
        };
        assert!(flatten("x", &[], &mut nested).is_err(), "jump inside jump");
    }

    /// **A saved name is a new GLSL name**: a `save`'s or a `state`'s name,
    /// and a `state:<name>` input's, becomes `sol_<name>` in every pass's
    /// prelude, so one that is no GLSL name, one of the prelude's own, or
    /// another such name's `_box` or `_sampler` is refused here rather than
    /// by the driver inside the prelude, even where no pass reads it.
    #[test]
    fn a_saved_name_is_a_new_glsl_name() {
        let state = |name: &str| Stage::State {
            name: name.to_owned(),
            format: Format::Rgba8,
            scale: 1.0,
            depends: Depends::Shape,
            body: vec![pass("d.frag", 1.0)],
        };
        let save = |name: &str| Stage::Save(name.to_owned());
        let cases: Vec<(&str, Vec<&str>, Vec<Stage>)> = vec![
            (
                "time",
                vec!["self"],
                vec![pass("a.frag", 1.0), save("time")],
            ),
            ("shape", vec!["self"], vec![state("shape")]),
            (
                "effect",
                vec!["self"],
                vec![pass("a.frag", 1.0), save("effect")],
            ),
            ("tex_box", vec!["self"], vec![state("tex_box")]),
            (
                "Sharp",
                vec!["self"],
                vec![pass("a.frag", 1.0), save("Sharp")],
            ),
            ("2x", vec!["self"], vec![state("2x")]),
            ("a-b", vec!["self"], vec![pass("a.frag", 1.0), save("a-b")]),
            ("", vec!["self"], vec![pass("a.frag", 1.0), save("")]),
            (
                "sharp_box",
                vec!["self"],
                vec![
                    pass("a.frag", 1.0),
                    save("sharp"),
                    save("sharp_box"),
                    pass_using("b.frag", &["sharp"]),
                ],
            ),
            ("self_sampler", vec!["self"], vec![state("self_sampler")]),
            (
                "seed",
                vec!["self", "state:seed"],
                vec![pass("a.frag", 1.0)],
            ),
            (
                "noise",
                vec!["self"],
                vec![Stage::Repeat {
                    over: Vec::new(),
                    as_name: "n".to_owned(),
                    body: vec![pass("a.frag", 1.0), save("noise")],
                }],
            ),
        ];
        for (name, inputs, stages) in cases {
            let mut lib = |_: &str, _: &[(String, Value)]| -> Result<Binding, String> {
                Ok(binding(&inputs, stages.clone()))
            };
            let refused = flatten("x", &[], &mut lib).expect_err(name);
            assert!(refused.contains(&format!("\"{name}\"")), "{refused}");
        }
        let mut fine = |_: &str, _: &[(String, Value)]| -> Result<Binding, String> {
            Ok(binding(
                &["self", "backdrop", "state:dist"],
                vec![
                    state("dist"),
                    pass("a.frag", 1.0),
                    save("sharp_2"),
                    save("box"),
                    pass_using("b.frag", &["sharp_2", "box", "dist", "backdrop"]),
                ],
            ))
        };
        assert!(flatten("x", &[], &mut fine).is_ok());
    }

    /// **A plan runs at most 256 passes**, so a `repeat` over a long list,
    /// or effects that each use the next many times, are refused at load
    /// rather than building a plan nothing could draw.
    #[test]
    fn a_plan_runs_at_most_256_passes() {
        let over = |count: u32| (0..count).map(f64::from).collect::<Vec<f64>>();
        let mut lib = |name: &str, _: &[(String, Value)]| -> Result<Binding, String> {
            let count = if name == "most" { 256 } else { 257 };
            Ok(binding(
                &["self"],
                vec![Stage::Repeat {
                    over: over(count),
                    as_name: "n".to_owned(),
                    body: vec![pass("a.frag", 1.0)],
                }],
            ))
        };
        assert_eq!(
            flatten("most", &[], &mut lib).expect("256").steps.len(),
            256
        );
        let refused = flatten("more", &[], &mut lib).expect_err("257");
        assert!(refused.contains("256"), "{refused}");
    }

    /// **A pass reads at most eight textures**: `sol_tex` on unit 0 and seven
    /// in `uses` on the next, GLES 2's guaranteed minimum (Ruling 10). An
    /// eighth name in `uses` is refused at load, naming the pass; `shape`,
    /// which is no texture, is not counted.
    #[test]
    fn a_pass_reads_at_most_eight_textures() {
        let names = ["s1", "s2", "s3", "s4", "s5", "s6", "s7", "s8"];
        let with = |uses: &[&str]| {
            let mut stages = vec![pass("a.frag", 1.0)];
            stages.extend(names.iter().map(|name| Stage::Save((*name).to_owned())));
            stages.push(pass_using("many.frag", uses));
            binding(&["self"], stages)
        };
        let mut seven = |_: &str, _: &[(String, Value)]| Ok(with(&names[..7]));
        assert_eq!(
            flatten("x", &[], &mut seven).expect("eight textures").steps[1]
                .uses
                .len(),
            7
        );
        let mut shaped = |_: &str, _: &[(String, Value)]| {
            let mut uses = names[..7].to_vec();
            uses.push("shape");
            Ok(with(&uses))
        };
        assert!(
            flatten("x", &[], &mut shaped).is_ok(),
            "shape is no texture"
        );
        let mut eight = |_: &str, _: &[(String, Value)]| Ok(with(&names));
        let refused = flatten("x", &[], &mut eight).expect_err("nine textures");
        assert!(
            refused.contains("many.frag") && refused.contains("at most 8"),
            "{refused}"
        );
    }

    /// **A three-pass blur returns to its odd size**: 1151×101 goes down to
    /// 144×13 and comes back to 1151×101, not 1152×104.
    #[test]
    fn a_three_pass_blur_returns_to_its_odd_size() {
        let plan = flatten("blur", &[], &mut library).expect("flattens");
        let (sizes, _) = plan.sizes((1151, 101));
        assert_eq!(
            sizes,
            [
                (576, 51),
                (288, 26),
                (144, 13),
                (288, 26),
                (576, 51),
                (1151, 101)
            ]
        );
    }

    /// A half-scale `rgba16f` state with a down pass of its own, read by a
    /// later pass's `uses` and, after a `get`, as a pass's first input.
    fn a_shrunk_state(_: &str, _: &[(String, Value)]) -> Result<Binding, String> {
        Ok(binding(
            &["self"],
            vec![
                Stage::State {
                    name: "field".to_owned(),
                    format: Format::Rgba16f,
                    scale: 0.5,
                    depends: Depends::Shape,
                    body: vec![pass("seed.frag", 1.0), pass("shrink.frag", 0.5)],
                },
                pass_using("use.frag", &["field"]),
                Stage::Get("field".to_owned()),
                pass("read.frag", 1.0),
            ],
        ))
    }

    /// **A state's steps start from its own scaled box**, and a step reading
    /// a state is sized from what the state's last step drew: a half-scale
    /// state of a 101×51 box starts at 51×26 and shrinks to 26×13.
    #[test]
    fn a_states_steps_start_from_its_scaled_box() {
        let plan = flatten("x", &[], &mut a_shrunk_state).expect("flattens");
        let (steps, states) = plan.sizes((101, 51));
        assert_eq!(states, [vec![(51, 26), (26, 13)]]);
        assert_eq!(steps, [(101, 51), (26, 13)]);
    }

    /// **A plan lists every format it draws into, once**: its steps', its
    /// states' steps' and its states' own.
    #[test]
    fn a_plan_lists_every_format_it_draws_into_once() {
        let blur = flatten("blur", &[], &mut library).expect("flattens");
        assert_eq!(blur.formats(), [Format::Rgba8]);
        let state = flatten("x", &[], &mut a_shrunk_state).expect("flattens");
        assert_eq!(state.formats(), [Format::Rgba8, Format::Rgba16f]);
    }

    /// **A repeated pass ping-pongs between two targets**: eight jump-flood
    /// steps of one size need two slots, because each reads only the last.
    #[test]
    fn a_repeated_pass_ping_pongs_between_two_targets() {
        let mut jump = |_: &str, _: &[(String, Value)]| -> Result<Binding, String> {
            Ok(Binding {
                stages: vec![Stage::Repeat {
                    over: vec![64.0, 32.0, 16.0, 8.0, 4.0, 2.0, 1.0, 1.0],
                    as_name: "jump".to_owned(),
                    body: vec![pass("jump.frag", 1.0)],
                }],
                inputs: vec!["self".to_owned()],
                params: Vec::new(),
            })
        };
        let plan = flatten("jump", &[], &mut jump).expect("flattens");
        let (sizes, _) = plan.sizes((300, 200));
        let (slots, count) = super::Plan::slots(&plan.steps, &sizes);
        assert_eq!(count, 2, "{slots:?}");
        assert_eq!(slots, [0, 1, 0, 1, 0, 1, 0, 1]);
    }

    /// A saved result stays live until its last reader: `both.frag` reads
    /// `sharp`, so `sharp`'s slot is not reused before it.
    #[test]
    fn a_saved_result_keeps_its_slot_until_its_last_reader() {
        let mut lib = |_: &str, _: &[(String, Value)]| -> Result<Binding, String> {
            Ok(Binding {
                stages: vec![
                    pass("a.frag", 1.0),
                    Stage::Save("sharp".to_owned()),
                    pass("b.frag", 1.0),
                    pass("c.frag", 1.0),
                    Stage::Pass {
                        frag: "both.frag".to_owned(),
                        scale: 1.0,
                        format: Format::Rgba8,
                        uses: vec!["sharp".to_owned()],
                        input: None,
                        per_part: false,
                    },
                ],
                inputs: vec!["self".to_owned()],
                params: Vec::new(),
            })
        };
        let plan = flatten("x", &[], &mut lib).expect("flattens");
        let (sizes, _) = plan.sizes((10, 10));
        let (slots, _) = super::Plan::slots(&plan.steps, &sizes);
        assert!(
            slots[2] != slots[0] && slots[3] != slots[0],
            "sharp's slot was reused while both.frag still reads it: {slots:?}"
        );
        assert_eq!(
            super::Plan::last_readers(&plan.steps),
            [Some(3), Some(2), Some(3), None],
            "a.frag's result is last read by both.frag, through `uses`"
        );
    }

    /// **A slot is shared only at one size and one format**: a two-pass
    /// dual Kawase of 257×129 draws 129×65, 65×33, 129×65 and 257×129, and
    /// its second 129×65 takes the first's slot once nothing reads it; after
    /// it, an `rgba16f` pass of 257×129 does not take the free `rgba8` slot
    /// of that size, and the `rgba8` pass after that does.
    #[test]
    fn a_slot_is_shared_only_at_one_size_and_format() {
        let plan = flatten(
            "blur",
            &[("passes".to_owned(), Value::Int(2))],
            &mut library,
        )
        .expect("flattens");
        let (mut sizes, _) = plan.sizes((257, 129));
        assert_eq!(sizes, [(129, 65), (65, 33), (129, 65), (257, 129)]);
        assert_eq!(
            super::Plan::slots(&plan.steps, &sizes),
            (vec![0, 1, 0, 2], 3)
        );
        let mut steps = plan.steps.clone();
        for format in [Format::Rgba8, Format::Rgba16f, Format::Rgba8] {
            let mut step = steps[0].clone();
            step.format = format;
            step.first = Feed::Step(steps.len() - 1);
            step.size = Size::Like(Feed::Step(steps.len() - 1));
            steps.push(step);
            sizes.push((257, 129));
        }
        assert_eq!(
            super::Plan::slots(&steps, &sizes),
            (vec![0, 1, 0, 2, 3, 4, 2], 5)
        );
    }

    /// A flattened plan from `(frag, first, uses, per_part)`, each step at
    /// scale 1 unless its frag is `down*` (0.5) or `up*` (like the input of
    /// the down step it undoes), its key its frag's hash.
    #[expect(
        clippy::type_complexity,
        reason = "each step's four parts, written out where the tests list them"
    )]
    fn plan(steps: &[(&str, Feed, &[(&str, Feed)], bool)]) -> Plan {
        let mut downs: Vec<Feed> = Vec::new();
        let steps = steps
            .iter()
            .map(|(frag, first, uses, per_part)| {
                let size = if frag.starts_with("down") {
                    downs.push(first.clone());
                    Size::Scaled {
                        of: first.clone(),
                        scale: 0.5,
                    }
                } else if frag.starts_with("up") {
                    Size::Like(downs.pop().unwrap_or(Feed::Input("backdrop".to_owned())))
                } else {
                    Size::Scaled {
                        of: first.clone(),
                        scale: 1.0,
                    }
                };
                Step {
                    effect: "test".to_owned(),
                    frag: (*frag).to_owned(),
                    signature: crate::glsl::Signature {
                        host: crate::glsl::Host::Pass,
                        params: Vec::new(),
                        uses: uses.iter().map(|(n, _)| (*n).to_owned()).collect(),
                        known: Vec::new(),
                    },
                    size,
                    format: Format::Rgba8,
                    first: first.clone(),
                    uses: uses
                        .iter()
                        .map(|(n, f)| ((*n).to_owned(), f.clone()))
                        .collect(),
                    uniforms: Vec::new(),
                    key: crate::glsl::content_hash(&[frag.as_bytes()]),
                    per_part: *per_part,
                }
            })
            .collect();
        Plan {
            steps,
            states: Vec::new(),
            reads: Reads {
                backdrop: true,
                ..Reads::default()
            },
            first_input: "backdrop".to_owned(),
        }
    }

    fn backdrop() -> Feed {
        Feed::Input("backdrop".to_owned())
    }

    /// **A blur over the backdrop is all prefix, and its suffix is empty**:
    /// the memo is the whole blur, and its result the last step's.
    #[test]
    fn a_blur_over_the_backdrop_is_all_prefix_and_its_suffix_is_empty() {
        let blur = plan(&[
            ("down1", backdrop(), &[], false),
            ("down2", Feed::Step(0), &[], false),
            ("down3", Feed::Step(1), &[], false),
            ("up1", Feed::Step(2), &[], false),
            ("up2", Feed::Step(3), &[], false),
            ("up3", Feed::Step(4), &[], false),
        ]);
        let memo = split_memo(&blur, Split::Resize);
        assert_eq!(memo.prefix.as_ref().map(|p| p.steps.len()), Some(6));
        assert!(memo.suffix.steps.is_empty());
        assert_eq!(
            memo.exports,
            vec![Export {
                step: 5,
                name: memo_name(5),
                scale: 1.0
            }]
        );
    }

    /// **`glass-rect` splits after its blur**: the refraction reads the
    /// shape, so it is the suffix, reading the blur's result as `memo:3`
    /// under its own name `soft` and the sharp backdrop as itself.
    #[test]
    fn glass_rect_splits_after_its_blur() {
        let glass = plan(&[
            ("down1", backdrop(), &[], false),
            ("down2", Feed::Step(0), &[], false),
            ("up1", Feed::Step(1), &[], false),
            ("up2", Feed::Step(2), &[], false),
            (
                "refract",
                Feed::Step(3),
                &[("sharp", backdrop()), ("soft", Feed::Step(3))],
                true,
            ),
        ]);
        let memo = split_memo(&glass, Split::Resize);
        assert_eq!(memo.prefix.as_ref().map(|p| p.steps.len()), Some(4));
        let refract = &memo.suffix.steps[0];
        assert_eq!(refract.first, Feed::Input(memo_name(3)));
        assert_eq!(
            refract.uses,
            vec![
                ("sharp".to_owned(), backdrop()),
                ("soft".to_owned(), Feed::Input(memo_name(3)))
            ]
        );
        assert_eq!(
            memo.exports.iter().map(|e| e.step).collect::<Vec<_>>(),
            vec![3]
        );
        assert!(memo.suffix.reads.backdrop, "the suffix's tier stays T2");
    }

    /// **A step reading a per-part name ends the prefix**, even if every
    /// step after it could have been shared.
    #[test]
    fn a_step_reading_a_per_part_name_ends_the_prefix() {
        let tinted = plan(&[
            ("tint-shape", backdrop(), &[], true),
            ("down1", Feed::Step(0), &[], false),
        ]);
        let memo = split_memo(&tinted, Split::Resize);
        assert!(memo.prefix.is_none());
        assert_eq!(memo.suffix, tinted);
    }

    /// **A step reading `self` or a state is never in the prefix**, under
    /// `Split::Longest` too, which keeps a trailing step the size of what it
    /// reads and so cuts nothing back.
    #[test]
    fn a_step_reading_self_or_a_state_ends_the_prefix() {
        let own = plan(&[
            ("down1", backdrop(), &[], false),
            (
                "mix",
                Feed::Step(0),
                &[("self", Feed::Input("self".to_owned()))],
                false,
            ),
        ]);
        let stated = plan(&[
            ("down1", backdrop(), &[], false),
            ("read", Feed::Step(0), &[("field", Feed::State(0))], false),
        ]);
        for split in [Split::Resize, Split::Longest] {
            assert_eq!(
                split_memo(&own, split).prefix.map(|p| p.steps.len()),
                Some(1),
                "{split:?}"
            );
            assert_eq!(
                split_memo(&stated, split).prefix.map(|p| p.steps.len()),
                Some(1),
                "{split:?}"
            );
        }
    }

    /// **A plan whose first input is rebound to `self` is never split**: it
    /// is T1, a capture of the part ([FX2] Ruling 14).
    #[test]
    fn a_plan_rebound_to_self_is_never_split() {
        let mut own = plan(&[("down1", backdrop(), &[], false)]);
        own.first_input = "self".to_owned();
        assert!(split_memo(&own, Split::Resize).prefix.is_none());
    }

    /// **A suffix step sized from a prefix step is sized from the padded box
    /// at that step's scale**: a pass over the second down step of a blur is
    /// a quarter of the box each way, whatever the monitor's size.
    #[test]
    fn a_suffix_step_sized_from_the_prefix_is_sized_from_the_padded_box_at_its_scale() {
        let quarter = plan(&[
            ("down1", backdrop(), &[], false),
            ("down2", Feed::Step(0), &[], false),
            ("tint-shape", Feed::Step(1), &[], true),
        ]);
        let memo = split_memo(&quarter, Split::Resize);
        let (sizes, _) = memo.suffix.sizes((400, 200));
        assert_eq!(sizes, vec![(100, 50)]);
        assert_eq!(
            memo.exports,
            vec![Export {
                step: 1,
                name: memo_name(1),
                scale: 0.25
            }]
        );
    }

    /// **The prefix's key is its own steps and not the suffix's**, so frost
    /// and glass with one blur share a memo; and a param of the blur moves
    /// it.
    #[test]
    fn the_prefix_key_is_its_steps_and_not_the_suffixs() {
        let blur = || {
            vec![
                ("down1", backdrop(), &[][..], false),
                ("up1", Feed::Step(0), &[][..], false),
            ]
        };
        let mut a = blur();
        a.push(("tint-shape", Feed::Step(1), &[], true));
        let mut b = blur();
        b.push(("refract", Feed::Step(1), &[], true));
        let (a, b) = (
            split_memo(&plan(&a), Split::Resize),
            split_memo(&plan(&b), Split::Resize),
        );
        assert_eq!(
            a.prefix.as_ref().map(Plan::key),
            b.prefix.as_ref().map(Plan::key)
        );
        let mut offset = plan(&blur());
        offset.steps[0]
            .uniforms
            .push(("offset".to_owned(), crate::spec::Value::Number(2.0)));
        assert_ne!(
            split_memo(&offset, Split::Resize).prefix.map(|p| p.key()),
            a.prefix.map(|p| p.key())
        );
    }

    /// **A trailing step the size of what it reads stays per part**: frost's
    /// tint after its blur is the suffix, reading the blur as `memo:3`, so
    /// a frost at any tint shares one blur (*Review log* finding 3).
    #[test]
    fn a_trailing_same_size_step_stays_per_part() {
        let frost = |tint: f64| {
            let mut frost = plan(&[
                ("down1", backdrop(), &[], false),
                ("down2", Feed::Step(0), &[], false),
                ("up1", Feed::Step(1), &[], false),
                ("up2", Feed::Step(2), &[], false),
                ("tint", Feed::Step(3), &[], false),
            ]);
            frost.steps[4]
                .uniforms
                .push(("tint".to_owned(), crate::spec::Value::Number(tint)));
            split_memo(&frost, Split::Resize)
        };
        let (light, dark) = (frost(0.08), frost(0.12));
        assert_eq!(light.prefix.as_ref().map(|p| p.steps.len()), Some(4));
        assert_eq!(light.suffix.steps.len(), 1);
        assert_eq!(light.suffix.steps[0].first, Feed::Input(memo_name(3)));
        assert_eq!(
            light.prefix.as_ref().map(Plan::key),
            dark.prefix.as_ref().map(Plan::key),
            "two tints, two blurs"
        );
        assert_ne!(light.suffix, dark.suffix);
    }

    /// **A plan with no step that changes size has no prefix**: a tint over
    /// the backdrop alone is per part, never a monitor-sized pass.
    #[test]
    fn a_plan_with_no_resizing_step_has_no_prefix() {
        let tint = plan(&[("tint", backdrop(), &[], false)]);
        assert!(split_memo(&tint, Split::Resize).prefix.is_none());
    }

    /// **The keys are chained**: two plans that begin alike share their keys
    /// as far as they agree, and the plan up to a step is keyed as that step;
    /// the same step after another head is another result, keyed apart.
    #[test]
    fn the_keys_are_chained_so_a_shared_head_has_one_key() {
        let a = plan(&[
            ("down1", backdrop(), &[], false),
            ("up1", Feed::Step(0), &[], false),
            ("down2", Feed::Step(1), &[], false),
        ]);
        let b = plan(&[
            ("down1", backdrop(), &[], false),
            ("up1", Feed::Step(0), &[], false),
            ("down3", Feed::Step(1), &[], false),
        ]);
        let (ka, kb) = (a.keys(), b.keys());
        assert_eq!(ka[..2], kb[..2]);
        assert_ne!(ka[2], kb[2]);
        assert_eq!(a.upto(1).key(), ka[1]);
        assert_eq!(a.key(), ka[2]);
        let c = plan(&[
            ("down3", backdrop(), &[], false),
            ("up1", Feed::Step(0), &[], false),
        ]);
        assert_ne!(c.keys()[1], ka[1], "one up pass after two other downs");
    }

    /// **A pass marked `per_part = true` stays in the suffix**, whatever it
    /// reads: a folder's opt-out of the memo (finding 4).
    #[test]
    fn a_pass_marked_per_part_stays_in_the_suffix() {
        let mut vignette = plan(&[
            ("down1", backdrop(), &[], false),
            ("up1", Feed::Step(0), &[], false),
        ]);
        vignette.steps[1].per_part = true;
        assert_eq!(
            split_memo(&vignette, Split::Resize)
                .prefix
                .map(|p| p.steps.len()),
            Some(1)
        );
    }

    /// **`effects.memo.split` chooses how far the prefix runs** (amended
    /// 2026-10-08, configurable): `Resize` stops at the last resize (frost's
    /// tint per part), `Longest` keeps the trailing tint in the memo (its
    /// suffix empty, one export), `Off` never splits; a tint over the
    /// backdrop alone is a monitor-sized prefix only under `Longest`.
    #[test]
    fn the_split_policy_chooses_how_far_the_prefix_runs() {
        let frost = plan(&[
            ("down1", backdrop(), &[], false),
            ("down2", Feed::Step(0), &[], false),
            ("up1", Feed::Step(1), &[], false),
            ("up2", Feed::Step(2), &[], false),
            ("tint", Feed::Step(3), &[], false),
        ]);
        assert_eq!(
            split_memo(&frost, Split::Resize)
                .prefix
                .map(|p| p.steps.len()),
            Some(4)
        );
        let longest = split_memo(&frost, Split::Longest);
        assert_eq!(longest.prefix.as_ref().map(|p| p.steps.len()), Some(5));
        assert!(longest.suffix.steps.is_empty());
        assert_eq!(
            longest.exports,
            vec![Export {
                step: 4,
                name: memo_name(4),
                scale: 1.0
            }]
        );
        let off = split_memo(&frost, Split::Off);
        assert!(off.prefix.is_none());
        assert_eq!(off.suffix, frost);
        let tint = plan(&[("tint", backdrop(), &[], false)]);
        assert_eq!(
            split_memo(&tint, Split::Longest)
                .prefix
                .map(|p| p.steps.len()),
            Some(1)
        );
        assert!(split_memo(&tint, Split::Resize).prefix.is_none());
    }
}
