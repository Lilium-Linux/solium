//! Sample every curve and write it out for the preview page.
//!
//! The page replays these samples rather than reimplementing the curves in
//! JavaScript, so what it shows is what the compositor will do. A second
//! implementation in the browser would drift from this one the first time
//! either changed, and the drift would be invisible — the page would still look
//! plausible.
//!
//! The output is a self-contained page: the samples are embedded in it, so it
//! opens straight from disk. Needing a web server to look at an animation curve
//! is exactly the friction this is here to remove.
//!
//! ```sh
//! cargo run -p solium-animation --bin preview > crates/animation/preview/curves.html
//! xdg-open crates/animation/preview/curves.html
//! ```

use std::{fmt::Write as _, time::Duration};

use solium_animation::{Animation, Curve, Spring};

/// Samples per curve. Enough to draw smoothly at any width.
const SAMPLES: usize = 240;

fn main() {
    let mut out = String::from("{\n  \"curves\": [\n");

    let mut entries: Vec<String> = Vec::new();
    for (name, curve) in Curve::all() {
        entries.push(sample(name, curve, Duration::from_millis(400)));
    }

    // A few springs side by side, because the interesting part of a spring is
    // how its settings change it and that is impossible to judge from one.
    for (name, spring) in [
        (
            "spring: soft",
            Spring {
                stiffness: 200.0,
                damping: 26.0,
                ..Spring::default()
            },
        ),
        (
            "spring: bouncy",
            Spring {
                stiffness: 400.0,
                damping: 12.0,
                ..Spring::default()
            },
        ),
        (
            "spring: firm",
            Spring {
                stiffness: 500.0,
                damping: 45.0,
                ..Spring::default()
            },
        ),
        (
            "spring: thrown",
            Spring {
                stiffness: 300.0,
                damping: 22.0,
                initial_velocity: 6.0,
                ..Spring::default()
            },
        ),
    ] {
        entries.push(sample(name, Curve::Spring(spring), spring.settle_time()));
    }

    out.push_str(&entries.join(",\n"));
    out.push_str("\n  ]\n}");

    // The page is the template; this only fills the data in. Keeping it a real
    // file means it can be edited and reloaded without touching Rust.
    const PAGE: &str = include_str!("../../preview/index.html");
    print!("{}", PAGE.replace("/*CURVES*/ null", &out));
}

fn sample(name: &str, curve: Curve, duration: Duration) -> String {
    let animation = Animation::new(Duration::ZERO, duration, curve);
    let duration = animation.duration();

    let mut values = String::new();
    for step in 0..=SAMPLES {
        #[expect(clippy::cast_precision_loss, reason = "SAMPLES is small and fixed")]
        let fraction = step as f64 / SAMPLES as f64;
        let at = duration.mul_f64(fraction);
        if step > 0 {
            values.push_str(", ");
        }
        let _ = write!(values, "{:.5}", animation.progress(at));
    }

    format!(
        "    {{ \"name\": {name:?}, \"duration_ms\": {}, \"values\": [{values}] }}",
        duration.as_millis()
    )
}
