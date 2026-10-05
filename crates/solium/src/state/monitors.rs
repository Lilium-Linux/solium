//! Monitors: which one the user is working on and which one a point, a rectangle or a surface is
//! on, the work area each one offers, placing and scaling them and the layer surfaces anchored to
//! them, and what a change in the set of monitors does, down to bringing back a window left on
//! none.

use super::*;

/// Whether `rect` lands on any of `screens`.
///
/// A free function over plain rectangles rather than a method, for one reason:
/// `Solium` cannot be built in a unit test — it needs a `Display` — and
/// neither can a `Space` with monitors mapped into it. This is the half that
/// decides, so this is the half that is testable, and [`Solium::on_any_output`]
/// is the two-line adapter that feeds it `space.outputs()`. Same trick, same
/// reason, as `pool::Pool` being generic over what it keeps.
///
/// Through [`crate::render::drawn_on`], the very call `render::elements`
/// makes when it culls a pane against one screen -- so exclusive: a window
/// whose right edge is exactly the monitor's left edge has no pixel on it.
pub(super) fn anywhere_on(
    rect: Rectangle<i32, Logical>,
    screens: impl IntoIterator<Item = Rectangle<i32, Logical>>,
) -> bool {
    screens
        .into_iter()
        .any(|screen| crate::render::drawn_on(rect, screen))
}

/// What is left of `whole` once each edge has lost its layer-shell zone (the
/// difference between `whole` and `zone`) plus what hosted surfaces reserve
/// there, keeping at least one logical pixel on each axis (Ruling 10).
/// `tests::a_reserve_adds_to_the_layer_zone_on_its_edge`,
/// `tests::a_reserve_never_takes_the_whole_monitor`.
pub(crate) fn within(
    whole: Rectangle<i32, Logical>,
    zone: Rectangle<i32, Logical>,
    reserved: crate::scripted::Edges,
) -> Rectangle<i32, Logical> {
    // `min` then `max` rather than `clamp`, which panics on a monitor with
    // no size yet: `tests::a_monitor_with_no_size_yet_is_reserved_on_without_a_panic`.
    // Saturating, because a reserve is whatever a scene or a script says:
    // `tests::a_reserve_too_large_for_any_monitor_leaves_one_pixel`.
    let top = (zone.loc.y - whole.loc.y)
        .saturating_add(reserved.top)
        .min(whole.size.h - 1)
        .max(0);
    let left = (zone.loc.x - whole.loc.x)
        .saturating_add(reserved.left)
        .min(whole.size.w - 1)
        .max(0);
    let bottom =
        ((whole.loc.y + whole.size.h) - (zone.loc.y + zone.size.h)).saturating_add(reserved.bottom);
    let right =
        ((whole.loc.x + whole.size.w) - (zone.loc.x + zone.size.w)).saturating_add(reserved.right);
    let (width, height) = (whole.size.w, whole.size.h);
    Rectangle::new(
        (whole.loc.x + left, whole.loc.y + top).into(),
        (
            width.saturating_sub(left).saturating_sub(right).max(1),
            height.saturating_sub(top).saturating_sub(bottom).max(1),
        )
            .into(),
    )
}

impl Solium {
    /// The monitor the user is working on.
    ///
    /// **The one the pointer is on.** One rule, and it needs no state: a new
    /// window opens where you are looking, `sol.monitor()` means the screen in
    /// front of you, and there is nothing to get out of step.
    ///
    /// The alternative — the focused window's monitor — sounds more careful and
    /// is worse in the case that actually happens: move the pointer to the
    /// second screen, click the empty desktop, open a terminal. Nothing was
    /// focused, so nothing changed, and the terminal appears on the screen you
    /// just looked away from. It is also inconsistent with the layout policy
    /// this project already has, where a new window splits *the window under
    /// the pointer*.
    ///
    /// A window is still maximised and fitted against the monitor **it** is on,
    /// not this one. Where a window goes and where a window is are different
    /// questions.
    pub(crate) fn active_output(&self) -> Option<Output> {
        let at = self
            .seat
            .get_pointer()
            .map(|pointer| pointer.current_location());
        match at {
            Some(at) => monitor::at(&self.space, at).or_else(|| monitor::nearest(&self.space, at)),
            // No pointer yet, which is the moment before the first input
            // device is reported. The primary monitor is the stable answer.
            None => self.primary_output(),
        }
    }

