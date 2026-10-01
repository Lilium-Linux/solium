//! Keyed row diffs: what changed between two lists of rows, as operations a
//! list model applies one by one, so it never resets and never rebuilds a
//! delegate for a changed value: `tests::a_changed_value_is_one_change_of_that_value_only`,
//! `tests::a_reordering_is_moves_not_removes_and_inserts`.

use std::collections::{BTreeMap, HashSet};

use crate::json::Json;

/// One row of a model: its key, and its values by role.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    pub(crate) key: String,
    pub(crate) values: BTreeMap<&'static str, Json>,
}

/// One step from the old rows to the new. Each index is into the list as the
/// steps before it left it: `tests::applying_the_ops_to_the_old_rows_gives_the_new_ones`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Op {
    Insert {
        at: usize,
        row: Row,
    },
    /// The values that differ, and only those:
    /// `tests::a_changed_value_is_one_change_of_that_value_only`.
    Change {
        at: usize,
        key: String,
        values: BTreeMap<&'static str, Json>,
    },
    Remove {
        at: usize,
        key: String,
    },
    Move {
        from: usize,
        to: usize,
        key: String,
    },
}

/// The steps from `old` to `new`: removes first, last first, then each new
/// row in order, moved into place or inserted, and changed where it differs.
/// `tests::applying_the_ops_to_the_old_rows_gives_the_new_ones`.
pub(crate) fn diff<'a>(old: &'a [Row], new: &'a [Row]) -> Vec<Op> {
    let wanted: HashSet<&str> = new.iter().map(|row| row.key.as_str()).collect();
    let mut current: Vec<&'a Row> = old.iter().collect();
    let mut ops = Vec::new();
    for at in (0..current.len()).rev() {
        let key = &current[at].key;
        if !wanted.contains(key.as_str()) {
            ops.push(Op::Remove {
                at,
                key: key.clone(),
            });
            current.remove(at);
        }
    }
    for (at, row) in new.iter().enumerate() {
        match current.iter().position(|each| each.key == row.key) {
            Some(from) => {
                if from != at {
                    let moved = current.remove(from);
                    current.insert(at, moved);
                    ops.push(Op::Move {
                        from,
                        to: at,
                        key: row.key.clone(),
                    });
                }
                let before = current[at];
                let values: BTreeMap<&'static str, Json> = row
                    .values
                    .iter()
                    .filter(|(name, value)| before.values.get(*name) != Some(*value))
                    .map(|(name, value)| (*name, value.clone()))
                    .collect();
                if !values.is_empty() {
                    ops.push(Op::Change {
                        at,
                        key: row.key.clone(),
                        values,
                    });
                }
                current[at] = row;
            }
            None => {
                current.insert(at, row);
                ops.push(Op::Insert {
                    at,
                    row: row.clone(),
                });
            }
        }
    }
    ops
}

