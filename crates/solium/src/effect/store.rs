//! What a slot keeps from one pass to the next: its part's capture, its
//! chain's targets, the id and commit its result is drawn with, and the
//! params it last ran at (Ruling 16), and for a T2 slot one result per
//! monitor (Ruling F30). Kept on `Solium`, swept once a pass, so a slot no
//! rule wants any more gives its targets back.
//! `tests::a_slot_no_rule_wanted_this_pass_is_dropped`,
//! `tests::a_t2_suffix_reruns_only_when_its_key_moves`.

use std::collections::{BTreeMap, HashMap};

use smithay::backend::renderer::{element::Id, gles::GlesTexture, utils::CommitCounter};

use super::memo::T2Key;
use super::plan::Owner;
use super::rules::{RuleKey, Slot};

/// One slot's state between passes.
/// `tests::a_new_rule_in_a_slot_starts_afresh`.
#[derive(Debug)]
pub(crate) struct SlotState {
    /// The rule the state was made for: another one in the slot starts afresh.
    pub(crate) rule: RuleKey,
    /// The part's self input, padded by its chain's reach.
    /// `state::tests::real_client::a_self_rule_captures_the_client_once_until_it_commits`.
    pub(crate) input: crate::keyed::Capture,
    /// The chain's targets, held across runs.
    pub(crate) held: super::run::Held,
    /// The id its result is drawn with, the same for as long as the slot
    /// keeps this state. `tests::an_unchanged_chain_keeps_its_outputs_id_and_commit`.
    pub(crate) id: Id,
    /// Its result's commit, moved when the chain re-ran or the result's box
    /// moved. `tests::an_unchanged_chain_keeps_its_outputs_id_and_commit`.
    commit: CommitCounter,
    /// Where its result was last placed.
    placed: Option<super::element::Placement>,
    /// The params' hash the chain last ran at.
    /// `tests::a_param_or_size_change_reruns_the_chain`.
    pub(crate) params: u64,
    /// The padded box's size the chain last ran at; `None` until it has run.
    /// `tests::a_self_chain_runs_once_until_its_part_commits`.
    size: Option<(u32, u32)>,
    /// The pass this slot was last wanted in.
    pub(crate) seen: u64,
    /// A T2 slot's result on each monitor its part's padded box overlaps, by
    /// the monitor's name (Ruling F30).
    /// `tests::a_t2_suffix_reruns_only_when_its_key_moves`.
    pub(crate) t2: BTreeMap<String, T2State>,
    /// The id of the hole a warped pane's capture keeps where this slot is
    /// drawn beneath the warp (`effect::beneath::Hole`, Ruling F37), the
    /// same for as long as the slot keeps this state.
    /// `render::tests::a_beneath_slot_is_a_hole_only_inside_a_capture`.
    pub(crate) hole: Id,
    /// The pass instant the chain last ran at, for `effects.motion_hz`
    /// ([`SlotState::waits`]).
    pub(crate) ran_at: Option<std::time::Duration>,
}

impl SlotState {
    /// A slot's state before anything is captured or run.
    /// `tests::a_new_rule_in_a_slot_starts_afresh`.
    pub(crate) fn fresh(rule: RuleKey) -> Self {
        Self {
            rule,
            input: crate::keyed::Capture::default(),
            held: super::run::Held::default(),
            id: Id::new(),
            commit: CommitCounter::default(),
            placed: None,
            params: 0,
            size: None,
            seen: 0,
            t2: BTreeMap::new(),
            hole: Id::new(),
            ran_at: None,
        }
    }

    /// The T2 result on `monitor`, made on first use.
    /// `tests::a_t2_suffix_reruns_only_when_its_key_moves`.
    pub(crate) fn t2_mut(&mut self, monitor: &str) -> &mut T2State {
        self.t2.entry(monitor.to_owned()).or_default()
    }

    /// Keep the T2 results of the monitors the slot is placed on this pass,
    /// giving every other monitor's targets back: a window moved off a
    /// monitor holds nothing there.
    /// `tests::a_t2_result_on_a_monitor_left_is_given_back`.
    pub(crate) fn keep_t2(&mut self, monitors: &[&str], pool: &mut crate::pool::Pool) {
        self.t2.retain(|monitor, t2| {
            let keep = monitors.contains(&monitor.as_str());
            if !keep {
                t2.held.release(pool);
            }
            keep
        });
    }

    /// Whether the chain must run this pass: it never has, its self input
    /// was `redrawn`, or its `params` or its padded box's `size` differ from
    /// the last run's (Ruling 16). Otherwise its last result is placed again.
    /// `tests::a_self_chain_runs_once_until_its_part_commits`,
    /// `tests::a_param_or_size_change_reruns_the_chain`,
    /// `tests::a_redrawn_input_reruns_the_chain_every_pass`.
    pub(crate) fn needs_run(&self, redrawn: bool, params: u64, size: (u32, u32)) -> bool {
        redrawn || self.size != Some(size) || self.params != params
    }

