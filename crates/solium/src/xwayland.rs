//! Running X11 clients, through XWayland.
//!
//! An X11 client cannot talk Wayland, so XWayland talks it on their behalf:
//! it is one Wayland client that happens to contain many X11 ones. In exchange
//! the compositor has to be their window manager, which is a second, older
//! protocol with its own ideas -- windows place themselves, ask to be resized
//! by the pointer, and expect a manager to be listening.
//!
//! What that buys is most of the software people actually run. Wayland-native
//! clients worked here already; Steam, glxgears, and anything Electron that
//! has not been told to prefer Wayland do not exist without this.
//!
//! The X11 surfaces are put in the same `Space` as everything else and go
//! through the same show path, so the layout scripts, the presentation
//! transform, and the deforms apply to them without knowing what they are.

use smithay::{
    desktop::Window,
    reexports::{
        calloop::{
            LoopHandle,
            timer::{TimeoutAction, Timer},
        },
        wayland_server::{DisplayHandle, protocol::wl_surface::WlSurface},
    },
    utils::{Logical, Rectangle, SERIAL_COUNTER},
    wayland::{
        seat::WaylandFocus,
        selection::SelectionTarget,
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    },
    xwayland::{
        X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler,
        xwm::{Reorder, XwmId},
    },
};

use crate::state::Solium;

/// The event loop's data, whatever the backend made it, has a compositor in it.
///
/// XWayland's sources are inserted into the loop the backend owns, and the two
/// backends do not agree on what the loop's data is: nested it is the
/// compositor itself, on the hardware it is a struct with the compositor
/// inside. Rather than teach XWayland about either, both say where the
/// compositor is.
pub(crate) trait HasSolium {
    fn solium(&mut self) -> &mut Solium;
}

impl HasSolium for Solium {
    fn solium(&mut self) -> &mut Solium {
        self
    }
}

/// Start XWayland and take over as its window manager.
///
/// Failure is not fatal and deliberately so: a session with no X11 support is
/// a working session, and one that refuses to start because XWayland is not
/// installed is not.
pub(crate) fn start<D>(handle: &LoopHandle<'static, D>, display: &DisplayHandle)
where
    D: HasSolium + XwmHandler + XWaylandShellHandler + 'static,
{
    let spawned = XWayland::spawn(
        display,
        None,
        std::iter::empty::<(String, String)>(),
        true,
        std::process::Stdio::null(),
        // Errors are inherited, not discarded, for the same reason spawning a
        // client inherits them: a program that refuses to start says why on
        // stderr, and sending that to /dev/null leaves "X11 does not work"
        // as the only symptom of every possible cause. This one was worth a
        // wasted benchmark round before anyone thought to look.
        std::process::Stdio::inherit(),
        |_| {},
    );
    let (xwayland, client) = match spawned {
        Ok(pair) => pair,
        Err(err) => {
            tracing::warn!(?err, "no XWayland: X11 clients will not run");
            return;
        }
    };

    // The loop handle is cloned into the callback rather than reached for
    // through the state: the manager needs somewhere to insert its own source,
    // and this is already the right handle.
    let loop_handle = handle.clone();
    let inserted = handle.insert_source(xwayland, move |event, (), data| match event {
        XWaylandEvent::Ready {
            x11_socket,
            display_number,
        } => match X11Wm::start_wm::<D>(loop_handle.clone(), x11_socket, client.clone()) {
            Ok(wm) => {
                let solium = data.solium();
                solium.xwm = Some(wm);
                solium.x11_display = Some(display_number);
                tracing::info!(display = display_number, "XWayland is up");
            }
            Err(err) => tracing::warn!(?err, "could not manage XWayland's windows"),
        },
        XWaylandEvent::Error => tracing::warn!("XWayland exited during startup"),
    });
    if let Err(err) = inserted {
        tracing::warn!(?err, "could not watch XWayland");
    }

    // Say so if it never arrives.
    //
    // `spawn` succeeding only means the sockets were made and the process was
    // started; XWayland can still exit before it is ready, and the event that
    // would tell us is one it never sends. So the log read "spawning XWayland
    // instance" and then nothing at all, forever, while every X11 client failed
    // with "couldn't open display" for what looked like an unrelated reason.
    //
    // Silence is the worst possible failure here, because X11 support is
    // optional: there is no crash to notice and nothing on screen is missing
    // until you happen to start a client that needs it.
    let timeout = handle.insert_source(
        Timer::from_duration(std::time::Duration::from_secs(5)),
        |_, (), data: &mut D| {
            if data.solium().x11_display.is_none() {
                tracing::warn!(
                    "XWayland did not become ready; X11 clients will not run. \
                     Its own error, if it printed one, is above this line."
                );
            }
            TimeoutAction::Drop
        },
    );
    if let Err(err) = timeout {
        tracing::warn!(?err, "could not watch for XWayland being slow");
    }
}

impl XWaylandShellHandler for Solium {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell_state
    }
}

