//! Audio (03 §3.2.5, #347): the default output and input, every sink and
//! source, every application's playback stream (#416), their volume and
//! mute, for scenes (`Audio`) and for Lua (`sol.audio`, `sol.on("audio")`,
//! `audio.*` actions), and which window each stream is.
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

use model::{Audio, Command, Direction, Endpoint, Event, Fact, Settings, Which, WindowKey};
use volume::Origin;

const MIN_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// What snapshots and scenes read.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct View {
    pub(crate) available: bool,
    pub(crate) ready: bool,
    pub(crate) output: Option<BTreeMap<&'static str, Json>>,
    pub(crate) input: Option<BTreeMap<&'static str, Json>>,
    pub(crate) outputs: Vec<BTreeMap<&'static str, Json>>,
    pub(crate) inputs: Vec<BTreeMap<&'static str, Json>>,
    /// Every application's playback stream (#416).
    pub(crate) streams: Vec<BTreeMap<&'static str, Json>>,
    /// Each window that has a stream, by its id: its streams as one, which
    /// `Audio.forWindow` reads and `audio.volume { window }` sets.
    pub(crate) windows: BTreeMap<u64, BTreeMap<&'static str, Json>>,
}

/// The service, owned by `Solium`.
#[derive(Debug, Default)]
pub(crate) struct Service {
    /// Asked for by the configuration (`sol.audio`, an `audio.*` action).
    wanted: bool,
    thread: Option<pw::Thread>,
    /// When to try PipeWire again after it went away, and how long the next
    /// wait is: 1 s, doubling to 30 s, back to 1 s once it answers.
    retry_at: Option<Duration>,
    backoff: Duration,
    pub(crate) model: Audio,
    /// The windows streams are matched to, as last told
    /// ([`Self::tell_windows`]).
    windows: Vec<WindowKey>,
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

    /// Whether any application is playing through a stream, so the windows
    /// are worth telling.
    pub(crate) fn has_streams(&self) -> bool {
        self.model.has_streams()
    }

    /// The windows, their processes and their application ids, which streams
    /// are matched to; the view is rebuilt when they changed.
    pub(crate) fn tell_windows(&mut self, windows: Vec<WindowKey>) {
        if self.windows != windows {
            self.windows = windows;
            self.dirty = true;
        }
    }

