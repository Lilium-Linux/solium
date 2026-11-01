// The window's quick settings, inside the titlebar: a right click on the bar
// grows the bar (Frame.qml asks the compositor for the room with `growTop`,
// so the window's content moves down rather than being covered) and these
// show in the new part of it. Esc or a press anywhere else folds it away.
//
// Frame.qml places it, tells it about the window, and holds the `Grab` that
// keeps the pointer and the keys here while it is open.

import QtQuick
import QtQuick.Shapes
import Solium

Item {
    id: quick

    // Set by Frame.qml.
    property int windowId: 0
    property int pid: -1
    property bool open: false
    property bool fullscreen: false
    property bool floating: false
    property var windowValues: ({})
    // How much of it shows, 0..1, as the bar grows.
    property real reveal: 0

    // Its natural height, which the bar grows by: one row on a wide window,
    // two on a narrow one.
    readonly property real fullHeight: body.implicitHeight + 2 * quick.pad
    readonly property real pad: 8
    readonly property bool wide: quick.width >= 1000

    // Nothing, while the window's id is not known yet (a frame is built
    // before it is told), rather than null.
    readonly property var sound: Audio.forWindow(quick.windowId)
        || ({ present: false, muted: false, volume: 0, max: 1, step: 0.05 })
    readonly property bool above: quick.windowValues.above === true
    readonly property bool follow: quick.windowValues.follow === true

    signal closeRequested()

    Keys.onEscapePressed: quick.closeRequested()

    onOpenChanged: {
        if (open) {
            quick.forceActiveFocus();
            forceQuit.armed = false;
        }
    }

    component Glyph: Shape {
        id: glyph
        // One stroked path in a 16x16 box, for the action buttons.
        property string path: ""
        property color tint: Theme.text
        width: 16; height: 16
        // Qt's ordinary renderer, not the curve one: on the software path
        // the curve renderer ignores the clip and fade of what holds it, and
        // the glyphs showed below the growing bar.
        antialiasing: true
        ShapePath {
            fillColor: "transparent"
            strokeColor: glyph.tint
            strokeWidth: 1.5
            capStyle: ShapePath.RoundCap
            joinStyle: ShapePath.RoundJoin
            PathSvg { path: glyph.path }
        }
    }

    // A slider: a track, its fill and a knob, all animated when the value
    // changes from elsewhere and following the pointer at once while dragged.
    component Level: Item {
        id: level
        property real value: 0
        property real from: 0
        property real to: 1
        property color tint: "#b9a6ff"
        signal moved(real value)
        readonly property real share: Math.max(0, Math.min(1, (value - from) / (to - from)))
        height: 20

        Rectangle {
            id: track
            anchors.verticalCenter: parent.verticalCenter
            width: parent.width
            height: 4
            radius: 2
            color: Qt.rgba(1, 1, 1, 0.14)
            Rectangle {
                height: parent.height
                radius: parent.radius
                width: knob.x + knob.width / 2
                color: level.tint
            }
        }
        Rectangle {
            id: knob
            width: 14; height: 14; radius: 7
            anchors.verticalCenter: parent.verticalCenter
            x: level.share * (level.width - width)
            color: "#f4f1ff"
            scale: drag.pressed ? 1.2 : (drag.containsMouse ? 1.1 : 1)
            Behavior on x { enabled: !drag.pressed; NumberAnimation { duration: 140; easing.type: Easing.OutCubic } }
            Behavior on scale { NumberAnimation { duration: 120; easing.type: Easing.OutCubic } }
        }
        MouseArea {
            id: drag
            anchors.fill: parent
            anchors.topMargin: -4
            anchors.bottomMargin: -4
            hoverEnabled: true
            function pick(mouseX) {
                const share = Math.max(0, Math.min(1, (mouseX - knob.width / 2) / (level.width - knob.width)));
                level.moved(level.from + share * (level.to - level.from));
            }
            onPressed: (mouse) => pick(mouse.x)
            onPositionChanged: (mouse) => { if (pressed) pick(mouse.x); }
        }
    }

    // An action: a glyph and a word side by side, lit while hovered.
    component Action: Rectangle {
        id: action
        property string label: ""
        property string glyph: ""
        property color tint: Theme.text
        property bool active: false
        signal clicked()
        height: 28
        radius: 8
        color: action.active ? Qt.rgba(0.72, 0.65, 1, 0.24)
             : hover.containsMouse ? Qt.rgba(1, 1, 1, 0.12) : Qt.rgba(1, 1, 1, 0.06)
        scale: hover.pressed ? 0.95 : 1
        Behavior on color { ColorAnimation { duration: 140 } }
        Behavior on scale { NumberAnimation { duration: 120; easing.type: Easing.OutCubic } }
        Row {
            anchors.centerIn: parent
            spacing: 6
            Glyph {
                anchors.verticalCenter: parent.verticalCenter
                path: action.glyph
                tint: action.tint
            }
            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: action.label
                color: action.tint
                font.family: "Noto Sans"
                font.pixelSize: 12
                // Shortened, and then left out, when the button is narrow.
                width: Math.min(implicitWidth, action.width - 16 - 22)
                // Left out only when not even a short word fits.
                visible: action.width - 16 - 22 >= Math.min(implicitWidth, 24)
                elide: Text.ElideRight
                Behavior on color { ColorAnimation { duration: 140 } }
            }
        }
        MouseArea {
            id: hover
            anchors.fill: parent
            hoverEnabled: true
            onClicked: action.clicked()
        }
    }

    // A level with its glyph before it and its value after it.
    component Setting: Item {
        id: setting
        property string glyph: ""
        property color glyphTint: Theme.text
        property string value: ""
        property real from: 0
        property real to: 1
        property real current: 0
        property color tint: "#b9a6ff"
        property bool live: true
        signal glyphClicked()
        signal moved(real value)
        height: 28
        Rectangle {
            id: glyphButton
            width: 26; height: 26; radius: 13
            anchors.verticalCenter: parent.verticalCenter
            color: glyphHover.containsMouse ? Qt.rgba(1, 1, 1, 0.12) : "transparent"
            Behavior on color { ColorAnimation { duration: 120 } }
            Glyph {
                anchors.centerIn: parent
                path: setting.glyph
                tint: setting.glyphTint
            }
            MouseArea {
                id: glyphHover
                anchors.fill: parent
                hoverEnabled: true
                onClicked: setting.glyphClicked()
            }
        }
        Level {
            id: level
            x: glyphButton.width + 6
            width: setting.width - x - valueText.width - 8
            anchors.verticalCenter: parent.verticalCenter
            from: setting.from
            to: setting.to
            value: setting.current
            tint: setting.tint
            enabled: setting.live
            opacity: setting.live ? 1 : 0.4
            onMoved: (v) => setting.moved(v)
        }
        Text {
            id: valueText
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            width: 38
            horizontalAlignment: Text.AlignRight
            text: setting.value
            color: Theme.textDim
            font.family: "Noto Sans"
            font.pixelSize: 12
            font.features: { "tnum": 1 }
        }
    }

    clip: true

    // Everything across the whole of the room the bar grew by: on a wide
    // window the volume and the six actions in one row, on a narrower one the
    // volume above the actions.
    Grid {
        id: body
        x: quick.pad + 4
        y: quick.pad
        width: quick.width - 2 * x
        columns: quick.wide ? 2 : 1
        columnSpacing: 16
        rowSpacing: 8
        // Slides down with the bar as it grows, and shows once most of the
        // room is there.
        transform: Translate { y: (1 - quick.reveal) * -8 }
        opacity: Math.max(0, (quick.reveal - 0.35) / 0.65)
        // Drawn as one picture while it fades, so the clip and the fade
        // reach the glyphs: on its own a Shape here ignores both.
        layer.enabled: quick.reveal > 0 && quick.reveal < 0.999

        // The application's volume, the speaker muting it.
        Setting {
            width: quick.wide ? 240 : body.width
            glyph: quick.sound.muted
                ? "M2 6 H5 L9 3 V13 L5 10 H2 Z M11 6 L15 10 M15 6 L11 10"
                : "M2 6 H5 L9 3 V13 L5 10 H2 Z M11.5 5.5 Q13.5 8 11.5 10.5"
            glyphTint: quick.sound.present ? Theme.text : Theme.textDim
            value: !quick.sound.present ? "–" : quick.sound.muted ? "off" : Math.round(quick.sound.volume * 100) + "%"
            onGlyphClicked: if (quick.sound.present) quick.sound.toggleMuted()
            live: quick.sound.present
            to: Math.max(1, quick.sound.max || 1)
            current: quick.sound.present ? quick.sound.volume : 0
            tint: quick.sound.muted ? "#6f6880" : "#b9a6ff"
            onMoved: (v) => quick.sound.setVolume(v)
        }

        Row {
            id: actions
            width: quick.wide ? body.width - 240 - body.columnSpacing : body.width
            spacing: 6
            readonly property real cell: (width - 5 * spacing) / 6

            Action {
                width: actions.cell
                label: "Keep above"
                active: quick.above
                // Two layers, the upper one lifted.
                glyph: "M2 10 H14 V14 H2 Z M4 2 H12 V7 H4 Z"
                onClicked: Solium.send("above", {})
            }
            Action {
                width: actions.cell
                label: "Follow workspace"
                active: quick.follow
                // A pin.
                glyph: "M8 14 V9.5 M4 9.5 H12 L10.5 6.5 V2 H5.5 V6.5 Z"
                onClicked: Solium.send("follow", {})
            }
            Action {
                width: actions.cell
                label: quick.floating ? "Tile" : "Float"
                glyph: quick.floating
                    ? "M2 2 H7 V7 H2 Z M9 2 H14 V7 H9 Z M2 9 H7 V14 H2 Z M9 9 H14 V14 H9 Z"
                    : "M2 4 H11 V13 H2 Z M5 4 V2 H14 V11 H11"
                onClicked: Solium.send("tile", {})
            }
            Action {
                width: actions.cell
                label: "Fullscreen"
                active: quick.fullscreen
                glyph: "M2 6 V2 H6 M10 2 H14 V6 M14 10 V14 H10 M6 14 H2 V10"
                onClicked: Solium.send("fullscreen", {})
            }
            Action {
                width: actions.cell
                label: "Screenshot"
                glyph: "M2 5 H5 L6.5 3 H9.5 L11 5 H14 V13 H2 Z M8 6.5 A2.5 2.5 0 1 0 8 11.5 A2.5 2.5 0 1 0 8 6.5"
                onClicked: Solium.send("screenshot", {})
            }
            Action {
                id: forceQuit
                // Two clicks: the first arms it, and it disarms itself.
                property bool armed: false
                width: actions.cell
                label: armed ? "Sure?" : "Force quit"
                tint: armed ? "#ff8a80" : "#ec6a5e"
                active: armed
                glyph: "M8 2 V8 M4.5 4.5 A5 5 0 1 0 11.5 4.5"
                enabled: quick.pid > 1
                opacity: enabled ? 1 : 0.4
                onClicked: {
                    if (armed) {
                        armed = false;
                        Solium.send("kill", {});
                    } else {
                        armed = true;
                        disarm.restart();
                    }
                }
                // An animation, not a Timer, so it runs on the
                // compositor's clock (Frame.qml says why).
                SequentialAnimation {
                    id: disarm
                    PauseAnimation { duration: 2500 }
                    ScriptAction { script: forceQuit.armed = false }
                }
            }
        }
    }
}
