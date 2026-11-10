//! The events the compositor tells the window manager: each entry point
//! `state/` calls builds one [`wire::Event`] and sends it in a
//! [`wire::Batch`] with what changed since the last.
//!
//! What an entry point hands back is the [`wire::Answer`] as an [`Outcome`]
//! when it came in time ([`crate::wm::Scripts`] says how long that is), and
//! otherwise the compositor's own answer: nothing, for most, and where an
//! event's answer decides what becomes of it (a key, a wheel turn, a new
//! window), what its entry point says. The window manager's own is applied
//! when it comes.

use solium_wm_protocol as wire;

use crate::script::lua::Ask;
use crate::script::*;
use crate::wm::{Scripts, Wait};

impl Scripts {
    /// Run whatever a key combination is bound to. Handled when a binding
    /// ran; an unbound key, or one whose binding failed, belongs to the
    /// focused client.
    ///
    /// At the lock screen, nothing the answer asks that reaches past the
    /// lock is applied, whatever the window manager let through.
    ///
    /// Unanswered in time, a bound key is the window manager's all the same:
    /// it is not typed into the focused client, and what its binding asks
    /// for is applied when it comes.
    pub(crate) fn key(&mut self, combo: &str, snapshot: Snapshot) -> Outcome {
        let locked = snapshot.locked;
        let event = wire::Event::Key(combo.to_owned());
        match self.event(Some(snapshot), event, Wait::Frame, locked, false) {
            Some((_, outcome)) => outcome,
            None => Outcome {
                handled: self.has_binding(combo),
                ..Outcome::default()
            },
        }
    }

