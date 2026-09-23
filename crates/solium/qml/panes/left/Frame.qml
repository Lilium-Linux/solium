// The one layer of `panes/left`: the bar, and the title turned on its side.

import QtQuick
import Solium

Item {
    id: frame

    // What this layer paints, set by the compositor from the `insets` that
    // `Pane.qml` declares. A style reserves space once for the whole pane, so
    // the number lives in the manifest and every layer is told it — the space
    // reserved and the space painted cannot be two different numbers, which is
    // what putting `insets` on `PaneStyle` rather than on `Layer` was for.
    //
    // Zero is what this reads if the file is built outside a pane — by
    // `--check-qml`, say — and drawing nothing is the honest answer there.
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

    // What a short tile leaves room for, by `top/Frame.qml`'s arithmetic
    // turned on its side (#133): this bar runs down the window, so its buttons
    // stack along the height, and the height is what runs out. The title keeps
    // this file's own 120px and the same 24px of its own.
    readonly property bool roomForTitle: frame.height - 120 >= 24
    readonly property bool roomForClose: frame.height >= 37
    readonly property bool roomForMaximize: frame.height >= 59

    // Rotated about its own centre, then placed: rotation happens after
    // layout, so a Text that is as wide as the window is tall ends up as tall
    // as the window once turned.
    Text {
        visible: frame.roomForTitle
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

        FrameButton { name: "maximize"; tint: Theme.warning; visible: frame.roomForMaximize }
        FrameButton { name: "close"; tint: Theme.danger; visible: frame.roomForClose }
    }
}
