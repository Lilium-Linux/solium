//! What a frame draws for effects: the rules' chains, bound at config load,
//! and the slots resolved once a pass ([`Slots`]).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use smithay::utils::{Physical, Rectangle, Size};
use solium_effects::stage::Plan;

use super::element::EffectElement;
use super::host::{Host, Problem};
use super::rules::{Fill, Origin, Part, Rule, RuleKey, Slot, Tier, runnable, tier};
use crate::pane::PaneId;

/// One rule's chain, bound and checked.
/// `state::tests::a_broken_rule_keeps_the_rules_that_ran`.
#[derive(Debug)]
pub(crate) struct BoundChain {
    pub(crate) plan: Plan,
    pub(crate) reach: f64,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "no frame reads it yet: the cull grows a pane by its results' reach"
        )
    )]
    pub(crate) bleed: f64,
    pub(crate) tier: Tier,
    /// A hash of every link's name and bound params: a chain whose params
    /// changed runs again (`store::SlotState::needs_run`).
    /// `tests::a_chain_reaches_as_its_first_link_and_bleeds_as_all_of_them`.
    pub(crate) params_hash: u64,
}

/// Every rule's bound chain, by its key.
/// `state::tests::a_replaced_rule_set_holds_only_its_own_programs`.
#[derive(Debug, Default)]
pub(crate) struct Chains {
    by_key: HashMap<RuleKey, BoundChain>,
    /// The rules whose chain failed on the GPU, each said once.
    /// `tests::a_failed_chain_is_said_once_per_rule`.
    refused: HashSet<RuleKey>,
}

impl Chains {
    /// Bind every link, chain them, and check the tier. GPU-free. A link's
    /// failure is its effect's own problem, file and line kept, so the
    /// overlay names the `.frag`'s line and not the configuration
    /// (`state::tests::a_rule_whose_effect_is_broken_is_named_at_the_frags_line_until_mended`);
    /// a tier this build cannot run is refused by name
    /// (`state::tests::a_blur_rule_without_source_is_refused_until_xray`).
    pub(crate) fn bind(host: &mut Host, rule: &Rule) -> Result<BoundChain, Problem> {
        let plain =
            |message: String| Problem::error("rules", Path::new("effects.rules"), None, message);
        let Fill::Chain(links) = &rule.fill else {
            return Err(plain("an empty slot has no chain".to_owned()));
        };
        let mut plans = Vec::with_capacity(links.len());
        let (mut reach, mut bleed) = (0.0_f64, 0.0_f64);
        let mut hashed: Vec<u8> = Vec::new();
        for (index, link) in links.iter().enumerate() {
            let bound = host.bind(&link.effect, &link.params)?;
            // A link's own `reach` and `bleed` win over its file's (Ruling 5):
            // `rules::tests::a_links_reach_and_bleed_override_the_files`,
            // `tests::a_chain_reaches_as_its_first_link_and_bleeds_as_all_of_them`.
            if index == 0 {
                reach = link.extents.reach.unwrap_or(bound.reach);
            }
            bleed += link.extents.bleed.unwrap_or(bound.bleed);
            hashed.extend(format!("{}{:?}", link.effect, bound.params).bytes());
            // The configured plan: the ladder that steps along the fallback
            // rungs is X2.7's, so FX2 runs a chain's configured plans only.
            plans.push(
                bound.plans.into_iter().next().ok_or_else(|| {
                    plain(format!("`{}` has no plan this GPU can run", link.effect))
                })?,
            );
        }
        let plan = solium_effects::stage::chain(plans);
        let tier = tier(plan.reads, rule.source);
        // A region's own pixels are a crop of the layer that draws it, which
        // waits for P15's published regions.
        // `tests::a_region_self_rule_is_refused_until_p15`.
        if matches!(rule.part, Part::Region(_)) && tier == Tier::Own {
            return Err(plain(
                "a region's own pixels arrive with P15's Solium.region".to_owned(),
            ));
        }
        runnable(tier).map_err(|why| {
            plain(format!(
                "`{}` {why}",
                links.first().map_or("", |link| link.effect.as_str())
            ))
        })?;
        Ok(BoundChain {
            plan,
            reach,
            bleed,
            tier,
            params_hash: solium_effects::glsl::content_hash(&[&hashed]),
        })
    }

