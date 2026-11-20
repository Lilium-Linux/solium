//! `notifications`, `tray`, `media` and `audio`: the session services
//! Solium serves, and how.

use std::time::Duration;

use super::read;
use crate::script::doc::{Doc, describe};

/// `notifications`: whether Solium is the notification server, how long
/// banners last by urgency (0 is never), and how much history is kept.
pub(crate) fn notifications(options: &Doc) -> crate::services::notifications::Config {
    let mut config = crate::services::notifications::Config::default();
    match options.get("serve") {
        Doc::Bool(on) => config.serve = *on,
        Doc::Nil => {}
        other => tracing::warn!(
            value = describe(other),
            "notifications.serve: true or false; keeping the default, false"
        ),
    }
    let millis = |table: &Doc, key: &str| -> Option<Option<Duration>> {
        let value = table.get(key);
        if value.is_nil() {
            return None;
        }
        match value.number().filter(|amount| *amount >= 0.0) {
            Some(0.0) => Some(None),
            Some(amount) => Duration::try_from_secs_f64(amount / 1000.0).ok().map(Some),
            None => {
                tracing::warn!(
                    key,
                    value = describe(value),
                    "notifications.timeouts: milliseconds of zero or more; keeping the default"
                );
                None
            }
        }
    };
    if let timeouts @ Doc::Table(_) = options.get("timeouts") {
        let defaults = &mut config.settings.timeouts;
        if let Some(low) = millis(timeouts, "low") {
            defaults.low = low;
        }
        if let Some(normal) = millis(timeouts, "normal") {
            defaults.normal = normal;
        }
        if let Some(critical) = millis(timeouts, "critical") {
            defaults.critical = critical;
        }
    }
    if let history @ Doc::Table(_) = options.get("history") {
        match history.get("max") {
            Doc::Nil => {}
            value => match value.number().filter(|n| *n >= 0.0 && *n <= 100_000.0) {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "checked to be from 0 to 100000"
                )]
                Some(max) => config.settings.history_max = max as usize,
                None => tracing::warn!(
                    value = describe(value),
                    "notifications.history.max: a count from 0 to 100000; keeping 200"
                ),
            },
        }
    }
    config
}

/// `tray`: whether Solium is the session's StatusNotifierWatcher.
pub(crate) fn tray(options: &Doc) -> crate::services::tray::Config {
    let mut config = crate::services::tray::Config::default();
    match options.get("serve") {
        Doc::Bool(on) => config.serve = *on,
        Doc::Nil => {}
        other => tracing::warn!(
            value = describe(other),
            "tray.serve: true or false; keeping the default, false"
        ),
    }
    config
}

/// `media`: whether the session's MPRIS players are watched, which are
/// ignored, and how often a playing one's position is asked.
pub(crate) fn media(options: &Doc) -> crate::services::media::Config {
    let mut config = crate::services::media::Config::default();
    match options.get("watch") {
        Doc::Bool(on) => config.watch = *on,
        Doc::Nil => {}
        other => tracing::warn!(
            value = describe(other),
            "media.watch: true or false; keeping the default, false"
        ),
    }
    match options.get("ignore") {
        names @ Doc::Table(_) => {
            config.ignore = names
                .list()
                .iter()
                .filter_map(|name| read::text_of(name).ok().flatten())
                .take(64)
                .collect();
        }
        Doc::Nil => {}
        other => tracing::warn!(
            value = describe(other),
            "media.ignore: a list of player names; keeping the default"
        ),
    }
    match options.get("position_interval_ms") {
        Doc::Int(ms) => {
            config.position_interval_ms = u32::try_from((*ms).clamp(0, 60_000)).unwrap_or(0);
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "clamped to 0 to 60000 first"
        )]
        Doc::Num(ms) if ms.is_finite() => {
            config.position_interval_ms = ms.clamp(0.0, 60_000.0) as u32;
        }
        Doc::Nil => {}
        other => tracing::warn!(
            value = describe(other),
            "media.position_interval_ms: milliseconds, 0 to 60000; keeping the default"
        ),
    }
    config
}

/// `audio`: the volume keys' ceiling and step.
pub(crate) fn audio(options: &Doc) -> crate::services::audio::model::Settings {
    let mut settings = crate::services::audio::model::Settings::default();
    for (key, slot, top) in [
        ("max", &mut settings.max, 1.5),
        ("step", &mut settings.step, 0.5),
    ] {
        match options.get(key) {
            Doc::Nil => {}
            value => match value.number().filter(|n| *n > 0.0 && *n <= top) {
                Some(n) => *slot = n,
                None => tracing::warn!(
                    key,
                    value = describe(value),
                    "audio: a number above 0 and at most {top}; keeping the default"
                ),
            },
        }
    }
    settings
}
