//! Solium's animation engine.
//!
//! Separated from the compositor on purpose, and the separation is load-bearing
//! rather than tidy: **an animation you can only judge by launching a
//! compositor is an animation nobody tunes.** Everything here is arithmetic on
//! `f64` and `Duration`, so the same code that moves windows can be driven by a
//! test, plotted by the preview tool in `preview/`, or embedded in whatever
//! else wants to show what a curve does.
//!
//! There will be many animations and many settings for them. That is the reason
//! for this shape: adding a curve here makes it available to every mode, every
//! script and the preview, without touching the renderer.
//!
//! ```
//! use std::time::Duration;
//! use solium_animation::{Animation, Curve};
//!
//! let move_window = Animation::new(Duration::ZERO, Duration::from_millis(200), Curve::OutCubic);
//! assert_eq!(move_window.progress(Duration::ZERO), 0.0);
//! assert_eq!(move_window.progress(Duration::from_millis(200)), 1.0);
//! ```

use std::time::Duration;

pub mod ffi;

/// A damped spring.
///
/// Springs are described by how they *feel* — how stiff, how damped, how heavy
/// — rather than by a duration, and they settle when they settle. That is why
/// they are worth having next to the fixed-duration curves: a window flung by a
/// gesture should carry the gesture's velocity into its animation, which no
/// amount of easing curve can express.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    /// How hard it pulls toward the target. Higher is faster and snappier.
    pub stiffness: f64,
    /// How much it resists moving. Higher settles sooner with less overshoot.
    pub damping: f64,
    /// How heavy the thing being moved is. Higher is slower and more sluggish.
    pub mass: f64,
    /// Velocity at the start, in fractions of the distance per second. A
    /// gesture's throw speed goes here.
    pub initial_velocity: f64,
    /// How close counts as arrived.
    pub epsilon: f64,
}

impl Default for Spring {
    fn default() -> Self {
        // Settles in a little over a third of a second with a small overshoot:
        // quick enough to feel responsive, soft enough not to look mechanical.
        Self {
            stiffness: 300.0,
            damping: 25.0,
            mass: 1.0,
            initial_velocity: 0.0,
            epsilon: 0.001,
        }
    }
}

impl Spring {
    /// The fraction of the way to the target at `seconds`.
    ///
    /// Solved in closed form rather than integrated step by step, so the value
    /// depends only on the time asked for. A stepped simulation would give
    /// different answers at different frame rates, which is precisely the bug
    /// that makes an animation feel different on a 60 Hz screen than on a
    /// 260 Hz one.
    #[must_use]
    pub fn value_at(&self, seconds: f64) -> f64 {
        if seconds <= 0.0 {
            return 0.0;
        }
        let (mass, stiffness) = (self.mass.max(f64::EPSILON), self.stiffness.max(0.0));
        let natural = (stiffness / mass).sqrt();
        if natural <= 0.0 {
            return 1.0;
        }
        let zeta = self.damping / (2.0 * (stiffness * mass).sqrt());
        let velocity = self.initial_velocity;

        // Displacement from the target, starting at -1 and decaying to 0.
        //
        // Solving u(0) = -1, u'(0) = velocity gives the term `(zeta * natural
        // - velocity)`. With a `+` there the sign of the initial velocity is
        // inverted: a spring thrown *toward* its target starts out behind one
        // thrown at nothing, which is backwards and is what a gesture would
        // hand it.
        let offset = if (zeta - 1.0).abs() < 1e-9 {
            // Critically damped: the fastest approach with no overshoot.
            -(1.0 + (natural - velocity) * seconds) * (-natural * seconds).exp()
        } else if zeta < 1.0 {
            // Underdamped: overshoots and rings back. What "bouncy" means.
            let damped = natural * (1.0 - zeta * zeta).sqrt();
            let decay = (-zeta * natural * seconds).exp();
            -decay
                * ((damped * seconds).cos()
                    + ((zeta * natural - velocity) / damped) * (damped * seconds).sin())
        } else {
            // Overdamped: crawls in without overshooting.
            let rate = natural * (zeta * zeta - 1.0).sqrt();
            let decay = (-zeta * natural * seconds).exp();
            -decay
                * ((rate * seconds).cosh()
                    + ((zeta * natural - velocity) / rate) * (rate * seconds).sinh())
        };

        1.0 + offset
    }

