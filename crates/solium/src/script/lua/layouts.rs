//! `sol.layout`: the layout engines as Lua userdata, and the tables they
//! are configured with.

use mlua::{Lua, Table, Value};

use solium_layout::{Rect as Slot, Settings};

/// The work area a layout call was given.
pub(crate) fn area(options: &Table) -> mlua::Result<Slot> {
    Ok(Slot::new(
        options.get("x")?,
        options.get("y")?,
        options.get("w")?,
        options.get("h")?,
    ))
}

pub(crate) fn tuning(options: &Table) -> mlua::Result<Settings> {
    let defaults = Settings::default();
    Ok(Settings {
        gap: options.get::<Option<f64>>("gap")?.unwrap_or(defaults.gap),
        ratio: options
            .get::<Option<f64>>("ratio")?
            .unwrap_or(defaults.ratio),
        column: options
            .get::<Option<f64>>("column")?
            .unwrap_or(defaults.column),
        padding: options
            .get::<Option<f64>>("padding")?
            .unwrap_or(defaults.padding),
        split: options
            .get::<Option<f64>>("split")?
            .unwrap_or(defaults.split),
        minimum: minimum_from(options),
    })
}

/// `options.minimum`, the smallest a dwindle tile may be: `{ w = ..., h = ... }`.
///
/// Absent is no minimum, which is what every options table held before #134.
/// So is a side that is not a finite size of zero or more -- a string, a
/// negative, `0/0` -- on that side only. Quietly, unlike `scroller_from`,
/// because this runs on every layout call and a line per call would bury the
/// log; `tiling.lua` checks `config.tiling.minimum` itself before handing it
/// over and says so there, once per value. See
/// `a_minimum_that_is_not_a_size_is_named_once_and_ignored`.
pub(crate) fn minimum_from(options: &Table) -> solium_layout::Minimum {
    let Ok(Value::Table(minimum)) = options.get::<Value>("minimum") else {
        return solium_layout::Minimum::default();
    };
    let side = |name: &str| {
        minimum
            .get::<Value>(name)
            .ok()
            .as_ref()
            .and_then(number)
            .filter(|size| size.is_finite() && *size >= 0.0)
            .unwrap_or(0.0)
    };
    solium_layout::Minimum {
        w: side("w"),
        h: side("h"),
    }
}

/// `options.floors`, each window's own floor (#115): `{ [id] = { w = ..., h =
/// ... } }`, frame included. What `tiling.lua` and `scrolling.lua` build from
/// `sol.windows()`'s `min` for every call they make.
///
/// Absent is no floors, which is every options table before #115. A side that
/// is not a finite size of zero or more is no floor on that side, and an entry
/// that is not a table under a window's id is no floor at all -- quietly, for
/// `minimum_from`'s reason: this runs on every layout call.
pub(crate) fn floors_from(options: &Table) -> solium_layout::Floors {
    let Ok(Value::Table(floors)) = options.get::<Value>("floors") else {
        return solium_layout::Floors::new();
    };
    floors
        .pairs::<Value, Value>()
        .filter_map(Result::ok)
        .filter_map(|(id, floor)| {
            let id = number(&id).filter(|id| id.is_finite() && *id >= 0.0)?;
            let Value::Table(floor) = floor else {
                return None;
            };
            let side = |name: &str| {
                floor
                    .get::<Value>(name)
                    .ok()
                    .as_ref()
                    .and_then(number)
                    .filter(|size| size.is_finite() && *size >= 0.0)
                    .unwrap_or(0.0)
            };
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a window id is a small whole number, and checked to be one above"
            )]
            let id = id as u64;
            Some((
                id,
                solium_layout::Minimum {
                    w: side("w"),
                    h: side("h"),
                },
            ))
        })
        .collect()
}

/// Which way a script means a seam to run.
///
/// "width" is the seam that bounds a window's width, and that seam is cut
/// *vertically*. The two names disagree, which is exactly why this is a named
/// function: `axis == "width"` producing `Vertical` reads like a bug at the
/// call site, and has been reported as one.
///
/// Anything else is "height", because the caller is a keyboard binding
/// choosing between two axes and there is no third answer to fall back to.
/// [`edge_named`] is stricter for the opposite reason: it has four answers and
/// a wrong guess moves a seam the user did not touch.
pub(crate) fn axis_named(name: &str) -> solium_layout::tree::Axis {
    if name == "width" {
        solium_layout::tree::Axis::Vertical
    } else {
        solium_layout::tree::Axis::Horizontal
    }
}

