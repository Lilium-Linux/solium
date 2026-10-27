//! The services hub (03 §3.1): the system data the compositor publishes to
//! scenes and to Lua, one service at a time, each one model that QML and Lua
//! both read, so a banner and a Lua rule can never disagree.
//!
//! Each service keeps its rules in a pure model (no bus, no Qt, time passed
//! in), talks to its daemon from threads of its own, and is driven from here,
//! once each time round the event loop, as `logind` and `idle` are: both
//! loops wake at least every 16 ms, and what a thread heard waits no longer
//! than that.

pub(crate) mod audio;
pub(crate) mod notifications;
pub(crate) mod tray;

use crate::{json::Json, state::Solium};

use notifications::{Joined, Origin, model::Decision};

/// Every service, owned by `Solium`.
#[derive(Debug, Default)]
pub(crate) struct Services {
    pub(crate) notifications: notifications::Service,
    pub(crate) audio: audio::Service,
    pub(crate) tray: tray::Service,
}

impl Services {
    pub(crate) fn new() -> Self {
        Self {
            notifications: notifications::Service::new(notifications::Service::state_file()),
            audio: audio::Service::default(),
            tray: tray::Service::default(),
        }
    }
}

/// Once a loop: what each service's threads heard, its timers, and the events
/// for Lua.
pub(crate) fn settle(state: &mut Solium) {
    settle_notifications(state);
    settle_audio(state);
    settle_tray(state);
}

fn settle_tray(state: &mut Solium) {
    state.services.tray.settle();
    state.services.tray.refresh_view();
    let events = std::mem::take(&mut state.services.tray.events);
    let view = state.services.tray.view();
    for event in events {
        let json = event.to_json(&view);
        let snapshot = state.snapshot();
        let Some(mut scripts) = state.scripts.take() else {
            break;
        };
        let outcome = scripts.tray_event(&json, snapshot);
        state.scripts = Some(scripts);
        state.apply(outcome);
    }
}

/// A `tray.*` action, from Lua or a scene (#363). Nothing behind the lock is
/// reached from it: an application's window or menu would open over it.
pub(crate) fn act_tray(
    state: &mut Solium,
    action: &str,
    data: &Json,
    origin: Origin,
) -> Result<(), &'static str> {
    if state.lock.is_some() {
        return Err("locked");
    }
    let activation = &mut state.activation_state;
    state.services.tray.act(action, data, origin, |item| {
        // A token the application can raise its window with, named for the
        // item's own id, so the window an activation opens may take the
        // keyboard, as a notification's action's may.
        let app_id = item.props.item_id.clone();
        let data = smithay::wayland::xdg_activation::XdgActivationTokenData {
            app_id: (!app_id.is_empty()).then_some(app_id),
            ..smithay::wayland::xdg_activation::XdgActivationTokenData::default()
        };
        let (token, _) = activation.create_external_token(data);
        Some(token.as_str().to_owned())
    })
}

fn settle_audio(state: &mut Solium) {
    let now = state.clock.now();
    state.services.audio.settle(now);
    state.services.audio.refresh_view();
    let events = std::mem::take(&mut state.services.audio.events);
    for event in events {
        let json = audio::event_json(&event);
        let snapshot = state.snapshot();
        let Some(mut scripts) = state.scripts.take() else {
            break;
        };
        let outcome = scripts.audio_event(&json, snapshot);
        state.scripts = Some(scripts);
        state.apply(outcome);
    }
}

/// Who asked, for audio's origin: a binding's handler, a scene, or a script.
pub(crate) fn audio_origin(source: crate::script::FocusSource) -> audio::volume::Origin {
    match source {
        crate::script::FocusSource::Binding => audio::volume::Origin::Binding,
        crate::script::FocusSource::Surface => audio::volume::Origin::Scene,
        _ => audio::volume::Origin::Script,
    }
}

/// An `audio.*` action, from Lua or a scene.
pub(crate) fn act_audio(
    state: &mut Solium,
    action: &str,
    data: &Json,
    origin: audio::volume::Origin,
) -> Result<(), &'static str> {
    let now = state.clock.now();
    state.services.audio.act(action, data, origin, now)
}

