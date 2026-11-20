//! A window manager written for river, as the compositor's window manager.
//!
//! The compositor speaks to one window manager, over the protocol in
//! `crates/wm-protocol`, and this is one: the far end of that socket, inside
//! the compositor. What the compositor tells it (the desktop's windows and
//! monitors, a key, a window opening) it tells the river window manager as
//! river's objects and events, and what that one asks for in its manage and
//! render sequences it sends back as ordinary commands (`place`, `focus`,
//! `close`), with a `submit`. So everything above the socket (who waits for
//! what, a window manager that stops and is started again, the problems
//! overlay) is the same for both protocols.
//!
//! It answers every batch at once, from what it knows: a window that opens
//! is hidden until the river window manager has placed it, which is what
//! river promises ("the window will not be displayed until the first
//! dimensions event is received and the render sequence is finished"); a
//! key is the window manager's when one of its bindings is that key. River's
//! loop then runs at the window manager's own pace, and the compositor never
//! waits for it.
//!
//! Where river and Solium differ:
//!
//! - River waits for each window to take its new size between the manage
//!   and the render sequence. Solium does not wait: a tiled window is drawn
//!   in its rectangle at once and its application catches up. So the
//!   dimensions a render sequence reports are the ones proposed, and the
//!   size an application takes of its own (`sized`) starts another render
//!   sequence.
//! - Stacking is the compositor's: `place_top` and the rest are taken and
//!   do nothing, and a window the keyboard moves to is raised as any is.
//! - Frames are the compositor's QML, so the bridge asks for none
//!   (`decoration` `none`) and a window's rectangle is its content, as
//!   river's is. Borders, decoration surfaces and shell surfaces are taken
//!   and not drawn.
//! - The protocol has no stream of pointer motion for an interactive move,
//!   so a pointer operation ends as soon as it starts.

use std::collections::BTreeMap;
use std::os::unix::net::UnixStream;

use smithay::reexports::wayland_server::{Client, DisplayHandle, Resource};
use solium_wm_protocol::{self as wire, Message, frame};

use super::protocol::window_management::{
    river_node_v1::RiverNodeV1,
    river_output_v1::RiverOutputV1,
    river_seat_v1::RiverSeatV1,
    river_window_manager_v1::RiverWindowManagerV1,
    river_window_v1::{DecorationHint, RiverWindowV1},
};
use super::protocol::xkb_bindings::river_xkb_binding_v1::RiverXkbBindingV1;

/// The events the bridge listens to, beyond the keys and timers every
/// window manager hears.
const LISTENS: &[&str] = &[
    "opened",
    "closed",
    "renamed",
    "sized",
    "focused",
    "fullscreen",
    "maximize",
    "monitors-changed",
    "lock-changed",
];

/// River's modifiers, as `river_seat_v1.modifiers` numbers them, and the
/// word a combination spells each with. Mod3 and Mod5 have no word in a
/// combination, so a binding that needs one is not declared.
const MODIFIERS: &[(u32, &str)] = &[(1, "shift"), (4, "ctrl"), (8, "alt"), (64, "super")];

/// The user data of a window's river objects: its id on the desktop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WindowId(pub(crate) u64);

/// The user data of an output's river object: its monitor's name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OutputName(pub(crate) String);

/// Which part of river's loop it is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sequence {
    /// Neither: the next change starts one.
    Idle,
    /// Between `manage_start` and `manage_finish`.
    Manage,
    /// Between `render_start` and `render_finish`.
    Render,
}

/// A window as the river window manager knows it.
#[derive(Debug)]
struct RiverWindow {
    resource: RiverWindowV1,
    node: Option<RiverNodeV1>,
    /// What it was told, so a change is told once.
    title: Option<String>,
    app_id: Option<String>,
    limits: (i32, i32, i32, i32),
    /// The size it proposed in the last manage sequence; 0 is the window's
    /// own choice.
    proposed: Option<(i32, i32)>,
    /// The size it was last told the window has.
    told: Option<(i32, i32)>,
    /// Where it put the window's node.
    position: Option<(i32, i32)>,
    /// The rectangle the compositor was last asked to place it in.
    placed: Option<(i32, i32, i32, i32)>,
    /// It asked for the window to be hidden.
    hidden: bool,
    /// The window is drawn: every window there is when the river window
    /// manager starts, and a new one once it has been placed.
    revealed: bool,
    /// The compositor said it is gone, and `closed` was sent.
    closed: bool,
}

