//! The presentation transform, and the one animation clock.
//!
//! The idea the rest of the compositor rests on: a window is *drawn* wherever
//! its presentation says, not where the layout put it. Overview, the app
//! switcher, peek and the icon→window genie then stop being five features and
//! become one operation with five different targets — see
//! `docs/architecture.md`.
//!
//! Two rules that must not be relaxed:
//!
//! * **A transform never changes real geometry.** Leaving a mode restores the
//!   layout exactly, because the layout was never touched. Everything here
//!   takes real geometry as an argument and never writes it back.
//! * **There is one clock**, ticked once per frame by the render loop. Not one
//!   per window, per subsystem or per script — separate clocks are how modes
//!   end up animating at subtly different speeds.

use std::{cell::RefCell, time::Duration};

use smithay::{
    desktop::Window,
    utils::{Logical, Point, Rectangle},
};

/// A monotonic clock, sampled once per frame.
///
/// Every animation reads the same instant, so two windows started together
/// stay together even if a frame takes longer than expected.
#[derive(Debug)]
pub(crate) struct Clock {
    start: std::time::Instant,
    now: Duration,
}

impl Clock {
    pub(crate) fn new() -> Self {
        Self {
            start: std::time::Instant::now(),
            now: Duration::ZERO,
        }
    }

    /// Sample the clock. Called once per frame, before anything reads it.
    pub(crate) fn tick(&mut self) {
        self.now = self.start.elapsed();
    }

    pub(crate) fn now(&self) -> Duration {
        self.now
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

/// Easing curves. Named after what they do, not after their polynomial.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Easing {
    /// Fast start, soft landing. The default because it reads as "responsive"
    /// without looking mechanical.
    #[default]
    OutCubic,
    /// Overshoots slightly and settles back — for things appearing.
    OutBack,
}

impl Easing {
    fn apply(self, t: f32) -> f32 {
        match self {
            Self::OutCubic => {
                let inv = 1.0 - t;
                1.0 - inv * inv * inv
            }
            Self::OutBack => {
                const OVERSHOOT: f32 = 1.70158;
                let inv = t - 1.0;
                1.0 + (OVERSHOOT + 1.0) * inv * inv * inv + OVERSHOOT * inv * inv
            }
        }
    }
}

/// A logical rectangle from plain numbers.
///
/// `Rectangle::new` takes a typed `Point` and `Size`, which is the right
/// discipline for a compositor and pure noise at the call sites doing the
/// arithmetic. The conversion happens here and nowhere else.
pub(crate) fn logical(loc: (f64, f64), size: (f64, f64)) -> Rectangle<f64, Logical> {
    Rectangle::new(loc.into(), size.into())
}

/// How a window is drawn for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Frame {
    pub(crate) rect: Rectangle<f64, Logical>,
    pub(crate) opacity: f32,
}

impl Frame {
    /// The identity frame: drawn at real geometry, fully opaque.
    pub(crate) fn real(geometry: Rectangle<i32, Logical>) -> Self {
        Self {
            rect: geometry.to_f64(),
            opacity: 1.0,
        }
    }

    /// Scaled about its own centre — what "smaller, in place" means, and the
    /// shape both the open animation and peek want.
    pub(crate) fn scaled(self, factor: f64) -> Self {
        let size = (self.rect.size.w * factor, self.rect.size.h * factor);
        let loc = (
            self.rect.loc.x + (self.rect.size.w - size.0) / 2.0,
            self.rect.loc.y + (self.rect.size.h - size.1) / 2.0,
        );
        Self {
            rect: logical(loc, size),
            opacity: self.opacity,
        }
    }

    pub(crate) fn with_opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity;
        self
    }

    fn blend(self, other: Self, t: f32) -> Self {
        let mix = |a: f64, b: f64| a + (b - a) * f64::from(t);
        Self {
            rect: logical(
                (
                    mix(self.rect.loc.x, other.rect.loc.x),
                    mix(self.rect.loc.y, other.rect.loc.y),
                ),
                (
                    mix(self.rect.size.w, other.rect.size.w),
                    mix(self.rect.size.h, other.rect.size.h),
                ),
            ),
            opacity: self.opacity + (other.opacity - self.opacity) * t,
        }
    }
}

