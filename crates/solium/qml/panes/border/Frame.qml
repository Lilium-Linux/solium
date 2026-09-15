// The one layer of `panes/border`: an outline, and nothing else.

import QtQuick
import Solium

Item {
    id: frame

    // What this layer paints, set by the compositor from the `insets` that
    // `Pane.qml` declares. A style reserves space once for the whole pane, so
    // the number lives in the manifest and every layer is told it — the space
    // reserved and the space painted cannot be two different numbers, which is
    // what putting `insets` on `PaneStyle` rather than on `Layer` was for.
    //
    // Zero is what this reads if the file is built outside a pane — by
    // `--check-qml`, say — and drawing nothing is the honest answer there.
    property int insetTop: 0

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
        radius: 0
        border {
            width: frame.insetTop
            color: frame.focused ? Theme.accent : Theme.edgeInactive
        }

        Behavior on border.color { ColorAnimation { duration: Theme.normal } }
    }
}
