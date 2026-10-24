// The lock screen Solium ships: `lock.scene = "Lock"`, the default.
//
// One of these is built for every monitor as the session starts and kept
// between locks, so the first locked frame is already this (`lock_scene.rs`).
// While the session is unlocked it is kept quiet: nothing draws it, and its
// clock's Timer runs only while `Lock.locked`.
//
// What it shows, as docs/ricing.md's "Your own lock screen" and the design
// (04-ui.md §4.16) lay it out:
//
// * the wallpaper, dimmed;
// * the date and a big clock;
// * on the monitor `lock.prompt_on` names (`Lock.promptOn`): the user, the
//   password field (`SecretInput`), and the line that says what is
//   happening: a wrong password, a lockout counting down, or how to unlock
//   from another console while the password cannot be checked;
// * on a touchscreen, a swipe up that follows the finger lifts the clock away
//   and brings up the passcode pad (`SecretKeypad`, laid out as
//   `lock.keypad` says); a swipe down puts it away;
// * all of it on one sheet, as an iPhone's or an iPad's lock is. As the
//   session locks (`Lock.state === "locking"`) the sheet comes down from the
//   top over the desktop, which Solium draws under it for `lock.enter_ms`,
//   with a soft shadow on its lower edge while it moves. Once Rust has
//   unlocked (`"unlocking"`) it goes back up and off the top, uncovering the
//   desktop, which has the keyboard and the pointer already. Solium ends
//   either at its bound whatever this is doing, so each one lasts exactly
//   that long. A lock taken before sleep has no entry: the sheet is down at
//   once.
// * pulled down by a finger before any of that (`Lock.state === "peeking"`,
//   which a configuration's own gesture asks for with `sol.lock_peek`), the
//   sheet is exactly `Lock.progress` of the way down, with no lag behind the
//   finger. Locked from there, it comes the rest of the way down in what is
//   left of the entry; let go, it goes back up as it does at an unlock.
//
// Nothing here sees the password. Keys go straight to Rust, which tells the
// field only how many characters there are; a key on the pad is found and
// typed by Rust too, and the scene is told only that one was pressed.
// Nothing here can unlock either: only Rust does, on `solium-auth`'s yes.
//
// To make your own, copy this to ~/.config/solium/qml/lock/Lock.qml (it is
// then the one used) or anywhere and name it in `lock.scene`.

import QtQuick
import Solium