/// An in-flight transform: where the window was, where it is going, and when.
#[derive(Clone, Copy, Debug)]
struct Transform {
    from: Frame,
    to: Frame,
    started: Duration,
    duration: Duration,
    easing: Easing,
    /// Drop the transform when it lands, so the window goes back to being drawn
    /// at real geometry with no per-frame cost. Set when leaving a mode.
    release: bool,
}

impl Transform {
    fn progress(&self, now: Duration) -> f32 {
        if self.duration.is_zero() {
            return 1.0;
        }
        let elapsed = now.saturating_sub(self.started).as_secs_f32();
        (elapsed / self.duration.as_secs_f32()).clamp(0.0, 1.0)
    }

    fn frame(&self, now: Duration) -> Frame {
        self.from
            .blend(self.to, self.easing.apply(self.progress(now)))
    }

    fn finished(&self, now: Duration) -> bool {
        self.progress(now) >= 1.0
    }
}

type Slot = RefCell<Option<Transform>>;

/// Run `f` against a window's transform slot, creating it on first use.
///
/// `try_borrow_mut` rather than `borrow_mut`: a re-entrant borrow would panic,
/// and a compositor panic takes the session with it. Losing one frame of an
/// animation is the better failure.
fn with_slot<T>(window: &Window, f: impl FnOnce(&mut Option<Transform>) -> T) -> Option<T> {
    window.user_data().insert_if_missing(Slot::default);
    let slot = window.user_data().get::<Slot>()?;
    match slot.try_borrow_mut() {
        Ok(mut current) => Some(f(&mut current)),
        Err(_) => {
            tracing::warn!("transform slot was already borrowed, skipping");
            None
        }
    }
}

/// Draw `window` at `to`, animating from wherever it is being drawn right now.
///
/// Animating from the *current* frame rather than from real geometry is what
/// makes re-entering a mode mid-animation continuous instead of a snap.
pub(crate) fn present(
    window: &Window,
    real: Rectangle<i32, Logical>,
    to: Frame,
    now: Duration,
    duration: Duration,
    easing: Easing,
) {
    let from = frame(window, real, now);
    with_slot(window, |slot| {
        *slot = Some(Transform {
            from,
            to,
            started: now,
            duration,
            easing,
            release: false,
        });
    });
}

/// Animate back to real geometry, then stop transforming this window.
pub(crate) fn clear(
    window: &Window,
    real: Rectangle<i32, Logical>,
    now: Duration,
    duration: Duration,
    easing: Easing,
) {
    let from = frame(window, real, now);
    with_slot(window, |slot| {
        *slot = Some(Transform {
            from,
            to: Frame::real(real),
            started: now,
            duration,
            easing,
            release: true,
        });
    });
}

/// The frame to draw this window in. Real geometry when nothing is animating.
pub(crate) fn frame(window: &Window, real: Rectangle<i32, Logical>, now: Duration) -> Frame {
    with_slot(window, |slot| {
        slot.map_or_else(|| Frame::real(real), |transform| transform.frame(now))
    })
    .unwrap_or_else(|| Frame::real(real))
}

/// Retire finished transforms. Returns whether this window is still animating.
pub(crate) fn settle(window: &Window, now: Duration) -> bool {
    with_slot(window, |slot| {
        let Some(transform) = *slot else {
            return false;
        };
        if !transform.finished(now) {
            return true;
        }
        if transform.release {
            *slot = None;
        }
        false
    })
    .unwrap_or(false)
}

/// Whether a window has ever been shown.
#[derive(Debug, Default)]
struct Shown(std::cell::Cell<bool>);

/// Claim the first-show moment, returning whether this call won it.
///
/// The moment is the first commit that carries a buffer, not the map request:
/// before that commit the client has not been told its size, so there is no
/// geometry either to place it at or to animate to. Both happen here, once.
pub(crate) fn mark_shown(window: &Window) -> bool {
    window.user_data().insert_if_missing(Shown::default);
    let Some(shown) = window.user_data().get::<Shown>() else {
        return false;
    };
    !shown.0.replace(true)
}

