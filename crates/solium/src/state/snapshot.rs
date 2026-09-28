//! What scripts and the shell are shown of the session: the `Snapshot` a script call is given, a
//! window's script-facing id, title, application id, parent and modality, and the window list
//! published to the shell.

use super::*;

pub(super) fn to_rect(rectangle: Rectangle<i32, Logical>) -> Rect {
    Rect {
        x: f64::from(rectangle.loc.x),
        y: f64::from(rectangle.loc.y),
        w: f64::from(rectangle.size.w),
        h: f64::from(rectangle.size.h),
    }
}

impl Solium {
    /// A window's application id, as the client set it.
    ///
    /// The shell tells its own surfaces from application windows by this, so
    /// an empty answer is better than a wrong one.
    pub(crate) fn window_app_id(&self, window: &Window) -> String {
        window
            .toplevel()
            .map(ToplevelSurface::wl_surface)
            .and_then(|surface| {
                with_states(surface, |states| {
                    states
                        .data_map
                        .get::<XdgToplevelSurfaceData>()
                        .and_then(|data| data.lock().ok())
                        .and_then(|attributes| attributes.app_id.clone())
                })
            })
            .unwrap_or_default()
    }

    /// A window's title, as the client set it.
    pub(crate) fn window_title(&self, window: &Window) -> String {
        window
            .toplevel()
            .map(ToplevelSurface::wl_surface)
            .and_then(|surface| {
                with_states(surface, |states| {
                    states
                        .data_map
                        .get::<XdgToplevelSurfaceData>()
                        // A poisoned lock means another thread panicked while
                        // holding it. Showing no title beats propagating that.
                        .and_then(|data| data.lock().ok())
                        .and_then(|attributes| attributes.title.clone())
                })
            })
            .unwrap_or_default()
    }

    /// A window's script-facing identity: the id of the pane it is inside.
    ///
    /// It belongs to the pane rather than to the surface, which is what lets it
    /// exist before the surface does — a script told about a window while its
    /// application was still starting is still talking about the same window
    /// once the application arrives, because nothing was replaced.
    ///
    /// Zero means a window the compositor is not tracking. Every window it maps
    /// gets a pane on the same line, so in practice this is a window Smithay
    /// put in the space behind our back, and a script can do nothing with it
    /// anyway.
    pub(crate) fn window_id(&self, window: &Window) -> u64 {
        self.panes.id_of(window).map_or(0, crate::pane::PaneId::get)
    }