/// An output as the river window manager knows it.
#[derive(Debug)]
struct RiverOutput {
    resource: RiverOutputV1,
    told: (i32, i32, i32, i32),
}

/// A key binding the river window manager asked for.
#[derive(Debug)]
struct KeyBinding {
    resource: RiverXkbBindingV1,
    /// Its combination, as the compositor spells it; none for one that
    /// needs a modifier a combination has no word for.
    combo: Option<String>,
    enabled: bool,
}

/// The river window manager, once it has bound the global.
#[derive(Debug)]
struct Bound {
    handle: DisplayHandle,
    client: Client,
    manager: RiverWindowManagerV1,
    seat: Option<RiverSeatV1>,
    windows: BTreeMap<u64, RiverWindow>,
    outputs: BTreeMap<String, RiverOutput>,
}

/// The bridge: the window manager's end of the compositor's socket, and the
/// river window manager's objects.
#[derive(Debug)]
pub(crate) struct Bridge {
    socket: UnixStream,
    reader: frame::Reader,
    /// The river window manager's process: only it, or a process it
    /// started, may bind the global.
    pid: u32,
    /// The desktop as the compositor last told it.
    rows: BTreeMap<u64, wire::Window>,
    monitors: Vec<wire::Monitor>,
    locked: bool,
    bound: Option<Bound>,
    sequence: Sequence,
    manage_wanted: bool,
    render_wanted: bool,
    /// The windows a click or a touch reached since the last manage
    /// sequence, which it is told as interactions.
    interactions: Vec<u64>,
    /// What the river window manager asked of the windows in this manage
    /// sequence, applied at `manage_finish`.
    managing: Vec<wire::Command>,
    bindings: Vec<KeyBinding>,
    /// The combinations last declared to the compositor.
    declared: Vec<String>,
}

