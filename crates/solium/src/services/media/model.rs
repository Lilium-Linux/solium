//! The media players, as a pure model: which players there are, what each
//! says it is playing, where it is in the track, which one is active, and
//! what an action asks of which player. No bus and no Qt: what a player said
//! comes in already read and made safe ([`Props`]), the time comes in as the
//! compositor's clock, and what must be said to a player goes out as a
//! [`Call`].

use std::{
    collections::{BTreeMap, HashMap},
    time::Duration,
};

/// `PlaybackStatus`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Status {
    #[default]
    Stopped,
    Paused,
    Playing,
}

impl Status {
    /// As the player sends it; anything else reads as `Stopped`.
    pub(crate) fn from_name(name: &str) -> Self {
        match name {
            "Playing" => Self::Playing,
            "Paused" => Self::Paused,
            _ => Self::Stopped,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Paused => "paused",
            Self::Playing => "playing",
        }
    }
}

/// `LoopStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Loop {
    None,
    Track,
    Playlist,
}

impl Loop {
    /// As the player sends it; anything else is no answer at all.
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        match name {
            "None" => Some(Self::None),
            "Track" => Some(Self::Track),
            "Playlist" => Some(Self::Playlist),
            _ => None,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Track => "track",
            Self::Playlist => "playlist",
        }
    }
}

/// The picture of the track, as the player names it in `mpris:artUrl`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Art {
    #[default]
    None,
    /// Read by Solium from a `file://` path or a `data:` URI: a digest of its
    /// pixels, never 0.
    Read(u64),
    /// An `http://` or `https://` address, handed to the scene as it is:
    /// Solium fetches nothing.
    Remote(String),
}

/// What a player says of itself, from its `org.mpris.MediaPlayer2` and
/// `org.mpris.MediaPlayer2.Player` properties, already made safe to show.
/// Its position is apart, in [`Player::position`]: it moves on its own.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Props {
    /// `Identity`: the player's own name, `mpv`, `Spotify`.
    pub(crate) identity: String,
    /// `DesktopEntry`: its `.desktop` file's name, without the suffix.
    pub(crate) desktop_entry: String,
    pub(crate) status: Status,
    /// `xesam:title`.
    pub(crate) title: String,
    /// `xesam:artist`, a list, joined with ", ".
    pub(crate) artist: String,
    /// `xesam:album`.
    pub(crate) album: String,
    pub(crate) art: Art,
    /// `mpris:trackid`: the object path `SetPosition` names the track by, or
    /// empty when the player gave none or `NoTrack`.
    pub(crate) track_id: String,
    /// `mpris:length`, in microseconds; 0 when it is not known.
    pub(crate) length: i64,
    /// `Rate`: 1.0 is normal speed.
    pub(crate) rate: f64,
    /// `Volume`, 0.0 to 1.0, when the player offers one.
    pub(crate) volume: Option<f64>,
    pub(crate) shuffle: Option<bool>,
    pub(crate) loop_status: Option<Loop>,
    pub(crate) can_play: bool,
    pub(crate) can_pause: bool,
    pub(crate) can_go_next: bool,
    pub(crate) can_go_previous: bool,
    pub(crate) can_seek: bool,
    pub(crate) can_control: bool,
}

impl Default for Props {
    fn default() -> Self {
        Self {
            identity: String::new(),
            desktop_entry: String::new(),
            status: Status::Stopped,
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            art: Art::None,
            track_id: String::new(),
            length: 0,
            rate: 1.0,
            volume: None,
            shuffle: None,
            loop_status: None,
            can_play: false,
            can_pause: false,
            can_go_next: false,
            can_go_previous: false,
            can_seek: false,
            can_control: false,
        }
    }
}

/// Where a player was in its track, and when: `Position` as it was read (or
/// as `Seeked` said), in microseconds, at a time on the compositor's clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Position {
    pub(crate) micros: i64,
    pub(crate) at: Duration,
}

