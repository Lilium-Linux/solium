//! `fullscreen`: what a fullscreen window covers.

use crate::script::doc::Doc;

/// Read the section: `covers`, `"top"` or `"none"`; a name it does not know
/// is named in the log and the default kept.
pub(crate) fn parse(options: &Doc) -> crate::stack::Covers {
    let mut covers = crate::stack::Covers::default();
    if let Doc::Text(name) = options.get("covers") {
        match crate::stack::Covers::named(name) {
            Some(named) => covers = named,
            None => tracing::warn!(
                covers = %name,
                "fullscreen.covers is \"top\" or \"none\"; keeping the default"
            ),
        }
    }
    covers
}
