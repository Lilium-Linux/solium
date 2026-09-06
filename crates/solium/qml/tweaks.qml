// Developer Tweaks.
//
// A list of whatever `lua/tweaks.lua` declares, and a press sends its id back
// to the script that declared it. Nothing here knows what a decoration or a
// genie is: the panel is a menu of strings, and the meaning lives in Lua.
//
// Deliberately plain, and deliberately not in the shell: this is a tool for
// trying effects out during development, and it is meant to be deletable.

import QtQuick
import Solium

Item {
    id: panel

    // Set once, when the panel is built: the entries the scripts declared.
    required property var entries

    // Read and cleared by the compositor, exactly like a window frame's.
    property string action: ""

    Rectangle {
        anchors.fill: parent
        color: Theme.surface
        opacity: 0.94

        Rectangle {
            anchors { left: parent.left; top: parent.top; bottom: parent.bottom }
            width: 1
            color: Theme.edge
        }
    }

    Column {
        anchors { fill: parent; margins: Theme.margin }
        spacing: Theme.gap

        Text {
            text: "Developer Tweaks"
            color: Theme.text
            font { pixelSize: Theme.fontSize + 2; family: Theme.fontFamily; bold: true }
        }

        Text {
            text: panel.entries.length + " from tweaks.lua"
            color: Theme.textDim
            font { pixelSize: Theme.fontSize - 2; family: Theme.fontFamily }
        }

        Item { width: 1; height: Theme.gap }

        Repeater {
            model: panel.entries

            Column {
                required property var modelData
                required property int index

                width: panel.width - Theme.margin * 2
                spacing: Theme.gap

                // A heading whenever the group changes, so the list reads as
                // sections without the script having to describe sections.
                Text {
                    visible: modelData.group !== ""
                             && (index === 0
                                 || panel.entries[index - 1].group !== modelData.group)
                    text: modelData.group
                    color: Theme.textDim
                    font { pixelSize: Theme.fontSize - 2; family: Theme.fontFamily }
                    topPadding: Theme.gap
                }

                Rectangle {
                    width: parent.width
                    height: 30
                    radius: 6
                    color: press.pressed
                           ? Theme.accent
                           : (press.containsMouse ? Theme.control : Theme.surfaceInactive)
                    border { width: 1; color: Theme.edge }

                    Behavior on color { ColorAnimation { duration: Theme.quick } }

                    Text {
                        anchors {
                            left: parent.left
                            leftMargin: Theme.margin
                            verticalCenter: parent.verticalCenter
                        }
                        text: modelData.label
                        color: Theme.text
                        elide: Text.ElideRight
                        width: parent.width - Theme.margin * 2
                        font { pixelSize: Theme.fontSize; family: Theme.fontFamily }
                    }

                    MouseArea {
                        id: press
                        anchors.fill: parent
                        hoverEnabled: true
                        onClicked: panel.action = parent.parent.modelData.id
                    }
                }
            }
        }
    }
}
