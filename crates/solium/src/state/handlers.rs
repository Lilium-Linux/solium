//! The smithay protocol handlers kept with the state -- every `*Handler` impl for `Solium` but
//! idle inhibit's, the session lock's and XWayland's -- with their `delegate_*!` lines and those
//! of viewporter, presentation, relative pointer, cursor shape and the XWayland shell;
//! `compositor_dispatch`; and the helpers they call: popup placement and grabs, the layer and
//! popup configures, validating a move's start, `goes_with` and `decorate`.

use super::*;

impl CompositorHandler for Solium {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        match client.get_data::<ClientState>() {
            Some(state) => &state.compositor_state,
            // Smithay only calls this for clients we created with ClientState,
            // so this is unreachable in practice -- but panicking here would
            // take the session down, so leak a default instead.
            None => Box::leak(Box::new(CompositorClientState::default())),
        }
    }

    fn commit(&mut self, surface: &WlSurface) {
        // Before the buffer is taken below: whether this commit gives a new
        // one to a surface a window that went is still drawn from.
        self.let_go_of_reused(surface);
        // Imports the client's attached buffer into renderer-visible state.
        // Without it every surface is silently empty: the window maps, the
        // client draws, and the compositor renders nothing.
        on_commit_buffer_handler::<Self>(surface);

        // A client committing is the screen changing. Nothing else says so --
        // and on the hardware, where drawing waits to be asked, nothing else
        // was asking: a terminal's own output only reached the screen when
        // some unrelated thing happened to want a frame. Which frame it is
        // and how much of it changed are the damage tracker's business; that
        // it changed at all is this.
        self.redraw = true;

        // What scale and rotation a surface should draw itself at, for clients
        // that never bind `wp_fractional_scale_v1`. That protocol is answered
        // too — see `new_fractional_scale` — but it is the newer one, and a
        // client that only knows `wl_surface.preferred_buffer_scale` would
        // otherwise draw at 1x on a 2x screen and be scaled up.
        if let Some(output) = self.output_for_surface(surface) {
            let scale = output.current_scale().integer_scale();
            let transform = output.current_transform();
            with_states(surface, |states| {
                smithay::wayland::compositor::send_surface_state(surface, states, scale, transform);
            });
        }

        // Sub-surfaces commit through their root; only the root needs handling.
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self.window_for(&root) {
                window.on_commit();
                self.show_if_new(&window);
                // After the first show, which is the window's `open` for one
                // that was not launched: that open already carried the
                // limits, and `trigger_open` recorded them as told.
                self.notice_limits(&window);
            }
        }
        self.popups.commit(surface);
        // After `popups.commit`, which is what moves a popup from the unmapped
        // list into its tree on its first commit. Ordering is not load-bearing
        // — `find_popup` searches both lists — but the configure answers the
        // commit that has just been applied, so it reads in the right order.
        self.configure_popup(surface);
        self.configure_layer(surface);
    }

    /// A surface is going.
    ///
    /// Two kinds need anything doing. The lock screen's: see
    /// `Solium::lock_surface_destroyed`. And a window's own surface -- the root
    /// of a pane's client -- which is how a client that crashed, was killed or
    /// disconnected is first heard of: its objects are destroyed in id order,
    /// and a `wl_surface` is usually older than the toplevel made from it.
    /// smithay calls this before it unlinks the surface from its tree or runs
    /// the hook that drops what the renderer imported for it, so the window's
    /// picture is still here to take (#126). See `crate::remains`.
    ///
    /// **Or one of that window's subsurfaces, for a client that has gone**
    /// (#126's review). Ids are recycled, so a subsurface's `wl_surface` can
    /// be older than the window's own, and then it is destroyed first -- and
    /// unlinked, and its pixels dropped, before the window's surface is heard
    /// of, so the picture taken there had no page or video in it.
    /// `a_client_that_disconnects_keeps_a_subsurface_older_than_its_window`.
    ///
    /// **A client that has gone is told apart from a live one by the
    /// surface's parent, never by the surface going** (#126's second review).
    /// Solium runs on libwayland (`use_system_lib`), where wayland-backend's
    /// `resource_destructor` marks an object dead before smithay calls this,
    /// and a dead object has no client: the surface going answers `None`
    /// whether its client is alive or not, and asking it ended a live window
    /// whose client destroyed a subsurface's `wl_surface` before its
    /// `wl_subsurface`. The parent is still linked -- smithay orphans a
    /// surface's children as it goes -- and it answers `None` exactly when
    /// its client is going: `wl_client_destroy` fires the client's destroy
    /// signal before it destroys any of the client's objects, and that
    /// unhooks the listener wayland-backend finds a client by from any object
    /// of it. `a_live_client_destroying_a_subsurfaces_surface_first_keeps_its_window`
    /// and `a_client_that_disconnects_keeps_a_subsurface_older_than_its_window`.
    ///
    /// A `wl_subsurface` going asks the same, of its surface: see
    /// [`Solium::goes_with`].
    fn destroyed(&mut self, surface: &WlSurface) {
        self.lock_surface_destroyed(surface);
        self.goes_with(surface);
    }
}

impl Solium {
    /// The window `surface` is part of goes now, if `surface` is that
    /// window's own or its client is going: what
    /// [`CompositorHandler::destroyed`] does for a surface, whose doc says
    /// how a client going is told from a live one.
    ///
    /// **And what a `wl_subsurface` going does for its surface** (#126's
    /// second review), since smithay's destructor for that object unlinks the
    /// subsurface from its window and resets where it was, and tells the
    /// compositor nothing. On a disconnect, one older than the window's
    /// surface and than its own went first, so no surface of the window was
    /// heard going until the page or the video had left it. The
    /// `Dispatch<WlSubsurface, _>` below asks this before smithay's
    /// destructor runs. `a_client_that_disconnects_keeps_a_subsurface_whose_wl_subsurface_is_older`.
    fn goes_with(&mut self, surface: &WlSurface) {
        let mut root = surface.clone();
        if get_parent(surface).is_some_and(|parent| parent.client().is_none()) {
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
        }
        let going = self
            .panes
            .iter()
            .find(|pane| {
                pane.client()
                    .and_then(Window::wl_surface)
                    .is_some_and(|own| *own == root)
            })
            .map(Pane::id);
        if let Some(pane) = going {
            self.depart(pane);
        }
    }
}

impl Solium {
    /// Send a layer surface its first configure, so it can draw.
    ///
    /// The protocol says the initial configure goes out in response to the
    /// surface's first commit, and Smithay is deliberate about not sending it
    /// from `arrange` — a client is allowed to set its size *before*
    /// committing, and a configure sent earlier would carry the wrong one.
    /// That leaves it to the compositor, and nothing here was doing it.
    ///
    /// So a bar mapped, took its exclusive zone, and was never told what size
    /// to be — and a client may not attach a buffer until it has been
    /// configured once. Every layer surface was invisible, which means the
    /// claim in `layer.rs` that any existing panel works was untrue for the
    /// whole time it has been written down. Found by `wl-probe` anchoring one
    /// and waiting, which is the entire reason that program exists.
    fn configure_layer(&mut self, surface: &WlSurface) {
        let outputs: Vec<Output> = self.space.outputs().cloned().collect();
        for output in outputs {
            let map = layer_map_for_output(&output);
            let Some(layer) = map
                .layers()
                .find(|layer| layer.layer_surface().wl_surface() == surface)
                .cloned()
            else {
                continue;
            };
            // The map is dropped before arranging: `arrange` takes it again,
            // and the lock is not reentrant.
            drop(map);
            let sent = with_states(surface, |states| {
                states
                    .data_map
                    .get::<LayerSurfaceData>()
                    .and_then(|data| data.lock().ok())
                    .is_some_and(|attributes| attributes.initial_configure_sent)
            });
            if !sent {
                // Arranged first, so the size it is told is the one it will
                // actually be given rather than a guess to be corrected.
                layer::arrange(&output);
                layer.layer_surface().send_configure();
                self.relayout_for_layers();
            }
            return;
        }
    }

