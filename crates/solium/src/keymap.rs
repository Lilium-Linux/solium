//! The keyboard: which layouts exist, which one is live, and how keys repeat.
//!
//! ## What was actually missing
//!
//! Not "the layout is hardcoded to US". It was not: the seat was created with
//! `XkbConfig::default()`, whose fields are empty strings, and xkbcommon reads
//! `XKB_DEFAULT_LAYOUT`, `XKB_DEFAULT_VARIANT`, `XKB_DEFAULT_MODEL`,
//! `XKB_DEFAULT_RULES` and `XKB_DEFAULT_OPTIONS` when they are. So
//! `XKB_DEFAULT_LAYOUT=ua,us` in front of the binary has always worked, and
//! was once written down in the docs as though it had not.
//!
//! What was missing is smaller and still real:
//!
//! * **No configuration.** An environment variable set in a wrapper script is
//!   not a setting; it is a thing you have to already know. Everything else in
//!   this compositor is configured in one Lua file and this was not in it.
//! * **The repeat rate was hardcoded** at 200 ms and 25 Hz, in the call that
//!   created the seat. xkbcommon has nothing to say about repeat rate, so no
//!   environment variable reached it and there was no way to change it at all.
//! * **No switching.** `grp:alt_shift_toggle` works because xkb implements it
//!   inside the keymap, but a script could not switch layouts, and a shell
//!   could not offer a menu of them.
//!
//! ## Reloading
//!
//! Applied on every configuration load, so `super+shift+r` changes the layout
//! without ending the session. Changing the keymap sends every client a new
//! one, which they are required to handle and universally do; changing only
//! the *active* layout does not recompile anything, because recompiling to
//! switch group would throw away the modifier state along with it.

use smithay::{
    backend::input::KeyState,
    input::keyboard::{KeyboardTarget, Keycode, Keysym, Layout, XkbConfig},
    utils::SERIAL_COUNTER,
};

use crate::state::Solium;

/// A keyboard configuration, as a script asks for it.
///
/// Every field is optional and absent means "leave it alone", so
/// `sol.keyboard{ active = 2 }` from a binding does not quietly reset the
/// repeat rate somebody set in their configuration.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Request {
    /// The five xkb names. `Some` only when a script named at least one of
    /// them, because compiling a keymap is the expensive half and doing it
    /// when nothing asked is how a layout switch loses its modifiers.
    pub(crate) keymap: Option<Keymap>,
    /// Repeats per second, and how long to wait before the first.
    pub(crate) repeat: Option<(i32, i32)>,
    /// Which layout to make live, one-based as a person counts them.
    pub(crate) active: Option<usize>,
    /// Caps Lock and Num Lock, on or off: `sol.keyboard{ caps = true }`.
    /// `tests::sol_keyboard_turns_the_locks_on_and_off_and_leaves_russian_live`.
    pub(crate) caps: Option<bool>,
    pub(crate) num: Option<bool>,
}

/// The five names xkb compiles a keymap from.
///
/// Empty is not the same as unset here: an empty string is what makes
/// xkbcommon fall back to the environment, which is the behaviour that was
/// already there and is worth keeping reachable rather than overriding with a
/// default of our own. A configuration that says nothing leaves the machine
/// exactly as it was.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Keymap {
    pub(crate) rules: String,
    pub(crate) model: String,
    pub(crate) layout: String,
    pub(crate) variant: String,
    pub(crate) options: Option<String>,
}

impl Keymap {
    fn config(&self) -> XkbConfig<'_> {
        XkbConfig {
            rules: &self.rules,
            model: &self.model,
            layout: &self.layout,
            variant: &self.variant,
            options: self.options.clone(),
        }
    }
}

