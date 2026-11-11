//! A Lua value as a settings document, for the Lua the compositor runs
//! itself: an effect's or a style's. A window manager sends its documents
//! already made.

use mlua::Value;

use crate::script::{Doc, DocTable};

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
            let mut read = DocTable::default();
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

#[cfg(test)]
mod tests {
    use super::doc_of;

    /// A Lua value read straight into a document, and sent as a wire
    /// document and read back, is the same document: what a window manager
    /// sends is what the compositor's own Lua reads.
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
            let wire = crate::script::lua::convert::wire_doc_of(&value).expect("reads");
            assert_eq!(crate::wm::convert::doc(&wire), Ok(direct), "{source}");
        }
    }
}
