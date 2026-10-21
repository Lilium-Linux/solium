//! How fast a finger is moving, measured by the compositor (#284).
//!
//! A configuration that works out a release velocity from consecutive events
//! divides a distance by the time between two of them. On a Surface Pro 7's
//! touchscreen samples arrive about every 7 ms while their millisecond stamps
//! advance in 10 ms steps, so that time is often zero or a third too long, and
//! the velocity jumped to whatever cap the configuration set. So each finger
//! keeps its recent samples with the event's microsecond stamp, and the
//! velocity is the least-squares slope of position over time across a window
//! that ends at the latest event: steady under coarse or bunched stamps, and
//! near zero for a finger that rested before it lifted.

use std::collections::VecDeque;

/// How far back the fit reaches when nothing configures it: about five
/// samples at the rates touchscreens report.
pub(crate) const WINDOW_MS: u32 = 80;

/// The most samples one finger keeps, whatever the window.
const KEPT: usize = 64;

#[derive(Clone, Debug)]
pub(crate) struct Tracker {
    /// (microseconds, x, y), oldest first.
    samples: VecDeque<(u64, f64, f64)>,
    /// The latest event's stamp, a sample's or a lift's.
    latest: u64,
    window: u64,
}

impl Tracker {
    /// A finger down at `(x, y)` at `time_us`, fitted over `window_ms`.
    pub(crate) fn new(time_us: u64, x: f64, y: f64, window_ms: u32) -> Self {
        let mut tracker = Self {
            samples: VecDeque::with_capacity(KEPT),
            latest: time_us,
            window: u64::from(window_ms.max(1)) * 1000,
        };
        tracker.push(time_us, x, y);
        tracker
    }

    pub(crate) fn push(&mut self, time_us: u64, x: f64, y: f64) {
        if self.samples.len() == KEPT {
            self.samples.pop_front();
        }
        self.samples.push_back((time_us, x, y));
        self.latest = self.latest.max(time_us);
    }

    /// An event with no position, a lift: it moves the window's end, so a
    /// finger that stopped before lifting is not still moving.
    pub(crate) fn at(&mut self, time_us: u64) {
        self.latest = self.latest.max(time_us);
    }

    /// The latest event's stamp, in seconds.
    #[expect(
        clippy::cast_precision_loss,
        reason = "microseconds since boot fit an f64's mantissa for centuries"
    )]
    pub(crate) fn seconds(&self) -> f64 {
        self.latest as f64 / 1e6
    }

    /// Pixels a second, x and y: the least-squares slope over the samples in
    /// the window that ends at the latest event. None or one sample, or all of
    /// them at one instant, is no velocity.
    #[expect(
        clippy::cast_precision_loss,
        reason = "a window of microseconds is far inside an f64's mantissa"
    )]
    pub(crate) fn velocity(&self) -> (f64, f64) {
        let since = self.latest.saturating_sub(self.window);
        let recent: Vec<(f64, f64, f64)> = self
            .samples
            .iter()
            .filter(|(t, _, _)| *t >= since)
            .map(|&(t, x, y)| ((t - since) as f64 / 1e6, x, y))
            .collect();
        if recent.len() < 2 {
            return (0.0, 0.0);
        }
        let n = recent.len() as f64;
        let (st, sx, sy) = recent
            .iter()
            .fold((0.0, 0.0, 0.0), |(a, b, c), &(t, x, y)| {
                (a + t, b + x, c + y)
            });
        let (mt, mx, my) = (st / n, sx / n, sy / n);
        let (mut stt, mut stx, mut sty) = (0.0, 0.0, 0.0);
        for &(t, x, y) in &recent {
            let dt = t - mt;
            stt += dt * dt;
            stx += dt * (x - mx);
            sty += dt * (y - my);
        }
        if stt <= f64::EPSILON {
            return (0.0, 0.0);
        }
        (stx / stt, sty / stt)
    }
}

#[cfg(test)]
mod tests {
    use super::Tracker;

    /// **Coarse stamps give a steady velocity (#284).** The Surface's
    /// touchscreen: a sample every 7 ms, stamped in 10 ms steps, a finger
    /// moving at 1000 px/s along x and -500 along y. Differences of
    /// consecutive events read 0 or 1430 px/s; the fit reads the real speed.
    #[test]
    fn coarse_stamps_give_a_steady_velocity() {
        let mut tracker = Tracker::new(0, 0.0, 0.0, 80);
        for step in 1..=20_u32 {
            let real_ms = f64::from(step) * 7.0;
            let stamp_us = u64::from(step * 7 / 10 * 10) * 1000;
            tracker.push(stamp_us, real_ms, -real_ms / 2.0);
        }
        let (vx, vy) = tracker.velocity();
        assert!((vx - 1000.0).abs() < 100.0, "vx {vx}");
        assert!((vy + 500.0).abs() < 50.0, "vy {vy}");
    }

    /// **A finger that rests before lifting is not moving.** The lift's
    /// stamp ends the window, and the motion is older than it.
    #[test]
    fn a_finger_that_rests_before_lifting_is_not_moving() {
        let mut tracker = Tracker::new(0, 0.0, 0.0, 80);
        for step in 1..=10_u64 {
            tracker.push(step * 8_000, step as f64 * 8.0, 0.0);
        }
        assert!(tracker.velocity().0 > 900.0, "moving while it moves");
        tracker.at(80_000 + 200_000);
        assert_eq!(tracker.velocity(), (0.0, 0.0), "rested 200 ms, then lifted");
    }

    /// **The window is the setting's (#284):** a turn 30 ms before the
    /// latest sample reads as the new direction in a 20 ms window and as a
    /// blend of both in an 80 ms one.
    #[test]
    fn the_window_decides_how_far_back_the_fit_reaches() {
        let path = |window| {
            let mut tracker = Tracker::new(0, 0.0, 0.0, window);
            for step in 1..=10_u64 {
                let ms = step * 10;
                // Right at 1000 px/s until 70 ms, then left at 1000 px/s.
                let x = if ms <= 70 {
                    ms as f64
                } else {
                    140.0 - ms as f64
                };
                tracker.push(ms * 1000, x, 0.0);
            }
            tracker.velocity().0
        };
        assert!((path(20) + 1000.0).abs() < 1.0, "20 ms: {}", path(20));
        assert!(path(80) > -1000.0 + 100.0, "80 ms: {}", path(80));
    }

    /// **Samples bunched at one instant are no velocity, not infinity.**
    #[test]
    fn samples_at_one_instant_are_no_velocity() {
        let mut tracker = Tracker::new(5_000, 0.0, 0.0, 80);
        tracker.push(5_000, 50.0, 0.0);
        tracker.push(5_000, 90.0, 0.0);
        assert_eq!(tracker.velocity(), (0.0, 0.0));
        assert!((tracker.seconds() - 0.005).abs() < 1e-12);
    }
}