/// What the keyboard currently is, for `sol.keyboard()` and for a shell.
#[derive(Clone, Debug, Default)]
pub(crate) struct State {
    /// Human-readable, in group order: "Ukrainian", "English (Dvorak)".
    pub(crate) layouts: Vec<String>,
    /// Each layout's short name, in the same order: "EN", "RU".
    /// `tests::a_layouts_short_name_is_the_one_xkbs_rules_give_it`.
    pub(crate) short: Vec<String>,
    /// One-based, to match how `sol.keyboard{ active = 2 }` reads.
    pub(crate) active: usize,
    /// Whether Caps Lock and Num Lock are on, as the keyboard's lights say.
    /// `tests::caps_and_num_are_what_the_keys_left_locked`.
    pub(crate) caps: bool,
    pub(crate) num: bool,
    pub(crate) repeat_rate: i32,
    pub(crate) repeat_delay: i32,
}

impl State {
    /// What a seat has before anything has configured it.
    ///
    /// The layouts are left empty rather than guessed: what they are depends
    /// on the environment xkbcommon read, and the answer arrives the first
    /// time anything asks the keymap -- `Solium::start_scripts` does, before
    /// a script can.
    pub(crate) fn initial() -> Self {
        Self {
            layouts: Vec::new(),
            short: Vec::new(),
            active: 1,
            caps: false,
            num: false,
            repeat_rate: REPEAT_RATE,
            repeat_delay: REPEAT_DELAY,
        }
    }
}

/// The repeat rate a keyboard starts with.
///
/// Was the only value there was. Kept as the default rather than lowered to
/// something the kernel would pick, because it is what every session on this
/// compositor has had and changing it silently would be a different bug.
pub(crate) const REPEAT_RATE: i32 = 25;
pub(crate) const REPEAT_DELAY: i32 = 200;

/// Apply what a script asked for.
///
/// Returns whether anything changed, so the caller can avoid telling clients
/// about a keymap that is the same one they already have — a reload that
/// re-sent the keymap every time would make every client rebuild its xkb state
/// for nothing, several times a second while somebody edits their config.
pub(crate) fn apply(state: &mut Solium, request: &Request) -> bool {
    let Some(keyboard) = state.seat.get_keyboard() else {
        return false;
    };
    let mut changed = false;

    if let Some(keymap) = request.keymap.as_ref()
        && state.keymap.as_ref() != Some(keymap)
    {
        match keyboard.set_xkb_config(state, keymap.config()) {
            Ok(()) => {
                state.keymap = Some(keymap.clone());
                changed = true;
            }
            // A keymap that will not compile is a typo in a configuration
            // file, and the session must survive one. The previous keymap is
            // still in force, which is the only safe place to stay: a
            // compositor that responded to a bad layout name by having no
            // keymap would be one you cannot type the correction into.
            Err(err) => tracing::error!(
                ?err,
                layout = keymap.layout,
                variant = keymap.variant,
                options = ?keymap.options,
                "that keyboard layout will not compile -- keeping the last one that did"
            ),
        }
    }

    if let Some((rate, delay)) = request.repeat
        && (rate, delay) != (state.keyboard.repeat_rate, state.keyboard.repeat_delay)
    {
        keyboard.change_repeat_info(rate, delay);
        // Written straight to the cache, because Smithay's keyboard has no
        // getter for it: what was last set is the only record there is, and
        // `describe` below reads this back out.
        state.keyboard.repeat_rate = rate;
        state.keyboard.repeat_delay = delay;
        changed = true;
    }

    if let Some(active) = request.active {
        // One-based outside, zero-based in xkb.
        let index = active.saturating_sub(1);
        let count = layouts(state).len();
        if index < count {
            keyboard.with_xkb_state(state, |mut context| {
                context.set_layout(Layout(index as u32));
            });
            changed = true;
        } else {
            tracing::warn!(
                asked = active,
                have = count,
                "no such keyboard layout -- the keymap has fewer than that"
            );
        }
    }

    for (wanted, lock) in [(request.caps, Lock::Caps), (request.num, Lock::Num)] {
        if let Some(on) = wanted {
            changed |= set_lock(state, lock, on);
        }
    }

    if changed {
        // One place, so nothing can change the keyboard and leave the cached
        // answer stale -- which a shell drawing a layout indicator would show
        // for as long as nobody else touched it.
        state.keyboard = describe(state);
    }
    changed
}

