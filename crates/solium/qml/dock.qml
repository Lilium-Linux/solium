// The dock, drawn by the compositor itself.
//
// Not a client. That is the whole point of it being here: an icon in this
// scene and a window on the screen are the same compositor's to place, so a
// window can grow out of an icon and shrink back into it. A dock in a separate
// process can only ever be a rectangle that a window happens to fly past —
// the two live in different scenes and nothing can interpolate between them.
//
// It imports the same `Solium.Theme` the titlebars do, so an object lifted
// from here into a window frame keeps its colours because it never left the
// design system.

import QtQuick
import Solium

Item {
    id: root

    // Set from Rust: the labels, tab-separated, and which one is hovered.
    property string items: ""
    property int hovered: -1

    readonly property var entries: items.length > 0 ? items.split("\t") : []

    // The surface is the whole screen now, so the plate places itself on it.
    Item {
        id: plate
        visible: root.entries.length > 0
        width: row.implicitWidth + 24
        height: 72
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom: parent.bottom
        anchors.bottomMargin: 12

    Rectangle {
        anchors.fill: parent
        radius: Theme.radius * 2
        color: Theme.surface
        border.color: Theme.edge
        border.width: 1
        opacity: 0.96
    }

    Row {
        id: row
        anchors.centerIn: parent
        spacing: Theme.gap

        Repeater {
            model: root.entries

            Rectangle {
                width: 48
                height: 48
                radius: Theme.radius
                color: index === root.hovered ? Theme.control : Theme.controlInactive
                border.color: index === root.hovered ? Theme.accent : Theme.edge
                border.width: 1

                // A letter stands in for an icon until there is an icon
                // theme to read. The shape and the geometry are what the
                // morph needs; the picture inside it can come later.
                Text {
                    anchors.centerIn: parent
                    text: modelData.length > 0 ? modelData.charAt(0).toUpperCase() : "?"
                    color: Theme.text
                    font.family: Theme.fontFamily
                    font.pixelSize: 22
                }

                Behavior on color {
                    ColorAnimation { duration: Theme.quick }
                }
            }
        }
    }
    }
}
