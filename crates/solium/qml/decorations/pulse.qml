// A titlebar with an animation running inside it.
//
// Two of them, in fact: a sheen that travels across the focused window's bar,
// and an accent line under it that breathes. Both are ordinary QML animations
// — the compositor drives the scene's clock every frame while it is changing,
// and stops driving it as soon as the scene says it has settled, so an
// unfocused window costs nothing to keep on screen.

import QtQuick
import Solium

Item {
    id: frame

    property int insetTop: 38
    property int insetRight: 0
    property int insetBottom: 0
    property int insetLeft: 0

    property string title: ""
    property bool focused: false
    property bool pointerInside: false
    property int contentWidth: 0
    property int contentHeight: 0

    property string action: ""
    property string hovered: ""

    Rectangle {
        id: bar

        anchors { left: parent.left; right: parent.right; top: parent.top }
        height: frame.insetTop - 2
        color: frame.focused ? Theme.surface : Theme.surfaceInactive
        clip: true

        // The sheen: a pale band that crosses the bar and waits before coming
        // round again. Only while focused, or every window on screen would be
        // asking for frames forever.
        Rectangle {
            width: 120
            height: parent.height
            visible: frame.focused
            gradient: Gradient {
                orientation: Gradient.Horizontal
                GradientStop { position: 0.0; color: "transparent" }
                GradientStop { position: 0.5; color: Theme.accent }
                GradientStop { position: 1.0; color: "transparent" }
            }
            opacity: 0.18

            SequentialAnimation on x {
                running: frame.focused
                loops: Animation.Infinite
                NumberAnimation {
                    from: -140
                    to: bar.width + 20
                    duration: 2600
                    easing.type: Easing.InOutSine
                }
                PauseAnimation { duration: 1400 }
            }
        }

        Text {
            anchors.centerIn: parent
            width: Math.min(implicitWidth, Math.max(frame.width - 150, 0))
            text: frame.title
            elide: Text.ElideRight
            color: frame.focused ? Theme.text : Theme.textDim
            font { pixelSize: Theme.fontSize; family: Theme.fontFamily }
        }

        Row {
            anchors { right: parent.right; rightMargin: Theme.margin; verticalCenter: parent.verticalCenter }
            spacing: Theme.gap

            Repeater {
                model: [{ name: "maximize", tint: Theme.warning },
                        { name: "close", tint: Theme.danger }]

                Rectangle {
                    required property var modelData

                    width: 13
                    height: 13
                    radius: width / 2
                    color: hit.containsMouse
                           ? modelData.tint
                           : (frame.focused ? Theme.control : Theme.controlInactive)
                    Behavior on color { ColorAnimation { duration: Theme.quick } }

                    MouseArea {
                        id: hit
                        anchors.fill: parent
                        hoverEnabled: true
                        onClicked: frame.action = parent.modelData.name
                        onContainsMouseChanged: {
                            if (containsMouse) {
                                frame.hovered = parent.modelData.name;
                            } else if (frame.hovered === parent.modelData.name) {
                                frame.hovered = "";
                            }
                        }
                    }
                }
            }
        }
    }

    // The breathing line where the frame meets the client. Inside the
    // reserved height rather than below it: a frame that stays within its own
    // insets only has those copied when it changes, and this one changes on
    // every frame.
    Rectangle {
        anchors { left: parent.left; right: parent.right; top: bar.bottom }
        height: frame.insetTop - bar.height
        color: Theme.accent
        opacity: frame.focused ? 0.9 : 0.2

        SequentialAnimation on opacity {
            running: frame.focused
            loops: Animation.Infinite
            NumberAnimation { to: 0.25; duration: 1300; easing.type: Easing.InOutSine }
            NumberAnimation { to: 0.9; duration: 1300; easing.type: Easing.InOutSine }
        }
    }
}