/// The layouts the current keymap holds, in group order.
pub(crate) fn layouts(state: &mut Solium) -> Vec<String> {
    let Some(keyboard) = state.seat.get_keyboard() else {
        return Vec::new();
    };
    keyboard.with_xkb_state(state, |context| {
        let xkb = context.xkb().lock().ok();
        xkb.map(|xkb| {
            xkb.layouts()
                .map(|layout| xkb.layout_name(layout).to_owned())
                .collect()
        })
        .unwrap_or_default()
    })
}

/// Everything a script or a shell can ask about the keyboard.
pub(crate) fn describe(state: &mut Solium) -> State {
    let layouts = layouts(state);
    // The short names are read from a file, so only when the layouts are not
    // the ones they were read for.
    let short = if layouts == state.keyboard.layouts && layouts.len() == state.keyboard.short.len()
    {
        state.keyboard.short.clone()
    } else {
        short_names(&layouts)
    };
    let (active, caps, num) = live(state);
    State {
        layouts,
        short,
        active,
        caps,
        num,
        repeat_rate: state.keyboard.repeat_rate,
        repeat_delay: state.keyboard.repeat_delay,
    }
}

impl State {
    /// The live layout's name, `""` before the keymap is known.
    /// `tests::sol_keyboard_names_the_live_layout_and_its_short_name`.
    pub(crate) fn layout(&self) -> &str {
        self.layouts
            .get(self.active.saturating_sub(1))
            .map_or("", String::as_str)
    }

    /// The live layout's short name: "EN", "RU".
    /// `tests::sol_keyboard_names_the_live_layout_and_its_short_name`.
    pub(crate) fn short_name(&self) -> &str {
        self.short
            .get(self.active.saturating_sub(1))
            .map_or("", String::as_str)
    }
}

/// The live layout, one-based, and whether Caps Lock and Num Lock are on:
/// what changes from one key to the next, read without walking the keymap.
/// `tests::caps_and_num_are_what_the_keys_left_locked`.
pub(crate) fn live(state: &mut Solium) -> (usize, bool, bool) {
    let Some(keyboard) = state.seat.get_keyboard() else {
        return (1, false, false);
    };
    // The locks as smithay last worked them out from the xkb state, which it
    // does on every key and every `set_layout`.
    let modifiers = keyboard.modifier_state();
    let group = keyboard.with_xkb_state(state, |context| {
        context
            .xkb()
            .lock()
            .ok()
            .map_or(0, |xkb| xkb.active_layout().0 as usize)
    });
    (group + 1, modifiers.caps_lock, modifiers.num_lock)
}

/// The two locks a script can turn on and off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lock {
    Caps,
    Num,
}

impl Lock {
    /// The key that toggles it, by keysym.
    const fn keysym(self) -> Keysym {
        match self {
            Self::Caps => Keysym::Caps_Lock,
            Self::Num => Keysym::Num_Lock,
        }
    }
}

/// Turn a lock on or off; whether it changed.
///
/// **By pressing its key**, inside the xkb state and nowhere else: the key
/// goes down and up through smithay's own `input_intercept`, which updates the
/// state exactly as the key on the keyboard would and forwards nothing, and
/// the window with the keyboard is then told its modifiers. So the lock does
/// whatever this keymap's own Caps Lock key does, the layout group is not
/// touched, and no binding sees a key. Setting the lock directly is not
/// sound: smithay's `set_modifier_state` puts the group back to the first
/// layout, because it masks the group's index with a state-component flag, so
/// Russian would become English under the user's fingers.
/// `tests::sol_keyboard_turns_the_locks_on_and_off_and_leaves_russian_live`.
///
/// A keymap with no such key, `caps:none` for one, cannot have the lock, and
/// says so in the log.
fn set_lock(state: &mut Solium, lock: Lock, on: bool) -> bool {
    let Some(keyboard) = state.seat.get_keyboard() else {
        return false;
    };
    let locked = |state: &mut Solium| {
        let (_, caps, num) = live(state);
        match lock {
            Lock::Caps => caps,
            Lock::Num => num,
        }
    };
    if locked(state) == on {
        return false;
    }
    let Some(code) = keycode_of(state, lock.keysym()) else {
        tracing::warn!(
            ?lock,
            "this keymap has no key for that lock, so it cannot be set"
        );
        return false;
    };
    for direction in [KeyState::Pressed, KeyState::Released] {
        keyboard.input_intercept::<(), _>(state, code, direction, |_, _, _| ());
    }
    if let Some(focus) = keyboard.current_focus() {
        let seat = state.seat.clone();
        focus.modifiers(
            &seat,
            state,
            keyboard.modifier_state(),
            SERIAL_COUNTER.next_serial(),
        );
    }
    locked(state) == on
}