    /// Whether a chain whose box has only changed size, its input not
    /// redrawn and its params as they were, waits for its next tick of
    /// `motion` before it runs again, its last result drawn stretched over
    /// the part meanwhile: while it last ran in the tick `now` falls in. So
    /// a T0 chain (a shade, a ring) on a window being resized runs at most
    /// `effects.motion_hz` times a second, at every size of a 260 Hz drag
    /// no more, and at its last size within one tick of the window
    /// stopping. `render::run_slots` asks it of a T0 chain only.
    /// `tests::a_resized_parts_t0_chain_runs_at_most_motion_hz_times_a_second`.
    pub(crate) fn waits(
        &self,
        redrawn: bool,
        params: u64,
        size: (u32, u32),
        now: std::time::Duration,
        motion: Motion,
    ) -> bool {
        let (Some(had), Some(ran)) = (self.size, self.ran_at) else {
            return false;
        };
        !redrawn
            && self.params == params
            && had != size
            && motion
                .tick(ran)
                .is_some_and(|tick| Some(tick) == motion.tick(now))
    }

    /// The chain ran at `params` over a box of `size`.
    /// `tests::a_self_chain_runs_once_until_its_part_commits`.
    pub(crate) fn ran(&mut self, params: u64, size: (u32, u32)) {
        self.params = params;
        self.size = Some(size);
    }

    /// The commit its result carries at `placement`: moved when the chain
    /// re-ran or the placement differs from the last, so a result placed
    /// again unchanged damages nothing under it (`element::commit_for`).
    /// `tests::an_unchanged_chain_keeps_its_outputs_id_and_commit`.
    pub(crate) fn commit_for(
        &mut self,
        placement: super::element::Placement,
        rerun: bool,
    ) -> CommitCounter {
        let commit = super::element::commit_for(&mut self.commit, self.placed, placement, rerun);
        self.placed = Some(placement);
        commit
    }

    /// Give the capture's target, the chain's and each monitor's T2
    /// result's back: what a sweep and a new rule in the slot do.
    /// `tests::a_slot_no_rule_wanted_this_pass_is_dropped`.
    fn release(mut self, pool: &mut crate::pool::Pool) {
        self.input.release(pool);
        self.held.release(pool);
        for t2 in self.t2.values_mut() {
            t2.held.release(pool);
        }
    }
}

/// One monitor's T2 result for a slot (Ruling F30): the key its suffix last
/// ran at, its targets, the id, commit and placement its result is drawn
/// with there, and the last pass its place read its memos (`read_at`), which
/// `memo::Memos::admit_pass` judges first.
/// `tests::a_t2_suffix_reruns_only_when_its_key_moves`,
/// `tests::a_t2_results_commit_moves_only_when_rerun_or_placed_elsewhere`.
#[derive(Debug)]
pub(crate) struct T2State {
    pub(crate) key: Option<T2Key>,
    pub(crate) held: super::run::Held,
    pub(crate) id: Id,
    pub(crate) commit: CommitCounter,
    pub(crate) placed: Option<super::element::Placement>,
    pub(crate) read_at: Option<u64>,
    /// The warp's commit its beneath draw last saw, and the beneath's own
    /// (Ruling F37): `tests::an_unders_commit_moves_with_its_warp_or_its_rerun`.
    pub(crate) under: Option<(CommitCounter, CommitCounter)>,
    /// The pass instant its suffix last ran at, for `effects.motion_hz`
    /// ([`T2State::waits`]).
    pub(crate) ran_at: Option<std::time::Duration>,
}

/// Never run, with an id of its own.
/// `tests::a_t2_suffix_reruns_only_when_its_key_moves`.
impl Default for T2State {
    fn default() -> Self {
        Self {
            key: None,
            held: super::run::Held::default(),
            id: Id::new(),
            commit: CommitCounter::default(),
            placed: None,
            read_at: None,
            under: None,
            ran_at: None,
        }
    }
}

/// How often, at most, a chain moved only by its part's place or size runs
/// again, `effects.motion_hz` (0: on every pass), and where in one
/// interval of that rate this place's ticks fall (`phase`, 0 to 1), so the
/// glass of windows that start moving together does not run together on
/// the same passes. `tests::a_moving_parts_suffix_runs_at_most_motion_hz_times_a_second`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Motion {
    pub(crate) hz: f64,
    pub(crate) phase: f64,
}

impl Motion {
    /// Which tick of this place's clock `at` falls in; `None` with no cap.
    fn tick(self, at: std::time::Duration) -> Option<u64> {
        if self.hz <= 0.0 || !self.hz.is_finite() {
            return None;
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a count of ticks of a session's clock, whole and far below 2^53"
        )]
        let tick = (at.as_secs_f64() * self.hz + self.phase.clamp(0.0, 1.0)).floor() as u64;
        Some(tick)
    }
}