    /// A layer surface changed the room windows get, so the layout is re-run.
    fn relayout_for_layers(&mut self) {
        self.trigger_relayout();
        self.redraw = true;
    }
}

impl BufferHandler for Solium {
    fn buffer_destroyed(
        &mut self,
        _buffer: &smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer,
    ) {
    }
}

impl ShmHandler for Solium {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl XdgShellHandler for Solium {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: smithay::wayland::shell::xdg::ToplevelSurface) {
        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Activated);
        });
        // A client may not attach a buffer until it has been configured once.
        surface.send_configure();

        let window = Window::new_wayland_window(surface.clone());
        self.map_stacked(window.clone(), (0, 0), true);
        // On the same line as the map, so nothing can observe a mapped window
        // that has no pane -- `trigger_open` is about to ask for its id.
        self.adopt_or_open(window);

        // No keyboard yet. It is given at the window's first frame, once a
        // layout has said where the window goes: see `offer_keyboard`.
    }

    /// `xdg_toplevel.set_parent` — a window saying which window it belongs to.
    ///
    /// Re-run the layout, for the same reason `modal_changed` does: a modal
    /// dialog is centred on its parent, so the answer to "where does it go"
    /// just changed. It matters more than it looks, because the order is not
    /// the one you would guess. GTK4 creates the toplevel, maps it, and calls
    /// `set_parent` and `set_modal` in whichever order the widget tree settles
    /// in -- so a dialog can easily be laid out once while its parent is still
    /// `Parentage::None`, land in the middle of the screen, and never move
    /// again. Without this, that is the last word.
    ///
    /// Cheap enough not to need a guard: the layout runs off a snapshot, and a
    /// window whose place has not changed is placed where it already is.
    fn parent_changed(&mut self, surface: ToplevelSurface) {
        // **The moment a client says which window its new one is about**, which
        // is the evidence `Solium::refused_with_a_dialog` acts on. Hooked here
        // rather than at the child's first commit because this fires whichever
        // order the client chooses: a toolkit that calls `set_parent` during
        // window setup and one that calls it after mapping both arrive here,
        // and only one of them has committed a buffer by now.
        if let Some(window) = self.window_for(surface.wl_surface()) {
            self.refused_with_a_dialog(&window);
        }
        self.trigger_relayout();
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        // The orderly way out: `exit`, `Ctrl+D`, an application's own Quit.
        // The client destroys its toplevel before its surface, so the surface
        // tree and what the renderer imported for it are all still here, and
        // the window fades out from its last picture (#126). See
        // `Self::depart`, and `crate::remains` for why the picture is there.
        //
        // Bound before the call, so the borrow of `space` ends here rather
        // than lasting across it. Not found for a window whose surface went
        // first -- a client that disconnected -- because `destroyed` below has
        // already taken it out of the space.
        let going = self
            .space
            .elements()
            .find(|window| window.toplevel().is_some_and(|top| *top == surface))
            .cloned();
        if let Some(pane) = going.as_ref().and_then(|window| self.panes.id_of(window)) {
            self.depart(pane);
        }
    }

    /// A menu has gone. If it was the last of the chain `popup_grab` holds,
    /// that grab is over and is let go of here.
    ///
    /// Nothing else ever cleared it. A menu that closed normally stayed
    /// recorded as "the chain holding the seat's grabs", keeping its window's
    /// surface with it, until the next menu replaced it or the next lock
    /// dismissed a chain that had ended long before -- which is harmless today
    /// only because dismissing an ended chain does nothing, and is not what
    /// the field says it is. `has_ended` reads the chain as live until the
    /// popup manager has tidied the dead popup out of it, which is what the
    /// `cleanup` first is for: the same call both backends make every frame.
    fn popup_destroyed(&mut self, _surface: PopupSurface) {
        self.popups.cleanup();
        if self.popup_grab.as_ref().is_some_and(PopupGrab::has_ended) {
            self.popup_grab = None;
        }
    }

    /// A client is opening a menu, a tooltip or a combo-box list.
    ///
    /// The positioner is the client's entire description of *where*: an anchor
    /// rectangle in its parent's coordinates, an edge of that rectangle to
    /// hang from, a direction to hang in, and — the part this handler exists
    /// for — the set of adjustments it permits us to make if the result would
    /// not fit on the screen. Until #100 the argument was named `_positioner`
    /// and dropped, which left Smithay's own initial geometry standing: the
    /// raw `get_geometry()` set in `xdg_surface::GetPopup`, which honours the
    /// anchor and the gravity and nothing else. A menu opened near an edge was
    /// drawn partly off the screen, and the part that was missing was the part
    /// with the entries in it.
    ///
    /// Placed before the popup is tracked so that the first geometry the popup
    /// tree — and therefore the renderer — ever reads is the constrained one;
    /// there is no frame in which the wrong position is on screen.
    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        self.place_popup(&surface, positioner);
        // Tracking failure here is not fatal: the popup simply will not be
        // positioned, which is better than ending the session.
        if let Err(err) = self.popups.track_popup(surface.into()) {
            tracing::warn!(?err, "failed to track popup");
        }
    }

    /// `xdg_popup.grab` — a client asking that a menu own input until it is
    /// dismissed.
    ///
    /// This is what makes a menu behave like a menu. The client asks for the
    /// grab; in return the compositor promises three things, none of which
    /// this did while it was an empty stub: a press anywhere outside the
    /// popup's own client dismisses the whole chain rather than reaching what
    /// it landed on, keyboard focus follows the chain so arrow keys and Escape
    /// go to the menu instead of the document behind it, and when the chain
    /// ends both are handed back to the surface the menu came from. Firefox
    /// asks for this for every context menu, so without it a menu opened and
    /// then could not be closed, dismissed or driven.
    ///
    /// **What releases it, because a grab that is never released leaves a
    /// session in which nothing can be clicked.** There are three exits and
    /// all of them are Smithay's, which is the argument for using its grabs
    /// rather than writing our own:
    ///
    /// * A press outside the grabbing client. `PopupPointerGrab::button`
    ///   compares the client of the surface under the pointer with the client
    ///   of the current grab, dismisses every popup in the chain, and calls
    ///   `handle.unset_grab`. Unsetting a pointer grab runs its `unset`, and
    ///   `PopupPointerGrab::unset` is what takes the keyboard grab off too —
    ///   so the click that closes the menu releases both devices.
    /// * The client destroying the popup. `PopupGrab::has_ended` then answers
    ///   true, and the next pointer motion or key press through either grab
    ///   unsets it. Motion arrives constantly whenever the pointer is in use,
    ///   and `popups.cleanup()` runs every frame from both backends, so this
    ///   is not a path that waits on the client for anything.
    /// * The root toplevel dying. `has_ended` covers that as well: it is
    ///   `!self.root.alive() || !self.toplevel_grab.active()`.
    ///
    /// A refused grab releases nothing because it takes nothing: every early
    /// return below happens before `set_grab` is called, except the one that
    /// has already called `ungrab` to undo what `grab_popup` recorded.
    fn grab(&mut self, surface: PopupSurface, seat: WlSeat, serial: Serial) {
        let Some(seat) = Seat::<Self>::from_resource(&seat) else {
            return;
        };
        let popup = PopupKind::Xdg(surface);

        // The root is computed here rather than left to `grab_popup`, and this
        // is not tidiness. `PopupManager::grab_popup` opens with
        // `assert_eq!(root.wl_surface(), find_popup_root_surface(&popup)?)` —
        // an assertion in a library we cannot annotate, in a compositor with
        // no supervisor to restart it. Deriving the focus we pass from the
        // same function it checks against is the only way to know the two
        // agree. A popup whose parent chain is already dead answers `Err` and
        // is refused here, before the assertion can be reached.
        //
        // Passing the root surface itself works because Solium's
        // `SeatHandler::KeyboardFocus` is a bare `WlSurface` — see the
        // `SeatHandler` impl. A compositor with a richer focus target would
        // have to look the window up; we do not, and that also means a menu
        // rooted in a layer surface (a bar's own menu) is grabbable on the
        // same path as one rooted in a window.
        let Ok(root) = find_popup_root_surface(&popup) else {
            tracing::debug!("refused a popup grab: the popup has no live root");
            return;
        };

        // A grab is the keyboard by another name -- see `grab_keyboard` in
        // `focus.rs` -- so it answers to the same rule, asked here because
        // this is before `grab_popup` has recorded anything that would then
        // have to be undone. A popup's root is a window or a layer surface,
        // never a lock surface, so while locked every grab stops here. Before
        // this line, a client that merely opened a menu behind the lock took
        // the keyboard from the lock screen and kept it.
        if !self.may_hold_keyboard(&root) {
            // And the client is told. The protocol's word for a denied grab is
            // a dismissed popup, and a menu left waiting on a grab that is not
            // coming would still be open when the session unlocked. Sent to
            // the popup directly: it has not been committed yet, so it is in
            // no tree that `PopupManager::dismiss_popup` could find it in.
            if let PopupKind::Xdg(surface) = &popup {
                surface.send_popup_done();
            }
            tracing::debug!("refused a popup grab: the session is locked");
            return;
        }

        // A stale serial is a refusal, not a crash. `grab_popup` returns
        // `Err` for a popup that is already mapped, one whose parent was
        // dismissed, and one that is not the topmost — and posts the protocol
        // error itself where the protocol calls for one, so there is nothing
        // to do here but decline and say so.
        let mut grab = match self.popups.grab_popup(root.clone(), popup, &seat, serial) {
            Ok(grab) => grab,
            Err(err) => {
                tracing::debug!(?err, "refused a popup grab");
                return;
            }
        };

        let keyboard = seat.get_keyboard();
        let pointer = seat.get_pointer();
        // `previous_serial` is the serial of the parent popup's grab, so a
        // submenu opening inside its parent's grab is recognised as the same
        // chain rather than as a stranger trying to steal the device.
        let chain = grab.previous_serial().unwrap_or_else(|| grab.serial());

        // Both devices are tested before either is taken. Anvil checks them
        // one at a time and calls `ungrab` from the middle, which can leave a
        // keyboard grab already installed for a chain that was then dismissed;
        // it recovers on the next key, but there is no reason to enter that
        // state. The case this refuses in practice is a client asking for a
        // menu grab while one of Solium's own grabs is running — a window
        // being dragged by `MoveGrab` or resized by `ResizeGrab` — where
        // handing the pointer to a popup would abandon the drag mid-motion.
        let keyboard_free = keyboard.as_ref().is_none_or(|keyboard| {
            may_grab(
                keyboard.is_grabbed(),
                keyboard.has_grab(serial),
                keyboard.has_grab(chain),
            )
        });
        let pointer_free = pointer.as_ref().is_none_or(|pointer| {
            may_grab(
                pointer.is_grabbed(),
                pointer.has_grab(serial),
                pointer.has_grab(chain),
            )
        });
        if !(keyboard_free && pointer_free) {
            // `grab_popup` has already recorded this popup in the seat's grab
            // chain, so declining now means undoing that — otherwise the next
            // popup would be told its parent holds a grab that nothing is
            // servicing. `All` rather than `Topmost` because the chain this
            // one was appended to is being abandoned with it.
            grab.ungrab(PopupUngrabStrategy::All);
            tracing::debug!("refused a popup grab: a device is grabbed by something else");
            return;
        }

        if keyboard.is_some() {
            // Keyboard before pointer, and the order matters. Installing the
            // pointer grab runs the *previous* pointer grab's `unset`, which
            // for a parent popup's `PopupPointerGrab` tries to take the
            // keyboard grab off again. It only does so if the keyboard grab's
            // serial is the parent's, so setting ours first is what makes a
            // submenu keep the keyboard instead of handing it back to the
            // window while its menu is still open.
            //
            // `give_keyboard` moves the selection focus with the keyboard, as
            // it does everywhere, and a menu opened from an unfocused window
            // is exactly the case where the two would otherwise part company:
            // the popup would take the keyboard while the clipboard still
            // answered to whoever had it before. Both are per-client, so for
            // the ordinary case of a menu in the already-focused window this
            // changes nothing.
            let focus = grab.current_grab();
            self.give_keyboard(focus, serial);
            self.grab_keyboard(&root, PopupKeyboardGrab::new(&grab), serial);
        }
        if let Some(pointer) = pointer {
            // `Focus::Keep`, not `Focus::Clear` as Solium's move and resize
            // grabs use: those want the pointer to stop pointing at anything
            // for the duration, whereas a menu is being pointed *at* and must
            // keep receiving enter/motion so its entries highlight.
            pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
        }
        // Kept, so that locking can dismiss the chain: see `release_grabs`.
        self.popup_grab = Some(grab);
    }

    /// A window asking for the whole screen.
    ///
    /// Not the same as maximised, and the difference is the whole point: a
    /// maximised window fills the *work area* and keeps its frame, a
    /// fullscreen one covers the monitor edge to edge with no frame and no
    /// bar over it. A video player, a game, a presentation. Without this the
    /// request was ignored entirely — the client had asked, been told nothing,
    /// and drew its own idea of fullscreen inside a titlebar.
    ///
    /// The monitor the window is on, not the active one: a video sent
    /// fullscreen on the second screen must not jump to whichever screen the
    /// pointer is over.
    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        wl_output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
    ) {
        let Some(window) = self.window_for(surface.wl_surface()) else {
            return;
        };
        let Some(id) = self.panes.id_of(&window) else {
            return;
        };
        let output = wl_output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| {
                self.real_geometry(&window)
                    .and_then(|real| self.output_of(real))
            })
            .or_else(|| self.active_output());
        let Some(screen) = output.and_then(|output| self.space.output_geometry(&output)) else {
            return;
        };

        // Where to come back to, kept before anything moves. The same slot a
        // maximise keeps, and for the same reason: a rect that was stored is a
        // rect that comes back exactly, where one recomputed afterwards is a
        // guess.
        //
        // Not by a window that is fullscreen already: a client may ask a
        // second time, and the rect it has by then is the monitor's. Asked of
        // the xdg state rather than of whether a rect is kept, because a
        // window that went fullscreen before it had drawn has none kept (see
        // below), and its second request would otherwise keep the monitor.
        //
        // Not over a rect kept already, either. A window maximised and then
        // sent fullscreen keeps the rect from before the maximise, and it stays
        // there while `unfullscreen_request` puts the window back to maximised,
        // for the un-maximise after that to take.
        //
        // And not a rect of no size. `new_toplevel` maps a window at 0,0 before
        // it has a buffer, so one asking for fullscreen before its first
        // commit -- a player started with `--fs` -- is there with no size, and
        // a rect that describes nothing is no way back. With none kept,
        // leaving fullscreen lets the client pick its own size.
        let already = surface
            .with_pending_state(|state| state.states.contains(xdg_toplevel::State::Fullscreen));
        if !already
            && let Some(real) = self.real_geometry(&window)
            && !real.is_empty()
            && let Some(pane) = self.panes.get_mut(id)
            && pane.restore().is_none()
        {
            pane.set_restore(Some(real));
        }
        // Out of its tile, for `toggle_maximize`'s reason: a fullscreen window
        // cut down to the tile it came from is a video playing in a corner of
        // the monitor. Unconditionally rather than beside the rect above,
        // because a window that is already fullscreen, or was maximised first,
        // may have been put back in a tile by a sweep since -- and
        // `leave_tile` keeps an older way back when there is no tile to take.
        if let Some(pane) = self.panes.get_mut(id) {
            pane.leave_tile();
        }

        // The whole monitor, and no frame over it. The frame is dropped and
        // the pane marked bare, and leaving fullscreen builds a new one from
        // the style that is current then.
        //
        // Which is why the way back is kept on the pane and not on the frame:
        // it was kept on the frame until #92, and `remove` below dropped it
        // with the frame, so leaving fullscreen never had a rect to put any
        // window back at.
        self.decorations.remove(&mut self.panes, id);
        self.decorations.set_bare(&mut self.panes, id);

        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Fullscreen);
            state.size = Some(screen.size);
        });
        if surface.is_initial_configure_sent() {
            surface.send_pending_configure();
        }
        if let Some(pane) = self.panes.get_mut(id) {
            pane.set_slot(screen);
        }
        self.map_stacked(window, screen.loc, true);
        self.redraw = true;
        tracing::debug!(?screen, "a window went fullscreen");
    }

    /// And asking for it back.
    ///
    /// Back to maximised if the window was maximised when it went fullscreen,
    /// and otherwise to the rect it had before. Maximise and fullscreen keep
    /// their way back in the one slot on the pane, and that is what the two
    /// checks below are about.
    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        let Some(window) = self.window_for(surface.wl_surface()) else {
            return;
        };
        let Some(id) = self.panes.id_of(&window) else {
            return;
        };

        // Only a window that is fullscreen has anything to leave. A client may
        // send this whenever it likes, and one that sent it while merely
        // maximised had the maximise's way back spent on it: the window jumped
        // to its pre-maximise rect still marked maximised, and the next toggle
        // maximised it again rather than restoring it.
        let (fullscreen, maximized) = surface.with_pending_state(|state| {
            (
                state.states.contains(xdg_toplevel::State::Fullscreen),
                state.states.contains(xdg_toplevel::State::Maximized),
            )
        });
        if !fullscreen {
            return;
        }

        // The frame comes back unless the client draws its own, which is what
        // `is_bare` cannot tell us on its own -- so the decoration mode is
        // asked again rather than assumed.
        let client_side =
            surface.with_pending_state(|state| state.decoration_mode) == Some(Mode::ClientSide);
        if !client_side {
            self.decorations.unset_bare(&mut self.panes, id);
            let size = self
                .real_geometry(&window)
                .map_or((TITLEBAR_HEIGHT * 20, TITLEBAR_HEIGHT * 15), |real| {
                    (real.size.w, real.size.h)
                });
            self.decorations.insert(&mut self.panes, id, size.0, size.1);
        }

        // Where it goes, decided after the frame is back -- a maximised
        // window's share of the work area depends on it -- and before the
        // client is told anything, so that it is told once. It used to be two
        // configures, no size and then the size to go back to, and a client
        // that acts on every configure it reads, a terminal reflowing its grid,
        // resized twice.
        let back = if maximized {
            // Still maximised: nothing has un-maximised it. So it fills the
            // work area of the monitor it is on now, and the rect from before
            // the maximise stays kept for the un-maximise to take. Taking it
            // here placed and sized the window un-maximised while its state
            // still said `Maximized`, and left the next toggle nothing to
            // restore.
            self.real_geometry(&window)
                .and_then(|real| self.maximised(&window, real))
        } else {
            // Back into the tile it left as well, with the rect that goes with
            // it; see `toggle_maximize`. A window still maximised stays out of
            // one, which is the arm above.
            let kept = self.panes.get_mut(id).and_then(|pane| {
                pane.return_to_tile();
                pane.take_restore()
            });
            kept.map(|kept| self.back_on_a_screen(&window, kept))
        };

        surface.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Fullscreen);
            // No rect, no size: the client picks its own.
            state.size = back.map(|back| back.size);
        });
        if surface.is_initial_configure_sent() {
            surface.send_pending_configure();
        }
        if let Some(back) = back {
            if let Some(pane) = self.panes.get_mut(id) {
                pane.set_slot(back);
            }
            self.map_stacked(window, back.loc, true);
        }
        self.trigger_relayout();
        self.redraw = true;
        tracing::debug!("a window left fullscreen");
    }

    /// A client asking to be dragged — what client-side decorations send when
    /// their own titlebar is grabbed.
    fn move_request(&mut self, surface: ToplevelSurface, seat: WlSeat, serial: Serial) {
        let Some(seat) = Seat::<Self>::from_resource(&seat) else {
            return;
        };
        let Some(start_data) = self.drag_start_data(&seat, surface.wl_surface(), serial) else {
            return;
        };
        let Some(window) = self.window_for(surface.wl_surface()) else {
            return;
        };
        let Some(location) = self.space.element_location(&window) else {
            return;
        };
        let Some(pointer) = seat.get_pointer() else {
            return;
        };

        pointer.set_grab(
            self,
            MoveGrab::new(start_data, window, location),
            serial,
            Focus::Clear,
        );
    }

    /// A popup asking to be moved — a submenu re-anchoring as the pointer
    /// walks down its parent, or a reactive popup whose window has moved.
    ///
    /// Through `place_popup` for the same reason `new_popup` is: this used to
    /// take the positioner's raw geometry, so a submenu that opened inside the
    /// screen and then repositioned towards an edge was pushed off it by the
    /// very request meant to keep it visible.
    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        self.place_popup(&surface, positioner);
        surface.send_repositioned(token);
    }
}