/// Which side of a window a script means.
///
/// The spellings are the ones the `resize` event emits — see
/// [`crate::input::resize::sides`] — so the ordinary script is passing back a
/// value the compositor just handed it and cannot misspell. `None` for
/// anything else, so a hand-written one is told rather than silently given a
/// side it did not ask for.
pub(crate) fn edge_named(name: &str) -> Option<solium_layout::tree::Edge> {
    use solium_layout::tree::Edge;
    match name {
        "left" => Some(Edge::Left),
        "right" => Some(Edge::Right),
        "top" => Some(Edge::Top),
        "bottom" => Some(Edge::Bottom),
        _ => None,
    }
}

/// A Lua number, whatever Lua happened to store it as.
///
/// `widths = { 1/3, 1/2, 1 }` puts two floats and an *integer* in one list, and
/// asking mlua for an `f64` on the wrong one is a conversion error rather than
/// a None -- which, with a `?` on it, would take the whole configuration down
/// over a list that is perfectly well written. Same reasoning as `scene` and
/// `cursor.size`, which are both read this way and say so.
pub(crate) fn number(value: &Value) -> Option<f64> {
    match value {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a share of a view, written as a whole number: 1, not 2^53"
        )]
        Value::Integer(whole) => Some(*whole as f64),
        Value::Number(fraction) => Some(*fraction),
        _ => None,
    }
}

/// Build a scrolling strip from `config.scrolling`.
///
/// Both of these keys were documented in `lua/config.lua` and read by nothing
/// until #117: the strip used the constant list in `solium_layout`, so a
/// configuration naming quarters got thirds, and the person who wrote it could
/// not tell whether they had misunderstood the setting or been ignored.
///
/// **A value that is not a share of a view is dropped with a line in the log,
/// not clamped and not fatal.** The same policy as `cursor.size`, for the same
/// reason: a column at 0.001 of the view is a column nobody can see and nobody
/// asked for, so guessing what was meant is worse than saying so and carrying
/// on. Fatal is wrong here too -- this runs while the configuration is being
/// read, and a hard error over one number would cost every other setting in the
/// file, the layouts and the bindings included.
pub(crate) fn scroller_from(options: &Table) -> mlua::Result<solium_layout::scroller::Scroller> {
    let mut widths = Vec::new();
    if let Ok(Value::Table(list)) = options.get::<Value>("widths") {
        for (index, value) in list.sequence_values::<Value>().flatten().enumerate() {
            match number(&value) {
                // Above 1.0 refused as well as below 0: a column wider than the
                // view can never be brought fully into view, so the strip would
                // scroll for ever trying to. `layout` clamps to 1.0 anyway,
                // which is how this used to be silent.
                Some(share) if share.is_finite() && share > 0.0 && share <= 1.0 => {
                    widths.push(share);
                }
                _ => tracing::warn!(
                    target: "solium::script",
                    at = index + 1,
                    "scrolling.widths holds something that is not a share of the view \
                     (a number above 0 and at most 1); that entry is ignored"
                ),
            }
        }
    }
    // 1-based, the way Lua counts and the way `config.lua` documents it. The
    // subtraction belongs at this boundary and nowhere deeper.
    //
    // Filtered before the cast rather than clamped after it, and `f64::clamp`
    // is deliberately not used -- clippy will offer it. `clamp` *returns* NaN
    // for a NaN input, `NaN as usize` saturates to 0, and `0 - 1` on a `usize`
    // is the underflow. Nobody writes `default_width = 0/0` on purpose, but a
    // generated configuration can produce one, and "the compositor panicked"
    // is not an acceptable answer to a number in a settings file.
    let start = match options.get::<Value>("default_width") {
        // Not written at all, which is the ordinary case and nothing to say
        // about. `Nil` rather than an error: mlua answers a missing key with
        // one, so this arm is "the setting is absent" and the next is "the
        // setting is there and is not an index".
        Ok(Value::Nil) | Err(_) => 0,
        Ok(value) => match number(&value).filter(|index| index.is_finite() && *index >= 1.0) {
            Some(index) => {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "finite, at least 1, and capped at 1000 on the line \
                                  itself; a fractional index is truncated on purpose"
                )]
                let index = index.min(1000.0) as usize;
                index.saturating_sub(1)
            }
            None => {
                tracing::warn!(
                    target: "solium::script",
                    "scrolling.default_width is not an index into scrolling.widths \
                     (a whole number from 1 up); a new column opens at the first \
                     width instead"
                );
                0
            }
        },
    };
    // Against the list that is actually in force, which is `PRESETS` when the
    // configuration's own list turned out to hold nothing usable. Gated on
    // `!widths.is_empty()` until #117's review, which meant a file whose every
    // width was rejected got a line per width and nothing at all about the
    // index -- so the clamp, the one step that decides what a new column opens
    // at, was the only silent thing left in a read that says everything else
    // out loud.
    let in_force = if widths.is_empty() {
        solium_layout::scroller::PRESETS.len()
    } else {
        widths.len()
    };
    if start >= in_force {
        tracing::warn!(
            target: "solium::script",
            default_width = start + 1,
            widths = in_force,
            // Named, because "not in scrolling.widths" is confusing advice when
            // the list being counted against is not the one in the file.
            fallback = widths.is_empty(),
            "scrolling.default_width names a width that is not in the list of widths \
             in force; a new column opens at the last one instead"
        );
    }
    Ok(solium_layout::scroller::Scroller::with_widths(
        &widths, start,
    ))
}

