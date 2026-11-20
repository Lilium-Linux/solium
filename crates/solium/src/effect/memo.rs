//! The per-monitor memo (Ruling F30): one result per monitor, cut and
//! prefix, shared by every slot whose chain's memoised prefix is the same,
//! re-run only when the xray under it was redrawn or its key moved.
//! `tests::two_slots_with_one_prefix_share_one_memo`,
//! `tests::a_memo_runs_once_per_xray_generation`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use smithay::backend::renderer::gles::GlesTexture;
use smithay::utils::{Physical, Rectangle};

use super::run::Held;
use super::xray::Cut;
use crate::script::{Doc, DocTable};

/// Which memo: a monitor, the cut of xray it reads, and the chained content
/// key of the prefix steps up to the one it holds (`upto`, the prefix's
/// `stage::Plan::keys()[export.step]`), so a prefix result two chains both
/// read is one memo, and another monitor's or cut's is another.
/// Ordered, so the budget gives back the same memo of two equally old ones
/// every time, and judges a pass's places in one order whatever order they
/// ask in. `tests::two_slots_with_one_prefix_share_one_memo`,
/// `tests::a_memo_is_one_per_monitor_and_cut`,
/// `tests::of_two_equally_old_memos_the_lesser_key_gives_way`,
/// `tests::the_outcome_is_the_same_whatever_order_the_places_ask_in`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct MemoKey {
    pub(crate) monitor: String,
    pub(crate) cut: Cut,
    pub(crate) upto: u64,
}

/// One memo's targets and its result, the xray generation it was made
/// from, its own generation (moved on each run), whether a run made it,
/// the last pass a slot wanted it (a place admitted reading it), the plan its run is handed
/// (the prefix up to the step it holds), and the bytes it is counted at
/// against `effects.memo.budget_mib`. `tests::a_memos_generation_moves_on_each_run`,
/// `tests::a_memo_keeps_the_plan_it_was_first_wanted_with`,
/// `tests::a_memo_whose_run_made_nothing_runs_again`,
/// `tests::a_held_memo_is_counted_at_its_latest_size`.
#[derive(Debug, Default)]
pub(crate) struct Memo {
    pub(crate) held: Held,
    pub(crate) generation: u64,
    pub(crate) xray: Option<u64>,
    pub(crate) made: bool,
    pub(crate) seen: u64,
    pub(crate) plan: Option<solium_effects::stage::Plan>,
    pub(crate) bytes: u64,
}

/// Every memo, by key, and the budget a refusal was last logged at.
/// Outside the pool's free-list budget: a memo holds its targets while it
/// is wanted, and [`Memos::sweep`] gives them back.
/// `tests::a_memo_no_slot_wanted_this_pass_is_dropped`,
/// `tests::a_refusal_is_logged_once_per_budget`.
#[derive(Debug, Default)]
pub(crate) struct Memos {
    by_key: HashMap<MemoKey, Memo>,
    refused_logged: Option<u64>,
}

/// One place's ask of [`Memos::admit_pass`]: the memos it reads, each with
/// the bytes it holds at its xray's size, and whether the place read its
/// memos last pass, which is judged first, so two places that cannot both
/// be held do not trade them back and forth as their keys move.
/// `tests::a_place_that_read_its_memos_last_pass_keeps_them`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Ask {
    pub(crate) memos: Vec<(MemoKey, u64)>,
    pub(crate) read_last_pass: bool,
}

/// A MiB's bytes, what `effects.memo.budget_mib` counts in.
/// `tests::a_memo_fits_at_exactly_its_budget`.
const MIB: u64 = 1024 * 1024;

impl Memos {
    /// A slot reads this memo this pass; [`Memos::admit_pass`] marks each
    /// memo it admits so. `tests::two_slots_with_one_prefix_share_one_memo`.
    pub(crate) fn want(&mut self, key: &MemoKey, pass: u64) {
        self.by_key.entry(key.clone()).or_default().seen = pass;
    }

    /// Held, and never run, run with nothing made, or run on another xray
    /// generation; a memo not held is never run.
    /// `tests::a_memo_runs_once_per_xray_generation`,
    /// `tests::a_memo_whose_run_made_nothing_runs_again`.
    pub(crate) fn needs_run(&self, key: &MemoKey, xray_generation: u64) -> bool {
        self.by_key
            .get(key)
            .is_some_and(|memo| !memo.made || memo.xray != Some(xray_generation))
    }

    /// A memo, to run into its `held` or to clear `made` after a run that
    /// made nothing. `tests::a_memo_whose_run_made_nothing_runs_again`.
    pub(crate) fn get_mut(&mut self, key: &MemoKey) -> Option<&mut Memo> {
        self.by_key.get_mut(key)
    }

    /// The plan a wanted memo runs, kept from the first slot that wanted
    /// it: the key is that plan's content, so every later one is the same.
    /// `tests::a_memo_keeps_the_plan_it_was_first_wanted_with`.
    pub(crate) fn set_plan(
        &mut self,
        key: &MemoKey,
        plan: impl FnOnce() -> solium_effects::stage::Plan,
    ) {
        if let Some(memo) = self.by_key.get_mut(key)
            && memo.plan.is_none()
        {
            memo.plan = Some(plan());
        }
    }

    /// `tests::a_memo_keeps_the_plan_it_was_first_wanted_with`; a run takes
    /// it from `Memo::plan` beside the targets it draws into
    /// (`render::run_xray`).
    #[cfg(test)]
    pub(crate) fn plan(&self, key: &MemoKey) -> Option<&solium_effects::stage::Plan> {
        self.by_key.get(key)?.plan.as_ref()
    }

    /// The memo ran over this xray generation: its own generation moves.
    /// `tests::a_memos_generation_moves_on_each_run`.
    pub(crate) fn ran(&mut self, key: &MemoKey, xray_generation: u64) {
        if let Some(memo) = self.by_key.get_mut(key) {
            memo.xray = Some(xray_generation);
            memo.made = true;
            memo.generation = memo.generation.wrapping_add(1);
        }
    }

    /// `tests::a_memos_generation_moves_on_each_run`.
    pub(crate) fn generation(&self, key: &MemoKey) -> Option<u64> {
        self.by_key.get(key).map(|memo| memo.generation)
    }

