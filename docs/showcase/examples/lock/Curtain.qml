// A lock screen that closes like a curtain: two halves slide in from the
// sides as the session locks and meet in the middle, and part again as it
// unlocks. A showcase example (docs/showcase.md): copy it to
// ~/.config/solium/qml/lock/Curtain.qml and say `lock = { scene = "Curtain" }`
// in user.lua.
//
// It is docs/ricing.md's minimal scene with another entry and exit: `shut`
// runs from 0 (open, the desktop showing) to 1 (closed), over `Lock.enterMs`
// as the session locks and back over `Lock.exitMs` as it unlocks. Solium
// draws the desktop under it while it moves and ends each at its bound, so
// the scene only has to look right at every value in between.

import QtQuick
import Solium

Item {
    id: root
    readonly property bool prompt: Lock.promptOn === Solium.monitor.name
    readonly property color ink: "#15131b"
    readonly property color accent: "#936dff"

    // 0 open, 1 closed.
    property real shut: 1

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
            root.shut = Lock.state === "locking" ? Lock.progress : 1
            if (Lock.state === "locking")
                motion.go(1, Lock.enterMs * (1 - Lock.progress), Easing.OutCubic)
        }
    }
    Lock.onStateChanged: {
        if (Lock.state === "unlocking")
            motion.go(0, Lock.exitMs, Easing.InOutCubic)
        else if (Lock.locked && Lock.state !== "locking") {
            motion.stop()
            root.shut = 1
        }
    }
    NumberAnimation {
        id: motion
        target: root; property: "shut"
        function go(to, ms, curve) {
            stop()
            motion.to = to
            motion.duration = Math.max(1, ms)
            motion.easing.type = curve
            start()
        }
    }

    // The two halves, each a little wider than half so they overlap where
    // they meet, with a violet edge on the side that leads.
    Repeater {
        model: 2
        Rectangle {
            readonly property bool leading: index === 0
            width: root.width / 2 + 1
            height: root.height
            x: leading ? -width * (1 - root.shut) : root.width / 2 + width * (1 - root.shut) - 1
            gradient: Gradient {
                orientation: Gradient.Horizontal
                GradientStop { position: 0; color: leading ? "#0e0c13" : root.ink }
                GradientStop { position: 1; color: leading ? root.ink : "#0e0c13" }
            }
            Rectangle {
                width: 2
                height: parent.height
                x: leading ? parent.width - width : 0
                color: root.accent
                opacity: 1 - root.shut * 0.7
            }
        }
    }

    // The time and the field come up once the halves have met.
    Column {
        anchors.centerIn: parent
        spacing: 18
        opacity: Math.max(0, (root.shut - 0.6) / 0.4)
        scale: 0.94 + 0.06 * opacity
        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            text: Qt.formatTime(root.now, "hh:mm")
            color: "#f4f3f7"
            font.pixelSize: 112
            font.weight: Font.Light
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
