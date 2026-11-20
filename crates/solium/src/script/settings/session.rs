//! `session`: whether systemd and D-Bus are told about this session.

use std::time::Duration;

use crate::script::doc::{Doc, describe};

/// Read the section. Each key left out keeps its default, and one of the
/// wrong kind is named in the log and keeps its default too.
pub(crate) fn parse(options: &Doc) -> crate::session::Settings {
    let mut settings = crate::session::Settings::default();
    for (key, slot) in [
        ("systemd", &mut settings.systemd),
        ("autostart", &mut settings.autostart),
    ] {
        match options.get(key) {
            Doc::Bool(on) => *slot = *on,
            Doc::Nil => {}
            other => tracing::warn!(
                key,
                value = describe(other),
                "session: true or false; keeping the default, true"
            ),
        }
    }
    match options.get("stop_timeout") {
        Doc::Nil => {}
        value => match value.as_u64().filter(|&millis| millis > 0) {
            Some(millis) => settings.stop_timeout = Duration::from_millis(millis),
            None => tracing::warn!(
                value = describe(value),
                "session.stop_timeout is a whole number of milliseconds above 0; \
                 keeping the default"
            ),
        },
    }
    settings
}
