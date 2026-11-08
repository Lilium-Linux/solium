//! `run`: the defaults for what a `sol.run{}` leaves out.

use std::time::Duration;

use crate::script::RunDefaults;
use crate::script::doc::{Doc, describe};

/// Read the section. Each key left out keeps its default, and one of the
/// wrong kind is named in the log and keeps its default too.
pub(crate) fn parse(options: &Doc) -> RunDefaults {
    let mut defaults = RunDefaults::default();
    match options.get("timeout") {
        Doc::Nil => {}
        value => match value
            .number()
            .filter(|ms| *ms > 0.0)
            .and_then(|ms| Duration::try_from_secs_f64(ms / 1000.0).ok())
        {
            Some(timeout) => defaults.timeout = timeout,
            None => tracing::warn!(
                value = describe(value),
                "run.timeout is a number of milliseconds above 0; keeping the \
                 default, 30000"
            ),
        },
    }
    match options.get("max_output") {
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
            Some(bytes) => defaults.max_output = bytes as usize,
            None => tracing::warn!(
                value = describe(value),
                "run.max_output is a whole number of bytes, 0 or more; keeping the \
                 default, 1048576"
            ),
        },
    }
    defaults
}