    /// Tell scripts a window has appeared.
    ///
    /// Every listener runs. Returns whether any of them did: nothing happening
    /// is a valid answer, and the compositor then uses its own plain animation
    /// rather than leaving a window to pop into existence.
    ///
    /// Unanswered in time, the window gets the compositor's plain placement,
    /// and the window manager's moves it from there.
    pub(crate) fn opened(&mut self, id: u64, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::Opened(id))
    }

    /// A window went fullscreen, or left it: `(id, entering)`. Its answer
    /// carries the motion the change is drawn with: the compositor's own
    /// when it is late.
    pub(crate) fn fullscreen(&mut self, id: u64, entering: bool, snapshot: Snapshot) -> Outcome {
        self.change(snapshot, wire::Event::Fullscreen((id, entering)))
    }

    /// A window was maximised, or let out of it: `(id, entering)`, as
    /// [`Self::fullscreen`] is told.
    pub(crate) fn maximize(&mut self, id: u64, entering: bool, snapshot: Snapshot) -> Outcome {
        self.change(snapshot, wire::Event::Maximize((id, entering)))
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
            wire::Event::Focused((id, crate::wm::convert::focus_cause(source), changed)),
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
    ///
    /// A layout that takes it moves a seam, and the compositor resizes the
    /// window itself when none does. Unanswered in time, it is taken as the
    /// last one was, a drag being a run of them.
    pub(crate) fn resized(
        &mut self,
        id: u64,
        edge_at: (f64, f64),
        sides: (Option<&'static str>, Option<&'static str>),
        snapshot: Snapshot,
    ) -> Outcome {
        let event = wire::Event::Resized(wire::Resized {
            id,
            x: edge_at.0,
            y: edge_at.1,
            horizontal: sides.0.map(str::to_owned),
            vertical: sides.1.map(str::to_owned),
        });
        match self.event(Some(snapshot), event, Wait::Frame, false, false) {
            Some((_, outcome)) => {
                self.took.resize = outcome.handled && !outcome.commands.is_empty();
                outcome
            }
            None => Outcome {
                handled: self.took.resize,
                ..Outcome::default()
            },
        }
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
    ///
    /// Unanswered in time, nothing was decided, and the notification is
    /// shown as one nobody decided about is.
    pub(crate) fn notification_decision(
        &mut self,
        arrival: &crate::json::Json,
        snapshot: Snapshot,
    ) -> (Option<crate::json::Json>, Outcome) {
        let event = wire::Event::Notification(arrival.render());
        let (decision, outcome) = self
            .event(Some(snapshot), event, Wait::Frame, false, false)
            .unwrap_or_default();
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
        let event = wire::Event::NotifyForget(tokens.to_vec());
        // Nothing to apply: what it asked for while forgetting, if
        // anything, is applied with the next answer.
        if let Some((_, outcome)) = self.event(None, event, Wait::Frame, false, false) {
            self.late.push_back(crate::wm::Late {
                outcome,
                change: false,
            });
        }
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
    ///
    /// One nobody takes goes on to the window under the pointer. Unanswered
    /// in time, it is taken as the last one was, a scroll being a run of
    /// them.
    pub(crate) fn scrolled(&mut self, dx: f64, dy: f64, snapshot: Snapshot) -> Outcome {
        let event = wire::Event::Scrolled((dx, dy));
        match self.event(Some(snapshot), event, Wait::Frame, false, false) {
            Some((_, outcome)) => {
                self.took.scroll = outcome.handled;
                outcome
            }
            None => Outcome {
                handled: self.took.scroll,
                ..Outcome::default()
            },
        }
    }

    /// Ask for the stores that changed, to save them: every
    /// `files.store_interval` on the compositor's clock.
    pub(crate) fn save_stores(&mut self, snapshot: Snapshot) -> Outcome {
        self.send(snapshot, wire::Event::SaveStores)
    }

    /// [`Self::save_stores`], before the configuration is loaded again and
    /// at the compositor's end: waited for the longest, since nothing comes
    /// after it to apply a late answer.
    pub(crate) fn save_stores_at_last(&mut self, snapshot: Snapshot) -> Outcome {
        self.event(
            Some(snapshot),
            wire::Event::SaveStores,
            Wait::Last,
            false,
            false,
        )
        .map(|(_, outcome)| outcome)
        .unwrap_or_default()
    }

    /// Send one event, with what changed since the last batch: its answer,
    /// or an empty one when it is late.
    fn send(&mut self, snapshot: Snapshot, event: wire::Event) -> Outcome {
        self.event(Some(snapshot), event, Wait::Frame, false, false)
            .map(|(_, outcome)| outcome)
            .unwrap_or_default()
    }

    /// Send a window's fullscreen or maximise, whose answer is applied as
    /// one whenever it comes.
    fn change(&mut self, snapshot: Snapshot, event: wire::Event) -> Outcome {
        self.event(Some(snapshot), event, Wait::Frame, false, true)
            .map(|(_, outcome)| outcome)
            .unwrap_or_default()
    }

    /// Send one event in a batch of its own, with what changed since the
    /// last when there is a snapshot to tell it from: its decision and its
    /// outcome when the answer came within `wait`.
    fn event(
        &mut self,
        snapshot: Option<Snapshot>,
        event: wire::Event,
        wait: Wait,
        at_the_lock: bool,
        change: bool,
    ) -> Option<(Option<String>, Outcome)> {
        self.serial += 1;
        let batch = wire::Batch {
            serial: self.serial,
            changes: snapshot
                .as_ref()
                .map(|snapshot| self.told.changes(snapshot))
                .unwrap_or_default(),
            events: vec![event],
        };
        let answers = self.answers(snapshot);
        self.send_batch(batch, answers, at_the_lock, change, wait)
    }
}

/// Take out of an answer to a binding fired at the lock screen what would
/// reach past the lock (#224): the compositor holds to the rule whatever the
/// window manager does.
pub(crate) fn refuse_past_the_lock(answer: &mut wire::Answer) {
    answer.commands.retain(|command| {
        let reaches = crate::wm::convert::reaches_past_the_lock(command);
        if reaches {
            tracing::warn!(
                ?command,
                "a binding at the lock screen asked past the lock; refused"
            );
        }
        !reaches
    });
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
            // Kept by [`Scripts::declarations_in`], not applied.
            wire::Command::Declare(_) => {}
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

/// The compositor's answers, in the same process: its own predicates over
/// the snapshot a dispatch was handed, when there is one, and the stores'
/// files.
#[derive(Debug)]
pub(crate) struct Answers {
    pub(crate) snapshot: Option<Snapshot>,
    pub(crate) stores: std::rc::Rc<crate::wm::stores::StoreReads>,
}

impl Answers {
    /// Answers over `snapshot`, with stores read for nobody in particular.
    #[cfg(test)]
    pub(crate) fn over(snapshot: Snapshot) -> Self {
        Self {
            snapshot: Some(snapshot),
            stores: std::rc::Rc::default(),
        }
    }
}

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
                let snapshot = self.snapshot.as_ref()?;
                let point = smithay::utils::Point::<f64, smithay::utils::Logical>::from((x, y));
                Some(wire::Reply::Window(
                    snapshot
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
                                    &snapshot.screens,
                                )
                        })
                        .map(|window| window.id),
                ))
            }
            wire::Query::Styles => Some(wire::Reply::Styles(
                crate::decoration::available()
                    .into_iter()
                    .map(|offered| wire::Style {
                        name: offered.name,
                        kind: match offered.kind {
                            crate::decoration::StyleKind::Bundle => wire::StyleKind::Bundle,
                            crate::decoration::StyleKind::File => wire::StyleKind::File,
                        },
                    })
                    .collect(),
            )),
            wire::Query::Store((name, max)) => {
                Some(wire::Reply::Store(self.stores.read(&name, max)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use solium_wm_protocol as wire;

    /// A window manager that lets a binding at the lock act on a window is
    /// refused by the compositor: the rule is not the window manager's to
    /// keep.
    #[test]
    fn the_compositor_refuses_what_reaches_past_the_lock_whatever_the_window_manager_sends() {
        let mut answer = wire::Answer {
            serial: 1,
            handled: true,
            commands: vec![
                wire::Command::Close(7),
                wire::Command::Act((
                    1,
                    "audio.mute".to_owned(),
                    "{}".to_owned(),
                    wire::FocusCause::Binding,
                )),
                wire::Command::Quit,
            ],
            motion: None,
            decision: None,
        };
        super::refuse_past_the_lock(&mut answer);
        assert_eq!(
            answer.commands,
            [wire::Command::Act((
                1,
                "audio.mute".to_owned(),
                "{}".to_owned(),
                wire::FocusCause::Binding
            ))]
        );
    }
}
