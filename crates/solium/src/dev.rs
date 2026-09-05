//! Development knobs.
//!
//! Every one of these exists so the compositor can be driven and photographed
//! without a human at the keyboard. That is the difference between a demo and a
//! regression test: a mode that can only be checked by someone pressing a key
//! is a mode nobody checks twice.
//!
//! All are read once at startup and documented in `dev/README.md`.

use std::{path::PathBuf, time::Duration};

/// Where to write a captured frame.
pub(crate) fn capture_path() -> Option<PathBuf> {
    std::env::var_os("SOLIUM_CAPTURE").map(PathBuf::from)
}

/// How many frames to capture, and how far apart.
///
/// One frame cannot show whether an animation is smooth — it shows a pose. A
/// burst can: the window's position per frame is a curve, and a curve can be
/// checked for jumps, for stalls, and for landing where it was aimed.
pub(crate) fn capture_frames() -> usize {
    std::env::var("SOLIUM_CAPTURE_FRAMES")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(1)
        .max(1)
}

pub(crate) fn capture_interval() -> Duration {
    millis("SOLIUM_CAPTURE_INTERVAL").unwrap_or(Duration::from_millis(16))
}

/// When to capture, if a moment was named instead of "once a window settles".
///
/// Naming a moment is what makes capturing an *animation* possible: the
/// interesting frames are the ones part-way through.
pub(crate) fn capture_at() -> Option<Duration> {
    millis("SOLIUM_CAPTURE_AT")
}

/// Key combinations to fire, as `<ms>:<combo>` separated by commas.
///
/// ```sh
/// SOLIUM_TRIGGER_AT="1000:super+space,2000:super+space"
/// ```
///
/// A list rather than a single moment because the interesting cases are
/// sequences: enter, leave, and check that everything came back.
pub(crate) fn triggers() -> Vec<(Duration, String)> {
    parse_list("SOLIUM_TRIGGER_AT", |value| Some(value.to_owned()))
}

/// Pointer clicks to fire, as `<ms>:<x>,<y>` separated by semicolons.
///
/// ```sh
/// SOLIUM_CLICK_AT="1500:400,300"
/// ```
pub(crate) fn clicks() -> Vec<(Duration, (f64, f64))> {
    parse_list_with("SOLIUM_CLICK_AT", ';', |value| {
        let (x, y) = value.split_once(',')?;
        Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
    })
}

fn millis(name: &str) -> Option<Duration> {
    let raw = std::env::var(name).ok()?;
    match raw.trim().parse::<u64>() {
        Ok(value) => Some(Duration::from_millis(value)),
        Err(err) => {
            tracing::warn!(?err, variable = name, value = raw, "not a number, ignoring");
            None
        }
    }
}

fn parse_list<T>(name: &str, parse: impl Fn(&str) -> Option<T>) -> Vec<(Duration, T)> {
    parse_list_with(name, ',', parse)
}

fn parse_list_with<T>(
    name: &str,
    separator: char,
    parse: impl Fn(&str) -> Option<T>,
) -> Vec<(Duration, T)> {
    let Ok(raw) = std::env::var(name) else {
        return Vec::new();
    };

    let mut parsed: Vec<(Duration, T)> = raw
        .split(separator)
        .filter(|entry| !entry.trim().is_empty())
        .filter_map(|entry| {
            let (at, value) = entry.split_once(':')?;
            let at = at.trim().parse::<u64>().ok()?;
            Some((Duration::from_millis(at), parse(value.trim())?))
        })
        .collect();

    if parsed.len()
        != raw
            .split(separator)
            .filter(|e| !e.trim().is_empty())
            .count()
    {
        tracing::warn!(variable = name, value = raw, "some entries did not parse");
    }

    // Fired in order, so the list can be written in any.
    parsed.sort_by_key(|(at, _)| *at);
    parsed
}
