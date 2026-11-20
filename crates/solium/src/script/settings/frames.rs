//! `frames`: whether a window is framed by the compositor or draws its own,
//! by default and by application.

use crate::framing::{Policy, Rule, Settings};
use crate::script::doc::{Doc, describe};

/// Read the section, or say why it cannot be: half a policy is a frame on
/// the windows nobody meant, so a section with a fault in it is refused
/// whole.
pub(crate) fn parse(options: &Doc) -> Result<Settings, String> {
    let policy = |value: &Doc, at: &str| -> Result<Option<Policy>, String> {
        match value {
            Doc::Nil => Ok(None),
            Doc::Text(word) => Policy::named(word).map(Some).ok_or_else(|| {
                format!("{at} is \"prefer\", \"force\" or \"allow\", not \"{word}\"")
            }),
            other => Err(format!(
                "{at} is \"prefer\", \"force\" or \"allow\", not {}",
                describe(other)
            )),
        }
    };
    let mut settings = Settings::default();
    if let Some(chosen) = policy(options.get("policy"), "frames.policy")? {
        settings.policy = chosen;
    }
    match options.get("rules") {
        Doc::Nil => {}
        rules @ Doc::Table(_) => {
            for (index, rule) in rules.list().iter().enumerate() {
                let at = format!("frames.rules[{}]", index + 1);
                if !matches!(rule, Doc::Table(_)) {
                    return Err(format!(
                        "{at} is a table: {{ match = {{ app_id = ... }}, policy = ... }}"
                    ));
                }
                let app_id = match rule.get("match") {
                    matching @ Doc::Table(_) => match matching.get("app_id") {
                        Doc::Text(app_id) => crate::effect::rules::Glob::parse(app_id),
                        _ => return Err(format!("{at}.match.app_id is a string")),
                    },
                    _ => return Err(format!("{at}.match is {{ app_id = ... }}")),
                };
                let Some(policy) = policy(rule.get("policy"), &format!("{at}.policy"))? else {
                    return Err(format!("{at} has no policy"));
                };
                settings.rules.push(Rule { app_id, policy });
            }
        }
        other => {
            return Err(format!(
                "frames.rules is a list of rules, not {}",
                describe(other)
            ));
        }
    }
    Ok(settings)
}
