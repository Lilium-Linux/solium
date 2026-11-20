//! Trees of boxes: `sol.arrange` hands one to the compositor, which places
//! its windows and drags its seams; `sol.solve` works out where they come
//! out, with the same arithmetic, so a layout can decide by the answer
//! before it lands; `sol.grow` moves a seam from a key.
//!
//! A box in Lua is a table, in a list whose first entry is the root:
//!
//! ```lua
//! { parent = 1, id = 4, share = 0.5,
//!   split = { axis = "x", gap = 12, resizable = true } }   -- or
//! { parent = 1, window = 7, min = { w = 160, h = 96 },
//!   client_min = true, client_max = true }                 -- or
//! { parent = 1, free = true }                               -- or
//! { parent = 1 }                                            -- room holding nothing
//! ```
//!
//! `parent` is the place of the box it is in, in the same list, counted
//! from 1 as Lua counts; `at = { x, y, w, h }` places a child of a `free`
//! box.

use mlua::{Function, Lua, Table, Value};
use solium_wm_protocol::space::{Limits, Tree};
use solium_wm_protocol::{Arrange, Axis, Command, Grow, Size, SpaceBox, SpaceKind, Split};

use crate::*;

/// Add this area's functions to `sol`.
pub(crate) fn register(lua: &Lua, sol: &Table) -> mlua::Result<()> {
    // `sol.arrange(key, area, boxes)`: the tree under `key`, replaced whole,
    // its windows moved by the motion `sol.animate` set. With an empty list
    // the tree is taken away. Checked here, so a mistake is said at the line
    // that made it.
    sol.set(
        "arrange",
        lua.create_function(|lua, (key, area, boxes): (String, Table, Table)| {
            let area = area_of(&area, "sol.arrange")?;
            let boxes = boxes_from(&boxes)?;
            if !boxes.is_empty() {
                Tree::of(&area, &boxes)
                    .map_err(|why| mlua::Error::runtime(format!("sol.arrange: {why}")))?;
            }
            with_pending(lua, |pending| {
                let motion = to_wire::motion(pending.animation);
                // The window a key is moving, when it is in this tree.
                let moved = pending.moving.filter(|moving| {
                    boxes
                        .iter()
                        .any(|each| each.kind == SpaceKind::Window(*moving))
                });
                pending.push(Command::Arrange(Arrange {
                    key,
                    area,
                    boxes,
                    motion,
                    moved,
                }));
            })
        })?,
    )?;

    sol.set("solve", solve_function(lua)?)?;

    // `sol.grow(id, axis, by)`: the seam after window `id` on `axis` ("x"
    // or "y"), or the one before it, moved by `by`, a share of its split's
    // room. The compositor tells where it was left with `seam`.
    sol.set(
        "grow",
        lua.create_function(|lua, (window, axis, by): (u64, String, f64)| {
            let axis = axis_named(&axis)?;
            if !by.is_finite() {
                return Err(mlua::Error::runtime(format!(
                    "sol.grow: {by} is not a share of a split's room"
                )));
            }
            with_pending(lua, |pending| {
                let motion = to_wire::motion(pending.animation);
                pending.push(Command::Grow(Grow {
                    window,
                    axis,
                    by,
                    motion,
                }));
            })
        })?,
    )?;
    Ok(())
}

/// `solve(area, boxes, limits)`: where each window of the tree comes out,
/// in the tree's order, as `{ id, x, y, w, h, cramped, tile = { x, y, w, h } }`.
/// `limits`, by window id, `{ min = { w, h }, max = { w, h } }`, stands for
/// what the windows' applications say; without it, the desktop as the
/// compositor last told it does. Needs nothing of `sol`, so the shipped
/// layouts' specs run it in a bare Lua.
pub fn solve_function(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(
        |lua, (area, boxes, limits): (Table, Table, Option<Table>)| {
            let area = area_of(&area, "solve")?;
            let boxes = boxes_from(&boxes)?;
            let out = lua.create_table()?;
            if boxes.is_empty() {
                return Ok(out);
            }
            let tree = Tree::of(&area, &boxes)
                .map_err(|why| mlua::Error::runtime(format!("solve: {why}")))?;
            let given = match limits {
                Some(limits) => Some(limits_from(&limits)?),
                None => None,
            };
            let mirror = mirror(lua);
            let layout = tree.solve(|window| match &given {
                Some(given) => given
                    .iter()
                    .find(|(id, _)| *id == window)
                    .map(|(_, limits)| limits.clone())
                    .unwrap_or_default(),
                None => mirror
                    .window(window)
                    .map(|row| Limits {
                        min: row.min.clone(),
                        max: row.max.clone(),
                    })
                    .unwrap_or_default(),
            });
            for (index, solved) in layout.windows.iter().enumerate() {
                let row = lua.create_table()?;
                row.set("id", solved.window)?;
                row.set("x", solved.rect.x)?;
                row.set("y", solved.rect.y)?;
                row.set("w", solved.rect.w)?;
                row.set("h", solved.rect.h)?;
                row.set("cramped", solved.cramped)?;
                let tile = lua.create_table()?;
                tile.set("x", solved.tile.x)?;
                tile.set("y", solved.tile.y)?;
                tile.set("w", solved.tile.w)?;
                tile.set("h", solved.tile.h)?;
                row.set("tile", tile)?;
                out.raw_set(index + 1, row)?;
            }
            Ok(out)
        },
    )
}