    pub(crate) fn insert(&mut self, key: RuleKey, chain: BoundChain) {
        self.by_key.insert(key, chain);
    }

    /// Drop one rule's chain: a style's rule that no longer binds leaves its
    /// slot empty. `state::tests::the_formats_probe_rebinds_the_styles_rules_too`.
    pub(crate) fn remove(&mut self, key: RuleKey) {
        self.by_key.remove(&key);
        self.refused.remove(&key);
    }

    /// Say `why` a rule's chain is not drawn, once per rule, as
    /// `pass::Programs::refuse` latches: a chain that failed on the GPU is
    /// latched failed in its slot's `Held`, and would otherwise be said at
    /// the refresh rate. Whether it was said now.
    /// `tests::a_failed_chain_is_said_once_per_rule`.
    pub(crate) fn refuse_once(&mut self, key: RuleKey, why: &str) -> bool {
        if !self.refused.insert(key) {
            return false;
        }
        tracing::warn!(?key, "{why}");
        true
    }

    /// A slot's chain: a resolved key with none bound wants no slot
    /// (Ruling 15). `state::tests::real_client::a_rule_whose_chain_is_not_bound_wants_no_slot`.
    pub(crate) fn get(&self, key: RuleKey) -> Option<&BoundChain> {
        self.by_key.get(&key)
    }

    /// Drop an origin's chains of an older generation (a set replaced whole).
    /// `state::tests::a_replaced_rule_set_holds_only_its_own_programs`.
    pub(crate) fn retain_generation(&mut self, origin: Origin, generation: u32) {
        self.by_key
            .retain(|key, _| key.origin != origin || key.generation == generation);
        self.refused
            .retain(|key| key.origin != origin || key.generation == generation);
    }

    /// The program of every step every chain runs, its states' included: what
    /// the host holds for the rules, so a set replaced gives its programs
    /// back. `state::tests::a_replaced_rule_set_holds_only_its_own_programs`.
    pub(crate) fn programs(&self) -> Vec<u64> {
        let mut keys: Vec<u64> = self
            .by_key
            .values()
            .flat_map(|chain| {
                chain.plan.steps.iter().chain(
                    chain
                        .plan
                        .states
                        .iter()
                        .flat_map(|state| state.steps.iter()),
                )
            })
            .map(|step| step.key)
            .collect();
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    /// The newest bound plan of `origin`'s, for a test to read.
    #[cfg(test)]
    pub(crate) fn first_plan_for_test(&self, origin: Origin) -> Option<&Plan> {
        self.by_key
            .iter()
            .filter(|(key, _)| key.origin == origin)
            .max_by_key(|(key, _)| (key.generation, std::cmp::Reverse(key.index)))
            .map(|(_, chain)| &chain.plan)
    }
}

/// A pane's parts that have slots.
/// `state::tests::real_client::a_rule_on_focused_follows_the_keyboard`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PaneSlot {
    Pane,
    Client,
    Titlebar,
    /// The style's layer at this index (`LayerSpec.index`).
    /// `decoration::tests::every_layer_is_named_with_its_place_among_the_styles_layers`.
    Layer(usize),
    Popups,
}

/// Who a slot belongs to.
/// `state::tests::real_client::a_surface_has_a_slot_per_monitor_and_a_layer_surface_one_of_its_own`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Owner {
    Pane(PaneId, PaneSlot),
    /// A scripted surface's instance on the output of this name.
    Surface(crate::scripted::SurfaceId, String),
    LayerShell(smithay::reexports::wayland_server::backend::ObjectId),
}

/// A slot's result, ready to place: the element, and the padded box it covers
/// in the part's own physical pixels. Filled by `render::run_slots`; with no
/// GPU a test marks a slot ready with none
/// (`tests::a_slot_is_wanted_by_its_owner_and_slot_and_ready_apart`).
#[derive(Clone, Debug)]
pub(crate) struct Ready {
    pub(crate) element: EffectElement,
    pub(crate) padded: Rectangle<i32, Physical>,
    pub(crate) reach: i32,
}

/// A slot's padded box in its part's own physical pixels: the box at
/// `(0, 0)`, the part inside it in `uv` (x, y, w, h), the part's mask radii
/// (top-left, top-right, bottom-left, bottom-right) and the reach that padded
/// it. What a run reads, whether or not a capture was drawn this pass
/// (Ruling 16). `tests::a_part_box_is_its_part_padded_by_its_reach_on_every_side`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PartBox {
    pub(crate) padded: Rectangle<i32, Physical>,
    pub(crate) content: [f32; 4],
    pub(crate) radii: [f32; 4],
    pub(crate) reach: i32,
}