/// Whether a popup grab may take a device that something already holds.
///
/// The three arguments are what the seat can answer about one device: whether
/// it is grabbed at all, whether the grab's serial is the one this popup is
/// being grabbed with, and whether it is the serial of the parent popup's
/// grab. A free device is takeable; so is one already held by this chain,
/// which is the submenu case and the common one. Anything else belongs to
/// somebody — one of Solium's own move or resize grabs, or another client's
/// menu — and the request is declined.
///
/// Booleans rather than the handles themselves so the rule can be tested
/// without a seat, a client and a live popup, none of which exist in a unit
/// test. See `a_grab_held_by_a_stranger_is_refused`.
pub(super) const fn may_grab(grabbed: bool, this_popup: bool, its_parent: bool) -> bool {
    !grabbed || this_popup || its_parent
}

/// The rectangle a popup has to stay inside, in the coordinates its positioner
/// speaks.
///
/// A positioner's geometry is relative to the *parent surface's* window
/// geometry, and the screen is in the compositor's coordinates, so the two
/// have to be brought together before `get_unconstrained_geometry` can compare
/// them. Two translations separate them: where the root toplevel's window
/// geometry sits on the desktop, and — for a submenu — how far down the chain
/// of parent popups this one hangs.
///
/// Expressed as a subtraction from the screen rather than an addition to the
/// popup because the popup's position is the unknown: it is what the
/// positioner is about to work out.
pub(super) fn popup_target(
    screen: Rectangle<i32, Logical>,
    root: Point<i32, Logical>,
    parents: Point<i32, Logical>,
) -> Rectangle<i32, Logical> {
    Rectangle::new(screen.loc - root - parents, screen.size)
}