    /// Roughly when it stops moving, so a caller knows when to stop asking.
    ///
    /// Found by walking forward rather than solved, because the exact answer
    /// needs the Lambert W function and this is only ever used to decide when
    /// to drop an animation.
    #[must_use]
    pub fn settle_time(&self) -> Duration {
        const STEP: f64 = 1.0 / 240.0;
        const LIMIT: f64 = 10.0;

        let mut seconds = 0.0;
        while seconds < LIMIT {
            seconds += STEP;
            if (1.0 - self.value_at(seconds)).abs() <= self.epsilon
                && (1.0 - self.value_at(seconds + STEP)).abs() <= self.epsilon
            {
                return Duration::from_secs_f64(seconds);
            }
        }
        Duration::from_secs_f64(LIMIT)
    }
}

/// How a value travels from one end to the other.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Curve {
    Linear,
    /// Fast start, soft landing. The default because it reads as responsive
    /// without looking mechanical.
    #[default]
    OutCubic,
    /// Overshoots slightly and settles back — for things appearing.
    OutBack,
    /// Slow at both ends. For things that move rather than appear.
    InOutQuad,
    Spring(Spring),
    /// Any curve at all, as the two control points of a cubic bezier from
    /// (0,0) to (1,1) -- the same four numbers CSS calls `cubic-bezier` and
    /// every easing site on the internet hands out.
    ///
    /// The named curves above are the ones worth having a name; this is so a
    /// feel nobody anticipated does not need a compositor release.
    Bezier {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    },
}

/// A cubic bezier from (0,0) to (1,1), evaluated at `t` along the *x* axis.
///
/// The curve is parametric, so a y for a given x needs the parameter that
/// puts x there. Newton's method converges in a couple of steps for the
/// well-behaved curves easings are, and bisection finishes the rest -- the
/// same approach browsers take, for the same reason.
fn bezier(t: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    /// Close enough that a further step would move the result by less than a
    /// pixel on any real screen.
    const EPSILON: f64 = 1e-6;
    const NEWTON_STEPS: u8 = 8;

    let curve = |a: f64, b: f64, u: f64| {
        // The Bernstein form, with the two ends pinned at 0 and 1.
        let inverse = 1.0 - u;
        3.0 * inverse * inverse * u * a + 3.0 * inverse * u * u * b + u * u * u
    };
    let slope = |a: f64, b: f64, u: f64| {
        let inverse = 1.0 - u;
        3.0 * inverse * inverse * a + 6.0 * inverse * u * (b - a) + 3.0 * u * u * (1.0 - b)
    };

    let mut guess = t;
    for _ in 0..NEWTON_STEPS {
        let error = curve(x1, x2, guess) - t;
        if error.abs() < EPSILON {
            return curve(y1, y2, guess);
        }
        let gradient = slope(x1, x2, guess);
        // A flat stretch cannot be stepped along; bisection below handles it.
        if gradient.abs() < EPSILON {
            break;
        }
        guess -= error / gradient;
    }

    let (mut low, mut high) = (0.0, 1.0);
    let mut guess = t.clamp(0.0, 1.0);
    for _ in 0..32 {
        let x = curve(x1, x2, guess);
        if (x - t).abs() < EPSILON {
            break;
        }
        if x < t {
            low = guess;
        } else {
            high = guess;
        }
        guess = f64::midpoint(low, high);
    }
    curve(y1, y2, guess)
}