/// One player on the bus.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Player {
    /// Solium's id for it, never given out twice in a run.
    pub(crate) id: u32,
    /// Its bus name, `org.mpris.MediaPlayer2.mpv`.
    pub(crate) key: String,
    pub(crate) props: Props,
    pub(crate) position: Position,
    /// How many times its art's pixels changed: a new picture is a new URL,
    /// so Qt does not show the cached old one.
    pub(crate) art_revision: u32,
    /// When it last started playing, as a count of the model's own: 0 for
    /// never. The active player is chosen by it.
    played: u64,
    /// When it appeared, the same way.
    seen: u64,
}

impl Player {
    /// Where it is now: where it was, moved on by the rate for the time since
    /// while it plays, and held within the track.
    /// `the_position_moves_on_by_the_rate_while_playing_and_stays_in_the_track`.
    pub(crate) fn micros_at(&self, now: Duration) -> i64 {
        position_at(&self.props, self.position, now)
    }

    pub(crate) fn playing(&self) -> bool {
        self.props.status == Status::Playing
    }
}

/// `position` moved on to `now` for a player that says `props`.
pub(crate) fn position_at(props: &Props, position: Position, now: Duration) -> i64 {
    let mut micros = position.micros;
    if props.status == Status::Playing && props.rate.is_finite() && props.rate > 0.0 {
        let since = now.saturating_sub(position.at).as_micros() as f64;
        micros = micros.saturating_add((since * props.rate) as i64);
    }
    if props.length > 0 {
        micros = micros.min(props.length);
    }
    micros.max(0)
}

/// What the bus is asked to do for a player.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Call {
    PlayPause,
    Play,
    Pause,
    Next,
    Previous,
    /// `Seek(offset)`, in microseconds, forward or back.
    Seek(i64),
    /// `SetPosition(track_id, position)`, in microseconds.
    SetPosition {
        track_id: String,
        micros: i64,
    },
    /// The `Volume` property set.
    Volume(f64),
}

/// What changed, for Lua's events and for the rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Change {
    Added(u32),
    Changed(u32),
    Removed(u32),
    /// Another player is active now, or none (0).
    Active(u32),
}

/// What reading a player changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Read {
    pub(crate) id: u32,
    pub(crate) changes: Vec<Change>,
    /// Its art's pixels are new: Qt is to be given them.
    pub(crate) new_art: bool,
    /// Its art is pixels no more: Qt is to let them go.
    pub(crate) dropped_art: bool,
}

/// Every player, in the order they appeared, and which is active.
#[derive(Clone, Debug, Default)]
pub(crate) struct Media {
    players: BTreeMap<u32, Player>,
    ids: HashMap<String, u32>,
    last: u32,
    /// The model's own count, for `played` and `seen`.
    stamp: u64,
    active: Option<u32>,
}

impl Media {
    /// In the order they appeared.
    pub(crate) fn players(&self) -> impl Iterator<Item = &Player> {
        self.players.values()
    }

    #[cfg(test)]
    pub(crate) fn get(&self, id: u32) -> Option<&Player> {
        self.players.get(&id)
    }

    pub(crate) fn id_of(&self, key: &str) -> Option<u32> {
        self.ids.get(key).copied()
    }

    /// The active player's id, if there is one.
    pub(crate) fn active(&self) -> Option<u32> {
        self.active
    }

    fn stamp(&mut self) -> u64 {
        self.stamp += 1;
        self.stamp
    }