    /// What the thread saw since the last call, applied; and PipeWire tried
    /// again, after a wait, when it went away, so the volume keys come back
    /// with it. `audio_comes_back_after_pipewire_restarts`.
    pub(crate) fn settle(&mut self, now: Duration) {
        if self.wanted && self.thread.is_none() {
            match self.retry_at {
                None => self.retry_at = Some(now + self.backoff.max(MIN_BACKOFF)),
                Some(at) if now >= at => {
                    self.thread = pw::Thread::start();
                    self.retry_at = None;
                    self.backoff = (self.backoff.max(MIN_BACKOFF) * 2).min(MAX_BACKOFF);
                }
                Some(_) => {}
            }
        }
        let facts: Vec<Fact> = self
            .thread
            .as_ref()
            .map(|thread| thread.facts.try_iter().collect())
            .unwrap_or_default();
        for fact in facts {
            match fact {
                Fact::Down => self.thread = None,
                Fact::Ready => self.backoff = MIN_BACKOFF,
                _ => {}
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
        if let Some(window) = data.get("window") {
            let window = window.as_u64().ok_or("bad-data")?;
            return self.act_on_window(action, window, data, origin, now);
        }
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

    /// An `audio.volume` or `audio.mute` on every stream of one window
    /// (#416), together: a value sets each to it, `by` moves each from where
    /// it is, and a toggle mutes them all unless every one is muted, which
    /// unmutes them all. A window with no stream answers `unknown-endpoint`.
    /// `tests::a_window_s_streams_are_set_together`.
    fn act_on_window(
        &mut self,
        action: &str,
        window: u64,
        data: &Json,
        origin: Origin,
        now: Duration,
    ) -> Result<(), &'static str> {
        let streams: Vec<u32> = self
            .model
            .windows_of_streams(&self.windows)
            .into_iter()
            .filter(|(_, windows)| windows.contains(&window))
            .map(|(stream, _)| stream)
            .collect();
        if streams.is_empty() {
            return Err("unknown-endpoint");
        }
        let number = |key: &str| match data.get(key) {
            Some(Json::Number(n)) => Some(*n),
            _ => None,
        };
        let mut commands = Vec::new();
        match action {
            "audio.volume" => {
                for stream in &streams {
                    let command = self.model.request_volume(
                        &Which::Id(*stream),
                        number("value"),
                        number("by"),
                        origin,
                        now,
                    )?;
                    commands.extend(command);
                }
            }
            "audio.mute" => {
                let muted = match data.get("value") {
                    Some(Json::Bool(on)) => *on,
                    Some(Json::Text(text)) if text == "toggle" => !self.model.all_muted(&streams),
                    None => !self.model.all_muted(&streams),
                    _ => return Err("bad-data"),
                };
                for stream in &streams {
                    commands.push(self.model.request_mute(
                        &Which::Id(*stream),
                        Some(muted),
                        origin,
                        now,
                    )?);
                }
            }
            _ => return Err("unknown-action"),
        }
        self.send(commands);
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
        let owners = self.model.windows_of_streams(&self.windows);
        let streams: Vec<&Endpoint> = self
            .model
            .endpoints()
            .filter(|each| each.is_stream())
            .collect();
        let view = View {
            available: self.thread.is_some(),
            ready: self.model.ready,
            output: self.model.default_of(Direction::Output).map(row),
            input: self.model.default_of(Direction::Input).map(row),
            outputs: self
                .model
                .endpoints()
                .filter(|each| !each.is_stream() && each.seen.direction == Direction::Output)
                .map(row)
                .collect(),
            inputs: self
                .model
                .endpoints()
                .filter(|each| !each.is_stream() && each.seen.direction == Direction::Input)
                .map(row)
                .collect(),
            streams: streams
                .iter()
                .map(|stream| self.stream_row(stream, owners.get(&stream.id)))
                .collect(),
            windows: self.window_rows(&streams, &owners),
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

impl Service {
    /// One playback stream as a scene and Lua read it (#416).
    fn stream_row(
        &self,
        stream: &Endpoint,
        windows: Option<&Vec<u64>>,
    ) -> BTreeMap<&'static str, Json> {
        let app = stream.seen.app.clone().unwrap_or_default();
        let number = |value: u64| {
            #[expect(clippy::cast_precision_loss, reason = "an id, far below 2^53")]
            let value = value as f64;
            Json::Number(value)
        };
        BTreeMap::from([
            ("id", Json::Number(f64::from(stream.id))),
            ("label", Json::Text(stream.seen.label.clone())),
            ("app", Json::Text(app.name.clone())),
            ("appId", Json::Text(app.id.clone())),
            ("binary", Json::Text(app.binary.clone())),
            (
                "pid",
                app.pid
                    .map_or(Json::Number(-1.0), |pid| Json::Number(f64::from(pid))),
            ),
            ("mediaName", Json::Text(app.media.clone())),
            (
                "icon",
                Json::Text(if app.icon.is_empty() {
                    String::new()
                } else {
                    format!("image://solium/icon/{}", app.icon)
                }),
            ),
            ("volume", Json::Number(stream.shown_volume())),
            ("muted", Json::Bool(stream.seen.muted)),
            ("pending", Json::Bool(stream.pending())),
            ("step", Json::Number(self.model.settings.step)),
            ("max", Json::Number(self.model.settings.max)),
            ("state", Json::Text(stream.seen.state.clone())),
            (
                "windows",
                Json::List(
                    windows
                        .into_iter()
                        .flatten()
                        .map(|window| number(*window))
                        .collect(),
                ),
            ),
        ])
    }

    /// Each window that has a stream, its streams as one: the first stream's
    /// row, with the loudest of their volumes, `muted` only when every one
    /// is, `pending` when any is, how many there are and their ids, as
    /// `count` and `streams`, and the window's own id.
    /// `tests::a_window_s_streams_are_set_together`.
    fn window_rows(
        &self,
        streams: &[&Endpoint],
        owners: &BTreeMap<u32, Vec<u64>>,
    ) -> BTreeMap<u64, BTreeMap<&'static str, Json>> {
        let mut by_window: BTreeMap<u64, Vec<&Endpoint>> = BTreeMap::new();
        for stream in streams {
            for window in owners.get(&stream.id).into_iter().flatten() {
                by_window.entry(*window).or_default().push(stream);
            }
        }
        by_window
            .into_iter()
            .filter_map(|(window, ours)| {
                let first = ours.first()?;
                let mut row = self.stream_row(first, Some(&vec![window]));
                let loudest = ours
                    .iter()
                    .map(|stream| stream.shown_volume())
                    .fold(0.0, f64::max);
                row.insert("volume", Json::Number(loudest));
                row.insert(
                    "muted",
                    Json::Bool(ours.iter().all(|stream| stream.seen.muted)),
                );
                row.insert(
                    "pending",
                    Json::Bool(ours.iter().any(|stream| stream.pending())),
                );
                #[expect(clippy::cast_precision_loss, reason = "a count of streams")]
                let count = ours.len() as f64;
                row.insert("count", Json::Number(count));
                row.insert(
                    "streams",
                    Json::List(
                        ours.iter()
                            .map(|stream| Json::Number(f64::from(stream.id)))
                            .collect(),
                    ),
                );
                Some((window, row))
            })
            .collect()
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
        ("stream".to_owned(), Json::Bool(event.stream)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_comes_back_after_pipewire_restarts() {
        // In a test the thread never starts, so the retries can be watched.
        let mut service = Service::default();
        service.want();
        assert!(service.thread.is_none());
        service.settle(Duration::from_secs(10));
        assert_eq!(service.retry_at, Some(Duration::from_secs(11)));
        service.settle(Duration::from_secs(11));
        assert_eq!(service.retry_at, None, "tried again");
        assert_eq!(service.backoff, Duration::from_secs(2));
        service.settle(Duration::from_secs(12));
        assert_eq!(
            service.retry_at,
            Some(Duration::from_secs(14)),
            "waits longer"
        );
    }

    /// One of Firefox's playback streams, process 300.
    fn stream(node: u32, volume: f64) -> model::Seen {
        model::Seen {
            name: format!("stream#{node}"),
            label: "Firefox".to_owned(),
            direction: Direction::Output,
            kind: model::Kind::Other,
            channels: volume::channels(volume::to_linear(volume), 2),
            muted: false,
            step: None,
            route: None,
            port_label: String::new(),
            port_kind: String::new(),
            state: "running".to_owned(),
            app: Some(Box::new(model::App {
                pid: Some(300),
                family: vec![300],
                name: "Firefox".to_owned(),
                ..model::App::default()
            })),
        }
    }

    /// **A window's streams are set together** (#416): a window with two
    /// streams is one row, the louder's volume, `count` 2; `by` moves each
    /// from where it is, a toggle mutes both, and a second unmutes both; a
    /// window with none answers `unknown-endpoint`.
    #[test]
    fn a_window_s_streams_are_set_together() {
        let now = Duration::from_secs(1);
        let mut service = Service::default();
        service.want();
        service.model.apply(Fact::Seen(stream(41, 0.5)), now);
        service.model.apply(Fact::Seen(stream(42, 0.2)), now);
        service.model.apply(Fact::Ready, now);
        service.tell_windows(vec![WindowKey {
            id: 7,
            pid: Some(300),
            app_id: "org.mozilla.firefox".to_owned(),
        }]);
        service.refresh_view();
        let window = |service: &Service, key: &str| service.view().windows[&7][key].clone();
        assert_eq!(window(&service, "count"), Json::Number(2.0));
        assert!(
            matches!(window(&service, "volume"), Json::Number(volume) if (volume - 0.5).abs() < 1e-9)
        );
        assert_eq!(service.view().streams.len(), 2);
        assert!(service.view().outputs.is_empty(), "a stream is no output");

        let by = Json::parse(r#"{"window":7,"by":0.1}"#).expect("valid JSON");
        assert_eq!(service.act("audio.volume", &by, Origin::Scene, now), Ok(()));
        service.refresh_view();
        let volumes: Vec<f64> = service
            .view()
            .streams
            .iter()
            .filter_map(|row| match row.get("volume") {
                Some(Json::Number(volume)) => Some(*volume),
                _ => None,
            })
            .collect();
        assert_eq!(volumes.len(), 2);
        assert!((volumes[0] - 0.6).abs() < 1e-9 && (volumes[1] - 0.3).abs() < 1e-9);

        let toggle = Json::parse(r#"{"window":7,"value":"toggle"}"#).expect("valid JSON");
        assert_eq!(
            service.act("audio.mute", &toggle, Origin::Scene, now),
            Ok(())
        );
        assert!(service.model.all_muted(&[1, 2]));
        assert_eq!(
            service.act("audio.mute", &toggle, Origin::Scene, now),
            Ok(())
        );
        assert!(!service.model.all_muted(&[1, 2]));

        let none = Json::parse(r#"{"window":8,"by":0.1}"#).expect("valid JSON");
        assert_eq!(
            service.act("audio.volume", &none, Origin::Scene, now),
            Err("unknown-endpoint")
        );
        let bad = Json::parse(r#"{"window":"seven","by":0.1}"#).expect("valid JSON");
        assert_eq!(
            service.act("audio.volume", &bad, Origin::Scene, now),
            Err("bad-data")
        );
    }

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
