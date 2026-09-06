// No bar at all: a border, and the window's own edges.
//
// The smallest useful decoration, and the one worth reading first — every
// other file here is this plus something.

import QtQuick
import Solium

Item {
    id: frame

    property int insetTop: 3
    property int insetRight: 3
    property int insetBottom: 3
    property int insetLeft: 3

    property string title: ""
    property bool focused: false
    property bool pointerInside: false
    property int contentWidth: 0
    property int contentHeight: 0

    property string action: ""
    property string hovered: ""

    // Painted as an outline rather than a filled rectangle, so the middle
    // stays transparent and the client shows through it.
    Rectangle {
        anchors.fill: parent
        color: "transparent"
        radius: 6
        border {
            width: frame.insetTop
            color: frame.focused ? Theme.accent : Theme.edgeInactive
        }

        Behavior on border.color { ColorAnimation { duration: Theme.normal } }
    }
}
