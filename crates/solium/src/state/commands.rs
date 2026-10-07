//! The scripts' side: starting and reloading them; running a key binding, a click, a drop or a
//! wheel turn through them; telling them to relay out, that the monitors changed, or that a
//! reload restored them; and `apply`, which carries out the commands they answer with.

use super::*;

/// How many rounds of `done`s one dispatch tells, each round the attempts the
/// round before it settled, before it leaves the rest to the next dispatch.
/// `real_client::reflow_on_close::hosted::a_done_that_acts_again_each_time_it_is_told_costs_rounds_not_the_session`.
pub(super) const ATTEMPT_ROUNDS: usize = 16;

/// The pure decision behind
/// [`Solium::warn_about_unwatched_configured_paths`]: which of `configured`
/// is not covered by `roots` *and* has not already been added to `warned` --
/// inserting into `warned` as it goes, so a path seen on an earlier reload is
/// not returned again.
///
/// Split out the same way [`crate::autoreload::resolve_roots`] is split from
/// `watch_roots`: a test drives this with plain `PathBuf`s it makes up,
/// rather than a real pane style or shell scene that would have to resolve
/// against the filesystem to be worth anything.
/// `tests::an_unwatched_path_is_returned_once`,
/// `tests::a_watched_path_is_never_returned`,
/// `tests::an_already_warned_path_is_not_returned_again`.
fn newly_unwatched_paths(
    configured: Vec<std::path::PathBuf>,
    roots: &[std::path::PathBuf],
    warned: &mut std::collections::HashSet<std::path::PathBuf>,
) -> Vec<std::path::PathBuf> {
    configured
        .into_iter()
        .filter(|path| !crate::autoreload::path_is_watched(path, roots))
        .filter(|path| warned.insert(path.clone()))
        .collect()
}

impl Solium {
    /// Turn what a script aimed at into what the compositor holds.
    ///
    /// The one place a surface's *name* becomes a [`crate::scripted::SurfaceId`]
    /// — which is what makes an anchor `Copy` and a `Frame` still cheap to
    /// blend. A name nobody has declared loses the effect and not the window,
    /// the same failure an anchor that stops resolving already has, and it says
    /// so once rather than every frame.
    fn aimed(&self, deform: &crate::script::Deform) -> Option<present::Deform> {
        let anchor = match &deform.aim {
            crate::script::Aim::Rect(rect) => {
                present::Anchor::Rect(present::logical((rect.x, rect.y), (rect.w, rect.h)))
            }
            crate::script::Aim::Window(id) => present::Anchor::Pane(*id),
            crate::script::Aim::Surface(name) => match self.surfaces.named(name) {
                Some(id) => present::Anchor::Surface(id),
                None => {
                    tracing::warn!(
                        surface = name,
                        "no surface by that name to aim at, drawing the window undeformed"
                    );
                    return None;
                }
            },
        };
        Some(present::Deform {
            effect: deform.effect,
            anchor,
        })
    }

    /// Put back the windows a membership change has just moved.
    ///
    /// **What happens when membership changes while things are animating**, and
    /// the reason it is not a jump. A window sent to another workspace leaves
    /// one selection for another, and the difference between the two shifts
    /// lands on it between one frame and the next; this displaces its own
    /// transform by exactly that much, so the frame after the change draws it
    /// where the frame before did, and animates it home.
    ///
    /// Only windows. A surface joining a selection has no transform of its own
    /// to displace — there is nowhere to put one, and the case it would cover
    /// (a wallpaper changing desk) is not a thing a desk does. A selection that
    /// names a monitor is not rebased either: what is on a screen changes
    /// because the *user* dragged a window across a bezel, which no declaration
    /// observes.
    fn keep_displaced(
        &mut self,
        displaced: &crate::group::Displaced,
        now: std::time::Duration,
        animation: crate::script::AnimationSpec,
    ) {
        if displaced.is_empty() {
            return;
        }
        for (id, by) in displaced {
            let Some(pane) = self.panes.by_script_id(*id) else {
                continue;
            };
            // Not a window whose client has gone: it is drawn under the
            // selections it was in when it went, by name, which no membership
            // change touches, so nothing moved it and there is nothing to put
            // back. Rebased, it was displaced twice, and glided out from under
            // its desk while it faded.
            // `a_window_that_left_keeps_the_shift_its_desk_had`.
            if pane.ghost() {
                continue;
            }
            let outer = self.pane_outer(pane);
            present::rebase(pane, outer, *by, now, animation.duration, animation.easing);
        }
        self.redraw = true;
    }

    /// Run whatever a key combination is bound to, and apply what it asked for.
    pub(crate) fn trigger(&mut self, combo: &str) -> bool {
        let snapshot = self.snapshot();
        // Taken out for the call so no part of the compositor is borrowed while
        // Lua runs, and a script cannot re-enter the seat mid-dispatch.
        let Some(mut scripts) = self.scripts.take() else {
            return false;
        };
        let outcome = scripts.key(combo, snapshot);
        self.scripts = Some(scripts);

        let handled = outcome.handled;
        self.apply(outcome);
        handled
    }