Item {
    id: root

    // The picture under the lock: the wallpaper Solium ships. A lock screen
    // of your own shows whatever it likes.
    property url wallpaper: "../wallpaper/solium.png"

    // Whether this monitor shows the password field: the one `lock.prompt_on`
    // names. The others show the wallpaper, the date and the clock.
    readonly property bool prompt: Lock.promptOn !== "" && Lock.promptOn === Solium.monitor.name
    // How many dots the field shows: what a test reads.
    readonly property int secretLength: field.length
    // Whether the passcode pad can be shown and typed on.
    readonly property bool padShown: root.prompt && root.lift > 0.01

    // How far the passcode pad is up, from 0 (the clock) to 1 (the pad):
    // the finger's, while a swipe lasts.
    property real lift: 0
    // How far down the sheet is, from 0 (above the top, off the screen) to 1
    // (covering it): what a test reads.
    property real down: 1
    // Whether the sheet is moving, which is when its shadow shows.
    readonly property bool moving: root.down > 0 && root.down < 1

    // The wall clock, read again at the start of every minute, and only
    // while locked or pulled down.
    property date now: new Date()

    Timer {
        interval: Math.max(1000, 60000 - root.now.getSeconds() * 1000 - root.now.getMilliseconds())
        running: Lock.locked || Lock.state === "peeking"
        repeat: true
        onTriggered: root.now = new Date()
    }

    // A new lock, or a scene built during one: the time now, the pad put
    // away, and the sheet coming down from where it is, which is off the
    // top unless an exit was still taking it there, or a finger had pulled
    // it part of the way (`Lock.progress`).
    function arrive() {
        root.now = new Date()
        settling.stop()
        root.lift = 0
        const exiting = leaving.running
        leaving.stop()
        if (Lock.state === "locking") {
            if (!exiting) {
                root.down = Lock.progress
            }
            entering.restart()
        } else {
            entering.stop()
            root.down = 1
        }
    }

    // Pulled down by a finger: the sheet is where the finger has it, at
    // once, whatever was moving it.
    function peek() {
        if (Lock.state !== "peeking") {
            return
        }
        entering.stop()
        leaving.stop()
        settling.stop()
        root.lift = 0
        root.down = Lock.progress
    }

    Component.onCompleted: {
        if (Lock.locked) {
            root.arrive()
        } else if (Lock.state === "peeking") {
            root.now = new Date()
            root.peek()
        }
    }
    Lock.onLockedChanged: if (Lock.locked) root.arrive()
    Lock.onProgressChanged: root.peek()
    Lock.onStateChanged: {
        if (Lock.state === "peeking") {
            root.now = new Date()
            root.peek()
        } else if (Lock.state === "unlocking") {
            entering.stop()
            leaving.restart()
        } else if (Lock.locked && Lock.state !== "locking" && root.down < 1) {
            // The entry is over, at its bound or cut short for sleep: the
            // sheet is down, whatever was left of its way.
            entering.stop()
            root.down = 1
        }
        if (Lock.state === "failed") {
            shake.restart()
        }
    }

    // The sheet coming down, for all of `lock.enter_ms`, on a curve that
    // slows into place and never passes it: iOS's sheets. From a peek, for
    // what is left of it, which is what Solium waits for.
    NumberAnimation {
        id: entering
        target: root
        property: "down"
        to: 1
        duration: Math.max(1, Lock.enterMs * (1 - Lock.progress))
        easing.type: Easing.BezierSpline
        easing.bezierCurve: [0.32, 0.72, 0, 1, 1, 1]
    }

    // The sheet going back up and off the top, from wherever it is, for all
    // of `lock.exit_ms`: easing out of place and into its way up, then
    // slowing as it leaves, so it is seen to slide off rather than vanish.
    NumberAnimation {
        id: leaving
        target: root
        property: "down"
        to: 0
        duration: Math.max(1, Lock.exitMs)
        easing.type: Easing.BezierSpline
        easing.bezierCurve: [0.4, 0, 0.2, 1, 1, 1]
    }

    // The pad going up or down after the finger lets go, on `snappy`'s curve.
    NumberAnimation {
        id: settling
        target: root
        property: "lift"
        duration: 263
        easing.type: Easing.OutCubic
    }

    function settle(up) {
        settling.stop()
        settling.to = up ? 1 : 0
        settling.start()
    }

    // The sheet: everything below moves with it.
    Item {
        id: sheet
        width: root.width
        height: root.height
        y: -(1 - root.down) * root.height

        // Its shadow, on the desktop just under its lower edge, while it moves:
        // faded in over the first fifth of the way down and out over the last
        // tenth, so a pull just begun casts none by the clock, and a sheet
        // all but down does not leave a band along the bottom.
        Rectangle {
            anchors.top: parent.bottom
            width: parent.width
            height: 48
            opacity: Math.min(1, root.down / 0.2) * Math.min(1, (1 - root.down) / 0.1)
            visible: root.moving && opacity > 0.01
            gradient: Gradient {
                GradientStop { position: 0; color: "#59000000" }
                GradientStop { position: 1; color: "#00000000" }
            }
        }

        // The wallpaper, dimmed, and dimmed a little more under the pad.
        Item {
            anchors.fill: parent

            Rectangle {
                anchors.fill: parent
                color: "#0d0d0f"
            }
            Image {
                anchors.fill: parent
                source: root.wallpaper
                // Decoded once, as the scene is built ahead of the lock, and not
                // again at the size it is first drawn at.
                fillMode: Image.PreserveAspectCrop
            }
            Rectangle {
                anchors.fill: parent
                color: "#000000"
                opacity: 0.45 + 0.2 * root.lift
            }
        }

        // Everything on top of the wallpaper.
        Item {
            id: content
            anchors.fill: parent

            // The date and the clock, lifted away as the pad comes up.
            Column {
                id: clock
                anchors.horizontalCenter: parent.horizontalCenter
                y: root.height * 0.16 - root.lift * root.height * 0.1
                opacity: 1 - 0.6 * root.lift
                spacing: 4

                Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    text: Qt.formatDate(root.now, Qt.locale().dateFormat(Locale.LongFormat))
                    color: Theme.text
                    opacity: 0.8
                    font.pixelSize: 22
                }

                Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    text: Qt.formatTime(root.now, Qt.locale().timeFormat(Locale.ShortFormat))
                    color: Theme.text
                    font.pixelSize: 96
                    font.weight: Font.Light
                }
            }

            // The user and the password, on the prompt's monitor only.
            Column {
                id: who
                visible: root.prompt
                anchors.horizontalCenter: parent.horizontalCenter
                y: root.height * 0.58 - root.lift * root.height * 0.3
                spacing: 10

                // The picture AccountsService has for the user, or their
                // initial in a circle.
                Item {
                    anchors.horizontalCenter: parent.horizontalCenter
                    width: 72
                    height: 72

                    Rectangle {
                        anchors.fill: parent
                        radius: width / 2
                        color: Theme.surface
                        border.width: 1
                        border.color: Theme.edge
                        visible: avatar.status !== Image.Ready

                        Text {
                            anchors.centerIn: parent
                            text: ((Lock.user.realName || Lock.user.name || "?") + "").charAt(0).toUpperCase()
                            color: Theme.text
                            font.pixelSize: 30
                        }
                    }

                    Image {
                        id: avatar
                        anchors.fill: parent
                        source: Lock.user.avatar || ""
                        fillMode: Image.PreserveAspectCrop
                        sourceSize.width: 144
                        sourceSize.height: 144
                    }
                }

                Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    text: Lock.user.realName || Lock.user.name || ""
                    color: Theme.text
                    font.pixelSize: 18
                }

                // The field, which shakes at a wrong password and pulses while
                // the password is checked.
                Item {
                    anchors.horizontalCenter: parent.horizontalCenter
                    width: field.width
                    height: field.height

                    SecretInput {
                        id: field
                        objectName: "secret"
                        width: 300
                        height: 40
                        x: 0
                    }

                    SequentialAnimation {
                        id: shake
                        NumberAnimation { target: field; property: "x"; to: 8; duration: Theme.quick / 2 }
                        NumberAnimation { target: field; property: "x"; to: -5; duration: Theme.quick }
                        NumberAnimation { target: field; property: "x"; to: 3; duration: Theme.quick }
                        NumberAnimation { target: field; property: "x"; to: 0; duration: Theme.quick / 2 }
                    }

                    SequentialAnimation {
                        running: Lock.locked && Lock.state === "checking"
                        loops: Animation.Infinite
                        onRunningChanged: if (!running) field.opacity = 1
                        NumberAnimation { target: field; property: "opacity"; to: 0.45; duration: Theme.quick * 2 }
                        NumberAnimation { target: field; property: "opacity"; to: 1; duration: Theme.quick * 2 }
                    }
                }

                // What the lock says of the password.
                Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    horizontalAlignment: Text.AlignHCenter
                    width: Math.min(root.width - 48, 520)
                    wrapMode: Text.Wrap
                    color: Theme.text
                    opacity: 0.8
                    font.pixelSize: 15
                    text: {
                        switch (Lock.state) {
                        case "failed":
                            return "Wrong password · " + Lock.attempts
                                + (Lock.attempts === 1 ? " attempt" : " attempts")
                        case "lockedOut": {
                            const left = Lock.retryAfter
                            const seconds = left % 60
                            return "Try again in " + Math.floor(left / 60) + ":"
                                + (seconds < 10 ? "0" : "") + seconds
                        }
                        case "unavailable":
                            return (Lock.retryAfter > 0
                                    ? "Can't check your password right now. Retrying…\n"
                                    : "Can't check your password.\n") + Lock.message
                        default:
                            return ""
                        }
                    }
                }

                // The layout the password is typed in, and Caps Lock.
                Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    text: (Keyboard.layoutShort || "") + (Keyboard.caps ? "  ⇪" : "")
                    color: Theme.text
                    opacity: 0.55
                    font.pixelSize: 13
                }
            }

            // The passcode pad, brought up by a swipe.
            SecretKeypad {
                id: pad
                visible: root.padShown
                opacity: root.lift
                anchors.horizontalCenter: parent.horizontalCenter
                y: who.y + who.height + 16 + (1 - root.lift) * 40
                width: layout === "keyboard" ? Math.min(root.width - 48, 760) : 264
                height: layout === "keyboard" ? 280 : 344

                delegate: Item {
                    id: key
                    required property string label
                    property string letters
                    property string kind
                    property int row
                    property int column

                    Rectangle {
                        anchors.fill: parent
                        anchors.margins: 6
                        radius: Math.min(width, height) / 2
                        color: key.kind === "key" ? Theme.surface : "transparent"
                        opacity: 0.85
                        border.width: key.kind === "key" ? 1 : 0
                        border.color: Theme.edge
                    }

                    Column {
                        anchors.centerIn: parent
                        Text {
                            anchors.horizontalCenter: parent.horizontalCenter
                            text: key.label
                            color: Theme.text
                            font.pixelSize: key.kind === "key" ? 26 : 15
                        }
                        Text {
                            anchors.horizontalCenter: parent.horizontalCenter
                            visible: key.letters !== ""
                            text: key.letters
                            color: Theme.text
                            opacity: 0.6
                            font.pixelSize: 10
                            font.letterSpacing: 1.5
                        }
                    }
                }
            }
        }

    }

    // The swipe: a finger dragged up lifts the pad into view and follows the
    // finger; let go past a third of the way, it goes up, and back down
    // otherwise. Down puts it away the same way. Only on the prompt's monitor,
    // only for a finger, and only while locked.
    DragHandler {
        id: swipe
        target: null
        enabled: root.prompt && Lock.locked
        acceptedDevices: PointerDevice.TouchScreen
        xAxis.enabled: false
        property real from: 0
        onActiveChanged: {
            if (active) {
                settling.stop()
                from = root.lift
            } else {
                root.settle(root.lift > 0.33)
            }
        }
        onTranslationChanged: {
            if (active) {
                const travel = root.height * 0.3
                root.lift = Math.max(0, Math.min(1, from - translation.y / travel))
            }
        }
    }
}
