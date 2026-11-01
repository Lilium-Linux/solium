// Volume for the taskbar: the pop-up the speaker button opens (a slider,
// mute, and which output plays), and the small capsule that shows the level
// for a moment when it changes from anywhere else -- the volume keys, a
// mixer. Solium reads PipeWire itself (`Audio`); this draws it and moves it.

import QtQuick
import Solium

Item {
    id: root

    // The bar (Shell.qml), for its colours and height.
    required property Item shell
    // Where the speaker button is, in this item, so the pop-up opens over it.
    property real anchorX: width - 200
    property bool open: false

    readonly property bool present: typeof Audio !== "undefined"
    readonly property var output: present ? Audio.output : null
    readonly property bool ready: present && Audio.ready && output && output.present
    readonly property real level: ready ? output.volume : 0
    readonly property bool muted: ready && output.muted

    // A speaker name for the level, as the icon theme spells it.
    function iconName(volume, isMuted) {
        if (isMuted || volume <= 0)
            return "audio-volume-muted"
        if (volume < 0.34)
            return "audio-volume-low"
        if (volume < 0.67)
            return "audio-volume-medium"
        return "audio-volume-high"
    }
    function icon(size) {
        return "image://solium/icon/" + iconName(root.level, root.muted) + "?size=" + size
    }

    // The level changed and nobody here moved it: show the capsule, on the
    // active monitor only. Not while the pop-up, which shows it anyway, is
    // open, and not for the first reading.
    property bool settled: false
    Timer { interval: 1500; running: root.ready; onTriggered: root.settled = true }
    onLevelChanged: cue()
    onMutedChanged: cue()
    function cue() {
        if (!settled || open || !Solium.monitor.active)
            return
        osd.shown = true
        hide.restart()
    }
    Timer { id: hide; interval: 1400; onTriggered: osd.shown = false }

    component Slider: Item {
        id: slider
        height: 24
        readonly property real max: root.ready ? Math.max(1, root.output.max) : 1
        readonly property real fraction: Math.min(1, root.level / max)

        Rectangle {
            id: track
            anchors { left: parent.left; right: parent.right; verticalCenter: parent.verticalCenter }
            height: 6
            radius: 3
            color: root.shell.active
            Rectangle {
                width: parent.width * slider.fraction
                height: parent.height
                radius: 3
                color: root.muted ? root.shell.accentDim : root.shell.accent
            }
        }
        Rectangle {
            x: (slider.width - width) * slider.fraction
            anchors.verticalCenter: parent.verticalCenter
            width: 16; height: 16
            radius: 8
            color: root.shell.text
        }
        MouseArea {
            anchors.fill: parent
            function set(x) {
                if (root.ready)
                    root.output.setVolume(Math.max(0, Math.min(1, x / width)) * slider.max)
            }
            onPressed: (mouse) => set(mouse.x)
            onPositionChanged: (mouse) => { if (pressed) set(mouse.x) }
            onWheel: (wheel) => { if (root.ready) root.output.adjust(wheel.angleDelta.y > 0 ? root.output.step : -root.output.step) }
        }
    }

    // The capsule: speaker, level, percent; violet like the keyboard's pill.
    Rectangle {
        id: osd
        property bool shown: false
        anchors { horizontalCenter: parent.horizontalCenter; bottom: parent.bottom; bottomMargin: root.shell.barHeight + 48 }
        width: 260
        height: 44
        radius: 22
        color: "#f21c1b1d"
        border { width: 1; color: "#2e2c32" }
        opacity: shown ? 1 : 0
        visible: opacity > 0
        scale: shown ? 1 : 0.92
        Behavior on opacity { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }
        Behavior on scale { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }
        Solium.input: false

        Image {
            id: osdIcon
            x: 14
            anchors.verticalCenter: parent.verticalCenter
            width: 20; height: 20
            source: root.icon(Math.round(20 * (Solium.monitor.scale || 1)))
        }
        Rectangle {
            anchors { left: osdIcon.right; leftMargin: 12; right: osdPercent.left; rightMargin: 12; verticalCenter: parent.verticalCenter }
            height: 6
            radius: 3
            color: root.shell.active
            Rectangle {
                width: parent.width * Math.min(1, root.level)
                height: parent.height
                radius: 3
                color: root.muted ? root.shell.accentDim : root.shell.accent
                Behavior on width { NumberAnimation { duration: 120 } }
            }
        }
        Text {
            id: osdPercent
            anchors { right: parent.right; rightMargin: 16; verticalCenter: parent.verticalCenter }
            width: 34
            horizontalAlignment: Text.AlignRight
            text: root.muted ? "off" : Math.round(root.level * 100) + "%"
            color: root.shell.text
            font { pixelSize: 13; family: root.shell.font; features: { "tnum": 1 } }
        }
    }

    // A click anywhere else closes the pop-up.
    MouseArea {
        anchors.fill: parent
        visible: root.open
        onPressed: root.open = false
    }

    Rectangle {
        id: popup
        visible: root.open
        x: Math.max(8, Math.min(root.width - width - 8, root.anchorX - width / 2))
        anchors { bottom: parent.bottom; bottomMargin: root.shell.barHeight + 8 }
        width: 320
        height: column.implicitHeight + 28
        radius: 14
        color: root.shell.barColor
        border { width: 1; color: "#2e2c32" }
        MouseArea { anchors.fill: parent }

        Column {
            id: column
            anchors { left: parent.left; right: parent.right; top: parent.top; margins: 14 }
            spacing: 10

            Text {
                width: parent.width
                text: root.ready ? root.output.label : "No sound output"
                textFormat: Text.PlainText
                color: root.shell.textDim
                font { pixelSize: 12; family: root.shell.font }
                elide: Text.ElideRight
            }

            Row {
                width: parent.width
                spacing: 10

                // Mute.
                Rectangle {
                    width: 32; height: 32
                    radius: 8
                    color: muteHover.hovered ? root.shell.hover : "transparent"
                    Image {
                        anchors.centerIn: parent
                        width: 20; height: 20
                        source: root.icon(Math.round(20 * (Solium.monitor.scale || 1)))
                    }
                    HoverHandler { id: muteHover }
                    TapHandler { onTapped: if (root.ready) root.output.toggleMuted() }
                }
                Slider {
                    anchors.verticalCenter: parent.verticalCenter
                    width: parent.width - 32 - 44 - 20
                }
                Text {
                    anchors.verticalCenter: parent.verticalCenter
                    width: 44
                    horizontalAlignment: Text.AlignRight
                    text: Math.round(root.level * 100) + "%"
                    color: root.shell.text
                    font { pixelSize: 13; family: root.shell.font; features: { "tnum": 1 } }
                }
            }

            // Which output plays, when there is more than one.
            Column {
                width: parent.width
                visible: root.present && Audio.outputs.count > 1
                spacing: 2
                Rectangle { width: parent.width; height: 1; color: "#2e2c32" }
                Item { width: 1; height: 4 }
                Repeater {
                    model: root.present ? Audio.outputs : null
                    delegate: Rectangle {
                        required property var model
                        width: column.width
                        height: 30
                        radius: 6
                        color: model.isDefault ? root.shell.active
                             : pick.hovered ? root.shell.hover : "transparent"
                        Text {
                            anchors { left: parent.left; leftMargin: 10; right: parent.right; rightMargin: 10; verticalCenter: parent.verticalCenter }
                            text: model.label
                            textFormat: Text.PlainText
                            color: model.isDefault ? root.shell.text : root.shell.textDim
                            font { pixelSize: 12; family: root.shell.font }
                            elide: Text.ElideRight
                        }
                        HoverHandler { id: pick }
                        TapHandler { onTapped: Audio.outputs.get(model.id).makeDefault() }
                    }
                }
            }
        }
    }
}
