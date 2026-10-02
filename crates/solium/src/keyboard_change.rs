//! Telling the configuration and the scenes when the keyboard's layout or a
//! lock changes: `sol.on("keyboard", function(state, changed) end)`, and the
//! `Keyboard` singleton's `changed(what)`.
//!
//! Told on a real change of the xkb group, Caps Lock or Num Lock, from a key
//! or from `sol.keyboard{ ... }`, and never for ordinary typing:
//! `tests::a_layout_switch_and_a_caps_toggle_by_key_are_told_once_each_with_russian_active`.

use crate::state::Solium;

/// What changed, as `sol.on("keyboard")`'s `changed` and the `Keyboard`
/// singleton's `changed(what)` name it.
/// `tests::a_layout_switch_and_a_caps_toggle_by_key_are_told_once_each_with_russian_active`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Change {
    Layout = 0,
    Caps = 1,
    Num = 2,
}

impl Change {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Layout => "layout",
            Self::Caps => "caps",
            Self::Num => "num",
        }
    }
}

/// The layout and the locks as they were last told, and how many times each
/// has changed since the compositor started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Told {
    /// The live layout, one-based, Caps and Num. `None` until the first look,
    /// and again once a keymap is compiled or a configuration starts, which
    /// are a new keyboard and not a change to the old one:
    /// `tests::a_new_keymap_and_a_starting_configuration_are_told_nothing`.
    last: Option<(usize, bool, bool)>,
    /// How many times each [`Change`] has been told, which is what the
    /// `Keyboard` singleton's `changed(what)` is sent from:
    /// `models::keyboard::tests::the_keyboard_singleton_changes_once_for_a_layout_switch_and_a_caps_toggle`.
    pub(crate) serials: [u64; 3],
    /// Whether a `keyboard` handler is running, so a change it makes itself
    /// is not told back to it:
    /// `tests::a_change_a_keyboard_handler_makes_is_not_told_back_to_it`.
    telling: bool,
}

impl Told {
    /// Look afresh at the next [`Solium::keyboard_changed`], telling nothing.
    /// `tests::a_new_keymap_and_a_starting_configuration_are_told_nothing`.
    pub(crate) fn forget(&mut self) {
        self.last = None;
    }
}

impl Solium {
    /// Wherever the xkb state may have changed: after every key, and after
    /// every `sol.keyboard{ ... }`. Refreshes the cached keyboard, and tells
    /// what changed since the last look, layout first.
    /// `tests::a_layout_switch_and_a_caps_toggle_by_key_are_told_once_each_with_russian_active`,
    /// `tests::sol_keyboard_active_is_told_as_a_layout_change`.
    pub(crate) fn keyboard_changed(&mut self) {
        let now = crate::keymap::live(self);
        (self.keyboard.active, self.keyboard.caps, self.keyboard.num) = now;
        let Some(was) = self.keyboard_told.last.replace(now) else {
            return;
        };
        let changes = [
            (Change::Layout, was.0 != now.0),
            (Change::Caps, was.1 != now.1),
            (Change::Num, was.2 != now.2),
        ];
        for (change, changed) in changes {
            if !changed {
                continue;
            }
            // The scenes reading the keyboard learn it at the next frame,
            // so there has to be one:
            // `models::keyboard::tests::the_keyboard_singleton_changes_once_for_a_layout_switch_and_a_caps_toggle`.
            self.redraw = true;
            self.keyboard_told.serials[change as usize] += 1;
            if !self.keyboard_told.telling {
                self.keyboard_told.telling = true;
                self.trigger_keyboard(change);
                self.keyboard_told.telling = false;
            }
        }
    }

    /// Run the `keyboard` listeners and apply what they asked for.
    fn trigger_keyboard(&mut self, change: Change) {
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return;
        };
        let outcome = scripts.keyboard_changed(change.name(), snapshot);
        self.scripts = Some(scripts);
        self.apply(outcome);
    }
}

#[cfg(test)]
mod tests {
    use crate::keymap::tests::{A, ALT, CAPS, NUM, SHIFT, tap, us_ru};
    use crate::script::Scripts;
    use crate::state::Solium;

    /// Every `keyboard` event as `changed:active:name:short:caps:num`, in
    /// `seen`, and a binding that switches to the second layout.
    const RECORDER: &str = r#"
        seen = {}
        sol.on("keyboard", function(state, changed)
            seen[#seen + 1] = string.format("%s:%d:%s:%s:%s:%s", changed, state.active,
                tostring(state.layout_name), tostring(state.layout_short),
                tostring(state.caps), tostring(state.num))
        end)
        sol.bind("super+k", function() sol.keyboard{ active = 2 } end)
        sol.bind("super+l", function() sol.keyboard{ layout = "us,ru,de" } end)
    "#;