    /// What the compositor looks like right now, as a script sees it.
    ///
    /// Built fresh per dispatch and handed over by value: a script holding a
    /// stale view of the windows is the mirror-of-state bug that cost this
    /// project a week in its previous life.
    pub(crate) fn snapshot(&self) -> Snapshot {
        let now = self.clock.now();
        let focused = self.focused_window();
        let cursor = self
            .seat
            .get_pointer()
            .map(|pointer| pointer.current_location())
            .unwrap_or_default();

        // Topmost first, which is the order a hit test wants.
        //
        // Built from panes, not from the space: this is the list scripts place,
        // so a window that exists but has no client yet has to be in it or the
        // layout will never give it anywhere to be.
        let windows = self
            .panes
            .iter()
            .rev()
            .filter_map(|pane| {
                // The remains of a window whose client has gone, fading out.
                // Never listed, in any event, and not under `reserves_a_slot`
                // below: a layout handed one places it and `adopt` puts it
                // into a tree -- an immortal invisible tile, since nothing
                // sends a second `close`. Its `close` went out while it was
                // still the window it had been (`Self::depart`).
                //
                // **Belt and braces, and said so.** Every such pane is also
                // `gone`, which the filter further down already leaves out, so
                // no test fails without this line; it holds for a pane that is
                // `Leaving` by any later route that forgets to mark it.
                if pane.ghost() {
                    return None;
                }
                // A pane whose application has not arrived is in this list --
                // that is what makes the layout reserve its place before there
                // is anything to put in it. Unless it was asked not to: a
                // window that takes no slot until it is really there is a
                // setting, because which of the two reads better is taste.
                if pane.client().is_none() && !self.loading.reserves_a_slot {
                    return None;
                }
                // A menu, a tooltip, a drag icon. On screen and under the
                // pointer, but not a window: a layout given one reserves a
                // slot for it and reflows the desktop around something that
                // will be gone in a moment.
                if !pane.managed() {
                    return None;
                }
                // A window scripts have been told has gone, in any event after
                // that one: its pane is only waiting for `sync_panes` to retire
                // it. Listed, it was a window a layout could place or `adopt`
                // put back into a tree. `close`'s own snapshot still lists it,
                // so a script can ask which window it was. See
                // `adopt_in_the_frame_a_window_went_keeps_no_leaf_for_it`.
                if pane.gone() && self.closing != Some(pane.id()) {
                    return None;
                }
                let outer = self.pane_outer(pane);
                Some(WindowInfo {
                    id: pane.id().get(),
                    rect: to_rect(outer),
                    drawn: Drawn {
                        slot: outer,
                        frame: self.drawn_at(pane, outer, now),
                    },
                    // What the user asked for, until the client has an opinion.
                    title: pane.client().map_or_else(
                        || pane.program().unwrap_or_default().to_owned(),
                        |window| self.window_title(window),
                    ),
                    focused: pane.client().is_some() && focused.as_ref() == pane.client(),
                    monitor: self
                        .output_of(outer)
                        .map(|output| output.name())
                        .unwrap_or_default(),
                    // A pane with no client yet is a reserved slot, and a
                    // reserved slot has no client to have said either of these
                    // things -- so it is an ordinary window until one arrives,
                    // and the `modal_changed` that arrives with it re-runs the
                    // layout.
                    modal: pane.client().is_some_and(|window| self.is_modal(window)),
                    parent: pane
                        .client()
                        .map_or(Parentage::None, |window| self.parent_of(window)),
                    // From `closing` until the window is given back, and in
                    // `close`'s own snapshot, which still lists the window so a
                    // script can ask which one it was. That includes a client
                    // that closed itself and was never `closing`, because
                    // `trigger_close` marks every pane it tells scripts about
                    // as gone, and a gone pane is leaving. See
                    // `WindowInfo::leaving` and
                    // `the_window_list_says_which_windows_are_leaving`.
                    leaving: pane.leaving(),
                })
            })
            .collect();

        let active = self.active_output();
        let primary = self.primary_output();
        let monitors = self
            .space
            .outputs()
            .map(|output| crate::script::MonitorInfo {
                name: output.name(),
                area: self.work_area_on(output).map(to_rect).unwrap_or_default(),
                whole: self
                    .space
                    .output_geometry(output)
                    .map(to_rect)
                    .unwrap_or_default(),
                scale: output.current_scale().fractional_scale(),
                focused: active.as_ref() == Some(output),
                primary: primary.as_ref() == Some(output),
                transform: format!("{:?}", output.current_transform()).to_lowercase(),
            })
            .collect();

        Snapshot {
            windows,
            monitors,
            keyboard: self.keyboard.clone(),
            work_area: self.work_area().map(to_rect).unwrap_or_default(),
            cursor: (cursor.x, cursor.y),
            screens: self.screens(),
        }
    }