/// The `sol.layout` table.
pub(crate) fn layouts(lua: &Lua) -> mlua::Result<Table> {
    fn to_lua(lua: &Lua, slots: &[Slot]) -> mlua::Result<Table> {
        let list = lua.create_table()?;
        for (index, slot) in slots.iter().enumerate() {
            let entry = lua.create_table()?;
            entry.set("x", slot.x)?;
            entry.set("y", slot.y)?;
            entry.set("w", slot.w)?;
            entry.set("h", slot.h)?;
            list.set(index + 1, entry)?;
        }
        Ok(list)
    }

    let layout = lua.create_table()?;

    layout.set(
        "master_stack",
        lua.create_function(|lua, (count, options): (usize, Table)| {
            let slots = solium_layout::master_stack(count, area(&options)?, tuning(&options)?);
            to_lua(lua, &slots)
        })?,
    )?;

    // A dwindle tree, held by the script that made it.
    //
    // Stateful on purpose, because the arrangement is: where a window lands
    // depends on which window was split and where the pointer was, and no
    // function of the window list can recover that after the fact.
    layout.set(
        "tree",
        lua.create_function(|_, ()| Ok(TilingTree::default()))?,
    )?;

    // A scrolling workspace, held by the script that made it. Stateful for the
    // same reason the tree is: which column is active and where the view sits
    // relative to it are not recoverable from a list of windows.
    // Takes `config.scrolling` -- the whole section, not a rewrapping of it --
    // and reads the two keys that describe the strip itself. Optional, because
    // a script may want a plain one and because a `config.lua` written before
    // #117 has nothing to hand over.
    layout.set(
        "scroller",
        lua.create_function(|_, options: Option<Table>| {
            let Some(options) = options else {
                return Ok(Scrolling::default());
            };
            Ok(Scrolling(scroller_from(&options)?))
        })?,
    )?;

    layout.set(
        "strip",
        lua.create_function(|lua, (columns, options): (Table, Table)| {
            let columns = columns_from(&columns)?;
            let offset = options.get::<Option<f64>>("offset")?.unwrap_or_default();
            let slots = solium_layout::strip(&columns, area(&options)?, tuning(&options)?, offset);
            to_lua(lua, &slots)
        })?,
    )?;

    layout.set(
        "strip_scroll_to",
        lua.create_function(|_, (index, columns, options): (usize, Table, Table)| {
            let columns = columns_from(&columns)?;
            let offset = options.get::<Option<f64>>("offset")?.unwrap_or_default();
            Ok(solium_layout::strip_scroll_to(
                index.saturating_sub(1),
                &columns,
                area(&options)?,
                tuning(&options)?,
                offset,
            ))
        })?,
    )?;

    layout.set(
        "scrolling",
        lua.create_function(|lua, (count, options): (usize, Table)| {
            let offset = options.get::<Option<f64>>("offset")?.unwrap_or_default();
            let slots = solium_layout::scrolling(count, area(&options)?, tuning(&options)?, offset);
            to_lua(lua, &slots)
        })?,
    )?;

    layout.set(
        "scroll_to",
        lua.create_function(|_, (index, count, options): (usize, usize, Table)| {
            let offset = options.get::<Option<f64>>("offset")?.unwrap_or_default();
            Ok(solium_layout::scroll_to(
                index.saturating_sub(1),
                count,
                area(&options)?,
                tuning(&options)?,
                offset,
            ))
        })?,
    )?;

    layout.set(
        "grid",
        lua.create_function(|lua, (sizes, options): (Table, Table)| {
            let mut windows = Vec::new();
            for size in sizes.sequence_values::<Table>() {
                let size = size?;
                windows.push(Slot::new(
                    size.get("x")?,
                    size.get("y")?,
                    size.get("w")?,
                    size.get("h")?,
                ));
            }
            let slots = solium_layout::grid(&windows, area(&options)?, tuning(&options)?);
            to_lua(lua, &slots)
        })?,
    )?;

    Ok(layout)
}

