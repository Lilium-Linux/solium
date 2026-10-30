//! The XWayland side of the clipboard and the primary selection: offering an X11 client's
//! selection to Wayland clients, dropping it when that client lets go, and serving a Wayland
//! client's selection to an X11 one. And where an X11 window is told it is (#441).

use super::*;

impl Solium {
    /// Tell every managed X11 window that is not where the X server thinks it
    /// is where it is now: the space's location for it, at the size it was
    /// last told (#441).
    ///
    /// **An X11 client places its own menus from where it thinks its window
    /// is**, in root coordinates, and a menu is override-redirect: mapped
    /// exactly where the client put it. So a window drawn in one place and
    /// believed in another opens every menu, tooltip and combo box at the
    /// difference. Most moves told the X server already, through
    /// `size_window`, but not all: a window dragged by the pointer is moved in
    /// the space alone (`MoveDrag::to`), the layout placing it where the drag
    /// left it is no change to `offers_size`, so nothing was sent; and a
    /// client moving itself was granted on the X side and never followed in
    /// the space. Steam's dropdowns opened about 1200 px right and 600 px
    /// down of their button that way.
    ///
    /// **Once a pass, for every window, rather than at each of those moves**:
    /// the space is the one answer to where a window is, and asking it here
    /// cannot miss a path that moves a window, which a hook at each move
    /// would the next time one is added. Only when the two differ, so a
    /// window at rest costs a comparison and the X server hears one
    /// `ConfigureWindow` per pass a window moves.
    ///
    /// Not a window before it is first shown, which `show_if_new` places and
    /// tells, and whose own position is still read there (#427's
    /// `USPosition`). Not a window held at a size by a resize ([`Self::held_slot`]): its
    /// client is told the held rectangle by the resize, and the space is
    /// behind it until the client answers. Not an unmanaged one, which is
    /// where its client put it by definition, nor one leaving.
    /// `xwayland::tests::an_x11_window_is_told_again_only_where_it_is_not_where_it_is_drawn`
    /// and `x11-probe popup` nested.
    pub(crate) fn tell_x11_where_they_are(&self) {
        if self.xwm.is_none() {
            return;
        }
        for window in self.space.elements() {
            let Some(x11) = window.x11_surface() else {
                continue;
            };
            if x11.is_override_redirect() || !x11.is_mapped() {
                continue;
            }
            if !self.panes.of(window).is_some_and(|pane| {
                pane.managed() && !pane.leaving() && crate::present::was_shown(pane)
            }) || self.held_slot(window).is_some()
            {
                continue;
            }
            let Some(at) = self.space.element_location(window) else {
                continue;
            };
            if let Some(rect) = crate::xwayland::told_again(at, x11.geometry())
                && let Err(err) = x11.configure(Some(rect))
            {
                tracing::warn!(?err, "could not tell an X11 window where it is");
            }
        }
    }

    /// An X11 client has copied something. Offer it to Wayland clients.
    ///
    /// Offered as the compositor's own selection rather than any client's,
    /// which is what `set_data_device_selection` is for: from a Wayland
    /// client's side there is simply a selection available in these formats,
    /// and it never learns that the thing holding it does not speak Wayland.
    pub(crate) fn take_x11_selection(&mut self, ty: SelectionTarget, mimes: Vec<String>) {
        let display = self.display_handle.clone();
        match ty {
            SelectionTarget::Clipboard => {
                set_data_device_selection(&display, &self.seat, mimes, ());
            }
            SelectionTarget::Primary => {
                set_primary_selection(&display, &self.seat, mimes, ());
            }
        }
    }

    /// The X11 client that owned a selection has let it go.
    pub(crate) fn drop_x11_selection(&mut self, ty: SelectionTarget) {
        let display = self.display_handle.clone();
        match ty {
            SelectionTarget::Clipboard => clear_data_device_selection(&display, &self.seat),
            SelectionTarget::Primary => clear_primary_selection(&display, &self.seat),
        }
    }

    /// An X11 client wants to read a selection a Wayland client owns.
    ///
    /// Asked of whichever client owns it, which writes into the descriptor X11
    /// gave us. Nothing is copied through the compositor.
    pub(crate) fn serve_x11_selection(
        &mut self,
        ty: SelectionTarget,
        mime_type: String,
        fd: std::os::fd::OwnedFd,
    ) {
        // Two calls rather than one `match` producing a result: the clipboard
        // and the primary selection fail with different error types, and
        // flattening them would mean stringifying one to match the other.
        match ty {
            SelectionTarget::Clipboard => {
                if let Err(err) = request_data_device_client_selection(&self.seat, mime_type, fd) {
                    tracing::warn!(?err, "no Wayland client would serve the clipboard to X11");
                }
            }
            SelectionTarget::Primary => {
                if let Err(err) = request_primary_client_selection(&self.seat, mime_type, fd) {
                    tracing::warn!(?err, "no Wayland client would serve the primary to X11");
                }
            }
        }
    }
}
