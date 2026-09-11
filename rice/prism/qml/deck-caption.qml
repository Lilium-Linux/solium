// The deck's caption: what is selected, and where in the deck it is.
//
// It is told nothing. Properties on a scripted surface are set once when the
// scene is built, so a caption fed by the script would mean rebuilding this
// scene on every press — instead it reads the compositor's window list, and
// the deck moves the keyboard as it steps, so the active window *is* the
// selection. One source of truth, and no message passing.

import QtQuick
import Quickshell.Wayland
import Solium

Item {
    id: root

    readonly property var active: ToplevelManager.activeToplevel
    // Quickshell's object models present themselves as `{ values: [...] }`
    // and the compatibility layer matches that shape, so the list is one
    // level in. Binding a Repeater straight to `toplevels` gives it a map,
    // which is not empty and not iterable — it silently shows nothing.
    //
    // Sorted by id, and that is not tidiness. The compositor publishes its
    // panes in stacking order, so focusing a window moves it to the front of
    // the list — which in a dock means every tile jumping sideways each time
    // you change window, and the lit one always being the leftmost. Ids are
    // handed out in the order windows were opened and never change, so
    // sorting by them is the one ordering that holds still.
    readonly property var windows: {
        const model = ToplevelManager.toplevels
        const list = (model && model.values) || []
        const copy = []
        for (let i = 0; i < list.length; ++i) {
            copy.push(list[i])
        }
        copy.sort(function (a, b) { return (a.id || 0) - (b.id || 0) })
        return copy
    }

    Column {
        anchors.centerIn: parent
        spacing: 12

        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            width: Math.min(implicitWidth, root.width * 0.6)
            elide: Text.ElideMiddle
            horizontalAlignment: Text.AlignHCenter
            text: root.active ? (root.active.title || root.active.appId || "") : ""
            color: Theme.text
            font.family: Theme.fontFamily
            font.pixelSize: Theme.fontSize + 3
            font.letterSpacing: 0.6

            opacity: 0
            Component.onCompleted: opacity = 1
            Behavior on opacity { NumberAnimation { duration: 260 } }
        }

        Row {
            anchors.horizontalCenter: parent.horizontalCenter
            spacing: 6
            Repeater {
                model: root.windows
                delegate: Rectangle {
                    required property var modelData
                    readonly property bool here: modelData.activated === true
                    width: here ? 18 : 5
                    height: 5
                    radius: 2.5
                    anchors.verticalCenter: parent.verticalCenter
                    color: here ? Theme.violet : Theme.textDim
                    opacity: here ? 1.0 : 0.4
                    Behavior on width { NumberAnimation { duration: 220; easing.type: Easing.OutCubic } }
                    Behavior on color { ColorAnimation { duration: 220 } }
                }
            }
        }
    }
}
