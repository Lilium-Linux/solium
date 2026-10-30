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

/// How many monitors the nested backend should pretend to have.
///
/// ```sh
/// SOLIUM_OUTPUTS=2 ./target/debug/solium
/// ```
///
/// Side by side inside the one window, each with its own layer map, work area
/// and render pass. This exists because nested and hardware are different
/// compositors and that has already cost this project a cursor that was
/// invisible for its entire life: without it, every single-output assumption
/// removed this week would first be exercised on a TTY, where nothing can be
/// read and a mistake costs a session.
///
/// One is the default and takes the ordinary path, unchanged — the second
/// monitor is what turns the extra machinery on, so everyday nested
/// development pays nothing for it.
pub(crate) fn outputs() -> usize {
    std::env::var("SOLIUM_OUTPUTS")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(1)
        .clamp(1, 4)
}

/// When to take a nested monitor away, and when to give it back.
///
/// ```sh
/// SOLIUM_OUTPUTS_AT="5000:1,9000:2"
/// ```
///
/// `<ms>:<count>` pairs. Exists because the half of hotplug that has broken
/// twice on real hardware is not the DRM half -- it is what the *layout* does
/// with a window whose monitor has gone, and that half has nothing to do with
/// cables. A window on a monitor that disappears is in a tree nothing
/// iterates, keeps a slot that is off every screen, and comes back on that
/// screen when the monitor returns. All of which happens identically nested,
/// where it can be captured frame by frame with nobody reaching behind a desk.
pub(crate) fn outputs_at() -> Vec<(Duration, usize)> {
    parse_list("SOLIUM_OUTPUTS_AT", |value| {
        value
            .trim()
            .parse::<usize>()
            .ok()
            .map(|count| count.clamp(1, 4))
    })
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

/// Keys to press and let go, as `<ms>:<key>` separated by commas, a key being
/// keysym names joined by `+`.
///
/// ```sh
/// SOLIUM_KEY_AT="1500:caps_lock,3000:shift+alt_l"
/// ```
///
/// Through the real input path, not to a binding: `SOLIUM_TRIGGER_AT` runs
/// a binding, and Caps Lock and a layout switch are xkb's, not a binding's.
/// See `synth::key`.
pub(crate) fn keys() -> Vec<(Duration, String)> {
    parse_list("SOLIUM_KEY_AT", |value| Some(value.to_owned()))
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

/// A drag: when to start it, where from, where to, and the keys held down
/// for it, as `SOLIUM_KEY_AT` names keys (`super`, `alt`), empty for none.
pub(crate) type Drag = (Duration, (f64, f64), (f64, f64), String);

/// Drags to perform, as `<ms>:<x1>,<y1>><x2>,<y2>` separated by semicolons,
/// each with the keys to hold for it before an `@` if any.
///
/// ```sh
/// SOLIUM_DRAG_AT="4000:300,200>1200,600"
/// SOLIUM_DRAG_AT="4000:super@300,200>1200,600"
/// ```
///
/// A drag is the interaction that could not be tested without a hand on a
/// mouse, and it is where the last two bugs in this compositor were — one of
/// them a deadlock that froze the machine. It runs through the real grab and
/// the real layout scripts; see `synth`. With `super` held it is the drag
/// modifier's move, which works anywhere in a window (#427).
pub(crate) fn drags() -> Vec<Drag> {
    parse_list_with("SOLIUM_DRAG_AT", ';', parse_drag)
        .into_iter()
        .map(|(at, (from, to, held))| (at, from, to, held))
        .collect()
}

/// A drag after its moment: where from, where to, and the keys held.
type DragValue = ((f64, f64), (f64, f64), String);

/// One drag's value, after its moment: `[<keys>@]<x1>,<y1>><x2>,<y2>`.
/// `tests::a_drag_may_hold_keys`.
fn parse_drag(value: &str) -> Option<DragValue> {
    let (held, value) = value.split_once('@').unwrap_or(("", value));
    let (from, to) = value.split_once('>')?;
    let point = |raw: &str| -> Option<(f64, f64)> {
        let (x, y) = raw.split_once(',')?;
        Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
    };
    Some((point(from)?, point(to)?, held.trim().to_owned()))
}

/// Scheduled touchscreen samples, in global logical coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TouchPhase {
    Down,
    Motion,
    Up,
    Cancel,
}

#[derive(Clone, Debug)]
pub(crate) struct TouchSample {
    pub(crate) at: Duration,
    pub(crate) phase: TouchPhase,
    pub(crate) slot: u32,
    pub(crate) point: (f64, f64),
}

pub(crate) fn parse_touches(raw: &str) -> Result<Vec<TouchSample>, String> {
    let mut samples = Vec::new();
    for line in raw.split(';').filter(|s| !s.trim().is_empty()) {
        let invalid = || format!("invalid touch sample: {line}");
        let parts: Vec<_> = line.split(':').map(str::trim).collect();
        let [time, phase, slot, point] = parts.as_slice() else {
            return Err(invalid());
        };
        let at = Duration::from_millis(time.parse::<u64>().map_err(|_| invalid())?);
        let phase = match *phase {
            "down" => TouchPhase::Down,
            "motion" => TouchPhase::Motion,
            "up" => TouchPhase::Up,
            "cancel" => TouchPhase::Cancel,
            _ => return Err(invalid()),
        };
        let slot = slot.parse::<i32>().map_err(|_| invalid())?;
        let slot = u32::try_from(slot).map_err(|_| invalid())?;
        let (x, y) = point.split_once(',').ok_or_else(invalid)?;
        let (x, y) = (
            x.parse::<f64>().map_err(|_| invalid())?,
            y.parse::<f64>().map_err(|_| invalid())?,
        );
        if !x.is_finite() || !y.is_finite() {
            return Err(invalid());
        }
        samples.push(TouchSample {
            at,
            phase,
            slot,
            point: (x, y),
        });
    }
    samples.sort_by_key(|s| s.at);
    Ok(samples)
}

pub(crate) fn touches() -> Vec<TouchSample> {
    match std::env::var("SOLIUM_TOUCH_AT") {
        Ok(raw) => match parse_touches(&raw) {
            Ok(samples) => samples,
            Err(error) => {
                tracing::warn!(%error, "touch replay rejected");
                Vec::new()
            }
        },
        Err(_) => Vec::new(),
    }
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

/// The D-Bus address to tell about this session, instead of the session bus.
///
/// ```sh
/// dbus-run-session -- sh -c 'SOLIUM_SESSION_BUS=$DBUS_SESSION_BUS_ADDRESS ./target/debug/solium'
/// ```
///
/// Nested, or `solium --tty` without `--session`, this is the only way the
/// session is told anything: without it such a run leaves the environment of
/// the session around it alone, as it must.
/// `a_nested_run_tells_nobody_unless_it_is_given_a_bus` and
/// `a_manual_tty_start_tells_nobody`.
pub(crate) fn session_bus() -> Option<String> {
    std::env::var("SOLIUM_SESSION_BUS")
        .ok()
        .filter(|address| !address.trim().is_empty())
}

/// The D-Bus address to ask for logind's `Lock` and sleep signals on, instead
/// of the system bus: this function's own `SOLIUM_LOGIND_BUS`. Read the same
/// way, for the same reason — a nested run or a manual `--tty` must never
/// reach the real logind, or a test the real system bus.
pub(crate) fn logind_bus() -> Option<String> {
    std::env::var("SOLIUM_LOGIND_BUS")
        .ok()
        .filter(|address| !address.trim().is_empty())
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

/// Which scene graph QML renders on: `auto`, `gpu` or `software`.
///
/// ```sh
/// SOLIUM_QML=software ./target/debug/solium --tty
/// ```
///
/// Below `--qml` and above `SOLIUM_QML_GPU` and the configuration's
/// `qml.renderer`. Read once, by `qml::renderer::decide`, before Qt starts. See
/// `qml::renderer::the_flag_beats_the_environment_beats_the_config`.
pub(crate) fn qml() -> Option<String> {
    std::env::var_os("SOLIUM_QML").map(|value| value.to_string_lossy().into_owned())
}

/// The older spelling of `SOLIUM_QML=gpu`, set to anything.
///
/// ```sh
/// SOLIUM_QML_GPU=1 ./target/debug/solium --tty
/// ```
///
/// See `qml::renderer::the_old_knob_is_an_alias_for_gpu`.
pub(crate) fn qml_gpu() -> bool {
    std::env::var_os("SOLIUM_QML_GPU").is_some()
}

/// Whether to report what the compositor is holding, once a second.
///
/// ```sh
/// SOLIUM_MEMDIAG=1
/// ```
pub(crate) fn memory_diagnostics() -> bool {
    std::env::var_os("SOLIUM_MEMDIAG").is_some()
}

/// Whether to say where a frame's time went, on the frames that took too long.
///
/// ```sh
/// SOLIUM_PACING=1
/// ```
///
/// A flag rather than a threshold, because the threshold is not a preference:
/// it is the monitor's own frame interval, read from its mode. A number named
/// here would be right for a 260 Hz screen and wrong for the 75 Hz one beside
/// it, which is the case this compositor actually runs on.
///
/// Read exactly once — `pacing::frame` caches it on the first frame — because
/// this sits on the path that runs at the refresh rate, and by the time the
/// phases are counted a `getenv` per frame would be a `getenv` per scene per
/// output per frame. See `pacing.rs`.
pub(crate) fn pacing() -> bool {
    std::env::var_os("SOLIUM_PACING").is_some()
}

/// Where to write the per-pass trace, which also turns pacing on.
///
/// ```sh
/// SOLIUM_TRACE=$XDG_RUNTIME_DIR/trace.jsonl ./target/debug/solium
/// ```
///
/// One JSON line per pass and per flip: see `pacing::Trace`.
pub(crate) fn trace_path() -> Option<PathBuf> {
    std::env::var_os("SOLIUM_TRACE").map(PathBuf::from)
}

/// Whether a capture waits on the CPU for its GPU work.
///
/// ```sh
/// SOLIUM_FENCE_WAIT=on ./target/debug/solium --tty
/// ```
///
/// `on`, `1`, `yes` or `cpu` wait; anything else, or nothing, drops the
/// fence unwaited, and a word that is neither is named in the log. Dropped
/// by default since #392: every reader of a capture is on the context that
/// drew it, where GL orders the read after the draw (wirecheck's cases 11c
/// and 11d), #59 has landed beside it, and the wait was measured on the TTY
/// at 2.4 ms of a pass's 3.2 ms at desktop clocks (`dev/pacing-tty.sh
/// fence`), the main thread blocked on the GPU. Read once and said once, so every
/// session log records which it ran with.
/// `tests::the_fence_wait_is_off_unless_switched_on`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FenceWait {
    /// Wait for the GPU before sampling: `SyncPoint::wait`.
    Cpu,
    /// Drop the fence: GL orders the sample after the draw on one context.
    Skip,
}

impl FenceWait {
    /// The mode a value names, and a warning when it names neither.
    pub(crate) fn named(value: Option<&str>) -> (Self, Option<String>) {
        match value.map(str::trim) {
            None | Some("" | "off" | "0" | "no" | "skip") => (Self::Skip, None),
            Some("on" | "1" | "yes" | "cpu") => (Self::Cpu, None),
            Some(other) => (
                Self::Skip,
                Some(format!(
                    "SOLIUM_FENCE_WAIT={other} is neither on nor off; captures are not waited for"
                )),
            ),
        }
    }
}

/// The fence wait this process runs with, decided and logged on first use.
pub(crate) fn fence_wait() -> FenceWait {
    thread_local! {
        static DECIDED: std::cell::OnceCell<FenceWait> = const { std::cell::OnceCell::new() };
    }
    DECIDED.with(|decided| {
        *decided.get_or_init(|| {
            let value = std::env::var("SOLIUM_FENCE_WAIT").ok();
            let (wait, warning) = FenceWait::named(value.as_deref());
            if let Some(warning) = warning {
                tracing::warn!("{warning}");
            }
            match wait {
                FenceWait::Cpu => tracing::info!(
                    fence_wait = "cpu",
                    "a capture waits on the CPU for its GPU work"
                ),
                FenceWait::Skip => tracing::info!(
                    fence_wait = "skip",
                    "captures are ordered on the GPU, not waited for on the CPU"
                ),
            }
            wait
        })
    })
}

/// Whether every window capture is drawn on every pass, as before captures
/// were keyed: `SOLIUM_RECAPTURE=always`. For an A/B on one build, and the
/// control of the nested capture check. `tests::recapture_always_is_asked_for_by_name`.
pub(crate) fn recapture_named(value: Option<&str>) -> bool {
    value.map(str::trim) == Some("always")
}

/// `SOLIUM_RECAPTURE`, read once.
pub(crate) fn recapture_always() -> bool {
    thread_local! {
        static DECIDED: std::cell::OnceCell<bool> = const { std::cell::OnceCell::new() };
    }
    DECIDED.with(|decided| {
        *decided.get_or_init(|| recapture_named(std::env::var("SOLIUM_RECAPTURE").ok().as_deref()))
    })
}

/// Whether every wanted xray cut is drawn on every pass, its memos run
/// again over it: `SOLIUM_XRAY_REBUILD=always`, the control of every
/// `xray_builds` and `memo_runs` count (Ruling F50). Dev only.
/// `tests::xray_rebuild_always_is_asked_for_by_name`.
pub(crate) fn xray_rebuild_named(value: Option<&str>) -> bool {
    value.map(str::trim) == Some("always")
}

/// `SOLIUM_XRAY_REBUILD`, read once.
pub(crate) fn xray_rebuild_always() -> bool {
    thread_local! {
        static DECIDED: std::cell::OnceCell<bool> = const { std::cell::OnceCell::new() };
    }
    DECIDED.with(|decided| {
        *decided.get_or_init(|| {
            xray_rebuild_named(std::env::var("SOLIUM_XRAY_REBUILD").ok().as_deref())
        })
    })
}

/// How many passes the first frame's gate is told the frame is not ready
/// for, whatever else it waits on: `SOLIUM_WARM_INJECT=<passes>`, the
/// control of `dev/effects-check.sh xray`'s first frames (Ruling F50), since
/// nested every pass is ready at once and nothing would show the gate hold
/// and let go. Dev only. Unset, 0, the gate as it is; a value that is not a
/// whole number is 0 too, with a warning naming it.
/// `tests::warm_inject_is_a_number_of_passes`.
pub(crate) fn warm_inject_named(value: Option<&str>) -> (u64, Option<String>) {
    match value.map(str::trim) {
        None => (0, None),
        Some(raw) => match raw.parse() {
            Ok(passes) => (passes, None),
            Err(_) => (
                0,
                Some(format!(
                    "SOLIUM_WARM_INJECT={raw} is not a number of passes; nothing is injected"
                )),
            ),
        },
    }
}

/// `SOLIUM_WARM_INJECT`, read once, a bad value said once.
pub(crate) fn warm_inject() -> u64 {
    thread_local! {
        static DECIDED: std::cell::OnceCell<u64> = const { std::cell::OnceCell::new() };
    }
    DECIDED.with(|decided| {
        *decided.get_or_init(|| {
            let (passes, warning) =
                warm_inject_named(std::env::var("SOLIUM_WARM_INJECT").ok().as_deref());
            if let Some(warning) = warning {
                tracing::warn!("{warning}");
            }
            passes
        })
    })
}

/// The geometry effect whose grid the Rust genie builds instead of its
/// file, `SOLIUM_GEOMETRY_ORACLE`, read once: for comparing the two on one
/// build in `dev/effects-check.sh genie` only. A debug build's knob alone,
/// so no release or packaged binary can draw an effect from Rust (Rulings
/// 19, 23). `tests::the_geometry_oracle_is_named_by_its_effect`.
#[cfg(debug_assertions)]
pub(crate) fn geometry_oracle() -> Option<String> {
    thread_local! {
        static DECIDED: std::cell::OnceCell<Option<String>> = const { std::cell::OnceCell::new() };
    }
    DECIDED.with(|decided| {
        decided
            .get_or_init(|| oracle_named(std::env::var("SOLIUM_GEOMETRY_ORACLE").ok().as_deref()))
            .clone()
    })
}

/// `SOLIUM_GEOMETRY_ORACLE`'s value as an effect's name: trimmed, and none
/// when empty. `tests::the_geometry_oracle_is_named_by_its_effect`.
#[cfg(debug_assertions)]
fn oracle_named(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
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

#[cfg(test)]
mod tests {
    /// **A drag may hold keys**, before an `@`, as the drag modifier's move
    /// needs `super` held (#427); without one it holds none.
    #[test]
    fn a_drag_may_hold_keys() {
        assert_eq!(
            super::parse_drag("super@300,200>1200,600"),
            Some(((300.0, 200.0), (1200.0, 600.0), "super".to_owned()))
        );
        assert_eq!(
            super::parse_drag("300,200>1200,600"),
            Some(((300.0, 200.0), (1200.0, 600.0), String::new()))
        );
        assert_eq!(super::parse_drag("super@300,200"), None);
    }

    #[test]
    fn motion_274_touch_replay_keeps_times_and_stable_sample_order() {
        let samples =
            super::parse_touches("20:motion:0:3,4;10:down:0:1,2;20:up:0:0,0").expect("samples");
        assert_eq!(samples.len(), 3);
        assert_eq!(samples[0].at, std::time::Duration::from_millis(10));
        assert_eq!(samples[0].phase, super::TouchPhase::Down);
        assert_eq!(samples[1].phase, super::TouchPhase::Motion);
        assert_eq!(samples[2].phase, super::TouchPhase::Up);
        for invalid in [
            "bad",
            "1:tap:0:1,2",
            "1:down:-1:1,2",
            "1:down:0:NaN,2",
            "1:motion:0:1,inf",
        ] {
            assert!(super::parse_touches(invalid).is_err(), "{invalid}");
        }
    }

    /// **The geometry oracle is named by its effect**, trimmed; unset or
    /// empty names none.
    #[test]
    #[cfg(debug_assertions)]
    fn the_geometry_oracle_is_named_by_its_effect() {
        assert_eq!(
            super::oracle_named(Some(" genie ")),
            Some("genie".to_owned())
        );
        assert_eq!(super::oracle_named(Some("")), None);
        assert_eq!(super::oracle_named(None), None);
    }

    use super::FenceWait;

    /// **The wait is off unless it is switched on** (#392), by a word that
    /// says so; anything else leaves it off and says why. Two-valued rather
    /// than a flag, so the wait can still be asked for, for an A/B on one
    /// build (§6.5, C2).
    #[test]
    fn the_fence_wait_is_off_unless_switched_on() {
        assert_eq!(FenceWait::named(None), (FenceWait::Skip, None));
        for off in ["off", "0", "no", "skip", ""] {
            assert_eq!(
                FenceWait::named(Some(off)),
                (FenceWait::Skip, None),
                "{off}"
            );
        }
        for on in ["on", "1", "yes", "cpu"] {
            assert_eq!(FenceWait::named(Some(on)), (FenceWait::Cpu, None), "{on}");
        }
        let (wait, warning) = FenceWait::named(Some("of"));
        assert_eq!(wait, FenceWait::Skip);
        assert!(
            warning.is_some_and(|said| said.contains("of")),
            "a typo is named"
        );
    }

    /// `SOLIUM_RECAPTURE=always` draws every capture on every pass; anything
    /// else keys them.
    #[test]
    fn recapture_always_is_asked_for_by_name() {
        assert!(super::recapture_named(Some("always")));
        assert!(!super::recapture_named(Some("never")));
        assert!(!super::recapture_named(None));
    }

    /// `SOLIUM_XRAY_REBUILD=always` draws every wanted xray cut on every
    /// pass; anything else keeps a cut until what it shows changes.
    #[test]
    fn xray_rebuild_always_is_asked_for_by_name() {
        assert!(super::xray_rebuild_named(Some("always")));
        assert!(super::xray_rebuild_named(Some(" always\n")));
        assert!(!super::xray_rebuild_named(Some("never")));
        assert!(!super::xray_rebuild_named(None));
    }

    /// `SOLIUM_WARM_INJECT=<passes>` is a whole number of passes; unset, the
    /// gate as it is (0), and anything else 0 with a warning naming it.
    #[test]
    fn warm_inject_is_a_number_of_passes() {
        assert_eq!(super::warm_inject_named(Some("3")), (3, None));
        assert_eq!(super::warm_inject_named(Some(" 2\n")), (2, None));
        assert_eq!(super::warm_inject_named(None), (0, None));
        let (passes, warning) = super::warm_inject_named(Some("three"));
        assert_eq!(passes, 0);
        assert!(
            warning.is_some_and(|said| said.contains("three")),
            "a value that is not a number is named"
        );
    }
}
