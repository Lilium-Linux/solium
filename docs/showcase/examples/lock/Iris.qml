// A lock screen that opens and closes as an iris: a disc grows out of the
// middle of the screen until it covers it as the session locks, and shrinks
// back into the middle as it unlocks. A showcase example
// (docs/showcase.md): copy it to ~/.config/solium/qml/lock/Iris.qml and say
// `lock = { scene = "Iris" }` in user.lua.
//
// docs/ricing.md's minimal scene with another entry and exit, as Curtain.qml
// is: `open` runs from 0 to 1 over `Lock.enterMs` as the session locks and
// back over `Lock.exitMs` as it unlocks. The disc is a rounded Rectangle, so
// it draws the same on the software path as on the GPU.

import QtQuick
import Solium

Item {
    id: root
    readonly property bool prompt: Lock.promptOn === Solium.monitor.name
    readonly property color accent: "#936dff"

    // 0 a point in the middle, 1 covering the screen.
    property real open: 1

    property date now: new Date()
    Timer {
        interval: 60000
        running: Lock.locked
        repeat: true
        onTriggered: root.now = new Date()
    }
    Lock.onLockedChanged: {
        if (Lock.locked) {
            root.now = new Date()
            motion.stop()
            root.open = Lock.state === "locking" ? Lock.progress : 1
            if (Lock.state === "locking")
                motion.go(1, Lock.enterMs * (1 - Lock.progress), Easing.InOutCubic)
        }
    }
    Lock.onStateChanged: {
        if (Lock.state === "unlocking")
            motion.go(0, Lock.exitMs, Easing.InOutCubic)
        else if (Lock.locked && Lock.state !== "locking") {
            motion.stop()
            root.open = 1
        }
    }
    NumberAnimation {
        id: motion
        target: root; property: "open"
        function go(to, ms, curve) {
            stop()
            motion.to = to
            motion.duration = Math.max(1, ms)
            motion.easing.type = curve
            start()
        }
    }

    // Large enough at 1 to cover the corners.
    readonly property real reach: Math.sqrt(width * width + height * height)

    Rectangle {
        id: disc
        width: root.reach * root.open
        height: width
        radius: width / 2
        anchors.centerIn: parent
        gradient: Gradient {
            GradientStop { position: 0; color: "#1f1a2e" }
            GradientStop { position: 1; color: "#0d0b12" }
        }
        // The rim, brightest while the disc is small.
        border.width: 3
        border.color: Qt.rgba(0.58, 0.43, 1, 1 - root.open * 0.8)
    }

    Column {
        anchors.centerIn: parent
        spacing: 18
        opacity: Math.max(0, (root.open - 0.55) / 0.45)
        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            text: Qt.formatTime(root.now, "hh:mm")
            color: "#f4f3f7"
            font.pixelSize: 112
            font.weight: Font.Light
            scale: 0.8 + 0.2 * root.open
        }
        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            text: Qt.formatDate(root.now, "dddd d MMMM")
            color: root.accent
            font.pixelSize: 20
        }
        SecretInput {
            anchors.horizontalCenter: parent.horizontalCenter
            visible: root.prompt
        }
        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            visible: root.prompt
            color: "#c9c6d1"
            text: Lock.state === "failed" ? "Wrong password"
                : Lock.state === "lockedOut" ? "Try again in " + Lock.retryAfter + " s"
                : Lock.message
        }
    }
}
