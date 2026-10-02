//! The compositor's input in Qt's terms: mouse buttons and keyboard
//! modifiers (`tests::the_mouse_buttons_map_onto_qt_bit_for_bit`,
//! `tests::the_modifiers_map_onto_qt`), and keys
//! (`tests::the_named_keys_map_to_qt_keys`,
//! `tests::a_cyrillic_letter_maps_to_its_uppercase_codepoint`).

use smithay::input::keyboard::{Keysym, ModifiersState};

/// `Qt::ShiftModifier` and its siblings (qnamespace.h).
pub(crate) const QT_SHIFT: u32 = 0x0200_0000;
pub(crate) const QT_CONTROL: u32 = 0x0400_0000;
pub(crate) const QT_ALT: u32 = 0x0800_0000;
pub(crate) const QT_META: u32 = 0x1000_0000;

/// The modifiers held, as Qt's `KeyboardModifiers`. `tests::the_modifiers_map_onto_qt`.
pub(crate) fn qt_modifiers(modifiers: &ModifiersState) -> u32 {
    [
        (modifiers.shift, QT_SHIFT),
        (modifiers.ctrl, QT_CONTROL),
        (modifiers.alt, QT_ALT),
        (modifiers.logo, QT_META),
    ]
    .into_iter()
    .filter(|(held, _)| *held)
    .fold(0, |all, (_, bit)| all | bit)
}

/// A kernel mouse button as Qt's `MouseButton`, or `None` past the mouse
/// range. `tests::the_mouse_buttons_map_onto_qt_bit_for_bit`,
/// `tests::a_button_past_the_mouse_range_is_not_delivered`.
pub(crate) fn qt_button(code: u32) -> Option<u32> {
    code.checked_sub(0x110)
        .filter(|bit| *bit < 16)
        .map(|bit| 1 << bit)
}

/// The named keys, as Qt numbers them (qnamespace.h, `Qt::Key_Escape` on).
const NAMED: [(Keysym, i32); 30] = [
    (Keysym::Escape, 0x0100_0000),
    (Keysym::Tab, 0x0100_0001),
    (Keysym::ISO_Left_Tab, 0x0100_0002),
    (Keysym::BackSpace, 0x0100_0003),
    (Keysym::Return, 0x0100_0004),
    (Keysym::KP_Enter, 0x0100_0005),
    (Keysym::Insert, 0x0100_0006),
    (Keysym::Delete, 0x0100_0007),
    (Keysym::Pause, 0x0100_0008),
    (Keysym::Print, 0x0100_0009),
    (Keysym::Home, 0x0100_0010),
    (Keysym::End, 0x0100_0011),
    (Keysym::Left, 0x0100_0012),
    (Keysym::Up, 0x0100_0013),
    (Keysym::Right, 0x0100_0014),
    (Keysym::Down, 0x0100_0015),
    (Keysym::Page_Up, 0x0100_0016),
    (Keysym::Page_Down, 0x0100_0017),
    (Keysym::Shift_L, 0x0100_0020),
    (Keysym::Shift_R, 0x0100_0020),
    (Keysym::Control_L, 0x0100_0021),
    (Keysym::Control_R, 0x0100_0021),
    (Keysym::Alt_L, 0x0100_0023),
    (Keysym::Alt_R, 0x0100_0023),
    (Keysym::Caps_Lock, 0x0100_0024),
    (Keysym::Num_Lock, 0x0100_0025),
    (Keysym::Scroll_Lock, 0x0100_0026),
    (Keysym::Super_L, 0x0100_0053),
    (Keysym::Super_R, 0x0100_0054),
    (Keysym::Menu, 0x0100_0055),
];

/// `Qt::Key_unknown`.
const UNKNOWN: i32 = 0x01ff_ffff;

