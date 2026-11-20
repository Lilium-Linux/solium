//! `x11`: the X11 windows refused a tile, a frame and a bar entry outright,
//! by their `WM_CLASS` (#221).

use crate::script::doc::{Doc, describe};

/// Read the section. Each key left out keeps its default, and one of the
/// wrong kind is named in the log and keeps its default too.
pub(crate) fn parse(options: &Doc) -> crate::xwayland::Settings {
    let mut settings = crate::xwayland::Settings::default();
    match options.get("hidden") {
        // Each element kept only when it is text: not read the way Lua's
        // own `tostring` would, because a WM_CLASS name that came from `5`
        // by accident is a configuration mistake, not a value worth keeping.
        list @ Doc::Table(_) => {
            let names: Option<Vec<String>> = list
                .list()
                .iter()
                .map(|value| value.text().map(str::to_owned))
                .collect();
            if let Some(names) = names {
                settings.hidden = names;
            } else {
                tracing::warn!("x11.hidden is a list of strings; keeping the default");
            }
        }
        Doc::Nil => {}
        other => tracing::warn!(
            value = describe(other),
            "x11.hidden is a list of strings; keeping the default"
        ),
    }
    settings
}