/// The first key that types `keysym` with no modifier, in any layout of the
/// live keymap. For a lock, and for `SOLIUM_KEY_AT`, which presses keys by
/// name. `tests::sol_keyboard_turns_the_locks_on_and_off_and_leaves_russian_live`.
pub(crate) fn keycode_of(state: &mut Solium, keysym: Keysym) -> Option<Keycode> {
    let keyboard = state.seat.get_keyboard()?;
    keyboard.with_xkb_state(state, |context| {
        let xkb = context.xkb().lock().ok()?;
        let layouts: Vec<Layout> = xkb.layouts().collect();
        // xkb keycodes are evdev's plus eight, and evdev's stop at KEY_MAX,
        // 0x2ff.
        (8..=0x2ff + 8).map(Keycode::new).find(|&code| {
            layouts.iter().any(|&layout| {
                xkb.raw_syms_for_key_in_layout(code, layout)
                    .contains(&keysym)
            })
        })
    })
}

/// Each layout's short name, as xkb's own rules file gives it in its
/// `shortDescription`, upper-cased: "EN", "RU".
/// `tests::a_layouts_short_name_is_the_one_xkbs_rules_give_it`.
fn short_names(layouts: &[String]) -> Vec<String> {
    let rules = std::env::var_os("XKB_CONFIG_ROOT")
        .map_or_else(|| "/usr/share/X11/xkb".into(), std::path::PathBuf::from)
        .join("rules/evdev.xml");
    let xml = std::fs::read_to_string(rules).unwrap_or_default();
    layouts
        .iter()
        .map(|name| short_name_in(&xml, name))
        .collect()
}

/// One layout's short name, from xkb's rules file `xml`, where `name` is a
/// layout's or a variant's `description`. A variant with no short name of
/// its own takes its layout's; a name the rules do not have, or no rules at
/// all, is its first two letters.
/// `tests::a_layouts_short_name_is_the_one_xkbs_rules_give_it`.
fn short_name_in(xml: &str, name: &str) -> String {
    for layout in elements(xml, "layout") {
        let (own, variants) = layout.split_once("<variantList").unwrap_or((layout, ""));
        let own = elements(own, "configItem").next().unwrap_or_default();
        let short = text_of(own, "shortDescription");
        if text_of(own, "description").as_deref() == Some(name) {
            return short.map_or_else(|| first_letters(name), |short| short.to_uppercase());
        }
        for variant in elements(variants, "variant") {
            let item = elements(variant, "configItem").next().unwrap_or_default();
            if text_of(item, "description").as_deref() == Some(name) {
                return text_of(item, "shortDescription")
                    .or_else(|| short.clone())
                    .map_or_else(|| first_letters(name), |short| short.to_uppercase());
            }
        }
    }
    first_letters(name)
}

/// What stands for a layout the rules do not name: its first two letters.
fn first_letters(name: &str) -> String {
    name.chars().take(2).collect::<String>().to_uppercase()
}

/// The insides of every `<tag>` element in `xml`, in order, attributes or not.
/// Not nested: what this reads has no element inside one of its own name.
fn elements<'a>(xml: &'a str, tag: &str) -> impl Iterator<Item = &'a str> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut rest = xml;
    std::iter::from_fn(move || {
        loop {
            let at = rest.find(&open)?;
            let after = &rest[at + open.len()..];
            // `<layout` is also the start of `<layoutList>`.
            if !after.starts_with(['>', ' ', '\t', '\n']) {
                rest = after;
                continue;
            }
            let body = &after[after.find('>')? + 1..];
            let end = body.find(&close)?;
            rest = &body[end + close.len()..];
            return Some(&body[..end]);
        }
    })
}

