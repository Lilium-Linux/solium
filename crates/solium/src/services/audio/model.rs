//! Audio's endpoints as a pure model (03 §3.2.5): what the PipeWire thread
//! saw, as [`Fact`]s, in; rows for scenes, events for Lua and [`Command`]s for
//! the PipeWire thread, out. Time is passed in; nothing here talks to
//! PipeWire.

use std::{collections::BTreeMap, time::Duration};

use super::volume::{self, Origin, Target, Write, Writer};

/// A sink or a source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Direction {
    Output,
    Input,
}

/// What kind of thing it is, for an icon and for a policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Speakers,
    Headphones,
    Headset,
    Hdmi,
    Bluetooth,
    Usb,
    Virtual,
    Other,
}

impl Kind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Speakers => "speakers",
            Self::Headphones => "headphones",
            Self::Headset => "headset",
            Self::Hdmi => "hdmi",
            Self::Bluetooth => "bluetooth",
            Self::Usb => "usb",
            Self::Virtual => "virtual",
            Self::Other => "other",
        }
    }

    /// From the node's and its device's properties: `device.form-factor`,
    /// `device.bus`, the active port's type, and whether the node is
    /// virtual. The port says more than the card: a laptop's one card has
    /// speakers and a headphone jack.
    /// `the_kind_comes_from_the_port_then_the_bus_then_the_form_factor`.
    pub(crate) fn from_props(
        form_factor: &str,
        bus: &str,
        port_type: &str,
        is_virtual: bool,
    ) -> Self {
        if is_virtual {
            return Self::Virtual;
        }
        match port_type {
            "headphones" => return Self::Headphones,
            "headset" => return Self::Headset,
            "hdmi" | "displayport" => return Self::Hdmi,
            "speaker" => return Self::Speakers,
            _ => {}
        }
        match bus {
            "bluetooth" => return Self::Bluetooth,
            "usb" => return Self::Usb,
            _ => {}
        }
        match form_factor {
            "headphone" => Self::Headphones,
            "headset" | "hands-free" | "handset" => Self::Headset,
            "speaker" | "internal" | "computer" => Self::Speakers,
            "tv" | "hdmi" => Self::Hdmi,
            _ => Self::Other,
        }
    }

    /// A theme icon for it.
    pub(crate) fn icon(self, direction: Direction) -> &'static str {
        match (self, direction) {
            (Self::Headphones, _) => "audio-headphones",
            (Self::Headset, _) => "audio-headset",
            (Self::Bluetooth, Direction::Output) => "audio-headphones",
            (_, Direction::Input) => "audio-input-microphone",
            (Self::Hdmi, Direction::Output) => "video-display",
            _ => "audio-speakers",
        }
    }
}

/// Which application one playback stream is, as its node's properties and
/// the kernel say (#416).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct App {
    /// Its process: the stream's `pipewire.sec.pid`, which PipeWire took
    /// from the socket and so is the host's even for a sandboxed client,
    /// else its `application.process.id`. A native PipeWire client's stream
    /// says neither, so then its client's (`client.id`): the client's
    /// `pipewire.sec.pid`, else its `application.process.id`. A stream's own
    /// always comes first, as a PulseAudio client's client is
    /// `pipewire-pulse`, whose socket is not the application's.
    pub(crate) pid: Option<u32>,
    /// That process and the ones that started it, nearest first
    /// (`state::ancestry`): what a window's process is looked for in, so a
    /// browser whose sound is played by a helper it started is matched to
    /// the browser's windows.
    pub(crate) family: Vec<u32>,
    /// `application.name`.
    pub(crate) name: String,
    /// `pipewire.access.portal.app_id` for a sandboxed client, else
    /// `application.id`.
    pub(crate) id: String,
    /// `application.process.binary`.
    pub(crate) binary: String,
    /// `media.name`: what it plays, as the application calls it.
    pub(crate) media: String,
    /// `application.icon_name`, else `media.icon_name`.
    pub(crate) icon: String,
}

