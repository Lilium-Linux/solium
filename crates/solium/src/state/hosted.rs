//! The compositor's side of hosted scenes: reading what they report, and what
//! that changes.

use std::{collections::BTreeMap, time::Duration};

use smithay::{
    input::pointer::MotionEvent,
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{IsAlive as _, Logical, Point, Rectangle, SERIAL_COUNTER},
    wayland::pointer_constraints::with_pointer_constraint,
};

use crate::{
    json::Json,
    qml::hosted::{GrabReport, KeyboardReport, PointerKind, SceneKey, ScenePointer},
    script::Command,
    scripted::{Edges, KeyPolicy, Outside, SurfaceId},
    state::{ScenePress, Solium},
};

/// The scene holding the keyboard (Ruling 14): whose, on which monitor, the
/// keys its holding item claims, its surface's policy for the bindings when
/// it took the keyboard, which a press reads from the surface again, and the
/// surface to give the keyboard back to.
/// `state::tests::real_client::reflow_on_close::hosted::the_window_gets_the_keyboard_back_when_the_shell_lets_go`,
/// `state::tests::real_client::reflow_on_close::hosted::a_bindings_policy_redeclared_while_the_shell_holds_the_keyboard_applies_at_once`.
#[derive(Clone, Debug)]
pub(crate) struct HostedKeyboard {
    pub(crate) surface: SurfaceId,
    pub(crate) output: Output,
    pub(crate) claims: Vec<String>,
    pub(crate) policy: KeyPolicy,
    pub(crate) returns_to: Option<WlSurface>,
}

/// A key held for the scene, and when it repeats next.
/// `input::tests::a_held_key_repeats_into_the_scene_at_the_keyboards_rate`.
#[derive(Clone, Debug)]
pub(crate) struct SceneRepeat {
    pub(crate) key: SceneKey,
    pub(crate) next: Duration,
}

/// The one grab a hosted scene holds the pointer with (Ruling 12): whose,
/// on which monitor, where that instance is drawn now, and its name, which
/// picks what a press outside it does (Ruling 13).
/// `state::tests::real_client::reflow_on_close::hosted::while_a_grab_is_held_the_pointer_is_the_scenes`,
/// `state::tests::real_client::reflow_on_close::hosted::a_grab_follows_its_scene_to_where_it_is_drawn`,
/// `state::tests::real_client::reflow_on_close::hosted::a_policy_named_for_the_grab_beats_the_default`.
#[derive(Clone, Debug)]
pub(crate) struct HostedGrab {
    pub(crate) surface: SurfaceId,
    pub(crate) output: Output,
    pub(crate) area: Rectangle<i32, Logical>,
    pub(crate) name: String,
}

/// What became of a button, with a grab held or a press swallowed.
/// `state::tests::real_client::reflow_on_close::hosted::a_press_outside_a_grab_dismisses_it_and_is_swallowed_by_default`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GrabRoute {
    /// Neither had a say in it: it goes where it always would.
    NoGrab,
    /// The grab's scene took it, or it was swallowed.
    Taken,
    /// It dismissed the grab, and goes on to what is under it.
    Passed,
}

/// What hosted surfaces reserve, by monitor, leaving out every monitor on
/// which they reserve nothing.
/// `state::tests::real_client::reflow_on_close::hosted::a_scene_reserve_overrides_its_edge_and_reflows_the_layout_once`.
pub(crate) type Reserves = BTreeMap<String, Edges>;

/// How many times one settle reads the scenes again for what its own layout
/// pass and action handlers declared, before it leaves the rest to the next
/// settle, so handlers that keep changing a reserve cannot hold a dispatch.
/// `state::tests::real_client::a_reserve_an_action_handler_changes_reflows_the_windows_in_the_clicks_dispatch`,
/// `state::tests::real_client::handlers_that_keep_changing_a_reserve_cannot_hold_the_clicks_dispatch`.
const SETTLE_ROUNDS: usize = 4;

