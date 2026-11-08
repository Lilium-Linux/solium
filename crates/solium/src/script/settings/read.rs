//! Typed reads of a section's keys, with the conversions Lua's own make, so a
//! section read from a document accepts and refuses what it did when it was
//! read from a Lua table: a number where text is wanted is its text.

use crate::script::Doc;

/// Why a value is not the type a key wants, in the words a Lua conversion
/// uses, so the message a configuration's author sees is the one it was.
pub(crate) fn not_a(value: &Doc, to: &str) -> String {
    format!("error converting Lua {} to {to}", value.type_name())
}

/// A table under `key`, or `None` when absent.
pub(crate) fn table<'a>(section: &'a Doc, key: &str) -> Result<Option<&'a Doc>, String> {
    match section.get(key) {
        Doc::Nil => Ok(None),
        table @ Doc::Table(_) => Ok(Some(table)),
        other => Err(not_a(other, "table")),
    }
}

/// Text under `key`, a number read as its text, or `None` when absent.
pub(crate) fn text(section: &Doc, key: &str) -> Result<Option<String>, String> {
    text_of(section.get(key))
}

/// A value as text, a number read as its text, `None` for nil.
pub(crate) fn text_of(value: &Doc) -> Result<Option<String>, String> {
    match value {
        Doc::Nil => Ok(None),
        Doc::Text(text) => Ok(Some(text.clone())),
        Doc::Int(whole) => Ok(Some(whole.to_string())),
        Doc::Num(number) => Ok(Some(lua_number_text(*number))),
        other => Err(not_a(other, "String")),
    }
}

/// A list of text under `key`, or `None` when absent.
pub(crate) fn texts(section: &Doc, key: &str) -> Result<Option<Vec<String>>, String> {
    match section.get(key) {
        Doc::Nil => Ok(None),
        Doc::Table(table) => table
            .list
            .iter()
            .map(|item| text_of(item)?.ok_or_else(|| not_a(item, "String")))
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        other => Err(not_a(other, "Vec")),
    }
}

/// A whole number of zero or more under `key`, as Lua reads one for a
/// Rust `u64`: a whole float and numeric text are read, and anything with a
/// fraction, negative or past the type is refused as out of range.
pub(crate) fn unsigned(section: &Doc, key: &str) -> Result<Option<u64>, String> {
    let value = section.get(key);
    let out_of_range = || format!("{} (out of range)", not_a(value, "u64"));
    let number = match value {
        Doc::Nil => return Ok(None),
        Doc::Int(whole) => return u64::try_from(*whole).map(Some).map_err(|_| out_of_range()),
        Doc::Num(number) => *number,
        Doc::Text(text) => match text.trim().parse::<i64>() {
            Ok(whole) => return u64::try_from(whole).map(Some).map_err(|_| out_of_range()),
            Err(_) => text.trim().parse::<f64>().map_err(|_| {
                format!(
                    "{} (expected number or string coercible to number)",
                    not_a(value, "u64")
                )
            })?,
        },
        _ => {
            return Err(format!(
                "{} (expected number or string coercible to number)",
                not_a(value, "u64")
            ));
        }
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "checked to be whole and within u64 first"
    )]
    if number.fract() == 0.0 && number >= 0.0 && number < u64::MAX as f64 {
        Ok(Some(number as u64))
    } else {
        Err(out_of_range())
    }
}

/// A flag under `key`: `None` when absent, and otherwise true for anything
/// but `false`, as Lua reads one for a Rust `bool`.
pub(crate) fn flag(section: &Doc, key: &str) -> Option<bool> {
    match section.get(key) {
        Doc::Nil => None,
        Doc::Bool(on) => Some(*on),
        _ => Some(true),
    }
}

/// How Lua writes a float as text: C's `%.14g`, with `.0` after a whole
/// number so it still reads as a float.
fn lua_number_text(number: f64) -> String {
    if number.is_nan() {
        return if number.is_sign_negative() {
            "-nan"
        } else {
            "nan"
        }
        .to_owned();
    }
    if number.is_infinite() {
        return if number > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    let text = g14(number);
    if text.contains(['.', 'e', 'n', 'i']) {
        text
    } else {
        format!("{text}.0")
    }
}

/// C's `%.14g` for a finite number.
fn g14(number: f64) -> String {
    const DIGITS: i32 = 14;
    if number == 0.0 {
        return if number.is_sign_negative() { "-0" } else { "0" }.to_owned();
    }
    // The exponent `%e` would print, after rounding to `DIGITS` digits.
    let scientific = format!("{number:.13e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let trim = |text: &str| -> String {
        if text.contains('.') {
            text.trim_end_matches('0').trim_end_matches('.').to_owned()
        } else {
            text.to_owned()
        }
    };
    if !(-4..DIGITS).contains(&exponent) {
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", trim(mantissa), exponent.abs())
    } else {
        let decimals = usize::try_from(DIGITS - 1 - exponent).unwrap_or(0);
        trim(&format!("{number:.decimals$}"))
    }
}

#[cfg(test)]
mod tests {
    use super::{flag, table, text, texts, unsigned};
    use crate::script::doc::tests::doc;

    #[test]
    fn a_table_key_refuses_a_number_in_luas_words() {
        let section = doc("{ apps = 3, none = nil }");
        assert_eq!(
            table(&section, "apps").expect_err("not a table"),
            "error converting Lua integer to table"
        );
        assert_eq!(table(&section, "none"), Ok(None));
    }

    #[test]
    fn text_takes_a_number_as_its_text() {
        let section = doc("{ a = 7, b = 2.5, c = '3', d = {} }");
        assert_eq!(text(&section, "a"), Ok(Some("7".to_owned())));
        assert_eq!(text(&section, "b"), Ok(Some("2.5".to_owned())));
        assert!(text(&section, "d").is_err());
        assert_eq!(
            texts(&doc("{ l = { 'x', 1 } }"), "l"),
            Ok(Some(vec!["x".to_owned(), "1".to_owned()]))
        );
    }

    #[test]
    fn unsigned_and_flag_read_as_luas_own_conversions_do() {
        let lua = mlua::Lua::new();
        for source in ["5", "2.7", "3.0", "'12'", "-1", "1e30", "{}", "true"] {
            let table: mlua::Table = lua
                .load(format!("return {{ x = {source} }}"))
                .eval()
                .expect("the test's Lua");
            let lua_says = table.get::<Option<u64>>("x").map_err(|err| err.to_string());
            let section = doc(&format!("{{ x = {source} }}"));
            assert_eq!(unsigned(&section, "x"), lua_says, "{source}");
            let lua_flag = table.get::<Option<bool>>("x").expect("a flag always reads");
            assert_eq!(flag(&section, "x"), lua_flag, "{source}");
        }
    }

    #[test]
    fn a_float_reads_as_the_text_lua_gives_it() {
        let lua = mlua::Lua::new();
        for source in [
            "2.5",
            "7.0",
            "0.1 + 0.2",
            "1e20",
            "1e-7",
            "-3.25",
            "123456.789",
            "1/3",
        ] {
            let expected: String = lua
                .load(format!("return tostring({source})"))
                .eval()
                .expect("the test's Lua");
            let section = doc(&format!("{{ x = {source} }}"));
            assert_eq!(text(&section, "x"), Ok(Some(expected)), "{source}");
        }
    }
}
