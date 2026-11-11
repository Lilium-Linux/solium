//! `toplevels`: which window protocols a shell that is its own program is
//! offered (#50).

use crate::script::doc::{Doc, describe};

/// Read the section. Each key left out keeps its default, and one of the
/// wrong kind is named in the log and keeps its default too.
pub(crate) fn parse(options: &Doc) -> crate::toplevels::Settings {
    let mut settings = crate::toplevels::Settings::default();
    for (key, slot) in [
        ("list", &mut settings.list),
        ("manage", &mut settings.manage),
    ] {
        match options.get(key) {
            Doc::Bool(on) => *slot = *on,
            Doc::Nil => {}
            other => tracing::warn!(
                key,
                value = describe(other),
                "toplevels: true or false; keeping it offered"
            ),
        }
    }
    match options.get("capture") {
        Doc::Bool(false) => {
            settings.capture = crate::toplevels::WindowCapture::Off;
        }
        Doc::Text(name) => match crate::toplevels::WindowCapture::named(name) {
            Some(capture) => settings.capture = capture,
            None => tracing::warn!(
                value = %name,
                "toplevels.capture: \"client\", \"pane\" or false; keeping \"client\""
            ),
        },
        Doc::Nil => {}
        other => tracing::warn!(
            value = describe(other),
            "toplevels.capture: \"client\", \"pane\" or false; keeping \"client\""
        ),
    }
    settings
}