impl Bridge {
    /// The bridge on `socket`, for the river window manager that is process
    /// `pid`.
    pub(crate) fn new(socket: UnixStream, pid: u32) -> std::io::Result<Self> {
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            reader: frame::Reader::default(),
            pid,
            rows: BTreeMap::new(),
            monitors: Vec::new(),
            locked: false,
            bound: None,
            sequence: Sequence::Idle,
            manage_wanted: false,
            render_wanted: false,
            interactions: Vec::new(),
            managing: Vec::new(),
            bindings: Vec::new(),
            declared: Vec::new(),
        })
    }

    /// The river window manager's process.
    pub(crate) fn pid(&self) -> u32 {
        self.pid
    }

    /// Whether a river window manager has bound the global.
    pub(crate) fn is_bound(&self) -> bool {
        self.bound.is_some()
    }

    /// Read and answer everything the compositor has written: called as
    /// soon as it writes, so an answer is there before it looks.
    pub(crate) fn pump(&mut self) {
        let messages = match self.reader.read_ready(&mut &self.socket) {
            Ok(messages) => messages,
            Err(err) => {
                tracing::warn!(%err, "the river bridge cannot read the compositor");
                return;
            }
        };
        for message in messages {
            self.hear(message);
        }
    }

    fn say(&self, message: &Message) {
        if let Err(err) = frame::write(&mut &self.socket, message) {
            tracing::warn!(%err, "the river bridge cannot write to the compositor");
        }
    }

    /// Commands of the river window manager's, asked for by no batch.
    fn submit(&self, commands: Vec<wire::Command>) {
        if !commands.is_empty() {
            tracing::debug!(?commands, "the river window manager asks");
            self.say(&Message::Submit(commands));
        }
    }

    fn hear(&mut self, message: Message) {
        match message {
            Message::Hello(_) => self.say(&Message::HelloAnswer(wire::HelloAnswer {
                version: wire::VERSION.to_owned(),
                commands: self.setup(),
                source: None,
            })),
            Message::Batch(batch) => {
                let answer = self.batch(batch);
                self.say(&Message::Answer(answer));
            }
            Message::Reload => self.say(&Message::Reloaded(wire::Reloaded::Loaded(wire::Answer {
                commands: self.setup(),
                ..wire::Answer::default()
            }))),
            Message::Goodbye(_) | Message::Reply(_) => {}
            other => tracing::debug!(case = other.case(), "the river bridge passes over"),
        }
    }

    /// What the bridge asks for before the first frame, and again at a
    /// reload: the events it listens to, frames that are only the window,
    /// and the river window manager's key bindings.
    fn setup(&self) -> Vec<wire::Command> {
        vec![
            wire::Command::Listen(LISTENS.iter().map(|event| (*event).to_owned()).collect()),
            wire::Command::Decoration(Some("none".to_owned())),
            wire::Command::Declare(self.declarations()),
        ]
    }

    fn declarations(&self) -> wire::Declarations {
        wire::Declarations {
            bindings: self
                .declared
                .iter()
                .map(|combo| wire::Binding {
                    combo: combo.clone(),
                    ..wire::Binding::default()
                })
                .collect(),
            ..wire::Declarations::default()
        }
    }

    /// One batch: what changed, then what happened, answered.
    fn batch(&mut self, batch: wire::Batch) -> wire::Answer {
        let mut answer = wire::Answer {
            serial: batch.serial,
            ..wire::Answer::default()
        };
        for change in batch.changes {
            match change {
                // A window already told closed stays closed, whatever row
                // the rest of its fade brings.
                wire::Change::Window(row)
                    if self
                        .bound
                        .as_ref()
                        .and_then(|bound| bound.windows.get(&row.id))
                        .is_some_and(|window| window.closed) => {}
                wire::Change::Window(row) => {
                    let changed = self
                        .rows
                        .get(&row.id)
                        .is_none_or(|before| managed_differs(before, &row));
                    self.manage_wanted |= changed;
                    let id = row.id;
                    self.rows.insert(id, row);
                    // A window that has drawn, or drawn another size of its
                    // own choosing: its dimensions are told in a render
                    // sequence.
                    if let Some(window) = self.bound.as_ref().and_then(|b| b.windows.get(&id)) {
                        let size = self.size_of(id, window.proposed);
                        self.render_wanted |= size.is_some() && size != window.told;
                    }
                }
                wire::Change::WindowGone(id) => {
                    self.rows.remove(&id);
                    self.manage_wanted = true;
                }
                wire::Change::Monitors(monitors) => {
                    self.monitors = monitors;
                    self.manage_wanted = true;
                }
                wire::Change::Locked(locked) => self.lock(locked),
                _ => {}
            }
        }
        for event in batch.events {
            match event {
                // Not drawn until the river window manager has placed it,
                // unless it already has: a window's row can come in an
                // earlier batch than its opening, and the window be placed
                // and drawn by then.
                wire::Event::Opened(id) if self.bound.is_some() => {
                    answer.handled = true;
                    let placed = self
                        .bound
                        .as_ref()
                        .and_then(|bound| bound.windows.get(&id))
                        .is_some_and(|window| window.revealed);
                    if !placed {
                        answer.commands.push(hidden(id));
                    }
                }
                wire::Event::Key(combo) => answer.handled |= self.pressed(&combo),
                wire::Event::Focused(moved)
                    if matches!(
                        moved.cause,
                        wire::FocusCause::Click | wire::FocusCause::Touch
                    ) =>
                {
                    self.interactions.push(moved.id);
                    self.manage_wanted = true;
                }
                wire::Event::Sized(_) => self.render_wanted = true,
                // Its life is over, though its row may stay while it fades:
                // the river window manager is told it closed.
                wire::Event::Closed(id) => {
                    self.rows.remove(&id);
                    self.manage_wanted = true;
                }
                _ => {}
            }
        }
        self.advance();
        answer
    }

    fn lock(&mut self, locked: bool) {
        if locked == self.locked {
            return;
        }
        self.locked = locked;
        if let Some(bound) = &self.bound {
            if locked {
                bound.manager.session_locked();
            } else {
                bound.manager.session_unlocked();
            }
        }
    }

    /// A key the compositor matched to a binding: each enabled river binding
    /// of that combination is pressed and let go, and a manage sequence
    /// follows, which is where a river window manager acts on it. Whether
    /// one was.
    fn pressed(&mut self, combo: &str) -> bool {
        let mut any = false;
        for binding in &self.bindings {
            if binding.enabled && binding.combo.as_deref() == Some(combo) {
                binding.resource.pressed();
                binding.resource.released();
                any = true;
            }
        }
        self.manage_wanted |= any;
        any
    }

    /// The river window manager bound the global: tell it everything there
    /// is, and start its first manage sequence.
    pub(crate) fn bind(
        &mut self,
        handle: DisplayHandle,
        client: Client,
        manager: RiverWindowManagerV1,
    ) {
        self.bound = Some(Bound {
            handle,
            client,
            manager,
            seat: None,
            windows: BTreeMap::new(),
            outputs: BTreeMap::new(),
        });
        self.sequence = Sequence::Idle;
        self.manage_wanted = true;
        self.advance();
        // What was on the desktop before it came is drawn as it was.
        if let Some(bound) = &mut self.bound {
            for window in bound.windows.values_mut() {
                window.revealed = true;
            }
        }
    }

    /// The compositor is done with this window manager: it is told it is
    /// finished.
    pub(crate) fn finish(&mut self) {
        if let Some(bound) = self.bound.take() {
            bound.manager.finished();
        }
    }

    /// The river window manager went, or asked to stop being told.
    pub(crate) fn unbind(&mut self) {
        self.bound = None;
        self.sequence = Sequence::Idle;
        self.managing.clear();
    }

    /// Start whatever sequence is wanted, if none is running.
    fn advance(&mut self) {
        if self.bound.is_none() || self.sequence != Sequence::Idle {
            return;
        }
        if self.manage_wanted {
            self.manage_wanted = false;
            self.tell();
            if let Some(bound) = &self.bound {
                bound.manager.manage_start();
            }
            tracing::debug!("river manage sequence started");
            self.sequence = Sequence::Manage;
        } else if self.render_wanted {
            self.render_wanted = false;
            self.start_render();
        }
    }

    /// Tell the river window manager every change since it was last told:
    /// new windows, outputs and the seat; what changed of each; what went.
    fn tell(&mut self) {
        let Some(bound) = &mut self.bound else {
            return;
        };
        if bound.seat.is_none() {
            match bound
                .client
                .create_resource::<RiverSeatV1, (), crate::state::Solium>(
                    &bound.handle,
                    bound.manager.version(),
                    (),
                ) {
                Ok(seat) => {
                    bound.manager.seat(&seat);
                    bound.seat = Some(seat);
                }
                Err(err) => tracing::warn!(%err, "no river seat"),
            }
        }
        // Outputs.
        bound.outputs.retain(|name, output| {
            let there = self.monitors.iter().any(|monitor| &monitor.name == name);
            if !there {
                output.resource.removed();
            }
            there
        });
        for monitor in &self.monitors {
            let area = rect(&monitor.whole);
            if let Some(output) = bound.outputs.get_mut(&monitor.name) {
                if output.told != area {
                    output.resource.position(area.0, area.1);
                    output.resource.dimensions(area.2, area.3);
                    output.told = area;
                }
                continue;
            }
            match bound
                .client
                .create_resource::<RiverOutputV1, OutputName, crate::state::Solium>(
                    &bound.handle,
                    bound.manager.version(),
                    OutputName(monitor.name.clone()),
                ) {
                Ok(resource) => {
                    bound.manager.output(&resource);
                    resource.position(area.0, area.1);
                    resource.dimensions(area.2, area.3);
                    bound.outputs.insert(
                        monitor.name.clone(),
                        RiverOutput {
                            resource,
                            told: area,
                        },
                    );
                }
                Err(err) => tracing::warn!(%err, "no river output"),
            }
        }
        // Windows that went.
        for (id, window) in &mut bound.windows {
            if !window.closed && !self.rows.contains_key(id) {
                window.resource.closed();
                window.closed = true;
            }
        }
        // New windows, and what changed of each.
        for row in self.rows.values() {
            if !bound.windows.contains_key(&row.id) {
                match bound
                    .client
                    .create_resource::<RiverWindowV1, WindowId, crate::state::Solium>(
                        &bound.handle,
                        bound.manager.version(),
                        WindowId(row.id),
                    ) {
                    Ok(resource) => {
                        bound.manager.window(&resource);
                        if resource.version() >= 4 {
                            resource.identifier(row.id.to_string());
                        }
                        if let (Some(pid), true) = (row.pid, resource.version() >= 2) {
                            resource.unreliable_pid(i32::try_from(pid).unwrap_or(0));
                        }
                        resource.decoration_hint(if row.client_side {
                            DecorationHint::PrefersCsd
                        } else {
                            DecorationHint::NoPreference
                        });
                        bound.windows.insert(
                            row.id,
                            RiverWindow {
                                resource,
                                node: None,
                                title: None,
                                app_id: None,
                                limits: (-1, -1, -1, -1),
                                proposed: None,
                                told: None,
                                position: None,
                                placed: None,
                                hidden: false,
                                revealed: false,
                                closed: false,
                            },
                        );
                    }
                    Err(err) => {
                        tracing::warn!(%err, "no river window");
                        continue;
                    }
                }
            }
            let Some(window) = bound.windows.get_mut(&row.id) else {
                continue;
            };
            let title = Some(row.title.clone());
            if window.title != title {
                window.resource.title(title.clone());
                window.title = title;
            }
            let app_id = (!row.app_id.is_empty()).then(|| row.app_id.clone());
            if window.app_id != app_id {
                window.resource.app_id(app_id.clone());
                window.app_id = app_id;
            }
            let limits = limits(row);
            if window.limits != limits {
                window
                    .resource
                    .dimensions_hint(limits.0, limits.1, limits.2, limits.3);
                window.limits = limits;
            }
        }
        // The parents, once every window of this batch exists.
        for row in self.rows.values() {
            let parent = match row.parent {
                wire::Parentage::Window(parent) => bound
                    .windows
                    .get(&parent)
                    .map(|parent| parent.resource.clone()),
                wire::Parentage::None | wire::Parentage::Unknown => None,
            };
            if let Some(window) = bound.windows.get(&row.id) {
                window.resource.parent(parent.as_ref());
            }
        }
        if let Some(seat) = &bound.seat {
            for id in self.interactions.drain(..) {
                if let Some(window) = bound.windows.get(&id).filter(|window| !window.closed) {
                    seat.window_interaction(&window.resource);
                }
            }
        }
    }

    /// The manage sequence is over: apply what it asked of the windows, and
    /// start the render sequence.
    pub(crate) fn manage_finish(&mut self) -> Result<(), &'static str> {
        if self.sequence != Sequence::Manage {
            return Err("manage_finish outside a manage sequence");
        }
        let commands = std::mem::take(&mut self.managing);
        self.submit(commands);
        self.start_render();
        Ok(())
    }

    /// Tell each window's dimensions where they changed, and start a render
    /// sequence.
    fn start_render(&mut self) {
        let Some(bound) = &mut self.bound else {
            return;
        };
        let sizes: Vec<(u64, Option<(i32, i32)>)> = bound
            .windows
            .iter()
            .filter(|(_, window)| !window.closed)
            .map(|(id, window)| (*id, size_of(self.rows.get(id), window.proposed)))
            .collect();
        for (id, size) in sizes {
            // Not until the window has a size: river does not draw a window
            // before its first dimensions, and neither does this.
            let (Some(size), Some(window)) = (size, bound.windows.get_mut(&id)) else {
                continue;
            };
            if window.told != Some(size) {
                window.resource.dimensions(size.0, size.1);
                window.told = Some(size);
            }
        }
        bound.manager.render_start();
        self.sequence = Sequence::Render;
        tracing::debug!("river render sequence started");
    }

    /// The render sequence is over: place every window where its node is,
    /// at the size it was told, and draw what is shown.
    pub(crate) fn render_finish(&mut self) -> Result<(), &'static str> {
        if self.sequence != Sequence::Render {
            return Err("render_finish outside a render sequence");
        }
        let mut commands = Vec::new();
        if let Some(bound) = &mut self.bound {
            for (id, window) in &mut bound.windows {
                if window.closed {
                    continue;
                }
                let Some(size) = window.told else {
                    continue;
                };
                if let Some((x, y)) = window.position {
                    let placed = (x, y, size.0, size.1);
                    if window.placed != Some(placed) {
                        commands.push(wire::Command::Place(wire::Place {
                            id: *id,
                            rect: wire::Rect {
                                x: f64::from(x),
                                y: f64::from(y),
                                w: f64::from(size.0),
                                h: f64::from(size.1),
                            },
                            tile: true,
                            ..wire::Place::default()
                        }));
                        window.placed = Some(placed);
                    }
                }
                let shown = !window.hidden;
                if shown && !window.revealed {
                    commands.push(wire::Command::Clear(wire::Clear {
                        id: *id,
                        motion: wire::Motion::default(),
                    }));
                    window.revealed = true;
                } else if !shown && window.revealed {
                    commands.push(hidden(*id));
                    window.revealed = false;
                }
            }
        }
        self.submit(commands);
        self.sequence = Sequence::Idle;
        self.advance();
        Ok(())
    }

    /// The size a window is told it has: what the river window manager
    /// proposed, the window's own where it proposed 0 or nothing.
    fn size_of(&self, id: u64, proposed: Option<(i32, i32)>) -> Option<(i32, i32)> {
        size_of(self.rows.get(&id), proposed)
    }

    /// The river window manager asked for a manage sequence.
    pub(crate) fn manage_dirty(&mut self) {
        self.manage_wanted = true;
        self.advance();
    }

    /// A request that changes window management state, taken only in a
    /// manage sequence.
    fn managing(&self) -> bool {
        self.sequence == Sequence::Manage
    }

    /// A request that changes rendering state, taken in either sequence.
    fn rendering(&self) -> bool {
        self.sequence != Sequence::Idle
    }

    pub(crate) fn propose(&mut self, id: u64, width: i32, height: i32) {
        if !self.managing() {
            tracing::debug!(id, "propose_dimensions outside a manage sequence; left out");
            return;
        }
        if let Some(window) = self.window_mut(id) {
            window.proposed = Some((width.max(0), height.max(0)));
        }
    }

    pub(crate) fn position(&mut self, id: u64, x: i32, y: i32) {
        if !self.rendering() {
            return;
        }
        if let Some(window) = self.window_mut(id) {
            window.position = Some((x, y));
        }
    }

    pub(crate) fn show(&mut self, id: u64, shown: bool) {
        if !self.rendering() {
            return;
        }
        if let Some(window) = self.window_mut(id) {
            window.hidden = !shown;
        }
    }

    pub(crate) fn node(&mut self, id: u64, node: RiverNodeV1) {
        if let Some(window) = self.window_mut(id) {
            window.node = Some(node);
        }
    }

    /// A window-management command of the river window manager's, applied
    /// at `manage_finish`.
    pub(crate) fn manage(&mut self, command: wire::Command) {
        if self.managing() {
            self.managing.push(command);
        } else {
            tracing::debug!(
                case = command.case(),
                "a river request outside a manage sequence; left out"
            );
        }
    }

    /// Whether the window is fullscreen, as the compositor last said.
    pub(crate) fn fullscreen(&self, id: u64) -> bool {
        self.rows.get(&id).is_some_and(|row| row.fullscreen)
    }

    /// The river window manager destroyed a window's object.
    pub(crate) fn forget(&mut self, id: u64) {
        if let Some(bound) = &mut self.bound {
            bound.windows.remove(&id);
        }
    }

    /// The river window manager destroyed an output's object.
    pub(crate) fn forget_output(&mut self, name: &str) {
        if let Some(bound) = &mut self.bound {
            bound.outputs.remove(name);
        }
    }

    /// A key binding, by its keysym and river's modifiers.
    pub(crate) fn add_binding(&mut self, resource: RiverXkbBindingV1, keysym: u32, modifiers: u32) {
        let combo = combo(keysym, modifiers);
        if combo.is_none() {
            tracing::warn!(
                keysym,
                modifiers,
                "a river key binding with a modifier a combination has no word for; it never fires"
            );
        }
        self.bindings.push(KeyBinding {
            resource,
            combo,
            enabled: false,
        });
    }

    /// A key binding turned on or off, or gone: declared to the compositor
    /// anew.
    pub(crate) fn enable_binding(&mut self, resource: &RiverXkbBindingV1, enabled: Option<bool>) {
        match enabled {
            Some(enabled) => {
                if let Some(binding) = self
                    .bindings
                    .iter_mut()
                    .find(|binding| binding.resource == *resource)
                {
                    binding.enabled = enabled;
                }
            }
            None => self
                .bindings
                .retain(|binding| binding.resource != *resource),
        }
        let mut combos: Vec<String> = self
            .bindings
            .iter()
            .filter(|binding| binding.enabled)
            .filter_map(|binding| binding.combo.clone())
            .collect();
        combos.sort();
        combos.dedup();
        if combos != self.declared {
            self.declared = combos;
            self.submit(vec![wire::Command::Declare(self.declarations())]);
        }
    }

    /// Commands the river window manager asks for at any time.
    pub(crate) fn now(&self, commands: Vec<wire::Command>) {
        self.submit(commands);
    }

    fn window_mut(&mut self, id: u64) -> Option<&mut RiverWindow> {
        self.bound
            .as_mut()
            .and_then(|bound| bound.windows.get_mut(&id))
    }

    /// The process that may bind the global: the river window manager, or
    /// one of its descendants (a wrapper script runs it as a child).
    pub(crate) fn may_bind(&self, client_pid: u32) -> bool {
        let mut pid = client_pid;
        for _ in 0..16 {
            if pid == self.pid {
                return true;
            }
            match parent_of(pid) {
                Some(parent) if parent > 1 => pid = parent,
                _ => return false,
            }
        }
        false
    }
}

