// The effects overlay: rows of text `lua/problems.lua` hands over. Nothing
// here knows what an effect is, and nothing here takes input
// (qml::hosted::tests::the_problems_overlay_lists_each_row_and_takes_no_input).

import QtQuick
import Solium

Item {
    id: overlay

    required property var rows

    Rectangle {
        anchors.fill: parent
        color: Theme.surface
        opacity: 0.94
        border.color: Theme.danger
    }

    Column {
        anchors { fill: parent; margins: Theme.margin }
        spacing: 4

        Repeater {
            model: overlay.rows

            delegate: Text {
                required property var modelData
                width: parent.width
                text: modelData.text
                color: modelData.severity === "warning" ? Theme.warning : Theme.danger
                elide: Text.ElideMiddle
                font.family: Theme.fontFamily
                font.pixelSize: Theme.fontSize
            }
        }
    }
}
