//! `qml`: which scene graph QML renders on, read once before Qt starts, and
//! how a GPU scene's buffer is sized.

use std::time::Duration;

use crate::script::doc::Doc;

/// Read the section. Each key left out keeps its default, and one of the
/// wrong kind is named in the log and keeps its default too.
pub(crate) fn parse(options: &Doc) -> crate::qml::renderer::Configured {
    let mut configured = crate::qml::renderer::Configured::default();
    match options.get("renderer") {
        Doc::Text(name) => configured.renderer = Some(name.clone()),
        Doc::Nil => {}
        other => tracing::warn!(
            renderer = ?other,
            "qml.renderer is one of auto, gpu or software; using auto"
        ),
    }
    match options.get("probe_timeout") {
        Doc::Nil => {}
        value => match value.as_u64().filter(|&millis| millis > 0) {
            Some(millis) => configured.probe_timeout = Some(Duration::from_millis(millis)),
            None => tracing::warn!(
                probe_timeout = ?value,
                "qml.probe_timeout is a whole number of milliseconds above 0; \
                 keeping the default"
            ),
        },
    }
    // How a GPU scene's buffer is sized (`qml::buffers`): a key left out
    // keeps its default, and a table out of bounds, or with a key that is
    // not a whole number, keeps both.
    // `tests::a_configured_qml_renderer_is_read_before_qt_starts`.
    match options.get("buffers") {
        Doc::Nil => {}
        buffers @ Doc::Table(_) => {
            let whole = |key: &str, default: i32| match buffers.get(key) {
                Doc::Nil => Some(default),
                value => value.as_i64().and_then(|whole| i32::try_from(whole).ok()),
            };
            let step = whole("step", crate::qml::buffers::STEP);
            let shrink = whole("shrink", crate::qml::buffers::SHRINK);
            match step
                .zip(shrink)
                .and_then(|(step, shrink)| crate::qml::buffers::Buffers::new(step, shrink))
            {
                Some(policy) => configured.buffers = Some(policy),
                None => tracing::warn!(
                    ?step,
                    ?shrink,
                    "qml.buffers.step is a whole number of pixels from 0 to 1024 and \
                     qml.buffers.shrink a whole number of steps from 2 to 64; keeping \
                     the defaults"
                ),
            }
        }
        other => tracing::warn!(
            buffers = ?other,
            "qml.buffers is a table, {{ step = , shrink = }}; keeping the defaults"
        ),
    }
    configured
}
