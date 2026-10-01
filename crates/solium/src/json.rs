//! JSON values, for what crosses between Lua, Rust and QML.
//!
//! The crate has no JSON dependency, and what crosses is small: a surface's
//! properties, a scene's action data, a model's rows. This is the one type for
//! all of it, written by hand and, from Task 10, read by hand.

use std::collections::BTreeMap;

/// A JSON value. An object's keys are sorted, so equal objects render equal
/// text: `tests::an_object_renders_its_keys_in_order`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Json {
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "JSON's null; nothing outside the tests writes one until a \
                      scene's action data is read"
        )
    )]
    Null,
    Bool(bool),
    Number(f64),
    Text(String),
    List(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

impl Json {
    /// This value as JSON text.
    pub(crate) fn render(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(yes) => out.push_str(if *yes { "true" } else { "false" }),
            Self::Number(number) => out.push_str(&render_number(*number)),
            Self::Text(text) => out.push_str(&crate::scripted::json_string(text)),
            Self::List(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Self::Object(fields) => {
                out.push('{');
                for (index, (key, value)) in fields.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push_str(&crate::scripted::json_string(key));
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }

    /// A Lua value as JSON, or `None` for something JSON has no word for: a
    /// function, userdata, or a number that is not finite.
    /// `tests::a_lua_table_becomes_an_object_or_a_list`.
    pub(crate) fn from_lua(value: &mlua::Value) -> mlua::Result<Option<Self>> {
        Ok(match value {
            mlua::Value::String(text) => Some(Self::Text(text.to_str()?.to_owned())),
            #[expect(
                clippy::cast_precision_loss,
                reason = "a property or an id, far below 2^53"
            )]
            mlua::Value::Integer(number) => Some(Self::Number(*number as f64)),
            mlua::Value::Number(number) if number.is_finite() => Some(Self::Number(*number)),
            mlua::Value::Boolean(yes) => Some(Self::Bool(*yes)),
            mlua::Value::Table(table) => {
                let items = table
                    .clone()
                    .sequence_values::<mlua::Value>()
                    .filter_map(|item| {
                        item.ok()
                            .and_then(|item| Self::from_lua(&item).ok().flatten())
                    })
                    .collect::<Vec<_>>();
                if items.is_empty() {
                    let mut fields = BTreeMap::new();
                    for pair in table.pairs::<String, mlua::Value>() {
                        let (key, value) = pair?;
                        if let Some(value) = Self::from_lua(&value)? {
                            fields.insert(key, value);
                        }
                    }
                    Some(Self::Object(fields))
                } else {
                    Some(Self::List(items))
                }
            }
            _ => None,
        })
    }
}

/// A whole number without a fraction, as Lua wrote it: `48`, not `48.0`.
/// `tests::an_integral_number_renders_without_a_fraction`.
fn render_number(value: f64) -> String {
    const EXACT: f64 = 9_007_199_254_740_992.0;
    if value.fract() == 0.0 && value.abs() < EXACT {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "integral, and inside f64's exact range"
        )]
        let whole = value as i64;
        whole.to_string()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::Json;

    #[test]
    fn an_object_renders_its_keys_in_order() {
        let value = Json::Object(BTreeMap::from([
            ("b".to_owned(), Json::Bool(true)),
            (
                "a".to_owned(),
                Json::List(vec![Json::Null, Json::Text("q\"r".to_owned())]),
            ),
        ]));
        assert_eq!(value.render(), r#"{"a":[null,"q\"r"],"b":true}"#);
    }

    #[test]
    fn an_integral_number_renders_without_a_fraction() {
        assert_eq!(Json::Number(48.0).render(), "48");
        assert_eq!(Json::Number(-3.0).render(), "-3");
        assert_eq!(Json::Number(0.25).render(), "0.25");
    }

    #[test]
    fn a_lua_table_becomes_an_object_or_a_list() {
        let lua = mlua::Lua::new();
        let value: mlua::Value = lua
            .load(r#"return { name = "bar", sizes = { 1, 2 }, nested = { y = 1, x = "s" }, gone = function() end }"#)
            .eval()
            .expect("the table evaluates");
        let json = Json::from_lua(&value)
            .expect("converts")
            .expect("a table is a value");
        assert_eq!(
            json.render(),
            r#"{"name":"bar","nested":{"x":"s","y":1},"sizes":[1,2]}"#,
            "a function is left out, a sequence is a list, and keys are sorted"
        );
    }
}