impl Solium {
    /// Work out where a popup goes and put it in the pending state.
    ///
    /// The positioner is stored alongside the geometry because a *reactive*
    /// popup is re-constrained later, when its window moves or the screen
    /// changes, and the rules to re-run it with are the ones the client sent
    /// with the original request.
    ///
    /// The unconstrained geometry when there is a screen to constrain
    /// against, and the client's own placement when there is not — a popup
    /// rooted in something that is not a mapped window, which today means a
    /// layer surface's menu. That fallback is the behaviour this whole path
    /// replaces, so the worst case is what every popup used to get.
    fn place_popup(&self, surface: &PopupSurface, positioner: PositionerState) {
        let geometry = match self.popup_screen(surface, positioner) {
            Some(target) => positioner.get_unconstrained_geometry(target),
            None => positioner.get_geometry(),
        };
        surface.with_pending_state(|state| {
            state.positioner = positioner;
            state.geometry = geometry;
        });
    }

    /// The screen a popup must fit on, in its positioner's coordinates.
    ///
    /// The monitor under the popup's *anchor point* rather than the one its
    /// window is mostly on. They differ exactly where it matters: a window
    /// straddling two screens has a right-click menu that belongs to whichever
    /// screen the pointer was over, and constraining it to the other one would
    /// shove it back across the seam it was opened on.
    fn popup_screen(
        &self,
        surface: &PopupSurface,
        positioner: PositionerState,
    ) -> Option<Rectangle<i32, Logical>> {
        let popup = PopupKind::Xdg(surface.clone());
        let root = find_popup_root_surface(&popup).ok()?;
        let window = self.window_for(&root)?;
        // `element_location` is the window *geometry* origin, which is the
        // origin a positioner measures from — not the buffer origin, which for
        // a client with its own shadows is a couple of dozen pixels up and
        // left of it. `real_geometry` is that pairing, and `render.rs` places
        // popups against the same point.
        let real = self.real_geometry(&window)?;
        let parents = get_popup_toplevel_coords(&popup);
        let anchor = real.loc + parents + positioner.get_anchor_point();
        let output = self.output_at(anchor)?;
        let screen = self.space.output_geometry(&output)?;
        Some(popup_target(screen, real.loc, parents))
    }

