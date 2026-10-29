//! Media (#379): the MPRIS players on the session bus (mpv, Spotify,
//! Firefox...), what each is playing and where it is in the track, and the
//! active one, for scenes (`Media`) and for Lua (`sol.media`,
//! `sol.on("media")`, `media.*` actions), so a shell shows what is playing
//! and its media keys need no `playerctl`.
//!
//! * [`content`] makes what a player sends safe to show: its text and its
//!   album art;
//! * [`model`] is the players, the active one and their positions, as a pure
//!   model;
//! * [`server`] is the bus, on threads of its own;
//! * this module holds them together for the event loop: it starts the
//!   server when the configuration asks and the session allows, applies what
//!   the server read, hands art to Qt, and turns what a scene or Lua asks
//!   into calls. `crate::services::settle` drives it.

pub(crate) mod content;
pub(crate) mod model;
pub(crate) mod server;

use std::{collections::BTreeMap, rc::Rc, time::Duration};

use crate::{json::Json, services::notifications::icon_name, session::Place};

use model::{Action, Art, Change, Media, Player, Position, Volume};
use server::{Fact, Server};

/// What the configuration says: `sol.media{ ... }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Config {
    /// Follow the session's players.
    pub(crate) watch: bool,
    /// Players left out, by the part of their name after
    /// `org.mpris.MediaPlayer2.`: a name that is one of these, or starts
    /// with one and a dot. `playerctld` repeats whichever player it last
    /// saw, so it is left out by default.
    pub(crate) ignore: Vec<String>,
    /// How often a scene's `position` moves on while a player plays, in
    /// milliseconds; 0 moves it only when the player says where it is.
    pub(crate) position_interval_ms: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            watch: false,
            ignore: vec!["playerctld".to_owned()],
            position_interval_ms: 1000,
        }
    }
}

impl Config {
    /// Whether the player at bus name `key` is left out.
    /// `an_ignored_player_is_left_out_by_its_name_or_its_instances`.
    pub(crate) fn ignores(&self, key: &str) -> bool {
        let Some(name) = key.strip_prefix(server::PREFIX) else {
            return true;
        };
        self.ignore.iter().any(|ignored| {
            name == ignored
                || name
                    .strip_prefix(ignored.as_str())
                    .is_some_and(|rest| rest.starts_with('.'))
        })
    }
}

/// Which bus the players are followed on, as `session::speaks` decides: a
/// nested run never follows the desktop's players unless it is given a bus
/// of its own, or its media keys would play and pause the desktop's music.
/// `a_nested_run_follows_no_players_unless_it_is_given_a_bus`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Bus {
    #[default]
    Nowhere,
    On(Option<String>),
}

/// What snapshots and scenes read.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct View {
    pub(crate) available: bool,
    /// The active player's id, or 0.
    pub(crate) active: u32,
    /// `position_interval_ms`, for the scenes.
    pub(crate) position_interval_ms: u32,
    /// In the order they appeared.
    pub(crate) players: Vec<BTreeMap<&'static str, Json>>,
}

/// What `sol.media()` reads: the view, and the time on the compositor's
/// clock, so a playing player's position is where it is now.
#[derive(Clone, Debug, Default)]
pub(crate) struct Seen {
    pub(crate) view: Rc<View>,
    pub(crate) now: Duration,
}

impl Seen {
    /// A player's row, its `position` moved on to now.
    pub(crate) fn player_json(&self, row: &BTreeMap<&'static str, Json>) -> Json {
        let mut player = crate::services::notifications::object(row);
        if let Json::Object(fields) = &mut player {
            fields.insert(
                "position".to_owned(),
                Json::Number(position_now(row, self.now)),
            );
        }
        player
    }

    /// The whole of it as one value: `available`, `players` and `active`.
    /// `sol_media_answers_its_players_with_their_positions_now`.
    pub(crate) fn to_json(&self) -> Json {
        let players: Vec<Json> = self
            .view
            .players
            .iter()
            .map(|row| self.player_json(row))
            .collect();
        let active = self
            .view
            .players
            .iter()
            .find(|row| {
                self.view.active != 0
                    && row.get("id").and_then(Json::as_u64) == Some(u64::from(self.view.active))
            })
            .map_or(Json::Null, |row| self.player_json(row));
        Json::Object(BTreeMap::from([
            ("available".to_owned(), Json::Bool(self.view.available)),
            ("players".to_owned(), Json::List(players)),
            ("active".to_owned(), active),
        ]))
    }
}