    /// The output whose place in the global space is exactly this rectangle.
    ///
    /// How the render loop gets from "the screen I am drawing" back to the
    /// output that owns the layer surfaces on it. Matched on geometry rather
    /// than carried through, so there is one fewer thing to keep in step.
    pub(crate) fn output_for(&self, screen: Rectangle<i32, Logical>) -> Option<Output> {
        self.space
            .outputs()
            .find(|output| self.space.output_geometry(output) == Some(screen))
            .cloned()
    }

    /// The monitor things belonging to *one* screen go on.
    ///
    /// A dock, a bar, a layer surface that named no output. Not the active
    /// monitor: a dock connects once, at startup, and pinning it to whichever
    /// screen the pointer happened to be over at that moment means it appears
    /// on a different monitor depending on where the mouse was left — which
    /// looks like the compositor placing it at random, because it is.
    ///
    /// `primary = true` in the configuration decides it. Otherwise the first
    /// monitor, which is at least stable across sessions.
    pub(crate) fn primary_output(&self) -> Option<Output> {
        let named = self.arrangement.primary();
        named
            .and_then(|name| {
                self.space
                    .outputs()
                    .find(|output| output.name() == name)
                    .cloned()
            })
            .or_else(|| self.space.outputs().next().cloned())
    }

    /// The monitor covering a point, or the nearest one to it.
    ///
    /// Never `None` while any output is mapped, on purpose. The callers are
    /// asking in order to place or size something, and "no monitor" is not an
    /// answer they can do anything with — an L-shaped arrangement has a hole in
    /// it, and a window whose centre lands in the hole still has to go
    /// somewhere.
    pub(crate) fn output_at(&self, point: Point<i32, Logical>) -> Option<Output> {
        let point = point.to_f64();
        monitor::at(&self.space, point).or_else(|| monitor::nearest(&self.space, point))
    }

    /// The monitor a rectangle is on, judged by its centre.
    ///
    /// Derived rather than stored, and that is the point: a window dragged to
    /// the next screen belongs to it the moment it is more than half way
    /// there, with no bookkeeping to keep in step and nothing to go stale.
    pub(crate) fn output_of(&self, rect: Rectangle<i32, Logical>) -> Option<Output> {
        self.output_at((rect.loc.x + rect.size.w / 2, rect.loc.y + rect.size.h / 2).into())
    }

    /// Whether a rectangle is on any monitor at all.
    ///
    /// **Not [`Self::output_of`], which never answers `None`**: that one falls
    /// back to the *nearest* monitor, because a window being dragged has to
    /// belong to something. This is the other question -- is any of this
    /// rectangle on a screen -- and a workspace that is hidden by being parked
    /// a screen away is precisely the case where the two answers differ.
    ///
    /// Asked by `render::prepare`, which runs before any output is bound and
    /// so has no one screen to test against; `render::elements` asks the same
    /// thing one monitor at a time and needs no such helper.
    pub(crate) fn on_any_output(&self, rect: Rectangle<i32, Logical>) -> bool {
        anywhere_on(
            rect,
            self.space
                .outputs()
                .filter_map(|output| self.space.output_geometry(output)),
        )
    }

    /// Every monitor's rectangle, for a caller asking about more than one pane.
    ///
    /// Collected once rather than per pane: [`Self::on_stage`] is asked in a
    /// walk, and re-deriving the screens inside it would make a question about
    /// one pane cost a pass over the outputs.
    pub(super) fn screens(&self) -> Vec<Rectangle<i32, Logical>> {
        self.space
            .outputs()
            .filter_map(|output| self.space.output_geometry(output))
            .collect()
    }

