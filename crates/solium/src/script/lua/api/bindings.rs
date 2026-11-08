//! Key bindings, and the listeners events go to.

use mlua::{Lua, Table, Value};

use crate::script::lua::*;
use crate::script::*;

/// Add this area's functions to `sol`.
pub(crate) fn register(lua: &Lua, sol: &Table) -> mlua::Result<()> {
    // The later call wins, and always has: two scripts binding one combination
    // is how `init.lua` and `scrolling.lua` coexist, and the alternative --
    // refusing the second -- would make the order files happen to be required
    // in decide which of them works.
    //
    // The third argument is optional and is a *note*, not a name: a short
    // phrase saying where this binding came from, which `solium --check` prints
    // beside the combination. It exists because that "later call wins" rule is
    // the whole mechanism behind `config.bindings` replacing a shipped binding
    // (#117), and a replacement nobody can see is indistinguishable from a key
    // that mysteriously stopped working. Shipped bindings pass nothing, so
    // `--check` stays a plain list until a configuration has something to say.
    //
    // Or the third argument is a table, which holds the note and what else the
    // binding asks for (#224): `repeating`, `locked`, and `inhibited` (#58).
    // What a binding asks for
    // is replaced with it, so a later call that asks for nothing asks for
    // nothing -- a configuration rebinding a shipped repeating key to launch
    // something does not inherit the repeat.
    sol.set(
        "bind",
        lua.create_function(
            |lua, (combo, handler, said): (String, mlua::Function, Option<Value>)| {
                let (note, asked) = binding_note_and_options(said)?;
                let sol: Table = lua.globals().get("sol")?;
                let bindings: Table = sol.get("_bindings")?;
                let combo = normalise_combo(&combo);
                bindings.set(combo.clone(), handler)?;
                let options: Table = sol.get("_binding_options")?;
                if asked == BindingOptions::default() {
                    options.set(combo.clone(), Value::Nil)?;
                } else {
                    let table = lua.create_table()?;
                    table.set("repeating", asked.repeating)?;
                    table.set("locked", asked.locked)?;
                    table.set("inhibited", asked.inhibited)?;
                    options.set(combo.clone(), table)?;
                }
                if let Some(note) = note {
                    let sources: Table = sol.get("_binding_sources")?;
                    sources.set(combo, note)?;
                }
                Ok(())
            },
        )?,
    )?;

    // Whether a combination is already bound, in its canonical spelling.
    //
    // Asking is the only way a script can tell that it is about to replace
    // somebody's binding rather than add one, and normalising is why it cannot
    // do this by reading `_bindings` itself: `Super+Q` and `shift+super+q` are
    // spellings, not keys, and only this side knows which spelling won.
    sol.set(
        "bound",
        lua.create_function(|lua, combo: String| {
            let sol: Table = lua.globals().get("sol")?;
            let bindings: Table = sol.get("_bindings")?;
            Ok(matches!(
                bindings.get::<Value>(normalise_combo(&combo))?,
                Value::Function(_)
            ))
        })?,
    )?;

    // Take a binding away.
    //
    // The counterpart to a configuration being able to replace one. Without it
    // a shipped binding is unremovable short of replacing `init.lua` -- and
    // binding it to a function that does nothing is not the same thing, because
    // the compositor would still swallow the key and `--check` would still list
    // it as bound.
    //
    // The note is kept even though the binding is gone, and that is the point
    // of keeping it: `--check` reads the combinations in `_binding_sources`
    // that are no longer in `_bindings` and reports them as deliberately
    // removed, so an absence is stated rather than merely being an absence.
    sol.set(
        "unbind",
        lua.create_function(|lua, (combo, note): (String, Option<String>)| {
            let sol: Table = lua.globals().get("sol")?;
            let bindings: Table = sol.get("_bindings")?;
            let combo = normalise_combo(&combo);
            bindings.set(combo.clone(), Value::Nil)?;
            let options: Table = sol.get("_binding_options")?;
            options.set(combo.clone(), Value::Nil)?;
            if let Some(note) = note {
                let sources: Table = sol.get("_binding_sources")?;
                sources.set(combo, note)?;
            }
            Ok(())
        })?,
    )?;

    // Appended, never replaced. Two scripts caring about the same event is
    // ordinary — a tiling script and an open animation both want to know a
    // window appeared — and the second one silently unhooking the first is the
    // kind of bug that gets blamed on the compositor.
    sol.set(
        "on",
        lua.create_function(|lua, (event, handler): (String, mlua::Function)| {
            let sol: Table = lua.globals().get("sol")?;
            let handlers: Table = sol.get("_handlers")?;
            let listeners: Table = match handlers.get::<Value>(event.clone())? {
                Value::Table(existing) => existing,
                _ => {
                    let created = lua.create_table()?;
                    handlers.set(event, &created)?;
                    created
                }
            };
            listeners.push(handler)?;
            Ok(())
        })?,
    )?;

    Ok(())
}