    /// Send a popup its first configure, so it can draw.
    ///
    /// The same omission `configure_layer` was written for, and with the same
    /// consequence: xdg-shell forbids a client to attach a buffer before it
    /// has been configured once, Smithay deliberately leaves the initial
    /// configure to the compositor, and nothing here was sending one. A menu
    /// was created, tracked, and then waited forever for an event that was
    /// never coming — which is why Firefox's context menus did not merely
    /// appear in the wrong place, they did not appear.
    ///
    /// It also matters to the placement above. `PopupKind::location`, which is
    /// what the renderer positions a popup by, reads the *current* geometry,
    /// and `current` is only taken from the client's ack — so until a
    /// configure goes out, every popup's position stays at the default of
    /// (0, 0) no matter what `place_popup` computed.
    ///
    /// On commit rather than at `new_popup` because that is what the protocol
    /// says: the configure answers the surface's first commit. Sending one
    /// earlier would carry a size the client had not finished asking for.
    fn configure_popup(&mut self, surface: &WlSurface) {
        // Only xdg popups. An input-method popup is positioned by the
        // text-input protocol and has no configure of this kind.
        let Some(PopupKind::Xdg(popup)) = self.popups.find_popup(surface) else {
            return;
        };
        if popup.is_initial_configure_sent() {
            return;
        }
        if let Err(err) = popup.send_configure() {
            // Not fatal, and not ours to retry: the two failures Smithay
            // reports here are a client too old to be re-configured and a
            // non-reactive positioner, both of which mean the popup keeps the
            // geometry it already has.
            tracing::warn!(?err, "could not configure a popup");
        }
    }

    /// Validate a client's request to start a drag.
    ///
    /// A client may only be dragged from a press it actually received: the
    /// serial has to match the live grab, and the surface that was pressed has
    /// to belong to the same client as the one asking. Without both checks any
    /// client could start a drag of any window at any time.
    fn drag_start_data(
        &self,
        seat: &Seat<Self>,
        surface: &WlSurface,
        serial: Serial,
    ) -> Option<GrabStartData<Self>> {
        use smithay::reexports::wayland_server::Resource;

        let pointer = seat.get_pointer()?;
        if !pointer.has_grab(serial) {
            return None;
        }
        let start_data = pointer.grab_start_data()?;
        let (focused, _) = start_data.focus.as_ref()?;
        if !focused.id().same_client_as(&surface.id()) {
            return None;
        }
        Some(start_data)
    }
}

impl WlrLayerShellHandler for Solium {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        wl_output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        // A surface may name an output or leave the choice to us; a shell that
        // puts a bar on each screen names one per bar, and that is the request
        // that has to be honoured for the second screen to get a bar at all.
        //
        // The primary monitor when it names none, and not the active one: a
        // dock connects at startup, and where the pointer happened to be then
        // is not a decision anybody made. See `primary_output`.
        let output = wl_output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| self.primary_output());
        let Some(output) = output else {
            tracing::warn!(
                namespace,
                "a layer surface arrived with no output to put it on"
            );
            return;
        };

        // The protocol object becomes a desktop surface, which is what carries
        // the geometry and can be arranged.
        let surface = LayerSurface::new(surface, namespace.clone());
        if let Err(err) = layer_map_for_output(&output).map_layer(&surface) {
            tracing::warn!(?err, namespace, "could not map a layer surface");
            return;
        }
        // Arranging assigns the size and position the client is waiting to be
        // told; it must happen before the client can draw anything.
        layer::arrange(&output);
        tracing::info!(namespace, monitor = output.name(), "layer surface mapped");
    }

    fn ack_configure(&mut self, _surface: WlSurface, _configure: LayerSurfaceConfigure) {}

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        // Unmapped *and* rearranged: the exclusive zone it held is now free,
        // and the work area is wrong until someone recomputes it.
        for output in self.space.outputs().cloned().collect::<Vec<_>>() {
            let mut map = layer_map_for_output(&output);
            let found = map
                .layers()
                .find(|layer| layer.layer_surface() == &surface)
                .cloned();
            if let Some(layer) = found {
                map.unmap_layer(&layer);
                drop(map);
                layer::arrange(&output);
            }
        }
        tracing::info!("layer surface gone");
    }
}

impl XdgDecorationHandler for Solium {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        // Server-side is offered without being asked: the frame is part of the
        // desktop's look, and a client drawing its own would be a second
        // titlebar with different rules.
        self.decorate(&toplevel, Mode::ServerSide);
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: Mode) {
        // A client that insists on drawing its own frame gets to: overriding it
        // means two frames or none, depending on who gives way.
        self.decorate(&toplevel, mode);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self.decorate(&toplevel, Mode::ServerSide);
    }
}

impl Solium {
    /// Agree a decoration mode with a client and act on it.
    fn decorate(&mut self, toplevel: &ToplevelSurface, mode: Mode) {
        let server_side = mode != Mode::ClientSide;

        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(if server_side {
                Mode::ServerSide
            } else {
                Mode::ClientSide
            });
        });

        // The frame belongs to the pane, so a client with no pane gets none.
        // It has still been told its mode, which is the part it is waiting on.
        let window = self.window_for(toplevel.wl_surface());
        let Some(id) = window.as_ref().and_then(|window| self.panes.id_of(window)) else {
            tracing::debug!(server_side, "decoration agreed for a window with no pane");
            if toplevel.is_initial_configure_sent() {
                toplevel.send_pending_configure();
            }
            return;
        };

        if server_side {
            let real = window.and_then(|window| self.real_geometry(&window));
            let width = real.map_or(TITLEBAR_HEIGHT * 20, |real| real.size.w);
            let height = real.map_or(TITLEBAR_HEIGHT * 15, |real| real.size.h);
            self.decorations.insert(&mut self.panes, id, width, height);
        } else {
            // Bare on purpose, not merely undecorated: the difference is
            // whether `insets_of` still reserves room for a frame that is
            // coming. For a client drawing its own, none is.
            self.decorations.remove(&mut self.panes, id);
            self.decorations.set_bare(&mut self.panes, id);
        }

        // The client has to learn its mode before it draws, or it decides for
        // itself and draws a frame we then draw over.
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
        tracing::debug!(server_side, "decoration mode agreed");
    }
}

