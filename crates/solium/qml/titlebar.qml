// A window's titlebar.
//
// Drawn by the compositor, not by the client: the frame reserves its own height
// so frame and window are one object, and the client is never covered.
//
// Every colour and measurement comes from `Solium.Theme`, which the shell's own
// surfaces import too — one engine, one singleton, so changing a colour there
// changes the titlebars and the dock together rather than in two places that
// drift.
//
// Properties in (`title`, `focused`) are set from Rust each frame. `action` is
// the one property that flows the other way — a button sets it, the compositor
// takes it and clears it. One direction, one owner.

import QtQuick
import Solium

Item {
    id: frame

    property string title: ""
    property bool focused: false
    property string action: ""

    // Which button the pointer is over, or empty. Read by the compositor to
    // decide whether a press starts a window drag — QML owns the button
    // layout, so QML is what knows. Duplicating the geometry in Rust would be
    // a mirror of state with two authorities.
    property string hovered: ""
    readonly property bool onButton: hovered !== ""

    // Shared look for the buttons, so a third one is three lines rather than a
    // copy of forty.
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
        Behavior on scale {
            NumberAnimation { duration: 100; easing.type: Easing.OutCubic }
        }

        MouseArea {
            id: pointer
            anchors.fill: parent
            hoverEnabled: true
            onClicked: frame.action = button.name

            // Cleared only by the button that set it, so moving from one
            // button straight onto another does not leave `hovered` empty.
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
        anchors.fill: parent
        color: frame.focused ? Theme.surface : Theme.surfaceInactive
        Behavior on color { ColorAnimation { duration: Theme.normal } }

        // A hairline where the frame meets the client, so the seam reads as
        // deliberate rather than as a gap.
        Rectangle {
            anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
            height: 1
            color: frame.focused ? Theme.edge : Theme.edgeInactive
        }
    }

    Text {
        anchors.centerIn: parent
        width: Math.min(implicitWidth, Math.max(frame.width - 150, 0))
        text: frame.title
        elide: Text.ElideRight
        horizontalAlignment: Text.AlignHCenter
        color: frame.focused ? Theme.text : Theme.textDim
        font { pixelSize: Theme.fontSize; family: Theme.fontFamily }

        Behavior on color { ColorAnimation { duration: Theme.normal } }
    }

    Row {
        anchors {
            right: parent.right
            rightMargin: Theme.margin
            verticalCenter: parent.verticalCenter
        }
        spacing: Theme.gap

        FrameButton { name: "maximize"; tint: Theme.warning }
        FrameButton { name: "close"; tint: Theme.danger }
    }
}
