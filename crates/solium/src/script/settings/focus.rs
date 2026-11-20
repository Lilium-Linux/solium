//! `focus_mode`: how a click and the pointer move the keyboard, and where it
//! goes when the window that has it closes.

use crate::script::doc::{Doc, describe};
use crate::successor::Successor;

/// The section, each key as given or `None` for the machine's own answer.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct FocusMode {
    pub(crate) click: Option<bool>,
    pub(crate) follow: Option<bool>,
    pub(crate) clear_on_empty_click: Option<bool>,
    pub(crate) after_close: Option<Vec<Successor>>,
}

/// Read the section. A key left out, or of the wrong kind, which the log
/// names, is the machine's own answer, not the one a call before left.
pub(crate) fn parse(options: &Doc) -> FocusMode {
    let flag = |key: &str| -> Option<bool> {
        match options.get(key) {
            Doc::Bool(on) => Some(*on),
            Doc::Nil => None,
            other => {
                tracing::warn!(
                    key,
                    value = describe(other),
                    "focus_mode: true, false or nil; leaving it unchanged"
                );
                None
            }
        }
    };
    FocusMode {
        click: flag("click"),
        follow: flag("follow"),
        clear_on_empty_click: flag("clear_on_empty_click"),
        after_close: after_close(options.get("after_close")),
    }
}

/// `after_close`: one word of [`Successor`], or a list of them in order
/// (#128).
///
/// A word it does not know is named in the log and left out, and the rest
/// still count; what is left can be nothing at all, which leaves the keyboard
/// on nothing when the window that has it closes, as asked. `nil` is `None`,
/// the compositor's own rule, and anything else is named and read as `nil`.
/// `sol_focus_mode_reads_after_close_word_by_word`.
fn after_close(value: &Doc) -> Option<Vec<Successor>> {
    let word = |each: &Doc| -> Option<Successor> {
        let parsed = match each {
            Doc::Text(text) => Successor::parse(text),
            _ => None,
        };
        if parsed.is_none() {
            tracing::warn!(
                word = describe(each),
                known = ?Successor::WORDS,
                "focus_mode: after_close names something that is not one of its words; \
                 leaving it out"
            );
        }
        parsed
    };
    match value {
        Doc::Nil => None,
        Doc::Text(_) => Some(word(value).into_iter().collect()),
        Doc::Table(table) => Some(table.list.iter().filter_map(word).collect()),
        other => {
            tracing::warn!(
                value = describe(other),
                "focus_mode: after_close is a word or a list of words; reading it as nil, the compositor's own rule"
            );
            None
        }
    }
}
