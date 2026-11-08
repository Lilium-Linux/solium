//! Values between Lua and the rest of `script/`, which knows no Lua.

use mlua::{IntoLua, Lua, Table, Value};

use crate::script::doc::{self, Doc};
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

/// What a warning shows of a Lua value that was not what a setting wanted.
/// The same text [`crate::script::doc::describe`] gives the value's
/// document: `doc::tests::describe_says_what_the_lua_describe_says`.
pub(crate) fn describe_value(value: &Value) -> String {
    match value {
        Value::Integer(number) => number.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.to_string_lossy(),
        other => format!("<{}>", other.type_name()),
    }
}

/// How deep a settings table is followed before it is taken for a ring: no
/// setting is nested this far, and a table holding itself must be an error,
/// not a stack that runs out.
const DOC_DEPTH: usize = 32;

/// A Lua value as a settings document.
///
/// A table's items keyed 1 to n are its list; its other whole-number keys
/// are `numbered`, its string keys `fields`, and keys of any other type are
/// not read. A function, userdata or thread is kept as [`Doc::Other`] by its
/// type name, so a parser can say what was written where it wanted something
/// else.
pub(crate) fn doc_of(value: &Value) -> mlua::Result<Doc> {
    doc_at(value, 0)
}

/// A Lua value as a plain document: [`Doc::plain`] of [`doc_of`], what the
/// effect parsers read.
pub(crate) fn doc_of_plain(value: &Value) -> mlua::Result<Option<Doc>> {
    doc_of(value).map(|doc| doc.plain())
}

/// A settings section as its Lua function takes it: a table, or nothing,
/// which reads as an empty section, every key at its default.
pub(crate) fn section(options: Option<Table>) -> mlua::Result<Doc> {
    options.map_or(Ok(Doc::table()), |table| doc_of(&Value::Table(table)))
}

fn doc_at(value: &Value, depth: usize) -> mlua::Result<Doc> {
    Ok(match value {
        Value::Nil => Doc::Nil,
        Value::Boolean(on) => Doc::Bool(*on),
        Value::Integer(whole) => Doc::Int(*whole),
        Value::Number(fraction) => Doc::Num(*fraction),
        Value::String(text) => Doc::Text(text.to_string_lossy()),
        Value::Table(table) => {
            if depth >= DOC_DEPTH {
                return Err(mlua::Error::runtime(format!(
                    "a table nested deeper than {DOC_DEPTH}: one that holds itself?"
                )));
            }
            let mut read = doc::Table::default();
            for pair in table.pairs::<Value, Value>() {
                let (key, value) = pair?;
                let value = doc_at(&value, depth + 1)?;
                match key {
                    Value::Integer(at) => {
                        read.numbered.insert(at, value);
                    }
                    Value::String(name) => {
                        read.fields.insert(name.to_string_lossy(), value);
                    }
                    _ => {}
                }
            }
            let mut next = 1;
            while let Some(item) = read.numbered.remove(&next) {
                read.list.push(item);
                next += 1;
            }
            Doc::Table(read)
        }
        other => Doc::Other(other.type_name()),
    })
}
