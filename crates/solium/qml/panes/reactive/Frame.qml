// The one layer of `panes/reactive`: the lit border and the bar above it.
//
// A `MouseArea` over the whole layer reads the pointer; nothing here has to
// know where the client stops.

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
    property int insetTop: 0
    property int insetLeft: 0

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
        radius: 0
        border { width: 1; color: frame.focused ? Theme.edge : Theme.edgeInactive }
    }

    // The glow: a soft disc clipped to the frame, following the pointer. Drawn
    // under the bar so it lights the border without washing out the title.
    Item {
        anchors.fill: parent
        clip: true
        opacity: frame.pointerInside ? 1.0 : 0.0
        Behavior on opacity { NumberAnimation { duration: 200 } }

        // A flat square, not a soft one. Software rasterisation has no radial
        // gradient and no ShaderEffect, so a glow here meant stacking alpha
        // discs — which is a lot of blending to say "the pointer is there".
        Item {
            id: glow

            // On the cursor, not chasing it. The 90ms ease that used to be
            // here was meant to read as a light with weight; on real hardware
            // it reads as the compositor being slow, because a pointer is the
            // one thing on screen the eye already knows the position of.
            x: tracker.mouseX
            y: tracker.mouseY

            Rectangle {
                width: 90
                height: 90
                x: -width / 2
                y: -height / 2
                color: Theme.accent
                opacity: 0.15
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

    // What a narrow tile leaves room for, by `top/Frame.qml`'s arithmetic and
    // for its reasons (#133): the title below 144px -- this file holds back
    // 120 for the buttons rather than 150 -- maximise below 59px, close below
    // 37px, and a bare bar under that. The centred title ends at `width - 60`
    // and the buttons start at `width - 47`, so the two never meet.
    readonly property bool roomForTitle: frame.width - 120 >= 24
    readonly property bool roomForClose: frame.width >= 37
    readonly property bool roomForMaximize: frame.width >= 59

    Rectangle {
        id: bar

        anchors { left: parent.left; right: parent.right; top: parent.top }
        height: frame.insetTop
        color: frame.focused ? Theme.surface : Theme.surfaceInactive
        opacity: 0.92

        Text {
            anchors.centerIn: parent
            visible: frame.roomForTitle
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

                visible: modelData.name === "close" ? frame.roomForClose
                                                    : frame.roomForMaximize
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
