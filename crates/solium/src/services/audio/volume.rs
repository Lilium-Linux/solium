//! Volume, as WirePlumber's mixer understands it, with no PipeWire in it
//! (03 §3.2.5, "the volume module").
//!
//! * **Which volume.** A sound card's node volume is the wrong one to write:
//!   the hardware mixer does not move and WirePlumber does not remember it.
//!   The card route's volume is right, except where there is no route (a Pro
//!   Audio profile, a virtual sink, a filter), where the node's `Props` are.
//! * **The scale.** PipeWire stores linear amplitude; `wpctl`, and people, see
//!   its cube root. The shell is given the cube root and asks in it.
//! * **Writes.** At most one write per endpoint is in flight. A newer request
//!   replaces the queued one, so a dragged slider never floods PipeWire, and
//!   the endpoint reads `pending` until PipeWire echoes the value back, or a
//!   short while passes without an echo (a change smaller than the
//!   hardware's step produces none).
//! * **Who changed it.** PipeWire carries no origin. Each write remembers what
//!   it asked for and why; an echo that matches one, within rounding, keeps
//!   that origin, and any other change is `external` (a headset's own
//!   buttons, another mixer).

use std::time::Duration;

/// How long a write waits for its echo before it is taken as settled.
pub(crate) const ECHO_PATIENCE: Duration = Duration::from_millis(300);

/// How long a write is remembered for matching a change to its origin.
const REMEMBERED_FOR: Duration = Duration::from_secs(2);

/// How close, in perceptual units, an observed volume must be to one asked
/// for to be that request's echo: the cube root of a 24-bit route volume,
/// rounded, lands within this.
const ECHO_TOLERANCE: f64 = 0.005;

/// Perceptual volume (what `wpctl` shows) to the linear amplitude PipeWire
/// stores.
/// `the_cubic_scale_matches_wpctl`.
pub(crate) fn to_linear(perceptual: f64) -> f64 {
    let v = perceptual.max(0.0);
    v * v * v
}

/// Linear amplitude to perceptual volume.
pub(crate) fn to_perceptual(linear: f64) -> f64 {
    linear.max(0.0).cbrt()
}

/// Where an endpoint's volume is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// The card's route, with its device id and route index.
    Route { device: u32, index: i32 },
    /// The node's own `Props`.
    Node,
}

/// What is known about where an endpoint's volume lives.
/// `a_card_with_a_route_is_written_through_the_route_and_pro_audio_on_the_node`.
pub(crate) fn target(route: Option<(u32, i32)>) -> Target {
    match route {
        Some((device, index)) => Target::Route { device, index },
        None => Target::Node,
    }
}

/// Who asked for a change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    /// A key binding's handler.
    Binding,
    /// A scene's own control (a slider, a mute button).
    Scene,
    /// Lua, anywhere else.
    Script,
    /// Not Solium: the device's own buttons, another mixer.
    External,
}

impl Origin {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Binding => "binding",
            Self::Scene => "scene",
            Self::Script => "script",
            Self::External => "external",
        }
    }
}

/// A request, clamped and stepped, ready to send.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Write {
    /// Perceptual, as asked.
    pub(crate) volume: f64,
    pub(crate) origin: Origin,
}

/// A request clamped to `0..=max` and rounded to `step` (perceptual), never
/// finer than the endpoint's own step.
/// `requests_are_clamped_to_max_and_rounded_to_the_step`.
pub(crate) fn clamp(volume: f64, max: f64, step: Option<f64>) -> f64 {
    let volume = if volume.is_finite() { volume } else { 0.0 };
    let clamped = volume.clamp(0.0, max.max(0.0));
    match step.filter(|step| *step > 0.0) {
        Some(step) => ((clamped / step).round() * step).clamp(0.0, max.max(0.0)),
        None => clamped,
    }
}

/// One endpoint's writes: what is in flight, what waits behind it, and what
/// was asked recently, for telling an echo from an outside change.
#[derive(Clone, Debug, Default)]
pub(crate) struct Writer {
    /// Sent, waiting for its echo, and when it was sent.
    in_flight: Option<(Write, Duration)>,
    /// The newest request made while one was in flight.
    queued: Option<Write>,
    /// Recent requests and when they were made.
    remembered: Vec<(Write, Duration)>,
}

impl Writer {
    /// Whether a write of Solium's is in flight or waiting.
    pub(crate) fn pending(&self) -> bool {
        self.in_flight.is_some() || self.queued.is_some()
    }

    /// What the endpoint should read while a write is pending: the newest
    /// request, so a slider shows where the finger is.
    pub(crate) fn shown(&self) -> Option<f64> {
        self.queued
            .or(self.in_flight.map(|(write, _)| write))
            .map(|write| write.volume)
    }

    /// A request: sent now when nothing is in flight, else queued in place of
    /// any request already waiting. The write to send, if one should be sent
    /// now.
    /// `a_dragged_slider_sends_one_write_at_a_time_and_the_newest_last`.
    pub(crate) fn request(&mut self, write: Write, now: Duration) -> Option<Write> {
        self.remembered.retain(|(_, at)| now.saturating_sub(*at) < REMEMBERED_FOR);
        self.remembered.push((write, now));
        if self.in_flight.is_some() {
            self.queued = Some(write);
            return None;
        }
        self.in_flight = Some((write, now));
        Some(write)
    }