impl Solium {
    /// Read what every hosted scene reports, reserves first, then grabs
    /// (`state::tests::real_client::reflow_on_close::hosted::a_grab_another_scene_takes_dismisses_the_one_held`),
    /// then keyboard wants
    /// (`state::tests::real_client::reflow_on_close::hosted::the_window_gets_the_keyboard_back_when_the_shell_lets_go`),
    /// and then the actions they asked for
    /// (`state::tests::real_client::reflow_on_close::hosted::two_actions_from_one_frame_both_reach_lua_in_order`).
    /// Called after anything that can run QML code in a dispatch -- the end
    /// of an input dispatch and a frame's settle among them -- once a
    /// declaration has been applied, and once the surfaces are placed on
    /// the monitors (`state::tests::real_client::a_scene_built_on_a_monitor_that_arrives_reserves_in_the_hotplugs_dispatch`),
    /// and never from inside itself (Ruling 11).
    ///
    /// When what hosted surfaces reserve is no longer what the last layout
    /// pass was laid out against, the layout runs once, here, in the
    /// dispatch that read it, so the windows glide into the new work area
    /// from this instant on the compositor's clock.
    /// `state::tests::real_client::reflow_on_close::hosted::a_scene_reserve_overrides_its_edge_and_reflows_the_layout_once`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_reserve_the_scene_changes_at_a_press_reflows_the_tiled_windows_once_from_that_instant`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_reserve_declared_again_or_taken_away_reflows_the_windows_once_each`.
    ///
    /// What the layout pass and the actions' handlers run here declare is
    /// read here too, in another round, and not at the next settle: a bar
    /// button whose handler hides the bar re-flows the windows at the
    /// release that pressed it.
    /// `state::tests::real_client::a_reserve_an_action_handler_changes_reflows_the_windows_in_the_clicks_dispatch`.
    pub(crate) fn settle_scenes(&mut self) {
        if self.settling_scenes {
            return;
        }
        self.settling_scenes = true;
        for _ in 0..SETTLE_ROUNDS {
            self.scenes_to_settle = false;
            for surface in self.surfaces.iter_mut() {
                surface.take_reserves();
            }
            if self.reserves() != self.laid_out_reserves {
                self.redraw = true;
                self.trigger_relayout();
            }
            self.settle_grabs();
            self.settle_keyboard();
            self.settle_actions();
            if !self.scenes_to_settle {
                break;
            }
        }
        self.settling_scenes = false;
    }

    /// Settle the scenes after a declaration: now, outside any dispatch, and
    /// otherwise once the outermost dispatch is applied whole. Not part way
    /// through it, where a `layout` handler that wrote a property moving the
    /// reserve would have the windows re-flowed and then put back by the
    /// places the same pass asked for after it.
    /// `state::tests::real_client::reflow_on_close::hosted::a_property_a_layout_handler_writes_reflows_the_windows_against_the_reserve_it_moved`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_reserve_declared_again_or_taken_away_reflows_the_windows_once_each`.
    /// While a settle is running, in that settle's next round instead,
    /// dispatch or none: the surfaces placed once a handler's dispatch is
    /// applied inside the settle are read there too.
    /// `state::tests::real_client::a_reserve_an_action_handler_changes_reflows_the_windows_in_the_clicks_dispatch`,
    /// `state::tests::real_client::a_scene_an_action_moves_to_the_new_primary_reserves_in_the_clicks_dispatch`.
    pub(crate) fn settle_scenes_once_dispatched(&mut self) {
        if self.dispatching == 0 && !self.settling_scenes {
            self.settle_scenes();
        } else {
            self.scenes_to_settle = true;
        }
    }

