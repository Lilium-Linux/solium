// A titlebar that is not there until you approach the window.
//
// Reserves nothing, so the client keeps the whole slot and nothing reflows
// when the bar appears — the frame simply draws over the window. That is what
// an inset of zero buys: an overlay rather than a band.
//
// The bar slides out of the window's own top edge, which reads as the window
// producing it rather than as a panel fading in on top.

import QtQuick
import Solium

Item {
    id: frame

    property int insetTop: 0
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

    readonly property int barHeight: 34

    Rectangle {
        id: bar

        anchors { left: parent.left; right: parent.right }
        height: frame.barHeight
        color: frame.focused ? Theme.surface : Theme.surfaceInactive
        radius: 8

        // Tucked just above the window when hidden, so it comes out from under
        // its own edge instead of materialising.
        y: frame.pointerInside ? 0 : -height
        opacity: frame.pointerInside ? 0.96 : 0.0

        Behavior on y {
            NumberAnimation { duration: 260; easing.type: Easing.OutBack; easing.overshoot: 0.9 }
        }
        Behavior on opacity { NumberAnimation { duration: 200 } }

        Text {
            anchors.centerIn: parent
            width: Math.min(implicitWidth, Math.max(frame.width - 140, 0))
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
}
