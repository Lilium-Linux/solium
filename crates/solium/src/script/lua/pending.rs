//! The queue a dispatch writes into, and the snapshot it reads, held in
//! the Lua state's app data for the length of the dispatch.

use mlua::{Lua, Value};
use smithay::utils::{Logical, Size};

use crate::script::*;

/// The queue a script writes into. Lives in Lua's app data for the length of a
/// dispatch, so no compositor state is borrowed while Lua runs.
/// A command a handler asked for: one in the protocol a window manager
/// speaks, or one of the compositor's own that has not moved to it yet.
#[derive(Clone, Debug)]
pub(crate) enum Queued {
    Wire(solium_wm_protocol::Command),
    Internal(Command),
}

impl From<Command> for Queued {
    fn from(command: Command) -> Self {
        Self::Internal(command)
    }
}

impl From<solium_wm_protocol::Command> for Queued {
    fn from(command: solium_wm_protocol::Command) -> Self {
        Self::Wire(command)
    }
}

impl Queued {
    /// The attempt a `sol.act` command makes, if this is one.
    pub(crate) fn attempt(&self) -> Option<u64> {
        match self {
            Self::Internal(Command::Act { attempt, .. }) => Some(*attempt),
            Self::Wire(solium_wm_protocol::Command::Act((attempt, ..))) => Some(*attempt),
            _ => None,
        }
    }

    /// Whether a binding at the lock screen may not ask for it: see
    /// [`Command::reaches_past_the_lock`] and its twin for the protocol's
    /// commands, [`crate::wm::convert::reaches_past_the_lock`].
    pub(crate) fn reaches_past_the_lock(&self) -> bool {
        match self {
            Self::Wire(command) => crate::wm::convert::reaches_past_the_lock(command),
            Self::Internal(command) => command.reaches_past_the_lock(),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct Pending {
    pub(crate) commands: Vec<Queued>,
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
    /// [`FocusSource`].
    pub(crate) source: FocusSource,
    /// Whether the handler running now is a binding fired at the lock screen,
    /// so that what it asks of a window is refused. See [`with_pending`].
    pub(crate) at_the_lock: bool,
}

/// What the configuration being replaced asked to keep, waiting in the new Lua
/// state for `sol.keep` to claim it.
///
/// App data rather than a field on [`Scripts`], because `sol.keep` is called
/// while the configuration is still being *read* — the whole point is that a
/// script's top level has its state back before it does anything with it — and
/// at that moment there is no `Scripts` yet, only a `Lua`.
#[derive(Debug, Default)]
pub(crate) struct Carried(pub(crate) Keep);

/// A size as a script reads one, `{ w = ..., h = ... }`, or nil for none.
pub(crate) fn size_table(lua: &Lua, size: Option<Size<i32, Logical>>) -> mlua::Result<Value> {
    let Some(size) = size else {
        return Ok(Value::Nil);
    };
    let table = lua.create_table()?;
    table.set("w", size.w)?;
    table.set("h", size.h)?;
    Ok(Value::Table(table))
}

pub(crate) fn snapshot(lua: &Lua) -> mlua::Result<Snapshot> {
    Ok(lua
        .app_data_ref::<Snapshot>()
        .map(|snapshot| snapshot.clone())
        .unwrap_or_default())
}

/// The window with the keyboard in this dispatch's snapshot, if any.
pub(crate) fn focused_id(lua: &Lua) -> Option<u64> {
    lua.app_data_ref::<Snapshot>().and_then(|snapshot| {
        snapshot
            .windows
            .iter()
            .find(|window| window.focused)
            .map(|window| window.id)
    })
}

impl Pending {
    /// Queue a command.
    pub(crate) fn push(&mut self, command: impl Into<Queued>) {
        self.commands.push(command.into());
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
                    .any(Queued::reaches_past_the_lock)
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
