//! Audio (03 §3.2.5, #347): the default output and input, every sink and
//! source, their volume and mute, for scenes (`Audio`) and for Lua
//! (`sol.audio`, `sol.on("audio")`, `audio.*` actions).
//!
//! * [`volume`] is WirePlumber's volume rules and the write discipline;
//! * [`model`] is the endpoints, as a pure model;
//! * [`pw`] is PipeWire, on a thread of its own;
//! * this module starts the thread the first time the configuration asks for
//!   audio, applies what it saw, and sends it what to do.

pub(crate) mod model;
pub(crate) mod pw;
pub(crate) mod volume;

use std::{collections::BTreeMap, rc::Rc, time::Duration};

use crate::json::Json;

use model::{Audio, Command, Direction, Endpoint, Event, Fact, Settings, Which};
use volume::Origin;

/// What snapshots and scenes read.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct View {
    pub(crate) available: bool,
    pub(crate) ready: bool,
    pub(crate) output: Option<BTreeMap<&'static str, Json>>,
    pub(crate) input: Option<BTreeMap<&'static str, Json>>,
    pub(crate) outputs: Vec<BTreeMap<&'static str, Json>>,
    pub(crate) inputs: Vec<BTreeMap<&'static str, Json>>,
}

/// The service, owned by `Solium`.
#[derive(Debug, Default)]
pub(crate) struct Service {
    /// Asked for by the configuration (`sol.audio`, an `audio.*` action).
    wanted: bool,
    thread: Option<pw::Thread>,
    pub(crate) model: Audio,
    view: Rc<View>,
    dirty: bool,
    pub(crate) events: Vec<Event>,
}

impl Service {
    /// The configuration asked for audio: the thread starts, once.
    pub(crate) fn want(&mut self) {
        if self.wanted {
            return;
        }
        self.wanted = true;
        self.thread = pw::Thread::start();
        self.dirty = true;
    }

    /// `sol.audio{ max, step }`.
    pub(crate) fn configure(&mut self, settings: Settings) {
        self.model.settings = settings;
        self.want();
        self.dirty = true;
    }

    pub(crate) fn view(&self) -> Rc<View> {
        self.view.clone()
    }

    /// What the thread saw since the last call, applied.
    pub(crate) fn settle(&mut self, now: Duration) {
        let facts: Vec<Fact> = self
            .thread
            .as_ref()
            .map(|thread| thread.facts.try_iter().collect())
            .unwrap_or_default();
        for fact in facts {
            if fact == Fact::Down {
                self.thread = None;
            }
            let (events, commands) = self.model.apply(fact, now);
            if !events.is_empty() {
                self.dirty = true;
            }
            self.events.extend(events);
            self.send(commands);
            self.dirty = true;
        }
        let commands = self.model.tick(now);
        if !commands.is_empty() {
            self.dirty = true;
        }
        self.send(commands);
    }

    fn send(&self, commands: Vec<Command>) {
        if let Some(thread) = &self.thread {
            for command in commands {
                thread.send(command);
            }
        }
    }

