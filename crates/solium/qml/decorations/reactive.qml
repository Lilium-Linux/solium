// A border that lights up where the cursor is, with a titlebar above it.
//
// The pointer is delivered to the frame while it is anywhere over the window,
// so the glow can follow the cursor across the client area rather than only
// along the frame's own band. A `MouseArea` over the whole frame reads it;
// nothing here has to know where the client stops.

import QtQuick
import Solium

Item {
    id: frame

    property int insetTop: 30
    property int insetRight: 4
    property int insetBottom: 4
    property int insetLeft: 4

    property string title: ""
    property bool focused: false
    property bool pointerInside: false
    property int contentWidth: 0
    property int contentHeight: 0

    property string action: ""
    property string hovered: ""

    // Painted outside the insets -- the glow crosses the client area -- so the whole frame is copied when
    // it changes rather than just its bands.
    property bool overlay: true

    // Only for reading the position: clicks are the compositor's business, and
    // it decides on its own whether a press belongs to the frame.
    MouseArea {
        id: tracker
        anchors.fill: parent
        hoverEnabled: true
        acceptedButtons: Qt.NoButton
    }

    Rectangle {
        anchors.fill: parent
        color: "transparent"
        radius: 8
        border { width: 1; color: frame.focused ? Theme.edge : Theme.edgeInactive }
    }

    // The glow: a soft disc clipped to the frame, following the pointer. Drawn
    // under the bar so it lights the border without washing out the title.
    Item {
        anchors.fill: parent
        clip: true
        opacity: frame.pointerInside ? 1.0 : 0.0
        Behavior on opacity { NumberAnimation { duration: 200 } }

        // Concentric rings rather than a radial gradient: the scene is
        // rasterised in software, where QML's Gradient is linear-only and
        // ShaderEffect does not run at all. Twelve circles of falling opacity
        // is a soft edge for the price of twelve circles.
        Item {
            id: glow

            // On the cursor, not chasing it. The 90ms ease that used to be
            // here was meant to read as a light with weight; on real hardware
            // it reads as the compositor being slow, because a pointer is the
            // one thing on screen the eye already knows the position of.
            x: tracker.mouseX
            y: tracker.mouseY

            // Eight rings rather than twelve: each is a full alpha-blended
            // circle rasterised on the CPU, and the falloff is indisguishable.
            Repeater {
                model: 8

                Rectangle {
                    required property int index

                    readonly property real step: (index + 1) / 8

                    // Radii bunched towards the centre and a constant alpha
                    // per disc: what makes the middle bright is how many discs
                    // overlap there, not how opaque any one of them is.
                    width: 30 + Math.pow(step, 0.7) * 210
                    height: width
                    radius: width / 2
                    x: -width / 2
                    y: -height / 2
                    color: Theme.accent
                    opacity: 0.12
                }
            }
        }

        // The client's own area punched back out, so the glow reads as being
        // in the frame rather than smeared across the window.
        Rectangle {
            x: frame.insetLeft
            y: frame.insetTop
            width: frame.contentWidth
            height: frame.contentHeight
            color: "black"
            visible: false
        }
    }

    Rectangle {
        id: bar

        anchors { left: parent.left; right: parent.right; top: parent.top }
        height: frame.insetTop
        color: frame.focused ? Theme.surface : Theme.surfaceInactive
        opacity: 0.92

        Text {
            anchors.centerIn: parent
            width: Math.min(implicitWidth, Math.max(frame.width - 120, 0))
            text: frame.title
            elide: Text.ElideRight
            color: frame.focused ? Theme.text : Theme.textDim
            font { pixelSize: Theme.fontSize; family: Theme.fontFamily }
        }
    }

    Row {
        anchors { right: bar.right; rightMargin: Theme.margin; verticalCenter: bar.verticalCenter }
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
