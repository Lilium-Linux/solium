//! The presentation transform: where a window is drawn, and how it gets there.

use std::time::Duration;

use mlua::{Lua, Table, Value};

use crate::*;

/// Add this area's functions to `sol`.
pub(crate) fn register(lua: &Lua, sol: &Table) -> mlua::Result<()> {
    sol.set(
        "present",
        lua.create_function(|lua, (id, options): (u64, Option<Table>)| {
            let (rect, opacity, client_opacity, matrix, deform) = match options.as_ref() {
                Some(options) => (
                    rect_from(options)?,
                    options.get::<Option<f32>>("opacity")?,
                    client_opacity_from(options, "sol.present")?,
                    transform_from(options)?,
                    deform_from(options)?,
                ),
                None => (None, None, None, None, None),
            };
            // Outside the match, because these two resolve to a value where
            // the five above resolve to "said nothing": `sol.present(id)` and
            // `sol.present(id, {})` have to produce the same depth and the
            // same pivot, and one function answering for both tables is how
            // the two answers cannot drift apart.
            let z = depth_from(options.as_ref())?;
            let pivot = pivot_from(options.as_ref())?;
            let motion = options
                .as_ref()
                .map(present_motion_from)
                .transpose()?
                .flatten();
            with_pending(lua, |pending| {
                let animation = pending.animation;
                let channels = resolved(motion, animation);
                pending.push(solium_wm_protocol::Command::Present(
                    solium_wm_protocol::Present {
                        id,
                        rect,
                        opacity,
                        client_opacity,
                        matrix: matrix.map(to_wire::matrix),
                        deform,
                        z,
                        pivot,
                        motion: to_wire::motion(animation),
                        channels,
                    },
                ));
            })
        })?,
    )?;

    // The other direction from `present`: this says where a window comes *from*
    // and lets it land where it belongs. A dock icon's rectangle here is the
    // macOS-style genie, and the compositor needs no idea that is what it is.
    sol.set(
        "present_from",
        lua.create_function(|lua, (id, options): (u64, Table)| {
            let Some(rect) = rect_from(&options)? else {
                return Err(mlua::Error::runtime(
                    "sol.present_from needs a rect to come from",
                ));
            };
            let opacity = options.get::<Option<f32>>("opacity")?;
            let client_opacity = client_opacity_from(&options, "sol.present_from")?;
            let motion = present_motion_from(&options)?;
            let deform = deform_from(&options)?;
            with_pending(lua, |pending| {
                let animation = pending.animation;
                let channels = resolved(motion, animation);
                pending.push(solium_wm_protocol::Command::PresentFrom(
                    solium_wm_protocol::PresentFrom {
                        id,
                        rect,
                        opacity,
                        client_opacity,
                        motion: to_wire::motion(animation),
                        channels,
                        deform,
                    },
                ));
            })
        })?,
    )?;

    sol.set(
        "present_clear",
        lua.create_function(|lua, id: u64| {
            with_pending(lua, |pending| {
                let animation = pending.animation;
                pending.push(solium_wm_protocol::Command::Clear((
                    id,
                    to_wire::motion(animation),
                )));
            })
        })?,
    )?;

    // Name a selection: windows, surfaces and whole monitors, under one name.
    //
    //     sol.group("desk-2", {
    //         windows  = { 3, 7 },
    //         surfaces = { "wallpaper-2" },
    //         monitor  = "DP-1",
    //     })
    //     sol.present_group("desk-2", { x = -2560 }, { duration = 300 })
    //
    // **A transform names a selection, and selections compose.** The wallpaper
    // travels because it is in the selection, not because the compositor knows
    // what a wallpaper is -- it does not, and `sol.surface` stays one primitive
    // doing five jobs. A member's own `sol.present` composes with the group's
    // rather than being replaced by it, so a window tilted inside a moving desk
    // stays tilted within it.
    //
    // `sol.group(name, false)` takes one away. Re-declaring the same name
    // replaces the membership and keeps the transform, so a mode that rebuilds
    // its groups every time it runs -- which is every mode -- does not restart
    // its own animation.
    sol.set(
        "group",
        lua.create_function(|lua, (name, options): (String, Value)| {
            let selection = match options {
                Value::Table(options) => Some(selection_from(&options)?),
                // `false` and `nil` both remove it, the same way `sol.surface`
                // reads them, so one spelling works for both primitives.
                _ => None,
            };
            with_pending(lua, |pending| {
                let animation = pending.animation;
                pending.push(solium_wm_protocol::Command::Group((
                    name.clone(),
                    selection.clone(),
                    to_wire::motion(animation),
                )));
            })
        })?,
    )?;

    sol.set(
        "present_group",
        lua.create_function(
            |lua, (name, options, motion): (String, Option<Table>, Option<Table>)| {
                let to = match options.as_ref() {
                    Some(options) => shift_from(options)?,
                    None => solium_wm_protocol::Shift {
                        dx: 0.0,
                        dy: 0.0,
                        opacity: 1.0,
                        matrix: to_wire::matrix(solium_animation::mat4::Mat4::IDENTITY),
                    },
                };
                let (duration, easing) = motion_from(motion.as_ref())?;
                with_pending(lua, |pending| {
                    let animation = pending.animation.with(duration, easing);
                    pending.push(solium_wm_protocol::Command::PresentGroup((
                        name.clone(),
                        to,
                        to_wire::motion(animation),
                    )));
                })
            },
        )?,
    )?;

    sol.set(
        "present_group_clear",
        lua.create_function(|lua, (name, motion): (String, Option<Table>)| {
            let (duration, easing) = motion_from(motion.as_ref())?;
            with_pending(lua, |pending| {
                let animation = pending.animation.with(duration, easing);
                pending.push(solium_wm_protocol::Command::ClearGroup((
                    name.clone(),
                    to_wire::motion(animation),
                )));
            })
        })?,
    )?;

    // Applies to everything queued after it, so a mode sets the feel of a batch
    // once instead of repeating it per window — and every window in that batch
    // is animated by the same clock over the same interval.
    sol.set(
        "animate",
        lua.create_function(|lua, options: Table| {
            let duration = options
                .get::<Option<u64>>("duration")?
                .map(Duration::from_millis);
            let easing = easing_from(&options)?;
            with_pending(lua, |pending| {
                if let Some(duration) = duration {
                    pending.animation.duration = duration;
                }
                if let Some(easing) = easing {
                    pending.animation.easing = easing;
                }
                pending.animated = true;
            })
        })?,
    )?;

    // Take the pointer to a window or a point (#220): the pointer goes at
    // once, and its picture flies along `motion`, or is there at once with
    // none. Carried out once every command of this dispatch is applied, so
    // a window this handler has just moved is warped to where it was moved.
    // `tests::sol_warp_names_a_window_or_a_point_and_a_bad_one_is_an_error`.
    sol.set(
        "warp",
        lua.create_function(|lua, (target, options): (Value, Option<Table>)| {
            let (at, motion, interrupt) = warp_options(options.as_ref(), "sol.warp")?;
            let to = match target {
                Value::Integer(id) => {
                    let id = u64::try_from(id).map_err(|_| {
                        mlua::Error::runtime(format!("sol.warp: {id} is no window"))
                    })?;
                    let (x, y) = at.unwrap_or(CENTRE);
                    solium_wm_protocol::WarpTo::Window((id, x, y))
                }
                Value::Table(point) => {
                    if at.is_some() {
                        return Err(mlua::Error::runtime(
                            "sol.warp: `to` is where on a window; a point is already a place",
                        ));
                    }
                    match (
                        point.get::<Option<f64>>("x")?,
                        point.get::<Option<f64>>("y")?,
                    ) {
                        (Some(x), Some(y)) if x.is_finite() && y.is_finite() => {
                            solium_wm_protocol::WarpTo::Point((x, y))
                        }
                        _ => {
                            return Err(mlua::Error::runtime(
                                "sol.warp: a point is { x = <number>, y = <number> }",
                            ));
                        }
                    }
                }
                other => {
                    return Err(mlua::Error::runtime(format!(
                        "sol.warp: a window's id or a point {{ x, y }}, not {}",
                        other.type_name()
                    )));
                }
            };
            with_pending(lua, |pending| {
                pending.push(solium_wm_protocol::Command::Warp(
                    solium_wm_protocol::Warp {
                        to,
                        motion,
                        interrupt,
                    },
                ));
            })
        })?,
    )?;

    Ok(())
}
