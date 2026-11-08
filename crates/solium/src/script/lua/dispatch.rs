//! The events a configuration is told, each a dispatch that answers with
//! an [`Outcome`].

use mlua::{Lua, Table, Value};

use crate::script::lua::*;
use crate::script::*;

impl Scripts {
    /// Whether any script has bound this key combination.
    ///
    /// Deliberately side-effect free, and separate from dispatching it: the
    /// keyboard filter runs while the seat holds its own lock, so the decision
    /// to intercept is made there and the handler runs afterwards. Calling a
    /// script from inside the filter would let it call back into the seat and
    /// deadlock.
    pub(crate) fn has_binding(&self, combo: &str) -> bool {
        let Ok(sol) = self.lua.globals().get::<Table>("sol") else {
            return false;
        };
        let Ok(bindings) = sol.get::<Table>("_bindings") else {
            return false;
        };
        bindings
            .get::<Value>(normalise_combo(combo))
            .is_ok_and(|value| matches!(value, Value::Function(_)))
    }

    /// Run the handler bound to a key combination.
    pub(crate) fn key(&mut self, combo: &str, snapshot: Snapshot) -> Outcome {
        self.dispatch_as(FocusSource::Binding, snapshot, |lua, sol| {
            let bindings: Table = sol.get("_bindings")?;
            let handler: Value = bindings.get(normalise_combo(combo))?;
            match handler {
                Value::Function(function) => match function.call::<()>(()) {
                    Ok(()) => Ok(true),
                    // Logged where it was written and where it was stopped,
                    // as a stopped listener is, and unhandled, as a binding
                    // that fails is.
                    // `tests::a_stopped_binding_is_logged_where_it_was_written_and_stopped`.
                    Err(err) if err.to_string().contains(STOPPED) => {
                        let info = function.info();
                        let file = file_of(info.source.as_deref(), info.short_src.as_deref());
                        let line = info.line_defined.unwrap_or_default();
                        let stopped_at = stopped_at(lua);
                        tracing::error!(
                            %err,
                            event = "binding",
                            combo,
                            file = %file,
                            line,
                            stopped_at = stopped_at.as_deref().map(tracing::field::display),
                            "a handler ran for longer than 100 ms and was stopped"
                        );
                        Ok(false)
                    }
                    Err(err) => Err(err),
                },
                _ => Ok(false),
            }
        })
    }

    /// Tell scripts a window has appeared.
    ///
    /// Every listener runs. Returns whether any of them did: nothing happening
    /// is a valid answer, and the compositor then uses its own plain animation
    /// rather than leaving a window to pop into existence.
    pub(crate) fn opened(&mut self, id: u64, snapshot: Snapshot) -> Outcome {
        self.dispatch_as(FocusSource::Open, snapshot, move |lua, _| {
            call_listeners(lua, "open", id)
        })
    }