fn settle_notifications(state: &mut Solium) {
    let now = state.clock.now();
    for fact in state.services.notifications.take_facts() {
        match fact {
            notifications::server::Fact::Notify(incoming) => {
                arrive(state, *incoming, Origin::External);
            }
            notifications::server::Fact::Close { id } => {
                let effects = state.services.notifications.model.close_by_app(id);
                state
                    .services
                    .notifications
                    .apply_effects(effects, Origin::External, None);
            }
            notifications::server::Fact::Serving(serving) => {
                state.services.notifications.set_serving(serving);
            }
            notifications::server::Fact::Owner(owner) => {
                state.services.notifications.set_owner(owner);
            }
            notifications::server::Fact::Down => state.services.notifications.down(),
        }
    }
    let pending: Vec<notifications::Own> = {
        let pending = &mut state.services.notifications.pending_own;
        let take = pending.len().min(OWN_PER_SETTLE);
        pending.drain(..take).collect()
    };
    for own in pending {
        let id = state.services.notifications.allocate_own();
        if own.token != 0 {
            state.services.notifications.own.insert(id, own.token);
        }
        arrive(state, own.incoming(id), Origin::Script);
    }
    let paused = state.lock.is_some() || state.idle.blanked();
    state.services.notifications.tick(now, paused);
    state.services.notifications.refresh_view();
    let events = std::mem::take(&mut state.services.notifications.events);
    let view = state.services.notifications.view();
    for event in events {
        let json = event.to_json(&view);
        let snapshot = state.snapshot();
        let Some(mut scripts) = state.scripts.take() else {
            break;
        };
        let outcome = scripts.notifications_event(&json, snapshot);
        state.scripts = Some(scripts);
        state.apply(outcome);
    }
    // `sol.notify`'s functions: an invoked one runs; a closed one's are let go.
    let run = std::mem::take(&mut state.services.notifications.run_own);
    let forget = std::mem::take(&mut state.services.notifications.forget_own);
    for (token, key) in run {
        let snapshot = state.snapshot();
        let Some(mut scripts) = state.scripts.take() else {
            break;
        };
        let outcome = scripts.notify_action(token, &key, snapshot);
        state.scripts = Some(scripts);
        state.apply(outcome);
    }
    if !forget.is_empty()
        && let Some(scripts) = state.scripts.as_mut()
    {
        scripts.notify_forget(&forget);
    }
}

/// One arrival: joined to its window and app, decided by Lua (or the shipped
/// rule), then given to the model.
pub(crate) fn arrive(state: &mut Solium, incoming: notifications::model::Incoming, origin: Origin) {
    let now = state.clock.now();
    let joined = join(state, &incoming);
    let dnd = state.services.notifications.model.dnd();
    let fallback = Decision::default_for(&incoming, dnd);
    let arrival = notifications::arrival_json(&incoming, &joined, dnd);
    let snapshot = state.snapshot();
    let decision = match state.scripts.take() {
        Some(mut scripts) => {
            let (decided, outcome) = scripts.notification_decision(&arrival, snapshot);
            state.scripts = Some(scripts);
            state.apply(outcome);
            decided.map_or(fallback, |table| {
                notifications::decision_from(&table, fallback)
            })
        }
        None => fallback,
    };
    state
        .services
        .notifications
        .arrive(incoming, decision, joined, origin, now);
}

/// The window of the sender's process, or of its parent up to four
/// generations (a browser's helper, a terminal's shell), and the app its
/// `desktop-entry` names.
fn join(state: &Solium, incoming: &notifications::model::Incoming) -> Joined {
    let mut joined = Joined::default();
    if let Some(pid) = incoming.pid {
        let windows = state.snapshot().windows;
        let mut current = Some(pid);
        for _ in 0..5 {
            let Some(pid) = current else { break };
            if pid <= 1 {
                break;
            }
            if let Some(window) = windows.iter().find(|window| window.pid == Some(pid)) {
                joined.window = window.id;
                joined.monitor = window.monitor.clone();
                break;
            }
            current = parent_of(pid);
        }
    }
    let entry = incoming.desktop_entry.trim_end_matches(".desktop");
    if !entry.is_empty()
        && let Some(app) = state.apps.iter().find(|app| app.id == entry)
    {
        joined.app_name = app.name.clone();
        joined.app_icon = app.icon.clone();
    }
    joined
}