impl App {
    /// Whether a window's application id names this application: equal to
    /// its id, its binary or its name, or the last part of a reverse-domain
    /// id (`org.mozilla.firefox`) equal to one of them, letter case aside.
    /// `tests::a_window_is_matched_to_its_stream_by_pid_then_by_app`.
    fn named_by(&self, app_id: &str) -> bool {
        let app_id = app_id.trim();
        if app_id.is_empty() {
            return false;
        }
        let last = app_id.rsplit('.').next().unwrap_or(app_id);
        [&self.id, &self.binary, &self.name]
            .into_iter()
            .filter(|ours| !ours.is_empty())
            .any(|ours| ours.eq_ignore_ascii_case(app_id) || ours.eq_ignore_ascii_case(last))
    }
}

/// What a window is matched to its streams by: its id, its process and its
/// application id, as `sol.windows()` gives them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WindowKey {
    pub(crate) id: u64,
    pub(crate) pid: Option<u32>,
    pub(crate) app_id: String,
}

/// What the PipeWire thread saw of one endpoint: a sink or a source node,
/// with its device's route when it has one; or an application's playback
/// stream, with `app` saying whose.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Seen {
    /// `node.name`: what identifies it across replugs and restarts.
    pub(crate) name: String,
    /// `node.nick`, else `node.description`.
    pub(crate) label: String,
    pub(crate) direction: Direction,
    pub(crate) kind: Kind,
    /// Linear, one per channel.
    pub(crate) channels: Vec<f32>,
    pub(crate) muted: bool,
    /// The route's volume step, linear, when it says.
    pub(crate) step: Option<f64>,
    /// The card's route (device id, route index), or none.
    pub(crate) route: Option<(u32, i32)>,
    /// The active port's description and type.
    pub(crate) port_label: String,
    pub(crate) port_kind: String,
    /// `running`, `idle`, `suspended` or `error`.
    pub(crate) state: String,
    /// The application, for a playback stream (`Stream/Output/Audio`);
    /// `None` for a sink or a source (#416). Boxed, as most of what is seen
    /// is a sink or a source, and every `Fact` would carry its size.
    pub(crate) app: Option<Box<App>>,
}

/// What the PipeWire thread tells the event loop.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Fact {
    /// An endpoint appeared or changed.
    Seen(Seen),
    /// It went, by `node.name`.
    Gone(String),
    /// The effective defaults, by `node.name`.
    Defaults {
        output: Option<String>,
        input: Option<String>,
    },
    /// Everything the first round trip had: the model may be shown.
    Ready,
    /// The connection is gone.
    Down,
}

/// What the event loop asks the PipeWire thread to do.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Command {
    Volume {
        name: String,
        target: Target,
        /// Linear, one per channel.
        channels: Vec<f32>,
    },
    Mute {
        name: String,
        target: Target,
        muted: bool,
    },
    Default {
        direction: Direction,
        name: String,
    },
}

/// One endpoint as the model holds it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Endpoint {
    /// Solium's id, the same for a `node.name` for the whole run.
    pub(crate) id: u32,
    pub(crate) seen: Seen,
    /// Perceptual, as last observed.
    pub(crate) volume: f64,
    writer: Writer,
}

impl Endpoint {
    /// What a scene reads: the newest request while one is pending, so a
    /// slider stays under the finger.
    pub(crate) fn shown_volume(&self) -> f64 {
        self.writer.shown().unwrap_or(self.volume)
    }

    pub(crate) fn pending(&self) -> bool {
        self.writer.pending()
    }

    /// Whether it is an application's playback stream rather than a sink or
    /// a source.
    pub(crate) const fn is_stream(&self) -> bool {
        self.seen.app.is_some()
    }
}

/// An event for `sol.on("audio")`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Event {
    pub(crate) what: &'static str,
    pub(crate) id: u32,
    pub(crate) direction: Direction,
    /// Whether it is the default output.
    pub(crate) output: bool,
    pub(crate) value: f64,
    pub(crate) muted: bool,
    pub(crate) origin: Origin,
    pub(crate) bus: Kind,
    /// Whether it is an application's playback stream (#416).
    pub(crate) stream: bool,
}

/// What the configuration says: `sol.audio{ max, step }`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Settings {
    pub(crate) max: f64,
    pub(crate) step: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max: 1.0,
            step: 0.05,
        }
    }
}

/// Which endpoint an action names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Which {
    Output,
    Input,
    Id(u32),
}

