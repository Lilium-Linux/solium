//! What a handler built, as the window manager protocol's values: the
//! motions, matrices, deforms, aims and the rest the `sol` functions read
//! out of Lua, handed over in the protocol's terms.

use solium_wm_protocol as wire;

use crate::mat4::Mat4;
use crate::present::{Channels, Curve};
use crate::script::*;

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
        // As nanoseconds over a million, which is exact for every whole
        // number of milliseconds and to the nanosecond for the rest.
        #[expect(
            clippy::cast_precision_loss,
            reason = "a motion's nanoseconds, far below 2^53"
        )]
        duration_ms: animation.duration.as_nanos() as f64 / 1e6,
        easing: easing(animation.easing),
    }
}

pub(crate) fn matrix(matrix: Mat4) -> wire::Matrix {
    matrix.0.to_vec()
}

/// A motion of its own for each property, by the property's name.
pub(crate) fn channels(
    channels: Option<Box<Channels<AnimationSpec>>>,
) -> Vec<(String, wire::Motion)> {
    channels.map_or_else(Vec::new, |channels| {
        let Channels {
            x,
            y,
            size,
            opacity,
            client_opacity,
        } = *channels;
        vec![
            ("x".to_owned(), motion(x)),
            ("y".to_owned(), motion(y)),
            ("size".to_owned(), motion(size)),
            ("opacity".to_owned(), motion(opacity)),
            ("client_opacity".to_owned(), motion(client_opacity)),
        ]
    })
}

pub(crate) fn aim(aim: Aim) -> wire::Aim {
    match aim {
        Aim::Rect(at) => wire::Aim::Rect(rect(at)),
        Aim::Window(id) => wire::Aim::Window(id),
        Aim::Surface(name) => wire::Aim::Surface(name),
    }
}

pub(crate) fn deform(deform: DeformSpec) -> wire::Deform {
    use crate::effect::settings::{PresentFailed, PresentReload};
    use solium_effects::spec::Value;
    let mut nodes = vec![wire::DocNode::Nil];
    let mut fields = Vec::new();
    for (name, value) in deform.params {
        let at = u32::try_from(nodes.len()).unwrap_or(u32::MAX);
        match value {
            Value::Bool(on) => nodes.push(wire::DocNode::Boolean(on)),
            Value::Int(whole) => nodes.push(wire::DocNode::Integer(whole)),
            Value::Number(number) => nodes.push(wire::DocNode::Number(number)),
            Value::Word(word) => nodes.push(wire::DocNode::Text(word)),
            Value::Vec4(four) => {
                nodes.push(wire::DocNode::Nil);
                let mut list = Vec::new();
                for component in four {
                    list.push(u32::try_from(nodes.len()).unwrap_or(u32::MAX));
                    nodes.push(wire::DocNode::Number(component));
                }
                nodes[at as usize] = wire::DocNode::Table(wire::DocTable {
                    list,
                    fields: Vec::new(),
                    numbered: Vec::new(),
                });
            }
        }
        fields.push((name, at));
    }
    nodes[0] = wire::DocNode::Table(wire::DocTable {
        list: Vec::new(),
        fields,
        numbered: Vec::new(),
    });
    wire::Deform {
        effect: deform.effect,
        params: nodes,
        axis: deform.axis,
        progress: deform.progress,
        aim: aim(deform.aim),
        seed: deform.seed,
        failed: deform.failed.map(|failed| {
            match failed {
                PresentFailed::Flat => "flat",
                PresentFailed::Hide => "hide",
            }
            .to_owned()
        }),
        on_reload: deform.on_reload.map(|reload| {
            match reload {
                PresentReload::Flat => "flat",
                PresentReload::Keep => "keep",
            }
            .to_owned()
        }),
    }
}

pub(crate) fn selection(selection: Selection) -> wire::Selection {
    wire::Selection {
        windows: selection.windows,
        surfaces: selection.surfaces,
        monitors: selection.monitors,
        on: selection.on,
    }
}

pub(crate) fn shift(shift: crate::group::Shift) -> wire::Shift {
    wire::Shift {
        dx: shift.dx,
        dy: shift.dy,
        opacity: shift.opacity,
        matrix: matrix(shift.matrix),
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