/// A key as Qt names it: a named key's `Qt::Key`, F1 to F35 counted from
/// `Qt::Key_F1`, or else the upper case of what it types, which is how Qt
/// names a printable key, Cyrillic ones included; `Qt::Key_unknown` for
/// anything else.
/// `tests::the_named_keys_map_to_qt_keys`,
/// `tests::a_cyrillic_letter_maps_to_its_uppercase_codepoint`,
/// `tests::a_named_key_that_types_a_control_character_keeps_its_name`.
pub(crate) fn qt_key(keysym: Keysym, text: &str) -> i32 {
    if let Some((_, key)) = NAMED.iter().find(|(sym, _)| *sym == keysym) {
        return *key;
    }
    let raw = keysym.raw();
    if (Keysym::F1.raw()..=Keysym::F35.raw()).contains(&raw) {
        return i32::try_from(raw - Keysym::F1.raw())
            .map_or(UNKNOWN, |offset| 0x0100_0030 + offset);
    }
    text.chars()
        .next()
        .filter(|character| !character.is_control())
        .and_then(|character| character.to_uppercase().next())
        .and_then(|upper| i32::try_from(u32::from(upper)).ok())
        .unwrap_or(UNKNOWN)
}

#[cfg(test)]
mod tests {
    use smithay::input::keyboard::{Keysym, ModifiersState};

    use super::{QT_ALT, QT_CONTROL, QT_META, QT_SHIFT, qt_button, qt_key, qt_modifiers};

    #[test]
    fn a_latin_letter_maps_to_its_uppercase_qt_key() {
        assert_eq!(qt_key(Keysym::q, "q"), 0x51, "Qt::Key_Q");
        assert_eq!(qt_key(Keysym::_1, "1"), 0x31, "Qt::Key_1");
        assert_eq!(qt_key(Keysym::space, " "), 0x20, "Qt::Key_Space");
    }

    /// **A Cyrillic letter is its uppercase code point**, as Qt names a key
    /// that has no Latin name (#132).
    #[test]
    fn a_cyrillic_letter_maps_to_its_uppercase_codepoint() {
        assert_eq!(qt_key(Keysym::Cyrillic_shorti, "й"), 0x0419, "Й");
        assert_eq!(
            qt_key(Keysym::Cyrillic_SHORTI, "Й"),
            0x0419,
            "and shifted, the same key"
        );
    }

    /// **The named keys are Qt's**, the function keys counted from F1, and a
    /// key that is neither named nor types anything is `Qt::Key_unknown`.
    #[test]
    fn the_named_keys_map_to_qt_keys() {
        assert_eq!(
            [
                Keysym::Escape,
                Keysym::Return,
                Keysym::BackSpace,
                Keysym::Up,
                Keysym::F1,
                Keysym::F12,
                Keysym::Super_L,
                Keysym::XF86_AudioMute,
            ]
            .map(|sym| qt_key(sym, "")),
            [
                0x0100_0000,
                0x0100_0004,
                0x0100_0003,
                0x0100_0013,
                0x0100_0030,
                0x0100_003b,
                0x0100_0053,
                0x01ff_ffff,
            ]
        );
    }

    /// **A named key is named whatever it types**: Return types a carriage
    /// return and Escape an escape, and each is still its own `Qt::Key`.
    #[test]
    fn a_named_key_that_types_a_control_character_keeps_its_name() {
        assert_eq!(
            (
                qt_key(Keysym::Return, "\r"),
                qt_key(Keysym::Escape, "\u{1b}")
            ),
            (0x0100_0004, 0x0100_0000)
        );
    }

    /// **Every mouse button is itself**: the kernel's `BTN_LEFT..=0x11f` are
    /// Qt's buttons bit for bit, as QtWayland maps them. Ruling 9.
    #[test]
    fn the_mouse_buttons_map_onto_qt_bit_for_bit() {
        assert_eq!(
            [0x110, 0x111, 0x112, 0x113, 0x114, 0x11f].map(qt_button),
            [
                Some(0x1),
                Some(0x2),
                Some(0x4),
                Some(0x8),
                Some(0x10),
                Some(0x8000)
            ],
            "left, right, middle, back, forward, and Qt's last extra button"
        );
    }

    #[test]
    fn a_button_past_the_mouse_range_is_not_delivered() {
        assert_eq!((qt_button(0x10f), qt_button(0x120)), (None, None));
    }

    #[test]
    fn the_modifiers_map_onto_qt() {
        let held = ModifiersState {
            ctrl: true,
            shift: true,
            logo: true,
            ..Default::default()
        };
        assert_eq!(qt_modifiers(&held), QT_CONTROL | QT_SHIFT | QT_META);
        let alt = ModifiersState {
            alt: true,
            ..Default::default()
        };
        assert_eq!(qt_modifiers(&alt), QT_ALT);
        assert_eq!(qt_modifiers(&ModifiersState::default()), 0);
    }
}