/// A row's position, in seconds, moved on to `now` as the model moves it.
fn position_now(row: &BTreeMap<&'static str, Json>, now: Duration) -> f64 {
    let number = |key: &str| match row.get(key) {
        Some(Json::Number(n)) => *n,
        _ => 0.0,
    };
    let props = model::Props {
        status: if row.get("playing") == Some(&Json::Bool(true)) {
            model::Status::Playing
        } else {
            model::Status::Paused
        },
        rate: number("rate"),
        length: (number("length") * 1e6) as i64,
        ..model::Props::default()
    };
    let position = Position {
        micros: (number("position") * 1e6) as i64,
        at: Duration::from_millis(number("positionAt").max(0.0) as u64),
    };
    model::position_at(&props, position, now) as f64 / 1e6
}

/// One event for `sol.on("media")`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Event {
    /// `added`, `changed`, `removed` or `active`.
    pub(crate) what: &'static str,
    /// The player; for `active`, the one active now, or 0 for none.
    pub(crate) id: u32,
}

impl Event {
    pub(crate) fn to_json(&self, seen: &Seen) -> Json {
        let mut fields = BTreeMap::from([
            ("what".to_owned(), Json::Text(self.what.to_owned())),
            ("id".to_owned(), Json::Number(f64::from(self.id))),
        ]);
        if let Some(row) = seen
            .view
            .players
            .iter()
            .find(|row| row.get("id").and_then(Json::as_u64) == Some(u64::from(self.id)))
        {
            fields.insert("player".to_owned(), seen.player_json(row));
        }
        Json::Object(fields)
    }
}

/// The most facts taken from the server in one settle: a flood waits for the
/// next loops rather than freezing this one.
const FACTS_PER_SETTLE: usize = 64;

/// The service, owned by `Solium`.
#[derive(Debug, Default)]
pub(crate) struct Service {
    bus: Bus,
    config: Config,
    server: Option<Server>,
    pub(crate) model: Media,
    view: Rc<View>,
    dirty: bool,
    /// For `sol.on("media")`, in order, drained each settle.
    pub(crate) events: Vec<Event>,
}

impl Service {
    /// The backend has started: which bus the players are followed on.
    pub(crate) fn permit(&mut self, place: Place, bus: Option<String>) {
        self.bus = if crate::session::speaks(place, bus.as_deref()) {
            Bus::On(bus)
        } else {
            if self.config.watch {
                tracing::info!(
                    ?place,
                    "not started as the session: the desktop's media players are left to it. \
                     SOLIUM_SESSION_BUS names a bus to follow players on instead"
                );
            }
            Bus::Nowhere
        };
        self.start_or_stop();
    }

    /// `sol.media{ ... }`, at load and on every reload. A changed `ignore`
    /// starts the following again, so a player no longer left out is found.
    pub(crate) fn configure(&mut self, config: Config) {
        let ignore_changed = config.ignore != self.config.ignore;
        self.config = config;
        if ignore_changed && self.server.is_some() {
            self.server = None;
            self.clear();
        }
        self.start_or_stop();
        self.dirty = true;
    }

    fn start_or_stop(&mut self) {
        match (&self.bus, self.config.watch, self.server.is_some()) {
            (Bus::On(address), true, false) => {
                self.server = Server::start(address.clone());
                self.dirty = true;
            }
            (_, false, true) | (Bus::Nowhere, _, true) => {
                self.server = None;
                self.clear();
            }
            _ => {}
        }
    }

    pub(crate) fn view(&self) -> Rc<View> {
        self.view.clone()
    }

    /// The view and the time, for `sol.media()` and events.
    pub(crate) fn seen(&self, now: Duration) -> Seen {
        Seen {
            view: self.view.clone(),
            now,
        }
    }

    /// Whether the server is running at all.
    #[cfg(test)]
    pub(crate) fn available(&self) -> bool {
        self.server.is_some()
    }

    /// What the server heard since the last settle, applied at `now` on the
    /// compositor's clock.
    pub(crate) fn settle(&mut self, now: Duration) {
        let facts: Vec<Fact> = self
            .server
            .as_ref()
            .map(|server| server.facts.try_iter().take(FACTS_PER_SETTLE).collect())
            .unwrap_or_default();
        for fact in facts {
            self.apply(fact, now);
        }
    }

