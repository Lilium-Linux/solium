// Prism — the window frame.
//
// A band across the top and nothing else. That is a cost decision as much as a
// visual one: a frame that stays inside the bands it reserves has only those
// bands copied and uploaded when it changes, and a 32px bar on a 900px window
// is about four per cent of it. Painting a border round the client as well
// would mean declaring `overlay` and paying for the other ninety-six per cent
// on every focus change.
//
// There is no blur behind this. A decoration is composited over the scene and
// cannot sample what is underneath, so the frost is built rather than
// sampled — a translucent fill, a lit top edge, a dark bottom edge, and a tint
// that answers focus.

import QtQuick
import Solium

Item {
    id: frame

    // What the compositor sets, every frame.
    property string title: ""
    property bool focused: false
    property bool pointerInside: false
    property int contentWidth: 0
    property int contentHeight: 0

    // What the compositor reads back.
    property string action: ""
    property string hovered: ""

    // What it reserves. Read once, when the frame is built; the client is
    // placed in what is left.
    property int insetTop: Theme.titlebarHeight
    property int insetRight: 0
    property int insetBottom: 0
    property int insetLeft: 0

    Item {
        id: bar
        height: frame.insetTop
        anchors { top: parent.top; left: parent.left; right: parent.right }

        // Rounded at the top, square at the bottom, which is two rectangles
        // rather than a clip: a clip would cost a layer, and a layer over the
        // whole bar is the thing this frame exists not to do.
        Rectangle {
            id: plate
            anchors.fill: parent
            radius: Theme.panelRadius
            gradient: Gradient {
                GradientStop { position: 0.0; color: Theme.glassFillDeep }
                GradientStop { position: 1.0; color: Theme.glassFillLow }
            }
        }
        Rectangle {
            anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
            height: Theme.panelRadius
            color: Theme.glassFillLow
        }

        // The tint. This is what makes a focused window read as focused before
        // you have looked at anything in the bar.
        Rectangle {
            anchors.fill: parent
            color: frame.focused ? Theme.glassTint : Theme.glassTintIdle
            Behavior on color { ColorAnimation { duration: Theme.normal } }
        }

        // The sheen: light arriving from the upper left, falling off across the
        // bar. Diagonal rather than vertical, because a vertical highlight on a
        // horizontal bar reads as a bevel and a diagonal one reads as glass.
        Rectangle {
            anchors.fill: parent
            opacity: frame.focused ? 0.9 : 0.4
            Behavior on opacity { NumberAnimation { duration: Theme.normal } }
            gradient: Gradient {
                orientation: Gradient.Horizontal
                GradientStop { position: 0.0; color: "#1effffff" }
                GradientStop { position: 0.35; color: "#08ffffff" }
                GradientStop { position: 1.0; color: "#00ffffff" }
            }
        }

        // The lit edge along the top, and the shadow along the bottom. Two one
        // pixel rectangles doing most of the work of the whole effect.
        Rectangle {
            anchors { top: parent.top; left: parent.left; right: parent.right }
            height: 1
            color: frame.focused ? Theme.glassRim : Theme.glassRimLow
            Behavior on color { ColorAnimation { duration: Theme.normal } }
        }
        Rectangle {
            anchors { bottom: parent.bottom; left: parent.left; right: parent.right }
            height: 1
            color: Theme.glassShade
        }

        // The prism mark. Three bars, one per light source in the wallpaper,
        // lit only while the window has the keyboard.
        Row {
            id: mark
            anchors { left: parent.left; leftMargin: Theme.margin; verticalCenter: parent.verticalCenter }
            spacing: 3
            Repeater {
                model: [Theme.rose, Theme.violet, Theme.cyan]
                delegate: Rectangle {
                    required property int index
                    required property var modelData
                    width: 2
                    height: 11 - Math.abs(index - 1) * 3
                    radius: 1
                    anchors.verticalCenter: parent.verticalCenter
                    color: modelData
                    opacity: frame.focused ? 0.95 : 0.28
                    Behavior on opacity { NumberAnimation { duration: Theme.normal } }
                }
            }
        }

        Text {
            anchors.centerIn: parent
            width: Math.min(implicitWidth, bar.width - 200)
            elide: Text.ElideMiddle
            horizontalAlignment: Text.AlignHCenter
            text: frame.title
            color: frame.focused ? Theme.text : Theme.textDim
            font.family: Theme.fontFamily
            font.pixelSize: Theme.fontSize
            font.letterSpacing: 0.4
            Behavior on color { ColorAnimation { duration: Theme.normal } }
        }

        Row {
            anchors { right: parent.right; rightMargin: Theme.margin; verticalCenter: parent.verticalCenter }
            spacing: 7

            // `hovered` is what decides whether a press drags the window, so a
            // button that does not set it is a button that moves the window
            // instead of doing its job.
            Repeater {
                model: [
                    { name: "maximize", tint: Theme.cyan },
                    { name: "close", tint: Theme.rose }
                ]
                delegate: Rectangle {
                    required property var modelData
                    width: 13
                    height: 13
                    radius: 6.5
                    anchors.verticalCenter: parent.verticalCenter
                    color: hover.containsMouse ? modelData.tint : Theme.glassFill
                    border.width: 1
                    border.color: hover.containsMouse ? modelData.tint : Theme.glassRimLow
                    opacity: frame.focused || hover.containsMouse ? 1.0 : 0.45
                    Behavior on color { ColorAnimation { duration: Theme.quick } }
                    Behavior on opacity { NumberAnimation { duration: Theme.quick } }

                    MouseArea {
                        id: hover
                        anchors.fill: parent
                        anchors.margins: -3
                        hoverEnabled: true
                        onContainsMouseChanged: frame.hovered = containsMouse ? modelData.name : ""
                        onClicked: frame.action = modelData.name
                    }
                }
            }
        }
    }
}
