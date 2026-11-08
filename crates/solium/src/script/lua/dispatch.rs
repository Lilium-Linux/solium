//! The window manager's half of an event: [`Host::handle`] brings its
//! mirror up to the batch, runs each event's listeners, and answers. The
//! compositor's half is `crate::wm::events`.

use mlua::{Lua, Table, Value};
use solium_wm_protocol as wire;

use crate::script::lua::*;
use crate::script::*;

/// What a dispatch adds to an answer: its commands, then whether
/// a mode holds input and the status line, as the commands of those names,
/// and the motion `sol.animate` set.
pub(crate) fn answered(answer: &mut wire::Answer, pending: Pending) {
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

impl Host {
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
            answer.commands.extend(declare_if_changed(&self.lua));
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
            let source = cause.name();
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
        E::SaveStores => {
            // Read whole before what was sent is looked at: reading them
            // borrows the Lua state's app data too.
            let stores: Vec<(String, String)> = super::stores_of(lua)
                .into_iter()
                .map(|(name, value)| (name, value.render()))
                .collect();
            let changed: Vec<(String, String)> = {
                let sent = lua.app_data_ref::<StoresSent>();
                stores
                    .into_iter()
                    .filter(|(name, text)| {
                        sent.as_ref().and_then(|sent| sent.0.get(name)) != Some(text)
                    })
                    .collect()
            };
            if let Some(mut sent) = lua.app_data_mut::<StoresSent>() {
                for (name, text) in &changed {
                    sent.0.insert(name.clone(), text.clone());
                }
            }
            with_pending(lua, |pending| {
                pending
                    .commands
                    .extend(changed.into_iter().map(wire::Command::Store));
            })?;
            Ok(false)
        }
    }
}