/// The size a window is told it has: each side as proposed, or the
/// window's own where the proposal is 0 or there is none. None while a side
/// it chooses is not known: before its application has drawn.
fn size_of(row: Option<&wire::Window>, proposed: Option<(i32, i32)>) -> Option<(i32, i32)> {
    let row = row?;
    let own = |side: f64| row.shown.then(|| rounded(side)).filter(|side| *side > 0);
    let (width, height) = proposed.unwrap_or((0, 0));
    let width = if width > 0 {
        Some(width)
    } else {
        own(row.rect.w)
    }?;
    let height = if height > 0 {
        Some(height)
    } else {
        own(row.rect.h)
    }?;
    Some((width, height))
}

/// Whether a row changed in what the river window manager is told of it.
fn managed_differs(before: &wire::Window, after: &wire::Window) -> bool {
    before.title != after.title
        || before.app_id != after.app_id
        || before.parent != after.parent
        || before.min != after.min
        || before.max != after.max
        || before.fullscreen != after.fullscreen
        || before.maximized != after.maximized
}

/// The command that keeps a window from being drawn.
fn hidden(id: u64) -> wire::Command {
    wire::Command::Present(wire::Present {
        id,
        opacity: Some(0.0),
        pivot: wire::Pivot { x: 0.5, y: 0.5 },
        ..wire::Present::default()
    })
}