    /// Read every scene's grab (Ruling 12). One is held at a time: a grab
    /// another instance reports dismisses the one held, and the instance
    /// holding it letting go of it ends it. The one held is then placed
    /// where its scene is drawn now, and a scene that is gone, with its
    /// surface, its monitor or its placement there, lets go of it. A scene
    /// still there that can hold it no longer, its surface declared out of
    /// the pointer's reach, hears it dismissed. A grab that ends here gives
    /// the pointer back to what is under it at once.
    /// `state::tests::real_client::reflow_on_close::hosted::a_grab_another_scene_takes_dismisses_the_one_held`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_grab_suspends_a_pointer_lock_and_the_lock_comes_back_after`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_surface_taken_away_lets_go_of_its_grab`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_grab_follows_its_scene_to_where_it_is_drawn`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_surface_declared_again_out_of_the_pointers_reach_dismisses_the_grab_it_held`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_popup_that_closes_by_itself_gives_the_pointer_back_to_the_window_under_it`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_surface_taken_away_gives_the_pointer_back_to_the_window_under_it`.
    fn settle_grabs(&mut self) {
        // Behind the lock the pointer is the lock screen's: what a scene
        // says of its grabs is read once the lock is gone.
        // `state::tests::real_client::lock_focus::a_grab_a_scene_takes_behind_the_lock_is_held_once_it_is_gone`.
        if self.lock.is_some() {
            return;
        }
        let (outputs, primary) = (self.monitor_rects(), self.primary_output());
        let mut reports = Vec::new();
        for surface in self.surfaces.iter_mut() {
            let on: Vec<String> = outputs
                .iter()
                .filter(|(output, geometry)| {
                    surface
                        .area_on(output, *geometry, primary.as_ref())
                        .is_some()
                })
                .map(|(output, _)| output.name())
                .collect();
            for (monitor, report) in surface.take_grabs(&on) {
                reports.push((surface.id(), monitor, report));
            }
        }
        for (id, monitor, report) in reports {
            let ours = self
                .hosted_grab
                .as_ref()
                .is_some_and(|held| held.surface == id && held.output.name() == monitor);
            match report {
                // A grab that cannot be placed, of a surface the pointer does
                // not reach or of a scene that is not drawn there, is not
                // held, nor does it dismiss the one that is.
                // `state::tests::real_client::reflow_on_close::hosted::a_surface_the_pointer_does_not_reach_holds_no_grab`.
                GrabReport::Held(name) => {
                    if let Some(grab) = self.hosted_grab_for(id, &monitor, name) {
                        if !ours {
                            self.dismiss_hosted_grab();
                        }
                        self.hosted_grab = Some(grab);
                        if !ours {
                            self.unpoint_clients();
                        }
                    }
                }
                GrabReport::Released if ours => {
                    self.hosted_grab = None;
                    self.repoint_clients();
                }
                GrabReport::Released | GrabReport::Unchanged => {}
            }
        }
        if let Some(held) = self.hosted_grab.take() {
            let (id, output) = (held.surface, held.output.clone());
            self.hosted_grab = self.hosted_grab_for(id, &output.name(), held.name);
            if self.hosted_grab.is_none() {
                if let Some(surface) = self.surfaces.get_mut(id) {
                    surface.dismiss(&output);
                    self.redraw = true;
                }
                self.repoint_clients();
            }
        }
    }

    /// The grab named `name` of `id`'s instance on the monitor named
    /// `monitor`, where that instance is drawn now, if it is there and the
    /// pointer reaches its surface at all.
    /// `state::tests::real_client::reflow_on_close::hosted::a_surface_the_pointer_does_not_reach_holds_no_grab`.
    fn hosted_grab_for(&self, id: SurfaceId, monitor: &str, name: String) -> Option<HostedGrab> {
        let output = self
            .space
            .outputs()
            .find(|output| output.name() == monitor)?
            .clone();
        let geometry = self.space.output_geometry(&output)?;
        let primary = self.primary_output();
        let surface = self
            .surfaces
            .get(id)
            .filter(|surface| surface.interactive() && surface.hosts_on(&output))?;
        let area = surface.area_on(&output, geometry, primary.as_ref())?;
        Some(HostedGrab {
            surface: id,
            area: self.carried(id, &output, area),
            output,
            name,
        })
    }

