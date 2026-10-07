//! A Lua value read whole: a table keeps its list part **and** its named keys,
//! which `Json::from_lua` cannot (`json.rs`), because a rule's effect is
//! `{ "blur", passes = 3 }`.
//! `rules::tests::a_mixed_table_keeps_its_params_beside_the_name`.

use std::collections::BTreeMap;

use solium_effects::spec::Value;

/// A Lua value as plain data: what `effects.rules` is read into before
/// `rules::parse` reads it. `tests::a_table_keeps_its_list_and_its_names`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Tree {
    Bool(bool),
    Number(f64),
    Int(i64),
    Text(String),
    Table {
        list: Vec<Tree>,
        fields: BTreeMap<String, Tree>,
    },
}

impl Tree {
    /// `None` for a function, userdata or a number that is not finite.
    /// `tests::a_function_or_a_number_that_is_not_finite_is_no_tree`.
    pub(crate) fn from_lua(value: &mlua::Value) -> mlua::Result<Option<Self>> {
        Ok(match value {
            mlua::Value::Boolean(yes) => Some(Self::Bool(*yes)),
            mlua::Value::Integer(int) => Some(Self::Int(*int)),
            mlua::Value::Number(number) if number.is_finite() => Some(Self::Number(*number)),
            mlua::Value::String(text) => Some(Self::Text(text.to_str()?.to_owned())),
            mlua::Value::Table(table) => {
                let mut list = Vec::new();
                for item in table.clone().sequence_values::<mlua::Value>() {
                    if let Some(item) = Self::from_lua(&item?)? {
                        list.push(item);
                    }
                }
                let mut fields = BTreeMap::new();
                for pair in table.clone().pairs::<mlua::Value, mlua::Value>() {
                    let (key, value) = pair?;
                    if let mlua::Value::String(key) = key
                        && let Some(value) = Self::from_lua(&value)?
                    {
                        fields.insert(key.to_str()?.to_owned(), value);
                    }
                }
                Some(Self::Table { list, fields })
            }
            _ => None,
        })
    }

    /// A table's named key, `None` for anything else.
    /// `tests::a_table_keeps_its_list_and_its_names`.
    pub(crate) fn field(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Table { fields, .. } => fields.get(key),
            _ => None,
        }
    }

    /// A table's list part, empty for anything else.
    /// `tests::a_table_keeps_its_list_and_its_names`.
    pub(crate) fn list(&self) -> &[Self] {
        match self {
            Self::Table { list, .. } => list,
            _ => &[],
        }
    }

    /// A param value: a number, an integer, a boolean, a word, or four numbers.
    /// `tests::four_numbers_are_a_vec4_and_three_are_not`.
    pub(crate) fn value(&self) -> Option<Value> {
        match self {
            Self::Bool(yes) => Some(Value::Bool(*yes)),
            Self::Int(int) => Some(Value::Int(*int)),
            Self::Number(number) => Some(Value::Number(*number)),
            Self::Text(word) => Some(Value::Word(word.clone())),
            Self::Table { list, fields } if fields.is_empty() && list.len() == 4 => {
                let four: Vec<f64> = list
                    .iter()
                    .filter_map(|each| match each {
                        #[expect(
                            clippy::cast_precision_loss,
                            reason = "a colour or a vector component"
                        )]
                        Self::Int(int) => Some(*int as f64),
                        Self::Number(number) => Some(*number),
                        _ => None,
                    })
                    .collect();
                let [a, b, c, d] = four[..] else {
                    return None;
                };
                Some(Value::Vec4([a, b, c, d]))
            }
            Self::Table { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use solium_effects::spec::Value;

    use super::Tree;

    fn tree(lua: &str) -> Option<Tree> {
        let state = mlua::Lua::new();
        let value: mlua::Value = state.load(lua).eval().expect("the test's Lua");
        Tree::from_lua(&value).expect("readable")
    }

    #[test]
    fn a_table_keeps_its_list_and_its_names() {
        let read = tree(r#"{ "blur", passes = 3, [10] = "a hole", f = function() end }"#)
            .expect("a table");
        assert_eq!(read.list(), [Tree::Text("blur".to_owned())]);
        assert_eq!(read.field("passes"), Some(&Tree::Int(3)));
        assert_eq!(read.field("f"), None, "a function is no tree");
        assert_eq!(Tree::Int(3).field("passes"), None);
        assert!(Tree::Int(3).list().is_empty());
    }

    #[test]
    fn a_function_or_a_number_that_is_not_finite_is_no_tree() {
        assert_eq!(tree("function() end"), None);
        assert_eq!(tree("1/0"), None);
        assert_eq!(tree("0/0"), None);
        assert_eq!(tree("1.5"), Some(Tree::Number(1.5)));
    }

    #[test]
    fn four_numbers_are_a_vec4_and_three_are_not() {
        let value = |lua: &str| tree(lua).and_then(|read| read.value());
        assert_eq!(
            value("{ 1, 0.5, 0, 1 }"),
            Some(Value::Vec4([1.0, 0.5, 0.0, 1.0]))
        );
        assert_eq!(value("{ 1, 0.5, 0 }"), None);
        assert_eq!(value("{ 1, 0.5, 0, 'one' }"), None);
        assert_eq!(value("{ 1, 0.5, 0, 1, x = 2 }"), None);
        assert_eq!(value("true"), Some(Value::Bool(true)));
        assert_eq!(value("'soft'"), Some(Value::Word("soft".to_owned())));
    }
}
