//! What scripts are shown of the session: the `Snapshot` a script call is given, and a window's
//! script-facing id, title, application id, parent and modality.

use super::*;

pub(super) fn to_rect(rectangle: Rectangle<i32, Logical>) -> Rect {
    Rect {
        x: f64::from(rectangle.loc.x),
        y: f64::from(rectangle.loc.y),
        w: f64::from(rectangle.size.w),
        h: f64::from(rectangle.size.h),
    }
}

/// What a client says about its own size (#115): the least and the most it
/// will be, in the logical pixels of its window geometry -- the client's own
/// rectangle, with no frame round it. Zero on a side is no limit on that side,
/// as both protocols have it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Limits {
    pub(crate) min: Size<i32, Logical>,
    pub(crate) max: Size<i32, Logical>,
}

/// The most a client's limit is read as, on either side, in logical pixels.
///
/// Neither protocol bounds what a client may say, and what it says goes
/// straight into geometry: a scrolling column as wide as a minimum of
/// `i32::MAX` put the next column past the end of `i32`, and the frame's
/// insets added to that overflowed. No screen is anywhere near this, and the
/// largest sum a layout makes of a few of them is nowhere near the end of
/// `i32`. `limits::a_limit_past_any_screen_is_read_as_the_most_there_is` and
/// `real_client::client_sizes::an_xdg_limit_past_any_screen_is_read_as_the_most_there_is`.
pub(crate) const MOST: i32 = 32_767;

impl Limits {
    /// What a client said, each side over [`MOST`] read as `MOST`.
    fn said(min: Size<i32, Logical>, max: Size<i32, Logical>) -> Self {
        let side = |size: i32| size.min(MOST);
        Self {
            min: Size::from((side(min.w), side(min.h))),
            max: Size::from((side(max.w), side(max.h))),
        }
    }

    /// An X11 window's, from `WM_NORMAL_HINTS` as smithay 0.7 reads it:
    /// `X11Surface::min_size` and `max_size` (`xwayland/xwm/surface.rs:402`
    /// and `:418`), each `None` where the client left that flag unset, and
    /// converted to logical pixels by the client's scale.
    /// `limits::x11_hints_that_say_nothing_are_no_limit` in `state/tests.rs`.
    pub(crate) fn from_hints(
        min: Option<Size<i32, Logical>>,
        max: Option<Size<i32, Logical>>,
    ) -> Self {
        Self::said(min.unwrap_or_default(), max.unwrap_or_default())
    }
}

/// A client's own size limits, read where each protocol keeps them, and held
/// to [`MOST`].
///
/// **xdg: the surface's committed [`SurfaceCachedState`]**, which is the
/// struct #115 cites (`wayland/shell/xdg/mod.rs:1070` and `:1077` are its
/// `min_size` and `max_size`), and its `current` half is what is read:
/// `xdg_toplevel.set_min_size` and `set_max_size` write the *pending* half of
/// that double-buffered state (smithay 0.7,
/// `wayland/shell/xdg/handlers/surface/toplevel.rs:119-128`), and a commit
/// makes it `current`. So this reads what the client has committed, which is
/// what its buffers are drawn to -- never a size it has asked for and not yet
/// committed to.
/// `real_client::client_sizes::an_xdg_window_says_how_small_and_how_large_it_can_be`.
///
/// **X11: `WM_NORMAL_HINTS`**, through [`Limits::from_hints`].
pub(crate) fn limits_of(window: &Window) -> Limits {
    if let Some(toplevel) = window.toplevel() {
        return with_states(toplevel.wl_surface(), |states| {
            let mut cached = states.cached_state.get::<SurfaceCachedState>();
            let current = cached.current();
            Limits::said(current.min_size, current.max_size)
        });
    }
    window
        .x11_surface()
        .map_or_else(Limits::default, |surface| {
            Limits::from_hints(surface.min_size(), surface.max_size())
        })
}

/// A limit as a layout reads it: in the same terms as the window's own `w`
/// and `h`, the frame's insets added to each side the client limited, and
/// `None` when it limited neither.
pub(crate) fn in_pane(size: Size<i32, Logical>, insets: Insets) -> Option<Size<i32, Logical>> {
    if size.w <= 0 && size.h <= 0 {
        return None;
    }
    let side = |own: i32, frame: i32| {
        if own > 0 {
            own.saturating_add(frame)
        } else {
            0
        }
    };
    Some(Size::from((
        side(size.w, insets.horizontal()),
        side(size.h, insets.vertical()),
    )))
}

impl Solium {
    /// A window's application id, as the client set it.
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

    /// What a script calls a window's application: its xdg `app_id`, or an
    /// X11 window's `WM_CLASS` class, which is what X11 has instead.
    ///
    /// Not [`Self::window_app_id`], which answers nothing for an X11 window.
    /// A script matching an application by name -- `tiling.client_size_ignore`
    /// -- has to be able to name an XWayland one too.
    pub(crate) fn script_app_id(&self, window: &Window) -> String {
        if window.toplevel().is_some() {
            return self.window_app_id(window);
        }
        window
            .x11_surface()
            .map(smithay::xwayland::X11Surface::class)
            .unwrap_or_default()
    }