impl XdgActivationHandler for Solium {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.activation_state
    }

    /// A window asking to be brought forward.
    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        // Ours, from a launch: the window belongs in the one we opened for it.
        if let Some(pane) = data.user_data.get::<LaunchedFor>().map(|it| it.0)
            && self.claim_into(pane, &surface)
        {
            self.activation_state.remove_token(&token);
            return;
        }

        // Anyone else's: a window asking for focus, which is what the protocol
        // is for, and honoured whoever minted the token. **That is not focus-
        // stealing prevention, and there is none.** This comment used to say a
        // client could not mint a token for itself; it can. `token_created` is
        // not overridden, and smithay's default keeps every token a client asks
        // for -- with no input serial and no surface -- so any client can make
        // one and bring itself forward:
        // `a_client_can_mint_itself_an_activation_token_and_take_the_keyboard_with_it`
        // pins that as it stands.
        //
        // **A window on a workspace nobody is looking at is refused before it
        // is focused**, and it is refused rather than focused and handed back.
        // Focused first, as it was until #134's third review, it was raised,
        // told it had the keyboard and the clipboard, and announced to the
        // scripts as focused -- and then `hand_off_keyboard`'s `settle_focus`
        // gave the keyboard to the window under the pointer or the topmost,
        // which with two tiles on screen need not be the one that had it
        // (`a_genuine_activation_of_a_window_on_a_hidden_workspace_leaves_the_keyboard_exactly_where_it_was`).
        // `carried_by_a_selection` asks it of the desk the window is on, not of
        // where its frame lands, so a column scrolled off a hidden desk and a
        // window being closed there are refused as well
        // (`a_genuine_activation_of_a_column_scrolled_off_a_hidden_workspace_leaves_the_keyboard_exactly_where_it_was`,
        // `a_genuine_activation_of_a_window_being_closed_on_a_hidden_workspace_leaves_the_keyboard_exactly_where_it_was`),
        // and so is a desk faded to nothing
        // (`a_genuine_activation_of_a_window_a_selection_fades_to_nothing_leaves_the_keyboard_exactly_where_it_was`);
        // while a column scrolled off the desk in view is still focused, which
        // is how the strip brings it back
        // (`a_genuine_activation_of_a_column_scrolled_off_screen_brings_it_back_with_the_keyboard`).
        //
        // **So is a window being closed, wherever it is**: `Pane::leaving`, on
        // its way off the screen. Until #134's fifth review one on the desk in
        // view was focused, found to be headed nowhere, and handed off -- and
        // the keyboard went to the tile under the pointer
        // (`a_genuine_activation_of_a_window_being_closed_on_the_desk_in_view_leaves_the_keyboard_exactly_where_it_was`,
        // and on one tile `a_genuine_activation_of_a_window_being_closed_does_not_keep_the_keyboard`).
        //
        // A window still headed nowhere the user can see once it is focused
        // and the layouts have had their say -- one a script holds invisible
        // on the desk in view, say -- gives the keyboard up again, to whatever
        // `settle_focus` picks
        // (`a_genuine_activation_of_a_window_its_own_frame_hides_does_not_keep_the_keyboard`).
        // Before #134's second review it kept it, and every key typed went
        // somewhere nobody could see.
        //
        // Switching to that workspace is arguably what a click on its
        // notification should do. That is `workspaces.lua`'s decision rather
        // than the compositor's, which does not know what a workspace is, and
        // nothing tells it an activation happened yet. Until something does, a
        // click like that changes nothing on screen, as it never did, and the
        // typing no longer goes with it.
        if let Some(window) = self.window_for(&surface) {
            let pane = self.panes.id_of(&window);
            if pane
                .and_then(|pane| self.panes.get(pane))
                .is_some_and(Pane::leaving)
            {
                tracing::debug!(
                    "a window being closed asked to be brought forward, and the keyboard stayed \
                     where it was"
                );
            } else if pane.is_some_and(|pane| self.carried_by_a_selection(pane)) {
                tracing::debug!(
                    "a window on a workspace nobody is looking at asked to be brought forward, \
                     and the keyboard stayed where it was"
                );
            } else {
                tracing::debug!("a window asked to be brought forward");
                self.focus_window(&window, SERIAL_COUNTER.next_serial());
                if !pane.is_some_and(|pane| self.pane_on_stage(pane)) {
                    tracing::debug!(
                        "a window asked to be brought forward where nobody can see it, and the \
                         keyboard went back on screen"
                    );
                    self.hand_off_keyboard(&window);
                }
            }
        }
        self.activation_state.remove_token(&token);
    }
}
smithay::delegate_xdg_activation!(Solium);

impl FractionalScaleHandler for Solium {
    /// A client has asked what scale it is really drawn at.
    ///
    /// `fractional_scale_for` has the answer, and how it is worked out; this
    /// is only the protocol's first-ask moment. The other one -- an output's
    /// scale changing later, after a client already asked -- is
    /// `resend_fractional_scale`, called from `scale_outputs`.
    fn new_fractional_scale(
        &mut self,
        surface: smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) {
        let scale = self.fractional_scale_for(&surface);
        with_states(&surface, |states| {
            with_fractional_scale(states, |fractional| {
                fractional.set_preferred_scale(scale);
            });
        });
    }
}
smithay::delegate_fractional_scale!(Solium);
// No `delegate_screencopy!`: Smithay has no handler for it, so `screencopy.rs`
// writes the `Dispatch` impls itself and there is nothing to delegate to.

smithay::delegate_viewporter!(Solium);
smithay::delegate_presentation!(Solium);

impl PointerConstraintsHandler for Solium {
    /// A client has asked for the pointer to be held still or kept inside a
    /// region.
    ///
    /// Granted straight away when the surface already has the pointer. A
    /// constraint is a request from a window that believes it is being used —
    /// a game entering mouse-look — and the honest test of that is whether the
    /// pointer is over it. One that is not is left inactive; the protocol
    /// expects it to be activated later, and the pointer arriving is when.
    fn new_constraint(
        &mut self,
        _surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
        _pointer: &smithay::input::pointer::PointerHandle<Self>,
    ) {
        // Deliberately not activated here.
        //
        // This runs inside the client's own `lock_pointer` request, and
        // activating sends `locked()` straight back down the same dispatch --
        // before the client has finished setting up. Firefox creates its
        // relative pointer six microseconds after asking for the lock and
        // attaches its handlers after that; `locked()` arriving in between was
        // discarded, and a lock the client never saw confirmed is a lock it
        // does not act on. The relative motion was delivered perfectly and the
        // page ignored every event of it.
        //
        // So it is activated on the next pointer motion instead, in `held`,
        // which is both a later dispatch and the first moment the answer
        // actually matters.
        tracing::debug!("a window asked for the pointer");
    }

    /// A locked pointer's client saying where it would like the cursor left.
    ///
    /// Taken as advice and acted on when the lock ends, which is what the hint
    /// is for: a game that locked the pointer in the middle of its window wants
    /// it back in the middle, not wherever it happened to be when the lock was
    /// taken.
    fn cursor_position_hint(
        &mut self,
        surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
        pointer: &smithay::input::pointer::PointerHandle<Self>,
        location: Point<f64, Logical>,
    ) {
        let active = with_pointer_constraint(surface, pointer, |constraint| {
            constraint.is_some_and(|constraint| constraint.is_active())
        });
        if !active {
            return;
        }
        if let Some(origin) = self
            .window_for(surface)
            .and_then(|window| self.real_geometry(&window))
        {
            self.constraint_hint = Some(origin.loc.to_f64() + location);
        }
    }
}
smithay::delegate_pointer_constraints!(Solium);
smithay::delegate_relative_pointer!(Solium);

