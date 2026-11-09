//! The queue a dispatch writes into, and the snapshot it reads, held in
//! the Lua state's app data for the length of the dispatch.

use mlua::{Lua, Value};
use solium_wm_protocol::mirror::Mirror;

use crate::script::lua::*;

/// The queue a script writes into. Lives in Lua's app data for the length of a
/// dispatch, so no compositor state is borrowed while Lua runs.
#[derive(Debug)]
pub(crate) struct Pending {
    /// What the handlers asked for, in the window manager protocol's terms.
    pub(crate) commands: Vec<solium_wm_protocol::Command>,
    pub(crate) animation: AnimationSpec,
    /// Whether `sol.animate` set `animation` in this dispatch. See
    /// [`Outcome::motion`].
    pub(crate) animated: bool,
    pub(crate) grab: Option<bool>,
    pub(crate) status: Option<String>,
    /// The window a `sol.move_direction` is moving, while its listeners run.
    /// See `Command::Place::moved`, and
    /// `a_fullscreen_or_maximised_window_moved_to_another_monitor_is_so_on_that_one`.
    pub(crate) moving: Option<u64>,
    /// What the handler running now is answering: what a `sol.focus` it
    /// makes is told with, and what `sol.focus_source()` says. See
    /// [`solium_wm_protocol::FocusCause`].
    pub(crate) source: solium_wm_protocol::FocusCause,
    /// Whether the handler running now is a binding fired at the lock screen,
    /// so that what it asks of a window is refused. See [`with_pending`].
    pub(crate) at_the_lock: bool,
}

impl Default for Pending {
    fn default() -> Self {
        Self {
            commands: Vec::new(),
            animation: AnimationSpec::default(),
            animated: false,
            grab: None,
            status: None,
            moving: None,
            source: solium_wm_protocol::FocusCause::Script,
            at_the_lock: false,
        }
    }
}

/// What the configuration being replaced asked to keep, waiting in the new Lua
/// state for `sol.keep` to claim it.
///
/// App data rather than a field on [`Host`], because `sol.keep` is called
/// while the configuration is still being *read* — the whole point is that a
/// script's top level has its state back before it does anything with it — and
/// at that moment there is no `Host` yet, only a `Lua`.
#[derive(Debug, Default)]
pub(crate) struct Carried(pub(crate) super::Keep);

/// A size as a script reads one, `{ w = ..., h = ... }`, or nil for none.
pub(crate) fn size_table(
    lua: &Lua,
    size: Option<&solium_wm_protocol::Size>,
) -> mlua::Result<Value> {
    let Some(size) = size else {
        return Ok(Value::Nil);
    };
    let table = lua.create_table()?;
    table.set("w", size.w)?;
    table.set("h", size.h)?;
    Ok(Value::Table(table))
}

/// The desktop as this window manager's mirror has it: the windows, the
/// monitors and the rest the compositor told it, as the protocol's rows.
pub(crate) fn mirror(lua: &Lua) -> Mirror {
    lua.app_data_ref::<Mirror>()
        .map(|mirror| mirror.clone())
        .unwrap_or_default()
}

/// The window with the keyboard, as the mirror has it.
pub(crate) fn focused_id(lua: &Lua) -> Option<u64> {
    lua.app_data_ref::<Mirror>()
        .and_then(|mirror| mirror.focused().map(|window| window.id))
}

/// Who answers a question only the compositor can: in the same process,
/// the compositor's own predicates over its snapshot; across a socket, a
/// query and its reply.
pub(crate) trait Ask {
    fn ask(&self, query: solium_wm_protocol::Query) -> Option<solium_wm_protocol::Reply>;
}

/// The [`Ask`] a handler's questions go to, in the Lua state's app data
/// while it runs.
pub(crate) struct Asker(pub(crate) Box<dyn Ask>);

impl std::fmt::Debug for Asker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Asker")
    }
}

/// Ask the compositor, or `None` when there is nobody to ask: outside a
/// dispatch.
pub(crate) fn ask(
    lua: &Lua,
    query: solium_wm_protocol::Query,
) -> Option<solium_wm_protocol::Reply> {
    lua.app_data_ref::<Asker>()
        .and_then(|asker| asker.0.ask(query))
}

impl Pending {
    /// Queue a command.
    pub(crate) fn push(&mut self, command: solium_wm_protocol::Command) {
        self.commands.push(command);
    }
}

pub(crate) fn with_pending(lua: &Lua, f: impl FnOnce(&mut Pending)) -> mlua::Result<()> {
    match lua.app_data_mut::<Pending>() {
        Some(mut pending) => {
            let before = pending.commands.len();
            f(&mut pending);
            // A binding fired at the lock screen, which it asked to be with
            // `locked` (#224), may not reach past it: what it asks of a window
            // or a workspace, or to end the session, is taken back and is an
            // error its handler can catch. Here, where every `sol` function
            // hands its commands over, so that no function can be the one
            // that forgot. `state::tests::real_client::lock_focus::held::a_binding_allowed_while_locked_cannot_act_on_windows`.
            if pending.at_the_lock
                && pending.commands[before..]
                    .iter()
                    .any(solium_wm_protocol::lock::reaches_past_the_lock)
            {
                pending.commands.truncate(before);
                return Err(mlua::Error::runtime(
                    "a binding at the lock screen cannot act on windows or end the session",
                ));
            }
            Ok(())
        }
        // Reachable only if a script squirrels a `sol` function away and calls
        // it outside a dispatch. Ignoring it is right: there is no batch for
        // the command to belong to.
        None => Err(mlua::Error::runtime(
            "sol functions may only be called from a handler",
        )),
    }
}
