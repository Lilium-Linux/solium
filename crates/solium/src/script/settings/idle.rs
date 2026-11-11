//! `idle`: when the screens lock and go dark on their own.

use std::time::Duration;

use crate::script::doc::{Doc, describe};

/// Read the section. Each key left out keeps its default, and one of the
/// wrong kind is named in the log and keeps its default too.
pub(crate) fn parse(options: &Doc) -> crate::idle::Settings {
    let mut idle = crate::idle::Settings::default();
    let read = |key: &str, unit: f64| -> Option<Duration> {
        let value = options.get(key);
        if value.is_nil() {
            return None;
        }
        // `try_`, because a number past what a `Duration` holds would
        // otherwise panic, and that is the session.
        match value
            .number()
            .filter(|amount| *amount >= 0.0)
            .and_then(|amount| Duration::try_from_secs_f64(amount * unit).ok())
        {
            Some(duration) => Some(duration),
            None => {
                tracing::warn!(
                    key,
                    value = describe(value),
                    "idle: not a number of zero or more; keeping the default"
                );
                None
            }
        }
    };
    if let Some(after) = read("lock_after", 1.0) {
        idle.lock_after = after;
    }
    if let Some(after) = read("screens_off_after", 1.0) {
        idle.screens_off_after = after;
    }
    if let Some(interval) = read("off_frame_interval", 0.001) {
        idle.off_frame_interval = interval;
    }
    match options.get("dbus_inhibit") {
        Doc::Bool(on) => idle.dbus_inhibit = *on,
        Doc::Nil => {}
        other => tracing::warn!(
            value = describe(other),
            "idle.dbus_inhibit: true or false; keeping the default, true"
        ),
    }
    idle
}