/// The text of the first `<tag>` in `xml`, unescaped.
fn text_of(xml: &str, tag: &str) -> Option<String> {
    let text = elements(xml, tag).next()?.trim();
    Some(
        text.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&"),
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use smithay::backend::input::KeyState;
    use smithay::input::keyboard::{Layout, XkbConfig};
    use smithay::reexports::wayland_server::Display;

    use super::{describe, live, short_name_in};
    use crate::state::Solium;

    /// Caps Lock, Num Lock, the left Shift and Alt, and the A key, as xkb
    /// keycodes: evdev 58, 69, 42, 56 and 30, plus eight.
    pub(crate) const CAPS: u32 = 66;
    pub(crate) const NUM: u32 = 77;
    pub(crate) const SHIFT: u32 = 50;
    pub(crate) const ALT: u32 = 64;
    pub(crate) const A: u32 = 38;

    /// A real `Solium` on `us,ru` with Alt+Shift switching layout, its group
    /// locked to `group`: zero-based, so `1` is Russian. Read back, so a
    /// lock that did not take cannot leave a test about Russian asking `us`.
    pub(crate) fn us_ru(group: u32) -> (Display<Solium>, Solium) {
        let display = Display::<Solium>::new().expect("a test display");
        let mut state = Solium::new(display.handle());
        let keyboard = state.seat.get_keyboard().expect("the seat has a keyboard");
        keyboard
            .set_xkb_config(
                &mut state,
                XkbConfig {
                    layout: "us,ru",
                    options: Some("grp:alt_shift_toggle".to_owned()),
                    ..Default::default()
                },
            )
            .expect("compiling us,ru; xkb data is missing, so this proves nothing");
        let active = keyboard.with_xkb_state(&mut state, |mut context| {
            context.set_layout(Layout(group));
            context
                .xkb()
                .lock()
                .map(|xkb| xkb.active_layout().0)
                .expect("reading the group back")
        });
        assert_eq!(active, group, "locking us,ru to group {group} did not take");
        state.keyboard = describe(&mut state);
        (display, state)
    }

    /// Press `codes` in order and let go in reverse, through the real input
    /// path.
    pub(crate) fn tap(state: &mut Solium, codes: &[u32]) {
        for &code in codes {
            crate::input::key(state, code.into(), KeyState::Pressed, 0);
        }
        for &code in codes.iter().rev() {
            crate::input::key(state, code.into(), KeyState::Released, 0);
        }
    }

    /// **A layout's short name is the one xkb's rules give it**, upper-cased:
    /// `en` for English (US), and a variant without one of its own takes its
    /// layout's. A name the rules do not have, or no rules at all, is its
    /// first two letters.
    #[test]
    fn a_layouts_short_name_is_the_one_xkbs_rules_give_it() {
        let xml = r#"
            <layoutList>
              <layout>
                <configItem>
                  <name>us</name>
                  <shortDescription>en</shortDescription>
                  <description>English (US)</description>
                </configItem>
                <variantList>
                  <variant>
                    <configItem>
                      <name>dvorak</name>
                      <description>English (Dvorak)</description>
                    </configItem>
                  </variant>
                  <variant>
                    <configItem>
                      <name>chr</name>
                      <shortDescription>chr</shortDescription>
                      <description>Cherokee</description>
                    </configItem>
                  </variant>
                </variantList>
              </layout>
              <layout>
                <configItem popularity="exotic">
                  <name>ru</name>
                  <shortDescription>ru</shortDescription>
                  <description>Russian</description>
                </configItem>
              </layout>
              <layout>
                <configItem>
                  <name>xx</name>
                  <shortDescription>tw</shortDescription>
                  <description>This &amp; That</description>
                </configItem>
              </layout>
            </layoutList>
        "#;
        let named = |name: &str| short_name_in(xml, name);
        assert_eq!(named("English (US)"), "EN");
        assert_eq!(
            named("English (Dvorak)"),
            "EN",
            "a variant takes its layout's"
        );
        assert_eq!(
            named("Cherokee"),
            "CHR",
            "a variant that names its own keeps it"
        );
        assert_eq!(named("Russian"), "RU");
        assert_eq!(
            named("This & That"),
            "TW",
            "an escaped name is matched as it reads"
        );
        assert_eq!(
            named("Klingon"),
            "KL",
            "not in the rules: its first two letters"
        );
        assert_eq!(
            short_name_in("", "Russian"),
            "RU",
            "no rules: its first two letters"
        );
    }

    /// **`sol.keyboard()`'s keyboard names the live layout and its short
    /// name**, from the keymap and the machine's own xkb rules: Russian, RU.
    #[test]
    fn sol_keyboard_names_the_live_layout_and_its_short_name() {
        let (_display, mut state) = us_ru(1);
        let keyboard = describe(&mut state);
        assert_eq!(keyboard.layouts, ["English (US)", "Russian"]);
        assert_eq!(keyboard.short, ["EN", "RU"]);
        assert_eq!(keyboard.active, 2);
        assert_eq!(
            (keyboard.layout(), keyboard.short_name()),
            ("Russian", "RU")
        );
    }

    /// **Caps and Num are what the keys left locked**, with Russian active:
    /// on after one press of Caps Lock, off after the next, and Num the same,
    /// and the layout never moves.
    #[test]
    fn caps_and_num_are_what_the_keys_left_locked() {
        let (_display, mut state) = us_ru(1);
        assert_eq!(live(&mut state), (2, false, false));
        tap(&mut state, &[CAPS]);
        assert_eq!(live(&mut state), (2, true, false), "Caps Lock once");
        tap(&mut state, &[A]);
        assert_eq!(
            live(&mut state),
            (2, true, false),
            "a letter changes nothing"
        );
        tap(&mut state, &[NUM]);
        assert_eq!(live(&mut state), (2, true, true), "Num Lock once");
        tap(&mut state, &[CAPS]);
        tap(&mut state, &[NUM]);
        assert_eq!(live(&mut state), (2, false, false), "both pressed again");
        assert!(
            !describe(&mut state).caps,
            "the cached keyboard says Caps is off"
        );
    }

    /// **`sol.keyboard{ caps = true }` turns Caps Lock on, and `false` off
    /// again, and Num Lock the same, with Russian left live**: the lock is
    /// pressed as xkb would have it pressed, and the group it was in stays.
    /// Asking for a lock that is already so changes nothing.
    #[test]
    fn sol_keyboard_turns_the_locks_on_and_off_and_leaves_russian_live() {
        let (_display, mut state) = us_ru(1);
        let directory = std::env::temp_dir().join("solium-keymap-locks");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"
            sol.bind("super+c", function() sol.keyboard{ caps = true } end)
            sol.bind("super+shift+c", function() sol.keyboard{ caps = false } end)
            sol.bind("super+n", function() sol.keyboard{ num = true } end)
            sol.bind("super+shift+n", function() sol.keyboard{ num = false } end)
            "#,
        )
        .expect("writing the script");
        let scripts = crate::script::Scripts::load(&config).expect("loading the script");
        state.start_scripts(Some(scripts));
        let _ = std::fs::remove_dir_all(&directory);

        state.trigger("super+c");
        assert_eq!(live(&mut state), (2, true, false), "Caps on, Russian kept");
        state.trigger("super+c");
        assert_eq!(live(&mut state), (2, true, false), "already on");
        state.trigger("super+n");
        assert_eq!(live(&mut state), (2, true, true), "Num on");
        state.trigger("super+shift+c");
        state.trigger("super+shift+n");
        assert_eq!(
            live(&mut state),
            (2, false, false),
            "both off, Russian kept"
        );
        assert_eq!(
            (state.keyboard.caps, state.keyboard.num),
            (false, false),
            "the cached keyboard follows"
        );
    }
}
