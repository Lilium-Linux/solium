//! A hosted scene's pointer, drawn as the shape it asks for (#373).
//!
//! A scene's `MouseArea` or `HoverHandler` names a `cursorShape`, and Qt Quick
//! sets that shape on the scene's window when the pointer is over the item
//! (`QQuickWindowPrivate::updateCursor`, run as each pointer event is
//! delivered). Nothing read it back, and so a shell's resize handles, a
//! screenshot tool's crosshair and a link's pointing hand all showed the
//! arrow. `qml/host.cpp` hands the window's shape over as a
//! `Qt::CursorShape` (`qml::Scene::cursor`), [`icon`] turns it into the shape
//! a theme draws, and the compositor asserts it, as it asserts its own over
//! a frame, while the pointer is over a scene that takes the press there
//! (`state::hit_test::Claim::cursor_over`).
//!
//! **The arrow asks for nothing.** Qt answers `ArrowCursor` both for an item
//! that names it and for no item naming anything, so it leaves the pointer to
//! what is under the scene's claim, which over a scene is the default arrow
//! anyway. A blank, a bitmap and a custom cursor have no name a theme draws,
//! and ask for nothing either: a scene that wants a picture of its own draws
//! it.

use smithay::input::pointer::CursorIcon;

/// The shape a Qt `Qt::CursorShape` stands for, by its value in
/// `qnamespace.h`, or `None` for the arrow and for what has no name.
/// `SizeAllCursor` is `all-scroll`, whose X11 spelling is Qt's own
/// `size_all` ([`super::shape::names`]); `DragMoveCursor` is `move`.
/// `tests::each_qt_shape_is_the_shape_it_draws`.
pub(crate) fn icon(shape: i32) -> Option<CursorIcon> {
    Some(match shape {
        2 => CursorIcon::Crosshair,
        3 => CursorIcon::Wait,
        4 => CursorIcon::Text,
        5 => CursorIcon::NsResize,
        6 => CursorIcon::EwResize,
        7 => CursorIcon::NeswResize,
        8 => CursorIcon::NwseResize,
        9 => CursorIcon::AllScroll,
        11 => CursorIcon::RowResize,
        12 => CursorIcon::ColResize,
        13 => CursorIcon::Pointer,
        14 => CursorIcon::NotAllowed,
        15 => CursorIcon::Help,
        16 => CursorIcon::Progress,
        17 => CursorIcon::Grab,
        18 => CursorIcon::Grabbing,
        19 => CursorIcon::Copy,
        20 => CursorIcon::Move,
        21 => CursorIcon::Alias,
        // ArrowCursor (0), UpArrowCursor (1), which has no shape of its own
        // in either vocabulary, BlankCursor (10), BitmapCursor (24) and
        // CustomCursor (25).
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Each Qt shape is the shape it draws**: the four the main PC's
    /// screenshot tool asks for among them, by their values in
    /// `qnamespace.h`, and nothing for the arrow, a blank or a value Qt does
    /// not have.
    #[test]
    fn each_qt_shape_is_the_shape_it_draws() {
        // Qt::CrossCursor, Qt::SizeFDiagCursor, Qt::PointingHandCursor,
        // Qt::SizeAllCursor.
        assert_eq!(icon(2), Some(CursorIcon::Crosshair));
        assert_eq!(icon(8), Some(CursorIcon::NwseResize));
        assert_eq!(icon(13), Some(CursorIcon::Pointer));
        assert_eq!(icon(9), Some(CursorIcon::AllScroll));
        assert_eq!(icon(7), Some(CursorIcon::NeswResize));
        assert_eq!(icon(4), Some(CursorIcon::Text));
        assert_eq!(icon(18), Some(CursorIcon::Grabbing));
        for nothing in [0, 1, 10, 24, 25, -1, 99] {
            assert_eq!(icon(nothing), None, "Qt::CursorShape {nothing}");
        }
    }
}