    /// A client's own size limits in its pane's outer terms, the frame's
    /// insets added: what a floating drag of it is held to (#115). See
    /// `input::resize::drag_rect`, and
    /// `real_client::client_sizes::a_floating_drag_is_held_to_what_the_client_accepts`.
    ///
    /// None at all, both ways, where the user has said not to believe it:
    /// `floating.client_limits = "ignore"`, or its application named in
    /// `tiling.client_size_ignore`, which `sizes.lua` hands over with
    /// `sol.client_sizes`. See [`crate::script::ClientSizes`], and
    /// `real_client::client_sizes::a_floating_drag_of_an_application_not_believed_is_not_held`.
    pub(crate) fn outer_limits(&self, window: &Window) -> (Size<i32, Logical>, Size<i32, Logical>) {
        if !self.client_sizes.believes(&self.script_app_id(window)) {
            return (Size::default(), Size::default());
        }
        let limits = limits_of(window);
        let insets = self
            .panes
            .id_of(window)
            .map_or(Insets::NONE, |pane| self.insets_of(pane));
        (
            in_pane(limits.min, insets).unwrap_or_default(),
            in_pane(limits.max, insets).unwrap_or_default(),
        )
    }

    /// Tell the layouts that a client's own size limits changed (#115), if
    /// they did.
    ///
    /// Through `trigger_relayout`, which is how a change to what a layout
    /// decides with is told everywhere else -- `modal_changed`,
    /// `parent_changed` -- so the layout re-runs from a snapshot that already
    /// has the new limits in it and nothing else is new to learn.
    ///
    /// **Once for each change, not once for each commit.** A client commits
    /// every frame it draws, and the limits are in every commit's state
    /// whether they changed or not: re-laying out the desktop on each of those
    /// is sixty relayouts a second for a video playing. So the pane keeps what
    /// the layouts were last told and this compares with it.
    /// `real_client::client_sizes::a_change_of_minimum_is_told_once`.
    ///
    /// **Recorded and not told for a window whose `open` is still to come**:
    /// one its application opened, before that application's first frame. The
    /// `open` carries the limits in the window's row, and a `layout` ahead of
    /// it would be a pass for a window no layout has heard of. A window
    /// launched with `sol.spawn` had its `open` at the launch, so its first
    /// limits are told like any change -- and that is the `layout` in which
    /// `tiling.lua` places it again if the tile it was given cannot hold it.
    /// `real_client::client_sizes::a_window_that_opens_with_a_minimum_hears_it_once`
    /// and `a_launched_window_whose_minimum_does_not_fit_goes_where_overflow_says`.
    ///
    /// Called on every commit of a window's root surface, and on X11's
    /// `WM_NORMAL_HINTS` changing, since an X11 client says it with a property
    /// rather than a commit.
    pub(crate) fn notice_limits(&mut self, window: &Window) {
        let Some(pane) = self.panes.id_of(window) else {
            return;
        };
        let limits = limits_of(window);
        let Some(held) = self.panes.get_mut(pane) else {
            return;
        };
        if held.limits() == limits {
            return;
        }
        held.set_limits(limits);
        // Launched and adopted, or shown -- which for a window its
        // application opened is the moment `show_if_new` sends its `open`.
        // See `Pane::adopted`.
        if !held.adopted() && !crate::present::was_shown(held) {
            return;
        }
        tracing::debug!(
            pane = pane.get(),
            min = ?limits.min,
            max = ?limits.max,
            "a client changed how small or large it can be"
        );
        self.trigger_relayout();
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
                    app_id: pane
                        .client()
                        .map(|window| self.script_app_id(window))
                        .unwrap_or_default(),
                    // Read live rather than from what the layouts were last
                    // told, so the `open` of a window whose client said so
                    // before its first buffer -- every X11 window, and an xdg
                    // one launched outside `sol.spawn` -- is decided with its
                    // limits in hand. A pane with no client has said nothing.
                    // `real_client::client_sizes::a_window_that_opens_with_a_minimum_hears_it_once`.
                    min: pane.client().and_then(|window| {
                        in_pane(limits_of(window).min, self.insets_of(pane.id()))
                    }),
                    max: pane.client().and_then(|window| {
                        in_pane(limits_of(window).max, self.insets_of(pane.id()))
                    }),
                    cramped: pane.cramped(),
                    shown: crate::present::was_shown(pane),
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
                off: self.power.is_off(output),
            })
            .collect();

        Snapshot {
            windows,
            monitors,
            keyboard: self.keyboard.clone(),
            work_area: self.work_area().map(to_rect).unwrap_or_default(),
            cursor: (cursor.x, cursor.y),
            screens: self.screens(),
            text_input: self.text_field(),
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
}