impl PartBox {
    /// A part of `own` pixels padded by `pad` on every side.
    /// `tests::a_part_box_is_its_part_padded_by_its_reach_on_every_side`.
    pub(crate) fn around(own: Size<i32, Physical>, pad: i32, radii: [f32; 4]) -> Self {
        let padded: Size<i32, Physical> = (own.w + 2 * pad, own.h + 2 * pad).into();
        Self {
            padded: Rectangle::from_size(padded),
            content: [
                uv(pad, padded.w),
                uv(pad, padded.h),
                uv(own.w, padded.w),
                uv(own.h, padded.h),
            ],
            radii,
            reach: pad,
        }
    }

    /// The padded box's size, as a run takes it.
    /// `tests::a_part_box_is_its_part_padded_by_its_reach_on_every_side`.
    pub(crate) fn size(&self) -> (u32, u32) {
        (
            u32::try_from(self.padded.size.w).unwrap_or(0),
            u32::try_from(self.padded.size.h).unwrap_or(0),
        )
    }
}

/// `part` pixels of `whole`, in `uv`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a fraction of a texture side, given to GL as a float"
)]
fn uv(part: i32, whole: i32) -> f32 {
    (f64::from(part) / f64::from(whole.max(1))) as f32
}

/// What effects this pass draws where: what the rules want, and what is
/// ready. Built once a pass by `render::build_slots`, so every output and
/// every screencopy places from the same answer.
/// `tests::a_slot_is_wanted_by_its_owner_and_slot_and_ready_apart`.
#[derive(Debug, Default)]
pub(crate) struct Slots {
    wants: HashMap<(Owner, Slot), RuleKey>,
    boxes: HashMap<(Owner, Slot), PartBox>,
    ready: HashMap<(Owner, Slot), Option<Ready>>,
    reach: HashMap<PaneId, i32>,
    facts: u32,
}

impl Slots {
    /// Nothing wanted.
    /// `state::tests::real_client::with_no_rules_no_slot_is_wanted_and_no_fact_is_gathered`.
    pub(crate) fn is_empty(&self) -> bool {
        self.wants.is_empty()
    }

    /// `tests::a_slot_is_wanted_by_its_owner_and_slot_and_ready_apart`.
    pub(crate) fn want(&mut self, owner: Owner, slot: Slot, key: RuleKey) {
        self.wants.insert((owner, slot), key);
    }

    /// `tests::a_slot_is_wanted_by_its_owner_and_slot_and_ready_apart`.
    pub(crate) fn wants(&self) -> impl Iterator<Item = (&Owner, Slot, RuleKey)> {
        self.wants
            .iter()
            .map(|((owner, slot), key)| (owner, *slot, *key))
    }

    /// A wanted slot's padded box (`render::record_boxes`).
    /// `state::tests::real_client::every_wanted_slot_has_its_part_box_padded_by_its_reach`.
    pub(crate) fn set_box(&mut self, owner: Owner, slot: Slot, part: PartBox) {
        self.boxes.insert((owner, slot), part);
    }

    /// `state::tests::real_client::every_wanted_slot_has_its_part_box_padded_by_its_reach`.
    pub(crate) fn boxed(&self, owner: &Owner, slot: Slot) -> Option<PartBox> {
        self.boxes.get(&(owner.clone(), slot)).copied()
    }

