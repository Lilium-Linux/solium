//! What a handler built, as the window manager protocol's values: the
//! motions, matrices, deforms, aims and the rest the `sol` functions read
//! out of Lua, handed over in the protocol's terms.

use solium_wm_protocol as wire;

use crate::mat4::Mat4;
use crate::script::*;
use solium_animation::Curve;

pub(crate) fn rect(rect: Rect) -> wire::Rect {
    wire::Rect {
        x: rect.x,
        y: rect.y,
        w: rect.w,
        h: rect.h,
    }
}

pub(crate) fn easing(curve: Curve) -> wire::Easing {
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

pub(crate) fn motion(animation: AnimationSpec) -> wire::Motion {
    wire::Motion {
        duration_ms: millis(animation.duration),
        easing: easing(animation.easing),
    }
}

pub(crate) fn matrix(matrix: Mat4) -> wire::Matrix {
    matrix.0.to_vec()
}

pub(crate) fn selection(selection: Selection) -> wire::Selection {
    wire::Selection {
        windows: selection.windows,
        surfaces: selection.surfaces,
        monitors: selection.monitors,
        on: selection.on,
    }
}

pub(crate) fn warp(warp: Warp) -> wire::Warp {
    wire::Warp {
        to: match warp.to {
            WarpTo::Point { x, y } => wire::WarpTo::Point((x, y)),
            WarpTo::Window { id, at } => wire::WarpTo::Window((id, at.0, at.1)),
        },
        motion: warp.motion.map(motion),
        interrupt: match warp.interrupt {
            Interrupt::Pointer => wire::Interrupt::Pointer,
            Interrupt::Picture => wire::Interrupt::Picture,
        },
    }
}

pub(crate) fn focus_cause(source: FocusSource) -> wire::FocusCause {
    match source {
        FocusSource::Binding => wire::FocusCause::Binding,
        FocusSource::Direction => wire::FocusCause::Direction,
        FocusSource::Move => wire::FocusCause::Move,
        FocusSource::Click => wire::FocusCause::Click,
        FocusSource::Scroll => wire::FocusCause::Scroll,
        FocusSource::Drop => wire::FocusCause::Drop,
        FocusSource::Touch => wire::FocusCause::Touch,
        FocusSource::Hover => wire::FocusCause::Hover,
        FocusSource::Surface => wire::FocusCause::Surface,
        FocusSource::Open => wire::FocusCause::Open,
        FocusSource::Activate => wire::FocusCause::Activate,
        FocusSource::Settle => wire::FocusCause::Settle,
        FocusSource::Script => wire::FocusCause::Script,
    }
}

pub(crate) fn press(press: Press) -> wire::Press {
    match press {
        Press::Button(button) => wire::Press::Button(button),
        Press::Keys(keys) => wire::Press::Keys(
            keys.into_iter()
                .map(smithay::input::keyboard::xkb::keysym_get_name)
                .collect(),
        ),
    }
}

/// A scene surface's declaration.
pub(crate) fn surface(declaration: crate::scripted::Declaration) -> wire::Surface {
    use crate::scripted::{KeyPolicy, Layer, On, Outside};
    let outside = |outside: Outside| match outside {
        Outside::Swallow => wire::Outside::Swallow,
        Outside::Pass => wire::Outside::Pass,
    };
    wire::Surface {
        name: declaration.name,
        scene: declaration.scene.to_string_lossy().into_owned(),
        layer: match declaration.layer {
            Layer::Background => wire::Layer::Background,
            Layer::Bottom => wire::Layer::Bottom,
            Layer::Top => wire::Layer::Top,
            Layer::Overlay => wire::Layer::Overlay,
        },
        on: match declaration.on {
            On::EveryMonitor => wire::Placement::EveryMonitor,
            On::Primary => wire::Placement::Primary,
            On::Monitor(name) => wire::Placement::Monitor(name),
            On::Rect(rect) => {
                wire::Placement::Rect((rect.loc.x, rect.loc.y, rect.size.w, rect.size.h))
            }
        },
        properties: declaration.properties.render(),
        interactive: declaration.interactive,
        reserve: wire::Insets {
            top: declaration.reserve.top,
            right: declaration.reserve.right,
            bottom: declaration.reserve.bottom,
            left: declaration.reserve.left,
        },
        outside_click: wire::OutsideClick {
            default: outside(declaration.outside_click.default),
            named: declaration
                .outside_click
                .named
                .into_iter()
                .map(|(name, policy)| (name, outside(policy)))
                .collect(),
        },
        keyboard: match declaration.keyboard {
            KeyPolicy::ExceptClaimed => wire::KeyPolicy::ExceptClaimed,
            KeyPolicy::All => wire::KeyPolicy::All,
            KeyPolicy::NoBindings => wire::KeyPolicy::NoBindings,
        },
    }
}

/// An object's fields as JSON text.
pub(crate) fn object(fields: &std::collections::BTreeMap<String, crate::json::Json>) -> String {
    crate::json::Json::Object(fields.clone()).render()
}

/// A duration as milliseconds, exact to the nanosecond.
pub(crate) fn millis(duration: std::time::Duration) -> f64 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "nanoseconds of a timer or a timeout, far below 2^53"
    )]
    let millis = duration.as_nanos() as f64 / 1e6;
    millis
}