/// A scrolling workspace, as scripts hold it.
#[derive(Debug, Default)]
pub(crate) struct Scrolling(pub(crate) solium_layout::scroller::Scroller);

impl mlua::UserData for Scrolling {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        // Every call that is handed `options` takes `options.floors` from it
        // first (#115), since each of them measures columns -- to lay them
        // out, or to bring one into view -- and a column is as wide as its
        // widest floor. See `Scroller::set_floors`.
        methods.add_method_mut("insert", |_, this, (id, options): (u64, Table)| {
            this.0.set_floors(floors_from(&options));
            this.0.insert(id, area(&options)?, tuning(&options)?);
            Ok(())
        });

        methods.add_method_mut(
            "insert_into_column",
            |_, this, (id, options): (u64, Table)| {
                this.0.set_floors(floors_from(&options));
                this.0
                    .insert_into_active(id, area(&options)?, tuning(&options)?);
                Ok(())
            },
        );

        methods.add_method_mut("remove", |_, this, id: u64| {
            this.0.remove(id);
            Ok(())
        });

        methods.add_method_mut("focus_window", |_, this, (id, options): (u64, Table)| {
            this.0.set_floors(floors_from(&options));
            this.0.focus_window(id, area(&options)?, tuning(&options)?);
            Ok(())
        });

        methods.add_method_mut("focus_sideways", |_, this, (by, options): (i32, Table)| {
            this.0.set_floors(floors_from(&options));
            this.0
                .focus_sideways(by as isize, area(&options)?, tuning(&options)?);
            Ok(())
        });

        methods.add_method_mut("focus_vertically", |_, this, by: i32| {
            this.0.focus_vertically(by as isize);
            Ok(())
        });

        // Whether the window moved. See `Scroller::move_vertically`.
        methods.add_method_mut("move_vertically", |_, this, by: i32| {
            Ok(this.0.move_vertically(by as isize))
        });

        // Whether the column moved. See `Scroller::move_column`.
        methods.add_method_mut("move_column", |_, this, (by, options): (i32, Table)| {
            this.0.set_floors(floors_from(&options));
            Ok(this
                .0
                .move_column(by as isize, area(&options)?, tuning(&options)?))
        });

        methods.add_method_mut("consume", |_, this, ()| {
            this.0.consume();
            Ok(())
        });

        methods.add_method_mut("expel", |_, this, options: Table| {
            this.0.set_floors(floors_from(&options));
            this.0.expel(area(&options)?, tuning(&options)?);
            Ok(())
        });

        methods.add_method_mut(
            "move_to_column_of",
            |_, this, (id, target, options): (u64, u64, Table)| {
                this.0.set_floors(floors_from(&options));
                this.0
                    .move_to_column_of(id, target, area(&options)?, tuning(&options)?);
                Ok(())
            },
        );

        methods.add_method_mut("widen", |_, this, (id, by, options): (u64, f64, Table)| {
            this.0.set_floors(floors_from(&options));
            this.0.widen(id, by, area(&options)?, tuning(&options)?);
            Ok(())
        });

        methods.add_method_mut("cycle_width", |_, this, options: Table| {
            this.0.set_floors(floors_from(&options));
            this.0.cycle_width(area(&options)?, tuning(&options)?);
            Ok(())
        });

