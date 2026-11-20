//! `layer_shell`: which of a client's layer surfaces below the windows take the
//! pointer and touch (#461).

use crate::script::doc::{Doc, describe};

/// Read the section. Each key left out keeps its default, and one of the
/// wrong kind is named in the log and keeps its default too.
pub(crate) fn parse(options: &Doc) -> crate::layer::Settings {
    let mut settings = crate::layer::Settings::default();
    match options.get("input_below") {
        Doc::Bool(true) | Doc::Nil => {}
        Doc::Bool(false) => {
            settings.input_below = crate::layer::InputBelow::Nothing;
        }
        list @ Doc::Table(_) => {
            // Every item a string, or none is kept: a namespace that came
            // from a number by accident is a mistake to say, not a name.
            let names: Option<Vec<_>> = list
                .list()
                .iter()
                .map(|value| value.text().map(crate::effect::rules::Glob::parse))
                .collect();
            if let Some(names) = names {
                settings.input_below = crate::layer::InputBelow::Named(names);
            } else {
                tracing::warn!(
                    "layer_shell.input_below is true, false or a list of \
                     namespaces; every surface takes input"
                );
            }
        }
        other => tracing::warn!(
            value = describe(other),
            "layer_shell.input_below is true, false or a list of namespaces; \
             every surface takes input"
        ),
    }
    settings
}