    /// The memo's result, while it stands: what a suffix reads as
    /// `memo:<k>`, and a crop places (`render::run_slots`).
    pub(crate) fn output(&self, key: &MemoKey) -> Option<&GlesTexture> {
        self.by_key.get(key)?.held.output()
    }

    /// Give back every memo no slot wanted for `keep_passes` passes
    /// (`effects.memo.keep_passes`; 0 gives back what this pass did not
    /// want). [`Memos::admit_pass`] gives one back sooner when a memo a
    /// place asks for needs its room under the budget.
    /// `tests::a_memo_no_slot_wanted_this_pass_is_dropped`,
    /// `tests::a_memo_is_kept_for_keep_passes`.
    pub(crate) fn sweep(&mut self, pass: u64, keep_passes: u64, pool: &mut crate::pool::Pool) {
        self.by_key.retain(|_, memo| {
            let keep = memo.seen.saturating_add(keep_passes) >= pass;
            if !keep {
                memo.held.release(pool);
            }
            keep
        });
    }

    /// `tests::two_slots_with_one_prefix_share_one_memo`.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.by_key.len()
    }

    /// Every memo held, for a test to read.
    /// `state::tests::real_client::a_reload_changing_xray_scale_rebuilds_its_cut_and_reruns_its_memos_once`.
    #[cfg(test)]
    pub(crate) fn keys(&self) -> impl Iterator<Item = &MemoKey> {
        self.by_key.keys()
    }

    /// One pass's memos, judged together (Ruling F30). `requests` holds
    /// each place's [`Ask`]: its memos, each with the bytes it holds at its
    /// xray's size, and whether it read its memos last pass. The answer
    /// says, place by place in the same order, whether the place may read
    /// its memos (`true`) or must run its whole plan (`false`, Task 44). A
    /// place gets all its memos or none.
    ///
    /// With `budget_mib` 0 (`effects.memo.budget_mib`, no cap) every place
    /// gets them. Else a memo is counted at the most any place asks for it,
    /// and the places are judged in an order of their own, never the order
    /// the requests come in: those that read their memos last pass first,
    /// so a place keeps its memos while a prefix uniform it animates gives
    /// it a new key every pass; then those whose memos are all held
    /// already, so what is held stays held; then the rest, each group in
    /// its memos' order. A place is admitted while the memos it asks for that are not
    /// admitted yet fit beside those that are, its other memos counted once.
    /// The memos admitted are marked wanted in `pass` at the bytes asked. A
    /// refused place's memos that no admitted place asks for are given back
    /// at once. Then the memos nobody asked for, kept only by `keep_passes`,
    /// give way, the least recently wanted first and the lesser key of two
    /// equally old, until what is held fits: after every call it fits the
    /// budget, a lowered budget or a grown memo included. A refusal is
    /// logged once per budget.
    /// `tests::a_new_memo_over_the_budget_is_not_admitted`,
    /// `tests::a_memo_fits_at_exactly_its_budget`,
    /// `tests::a_held_memo_is_counted_at_its_latest_size`,
    /// `tests::a_held_memo_grown_past_the_budget_is_refused`,
    /// `tests::a_lowered_budget_refuses_a_held_memo_past_it`,
    /// `tests::a_place_gets_all_its_memos_or_none`,
    /// `tests::an_unwanted_kept_memo_gives_way_to_a_wanted_one`,
    /// `tests::the_least_recently_wanted_memo_gives_way_first`,
    /// `tests::of_two_equally_old_memos_the_lesser_key_gives_way`,
    /// `tests::two_memos_that_cannot_share_the_budget_do_not_trade_places`,
    /// `tests::a_key_that_changes_every_pass_is_admitted_every_pass`,
    /// `tests::the_outcome_is_the_same_whatever_order_the_places_ask_in`,
    /// `tests::what_is_held_never_passes_the_budget`,
    /// `tests::a_place_that_read_its_memos_last_pass_keeps_them`.
    pub(crate) fn admit_pass(
        &mut self,
        requests: &[Ask],
        budget_mib: u64,
        pass: u64,
        pool: &mut crate::pool::Pool,
    ) -> Vec<bool> {
        let cap = budget_mib.saturating_mul(MIB);
        let mut asked: BTreeMap<&MemoKey, u64> = BTreeMap::new();
        for (key, bytes) in requests.iter().flat_map(|place| &place.memos) {
            let at = asked.entry(key).or_insert(0);
            *at = (*at).max(*bytes);
        }
        let places: Vec<Vec<&MemoKey>> = requests
            .iter()
            .map(|place| {
                let mut keys: Vec<&MemoKey> = place.memos.iter().map(|(key, _)| key).collect();
                keys.sort_unstable();
                keys.dedup();
                keys
            })
            .collect();
        let mut order: Vec<(bool, bool, &[&MemoKey], usize)> = places
            .iter()
            .zip(requests)
            .enumerate()
            .map(|(index, (keys, ask))| {
                let new = !keys.iter().all(|key| self.by_key.contains_key(*key));
                (!ask.read_last_pass, new, keys.as_slice(), index)
            })
            .collect();
        order.sort_unstable();
        let mut admitted: BTreeSet<&MemoKey> = BTreeSet::new();
        let mut total: u64 = 0;
        let mut answers = vec![false; requests.len()];
        for (_, _, keys, index) in order {
            let need = keys
                .iter()
                .filter(|key| !admitted.contains(**key))
                .fold(0, |need: u64, key| {
                    need.saturating_add(asked.get(*key).copied().unwrap_or(0))
                });
            if budget_mib == 0 || total.saturating_add(need) <= cap {
                total = total.saturating_add(need);
                admitted.extend(keys.iter().copied());
                if let Some(answer) = answers.get_mut(index) {
                    *answer = true;
                }
            }
        }
        for key in asked.keys() {
            if !admitted.contains(key)
                && let Some(mut memo) = self.by_key.remove(*key)
            {
                memo.held.release(pool);
            }
        }
        for key in &admitted {
            self.want(key, pass);
            if let Some(memo) = self.by_key.get_mut(*key) {
                memo.bytes = asked.get(key).copied().unwrap_or(0);
            }
        }
        while budget_mib != 0 && self.held_bytes() > cap {
            let idle = self
                .by_key
                .iter()
                .filter(|(key, _)| !admitted.contains(key))
                .min_by_key(|(key, memo)| (memo.seen, *key))
                .map(|(key, _)| key.clone());
            let Some(idle) = idle else {
                break;
            };
            if let Some(mut memo) = self.by_key.remove(&idle) {
                memo.held.release(pool);
            }
        }
        if answers.contains(&false) {
            self.note_refusal(budget_mib);
        }
        answers
    }

    /// Log a refusal under `budget_mib` once per budget: again after a
    /// reload sets another, never on every pass at one. Whether it logged.
    /// `tests::a_refusal_is_logged_once_per_budget`.
    fn note_refusal(&mut self, budget_mib: u64) -> bool {
        if self.refused_logged == Some(budget_mib) {
            return false;
        }
        self.refused_logged = Some(budget_mib);
        tracing::warn!(
            budget_mib,
            held = self.held_bytes(),
            "effects.memo.budget_mib reached: a memo is not held, its parts run their whole chain"
        );
        true
    }

    /// The bytes every held memo is counted at.
    /// `tests::a_new_memo_over_the_budget_is_not_admitted`.
    pub(crate) fn held_bytes(&self) -> u64 {
        self.by_key
            .values()
            .fold(0, |held, memo| held.saturating_add(memo.bytes))
    }
}