    /// A window went fullscreen, or left it: `(id, entering)`.
    ///
    /// Told after the change is made: the client has been sent its new size,
    /// and the window lives at its new rectangle. What is left is moving the
    /// picture there, which the compositor does once every listener has run,
    /// from where the window is drawn, with the timing the listeners set with
    /// `sol.animate` -- [`Outcome::motion`] -- and at once when none set one.
    /// `tests::a_change_is_answered_with_the_motion_sol_animate_set`.
    pub(crate) fn fullscreen(&mut self, id: u64, entering: bool, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| {
            call_listeners(lua, "fullscreen", (id, entering))
        })
    }

    /// A window was maximised, or restored: `(id, entering)`. Answered the
    /// way [`Self::fullscreen`] is, and told on its own, so the two can be
    /// given different motions.
    /// `tests::a_change_is_answered_with_the_motion_sol_animate_set`.
    pub(crate) fn maximize(&mut self, id: u64, entering: bool, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| {
            call_listeners(lua, "maximize", (id, entering))
        })
    }

    /// Run the handler for a pointer press, while a mode owns input.
    pub(crate) fn click(&mut self, x: f64, y: f64, snapshot: Snapshot) -> Outcome {
        self.dispatch_as(FocusSource::Click, snapshot, move |lua, _| {
            call_listeners(lua, "click", (x, y))
        })
    }

    /// Down runs before delivery so Lua can capture; always outside seat locks.
    pub(crate) fn touch(&mut self, event: TouchEvent, snapshot: Snapshot) -> Outcome {
        self.dispatch_as(FocusSource::Touch, snapshot, move |lua, _| {
            let table = lua.create_table()?;
            table.set("kind", event.kind)?;
            table.set("slot", event.slot)?;
            table.set("x", event.x)?;
            table.set("y", event.y)?;
            table.set("output", event.output)?;
            table.set("target", event.target)?;
            table.set("window", event.window)?;
            table.set("control", event.control)?;
            table.set("edges", event.edges)?;
            table.set("time", event.time)?;
            table.set("t", event.t)?;
            table.set("vx", event.velocity.0)?;
            table.set("vy", event.velocity.1)?;
            call_listeners(lua, "touch", table)
        })
    }

    /// Whatever the configuration asked for while it was being read.
    ///
    /// A script's top level runs once, at load, and anything it requests there
    /// — the dock's contents, a starting mode — lands in the same pending
    /// buffer an event handler would use. Without draining it that work is
    /// simply dropped, silently, which is what happened to the first dock: no
    /// error, no items, nothing drawn.
    pub(crate) fn startup(&mut self) -> Outcome {
        // Takes the buffer the configuration filled, rather than dispatching:
        // `dispatch` installs a fresh `Pending` before it calls anything, so
        // routing this through it would throw away the very requests it is
        // here to collect.
        let pending = self.lua.remove_app_data::<Pending>().unwrap_or_default();
        self.lua.set_app_data(Pending::default());
        Outcome {
            handled: false,
            commands: applied(pending.commands),
            grab: pending.grab,
            status: pending.status,
            motion: pending.animated.then_some(pending.animation),
        }
    }

    /// Focus moved to a window: `(id, source, changed)`, `source` the word
    /// for what moved it, and `changed` whether the keyboard came from
    /// another window or none, `false` for the window that had it focused
    /// again (#220). `tests::the_focus_event_says_what_moved_the_keyboard`,
    /// `state::tests::real_client::follow::the_focus_event_says_whether_the_keyboard_moved`.
    pub(crate) fn focused(
        &mut self,
        id: u64,
        source: FocusSource,
        changed: bool,
        snapshot: Snapshot,
    ) -> Outcome {
        self.dispatch(snapshot, move |lua, _| {
            call_listeners(lua, "focus", (id, source.name(), changed))
        })
    }

    /// The keyboard's layout, Caps Lock or Num Lock changed: `(state,
    /// changed)`, `state` being what `sol.keyboard()` answers and `changed`
    /// `"layout"`, `"caps"` or `"num"`.
    /// `keyboard_change::tests::a_layout_switch_and_a_caps_toggle_by_key_are_told_once_each_with_russian_active`.
    pub(crate) fn keyboard_changed(
        &mut self,
        changed: &'static str,
        snapshot: Snapshot,
    ) -> Outcome {
        self.dispatch(snapshot, move |lua, sol| {
            let state: Value = sol.get::<mlua::Function>("keyboard")?.call(())?;
            call_listeners(lua, "keyboard", (state, changed))
        })
    }

    /// The session's lock changed: `(state)`, `"locking"` while Solium's own
    /// lock comes down over the desktop, `"locked"`, `"unlocking"` while it
    /// goes, and `"unlocked"`. Nothing else of the lock: not who holds it,
    /// nothing of what is typed at it.
    /// `lock_auth::tests::the_scripts_hear_the_lock_come_and_go`.
    pub(crate) fn lock_changed(&mut self, state: &'static str, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| call_listeners(lua, "lock", state))
    }

    /// The focused text field changed: `(field, why)`, `field` being what
    /// `sol.text_input()` answers and `why` `"field"`, `"caret"` or
    /// `"framed"`, as `text_input::Why` names it.
    /// `text_input::tests::text_input_is_told_when_a_field_is_enabled_and_when_it_is_focused`,
    /// `text_input::tests::text_input_is_told_when_the_caret_moves_once_a_pass`.
    pub(crate) fn text_input(&mut self, why: &'static str, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, sol| {
            let field: Value = sol.get::<mlua::Function>("text_input")?.call(())?;
            call_listeners(lua, "text_input", (field, why))
        })
    }

    /// A window asked to be brought forward, and the compositor has answered
    /// as it does: focused it, or refused it on a workspace nobody is looking
    /// at or while it is being closed. `(id, why)`.
    ///
    /// `why` is `"launch"` for an application that was already running and
    /// answered a launch with a window it had, the launch's own window having
    /// dissolved (#177), and `"request"` for anything else: a token nothing
    /// was launched with, which any client can make for itself. Never for a
    /// launch's window arriving, which is `open` and `focus`.
    /// `relaunch::relaunching_an_application_tells_the_scripts_no_close_for_its_window`,
    /// `relaunch::a_window_opened_for_a_relaunch_still_arrives_in_the_window_opened_for_the_launch`
    /// and `relaunch::a_genuine_activation_is_told_as_a_request_and_the_view_stays_where_it_is`,
    /// in `state/tests.rs`.
    pub(crate) fn activated(&mut self, id: u64, why: &'static str, snapshot: Snapshot) -> Outcome {
        self.dispatch_as(FocusSource::Activate, snapshot, move |lua, _| {
            call_listeners(lua, "activate", (id, why))
        })
    }

    /// An edge was dragged.
    ///
    /// Offered to layouts before the compositor resizes anything, so a tiled
    /// window can move its seam instead of growing over its neighbour.
    ///
    /// `(id, edge_x, edge_y, horizontal_side, vertical_side)`.
    ///
    /// This signature line has been wrong twice, so it is written out rather
    /// than summarised. It said `(id, x, y, horizontal, vertical)` until now —
    /// the names of the *pre-#120* booleans, which that issue replaced with
    /// sides in the code and forgot to replace here — and the prose under it
    /// called the first pair "the delta" for the whole life of the event and
    /// then "where the pointer is", neither of which it is any more.
    ///
    /// `edge_x, edge_y` is **where the dragged edge should come to rest** on
    /// each axis, in the coordinates `sol.place` and `tree:layout` already
    /// speak. A position and not a delta, so handling the same drag twice gives
    /// the same layout; derived from the drag's own rectangle rather than from
    /// the seat, so the edge moves *with* the pointer instead of jumping to it.
    /// That is #124, and the difference is most of a window's width on a
    /// `super`+right-button drag, which begins in the middle of one. See
    /// [`crate::input::resize::dragged_edge`].
    ///
    /// On an axis this drag does not move there is no such edge, and the
    /// pointer's own coordinate is passed through there instead. A handler that
    /// checks its side before using the coordinate — which is what the side is
    /// for — never sees it.
    ///
    /// `horizontal_side, vertical_side` are the *side* of the window being
    /// dragged on each axis: `"left"`/`"right"`, `"top"`/`"bottom"`, or nil
    /// where that axis is not in play. They were a pair of booleans until #120;
    /// a script that only tested them for truthiness still reads the same,
    /// because a side is truthy and nil is not, but one that passes them on now
    /// passes on something a layout can choose a seam from. See
    /// [`crate::input::resize::sides`].
    pub(crate) fn resized(
        &mut self,
        id: u64,
        edge_at: (f64, f64),
        sides: (Option<&'static str>, Option<&'static str>),
        snapshot: Snapshot,
    ) -> Outcome {
        self.dispatch(snapshot, move |lua, _| {
            call_listeners(lua, "resize", (id, edge_at.0, edge_at.1, sides.0, sides.1))
        })
    }

    /// A window went away.
    ///
    /// Needed by any layout that keeps state of its own: a tree cannot drop a
    /// node it is never told about, and diffing the window list on every pass
    /// — which is what the layouts did instead — cannot tell "closed" from
    /// "moved to another workspace".
    pub(crate) fn closed(&mut self, id: u64, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| call_listeners(lua, "close", id))
    }

    /// The compositor has started closing a window: its leaving animation is
    /// playing, and the client has not been asked yet.
    ///
    /// Not `close`, and not a replacement for it. The client may still refuse
    /// -- an unsaved-changes prompt -- and a script that treated this as the
    /// window being gone would forget a window that is about to come back.
    /// `close` still fires when it really goes; `refused` fires instead when it
    /// does not. This is the moment a layout may reflow if it wants to, which
    /// is why it exists: the compositor answers a close at once, the way it
    /// answers an open or a resize, and the client catches up inside that
    /// answer (#128).
    pub(crate) fn closing(&mut self, id: u64, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| call_listeners(lua, "closing", id))
    }

    /// A close has landed: its animation has played, the window has faded
    /// out of sight, and its client has just been asked to go (an X11 one
    /// that let the last request go unanswered, ended). Between `closing` and
    /// `close` or `refused`, once per close the compositor asked for; never
    /// for a window that went, or answered with a dialog, before its close
    /// landed, nor for one whose client never came.
    ///
    /// The moment a layout set to wait for the animation reflows
    /// (`reflow_on_close = "when_landed"`, #128): later than `closing`, so the
    /// other windows stay put while the window plays out, and earlier than
    /// `close`, so they do not wait on the application.
    /// `with_reflow_on_close_when_landed_the_grid_waits_for_the_close_to_land`
    /// in `state/tests.rs`.
    pub(crate) fn close_landed(&mut self, id: u64, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| {
            call_listeners(lua, "close_landed", id)
        })
    }

    /// A window that was closing is staying: its client declined, and the
    /// window is back on screen.
    ///
    /// Once per refusal, on the frame the window is given back, and never
    /// after `close` for the same window. A layout that took the window out
    /// on `closing` puts it back here. Not `open`: the window never went, and
    /// an arrival would run `open.lua`'s animation on it a second time.
    pub(crate) fn refused(&mut self, id: u64, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| call_listeners(lua, "refused", id))
    }

    /// A window has just been minimized (#30): by `sol.minimize`, or by its
    /// own client asking. Fired once, before anything else reacts, with the
    /// window already `minimized` in the snapshot, so a configuration starts
    /// its present -- the shipped one plays the genie into its slot -- in the
    /// same dispatch, and a layout takes the window out of its arrangement.
    /// The window stays drawn while that present plays out.
    /// A notification arrived (#346): the `notification` hooks decide what
    /// happens to it, the first that returns a table deciding (`banner`,
    /// `keep`), each under the handler deadline. `None` when none answered,
    /// so the shipped rule applies.
    /// `tests::the_first_notification_hook_that_answers_decides`.
    pub(crate) fn notification_decision(
        &mut self,
        arrival: &crate::json::Json,
        snapshot: Snapshot,
    ) -> (Option<crate::json::Json>, Outcome) {
        let data = arrival.to_lua(&self.lua).unwrap_or(Value::Nil);
        let decided = std::rc::Rc::new(std::cell::RefCell::new(None));
        let slot = decided.clone();
        let outcome = self.dispatch(snapshot, move |lua, _| {
            *slot.borrow_mut() = call_deciders(lua, "notification", data.clone())?;
            Ok(true)
        });
        let decided = decided.borrow_mut().take();
        (decided, outcome)
    }

    /// Something happened to a notification (#346): `added`, `replaced`,
    /// `closed`, `invoked`, `dnd` or `owner`, for `sol.on("notifications")`.
    /// `tests::notification_events_reach_their_listeners_with_their_origin`.
    pub(crate) fn notifications_event(
        &mut self,
        event: &crate::json::Json,
        snapshot: Snapshot,
    ) -> Outcome {
        let data = event.to_lua(&self.lua).unwrap_or(Value::Nil);
        self.dispatch(snapshot, move |lua, _| {
            call_listeners(lua, "notifications", data.clone())
        })
    }

    /// Something changed in audio (#347): `volume`, `mute`, `default`,
    /// `added` or `removed`, with who did it, for `sol.on("audio")`.
    /// `tests::audio_events_reach_their_listeners_with_their_origin`.
    pub(crate) fn audio_event(&mut self, event: &crate::json::Json, snapshot: Snapshot) -> Outcome {
        let data = event.to_lua(&self.lua).unwrap_or(Value::Nil);
        self.dispatch(snapshot, move |lua, _| {
            call_listeners(lua, "audio", data.clone())
        })
    }

    /// Something changed in the tray (#363): an item `added`, `changed` or
    /// `removed`, its `menu` opened, read or closed, or the `owner` of the
    /// watcher's name, for `sol.on("tray")`.
    /// `tests::tray_events_reach_their_listeners_with_their_origin`.
    pub(crate) fn tray_event(&mut self, event: &crate::json::Json, snapshot: Snapshot) -> Outcome {
        let data = event.to_lua(&self.lua).unwrap_or(Value::Nil);
        self.dispatch(snapshot, move |lua, _| {
            call_listeners(lua, "tray", data.clone())
        })
    }

    /// Something changed in media (#379): a player `added`, `changed` or
    /// `removed`, or another player `active`, for `sol.on("media")`.
    /// `tests::media_events_reach_their_listeners_with_the_player`.
    pub(crate) fn media_event(&mut self, event: &crate::json::Json, snapshot: Snapshot) -> Outcome {
        let data = event.to_lua(&self.lua).unwrap_or(Value::Nil);
        self.dispatch(snapshot, move |lua, _| {
            call_listeners(lua, "media", data.clone())
        })
    }

    /// `sol.notify`'s notifications that closed without an action: their
    /// functions are let go, so a shell that posts many does not keep them
    /// all. `tests::sol_notify_queues_one_of_the_shells_own_with_its_actions`.
    pub(crate) fn notify_forget(&mut self, tokens: &[u64]) {
        let forgotten = self
            .lua
            .globals()
            .get::<Table>("sol")
            .and_then(|sol| sol.get::<Table>("_notify_actions"))
            .and_then(|kept| {
                for token in tokens {
                    kept.set(*token, Value::Nil)?;
                }
                Ok(())
            });
        if let Err(err) = forgotten {
            tracing::debug!(%err, "sol.notify's functions were not let go");
        }
    }

    /// An action of one of `sol.notify`'s own notifications was invoked: its
    /// function runs, and the notification's functions are let go.
    /// `tests::sol_notify_queues_one_of_the_shells_own_with_its_actions`.
    pub(crate) fn notify_action(&mut self, token: u64, key: &str, snapshot: Snapshot) -> Outcome {
        let key = key.to_owned();
        self.dispatch(snapshot, move |lua, sol| {
            let kept: Table = sol.get("_notify_actions")?;
            let Some(functions) = kept.get::<Option<Table>>(token)? else {
                return Ok(false);
            };
            kept.set(token, Value::Nil)?;
            let Some(run) = functions.get::<Option<mlua::Function>>(key.as_str())? else {
                return Ok(false);
            };
            start_clock(lua);
            match run.call::<()>(()) {
                Ok(()) => Ok(true),
                Err(err) => {
                    tracing::error!(%err, key, "a sol.notify action failed");
                    Ok(false)
                }
            }
        })
    }

    pub(crate) fn minimized(&mut self, id: u64, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| call_listeners(lua, "minimize", id))
    }

    /// A minimized window is shown again (#30): by `sol.unminimize`, by its
    /// client, or by being focused. A layout takes it back where it was, and
    /// a configuration presents it back from wherever it put it.
    pub(crate) fn unminimized(&mut self, id: u64, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| {
            call_listeners(lua, "unminimize", id)
        })
    }

    /// Something the compositor owns changed the space windows get.
    ///
    /// A decoration reserving a different amount is the case that needs it:
    /// the slots are the same but what fits in them is not, and a layout that
    /// is never told goes on believing its own last arithmetic.
    /// The set of monitors changed: one arrived, or one went away.
    ///
    /// Separate from `layout` because it is a different question. `layout`
    /// asks a mode to arrange the windows it already knows about; this says
    /// the screens themselves are not the screens they were, so a mode holding
    /// per-monitor state -- which both shipped layouts do, one tree or one
    /// view per screen -- has to re-home the windows whose monitor is gone
    /// before arranging anything.
    ///
    /// Without it a window on an unplugged monitor is in a tree that nothing
    /// iterates, because `tiling.apply` walks the monitors that *exist*. It
    /// keeps its old slot, which is now off every screen, and it reappears
    /// there when the monitor comes back -- which is exactly what a hardware
    /// test found.
    pub(crate) fn monitors_changed(&mut self, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| call_listeners(lua, "monitors", ()))
    }

    /// The problems changed: an effect broke or was mended, or a reload failed.
    /// `tests::the_problems_event_reaches_its_listeners`.
    pub(crate) fn problems_changed(&mut self, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| call_listeners(lua, "problems", ()))
    }

    /// These scripts have replaced a running session's, rather than started one.
    ///
    /// The first half of the reload contract on [`Self::load_carrying`], and
    /// the half a keep cannot cover. `sol.keep` gives a script its *data* back;
    /// this is the moment it may act on it — after every script has loaded, so
    /// a mode restored here can be sure the layout it names has registered
    /// itself, which it cannot be at its own top level.
    ///
    /// It does not fire at startup, and that asymmetry is the whole meaning of
    /// the event: a cold start has nothing to restore, and a script that
    /// listens for this is saying "this is what I do differently when I am not
    /// the first configuration this session has had".
    pub(crate) fn restored(&mut self, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| call_listeners(lua, "restore", ()))
    }

    /// A hosted scene sent an action, with data: the `surface` listeners
    /// hear the surface's name, the action and the data, a table, a value or
    /// `nil` (Ruling 15).
    /// `tests::a_surface_action_reaches_lua_with_its_data`,
    /// `tests::an_action_with_no_data_reaches_lua_as_nil`.
    /// A window frame's layer sent something that is not a built-in action
    /// (#417): `sol.on("frame", function(id, name, data) ... end)`, with the
    /// window's id, the name it sent and its data, as a scene's `surface`
    /// listeners hear theirs. `tests::frame_listeners_hear_the_window_the_name_and_the_data`.
    pub(crate) fn frame_action(
        &mut self,
        id: u64,
        name: &str,
        data: &crate::json::Json,
        snapshot: Snapshot,
    ) -> Outcome {
        let data = data.to_lua(&self.lua).unwrap_or_else(|err| {
            tracing::warn!(%err, name, "a frame's data did not reach Lua");
            Value::Nil
        });
        let name = name.to_owned();
        self.dispatch_as(FocusSource::Surface, snapshot, move |lua, _| {
            call_listeners(lua, "frame", (id, name.clone(), data.clone()))
        })
    }

    pub(crate) fn surface_action(
        &mut self,
        name: &str,
        action: &str,
        data: &crate::json::Json,
        snapshot: Snapshot,
    ) -> Outcome {
        let data = data.to_lua(&self.lua).unwrap_or_else(|err| {
            tracing::warn!(%err, action, "an action's data did not reach Lua");
            Value::Nil
        });
        let (name, action) = (name.to_owned(), action.to_owned());
        self.dispatch_as(FocusSource::Surface, snapshot, move |lua, _| {
            call_listeners(lua, "surface", (name.clone(), action.clone(), data.clone()))
        })
    }

    /// Attempts whose outcomes are known: each one's `done` is called once,
    /// and forgotten, in a dispatch of its own, each under the whole handler
    /// deadline.
    /// `tests::sol_act_returns_an_attempt_and_done_hears_the_outcome_once`,
    /// `tests::a_done_that_never_returns_is_stopped_and_the_next_done_still_hears_its_outcome`.
    ///
    /// A `done` stopped at the deadline is struck as a listener is, by
    /// function, and one stopped three times is not called again until the
    /// next reload. What it asked for in the run that was stopped is
    /// dropped, the attempts it started forgotten with it, so a retry that
    /// acts again and then never returns is not told again, round after
    /// round.
    /// `tests::a_done_stopped_three_times_is_not_called_again`,
    /// `tests::what_a_stopped_done_asked_for_is_dropped`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_done_that_acts_again_and_never_returns_does_not_stall_every_dispatch`.
    pub(crate) fn attempts_settled(&mut self, settled: &[Settled], snapshot: Snapshot) -> Outcome {
        let settled = settled.to_vec();
        self.dispatch(snapshot, move |lua, sol| {
            let attempts: Table = sol.get("_attempts")?;
            let strikes: Table = sol.get("_strikes")?;
            for each in &settled {
                let done: Option<mlua::Function> = attempts.get(each.attempt)?;
                attempts.set(each.attempt, Value::Nil)?;
                let Some(done) = done else { continue };
                if strikes.get::<Option<u32>>(&done)?.unwrap_or(0) >= STRIKES {
                    continue;
                }
                let asked = lua
                    .try_app_data_ref::<Pending>()
                    .ok()
                    .flatten()
                    .map_or(0, |pending| pending.commands.len());
                start_clock(lua);
                match done.call::<()>((each.ok, each.reason)) {
                    Ok(()) => {}
                    Err(err) if err.to_string().contains(STOPPED) => {
                        let dropped = lua
                            .try_app_data_mut::<Pending>()
                            .ok()
                            .flatten()
                            .map(|mut pending| {
                                let at = asked.min(pending.commands.len());
                                pending.commands.split_off(at)
                            })
                            .unwrap_or_default();
                        for command in dropped {
                            if let solium_wm_protocol::Command::Act((attempt, ..)) = command {
                                attempts.set(attempt, Value::Nil)?;
                            }
                        }
                        let _ = strike(lua, &strikes, &done, "done", &err)?;
                    }
                    Err(err) => tracing::error!(%err, "an attempt's done failed"),
                }
            }
            Ok(true)
        })
    }

    /// What `sol.run{}`'s children said and how they ended (#472), in the
    /// order it was heard, in one dispatch: each line to its run's `lines`
    /// as `(line, stream)`, and each end to its `done`, once, with the
    /// result, after which the run's functions are let go. Each call has the
    /// whole handler deadline, and a function stopped there three times is
    /// not called again until the next reload.
    /// `tests::sol_run_with_a_table_hands_its_output_to_done`,
    /// `tests::sol_run_tells_lines_then_done`.
    pub(crate) fn runs_heard(
        &mut self,
        heard: &[crate::runs::Heard],
        snapshot: Snapshot,
    ) -> Outcome {
        use crate::runs::Heard;
        self.dispatch(snapshot, move |lua, sol| {
            let runs: Table = sol.get("_runs")?;
            let strikes: Table = sol.get("_strikes")?;
            for each in heard {
                match each {
                    Heard::Line { id, stream, line } => {
                        let Some(kept) = runs.get::<Option<Table>>(*id)? else {
                            continue;
                        };
                        let Some(lines) = kept.get::<Option<mlua::Function>>("lines")? else {
                            continue;
                        };
                        let line = lua.create_string(line)?;
                        call_struck(lua, &strikes, &lines, (line, stream.name()), "lines")?;
                    }
                    Heard::Done { id, ended } => {
                        let Some(kept) = runs.get::<Option<Table>>(*id)? else {
                            continue;
                        };
                        runs.set(*id, Value::Nil)?;
                        let Some(done) = kept.get::<Option<mlua::Function>>("done")? else {
                            continue;
                        };
                        let result = lua.create_table()?;
                        result.set("code", ended.code)?;
                        result.set("signal", ended.signal)?;
                        result.set("timed_out", ended.timed_out)?;
                        result.set("truncated", ended.truncated)?;
                        result.set("stdout", lua.create_string(&ended.stdout)?)?;
                        result.set("stderr", lua.create_string(&ended.stderr)?)?;
                        result.set("error", ended.error.as_deref())?;
                        call_struck(lua, &strikes, &done, result, "done")?;
                    }
                }
            }
            Ok(true)
        })
    }

    /// Timers due (#472), in the order they fell due, in one dispatch: each
    /// one's function, under the whole handler deadline. A `sol.after`'s is
    /// let go before it runs. A timer with no function left, and one whose
    /// function was stopped three times, is cancelled.
    /// `tests::timers_run_in_the_order_they_fall_due`,
    /// `tests::a_timer_stopped_three_times_is_cancelled`.
    pub(crate) fn timers_due(&mut self, due: &[(u64, bool)], snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, sol| {
            let timers: Table = sol.get("_timers")?;
            let strikes: Table = sol.get("_strikes")?;
            let mut gone = Vec::new();
            for &(id, last) in due {
                let Some(run) = timers.get::<Option<mlua::Function>>(id)? else {
                    gone.push(id);
                    continue;
                };
                if last {
                    timers.set(id, Value::Nil)?;
                }
                if call_struck(lua, &strikes, &run, (), "timer")? && !last {
                    timers.set(id, Value::Nil)?;
                    gone.push(id);
                }
            }
            if !gone.is_empty() {
                with_pending(lua, |pending| {
                    pending.commands.extend(
                        gone.into_iter()
                            .map(solium_wm_protocol::Command::TimerCancel),
                    );
                })?;
            }
            Ok(true)
        })
    }

    /// What the configuration's reads, writes and watches came to (#473),
    /// in the order heard, in one dispatch: each read's function with
    /// `(data, nil)` or `(nil, err)`, each write's with `err` or `nil`, after
    /// which they are let go, and each watch's with its event. Each call has
    /// the whole handler deadline; a watch whose function was stopped three
    /// times, and one with no function left, is cancelled.
    /// `tests::sol_read_hands_the_file_to_its_function`,
    /// `tests::a_cancelled_watch_is_not_told`.
    pub(crate) fn files_heard(
        &mut self,
        heard: &[crate::files::Heard],
        snapshot: Snapshot,
    ) -> Outcome {
        use crate::files::Heard;
        self.dispatch(snapshot, move |lua, sol| {
            let files: Table = sol.get("_files")?;
            let watches: Table = sol.get("_watches")?;
            let strikes: Table = sol.get("_strikes")?;
            let mut gone = Vec::new();
            for each in heard {
                match each {
                    Heard::Read { id, result } => {
                        let Some(done) = files.get::<Option<mlua::Function>>(*id)? else {
                            continue;
                        };
                        files.set(*id, Value::Nil)?;
                        match result {
                            Ok(data) => {
                                let data = lua.create_string(data)?;
                                call_struck(lua, &strikes, &done, (data, Value::Nil), "read")?;
                            }
                            Err(err) => {
                                call_struck(
                                    lua,
                                    &strikes,
                                    &done,
                                    (Value::Nil, err.as_str()),
                                    "read",
                                )?;
                            }
                        }
                    }
                    Heard::Wrote { id, error } => {
                        let Some(done) = files.get::<Option<mlua::Function>>(*id)? else {
                            continue;
                        };
                        files.set(*id, Value::Nil)?;
                        call_struck(lua, &strikes, &done, error.as_deref(), "write")?;
                    }
                    Heard::WatchEnded { id } => {
                        watches.set(*id, Value::Nil)?;
                    }
                    Heard::Watched { id, kind, path } => {
                        let Some(run) = watches.get::<Option<mlua::Function>>(*id)? else {
                            gone.push(*id);
                            continue;
                        };
                        let event = lua.create_table()?;
                        event.set("kind", kind.name())?;
                        event.set("path", path.as_str())?;
                        if call_struck(lua, &strikes, &run, event, "watch")? {
                            watches.set(*id, Value::Nil)?;
                            gone.push(*id);
                        }
                    }
                }
            }
            if !gone.is_empty() {
                gone.sort_unstable();
                gone.dedup();
                with_pending(lua, |pending| {
                    pending.commands.extend(
                        gone.into_iter()
                            .map(solium_wm_protocol::Command::WatchCancel),
                    );
                })?;
            }
            Ok(true)
        })
    }

    pub(crate) fn relayout(&mut self, snapshot: Snapshot) -> Outcome {
        self.dispatch(snapshot, move |lua, _| call_listeners(lua, "layout", ()))
    }

    /// A move drag of a window began, moved or ended (#452): `(id, phase,
    /// x, y)`, told by `crate::input::grab::settle_drags`.
    pub(crate) fn dragged(
        &mut self,
        id: u64,
        phase: &'static str,
        x: f64,
        y: f64,
        snapshot: Snapshot,
    ) -> Outcome {
        self.dispatch_as(FocusSource::Drop, snapshot, move |lua, _| {
            call_listeners(lua, "drag", (id, phase, x, y))
        })
    }

    /// An edge or modifier resize of a window began or ended (#454): `(id,
    /// phase, x, y)`, told by `crate::input::grab::settle_drags`.
    pub(crate) fn resizing(
        &mut self,
        id: u64,
        phase: &'static str,
        x: f64,
        y: f64,
        snapshot: Snapshot,
    ) -> Outcome {
        self.dispatch(snapshot, move |lua, _| {
            call_listeners(lua, "resizing", (id, phase, x, y))
        })
    }

    /// A window was dragged and let go.
    ///
    /// The event a layout needs and could not have: without it a drag in a
    /// tiled layout leaves the window wherever the cursor stopped, because
    /// nothing ever tells the layout to think again.
    pub(crate) fn dropped(&mut self, id: u64, x: f64, y: f64, snapshot: Snapshot) -> Outcome {
        self.dispatch_as(FocusSource::Drop, snapshot, move |lua, _| {
            call_listeners(lua, "drop", (id, x, y))
        })
    }

    /// The pointer wheel turned, with the compositor's modifier held.
    ///
    /// Only offered to scripts when Super is down, so an unmodified wheel keeps
    /// belonging to whatever is under the cursor. A scrolling layout that ate
    /// every wheel event would make every terminal in it unusable.
    pub(crate) fn scrolled(&mut self, dx: f64, dy: f64, snapshot: Snapshot) -> Outcome {
        self.dispatch_as(FocusSource::Scroll, snapshot, move |lua, _| {
            call_listeners(lua, "scroll", (dx, dy))
        })
    }

    /// The shared shape of every dispatch: snapshot in, commands out.
    fn dispatch(
        &mut self,
        snapshot: Snapshot,
        call: impl FnOnce(&Lua, &Table) -> mlua::Result<bool>,
    ) -> Outcome {
        self.dispatch_as(FocusSource::Script, snapshot, call)
    }

    /// [`Self::dispatch`], for a handler answering `source`: what a
    /// `sol.focus` it makes is told with (#220).
    /// `tests::the_focus_event_says_what_moved_the_keyboard`.
    fn dispatch_as(
        &mut self,
        source: FocusSource,
        snapshot: Snapshot,
        call: impl FnOnce(&Lua, &Table) -> mlua::Result<bool>,
    ) -> Outcome {
        let at_the_lock = source == FocusSource::Binding && snapshot.locked;
        self.tell(&snapshot);
        self.lua.set_app_data(Asker(Box::new(Answers(snapshot))));
        self.lua.set_app_data(Pending {
            source,
            at_the_lock,
            ..Pending::default()
        });
        start_clock(&self.lua);

        let handled = match self.lua.globals().get::<Table>("sol") {
            Ok(sol) => match call(&self.lua, &sol) {
                Ok(handled) => handled,
                Err(err) => {
                    // A script erroring must not take the compositor with it,
                    // and must not leave the trigger swallowed either: an
                    // unhandled key belongs to the focused client.
                    tracing::error!(%err, "a script failed");
                    false
                }
            },
            Err(err) => {
                tracing::error!(%err, "`sol` is missing");
                false
            }
        };
        set_deadline(&self.lua, None);
        self.lua.remove_app_data::<Asker>();

        let pending = self.lua.remove_app_data::<Pending>().unwrap_or_default();

        Outcome {
            handled,
            commands: applied(pending.commands),
            grab: pending.grab,
            status: pending.status,
            motion: pending.animated.then_some(pending.animation),
        }
    }
}

