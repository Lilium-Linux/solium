// Prism — the dock.
//
// A tile per window, from the compositor's own window list. Clicking one asks
// for it: the scene sets `action`, the compositor takes the string and clears
// it, and the script on the other end turns "focus:7" into `sol.focus(7)`.
// That one-way channel is the whole interface — QML never calls into the
// compositor, it only leaves a note.
//
// The surface this draws into is sized to the number of windows by the script,
// because an interactive surface swallows every press inside its rectangle.
// A full-width strip here would look identical and eat every click along the
// bottom of the screen.

import QtQuick
import Quickshell.Wayland
import Solium

Item {
    id: root

    property string action: ""

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

    Item {
        id: panel
        width: Math.min(parent.width, tiles.width + 28)
        height: parent.height - Theme.panelInset
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.top: parent.top

        Rectangle {
            id: plate
            anchors.fill: parent
            radius: Theme.panelRadius + 4
            gradient: Gradient {
                GradientStop { position: 0.0; color: Theme.glassFill }
                GradientStop { position: 1.0; color: Theme.glassFillLow }
            }
            border.width: 1
            border.color: Theme.glassRimLow
        }
        Rectangle {
            anchors.fill: plate
            radius: plate.radius
            color: Theme.glassTintIdle
        }
        Rectangle {
            anchors { top: plate.top; topMargin: 1; left: plate.left; right: plate.right }
            anchors.leftMargin: plate.radius
            anchors.rightMargin: plate.radius
            height: 1
            color: Theme.glassRim
        }

        Row {
            id: tiles
            anchors.centerIn: parent
            spacing: 8

            Repeater {
                model: root.windows
                delegate: Item {
                    id: tile
                    required property var modelData
                    required property int index
                    width: 46
                    height: 46

                    readonly property bool active: modelData.activated === true
                    // The icon is looked up by app id, because that is what an
                    // icon theme is keyed by. The letter comes from the title,
                    // because four terminals all have the app id "foot" and a
                    // dock of four identical Fs tells you nothing.
                    readonly property string iconName: (modelData.appId || "")
                    readonly property string label: (modelData.title || modelData.appId || "?")

                    Rectangle {
                        anchors.fill: parent
                        radius: 11
                        color: tile.active ? Theme.glassFillDeep
                                           : (press.containsMouse ? Theme.glassFill : "#00000000")
                        border.width: 1
                        border.color: tile.active ? Theme.glassRim : "#00000000"
                        Behavior on color { ColorAnimation { duration: Theme.quick } }
                        Behavior on border.color { ColorAnimation { duration: Theme.quick } }
                    }

                    // The icon theme, through the provider the compatibility
                    // layer installs. A container has almost no icon theme, so
                    // the initial underneath is not a fallback for failure —
                    // it is what is normally shown.
                    Image {
                        id: icon
                        anchors.centerIn: parent
                        width: 24
                        height: 24
                        sourceSize: Qt.size(24, 24)
                        source: "image://theme/" + tile.iconName
                        visible: status === Image.Ready
                        smooth: true
                    }

                    Text {
                        anchors.centerIn: parent
                        visible: !icon.visible
                        text: tile.label.charAt(0).toUpperCase()
                        color: tile.active ? Theme.text : Theme.textDim
                        font.family: Theme.fontFamily
                        font.pixelSize: 17
                        font.letterSpacing: 1
                        Behavior on color { ColorAnimation { duration: Theme.quick } }
                    }

                    // Lit while the window has the keyboard. The one piece of
                    // state in the dock worth reading at a glance.
                    Rectangle {
                        anchors.horizontalCenter: parent.horizontalCenter
                        anchors.bottom: parent.bottom
                        anchors.bottomMargin: 3
                        width: tile.active ? 14 : 4
                        height: 2.5
                        radius: 1.25
                        color: tile.active ? Theme.violet : Theme.textDim
                        opacity: tile.active ? 1.0 : 0.35
                        Behavior on width { NumberAnimation { duration: Theme.normal; easing.type: Easing.OutCubic } }
                        Behavior on color { ColorAnimation { duration: Theme.normal } }
                    }

                    MouseArea {
                        id: press
                        anchors.fill: parent
                        hoverEnabled: true
                        onClicked: root.action = "focus:" + tile.modelData.id
                    }

                    states: State {
                        when: press.containsMouse
                        PropertyChanges { tile.scale: 1.09 }
                    }
                    transitions: Transition {
                        NumberAnimation { property: "scale"; duration: Theme.quick; easing.type: Easing.OutCubic }
                    }
                }
            }
        }
    }
}