    /// The monitor a surface is on, for telling it what to draw itself like.
    ///
    /// A window's own monitor when it has one, and the active one otherwise —
    /// which covers a surface that has committed but is not placed yet, and is
    /// the monitor it is about to be on.
    pub(super) fn output_for_surface(&self, surface: &WlSurface) -> Option<Output> {
        self.window_for(surface)
            .and_then(|window| {
                self.real_geometry(&window)
                    .filter(|real| real.size.w > 0 && real.size.h > 0)
                    .and_then(|real| self.output_of(real))
            })
            .or_else(|| self.active_output())
    }

    /// A monitor's size in its own logical coordinates.
    ///
    /// The mode divided by the scale, which is what every protocol that
    /// positions something against an output speaks in.
    pub(crate) fn output_logical_size(
        &self,
        output: &Output,
    ) -> smithay::utils::Size<i32, Logical> {
        self.space
            .output_geometry(output)
            .map(|geometry| geometry.size)
            .unwrap_or_default()
    }

    /// How many device pixels to a logical one, on a rectangle's own monitor.
    ///
    /// What everything the compositor draws itself has to rasterise at. One
    /// frame can span monitors at different scales, so this is asked per
    /// window rather than once for the frame.
    pub(crate) fn scale_of(&self, rect: Rectangle<i32, Logical>) -> f64 {
        self.output_of(rect)
            .map_or(1.0, |output| output.current_scale().fractional_scale())
    }

    /// The area windows may use on the monitor the user is working on.
    ///
    /// Whatever is left once every anchored surface has taken its exclusive
    /// zone — a number the *shell* chooses and may change at runtime, not a
    /// constant here. Every placement decision reads this rather than the raw
    /// output.
    pub(crate) fn work_area(&self) -> Option<Rectangle<i32, Logical>> {
        self.work_area_on(&self.active_output()?)
    }

    /// The same, for a monitor you already have: what layer-shell clients'
    /// exclusive zones and hosted surfaces' reserves leave of it (Ruling 10).
    /// `tests::real_client::reflow_on_close::hosted::a_declared_reserve_takes_its_edge_out_of_the_work_area`.
    pub(crate) fn work_area_on(&self, output: &Output) -> Option<Rectangle<i32, Logical>> {
        // The layer map's zone is in the output's own coordinates; every rect
        // the compositor works in is global. Without this offset a bar on the
        // second monitor reserves its strip from the *first* one, which looks
        // like the exclusive zone being applied to the wrong screen because it
        // is.
        let geometry = self.space.output_geometry(output)?;
        let mut area = within(
            Rectangle::from_size(geometry.size),
            layer::work_area(output),
            self.reserved_on(output),
        );
        area.loc += geometry.loc;
        Some(area)
    }

    /// The area a rectangle's own monitor offers it.
    pub(crate) fn work_area_of(
        &self,
        rect: Rectangle<i32, Logical>,
    ) -> Option<Rectangle<i32, Logical>> {
        self.work_area_on(&self.output_of(rect)?)
    }

