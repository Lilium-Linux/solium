// The deck's backdrop.
//
// On the `bottom` layer, which is under the windows and over the wallpaper —
// so it darkens the field the cards are turning in without touching the cards
// themselves. That is the whole reason it is a separate surface rather than
// something drawn into the wallpaper: the wallpaper cannot dim only when a
// mode is up, and a surface can simply not exist the rest of the time.

import QtQuick
import Solium

Item {
    id: scrim

    Rectangle {
        anchors.fill: parent
        color: "#000000"
        opacity: 0
        Component.onCompleted: opacity = 0.62
        Behavior on opacity { NumberAnimation { duration: 280; easing.type: Easing.OutCubic } }
    }

    // A pool of light where the selected card stands, so the middle of the
    // deck is lifted out of the dark rather than merely less dark. Glow rather
    // than a gradient for the same reason the wallpaper uses one: a Rectangle
    // gradient fades along one axis, and this needs to fade in every direction.
    Glow {
        anchors.centerIn: parent
        width: parent.width * 0.85
        height: parent.width * 0.85
        tint: Theme.violet
        strength: 0.22
        opacity: 0
        Component.onCompleted: opacity = 1
        Behavior on opacity { NumberAnimation { duration: 420; easing.type: Easing.OutCubic } }
    }
}