impl T2Key {
    /// Whether this key differs from `last` only in where its part is drawn
    /// and how big it is (its padded box and its shape): what moves it on
    /// every pass of a glide, a drag or a resize, and nothing else does.
    /// `tests::a_moving_parts_suffix_runs_at_most_motion_hz_times_a_second`.
    pub(crate) fn moved_only(&self, last: &Self) -> bool {
        self.memo == last.memo
            && self.xray == last.xray
            && self.params == last.params
            && self.own == last.own
            && (self.padded != last.padded || self.shape != last.shape)
    }
}

impl T2State {
    /// Whether the suffix must run: it never has (or its last run left no
    /// result, Task 44), or its key moved.
    /// `tests::a_t2_suffix_reruns_only_when_its_key_moves`.
    pub(crate) fn needs_run(&self, key: T2Key) -> bool {
        self.key != Some(key)
    }

    /// Whether a suffix whose key moved only with its part ([`T2Key::moved_only`])
    /// waits for its next tick of `motion` before it runs again, its last
    /// result drawn where the part is meanwhile: while it last ran in the
    /// tick `now` falls in. So a moving part's glass is run at most
    /// `motion_hz` times a second, and at its last place within one tick of
    /// its stopping. A key moved by anything else (the backdrop redrawn, its
    /// params) runs at once.
    /// `tests::a_moving_parts_suffix_runs_at_most_motion_hz_times_a_second`.
    pub(crate) fn waits(&self, key: T2Key, now: std::time::Duration, motion: Motion) -> bool {
        let (Some(last), Some(ran)) = (self.key, self.ran_at) else {
            return false;
        };
        key != last
            && key.moved_only(&last)
            && motion
                .tick(ran)
                .is_some_and(|tick| Some(tick) == motion.tick(now))
    }

    /// The commit its result carries at `placement` on this monitor, as
    /// [`SlotState::commit_for`]: moved when it re-ran or is placed
    /// elsewhere, a crop moved included.
    /// `tests::a_t2_results_commit_moves_only_when_rerun_or_placed_elsewhere`.
    pub(crate) fn commit_for(
        &mut self,
        placement: super::element::Placement,
        rerun: bool,
    ) -> CommitCounter {
        let commit = super::element::commit_for(&mut self.commit, self.placed, placement, rerun);
        self.placed = Some(placement);
        commit
    }

    /// The commit its beneath draw carries (Ruling F37, amended 2026-10-14,
    /// live under motion and warps): moved when the warp's moved or its
    /// texture re-ran, kept otherwise, so a held tilt is not damaged pass
    /// after pass.
    /// `tests::an_unders_commit_moves_with_its_warp_or_its_rerun`.
    pub(crate) fn under_commit(&mut self, warp: CommitCounter, rerun: bool) -> CommitCounter {
        let (seen, mut own) = self.under.unwrap_or((warp, CommitCounter::default()));
        if rerun || seen != warp {
            own.increment();
        }
        self.under = Some((warp, own));
        own
    }

    /// Whether its place read its memos in the pass before `pass`.
    /// `tests::a_t2_results_commit_moves_only_when_rerun_or_placed_elsewhere`.
    pub(crate) fn read_last_pass(&self, pass: u64) -> bool {
        self.read_at
            .is_some_and(|read| read.saturating_add(1) == pass)
    }

    /// Its place is a crop of a memo this pass, with `now` its key, and
    /// runs no pass (`render::T2Work::Crop`): the targets a suffix or a
    /// whole-plan run held for it go back, so a place that ran its whole
    /// plan over `effects.memo.budget_mib` holds nothing part-sized once
    /// its memo is admitted. Whether the memo it crops moved since, so its
    /// commit moves.
    /// `tests::a_t2_place_that_turns_to_a_crop_gives_its_targets_back`.
    pub(crate) fn cropped(&mut self, now: T2Key, pool: &mut crate::pool::Pool) -> bool {
        let rerun = self.key.is_none_or(|was| was.memo != now.memo);
        self.key = Some(now);
        self.held.release(pool);
        rerun
    }
}

/// Every slot's state, by its owner and slot.
/// `tests::a_slot_no_rule_wanted_this_pass_is_dropped`.
#[derive(Debug, Default)]
pub(crate) struct Store {
    slots: HashMap<(Owner, Slot), SlotState>,
    /// States replaced by a new rule in their slot, whose targets go back at
    /// the next sweep. `tests::a_new_rule_in_a_slot_starts_afresh`.
    retired: Vec<SlotState>,
    /// The passes counted so far.
    pass: u64,
}

impl Store {
    /// A new pass: what the slots wanted in it are marked with.
    /// `tests::a_slot_no_rule_wanted_this_pass_is_dropped`.
    pub(crate) fn next_pass(&mut self) -> u64 {
        self.pass += 1;
        self.pass
    }

