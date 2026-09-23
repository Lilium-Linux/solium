// The one layer of `panes/reveal`: a bar tucked above the window's own edge.

import QtQuick
import Solium

Item {
    id: frame

    property string title: ""
    property bool focused: false
    property bool pointerInside: false
    property int contentWidth: 0
    property int contentHeight: 0

    property string action: ""
    property string hovered: ""

    // Painted outside the insets -- the bar floats over the window and reserves nothing -- so the whole frame is copied when
    // it changes rather than just its bands.
    property bool overlay: true

    // The bar's own height, and nothing to do with what the pane reserves:
    // `Pane.qml` reserves nothing at all, which is why this number lives only
    // here.
    readonly property int barHeight: 34

    // What a narrow tile leaves room for, by `top/Frame.qml`'s arithmetic and
    // for its reasons (#133): the title below 164px -- this file holds back
    // 140 for the buttons rather than 150 -- maximise below 59px, close below
    // 37px, and a bare bar under that. The centred title ends at `width - 70`
    // and the buttons start at `width - 47`, so the two never meet.
    readonly property bool roomForTitle: frame.width - 140 >= 24
    readonly property bool roomForClose: frame.width >= 37
    readonly property bool roomForMaximize: frame.width >= 59

    Rectangle {
        id: bar

        anchors { left: parent.left; right: parent.right }
        height: frame.barHeight
        color: frame.focused ? Theme.surface : Theme.surfaceInactive
        radius: 0

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
            visible: frame.roomForTitle
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
}
