// A titlebar down the left-hand side.
//
// The same frame as `top.qml` turned ninety degrees, which is the point: where
// a bar lives is two numbers and an anchor, not a different mechanism.

import QtQuick
import Solium

Item {
    id: frame

    property int insetTop: 0
    property int insetRight: 0
    property int insetBottom: 0
    property int insetLeft: 34

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

        anchors { top: parent.top; bottom: parent.bottom; left: parent.left }
        width: frame.insetLeft
        color: frame.focused ? Theme.surface : Theme.surfaceInactive
        Behavior on color { ColorAnimation { duration: Theme.normal } }

        Rectangle {
            anchors { top: parent.top; bottom: parent.bottom; right: parent.right }
            width: 1
            color: frame.focused ? Theme.edge : Theme.edgeInactive
        }
    }

    // Rotated about its own centre, then placed: rotation happens after
    // layout, so a Text that is as wide as the window is tall ends up as tall
    // as the window once turned.
    Text {
        width: Math.min(implicitWidth, Math.max(frame.height - 120, 0))
        text: frame.title
        elide: Text.ElideRight
        horizontalAlignment: Text.AlignHCenter
        color: frame.focused ? Theme.text : Theme.textDim
        font { pixelSize: Theme.fontSize; family: Theme.fontFamily }

        rotation: -90
        transformOrigin: Item.Center
        x: bar.x + (bar.width - width) / 2
        y: (frame.height - height) / 2

        Behavior on color { ColorAnimation { duration: Theme.normal } }
    }

    Column {
        anchors {
            bottom: bar.bottom
            bottomMargin: Theme.margin
            horizontalCenter: bar.horizontalCenter
        }
        spacing: Theme.gap

        FrameButton { name: "maximize"; tint: Theme.warning }
        FrameButton { name: "close"; tint: Theme.danger }
    }
}
