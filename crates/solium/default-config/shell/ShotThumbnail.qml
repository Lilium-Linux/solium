// A screenshot just taken, as macOS shows one: a white flash, then the
// picture shrinks from where it was on screen into the bottom-right corner,
// stays ten seconds -- the number on it counts them down -- and slides out.
// Hovering it stops the count; a click opens the file.
//
// It goes to the monitor that was active when the shot was asked for, and
// stays there. A new shot, or the tool opening, takes it off screen at once,
// before the next picture is taken.

import QtQuick
import Solium

Item {
    id: root

    // The bar (Shell.qml), for its colours and height.
    required property Item shell
    // From screenshot.lua: { serial, phase, path, from, rect, desktop }.
    property var shot: ({})

    readonly property int seconds: 10
    property int remaining: seconds
    property bool mine: false
    property int attempt: 0
    property string path: ""
    property string phase: "gone"   // gone, waiting, flying, resting, leaving

    // Where it lands: 260 wide, or less so it is no taller than 170.
    readonly property real aspect: picture.implicitHeight > 0 ? picture.implicitWidth / picture.implicitHeight : 16 / 9
    readonly property real restW: Math.min(260, 170 * aspect)
    readonly property real restH: restW / aspect
    readonly property real restX: width - restW - 16
    readonly property real restY: height - shell.barHeight - restH - 16

    onShotChanged: {
        if (!shot)
            return
        // Whatever is up goes at once, so the next picture does not have it.
        fly.stop(); out.stop(); tick.stop()
        phase = "gone"
        if (shot.phase !== "taken")
            return
        mine = Solium.monitor.active
        path = shot.path
        attempt = 0
        phase = "waiting"
    }

    Timer {
        // Asked again until the file is there; ten seconds, then given up.
        interval: 100
        repeat: true
        running: root.phase === "waiting" && picture.status !== Image.Ready
        onTriggered: if (++root.attempt > 100) root.phase = "gone"
    }

    Connections {
        target: picture
        function onStatusChanged() {
            if (picture.status === Image.Ready && root.phase === "waiting")
                root.start()
        }
    }

    function start() {
        flash.play()
        if (!mine) {
            phase = "gone"
            return
        }
        // From where the shot was on this monitor, at its size.
        const r = shot.rect || shot.desktop
        frame.x = r.x - Solium.monitor.whole.x
        frame.y = r.y - Solium.monitor.whole.y
        frame.width = r.w
        frame.height = r.h
        remaining = seconds
        phase = "flying"
        fly.restart()
    }

    function leave() {
        tick.stop()
        phase = "leaving"
        out.restart()
    }

    // The flash: on every monitor for the whole desktop, on this one for a
    // region. A light, not a surface: white whatever the theme.
    Rectangle {
        id: flash
        anchors.fill: parent
        color: "#ffffff"
        opacity: 0
        Solium.input: false
        function play() {
            if (root.shot.from === "screen" || root.mine)
                flashing.restart()
        }
        SequentialAnimation {
            id: flashing
            PropertyAction { target: flash; property: "opacity"; value: 0.55 }
            NumberAnimation { target: flash; property: "opacity"; to: 0; duration: 220; easing.type: Easing.OutCubic }
        }
    }

    Item {
        id: frame
        visible: root.phase === "flying" || root.phase === "resting" || root.phase === "leaving"

        Rectangle {
            anchors.fill: parent
            anchors.margins: -1
            radius: 3
            color: "transparent"
            border { width: 1; color: "#40ffffff" }
            opacity: root.phase === "flying" ? 0 : 1
            Behavior on opacity { NumberAnimation { duration: 200 } }
        }
        Image {
            id: picture
            anchors.fill: parent
            cache: false
            smooth: true
            mipmap: true
            fillMode: Image.PreserveAspectCrop
            source: root.phase !== "gone" && root.path
                    ? "file://" + root.path + "?" + root.shot.serial + "-" + root.attempt : ""
            layer.enabled: root.phase !== "flying"
            layer.smooth: true
        }

        // The seconds left, and nothing else.
        Rectangle {
            visible: root.phase === "resting" || root.phase === "leaving"
            anchors { right: parent.right; top: parent.top; margins: 8 }
            width: 26; height: 26
            radius: 13
            color: "#cc1c1b1d"
            border { width: 1; color: "#2e2c32" }
            Text {
                anchors.centerIn: parent
                text: root.remaining
                color: root.shell.text
                font { pixelSize: 13; family: root.shell.font; weight: Font.DemiBold; features: { "tnum": 1 } }
            }
        }

        HoverHandler { id: hover; enabled: root.phase === "resting" }
        TapHandler {
            enabled: root.phase === "resting"
            onTapped: {
                Solium.send("shot.open", { path: root.path })
                root.leave()
            }
        }
        scale: hover.hovered ? 1.03 : 1
        Behavior on scale { NumberAnimation { duration: 140; easing.type: Easing.OutCubic } }
    }

    ParallelAnimation {
        id: fly
        NumberAnimation { target: frame; property: "x"; to: root.restX; duration: 520; easing.type: Easing.InOutCubic }
        NumberAnimation { target: frame; property: "y"; to: root.restY; duration: 520; easing.type: Easing.InOutCubic }
        NumberAnimation { target: frame; property: "width"; to: root.restW; duration: 520; easing.type: Easing.InOutCubic }
        NumberAnimation { target: frame; property: "height"; to: root.restH; duration: 520; easing.type: Easing.InOutCubic }
        onFinished: {
            root.phase = "resting"
            tick.restart()
        }
    }

    Timer {
        id: tick
        interval: 1000
        repeat: true
        // The count stops while the pointer is on it.
        running: false
        onTriggered: {
            if (hover.hovered)
                return
            if (--root.remaining <= 0)
                root.leave()
        }
    }

    NumberAnimation {
        id: out
        target: frame
        property: "x"
        to: root.width + 24
        duration: 320
        easing.type: Easing.InCubic
        onFinished: root.phase = "gone"
    }
}