        // The widths from `config.scrolling` as the file reads now -- the list
        // a column cycles through and the width the next one opens at -- with
        // the columns, the focus and the view left as they are. Takes what
        // `sol.layout.scroller` takes, and reads it the same way. For a strip
        // `sol.keep` carried across a reload, which is not made again and so
        // would go on with the last file's widths (#118). See
        // `a_reload_keeps_the_strip_and_takes_the_widths_the_file_now_names`.
        methods.add_method_mut("configure", |_, this, options: Table| {
            this.0.configure_like(&scroller_from(&options)?);
            Ok(())
        });

        methods.add_method_mut("scroll_by", |_, this, delta: f64| {
            this.0.scroll_by(delta);
            Ok(())
        });

        methods.add_method("focused", |_, this, ()| Ok(this.0.focused()));
        methods.add_method("contains", |_, this, id: u64| Ok(this.0.contains(id)));

        // Every window in the strip, left to right and top to bottom within a
        // column: the order a user reads it in. What `scrolling.lua` looks up a
        // closing window's neighbours in, so a refused one can be put back
        // beside them.
        methods.add_method("windows", |lua, this, ()| {
            let out = lua.create_table()?;
            let order = this
                .0
                .columns()
                .iter()
                .flat_map(|column| column.windows.iter().copied());
            for (index, id) in order.enumerate() {
                out.set(index + 1, id)?;
            }
            Ok(out)
        });

        methods.add_method_mut("layout", |lua, this, options: Table| {
            this.0.set_floors(floors_from(&options));
            let out = lua.create_table()?;
            for (index, (id, rect)) in this
                .0
                .layout(area(&options)?, tuning(&options)?)
                .into_iter()
                .enumerate()
            {
                let entry = lua.create_table()?;
                entry.set("id", id)?;
                entry.set("x", rect.x)?;
                entry.set("y", rect.y)?;
                entry.set("w", rect.w)?;
                entry.set("h", rect.h)?;
                out.set(index + 1, entry)?;
            }
            Ok(out)
        });
    }
}

/// A dwindle tree, as scripts hold it.
#[derive(Debug, Default)]
pub(crate) struct TilingTree(pub(crate) solium_layout::tree::Tiling);

impl mlua::UserData for TilingTree {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        // `target` is the window to split and `x`/`y` are where the pointer
        // was; both may be nil, and then the pointer alone decides — which is
        // how Hyprland picks what to divide.
        //
        // Every call that is handed `options` takes `options.floors` from it
        // first (#115): each of them lays the tree out, to split a tile, move
        // a seam or place the windows, and the tree is laid out around each
        // window's own floor. See `Tiling::set_floors`.
        methods.add_method_mut(
            "insert",
            |_, this, (id, target, x, y, options): (u64, Option<u64>, Option<f64>, Option<f64>, Table)| {
                let at = x.zip(y);
                this.0.set_floors(floors_from(&options));
                this.0
                    .insert(id, target, at, area(&options)?, tuning(&options)?);
                Ok(())
            },
        );

        // `insert` with `options.minimum` under it (#134): the same target and
        // the same side, then the other axis, and `false` -- with the tree
        // untouched -- when neither leaves both halves at the minimum. What a
        // layout tries first, before anything in `config.tiling.overflow`.
        methods.add_method_mut(
            "insert_fitting",
            |_, this, (id, target, x, y, options): (u64, Option<u64>, Option<f64>, Option<f64>, Table)| {
                let at = x.zip(y);
                this.0.set_floors(floors_from(&options));
                Ok(this
                    .0
                    .insert_fitting(id, target, at, area(&options)?, tuning(&options)?))
            },
        );

        // `"largest"` in `config.tiling.overflow`: split the largest tile that
        // has room for another window at `options.minimum`, or answer `false`
        // and leave the tree alone. No target and no pointer, because the tile
        // under the pointer is the one that has just been found to be full.
        methods.add_method_mut("insert_largest", |_, this, (id, options): (u64, Table)| {
            this.0.set_floors(floors_from(&options));
            Ok(this
                .0
                .insert_largest(id, area(&options)?, tuning(&options)?))
        });

        methods.add_method_mut("remove", |_, this, id: u64| {
            this.0.remove(id);
            Ok(())
        });

        // Whether both were in the tree. See `Tiling::swap`.
        methods.add_method_mut("swap", |_, this, (a, b): (u64, u64)| Ok(this.0.swap(a, b)));

