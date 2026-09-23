// The one layer of `panes/bottom`: the bar, under the client.

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
    property int insetBottom: 0

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

    // What a narrow tile leaves room for, by `top/Frame.qml`'s arithmetic and
    // for its reasons (#133): the title below 174px, maximise below 59px,
    // close below 37px, and a bare bar under that.
    readonly property bool roomForTitle: frame.width - 150 >= 24
    readonly property bool roomForClose: frame.width >= 37
    readonly property bool roomForMaximize: frame.width >= 59

    Text {
        anchors.centerIn: bar
        visible: frame.roomForTitle
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

        FrameButton { name: "maximize"; tint: Theme.warning; visible: frame.roomForMaximize }
        FrameButton { name: "close"; tint: Theme.danger; visible: frame.roomForClose }
    }
}
