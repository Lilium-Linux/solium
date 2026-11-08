//! Values between Lua and the rest of `script/`, which knows no Lua.

use mlua::{IntoLua, Lua, Table, Value};

use crate::script::Kept;
use crate::script::doc::{self, Doc};
use crate::script::lua::*;
use crate::script::*;

/// A rectangle as a Lua table: `{ x, y, w, h }`.
pub(crate) fn rect_table(rect: &solium_wm_protocol::Rect, lua: &Lua) -> mlua::Result<Table> {
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
pub(crate) fn parentage_value(
    parentage: &solium_wm_protocol::Parentage,
    lua: &Lua,
) -> mlua::Result<Value> {
    use solium_wm_protocol::Parentage;
    match parentage {
        Parentage::None => Ok(Value::Nil),
        Parentage::Unknown => Ok(Value::Boolean(false)),
        Parentage::Window(id) => id.into_lua(lua),
    }
}

/// A document from the compositor, a service's view, as the Lua table its
/// getter answers: a list part, named and numbered keys, whole numbers as
/// integers.
pub(crate) fn doc_to_lua(nodes: &solium_wm_protocol::Doc, lua: &Lua) -> mlua::Result<Value> {
    node_to_lua(nodes, 0, 0, lua)
}

fn node_to_lua(
    nodes: &solium_wm_protocol::Doc,
    at: u32,
    depth: usize,
    lua: &Lua,
) -> mlua::Result<Value> {
    use solium_wm_protocol::DocNode;
    if depth > DOC_DEPTH {
        return Err(mlua::Error::runtime(format!(
            "a view nested deeper than {DOC_DEPTH}"
        )));
    }
    let Some(node) = nodes.get(at as usize) else {
        return Ok(Value::Nil);
    };
    Ok(match node {
        DocNode::Nil | DocNode::Other(_) => Value::Nil,
        DocNode::Boolean(on) => Value::Boolean(*on),
        DocNode::Integer(whole) => Value::Integer(*whole),
        DocNode::Number(number) => Value::Number(*number),
        DocNode::Text(text) => Value::String(lua.create_string(text)?),
        DocNode::Table(table) => {
            let read = lua.create_table()?;
            for (index, item) in table.list.iter().enumerate() {
                read.set(index + 1, node_to_lua(nodes, *item, depth + 1, lua)?)?;
            }
            for (key, item) in &table.numbered {
                read.set(*key, node_to_lua(nodes, *item, depth + 1, lua)?)?;
            }
            for (name, item) in &table.fields {
                read.set(name.as_str(), node_to_lua(nodes, *item, depth + 1, lua)?)?;
            }
            Value::Table(read)
        }
    })
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
        // A document's text is UTF-8, as a WIT string is; text that is not
        // is named, so the setting it was written for says so and keeps its
        // default.
        Value::String(text) => match text.to_str() {
            Ok(text) => Doc::Text(text.to_owned()),
            Err(_) => Doc::Other("string that is not UTF-8"),
        },
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

impl Kept {
    /// Read a Lua value, or say why it cannot be kept.
    ///
    /// `where_it_is` is the path from the keep's name down to this value, so a
    /// warning names the key someone can go and look at rather than saying
    /// "something in your table".
    pub(crate) fn read(value: &Value, depth: usize, where_it_is: &str) -> Option<Self> {
        match value {
            Value::Boolean(flag) => Some(Self::Bool(*flag)),
            Value::Integer(number) => Some(Self::Int(*number)),
            Value::Number(number) => Some(Self::Number(*number)),
            Value::String(text) => text.to_str().ok().map(|text| Self::Text(text.to_owned())),
            Value::Table(table) => {
                if depth >= KEEP_DEPTH {
                    tracing::warn!(
                        key = where_it_is,
                        depth = KEEP_DEPTH,
                        "a kept table is deeper than the host will follow; this branch of it \
                         will not survive the next reload"
                    );
                    return None;
                }
                let mut pairs = Vec::new();
                for (key, value) in table.pairs::<Value, Value>().flatten() {
                    let named = format!("{where_it_is}.{}", describe(&key));
                    let (Some(key), Some(value)) = (
                        Self::read(&key, depth + 1, &named),
                        Self::read(&value, depth + 1, &named),
                    ) else {
                        continue;
                    };
                    pairs.push((key, value));
                }
                Some(Self::Table(pairs))
            }
            // A copy, so the running scripts are left as they were: see
            // `Scripts::kept` for why reading the keep must not damage them,
            // and `a_reload_leaves_a_tiled_desk_exactly_as_it_was`.
            Value::UserData(held) if held.is::<TilingTree>() => held
                .borrow::<TilingTree>()
                .ok()
                .map(|tree| Self::Tree(tree.0.clone())),
            Value::UserData(held) if held.is::<Scrolling>() => held
                .borrow::<Scrolling>()
                .ok()
                .map(|strip| Self::Strip(strip.0.clone())),
            // `nil` is not a failure: a key that has been cleared is a key that
            // is not there, and Lua's own iteration never yields one.
            Value::Nil => None,
            other => {
                tracing::warn!(
                    key = where_it_is,
                    kind = other.type_name(),
                    "only plain data and the layouts sol.layout makes survive a reload, and this \
                     is neither; it will be missing from the keep when the configuration is read \
                     again"
                );
                None
            }
        }
    }

    /// Build it again, in the Lua state that replaced the one it came from.
    pub(crate) fn into_value(self, lua: &Lua) -> mlua::Result<Value> {
        Ok(match self {
            Self::Bool(flag) => Value::Boolean(flag),
            Self::Int(number) => Value::Integer(number),
            Self::Number(number) => Value::Number(number),
            Self::Text(text) => Value::String(lua.create_string(&text)?),
            Self::Table(pairs) => {
                let table = lua.create_table()?;
                for (key, value) in pairs {
                    table.set(key.into_value(lua)?, value.into_value(lua)?)?;
                }
                Value::Table(table)
            }
            Self::Tree(tree) => Value::UserData(lua.create_userdata(TilingTree(tree))?),
            Self::Strip(strip) => Value::UserData(lua.create_userdata(Scrolling(strip))?),
        })
    }
}

/// A Lua value as the protocol's settings document: its nodes, the first
/// the root, a table's children by index. The same reading as [`doc_of`]:
/// a list part keyed 1 to n, named and numbered keys, other keys not read,
/// a function and the like named by its type, and a table nested past 32
/// refused.
pub(crate) fn wire_doc_of(value: &Value) -> mlua::Result<solium_wm_protocol::Doc> {
    let mut nodes = Vec::new();
    wire_node(value, 0, &mut nodes)?;
    Ok(nodes)
}

/// A settings section as its Lua function takes it, as a document: a table,
/// or nothing, which reads as an empty section.
pub(crate) fn wire_section(options: Option<Table>) -> mlua::Result<solium_wm_protocol::Doc> {
    wire_doc_of(&options.map_or(Value::Nil, Value::Table)).map(|nodes| {
        if matches!(nodes.first(), Some(solium_wm_protocol::DocNode::Nil)) {
            vec![solium_wm_protocol::DocNode::Table(
                solium_wm_protocol::DocTable {
                    list: Vec::new(),
                    fields: Vec::new(),
                    numbered: Vec::new(),
                },
            )]
        } else {
            nodes
        }
    })
}

fn wire_node(
    value: &Value,
    depth: usize,
    nodes: &mut Vec<solium_wm_protocol::DocNode>,
) -> mlua::Result<u32> {
    use solium_wm_protocol::{DocNode, DocTable};
    let at = u32::try_from(nodes.len())
        .map_err(|_| mlua::Error::runtime("a settings document of more than 2^32 values"))?;
    let node = match value {
        Value::Nil => DocNode::Nil,
        Value::Boolean(on) => DocNode::Boolean(*on),
        Value::Integer(whole) => DocNode::Integer(*whole),
        Value::Number(fraction) => DocNode::Number(*fraction),
        Value::String(text) => match text.to_str() {
            Ok(text) => DocNode::Text(text.to_owned()),
            Err(_) => DocNode::Other("string that is not UTF-8".to_owned()),
        },
        Value::Table(table) => {
            if depth >= DOC_DEPTH {
                return Err(mlua::Error::runtime(format!(
                    "a table nested deeper than {DOC_DEPTH}: one that holds itself?"
                )));
            }
            // Its place is taken first, so its children come after it.
            nodes.push(DocNode::Nil);
            let mut numbered = std::collections::BTreeMap::new();
            let mut fields = std::collections::BTreeMap::new();
            for pair in table.pairs::<Value, Value>() {
                let (key, value) = pair?;
                match key {
                    Value::Integer(key) => {
                        numbered.insert(key, value);
                    }
                    Value::String(name) => {
                        fields.insert(name.to_string_lossy(), value);
                    }
                    _ => {}
                }
            }
            let mut read = DocTable {
                list: Vec::new(),
                fields: Vec::new(),
                numbered: Vec::new(),
            };
            let mut next = 1;
            while let Some(item) = numbered.remove(&next) {
                read.list.push(wire_node(&item, depth + 1, nodes)?);
                next += 1;
            }
            for (key, item) in numbered {
                read.numbered
                    .push((key, wire_node(&item, depth + 1, nodes)?));
            }
            for (name, item) in fields {
                read.fields
                    .push((name, wire_node(&item, depth + 1, nodes)?));
            }
            nodes[at as usize] = DocNode::Table(read);
            return Ok(at);
        }
        other => DocNode::Other(other.type_name().to_owned()),
    };
    nodes.push(node);
    Ok(at)
}

#[cfg(test)]
mod tests {
    use super::{doc_of, wire_doc_of};

    /// A Lua value read straight into a document, and read as a wire
    /// document and back, is the same document: what a window manager
    /// sends is what an in-process configuration was read as.
    #[test]
    fn a_wire_document_reads_back_as_the_document_lua_makes() {
        let lua = mlua::Lua::new();
        for source in [
            "nil",
            "true",
            "7",
            "7.5",
            "1/0",
            "'text'",
            "{}",
            "{ 1, 2, 3 }",
            r#"{ "blur", passes = 3, [10] = "a hole", f = function() end }"#,
            "{ timeouts = { low = 5000, normal = 7000 }, history = { max = 200 } }",
            "{ devices = { { name = 'Mouse', vendor = 0x046d, tap = true } } }",
            "{ [0] = 'zero', [-1] = 'minus', { 'nested', { deeper = { 1 } } } }",
        ] {
            let value: mlua::Value = lua.load(source).eval().expect("the test's Lua");
            let direct = doc_of(&value).expect("reads");
            let wire = wire_doc_of(&value).expect("reads");
            assert_eq!(crate::wm::convert::doc(&wire), Ok(direct), "{source}");
        }
    }

    #[test]
    fn a_table_that_holds_itself_is_refused_on_the_wire_too() {
        let lua = mlua::Lua::new();
        let value: mlua::Value = lua
            .load("local t = {} t.again = t return t")
            .eval()
            .expect("the test's Lua");
        assert!(wire_doc_of(&value).is_err());
    }
}
