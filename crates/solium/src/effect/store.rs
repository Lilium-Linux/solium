//! What a slot keeps from one pass to the next: its part's capture, its
//! chain's targets, the id and commit its result is drawn with, and the
//! params it last ran at (Ruling 16). Kept on `Solium`, swept once a pass, so
//! a slot no rule wants any more gives its targets back.
//! `tests::a_slot_no_rule_wanted_this_pass_is_dropped`.

use std::collections::HashMap;

use smithay::backend::renderer::{element::Id, gles::GlesTexture, utils::CommitCounter};

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
        }
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

    /// Give the capture's target and the chain's back.
    /// `tests::a_slot_no_rule_wanted_this_pass_is_dropped`.
    fn release(mut self, pool: &mut crate::pool::Pool) {
        self.input.release(pool);
        self.held.release(pool);
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

    /// A slot's state as it is, made by [`Self::slot_mut`] this pass.
    /// `state::tests::real_client::a_self_rule_captures_the_client_once_until_it_commits`.
    pub(crate) fn get_mut(&mut self, owner: &Owner, slot: Slot) -> Option<&mut SlotState> {
        self.slots.get_mut(&(owner.clone(), slot))
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

/// Each self input this pass: its texture, whether it was drawn this pass or
/// kept from an earlier one, and its capture's commit. What the runs read
/// (`render::run_slots`): a chain re-runs only when its input was redrawn.
/// `tests::drawn_tells_an_input_redrawn_this_pass_from_one_kept`.
#[derive(Debug)]
pub(crate) struct Drawn<T = GlesTexture> {
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
}