    /// A slot's state, made on first use and made afresh when another rule
    /// fills the slot, the old one's targets going back at the next sweep.
    /// `tests::a_new_rule_in_a_slot_starts_afresh`.
    pub(crate) fn slot_mut(&mut self, owner: &Owner, slot: Slot, rule: RuleKey) -> &mut SlotState {
        let key = (owner.clone(), slot);
        if self.slots.get(&key).is_some_and(|state| state.rule != rule)
            && let Some(old) = self.slots.remove(&key)
        {
            self.retired.push(old);
        }
        self.slots
            .entry(key)
            .or_insert_with(|| SlotState::fresh(rule))
    }

    /// Before a pass, for a slot crossfading from rule `from` to rule `to`
    /// (`effect::fade`): whichever of the slot and its `Leaving` twin holds
    /// each rule's state, `to`'s is put in the slot and `from`'s in the twin,
    /// and any other rule's state there goes back at the next sweep. So the
    /// rule a slot crossfades away from keeps its result, captured and run
    /// as it was, rather than starting afresh, and a crossfade turned round
    /// part of the way gets each back where it is wanted.
    /// `tests::a_crossfading_slot_keeps_both_rules_states`.
    pub(crate) fn arrange(&mut self, owner: &Owner, slot: Slot, to: RuleKey, from: RuleKey) {
        let here = (owner.clone(), slot.placed());
        let twin = (owner.clone(), slot.leaving());
        let held = [self.slots.remove(&here), self.slots.remove(&twin)];
        for state in held.into_iter().flatten() {
            if state.rule == to && !self.slots.contains_key(&here) {
                self.slots.insert(here.clone(), state);
            } else if state.rule == from && !self.slots.contains_key(&twin) {
                self.slots.insert(twin.clone(), state);
            } else {
                self.retired.push(state);
            }
        }
    }

    /// A slot's state as it is, made by [`Self::slot_mut`] this pass.
    /// `state::tests::real_client::a_self_rule_captures_the_client_once_until_it_commits`.
    pub(crate) fn get_mut(&mut self, owner: &Owner, slot: Slot) -> Option<&mut SlotState> {
        self.slots.get_mut(&(owner.clone(), slot))
    }

    /// A slot's state as it is, to read.
    /// `memo::tests::a_place_that_read_its_memos_last_pass_keeps_them`'s
    /// flag is read here by `render::want_t2`.
    pub(crate) fn get(&self, owner: &Owner, slot: Slot) -> Option<&SlotState> {
        self.slots.get(&(owner.clone(), slot))
    }

