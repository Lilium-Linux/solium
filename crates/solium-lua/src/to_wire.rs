//! What a handler built, as the window manager protocol's values: the
//! motions, matrices, deforms, aims and the rest the `sol` functions read
//! out of Lua, handed over in the protocol's terms.

use solium_wm_protocol as wire;

use solium_animation::Curve;
use solium_animation::mat4::Mat4;

use crate::AnimationSpec;

pub fn easing(curve: Curve) -> wire::Easing {
    match curve {
        Curve::Linear => wire::Easing::Linear,
        Curve::OutCubic => wire::Easing::OutCubic,
        Curve::OutBack => wire::Easing::OutBack,
        Curve::InOutQuad => wire::Easing::InOutQuad,
        Curve::InOutCubic => wire::Easing::InOutCubic,
        Curve::Bezier { x1, y1, x2, y2 } => wire::Easing::Bezier((x1, y1, x2, y2)),
        Curve::Spring(spring) => wire::Easing::Spring(wire::Spring {
            stiffness: spring.stiffness,
            damping: spring.damping,
            mass: spring.mass,
            initial_velocity: spring.initial_velocity,
            epsilon: spring.epsilon,
        }),
    }
}

pub fn motion(animation: AnimationSpec) -> wire::Motion {
    wire::Motion {
        duration_ms: millis(animation.duration),
        easing: easing(animation.easing),
    }
}

pub fn matrix(matrix: Mat4) -> wire::Matrix {
    matrix.0.to_vec()
}

/// An object's fields as JSON text.
pub fn object(fields: &std::collections::BTreeMap<String, solium_json::Json>) -> String {
    solium_json::Json::Object(fields.clone()).render()
}

/// A duration as milliseconds, exact to the nanosecond.
pub fn millis(duration: std::time::Duration) -> f64 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "nanoseconds of a timer or a timeout, far below 2^53"
    )]
    let millis = duration.as_nanos() as f64 / 1e6;
    millis
}
