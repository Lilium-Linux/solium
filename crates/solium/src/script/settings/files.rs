//! `files`: how often the window manager's stores are saved. The rest of
//! the section is the window manager's own.

use std::time::Duration;

use crate::script::doc::{Doc, describe};

/// How often the stores are asked for, from the section: the default when
/// left out, and when of the wrong kind, named in the log.
pub(crate) fn store_interval(options: &Doc) -> Duration {
    match options.get("store_interval") {
        Doc::Nil => crate::files::STORE_INTERVAL,
        value => match value
            .number()
            .filter(|ms| *ms > 0.0)
            .and_then(|ms| Duration::try_from_secs_f64(ms / 1000.0).ok())
        {
            Some(every) => every,
            None => {
                tracing::warn!(
                    value = describe(value),
                    "files.store_interval is a number of milliseconds above 0; keeping \
                     the default, 1000"
                );
                crate::files::STORE_INTERVAL
            }
        },
    }
}
