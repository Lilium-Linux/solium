//! The events a configuration is told, in two halves that meet only in the
//! window manager protocol's terms.
//!
//! The compositor's half is the entry points `state/` calls: each builds one
//! [`wire::Event`], sends it in a [`wire::Batch`] with what changed since
//! the last, and reads the [`wire::Answer`] back as an [`Outcome`]. The
//! window manager's half is [`Scripts::handle`]: it brings its mirror up to
//! date, runs each event's listeners, and answers. Between them is a
//! function call here, and a socket when the window manager is a process of
//! its own.

use mlua::{Lua, Table, Value};
use solium_wm_protocol as wire;

use crate::script::lua::*;
use crate::script::*;

impl Scripts {
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

    /// Run whatever a key combination is bound to. Handled when a binding
    /// ran; an unbound key, or one whose binding failed, belongs to the
    /// focused client.
    pub(crate) fn key(&mut self, combo: &str, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::Key(combo.to_owned()))
    }

    /// Tell scripts a window has appeared.
    ///
    /// Every listener runs. Returns whether any of them did: nothing happening
    /// is a valid answer, and the compositor then uses its own plain animation
    /// rather than leaving a window to pop into existence.
    pub(crate) fn opened(&mut self, id: u64, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::Opened(id))
    }

    /// A window went fullscreen, or left it: `(id, entering)`.
    pub(crate) fn fullscreen(&mut self, id: u64, entering: bool, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::Fullscreen((id, entering)))
    }

    /// A window was maximised, or let out of it: `(id, entering)`.
    pub(crate) fn maximize(&mut self, id: u64, entering: bool, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::Maximize((id, entering)))
    }

    /// A press while a mode holds input.
    pub(crate) fn click(&mut self, x: f64, y: f64, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::Click((x, y)))
    }

    /// A finger, routed to the scripts.
    pub(crate) fn touch(&mut self, event: TouchEvent, snapshot: Snapshot) -> Outcome {
        self.send(
            snapshot,
            wire::Event::Touch(wire::TouchEvent {
                kind: event.kind.to_owned(),
                slot: event.slot,
                x: event.x,
                y: event.y,
                output: event.output,
                target: event.target.to_owned(),
                window: event.window,
                control: event.control,
                edges: event.edges.map(str::to_owned),
                time: event.time,
                t: event.t,
                vx: event.velocity.0,
                vy: event.velocity.1,
            }),
        )
    }

    /// What the configuration asked for as it loaded: its top level runs
    /// `sol` functions before any event, and what they queued lands in the
    /// same buffer an event handler would use. Without draining it that work
    /// is simply dropped, silently, which is what happened to the first
    /// dock: no error, no items, nothing drawn.
    pub(crate) fn startup(&mut self) -> Outcome {
        // Takes the buffer the configuration filled, rather than dispatching:
        // a dispatch installs a fresh `Pending` before it calls anything, so
        // routing this through it would throw away the very requests it is
        // here to collect.
        let pending = self.lua.remove_app_data::<Pending>().unwrap_or_default();
        self.lua.set_app_data(Pending::default());
        let mut answer = wire::Answer {
            serial: 0,
            handled: false,
            commands: Vec::new(),
            motion: None,
            decision: None,
        };
        answered(&mut answer, pending);
        outcome_of(answer)
    }

    /// Focus moved to a window: `(id, source, changed)`, `source` the word
    /// of what moved it.
    pub(crate) fn focused(
        &mut self,
        id: u64,
        source: FocusSource,
        changed: bool,
        snapshot: Snapshot,
    ) -> Outcome {
        self.send(
            snapshot,
            wire::Event::Focused((id, to_wire::focus_cause(source), changed)),
        )
    }

    /// The keyboard's layout or a lock changed: `changed` says which.
    pub(crate) fn keyboard_changed(
        &mut self,
        changed: &'static str,
        snapshot: Snapshot,
    ) -> Outcome {
        self.send(snapshot, wire::Event::KeyboardChanged(changed.to_owned()))
    }

    /// The session locked, or unlocked: `state` says which.
    pub(crate) fn lock_changed(&mut self, state: &'static str, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::LockChanged(state.to_owned()))
    }

    /// A text field took the keyboard, moved its caret, or let go.
    pub(crate) fn text_input(&mut self, why: &'static str, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::TextInput(why.to_owned()))
    }

    /// A window asked to be activated, and why.
    pub(crate) fn activated(&mut self, id: u64, why: &'static str, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::Activated((id, why.to_owned())))
    }

    /// A window's own resize moved an edge to `edge_at`.
    pub(crate) fn resized(
        &mut self,
        id: u64,
        edge_at: (f64, f64),
        sides: (Option<&'static str>, Option<&'static str>),
        snapshot: Snapshot,
    ) -> Outcome {
        self.send(
            snapshot,
            wire::Event::Resized(wire::Resized {
                id,
                x: edge_at.0,
                y: edge_at.1,
                horizontal: sides.0.map(str::to_owned),
                vertical: sides.1.map(str::to_owned),
            }),
        )
    }

    pub(crate) fn closed(&mut self, id: u64, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::Closed(id))
    }

    pub(crate) fn closing(&mut self, id: u64, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::Closing(id))
    }

    pub(crate) fn close_landed(&mut self, id: u64, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::CloseLanded(id))
    }

    pub(crate) fn refused(&mut self, id: u64, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::Refused(id))
    }

    /// A notification arriving, for `sol.on("notification")` to decide its
    /// fate; `None` when no decider said anything.
    pub(crate) fn notification_decision(
        &mut self,
        arrival: &crate::json::Json,
        snapshot: Snapshot,
    ) -> (Option<crate::json::Json>, Outcome) {
        let (decision, outcome) =
            self.send_for(snapshot, wire::Event::Notification(arrival.render()));
        (
            decision.and_then(|text| crate::json::Json::parse(&text)),
            outcome,
        )
    }

    /// Something happened to a notification (#346): `added`, `replaced`,
    /// `closed`, `invoked`, `dnd` or `owner`, for `sol.on("notifications")`.
    pub(crate) fn notifications_event(
        &mut self,
        event: &crate::json::Json,
        snapshot: Snapshot,
    ) -> Outcome {
        self.send(snapshot, wire::Event::Notifications(event.render()))
    }

    pub(crate) fn audio_event(&mut self, event: &crate::json::Json, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::Audio(event.render()))
    }

    pub(crate) fn tray_event(&mut self, event: &crate::json::Json, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::Tray(event.render()))
    }

    pub(crate) fn media_event(&mut self, event: &crate::json::Json, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::Media(event.render()))
    }

    /// The notifications of the configuration's own that are gone, whose
    /// action functions are let go.
    pub(crate) fn notify_forget(&mut self, tokens: &[u64]) {
        self.serial += 1;
        let batch = wire::Batch {
            serial: self.serial,
            changes: Vec::new(),
            events: vec![wire::Event::NotifyForget(tokens.to_vec())],
        };
        let _ = self.handle(batch, Box::new(NoAnswers));
    }

    /// One of the configuration's own notifications was acted on.
    pub(crate) fn notify_action(&mut self, token: u64, key: &str, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::NotifyAction((token, key.to_owned())))
    }

    pub(crate) fn minimized(&mut self, id: u64, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::Minimized(id))
    }

    pub(crate) fn unminimized(&mut self, id: u64, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::Unminimized(id))
    }

    pub(crate) fn monitors_changed(&mut self, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::MonitorsChanged)
    }

    pub(crate) fn problems_changed(&mut self, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::ProblemsChanged)
    }

    pub(crate) fn restored(&mut self, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::Restored)
    }

    /// An action a window's frame sent, with its data.
    pub(crate) fn frame_action(
        &mut self,
        id: u64,
        name: &str,
        data: &crate::json::Json,
        snapshot: Snapshot,
    ) -> Outcome {
        self.send(
            snapshot,
            wire::Event::FrameAction((id, name.to_owned(), data.render())),
        )
    }

    /// An action a scene sent with `Solium.send`, with its data.
    pub(crate) fn surface_action(
        &mut self,
        name: &str,
        action: &str,
        data: &crate::json::Json,
        snapshot: Snapshot,
    ) -> Outcome {
        self.send(
            snapshot,
            wire::Event::SurfaceAction((name.to_owned(), action.to_owned(), data.render())),
        )
    }

    /// What `sol.act` attempts came to.
    pub(crate) fn attempts_settled(&mut self, settled: &[Settled], snapshot: Snapshot) -> Outcome {
        self.send(
            snapshot,
            wire::Event::AttemptsSettled(
                settled
                    .iter()
                    .map(|each| wire::Settled {
                        attempt: each.attempt,
                        ok: each.ok,
                        reason: each.reason.map(str::to_owned),
                    })
                    .collect(),
            ),
        )
    }

    /// What `sol.run{}`'s programs printed, and how they ended.
    pub(crate) fn runs_heard(
        &mut self,
        heard: &[crate::runs::Heard],
        snapshot: Snapshot,
    ) -> Outcome {
        use crate::runs::{Heard, Stream};
        let heard = heard
            .iter()
            .map(|each| match each {
                Heard::Line {
                    id,
                    stream: Stream::Stdout,
                    line,
                } => wire::RunHeard::Stdout((*id, line.clone())),
                Heard::Line {
                    id,
                    stream: Stream::Stderr,
                    line,
                } => wire::RunHeard::Stderr((*id, line.clone())),
                Heard::Done { id, ended } => wire::RunHeard::Done(wire::RunEnded {
                    id: *id,
                    code: ended.code,
                    signal: ended.signal,
                    timed_out: ended.timed_out,
                    truncated: ended.truncated,
                    stdout: ended.stdout.clone(),
                    stderr: ended.stderr.clone(),
                    error: ended.error.clone(),
                }),
            })
            .collect();
        self.send(snapshot, wire::Event::Runs(heard))
    }

    /// The timers due, each with whether this is its last time.
    pub(crate) fn timers_due(&mut self, due: &[(u64, bool)], snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::Timers(due.to_vec()))
    }

    /// What the configuration's reads, writes and watches came to (#473),
    /// in the order heard.
    pub(crate) fn files_heard(
        &mut self,
        heard: &[crate::files::Heard],
        snapshot: Snapshot,
    ) -> Outcome {
        use crate::files::{Heard, Kind};
        let heard = heard
            .iter()
            .map(|each| match each {
                Heard::Read { id, result } => wire::FileHeard::Read((
                    *id,
                    match result {
                        Ok(data) => wire::ResultBytes::Ok(data.clone()),
                        Err(err) => wire::ResultBytes::Error(err.clone()),
                    },
                )),
                Heard::Wrote { id, error } => wire::FileHeard::Wrote((*id, error.clone())),
                Heard::Watched { id, kind, path } => wire::FileHeard::Watched((
                    *id,
                    match kind {
                        Kind::Changed => wire::FileChange::Changed,
                        Kind::Created => wire::FileChange::Created,
                        Kind::Removed => wire::FileChange::Removed,
                    },
                    path.clone(),
                )),
                Heard::WatchEnded { id } => wire::FileHeard::WatchEnded(*id),
            })
            .collect();
        self.send(snapshot, wire::Event::Files(heard))
    }

    pub(crate) fn relayout(&mut self, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::Relayout)
    }

    /// How a drag of `id` has got on.
    pub(crate) fn dragged(
        &mut self,
        id: u64,
        phase: &'static str,
        x: f64,
        y: f64,
        snapshot: Snapshot,
    ) -> Outcome {
        self.send(
            snapshot,
            wire::Event::Dragged(wire::PointerMove {
                id,
                phase: phase.to_owned(),
                x,
                y,
            }),
        )
    }

    /// How a resize of `id` has got on.
    pub(crate) fn resizing(
        &mut self,
        id: u64,
        phase: &'static str,
        x: f64,
        y: f64,
        snapshot: Snapshot,
    ) -> Outcome {
        self.send(
            snapshot,
            wire::Event::Resizing(wire::PointerMove {
                id,
                phase: phase.to_owned(),
                x,
                y,
            }),
        )
    }

    pub(crate) fn dropped(&mut self, id: u64, x: f64, y: f64, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::Dropped((id, x, y)))
    }

    /// A modified wheel turn, offered to the scripts.
    pub(crate) fn scrolled(&mut self, dx: f64, dy: f64, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::Scrolled((dx, dy)))
    }

    /// Send one event, with what changed since the last batch, and read the
    /// answer as the compositor applies it.
    fn send(&mut self, snapshot: Snapshot, event: wire::Event) -> Outcome {
        self.send_for(snapshot, event).1
    }

    /// [`Self::send`], with the answer's decision, as JSON text.
    fn send_for(&mut self, snapshot: Snapshot, event: wire::Event) -> (Option<String>, Outcome) {
        self.serial += 1;
        let batch = wire::Batch {
            serial: self.serial,
            changes: self.told.changes(&snapshot),
            events: vec![event],
        };
        let answer = self.handle(batch, Box::new(Answers(snapshot)));
        let decision = answer.decision.clone();
        (decision, outcome_of(answer))
    }
}

