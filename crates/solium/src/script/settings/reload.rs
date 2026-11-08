//! `reload`: whether a change to a file the configuration loaded reloads it,
//! and how long the files must be quiet first.

use crate::script::doc::{Doc, describe};

/// Read the section. Each key left out keeps its default, and one of the
/// wrong kind is named in the log and keeps its default too.
pub(crate) fn parse(options: &Doc) -> crate::autoreload::Settings {
    let mut settings = crate::autoreload::Settings::default();
    match options.get("automatic") {
        Doc::Bool(on) => settings.automatic = *on,
        Doc::Nil => {}
        other => tracing::warn!(
            value = describe(other),
            "reload.automatic is true or false; keeping the default, true"
        ),
    }
    match options.get("quiet_ms") {
        Doc::Nil => {}
        value => match value.number().filter(|amount| *amount >= 0.0) {
            Some(amount) => settings.quiet_ms = amount as u64,
            None => tracing::warn!(
                value = describe(value),
                "reload.quiet_ms is a number of milliseconds, zero or more; keeping \
                 the default"
            ),
        },
    }
    settings
}
