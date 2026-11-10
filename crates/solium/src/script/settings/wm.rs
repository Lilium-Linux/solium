//! `wm`: how long the compositor waits for the window manager.

use std::time::Duration;

use crate::script::doc::{Doc, describe};
use crate::wm::Waits;

/// Read the section. Each key left out keeps its default, and one of the
/// wrong kind is named in the log and keeps its default too.
pub(crate) fn parse(options: &Doc) -> Waits {
    let default = Waits::default();
    Waits {
        ask: millis(options, "ask_ms", default.ask),
        hard: millis(options, "hard_ms", default.hard),
        load: millis(options, "load_ms", default.load),
    }
}

/// A number of milliseconds above 0 under `key`, or `default`.
fn millis(options: &Doc, key: &str, default: Duration) -> Duration {
    match options.get(key) {
        Doc::Nil => default,
        value => match value
            .number()
            .filter(|ms| *ms > 0.0)
            .and_then(|ms| Duration::try_from_secs_f64(ms / 1000.0).ok())
        {
            Some(wait) => wait,
            None => {
                tracing::warn!(
                    value = describe(value),
                    "wm.{key} is a number of milliseconds above 0; keeping the default, {}",
                    default.as_secs_f64() * 1000.0
                );
                default
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::parse;
    use crate::script::doc::{Doc, Table};
    use crate::wm::Waits;

    fn section(pairs: &[(&str, Doc)]) -> Doc {
        Doc::Table(Table {
            fields: pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
            ..Table::default()
        })
    }

    #[test]
    fn each_wait_is_read_and_one_left_out_keeps_its_default() {
        let waits = parse(&section(&[
            ("ask_ms", Doc::Num(2.5)),
            ("hard_ms", Doc::Int(300)),
        ]));
        assert_eq!(waits.ask, Duration::from_micros(2500));
        assert_eq!(waits.hard, Duration::from_millis(300));
        assert_eq!(waits.load, Waits::default().load);
    }

    #[test]
    fn a_wait_that_is_no_time_at_all_keeps_its_default() {
        let waits = parse(&section(&[
            ("ask_ms", Doc::Int(0)),
            ("hard_ms", Doc::Text("soon".to_owned())),
            ("load_ms", Doc::Num(-1.0)),
        ]));
        assert_eq!(waits, Waits::default());
    }
}