    /// One fact from the server, at `now`.
    pub(crate) fn apply(&mut self, fact: Fact, now: Duration) {
        match fact {
            Fact::Read(read) => {
                let server::ReadPlayer {
                    key,
                    props,
                    position,
                    art,
                } = *read;
                if self.config.ignores(&key) {
                    return;
                }
                // When it was read, on the compositor's clock: a fact that
                // waited a loop or two is not that much further on.
                let position = position.map(|(micros, read)| Position {
                    micros,
                    at: now.saturating_sub(read.elapsed()),
                });
                let outcome = self.model.read(&key, props, position, now);
                let id = outcome.id;
                if outcome.new_art
                    && let Some(image) = &art
                {
                    crate::qml::hosted::set_media_art(id, image);
                } else if outcome.dropped_art {
                    crate::qml::hosted::drop_media_art(id);
                }
                self.changed(outcome.changes);
            }
            Fact::Gone { key } => {
                if let Some(id) = self.model.id_of(&key) {
                    crate::qml::hosted::drop_media_art(id);
                }
                let changes = self.model.gone(&key);
                self.changed(changes);
            }
            // The connection went away: no players until a reload starts it
            // again.
            Fact::Down => {
                self.server = None;
                self.clear();
            }
        }
    }

    /// Every player gone.
    fn clear(&mut self) {
        for player in self.model.players() {
            crate::qml::hosted::drop_media_art(player.id);
        }
        let changes = self.model.clear();
        self.changed(changes);
    }

    fn changed(&mut self, changes: Vec<Change>) {
        if changes.is_empty() {
            return;
        }
        self.dirty = true;
        for change in changes {
            let (what, id) = match change {
                Change::Added(id) => ("added", id),
                Change::Changed(id) => ("changed", id),
                Change::Removed(id) => ("removed", id),
                Change::Active(id) => ("active", id),
            };
            self.events.push(Event { what, id });
        }
    }

    /// A `media.*` action from Lua or a scene, done at `now`: the player its
    /// `id` names, or the active one. Returns why it could not be done.
    /// `a_media_action_reads_its_data_and_answers_why_it_could_not`.
    pub(crate) fn act(
        &mut self,
        action: &str,
        data: &Json,
        now: Duration,
    ) -> Result<(), &'static str> {
        let number = |key: &str| -> Option<f64> {
            match data.get(key) {
                Some(Json::Number(n)) if n.is_finite() => Some(*n),
                _ => None,
            }
        };
        let id = match data.get("id") {
            None | Some(Json::Null) => None,
            Some(id) => Some(
                id.as_u64()
                    .and_then(|id| u32::try_from(id).ok())
                    .ok_or("bad-data")?,
            ),
        };
        let action = match action {
            "media.play_pause" => Action::PlayPause,
            "media.play" => Action::Play,
            "media.pause" => Action::Pause,
            "media.next" => Action::Next,
            "media.previous" => Action::Previous,
            "media.seek" => Action::Seek(number("offset").ok_or("bad-data")?),
            "media.set_position" => Action::SetPosition(number("position").ok_or("bad-data")?),
            "media.volume" => Action::Volume(match (number("value"), number("by")) {
                (Some(value), None) => Volume::To(value),
                (None, Some(by)) => Volume::By(by),
                _ => return Err("bad-data"),
            }),
            _ => return Err("unknown-action"),
        };
        let Some(server) = &self.server else {
            return Err("unavailable");
        };
        let (key, call) = self.model.act(action, id, now)?;
        server.call(&key, call);
        Ok(())
    }

    /// The view rebuilt, if anything changed since it was last built: true
    /// when it was.
    pub(crate) fn refresh_view(&mut self) -> bool {
        if !self.dirty {
            return false;
        }
        self.dirty = false;
        let active = self.model.active();
        let view = View {
            available: self.server.is_some(),
            active: active.unwrap_or(0),
            position_interval_ms: self.config.position_interval_ms,
            players: self
                .model
                .players()
                .map(|player| row(player, Some(player.id) == active))
                .collect(),
        };
        if *self.view == view {
            return false;
        }
        self.view = Rc::new(view);
        true
    }
}

