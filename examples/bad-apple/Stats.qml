// The panel `bad-apple` puts on the desktop: rows of a name and a number,
// which the window manager hands over as it measures them. A scene knows
// nothing of where its numbers come from.

import QtQuick
import Solium

Item {
    id: panel

    required property var rows

    Rectangle {
        anchors.fill: parent
        color: "#101014"
        opacity: 0.88
        radius: 8
    }

    Column {
        anchors { fill: parent; margins: 12 }
        spacing: 6

        Repeater {
            model: panel.rows

            delegate: Item {
                required property var modelData
                width: parent.width
                height: 20

                Text {
                    anchors.left: parent.left
                    text: modelData.name
                    color: "#9a9aae"
                    font.family: Theme.fontFamily
                    font.pixelSize: 15
                }
                Text {
                    anchors.right: parent.right
                    text: modelData.value
                    color: "white"
                    font.family: Theme.fontFamily
                    font.pixelSize: 15
                    font.bold: true
                }
            }
        }
    }
}