/// `effects.memo` (Ruling F30; amended 2026-10-08, configurable): how far a
/// memo's prefix runs, how many passes an unwanted memo is kept, and the
/// most the held memos may take, in MiB (0: no cap). The default is
/// `Split::Resize`, 0 and 0. `tests::effects_memo_is_read_and_checked`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MemoConfig {
    pub(crate) split: solium_effects::stage::Split,
    pub(crate) keep_passes: u64,
    pub(crate) budget_mib: u64,
}

/// The keys `effects.memo` takes.
const KEYS: [&str; 3] = ["split", "keep_passes", "budget_mib"];

/// `effects.memo`, read and checked through `Doc`'s own shape: a table of
/// named keys, each refused by name outside its words or bounds, an unknown
/// key named with the nearest known one (as Ruling 28's are).
/// `tests::effects_memo_is_read_and_checked`,
/// `tests::effects_memo_refuses_what_is_not_its_shape`.
pub(crate) fn parse(tree: &Doc) -> Result<MemoConfig, String> {
    use solium_effects::stage::Split;
    let Doc::Table(DocTable { list, fields, .. }) = tree else {
        return Err(
            "`effects.memo` is a table: { split = \"resize\", keep_passes = 0, budget_mib = 0 }"
                .to_owned(),
        );
    };
    if !list.is_empty() {
        return Err("`effects.memo` takes named keys: split, keep_passes or budget_mib".to_owned());
    }
    let mut config = MemoConfig::default();
    for (key, value) in fields {
        match key.as_str() {
            "split" => {
                config.split = match value {
                    Doc::Text(word) if word == "resize" => Split::Resize,
                    Doc::Text(word) if word == "longest" => Split::Longest,
                    Doc::Text(word) if word == "off" => Split::Off,
                    _ => return Err("`effects.memo.split` is resize, longest or off".to_owned()),
                };
            }
            "keep_passes" => config.keep_passes = whole("keep_passes", value, 3600)?,
            "budget_mib" => config.budget_mib = whole("budget_mib", value, 4096)?,
            other => {
                let meant = solium_effects::spec::nearest(other, &KEYS)
                    .map(|meant| format!("; did you mean `{meant}`?"))
                    .unwrap_or_default();
                return Err(format!(
                    "`{other}` is not a memo key: split, keep_passes or budget_mib{meant}"
                ));
            }
        }
    }
    Ok(config)
}

/// `effects.memo.<name>`, a whole number from 0 to `max`: an integer, or a
/// float with no fraction, as Lua holds `2^6`.
/// `tests::effects_memo_refuses_what_is_not_its_shape`.
fn whole(name: &str, value: &Doc, max: u32) -> Result<u64, String> {
    let number = match value {
        Doc::Int(int) => *int,
        #[expect(
            clippy::cast_possible_truncation,
            reason = "finite, whole and far inside an i64, so the cast is exact"
        )]
        Doc::Num(float) if float.fract() == 0.0 && float.abs() < 1e15 => *float as i64,
        _ => {
            return Err(format!(
                "`effects.memo.{name}` is a whole number, 0 to {max}"
            ));
        }
    };
    u64::try_from(number)
        .ok()
        .filter(|read| *read <= u64::from(max))
        .ok_or_else(|| format!("`effects.memo.{name}` is 0 to {max}, not {number}"))
}

/// What the memo of `plan` holds at the xray's `size`: each step's target
/// at its size, rounded up as the run sizes it, 4 bytes a pixel and 8 for
/// `rgba16f`, saturating at `u64::MAX` for a size no GPU makes. An upper
/// bound of the run's ping-pong targets, which is what a budget should
/// count. `tests::a_memos_bytes_are_each_steps_target_at_its_size`,
/// `tests::a_memo_too_big_to_count_saturates`.
pub(crate) fn plan_bytes(plan: &solium_effects::stage::Plan, size: (u32, u32)) -> u64 {
    use solium_effects::stage::Format;
    let (sizes, _) = plan.sizes(size);
    plan.steps
        .iter()
        .zip(sizes)
        .map(|(step, (w, h))| {
            let pixel = match step.format {
                Format::Rgba8 => 4,
                Format::Rgba16f => 8,
            };
            u64::from(w)
                .saturating_mul(u64::from(h))
                .saturating_mul(pixel)
        })
        .fold(0, u64::saturating_add)
}