/// Every endpoint, and the defaults.
#[derive(Clone, Debug, Default)]
pub(crate) struct Audio {
    endpoints: BTreeMap<String, Endpoint>,
    ids: BTreeMap<String, u32>,
    last_id: u32,
    output: Option<String>,
    input: Option<String>,
    /// Mute changes Solium asked for, by name, with their origin and when.
    mutes: BTreeMap<String, (bool, Origin, Duration)>,
    pub(crate) settings: Settings,
    pub(crate) ready: bool,
}

impl Audio {
    fn id_for(&mut self, name: &str) -> u32 {
        if let Some(id) = self.ids.get(name) {
            return *id;
        }
        self.last_id += 1;
        self.ids.insert(name.to_owned(), self.last_id);
        self.last_id
    }

    /// Every sink, source and playback stream.
    pub(crate) fn endpoints(&self) -> impl Iterator<Item = &Endpoint> {
        self.endpoints.values()
    }

    /// Whether any application is playing through a stream now (#416).
    pub(crate) fn has_streams(&self) -> bool {
        self.endpoints.values().any(Endpoint::is_stream)
    }

    /// Which windows each playback stream is, by the stream's id (#416).
    ///
    /// By process first: the stream's own, then each that started it, the
    /// nearest first, and the first that is a window's process gives the
    /// stream to every window of that process. So a browser whose sound is
    /// played by a helper it started is the browser's, and a player started
    /// from a terminal, which has no window of its own, is the terminal's.
    /// A stream no window's process is in the family of is given by its
    /// application to the windows whose application id names it
    /// ([`App::named_by`]), for a client whose process the stream does not
    /// say. A stream may be no window's, and a window may have several.
    /// `tests::a_window_is_matched_to_its_stream_by_pid_then_by_app`.
    pub(crate) fn windows_of_streams(&self, windows: &[WindowKey]) -> BTreeMap<u32, Vec<u64>> {
        let mut owned = BTreeMap::new();
        for endpoint in self.endpoints.values() {
            let Some(app) = &endpoint.seen.app else {
                continue;
            };
            let family = app.pid.filter(|_| app.family.is_empty()).into_iter();
            let by_process = family.chain(app.family.iter().copied()).find_map(|pid| {
                let ids: Vec<u64> = windows
                    .iter()
                    .filter(|window| window.pid == Some(pid))
                    .map(|window| window.id)
                    .collect();
                (!ids.is_empty()).then_some(ids)
            });
            let ids = by_process.unwrap_or_else(|| {
                windows
                    .iter()
                    .filter(|window| app.named_by(&window.app_id))
                    .map(|window| window.id)
                    .collect()
            });
            owned.insert(endpoint.id, ids);
        }
        owned
    }

    pub(crate) fn default_of(&self, direction: Direction) -> Option<&Endpoint> {
        let name = match direction {
            Direction::Output => self.output.as_ref(),
            Direction::Input => self.input.as_ref(),
        }?;
        self.endpoints.get(name)
    }

    pub(crate) fn is_default(&self, endpoint: &Endpoint) -> bool {
        let name = Some(&endpoint.seen.name);
        match endpoint.seen.direction {
            Direction::Output => self.output.as_ref() == name,
            Direction::Input => self.input.as_ref() == name,
        }
    }

    fn resolve(&self, which: &Which) -> Option<&Endpoint> {
        match which {
            Which::Output => self.default_of(Direction::Output),
            Which::Input => self.default_of(Direction::Input),
            Which::Id(id) => self.endpoints.values().find(|each| each.id == *id),
        }
    }

