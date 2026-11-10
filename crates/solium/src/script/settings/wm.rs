//! `wm`: how long the compositor waits for the window manager, and how
//! often one that stopped is started again.

use std::time::Duration;

use crate::script::doc::{Doc, describe};
use crate::wm::Settings;

/// Read the section. Each key left out keeps its default, and one of the
/// wrong kind is named in the log and keeps its default too.
pub(crate) fn parse(options: &Doc) -> Settings {
    let default = Settings::default();
    Settings {
        ask: millis(options, "ask_ms", default.ask),
        hard: millis(options, "hard_ms", default.hard),
        load: millis(options, "load_ms", default.load),
        strikes: count(options, "strikes", 1, default.strikes),
        restarts: count(options, "restarts", 0, default.restarts),
        restart_window: match options.get("restart_window_s") {
            Doc::Nil => default.restart_window,
            value => match value
                .number()
                .filter(|seconds| *seconds > 0.0)
                .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
            {
                Some(window) => window,
                None => {
                    tracing::warn!(
                        value = describe(value),
                        "wm.restart_window_s is a number of seconds above 0; keeping the \
                         default, {}",
                        default.restart_window.as_secs()
                    );
                    default.restart_window
                }
            },
        },
    }
}

/// A whole number of at least `least` under `key`, or `default`.
fn count(options: &Doc, key: &str, least: u32, default: u32) -> u32 {
    match options.get(key) {
        Doc::Nil => default,
        value => match value
            .number()
            .filter(|count| count.fract() == 0.0 && *count >= f64::from(least))
            .and_then(|count| format!("{count:.0}").parse().ok())
        {
            Some(count) => count,
            None => {
                tracing::warn!(
                    value = describe(value),
                    "wm.{key} is a whole number, {least} or more; keeping the default, {default}"
                );
                default
            }
        },
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
    use crate::wm::Settings;

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
        assert_eq!(waits.load, Settings::default().load);
    }

    #[test]
    fn how_often_it_is_started_again_is_read_and_a_fraction_of_a_time_is_not() {
        let settings = parse(&section(&[
            ("strikes", Doc::Int(5)),
            ("restarts", Doc::Int(0)),
            ("restart_window_s", Doc::Num(7.5)),
        ]));
        assert_eq!(
            (settings.strikes, settings.restarts, settings.restart_window),
            (5, 0, Duration::from_millis(7500))
        );
        let refused = parse(&section(&[
            ("strikes", Doc::Int(0)),
            ("restarts", Doc::Num(1.5)),
            ("restart_window_s", Doc::Int(0)),
        ]));
        assert_eq!(refused, Settings::default());
    }

    #[test]
    fn a_wait_that_is_no_time_at_all_keeps_its_default() {
        let waits = parse(&section(&[
            ("ask_ms", Doc::Int(0)),
            ("hard_ms", Doc::Text("soon".to_owned())),
            ("load_ms", Doc::Num(-1.0)),
        ]));
        assert_eq!(waits, Settings::default());
    }
}