impl SeatHandler for Solium {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    /// Both client-side cursor sources arrive here, and which one it was is
    /// readable off the variant.
    ///
    /// `Surface` and `Hidden` are `wl_pointer.set_cursor`: the client
    /// rasterised a cursor itself and we draw its pixels. `Named` is
    /// `wp_cursor_shape_v1.set_shape` — the only thing that can produce one,
    /// since `set_cursor` carries a surface or nothing — and it means the
    /// client named a shape and left the picture to us. Dropping either on the
    /// floor leaves every application with our arrow, which for the named case
    /// is exactly what #24 was: a text field that never showed an I-beam.
    ///
    /// Through `show` rather than assigned, so that a client alternating
    /// between its own two mechanisms cannot leave a fragment of the other
    /// behind. See `cursor::Pointer::show`, which is the only writer and where
    /// the precedence between all three sources is set out.
    ///
    /// **And it damages the screen, which is not optional.** Both backends draw
    /// only when something has changed — `tty.rs`'s loop and `winit.rs`'s make
    /// the same test, each with a comment arguing for it — and a client's reply
    /// to `wl_pointer.enter` arrives a round trip *after* the motion that
    /// provoked it, by which time the frame that motion caused has already been
    /// drawn. Without this, moving onto a text field and stopping leaves the
    /// old arrow sitting there until something unrelated damages the screen,
    /// and jiggling the mouse is the only way to see the I-beam.
    ///
    /// It cost nothing before #24 only because `Named(_)` was discarded, so the
    /// picture genuinely did not change. It is now the main visible path of the
    /// whole feature, and a feature that only works while the mouse is moving
    /// reads as a broken one.
    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.pointer.show(image);
        self.redraw = true;
    }
    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) {}
}

/// Required by smithay's `wp_cursor_shape_v1` dispatch, and empty on purpose.
///
/// The protocol hands out a shape device for a `wl_pointer` *or* for a
/// `zwp_tablet_tool_v2`, so its `Dispatch` impl is bound on `TabletSeatHandler`
/// whether or not the compositor has tablets — see
/// `wayland/cursor_shape.rs:240`. Solium advertises no tablet manager, so no
/// client can ever hold a `zwp_tablet_tool_v2` to ask for one, and
/// `tablet_tool_image` is unreachable rather than unimplemented. The default
/// body discards the image, which is the right thing for a cursor that has no
/// device to be drawn for.
impl smithay::wayland::tablet_manager::TabletSeatHandler for Solium {}

impl DmabufHandler for Solium {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    /// A client has built a buffer out of GPU memory and wants to know whether
    /// it is usable.
    ///
    /// Accepted on the strength of the format list the global was created with
    /// — the renderer's own — rather than by importing here, because the
    /// renderer belongs to the backend and this does not. The real import
    /// happens when the buffer is committed, and says so in the log if it
    /// fails. The honest cost: a client whose buffer we cannot import is told
    /// "yes" and then shows nothing, instead of being told "no" and falling
    /// back to shared memory.
    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        _dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        if let Err(err) = notifier.successful::<Self>() {
            tracing::warn!(?err, "could not accept a client's dmabuf");
        }
    }
}
delegate_dmabuf!(Solium);

impl SelectionHandler for Solium {
    type SelectionUserData = ();

    /// A Wayland client has copied something. Tell the X11 side it exists.
    ///
    /// Only that it exists, and in which formats — the data itself is not moved
    /// anywhere. X11 selections are the same idea: the owner advertises types
    /// and hands over bytes when someone asks. Copying a megabyte in one
    /// toolkit and pasting nothing in the other should cost nothing.
    fn new_selection(
        &mut self,
        ty: SelectionTarget,
        source: Option<SelectionSource>,
        _seat: Seat<Self>,
    ) {
        let Some(xwm) = self.xwm.as_mut() else {
            return;
        };
        let mimes = source.map(|source| source.mime_types());
        if let Err(err) = xwm.new_selection(ty, mimes) {
            tracing::warn!(?err, ?ty, "could not offer a selection to X11");
            return;
        }

        // Make the X server actually hear about it.
        //
        // `new_selection` issues `SetSelectionOwner` and does not flush, and
        // x11rb buffers requests -- so the ownership change sits in the output
        // buffer until something unrelated forces a flush, which is the next X
        // event to arrive. If none does, the X server still believes nobody
        // owns the selection: a client asking gets nothing, and the compositor
        // is never even consulted. Copying in a Wayland app and pasting in an
        // X11 one worked or did not depending on whether anything else
        // happened to be talking to X, which is as good as a coin toss.
        //
        // There is no public `flush` on `X11Wm`. This is a read-only query
        // that round-trips, and a round-trip has to flush the output buffer
        // before it can wait for the reply. The answer is discarded; the flush
        // is the point.
        let _ = xwm.get_randr_primary_output();
    }

    /// A Wayland client wants to read a selection an X11 client owns.
    ///
    /// Recorded here and carried out by the backend: see `pending_selection`.
    fn send_selection(
        &mut self,
        ty: SelectionTarget,
        mime_type: String,
        fd: std::os::fd::OwnedFd,
        _seat: Seat<Self>,
        (): &(),
    ) {
        self.pending_selection = Some((ty, mime_type, fd));
    }
}

impl OutputHandler for Solium {}

impl DataDeviceHandler for Solium {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}
impl PrimarySelectionHandler for Solium {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}

impl ClientDndGrabHandler for Solium {
    /// A client has started a drag, and this is the only moment its icon is
    /// offered. See [`Solium::dnd_icon`] for why nothing can ask for it later.
    ///
    /// `icon` is `None` for a drag the client chose not to illustrate, which is
    /// ordinary and not a failure — a text selection dragged inside one window
    /// often has no icon at all. Stored as-is: the surface already carries the
    /// `dnd_icon` role, which smithay gave it in `data_device::device.rs`
    /// before this is called and which is what stops the same surface being a
    /// cursor or a toplevel at the same time.
    ///
    /// No cursor assertion is made here, and that is deliberate. A drag begun
    /// with the pointer installs a `DnDGrab` on it, so `pointer.is_grabbed()`
    /// is true for the whole gesture, and that one test holds *both* halves of
    /// the pointer: [`Solium::assert_cursor`] declines to recompute `chrome`,
    /// and [`crate::input::release_cursor`] declines to clear `status` over the
    /// gaps the drag crosses. The drag owns the pointer until it ends, exactly
    /// as a resize drag does, and it needs no flag of its own to say so.
    ///
    /// So the compositor says nothing about the shape for the length of the
    /// drag — which is not the same as the shape being frozen, and the
    /// difference matters. The drag's own client may still change it, and is
    /// meant to: `wl_pointer.set_cursor` is accepted from the holder of a grab
    /// (`wayland/seat/pointer.rs:521`, and its comment names drag and drop),
    /// so a toolkit swapping between `dnd-copy` and `dnd-no-drop` as it crosses
    /// drop targets reaches [`SeatHandler::cursor_image`] mid-drag and is
    /// obeyed. What is held is the compositor's hands off it.
    ///
    /// **The grab is the pointer's only for a pointer-initiated drag.**
    /// `start_drag` installs it on whichever device the start serial came from:
    /// a pointer serial takes `PointerHandle::set_grab`, a touch serial takes
    /// `TouchHandle::set_grab` and returns before the pointer branch is ever
    /// reached — `selection/data_device/device.rs:95`. A drag begun with a
    /// finger therefore leaves `pointer.is_grabbed()` false for its whole
    /// length, and neither guard above holds anything. That is reachable rather
    /// than hypothetical: [`Solium::new`] calls `seat.add_touch()`, and
    /// `input::handle` routes `TouchDown` into it.
    ///
    /// It costs nothing today because a finger moves no pointer. Both guarded
    /// writes sit on the pointer motion paths, so a touch drag reaches neither
    /// unless a mouse is moved alongside the finger — and at that point the
    /// pointer honestly is not the thing dragging, so describing what is under
    /// it is the right answer rather than a missed one. What a touch drag does
    /// get wrong is the icon, which `render::elements` puts at the pointer
    /// because the pointer is the only position it has; re-deriving that from
    /// the touch grab is the work touch support will bring, and the cursor
    /// rules here are the pointer's and stay the pointer's.
    fn started(
        &mut self,
        _source: Option<WlDataSource>,
        icon: Option<WlSurface>,
        _seat: Seat<Self>,
    ) {
        self.dnd_icon = icon;
        // The icon appears at the pointer on the next frame and nothing else
        // on screen has changed, so without this a drag started without moving
        // the mouse would draw nothing until something unrelated redrew.
        self.redraw = true;
    }

