//! PipeWire, on a thread of its own (03 §3.2.5, "Lifecycle").
//!
//! The thread runs PipeWire's own main loop. It binds every sink and source
//! node, and every audio device, with only the params it uses (`Props` for a
//! node, `Route` for a device), and the `default` metadata. It turns what
//! they say into [`Fact`]s for the event loop, and the event loop's
//! [`Command`]s into params set: a card's volume through its route, so the
//! hardware mixer moves and WirePlumber remembers it, and a node's own
//! `Props` only where there is no route.

use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::mpsc};

use pipewire as pw;
use pw::{
    spa::{
        self,
        param::ParamType,
        pod::{
            Object, Pod, Property, PropertyFlags, Value, ValueArray,
            deserialize::PodDeserializer, serialize::PodSerializer,
        },
    },
    types::ObjectType,
};

use super::{
    model::{Command, Direction, Fact, Kind, Seen},
    volume::Target,
};

/// The thread, from the event loop's side.
pub(crate) struct Thread {
    pub(crate) facts: mpsc::Receiver<Fact>,
    commands: pw::channel::Sender<Command>,
}

impl std::fmt::Debug for Thread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("audio::pw::Thread")
    }
}

impl Thread {
    /// Start it. `None` when no thread could be made.
    pub(crate) fn start() -> Option<Self> {
        let (facts_out, facts) = mpsc::channel();
        let (commands, commands_in) = pw::channel::channel();
        let spawned = std::thread::Builder::new()
            .name("solium-audio".to_owned())
            .spawn(move || {
                if let Err(err) = run(&facts_out, commands_in) {
                    tracing::warn!(%err, "PipeWire: audio is not available");
                }
                let _ = facts_out.send(Fact::Down);
            });
        match spawned {
            Ok(_) => Some(Self { facts, commands }),
            Err(err) => {
                tracing::warn!(?err, "no thread for PipeWire: audio is not available");
                None
            }
        }
    }

    pub(crate) fn send(&self, command: Command) {
        if self.commands.send(command).is_err() {
            tracing::debug!("PipeWire's thread has gone; a command was not sent");
        }
    }
}

/// One route of a card, as its `Route` param says.
#[derive(Clone, Debug, Default)]
struct Route {
    index: i32,
    /// The card's profile device this route drives: what a node's
    /// `card.profile.device` names.
    device: i32,
    output: bool,
    volumes: Vec<f32>,
    mute: bool,
    description: String,
    port_type: String,
}

struct NodeEntry {
    proxy: pw::node::Node,
    _listener: pw::node::NodeListener,
    name: String,
    label: String,
    direction: Direction,
    /// The PipeWire id of its device, and the profile device it is.
    device: Option<(u32, i32)>,
    volumes: Vec<f32>,
    mute: bool,
    state: String,
    is_virtual: bool,
    bus: String,
    form_factor: String,
}

struct DeviceEntry {
    proxy: pw::device::Device,
    _listener: pw::device::DeviceListener,
    routes: Vec<Route>,
    bus: String,
    form_factor: String,
}

struct State {
    nodes: HashMap<u32, NodeEntry>,
    devices: HashMap<u32, DeviceEntry>,
    metadata: Option<(pw::metadata::Metadata, pw::metadata::MetadataListener)>,
    defaults: (Option<String>, Option<String>),
    facts: mpsc::Sender<Fact>,
}

impl State {
    /// What the event loop is told of one node: its volume and mute from its
    /// card's route when it has one, else from its own `Props`.
    fn seen(&self, id: u32) -> Option<Seen> {
        let node = self.nodes.get(&id)?;
        let route = node.device.and_then(|(device, profile_device)| {
            let entry = self.devices.get(&device)?;
            let route = entry.routes.iter().find(|route| {
                route.device == profile_device
                    && route.output == (node.direction == Direction::Output)
            })?;
            Some((device, route, entry))
        });
        let (bus, form_factor) = match route.map(|(_, _, entry)| entry) {
            Some(entry) => (entry.bus.as_str(), entry.form_factor.as_str()),
            None => (node.bus.as_str(), node.form_factor.as_str()),
        };
        let port_type = route.map_or("", |(_, route, _)| route.port_type.as_str());
        Some(Seen {
            name: node.name.clone(),
            label: node.label.clone(),
            direction: node.direction,
            kind: Kind::from_props(form_factor, bus, port_type, node.is_virtual),
            channels: route.map_or_else(|| node.volumes.clone(), |(_, route, _)| route.volumes.clone()),
            muted: route.map_or(node.mute, |(_, route, _)| route.mute),
            step: None,
            route: route.map(|(device, route, _)| (device, route.index)),
            port_label: route.map_or_else(String::new, |(_, route, _)| route.description.clone()),
            port_kind: port_type.to_owned(),
            state: node.state.clone(),
        })
    }