fn rounded(value: f64) -> i32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a window's size is a few thousand pixels"
    )]
    let rounded = value.round() as i32;
    rounded
}

fn rect(rect: &wire::Rect) -> (i32, i32, i32, i32) {
    (
        rounded(rect.x),
        rounded(rect.y),
        rounded(rect.w),
        rounded(rect.h),
    )
}

/// A window's least and greatest size, as river's `dimensions_hint` says
/// them: 0 for no limit.
fn limits(row: &wire::Window) -> (i32, i32, i32, i32) {
    let min = row.min.as_ref().map_or((0, 0), |size| (size.w, size.h));
    let max = row.max.as_ref().map_or((0, 0), |size| (size.w, size.h));
    (min.0, min.1, max.0, max.1)
}

/// A river binding's combination, as the compositor spells one: its
/// modifiers' words and its keysym's xkb name, in the one spelling.
pub(crate) fn combo(keysym: u32, modifiers: u32) -> Option<String> {
    let known: u32 = MODIFIERS.iter().map(|(bit, _)| bit).sum();
    if modifiers & !known != 0 {
        return None;
    }
    let mut spelled = String::new();
    for (bit, word) in MODIFIERS {
        if modifiers & bit != 0 {
            spelled.push_str(word);
            spelled.push('+');
        }
    }
    let name = smithay::input::keyboard::xkb::keysym_get_name(keysym.into());
    if name.is_empty() || name == "NoSymbol" {
        return None;
    }
    spelled.push_str(&name);
    Some(solium_wm_protocol::combo::normalise(&spelled))
}

/// A process's parent, from `/proc`.
fn parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name is in parentheses and may hold anything, spaces and
    // parentheses included: the fields after it are counted from its end.
    let after = &stat[stat.rfind(')')? + 1..];
    after.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_river_binding_is_spelled_as_the_compositor_spells_a_combination() {
        // super+q: Mod4 is 64, `q` is 0x71.
        assert_eq!(combo(0x71, 64).as_deref(), Some("super+q"));
        // shift+super+Return: Return is 0xff0d.
        assert_eq!(combo(0xff0d, 64 | 1).as_deref(), Some("shift+super+return"));
        // ctrl+alt+Escape.
        assert_eq!(combo(0xff1b, 4 | 8).as_deref(), Some("ctrl+alt+escape"));
        // Mod5 (128) has no word in a combination.
        assert_eq!(combo(0x71, 128), None);
    }

    #[test]
    fn a_process_knows_its_parent() {
        let me = std::process::id();
        let parent = parent_of(me).expect("a parent");
        assert!(parent > 0 && parent != me);
    }
}