/// An answer as the compositor applies it: its commands converted in
/// [`crate::wm::convert::commands_of`], whether a mode holds input and the
/// status line read out of them, the motion as the compositor's.
pub(crate) fn outcome_of(answer: wire::Answer) -> Outcome {
    let mut outcome = Outcome {
        handled: answer.handled,
        ..Outcome::default()
    };
    for command in answer.commands {
        match command {
            wire::Command::Grab(grab) => outcome.grab = Some(grab),
            wire::Command::Status(status) => outcome.status = Some(status),
            command => outcome
                .commands
                .extend(crate::wm::convert::commands_of(command)),
        }
    }
    outcome.motion = answer
        .motion
        .and_then(|motion| crate::wm::convert::animation(&motion).ok());
    outcome
}

/// What a handler's dispatch adds to an answer: its commands, then whether
/// a mode holds input and the status line, as the commands of those names,
/// and the motion `sol.animate` set.
fn answered(answer: &mut wire::Answer, pending: Pending) {
    answer.commands.extend(pending.commands);
    if let Some(grab) = pending.grab {
        answer.commands.push(wire::Command::Grab(grab));
    }
    if let Some(status) = pending.status {
        answer.commands.push(wire::Command::Status(status));
    }
    if pending.animated {
        answer.motion = Some(to_wire::motion(pending.animation));
    }
}

