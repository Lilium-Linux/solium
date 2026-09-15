// Eight rounded rectangles of falling opacity, each wider than the last and
// offset downward. The falloff is the overlap, not a gradient.
//
// This is not a blur and does not pretend to be — see `Pane.qml`.

import QtQuick

Item {
    id: cast

    property bool focused: false
    property int bleedTop: 0
    property int bleedLeft: 0

    Repeater {
        model: 8

        Rectangle {
            required property int index

            // Innermost hugs the window; each one out is wider and fainter.
            // What darkens the middle is how many overlap there.
            readonly property real out: index * 3.5

            anchors.centerIn: parent
            width: parent.width - 60 + out * 2
            height: parent.height - 60 + out * 2
            y: 8
            radius: 10 + out
            color: "#000000"
            opacity: cast.focused ? 0.055 : 0.025
            Behavior on opacity { NumberAnimation { duration: 160 } }
        }
    }
}