    /// A player's properties, read: added the first time, changed after.
    /// `position` is where it is, when this read asked; `None` keeps the one
    /// held. A picture whose pixels changed gets a new revision.
    /// `a_player_is_added_at_its_first_read_and_changed_at_the_next`,
    /// `new_art_is_a_new_revision_and_the_same_art_is_not`.
    pub(crate) fn read(
        &mut self,
        key: &str,
        props: Props,
        position: Option<Position>,
        now: Duration,
    ) -> Read {
        let (id, added) = match self.ids.get(key) {
            Some(&id) => (id, false),
            None => {
                self.last = self.last.wrapping_add(1).max(1);
                let id = self.last;
                let seen = self.stamp();
                self.ids.insert(key.to_owned(), id);
                self.players.insert(
                    id,
                    Player {
                        id,
                        key: key.to_owned(),
                        props: Props::default(),
                        position: Position { micros: 0, at: now },
                        art_revision: 0,
                        played: 0,
                        seen,
                    },
                );
                (id, true)
            }
        };
        let mut read = Read {
            id,
            ..Read::default()
        };
        let stamp = self.stamp + 1;
        let Some(player) = self.players.get_mut(&id) else {
            return read;
        };
        match (&player.props.art, &props.art) {
            (before, Art::Read(after)) if *before != Art::Read(*after) => {
                player.art_revision = player.art_revision.wrapping_add(1);
                read.new_art = true;
            }
            (Art::Read(_), after) if !matches!(after, Art::Read(_)) => read.dropped_art = true,
            _ => {}
        }
        // With no position of its own, a read that changes how it moves (a
        // pause, a new rate) carries the old one to now first, so a pause
        // holds the place it reached; any other keeps it as it was.
        let position = match position {
            Some(position) => position,
            None if player.props.status != props.status || player.props.rate != props.rate => {
                Position {
                    micros: player.micros_at(now),
                    at: now,
                }
            }
            None => player.position,
        };
        let started = props.status == Status::Playing && player.props.status != Status::Playing;
        let changed = player.props != props || player.position != position;
        player.props = props;
        player.position = position;
        if started {
            player.played = stamp;
            self.stamp = stamp;
        }
        if added {
            read.changes.push(Change::Added(id));
        } else if changed {
            read.changes.push(Change::Changed(id));
        }
        read.changes.extend(self.choose());
        read
    }

    /// The player left the bus: its changes, if it was shown.
    /// `a_player_that_leaves_is_removed_and_the_next_one_is_active`.
    pub(crate) fn gone(&mut self, key: &str) -> Vec<Change> {
        let Some(id) = self.ids.remove(key) else {
            return Vec::new();
        };
        self.players.remove(&id);
        let mut changes = vec![Change::Removed(id)];
        changes.extend(self.choose());
        changes
    }

    /// Every player gone at once.
    pub(crate) fn clear(&mut self) -> Vec<Change> {
        self.ids.clear();
        let mut changes: Vec<Change> = self.players.keys().map(|id| Change::Removed(*id)).collect();
        self.players.clear();
        changes.extend(self.choose());
        changes
    }

    /// The active player chosen again: the one playing (of several, the one
    /// that started last), else the one that played last, else the one that
    /// appeared last, so a player opened paused on a track still shows. A
    /// change, when it is another.
    /// `the_active_player_is_the_one_playing_else_the_last_that_played`.
    fn choose(&mut self) -> Option<Change> {
        let chosen = self
            .players
            .values()
            .max_by_key(|player| (player.playing(), player.played, player.seen))
            .map(|player| player.id);
        if chosen == self.active {
            return None;
        }
        self.active = chosen;
        Some(Change::Active(chosen.unwrap_or(0)))
    }

