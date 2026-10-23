// What the native lock shows on a monitor when its own scene is missing, would
// not load, or failed while it ran: the time, the date, "Locked", and the
// password field, so the session can still be unlocked; nothing of the
// session.
//
// Compiled into the compositor (`lock_scene.rs`, `include_str!`), so it loads
// when nothing on disk does: a broken install, a deleted configuration, a
// scene with a typo in it. For the same reason it uses nothing of the
// `Solium` module that is a file on disk, not `Theme` and not `SecretInput`:
// only `Lock` and `SecretField`, which are compiled in too (`qml/lock.h`),
// and its dots are drawn here. Its grey is the lock's own backdrop,
// `lock::BLANK`, drawn under it on every frame.
//
// What is typed is never here: the field is told how many characters there
// are, and Return, typed anywhere, asks Rust to check them.
//
// Editing this file changes the next build of the compositor, not a running
// one: it is never reloaded. A lock screen of your own is a scene of your own
// (`lock.scene`), and this is what stands in when that one cannot.

import QtQuick
import Solium

Rectangle {
    id: root

    color: "#1a1a1a"

    // How many dots the field shows: what a test reads.
    readonly property int secretLength: field.length

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

        // The password, as dots: one for each character typed, dimmed while
        // it is being checked or the field takes nothing.
        SecretField {
            id: field
            objectName: "secret"
            anchors.horizontalCenter: parent.horizontalCenter
            width: 320
            height: 40

            Rectangle {
                anchors.fill: parent
                radius: height / 2
                color: "#262626"
                border.width: 1
                border.color: field.accepting ? "#5a5a5a" : "#333333"
            }

            Row {
                anchors.centerIn: parent
                spacing: 8
                Repeater {
                    model: Math.min(field.length, 24)
                    Rectangle {
                        width: 9
                        height: 9
                        radius: 4.5
                        color: field.accepting ? "#e6e6e6" : "#808080"
                    }
                }
            }
        }

        // What the lock says of the password: wrong, locked out for so long,
        // or how to unlock from another console while it cannot be checked.
        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            text: Lock.state === "lockedOut"
                ? "Try again in " + Lock.retryAfter + " s"
                : Lock.state === "unavailable"
                    ? "Can't check your password right now. Retrying\u2026\n" + Lock.message
                    : Lock.message
            horizontalAlignment: Text.AlignHCenter
            color: "#b3b3b3"
            font.pixelSize: 15
        }
    }
}