    /// One fact applied: the events it makes, and any write that was waiting
    /// on an echo it carried.
    /// `an_echo_of_a_key_s_change_is_told_as_the_key_s`,
    /// `an_endpoint_keeps_its_id_across_a_replug`.
    pub(crate) fn apply(&mut self, fact: Fact, now: Duration) -> (Vec<Event>, Vec<Command>) {
        let mut events = Vec::new();
        let mut commands = Vec::new();
        match fact {
            Fact::Seen(seen) => {
                let name = seen.name.clone();
                let id = self.id_for(&name);
                let observed = volume::of_channels(&seen.channels);
                let muted = seen.muted;
                let direction = seen.direction;
                let kind = seen.kind;
                let stream = seen.app.is_some();
                match self.endpoints.get_mut(&name) {
                    None => {
                        self.endpoints.insert(
                            name.clone(),
                            Endpoint {
                                id,
                                seen,
                                volume: observed,
                                writer: Writer::default(),
                            },
                        );
                        events.push(Event {
                            what: "added",
                            id,
                            direction,
                            output: false,
                            value: observed,
                            muted,
                            origin: Origin::External,
                            bus: kind,
                            stream,
                        });
                    }
                    Some(endpoint) => {
                        let was_volume = endpoint.volume;
                        let was_muted = endpoint.seen.muted;
                        let target = volume::target(seen.route);
                        endpoint.seen = seen;
                        endpoint.volume = observed;
                        if (was_volume - observed).abs() > 1e-4 {
                            let (origin, next) = endpoint.writer.observed(observed, now);
                            if let Some(write) = next {
                                commands.push(volume_command(
                                    &name,
                                    target,
                                    write,
                                    &endpoint.seen.channels,
                                ));
                            }
                            events.push(Event {
                                what: "volume",
                                id,
                                direction,
                                output: false,
                                value: observed,
                                muted,
                                origin,
                                bus: kind,
                                stream,
                            });
                        }
                        if was_muted != muted {
                            let origin = match self.mutes.remove(&name) {
                                Some((asked, origin, at))
                                    if asked == muted
                                        && now.saturating_sub(at) < Duration::from_secs(2) =>
                                {
                                    origin
                                }
                                _ => Origin::External,
                            };
                            events.push(Event {
                                what: "mute",
                                id,
                                direction,
                                output: false,
                                value: observed,
                                muted,
                                origin,
                                bus: kind,
                                stream,
                            });
                        }
                    }
                }
            }
            Fact::Gone(name) => {
                if let Some(endpoint) = self.endpoints.remove(&name) {
                    // A stream is named by its node, which never comes back:
                    // its id goes with it, where an endpoint's is kept for
                    // the next time it is plugged in.
                    if endpoint.is_stream() {
                        self.ids.remove(&name);
                        self.mutes.remove(&name);
                    }
                    events.push(Event {
                        what: "removed",
                        id: endpoint.id,
                        direction: endpoint.seen.direction,
                        output: false,
                        value: endpoint.volume,
                        muted: endpoint.seen.muted,
                        origin: Origin::External,
                        bus: endpoint.seen.kind,
                        stream: endpoint.is_stream(),
                    });
                }
            }
            Fact::Defaults { output, input } => {
                for (direction, now_default) in
                    [(Direction::Output, output), (Direction::Input, input)]
                {
                    let slot = match direction {
                        Direction::Output => &mut self.output,
                        Direction::Input => &mut self.input,
                    };
                    // The default is briefly none while it changes: keep the
                    // last one rather than flash "no device".
                    if now_default.is_some() && *slot != now_default {
                        *slot = now_default;
                        if let Some(endpoint) =
                            slot.as_ref().and_then(|name| self.endpoints.get(name))
                        {
                            events.push(Event {
                                what: "default",
                                id: endpoint.id,
                                direction,
                                output: false,
                                value: endpoint.volume,
                                muted: endpoint.seen.muted,
                                origin: Origin::External,
                                bus: endpoint.seen.kind,
                                stream: false,
                            });
                        }
                    }
                }
            }
            Fact::Ready => self.ready = true,
            Fact::Down => {
                self.ready = false;
            }
        }
        // Mark the default output's events as such, once the defaults are known.
        let output = self.output.clone();
        for event in &mut events {
            event.output = event.direction == Direction::Output
                && self
                    .endpoints
                    .values()
                    .any(|each| each.id == event.id && Some(&each.seen.name) == output.as_ref());
        }
        (events, commands)
    }

