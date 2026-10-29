//! A window kept above the others (#424): `sol.keep_above`, and the one place
//! the stack is put in order for it.
//!
//! The stack is the space's order, which `sync_panes` hands to the panes and
//! so to both the renderer and every hit test. Kept-above windows are moved
//! over the rest there, once, before it is read: so what is drawn on top is
//! what is clicked, and a click or the keyboard raising a window -- which
//! raises it to the very top of the space -- raises it only among its own
//! kind. Fullscreen, the bars and the lock are bands of their own
//! (`crate::stack`) and are not touched.

use crate::script::Parentage;

use super::*;

/// Where a window stands in the stack: the windows of each band are over
/// every window of the bands below it, and in their usual order among
/// themselves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Band {
    /// Every ordinary window.
    Ordinary,
    /// A window kept above, and the windows that belong to it -- its dialogs
    /// -- so a kept window's dialog is not put under it.
    Kept,
    /// The focused window, while it is fullscreen: it covers a kept-above
    /// window as it covers everything else of the workspace, as in KWin.
    FocusedFullscreen,
}

impl Solium {
    /// Keep `pane`'s window above every window that is not, or stop; `None`
    /// toggles it. The space's stack is put in order at once, and the
    /// frame's layers are told `keptAbove` at their next draw.
    /// `tests::real_client::reflow_on_close::above::a_kept_above_window_stays_over_a_window_focused_after_it_and_takes_its_click`,
    /// `tests::real_client::reflow_on_close::above::toggled_off_a_window_is_stacked_as_any_other`.
    pub(crate) fn keep_above(&mut self, pane: crate::pane::PaneId, on: Option<bool>) {
        let Some(held) = self.panes.get_mut(pane) else {
            return;
        };
        let above = on.unwrap_or(!held.above());
        if held.above() == above {
            return;
        }
        held.set_above(above);
        // Turned on, it goes over the others in the space now, and the panes
        // take that order at the frame this asks for, as they take any
        // raise. Turned off, it stays where it is in the stack, which is over
        // them, until something else is raised over it, as a window clicked
        // last would.
        self.stack_kept_above();
        self.redraw = true;
    }

    /// Put the space's stack in order for the windows kept above: every
    /// window of a higher [`Band`] over every window of a lower one, each
    /// band in the order it had. Nothing at all when no window is kept above,
    /// which is every desktop that never asks. Called by `sync_panes` before
    /// it reads the stack.
    /// `tests::real_client::reflow_on_close::above::a_focused_fullscreen_window_covers_one_kept_above`.
    pub(crate) fn stack_kept_above(&mut self) {
        if !self.panes.iter().any(Pane::above) {
            return;
        }
        // Bottom to top, as the space has them.
        let stack: Vec<(Band, Window)> = self
            .space
            .elements()
            .map(|window| (self.band_of(window), window.clone()))
            .collect();
        if stack.windows(2).all(|pair| pair[0].0 <= pair[1].0) {
            return;
        }
        let mut ordered = stack;
        // Stable, so each band keeps the order it had.
        ordered.sort_by_key(|(band, _)| *band);
        for (_, window) in &ordered {
            self.space.raise_element(window, false);
        }
    }

    /// Which band `window` is in.
    fn band_of(&self, window: &Window) -> Band {
        if workspaces::fullscreen(window) && self.is_focused(window) {
            return Band::FocusedFullscreen;
        }
        if self.kept_or_belongs_to_kept(window) {
            return Band::Kept;
        }
        Band::Ordinary
    }

    /// Whether `window` is kept above, or is the dialog of one that is, up
    /// its chain of parents. Bounded by the number of panes, so a cycle of
    /// parents, which neither protocol forbids, ends.
    fn kept_or_belongs_to_kept(&self, window: &Window) -> bool {
        let mut at = window.clone();
        for _ in 0..=self.panes.iter().count() {
            if self.panes.of(&at).is_some_and(Pane::above) {
                return true;
            }
            let Parentage::Window(parent) = self.parent_of(&at) else {
                return false;
            };
            let Some(next) = self.panes.by_script_id(parent).and_then(Pane::client) else {
                return false;
            };
            at = next.clone();
        }
        false
    }
}