    /// Put every mapped output where the arrangement says.
    ///
    /// Called when an output appears or goes away and when the configuration is
    /// read again, and it is the only place an output's position is decided.
    /// Re-running it with nothing changed is harmless and cheap, which is what
    /// makes it safe to call from a reload.
    pub(crate) fn place_outputs(&mut self) {
        // Scale first, because it decides each monitor's *logical* size and
        // the positions are laid out in logical space.
        //
        // Here and not where the outputs are created, which is where it was:
        // the nested backend loads its scripts after making its outputs, so
        // the arrangement was empty and every scale read as automatic. Doing
        // it in the one place both backends already call also means
        // `super+shift+r` can change a scale without ending the session.
        self.scale_outputs();

        let monitors: Vec<_> = self
            .space
            .outputs()
            .map(|output| {
                let size = output
                    .current_mode()
                    .map(|mode| mode.size.to_logical(1))
                    .unwrap_or_default();
                (output.name(), size)
            })
            .collect();
        if monitors.is_empty() {
            return;
        }

        for name in self.arrangement.unmatched(&monitors) {
            // Warned rather than ignored: a connector name that does not exist
            // on this machine is the usual reason a monitor configuration
            // appears to do nothing at all, and it is invisible otherwise.
            tracing::warn!(
                monitor = name,
                "no connector by that name -- run `solium --probe` for the ones this machine has"
            );
        }

        let layout = self.arrangement.place(&monitors);
        for name in &layout.unresolved {
            // Named a neighbour that is not here, or two monitors named each
            // other. Placed to the right of everything rather than dropped,
            // and said out loud: the position it ends up at is the one thing
            // that will not look like the configuration was read.
            tracing::warn!(
                monitor = name,
                "could not be placed beside what it names -- put it at the right-hand end"
            );
        }
        let outputs: Vec<Output> = self.space.outputs().cloned().collect();
        for (output, at) in outputs.iter().zip(layout.at) {
            // Only when it actually moved. Remapping an output resets its
            // damage memory, so re-placing everything on every reload would
            // throw away damage tracking to achieve nothing -- and would log a
            // line per monitor per reload, which is how a log stops being read.
            if self
                .space
                .output_geometry(output)
                .map(|geometry| geometry.loc)
                == Some(at)
            {
                continue;
            }
            self.space.map_output(output, at);
            tracing::info!(monitor = output.name(), x = at.x, y = at.y, "placed");
        }
        self.arrange_layers();
    }

    /// Give every monitor the scale it asked for, or the one its size implies.
    pub(super) fn scale_outputs(&mut self) {
        for output in self.space.outputs().cloned().collect::<Vec<_>>() {
            let name = output.name();
            let physical = output.physical_properties().size;
            let Some(mode) = output.current_mode() else {
                continue;
            };
            let scale = match self.arrangement.scale(&name) {
                monitor::Scaling::Fixed(scale) => scale,
                // A window has no physical size, so a nested output reports
                // 0x0 and lands on 1x: a scale there has to be asked for.
                monitor::Scaling::Auto => monitor::automatic(physical, mode.size),
            };
            let current = output.current_scale().fractional_scale();
            if (current - scale).abs() < f64::EPSILON {
                continue;
            }
            if physical.w > 0 {
                #[expect(clippy::cast_possible_truncation, reason = "reported, not measured")]
                let dpi = (f64::from(mode.size.w) / (f64::from(physical.w) / 25.4)).round() as i32;
                tracing::info!(monitor = name, dpi, scale, "scale");
            } else {
                tracing::info!(monitor = name, scale, "scale");
            }
            output.change_current_state(None, None, Some(Scale::Fractional(scale)), None);

            // `wl_surface.preferred_buffer_scale` reaches an existing client
            // for free on its next commit (`commit`, below, sends it on every
            // one). `wp_fractional_scale_v1` does not: `new_fractional_scale`
            // answers it once, when a client first asks, and nothing calls it
            // again on its own. Without this, a window opened before a
            // `super+shift+r` rescale keeps drawing at the scale it had at
            // startup, upscaled by the compositor -- issue #99. Only reached
            // when the scale actually changed, by the `continue` above.
            self.resend_fractional_scale(&output);
        }
    }

    /// Re-tell every surface on `output` the fractional scale it should draw
    /// at, once `scale_outputs` has actually changed that output's scale.
    ///
    /// Per window, not per output: `fractional_scale_for` reads each
    /// surface's *own* output rather than being handed this one, so a window
    /// on some other monitor is never touched even though this function only
    /// runs for the monitor that changed, and one straddling two monitors at
    /// different scales is told the one it actually reads its scale from.
    ///
    /// Walks every window's full surface tree -- subsurfaces and popups, not
    /// just the toplevel -- the same way `send_frame` and
    /// `take_presentation_feedback` already do elsewhere in this file.
    ///
    /// Writes through the `SurfaceData` `with_surfaces` already hands its
    /// callback, rather than looking it up again with `with_states`: that
    /// lookup takes the same per-surface lock `with_surfaces` is already
    /// holding while it calls this closure, and a second, nested attempt on
    /// it from the same thread is a self-deadlock, not a wait -- found by
    /// this function's own test hanging instead of failing.
    fn resend_fractional_scale(&self, output: &Output) {
        for window in self.space.elements_for_output(output) {
            window.with_surfaces(|surface, states| {
                let scale = self.fractional_scale_for(surface);
                with_fractional_scale(states, |fractional| {
                    fractional.set_preferred_scale(scale);
                });
            });
        }
    }

