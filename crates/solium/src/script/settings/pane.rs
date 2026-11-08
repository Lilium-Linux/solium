//! `pane_build` and `loading`: how long a frame may spend building frames
//! for a new pane style, and what a window shows before its application has.

use std::time::Duration;

use super::read;
use crate::script::Loading;
use crate::script::doc::Doc;

/// `pane_build`: a number of milliseconds, 0 or more. `None` when left out,
/// or of the wrong kind, which the log names: what it was is kept.
pub(crate) fn build_budget(millis: &Doc) -> Option<Duration> {
    let budget = match millis {
        Doc::Nil => return None,
        Doc::Int(whole) => u64::try_from(*whole).ok().map(Duration::from_millis),
        Doc::Num(part) if part.is_finite() && *part >= 0.0 => {
            Some(Duration::from_secs_f64(part / 1000.0))
        }
        _ => None,
    };
    if budget.is_none() {
        tracing::warn!(
            ?millis,
            "pane_build is a number of milliseconds, 0 or more; keeping what it was"
        );
    }
    budget
}

/// `loading`. A number or flag of the wrong kind refuses the section, in the
/// words Lua's conversions use; `scene` that is not text is read as left out.
pub(crate) fn loading(options: &Doc) -> Result<Loading, String> {
    let mut loading = Loading::default();
    if let Some(millis) = read::unsigned(options, "patience")? {
        loading.patience = Duration::from_millis(millis);
    }
    if let Some(reserves) = read::flag(options, "reserves_a_slot") {
        loading.reserves_a_slot = reserves;
    }
    if let Some(decorated) = read::flag(options, "decorated") {
        loading.decorated = decorated;
    }
    if let Some(millis) = read::unsigned(options, "fade")? {
        loading.fade = Duration::from_millis(millis);
    }
    if let Doc::Text(scene) = options.get("scene") {
        loading.scene = Some(scene.clone());
    }
    Ok(loading)
}
