//! Which monitors a pass draws, for the work `render::prepare` does per
//! monitor (#392).
//!
//! On the hardware each monitor is drawn on its own vblank, so most passes
//! draw one of two monitors. `prepare` drew every wanted xray cut and ran
//! every memo over it whichever monitor the pass drew: with a wallpaper
//! moving behind a 260 Hz and a 75 Hz monitor, three in four of the 260 Hz
//! monitor's backdrop blurs were run in passes that did not draw it, for a
//! picture the next pass that does draw it makes again. A cut of a monitor
//! the pass does not draw is left as it is, once it has a picture: its
//! generation does not move, so no memo over it runs, and the pass that
//! next draws that monitor finds it stale and draws it then. What a
//! monitor shows is drawn from what was made in the pass that draws it,
//! exactly as before.

/// The monitors of one pass: every one (the nested backend draws them all
/// each pass), or only those named.
#[derive(Clone, Debug, Default)]
pub(crate) struct Drawing(Option<Vec<String>>);

impl Drawing {
    /// Every monitor.
    pub(crate) const fn every() -> Self {
        Self(None)
    }

    /// Only the monitors `names` names.
    pub(crate) const fn only(names: Vec<String>) -> Self {
        Self(Some(names))
    }

    /// Whether the pass draws `monitor`.
    /// `tests::a_cut_is_kept_only_off_the_monitors_drawn_once_it_has_a_picture`.
    pub(crate) fn draws(&self, monitor: &str) -> bool {
        self.0
            .as_ref()
            .is_none_or(|names| names.iter().any(|name| name == monitor))
    }

    /// Whether the pass may leave the cut of `monitor` as it is: it does not
    /// draw that monitor, and the cut has a picture already. One with none
    /// yet is drawn, so the first frame's gate (`Xrays::ready`) is never
    /// told of a picture that does not exist.
    /// `tests::a_cut_is_kept_only_off_the_monitors_drawn_once_it_has_a_picture`.
    pub(crate) fn may_keep(&self, monitor: &str, has_picture: bool) -> bool {
        has_picture && !self.draws(monitor)
    }
}

#[cfg(test)]
mod tests {
    use super::Drawing;

    #[test]
    fn a_cut_is_kept_only_off_the_monitors_drawn_once_it_has_a_picture() {
        let one = Drawing::only(vec!["DP-2".to_owned()]);
        assert!(one.draws("DP-2") && !one.draws("DP-1"));
        assert!(
            one.may_keep("DP-1", true),
            "a monitor not drawn redrew its cut"
        );
        assert!(
            !one.may_keep("DP-1", false),
            "a cut with no picture was not drawn"
        );
        assert!(
            !one.may_keep("DP-2", true),
            "the monitor drawn kept a stale cut"
        );
        let every = Drawing::every();
        assert!(every.draws("DP-1") && !every.may_keep("DP-1", true));
    }
}
