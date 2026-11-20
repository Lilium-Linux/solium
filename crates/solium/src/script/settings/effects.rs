//! `effects`: which effects go where, and the engine's own settings.

use crate::effect::rules::{Rule, RuleError};
use crate::script::Doc;

/// Read the section: its `rules`, read plain, and the engine's settings,
/// read with the numbers that are not finite kept so their bounds refuse
/// them. `given` is false for a section left out entirely, which is every
/// default and no rules.
pub(crate) fn parse(
    options: &Doc,
    given: bool,
) -> (
    Result<Vec<Rule>, Vec<RuleError>>,
    Result<crate::effect::settings::Settings, String>,
) {
    let rules = match options.get("rules").plain() {
        Some(tree) => crate::effect::rules::parse(&tree),
        None => Ok(Vec::new()),
    };
    let settings = if given {
        crate::effect::settings::parse(&crate::effect::settings::tree_of(options))
    } else {
        Ok(crate::effect::settings::Settings::default())
    };
    (rules, settings)
}
