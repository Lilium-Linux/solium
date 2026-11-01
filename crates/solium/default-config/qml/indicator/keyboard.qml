// Shadows the shipped indicator/keyboard.qml: the same overlay pill, violet.

import QtQuick
import Solium
import "../accent"

Item {
    id: root

    property var cue: ({})

    Solium.input: false

    Pill {
        anchors.centerIn: parent
        cue: root.cue
    }
}