    /// PipeWire says the endpoint's volume is now `observed` (perceptual):
    /// who changed it, and the next write to send, if one was waiting.
    /// `an_echo_keeps_its_origin_and_any_other_change_is_external`.
    pub(crate) fn observed(&mut self, observed: f64, now: Duration) -> (Origin, Option<Write>) {
        let origin = self
            .remembered
            .iter()
            .rev()
            .find(|(write, at)| {
                now.saturating_sub(*at) < REMEMBERED_FOR
                    && (write.volume - observed).abs() <= ECHO_TOLERANCE
            })
            .map_or(Origin::External, |(write, _)| write.origin);
        // The write in flight is answered by this change, its own echo or not:
        // PipeWire has moved since it was sent, so the next may go.
        let next = if self.in_flight.take().is_some() {
            self.queued.take().map(|write| {
                self.in_flight = Some((write, now));
                write
            })
        } else {
            None
        };
        (origin, next)
    }

    /// No echo came in time (a change smaller than the hardware's step):
    /// the write in flight is settled, and the next waiting one goes.
    /// `a_write_with_no_echo_settles_after_a_while`.
    pub(crate) fn expire(&mut self, now: Duration) -> Option<Write> {
        let (_, sent) = self.in_flight?;
        if now.saturating_sub(sent) < ECHO_PATIENCE {
            return None;
        }
        self.in_flight = None;
        self.queued.take().map(|write| {
            self.in_flight = Some((write, now));
            write
        })
    }
}

/// The same volume on every channel, keeping the channel count: a stereo
/// route gets two values, a 5.1 one six, never one.
/// `every_channel_gets_the_value_and_the_count_is_kept`.
pub(crate) fn channels(linear: f64, count: usize) -> Vec<f32> {
    vec![linear as f32; count.max(1)]
}

/// The perceptual volume of a channel list: their mean amplitude, as
/// `wpctl` shows a stereo sink's.
pub(crate) fn of_channels(linear: &[f32]) -> f64 {
    if linear.is_empty() {
        return 0.0;
    }
    let mean = linear.iter().map(|v| f64::from(*v)).sum::<f64>() / linear.len() as f64;
    to_perceptual(mean)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    fn write(volume: f64, origin: Origin) -> Write {
        Write { volume, origin }
    }

    #[test]
    fn the_cubic_scale_matches_wpctl() {
        // wpctl shows 0.50 for a linear 0.125.
        assert!((to_perceptual(0.125) - 0.5).abs() < 1e-9);
        assert!((to_linear(0.5) - 0.125).abs() < 1e-9);
        assert_eq!(to_linear(-1.0), 0.0);
        assert!((to_perceptual(to_linear(0.73)) - 0.73).abs() < 1e-9);
    }

    #[test]
    fn a_card_with_a_route_is_written_through_the_route_and_pro_audio_on_the_node() {
        assert_eq!(target(Some((42, 3))), Target::Route { device: 42, index: 3 });
        assert_eq!(target(None), Target::Node);
    }

    #[test]
    fn requests_are_clamped_to_max_and_rounded_to_the_step() {
        assert_eq!(clamp(1.4, 1.0, None), 1.0);
        assert_eq!(clamp(-0.2, 1.0, None), 0.0);
        assert_eq!(clamp(f64::NAN, 1.0, None), 0.0);
        assert!((clamp(0.52, 1.0, Some(0.05)) - 0.5).abs() < 1e-9);
        assert!((clamp(1.4, 1.5, Some(0.05)) - 1.4).abs() < 1e-9);
    }

    #[test]
    fn a_dragged_slider_sends_one_write_at_a_time_and_the_newest_last() {
        let mut writer = Writer::default();
        assert_eq!(
            writer.request(write(0.30, Origin::Scene), ms(0)),
            Some(write(0.30, Origin::Scene))
        );
        assert_eq!(writer.request(write(0.35, Origin::Scene), ms(5)), None);
        assert_eq!(writer.request(write(0.40, Origin::Scene), ms(10)), None);
        assert!(writer.pending());
        assert_eq!(writer.shown(), Some(0.40));
        // The echo of 0.30 lets the newest waiting request go; 0.35 never does.
        let (origin, next) = writer.observed(0.30, ms(20));
        assert_eq!(origin, Origin::Scene);
        assert_eq!(next, Some(write(0.40, Origin::Scene)));
        let (_, next) = writer.observed(0.40, ms(30));
        assert_eq!(next, None);
        assert!(!writer.pending());
    }

    #[test]
    fn an_echo_keeps_its_origin_and_any_other_change_is_external() {
        let mut writer = Writer::default();
        writer.request(write(0.6, Origin::Binding), ms(0));
        assert_eq!(writer.observed(0.6001, ms(10)).0, Origin::Binding);
        // The headset's own button.
        assert_eq!(writer.observed(0.45, ms(500)).0, Origin::External);
        // Long after, even the same value is no longer Solium's.
        assert_eq!(writer.observed(0.6, ms(5000)).0, Origin::External);
    }

    #[test]
    fn a_write_with_no_echo_settles_after_a_while() {
        let mut writer = Writer::default();
        writer.request(write(0.31, Origin::Scene), ms(0));
        writer.request(write(0.32, Origin::Scene), ms(10));
        assert_eq!(writer.expire(ms(100)), None);
        assert_eq!(writer.expire(ms(300)), Some(write(0.32, Origin::Scene)));
        assert!(writer.pending());
        assert_eq!(writer.expire(ms(700)), None);
        assert!(!writer.pending());
    }

    #[test]
    fn every_channel_gets_the_value_and_the_count_is_kept() {
        assert_eq!(channels(0.125, 2), vec![0.125, 0.125]);
        assert_eq!(channels(0.5, 6).len(), 6);
        assert_eq!(channels(0.5, 0).len(), 1);
        assert!((of_channels(&[0.125, 0.125]) - 0.5).abs() < 1e-6);
        assert_eq!(of_channels(&[]), 0.0);
    }
}
