//! JSON values, for what crosses between Lua, Rust and QML.
//!
//! The crate has no JSON dependency, and what crosses is small: a surface's
//! properties, a scene's action data, a model's rows. This is the one type for
//! all of it, written and read by hand.

use std::collections::BTreeMap;

/// A JSON value. An object's keys are sorted, so equal objects render equal
/// text: `tests::an_object_renders_its_keys_in_order`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Json {
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
            Self::Object(fields) => write_object(fields, out),
        }
    }

    /// A Lua value as JSON, or `None` for something JSON has no word for: a
    /// function, userdata, or a number that is not finite.
    /// `tests::a_lua_table_becomes_an_object_or_a_list`.
    pub(crate) fn from_lua(value: &mlua::Value) -> mlua::Result<Option<Self>> {
        Self::from_lua_inside(value, 0)
    }

    /// [`Self::from_lua`] for a value inside `depth` tables. A table nested
    /// deeper than JSON text may be is an error, and so is a table that
    /// contains itself, which is nested without end: the walk is Rust, which
    /// the handler deadline cannot stop, so it must not run the stack out.
    /// An error in an item fails the whole value, as one in a field does, so
    /// a table that is its own item ends at the first error too.
    /// `tests::a_table_that_contains_itself_is_an_error_not_a_crash`,
    /// `tests::a_table_64_deep_is_read_and_65_deep_is_an_error`.
    fn from_lua_inside(value: &mlua::Value, depth: usize) -> mlua::Result<Option<Self>> {
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
                let depth = depth + 1;
                if depth > Reader::DEEPEST {
                    return Err(mlua::Error::runtime(
                        "nested deeper than 64, or a table that contains itself",
                    ));
                }
                let mut items = Vec::new();
                for item in table.clone().sequence_values::<mlua::Value>() {
                    let Ok(item) = item else { continue };
                    if let Some(item) = Self::from_lua_inside(&item, depth)? {
                        items.push(item);
                    }
                }
                if items.is_empty() {
                    Some(Self::Object(Self::fields_from_lua(table, depth)?))
                } else {
                    Some(Self::List(items))
                }
            }
            _ => None,
        })
    }

    /// JSON text as a value, or `None` for anything that is not exactly one
    /// value. Nesting deeper than 64 is refused, so nothing a scene sends can
    /// run the stack out.
    /// `tests::json_reads_what_qt_writes`, `tests::what_is_not_json_is_none`,
    /// `tests::json_nested_too_deep_is_none`.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let mut reader = Reader {
            bytes: text.as_bytes(),
            at: 0,
        };
        let value = reader.value(0)?;
        reader.space();
        (reader.at == reader.bytes.len()).then_some(value)
    }

    /// An object's field. `tests::json_reads_what_qt_writes`.
    pub(crate) fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Object(fields) => fields.get(key),
            _ => None,
        }
    }

    /// A whole, non-negative number below 2^53: a window's id.
    /// `tests::only_a_whole_non_negative_number_is_an_id`.
    pub(crate) fn as_u64(&self) -> Option<u64> {
        const EXACT: f64 = 9_007_199_254_740_992.0;
        match self {
            Self::Number(number) if *number >= 0.0 && number.fract() == 0.0 && *number < EXACT => {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "whole, non-negative and inside f64's exact range"
                )]
                let whole = *number as u64;
                Some(whole)
            }
            _ => None,
        }
    }

    /// This value in Lua: an object a table by key, a list a sequence, null
    /// `nil`, and a whole number an integer. `tests::a_value_reaches_lua_as_a_table`.
    pub(crate) fn to_lua(&self, lua: &mlua::Lua) -> mlua::Result<mlua::Value> {
        const EXACT: f64 = 9_007_199_254_740_992.0;
        Ok(match self {
            Self::Null => mlua::Value::Nil,
            Self::Bool(yes) => mlua::Value::Boolean(*yes),
            Self::Number(number) if number.fract() == 0.0 && number.abs() < EXACT => {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "whole, and inside f64's exact range"
                )]
                let whole = *number as i64;
                mlua::Value::Integer(whole)
            }
            Self::Number(number) => mlua::Value::Number(*number),
            Self::Text(text) => mlua::Value::String(lua.create_string(text)?),
            Self::List(items) => {
                let table = lua.create_table()?;
                for (index, item) in items.iter().enumerate() {
                    table.set(index + 1, item.to_lua(lua)?)?;
                }
                mlua::Value::Table(table)
            }
            Self::Object(fields) => {
                let table = lua.create_table()?;
                for (key, value) in fields {
                    table.set(key.as_str(), value.to_lua(lua)?)?;
                }
                mlua::Value::Table(table)
            }
        })
    }

    /// A Lua table's every key as an object's field, whether or not it also
    /// has a list part, which is how a surface's top-level `properties` is
    /// read: `crate::script::tests::a_surfaces_properties_keep_their_named_keys_beside_a_list_part`.
    /// The table is the first of the 64 its values may be nested in:
    /// `crate::script::tests::data_that_contains_itself_is_an_error_in_the_handler`.
    pub(crate) fn object_from_lua(table: &mlua::Table) -> mlua::Result<BTreeMap<String, Self>> {
        Self::fields_from_lua(table, 1)
    }

    /// [`Self::object_from_lua`] for a table inside `depth - 1` others.
    /// `tests::a_table_64_deep_is_read_and_65_deep_is_an_error`.
    fn fields_from_lua(table: &mlua::Table, depth: usize) -> mlua::Result<BTreeMap<String, Self>> {
        let mut fields = BTreeMap::new();
        for pair in table.pairs::<String, mlua::Value>() {
            let (key, value) = pair?;
            if let Some(value) = Self::from_lua_inside(&value, depth)? {
                fields.insert(key, value);
            }
        }
        Ok(fields)
    }
}

