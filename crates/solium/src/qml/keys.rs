//! The compositor's input in Qt's terms: mouse buttons and keyboard
//! modifiers (`tests::the_mouse_buttons_map_onto_qt_bit_for_bit`,
//! `tests::the_modifiers_map_onto_qt`), and, once a scene can take the
//! keyboard (#163), keys.

use smithay::input::keyboard::ModifiersState;

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

#[cfg(test)]
mod tests {
    use smithay::input::keyboard::ModifiersState;

    use super::{QT_ALT, QT_CONTROL, QT_META, QT_SHIFT, qt_button, qt_modifiers};

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
