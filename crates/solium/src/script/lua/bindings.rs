//! Bindings as Lua writes them: a press spec, and a binding's note and options.

use mlua::{Lua, Table, Value};

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
pub(crate) fn press_from(spec: &Table) -> mlua::Result<solium_wm_protocol::Press> {
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
                .map(|(_, code)| solium_wm_protocol::Press::Button(*code))
                .ok_or_else(|| refused(format!("no button is called `{name}`")))
        }
        (None, Some(Value::String(combo))) => {
            let combo = combo.to_string_lossy();
            press_names(&combo)
                .map(solium_wm_protocol::Press::Keys)
                .ok_or_else(|| refused(format!("`{combo}` names no key")))
        }
        (Some(_), Some(_)) => Err(refused("a button and a key at once".to_owned())),
        (None, None) => Err(refused("it names nothing".to_owned())),
        _ => Err(refused("a button or a key is a string".to_owned())),
    }
}

/// A combination's keys, by their xkb names, in the order they go down: its
/// modifiers, as [`normalise_combo`] orders them and as the left-hand keys,
/// then its key, as xkb spells it. `None` for a combination with no key, or a
/// key no keysym is called.
pub(crate) fn press_names(combo: &str) -> Option<Vec<String>> {
    use xkbcommon::xkb;
    let canonical = normalise_combo(combo);
    let (modifiers, key) = canonical
        .rsplit_once('+')
        .unwrap_or(("", canonical.as_str()));
    if key.is_empty() {
        return None;
    }
    let mut names: Vec<String> = modifiers
        .split('+')
        .filter_map(|modifier| match modifier {
            "ctrl" => Some("Control_L"),
            "alt" => Some("Alt_L"),
            "shift" => Some("Shift_L"),
            "super" => Some("Super_L"),
            _ => None,
        })
        .map(str::to_owned)
        .collect();
    let keysym = xkb::keysym_from_name(key, xkb::KEYSYM_CASE_INSENSITIVE);
    if keysym.raw() == xkb::keysyms::KEY_NoSymbol {
        return None;
    }
    names.push(xkb::keysym_get_name(keysym));
    Some(names)
}

/// What a binding asked for besides its handler: the table form of
/// `sol.bind`'s third argument (#224), each off unless asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Asked {
    pub(crate) repeating: bool,
    pub(crate) locked: bool,
    pub(crate) inhibited: bool,
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
) -> mlua::Result<(Option<String>, Asked)> {
    let table = match said {
        None | Some(Value::Nil) => return Ok((None, Asked::default())),
        Some(Value::String(note)) => {
            return Ok((Some(note.to_str()?.to_owned()), Asked::default()));
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
    let mut asked = Asked::default();
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

/// Set when something the window manager declares has changed since it was
/// last sent: a binding made or taken away, a setting nothing reads, a note
/// for `--check`. The declarations are sent whole, and only then.
#[derive(Debug, Default)]
pub(crate) struct Undeclared;

/// Mark the declarations as changed, so the next answer carries them.
pub(crate) fn undeclared(lua: &Lua) {
    lua.set_app_data(Undeclared);
}

/// The declarations, when they changed since they were last sent.
pub(crate) fn declare_if_changed(lua: &Lua) -> Option<solium_wm_protocol::Command> {
    lua.remove_app_data::<Undeclared>()
        .map(|_| solium_wm_protocol::Command::Declare(declarations(lua)))
}

/// What this window manager declares, read from where `sol.bind`,
/// `sol.unbind` and `sol.unknown` keep it. Each list is sorted: a person
/// compares one run's `--check` to the next.
pub(crate) fn declarations(lua: &Lua) -> solium_wm_protocol::Declarations {
    let sol = lua.globals().get::<Table>("sol").ok();
    let field = |name: &str| sol.as_ref().and_then(|sol| sol.get::<Table>(name).ok());
    let bindings = field("_bindings");
    let sources = field("_binding_sources");
    let options = field("_binding_options");

    let mut bound: Vec<Binding> = bindings
        .iter()
        .flat_map(|bindings| bindings.pairs::<String, Value>().filter_map(Result::ok))
        .filter(|(_, handler)| matches!(handler, Value::Function(_)))
        .map(|(combo, _)| {
            let asked = options
                .as_ref()
                .and_then(|options| options.get::<Option<Table>>(combo.clone()).ok())
                .flatten();
            let flag = |name: &str| {
                asked
                    .as_ref()
                    .is_some_and(|asked| asked.get::<bool>(name).unwrap_or(false))
            };
            Binding {
                note: sources
                    .as_ref()
                    .and_then(|sources| sources.get::<Option<String>>(combo.clone()).ok())
                    .flatten(),
                repeating: flag("repeating"),
                locked: flag("locked"),
                inhibited: flag("inhibited"),
                combo,
            }
        })
        .collect();
    bound.sort_by(|left, right| left.combo.cmp(&right.combo));

    // A combination somebody said something about and which is now bound to
    // nothing, which is exactly `sol.unbind`: in a list of what survived, a
    // key missing on purpose looks identical to one that was never there.
    let mut unbound: Vec<Binding> = sources
        .iter()
        .flat_map(|sources| sources.pairs::<String, String>().filter_map(Result::ok))
        .filter(|(combo, _)| !bound.iter().any(|binding| &binding.combo == combo))
        .map(|(combo, note)| Binding {
            combo,
            note: Some(note),
            // Gone, so it asks for nothing.
            repeating: false,
            locked: false,
            inhibited: false,
        })
        .collect();
    unbound.sort_by(|left, right| left.combo.cmp(&right.combo));

    let mut unknown: Vec<UnknownSetting> = field("_unknown_settings")
        .iter()
        .flat_map(|unknown| unknown.sequence_values::<Table>().filter_map(Result::ok))
        .filter_map(|entry| {
            Some(UnknownSetting {
                key: entry.get::<String>("key").ok()?,
                meant: entry.get::<Option<String>>("meant").ok().flatten(),
            })
        })
        .collect();
    unknown.sort_by(|left, right| left.key.cmp(&right.key));

    solium_wm_protocol::Declarations {
        bindings: bound,
        unbound,
        unknown,
        // The Lua host has nothing of its own for a check to say.
        notes: Vec::new(),
    }
}