    /// A volume asked for: `value` (perceptual) or `by` a step, clamped to
    /// `settings.max` and rounded to the step. The command to send now, if
    /// one should be sent now; none while a write is in flight, which then
    /// sends the newest request when it is answered.
    /// `a_volume_request_is_clamped_and_coalesced`.
    pub(crate) fn request_volume(
        &mut self,
        which: &Which,
        value: Option<f64>,
        by: Option<f64>,
        origin: Origin,
        now: Duration,
    ) -> Result<Option<Command>, &'static str> {
        let settings = self.settings;
        let endpoint = self.resolve(which).ok_or("unknown-endpoint")?;
        let name = endpoint.seen.name.clone();
        let base = endpoint.shown_volume();
        let wanted = match (value, by) {
            (Some(value), _) => value,
            (None, Some(by)) => base + by,
            (None, None) => return Err("bad-data"),
        };
        // A step up or down lands on a multiple of its own size: 5% steps
        // from 52% land on 55%, 1% steps on whole percents.
        let step = by.map(f64::abs).filter(|step| *step > 0.0);
        let volume = volume::clamp(wanted, settings.max, step);
        let endpoint = self.endpoints.get_mut(&name).ok_or("unknown-endpoint")?;
        let target = volume::target(endpoint.seen.route);
        let write = endpoint.writer.request(Write { volume, origin }, now);
        Ok(write.map(|write| volume_command(&name, target, write, &endpoint.seen.channels)))
    }

    /// Mute asked for: `Some` to set, `None` to toggle.
    pub(crate) fn request_mute(
        &mut self,
        which: &Which,
        muted: Option<bool>,
        origin: Origin,
        now: Duration,
    ) -> Result<Command, &'static str> {
        let endpoint = self.resolve(which).ok_or("unknown-endpoint")?;
        let name = endpoint.seen.name.clone();
        // A toggle toggles what was last asked, while that is still on its
        // way: two quick presses mute and unmute, not mute twice.
        // `two_quick_mute_toggles_mute_and_unmute`.
        let current = match self.mutes.get(&name) {
            Some((asked, _, at)) if now.saturating_sub(*at) < Duration::from_secs(2) => *asked,
            _ => endpoint.seen.muted,
        };
        let muted = muted.unwrap_or(!current);
        let target = volume::target(endpoint.seen.route);
        self.mutes.insert(name.clone(), (muted, origin, now));
        Ok(Command::Mute {
            name,
            target,
            muted,
        })
    }

    /// Whether every one of these streams is muted, as last asked or seen,
    /// which a window's toggle reads.
    pub(crate) fn all_muted(&self, ids: &[u32]) -> bool {
        ids.iter().all(|id| {
            self.resolve(&Which::Id(*id)).is_some_and(|endpoint| {
                match self.mutes.get(&endpoint.seen.name) {
                    Some((asked, ..)) => *asked,
                    None => endpoint.seen.muted,
                }
            })
        })
    }

    /// A new default asked for.
    pub(crate) fn request_default(&self, id: u32) -> Result<Command, &'static str> {
        let endpoint = self.resolve(&Which::Id(id)).ok_or("unknown-endpoint")?;
        Ok(Command::Default {
            direction: endpoint.seen.direction,
            name: endpoint.seen.name.clone(),
        })
    }

    /// Writes that waited too long for an echo settle, and the next waiting
    /// one of each goes.
    pub(crate) fn tick(&mut self, now: Duration) -> Vec<Command> {
        let mut commands = Vec::new();
        for (name, endpoint) in &mut self.endpoints {
            if let Some(write) = endpoint.writer.expire(now) {
                let target = volume::target(endpoint.seen.route);
                commands.push(volume_command(name, target, write, &endpoint.seen.channels));
            }
        }
        commands
    }
}