/// The animation a window plays when it first has something to show.
pub(crate) fn open(window: &Window, real: Rectangle<i32, Logical>, now: Duration) {
    let target = Frame::real(real);
    with_slot(window, |slot| {
        *slot = Some(Transform {
            from: target.scaled(0.88).with_opacity(0.0),
            to: target,
            started: now,
            duration: Duration::from_millis(220),
            easing: Easing::OutBack,
            release: true,
        });
    });
}

/// Map a point in drawn space into a window's own coordinates.
///
/// The inverse of the transform, and the reason hit-testing keeps working in a
/// mode: a thumbnail must be clickable where it is drawn, and the client must
/// still receive coordinates in its own scale.
pub(crate) fn to_window_space(
    frame: Frame,
    real: Rectangle<i32, Logical>,
    point: Point<f64, Logical>,
) -> Point<f64, Logical> {
    let scale_x = if frame.rect.size.w == 0.0 {
        1.0
    } else {
        f64::from(real.size.w) / frame.rect.size.w
    };
    let scale_y = if frame.rect.size.h == 0.0 {
        1.0
    } else {
        f64::from(real.size.h) / frame.rect.size.h
    };
    Point::from((
        (point.x - frame.rect.loc.x) * scale_x + f64::from(real.loc.x),
        (point.y - frame.rect.loc.y) * scale_y + f64::from(real.loc.y),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    #[test]
    fn a_transform_starts_where_it_was_and_lands_on_its_target() {
        let from = Frame::real(rect(0, 0, 100, 100));
        let to = Frame::real(rect(500, 400, 50, 50));
        let transform = Transform {
            from,
            to,
            started: Duration::from_millis(1000),
            duration: Duration::from_millis(200),
            easing: Easing::OutCubic,
            release: false,
        };

        assert_eq!(transform.frame(Duration::from_millis(1000)), from);
        assert_eq!(transform.frame(Duration::from_millis(1200)), to);
        // Past the end it stays landed rather than overshooting off-screen.
        assert_eq!(transform.frame(Duration::from_millis(5000)), to);
        assert!(transform.finished(Duration::from_millis(1200)));
        assert!(!transform.finished(Duration::from_millis(1100)));
    }

    #[test]
    fn a_zero_length_transform_is_immediately_at_its_target() {
        let to = Frame::real(rect(10, 10, 20, 20));
        let transform = Transform {
            from: Frame::real(rect(0, 0, 100, 100)),
            to,
            started: Duration::from_millis(500),
            duration: Duration::ZERO,
            easing: Easing::OutCubic,
            release: false,
        };
        assert_eq!(transform.frame(Duration::from_millis(500)), to);
    }

    #[test]
    fn scaling_is_about_the_centre() {
        let frame = Frame::real(rect(0, 0, 100, 100)).scaled(0.5);
        assert_eq!(frame.rect.loc.x, 25.0);
        assert_eq!(frame.rect.loc.y, 25.0);
        assert_eq!(frame.rect.size.w, 50.0);
    }

    #[test]
    fn easing_starts_at_zero_and_ends_at_one() {
        for easing in [Easing::OutCubic, Easing::OutBack] {
            assert!(easing.apply(0.0).abs() < 1e-6, "{easing:?} must start at 0");
            assert!(
                (easing.apply(1.0) - 1.0).abs() < 1e-6,
                "{easing:?} must land on 1"
            );
        }
    }

    #[test]
    fn hit_testing_inverts_the_transform() {
        let real = rect(0, 0, 400, 300);
        // Drawn at half size, offset to the right.
        let drawn = Frame {
            rect: logical((500.0, 100.0), (200.0, 150.0)),
            opacity: 1.0,
        };
        // The centre of the thumbnail is the centre of the window.
        let mapped = to_window_space(drawn, real, Point::from((600.0, 175.0)));
        assert!((mapped.x - 200.0).abs() < 1e-6, "got {}", mapped.x);
        assert!((mapped.y - 150.0).abs() < 1e-6, "got {}", mapped.y);
    }
}