impl XwmHandler for Solium {
    #[expect(
        clippy::expect_used,
        reason = "the manager is stored before the event source that calls \
                  this is created, so there is no order in which it is absent"
    )]
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.xwm
            .as_mut()
            .expect("the X11 window manager is what asked us")
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    /// An X11 client asking to be shown.
    ///
    /// Unlike an xdg toplevel, which is told its size before it may draw, an
    /// X11 window arrives with a size it chose. It is honoured here and the
    /// layout gets its say through the ordinary show path.
    fn map_window_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Err(err) = window.set_mapped(true) {
            tracing::warn!(?err, "could not map an X11 window");
            return;
        }
        let element = Window::new_x11_window(window);
        self.space.map_element(element.clone(), (0, 0), true);
        self.take_pane(element);
    }

    /// A window that manages its own placement: menus, tooltips, drag icons.
    ///
    /// Override-redirect means "do not manage me", so it is placed exactly
    /// where it asked and never laid out.
    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        let location = window.geometry().loc;
        let element = Window::new_x11_window(window);
        self.space.map_element(element.clone(), location, true);
        // A pane even for these -- they are on screen and under the pointer,
        // and every path that asks what is on screen asks for panes -- but an
        // *unmanaged* one. "Never laid out" was the intent and not the
        // behaviour: they went into the window list a layout reads, so
        // dragging a text selection out of an application opened a menu, and
        // the desktop reflowed to make room for it.
        self.take_unmanaged_pane(element);
    }

    fn unmapped_window(&mut self, _xwm: XwmId, window: X11Surface) {
        let going = self
            .space
            .elements()
            .find(|element| element.x11_surface() == Some(&window))
            .cloned();
        if let Some(element) = going {
            if let Some(pane) = self.panes.id_of(&element) {
                self.trigger_close(pane);
            }
            if let Some(id) = self.panes.id_of(&element) {
                self.decorations.remove(&mut self.panes, id);
            }
            self.space.unmap_elem(&element);
        }
        if !window.is_override_redirect()
            && let Err(err) = window.set_mapped(false)
        {
            tracing::warn!(?err, "could not unmap an X11 window");
        }
    }

    fn destroyed_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    /// An X11 window asking to move or resize itself.
    ///
    /// Granted only for windows nothing else owns the geometry of. A tiled
    /// window's size is the layout's decision, and letting the client win
    /// there is how an X11 app ends up fighting the tiler forever.
    fn configure_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        let current = window.geometry();
        let size = (
            w.and_then(|w| i32::try_from(w).ok())
                .unwrap_or(current.size.w),
            h.and_then(|h| i32::try_from(h).ok())
                .unwrap_or(current.size.h),
        );
        let wanted = Rectangle::new(
            (x.unwrap_or(current.loc.x), y.unwrap_or(current.loc.y)).into(),
            (size.0.max(1), size.1.max(1)).into(),
        );
        if let Err(err) = window.configure(Some(wanted)) {
            tracing::warn!(?err, "could not configure an X11 window");
        }
    }

    /// XWayland telling us where a window ended up.
    ///
    /// Only override-redirect windows are followed: for everything else the
    /// `Space` is the authority on position, and taking the client's word for
    /// it would move windows out from under the layout.
    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        geometry: Rectangle<i32, Logical>,
        _above: Option<u32>,
    ) {
        if !window.is_override_redirect() {
            return;
        }
        let element = self
            .space
            .elements()
            .find(|element| element.x11_surface() == Some(&window))
            .cloned();
        if let Some(element) = element {
            self.space.map_element(element, geometry.loc, false);
        }
    }

    /// Interactive resize, started by the client rather than by us.
    ///
    /// Solium resizes on its own terms -- super and the right button, through
    /// the layout -- so this is declined rather than half-honoured.
    fn resize_request(
        &mut self,
        _xwm: XwmId,
        _window: X11Surface,
        _button: u32,
        _edges: smithay::xwayland::xwm::ResizeEdge,
    ) {
    }

    fn move_request(&mut self, _xwm: XwmId, _window: X11Surface, _button: u32) {}

    /// Whether an X11 client may read a selection a Wayland client owns.
    ///
    /// Yes. The default is `false`, which is the right default for a library
    /// that cannot know what its user wants; here the two halves of one
    /// session should be able to paste into each other, and refusing is what
    /// made copying in Steam and pasting in a terminal silently do nothing.
    fn allow_selection_access(&mut self, _xwm: XwmId, _selection: SelectionTarget) -> bool {
        true
    }

    /// An X11 client wants a Wayland client's selection written to `fd`.
    fn send_selection(
        &mut self,
        _xwm: XwmId,
        selection: SelectionTarget,
        mime_type: String,
        fd: std::os::fd::OwnedFd,
    ) {
        self.serve_x11_selection(selection, mime_type, fd);
    }

    /// An X11 client has copied something.
    fn new_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mimes: Vec<String>) {
        tracing::debug!(
            ?selection,
            count = mimes.len(),
            "an X11 client copied something"
        );
        self.take_x11_selection(selection, mimes);
    }

    /// And has let it go again.
    fn cleared_selection(&mut self, _xwm: XwmId, selection: SelectionTarget) {
        self.drop_x11_selection(selection);
    }
}

