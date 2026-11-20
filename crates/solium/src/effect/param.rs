//! An effect's param value as a document writes it.

use solium_effects::spec::Value;

use crate::script::Doc;

/// A param value: a number, an integer, a boolean, a word, or four numbers.
/// `tests::four_numbers_are_a_vec4_and_three_are_not`.
pub(crate) fn param_value(doc: &Doc) -> Option<Value> {
    match doc {
        Doc::Bool(yes) => Some(Value::Bool(*yes)),
        Doc::Int(int) => Some(Value::Int(*int)),
        Doc::Num(number) if number.is_finite() => Some(Value::Number(*number)),
        Doc::Text(word) => Some(Value::Word(word.clone())),
        Doc::Table(table) if table.fields.is_empty() && table.list.len() == 4 => {
            let four: Vec<f64> = table
                .list
                .iter()
                .filter_map(|each| match each {
                    #[expect(
                        clippy::cast_precision_loss,
                        reason = "a colour or a vector component"
                    )]
                    Doc::Int(int) => Some(*int as f64),
                    Doc::Num(number) if number.is_finite() => Some(*number),
                    _ => None,
                })
                .collect();
            let [a, b, c, d] = four[..] else {
                return None;
            };
            Some(Value::Vec4([a, b, c, d]))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use solium_effects::spec::Value;

    use super::param_value;
    use crate::script::doc::tests::plain;

    #[test]
    fn four_numbers_are_a_vec4_and_three_are_not() {
        let value = |lua: &str| plain(lua).and_then(|read| param_value(&read));
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