    /// Whether a layout should float this window over the one waiting on it.
    ///
    /// Two protocols, one question, and they are not symmetrical: Wayland has a
    /// flag that means exactly this, and X11 does not, so the X11 side reads
    /// the window type instead. The whole of that argument is in
    /// `xwayland::floats_over_its_parent`; what is here is only the lookup.
    pub(super) fn is_modal(&self, window: &Window) -> bool {
        if let Some(toplevel) = window.toplevel() {
            return with_states(toplevel.wl_surface(), |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    // A poisoned lock is a panic somewhere else in this
                    // process, and the honest answer to "is this modal" at that
                    // point is "no" -- an ordinary window, laid out the
                    // ordinary way. `unwrap` here would turn one panic into
                    // two, in a compositor with nothing to restart it.
                    .and_then(|data| data.lock().ok())
                    .is_some_and(|attributes| attributes.modal)
            });
        }
        window
            .x11_surface()
            .is_some_and(|surface| crate::xwayland::floats_over_its_parent(surface.window_type()))
    }

    /// A parent that was found, as a script sees it.
    ///
    /// `Unknown` for a pane that is on its way out, which is not a special case
    /// so much as the honest reading of it: [`Self::trigger_close`] runs while
    /// the dying pane is still in `panes`, because a script has to be able to
    /// ask which window it was. Answer `Window` there and the dialog it was
    /// waiting on is centred on the rect of a window that is going away — and
    /// the close pass is the last one, so nothing runs again to move it off.
    /// "Named, and cannot be pointed at" is exactly what `Unknown` means.
    fn parented(&self, pane: crate::pane::PaneId) -> Parentage {
        if self.closing == Some(pane) {
            return Parentage::Unknown;
        }
        Parentage::Window(pane.get())
    }

    /// Which window this one belongs to, as far as this compositor can tell.
    ///
    /// The distinction [`Parentage`] exists for is made here and only here: a
    /// parent that was named and cannot be found is `Unknown`, and a parent
    /// that was never named is `None`. Both end up as "no rect to centre on" in
    /// a layout, but only one of them means something has gone missing.
    pub(super) fn parent_of(&self, window: &Window) -> Parentage {
        if let Some(toplevel) = window.toplevel() {
            let Some(parent) = toplevel.parent() else {
                return Parentage::None;
            };
            return self
                .window_for(&parent)
                .and_then(|window| self.panes.id_of(&window))
                .map_or(Parentage::Unknown, |pane| self.parented(pane));
        }

        let Some(surface) = window.x11_surface() else {
            return Parentage::None;
        };
        // `WM_TRANSIENT_FOR`, which smithay reads at `CreateNotify` and again
        // on every property change. It holds an X11 window id rather than a
        // surface, so the match is against the id side -- and a client that
        // points it at the root window, which is a common way of saying "I am
        // transient for the session", names an id no element here has and comes
        // out `Unknown`. That is the right answer: there is no window to centre
        // on.
        let Some(parent) = surface.is_transient_for() else {
            return Parentage::None;
        };
        self.space
            .elements()
            .find(|element| {
                element
                    .x11_surface()
                    .is_some_and(|surface| surface.window_id() == parent)
            })
            .and_then(|element| self.panes.id_of(element))
            .map_or(Parentage::Unknown, |pane| self.parented(pane))
    }

    /// Tell the shell what windows exist.
    ///
    /// Sent when the list changes rather than every frame: the shell rebinds
    /// on it, and a bar that re-evaluates sixty times a second because nothing
    /// happened is a bar that costs something to look at.
    pub(crate) fn publish_windows(&mut self) {
        // Nobody to tell, nothing to say. The window list is serialised for the
        // shell, and building it walks every window, asks each for its title
        // and app id, and allocates a string per window -- every frame, once
        // something is animating. With no shell hosted that is pure waste, and
        // the ordinary case is no shell hosted.
        // Only when a foreign shell is hosted. The list is for the Quickshell
        // compatibility layer -- `ToplevelManager.toplevels` and friends -- and
        // building it walks every window, asks each for its title and app id,
        // and allocates a string per window, every time anything changes.
        //
        // This used to test whether the in-process shell existed, which stopped
        // meaning anything the moment the shell became an ordinary scripted
        // surface: a wallpaper is one of those, and there is always a
        // wallpaper.
        if std::env::var_os("SOLIUM_SHELL_SCENE").is_none() {
            return;
        }
        let focused = self.focused_window();
        let mut windows = String::from("{\"windows\":[");
        let mut active = String::from("null");
        for (index, pane) in self.panes.iter().rev().enumerate() {
            let Some(window) = pane.client() else {
                continue;
            };
            let id = pane.id().get();
            let title = self.window_title(window).replace('"', "'");
            let app_id = self.window_app_id(window).replace('"', "'");
            let is_active = focused.as_ref() == Some(window);
            let entry = format!(
                "{{\"id\":{id},\"title\":\"{title}\",\"appId\":\"{app_id}\",\"activated\":{is_active}}}"
            );
            if index > 0 {
                windows.push(',');
            }
            windows.push_str(&entry);
            if is_active {
                active = entry;
            }
        }
        windows.push_str("],\"active\":");
        windows.push_str(&active);
        windows.push('}');

        if windows != self.published_windows {
            crate::qml::set_windows(&windows);
            self.published_windows = windows;
        }
    }
}