    fn tell(&self, id: u32) {
        if let Some(seen) = self.seen(id)
            && !seen.channels.is_empty()
        {
            let _ = self.facts.send(Fact::Seen(seen));
        }
    }

    fn tell_device(&self, device: u32) {
        for (id, node) in &self.nodes {
            if node.device.is_some_and(|(of, _)| of == device) {
                self.tell(*id);
            }
        }
    }
}

fn run(facts: &mpsc::Sender<Fact>, commands: pw::channel::Receiver<Command>) -> Result<(), pw::Error> {
    pw::init();
    let main_loop = pw::main_loop::MainLoop::new(None)?;
    let context = pw::context::Context::new(&main_loop)?;
    let core = context.connect(None)?;
    let registry = Rc::new(core.get_registry()?);
    let state = Rc::new(RefCell::new(State {
        nodes: HashMap::new(),
        devices: HashMap::new(),
        metadata: None,
        defaults: (None, None),
        facts: facts.clone(),
    }));

    // Two round trips: the first lists the globals, the second their params.
    let rounds = Rc::new(RefCell::new(0u32));
    let pending_sync = Rc::new(RefCell::new(core.sync(0)?));
    let _core_listener = core
        .add_listener_local()
        .done({
            let rounds = rounds.clone();
            let pending_sync = pending_sync.clone();
            let core = core.clone();
            let facts = facts.clone();
            move |id, seq| {
                if id != pw::core::PW_ID_CORE || seq != *pending_sync.borrow() {
                    return;
                }
                let mut rounds = rounds.borrow_mut();
                *rounds += 1;
                if *rounds == 1 {
                    if let Ok(next) = core.sync(0) {
                        *pending_sync.borrow_mut() = next;
                    }
                } else if *rounds == 2 {
                    let _ = facts.send(Fact::Ready);
                }
            }
        })
        .error({
            let main_loop = main_loop.clone();
            move |id, _seq, res, message| {
                tracing::warn!(id, res, message, "PipeWire said");
                if id == pw::core::PW_ID_CORE {
                    main_loop.quit();
                }
            }
        })
        .register();

    let _registry_listener = registry
        .add_listener_local()
        .global({
            let registry = Rc::downgrade(&registry);
            let state = state.clone();
            move |global| {
                let Some(registry) = registry.upgrade() else {
                    return;
                };
                let Some(props) = global.props else {
                    return;
                };
                match global.type_ {
                    ObjectType::Node => {
                        let direction = match props.get("media.class") {
                            Some("Audio/Sink") => Direction::Output,
                            Some("Audio/Source") => Direction::Input,
                            _ => return,
                        };
                        bind_node(&registry, global, direction, &state);
                    }
                    ObjectType::Device => {
                        if props.get("media.class") == Some("Audio/Device") {
                            bind_device(&registry, global, &state);
                        }
                    }
                    ObjectType::Metadata => {
                        if props.get("metadata.name") == Some("default") {
                            bind_metadata(&registry, global, &state);
                        }
                    }
                    _ => {}
                }
            }
        })
        .global_remove({
            let state = state.clone();
            move |id| {
                let mut state = state.borrow_mut();
                if let Some(node) = state.nodes.remove(&id) {
                    let _ = state.facts.send(Fact::Gone(node.name));
                }
                if state.devices.remove(&id).is_some() {
                    // Its nodes go with it, each told on its own.
                }
            }
        })
        .register();

    let _commands = commands.attach(main_loop.loop_(), {
        let state = state.clone();
        move |command| perform(&state.borrow(), command)
    });

    main_loop.run();
    Ok(())
}