    /// `tests::a_slot_is_wanted_by_its_owner_and_slot_and_ready_apart`.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "read by tests: the walk asks `is_ready`")
    )]
    pub(crate) fn wanted(&self, owner: &Owner, slot: Slot) -> bool {
        self.wants.contains_key(&(owner.clone(), slot))
    }

    /// The rule a wanted slot resolved to, whose mask and chain say how its
    /// result is cut (`render::cut_by_shape`).
    /// `tests::a_slot_is_wanted_by_its_owner_and_slot_and_ready_apart`.
    pub(crate) fn key(&self, owner: &Owner, slot: Slot) -> Option<RuleKey> {
        self.wants.get(&(owner.clone(), slot)).copied()
    }

    /// A slot's result, its pane reaching as far as it does
    /// (`tests::a_pane_reaches_as_far_as_its_furthest_ready_slot`).
    pub(crate) fn set_ready(&mut self, owner: Owner, slot: Slot, ready: Ready) {
        self.reached(&owner, ready.reach);
        self.ready.insert((owner, slot), Some(ready));
    }

    /// A pane reaches as far as the furthest of its slots' results; a
    /// surface's reach no pane.
    /// `tests::a_pane_reaches_as_far_as_its_furthest_ready_slot`.
    fn reached(&mut self, owner: &Owner, reach: i32) {
        if let Owner::Pane(pane, _) = owner {
            let furthest = self.reach.entry(*pane).or_insert(0);
            *furthest = (*furthest).max(reach);
        }
    }

    /// `tests::a_slot_is_wanted_by_its_owner_and_slot_and_ready_apart`.
    pub(crate) fn at(&self, owner: &Owner, slot: Slot) -> Option<&Ready> {
        self.ready
            .get(&(owner.clone(), slot))
            .and_then(Option::as_ref)
    }

    /// Whether the slot has a result to draw: what the walk asks.
    /// `tests::a_slot_is_wanted_by_its_owner_and_slot_and_ready_apart`,
    /// `render::tests::a_wanted_slot_with_nothing_ready_draws_what_no_slot_draws`.
    pub(crate) fn is_ready(&self, owner: &Owner, slot: Slot) -> bool {
        self.ready.contains_key(&(owner.clone(), slot))
    }

    /// A slot made ready with no texture, so the walk's order is testable
    /// with no GPU (Tasks 19 and 25).
    /// `tests::a_slot_is_wanted_by_its_owner_and_slot_and_ready_apart`.
    #[cfg(test)]
    pub(crate) fn mark_ready(&mut self, owner: Owner, slot: Slot) {
        self.ready.insert((owner, slot), None);
    }

    /// `tests::a_pane_reaches_as_far_as_its_furthest_ready_slot`,
    /// `render::tests::the_bleed_cull_counts_an_effects_reach`.
    pub(crate) fn reach(&self, pane: PaneId) -> i32 {
        self.reach.get(&pane).copied().unwrap_or(0)
    }

    /// One owner's facts gathered and its rules resolved.
    /// `state::tests::real_client::with_no_rules_no_slot_is_wanted_and_no_fact_is_gathered`.
    pub(crate) fn gathered(&mut self) {
        self.facts += 1;
    }

    /// How many owners' facts this pass gathered: 0 with no rules (spec §8.4).
    /// `state::tests::real_client::with_no_rules_no_slot_is_wanted_and_no_fact_is_gathered`.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "read by the §8.4 guard's tests only")
    )]
    pub(crate) fn facts_gathered(&self) -> u32 {
        self.facts
    }
}

/// A rule's problem as the overlay lists it: under `"rules"`, numbered as
/// Lua counts, at its effect's own file and line when it has one and at the
/// configuration otherwise.
/// `state::tests::a_rule_whose_effect_is_broken_is_named_at_the_frags_line_until_mended`,
/// `check::tests::a_rule_reading_xray_fails_the_check`.
pub(crate) fn rule_problem(number: usize, problem: Problem, config: &Path) -> Problem {
    Problem {
        effect: "rules".to_owned(),
        message: format!("effects.rules, rule {number}: {}", problem.message),
        file: if problem.line.is_some() {
            problem.file
        } else {
            config.to_path_buf()
        },
        ..problem
    }
}

#[cfg(test)]
mod tests {
    use super::Chains;
    use crate::effect::host::tests::{folder, scratch};

    /// Rules from Lua source, read the way `sol.effects` reads them.
    fn rules(lua: &str) -> Vec<crate::effect::rules::Rule> {
        let state = mlua::Lua::new();
        let value: mlua::Value = state.load(lua).eval().expect("the test's Lua");
        let tree = crate::effect::tree::Tree::from_lua(&value)
            .expect("readable")
            .expect("a value");
        crate::effect::rules::parse(&tree).expect("parses")
    }