    /// The fractional scale a surface should draw itself at: its own
    /// window's own output, or the active one for a surface not placed yet.
    ///
    /// Shared by `new_fractional_scale`, which answers a client's first ask,
    /// and `resend_fractional_scale`, which repeats the answer when an
    /// output's scale changes after that -- one copy of "what scale is this
    /// surface drawn at" rather than two that can drift apart. Deliberately
    /// just the computation: how the answer gets written back to the surface
    /// differs between the two callers, and that part is not shared -- see
    /// `resend_fractional_scale`'s own doc comment for why.
    pub(super) fn fractional_scale_for(&self, surface: &WlSurface) -> f64 {
        self.window_for(surface)
            .and_then(|window| self.space.outputs_for_element(&window).first().cloned())
            .or_else(|| self.active_output())
            .map_or(1.0, |output| output.current_scale().fractional_scale())
    }

    /// Arrange anchored surfaces on every monitor.
    ///
    /// Returns whether anything moved, because a changed exclusive zone changes
    /// a work area and the windows placed against the old one are now wrong.
    pub(crate) fn arrange_layers(&mut self) -> bool {
        let outputs: Vec<Output> = self.space.outputs().cloned().collect();
        // A plain loop rather than `any`, deliberately: `any` short-circuits,
        // and the first output reporting a change would leave every one after
        // it unarranged — a bar on the second monitor placed against nothing.
        let mut moved = false;
        for output in &outputs {
            if layer::arrange(output) {
                moved = true;
            }
        }
        moved
    }

    /// Where a window drawn at `outer` goes if it is on no screen at all:
    /// onto the nearest one, keeping its size where that fits.
    ///
    /// `None` when any of `outer` is on a screen already -- one hanging half
    /// off an edge is a normal thing to have arranged on purpose -- and when
    /// there is no screen to put it on. With no screens every window is off
    /// every screen, and leaving it where it was means it is still there when
    /// a monitor comes back, which is the best available answer.
    pub(super) fn rescued(
        &self,
        outer: Rectangle<i32, Logical>,
    ) -> Option<Rectangle<i32, Logical>> {
        if self.on_any_output(outer) {
            return None;
        }
        let centre = (
            f64::from(outer.loc.x) + f64::from(outer.size.w) / 2.0,
            f64::from(outer.loc.y) + f64::from(outer.size.h) / 2.0,
        );
        let screen = monitor::nearest(&self.space, centre.into())
            .and_then(|output| self.space.output_geometry(&output))?;
        // Onto the nearest screen, keeping its size, clamped so the whole
        // window is on it when it fits. Not centred: a window that was in the
        // top-left of the monitor that went should still feel like the window
        // that was in the top-left.
        let size = (
            outer.size.w.min(screen.size.w),
            outer.size.h.min(screen.size.h),
        );
        let x = outer
            .loc
            .x
            .clamp(screen.loc.x, screen.loc.x + screen.size.w - size.0);
        let y = outer
            .loc
            .y
            .clamp(screen.loc.y, screen.loc.y + screen.size.h - size.1);
        Some(Rectangle::new((x, y).into(), (size.0, size.1).into()))
    }