fn bind_node(
    registry: &pw::registry::Registry,
    global: &pw::registry::GlobalObject<&spa::utils::dict::DictRef>,
    direction: Direction,
    state: &Rc<RefCell<State>>,
) {
    let Ok(proxy) = registry.bind::<pw::node::Node, _>(global) else {
        return;
    };
    let id = global.id;
    let listener = proxy
        .add_listener_local()
        .info({
            let state = Rc::downgrade(state);
            move |info| {
                let Some(state) = state.upgrade() else { return };
                let mut borrowed = state.borrow_mut();
                let Some(node) = borrowed.nodes.get_mut(&id) else {
                    return;
                };
                if let Some(props) = info.props() {
                    if let Some(name) = props.get("node.name") {
                        name.clone_into(&mut node.name);
                    }
                    let label = props
                        .get("node.nick")
                        .filter(|nick| !nick.is_empty())
                        .or_else(|| props.get("node.description"))
                        .unwrap_or_default();
                    label.clone_into(&mut node.label);
                    node.device = props
                        .get("device.id")
                        .and_then(|device| device.parse().ok())
                        .zip(props.get("card.profile.device").and_then(|d| d.parse().ok()));
                    node.is_virtual = props.get("node.virtual") == Some("true")
                        || props.get("device.id").is_none();
                    node.bus = props.get("device.bus").unwrap_or_default().to_owned();
                    node.form_factor = props.get("device.form-factor").unwrap_or_default().to_owned();
                }
                node.state = match info.state() {
                    pw::node::NodeState::Running => "running",
                    pw::node::NodeState::Idle => "idle",
                    pw::node::NodeState::Suspended => "suspended",
                    pw::node::NodeState::Creating => "idle",
                    pw::node::NodeState::Error(_) => "error",
                }
                .to_owned();
                drop(borrowed);
                state.borrow().tell(id);
            }
        })
        .param({
            let state = Rc::downgrade(state);
            move |_seq, which, _index, _next, param| {
                if which != ParamType::Props {
                    return;
                }
                let Some(state) = state.upgrade() else { return };
                let Some((volumes, mute)) = param.and_then(props_of) else {
                    return;
                };
                let mut borrowed = state.borrow_mut();
                let Some(node) = borrowed.nodes.get_mut(&id) else {
                    return;
                };
                if let Some(volumes) = volumes {
                    node.volumes = volumes;
                }
                if let Some(mute) = mute {
                    node.mute = mute;
                }
                drop(borrowed);
                state.borrow().tell(id);
            }
        })
        .register();
    proxy.subscribe_params(&[ParamType::Props]);
    state.borrow_mut().nodes.insert(
        id,
        NodeEntry {
            proxy,
            _listener: listener,
            name: global.props.and_then(|p| p.get("node.name")).unwrap_or_default().to_owned(),
            label: String::new(),
            direction,
            device: None,
            volumes: Vec::new(),
            mute: false,
            state: "idle".to_owned(),
            is_virtual: false,
            bus: String::new(),
            form_factor: String::new(),
        },
    );
}

fn bind_device(
    registry: &pw::registry::Registry,
    global: &pw::registry::GlobalObject<&spa::utils::dict::DictRef>,
    state: &Rc<RefCell<State>>,
) {
    let Ok(proxy) = registry.bind::<pw::device::Device, _>(global) else {
        return;
    };
    let id = global.id;
    let props = global.props;
    let listener = proxy
        .add_listener_local()
        .param({
            let state = Rc::downgrade(state);
            move |_seq, which, index, _next, param| {
                if which != ParamType::Route {
                    return;
                }
                let Some(state) = state.upgrade() else { return };
                let Some(route) = param.and_then(route_of) else {
                    return;
                };
                let mut borrowed = state.borrow_mut();
                let Some(device) = borrowed.devices.get_mut(&id) else {
                    return;
                };
                // A device's routes come one param each; the first of a new
                // round (index 0) starts the list again.
                if index == 0 {
                    device.routes.clear();
                }
                device.routes.retain(|each| !(each.device == route.device && each.output == route.output));
                device.routes.push(route);
                drop(borrowed);
                state.borrow().tell_device(id);
            }
        })
        .register();
    proxy.subscribe_params(&[ParamType::Route]);
    state.borrow_mut().devices.insert(
        id,
        DeviceEntry {
            proxy,
            _listener: listener,
            routes: Vec::new(),
            bus: props.and_then(|p| p.get("device.bus")).unwrap_or_default().to_owned(),
            form_factor: props.and_then(|p| p.get("device.form-factor")).unwrap_or_default().to_owned(),
        },
    );
}