/// The commands a dispatch queued, as the compositor applies them,
/// converted in [`crate::wm::convert::commands_of`].
pub(crate) fn applied(queued: Vec<solium_wm_protocol::Command>) -> Vec<Command> {
    queued
        .into_iter()
        .flat_map(crate::wm::convert::commands_of)
        .collect()
}

impl Scripts {
    /// Bring the scripts' mirror up to `snapshot`, with the changes a
    /// window manager in another process would be sent, and hand the Lua
    /// state its copy for the dispatch.
    pub(crate) fn tell(&mut self, snapshot: &Snapshot) {
        self.mirror.apply_all(self.told.changes(snapshot));
        self.lua.set_app_data(self.mirror.clone());
    }
}

/// The compositor's answers, in the same process: its own predicates over
/// the snapshot a dispatch was handed.
pub(crate) struct Answers(pub(crate) Snapshot);

impl Ask for Answers {
    fn ask(&self, query: solium_wm_protocol::Query) -> Option<solium_wm_protocol::Reply> {
        match query {
            // Asked exactly as the compositor asks it, through
            // `crate::state::owns`, the predicate `Solium::window_under`
            // walks with: on a screen that draws the window, and inside what
            // it paints there. No row of a snapshot is what is left of a
            // window whose client has gone, and a minimized window is under
            // no point, even while a present on it plays out (#30).
            solium_wm_protocol::Query::WindowAt((x, y, skip)) => {
                let point = smithay::utils::Point::<f64, smithay::utils::Logical>::from((x, y));
                Some(solium_wm_protocol::Reply::Window(
                    self.0
                        .windows
                        .iter()
                        .find(|window| {
                            Some(window.id) != skip
                                && !window.minimized
                                && crate::state::owns(
                                    window.drawn.slot,
                                    false,
                                    window.drawn.frame,
                                    point,
                                    &self.0.screens,
                                )
                        })
                        .map(|window| window.id),
                ))
            }
        }
    }
}