    /// Tell scripts a window has gone, so a layout can forget it.
    /// Tell the layout the space windows get has changed.
    /// Tell the scripts the set of monitors is not what it was.
    ///
    /// Fired before the relayout rather than instead of it: re-homing the
    /// windows and arranging them are two steps, and a mode that does the
    /// first is still expecting the second.
    /// Everything a change in the set of monitors has to do, in one call.
    ///
    /// Both backends call this and nothing else, so the nested one and the
    /// hardware one cannot drift -- which matters more here than usual,
    /// because the hardware path needs a cable to exercise and the nested one
    /// does not.
    /// The screens are known: tell the scripts, once, at startup.
    ///
    /// Scripts load before the monitors exist -- on the hardware backend they
    /// load before the GPU is even opened -- so a script that computes where
    /// to put something computes it against nothing. The Developer Tweaks
    /// panel did exactly that and came out 200x0 pixels, which is a panel that
    /// is there and invisible.
    ///
    /// The same call a hotplug makes, deliberately: "the monitors are not what
    /// you last knew" covers both, and having one event rather than a
    /// `startup` and a `changed` means a script cannot handle one and forget
    /// the other.
    pub(crate) fn monitors_ready(&mut self) {
        self.settle_monitors();
    }

    /// The monitors changed size without one coming or going, as the nested
    /// window's do when it is resized: place them again, and give every
    /// surface an instance on each monitor it now covers.
    /// `tests::real_client::a_resize_that_brings_a_monitor_under_a_surface_gives_it_an_instance_there`.
    pub(crate) fn outputs_resized(&mut self) {
        self.place_outputs();
        self.sync_instances();
    }

    pub(crate) fn settle_monitors(&mut self) {
        self.place_outputs();
        // A monitor that has gone is not off, it is gone, and its power
        // controls are told so. See `power.rs`, and
        // `the_power_protocol_is_advertised_answers_mode_on_bind_and_turns_a_monitor_off_and_on`.
        self.settle_power();
        self.rescue_offscreen();
        // Held as one dispatch, as a reload holds its handlers, so a
        // `sol.monitors{}` in a `monitors` handler places no surface before
        // the `layout` handlers have run too: they are placed once, below.
        // `a_hotplug_whose_handler_rearranges_the_monitors_places_the_surfaces_once`.
        self.dispatching += 1;
        self.trigger_monitors_changed();
        self.trigger_relayout();
        self.dispatching -= 1;
        // After the handlers, which may declare a surface where its monitor
        // now is: `a_monitor_an_unplug_moves_keeps_the_scene_its_handler_declares_there`.
        // A monitor that arrived gets its scenes here rather than at its first
        // frame: `scripted::tests::an_instance_goes_with_its_monitor_and_comes_with_a_new_one`.
        self.sync_instances();
        // And what the held handlers' `sol.act`s came to, which no dispatch
        // inside the hold could tell
        // (`a_sol_act_in_a_hotplugs_handler_hears_done_in_the_hotplugs_dispatch`).
        self.tell_settled_attempts();
        self.redraw = true;
        // A lock waiting for its monitors may have been waiting for the one
        // that went. See `Solium::confirm_lock`, and
        // `a_monitor_unplugged_while_locking_does_not_hold_locked_back`.
        self.confirm_lock();
    }

