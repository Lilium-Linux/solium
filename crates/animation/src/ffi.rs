//! A C ABI over the engine, so it can be called from outside Rust.
//!
//! This exists for the preview: the engine is built for `wasm32` and driven
//! from the page, which means what you tune in a browser is *this* code and not
//! a copy of it in JavaScript. A copy would drift the first time either
//! changed, and the drift would be invisible — the page would still animate
//! plausibly.
//!
//! Only `f64` and integers cross the boundary, so there is nothing to allocate,
//! free or get wrong about ownership.

use std::time::Duration;

use crate::{Animation, Curve, Spring};

/// The curve at `index` in [`Curve::all`], or the default if out of range.
fn curve(index: u32) -> Curve {
    Curve::all()
        .get(index as usize)
        .map_or_else(Curve::default, |(_, curve)| *curve)
}

/// How many curves there are, so a caller can enumerate them.
#[expect(unsafe_code, reason = "exporting a C symbol for the preview")]
#[unsafe(no_mangle)]
pub extern "C" fn solium_curve_count() -> u32 {
    Curve::all().len() as u32
}

/// A curve's value at normalised time `t`.
#[expect(unsafe_code, reason = "exporting a C symbol for the preview")]
#[unsafe(no_mangle)]
pub extern "C" fn solium_curve_at(index: u32, t: f64) -> f64 {
    curve(index).at(t)
}

/// A spring's value `seconds` after it was released.
#[expect(unsafe_code, reason = "exporting a C symbol for the preview")]
#[unsafe(no_mangle)]
pub extern "C" fn solium_spring_at(
    stiffness: f64,
    damping: f64,
    mass: f64,
    initial_velocity: f64,
    seconds: f64,
) -> f64 {
    spring(stiffness, damping, mass, initial_velocity).value_at(seconds)
}

/// How long a spring takes to settle, in milliseconds.
#[expect(unsafe_code, reason = "exporting a C symbol for the preview")]
#[unsafe(no_mangle)]
pub extern "C" fn solium_spring_settle_ms(
    stiffness: f64,
    damping: f64,
    mass: f64,
    initial_velocity: f64,
) -> f64 {
    spring(stiffness, damping, mass, initial_velocity)
        .settle_time()
        .as_secs_f64()
        * 1000.0
}

/// Progress of a fixed-duration animation, for callers that would rather not
/// divide by the duration themselves.
#[expect(unsafe_code, reason = "exporting a C symbol for the preview")]
#[unsafe(no_mangle)]
pub extern "C" fn solium_progress(index: u32, duration_ms: f64, elapsed_ms: f64) -> f64 {
    let animation = Animation::new(
        Duration::ZERO,
        Duration::from_secs_f64(duration_ms.max(0.0) / 1000.0),
        curve(index),
    );
    animation.progress(Duration::from_secs_f64(elapsed_ms.max(0.0) / 1000.0))
}

fn spring(stiffness: f64, damping: f64, mass: f64, initial_velocity: f64) -> Spring {
    Spring {
        stiffness,
        damping,
        mass,
        initial_velocity,
        ..Spring::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_exported_surface_agrees_with_the_engine() {
        // The preview must not be able to see a different answer than the
        // compositor does.
        for index in 0..solium_curve_count() {
            for step in 0..=10 {
                let t = f64::from(step) / 10.0;
                assert!((solium_curve_at(index, t) - curve(index).at(t)).abs() < f64::EPSILON);
            }
        }
    }

    #[test]
    fn an_out_of_range_curve_falls_back_rather_than_panicking() {
        // Reachable from a page that is out of date with the engine.
        assert!((solium_curve_at(9999, 1.0) - Curve::default().at(1.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn settling_is_reported_in_milliseconds() {
        let millis = solium_spring_settle_ms(300.0, 25.0, 1.0, 0.0);
        assert!(millis > 50.0 && millis < 3000.0, "got {millis}ms");
    }
}
