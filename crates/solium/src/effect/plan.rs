//! What a frame draws for effects: the rules' chains, bound at config load,
//! and (Task 18) the slots resolved once a pass.

use std::collections::HashMap;
use std::path::Path;

use solium_effects::stage::Plan;

use super::host::{Host, Problem};
use super::rules::{Fill, Origin, Rule, RuleKey, Tier, runnable, tier};

/// One rule's chain, bound and checked.
/// `state::tests::a_broken_rule_keeps_the_rules_that_ran`.
#[derive(Debug)]
pub(crate) struct BoundChain {
    pub(crate) plan: Plan,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "Task 20's part captures are padded by it")
    )]
    pub(crate) reach: f64,
    #[cfg_attr(not(test), expect(dead_code, reason = "Task 19's bleed cull reads it"))]
    pub(crate) bleed: f64,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "Task 18's slot plan reads the tier")
    )]
    pub(crate) tier: Tier,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "Task 21 re-runs a chain whose params changed")
    )]
    pub(crate) params_hash: u64,
}

/// Every rule's bound chain, by its key.
/// `state::tests::a_replaced_rule_set_holds_only_its_own_programs`.
#[derive(Debug, Default)]
pub(crate) struct Chains {
    by_key: HashMap<RuleKey, BoundChain>,
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

    #[expect(dead_code, reason = "Task 18's slot plan looks a slot's chain up")]
    pub(crate) fn get(&self, key: RuleKey) -> Option<&BoundChain> {
        self.by_key.get(&key)
    }

    /// Drop an origin's chains of an older generation (a set replaced whole).
    /// `state::tests::a_replaced_rule_set_holds_only_its_own_programs`.
    pub(crate) fn retain_generation(&mut self, origin: Origin, generation: u32) {
        self.by_key
            .retain(|key, _| key.origin != origin || key.generation == generation);
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

/// A rule's problem as the overlay lists it: under `"rules"`, numbered as
/// Lua counts, at its effect's own file and line when it has one and at the
/// configuration otherwise.
/// `state::tests::a_rule_whose_effect_is_broken_is_named_at_the_frags_line_until_mended`.
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
        // And a chain's params are its key's: Task 21 re-runs a chain whose
        // params changed.
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
}
