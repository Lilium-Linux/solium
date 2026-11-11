//! `cursor`: the pointer's theme, size and scene.

use crate::script::doc::Doc;

/// Read the section.
///
/// Absent is not "reset it": it means the configuration did not say, which
/// is the default `Configured`, which is what lets `XCURSOR_THEME` and
/// `XCURSOR_SIZE` have their turn. A key of the wrong kind is named in the
/// log and read as absent, never a configuration that does not load.
///
/// A size out of *range* is passed on rather than refused here:
/// `Settings::resolve` is the one place that decides what a size that is not
/// a size means, and it needs to see it to fall through to the environment
/// rather than to a clamp. Only numbers are passed on; a value that is not a
/// number at all is not a size `resolve` could be shown.
/// `tests::a_configured_cursor_scene_reaches_the_compositor`.
pub(crate) fn parse(options: &Doc) -> crate::cursor::theme::Configured {
    let mut configured = crate::cursor::theme::Configured::default();
    if let Doc::Text(name) = options.get("theme")
        && !name.is_empty()
    {
        configured.theme = Some(name.clone());
    }
    match options.get("size") {
        Doc::Nil => {}
        value => match value.as_i64().and_then(|size| i32::try_from(size).ok()) {
            Some(size) => configured.size = Some(size),
            None => tracing::warn!(
                size = ?value,
                "cursor size is not a whole number; ignoring it"
            ),
        },
    }
    match options.get("scene") {
        Doc::Nil => {}
        Doc::Text(scene) => {
            if !scene.is_empty() {
                configured.scene = Some(scene.clone());
            }
        }
        value => tracing::warn!(
            scene = ?value,
            "cursor scene is not a file name; ignoring it"
        ),
    }
    configured
}