/// Serve a selection an X11 client owns to the Wayland client that asked.
///
/// The last link, and it lives here rather than on `Solium` because
/// `X11Wm::send_selection` needs the event loop to pump the transfer with, and
/// the two backends have loops over different state types. So the request is
/// recorded where it arrives and carried out where there is a loop — the same
/// arrangement `pending_resize` and `pending_drop` already use, and for the
/// same reason.
pub(crate) fn settle_selection<D>(solium: &mut Solium, handle: &LoopHandle<'static, D>)
where
    D: XwmHandler + 'static,
{
    let Some((selection, mime_type, fd)) = solium.pending_selection.take() else {
        return;
    };
    let Some(xwm) = solium.xwm.as_mut() else {
        return;
    };
    if let Err(err) = xwm.send_selection(selection, mime_type, fd, handle.clone()) {
        tracing::warn!(?err, ?selection, "an X11 selection could not be read");
    }
}

/// Focus an X11 window, if that is what this surface belongs to.
///
/// X11 wants to be told which window is active in its own terms as well as
/// through the keyboard focus, and a client that is not told draws itself
/// unfocused however much typing goes into it.
pub(crate) fn activate(solium: &mut Solium, surface: Option<&WlSurface>) {
    let Some(xwm) = solium.xwm.as_mut() else {
        return;
    };
    let focused = surface.and_then(|surface| {
        solium
            .space
            .elements()
            .find(|element| element.wl_surface().as_deref() == Some(surface))
            .and_then(Window::x11_surface)
            .cloned()
    });
    for element in solium.space.elements() {
        if let Some(x11) = element.x11_surface() {
            let active = focused.as_ref() == Some(x11);
            if let Err(err) = x11.set_activated(active) {
                tracing::warn!(?err, "could not tell an X11 window it has focus");
            }
        }
    }
    if let Some(x11) = focused {
        if let Err(err) = xwm.raise_window(&x11) {
            tracing::warn!(?err, "could not raise an X11 window");
        }
        let _ = SERIAL_COUNTER.next_serial();
    }
}

/// The hardware backend's loop data, forwarding to the compositor inside it.
///
/// XWayland's window manager is driven from the event loop, and on the
/// hardware the loop's data is the backend's own struct rather than the
/// compositor. None of these decisions belong to the backend, so every one of
/// them is handed straight through.
impl HasSolium for crate::tty::State {
    fn solium(&mut self) -> &mut Solium {
        &mut self.solium
    }
}

impl XWaylandShellHandler for crate::tty::State {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        self.solium.xwayland_shell_state()
    }
}

