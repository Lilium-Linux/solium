// The thin bar the wave stands on, so the crests have an edge to rise from.
import QtQuick
import Solium

Item {
    id: sill
    property bool focused: false

    Rectangle {
        anchors { left: parent.left; right: parent.right; top: parent.top }
        height: 10
        color: sill.focused ? Theme.surface : Theme.surfaceInactive
    }
}
