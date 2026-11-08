//! Bindings as Lua writes them: a press spec, and a binding's note and options.

use mlua::{Table, Value};

use crate::script::*;

/// The buttons `sol.press` names, with their evdev codes: `side` is
/// `BTN_SIDE`, a mouse's "back", and `extra` is `BTN_EXTRA`, its "forward".
pub(crate) const PRESS_BUTTONS: [(&str, u32); 5] = [
    ("left", 0x110),
    ("right", 0x111),
    ("middle", 0x112),
    ("side", 0x113),
    ("extra", 0x114),
];

/// `sol.press`'s spec: `{ button = name }` or `{ key = combo }`, one of the
/// two and nothing else. A combination is spelled as `sol.bind` spells it,
/// and normalised by the same [`normalise_combo`]; its key is an xkb keysym
/// name, in any case. Anything else is an error, so a spec that would press
/// nothing never looks like one that pressed something.
/// `directions::sol_press_names_a_button_or_a_key_and_refuses_what_it_cannot_reach`.
pub(crate) fn press_from(spec: &Table) -> mlua::Result<Press> {
    let refused = |why: String| {
        mlua::Error::runtime(format!(
            "sol.press: {why}; the spec is {{ button = \"left\"|\"right\"|\"middle\"|\"side\"|\"extra\" }} \
             or {{ key = \"Escape\" }}"
        ))
    };
    let mut button = None;
    let mut key = None;
    for pair in spec.pairs::<Value, Value>() {
        let (name, value) = pair?;
        let name = match &name {
            Value::String(name) => name.to_string_lossy(),
            other => {
                return Err(refused(format!(
                    "a key of {} is not a name",
                    other.type_name()
                )));
            }
        };
        match name.as_str() {
            "button" => button = Some(value),
            "key" => key = Some(value),
            other => return Err(refused(format!("`{other}` is not in it"))),
        }
    }
    match (button, key) {
        (Some(Value::String(name)), None) => {
            let name = name.to_string_lossy();
            PRESS_BUTTONS
                .iter()
                .find(|(each, _)| *each == name)
                .map(|(_, code)| Press::Button(*code))
                .ok_or_else(|| refused(format!("no button is called `{name}`")))
        }
        (None, Some(Value::String(combo))) => {
            let combo = combo.to_string_lossy();
            press_keysyms(&combo)
                .map(Press::Keys)
                .ok_or_else(|| refused(format!("`{combo}` names no key")))
        }
        (Some(_), Some(_)) => Err(refused("a button and a key at once".to_owned())),
        (None, None) => Err(refused("it names nothing".to_owned())),
        _ => Err(refused("a button or a key is a string".to_owned())),
    }
}

/// A combination's keysyms, in the order they go down: its modifiers, as
/// [`normalise_combo`] orders them and as the left-hand keys, then its key.
/// `None` for a combination with no key, or a key no keysym is called.
pub(crate) fn press_keysyms(combo: &str) -> Option<Vec<smithay::input::keyboard::Keysym>> {
    use smithay::input::keyboard::{Keysym, xkb};
    let canonical = normalise_combo(combo);
    let (modifiers, key) = canonical
        .rsplit_once('+')
        .unwrap_or(("", canonical.as_str()));
    if key.is_empty() {
        return None;
    }
    let mut keysyms: Vec<Keysym> = modifiers
        .split('+')
        .filter_map(|modifier| match modifier {
            "ctrl" => Some(Keysym::Control_L),
            "alt" => Some(Keysym::Alt_L),
            "shift" => Some(Keysym::Shift_L),
            "super" => Some(Keysym::Super_L),
            _ => None,
        })
        .collect();
    let keysym = xkb::keysym_from_name(key, xkb::KEYSYM_CASE_INSENSITIVE);
    if keysym == Keysym::NoSymbol {
        return None;
    }
    keysyms.push(keysym);
    Some(keysyms)
}

/// `sol.bind`'s third argument: nothing, a note, or a table holding the note
/// and what else the binding asks for (#224).
///
/// Anything else in the table is refused rather than ignored, as a setting
/// nothing reads is: a binding asking for something it does not get looks
/// exactly like one that is broken. `repeat` is Lua's own keyword, so the
/// one way to write it is `["repeat"]`, and its refusal names the spelling
/// that works. `binding_options::an_option_a_binding_does_not_have_is_refused_by_name`.
pub(crate) fn binding_note_and_options(
    said: Option<Value>,
) -> mlua::Result<(Option<String>, BindingOptions)> {
    let table = match said {
        None | Some(Value::Nil) => return Ok((None, BindingOptions::default())),
        Some(Value::String(note)) => {
            return Ok((Some(note.to_str()?.to_owned()), BindingOptions::default()));
        }
        Some(Value::Table(table)) => table,
        Some(other) => {
            return Err(mlua::Error::runtime(format!(
                "sol.bind: the third argument is a note or a table of options, not {}",
                other.type_name()
            )));
        }
    };
    let mut note = None;
    let mut asked = BindingOptions::default();
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let key = match &key {
            Value::String(key) => key.to_str()?.to_owned(),
            other => {
                return Err(mlua::Error::runtime(format!(
                    "sol.bind: an option is named by a word, not by a {}",
                    other.type_name()
                )));
            }
        };
        let flag = |value: &Value| match value {
            Value::Boolean(flag) => Ok(*flag),
            other => Err(mlua::Error::runtime(format!(
                "sol.bind: `{key}` is true or false, not {}",
                other.type_name()
            ))),
        };
        match key.as_str() {
            "note" => match value {
                Value::String(text) => note = Some(text.to_str()?.to_owned()),
                other => {
                    return Err(mlua::Error::runtime(format!(
                        "sol.bind: `note` is text, not {}",
                        other.type_name()
                    )));
                }
            },
            "repeating" => asked.repeating = flag(&value)?,
            "locked" => asked.locked = flag(&value)?,
            "inhibited" => asked.inhibited = flag(&value)?,
            "repeat" => {
                return Err(mlua::Error::runtime(
                    "sol.bind: a binding that repeats while held is `repeating = true`",
                ));
            }
            other => {
                return Err(mlua::Error::runtime(format!(
                    "sol.bind: there is no option `{other}`; a binding takes `note`, \
                     `repeating`, `locked` and `inhibited`"
                )));
            }
        }
    }
    Ok((note, asked))
}