impl Curve {
    /// The curve's value for a normalised time in `0..=1`.
    ///
    /// May exceed 1 on the way — that is what overshoot is — so callers must
    /// not clamp the result, only the time.
    #[must_use]
    pub fn at(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::OutCubic => {
                let inverse = 1.0 - t;
                1.0 - inverse * inverse * inverse
            }
            Self::OutBack => {
                const OVERSHOOT: f64 = 1.701_58;
                let inverse = t - 1.0;
                1.0 + (OVERSHOOT + 1.0) * inverse * inverse * inverse
                    + OVERSHOOT * inverse * inverse
            }
            Self::Bezier { x1, y1, x2, y2 } => bezier(t, x1, y1, x2, y2),
            Self::InOutQuad => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    let inverse = -2.0 * t + 2.0;
                    1.0 - (inverse * inverse) / 2.0
                }
            }
            // A spring is described in seconds, not in fractions of a fixed
            // duration, so a normalised time has to be given one: its own
            // settling time.
            Self::Spring(spring) => spring.value_at(t * spring.settle_time().as_secs_f64()),
        }
    }

    /// Every curve, for tools that want to show them all.
    #[must_use]
    pub fn all() -> [(&'static str, Self); 5] {
        [
            ("linear", Self::Linear),
            ("outCubic", Self::OutCubic),
            ("outBack", Self::OutBack),
            ("inOutQuad", Self::InOutQuad),
            ("spring", Self::Spring(Spring::default())),
        ]
    }

    /// Look a curve up by the name a script or a settings file would use.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let wanted = name.trim().to_ascii_lowercase().replace(['_', '-'], "");
        Self::all()
            .into_iter()
            .find(|(known, _)| known.to_ascii_lowercase() == wanted)
            .map(|(_, curve)| curve)
    }

    /// What to call this curve.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::OutCubic => "outCubic",
            Self::OutBack => "outBack",
            Self::InOutQuad => "inOutQuad",
            Self::Spring(_) => "spring",
            Self::Bezier { .. } => "bezier",
        }
    }
}

/// One animation: when it started, how long it takes, and how it moves.
///
/// Deliberately holds no value of its own. It reports *progress*, and the
/// caller interpolates whatever it is animating — a rectangle, an opacity, a
/// colour. That is what lets one engine drive window geometry in the compositor
/// and a box in a preview page without knowing about either.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Animation {
    start: Duration,
    duration: Duration,
    curve: Curve,
}

impl Animation {
    #[must_use]
    pub fn new(start: Duration, duration: Duration, curve: Curve) -> Self {
        // A spring's duration is a property of the spring, not a setting: it
        // arrives when the physics says it does.
        let duration = match curve {
            Curve::Spring(spring) => spring.settle_time(),
            _ => duration,
        };
        Self {
            start,
            duration,
            curve,
        }
    }

    #[must_use]
    pub fn duration(&self) -> Duration {
        self.duration
    }

    #[must_use]
    pub fn curve(&self) -> Curve {
        self.curve
    }

    /// How far along it is, which is not the same as how much time has passed.
    #[must_use]
    pub fn progress(&self, now: Duration) -> f64 {
        if self.duration.is_zero() {
            return 1.0;
        }
        let elapsed = now.saturating_sub(self.start).as_secs_f64();
        self.curve.at(elapsed / self.duration.as_secs_f64())
    }

    #[must_use]
    pub fn done(&self, now: Duration) -> bool {
        now.saturating_sub(self.start) >= self.duration
    }
}

