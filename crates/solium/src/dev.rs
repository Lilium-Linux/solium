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

/// A drag: when to start it, where from, and where to.
pub(crate) type Drag = (Duration, (f64, f64), (f64, f64));

/// Drags to perform, as `<ms>:<x1>,<y1>><x2>,<y2>` separated by semicolons.
///
/// ```sh
/// SOLIUM_DRAG_AT="4000:300,200>1200,600"
/// ```
///
/// A drag is the interaction that could not be tested without a hand on a
/// mouse, and it is where the last two bugs in this compositor were — one of
/// them a deadlock that froze the machine. It runs through the real grab and
/// the real layout scripts; see `synth`.
pub(crate) fn drags() -> Vec<Drag> {
    parse_list_with("SOLIUM_DRAG_AT", ';', |value| {
        let (from, to) = value.split_once('>')?;
        let point = |raw: &str| -> Option<(f64, f64)> {
            let (x, y) = raw.split_once(',')?;
            Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
        };
        Some((point(from)?, point(to)?))
    })
    .into_iter()
    .map(|(at, (from, to))| (at, from, to))
    .collect()
}

/// Windows to open for an application that will never arrive, as
/// `<ms>:<program>` separated by commas.
///
/// ```sh
/// SOLIUM_LOADING_AT="3000:firefox,5000:slack"
/// ```
///
/// A window whose life has begun and whose application has not connected is
/// the one state that cannot be reached by using the compositor normally --
/// every real program connects, and fast. This makes one on demand, so the
/// layout reserving its place, the patience running out, and closing it
/// mid-load can all be exercised without waiting on a slow application to be
/// slow at the right moment.
pub(crate) fn loading_at() -> Vec<(Duration, String)> {
    parse_list("SOLIUM_LOADING_AT", |value| Some(value.trim().to_owned()))
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

/// Whether to report what the compositor is holding, once a second.
///
/// ```sh
/// SOLIUM_MEMDIAG=1
/// ```
pub(crate) fn memory_diagnostics() -> bool {
    std::env::var_os("SOLIUM_MEMDIAG").is_some()
}

/// Whether to show the Developer Tweaks panel.
///
/// `--debug-mode` anywhere in the arguments, so it composes with the backend
/// selection: `solium --tty --debug-mode`. Deliberately a flag rather than
/// something in the configuration -- it is a thing you turn on for a session
/// to try effects out, and it is meant to be removed when it stops earning
/// its place.
pub(crate) fn debug_mode() -> bool {
    std::env::args().any(|argument| argument == "--debug-mode")
        || std::env::var_os("SOLIUM_DEBUG_MODE").is_some()
}
