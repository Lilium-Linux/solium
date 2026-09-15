// The one layer of `panes/proximity`: a border, thin until you approach.

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