/// A cursor over JSON text. `tests::json_reads_what_qt_writes`.
#[derive(Debug)]
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    /// How deep a value may nest, as JSON text or as a Lua table.
    /// `tests::json_nested_too_deep_is_none`,
    /// `tests::a_table_64_deep_is_read_and_65_deep_is_an_error`.
    const DEEPEST: usize = 64;

    fn space(&mut self) {
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> Option<()> {
        self.space();
        (self.bytes.get(self.at) == Some(&byte)).then(|| self.at += 1)
    }

    fn word(&mut self, word: &str) -> Option<()> {
        let end = self.at + word.len();
        (self.bytes.get(self.at..end) == Some(word.as_bytes())).then(|| self.at = end)
    }

    fn value(&mut self, depth: usize) -> Option<Json> {
        if depth > Self::DEEPEST {
            return None;
        }
        self.space();
        match self.bytes.get(self.at)? {
            b'n' => self.word("null").map(|()| Json::Null),
            b't' => self.word("true").map(|()| Json::Bool(true)),
            b'f' => self.word("false").map(|()| Json::Bool(false)),
            b'"' => self.text().map(Json::Text),
            b'[' => {
                self.at += 1;
                let mut items = Vec::new();
                if self.eat(b']').is_some() {
                    return Some(Json::List(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    if self.eat(b']').is_some() {
                        return Some(Json::List(items));
                    }
                    self.eat(b',')?;
                }
            }
            b'{' => {
                self.at += 1;
                let mut fields = BTreeMap::new();
                if self.eat(b'}').is_some() {
                    return Some(Json::Object(fields));
                }
                loop {
                    self.space();
                    let key = self.text()?;
                    self.eat(b':')?;
                    fields.insert(key, self.value(depth + 1)?);
                    if self.eat(b'}').is_some() {
                        return Some(Json::Object(fields));
                    }
                    self.eat(b',')?;
                }
            }
            _ => self.number(),
        }
    }

    fn number(&mut self) -> Option<Json> {
        let start = self.at;
        while self
            .bytes
            .get(self.at)
            .is_some_and(|byte| matches!(byte, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'))
        {
            self.at += 1;
        }
        std::str::from_utf8(self.bytes.get(start..self.at)?)
            .ok()?
            .parse::<f64>()
            .ok()
            .filter(|number| number.is_finite())
            .map(Json::Number)
    }

    /// A string, its escapes read, a surrogate pair as the one character it
    /// is and half of one refused. `tests::json_reads_escapes_and_surrogate_pairs`.
    fn text(&mut self) -> Option<String> {
        if self.bytes.get(self.at) != Some(&b'"') {
            return None;
        }
        self.at += 1;
        let mut out = String::new();
        loop {
            let byte = *self.bytes.get(self.at)?;
            match byte {
                b'"' => {
                    self.at += 1;
                    return Some(out);
                }
                b'\\' => {
                    self.at += 1;
                    let escape = *self.bytes.get(self.at)?;
                    self.at += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let high = self.hex4()?;
                            let code = if (0xd800..0xdc00).contains(&high) {
                                self.word("\\u")?;
                                let low = self.hex4()?;
                                if !(0xdc00..0xe000).contains(&low) {
                                    return None;
                                }
                                0x10000 + ((high - 0xd800) << 10) + (low - 0xdc00)
                            } else {
                                high
                            };
                            out.push(char::from_u32(code)?);
                        }
                        _ => return None,
                    }
                }
                _ => {
                    // Everything up to the next quote or escape, whole: the
                    // text is a `str`, so a character is never cut.
                    // `tests::json_reads_what_qt_writes`.
                    let run = self.at;
                    while self
                        .bytes
                        .get(self.at)
                        .is_some_and(|byte| *byte != b'"' && *byte != b'\\')
                    {
                        self.at += 1;
                    }
                    out.push_str(std::str::from_utf8(self.bytes.get(run..self.at)?).ok()?);
                }
            }
        }
    }

    fn hex4(&mut self) -> Option<u32> {
        let digits = std::str::from_utf8(self.bytes.get(self.at..self.at + 4)?).ok()?;
        self.at += 4;
        u32::from_str_radix(digits, 16).ok()
    }
}

/// An object's fields as JSON text, from a borrowed map, so a bag is rendered
/// without being copied into a [`Json::Object`] first.
/// `tests::an_object_renders_its_keys_in_order`.
pub(crate) fn write_object(fields: &BTreeMap<String, Json>, out: &mut String) {
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

/// A whole number without a fraction, as Lua wrote it: `48`, not `48.0`
/// (`tests::an_integral_number_renders_without_a_fraction`), and one that is
/// not finite as `null`, as `JSON.stringify` writes it
/// (`tests::a_number_that_is_not_finite_renders_as_null`).
fn render_number(value: f64) -> String {
    const EXACT: f64 = 9_007_199_254_740_992.0;
    if !value.is_finite() {
        return "null".to_owned();
    }
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

    /// **A number JSON has no word for is `null`**, as `JSON.stringify`
    /// writes it, so one bad value costs that value and not the whole bag
    /// the scene is built with.
    #[test]
    fn a_number_that_is_not_finite_renders_as_null() {
        let value = Json::Object(BTreeMap::from([
            ("a".to_owned(), Json::Number(f64::NAN)),
            ("b".to_owned(), Json::Number(f64::INFINITY)),
            ("c".to_owned(), Json::Number(f64::NEG_INFINITY)),
            ("d".to_owned(), Json::Number(1.0)),
        ]));
        assert_eq!(value.render(), r#"{"a":null,"b":null,"c":null,"d":1}"#);
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

    /// What a table nested too deep is refused with.
    const TOO_DEEP: &str = "nested deeper than 64, or a table that contains itself";

    /// **A table that contains itself is an error, not a crash**, as its own
    /// field or as its own item: the walk stops at 64 deep, so it cannot run
    /// the stack out.
    #[test]
    fn a_table_that_contains_itself_is_an_error_not_a_crash() {
        let lua = mlua::Lua::new();
        for chunk in [
            "local t = {} t.t = t return t",
            "local t = {} t[1] = t return t",
        ] {
            let value: mlua::Value = lua.load(chunk).eval().expect("the table evaluates");
            let said = Json::from_lua(&value)
                .err()
                .map(|err| err.to_string())
                .unwrap_or_default();
            assert!(said.contains(TOO_DEEP), "{chunk}: {said:?}");
        }
    }

    /// **A table 64 deep is read, and one 65 deep is an error**, a list or
    /// an object, as JSON text 64 deep is read.
    #[test]
    fn a_table_64_deep_is_read_and_65_deep_is_an_error() {
        let lua = mlua::Lua::new();
        let nested = |depth: usize, wrap: &str| -> mlua::Value {
            lua.load(format!(
                "local t = {{}} for _ = 2, {depth} do t = {wrap} end return t"
            ))
            .eval()
            .expect("the table evaluates")
        };
        let read = |value: &mlua::Value| {
            Json::from_lua(value)
                .ok()
                .flatten()
                .map(|json| json.render())
        };
        assert_eq!(
            (read(&nested(64, "{ t }")), read(&nested(64, "{ t = t }"))),
            (
                Some(format!("{}{{}}{}", "[".repeat(63), "]".repeat(63))),
                Some(format!("{}{{}}{}", r#"{"t":"#.repeat(63), "}".repeat(63)))
            ),
            "(64 lists deep, 64 objects deep)"
        );
        for wrap in ["{ t }", "{ t = t }"] {
            let said = Json::from_lua(&nested(65, wrap))
                .err()
                .map(|err| err.to_string())
                .unwrap_or_default();
            assert!(said.contains(TOO_DEEP), "65 deep, {wrap}: {said:?}");
        }
    }

    /// **What Qt writes, Rust reads**: the shape `Solium.send` sends.
    #[test]
    fn json_reads_what_qt_writes() {
        let parsed = Json::parse(r#" {"data":{"id":7,"name":"q\"й","on":[true,null,-1.5e1]}} "#)
            .expect("valid JSON");
        let data = parsed.get("data").expect("a data field");
        assert_eq!(data.get("id").and_then(Json::as_u64), Some(7));
        assert_eq!(data.get("name"), Some(&Json::Text("q\"й".to_owned())));
        assert_eq!(
            data.get("on"),
            Some(&Json::List(vec![
                Json::Bool(true),
                Json::Null,
                Json::Number(-15.0)
            ]))
        );
    }

    #[test]
    fn what_is_not_json_is_none() {
        for text in [
            "",
            "{",
            r#"{"a":}"#,
            "[1,]",
            "nul",
            r#""unterminated"#,
            "1 2",
        ] {
            assert_eq!(Json::parse(text), None, "{text:?}");
        }
    }

    /// **Nothing a scene sends can run the stack out**: a value nested
    /// deeper than 64 is refused, and one 64 deep is read.
    #[test]
    fn json_nested_too_deep_is_none() {
        let nested = |depth: usize| format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        assert!(Json::parse(&nested(64)).is_some(), "64 deep is read");
        assert_eq!(Json::parse(&nested(10_000)), None, "10 000 deep is refused");
    }

    /// **An escaped character and a surrogate pair are read**, and a lone
    /// half of a pair is not.
    #[test]
    fn json_reads_escapes_and_surrogate_pairs() {
        assert_eq!(
            Json::parse(r#""a\n\té😀\/""#),
            Some(Json::Text("a\n\té\u{1f600}/".to_owned()))
        );
        assert_eq!(Json::parse(r#""\ud83d""#), None);
        assert_eq!(Json::parse(r#""\ude00""#), None);
    }

    #[test]
    fn a_parsed_value_renders_back_as_it_was() {
        let text = r#"{"a":[1,2.5,"x"],"b":{"c":false}}"#;
        assert_eq!(
            Json::parse(text).map(|value| value.render()),
            Some(text.to_owned())
        );
    }

    /// **Only a whole, non-negative number is a window's id.**
    #[test]
    fn only_a_whole_non_negative_number_is_an_id() {
        assert_eq!(Json::Number(7.0).as_u64(), Some(7));
        for not in [
            Json::Number(-1.0),
            Json::Number(1.5),
            Json::Number(f64::NAN),
            Json::Number(1e300),
            Json::Text("7".to_owned()),
        ] {
            assert_eq!(not.as_u64(), None, "{not:?}");
        }
    }

    #[test]
    fn a_value_reaches_lua_as_a_table() {
        let lua = mlua::Lua::new();
        let value = Json::parse(r#"{"id":7,"tags":["a","b"]}"#)
            .expect("valid JSON")
            .to_lua(&lua)
            .expect("converts");
        lua.globals().set("value", value).expect("set");
        let read: String = lua
            .load(r#"return value.id .. " " .. value.tags[2] .. " " .. math.type(value.id)"#)
            .eval()
            .expect("reads");
        assert_eq!(read, "7 b integer");
    }
}