fn bind_metadata(
    registry: &pw::registry::Registry,
    global: &pw::registry::GlobalObject<&spa::utils::dict::DictRef>,
    state: &Rc<RefCell<State>>,
) {
    let Ok(proxy) = registry.bind::<pw::metadata::Metadata, _>(global) else {
        return;
    };
    let listener = proxy
        .add_listener_local()
        .property({
            let state = Rc::downgrade(state);
            move |_subject, key, _type, value| {
                let Some(state) = state.upgrade() else { return 0 };
                let mut borrowed = state.borrow_mut();
                let name = value.and_then(json_name);
                let changed = match key {
                    Some("default.audio.sink") => {
                        let changed = borrowed.defaults.0 != name;
                        borrowed.defaults.0 = name;
                        changed
                    }
                    Some("default.audio.source") => {
                        let changed = borrowed.defaults.1 != name;
                        borrowed.defaults.1 = name;
                        changed
                    }
                    _ => false,
                };
                if changed {
                    let (output, input) = borrowed.defaults.clone();
                    let _ = borrowed.facts.send(Fact::Defaults { output, input });
                }
                0
            }
        })
        .register();
    state.borrow_mut().metadata = Some((proxy, listener));
}

/// `{"name":"alsa_output…"}`, the value the default metadata keeps.
/// `the_default_metadata_s_json_names_the_node`.
fn json_name(value: &str) -> Option<String> {
    crate::json::Json::parse(value)?
        .get("name")?
        .as_str()
        .map(str::to_owned)
}

/// A `Props` param's channel volumes and mute, each when it carries one.
fn props_of(pod: &Pod) -> Option<(Option<Vec<f32>>, Option<bool>)> {
    let (_, Value::Object(object)) = PodDeserializer::deserialize_any_from(pod.as_bytes()).ok()? else {
        return None;
    };
    Some(props_values(&object))
}

fn props_values(object: &Object) -> (Option<Vec<f32>>, Option<bool>) {
    let mut volumes = None;
    let mut mute = None;
    for property in &object.properties {
        match (property.key, &property.value) {
            (spa::sys::SPA_PROP_channelVolumes, Value::ValueArray(ValueArray::Float(values))) => {
                volumes = Some(values.clone());
            }
            (spa::sys::SPA_PROP_mute, Value::Bool(on)) => mute = Some(*on),
            _ => {}
        }
    }
    (volumes, mute)
}

/// A `Route` param: its index, profile device, direction, volumes, mute,
/// description and port type.
fn route_of(pod: &Pod) -> Option<Route> {
    let (_, Value::Object(object)) = PodDeserializer::deserialize_any_from(pod.as_bytes()).ok()? else {
        return None;
    };
    let mut route = Route::default();
    for property in &object.properties {
        match (property.key, &property.value) {
            (spa::sys::SPA_PARAM_ROUTE_index, Value::Int(index)) => route.index = *index,
            (spa::sys::SPA_PARAM_ROUTE_device, Value::Int(device)) => route.device = *device,
            (spa::sys::SPA_PARAM_ROUTE_direction, Value::Id(direction)) => {
                route.output = direction.0 == spa::sys::SPA_DIRECTION_OUTPUT;
            }
            (spa::sys::SPA_PARAM_ROUTE_description, Value::String(text)) => {
                route.description.clone_from(text);
            }
            (spa::sys::SPA_PARAM_ROUTE_props, Value::Object(props)) => {
                let (volumes, mute) = props_values(props);
                route.volumes = volumes.unwrap_or_default();
                route.mute = mute.unwrap_or(false);
            }
            (spa::sys::SPA_PARAM_ROUTE_info, Value::Struct(fields)) => {
                // A count, then key and value strings in turn.
                let strings: Vec<&str> = fields
                    .iter()
                    .filter_map(|field| match field {
                        Value::String(text) => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                for pair in strings.chunks_exact(2) {
                    if pair[0] == "port.type" {
                        route.port_type = pair[1].to_owned();
                    }
                }
            }
            _ => {}
        }
    }
    Some(route)
}

fn serialized(value: &Value) -> Option<Vec<u8>> {
    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), value)
        .ok()
        .map(|(cursor, _)| cursor.into_inner())
}

fn props_object(id: u32, volumes: Option<&[f32]>, mute: Option<bool>) -> Object {
    let mut properties = Vec::new();
    if let Some(volumes) = volumes {
        properties.push(Property {
            key: spa::sys::SPA_PROP_channelVolumes,
            flags: PropertyFlags::empty(),
            value: Value::ValueArray(ValueArray::Float(volumes.to_vec())),
        });
    }
    if let Some(mute) = mute {
        properties.push(Property {
            key: spa::sys::SPA_PROP_mute,
            flags: PropertyFlags::empty(),
            value: Value::Bool(mute),
        });
    }
    Object {
        type_: spa::sys::SPA_TYPE_OBJECT_Props,
        id,
        properties,
    }
}

