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
    #[expect(dead_code, reason = "Task 21 draws a slot's result with its own id")]
    pub(crate) id: Id,
    #[expect(
        dead_code,
        reason = "Task 21 moves a result's commit when its chain re-ran or it moved"
    )]
    pub(crate) commit: CommitCounter,
    #[expect(
        dead_code,
        reason = "Task 21 compares a result's placement with the last one"
    )]
    pub(crate) placed: Option<super::element::Placement>,
    /// The params' hash the chain last ran at.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "Task 21 re-runs a chain whose params changed")
    )]
    pub(crate) params: u64,
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
            seen: 0,
        }
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
/// kept from an earlier one, and its capture's commit. What Task 21's runs
/// read: a chain re-runs only when its input was redrawn.
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
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "Task 21's runs read a slot's self input")
    )]
    pub(crate) fn get(&self, owner: &Owner, slot: Slot) -> Option<(&T, bool)> {
        self.inputs
            .get(&(owner.clone(), slot))
            .map(|(texture, redrawn, _)| (texture, *redrawn))
    }

    /// The input's commit, as the number a state depending on `self` is kept
    /// on (`run::Keys::commit`).
    /// `tests::drawn_tells_an_input_redrawn_this_pass_from_one_kept`.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "Task 21 keys a state on the self input's commit")
    )]
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