impl Scripts {
    /// The window manager's half: bring the mirror up to the batch, run
    /// each event's listeners in order, and answer. Each event is its own
    /// dispatch, under the deadline, answering for what moved it.
    pub(crate) fn handle(&mut self, batch: wire::Batch, ask: Box<dyn Ask>) -> wire::Answer {
        self.mirror.apply_all(batch.changes);
        self.lua.set_app_data(self.mirror.clone());
        self.lua.set_app_data(Asker(ask));
        let mut answer = wire::Answer {
            serial: batch.serial,
            handled: false,
            commands: Vec::new(),
            motion: None,
            decision: None,
        };
        for event in batch.events {
            let source = source_of(&event);
            let at_the_lock = source == FocusSource::Binding && self.mirror.locked;
            self.lua.set_app_data(Pending {
                source,
                at_the_lock,
                ..Pending::default()
            });
            start_clock(&self.lua);
            let mut decision = None;
            let handled = match self.lua.globals().get::<Table>("sol") {
                Ok(sol) => match run(&self.lua, &sol, event, &mut decision) {
                    Ok(handled) => handled,
                    Err(err) => {
                        // A script erroring must not take the compositor with
                        // it, and must not leave the trigger swallowed either:
                        // an unhandled key belongs to the focused client.
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
            let pending = self.lua.remove_app_data::<Pending>().unwrap_or_default();
            answer.handled |= handled;
            answered(&mut answer, pending);
            if decision.is_some() {
                answer.decision = decision;
            }
        }
        self.lua.remove_app_data::<Asker>();
        answer
    }
}

/// What moved the keyboard, for a `sol.focus` an event's handler makes
/// (#220).
fn source_of(event: &wire::Event) -> FocusSource {
    match event {
        wire::Event::Key(_) => FocusSource::Binding,
        wire::Event::Opened(_) => FocusSource::Open,
        wire::Event::Click(_) => FocusSource::Click,
        wire::Event::Touch(_) => FocusSource::Touch,
        wire::Event::Activated(_) => FocusSource::Activate,
        wire::Event::FrameAction(_) | wire::Event::SurfaceAction(_) => FocusSource::Surface,
        wire::Event::Dragged(_) | wire::Event::Dropped(_) => FocusSource::Drop,
        wire::Event::Scrolled(_) => FocusSource::Scroll,
        _ => FocusSource::Script,
    }
}

/// A JSON value from the compositor, as Lua reads it.
fn json_value(lua: &Lua, text: &str) -> Value {
    crate::json::Json::parse(text)
        .and_then(|json| json.to_lua(lua).ok())
        .unwrap_or(Value::Nil)
}

/// One event's listeners, run: whether it was handled.
fn run(
    lua: &Lua,
    sol: &Table,
    event: wire::Event,
    decision: &mut Option<String>,
) -> mlua::Result<bool> {
    use wire::Event as E;
    match event {
        E::Startup => Ok(false),
        E::Key(combo) => {
            let bindings: Table = sol.get("_bindings")?;
            let handler: Value = bindings.get(normalise_combo(&combo))?;
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
        }
        E::Opened(id) => call_listeners(lua, "open", id),
        E::Fullscreen((id, entering)) => call_listeners(lua, "fullscreen", (id, entering)),
        E::Maximize((id, entering)) => call_listeners(lua, "maximize", (id, entering)),
        E::Click((x, y)) => call_listeners(lua, "click", (x, y)),
        E::Touch(event) => {
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
            table.set("vx", event.vx)?;
            table.set("vy", event.vy)?;
            call_listeners(lua, "touch", table)
        }
        E::Focused((id, cause, changed)) => {
            let source = crate::wm::convert::focus_source(cause).name();
            call_listeners(lua, "focus", (id, source, changed))
        }
        E::KeyboardChanged(changed) => {
            let state: Value = sol.get::<mlua::Function>("keyboard")?.call(())?;
            call_listeners(lua, "keyboard", (state, changed))
        }
        E::LockChanged(state) => call_listeners(lua, "lock", state),
        E::TextInput(why) => {
            let field: Value = sol.get::<mlua::Function>("text_input")?.call(())?;
            call_listeners(lua, "text_input", (field, why))
        }
        E::Activated((id, why)) => call_listeners(lua, "activate", (id, why)),
        E::Resized(resized) => {
            let wire::Resized {
                id,
                x,
                y,
                horizontal,
                vertical,
            } = resized;
            call_listeners(lua, "resize", (id, x, y, horizontal, vertical))
        }
        E::Closed(id) => call_listeners(lua, "close", id),
        E::Closing(id) => call_listeners(lua, "closing", id),
        E::CloseLanded(id) => call_listeners(lua, "close_landed", id),
        E::Refused(id) => call_listeners(lua, "refused", id),
        E::Notification(arrival) => {
            let data = json_value(lua, &arrival);
            *decision = call_deciders(lua, "notification", data)?.map(|decided| decided.render());
            Ok(true)
        }
        E::Notifications(event) => call_listeners(lua, "notifications", json_value(lua, &event)),
        E::Audio(event) => call_listeners(lua, "audio", json_value(lua, &event)),
        E::Tray(event) => call_listeners(lua, "tray", json_value(lua, &event)),
        E::Media(event) => call_listeners(lua, "media", json_value(lua, &event)),
        E::NotifyForget(tokens) => {
            let kept: Table = sol.get("_notify_actions")?;
            for token in tokens {
                kept.set(token, Value::Nil)?;
            }
            Ok(false)
        }
        E::NotifyAction((token, key)) => {
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
        }
        E::Minimized(id) => call_listeners(lua, "minimize", id),
        E::Unminimized(id) => call_listeners(lua, "unminimize", id),
        E::MonitorsChanged => call_listeners(lua, "monitors", ()),
        E::ProblemsChanged => call_listeners(lua, "problems", ()),
        E::Restored => call_listeners(lua, "restore", ()),
        E::FrameAction((id, name, data)) => {
            call_listeners(lua, "frame", (id, name, json_value(lua, &data)))
        }
        E::SurfaceAction((name, action, data)) => {
            call_listeners(lua, "surface", (name, action, json_value(lua, &data)))
        }
        E::AttemptsSettled(settled) => {
            let attempts: Table = sol.get("_attempts")?;
            let strikes: Table = sol.get("_strikes")?;
            for each in settled {
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
                        // What a stopped `done` asked for is dropped, and
                        // the attempts it made forgotten with it.
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
                            if let wire::Command::Act((attempt, ..)) = command {
                                attempts.set(attempt, Value::Nil)?;
                            }
                        }
                        let _ = strike(lua, &strikes, &done, "done", &err)?;
                    }
                    Err(err) => tracing::error!(%err, "an attempt's done failed"),
                }
            }
            Ok(true)
        }
        E::Runs(heard) => {
            let runs: Table = sol.get("_runs")?;
            let strikes: Table = sol.get("_strikes")?;
            for each in heard {
                let (id, stream, line) = match each {
                    wire::RunHeard::Stdout((id, line)) => (id, "stdout", line),
                    wire::RunHeard::Stderr((id, line)) => (id, "stderr", line),
                    wire::RunHeard::Done(ended) => {
                        let Some(kept) = runs.get::<Option<Table>>(ended.id)? else {
                            continue;
                        };
                        runs.set(ended.id, Value::Nil)?;
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
                        continue;
                    }
                };
                let Some(kept) = runs.get::<Option<Table>>(id)? else {
                    continue;
                };
                let Some(lines) = kept.get::<Option<mlua::Function>>("lines")? else {
                    continue;
                };
                let line = lua.create_string(&line)?;
                call_struck(lua, &strikes, &lines, (line, stream), "lines")?;
            }
            Ok(true)
        }
        E::Timers(due) => {
            let timers: Table = sol.get("_timers")?;
            let strikes: Table = sol.get("_strikes")?;
            let mut gone = Vec::new();
            for (id, last) in due {
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
                    pending
                        .commands
                        .extend(gone.into_iter().map(wire::Command::TimerCancel));
                })?;
            }
            Ok(true)
        }
        E::Files(heard) => {
            let files: Table = sol.get("_files")?;
            let watches: Table = sol.get("_watches")?;
            let strikes: Table = sol.get("_strikes")?;
            let mut gone = Vec::new();
            for each in heard {
                match each {
                    wire::FileHeard::Read((id, result)) => {
                        let Some(done) = files.get::<Option<mlua::Function>>(id)? else {
                            continue;
                        };
                        files.set(id, Value::Nil)?;
                        match result {
                            wire::ResultBytes::Ok(data) => {
                                let data = lua.create_string(&data)?;
                                call_struck(lua, &strikes, &done, (data, Value::Nil), "read")?;
                            }
                            wire::ResultBytes::Error(err) => {
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
                    wire::FileHeard::Wrote((id, error)) => {
                        let Some(done) = files.get::<Option<mlua::Function>>(id)? else {
                            continue;
                        };
                        files.set(id, Value::Nil)?;
                        call_struck(lua, &strikes, &done, error.as_deref(), "write")?;
                    }
                    wire::FileHeard::WatchEnded(id) => {
                        watches.set(id, Value::Nil)?;
                    }
                    wire::FileHeard::Watched((id, kind, path)) => {
                        let Some(run) = watches.get::<Option<mlua::Function>>(id)? else {
                            gone.push(id);
                            continue;
                        };
                        let event = lua.create_table()?;
                        event.set(
                            "kind",
                            match kind {
                                wire::FileChange::Changed => "changed",
                                wire::FileChange::Created => "created",
                                wire::FileChange::Removed => "removed",
                            },
                        )?;
                        event.set("path", path.as_str())?;
                        if call_struck(lua, &strikes, &run, event, "watch")? {
                            watches.set(id, Value::Nil)?;
                            gone.push(id);
                        }
                    }
                }
            }
            if !gone.is_empty() {
                gone.sort_unstable();
                gone.dedup();
                with_pending(lua, |pending| {
                    pending
                        .commands
                        .extend(gone.into_iter().map(wire::Command::WatchCancel));
                })?;
            }
            Ok(true)
        }
        E::Relayout => call_listeners(lua, "layout", ()),
        E::Dragged(moved) => call_listeners(lua, "drag", (moved.id, moved.phase, moved.x, moved.y)),
        E::Resizing(moved) => {
            call_listeners(lua, "resizing", (moved.id, moved.phase, moved.x, moved.y))
        }
        E::Dropped((id, x, y)) => call_listeners(lua, "drop", (id, x, y)),
        E::Scrolled((dx, dy)) => call_listeners(lua, "scroll", (dx, dy)),
    }
}

/// The compositor's answers, in the same process: its own predicates over
/// the snapshot a dispatch was handed.
pub(crate) struct Answers(pub(crate) Snapshot);

impl Ask for Answers {
    fn ask(&self, query: wire::Query) -> Option<wire::Reply> {
        match query {
            // Asked exactly as the compositor asks it, through
            // `crate::state::owns`, the predicate `Solium::window_under`
            // walks with: on a screen that draws the window, and inside what
            // it paints there. No row of a snapshot is what is left of a
            // window whose client has gone, and a minimized window is under
            // no point, even while a present on it plays out (#30).
            wire::Query::WindowAt((x, y, skip)) => {
                let point = smithay::utils::Point::<f64, smithay::utils::Logical>::from((x, y));
                Some(wire::Reply::Window(
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

/// Nobody to ask: a batch the compositor sends with no snapshot behind it.
struct NoAnswers;

impl Ask for NoAnswers {
    fn ask(&self, _: wire::Query) -> Option<wire::Reply> {
        None
    }
}