/// The steps as the JSON array `solium_qml_rows_apply` reads:
/// `tests::the_ops_render_as_the_array_the_host_reads`.
pub(crate) fn render(ops: &[Op]) -> String {
    let index = |value: usize| {
        #[expect(clippy::cast_precision_loss, reason = "a row index, far below 2^53")]
        let value = value as f64;
        Json::Number(value)
    };
    let text = |text: &str| Json::Text(text.to_owned());
    let values = |values: &BTreeMap<&'static str, Json>| {
        Json::Object(
            values
                .iter()
                .map(|(name, value)| ((*name).to_owned(), value.clone()))
                .collect(),
        )
    };
    Json::List(
        ops.iter()
            .map(|op| {
                let fields: BTreeMap<String, Json> = match op {
                    Op::Insert { at, row } => BTreeMap::from([
                        ("op".to_owned(), text("insert")),
                        ("at".to_owned(), index(*at)),
                        ("key".to_owned(), text(&row.key)),
                        ("values".to_owned(), values(&row.values)),
                    ]),
                    Op::Change {
                        at,
                        key,
                        values: changed,
                    } => BTreeMap::from([
                        ("op".to_owned(), text("change")),
                        ("at".to_owned(), index(*at)),
                        ("key".to_owned(), text(key)),
                        ("values".to_owned(), values(changed)),
                    ]),
                    Op::Remove { at, key } => BTreeMap::from([
                        ("op".to_owned(), text("remove")),
                        ("at".to_owned(), index(*at)),
                        ("key".to_owned(), text(key)),
                    ]),
                    Op::Move { from, to, key } => BTreeMap::from([
                        ("op".to_owned(), text("move")),
                        ("from".to_owned(), index(*from)),
                        ("to".to_owned(), index(*to)),
                        ("key".to_owned(), text(key)),
                    ]),
                };
                Json::Object(fields)
            })
            .collect(),
    )
    .render()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{Op, Row, diff};
    use crate::json::Json;

    fn row(key: &str, value: f64) -> Row {
        Row {
            key: key.to_owned(),
            values: BTreeMap::from([
                ("v", Json::Number(value)),
                ("k", Json::Text(key.to_owned())),
            ]),
        }
    }

    /// The old rows with `ops` applied in order, as a list model applies them.
    fn applied(old: &[Row], ops: &[Op]) -> Vec<Row> {
        let mut rows = old.to_vec();
        for op in ops {
            match op {
                Op::Insert { at, row } => rows.insert(*at, row.clone()),
                Op::Change { at, key, values } => {
                    assert_eq!(&rows[*at].key, key, "a change aimed at the wrong row");
                    for (name, value) in values {
                        rows[*at].values.insert(name, value.clone());
                    }
                }
                Op::Remove { at, key } => {
                    assert_eq!(&rows[*at].key, key, "a remove aimed at the wrong row");
                    rows.remove(*at);
                }
                Op::Move { from, to, key } => {
                    assert_eq!(&rows[*from].key, key, "a move aimed at the wrong row");
                    let moved = rows.remove(*from);
                    rows.insert(*to, moved);
                }
            }
        }
        rows
    }

    #[test]
    fn nothing_changed_is_no_ops() {
        let rows = [row("a", 1.0), row("b", 2.0)];
        assert!(diff(&rows, &rows).is_empty());
    }

    #[test]
    fn a_new_row_is_one_insert_at_its_place() {
        let ops = diff(
            &[row("a", 1.0), row("c", 3.0)],
            &[row("a", 1.0), row("b", 2.0), row("c", 3.0)],
        );
        assert_eq!(
            ops,
            vec![Op::Insert {
                at: 1,
                row: row("b", 2.0)
            }]
        );
    }

    #[test]
    fn a_gone_row_is_one_remove() {
        let ops = diff(&[row("a", 1.0), row("b", 2.0)], &[row("b", 2.0)]);
        assert_eq!(
            ops,
            vec![Op::Remove {
                at: 0,
                key: "a".to_owned()
            }]
        );
    }

    /// **A changed value is a change of that value only**, so a model emits
    /// `dataChanged` for that role and a delegate keeps its animations.
    #[test]
    fn a_changed_value_is_one_change_of_that_value_only() {
        let ops = diff(&[row("a", 1.0)], &[row("a", 5.0)]);
        assert_eq!(
            ops,
            vec![Op::Change {
                at: 0,
                key: "a".to_owned(),
                values: BTreeMap::from([("v", Json::Number(5.0))])
            }]
        );
    }

    /// **A reordering is moves, never a remove and an insert**, which would
    /// rebuild the delegate.
    #[test]
    fn a_reordering_is_moves_not_removes_and_inserts() {
        let ops = diff(
            &[row("a", 1.0), row("b", 2.0), row("c", 3.0)],
            &[row("c", 3.0), row("a", 1.0), row("b", 2.0)],
        );
        assert!(
            ops.iter().all(|op| matches!(op, Op::Move { .. })),
            "{ops:?}"
        );
    }

    #[test]
    fn applying_the_ops_to_the_old_rows_gives_the_new_ones() {
        let cases: [(Vec<Row>, Vec<Row>); 4] = [
            (vec![], vec![row("a", 1.0), row("b", 2.0)]),
            (
                vec![row("a", 1.0), row("b", 2.0), row("c", 3.0)],
                vec![row("c", 9.0), row("d", 4.0), row("a", 1.0)],
            ),
            (vec![row("a", 1.0), row("b", 2.0)], vec![]),
            (
                vec![row("a", 1.0), row("b", 2.0), row("c", 3.0), row("d", 4.0)],
                vec![row("d", 4.0), row("b", 7.0), row("e", 5.0), row("a", 1.0)],
            ),
        ];
        for (old, new) in cases {
            assert_eq!(applied(&old, &diff(&old, &new)), new, "from {old:?}");
        }
    }

    #[test]
    fn the_ops_render_as_the_array_the_host_reads() {
        let ops = vec![
            Op::Remove {
                at: 2,
                key: "x".to_owned(),
            },
            Op::Move {
                from: 1,
                to: 0,
                key: "y".to_owned(),
            },
        ];
        assert_eq!(
            super::render(&ops),
            r#"[{"at":2,"key":"x","op":"remove"},{"from":1,"key":"y","op":"move","to":0}]"#
        );
    }
}