fn volume_command(name: &str, target: Target, write: Write, channels: &[f32]) -> Command {
    Command::Volume {
        name: name.to_owned(),
        target,
        channels: volume::channels(volume::to_linear(write.volume), channels.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    fn speakers(volume: f64) -> Seen {
        Seen {
            name: "alsa_output.pci-0000_00_1f.3.analog-stereo".to_owned(),
            label: "Speakers".to_owned(),
            direction: Direction::Output,
            kind: Kind::Speakers,
            channels: volume::channels(volume::to_linear(volume), 2),
            muted: false,
            step: None,
            route: Some((40, 2)),
            port_label: "Speakers".to_owned(),
            port_kind: "speaker".to_owned(),
            state: "running".to_owned(),
            app: None,
        }
    }

    fn ready_with_speakers(volume: f64) -> Audio {
        let mut audio = Audio::default();
        audio.apply(Fact::Seen(speakers(volume)), ms(0));
        audio.apply(
            Fact::Defaults {
                output: Some(speakers(volume).name),
                input: None,
            },
            ms(0),
        );
        audio.apply(Fact::Ready, ms(0));
        audio
    }

    /// Firefox's playback stream: process 300, started by 200, started by
    /// 100.
    fn firefox_stream(node: u32, volume: f64) -> Seen {
        Seen {
            name: format!("stream#{node}"),
            label: "Firefox".to_owned(),
            direction: Direction::Output,
            kind: Kind::Other,
            channels: volume::channels(volume::to_linear(volume), 2),
            muted: false,
            step: None,
            route: None,
            port_label: String::new(),
            port_kind: String::new(),
            state: "running".to_owned(),
            app: Some(Box::new(App {
                pid: Some(300),
                family: vec![300, 200, 100],
                name: "Firefox".to_owned(),
                id: String::new(),
                binary: "firefox".to_owned(),
                media: "AudioStream".to_owned(),
                icon: "firefox".to_owned(),
            })),
        }
    }

    fn window(id: u64, pid: Option<u32>, app_id: &str) -> WindowKey {
        WindowKey {
            id,
            pid,
            app_id: app_id.to_owned(),
        }
    }

    /// **A window is matched to its stream by its process, then by its
    /// application** (#416): the nearest of the stream's process and the
    /// ones that started it that is a window's gives the stream to every
    /// window of that process; with none, a window whose application id
    /// names the stream's application has it; and a stream no window is
    /// the process or the application of is no window's.
    #[test]
    fn a_window_is_matched_to_its_stream_by_pid_then_by_app() {
        let mut audio = ready_with_speakers(0.5);
        audio.apply(Fact::Seen(firefox_stream(41, 0.5)), ms(0));
        let id = audio
            .endpoints()
            .find(|each| each.is_stream())
            .expect("the stream")
            .id;
        assert!(audio.has_streams());
        // Its helper's parent, 200, has two windows; 100 has one too, further
        // up, and a window of another application has none of it.
        let windows = [
            window(1, Some(200), "org.mozilla.firefox"),
            window(2, Some(200), "org.mozilla.firefox"),
            window(3, Some(100), "foot"),
            window(4, Some(999), "org.mozilla.firefox"),
        ];
        assert_eq!(audio.windows_of_streams(&windows)[&id], vec![1, 2]);
        // No window of any process in the family: by the application, which
        // a reverse-domain id names by its last part.
        let windows = [
            window(4, Some(999), "org.mozilla.firefox"),
            window(5, None, "kitty"),
            window(6, None, "FIREFOX"),
        ];
        assert_eq!(audio.windows_of_streams(&windows)[&id], vec![4, 6]);
        assert_eq!(
            audio.windows_of_streams(&[window(5, None, "kitty")])[&id],
            Vec::<u64>::new()
        );
        // The speakers are no window's: only streams are matched.
        assert_eq!(audio.windows_of_streams(&windows).len(), 1);
    }

    /// **A stream's id goes with it**, where a sink's is kept for its
    /// replug: a stream is named by its node, which never comes back, so a
    /// session's thousand short sounds leave nothing behind.
    #[test]
    fn a_stream_s_id_goes_with_it() {
        let mut audio = ready_with_speakers(0.5);
        audio.apply(Fact::Seen(firefox_stream(41, 0.5)), ms(0));
        let (events, _) = audio.apply(Fact::Gone("stream#41".to_owned()), ms(10));
        assert_eq!(events[0].what, "removed");
        assert!(events[0].stream);
        assert!(!audio.has_streams());
        assert!(!audio.ids.contains_key("stream#41"));
    }

    #[test]
    fn the_kind_comes_from_the_port_then_the_bus_then_the_form_factor() {
        assert_eq!(
            Kind::from_props("internal", "pci", "headphones", false),
            Kind::Headphones
        );
        assert_eq!(
            Kind::from_props("internal", "pci", "speaker", false),
            Kind::Speakers
        );
        assert_eq!(
            Kind::from_props("headset", "bluetooth", "", false),
            Kind::Bluetooth
        );
        assert_eq!(Kind::from_props("", "usb", "", false), Kind::Usb);
        assert_eq!(
            Kind::from_props("headphone", "", "", false),
            Kind::Headphones
        );
        assert_eq!(Kind::from_props("", "", "", true), Kind::Virtual);
        assert_eq!(Kind::from_props("", "", "", false), Kind::Other);
    }

    #[test]
    fn a_volume_request_is_clamped_and_coalesced() {
        let mut audio = ready_with_speakers(0.5);
        let first = audio
            .request_volume(&Which::Output, Some(1.3), None, Origin::Scene, ms(0))
            .expect("asked");
        let Some(Command::Volume {
            target, channels, ..
        }) = first
        else {
            panic!("a write: {first:?}");
        };
        assert_eq!(
            target,
            Target::Route {
                device: 40,
                index: 2
            }
        );
        assert_eq!(
            channels,
            vec![1.0, 1.0],
            "clamped to max 1.0, both channels"
        );
        assert_eq!(
            audio.request_volume(&Which::Output, Some(0.8), None, Origin::Scene, ms(5)),
            Ok(None),
            "waits for the first"
        );
        let output = audio.default_of(Direction::Output).expect("speakers");
        assert!(output.pending());
        assert!((output.shown_volume() - 0.8).abs() < 1e-9);
        // The echo of 1.0 sends the 0.8.
        let (events, commands) = audio.apply(Fact::Seen(speakers(1.0)), ms(20));
        assert_eq!(events[0].what, "volume");
        assert_eq!(events[0].origin, Origin::Scene);
        assert!(events[0].output);
        assert_eq!(commands.len(), 1);
        assert_eq!(
            audio.request_volume(&Which::Output, None, None, Origin::Scene, ms(30)),
            Err("bad-data")
        );
        assert_eq!(
            audio.request_volume(&Which::Id(99), Some(0.1), None, Origin::Scene, ms(30)),
            Err("unknown-endpoint")
        );
    }

    #[test]
    fn a_step_up_from_a_key_rounds_to_the_step() {
        let mut audio = ready_with_speakers(0.52);
        let Ok(Some(Command::Volume { channels, .. })) =
            audio.request_volume(&Which::Output, None, Some(0.05), Origin::Binding, ms(0))
        else {
            panic!("a write");
        };
        assert!((volume::to_perceptual(f64::from(channels[0])) - 0.55).abs() < 1e-4);
    }

    #[test]
    fn an_echo_of_a_key_s_change_is_told_as_the_key_s() {
        let mut audio = ready_with_speakers(0.5);
        audio
            .request_mute(&Which::Output, None, Origin::Binding, ms(0))
            .expect("asked");
        let mut muted = speakers(0.5);
        muted.muted = true;
        let (events, _) = audio.apply(Fact::Seen(muted.clone()), ms(10));
        assert_eq!(events[0].what, "mute");
        assert_eq!(events[0].origin, Origin::Binding);
        // The headset's own button unmutes it.
        let (events, _) = audio.apply(Fact::Seen(speakers(0.5)), ms(500));
        assert_eq!(events[0].origin, Origin::External);
    }

    #[test]
    fn two_quick_mute_toggles_mute_and_unmute() {
        let mut audio = ready_with_speakers(0.5);
        let first = audio.request_mute(&Which::Output, None, Origin::Binding, ms(0));
        let second = audio.request_mute(&Which::Output, None, Origin::Binding, ms(50));
        assert!(matches!(first, Ok(Command::Mute { muted: true, .. })));
        assert!(matches!(second, Ok(Command::Mute { muted: false, .. })));
    }

    #[test]
    fn an_endpoint_keeps_its_id_across_a_replug() {
        let mut audio = ready_with_speakers(0.5);
        let id = audio.default_of(Direction::Output).expect("speakers").id;
        audio.apply(Fact::Gone(speakers(0.5).name), ms(10));
        assert!(audio.default_of(Direction::Output).is_none());
        audio.apply(Fact::Seen(speakers(0.5)), ms(20));
        assert_eq!(audio.default_of(Direction::Output).expect("back").id, id);
    }

    #[test]
    fn a_default_that_is_briefly_none_keeps_the_last_one() {
        let mut audio = ready_with_speakers(0.5);
        audio.apply(
            Fact::Defaults {
                output: None,
                input: None,
            },
            ms(10),
        );
        assert!(audio.default_of(Direction::Output).is_some());
    }

    #[test]
    fn a_write_with_no_echo_lets_the_next_one_go() {
        let mut audio = ready_with_speakers(0.5);
        audio
            .request_volume(&Which::Output, Some(0.51), None, Origin::Scene, ms(0))
            .expect("asked");
        audio
            .request_volume(&Which::Output, Some(0.6), None, Origin::Scene, ms(10))
            .expect("asked");
        assert!(audio.tick(ms(100)).is_empty());
        assert_eq!(audio.tick(ms(400)).len(), 1);
    }
}
