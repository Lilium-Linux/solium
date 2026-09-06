// A titlebar underneath the window.
//
// Reserves at the bottom instead of the top, so the client sits above it. The
// buttons stay on the right, where the hand expects them.

import QtQuick
import Solium

Item {
    id: frame

    property int insetTop: 0
    property int insetRight: 0
    property int insetBottom: 30
    property int insetLeft: 0

    property string title: ""
    property bool focused: false
    property bool pointerInside: false
    property int contentWidth: 0
    property int contentHeight: 0

    property string action: ""
    property string hovered: ""

    component FrameButton: Rectangle {
        id: button

        property color tint: Theme.control
        property string name: ""

        width: 13
        height: 13
        radius: width / 2
        color: pointer.containsMouse
               ? tint
               : (frame.focused ? Theme.control : Theme.controlInactive)
        scale: pointer.pressed ? 0.86 : 1.0

        Behavior on color { ColorAnimation { duration: Theme.quick } }
        Behavior on scale { NumberAnimation { duration: 100; easing.type: Easing.OutCubic } }

        MouseArea {
            id: pointer
            anchors.fill: parent
            hoverEnabled: true
            onClicked: frame.action = button.name
            onContainsMouseChanged: {
                if (containsMouse) {
                    frame.hovered = button.name;
                } else if (frame.hovered === button.name) {
                    frame.hovered = "";
                }
            }
        }
    }

    Rectangle {
        id: bar

        anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
        height: frame.insetBottom
        color: frame.focused ? Theme.surface : Theme.surfaceInactive
        Behavior on color { ColorAnimation { duration: Theme.normal } }

        Rectangle {
            anchors { left: parent.left; right: parent.right; top: parent.top }
            height: 1
            color: frame.focused ? Theme.edge : Theme.edgeInactive
        }
    }

    Text {
        anchors.centerIn: bar
        width: Math.min(implicitWidth, Math.max(frame.width - 150, 0))
        text: frame.title
        elide: Text.ElideRight
        horizontalAlignment: Text.AlignHCenter
        color: frame.focused ? Theme.text : Theme.textDim
        font { pixelSize: Theme.fontSize; family: Theme.fontFamily }
    }

    Row {
        anchors { right: bar.right; rightMargin: Theme.margin; verticalCenter: bar.verticalCenter }
        spacing: Theme.gap

        FrameButton { name: "maximize"; tint: Theme.warning }
        FrameButton { name: "close"; tint: Theme.danger }
    }
}