    /// Bring back any window that is no longer on any screen.
    ///
    /// This is the compositor's job and not a layout's, which took two
    /// hardware reports and a nested reproduction to establish. The obvious
    /// place for it is the layout -- a monitor went, so re-run the layout and
    /// it will put everything somewhere -- and that is wrong twice over. A
    /// tiling layout only walks the monitors that *exist*, so a window in a
    /// departed monitor's tree is in a tree nothing iterates. And the default
    /// mode is floating, where no layout runs at all: `tiling.active` and
    /// `scrolling.active` both start false, so on a stock configuration there
    /// is nobody to ask.
    ///
    /// A window nobody can reach is not a layout preference, it is a window
    /// the user has lost. So it is an invariant the compositor keeps, and a
    /// script is free to move it again afterwards -- `trigger_relayout` runs
    /// straight after this.
    ///
    /// Only windows that are *entirely* off every screen are touched. One
    /// hanging half off an edge is a normal thing to have arranged on purpose.
    pub(super) fn rescue_offscreen(&mut self) {
        // Which windows are stranded and where each goes is `rescued`'s,
        // shared with the maximise and fullscreen way back so the two cannot
        // disagree about where a window on no screen belongs.
        let stranded: Vec<_> = self
            .panes
            .iter()
            .filter_map(|pane| {
                let outer = self.pane_outer(pane);
                Some((pane.id(), outer, self.rescued(outer)?))
            })
            .collect();

        for (pane, outer, moved) in stranded {
            // Through the same move every layout uses. Setting the slot alone
            // looks like it works and does not: the space still holds the old
            // position and writes it back the next frame.
            //
            // Animated from where it was, which is off screen -- so it flies
            // in from the edge the monitor was on rather than appearing. That
            // is worth the two lines: a window that teleports is one the user
            // has to find again.
            //
            // `Kept`, because a rescue is not a layout's opinion: a tiled pane
            // is still tiled, at the rectangle it was brought back to, and a
            // floating one must not become held inside a tile nobody gave it.
            let now = self.clock.now();
            self.move_pane(
                pane,
                moved,
                outer,
                AnimationSpec::default(),
                now,
                Standing::Kept,
            );
            tracing::info!(
                from = ?outer.loc,
                to = ?moved.loc,
                "a window was left on no screen and has been brought back"
            );
        }

        // **And the tile a maximised or fullscreen window is waiting to go
        // back into, by the same rule.** It was on the monitor that went as
        // well, and `pane_laid_out` answers it for a drag on that window, so
        // left there the drag would start from a rectangle on no screen. Every
        // pane rather than the stranded ones above, because the tile and the
        // window are two rectangles and it is the tile being asked about. The
        // next sweep replaces it with the layout's own answer either way.
        let tiles: Vec<_> = self
            .panes
            .iter()
            .filter_map(|pane| Some((pane.id(), self.rescued(pane.left_tile()?)?)))
            .collect();
        for (pane, back) in tiles {
            if let Some(held) = self.panes.get_mut(pane) {
                held.move_left_tile(back);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use smithay::utils::{Logical, Rectangle};

    use super::within;
    use crate::scripted::Edges;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    /// **A reserve adds to the layer-shell zone on its edge** (Ruling 10).
    #[test]
    fn a_reserve_adds_to_the_layer_zone_on_its_edge() {
        let whole = rect(0, 0, 1920, 1080);
        let zone = rect(0, 30, 1920, 1050);
        assert_eq!(
            within(
                whole,
                zone,
                Edges {
                    top: 20,
                    bottom: 48,
                    ..Edges::default()
                }
            ),
            rect(0, 50, 1920, 982)
        );
    }

    /// **A reserve never takes the whole monitor**: at least one logical
    /// pixel is left on each axis (Ruling 10).
    #[test]
    fn a_reserve_never_takes_the_whole_monitor() {
        let whole = rect(0, 0, 1920, 1080);
        let area = within(
            whole,
            whole,
            Edges {
                top: 2000,
                ..Edges::default()
            },
        );
        assert!(area.size.h >= 1 && area.size.w == 1920, "{area:?}");
    }

    /// **A reserve too large for any monitor leaves one pixel**, beside a
    /// layer-shell bar on the same edge and with two of them added
    /// together, rather than overflowing (Ruling 10).
    #[test]
    fn a_reserve_too_large_for_any_monitor_leaves_one_pixel() {
        let whole = rect(0, 0, 1920, 1080);
        let zone = rect(0, 0, 1920, 1050);
        let huge = Edges {
            bottom: i32::MAX,
            right: i32::MAX,
            ..Edges::default()
        };
        assert_eq!(
            (
                within(whole, zone, huge),
                within(whole, whole, huge.add(huge))
            ),
            (rect(0, 0, 1, 1), rect(0, 0, 1, 1))
        );
    }

    /// **A monitor with no size yet still has a work area**, and reserving
    /// on it does not panic.
    #[test]
    fn a_monitor_with_no_size_yet_is_reserved_on_without_a_panic() {
        let none = rect(0, 0, 0, 0);
        let area = within(
            none,
            none,
            Edges {
                top: 10,
                left: 10,
                ..Edges::default()
            },
        );
        assert_eq!(area, rect(0, 0, 1, 1));
    }
}
