// A border that answers the pointer entering and leaving the window.
//
// `pointerInside` is true while the pointer is anywhere over the window, the
// client included — the compositor tracks that, because the client's own
// surface tells us nothing. So the border can wake as the cursor arrives
// rather than only when it reaches the frame itself.

import QtQuick
import Solium

Item {
    id: frame

    property int insetTop: 4
    property int insetRight: 4
    property int insetBottom: 4
    property int insetLeft: 4

    property string title: ""
    property bool focused: false
    property bool pointerInside: false
    property int contentWidth: 0
    property int contentHeight: 0

    property string action: ""
    property string hovered: ""

    Rectangle {
        anchors.fill: parent
        color: "transparent"
        radius: 0
        opacity: frame.pointerInside ? 1.0 : 0.45
        border {
            width: frame.pointerInside ? frame.insetTop : 1
            color: frame.pointerInside
                   ? Theme.accent
                   : (frame.focused ? Theme.edge : Theme.edgeInactive)
        }

        // All three at once, and slowly enough to read as the window noticing
        // you rather than as a flicker.
        Behavior on border.width { NumberAnimation { duration: 160; easing.type: Easing.OutCubic } }
        Behavior on border.color { ColorAnimation { duration: 160 } }
        Behavior on opacity { NumberAnimation { duration: 160 } }
    }
}