    /// Give a pointer press to the mode that owns input.
    pub(crate) fn trigger_click(&mut self, x: f64, y: f64) -> bool {
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return false;
        };
        let outcome = scripts.click(x, y, snapshot);
        self.scripts = Some(scripts);

        let handled = outcome.handled;
        self.apply(outcome);
        handled
    }

    /// Apply what a script asked for.
    pub(crate) fn apply(&mut self, outcome: Outcome) {
        self.dispatching += 1;
        // Anything a script asked for changes what is on screen, and almost
        // all of it starts an animation. Damage-driven rendering only draws
        // when something says it must, and a transform created here says
        // nothing on its own — so without this the animation does not advance
        // until some *unrelated* damage happens to wake the loop, at which
        // point it jumps straight to wherever the clock says it should be.
        //
        // That is the whole of "sometimes the animation is instant, sometimes
        // too fast, sometimes right": it was running at the mercy of whatever
        // else happened to be redrawing.
        if !outcome.commands.is_empty() {
            self.redraw = true;
        }
        // Whether a placement below changed what a window says about being
        // cramped. See the end of this function.
        let mut cramped_changed = false;

        if let Some(grab) = outcome.grab
            && grab != self.script_grab
        {
            self.script_grab = grab;
            tracing::debug!(grab, "script input grab changed");
        }
        if let Some(status) = outcome.status
            && status != self.status
        {
            tracing::debug!(status, "mode changed");
            self.status = status;
            // `Solium.status` is published only from a frame, same as every
            // other model; a binding that only calls `sol.status` must still
            // ask for one, not rely on a command alongside it.
            self.redraw = true;
        }

        let now = self.clock.now();
        for command in outcome.commands {
            match command {
                Command::Present {
                    id,
                    rect,
                    opacity,
                    matrix,
                    deform,
                    z,
                    pivot,
                    animation,
                } => {
                    let Some(pane) = self.panes.by_script_id(id) else {
                        continue;
                    };
                    let outer = self.pane_outer(pane);
                    let rect = rect.map_or_else(
                        || outer.to_f64(),
                        |rect| present::logical((rect.x, rect.y), (rect.w, rect.h)),
                    );
                    let target = Frame {
                        matrix: matrix.unwrap_or(crate::mat4::Mat4::IDENTITY),
                        rect,
                        // A script's rectangle is a picture of the window it
                        // moves there -- a thumbnail is the window made small
                        // -- so it is zoomed by what it is over the window's
                        // own rectangle. See `Frame::zoom`.
                        zoom: Frame::zoom_of(rect, outer),
                        opacity: opacity.unwrap_or(1.0),
                        deform: deform.and_then(|deform| self.aimed(&deform)),
                        // Both arrive resolved: `script::depth_from` and
                        // `script::pivot_from` hold the defaults, so a table
                        // mentioning neither key produces them there rather
                        // than here. Still spelled out rather than
                        // `..Frame::real(outer)`, because a struct update
                        // would take whatever field is added next without
                        // anyone looking at this line again.
                        z,
                        pivot,
                    };
                    present::present(
                        pane,
                        outer,
                        target,
                        now,
                        animation.duration,
                        animation.easing,
                    );
                }
                Command::PresentFrom {
                    id,
                    rect,
                    opacity,
                    animation,
                } => {
                    let Some(pane) = self.panes.by_script_id(id) else {
                        continue;
                    };
                    let outer = self.pane_outer(pane);
                    let rect = present::logical((rect.x, rect.y), (rect.w, rect.h));
                    let start = Frame {
                        matrix: crate::mat4::Mat4::IDENTITY,
                        rect,
                        // The window it grows into, drawn smaller: `open.lua`
                        // shrinks the window's own rectangle. See `Frame::zoom`.
                        zoom: Frame::zoom_of(rect, outer),
                        opacity: opacity.unwrap_or(1.0),
                        deform: None,
                        // Explicit so the next field added breaks this line
                        // instead of being defaulted past it -- and these two
                        // stay the defaults because `sol.present_from` reads
                        // no keys for them. It could not honour them if it
                        // did: this is the frame a window animates *from*, and
                        // both fields select the destination's value at the
                        // first blended frame, so a depth or a pivot here
                        // would never be on screen. A script wanting either
                        // says so with `sol.present` once it has landed.
                        z: 0.0,
                        pivot: (0.5, 0.5),
                    };
                    present::from(
                        pane,
                        outer,
                        start,
                        now,
                        animation.duration,
                        animation.easing,
                    );
                }
                Command::Clear { id, animation } => {
                    let Some(pane) = self.panes.by_script_id(id) else {
                        continue;
                    };
                    let outer = self.pane_outer(pane);
                    // Deliberately discarded, unlike `give_back`'s. A script
                    // clearing a transform is restated by the next call through
                    // this queue, and nothing here retires a piece of state that
                    // the clear is the only way out of -- which is the whole of
                    // why `clear` reports at all.
                    let _ = present::clear(pane, outer, now, animation.duration, animation.easing);
                }
                Command::Focus { id } => {
                    if let Some(window) = self.window_by_id(id) {
                        tracing::debug!(id, title = self.window_title(&window), "script focused");
                        self.focus_window(&window, SERIAL_COUNTER.next_serial());
                    }
                }
                // The client's own requests, made for it, so a key sends a
                // window fullscreen exactly as the window asking would. See
                // `a_script_toggles_fullscreen_and_maximised_as_the_client_would`.
                Command::ToggleFullscreen { id } => {
                    if let Some(toplevel) = self
                        .window_by_id(id)
                        .and_then(|window| window.toplevel().cloned())
                    {
                        let fullscreen = toplevel.with_pending_state(|state| {
                            state.states.contains(xdg_toplevel::State::Fullscreen)
                        });
                        if fullscreen {
                            XdgShellHandler::unfullscreen_request(self, toplevel);
                        } else {
                            XdgShellHandler::fullscreen_request(self, toplevel, None);
                        }
                    }
                }
                Command::ToggleMaximize { id } => {
                    if let Some(window) = self.window_by_id(id) {
                        self.toggle_maximize(&window);
                    }
                }
                Command::Place {
                    id,
                    rect,
                    animation,
                    tile,
                    inside,
                    cramped,
                    moved,
                } => {
                    let standing = match (tile, inside) {
                        (false, _) => Standing::Free,
                        (true, Some(inside)) => Standing::Within(outer_of(inside)),
                        (true, None) => Standing::Tile,
                    };
                    if moved {
                        self.carry_across(id, rect, standing);
                    }
                    self.place(id, rect, animation, now, standing);
                    // The layout's word for this placement, said afresh each
                    // time; a window placed out of a tile is in no tile to be
                    // cramped in.
                    if let Some(pane) = self.panes.by_script_id(id).map(Pane::id)
                        && let Some(pane) = self.panes.get_mut(pane)
                    {
                        cramped_changed |= pane.cramped() != (cramped && tile);
                        pane.set_cramped(cramped && tile);
                    }
                }
                Command::Unplace { id } => {
                    if let Some(pane) = self.panes.by_script_id(id).map(Pane::id)
                        && let Some(pane) = self.panes.get_mut(pane)
                    {
                        pane.untile();
                        cramped_changed |= pane.cramped();
                        pane.set_cramped(false);
                    }
                }
                Command::Close { id } => {
                    if let Some(pane) = self.panes.by_script_id(id).map(Pane::id) {
                        self.close_pane(pane);
                    }
                }
                // One of the compositor's verbs, done as the command that does
                // it, and its outcome kept for the attempt's `done` (Ruling 15).
                // `real_client::reflow_on_close::hosted::sol_act_tells_done_once_the_window_was_asked_to_close`,
                // `real_client::reflow_on_close::hosted::sol_act_answers_why_it_could_not`.
                Command::Act {
                    attempt,
                    action,
                    data,
                } => match self.act(&action, &data) {
                    Ok(command) => {
                        self.apply(Outcome {
                            commands: vec![command],
                            ..Outcome::default()
                        });
                        self.settled_attempts.push(crate::script::Settled {
                            attempt,
                            ok: true,
                            reason: None,
                        });
                    }
                    Err(reason) => self.settled_attempts.push(crate::script::Settled {
                        attempt,
                        ok: false,
                        reason: Some(reason),
                    }),
                },
                // What Lua says the workspaces are, less the monitors and
                // windows the compositor does not have and any workspace
                // declared twice, each logged once.
                // `real_client::reflow_on_close::hosted::workspace_rows_count_their_windows_and_say_which_is_shown`,
                // `crate::models::workspaces::tests::an_unknown_monitor_or_window_is_logged_once_per_name`,
                // `crate::models::workspaces::tests::a_workspace_declared_twice_is_one_row`.
                Command::Workspaces(declared) => {
                    let monitors: Vec<String> = self.space.outputs().map(Output::name).collect();
                    let windows: Vec<u64> = self
                        .snapshot()
                        .windows
                        .iter()
                        .map(|window| window.id)
                        .collect();
                    crate::models::workspaces::log_unknown(
                        &declared,
                        &monitors,
                        &windows,
                        &mut self.unknown_in_workspaces,
                    );
                    self.workspaces = Some(declared.validated(&monitors, &windows));
                }
                Command::Loading(loading) => {
                    if self.loading != loading {
                        tracing::debug!(?loading, "loading behaviour set");
                        self.loading = loading;
                    }
                }
                Command::ClientSizes(sizes) => {
                    if self.client_sizes != sizes {
                        tracing::debug!(?sizes, "whose own sizes a floating drag believes set");
                        self.client_sizes = sizes;
                    }
                }
                Command::Idle(settings) => self.idle.configure(settings),
                Command::AutoReload(settings) => self.configure_autoreload(settings),
                // Only the data: applying it to real devices needs a
                // libinput handle, which only a backend has. `tty.rs` reads
                // `self.input.config()` on `DeviceAdded` and again on
                // reload; the nested backend never does, which is correct --
                // it has no libinput devices to apply anything to.
                Command::Input(config) => self.input.configure(config),
                Command::Lock(settings) => {
                    self.logind.configure(settings, self.lock.is_some());
                }
                Command::Power { monitor, on } => match monitor {
                    None => self.power_all(on),
                    Some(name) => {
                        let found = self
                            .space
                            .outputs()
                            .find(|output| output.name() == name)
                            .cloned();
                        match found {
                            Some(output) => {
                                self.set_power(&output, on);
                            }
                            None => tracing::warn!(
                                monitor = name,
                                "sol.monitor_power: no monitor by that name -- `sol.monitors()` \
                                 lists the ones there are"
                            ),
                        }
                    }
                },
                Command::Resize(resizing) => {
                    if self.resizing != resizing {
                        tracing::debug!(?resizing, "resize behaviour set");
                        self.resizing = resizing;
                    }
                }
                Command::Fullscreen(covers) => {
                    if self.fullscreen_covers != covers {
                        tracing::debug!(?covers, "what a fullscreen window covers set");
                        self.fullscreen_covers = covers;
                        self.redraw = true;
                    }
                }
                Command::Cursor(configured) => {
                    // The environment is re-read here rather than cached at
                    // startup, because this also runs on `super+shift+r` and a
                    // reload is the one moment a session can pick up an
                    // `XCURSOR_THEME` that was exported after the compositor
                    // started. Two `env::var` calls per reload.
                    //
                    // And a reload that really did change the pointer damages
                    // the screen. The pointer is rebuilt for every output on
                    // every *frame* (see `render::cursor`), which is not the
                    // same as there being a frame: both backends draw on
                    // damage, and a pointer sitting still produces none. So a
                    // `super+shift+r` that changed only the cursor theme or
                    // size would otherwise show the new pointer whenever
                    // something unrelated next happened to redraw — which,
                    // while trying a theme out, is when the mouse is jiggled.
                    //
                    // `configure` answers `false` when nothing changed, which
                    // on a reload that changed a keybinding is every time, so
                    // the ordinary reload still schedules nothing.
                    if self
                        .pointer
                        .configure(&configured, &crate::cursor::theme::Environment::read())
                    {
                        self.redraw = true;
                    }
                }
                Command::Decoration { name } => {
                    // The slots windows occupy are kept; what changes is how
                    // much of each slot the frame takes, so every client is
                    // resized to whatever the new decoration left it.
                    let slots: Vec<(Window, Rectangle<i32, Logical>)> = self
                        .space
                        .elements()
                        .cloned()
                        .collect::<Vec<_>>()
                        .into_iter()
                        .filter_map(|window| {
                            self.outer_geometry(&window).map(|outer| (window, outer))
                        })
                        .collect();
                    if self.decorations.set_style(&mut self.panes, name) {
                        for (window, outer) in slots {
                            self.resize_to(&window, outer);
                        }
                        self.redraw = true;
                        // Resizing each window in place keeps a floating
                        // arrangement looking right, but a tiled one is the
                        // layout's arithmetic and only the layout can redo it.
                        self.trigger_relayout();
                    }
                }
                Command::PaneValues(fields) => {
                    if self.decorations.merge_values(fields) {
                        self.redraw = true;
                    }
                }
                Command::Spawn { program, args } => self.spawn(&program, &args),
                Command::FolderTrust { absolute } => {
                    self.folder_trust.trust(&absolute);
                    if let Some(entry) = self.folder.iter_mut().find(|entry| {
                        crate::folder::path_from_uri(&entry.uri)
                            .is_some_and(|path| path.display().to_string() == absolute)
                    }) {
                        entry.trusted = true;
                    }
                }
                Command::Reload => self.request = Some(Request::Reload),
                Command::Keyboard(request) => {
                    let keymap = self.keymap.clone();
                    if crate::keymap::apply(self, &request) {
                        let now = crate::keymap::describe(self);
                        tracing::info!(
                            layouts = ?now.layouts,
                            active = now.active,
                            repeat = format!("{}/s after {}ms", now.repeat_rate, now.repeat_delay),
                            "keyboard"
                        );
                    }
                    // A new keymap is a new keyboard and not a switch, and is
                    // told nothing; a switch or a lock is told:
                    // `keyboard_change::tests::a_new_keymap_and_a_starting_configuration_are_told_nothing`,
                    // `keyboard_change::tests::sol_keyboard_active_is_told_as_a_layout_change`.
                    if self.keymap != keymap {
                        self.keyboard_told.forget();
                    }
                    self.keyboard_changed();
                }
                Command::Surface(surface) => self.declare_surface(*surface),
                Command::SurfaceGone(name) => self.remove_surface(&name),
                Command::Group {
                    name,
                    selection,
                    animation,
                } => {
                    let displaced = match selection {
                        Some(selection) => {
                            let selection = crate::group::selection_of(&selection, &self.surfaces);
                            self.groups.declare(&name, selection, now)
                        }
                        None => self.groups.forget(&name, now),
                    };
                    self.keep_displaced(&displaced, now, animation);
                }
                Command::PresentGroup {
                    name,
                    to,
                    animation,
                } => {
                    if !self
                        .groups
                        .present(&name, to, now, animation.duration, animation.easing)
                    {
                        // Named rather than ignored, for the reason
                        // `sol.surface` names a scene it cannot find: a
                        // transform on a selection nobody declared is a typo,
                        // and a mode that silently does nothing is the hardest
                        // kind of configuration mistake to find.
                        tracing::warn!(group = name, "no selection by that name to carry");
                    }
                }
                Command::ClearGroup { name, animation } => {
                    self.groups
                        .clear(&name, now, animation.duration, animation.easing);
                }
                Command::Monitors(arrangement) => {
                    let was = std::mem::replace(&mut self.arrangement, arrangement);
                    // `enabled = false` on a monitor is an unplug as far as
                    // everything downstream is concerned, and `enabled = true`
                    // is a plug -- so the backend is asked to look again
                    // rather than this growing its own way to drop a screen.
                    // Nothing to do nested: there are no connectors there.
                    if was.enablement() != self.arrangement.enablement() {
                        self.rescan_outputs = true;
                    }
                    // Applied immediately, and applied again on reload, so
                    // moving a monitor is `super+shift+r` rather than logging
                    // out. Anything already placed is now measured against a
                    // different work area, which is why the layout is asked to
                    // run again.
                    self.place_outputs();
                    self.monitors_rearranged = true;
                    self.trigger_relayout();
                    self.redraw = true;
                }
                Command::Quit => {
                    tracing::info!("a script asked to stop");
                    self.request = Some(Request::Quit);
                }
            }
        }
        // **A window whose `cramped` changed is told again, once** (#115).
        // `sol.windows()` is the snapshot taken before the handler that said
        // it ran, so nothing reading the window list in that handler -- a bar
        // redrawing on `layout` -- could see what the layout had just said,
        // and on a quiet desktop no later event comes to show it. So the
        // layouts run once more, as `Command::Monitors` has them do above, with
        // a snapshot that has it. Once: a pass that changes it again is not
        // told again, so a layout that cannot make up its mind cannot loop.
        // `real_client::client_sizes::a_window_that_becomes_cramped_is_told_again_so_a_bar_can_see_it`.
        if cramped_changed && !self.retelling_cramped {
            self.retelling_cramped = true;
            self.trigger_relayout();
            self.retelling_cramped = false;
        }
        self.dispatching -= 1;
        // Once the whole dispatch is done, every handler it ran included, and
        // not when the layout pass inside it is: what follows `sol.monitors{}`
        // may say where a surface now is.
        // `real_client::a_runtime_primary_change_drops_the_old_primarys_scene`,
        // `real_client::a_binding_that_moves_the_primary_and_its_surface_together_keeps_the_scene`.
        if self.dispatching == 0 && self.monitors_rearranged {
            self.sync_instances();
        }
        // And what its declarations made the scenes say, once it is all
        // applied (`Solium::settle_scenes_once_dispatched`):
        // `real_client::reflow_on_close::hosted::a_property_a_layout_handler_writes_reflows_the_windows_against_the_reserve_it_moved`.
        if self.dispatching == 0 && self.scenes_to_settle {
            self.settle_scenes();
        }
        self.tell_settled_attempts();
    }

    /// Tell each `done` what became of its `sol.act`, in a dispatch of their
    /// own, once the outermost dispatch that settled it is applied whole,
    /// every command after it in its batch included (03 §3.3.2), and after a
    /// hotplug's or a reload's held handlers.
    /// `real_client::reflow_on_close::hosted::sol_act_tells_done_once_the_window_was_asked_to_close`,
    /// `real_client::reflow_on_close::hosted::sol_act_answers_why_it_could_not`,
    /// `real_client::reflow_on_close::hosted::done_is_told_after_every_command_of_the_batch_that_ran_its_act`,
    /// `real_client::reflow_on_close::hosted::a_sol_act_in_a_hotplugs_handler_hears_done_in_the_hotplugs_dispatch`,
    /// `real_client::reflow_on_close::hosted::a_sol_act_in_a_reloaded_configuration_hears_done_in_the_reloads_dispatch`.
    ///
    /// Never from inside itself: what a `done` asks for is told by this
    /// loop, a round at a time, so a `done` that acts again each time it is
    /// told cannot run the stack out, and past `ATTEMPT_ROUNDS` rounds the
    /// rest wait for the next dispatch.
    /// `real_client::reflow_on_close::hosted::a_done_that_acts_again_each_time_it_is_told_costs_rounds_not_the_session`.
    pub(crate) fn tell_settled_attempts(&mut self) {
        if self.dispatching > 0 || self.telling_attempts {
            return;
        }
        self.telling_attempts = true;
        for _ in 0..ATTEMPT_ROUNDS {
            if self.settled_attempts.is_empty() {
                break;
            }
            let snapshot = self.snapshot();
            let Some(mut scripts) = self.scripts.take() else {
                break;
            };
            let settled = std::mem::take(&mut self.settled_attempts);
            let outcome = scripts.attempts_settled(&settled, snapshot);
            self.scripts = Some(scripts);
            self.apply(outcome);
        }
        self.telling_attempts = false;
    }

    /// The window a script means by an id.
    ///
    /// Ids that no longer exist are simply not found — a window closing while a
    /// mode holds its id is ordinary, not an error.
    pub(super) fn window_by_id(&self, id: u64) -> Option<Window> {
        self.panes.by_script_id(id).and_then(Pane::client).cloned()
    }

    /// Offer a newly shown window to whatever script wants to animate it in.
    /// Read the configuration again and swap it in.
    ///
    /// The QML cache is cleared and every frame rebuilt too, so editing a
    /// decoration is the same one keystroke as editing a binding. A file that
    /// fails to load leaves the running configuration alone: a typo should
    /// cost a log line, not the session.
    ///
    /// ## A reload replaces the scripts, not the session
    ///
    /// The session is older than the scripts reading it. The windows, the
    /// monitors, the workspace in view and the layout in charge all outlive
    /// `super+shift+r`; the Lua state does not. So the second half of a reload
    /// is putting the new scripts back in touch with the session they have
    /// inherited, and it has two parts, both of which are the contract on
    /// [`Scripts::load_carrying`]:
    ///
    ///  * `Scripts::kept` and `load_carrying` hand back what the old scripts
    ///    asked to keep, *before* the new ones run, so a script's top level
    ///    sees its own state rather than its defaults;
    ///  * `restore`, `monitors` and `layout` re-announce the world, in that
    ///    order, so nothing has to be kept that could be recomputed.
    ///
    /// **The order is the same one a hotplug uses** — see
    /// [`Self::settle_monitors`] — with `restore` in front of it. That is not
    /// a coincidence to be tidied away later: "the screens are not the screens
    /// you knew" is exactly a new script set's position, and a layout that
    /// handles a monitor arriving already handles this.
    ///
    /// **This is what issue #116 was.** Only `layout` reached the new scripts,
    /// and only by accident: shipped `init.lua` calls `sol.monitors`, whose
    /// command happens to trigger a relayout. `workspaces.lua` regrouped every
    /// window onto desk 1 from that `layout` while desk 1 was still carried
    /// two screen-widths off-stage by the *previous* session's view, and
    /// nothing put it back — `super+1` did not, because the fresh Lua state
    /// believed workspace 1 was already showing.
    pub(crate) fn reload(&mut self) {
        self.reload_from(&Scripts::config_path());
    }

    /// [`Self::reload`], from the file at `path`, which is how
    /// `tests::real_client::a_reload_that_moves_the_primary_drops_the_old_primarys_scene`
    /// reloads.
    pub(crate) fn reload_from(&mut self, path: &std::path::Path) {
        // Collected before the new configuration is even read, because reading
        // it is what may fail, and the failure path has to leave the running
        // scripts -- and therefore their keep -- untouched.
        let carried = self.scripts.as_ref().map(Scripts::kept).unwrap_or_default();
        match Scripts::load_carrying(path, carried) {
            Ok(scripts) => {
                crate::qml::clear_cache();
                // Installed applications can change between one session and
                // the next edit of a configuration (an install, an update), so
                // a reload is the one point this version rescans them
                // (`apps.rs`'s module doc, `models::mod`'s `publish_models`).
                self.apps_scan_pending = true;
                // `Solium.dirs.desktop` can change too (a session's
                // `XDG_DESKTOP_DIR` edited, `user-dirs.dirs` regenerated), so
                // a reload re-resolves and rescans the desktop folder the
                // same way (`folder.rs`'s module doc, `models::mod`'s
                // `publish_models`).
                self.folder_scan_pending = true;
                // And with Qt's cache of a scene that would not load gone, the
                // scene is tried again: a reload is what anybody presses after
                // mending one (`a_reload_tries_again_a_scene_that_would_not_load`).
                for surface in self.surfaces.iter_mut() {
                    surface.forget_failures();
                }
                let style = self.decorations.style().map(ToOwned::to_owned);
                // Twice, and to the same place it started, to defeat
                // `set_style`'s "nothing changed" guard. What the second call
                // does is *rebuild* every existing frame rather than drop it
                // -- see the comment on it, and the reason: dropping leaves
                // every open window bare until it is reopened -- so a window
                // that is framed before a reload is framed after it, by a
                // different `Decoration` built from the file as it now reads.
                self.decorations.set_style(&mut self.panes, None);
                self.decorations.set_style(&mut self.panes, style);
                // Held as one dispatch, so a `sol.monitors{}` at the top of
                // the new configuration places no surface before the handlers
                // below have said where it now is
                // (`a_reload_that_moves_a_monitor_keeps_the_scene_its_handler_declares_there`).
                self.dispatching += 1;
                // What the old configuration handed every frame was its own:
                // the new one starts with none, so a key it no longer hands
                // over is not left on screen
                // (`decoration::tests::a_reload_starts_the_frames_values_afresh`).
                self.decorations.clear_values();
                // Likewise: the old declaration is the previous session's,
                // and a configuration that stops calling `sol.workspaces`
                // must publish none, not what it last said. `workspaces.lua`
                // declares again inside this same held dispatch, so nothing
                // flickers for one that still does.
                self.workspaces = None;
                self.start_scripts(Some(scripts));
                // The re-announcement, in the order the doc comment states.
                // Three dispatches and not one, each with its own snapshot,
                // because what `monitors` does changes what `layout` is
                // looking at -- `workspaces.lua` moves every desk in the first
                // and arranges the windows on the one in view in the second.
                self.trigger_restored();
                self.trigger_monitors_changed();
                self.trigger_relayout();
                self.dispatching -= 1;
                // As a hotplug does, after the handlers: a configuration that
                // made another monitor primary may declare nothing differently
                // (`a_reload_that_moves_the_primary_drops_the_old_primarys_scene`).
                self.sync_instances();
                // And what the held handlers' `sol.act`s came to, which no
                // dispatch inside the hold could tell
                // (`a_sol_act_in_a_reloaded_configuration_hears_done_in_the_reloads_dispatch`).
                self.tell_settled_attempts();
                self.redraw = true;
                tracing::info!(config = %path.display(), "configuration reloaded");
                // And then look at what that produced -- at where it *lands*,
                // not at this frame, which is still the previous session's.
                // See `everything_is_off_stage` for both halves: why the
                // question is asked here and nowhere else -- a reload is both
                // the keypress that lost the desktop and the keypress anybody
                // reaches for when it is gone, so it is the one moment where
                // the answer is worth having whichever way it comes out -- and
                // why asking it of the current frame answered about the wrong
                // session.
                if self.everything_is_off_stage() == Some(true) {
                    tracing::warn!(
                        "when the movement this reload started has landed, every window will be \
                         drawn outside every screen. If that is not simply a workspace with \
                         nothing on it, a selection is carrying the desktop off-stage and only \
                         something that names that selection can carry it back: switch workspace \
                         away and back again, which re-states where every desk sits"
                    );
                }
            }
            Err(err) => {
                tracing::error!(?err, config = %path.display(), "reload failed, keeping what was running");
            }
        }
    }

    /// Automatic reload (#223): store the settings, and arm or disarm the
    /// watch to match.
    ///
    /// `automatic = false` does not merely let the quiet period run out and
    /// reload nothing -- it tears every watch down, so the loop source that
    /// would otherwise wake for a change never fires at all. That is what
    /// makes the setting answer "never", not "eventually, if you wait long
    /// enough" (`autoreload::tests` and this module's own
    /// `tests::automatic_false_leaves_nothing_watched`).
    ///
    /// Reached from `Command::AutoReload`, which `self.apply` can run from
    /// anywhere a script runs -- `start_scripts` (cold start and every
    /// reload) and a binding of the user's own alike -- so this recomputes
    /// [`crate::autoreload::watch_roots`] every time rather than only at
    /// start-up: a directory created since the last reload (a first
    /// `user.lua`, just written) is picked up the next time anything calls
    /// `sol.auto_reload`, which the shipped `init.lua` does on every reload.
    pub(crate) fn configure_autoreload(&mut self, settings: crate::autoreload::Settings) {
        if self.autoreload_settings != settings {
            tracing::debug!(?settings, "automatic reload settings set");
            self.autoreload_settings = settings;
        }
        let roots = if settings.automatic {
            crate::autoreload::watch_roots()
        } else {
            Vec::new()
        };
        self.autoreload_watcher.set_roots(&roots);
    }

    /// Warn once per path when a shell scene, pane style, or loading scene a
    /// script configured lives somewhere automatic reload does not watch --
    /// the gap `autoreload`'s module doc names: a plain absolute path set
    /// directly in `config.lua`, with none of `SOLIUM_SHELL_SCENE`/
    /// `SOLIUM_PANE`/`SOLIUM_QML_TITLEBAR`/`SOLIUM_LOADING` naming it instead
    /// (those are already folded into [`crate::autoreload::watch_roots`] by
    /// its own `override_roots`, so they never reach this function's warning).
    ///
    /// Called from `start_scripts`, after every reload as well as cold start,
    /// so a path a reload just changed to is checked too. Padded with the
    /// shipped `qml/` and `lua/` directories before comparing, so the
    /// ordinary case -- nothing overridden, every scene the one Solium ships
    /// -- never warns.
    /// `tests::warns_once_for_a_pane_style_set_outside_every_watched_root`,
    /// `tests::does_not_warn_for_a_shipped_default_style`,
    /// `tests::automatic_false_warns_about_nothing`.
    pub(crate) fn warn_about_unwatched_configured_paths(&mut self) {
        if !self.autoreload_settings.automatic {
            return;
        }
        let mut roots = crate::autoreload::watch_roots();
        roots.push(crate::assets::qml());
        roots.push(crate::assets::lua());

        let mut configured: Vec<std::path::PathBuf> = self
            .surfaces
            .iter()
            .map(|surface| surface.declared.scene.clone())
            .collect();
        if let Some(style) = crate::decoration::style_file(self.decorations.style()) {
            configured.push(style);
        }
        configured.push(crate::pane::loading_source(self.loading.scene.as_deref()));

        for path in newly_unwatched_paths(configured, &roots, &mut self.autoreload_unwatched_warned)
        {
            tracing::warn!(
                path = %path.display(),
                "set outside every directory automatic reload watches (#223); \
                 editing it will not reload Solium on its own"
            );
        }
    }

    /// Take the scripts, and act on whatever they asked for while loading.
    pub(crate) fn start_scripts(&mut self, scripts: Option<Scripts>) {
        // Before the scripts run, so `sol.keyboard()` answers truthfully even
        // in a configuration that never calls `sol.keyboard{…}` -- which is
        // the common case, since the useful default is whatever the session's
        // `XKB_DEFAULT_*` already said.
        self.keyboard = crate::keymap::describe(self);
        let Some(mut scripts) = scripts else {
            self.scripts = None;
            return;
        };
        let outcome = scripts.startup();
        self.scripts = Some(scripts);
        // What the configuration does to the keyboard as it starts is told
        // nothing, and what the keys do after it is:
        // `keyboard_change::tests::a_new_keymap_and_a_starting_configuration_are_told_nothing`.
        self.keyboard_told.forget();
        self.apply(outcome);
        self.keyboard_changed();
        self.warn_about_unwatched_configured_paths();
    }

    pub(crate) fn trigger_monitors_changed(&mut self) {
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return;
        };
        let outcome = scripts.monitors_changed(snapshot);
        self.scripts = Some(scripts);
        self.apply(outcome);
    }

    /// Tell the scripts they have replaced a running session's, not started one.
    ///
    /// Called from [`Self::reload`] and from nowhere else: a cold start has
    /// nothing to restore, and firing it there would make the event mean
    /// "loaded", which is a thing a script's own top level already is.
    fn trigger_restored(&mut self) {
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return;
        };
        let outcome = scripts.restored(snapshot);
        self.scripts = Some(scripts);
        self.apply(outcome);
    }

    pub(crate) fn trigger_relayout(&mut self) {
        // What this pass lays the windows out against, so a reserve that
        // changes later runs it again, and one that has not does not
        // (`real_client::reflow_on_close::hosted::a_scene_reserve_overrides_its_edge_and_reflows_the_layout_once`).
        self.laid_out_reserves = self.reserves();
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return;
        };
        let outcome = scripts.relayout(snapshot);
        self.scripts = Some(scripts);
        self.apply(outcome);
    }

    /// Tell scripts a drag finished, so a layout can put the window back.
    pub(crate) fn trigger_drop(&mut self, window: &Window, x: f64, y: f64) {
        let id = self.window_id(window);
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return;
        };
        let outcome = scripts.dropped(id, x, y, snapshot);
        self.scripts = Some(scripts);
        self.apply(outcome);
    }

    /// Offer a modified wheel turn to scripts. Returns whether one took it.
    pub(crate) fn trigger_scroll(&mut self, dx: f64, dy: f64) -> bool {
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return false;
        };
        let outcome = scripts.scrolled(dx, dy, snapshot);
        self.scripts = Some(scripts);
        let handled = outcome.handled;
        self.apply(outcome);
        handled
    }
}

