//! `files`: how much a `sol.read` reads, and how often stores are saved.

use std::time::Duration;

use crate::script::FileSettings;
use crate::script::doc::{Doc, describe};

/// Read the section. Each key left out keeps its default, and one of the
/// wrong kind is named in the log and keeps its default too.
pub(crate) fn parse(options: &Doc) -> FileSettings {
    let mut settings = FileSettings::default();
    match options.get("max_read") {
        Doc::Nil => {}
        value => match value
            .number()
            .filter(|bytes| *bytes >= 0.0 && bytes.fract() == 0.0)
        {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "checked to be a whole number from 0, and saturating past usize"
            )]
            Some(bytes) => settings.max_read = bytes as usize,
            None => tracing::warn!(
                value = describe(value),
                "files.max_read is a whole number of bytes, 0 or more; keeping the \
                 default, 4194304"
            ),
        },
    }
    match options.get("store_interval") {
        Doc::Nil => {}
        value => match value
            .number()
            .filter(|ms| *ms > 0.0)
            .and_then(|ms| Duration::try_from_secs_f64(ms / 1000.0).ok())
        {
            Some(every) => settings.store_interval = every,
            None => tracing::warn!(
                value = describe(value),
                "files.store_interval is a number of milliseconds above 0; keeping \
                 the default, 1000"
            ),
        },
    }
    match options.get("store_max") {
        Doc::Nil => {}
        value => match value
            .number()
            .filter(|bytes| *bytes >= 0.0 && bytes.fract() == 0.0)
        {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "checked to be a whole number from 0, and saturating past u64"
            )]
            Some(bytes) => settings.store_max = bytes as u64,
            None => tracing::warn!(
                value = describe(value),
                "files.store_max is a whole number of bytes, 0 or more; keeping the \
                 default, 1048576"
            ),
        },
    }
    settings
}