        // The keyboard path: `axis` is "width" or "height" and `by` is a signed
        // fraction that always *grows* the window when positive, from whichever
        // of the two seams beside it exists. An axis and not an edge because a
        // keypress names no side — `super+equal` means "wider" and nothing
        // about which neighbour pays for it. See `Tiling::resize`.
        //
        // Existence and not room, which this said before and `Tiling::resize`'s
        // own doc has always had right. The trailing seam is preferred and the
        // leading one is a fallback only when there is no trailing seam at all
        // — the window is against its container on that side. A trailing seam
        // that exists but is already at its clamp -- 0.95, or since #134 the
        // point where a tile beside it reaches `options.minimum` -- does
        // nothing, and does not hand the press to the seam on the other side.
        // That is the behaviour; whether it is the behaviour a user expects is
        // a separate question from whether the comment describes it.
        //
        // `options` is optional because it was not an argument before #134,
        // and a script written then still passes three. Without it there is no
        // minimum and nothing to measure one in, which is the bound this had.
        methods.add_method_mut(
            "resize",
            |_, this, (id, axis, by, options): (u64, String, f64, Option<Table>)| {
                let (area, settings) = match options {
                    Some(options) => {
                        this.0.set_floors(floors_from(&options));
                        (area(&options)?, tuning(&options)?)
                    }
                    None => (Slot::default(), Settings::default()),
                };
                this.0.resize(id, axis_named(&axis), by, area, settings);
                Ok(())
            },
        );

        // The drag path: `edge` is the side the pointer has hold of, which is
        // what the `resize` event hands the script. Not an axis — see
        // `Tiling::drag_seam` for why an axis picks the wrong seam for two of
        // the four sides.
        //
        // `edge_x`/`edge_y` are where that side should come to rest, not where
        // the pointer is, and the two stopped being the same thing in #124.
        // Named for what they are so a script passing the pointer here reads as
        // the mistake it is; `drag_seam` reads only the one `edge` names.
        methods.add_method_mut(
            "drag_seam",
            |_, this, (id, edge, edge_x, edge_y, options): (u64, String, f64, f64, Table)| {
                let Some(edge) = edge_named(&edge) else {
                    // Named rather than guessed at. Falling back to a side
                    // would move a seam the user did not grab, which is the
                    // failure #120 was, and a script with a typo would see it
                    // as the compositor being wrong.
                    tracing::warn!(
                        edge,
                        "not a side a window has; expected left, right, top or bottom"
                    );
                    return Ok(());
                };
                this.0.set_floors(floors_from(&options));
                this.0.drag_seam(
                    id,
                    edge,
                    (edge_x, edge_y),
                    area(&options)?,
                    tuning(&options)?,
                );
                Ok(())
            },
        );

        methods.add_method("contains", |_, this, id: u64| Ok(this.0.contains(id)));

        methods.add_method("windows", |lua, this, ()| {
            let out = lua.create_table()?;
            for (index, id) in this.0.windows().into_iter().enumerate() {
                out.set(index + 1, id)?;
            }
            Ok(out)
        });

        // Rects come back tagged with the window they belong to, because tree
        // order is not the order the script knows its windows in -- and with
        // `cramped = true` on a tile smaller than its window's own floor
        // (#115), which the rect can be handed straight on to `sol.place` with.
        // See `Tiling::cramped`.
        methods.add_method_mut("layout", |lua, this, options: Table| {
            this.0.set_floors(floors_from(&options));
            let out = lua.create_table()?;
            for (index, (id, rect)) in this
                .0
                .layout(area(&options)?, tuning(&options)?)
                .into_iter()
                .enumerate()
            {
                let entry = lua.create_table()?;
                entry.set("id", id)?;
                entry.set("x", rect.x)?;
                entry.set("y", rect.y)?;
                entry.set("w", rect.w)?;
                entry.set("h", rect.h)?;
                if this.0.cramped(id, rect) {
                    entry.set("cramped", true)?;
                }
                out.set(index + 1, entry)?;
            }
            Ok(out)
        });
    }
}

/// Read a list of columns out of a Lua table.
pub(crate) fn columns_from(columns: &Table) -> mlua::Result<Vec<solium_layout::Column>> {
    let mut out = Vec::new();
    for column in columns.sequence_values::<Table>() {
        let column = column?;
        out.push(solium_layout::Column {
            width: column.get::<Option<f64>>("width")?.unwrap_or(0.5),
            windows: column.get::<Option<usize>>("windows")?.unwrap_or(1),
        });
    }
    Ok(out)
}