/// What a T2 suffix's result was made from: the generations of the memos
/// it reads (hashed; 0 when it reads none), the generation of the xray cut
/// its place reads, the part's padded box in the monitor's pixels, its
/// params, its shape, and its self capture's commit when it reads one. A
/// struct with named fields, so FX6's `animate` adds a field (Ruling F52).
/// `xray` is its own field because every T2 run binds the cut as
/// `backdrop`, and a run that reads no memo (`memo = false`,
/// `effects.memo.split = "off"`, a plan with no resizing step, a place over
/// `effects.memo.budget_mib`, `glass-rect` with `soften_passes = 0`) would
/// otherwise keep a stale backdrop after the wallpaper redraws.
/// `tests::a_t2_key_moves_with_each_of_its_fields`. No `Hash`: nothing
/// hashes a `T2Key` (`needs_run` compares it), and smithay 0.7.0's
/// `Rectangle` implements no `Hash`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct T2Key {
    pub(crate) memo: u64,
    pub(crate) xray: u64,
    pub(crate) padded: Rectangle<i32, Physical>,
    pub(crate) params: u64,
    pub(crate) shape: u64,
    pub(crate) own: Option<u64>,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{Ask, MemoKey, Memos, T2Key};
    use crate::effect::xray::Cut;

    fn key(upto: u64) -> MemoKey {
        MemoKey {
            monitor: "DP-1".to_owned(),
            cut: Cut::Background,
            upto,
        }
    }

    /// A plan of `frags` in a chain, built as `stage::tests::plan` builds
    /// one: each step reads the one before it (the first reads `backdrop`),
    /// a `down*` frag at scale 0.5, an `up*` frag sized like the input of the
    /// down step it undoes, any other at 1, each keyed by its frag's hash.
    /// `render`'s tests build their plans with it too.
    pub(crate) fn chain(frags: &[&str]) -> solium_effects::stage::Plan {
        use solium_effects::glsl::{Host, Signature, content_hash};
        use solium_effects::stage::{Feed, Format, Plan, Reads, Size, Step};
        let backdrop = || Feed::Input("backdrop".to_owned());
        let mut downs: Vec<Feed> = Vec::new();
        let steps = frags
            .iter()
            .enumerate()
            .map(|(index, frag)| {
                let first = index.checked_sub(1).map_or_else(backdrop, Feed::Step);
                let size = if frag.starts_with("down") {
                    downs.push(first.clone());
                    Size::Scaled {
                        of: first.clone(),
                        scale: 0.5,
                    }
                } else if frag.starts_with("up") {
                    Size::Like(downs.pop().unwrap_or_else(backdrop))
                } else {
                    Size::Scaled {
                        of: first.clone(),
                        scale: 1.0,
                    }
                };
                Step {
                    effect: "test".to_owned(),
                    frag: (*frag).to_owned(),
                    signature: Signature {
                        host: Host::Pass,
                        params: Vec::new(),
                        uses: Vec::new(),
                        known: Vec::new(),
                    },
                    size,
                    format: Format::Rgba8,
                    first,
                    uses: Vec::new(),
                    uniforms: Vec::new(),
                    key: content_hash(&[frag.as_bytes()]),
                    per_part: false,
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

    /// A one-step plan of `frag`, built as `stage::tests::plan` builds one.
    pub(super) fn one_step(frag: &str) -> solium_effects::stage::Plan {
        chain(&[frag])
    }

    /// **A memo runs once per xray generation**: wanted and never run, it
    /// runs; run on generation 3, it waits for 4.
    #[test]
    fn a_memo_runs_once_per_xray_generation() {
        let mut memos = Memos::default();
        memos.want(&key(7), 1);
        assert!(memos.needs_run(&key(7), 3));
        memos.ran(&key(7), 3);
        assert!(!memos.needs_run(&key(7), 3));
        assert!(memos.needs_run(&key(7), 4));
    }

    /// **A memo whose run made nothing runs again**: Task 44 clears `made`
    /// when a prefix run comes back `Pending` or `Failed`, and `needs_run`
    /// reads the flag, so a memo with no result runs again on the same
    /// xray generation.
    #[test]
    fn a_memo_whose_run_made_nothing_runs_again() {
        let mut memos = Memos::default();
        memos.want(&key(7), 1);
        memos.ran(&key(7), 3);
        memos.get_mut(&key(7)).expect("wanted").made = false;
        assert!(memos.needs_run(&key(7), 3));
    }

    /// **A memo wanted again keeps its result**: the next pass's want marks
    /// it seen and leaves its run, generation and plan as they were, so it
    /// is not run again over the same xray.
    #[test]
    fn a_memo_wanted_again_keeps_its_result() {
        let mut memos = Memos::default();
        memos.want(&key(7), 1);
        memos.set_plan(&key(7), || one_step("down1"));
        memos.ran(&key(7), 3);
        let generation = memos.generation(&key(7));
        memos.want(&key(7), 2);
        assert!(!memos.needs_run(&key(7), 3), "run again over the same xray");
        assert_eq!(memos.generation(&key(7)), generation);
        assert!(memos.plan(&key(7)).is_some());
    }

    /// **Two slots with one prefix share one memo**: wanted twice, it is one
    /// entry, run once.
    #[test]
    fn two_slots_with_one_prefix_share_one_memo() {
        let mut memos = Memos::default();
        memos.want(&key(7), 1);
        memos.want(&key(7), 1);
        memos.want(&key(9), 1);
        assert_eq!(memos.len(), 2);
    }

    /// **A memo is one per monitor and cut** (Ruling F30's key): the same
    /// prefix over another monitor's xray, or over the other cut of the same
    /// monitor, is a memo of its own.
    #[test]
    fn a_memo_is_one_per_monitor_and_cut() {
        let mut memos = Memos::default();
        memos.want(&key(7), 1);
        memos.want(
            &MemoKey {
                monitor: "DP-2".to_owned(),
                ..key(7)
            },
            1,
        );
        memos.want(
            &MemoKey {
                cut: Cut::Bottom,
                ..key(7)
            },
            1,
        );
        assert_eq!(memos.len(), 3);
    }

    /// **A memo's generation moves on each run**, which is what a T2
    /// suffix keys on.
    #[test]
    fn a_memos_generation_moves_on_each_run() {
        let mut memos = Memos::default();
        memos.want(&key(7), 1);
        memos.ran(&key(7), 1);
        let first = memos.generation(&key(7));
        memos.ran(&key(7), 2);
        assert_ne!(memos.generation(&key(7)), first);
    }

    /// **A memo keeps the plan it was first wanted with**: one key, one
    /// plan, since the key is the plan's content.
    #[test]
    fn a_memo_keeps_the_plan_it_was_first_wanted_with() {
        let mut memos = Memos::default();
        memos.want(&key(7), 1);
        assert!(memos.plan(&key(7)).is_none());
        memos.set_plan(&key(7), || crate::effect::memo::tests::one_step("down1"));
        memos.set_plan(&key(7), || unreachable!("built twice"));
        assert_eq!(memos.plan(&key(7)).map(|plan| plan.steps.len()), Some(1));
    }

    /// **A memo nothing wants this pass is given back** at the default
    /// `keep_passes = 0`.
    #[test]
    fn a_memo_no_slot_wanted_this_pass_is_dropped() {
        let (mut memos, mut pool) = (Memos::default(), crate::pool::Pool::new(0));
        memos.want(&key(7), 1);
        memos.want(&key(9), 2);
        memos.sweep(2, 0, &mut pool);
        assert_eq!(memos.len(), 1);
        assert!(memos.generation(&key(9)).is_some());
    }

    /// **`effects.memo.keep_passes` keeps an unwanted memo that long**
    /// (amended 2026-10-08, configurable): wanted at pass 1 and kept for 60
    /// passes, it is still held at pass 61 and given back at 62, so a part
    /// unwanted for a few passes (a covered `auto` bar) reads it again.
    #[test]
    fn a_memo_is_kept_for_keep_passes() {
        let (mut memos, mut pool) = (Memos::default(), crate::pool::Pool::new(0));
        memos.want(&key(7), 1);
        memos.sweep(61, 60, &mut pool);
        assert_eq!(memos.len(), 1, "given back before keep_passes ran out");
        memos.sweep(62, 60, &mut pool);
        assert_eq!(memos.len(), 0);
    }

    /// One place's request: its memos by `upto`, each with its bytes, and
    /// no memo read last pass.
    fn place(memos: &[(u64, u64)]) -> Ask {
        Ask {
            memos: memos
                .iter()
                .map(|&(upto, bytes)| (key(upto), bytes))
                .collect(),
            read_last_pass: false,
        }
    }

    /// Whether each memo of `uptos` is held.
    fn held(memos: &Memos, uptos: &[u64]) -> Vec<bool> {
        uptos
            .iter()
            .map(|upto| memos.generation(&key(*upto)).is_some())
            .collect()
    }

    /// Every order of `0..n`.
    fn permutations(n: usize) -> Vec<Vec<usize>> {
        let Some(last) = n.checked_sub(1) else {
            return vec![Vec::new()];
        };
        let mut all = Vec::new();
        for rest in permutations(last) {
            for at in 0..=rest.len() {
                let mut each = rest.clone();
                each.insert(at, last);
                all.push(each);
            }
        }
        all
    }

    /// **`effects.memo.budget_mib` caps what memos hold** (amended
    /// 2026-10-08, configurable): a memo already held is admitted again at
    /// its size, a new one only while it fits beside the memos admitted; 0
    /// is no cap.
    #[test]
    fn a_new_memo_over_the_budget_is_not_admitted() {
        let mib = 1024 * 1024;
        let (mut memos, mut pool) = (Memos::default(), crate::pool::Pool::new(0));
        let one = [place(&[(7, 3 * mib / 4)])];
        assert_eq!(memos.admit_pass(&one, 1, 1, &mut pool), [true]);
        let both = [place(&[(7, 3 * mib / 4)]), place(&[(9, mib / 2)])];
        assert_eq!(
            memos.admit_pass(&both, 1, 2, &mut pool),
            [true, false],
            "over a 1 MiB budget"
        );
        assert_eq!(
            memos.admit_pass(&both, 0, 3, &mut pool),
            [true, true],
            "0 is no cap"
        );
        assert_eq!(memos.held_bytes(), 3 * mib / 4 + mib / 2);
    }

    /// **A memo fits at exactly its budget, counted in MiB**: 1 MiB of
    /// memos is held under `budget_mib = 1`, and one byte more is not.
    #[test]
    fn a_memo_fits_at_exactly_its_budget() {
        let mib = 1024 * 1024;
        let (mut memos, mut pool) = (Memos::default(), crate::pool::Pool::new(0));
        let three = [
            place(&[(7, mib / 2)]),
            place(&[(9, mib / 2)]),
            place(&[(11, 1)]),
        ];
        assert_eq!(
            memos.admit_pass(&three, 1, 1, &mut pool),
            [true, true, false],
            "1 MiB under a 1 MiB budget, then one byte more"
        );
    }

    /// **A held memo is counted at its latest size**: asked for at another
    /// size (a new mode, another `effects.xray.scale`), it is judged and
    /// held at that size, so a new memo is judged against what is held now.
    #[test]
    fn a_held_memo_is_counted_at_its_latest_size() {
        let mib = 1024 * 1024;
        let (mut memos, mut pool) = (Memos::default(), crate::pool::Pool::new(0));
        assert_eq!(
            memos.admit_pass(&[place(&[(7, mib / 4)])], 1, 1, &mut pool),
            [true]
        );
        let grown = [place(&[(7, 3 * mib / 4)]), place(&[(9, mib / 2)])];
        assert_eq!(
            memos.admit_pass(&grown, 1, 2, &mut pool),
            [true, false],
            "judged against the memo's first size"
        );
        assert_eq!(memos.held_bytes(), 3 * mib / 4);
    }

    /// **A held memo grown past the budget is refused**: a new mode or a
    /// larger `effects.xray.scale` keeps its key and grows its bytes. Its
    /// places run their whole plan, and what it held is given back at once.
    #[test]
    fn a_held_memo_grown_past_the_budget_is_refused() {
        let mib = 1024 * 1024;
        let (mut memos, mut pool) = (Memos::default(), crate::pool::Pool::new(0));
        assert_eq!(
            memos.admit_pass(&[place(&[(7, mib / 2)])], 1, 1, &mut pool),
            [true]
        );
        assert_eq!(
            memos.admit_pass(&[place(&[(7, 4 * mib)])], 1, 2, &mut pool),
            [false],
            "4 MiB held under a 1 MiB budget"
        );
        assert_eq!((memos.len(), memos.held_bytes()), (0, 0));
    }

    /// **A budget lowered by a reload reaches the memos already held**: one
    /// held at 3 MiB under a 4 MiB budget is refused once it is 1, and given
    /// back at once.
    #[test]
    fn a_lowered_budget_refuses_a_held_memo_past_it() {
        let mib = 1024 * 1024;
        let (mut memos, mut pool) = (Memos::default(), crate::pool::Pool::new(0));
        let one = [place(&[(7, 3 * mib)])];
        assert_eq!(memos.admit_pass(&one, 4, 1, &mut pool), [true]);
        assert_eq!(
            memos.admit_pass(&one, 1, 2, &mut pool),
            [false],
            "3 MiB held under a 1 MiB budget"
        );
        assert_eq!(memos.held_bytes(), 0, "3 MiB still held under 1 MiB");
    }

    /// **A place gets all its memos or none**: one whose second memo does
    /// not fit gets neither, and its first is not counted, so a place judged
    /// after it still fits. A memo it held from an earlier pass that no
    /// admitted place asks for is given back, though there is room for it.
    #[test]
    fn a_place_gets_all_its_memos_or_none() {
        let mib = 1024 * 1024;
        let (mut memos, mut pool) = (Memos::default(), crate::pool::Pool::new(0));
        let pair = [
            place(&[(5, mib / 2), (9, 3 * mib / 4)]),
            place(&[(7, 3 * mib / 4)]),
        ];
        assert_eq!(
            memos.admit_pass(&pair, 1, 1, &mut pool),
            [false, true],
            "the refused place's first memo was counted"
        );
        assert_eq!(held(&memos, &[5, 7, 9]), [false, true, false]);
        assert_eq!(
            memos.admit_pass(&[place(&[(5, mib / 2)])], 2, 2, &mut pool),
            [true]
        );
        let refused = [
            place(&[(5, mib / 2), (9, 2 * mib)]),
            place(&[(7, 3 * mib / 4)]),
        ];
        assert_eq!(memos.admit_pass(&refused, 2, 3, &mut pool), [false, true]);
        assert_eq!(
            held(&memos, &[5, 7, 9]),
            [false, true, false],
            "a refused place's memo was kept"
        );
    }

    /// **A memo nobody asks for gives way to one a place asks for**: the
    /// budget caps what is held, so a memo kept only by `keep_passes` is
    /// given back to make room in the first pass that needs it, while a memo
    /// admitted this pass never is.
    #[test]
    fn an_unwanted_kept_memo_gives_way_to_a_wanted_one() {
        let mib = 1024 * 1024;
        let (mut memos, mut pool) = (Memos::default(), crate::pool::Pool::new(0));
        assert_eq!(
            memos.admit_pass(&[place(&[(5, mib / 2)])], 1, 1, &mut pool),
            [true]
        );
        let three = [
            place(&[(7, mib / 2)]),
            place(&[(9, mib / 2)]),
            place(&[(11, mib / 2)]),
        ];
        assert_eq!(
            memos.admit_pass(&three, 1, 2, &mut pool),
            [true, true, false],
            "a memo kept by keep_passes refused a wanted one"
        );
        assert_eq!(held(&memos, &[5, 7, 9]), [false, true, true]);
        assert_eq!(memos.held_bytes(), mib);
    }

    /// **The least recently wanted memo gives way first**: of three memos
    /// nobody asks for, wanted in passes 1, 2 and 3, the one of pass 1 is
    /// given back to make room, and the two more recent are kept.
    #[test]
    fn the_least_recently_wanted_memo_gives_way_first() {
        let mib = 1024 * 1024;
        let (mut memos, mut pool) = (Memos::default(), crate::pool::Pool::new(0));
        for (pass, upto) in [(1, 5), (2, 7), (3, 9)] {
            assert_eq!(
                memos.admit_pass(&[place(&[(upto, mib / 4)])], 1, pass, &mut pool),
                [true]
            );
        }
        assert_eq!(
            memos.admit_pass(&[place(&[(11, mib / 2)])], 1, 4, &mut pool),
            [true]
        );
        assert_eq!(held(&memos, &[5, 7, 9, 11]), [false, true, true, true]);
    }

    /// **Of two equally old memos, the lesser key gives way**: `MemoKey`'s
    /// order breaks the tie, so the budget gives back the same memo whatever
    /// order a hash map keeps them in (64 maps, each seeded afresh).
    #[test]
    fn of_two_equally_old_memos_the_lesser_key_gives_way() {
        let mib = 1024 * 1024;
        for _ in 0..64 {
            let (mut memos, mut pool) = (Memos::default(), crate::pool::Pool::new(0));
            let two = [place(&[(5, mib / 2)]), place(&[(7, mib / 2)])];
            assert_eq!(memos.admit_pass(&two, 1, 1, &mut pool), [true, true]);
            assert_eq!(
                memos.admit_pass(&[place(&[(9, mib / 2)])], 1, 2, &mut pool),
                [true]
            );
            assert_eq!(
                held(&memos, &[5, 7]),
                [false, true],
                "the greater key gave way"
            );
        }
    }

    /// **Two memos that cannot share the budget do not trade places**: the
    /// one held stays held whatever order the places ask in (Task 44 walks
    /// slots kept in hash maps, whose order changes every pass), though the
    /// other's key is the lesser; the other's places run their whole plan.
    /// Neither gives the other back on some passes, its prefix run again
    /// over the monitor and its parts' edges changing with it.
    #[test]
    fn two_memos_that_cannot_share_the_budget_do_not_trade_places() {
        let mib = 1024 * 1024;
        let (mut memos, mut pool) = (Memos::default(), crate::pool::Pool::new(0));
        assert_eq!(
            memos.admit_pass(&[place(&[(9, 3 * mib / 4)])], 1, 1, &mut pool),
            [true]
        );
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
        for pass in 2..=1000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let mut both = vec![place(&[(7, 3 * mib / 4)]), place(&[(9, 3 * mib / 4)])];
            if state.is_multiple_of(2) {
                both.reverse();
            }
            memos.admit_pass(&both, 1, pass, &mut pool);
            assert_eq!(
                held(&memos, &[7, 9]),
                [false, true],
                "pass {pass}: the memo held changed"
            );
            memos.sweep(pass, 0, &mut pool);
        }
    }

    /// **A key that changes every pass is admitted on every pass**: a prefix
    /// uniform written on the tick (Task 59) gives the part a new memo key
    /// each pass. Under a budget that holds one such memo, the memo of the
    /// pass before, which nobody asks for now, gives way to it every time.
    #[test]
    fn a_key_that_changes_every_pass_is_admitted_every_pass() {
        let mib = 1024 * 1024;
        let (mut memos, mut pool) = (Memos::default(), crate::pool::Pool::new(0));
        for pass in 1..=100 {
            assert_eq!(
                memos.admit_pass(&[place(&[(1000 + pass, 3 * mib / 4)])], 1, pass, &mut pool),
                [true],
                "refused in pass {pass}"
            );
            assert_eq!(memos.held_bytes(), 3 * mib / 4);
            memos.sweep(pass, 0, &mut pool);
        }
    }

    /// **A place that read its memos last pass keeps them**: two parts on
    /// one monitor and cut, each animating a prefix uniform (Task 59), ask
    /// for a new memo key every pass, under a budget that holds one. The
    /// part that read its memo last pass is judged first, so over 1000
    /// passes the same part has it every time and the other runs its whole
    /// plan, whichever new key sorts first (new keys sort by hash).
    #[test]
    fn a_place_that_read_its_memos_last_pass_keeps_them() {
        let mib = 1024 * 1024;
        let (mut memos, mut pool) = (Memos::default(), crate::pool::Pool::new(0));
        let mut state: u64 = 0x5851_f42d_4c95_7f2d;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut read = [false, false];
        let mut holder = None;
        for pass in 1..=1000 {
            let asks: Vec<Ask> = read
                .iter()
                .map(|read_last_pass| Ask {
                    memos: vec![(key(next()), 3 * mib / 4)],
                    read_last_pass: *read_last_pass,
                })
                .collect();
            let answers = memos.admit_pass(&asks, 1, pass, &mut pool);
            assert_eq!(answers.iter().filter(|yes| **yes).count(), 1, "pass {pass}");
            let now = answers.iter().position(|yes| *yes);
            assert_eq!(
                *holder.get_or_insert(now),
                now,
                "pass {pass}: the memo changed hands"
            );
            read = [answers[0], answers[1]];
            memos.sweep(pass, 0, &mut pool);
        }
    }

    /// **The outcome is the same whatever order the places ask in**: each
    /// of the 24 orders of four places, two sharing a memo that one asks for
    /// at a larger size than the other, from the same memos held, gives each
    /// place the same answer and leaves the same memos held at the same size.
    #[test]
    fn the_outcome_is_the_same_whatever_order_the_places_ask_in() {
        let mib = 1024 * 1024;
        let places = [
            place(&[(5, mib / 4), (7, mib / 4)]),
            place(&[(9, mib / 2)]),
            place(&[(11, mib / 2)]),
            place(&[(7, mib / 8), (13, mib / 4)]),
        ];
        let mut outcomes = Vec::new();
        for order in permutations(places.len()) {
            let (mut memos, mut pool) = (Memos::default(), crate::pool::Pool::new(0));
            let earlier = [place(&[(5, mib / 4)]), place(&[(11, mib / 2)])];
            assert_eq!(memos.admit_pass(&earlier, 1, 1, &mut pool), [true, true]);
            let asked: Vec<Ask> = order.iter().map(|each| places[*each].clone()).collect();
            let answers = memos.admit_pass(&asked, 1, 2, &mut pool);
            let mut by_place = [false; 4];
            for (each, answer) in order.iter().zip(answers) {
                by_place[*each] = answer;
            }
            let mut kept: Vec<(u64, u64)> = memos
                .by_key
                .iter()
                .map(|(key, memo)| (key.upto, memo.bytes))
                .collect();
            kept.sort_unstable();
            outcomes.push((by_place, kept));
        }
        assert!(
            outcomes.windows(2).all(|pair| pair[0] == pair[1]),
            "{outcomes:?}"
        );
        assert_eq!(
            outcomes[0],
            (
                [true, false, true, false],
                vec![(5, mib / 4), (7, mib / 4), (11, mib / 2)]
            )
        );
    }

    /// **What is held is what was asked, and never passes the budget**: over
    /// 4000 passes of places asking for memos of changing sizes, under a
    /// budget a reload changes about every fourth pass (so held memos meet a
    /// lowered one), swept with a changing `keep_passes`, after every pass
    /// each admitted place's memos are held at the bytes asked and marked
    /// wanted, no refused place's memo is held unless an admitted place asked
    /// for it, a memo nobody asked for is kept as it was or given back, what
    /// is held is the sum of those, and it fits the budget.
    #[test]
    fn what_is_held_never_passes_the_budget() {
        use std::collections::BTreeSet;
        let mib = 1024 * 1024;
        let (mut memos, mut pool) = (Memos::default(), crate::pool::Pool::new(0));
        let mut state: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = |below: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % below
        };
        let mut budget = 2;
        for pass in 1..=4000 {
            if next(4) == 0 {
                budget = 1 + next(4);
            }
            let sizes: Vec<u64> = (0..8).map(|_| (1 + next(8)) * mib / 4).collect();
            let places: Vec<Ask> = (0..next(5))
                .map(|_| Ask {
                    memos: (0..1 + next(3))
                        .map(|_| {
                            let upto = next(8);
                            (key(upto), sizes[upto as usize])
                        })
                        .collect(),
                    read_last_pass: next(2) == 0,
                })
                .collect();
            let before: Vec<(u64, u64)> = memos
                .by_key
                .iter()
                .map(|(key, memo)| (key.upto, memo.bytes))
                .collect();
            let answers = memos.admit_pass(&places, budget, pass, &mut pool);
            let asked: BTreeSet<u64> = places
                .iter()
                .flat_map(|place| &place.memos)
                .map(|(key, _)| key.upto)
                .collect();
            let admitted: BTreeSet<u64> = places
                .iter()
                .zip(&answers)
                .filter(|(_, yes)| **yes)
                .flat_map(|(place, _)| place.memos.iter().map(|(key, _)| key.upto))
                .collect();
            let mut sum = 0;
            for (key, memo) in &memos.by_key {
                if admitted.contains(&key.upto) {
                    assert_eq!(
                        (memo.bytes, memo.seen),
                        (sizes[key.upto as usize], pass),
                        "pass {pass}: an admitted memo"
                    );
                } else {
                    assert!(
                        !asked.contains(&key.upto),
                        "pass {pass}: a refused place's memo is held"
                    );
                    assert!(
                        before.contains(&(key.upto, memo.bytes)),
                        "pass {pass}: a memo nobody asked for changed"
                    );
                }
                sum += memo.bytes;
            }
            assert!(
                admitted
                    .iter()
                    .all(|upto| memos.generation(&key(*upto)).is_some()),
                "pass {pass}: an admitted memo is not held"
            );
            assert_eq!(memos.held_bytes(), sum);
            assert!(
                sum <= budget * mib,
                "pass {pass}: {sum} bytes held under {budget} MiB"
            );
            memos.sweep(pass, next(3), &mut pool);
        }
    }

    /// **A refusal is logged once per budget**: again after a reload sets
    /// another `effects.memo.budget_mib`, never on every pass at one.
    #[test]
    fn a_refusal_is_logged_once_per_budget() {
        let mib = 1024 * 1024;
        let (mut memos, mut pool) = (Memos::default(), crate::pool::Pool::new(0));
        assert_eq!(
            memos.admit_pass(&[place(&[(7, 2 * mib)])], 1, 1, &mut pool),
            [false]
        );
        assert!(
            !memos.note_refusal(1),
            "admit_pass's refusal at 1 MiB was not logged"
        );
        assert!(
            memos.note_refusal(2),
            "a reload's new budget was not logged"
        );
        assert!(!memos.note_refusal(2), "logged twice at one budget");
    }

    /// **What a memo holds is each step's target at its size**, rounded up
    /// as the run sizes it, 4 bytes a pixel and 8 in `rgba16f`: what
    /// `effects.memo.budget_mib` counts.
    #[test]
    fn a_memos_bytes_are_each_steps_target_at_its_size() {
        let mut plan = chain(&["down1", "up1"]);
        assert_eq!(
            super::plan_bytes(&plan, (101, 61)),
            (51 * 31 + 101 * 61) * 4
        );
        if let Some(up) = plan.steps.get_mut(1) {
            up.format = solium_effects::stage::Format::Rgba16f;
        }
        assert_eq!(
            super::plan_bytes(&plan, (101, 61)),
            51 * 31 * 4 + 101 * 61 * 8
        );
    }

    /// **A memo too big to count is counted as the most there is**: a stage
    /// scaled past what a `u32` side holds (`scale = 1e9`, or `math.huge`)
    /// saturates `Plan::sizes`, and its bytes saturate rather than overflow,
    /// so any budget refuses it.
    #[test]
    fn a_memo_too_big_to_count_saturates() {
        use solium_effects::stage::{Feed, Size};
        let mut plan = chain(&["down1"]);
        if let Some(step) = plan.steps.get_mut(0) {
            step.size = Size::Scaled {
                of: Feed::Input("backdrop".to_owned()),
                scale: 1e9,
            };
        }
        assert_eq!(super::plan_bytes(&plan, (101, 61)), u64::MAX);
    }

    fn tree(lua: &str) -> crate::script::Doc {
        crate::script::doc::tests::plain(lua).expect("a table")
    }

    /// **`effects.memo` is read and checked by name.**
    #[test]
    fn effects_memo_is_read_and_checked() {
        use solium_effects::stage::Split;
        assert_eq!(
            super::parse(&tree(
                r#"{ split = "longest", keep_passes = 60, budget_mib = 512 }"#
            )),
            Ok(super::MemoConfig {
                split: Split::Longest,
                keep_passes: 60,
                budget_mib: 512
            })
        );
        assert_eq!(super::parse(&tree("{}")), Ok(super::MemoConfig::default()));
        assert_eq!(
            super::MemoConfig::default(),
            super::MemoConfig {
                split: Split::Resize,
                keep_passes: 0,
                budget_mib: 0
            },
            "the shipped defaults"
        );
        assert!(
            super::parse(&tree(r#"{ split = "short" }"#))
                .expect_err("refused")
                .contains("resize, longest or off")
        );
        assert!(
            super::parse(&tree("{ keep_passes = 4000 }"))
                .expect_err("refused")
                .contains("0 to 3600")
        );
        assert!(
            super::parse(&tree("{ budget_mib = 5000 }"))
                .expect_err("refused")
                .contains("0 to 4096")
        );
        assert!(
            super::parse(&tree("{ budgt_mb = 1 }"))
                .expect_err("refused")
                .contains("split, keep_passes or budget_mib")
        );
    }

    /// **`effects.memo` is a table of named keys and whole numbers**:
    /// anything else is refused naming the key (a number that is not finite
    /// included, as Ruling 28's `tree_of` keeps it), a whole number Lua
    /// holds as a float (`2^6`) is read, and a misspelt key is named with
    /// the nearest one.
    #[test]
    fn effects_memo_refuses_what_is_not_its_shape() {
        use crate::script::{Doc, DocTable};
        assert_eq!(
            super::parse(&tree("{ keep_passes = 2^6 }")).map(|config| config.keep_passes),
            Ok(64)
        );
        let not_finite = Doc::Table(DocTable {
            list: Vec::new(),
            fields: [("budget_mib".to_owned(), Doc::Num(f64::INFINITY))].into(),
            ..DocTable::default()
        });
        assert!(
            super::parse(&not_finite)
                .expect_err("refused")
                .contains("`effects.memo.budget_mib` is a whole number")
        );
        for (lua, says) in [
            ("true", "`effects.memo` is a table"),
            (r#"{ "resize" }"#, "takes named keys"),
            (
                "{ keep_passes = 1.5 }",
                "`effects.memo.keep_passes` is a whole number",
            ),
            (
                "{ keep_passes = -1 }",
                "`effects.memo.keep_passes` is 0 to 3600",
            ),
            (
                r#"{ budget_mib = "lots" }"#,
                "`effects.memo.budget_mib` is a whole number",
            ),
            (
                r#"{ split = 1 }"#,
                "`effects.memo.split` is resize, longest or off",
            ),
            ("{ budgt_mb = 1 }", "did you mean `budget_mib`?"),
        ] {
            let refused = super::parse(&tree(lua)).expect_err(lua);
            assert!(refused.contains(says), "{lua}: {refused}");
        }
    }

    /// **The T2 key is a struct with named fields** (Ruling F52): a suffix
    /// re-runs when any one of them moves, and two keys equal in every
    /// field are equal.
    #[test]
    fn a_t2_key_moves_with_each_of_its_fields() {
        let base = T2Key {
            memo: 1,
            xray: 4,
            padded: smithay::utils::Rectangle::new((10, 10).into(), (100, 40).into()),
            params: 2,
            shape: 3,
            own: None,
        };
        assert_eq!(base, T2Key { ..base });
        for moved in [
            T2Key { memo: 9, ..base },
            T2Key { xray: 9, ..base },
            T2Key {
                padded: smithay::utils::Rectangle::new((11, 10).into(), (100, 40).into()),
                ..base
            },
            T2Key { params: 9, ..base },
            T2Key { shape: 9, ..base },
            T2Key {
                own: Some(1),
                ..base
            },
        ] {
            assert_ne!(base, moved);
        }
    }
}