/// One player as a row: what a scene's delegate and a Lua handler read.
/// Times are in seconds; `position` is where it was at `positionAt`, a time
/// in milliseconds on the compositor's clock, and moves on at `rate` while
/// `playing`.
/// `a_player_s_row_names_its_art_its_times_in_seconds_and_whether_it_is_active`.
fn row(player: &Player, active: bool) -> BTreeMap<&'static str, Json> {
    let props = &player.props;
    let (art, art_url) = match &props.art {
        Art::Read(_) => (
            format!("image://solium/media/{}/{}", player.id, player.art_revision),
            String::new(),
        ),
        Art::Remote(url) => (String::new(), url.clone()),
        Art::None => (String::new(), String::new()),
    };
    let entry = icon_name(&props.desktop_entry);
    let icon = if entry.is_empty() {
        String::new()
    } else {
        format!("image://solium/icon/{entry}")
    };
    let seconds = |micros: i64| Json::Number(micros as f64 / 1e6);
    let optional = |value: Option<Json>| value.unwrap_or(Json::Null);
    BTreeMap::from([
        ("id", Json::Number(f64::from(player.id))),
        ("service", Json::Text(player.key.clone())),
        ("identity", Json::Text(props.identity.clone())),
        ("desktopEntry", Json::Text(props.desktop_entry.clone())),
        ("icon", Json::Text(icon)),
        ("status", Json::Text(props.status.name().to_owned())),
        ("playing", Json::Bool(player.playing())),
        ("active", Json::Bool(active)),
        ("title", Json::Text(props.title.clone())),
        ("artist", Json::Text(props.artist.clone())),
        ("album", Json::Text(props.album.clone())),
        ("art", Json::Text(art)),
        ("artUrl", Json::Text(art_url)),
        ("trackId", Json::Text(props.track_id.clone())),
        ("length", seconds(props.length)),
        ("position", seconds(player.position.micros)),
        (
            "positionAt",
            Json::Number(player.position.at.as_millis() as f64),
        ),
        ("rate", Json::Number(props.rate)),
        ("volume", optional(props.volume.map(Json::Number))),
        ("shuffle", optional(props.shuffle.map(Json::Bool))),
        (
            "loopStatus",
            Json::Text(props.loop_status.map_or("", model::Loop::name).to_owned()),
        ),
        ("canPlay", Json::Bool(props.can_control && props.can_play)),
        ("canPause", Json::Bool(props.can_control && props.can_pause)),
        (
            "canGoNext",
            Json::Bool(props.can_control && props.can_go_next),
        ),
        (
            "canGoPrevious",
            Json::Bool(props.can_control && props.can_go_previous),
        ),
        ("canSeek", Json::Bool(props.can_control && props.can_seek)),
        ("canControl", Json::Bool(props.can_control)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::{Props, Status};
    use std::time::Instant;

    const MPV: &str = "org.mpris.MediaPlayer2.mpv";

    fn mpv(status: Status) -> Props {
        Props {
            identity: "mpv".to_owned(),
            desktop_entry: "mpv".to_owned(),
            status,
            title: "Song".to_owned(),
            artist: "Band".to_owned(),
            length: 180_000_000,
            can_control: true,
            can_play: true,
            can_pause: true,
            can_seek: true,
            volume: Some(0.5),
            ..Props::default()
        }
    }

    fn read(service: &mut Service, key: &str, props: Props, position: Option<i64>, now: Duration) {
        service.apply(
            Fact::Read(Box::new(server::ReadPlayer {
                key: key.to_owned(),
                props,
                position: position.map(|micros| (micros, Instant::now())),
                art: None,
            })),
            now,
        );
    }

    fn data(fields: &[(&str, Json)]) -> Json {
        Json::Object(
            fields
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
        )
    }

    #[test]
    fn a_nested_run_follows_no_players_unless_it_is_given_a_bus() {
        let mut service = Service::default();
        service.configure(Config {
            watch: true,
            ..Config::default()
        });
        service.permit(Place::Nested, None);
        assert!(!service.available());
        assert_eq!(service.bus, Bus::Nowhere);
        assert_eq!(
            service.act("media.play_pause", &Json::Null, Duration::ZERO),
            Err("unavailable")
        );
    }

    #[test]
    fn an_ignored_player_is_left_out_by_its_name_or_its_instances() {
        let config = Config::default();
        assert!(config.ignores("org.mpris.MediaPlayer2.playerctld"));
        assert!(config.ignores("org.mpris.MediaPlayer2.playerctld.instance2"));
        assert!(!config.ignores("org.mpris.MediaPlayer2.playerctldx"));
        assert!(!config.ignores(MPV));
        assert!(config.ignores("org.example.NotAPlayer"));
        let mut service = Service::default();
        service.configure(Config {
            ignore: vec!["mpv".to_owned()],
            ..Config::default()
        });
        read(
            &mut service,
            MPV,
            mpv(Status::Playing),
            None,
            Duration::ZERO,
        );
        read(
            &mut service,
            "org.mpris.MediaPlayer2.mpv.instance7",
            mpv(Status::Playing),
            None,
            Duration::ZERO,
        );
        assert_eq!(service.model.players().count(), 0);
    }

    #[test]
    fn a_player_s_row_names_its_art_its_times_in_seconds_and_whether_it_is_active() {
        let mut service = Service::default();
        let mut props = mpv(Status::Playing);
        props.art = Art::Remote("https://i.scdn.co/image/ab67".to_owned());
        props.loop_status = Some(model::Loop::Track);
        read(
            &mut service,
            MPV,
            props,
            Some(30_000_000),
            Duration::from_secs(10),
        );
        assert!(service.refresh_view());
        let view = service.view.clone();
        let row = &view.players[0];
        let id = row.get("id").and_then(Json::as_u64).unwrap_or(0);
        assert_eq!(view.active as u64, id);
        assert_eq!(row.get("active"), Some(&Json::Bool(true)));
        assert_eq!(row.get("art"), Some(&Json::Text(String::new())));
        assert_eq!(
            row.get("artUrl"),
            Some(&Json::Text("https://i.scdn.co/image/ab67".to_owned()))
        );
        assert_eq!(row.get("length"), Some(&Json::Number(180.0)));
        assert_eq!(row.get("loopStatus"), Some(&Json::Text("track".to_owned())));
        assert_eq!(row.get("shuffle"), Some(&Json::Null));
        assert_eq!(row.get("canGoNext"), Some(&Json::Bool(false)));
        assert_eq!(
            row.get("icon"),
            Some(&Json::Text("image://solium/icon/mpv".to_owned()))
        );
        let position = row.get("position").cloned();
        assert!(
            matches!(position, Some(Json::Number(n)) if (n - 30.0).abs() < 0.1),
            "{position:?}"
        );
        // Read art becomes the image route, with its revision.
        let mut pictured = mpv(Status::Playing);
        pictured.art = Art::Read(42);
        read(&mut service, MPV, pictured, None, Duration::from_secs(11));
        assert!(service.refresh_view());
        assert_eq!(
            service.view.players[0].get("art"),
            Some(&Json::Text(format!("image://solium/media/{id}/1")))
        );
    }

    #[test]
    fn sol_media_answers_its_players_with_their_positions_now() {
        let mut service = Service::default();
        read(
            &mut service,
            MPV,
            mpv(Status::Playing),
            Some(30_000_000),
            Duration::from_secs(10),
        );
        service.refresh_view();
        let seen = service.seen(Duration::from_secs(15));
        let json = seen.to_json();
        let active = json.get("active").cloned().unwrap_or(Json::Null);
        let position = active.get("position").cloned();
        assert!(
            matches!(position, Some(Json::Number(n)) if (n - 35.0).abs() < 0.1),
            "five seconds on: {position:?}"
        );
        assert_eq!(active.get("title"), Some(&Json::Text("Song".to_owned())));
        let Some(Json::List(players)) = json.get("players") else {
            panic!("a list of players");
        };
        assert_eq!(players.len(), 1);
        let events: Vec<&str> = service.events.iter().map(|event| event.what).collect();
        assert_eq!(events, ["added", "active"]);
        let event = service.events[0].to_json(&seen);
        assert_eq!(
            event.get("player").and_then(|p| p.get("identity")),
            Some(&Json::Text("mpv".to_owned()))
        );
    }

    #[test]
    fn a_player_that_leaves_is_removed_with_its_events_and_another_becomes_active() {
        let mut service = Service::default();
        read(&mut service, MPV, mpv(Status::Paused), None, Duration::ZERO);
        read(
            &mut service,
            "org.mpris.MediaPlayer2.spotify",
            mpv(Status::Playing),
            None,
            Duration::ZERO,
        );
        service.events.clear();
        service.apply(
            Fact::Gone {
                key: "org.mpris.MediaPlayer2.spotify".to_owned(),
            },
            Duration::ZERO,
        );
        let events: Vec<(&str, u32)> = service
            .events
            .iter()
            .map(|event| (event.what, event.id))
            .collect();
        let mpv = service.model.id_of(MPV).unwrap_or(0);
        assert_eq!(events, [("removed", 2), ("active", mpv)]);
        service.apply(Fact::Down, Duration::ZERO);
        assert_eq!(service.model.players().count(), 0);
    }

    #[test]
    fn a_media_action_reads_its_data_and_answers_why_it_could_not() {
        let mut service = Service::default();
        let now = Duration::ZERO;
        assert_eq!(service.act("media.seek", &data(&[]), now), Err("bad-data"));
        assert_eq!(
            service.act(
                "media.volume",
                &data(&[("value", Json::Number(1.0)), ("by", Json::Number(0.1))]),
                now
            ),
            Err("bad-data")
        );
        assert_eq!(
            service.act(
                "media.play",
                &data(&[("id", Json::Text("x".to_owned()))]),
                now
            ),
            Err("bad-data")
        );
        assert_eq!(
            service.act("media.dance", &data(&[]), now),
            Err("unknown-action")
        );
        assert_eq!(
            service.act("media.play_pause", &Json::Null, now),
            Err("unavailable"),
            "nothing is followed"
        );
    }
}
