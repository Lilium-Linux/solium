//! The keyboard: which layouts exist, which one is live, and how keys repeat.
//!
//! ## What was actually missing
//!
//! Not "the layout is hardcoded to US". It was not: the seat was created with
//! `XkbConfig::default()`, whose fields are empty strings, and xkbcommon reads
//! `XKB_DEFAULT_LAYOUT`, `XKB_DEFAULT_VARIANT`, `XKB_DEFAULT_MODEL`,
//! `XKB_DEFAULT_RULES` and `XKB_DEFAULT_OPTIONS` when they are. So
//! `XKB_DEFAULT_LAYOUT=ua,us` in front of the binary has always worked, and
//! this was written down as though it had not — see the note in `docs/gaps.md`.
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

use smithay::input::keyboard::{Layout, XkbConfig};

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
    /// One-based, to match how `sol.keyboard{ active = 2 }` reads.
    pub(crate) active: usize,
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
            active: 1,
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
    let active = state
        .seat
        .get_keyboard()
        .map(|keyboard| {
            keyboard.with_xkb_state(state, |context| {
                context
                    .xkb()
                    .lock()
                    .ok()
                    .map_or(0, |xkb| xkb.active_layout().0 as usize)
            })
        })
        .unwrap_or(0);
    State {
        layouts,
        active: active + 1,
        repeat_rate: state.keyboard.repeat_rate,
        repeat_delay: state.keyboard.repeat_delay,
    }
}