/// Interpolate between two values.
///
/// Takes progress rather than time, so overshoot passes through: a curve that
/// goes past 1 moves the value past its target and back, which is the whole
/// point of `OutBack` and of an underdamped spring.
#[must_use]
pub fn lerp(from: f64, to: f64, progress: f64) -> f64 {
    from + (to - from) * progress
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_bezier_hits_both_ends_and_stays_monotonic() {
        // ease-in-out, as CSS spells it.
        let curve = Curve::Bezier {
            x1: 0.42,
            y1: 0.0,
            x2: 0.58,
            y2: 1.0,
        };
        assert!(curve.at(0.0).abs() < 1e-6);
        assert!((curve.at(1.0) - 1.0).abs() < 1e-6);
        // Slow at the start, fast in the middle: half way through the time is
        // half way along the distance, and the quarter point is behind it.
        assert!((curve.at(0.5) - 0.5).abs() < 1e-6);
        assert!(curve.at(0.25) < 0.25);
        assert!(curve.at(0.75) > 0.75);
    }

    #[test]
    fn a_bezier_can_overshoot() {
        // The four numbers every "ease-out-back" generator produces.
        let curve = Curve::Bezier {
            x1: 0.34,
            y1: 1.56,
            x2: 0.64,
            y2: 1.0,
        };
        let peak = (0..=100)
            .map(|step| curve.at(f64::from(step) / 100.0))
            .fold(f64::MIN, f64::max);
        assert!(peak > 1.0, "an overshoot that does not exceed 1 is not one");
    }

    use super::*;

    #[test]
    fn every_curve_starts_at_zero_and_lands_on_one() {
        for (name, curve) in Curve::all() {
            assert!(curve.at(0.0).abs() < 1e-6, "{name} must start at 0");
            assert!(
                (curve.at(1.0) - 1.0).abs() < 1e-2,
                "{name} must land on 1, got {}",
                curve.at(1.0)
            );
        }
    }

    #[test]
    fn time_is_clamped_but_value_is_not() {
        // Past the end it stays landed rather than running away.
        assert!((Curve::OutCubic.at(5.0) - 1.0).abs() < 1e-9);
        assert_eq!(Curve::OutCubic.at(-1.0), 0.0);
        // OutBack overshoots on the way, and that must survive.
        let peak = (1..100)
            .map(|step| Curve::OutBack.at(f64::from(step) / 100.0))
            .fold(f64::MIN, f64::max);
        assert!(peak > 1.0, "outBack must overshoot, peaked at {peak}");
    }

    #[test]
    fn a_bouncy_spring_overshoots_and_a_stiff_one_does_not() {
        let bouncy = Spring {
            stiffness: 300.0,
            damping: 10.0,
            ..Spring::default()
        };
        let firm = Spring {
            stiffness: 300.0,
            damping: 40.0,
            ..Spring::default()
        };

        let peak = |spring: Spring| {
            (1..2000)
                .map(|step| spring.value_at(f64::from(step) / 1000.0))
                .fold(f64::MIN, f64::max)
        };

        assert!(peak(bouncy) > 1.02, "an underdamped spring rings past 1");
        assert!(peak(firm) <= 1.0 + 1e-6, "an overdamped spring must not");
    }

    #[test]
    fn a_spring_settles_and_says_when() {
        let spring = Spring::default();
        let settled = spring.settle_time();
        assert!(
            settled > Duration::from_millis(50) && settled < Duration::from_secs(3),
            "implausible settling time: {settled:?}"
        );
        assert!((1.0 - spring.value_at(settled.as_secs_f64())).abs() <= spring.epsilon);
    }

    #[test]
    fn an_initial_velocity_throws_it_forward() {
        let still = Spring::default();
        let thrown = Spring {
            initial_velocity: 5.0,
            ..Spring::default()
        };
        assert!(
            thrown.value_at(0.02) > still.value_at(0.02),
            "a spring given velocity must start out ahead"
        );
    }

    #[test]
    fn a_springs_duration_comes_from_the_spring_not_the_caller() {
        let spring = Spring::default();
        let animation = Animation::new(
            Duration::ZERO,
            // Ignored: a spring arrives when the physics says it does.
            Duration::from_secs(99),
            Curve::Spring(spring),
        );
        assert_eq!(animation.duration(), spring.settle_time());
    }

    #[test]
    fn curves_are_found_by_the_names_scripts_use() {
        assert_eq!(Curve::from_name("outCubic"), Some(Curve::OutCubic));
        assert_eq!(Curve::from_name("out_cubic"), Some(Curve::OutCubic));
        assert_eq!(Curve::from_name("OUT-CUBIC"), Some(Curve::OutCubic));
        assert_eq!(Curve::from_name("nonsense"), None);
    }

    #[test]
    fn a_zero_length_animation_is_immediately_finished() {
        let animation = Animation::new(Duration::ZERO, Duration::ZERO, Curve::OutCubic);
        assert_eq!(animation.progress(Duration::ZERO), 1.0);
        assert!(animation.done(Duration::ZERO));
    }

    #[test]
    fn lerp_carries_overshoot_through() {
        assert!((lerp(0.0, 100.0, 0.5) - 50.0).abs() < 1e-9);
        // Progress past 1 must move the value past its target, or overshoot
        // would be silently flattened at the point it matters.
        assert!((lerp(0.0, 100.0, 1.1) - 110.0).abs() < 1e-9);
    }
}