    /// The player an action names, or the active one with no id.
    fn target(&self, id: Option<u32>) -> Result<&Player, &'static str> {
        match id {
            Some(id) => self.players.get(&id).ok_or("unknown-player"),
            None => self
                .active
                .and_then(|id| self.players.get(&id))
                .ok_or("no-player"),
        }
    }

    /// What an action asks of a player, by its key: refused with why when the
    /// player says it cannot do it (`CanControl`, `CanPlay`...), since
    /// compliance varies and a call it ignores would look like a dead key.
    /// `an_action_becomes_its_call_and_is_refused_when_the_player_cannot`,
    /// `set_position_without_a_track_id_seeks_by_the_difference`.
    pub(crate) fn act(
        &self,
        action: Action,
        id: Option<u32>,
        now: Duration,
    ) -> Result<(String, Call), &'static str> {
        let player = self.target(id)?;
        let props = &player.props;
        let allowed = |can: bool| {
            if props.can_control && can {
                Ok(())
            } else {
                Err("unsupported")
            }
        };
        let call = match action {
            Action::PlayPause => {
                allowed(props.can_play || props.can_pause)?;
                Call::PlayPause
            }
            Action::Play => {
                allowed(props.can_play)?;
                Call::Play
            }
            Action::Pause => {
                allowed(props.can_pause)?;
                Call::Pause
            }
            Action::Next => {
                allowed(props.can_go_next)?;
                Call::Next
            }
            Action::Previous => {
                allowed(props.can_go_previous)?;
                Call::Previous
            }
            Action::Seek(seconds) => {
                allowed(props.can_seek)?;
                Call::Seek(micros_of(seconds))
            }
            Action::SetPosition(seconds) => {
                allowed(props.can_seek)?;
                let mut micros = micros_of(seconds).max(0);
                if props.length > 0 {
                    micros = micros.min(props.length);
                }
                if props.track_id.is_empty() {
                    // `SetPosition` without a track is ignored by the player:
                    // the same place reached by `Seek` from where it is.
                    Call::Seek(micros - player.micros_at(now))
                } else {
                    Call::SetPosition {
                        track_id: props.track_id.clone(),
                        micros,
                    }
                }
            }
            Action::Volume(volume) => {
                let Some(current) = props.volume else {
                    return Err("unsupported");
                };
                allowed(true)?;
                let value = match volume {
                    Volume::To(value) => value,
                    Volume::By(delta) => current + delta,
                };
                Call::Volume(value.clamp(0.0, 1.0))
            }
        };
        Ok((player.key.clone(), call))
    }
}

/// What a `media.*` action asks, once its data is read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Action {
    PlayPause,
    Play,
    Pause,
    Next,
    Previous,
    /// By so many seconds, forward or back.
    Seek(f64),
    /// To so many seconds into the track.
    SetPosition(f64),
    Volume(Volume),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Volume {
    To(f64),
    By(f64),
}