    /// **A chain reaches as far as its first link and bleeds as far as all
    /// of them**, each link's own `reach` and `bleed` winning over its
    /// file's (Ruling 5); its tier is what the chained plan reads, and its
    /// params hash changes with its params.
    #[test]
    fn a_chain_reaches_as_its_first_link_and_bleeds_as_all_of_them() {
        let place = scratch("plan-extents");
        folder(
            &place,
            "wide",
            "return { api = 1, inputs = { 'self' }, params = { amount = { 1 } }, reach = 6, bleed = 2, frag = 'effect.frag' }",
            &[(
                "effect.frag",
                "vec4 sol_effect(vec2 uv) { return sol_tex(uv) * p_amount; }\n",
            )],
        );
        folder(
            &place,
            "ring",
            "return { api = 1, inputs = { 'shape' }, bleed = 5, frag = 'effect.frag' }",
            &[(
                "effect.frag",
                "vec4 sol_effect(vec2 uv) { return vec4(sol_shape(uv)); }\n",
            )],
        );
        let mut host = crate::effect::host::Host::new(crate::effect::host::Library::with(
            Some(place.clone()),
            place.join("none"),
        ));
        host.want("rules", ["wide".to_owned(), "ring".to_owned()]);
        let bound = |host: &mut crate::effect::host::Host, lua: &str| {
            Chains::bind(host, &rules(lua)[0]).expect("binds")
        };
        let chain = bound(
            &mut host,
            r#"{ { match = "*", part = "client", slot = "behind", effect = { { "wide" }, { "ring", bleed = 1 } } } }"#,
        );
        assert_eq!((chain.reach, chain.bleed), (6.0, 3.0));
        assert_eq!(chain.tier, crate::effect::rules::Tier::Own);
        let chain = bound(
            &mut host,
            r#"{ { match = "*", part = "client", slot = "behind", effect = { "ring", reach = 4 } } }"#,
        );
        assert_eq!((chain.reach, chain.bleed), (4.0, 5.0));
        assert_eq!(chain.tier, crate::effect::rules::Tier::Generated);
        // And a chain's params are its key's: a chain whose params changed
        // runs again (`store::tests::a_param_or_size_change_reruns_the_chain`).
        let hash = |host: &mut crate::effect::host::Host, amount: &str| {
            bound(
                host,
                &format!(
                    r#"{{ {{ match = "*", part = "client", slot = "behind", effect = {{ "wide", amount = {amount} }} }} }}"#
                ),
            )
            .params_hash
        };
        assert_eq!(hash(&mut host, "2"), hash(&mut host, "2"));
        assert_ne!(hash(&mut host, "2"), hash(&mut host, "3"));
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A failed chain is said once per rule**: again for the same rule is
    /// silent, another rule is said, and a rule set replaced (a reload) says
    /// its own failures afresh.
    #[test]
    fn a_failed_chain_is_said_once_per_rule() {
        use crate::effect::rules::{Origin, RuleKey};
        let mut chains = Chains::default();
        let key = |index, generation| RuleKey {
            origin: Origin::User,
            index,
            generation,
        };
        assert!(chains.refuse_once(key(0, 1), "failed"));
        assert!(!chains.refuse_once(key(0, 1), "failed"), "said twice");
        assert!(chains.refuse_once(key(1, 1), "failed"), "another rule");
        chains.retain_generation(Origin::User, 2);
        assert!(chains.refuse_once(key(0, 2), "failed"), "a new rule set");
        assert_eq!(chains.refused.len(), 1, "the old set's latches stayed");
    }

    /// **A region's own pixels wait for P15**: a titlebar rule whose chain
    /// reads `self` is refused at bind, naming it; one reading nothing of the
    /// frame binds.
    #[test]
    fn a_region_self_rule_is_refused_until_p15() {
        let place = scratch("plan-region-self");
        folder(
            &place,
            "tint",
            "return { api = 1, inputs = { 'self' }, frag = 'effect.frag' }",
            &[(
                "effect.frag",
                "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n",
            )],
        );
        folder(
            &place,
            "ring",
            "return { api = 1, inputs = { 'shape' }, frag = 'effect.frag' }",
            &[(
                "effect.frag",
                "vec4 sol_effect(vec2 uv) { return vec4(sol_shape(uv)); }\n",
            )],
        );
        let mut host = crate::effect::host::Host::new(crate::effect::host::Library::with(
            Some(place.clone()),
            place.join("none"),
        ));
        host.want("rules", ["tint".to_owned(), "ring".to_owned()]);
        let refused = Chains::bind(
            &mut host,
            &rules(r#"{ { match = "*", part = "region:titlebar", slot = "behind", effect = "tint" } }"#)[0],
        )
        .expect_err("a region's self rule bound");
        assert!(refused.message.contains("P15"), "{}", refused.message);
        assert!(
            Chains::bind(
                &mut host,
                &rules(r#"{ { match = "*", part = "client", slot = "behind", effect = "tint" } }"#)
                    [0],
            )
            .is_ok(),
            "a client's self rule was refused"
        );
        assert!(
            Chains::bind(
                &mut host,
                &rules(r#"{ { match = "*", part = "region:titlebar", slot = "behind", effect = "ring" } }"#)[0],
            )
            .is_ok(),
            "a titlebar rule reading nothing of the frame was refused"
        );
        let _ = std::fs::remove_dir_all(place);
    }

    /// **A part's box is the part padded by its reach on every side**, the
    /// part inside it in `uv`.
    #[test]
    fn a_part_box_is_its_part_padded_by_its_reach_on_every_side() {
        let part = super::PartBox::around((100, 50).into(), 6, [4.0; 4]);
        assert_eq!(
            part.padded,
            smithay::utils::Rectangle::from_size((112, 62).into())
        );
        assert_eq!(part.size(), (112, 62));
        assert_eq!(
            part.content,
            [6.0 / 112.0, 6.0 / 62.0, 100.0 / 112.0, 50.0 / 62.0]
        );
        assert_eq!((part.radii, part.reach), ([4.0; 4], 6));
        let bare = super::PartBox::around((100, 50).into(), 0, [0.0; 4]);
        assert_eq!(bare.content, [0.0, 0.0, 1.0, 1.0]);
    }

    /// **A slot is wanted by its owner and slot alone, and ready apart from
    /// being wanted**: what the walk asks of a pass's plan (Tasks 19, 25).
    #[test]
    fn a_slot_is_wanted_by_its_owner_and_slot_and_ready_apart() {
        use super::{Owner, PaneSlot, Slots};
        use crate::effect::rules::{Origin, RuleKey, Slot};
        let client = Owner::Pane(crate::pane::PaneId::from_raw(7), PaneSlot::Client);
        let key = RuleKey {
            origin: Origin::User,
            index: 0,
            generation: 1,
        };
        let mut slots = Slots::default();
        assert!(slots.is_empty());
        slots.want(client.clone(), Slot::Behind, key);
        assert!(!slots.is_empty());
        assert!(slots.wanted(&client, Slot::Behind));
        assert!(!slots.wanted(&client, Slot::Front));
        assert_eq!(slots.key(&client, Slot::Behind), Some(key));
        assert_eq!(slots.key(&client, Slot::Front), None);
        assert!(!slots.wanted(
            &Owner::Pane(crate::pane::PaneId::from_raw(7), PaneSlot::Pane),
            Slot::Behind
        ));
        assert_eq!(
            slots.wants().collect::<Vec<_>>(),
            [(&client, Slot::Behind, key)]
        );
        assert!(!slots.is_ready(&client, Slot::Behind));
        slots.mark_ready(client.clone(), Slot::Behind);
        assert!(slots.is_ready(&client, Slot::Behind));
        assert!(
            slots.at(&client, Slot::Behind).is_none(),
            "a slot marked ready in a test has no result to draw"
        );
    }

    /// **A pane reaches as far as the furthest of its ready slots**, and a
    /// surface's slots reach no pane: what the bleed cull grows a pane by
    /// (Task 19).
    #[test]
    fn a_pane_reaches_as_far_as_its_furthest_ready_slot() {
        use super::{Owner, PaneSlot, Slots};
        let (pane, other) = (
            crate::pane::PaneId::from_raw(1),
            crate::pane::PaneId::from_raw(2),
        );
        let mut slots = Slots::default();
        assert_eq!(slots.reach(pane), 0);
        slots.reached(&Owner::Pane(pane, PaneSlot::Client), 12);
        slots.reached(&Owner::Pane(pane, PaneSlot::Titlebar), 4);
        slots.reached(
            &Owner::Surface(crate::scripted::SurfaceId::from_raw(1), "DP-1".to_owned()),
            40,
        );
        assert_eq!((slots.reach(pane), slots.reach(other)), (12, 0));
    }
}
