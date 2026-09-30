// Which edges a panel asks to be anchored to, as `anchors { top: true }` says
// it. See PanelWindow.qml.
import QtQuick
QtObject {
    property bool top: false
    property bool bottom: false
    property bool left: false
    property bool right: false
}
