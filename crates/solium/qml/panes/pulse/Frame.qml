// The one layer of `panes/pulse`: the bar, the sheen, and the breathing line.

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

    property string title: ""
    property bool focused: false
    property bool pointerInside: false
    property int contentWidth: 0
    property int contentHeight: 0

    property string action: ""
    property string hovered: ""

    // What a narrow tile leaves room for, by `top/Frame.qml`'s arithmetic and
    // for its reasons (#133): the title below 174px, maximise below 59px,
    // close below 37px, and a bare bar under that.
    readonly property bool roomForTitle: frame.width - 150 >= 24
    readonly property bool roomForClose: frame.width >= 37
    readonly property bool roomForMaximize: frame.width >= 59

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
            color: Theme.accent
            opacity: 0.12

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
            visible: frame.roomForTitle
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

                    visible: modelData.name === "close" ? frame.roomForClose
                                                        : frame.roomForMaximize
                    width: 13
                    height: 13
                    radius: width / 2
                    color: hit.containsMouse
                           ? modelData.tint
                           : (frame.focused ? Theme.control : Theme.controlInactive)
                    Behavior on color { ColorAnimation { duration: Theme.quick } }

                    // Its glyph, under the pointer: `×` to close, `+` to
                    // maximise, dark on the grey it turns. With no hue to say
                    // which button is which, the glyph and the shade do
                    // (`tests/scenarios/pane-buttons-drawn.lua`).
                    Rectangle {
                        anchors.centerIn: parent
                        width: 7
                        height: 1.5
                        radius: 0.75
                        antialiasing: true
                        color: Theme.accentInk
                        rotation: parent.modelData.name === "close" ? 45 : 0
                        visible: hit.containsMouse
                    }
                    Rectangle {
                        anchors.centerIn: parent
                        width: 7
                        height: 1.5
                        radius: 0.75
                        antialiasing: true
                        color: Theme.accentInk
                        rotation: parent.modelData.name === "close" ? -45 : 90
                        visible: hit.containsMouse
                    }

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
    // reserved height rather than below it: a layer that stays within its own
    // insets only has those copied when it changes, and this one changes on
    // every frame.
    //
    // On an opaque base in the bar's own grey, so it fades towards the bar
    // rather than towards whatever is behind the window: drawn straight over
    // the gap, it showed the wallpaper's colour through it on every unfocused
    // window and at every faint moment of a focused one
    // (`tests/scenarios/pane-pulse-drawn.lua`).
    Rectangle {
        anchors { left: parent.left; right: parent.right; top: bar.bottom }
        height: frame.insetTop - bar.height
        color: bar.color

        Rectangle {
            anchors.fill: parent
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
}