    /// An `audio.*` action: `audio.volume { target, value | by }`,
    /// `audio.mute { target, value = true | false | "toggle" }`,
    /// `audio.default { endpoint }`.
    /// `an_audio_action_names_its_target_by_output_input_or_id`.
    pub(crate) fn act(
        &mut self,
        action: &str,
        data: &Json,
        origin: Origin,
        now: Duration,
    ) -> Result<(), &'static str> {
        self.want();
        let which = which(data.get("target"))?;
        let number = |key: &str| match data.get(key) {
            Some(Json::Number(n)) => Some(*n),
            _ => None,
        };
        match action {
            "audio.volume" => {
                let command = self.model.request_volume(
                    &which,
                    number("value"),
                    number("by"),
                    origin,
                    now,
                )?;
                self.send(command.into_iter().collect());
            }
            "audio.mute" => {
                let muted = match data.get("value") {
                    Some(Json::Bool(on)) => Some(*on),
                    Some(Json::Text(text)) if text == "toggle" => None,
                    None => None,
                    _ => return Err("bad-data"),
                };
                let command = self.model.request_mute(&which, muted, origin, now)?;
                self.send(vec![command]);
            }
            "audio.default" => {
                let id = data
                    .get("endpoint")
                    .and_then(Json::as_u64)
                    .and_then(|id| u32::try_from(id).ok())
                    .ok_or("bad-data")?;
                let command = self.model.request_default(id)?;
                self.send(vec![command]);
            }
            _ => return Err("unknown-action"),
        }
        self.dirty = true;
        Ok(())
    }

    /// The view rebuilt if anything changed: true when it was.
    pub(crate) fn refresh_view(&mut self) -> bool {
        if !self.dirty {
            return false;
        }
        self.dirty = false;
        let row = |endpoint: &Endpoint| self.row(endpoint);
        let view = View {
            available: self.thread.is_some(),
            ready: self.model.ready,
            output: self.model.default_of(Direction::Output).map(row),
            input: self.model.default_of(Direction::Input).map(row),
            outputs: self
                .model
                .endpoints()
                .filter(|each| each.seen.direction == Direction::Output)
                .map(row)
                .collect(),
            inputs: self
                .model
                .endpoints()
                .filter(|each| each.seen.direction == Direction::Input)
                .map(row)
                .collect(),
        };
        if *self.view == view {
            return false;
        }
        self.view = Rc::new(view);
        true
    }

    fn row(&self, endpoint: &Endpoint) -> BTreeMap<&'static str, Json> {
        let seen = &endpoint.seen;
        BTreeMap::from([
            ("id", Json::Number(f64::from(endpoint.id))),
            ("name", Json::Text(seen.name.clone())),
            ("label", Json::Text(seen.label.clone())),
            ("kind", Json::Text(seen.kind.name().to_owned())),
            (
                "icon",
                Json::Text(format!(
                    "image://solium/icon/{}",
                    seen.kind.icon(seen.direction)
                )),
            ),
            ("volume", Json::Number(endpoint.shown_volume())),
            ("muted", Json::Bool(seen.muted)),
            ("pending", Json::Bool(endpoint.pending())),
            ("step", Json::Number(self.model.settings.step)),
            ("max", Json::Number(self.model.settings.max)),
            ("isDefault", Json::Bool(self.model.is_default(endpoint))),
            ("portLabel", Json::Text(seen.port_label.clone())),
            ("portKind", Json::Text(seen.port_kind.clone())),
            ("state", Json::Text(seen.state.clone())),
            (
                "direction",
                Json::Text(
                    match seen.direction {
                        Direction::Output => "output",
                        Direction::Input => "input",
                    }
                    .to_owned(),
                ),
            ),
        ])
    }
}

/// `target`: `"output"`, `"input"`, or an endpoint's id; the output when
/// left out.
fn which(target: Option<&Json>) -> Result<Which, &'static str> {
    match target {
        None => Ok(Which::Output),
        Some(Json::Text(text)) if text == "output" => Ok(Which::Output),
        Some(Json::Text(text)) if text == "input" => Ok(Which::Input),
        Some(value) => value
            .as_u64()
            .and_then(|id| u32::try_from(id).ok())
            .map(Which::Id)
            .ok_or("bad-data"),
    }
}

/// An event as Lua's `sol.on("audio")` reads it.
pub(crate) fn event_json(event: &Event) -> Json {
    Json::Object(BTreeMap::from([
        ("what".to_owned(), Json::Text(event.what.to_owned())),
        ("target".to_owned(), Json::Number(f64::from(event.id))),
        (
            "direction".to_owned(),
            Json::Text(
                match event.direction {
                    Direction::Output => "output",
                    Direction::Input => "input",
                }
                .to_owned(),
            ),
        ),
        ("output".to_owned(), Json::Bool(event.output)),
        ("value".to_owned(), Json::Number(event.value)),
        ("muted".to_owned(), Json::Bool(event.muted)),
        (
            "origin".to_owned(),
            Json::Text(event.origin.name().to_owned()),
        ),
        ("bus".to_owned(), Json::Text(event.bus.name().to_owned())),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_audio_action_names_its_target_by_output_input_or_id() {
        assert_eq!(which(None), Ok(Which::Output));
        assert_eq!(
            which(Some(&Json::Text("input".to_owned()))),
            Ok(Which::Input)
        );
        assert_eq!(which(Some(&Json::Number(4.0))), Ok(Which::Id(4)));
        assert_eq!(which(Some(&Json::Bool(true))), Err("bad-data"));
    }
}