    /// Drop every slot not wanted in `pass`, and every state a new rule
    /// replaced, giving their targets back.
    /// `tests::a_slot_no_rule_wanted_this_pass_is_dropped`.
    pub(crate) fn sweep(&mut self, pass: u64, pool: &mut crate::pool::Pool) {
        for state in self.retired.drain(..) {
            state.release(pool);
        }
        if self.slots.values().all(|state| state.seen >= pass) {
            return;
        }
        let gone: Vec<(Owner, Slot)> = self
            .slots
            .iter()
            .filter(|(_, state)| state.seen < pass)
            .map(|(key, _)| key.clone())
            .collect();
        for key in gone {
            if let Some(state) = self.slots.remove(&key) {
                state.release(pool);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, owner: &Owner, slot: Slot) -> bool {
        self.slots.contains_key(&(owner.clone(), slot))
    }
}

/// Each self input this pass: its texture and the size it was drawn at in
/// that texture's corner (a slot's self input is drawn into a target with
/// room, #428), whether it was drawn this pass or kept from an earlier one,
/// and its capture's commit. What the runs read
/// (`render::run_slots`): a chain re-runs only when its input was redrawn.
/// `tests::drawn_tells_an_input_redrawn_this_pass_from_one_kept`.
#[derive(Debug)]
pub(crate) struct Drawn<T = (GlesTexture, (u32, u32))> {
    inputs: HashMap<(Owner, Slot), (T, bool, CommitCounter)>,
}

impl<T> Default for Drawn<T> {
    fn default() -> Self {
        Self {
            inputs: HashMap::new(),
        }
    }
}

impl<T> Drawn<T> {
    /// `tests::drawn_tells_an_input_redrawn_this_pass_from_one_kept`.
    pub(crate) fn insert(
        &mut self,
        owner: Owner,
        slot: Slot,
        texture: T,
        redrawn: bool,
        commit: CommitCounter,
    ) {
        self.inputs
            .insert((owner, slot), (texture, redrawn, commit));
    }

    /// The input's texture, and whether it was redrawn this pass.
    /// `tests::drawn_tells_an_input_redrawn_this_pass_from_one_kept`.
    pub(crate) fn get(&self, owner: &Owner, slot: Slot) -> Option<(&T, bool)> {
        self.inputs
            .get(&(owner.clone(), slot))
            .map(|(texture, redrawn, _)| (texture, *redrawn))
    }

    /// The input's commit, as the number a state depending on `self` is kept
    /// on (`run::Keys::commit`).
    /// `tests::drawn_tells_an_input_redrawn_this_pass_from_one_kept`.
    pub(crate) fn commit(&self, owner: &Owner, slot: Slot) -> Option<u64> {
        self.inputs
            .get(&(owner.clone(), slot))
            .map(|(_, _, commit)| super::run::Keys::commit(*commit))
    }
}

#[cfg(test)]
mod tests {
    use super::{Drawn, Store};
    use crate::effect::plan::{Owner, PaneSlot};
    use crate::effect::rules::{Origin, RuleKey, Slot};

    fn rule() -> RuleKey {
        RuleKey {
            origin: Origin::User,
            index: 0,
            generation: 1,
        }
    }

    fn client(pane: u64) -> Owner {
        Owner::Pane(crate::pane::PaneId::from_raw(pane), PaneSlot::Client)
    }

    /// **A slot that stops resolving gives its targets back**: one not seen
    /// this pass is dropped by the sweep, and one seen stays.
    #[test]
    fn a_slot_no_rule_wanted_this_pass_is_dropped() {
        let mut store = Store::default();
        let mut pool = crate::pool::Pool::new(0);
        let (gone, kept) = (client(1), client(2));
        store.slot_mut(&gone, Slot::Replace, rule()).seen = 1;
        store.slot_mut(&kept, Slot::Replace, rule()).seen = 2;
        store.sweep(2, &mut pool);
        assert_eq!(store.len(), 1);
        assert!(store.contains(&kept, Slot::Replace));
        assert!(store.get_mut(&gone, Slot::Replace).is_none());
    }

    /// A new rule in a slot starts afresh: its old state is given back.
    #[test]
    fn a_new_rule_in_a_slot_starts_afresh() {
        let mut store = Store::default();
        let owner = client(1);
        store.slot_mut(&owner, Slot::Replace, rule()).params = 7;
        assert_eq!(
            store.slot_mut(&owner, Slot::Replace, rule()).params,
            7,
            "the same rule kept its state"
        );
        let other = RuleKey { index: 1, ..rule() };
        assert_eq!(store.slot_mut(&owner, Slot::Replace, other).params, 0);
        assert_eq!(store.len(), 1);
    }

    /// **A crossfading slot keeps both rules' states** (#389): handed over
    /// from one rule to another, the first's state, params and all, moves to
    /// the slot's `Leaving` twin rather than starting afresh, and the second
    /// starts afresh in the slot; turned round, each goes back where it is
    /// wanted; a third rule's state there is given back.
    #[test]
    fn a_crossfading_slot_keeps_both_rules_states() {
        let mut store = Store::default();
        let mut pool = crate::pool::Pool::new(0);
        let owner = client(1);
        let (first, second, third) = (
            rule(),
            RuleKey { index: 1, ..rule() },
            RuleKey { index: 2, ..rule() },
        );
        store.slot_mut(&owner, Slot::Replace, first).params = 7;
        store.arrange(&owner, Slot::Replace, second, first);
        assert_eq!(
            store.slot_mut(&owner, Slot::LeavingReplace, first).params,
            7,
            "the rule crossfaded from kept its state"
        );
        store.slot_mut(&owner, Slot::Replace, second).params = 9;
        store.arrange(&owner, Slot::Replace, first, second);
        assert_eq!(store.slot_mut(&owner, Slot::Replace, first).params, 7);
        assert_eq!(
            store.slot_mut(&owner, Slot::LeavingReplace, second).params,
            9
        );
        assert_eq!(store.len(), 2);
        store.arrange(&owner, Slot::Replace, third, second);
        assert_eq!(store.len(), 1, "the first rule's state is given back");
        assert_eq!(
            store.slot_mut(&owner, Slot::LeavingReplace, second).params,
            9
        );
        store.sweep(0, &mut pool);
    }

    /// The passes count up, so a slot marked in one is older than the next.
    #[test]
    fn each_pass_is_later_than_the_last() {
        let mut store = Store::default();
        let first = store.next_pass();
        assert!(store.next_pass() > first);
    }

    fn slot() -> super::SlotState {
        super::SlotState::fresh(rule())
    }

    /// **A self chain runs once until its part commits.**
    #[test]
    fn a_self_chain_runs_once_until_its_part_commits() {
        let mut slot = slot();
        assert!(slot.needs_run(false, 7, (100, 80)), "never run");
        slot.ran(7, (100, 80));
        assert!(!slot.needs_run(false, 7, (100, 80)), "nothing changed");
        assert!(
            slot.needs_run(true, 7, (100, 80)),
            "the part was recaptured"
        );
    }

    /// **A param change re-runs the chain without a recapture**, and a size
    /// change re-runs it too.
    #[test]
    fn a_param_or_size_change_reruns_the_chain() {
        let mut slot = slot();
        slot.ran(7, (100, 80));
        assert!(slot.needs_run(false, 8, (100, 80)));
        assert!(slot.needs_run(false, 7, (101, 80)));
    }

    /// **A generated effect runs once, and not again until its size or its
    /// params change**: a T0 slot is asked with `redrawn = false`, whatever
    /// commits under it.
    #[test]
    fn a_generated_effect_runs_once_and_not_again_until_its_size_or_params_change() {
        let mut slot = slot();
        assert!(slot.needs_run(false, 4, (548, 388)));
        slot.ran(4, (548, 388));
        assert!(
            !slot.needs_run(false, 4, (548, 388)),
            "re-ran with nothing of its own changed"
        );
        assert!(
            slot.needs_run(false, 6, (548, 388)),
            "new params did not re-run it"
        );
        assert!(
            slot.needs_run(false, 4, (560, 388)),
            "a new size did not re-run it"
        );
    }

    /// **An unchanged chain keeps its output's id and commit.**
    #[test]
    fn an_unchanged_chain_keeps_its_outputs_id_and_commit() {
        let mut slot = slot();
        let id = slot.id.clone();
        let at = crate::effect::element::Placement::of(
            smithay::utils::Rectangle::new((0, 0).into(), (100, 80).into()),
            None,
            1.0,
        );
        let first = slot.commit_for(at, true);
        assert_eq!(slot.commit_for(at, false), first);
        assert_eq!(slot.id, id);
    }

    /// **With `SOLIUM_RECAPTURE=always` the chain runs every pass**: a
    /// redrawn input re-runs it each time, and [fx0] Task 17's
    /// `recapture_always` makes every capture stale, so every pass is one
    /// whose input was redrawn (`recapture_always_is_asked_for_by_name`).
    #[test]
    fn a_redrawn_input_reruns_the_chain_every_pass() {
        let mut slot = slot();
        for _ in 0..3 {
            assert!(slot.needs_run(true, 7, (100, 80)));
            slot.ran(7, (100, 80));
        }
    }

    /// **A T2 suffix re-runs only when its key moves**, per monitor; never
    /// run, it runs.
    #[test]
    fn a_t2_suffix_reruns_only_when_its_key_moves() {
        let mut state = slot();
        let key = crate::effect::memo::T2Key {
            memo: 1,
            xray: 1,
            padded: smithay::utils::Rectangle::new((0, 0).into(), (10, 10).into()),
            params: 0,
            shape: 0,
            own: None,
        };
        assert!(state.t2_mut("DP-1").needs_run(key));
        state.t2_mut("DP-1").key = Some(key);
        assert!(!state.t2_mut("DP-1").needs_run(key));
        assert!(
            state
                .t2_mut("DP-1")
                .needs_run(crate::effect::memo::T2Key { memo: 2, ..key })
        );
        assert!(
            state
                .t2_mut("DP-1")
                .needs_run(crate::effect::memo::T2Key { xray: 2, ..key }),
            "a redrawn xray with no memo read"
        );
        assert!(
            state.t2_mut("DP-2").needs_run(key),
            "another monitor's result is its own"
        );
    }

    /// **A resized part's T0 chain runs at most `motion_hz` times a
    /// second**: a part a pixel wider on every 4 ms pass for a second runs
    /// about 60 times at 60, not 250, its last result drawn stretched
    /// between; at its last size within a sixtieth of a second of stopping;
    /// on every pass at 0; and at once whatever the cap when its params
    /// change or its input was redrawn.
    #[test]
    fn a_resized_parts_t0_chain_runs_at_most_motion_hz_times_a_second() {
        use super::{Motion, SlotState};
        use std::time::Duration;
        // One pass at `ms` over a box `width` wide: the chain runs when it
        // needs to and does not wait. Whether it ran.
        let pass = |slot: &mut SlotState, width: u32, params: u64, ms: u64, motion: Motion| {
            let now = Duration::from_millis(ms);
            let size = (width, 300);
            let runs = slot.needs_run(false, params, size)
                && !slot.waits(false, params, size, now, motion);
            if runs {
                slot.ran(params, size);
                slot.ran_at = Some(now);
            }
            u32::from(runs)
        };
        let resize = |motion: Motion| {
            let mut state = slot();
            let runs: u32 = (0..250_u64)
                .map(|step| {
                    let width = 400 + u32::try_from(step).unwrap_or(0);
                    pass(&mut state, width, 7, 10_000 + step * 4, motion)
                })
                .sum();
            (state, runs)
        };
        let sixty = Motion {
            hz: 60.0,
            phase: 0.0,
        };
        let (mut state, runs) = resize(sixty);
        assert!((58..=62).contains(&runs), "ran {runs} times at 60");
        let after: Vec<u32> = (0..10_u64)
            .map(|step| pass(&mut state, 649, 7, 11_000 + step * 4, sixty))
            .collect();
        assert_eq!(after.iter().sum::<u32>(), 1, "{after:?}");
        assert!(after.iter().take(5).any(|ran| *ran == 1), "{after:?}");
        let (_, every) = resize(Motion {
            hz: 0.0,
            phase: 0.0,
        });
        assert_eq!(every, 250, "at 0");
        // New params, mid-tick, run at once; so does a redrawn input.
        assert_eq!(pass(&mut state, 649, 8, 11_100, sixty), 1, "new params");
        let now = Duration::from_millis(11_101);
        assert!(
            !state.waits(true, 8, (650, 300), now, sixty),
            "a redrawn input"
        );
    }

    /// **A moving part's suffix runs at most `motion_hz` times a second**:
    /// a part gliding a pixel on every 4 ms pass for a second runs about 60
    /// times at 60, not 250, its last result drawn where it is between; at
    /// its last place within a sixtieth of a second of stopping; on every
    /// pass at 0; and on every pass whatever the cap when what moved its key
    /// is not its place (the backdrop redrawn on each pass). Two places a
    /// phase apart run on different passes.
    #[test]
    fn a_moving_parts_suffix_runs_at_most_motion_hz_times_a_second() {
        use super::{Motion, T2State};
        use crate::effect::memo::T2Key;
        use smithay::utils::Rectangle;
        use std::time::Duration;
        let at = |x: i32| T2Key {
            memo: 1,
            xray: 1,
            padded: Rectangle::new((x, 0).into(), (300, 200).into()),
            params: 0,
            shape: 0,
            own: None,
        };
        // One pass at `ms`: the suffix runs when its key moved and it does not
        // wait. Whether it ran.
        let pass = |t2: &mut T2State, key: T2Key, ms: u64, motion: Motion| {
            let now = Duration::from_millis(ms);
            let runs = t2.needs_run(key) && !t2.waits(key, now, motion);
            if runs {
                t2.key = Some(key);
                t2.ran_at = Some(now);
            }
            u32::from(runs)
        };
        let glide = |motion: Motion, moved: &dyn Fn(u64) -> T2Key| {
            let mut t2 = T2State::default();
            let runs: u32 = (0..250_u64)
                .map(|step| pass(&mut t2, moved(step), 10_000 + step * 4, motion))
                .sum();
            (t2, runs)
        };
        let sixty = Motion {
            hz: 60.0,
            phase: 0.0,
        };
        let gliding = |step: u64| at(i32::try_from(step).unwrap_or(0));
        let (mut t2, runs) = glide(sixty, &gliding);
        assert!((58..=62).contains(&runs), "ran {runs} times at 60");
        // Stopped at x = 249: run there within 17 ms, and not again.
        let after: Vec<u32> = (0..10_u64)
            .map(|step| pass(&mut t2, at(249), 11_000 + step * 4, sixty))
            .collect();
        assert_eq!(after.iter().sum::<u32>(), 1, "{after:?}");
        assert!(after.iter().take(5).any(|ran| *ran == 1), "{after:?}");
        assert_eq!(t2.key, Some(at(249)));
        let (_, every) = glide(
            Motion {
                hz: 0.0,
                phase: 0.0,
            },
            &gliding,
        );
        assert_eq!(every, 250, "at 0");
        let redrawn = |step: u64| T2Key {
            xray: step,
            ..at(i32::try_from(step).unwrap_or(0))
        };
        let (_, backdrop) = glide(sixty, &redrawn);
        assert_eq!(backdrop, 250, "a backdrop redrawn on every pass");
        // Two places a half-tick apart: their runs fall on different passes.
        let ran_on = |phase: f64| {
            let mut t2 = T2State::default();
            (0..250_u64)
                .filter(|step| {
                    pass(
                        &mut t2,
                        gliding(*step),
                        10_000 + step * 4,
                        Motion { hz: 60.0, phase },
                    ) == 1
                })
                .collect::<Vec<u64>>()
        };
        let (one, other) = (ran_on(0.0), ran_on(0.5));
        let shared = one.iter().filter(|step| other.contains(step)).count();
        assert!(shared <= 2, "{shared} passes ran both: {one:?} {other:?}");
    }

    /// **A T2 result's commit moves only when it re-ran or is placed
    /// elsewhere**, a new crop of the same memo included, and its place
    /// read its memos last pass only when it read them in the pass before.
    #[test]
    fn a_t2_results_commit_moves_only_when_rerun_or_placed_elsewhere() {
        use crate::effect::element::Placement;
        use smithay::utils::Rectangle;
        let mut state = slot();
        let t2 = state.t2_mut("DP-1");
        let at = Placement::of(Rectangle::new((0, 0).into(), (10, 10).into()), None, 1.0);
        let crop = Rectangle::new((5.0, 5.0).into(), (10.0, 10.0).into());
        let first = t2.commit_for(at.cropped(crop), false);
        assert_eq!(
            t2.commit_for(at.cropped(crop), false),
            first,
            "placed again"
        );
        let moved = t2.commit_for(
            at.cropped(Rectangle::new((6.0, 5.0).into(), (10.0, 10.0).into())),
            false,
        );
        assert_ne!(moved, first, "a crop moved kept its commit");
        assert_ne!(t2.commit_for(at.cropped(crop), true), moved, "a re-run");
        assert!(!t2.read_last_pass(5));
        t2.read_at = Some(4);
        assert!(t2.read_last_pass(5));
        assert!(!t2.read_last_pass(6), "read two passes ago");
    }

    /// **A T2 place that turns to a crop gives its targets back**: one that
    /// ran its whole plan over `effects.memo.budget_mib` and is admitted
    /// now holds nothing part-sized while it crops the memo (Task 44's
    /// review); its commit moves only when the memo it crops moved.
    #[test]
    fn a_t2_place_that_turns_to_a_crop_gives_its_targets_back() {
        let mut state = slot();
        let t2 = state.t2_mut("DP-1");
        t2.held = super::super::run::Held::made_for_test();
        let mut pool = crate::pool::Pool::new(0);
        let key = crate::effect::memo::T2Key {
            memo: 1,
            xray: 1,
            padded: smithay::utils::Rectangle::new((0, 0).into(), (10, 10).into()),
            params: 0,
            shape: 0,
            own: None,
        };
        assert!(t2.cropped(key, &mut pool), "its first crop");
        assert!(
            t2.held.is_empty_for_test(),
            "a crop kept the targets of a whole-plan run"
        );
        let moved = crate::effect::memo::T2Key {
            padded: smithay::utils::Rectangle::new((4, 0).into(), (10, 10).into()),
            ..key
        };
        assert!(
            !t2.cropped(moved, &mut pool),
            "the same memo, cropped elsewhere"
        );
        assert!(
            t2.cropped(crate::effect::memo::T2Key { memo: 2, ..moved }, &mut pool),
            "the memo re-ran"
        );
    }

    /// **A T2 result on a monitor the slot left is given back**; the one
    /// on the monitor it is on is kept, key and all.
    #[test]
    fn a_t2_result_on_a_monitor_left_is_given_back() {
        let mut state = slot();
        let key = crate::effect::memo::T2Key {
            memo: 1,
            xray: 1,
            padded: smithay::utils::Rectangle::new((0, 0).into(), (10, 10).into()),
            params: 0,
            shape: 0,
            own: None,
        };
        state.t2_mut("DP-1").key = Some(key);
        state.t2_mut("DP-2").key = Some(key);
        state.keep_t2(&["DP-2"], &mut crate::pool::Pool::new(0));
        assert_eq!(state.t2.keys().collect::<Vec<_>>(), ["DP-2"]);
        assert_eq!(
            state.t2_mut("DP-2").key,
            Some(key),
            "the kept one kept its key"
        );
    }

    /// **Drawn tells an input redrawn this pass from one kept**, and gives
    /// its capture's commit as a state's key reads it.
    #[test]
    fn drawn_tells_an_input_redrawn_this_pass_from_one_kept() {
        use smithay::backend::renderer::utils::CommitCounter;
        let mut drawn = Drawn::<u32>::default();
        let mut commit = CommitCounter::default();
        commit.increment();
        drawn.insert(client(1), Slot::Behind, 4, true, commit);
        drawn.insert(client(2), Slot::Behind, 5, false, CommitCounter::default());
        assert_eq!(drawn.get(&client(1), Slot::Behind), Some((&4, true)));
        assert_eq!(drawn.get(&client(2), Slot::Behind), Some((&5, false)));
        assert_eq!(drawn.get(&client(1), Slot::Front), None);
        assert_eq!(drawn.commit(&client(1), Slot::Behind), Some(1));
        assert_eq!(drawn.commit(&client(2), Slot::Behind), Some(0));
    }

    /// **A beneath draw's commit moves with its warp or its run**, and not
    /// otherwise, so a held tilt is not damaged pass after pass (amended
    /// 2026-10-14, live under motion and warps).
    #[test]
    fn an_unders_commit_moves_with_its_warp_or_its_rerun() {
        use smithay::backend::renderer::utils::CommitCounter;
        let mut t2 = super::T2State::default();
        let still = CommitCounter::default();
        let mut moved = CommitCounter::default();
        moved.increment();
        let first = t2.under_commit(still, false);
        assert_eq!(
            t2.under_commit(still, false),
            first,
            "a held warp damaged its beneath"
        );
        let rerun = t2.under_commit(still, true);
        assert_ne!(rerun, first, "a re-run left its beneath undamaged");
        assert_ne!(
            t2.under_commit(moved, false),
            rerun,
            "the warp moved and its beneath did not"
        );
    }
}