    /// `script` loaded into `state` as its configuration.
    fn configured(state: &mut Solium, name: &str, script: &str) {
        let directory = std::env::temp_dir().join(format!("solium-keyboard-change-{name}"));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let config = directory.join("init.lua");
        std::fs::write(&config, script).expect("writing the script");
        let scripts = Scripts::load(&config).expect("loading the script");
        state.start_scripts(Some(scripts));
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// What the scripts have been told, and cleared.
    fn seen(state: &Solium) -> String {
        state.scripts.as_ref().map_or_else(String::new, |scripts| {
            scripts.evaluate("local all = table.concat(seen, ' '); seen = {}; return all")
        })
    }

    /// **A layout switch and a Caps toggle, each by key, are told once each,
    /// with `us,ru` and Russian active**, and a letter is told nothing. The
    /// state is the keyboard after the change.
    #[test]
    fn a_layout_switch_and_a_caps_toggle_by_key_are_told_once_each_with_russian_active() {
        let (_display, mut state) = us_ru(1);
        configured(&mut state, "keys", RECORDER);
        tap(&mut state, &[A]);
        assert_eq!(seen(&state), "", "a letter is no change");
        tap(&mut state, &[CAPS]);
        assert_eq!(seen(&state), "caps:2:Russian:RU:true:false");
        tap(&mut state, &[A]);
        assert_eq!(seen(&state), "", "a letter with Caps on is no change");
        tap(&mut state, &[CAPS]);
        assert_eq!(seen(&state), "caps:2:Russian:RU:false:false");
        tap(&mut state, &[NUM]);
        assert_eq!(seen(&state), "num:2:Russian:RU:false:true");
        tap(&mut state, &[ALT, SHIFT]);
        assert_eq!(seen(&state), "layout:1:English (US):EN:false:true");
        tap(&mut state, &[ALT, SHIFT]);
        assert_eq!(seen(&state), "layout:2:Russian:RU:false:true");
    }

    /// **`sol.keyboard{ active = 2 }` from a binding is told as a layout
    /// change**, and asking for the layout already live is told nothing.
    #[test]
    fn sol_keyboard_active_is_told_as_a_layout_change() {
        let (_display, mut state) = us_ru(0);
        configured(&mut state, "active", RECORDER);
        state.trigger("super+k");
        assert_eq!(seen(&state), "layout:2:Russian:RU:false:false");
        state.trigger("super+k");
        assert_eq!(seen(&state), "", "the layout already live");
    }

    /// **A new keymap and a starting configuration are told nothing**: they
    /// are a new keyboard, not a switch on the old one. The next key is told
    /// as ever.
    #[test]
    fn a_new_keymap_and_a_starting_configuration_are_told_nothing() {
        let (_display, mut state) = us_ru(1);
        configured(
            &mut state,
            "keymap",
            &format!("{RECORDER}\nsol.keyboard{{ active = 1 }}"),
        );
        assert_eq!(seen(&state), "", "the configuration's own start");
        state.trigger("super+k");
        assert_eq!(seen(&state), "layout:2:Russian:RU:false:false");
        state.trigger("super+l");
        assert_eq!(
            seen(&state),
            "",
            "a keymap compiled back to its first layout"
        );
        tap(&mut state, &[CAPS]);
        assert_eq!(seen(&state), "caps:1:English (US):EN:true:false");
    }

    /// **A change a `keyboard` handler makes itself is not told back to it**,
    /// so a handler that answers a switch with another cannot loop.
    #[test]
    fn a_change_a_keyboard_handler_makes_is_not_told_back_to_it() {
        let (_display, mut state) = us_ru(1);
        configured(
            &mut state,
            "loop",
            r#"
            seen = {}
            sol.on("keyboard", function(state, changed)
                seen[#seen + 1] = changed .. ":" .. state.active
                sol.keyboard{ active = state.active % 2 + 1 }
            end)
            "#,
        );
        tap(&mut state, &[ALT, SHIFT]);
        assert_eq!(
            seen(&state),
            "layout:1",
            "told once, and not of its own switch"
        );
        assert_eq!(
            state.keyboard.active, 2,
            "the handler's own switch still happened"
        );
    }
}