/// A command, done: a volume or a mute through the card's route (saved, so
/// WirePlumber remembers it) or the node's `Props`, or a new default.
fn perform(state: &State, command: Command) {
    let (name, target, volumes, mute) = match &command {
        Command::Volume { name, target, channels } => (name, *target, Some(channels.as_slice()), None),
        Command::Mute { name, target, muted } => (name, *target, None, Some(*muted)),
        Command::Default { direction, name } => {
            let key = match direction {
                Direction::Output => "default.configured.audio.sink",
                Direction::Input => "default.configured.audio.source",
            };
            let value = crate::json::Json::Object(std::collections::BTreeMap::from([(
                "name".to_owned(),
                crate::json::Json::Text(name.clone()),
            )]))
            .render();
            if let Some((metadata, _)) = &state.metadata {
                metadata.set_property(0, key, Some("Spa:String:JSON"), Some(&value));
            }
            return;
        }
    };
    let Some((id, node)) = state.nodes.iter().find(|(_, node)| node.name == *name) else {
        tracing::debug!(name, "no such endpoint; the command is dropped");
        return;
    };
    match target {
        Target::Route { device, index } => {
            let Some(entry) = state.devices.get(&device) else {
                return;
            };
            let Some(route) = entry.routes.iter().find(|route| route.index == index) else {
                return;
            };
            let object = Object {
                type_: spa::sys::SPA_TYPE_OBJECT_ParamRoute,
                id: spa::sys::SPA_PARAM_Route,
                properties: vec![
                    Property {
                        key: spa::sys::SPA_PARAM_ROUTE_index,
                        flags: PropertyFlags::empty(),
                        value: Value::Int(route.index),
                    },
                    Property {
                        key: spa::sys::SPA_PARAM_ROUTE_device,
                        flags: PropertyFlags::empty(),
                        value: Value::Int(route.device),
                    },
                    Property {
                        key: spa::sys::SPA_PARAM_ROUTE_props,
                        flags: PropertyFlags::empty(),
                        value: Value::Object(props_object(spa::sys::SPA_PARAM_Route, volumes, mute)),
                    },
                    Property {
                        key: spa::sys::SPA_PARAM_ROUTE_save,
                        flags: PropertyFlags::empty(),
                        value: Value::Bool(true),
                    },
                ],
            };
            if let Some(bytes) = serialized(&Value::Object(object))
                && let Some(pod) = Pod::from_bytes(&bytes)
            {
                entry.proxy.set_param(ParamType::Route, 0, pod);
            }
        }
        Target::Node => {
            let object = props_object(spa::sys::SPA_PARAM_Props, volumes, mute);
            if let Some(bytes) = serialized(&Value::Object(object))
                && let Some(pod) = Pod::from_bytes(&bytes)
            {
                node.proxy.set_param(ParamType::Props, 0, pod);
            }
        }
    }
    let _ = id;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_metadata_s_json_names_the_node() {
        assert_eq!(
            json_name(r#"{"name":"alsa_output.usb-headset"}"#).as_deref(),
            Some("alsa_output.usb-headset")
        );
        assert_eq!(json_name("not json"), None);
        assert_eq!(json_name(r#"{"other":1}"#), None);
    }

    #[test]
    fn a_route_volume_round_trips_through_a_pod() {
        let object = Object {
            type_: spa::sys::SPA_TYPE_OBJECT_ParamRoute,
            id: spa::sys::SPA_PARAM_Route,
            properties: vec![
                Property {
                    key: spa::sys::SPA_PARAM_ROUTE_index,
                    flags: PropertyFlags::empty(),
                    value: Value::Int(3),
                },
                Property {
                    key: spa::sys::SPA_PARAM_ROUTE_device,
                    flags: PropertyFlags::empty(),
                    value: Value::Int(1),
                },
                Property {
                    key: spa::sys::SPA_PARAM_ROUTE_props,
                    flags: PropertyFlags::empty(),
                    value: Value::Object(props_object(
                        spa::sys::SPA_PARAM_Route,
                        Some(&[0.125, 0.125]),
                        Some(true),
                    )),
                },
            ],
        };
        let bytes = serialized(&Value::Object(object)).expect("serialized");
        let route = route_of(Pod::from_bytes(&bytes).expect("a pod")).expect("a route");
        assert_eq!((route.index, route.device), (3, 1));
        assert_eq!(route.volumes, vec![0.125, 0.125]);
        assert!(route.mute);
    }
}