/// Seconds as microseconds, within what a player can be asked.
fn micros_of(seconds: f64) -> i64 {
    // A day either way is past any track; past it is nonsense.
    const DAY: f64 = 86_400.0;
    (seconds.clamp(-DAY, DAY) * 1_000_000.0) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    const MPV: &str = "org.mpris.MediaPlayer2.mpv";
    const SPOTIFY: &str = "org.mpris.MediaPlayer2.spotify";

    fn secs(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }

    fn props(status: Status) -> Props {
        Props {
            identity: "mpv".to_owned(),
            status,
            title: "Song".to_owned(),
            track_id: "/org/mpv/Track/1".to_owned(),
            length: 200_000_000,
            can_play: true,
            can_pause: true,
            can_go_next: true,
            can_go_previous: true,
            can_seek: true,
            can_control: true,
            volume: Some(0.5),
            ..Props::default()
        }
    }

    fn at(micros: i64, at: Duration) -> Option<Position> {
        Some(Position { micros, at })
    }

    #[test]
    fn a_player_is_added_at_its_first_read_and_changed_at_the_next() {
        let mut media = Media::default();
        let first = media.read(MPV, props(Status::Paused), at(0, secs(1)), secs(1));
        assert_eq!(
            first.changes,
            [Change::Added(first.id), Change::Active(first.id)]
        );
        let same = media.read(MPV, props(Status::Paused), at(0, secs(1)), secs(2));
        assert!(same.changes.is_empty(), "nothing changed");
        let mut renamed = props(Status::Paused);
        renamed.title = "Another".to_owned();
        let changed = media.read(MPV, renamed, None, secs(3));
        assert_eq!(changed.changes, [Change::Changed(first.id)]);
        assert_eq!(media.id_of(MPV), Some(first.id));
    }

    #[test]
    fn the_position_moves_on_by_the_rate_while_playing_and_stays_in_the_track() {
        let mut media = Media::default();
        let id = media
            .read(
                MPV,
                props(Status::Playing),
                at(10_000_000, secs(5)),
                secs(5),
            )
            .id;
        let player = media.get(id).expect("a player").clone();
        assert_eq!(player.micros_at(secs(7)), 12_000_000);
        // A clock read before the position was is not a step back.
        assert_eq!(player.micros_at(secs(4)), 10_000_000);
        // Never past the end of the track.
        assert_eq!(player.micros_at(secs(1_000)), 200_000_000);
        let mut faster = props(Status::Playing);
        faster.rate = 2.0;
        media.read(MPV, faster, at(10_000_000, secs(5)), secs(5));
        assert_eq!(
            media.get(id).map(|p| p.micros_at(secs(6))),
            Some(12_000_000)
        );
        // Paused, it stays where it was paused, carried by its last rate.
        media.read(MPV, props(Status::Paused), None, secs(8));
        let paused = media.get(id).expect("a player");
        assert_eq!(paused.micros_at(secs(8)), 16_000_000);
        assert_eq!(paused.micros_at(secs(60)), 16_000_000);
        // A seek moves it at once, though nothing else changed.
        let seeked = media.read(
            MPV,
            props(Status::Paused),
            at(30_000_000, secs(61)),
            secs(61),
        );
        assert_eq!(seeked.changes, [Change::Changed(id)]);
        assert_eq!(
            media.get(id).map(|p| p.micros_at(secs(99))),
            Some(30_000_000)
        );
    }

    #[test]
    fn the_active_player_is_the_one_playing_else_the_last_that_played() {
        let mut media = Media::default();
        let mpv = media.read(MPV, props(Status::Paused), None, secs(1)).id;
        assert_eq!(media.active(), Some(mpv), "the only one, though paused");
        let spotify = media.read(SPOTIFY, props(Status::Paused), None, secs(2)).id;
        assert_eq!(media.active(), Some(spotify), "neither played: the newest");
        let playing = media.read(MPV, props(Status::Playing), None, secs(3));
        assert!(playing.changes.contains(&Change::Active(mpv)));
        assert_eq!(media.active(), Some(mpv));
        // Paused, it is still the last that played.
        media.read(MPV, props(Status::Paused), None, secs(4));
        assert_eq!(media.active(), Some(mpv));
        // Two playing: the one that started last.
        media.read(SPOTIFY, props(Status::Playing), None, secs(5));
        media.read(MPV, props(Status::Playing), None, secs(6));
        assert_eq!(media.active(), Some(mpv));
        // A change that is not a start does not take it back.
        let mut renamed = props(Status::Playing);
        renamed.title = "Next song".to_owned();
        media.read(SPOTIFY, renamed, None, secs(7));
        assert_eq!(media.active(), Some(mpv));
        media.read(MPV, props(Status::Stopped), None, secs(8));
        assert_eq!(media.active(), Some(spotify), "the one still playing");
    }

    #[test]
    fn a_player_that_leaves_is_removed_and_the_next_one_is_active() {
        let mut media = Media::default();
        let mpv = media.read(MPV, props(Status::Paused), None, secs(1)).id;
        let spotify = media
            .read(SPOTIFY, props(Status::Playing), None, secs(2))
            .id;
        assert_eq!(
            media.gone(SPOTIFY),
            [Change::Removed(spotify), Change::Active(mpv)]
        );
        assert!(media.gone(SPOTIFY).is_empty(), "gone once");
        assert_eq!(media.clear(), [Change::Removed(mpv), Change::Active(0)]);
        assert_eq!(media.active(), None);
        // A name that comes back is a new player.
        let again = media.read(SPOTIFY, props(Status::Paused), None, secs(3)).id;
        assert_ne!(again, spotify);
    }

    #[test]
    fn new_art_is_a_new_revision_and_the_same_art_is_not() {
        let mut media = Media::default();
        let mut pictured = props(Status::Playing);
        pictured.art = Art::Read(7);
        let first = media.read(MPV, pictured.clone(), None, secs(1));
        assert!(first.new_art);
        assert_eq!(media.get(first.id).map(|p| p.art_revision), Some(1));
        assert!(!media.read(MPV, pictured.clone(), None, secs(2)).new_art);
        pictured.art = Art::Read(8);
        assert!(media.read(MPV, pictured.clone(), None, secs(3)).new_art);
        assert_eq!(media.get(first.id).map(|p| p.art_revision), Some(2));
        pictured.art = Art::Remote("https://example.org/a.jpg".to_owned());
        let remote = media.read(MPV, pictured, None, secs(4));
        assert!(remote.dropped_art && !remote.new_art);
    }

    #[test]
    fn an_action_becomes_its_call_and_is_refused_when_the_player_cannot() {
        let mut media = Media::default();
        assert_eq!(
            media.act(Action::PlayPause, None, secs(0)),
            Err("no-player")
        );
        let mpv = media
            .read(MPV, props(Status::Playing), at(0, secs(0)), secs(0))
            .id;
        assert_eq!(
            media.act(Action::PlayPause, None, secs(1)),
            Ok((MPV.to_owned(), Call::PlayPause))
        );
        assert_eq!(
            media.act(Action::Next, Some(mpv), secs(1)),
            Ok((MPV.to_owned(), Call::Next))
        );
        assert_eq!(
            media.act(Action::Seek(-5.5), None, secs(1)),
            Ok((MPV.to_owned(), Call::Seek(-5_500_000)))
        );
        assert_eq!(
            media.act(Action::SetPosition(500.0), None, secs(1)),
            Ok((
                MPV.to_owned(),
                Call::SetPosition {
                    track_id: "/org/mpv/Track/1".to_owned(),
                    micros: 200_000_000,
                }
            )),
            "held to the track's length"
        );
        assert_eq!(
            media.act(Action::Volume(Volume::By(0.75)), None, secs(1)),
            Ok((MPV.to_owned(), Call::Volume(1.0)))
        );
        assert_eq!(
            media.act(Action::Volume(Volume::To(0.25)), None, secs(1)),
            Ok((MPV.to_owned(), Call::Volume(0.25)))
        );
        assert_eq!(
            media.act(Action::Play, Some(99), secs(1)),
            Err("unknown-player")
        );
        let mut limited = props(Status::Playing);
        limited.can_go_next = false;
        limited.volume = None;
        media.read(MPV, limited.clone(), None, secs(2));
        assert_eq!(media.act(Action::Next, None, secs(2)), Err("unsupported"));
        assert_eq!(
            media.act(Action::Volume(Volume::To(1.0)), None, secs(2)),
            Err("unsupported")
        );
        limited.can_control = false;
        media.read(MPV, limited, None, secs(3));
        assert_eq!(
            media.act(Action::PlayPause, None, secs(3)),
            Err("unsupported"),
            "a player that cannot be controlled is not"
        );
    }

    #[test]
    fn set_position_without_a_track_id_seeks_by_the_difference() {
        let mut media = Media::default();
        let mut untracked = props(Status::Playing);
        untracked.track_id = String::new();
        media.read(MPV, untracked, at(10_000_000, secs(0)), secs(0));
        assert_eq!(
            media.act(Action::SetPosition(5.0), None, secs(2)),
            Ok((MPV.to_owned(), Call::Seek(-7_000_000)))
        );
    }
}