    /// The buttons came up. Whether the drop was accepted or refused, the icon
    /// stops being drawn now.
    ///
    /// **This is the reliable end of a drag, and the only one.** `DnDGrab`
    /// implements `PointerGrab::unset` (and `TouchGrab::unset`) as a call to
    /// its own `drop`, and `drop` calls this — so a grab taken away by
    /// something else, a cancelled touch, and an ordinary button release all
    /// arrive here. Clearing anywhere else would leave the icon painted over
    /// the session after the gesture that owned it was over.
    fn dropped(&mut self, _target: Option<WlSurface>, _validated: bool, _seat: Seat<Self>) {
        self.dnd_icon = None;
        // The icon was drawn last frame and will not be this one. Nothing else
        // damages that region, so a drop onto a still window would otherwise
        // leave the icon on screen until the next unrelated frame.
        self.redraw = true;
    }
}

impl ServerDndGrabHandler for Solium {}

/// `xdg_dialog_v1` -- a client saying a toplevel is a modal dialog.
///
/// The protocol is one flag on an object hung off a toplevel, and smithay
/// already keeps it: `set_modal`/`unset_modal` write
/// `XdgToplevelSurfaceRoleAttributes::modal`, and this handler is told only
/// when the value *changes* (`wayland/shell/xdg/dialog.rs` returns early when
/// it does not). So there is no state to mirror here; there is only the fact
/// that the layout's input just changed, and something has to say so.
///
/// ## What a non-modal dialog gets, and why
///
/// The protocol makes modality a flag on a dialog object rather than the
/// meaning of the object, so a client may create an `xdg_dialog_v1` for a
/// toplevel and never call `set_modal`. **Such a window gets the ordinary
/// treatment here: it is laid out like any other, with a share of the screen.**
///
/// It is worth being plain that this is not a free choice: with smithay 0.7 it
/// is the only one that can be implemented. Nothing reaches this compositor
/// when a dialog object is created. `XdgDialogHandler` has exactly one method,
/// the `modal_changed` below, and creating the object changes no flag; the
/// object itself is stored in `XdgShellSurfaceUserData::dialog`, which is
/// `pub(crate)` to smithay and has no accessor (`shell/xdg/handlers/surface.rs`
/// -- read the source, not the docs). So "this toplevel is a dialog but not a
/// modal one" is a state Solium cannot observe at all. Taking the other branch
/// would mean dispatching `xdg_wm_dialog_v1` ourselves and keeping a second
/// copy of state smithay already holds, which is how two answers to one
/// question get out of step.
///
/// That said, it is also the answer this would pick with the field in hand, and
/// that matters more than which one is cheap. What the protocol actually
/// *defines* for a non-modal dialog is nothing: `set_modal` is described as the
/// hint that the window must be addressed before its parent can be used again,
/// and the dialog object without it carries no stated behaviour, only the
/// possibility of future hints. The whole argument for lifting a window out of
/// the arrangement is that it is blocking the window underneath it and will be
/// gone in a moment. A dialog that blocks nothing has neither half of that: a
/// non-modal find bar or a colour picker is a window somebody keeps open beside
/// their document, and floating it in the middle of the screen, over the
/// document, is a worse answer than tiling it. Compare the X11 side, where the
/// same question is decided the same way for the same reason:
/// `xwayland::floats_over_its_parent` floats `Dialog` and not `Utility`.
///
/// If a toolkit is ever found creating dialog objects for prompts and leaving
/// `set_modal` unsent, this is the paragraph to revisit -- and the revision
/// would start with smithay, not here.
impl XdgDialogHandler for Solium {
    /// Re-run the layout, because a window just left the arrangement or
    /// rejoined it.
    ///
    /// `trigger_relayout` and nothing else. The alternative -- placing the
    /// dialog from here -- would put a second opinion about where a window goes
    /// next to the layout scripts' one, and the two would disagree the first
    /// time somebody wrote their own `tiling.lua`. Where a modal dialog goes is
    /// a layout question; that it *is* one is the only thing the compositor
    /// knows and the only thing it says.
    ///
    /// This is also what makes `unset_modal` work at all. Without it, a dialog
    /// that stopped being modal would sit floating until some unrelated event
    /// happened to re-run the layout, which on a quiet desktop is never.
    fn modal_changed(&mut self, toplevel: ToplevelSurface, is_modal: bool) {
        let id = self
            .window_for(toplevel.wl_surface())
            .and_then(|window| self.panes.id_of(&window))
            .map(crate::pane::PaneId::get);
        tracing::debug!(?id, is_modal, "a toplevel changed its modal hint");
        self.trigger_relayout();
    }
}

/// `delegate_compositor!`, written out, for every object but `wl_subsurface`:
/// Solium is asked about one going before smithay's destructor for it runs.
/// See [`Solium::goes_with`].
mod compositor_dispatch {
    use super::Solium;
    use smithay::reexports::wayland_server::{
        Client, DataInit, Dispatch, DisplayHandle,
        backend::ClientId,
        delegate_dispatch, delegate_global_dispatch,
        protocol::{
            wl_callback::WlCallback,
            wl_compositor::WlCompositor,
            wl_region::WlRegion,
            wl_subcompositor::WlSubcompositor,
            wl_subsurface::{self, WlSubsurface},
            wl_surface::WlSurface,
        },
    };
    use smithay::wayland::compositor::{
        CompositorState, RegionUserData, SubsurfaceUserData, SurfaceUserData,
    };

    delegate_global_dispatch!(Solium: [WlCompositor: ()] => CompositorState);
    delegate_global_dispatch!(Solium: [WlSubcompositor: ()] => CompositorState);
    delegate_dispatch!(Solium: [WlCompositor: ()] => CompositorState);
    delegate_dispatch!(Solium: [WlSurface: SurfaceUserData] => CompositorState);
    delegate_dispatch!(Solium: [WlRegion: RegionUserData] => CompositorState);
    delegate_dispatch!(Solium: [WlCallback: ()] => CompositorState);
    delegate_dispatch!(Solium: [WlSubcompositor: ()] => CompositorState);

    impl Dispatch<WlSubsurface, SubsurfaceUserData> for Solium {
        fn request(
            state: &mut Self,
            client: &Client,
            resource: &WlSubsurface,
            request: wl_subsurface::Request,
            data: &SubsurfaceUserData,
            dhandle: &DisplayHandle,
            data_init: &mut DataInit<'_, Self>,
        ) {
            <CompositorState as Dispatch<WlSubsurface, SubsurfaceUserData, Self>>::request(
                state, client, resource, request, data, dhandle, data_init,
            );
        }

        fn destroyed(
            state: &mut Self,
            client: ClientId,
            resource: &WlSubsurface,
            data: &SubsurfaceUserData,
        ) {
            state.goes_with(data.surface());
            <CompositorState as Dispatch<WlSubsurface, SubsurfaceUserData, Self>>::destroyed(
                state, client, resource, data,
            );
        }
    }
}
delegate_shm!(Solium);
delegate_xdg_shell!(Solium);
delegate_xdg_decoration!(Solium);
smithay::delegate_xdg_dialog!(Solium);
delegate_layer_shell!(Solium);
delegate_seat!(Solium);
// Routes `wp_cursor_shape_manager_v1` and the per-pointer device it hands out.
// smithay's dispatch turns a `set_shape` into `SeatHandler::cursor_image` with
// a `CursorImageStatus::Named`, so the handler for this protocol is the seat
// handler above rather than a trait of its own.
smithay::delegate_cursor_shape!(Solium);
delegate_output!(Solium);
delegate_data_device!(Solium);
smithay::delegate_primary_selection!(Solium);
smithay::delegate_xwayland_shell!(Solium);
