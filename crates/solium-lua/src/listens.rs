//! Which of the compositor's events the configuration listens to, said in
//! the protocol's `listen`, so that an event with no `sol.on` for it is
//! never sent: a configuration with no `drag` listener is not woken for
//! every motion of a dragged window.

use mlua::{Lua, Table, Value};
use solium_wm_protocol::Command;

/// Each event `sol.on` hears, by the name a configuration gives it, and
/// the protocol's name for it.
const HEARD: &[(&str, &str)] = &[
    ("open", "opened"),
    ("fullscreen", "fullscreen"),
    ("maximize", "maximize"),
    ("click", "click"),
    ("touch", "touch"),
    ("focus", "focused"),
    ("keyboard", "keyboard-changed"),
    ("lock", "lock-changed"),
    ("text_input", "text-input"),
    ("activate", "activated"),
    ("rename", "renamed"),
    ("size", "sized"),
    ("resize", "resized"),
    ("close", "closed"),
    ("closing", "closing"),
    ("close_landed", "close-landed"),
    ("refused", "refused"),
    ("notification", "notification"),
    ("notifications", "notifications"),
    ("audio", "audio"),
    ("tray", "tray"),
    ("media", "media"),
    ("minimize", "minimized"),
    ("unminimize", "unminimized"),
    ("monitors", "monitors-changed"),
    ("problems", "problems-changed"),
    ("restore", "restored"),
    ("frame", "frame-action"),
    ("surface", "surface-action"),
    ("layout", "relayout"),
    ("drag", "dragged"),
    ("resizing", "resizing"),
    ("drop", "dropped"),
    ("scroll", "scrolled"),
];

/// Heard whatever the configuration listens to: the stores and the keeps
/// are the runtime's to save, not a listener's.
const RUNTIME: &[&str] = &["save-stores"];

/// The list last sent.
#[derive(Debug, Default)]
pub(crate) struct ListensSent(Vec<String>);

/// The events to be sent, in the protocol's names, sorted: each with a
/// listener, and the runtime's own.
pub fn listens(lua: &Lua) -> Vec<String> {
    let handlers = lua
        .globals()
        .get::<Table>("sol")
        .and_then(|sol| sol.get::<Table>("_handlers"))
        .ok();
    let mut names: Vec<String> = HEARD
        .iter()
        .filter(|(heard, _)| {
            handlers.as_ref().is_some_and(|handlers| {
                matches!(
                    handlers.get::<Value>(*heard),
                    Ok(Value::Table(listeners)) if listeners.raw_len() > 0
                )
            })
        })
        .map(|(_, event)| (*event).to_owned())
        .chain(RUNTIME.iter().map(|event| (*event).to_owned()))
        .collect();
    names.sort_unstable();
    names
}

/// The list, whole, as a load's answer says it.
pub fn listen(lua: &Lua) -> Command {
    let names = listens(lua);
    lua.set_app_data(ListensSent(names.clone()));
    Command::Listen(names)
}

/// The list, when it changed since it was last sent: a handler listened to
/// something new, or the last listener of an event was taken out.
pub fn listen_if_changed(lua: &Lua) -> Option<Command> {
    let names = listens(lua);
    let same = lua
        .app_data_ref::<ListensSent>()
        .is_some_and(|sent| sent.0 == names);
    (!same).then(|| listen(lua))
}

#[cfg(test)]
mod tests {
    use super::HEARD;

    /// Every name the list gives the protocol is an event of the
    /// protocol's that is listened to, and each is given once: a misspelt
    /// one would be an event the configuration never hears.
    #[test]
    fn every_event_a_configuration_hears_is_one_the_protocol_has() {
        let listenable: Vec<&str> = solium_wm_protocol::listen::listenable().collect();
        let mut given: Vec<&str> = HEARD.iter().map(|(_, event)| *event).collect();
        for event in &given {
            assert!(
                listenable.contains(event),
                "no event called {event} to listen to"
            );
        }
        given.push("save-stores");
        given.sort_unstable();
        let mut all = listenable.clone();
        all.sort_unstable();
        assert_eq!(
            given, all,
            "an event of the protocol's that no `sol.on` hears"
        );
    }
}