/// `/proc/<pid>/stat`'s fourth field.
fn parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name is in parentheses and may hold spaces: read after it.
    let after = &stat[stat.rfind(')')? + 1..];
    after.split_whitespace().nth(1)?.parse().ok()
}

/// A notifications action from Lua or a scene (`sol.act("notifications.…")`),
/// done here. Returns why it could not be done.
pub(crate) fn act(
    state: &mut Solium,
    action: &str,
    data: &Json,
    origin: Origin,
) -> Result<(), &'static str> {
    let now = state.clock.now();
    let id = || {
        data.get("id")
            .and_then(Json::as_u64)
            .and_then(|id| u32::try_from(id).ok())
            .ok_or("bad-data")
    };
    let service = &mut state.services.notifications;
    match action {
        "notifications.dismiss" => {
            let effects = if matches!(data.get("all"), Some(Json::Bool(true))) {
                service.model.dismiss_all()
            } else {
                let id = id()?;
                if service.model.get(id).is_none() {
                    return Err("unknown-notification");
                }
                service.model.dismiss(id)
            };
            service.apply_effects(effects, origin, None);
            Ok(())
        }
        "notifications.hold" | "notifications.release" => {
            let id = id()?;
            service.hold(id, action == "notifications.hold", now);
            Ok(())
        }
        "notifications.dnd" => {
            let on = match data.get("value") {
                Some(Json::Bool(on)) => *on,
                Some(Json::Text(text)) if text == "toggle" => !service.model.dnd(),
                _ => return Err("bad-data"),
            };
            service.set_dnd(on, origin);
            Ok(())
        }
        "notifications.invoke" => {
            // Nothing behind the lock is opened from it.
            if state.lock.is_some() {
                return Err("locked");
            }
            let id = id()?;
            let key = data
                .get("key")
                .and_then(Json::as_str)
                .unwrap_or("default")
                .to_owned();
            let Some(item) = service.model.get(id) else {
                return Err("unknown-notification");
            };
            if !item.incoming.actions.iter().any(|(each, _)| *each == key) {
                return Err("unknown-action");
            }
            let window = service.joined(id).map_or(0, |joined| joined.window);
            let app_id = item.incoming.desktop_entry.clone();
            let sender = item.incoming.sender.clone();
            let effects = service.model.invoke(id, &key);
            // A token the application can raise its window with, named for
            // its desktop entry, so a window it opens for the action may take
            // the keyboard (03 §3.2.11, "Activation").
            let token = {
                let data = smithay::wayland::xdg_activation::XdgActivationTokenData {
                    app_id: (!app_id.is_empty()).then_some(app_id),
                    ..smithay::wayland::xdg_activation::XdgActivationTokenData::default()
                };
                let (token, _) = state.activation_state.create_external_token(data);
                token.as_str().to_owned()
            };
            state.services.notifications.apply_effects(
                effects,
                origin,
                Some(notifications::Answer {
                    token: Some(token),
                    to: sender,
                }),
            );
            // The window that sent it is brought forward at once; an
            // application that opens another uses the token.
            if window != 0 && key == "default" {
                state.apply(crate::script::Outcome {
                    commands: vec![crate::script::Command::Focus {
                        id: window,
                        source: crate::script::FocusSource::Script,
                        warp: None,
                    }],
                    ..crate::script::Outcome::default()
                });
            }
            Ok(())
        }
        _ => Err("unknown-action"),
    }
}

/// `sol.notify{ ... }`: one of the shell's own, queued, to arrive at the
/// next settle through the same decision as any other. Never from here: a
/// `notification` hook that posts one would otherwise recurse until the stack
/// ran out.
pub(crate) fn notify_own(state: &mut Solium, own: &notifications::Own) {
    state.services.notifications.pending_own.push(own.clone());
}

/// How many of the shell's own notifications arrive in one settle.
const OWN_PER_SETTLE: usize = 16;