impl XwmHandler for crate::tty::State {
    fn xwm_state(&mut self, xwm: XwmId) -> &mut X11Wm {
        self.solium.xwm_state(xwm)
    }

    fn new_window(&mut self, xwm: XwmId, window: X11Surface) {
        self.solium.new_window(xwm, window);
    }

    fn new_override_redirect_window(&mut self, xwm: XwmId, window: X11Surface) {
        self.solium.new_override_redirect_window(xwm, window);
    }

    fn map_window_request(&mut self, xwm: XwmId, window: X11Surface) {
        self.solium.map_window_request(xwm, window);
    }

    fn mapped_override_redirect_window(&mut self, xwm: XwmId, window: X11Surface) {
        self.solium.mapped_override_redirect_window(xwm, window);
    }

    fn unmapped_window(&mut self, xwm: XwmId, window: X11Surface) {
        self.solium.unmapped_window(xwm, window);
    }

    fn destroyed_window(&mut self, xwm: XwmId, window: X11Surface) {
        self.solium.destroyed_window(xwm, window);
    }

    fn configure_request(
        &mut self,
        xwm: XwmId,
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        reorder: Option<Reorder>,
    ) {
        self.solium
            .configure_request(xwm, window, x, y, w, h, reorder);
    }

    fn configure_notify(
        &mut self,
        xwm: XwmId,
        window: X11Surface,
        geometry: Rectangle<i32, Logical>,
        above: Option<u32>,
    ) {
        self.solium.configure_notify(xwm, window, geometry, above);
    }

    fn resize_request(
        &mut self,
        xwm: XwmId,
        window: X11Surface,
        button: u32,
        edges: smithay::xwayland::xwm::ResizeEdge,
    ) {
        self.solium.resize_request(xwm, window, button, edges);
    }

    fn move_request(&mut self, xwm: XwmId, window: X11Surface, button: u32) {
        self.solium.move_request(xwm, window, button);
    }

    fn allow_selection_access(&mut self, xwm: XwmId, selection: SelectionTarget) -> bool {
        self.solium.allow_selection_access(xwm, selection)
    }

    fn new_selection(&mut self, xwm: XwmId, selection: SelectionTarget, mimes: Vec<String>) {
        self.solium.new_selection(xwm, selection, mimes);
    }

    fn cleared_selection(&mut self, xwm: XwmId, selection: SelectionTarget) {
        self.solium.cleared_selection(xwm, selection);
    }

    fn send_selection(
        &mut self,
        xwm: XwmId,
        selection: SelectionTarget,
        mime_type: String,
        fd: std::os::fd::OwnedFd,
    ) {
        self.solium.send_selection(xwm, selection, mime_type, fd);
    }
}
