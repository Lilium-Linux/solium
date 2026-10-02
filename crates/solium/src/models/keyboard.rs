//! The keyboard, for the `Keyboard` singleton every scene reads: the live
//! layout, its names, the locks, and which of them changed since Qt last took
//! a batch. Sent once a frame, and only when something moved:
//! `tests::the_keyboard_singleton_changes_once_for_a_layout_switch_and_a_caps_toggle`.

use std::collections::BTreeMap;

use crate::json::Json;
use crate::keyboard_change::Change;
use crate::state::Solium;

#[expect(unsafe_code, reason = "the Qt host is C++; this is its C ABI")]
mod ffi {
    use std::ffi::{c_char, c_int};

    unsafe extern "C" {
        pub(super) fn solium_qml_keyboard_publish(json: *const c_char) -> c_int;
    }
}

/// What Qt last took: the values, and how many of each change it has been
/// told of.
#[derive(Debug, Default)]
pub(crate) struct Published {
    values: Option<BTreeMap<String, Json>>,
    serials: [u64; 3],
}

/// Send the keyboard if it moved since Qt last took it. A batch Qt could not
/// take, before it has started, is sent again whole at the next frame.
/// `tests::the_keyboard_singleton_changes_once_for_a_layout_switch_and_a_caps_toggle`.
pub(crate) fn publish(state: &mut Solium, last: &mut Published) {
    let keyboard = &state.keyboard;
    let values = BTreeMap::from([
        (
            "layout".to_owned(),
            Json::Number(index(keyboard.active.saturating_sub(1))),
        ),
        (
            "layoutName".to_owned(),
            Json::Text(keyboard.layout().to_owned()),
        ),
        (
            "layoutShort".to_owned(),
            Json::Text(keyboard.short_name().to_owned()),
        ),
        (
            "layouts".to_owned(),
            Json::List(keyboard.layouts.iter().cloned().map(Json::Text).collect()),
        ),
        ("caps".to_owned(), Json::Bool(keyboard.caps)),
        ("num".to_owned(), Json::Bool(keyboard.num)),
    ]);
    let serials = state.keyboard_told.serials;
    let changed: Vec<Json> = [Change::Layout, Change::Caps, Change::Num]
        .into_iter()
        .filter(|&change| serials[change as usize] != last.serials[change as usize])
        .map(|change| Json::Text(change.name().to_owned()))
        .collect();
    if last.values.as_ref() == Some(&values) && changed.is_empty() {
        return;
    }
    let mut batch = values.clone();
    batch.insert("changed".to_owned(), Json::List(changed));
    if apply(&Json::Object(batch).render()) {
        *last = Published {
            values: Some(values),
            serials,
        };
    }
}

/// A layout's index as a JSON number.
fn index(at: usize) -> f64 {
    f64::from(u32::try_from(at).unwrap_or(u32::MAX))
}

#[expect(unsafe_code, reason = "calling into the Qt host")]
fn apply(json: &str) -> bool {
    // A value a binding reads can move a scene's items.
    crate::qml::hosted::touched();
    let Ok(json) = std::ffi::CString::new(json) else {
        return false;
    };
    // SAFETY: `json` outlives the call; the host copies what it keeps.
    unsafe { ffi::solium_qml_keyboard_publish(json.as_ptr()) != 0 }
}

#[cfg(test)]
mod tests {
    use crate::keymap::tests::{A, ALT, CAPS, SHIFT, tap, us_ru};

    /// **The `Keyboard` singleton changes once for a layout switch and once
    /// for a Caps toggle, each by key, with `us,ru` and Russian active**, and
    /// never for a letter. A scene reads the live layout, its short name and
    /// the lock, and hears `changed(what)` once for each.
    #[test]
    fn the_keyboard_singleton_changes_once_for_a_layout_switch_and_a_caps_toggle() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            let (directory, mut scene) = crate::qml::hosted::tests::hosted(
                "solium-models-keyboard",
                r#"
                import QtQuick
                import Solium
                Item {
                    property int layouts: 0
                    property int capses: 0
                    property int others: 0
                    readonly property int layout: Keyboard.layout
                    readonly property int russian: Keyboard.layoutShort === "RU"
                        && Keyboard.layoutName === "Russian"
                        && Keyboard.layouts[Keyboard.layout] === "Russian" ? 1 : 0
                    readonly property int caps: Keyboard.caps ? 1 : 0
                    Connections {
                        target: Keyboard
                        function onChanged(what) {
                            if (what === "layout") layouts++
                            else if (what === "caps") capses++
                            else others++
                        }
                    }
                }
                "#,
                "keyboard-1",
            );
            let (_display, mut state) = us_ru(1);
            let mut read = |state: &mut crate::state::Solium| {
                state.publish_models();
                ["layout", "russian", "caps", "layouts", "capses", "others"]
                    .map(|name| scene.get_int(name))
            };
            assert_eq!(
                read(&mut state),
                [1, 1, 0, 0, 0, 0],
                "Russian, told nothing"
            );

            tap(&mut state, &[A]);
            assert_eq!(read(&mut state), [1, 1, 0, 0, 0, 0], "a letter");

            tap(&mut state, &[CAPS]);
            assert_eq!(read(&mut state), [1, 1, 1, 0, 1, 0], "Caps on, once");
            assert_eq!(read(&mut state), [1, 1, 1, 0, 1, 0], "and not again");

            tap(&mut state, &[ALT, SHIFT]);
            assert_eq!(read(&mut state), [0, 0, 1, 1, 1, 0], "English, once");

            tap(&mut state, &[A]);
            tap(&mut state, &[CAPS]);
            assert_eq!(read(&mut state), [0, 0, 0, 1, 2, 0], "Caps off, once");
            drop(scene);
            let _ = std::fs::remove_dir_all(&directory);
        });
    }
}