#[cfg(test)]
mod tests {
    use super::newly_unwatched_paths;
    use std::{collections::HashSet, path::PathBuf};

    /// A path with no root covering it is reported, once.
    #[test]
    fn an_unwatched_path_is_returned_once() {
        let path = PathBuf::from("/home/me/dev/my-shell/Shell.qml");
        let mut warned = HashSet::new();
        assert_eq!(
            newly_unwatched_paths(vec![path.clone()], &[], &mut warned),
            vec![path.clone()]
        );
        assert!(warned.contains(&path));
    }

    /// A root covering the path (an exact root, or an ancestor directory)
    /// means nothing is reported.
    #[test]
    fn a_watched_path_is_never_returned() {
        let root = PathBuf::from("/home/me/.config/solium");
        let path = root.join("qml").join("panes").join("mine").join("Pane.qml");
        let mut warned = HashSet::new();
        assert!(newly_unwatched_paths(vec![path], &[root], &mut warned).is_empty());
        assert!(warned.is_empty());
    }

    /// Having warned about a path once, a later call with the same `warned`
    /// set does not return it again -- the log line it drove is not meant to
    /// repeat every reload.
    #[test]
    fn an_already_warned_path_is_not_returned_again() {
        let path = PathBuf::from("/home/me/dev/my-shell/Shell.qml");
        let mut warned = HashSet::new();
        warned.insert(path.clone());
        assert!(newly_unwatched_paths(vec![path], &[], &mut warned).is_empty());
    }
}
