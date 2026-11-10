//! A window manager in one file, to read and to start from.
//!
//! It tiles in columns: every window gets an equal share of the screen's
//! width, in the order the windows opened. `super+return` opens a terminal
//! (`$TERMINAL`, or `foot`), `super+q` closes the window that has the
//! keyboard, `super+j` and `super+k` move the keyboard to the next window
//! and the one before, and `super+shift+e` ends the session.
//!
//! Build it with `cargo build -p solium-wm-protocol --example tiny_wm`, and
//! name it in `~/.config/solium/wm.toml`:
//!
//! ```toml
//! command = ["/path/to/target/debug/examples/tiny_wm"]
//! ```
//!
//! Solium starts it with the socket as its standard input, and everything
//! it knows of the desktop comes over that socket: nothing here links the
//! compositor, and a window manager in any other language does what this
//! one does with the same messages.

use solium_wm_protocol::client::{Client, ClientError, Heard};
use solium_wm_protocol::mirror::Mirror;
use solium_wm_protocol::{
    Answer, Binding, Command, Declarations, Easing, Event, FocusCause, Motion, Place, Rect,
    Reloaded,
};

/// The keys it binds, and what each does, as `solium --check` lists them.
const BINDINGS: &[(&str, &str)] = &[
    ("super+return", "open a terminal"),
    ("super+q", "close the focused window"),
    ("super+j", "focus the next window"),
    ("super+k", "focus the window before"),
    ("super+shift+e", "end the session"),
];

/// What it tells the compositor before the first frame, and again after a
/// reload: the keys that are its own, so the compositor never has to ask
/// whether a key is bound, and the events it wants to hear. It is sent no
/// other, but for the keys it bound.
fn setup() -> Vec<Command> {
    vec![
        Command::Declare(Declarations {
            bindings: BINDINGS
                .iter()
                .map(|(combo, note)| Binding {
                    combo: (*combo).to_owned(),
                    note: Some((*note).to_owned()),
                    repeating: false,
                    locked: false,
                    inhibited: false,
                })
                .collect(),
            unbound: Vec::new(),
            unknown: Vec::new(),
            notes: Vec::new(),
        }),
        Command::Listen(vec![
            "opened".to_owned(),
            "closed".to_owned(),
            "monitors-changed".to_owned(),
        ]),
    ]
}

/// The window manager: what it has been told of the desktop, and the
/// windows it tiles, in the order they opened.
#[derive(Debug, Default)]
struct Tiny {
    mirror: Mirror,
    tiled: Vec<u64>,
}

impl Tiny {
    /// Every tiled window in a column of its own, side by side across the
    /// part of the screen that bars and docks leave.
    fn arrange(&self) -> Vec<Command> {
        let primary = self.mirror.monitors.iter().find(|monitor| monitor.primary);
        let Some(area) = self
            .mirror
            .work_area
            .as_ref()
            .or(primary.map(|monitor| &monitor.area))
        else {
            return Vec::new();
        };
        let width = area.w / self.tiled.len().max(1) as f64;
        self.tiled
            .iter()
            .enumerate()
            .map(|(column, id)| {
                Command::Place(Place {
                    id: *id,
                    rect: Rect {
                        x: area.x + width * column as f64,
                        y: area.y,
                        w: width,
                        h: area.h,
                    },
                    // The compositor moves it there: a window manager says
                    // where and how, and never draws a frame of it.
                    motion: Motion {
                        duration_ms: 180.0,
                        easing: Easing::OutCubic,
                    },
                    tile: true,
                    inside: None,
                    cramped: false,
                    moved: false,
                })
            })
            .collect()
    }

    /// The window `step` places along from the one with the keyboard.
    fn neighbour(&self, step: isize) -> Option<u64> {
        let focused = self.mirror.focused()?.id;
        let at = self.tiled.iter().position(|id| *id == focused)?;
        let count = self.tiled.len() as isize;
        let next = (at as isize + step).rem_euclid(count);
        self.tiled.get(next as usize).copied()
    }

    /// What a key it bound does.
    fn key(&self, combo: &str) -> Vec<Command> {
        let focus = |id: u64| Command::Focus((id, FocusCause::Binding, None));
        match combo {
            "super+return" => {
                let terminal = std::env::var("TERMINAL").unwrap_or_else(|_| "foot".to_owned());
                vec![Command::Spawn(vec![terminal])]
            }
            "super+q" => self
                .mirror
                .focused()
                .map(|window| Command::Close(window.id))
                .into_iter()
                .collect(),
            "super+j" => self.neighbour(1).map(focus).into_iter().collect(),
            "super+k" => self.neighbour(-1).map(focus).into_iter().collect(),
            "super+shift+e" => vec![Command::Quit],
            _ => Vec::new(),
        }
    }

    /// One batch: what changed is taken in first, so every event is
    /// handled with the desktop as it was when it happened.
    fn handle(&mut self, events: Vec<Event>) -> (bool, Vec<Command>) {
        let mut handled = false;
        let mut commands = Vec::new();
        for event in events {
            match event {
                Event::Opened(id) => {
                    self.tiled.push(id);
                    commands.extend(self.arrange());
                    // Taken: the compositor does not place it itself.
                    handled = true;
                }
                Event::Closed(id) => {
                    self.tiled.retain(|each| *each != id);
                    commands.extend(self.arrange());
                }
                Event::MonitorsChanged => commands.extend(self.arrange()),
                Event::Key(combo) => {
                    commands.extend(self.key(&combo));
                    // Its own key: not typed into the window under it.
                    handled = true;
                }
                _ => {}
            }
        }
        (handled, commands)
    }
}

/// The whole conversation, until the compositor says goodbye.
pub fn serve(client: &Client) -> Result<(), ClientError> {
    // `hello` first. It names the protocol the compositor speaks, and one
    // this was not built for is refused there.
    client.hello()?;
    client.ready(setup())?;
    let mut tiny = Tiny::default();
    loop {
        match client.next()? {
            Heard::Batch(batch) => {
                tiny.mirror.apply_all(batch.changes);
                let (handled, commands) = tiny.handle(batch.events);
                client.reply(batch.serial, handled, commands)?;
            }
            // It has no configuration to read again: it says what it said
            // at the start, which is what a reload asks for.
            Heard::Reload(_) => client.reloaded(Reloaded::Loaded(Answer {
                serial: 0,
                handled: false,
                commands: setup(),
                motion: None,
                decision: None,
            }))?,
            Heard::Goodbye(_) => return Ok(()),
        }
    }
}

#[allow(
    dead_code,
    reason = "the compositor's tests run `serve` on a thread, with no `main`"
)]
fn main() -> std::process::ExitCode {
    let served = Client::from_stdin()
        .map_err(ClientError::from)
        .and_then(|client| serve(&client));
    match served {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("tiny_wm: {err}");
            std::process::ExitCode::FAILURE
        }
    }
}
