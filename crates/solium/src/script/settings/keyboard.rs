//! `keyboard`: the keymap, key repeat, the active layout and the locks.

use super::read;
use crate::script::doc::Doc;

/// Read the section into what it asks of the keyboard. A key of the wrong
/// kind refuses the section, in the words Lua's conversions use.
pub(crate) fn parse(options: &Doc) -> Result<crate::keymap::Request, String> {
    let text = |key: &str| read::text(options, key);
    let rules = text("rules")?;
    let model = text("model")?;
    let layout = text("layout")?;
    let variant = text("variant")?;
    let keyboard_options = text("options")?;

    // A keymap is compiled only when a script named one of its parts.
    // Recompiling to change the active layout would throw away the modifier
    // state with it, and recompiling on every reload would make every client
    // rebuild its xkb state for nothing.
    let names = [&rules, &model, &layout, &variant, &keyboard_options];
    let keymap = names
        .iter()
        .any(|name| name.is_some())
        .then(|| crate::keymap::Keymap {
            rules: rules.clone().unwrap_or_default(),
            model: model.clone().unwrap_or_default(),
            layout: layout.clone().unwrap_or_default(),
            variant: variant.clone().unwrap_or_default(),
            // `None`, not `Some("")`. An empty string is a real instruction
            // to xkb meaning "no options at all", which is not the same as
            // "whatever the environment says".
            options: keyboard_options.clone().filter(|it| !it.is_empty()),
        });

    let rate = read::whole::<i32>(options, "repeat_rate", "i32")?;
    let delay = read::whole::<i32>(options, "repeat_delay", "i32")?;
    let repeat = match (rate, delay) {
        (None, None) => None,
        // One given and not the other keeps the other as it is, which is
        // what every other key in this table does.
        (rate, delay) => Some((
            rate.unwrap_or(crate::keymap::REPEAT_RATE),
            delay.unwrap_or(crate::keymap::REPEAT_DELAY),
        )),
    };

    Ok(crate::keymap::Request {
        keymap,
        repeat,
        active: read::whole::<usize>(options, "active", "usize")?,
        // `keymap::tests::sol_keyboard_turns_the_locks_on_and_off_and_leaves_russian_live`.
        caps: read::flag(options, "caps"),
        num: read::flag(options, "num"),
    })
}
