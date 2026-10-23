// What the native lock shows on a monitor when its own scene is missing, would
// not load, or failed while it ran: the time, the date and "Locked", and
// nothing of the session.
//
// Compiled into the compositor (`lock_scene.rs`, `include_str!`), so it loads
// when nothing on disk does: a broken install, a deleted configuration, a
// scene with a typo in it. For the same reason it imports nothing but QtQuick,
// not even the `Solium` design system, whose files are on disk too. Its grey
// is the lock's own backdrop, `lock::BLANK`, drawn under it on every frame.
//
// Editing this file changes the next build of the compositor, not a running
// one: it is never reloaded. A lock screen of your own is a scene of your own
// (`lock.scene`), and this is what stands in when that one cannot.

import QtQuick

Rectangle {
    id: root

    color: "#1a1a1a"

    // The wall clock, read again at the start of every minute: once a minute
    // is all a clock showing minutes needs, and a lock screen on a desk all
    // night should not wake the compositor every second.
    property date now: new Date()

    Timer {
        interval: 60000 - root.now.getSeconds() * 1000 - root.now.getMilliseconds()
        running: true
        repeat: true
        onTriggered: root.now = new Date()
    }

    Column {
        anchors.centerIn: parent
        spacing: 12

        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            text: Qt.formatTime(root.now, Qt.locale().timeFormat(Locale.ShortFormat))
            color: "#e6e6e6"
            font.pixelSize: 96
            font.weight: Font.Light
        }

        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            text: Qt.formatDate(root.now, Qt.locale().dateFormat(Locale.LongFormat))
            color: "#b3b3b3"
            font.pixelSize: 22
        }

        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            topPadding: 24
            text: "Locked"
            color: "#808080"
            font.pixelSize: 18
        }
    }
}