    /// No client keeps the pointer while a grab is held, and a client's
    /// pointer lock or confinement goes with it, as smithay lets one go when
    /// its surface loses the pointer; the motion after the one that brings
    /// the pointer back grants it again, as it grants every constraint
    /// (Ruling 12). A client a press of its own still keeps the pointer on,
    /// through the grab smithay started for it, has its lock let go of here,
    /// at once, and not at the release, so it is told no motion while it
    /// thinks itself locked.
    /// `state::tests::real_client::reflow_on_close::hosted::a_grab_suspends_a_pointer_lock_and_the_lock_comes_back_after`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_grab_begun_during_a_press_on_a_locked_window_lets_go_of_the_lock_at_once`.
    fn unpoint_clients(&mut self) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let Some(focus) = pointer.current_focus() else {
            return;
        };
        if pointer.is_grabbed() {
            with_pointer_constraint(&focus, &pointer, |constraint| {
                if let Some(constraint) = constraint
                    && constraint.is_active()
                {
                    constraint.deactivate();
                }
            });
        }
        self.motion_in_place(None);
    }

    /// Give the pointer back once a grab is over, as a motion that does not
    /// move it would: the window under a still pointer has it at once, so
    /// the next press, with no motion before it, reaches that window. While
    /// the grab was held no client had the pointer, and nothing else gives
    /// it back before the pointer moves.
    /// `state::tests::real_client::reflow_on_close::hosted::a_swallowed_outside_press_gives_the_pointer_back_to_the_window_under_it`,
    /// `state::tests::real_client::reflow_on_close::hosted::with_outside_click_pass_the_dismissing_press_reaches_the_window_under_it`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_popup_that_closes_by_itself_gives_the_pointer_back_to_the_window_under_it`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_surface_taken_away_gives_the_pointer_back_to_the_window_under_it`.
    ///
    /// A grab that ends while its scene holds a press leaves the pointer
    /// with the scene until that press's release, which gives it back
    /// then.
    /// `state::tests::real_client::reflow_on_close::hosted::a_popup_closed_during_a_press_inside_it_gives_the_pointer_back_at_the_release`.
    pub(crate) fn repoint_clients(&mut self) {
        let Some(location) = self
            .seat
            .get_pointer()
            .map(|pointer| pointer.current_location())
        else {
            return;
        };
        if self.scene_press.is_some() {
            self.repoint_at_release = true;
        }
        let under = self.surface_under(location);
        self.motion_in_place(under);
    }

    /// A motion that does not move the pointer, giving it to `focus`.
    /// `state::tests::real_client::reflow_on_close::hosted::a_grab_suspends_a_pointer_lock_and_the_lock_comes_back_after`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_swallowed_outside_press_gives_the_pointer_back_to_the_window_under_it`.
    fn motion_in_place(&mut self, focus: Option<(WlSurface, Point<f64, Logical>)>) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let location = pointer.current_location();
        let time = u32::try_from(self.clock.now().as_millis()).unwrap_or(u32::MAX);
        pointer.motion(
            self,
            focus,
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        pointer.frame(self);
    }

    /// Hand every action the scenes queued to the `surface` listeners, scene
    /// by scene and each scene's in order, with the surface's name and the
    /// action's data, each in a dispatch of its own (Ruling 15).
    /// `state::tests::real_client::reflow_on_close::hosted::two_actions_from_one_frame_both_reach_lua_in_order`,
    /// `state::tests::real_client::a_click_on_a_hosted_button_is_acted_on_at_its_release`.
    pub(crate) fn settle_actions(&mut self) {
        let mut asked = Vec::new();
        for surface in self.surfaces.iter_mut() {
            for (action, data) in surface.take_actions() {
                asked.push((surface.name().to_owned(), action, data));
            }
        }
        for (name, action, data) in asked {
            let snapshot = self.snapshot();
            let Some(mut scripts) = self.scripts.take() else {
                return;
            };
            let outcome = scripts.surface_action(&name, &action, &data, snapshot);
            self.scripts = Some(scripts);
            self.apply(outcome);
        }
    }

    /// One of the compositor's verbs, as the command that does it, or why it
    /// cannot be done: an action it does not know, logged once by name, data
    /// with no window's id, or a window that is not there (Ruling 15).
    /// `state::tests::real_client::reflow_on_close::hosted::sol_act_answers_why_it_could_not`,
    /// `state::tests::real_client::reflow_on_close::hosted::windows_focus_from_a_scene_focuses_the_window`.
    pub(crate) fn act(&mut self, action: &str, data: &Json) -> Result<Command, &'static str> {
        let make: fn(u64) -> Command = match action {
            "windows.focus" => |id| Command::Focus { id },
            "windows.close" => |id| Command::Close { id },
            "windows.fullscreen" => |id| Command::ToggleFullscreen { id },
            "windows.maximize" => |id| Command::ToggleMaximize { id },
            _ => {
                if self.unknown_actions.insert(action.to_owned()) {
                    tracing::warn!(action, "sol.act: no such action");
                }
                return Err("unknown-action");
            }
        };
        let id = data.get("id").and_then(Json::as_u64).ok_or("bad-data")?;
        if self.panes.by_script_id(id).is_none() {
            return Err("unknown-window");
        }
        Ok(make(id))
    }

    /// Dismiss the hosted grab: its scene hears every active grab of its
    /// dismissed, newest first.
    /// `state::tests::real_client::reflow_on_close::hosted::locking_the_session_dismisses_a_hosted_grab`.
    pub(crate) fn dismiss_hosted_grab(&mut self) {
        if let Some(grab) = self.hosted_grab.take()
            && let Some(surface) = self.surfaces.get_mut(grab.surface)
        {
            surface.dismiss(&grab.output);
            self.redraw = true;
        }
    }

    /// Motion and the wheel while a grab is held: the grab's scene's,
    /// wherever the pointer is. True when it took them. A press a scene
    /// holds keeps them until its release, as it keeps the release (Ruling
    /// 7).
    /// `state::tests::real_client::reflow_on_close::hosted::while_a_grab_is_held_the_pointer_is_the_scenes`,
    /// `state::tests::real_client::reflow_on_close::hosted::while_a_grab_is_held_the_wheel_is_the_scenes`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_press_held_when_a_grab_begins_keeps_the_pointer_until_its_release`.
    pub(crate) fn grab_pointer(
        &mut self,
        location: Point<f64, Logical>,
        event: ScenePointer,
    ) -> bool {
        if self.scene_press.is_some() {
            return false;
        }
        let Some(grab) = self.hosted_grab.clone() else {
            return false;
        };
        if let Some(surface) = self.surfaces.get_mut(grab.surface) {
            surface.deliver(&grab.output, grab.area, location, event);
        }
        if event.kind == PointerKind::Motion {
            self.scene_hover_seen = Some((grab.surface, grab.output));
        }
        self.redraw = true;
        true
    }

    /// A button while a grab is held, or the release of a press the
    /// compositor swallowed. A press inside an active grab's target is the
    /// grab's scene's, and holds the pointer for it until the release; one
    /// outside dismisses the grab and is swallowed, with its release, or
    /// passed on to what is under it, as the policy for the grab's name
    /// says. A press a scene holds has the buttons until it is let go of.
    /// `state::tests::real_client::reflow_on_close::hosted::a_press_outside_a_grab_dismisses_it_and_is_swallowed_by_default`,
    /// `state::tests::real_client::reflow_on_close::hosted::with_outside_click_pass_the_dismissing_press_reaches_the_window_under_it`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_press_inside_the_grab_target_reaches_the_scene`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_press_held_when_a_grab_begins_keeps_the_pointer_until_its_release`.
    pub(crate) fn grab_button(
        &mut self,
        location: Point<f64, Logical>,
        code: u32,
        pressed: bool,
        event: Option<ScenePointer>,
    ) -> GrabRoute {
        if !pressed && self.swallowed.remove(&code) {
            return GrabRoute::Taken;
        }
        if !pressed || self.scene_press.is_some() {
            return GrabRoute::NoGrab;
        }
        let Some(grab) = self.hosted_grab.clone() else {
            return GrabRoute::NoGrab;
        };
        let inside = self
            .surfaces
            .get(grab.surface)
            .is_some_and(|surface| surface.grab_contains(&grab.output, grab.area, location));
        if inside {
            match event {
                Some(event) => {
                    if let Some(surface) = self.surfaces.get_mut(grab.surface) {
                        surface.deliver(&grab.output, grab.area, location, event);
                    }
                    self.scene_press = Some(ScenePress {
                        surface: grab.surface,
                        output: grab.output,
                        area: grab.area,
                    });
                }
                // A button Qt has no name for is told to no scene (Ruling 9),
                // and is nobody else's inside the grab either.
                // `state::tests::real_client::reflow_on_close::hosted::an_unnamed_button_pressed_in_a_grab_swallows_its_release`.
                None => {
                    self.swallowed.insert(code);
                }
            }
            self.redraw = true;
            return GrabRoute::Taken;
        }
        let policy = self
            .surfaces
            .get(grab.surface)
            .map(|surface| surface.declared.outside_click.for_grab(&grab.name))
            .unwrap_or_default();
        self.dismiss_hosted_grab();
        // The pointer was the grab's: it goes back to what is under it,
        // whatever becomes of the press, so a press that goes on reaches that
        // client, and so does the next one after a press swallowed.
        // `state::tests::real_client::reflow_on_close::hosted::with_outside_click_pass_the_dismissing_press_reaches_the_window_under_it`,
        // `state::tests::real_client::reflow_on_close::hosted::a_swallowed_outside_press_gives_the_pointer_back_to_the_window_under_it`.
        self.repoint_clients();
        match policy {
            Outside::Swallow => {
                self.swallowed.insert(code);
                GrabRoute::Taken
            }
            Outside::Pass => GrabRoute::Passed,
        }
    }

    /// Read every scene's keyboard wants (Ruling 14). One scene holds the
    /// keyboard at a time: an instance that comes to want it takes it from
    /// the window that had it, or from the scene that held it, which is told
    /// to let go and whose window it will be given back to; the instance
    /// holding it letting go gives the keyboard back. The one held is then
    /// let go of if its scene is gone, with its surface or its monitor, or
    /// its surface is declared out of the pointer's reach: such a surface
    /// holds no keyboard, as it holds no grab, since nothing could click it.
    /// `state::tests::real_client::reflow_on_close::hosted::the_window_gets_the_keyboard_back_when_the_shell_lets_go`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_hold_another_scene_takes_returns_to_the_window_the_first_took_it_from`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_surface_taken_away_gives_the_keyboard_back`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_monitor_unplugged_while_its_scene_holds_the_keyboard_gives_it_back`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_surface_the_pointer_does_not_reach_holds_no_keyboard`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_surface_declared_again_out_of_the_pointers_reach_gives_the_keyboard_back`.
    fn settle_keyboard(&mut self) {
        // Behind the lock the keyboard is the lock screen's: what a scene
        // says of it is read once the lock is gone, as its grabs are.
        // `state::tests::real_client::lock_focus::no_scene_holds_the_keyboard_while_the_session_is_locked`.
        if self.lock.is_some() {
            return;
        }
        let (outputs, primary) = (self.monitor_rects(), self.primary_output());
        let mut reports = Vec::new();
        for surface in self.surfaces.iter_mut() {
            let on: Vec<String> = outputs
                .iter()
                .filter(|(output, geometry)| {
                    surface
                        .area_on(output, *geometry, primary.as_ref())
                        .is_some()
                })
                .map(|(output, _)| output.name())
                .collect();
            let (policy, interactive) = (surface.declared.keyboard, surface.interactive());
            for (monitor, report) in surface.take_keyboards(&on) {
                reports.push((surface.id(), policy, interactive, monitor, report));
            }
        }
        for (id, policy, interactive, monitor, report) in reports {
            let ours = self
                .hosted_keyboard
                .as_ref()
                .is_some_and(|held| held.surface == id && held.output.name() == monitor);
            match report {
                KeyboardReport::Wanted(claims) if ours => {
                    if let Some(held) = self.hosted_keyboard.as_mut() {
                        held.claims = claims;
                    }
                }
                // A surface the pointer does not reach takes no keyboard.
                // `state::tests::real_client::reflow_on_close::hosted::a_surface_the_pointer_does_not_reach_holds_no_keyboard`.
                KeyboardReport::Wanted(_) if !interactive => {}
                KeyboardReport::Wanted(claims) => {
                    let Some(output) = self
                        .space
                        .outputs()
                        .find(|output| output.name() == monitor)
                        .cloned()
                    else {
                        continue;
                    };
                    // The surface to give the keyboard back to: the one that
                    // had it, or, from another scene's hold, the one that
                    // scene was going to give it back to.
                    let returns_to = match self.hosted_keyboard.as_ref() {
                        Some(held) => held.returns_to.clone(),
                        None => self
                            .seat
                            .get_keyboard()
                            .and_then(|keyboard| keyboard.current_focus()),
                    };
                    self.end_keyboard_hold(false);
                    self.give_keyboard(None, SERIAL_COUNTER.next_serial());
                    self.hosted_keyboard = Some(HostedKeyboard {
                        surface: id,
                        output,
                        claims,
                        policy,
                        returns_to,
                    });
                    self.redraw = true;
                }
                KeyboardReport::LetGo if ours => self.end_keyboard_hold(true),
                KeyboardReport::LetGo | KeyboardReport::Unchanged => {}
            }
        }
        let gone = self.hosted_keyboard.as_ref().is_some_and(|held| {
            !self.space.outputs().any(|output| *output == held.output)
                || self
                    .surfaces
                    .get(held.surface)
                    .is_none_or(|surface| !surface.interactive() || !surface.hosts_on(&held.output))
        });
        if gone {
            self.end_keyboard_hold(true);
        }
    }

    /// One key for the scene holding the keyboard, and the repeat of a key
    /// held for it.
    /// `input::tests::while_the_shell_holds_the_keyboard_russian_letters_reach_it_as_cyrillic`,
    /// `input::tests::a_held_key_repeats_into_the_scene_at_the_keyboards_rate`.
    pub(crate) fn deliver_scene_key(&mut self, key: SceneKey) {
        #[cfg(test)]
        self.scene_keys.push(key.clone());
        if let Some(held) = self.hosted_keyboard.clone()
            && let Some(surface) = self.surfaces.get_mut(held.surface)
        {
            surface.key(&held.output, &key);
        }
        // A key the keymap says does not repeat, a modifier or a group
        // toggle, does not.
        // `input::tests::a_held_modifier_does_not_repeat_into_the_scene`,
        // `input::tests::a_held_group_toggle_does_not_repeat_into_the_scene`.
        if key.pressed && key.repeats && self.hosted_keyboard.is_some() {
            let delay =
                Duration::from_millis(u64::try_from(self.keyboard.repeat_delay).unwrap_or(600));
            self.scene_repeat = Some(SceneRepeat {
                next: self.clock.now() + delay,
                key: SceneKey {
                    autorepeat: true,
                    ..key
                },
            });
        } else if !key.pressed
            && self
                .scene_repeat
                .as_ref()
                .is_some_and(|repeat| repeat.key.code == key.code)
        {
            self.scene_repeat = None;
        }
        self.redraw = true;
    }

    /// A key held for the scene holding the keyboard repeats at the
    /// keyboard's rate after its delay, checked once per loop iteration, as
    /// idleness is (Ruling 14). Each repeat is due an interval after the last
    /// was due, so a loop that looks late does not slow the rate.
    /// `input::tests::a_held_key_repeats_into_the_scene_at_the_keyboards_rate`,
    /// `input::tests::a_held_key_noticed_late_still_repeats_at_the_keyboards_rate`.
    pub(crate) fn repeat_scene_key(&mut self, now: Duration) {
        let Some(repeat) = self.scene_repeat.as_mut() else {
            return;
        };
        let rate = u64::try_from(self.keyboard.repeat_rate).unwrap_or(0);
        if self.hosted_keyboard.is_none() || rate == 0 {
            self.scene_repeat = None;
            return;
        }
        if now < repeat.next {
            return;
        }
        let interval = Duration::from_millis(1000 / rate);
        repeat.next += interval;
        if repeat.next <= now {
            repeat.next = now + interval;
        }
        let key = repeat.key.clone();
        #[cfg(test)]
        self.scene_keys.push(key.clone());
        if let Some(held) = self.hosted_keyboard.clone()
            && let Some(surface) = self.surfaces.get_mut(held.surface)
        {
            surface.key(&held.output, &key);
        }
        self.redraw = true;
    }

    /// End a scene's hold on the keyboard, and tell the scene. With
    /// `give_back`, the surface it came from gets the keyboard again, through
    /// the gate, or, when that has gone, wherever the keyboard goes when a
    /// window goes; otherwise the caller is about to give it to someone.
    /// `state::tests::real_client::reflow_on_close::hosted::the_window_gets_the_keyboard_back_when_the_shell_lets_go`,
    /// `state::tests::real_client::reflow_on_close::hosted::clicking_a_window_ends_the_shells_hold`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_window_closed_while_the_shell_holds_the_keyboard_is_not_given_it_back`.
    pub(crate) fn end_keyboard_hold(&mut self, give_back: bool) {
        let Some(held) = self.hosted_keyboard.take() else {
            return;
        };
        self.scene_repeat = None;
        if let Some(surface) = self.surfaces.get_mut(held.surface) {
            surface.let_go_keyboard(&held.output);
        }
        self.redraw = true;
        if !give_back {
            return;
        }
        match held
            .returns_to
            .filter(|surface| surface.alive() && self.may_hold_keyboard(surface))
        {
            Some(surface) => {
                self.give_keyboard(Some(surface.clone()), SERIAL_COUNTER.next_serial());
                crate::xwayland::activate(self, Some(&surface));
            }
            None => self.settle_focus(),
        }
    }

    /// What hosted surfaces reserve on one monitor, every edge summed.
    /// `state::tests::real_client::reflow_on_close::hosted::a_declared_reserve_takes_its_edge_out_of_the_work_area`,
    /// `state::tests::real_client::reflow_on_close::hosted::two_surfaces_reserving_one_edge_take_both`.
    pub(crate) fn reserved_on(&self, output: &Output) -> Edges {
        let Some(geometry) = self.space.output_geometry(output) else {
            return Edges::default();
        };
        let primary = self.primary_output();
        self.surfaces
            .iter()
            .filter(|surface| {
                surface
                    .area_on(output, geometry, primary.as_ref())
                    .is_some()
            })
            .fold(Edges::default(), |all, surface| {
                all.add(surface.reserve_on(output))
            })
    }

    /// What hosted surfaces reserve on every monitor now.
    /// `state::tests::real_client::reflow_on_close::hosted::a_scene_reserve_overrides_its_edge_and_reflows_the_layout_once`.
    pub(crate) fn reserves(&self) -> Reserves {
        self.space
            .outputs()
            .map(|output| (output.name(), self.reserved_on(output)))
            .filter(|(_, reserved)| *reserved != Edges::default())
            .collect()
    }
}
