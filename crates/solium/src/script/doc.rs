//! A setting as written, in no language.
//!
//! A section of settings (`sol.idle{...}`, `sol.effects{...}`) arrives as a
//! tree of plain values, and its parser reads that tree, not a Lua table: the
//! same parser serves a configuration written in Lua and a window manager
//! written in anything else that sends the same document. Effect rules are
//! read from it too, which is why a table keeps its list part beside its
//! named keys: a rule's effect is `{ "blur", passes = 3 }`.

use std::collections::BTreeMap;

/// One value of a settings document.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Doc {
    /// Nothing written. Never inside a table, which cannot hold it.
    Nil,
    Bool(bool),
    /// Kept apart from `Num`, because "a whole number" is a type some
    /// settings ask for, and `7` and `7.0` are different answers to it.
    Int(i64),
    /// Any number that is not an integer, infinities and NaN included, so a
    /// parser can refuse `1/0` by its bounds rather than never see it.
    Num(f64),
    Text(String),
    Table(Table),
    /// Something a setting cannot be, such as a function, by the name its
    /// language gives it, so a warning can say what was written.
    Other(&'static str),
}

/// A table: its list part, the items keyed 1 to n, and the rest by key.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Table {
    pub(crate) list: Vec<Doc>,
    /// Named keys, in order, so what is read from them is said in an order
    /// that does not change between runs.
    pub(crate) fields: BTreeMap<String, Doc>,
    /// Whole-number keys outside the list part: `[10] = "a hole"`.
    pub(crate) numbered: BTreeMap<i64, Doc>,
}

const NIL: Doc = Doc::Nil;

impl Doc {
    /// An empty table: a section written with nothing in it.
    pub(crate) fn table() -> Self {
        Self::Table(Table::default())
    }

    /// The value under a named key, or `Nil` when there is none, or when
    /// this is not a table at all: the same answer a Lua table gives.
    pub(crate) fn get(&self, key: &str) -> &Doc {
        self.field(key).unwrap_or(&NIL)
    }

    /// A table's named key, `None` when absent or not a table.
    pub(crate) fn field(&self, key: &str) -> Option<&Doc> {
        match self {
            Self::Table(table) => table.fields.get(key),
            _ => None,
        }
    }

    /// A table's named key, to change, `None` when absent or not a table.
    pub(crate) fn field_mut(&mut self, key: &str) -> Option<&mut Doc> {
        match self {
            Self::Table(table) => table.fields.get_mut(key),
            _ => None,
        }
    }

    /// A table's list part, empty for anything else.
    pub(crate) fn list(&self) -> &[Doc] {
        match self {
            Self::Table(table) => &table.list,
            _ => &[],
        }
    }

    pub(crate) fn is_nil(&self) -> bool {
        matches!(self, Self::Nil)
    }

    /// A number, whole or not.
    pub(crate) fn number(&self) -> Option<f64> {
        match self {
            #[expect(
                clippy::cast_precision_loss,
                reason = "a setting written as a whole number: 1, not 2^53"
            )]
            Self::Int(whole) => Some(*whole as f64),
            Self::Num(fraction) => Some(*fraction),
            _ => None,
        }
    }

    /// The name of its type, as a configuration's author would say it.
    pub(crate) fn type_name(&self) -> &'static str {
        match self {
            Self::Nil => "nil",
            Self::Bool(_) => "boolean",
            Self::Int(_) => "integer",
            Self::Num(_) => "number",
            Self::Text(_) => "string",
            Self::Table(_) => "table",
            Self::Other(name) => name,
        }
    }

    /// Only what a setting can be: functions and the like dropped, and
    /// numbers that are not finite too, wherever they are; a list closes up
    /// over what it lost. `None` when this value itself is dropped, or nil.
    ///
    /// What the effect parsers read, which take an absent value for its
    /// default; a section that refuses `1/0` by its bounds reads the
    /// document whole instead.
    pub(crate) fn plain(&self) -> Option<Doc> {
        match self {
            Self::Nil | Self::Other(_) => None,
            Self::Num(number) if !number.is_finite() => None,
            Self::Table(table) => Some(Self::Table(Table {
                list: table.list.iter().filter_map(Self::plain).collect(),
                fields: table
                    .fields
                    .iter()
                    .filter_map(|(key, value)| Some((key.clone(), value.plain()?)))
                    .collect(),
                numbered: table
                    .numbered
                    .iter()
                    .filter_map(|(key, value)| Some((*key, value.plain()?)))
                    .collect(),
            })),
            other => Some(other.clone()),
        }
    }

    /// Put back into this plain table the named numbers that are not finite
    /// in `whole`, the document it was made from: for a section that refuses
    /// `max_ms = 1/0` by its bounds rather than reading it as the default.
    pub(crate) fn keep_non_finite_fields(&mut self, whole: &Doc) {
        if let (Self::Table(kept), Self::Table(whole)) = (self, whole) {
            for (name, value) in &whole.fields {
                if matches!(value, Self::Num(number) if !number.is_finite()) {
                    kept.fields.insert(name.clone(), value.clone());
                }
            }
        }
    }

    /// Put back into each list under a named key of this plain table the
    /// numbers that are not finite in `whole`, in their places.
    pub(crate) fn keep_non_finite_items(&mut self, whole: &Doc) {
        let (Self::Table(kept), Self::Table(whole)) = (self, whole) else {
            return;
        };
        for (name, value) in &mut kept.fields {
            if let (Self::Table(list), Some(Self::Table(items))) = (value, whole.fields.get(name)) {
                list.list = items
                    .list
                    .iter()
                    .filter_map(|item| match item {
                        Self::Num(number) if !number.is_finite() => Some(item.clone()),
                        item => item.plain(),
                    })
                    .collect();
            }
        }
    }
}

