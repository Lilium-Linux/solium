import QtQuick

Rectangle {
    id: button
    property string text: ""
    property color accent: "#a38aff"
    property bool checked: false
    property bool dark: true
    signal triggered()
    implicitWidth: 220; implicitHeight: 38
    radius: 10
    color: mouse.pressed ? (dark ? "#494154" : "#ded5f4") : mouse.containsMouse ? (dark ? "#36303e" : "#eee8fa") : checked ? (dark ? "#382e4e" : "#e8dffc") : "transparent"
    opacity: enabled ? 1 : 0.4
    Accessible.role: Accessible.Button
    Accessible.name: text
    Text { anchors { left: parent.left; right: parent.right; verticalCenter: parent.verticalCenter; margins: 12 }
        text: button.text; textFormat: Text.PlainText; color: button.checked ? button.accent : button.dark ? "#efedf5" : "#28232f"
        font { family: "Noto Sans"; pixelSize: 13 }
 elide: Text.ElideRight }
    MouseArea { id: mouse; anchors.fill: parent; hoverEnabled: true; onClicked: button.triggered() }
}
