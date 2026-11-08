//! Values between Lua and the rest of `script/`, which knows no Lua.

use mlua::{IntoLua, Lua, Table, Value};

use crate::script::{Parentage, Rect};

/// A rectangle as a Lua table: `{ x, y, w, h }`.
pub(crate) fn rect_table(rect: Rect, lua: &Lua) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("x", rect.x)?;
    table.set("y", rect.y)?;
    table.set("w", rect.w)?;
    table.set("h", rect.h)?;
    Ok(table)
}

/// How a script sees a window's parent.
///
/// A number when the parent can be pointed at, `false` when one was named
/// and cannot be, and `nil` when there is none. Chosen so that the common
/// question — "have I got a window to centre on?" — is `if window.parent
/// then`, which is correct for all three, while a script that wants to
/// distinguish "named but lost" from "never named" can still ask
/// `window.parent == false`. An `Option<u64>` flattened to nil-or-number
/// would have thrown the distinction away at the boundary.
pub(crate) fn parentage_value(parentage: Parentage, lua: &Lua) -> mlua::Result<Value> {
    match parentage {
        Parentage::None => Ok(Value::Nil),
        Parentage::Unknown => Ok(Value::Boolean(false)),
        Parentage::Window(id) => id.into_lua(lua),
    }
}