/// What a warning shows of a value that was not what a setting wanted: a
/// number or text as written, anything else by its type, in angle brackets.
pub(crate) fn describe(doc: &Doc) -> String {
    match doc {
        Doc::Int(number) => number.to_string(),
        Doc::Num(number) => number.to_string(),
        Doc::Text(text) => text.clone(),
        other => format!("<{}>", other.type_name()),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{Doc, describe};
    use crate::script::lua::convert::{describe_value, doc_of};

    /// `lua`, evaluated, as a document.
    pub(crate) fn doc(lua: &str) -> Doc {
        let state = mlua::Lua::new();
        let value: mlua::Value = state.load(lua).eval().expect("the test's Lua");
        doc_of(&value).expect("converts")
    }

    /// `lua`, evaluated, as a plain document: what the effect parsers read.
    pub(crate) fn plain(lua: &str) -> Option<Doc> {
        doc(lua).plain()
    }

    #[test]
    fn describe_says_what_the_lua_describe_says() {
        let state = mlua::Lua::new();
        for source in [
            "nil",
            "true",
            "7",
            "7.5",
            "7.0",
            "'x'",
            "{1, 2}",
            "{a = 1}",
            "{}",
            "function() end",
        ] {
            let value: mlua::Value = state.load(source).eval().expect("the test's Lua");
            let read = doc_of(&value).expect("converts");
            assert_eq!(describe(&read), describe_value(&value), "{source}");
        }
    }

    #[test]
    fn a_table_keeps_its_list_its_names_and_its_numbered_keys() {
        let read = doc(r#"{ "blur", passes = 3, [10] = "a hole", f = function() end }"#);
        assert_eq!(read.list(), [Doc::Text("blur".to_owned())]);
        assert_eq!(read.field("passes"), Some(&Doc::Int(3)));
        assert_eq!(read.field("f"), Some(&Doc::Other("function")));
        let Doc::Table(table) = &read else {
            panic!("a table: {read:?}");
        };
        assert_eq!(
            table.numbered.get(&10),
            Some(&Doc::Text("a hole".to_owned()))
        );
        assert_eq!(Doc::Int(3).field("passes"), None);
        assert!(Doc::Int(3).list().is_empty());
        assert!(Doc::Int(3).get("passes").is_nil());
    }

    #[test]
    fn an_integer_stays_an_integer_and_a_float_a_float() {
        assert_eq!(doc("7"), Doc::Int(7));
        assert_eq!(doc("7.0"), Doc::Num(7.0));
    }

    #[test]
    fn a_number_that_is_not_finite_is_kept_and_plain_drops_it() {
        assert!(matches!(doc("1/0"), Doc::Num(n) if n.is_infinite()));
        assert_eq!(plain("1/0"), None);
        assert_eq!(plain("0/0"), None);
        assert_eq!(plain("function() end"), None);
        assert_eq!(plain("1.5"), Some(Doc::Num(1.5)));
        assert_eq!(
            plain("{ 1, 1/0, 2, f = function() end, g = 0/0, h = 3 }"),
            Some(doc("{ 1, 2, h = 3 }")),
            "a list closes up over what it lost"
        );
    }

    #[test]
    fn a_table_that_holds_itself_is_an_error_not_a_crash() {
        let state = mlua::Lua::new();
        let value: mlua::Value = state
            .load("local t = {} t.again = t return t")
            .eval()
            .expect("the test's Lua");
        let error = doc_of(&value).expect_err("a ring cannot be a document");
        assert!(error.to_string().contains("deeper than"), "{error}");
    }
}