pub(crate) fn area_of(table: &Table, call: &str) -> mlua::Result<solium_wm_protocol::Rect> {
    rect_from(table)?
        .ok_or_else(|| mlua::Error::runtime(format!("{call} needs an area: {{ x, y, w, h }}")))
}

pub(crate) fn axis_named(name: &str) -> mlua::Result<Axis> {
    match name {
        "x" => Ok(Axis::X),
        "y" => Ok(Axis::Y),
        other => Err(mlua::Error::runtime(format!(
            "`{other}` is not an axis: \"x\" or \"y\""
        ))),
    }
}

/// A size, `{ w, h }`, in whole pixels.
fn size_from(table: &Table, what: &str) -> mlua::Result<Size> {
    let side = |name: &str| -> mlua::Result<i32> {
        let value = table.get::<Option<f64>>(name)?.unwrap_or(0.0);
        if !value.is_finite() {
            return Err(mlua::Error::runtime(format!("{what}.{name} is not a size")));
        }
        #[allow(clippy::cast_possible_truncation, reason = "held to i32's range first")]
        let whole = value.round().clamp(0.0, f64::from(i32::MAX)) as i32;
        Ok(whole)
    };
    Ok(Size {
        w: side("w")?,
        h: side("h")?,
    })
}

/// The boxes of a tree, from Lua's list of them.
pub(crate) fn boxes_from(list: &Table) -> mlua::Result<Vec<SpaceBox>> {
    let mut out = Vec::new();
    for (index, each) in list.sequence_values::<Value>().enumerate() {
        let place = index + 1;
        let Value::Table(each) = each? else {
            return Err(mlua::Error::runtime(format!("box {place} is not a table")));
        };
        let parent = match each.get::<Option<i64>>("parent")? {
            None => None,
            Some(parent) => {
                let parent = u32::try_from(parent - 1).map_err(|_| {
                    mlua::Error::runtime(format!(
                        "box {place}'s parent, {parent}, is not a place in the list"
                    ))
                })?;
                Some(parent)
            }
        };
        let kind = if let Some(split) = each.get::<Option<Table>>("split")? {
            let axis: String = split
                .get::<Option<String>>("axis")?
                .ok_or_else(|| mlua::Error::runtime(format!("box {place}'s split has no axis")))?;
            SpaceKind::Split(Split {
                axis: axis_named(&axis)?,
                gap: split.get::<Option<f64>>("gap")?.unwrap_or(0.0),
                resizable: split.get::<Option<bool>>("resizable")?.unwrap_or(true),
            })
        } else if let Some(window) = each.get::<Option<u64>>("window")? {
            SpaceKind::Window(window)
        } else if each.get::<Option<bool>>("free")?.unwrap_or(false) {
            SpaceKind::Free
        } else {
            SpaceKind::Empty
        };
        let at = match each.get::<Option<Table>>("at")? {
            Some(at) => Some(rect_from(&at)?.ok_or_else(|| {
                mlua::Error::runtime(format!("box {place}'s `at` is not {{ x, y, w, h }}"))
            })?),
            None => None,
        };
        let min = match each.get::<Option<Table>>("min")? {
            Some(min) => Some(size_from(&min, &format!("box {place}'s min"))?),
            None => None,
        };
        out.push(SpaceBox {
            parent,
            id: each.get::<Option<u64>>("id")?.unwrap_or(0),
            kind,
            share: each.get::<Option<f64>>("share")?.unwrap_or(1.0),
            at,
            min,
            client_min: each.get::<Option<bool>>("client_min")?.unwrap_or(false),
            client_max: each.get::<Option<bool>>("client_max")?.unwrap_or(false),
        });
    }
    Ok(out)
}

/// What stands for the windows' applications' sizes, by window id.
pub(crate) fn limits_from(table: &Table) -> mlua::Result<Vec<(u64, Limits)>> {
    let mut out = Vec::new();
    for pair in table.pairs::<u64, Table>() {
        let (id, limits) = pair?;
        let min = match limits.get::<Option<Table>>("min")? {
            Some(min) => Some(size_from(&min, "a limit's min")?),
            None => None,
        };
        let max = match limits.get::<Option<Table>>("max")? {
            Some(max) => Some(size_from(&max, "a limit's max")?),
            None => None,
        };
        out.push((id, Limits { min, max }));
    }
    Ok(out)
}
