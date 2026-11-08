// The bar: one full-width strip along the bottom of every monitor, with the
// current workspace's number, every open window, the tray, volume,
// notifications, the battery and a clock. A surface of its own (bar.lua), so
// a blur can sit behind exactly it (looks.lua); the pop-ups its buttons open
// are the shell's (Shell.qml), over the whole monitor. The Lua half of the
// tasks is ~/.config/solium/taskbar.lua.

import QtQuick
import Solium

Item {
    id: root

    // Written by bar.lua: the battery (taskbar.lua reads it), and which
    // pop-up the shell shows -- { what, monitor, x, id } -- so its button
    // looks pressed.
    property var battery: ({ present: false, percent: 0, status: "" })
    property var popup: ({})
    // The clock put away while the lock's is out of it, and how long it takes
    // to come back as that one lands (bar.lua).
    property bool clockAway: false
    property int clockLanding: 60

    // The tray: Solium's own (#363), absent on a Solium built before it.
    readonly property bool hasTray: typeof Tray !== "undefined"
    // Now playing: Solium's Media (#379), absent on a Solium built before it.
    readonly property bool hasMedia: typeof Media !== "undefined"
    readonly property bool audioReady: typeof Audio !== "undefined" && Audio.ready && Audio.output.present

    // The bar fills its surface; `topY` is where that is in the global space.
    readonly property int barHeight: height
    readonly property real topY: Solium.monitor.whole.y + Solium.monitor.whole.height - height
    readonly property color barColor: "#1c1b1d"
    readonly property color hover: "#2b2a2e"
    readonly property color pressed: "#36343a"
    readonly property color active: "#322f3a"
    readonly property color accent: "#936dff"
    readonly property color accentDim: "#625f6b"
    readonly property color text: "#e9e8ec"
    readonly property color textDim: "#9c9aa3"
    readonly property string font: "Noto Sans"

    function isOpen(what, id) {
        return !!popup && popup.what === what && popup.monitor === Solium.monitor.name
            && (id === undefined || popup.id === id)
    }
    // Open a pop-up over `button`, or close it if it is the one open.
    function toggle(what, button, id) {
        Solium.send("bar.popup", { what: what, monitor: Solium.monitor.name,
                                   x: button.mapToItem(root, button.width / 2, 0).x, id: id })
    }

    // The speaker for the level, as the theme names it.
    function speakerIcon(size) {
        const v = audioReady ? Audio.output.volume : 0
        const muted = audioReady && Audio.output.muted
        const name = muted || v <= 0 ? "audio-volume-muted"
            : v < 0.34 ? "audio-volume-low" : v < 0.67 ? "audio-volume-medium" : "audio-volume-high"
        return "image://solium/icon/" + name + "?size=" + size
    }

    // Icons for the task buttons. A window's appId is often not an icon name
    // (Slack's window is "Slack", its app com.slack.Slack with its own icon),
    // so installed apps are indexed by desktop id, by the id's last part and
    // by name, and a window's appId is looked up in that first.
    property var appIcons: ({})
    property int appIconsVersion: 0

    function learnApp(id, name, icon) {
        if (!icon)
            return
        const keys = [id, id.split(".").pop(), name]
        for (const key of keys) {
            if (key && !(key.toLowerCase() in root.appIcons))
                root.appIcons[key.toLowerCase()] = icon
        }
        root.appIconsVersion++
    }

    function iconFor(appId, size) {
        root.appIconsVersion
        if (!appId)
            return ""
        const entry = Apps.get(appId)
        const icon = (entry.valid && entry.icon)
            || root.appIcons[appId.toLowerCase()]
            || root.appIcons[appId.split(".").pop().toLowerCase()]
            || appId
        // Apps hands over plain theme names ("kitty"), not provider URLs.
        return icon.indexOf("://") >= 0 ? icon
            : "image://solium/icon/" + encodeURIComponent(icon) + "?size=" + size
    }

    Repeater {
        model: Apps
        delegate: Item {
            Component.onCompleted: root.learnApp(model.id || "", model.name || "", model.icon || "")
        }
    }

    component BarButton: Rectangle {
        id: button
        property bool current: false
        signal clicked()
        signal wheel(int delta)
        height: root.barHeight - 8
        anchors.verticalCenter: parent ? parent.verticalCenter : undefined
        radius: 6
        color: area.pressed ? root.pressed
             : current ? root.active
             : area.containsMouse ? root.hover : "transparent"
        Behavior on color { ColorAnimation { duration: Theme.quick } }

        MouseArea {
            id: area
            anchors.fill: parent
            hoverEnabled: true
            onClicked: button.clicked()
            onWheel: (event) => button.wheel(event.angleDelta.y)
        }
    }

    Rectangle {
        id: bar
        anchors.fill: parent
        // See-through, with the glass (or the blur) behind it, looks.lua, showing.
        color: Qt.rgba(root.barColor.r, root.barColor.g, root.barColor.b, 0.3)

        Rectangle {
            anchors { left: parent.left; right: parent.right; top: parent.top }
            height: 1
            color: "#2e2c32"
        }

        // The workspace this monitor shows; the wheel steps through them.
        BarButton {
            id: workspace
            anchors { left: parent.left; leftMargin: 4 }
            width: 36
            onWheel: (delta) => Solium.send("workspace.step", { by: delta > 0 ? -1 : 1, monitor: Solium.monitor.name })

            Text {
                anchors.centerIn: parent
                text: Workspaces.showing(Solium.monitor.name).id || "1"
                color: root.accent
                font { pixelSize: 15; family: root.font; bold: true }
            }
        }

        // Every open window of this monitor, grouped by workspace in
        // workspace order, the groups divided by a "|".
        Item {
            id: tasks
            anchors {
                left: workspace.right; leftMargin: 6
                right: trailing.left; rightMargin: 6
                top: parent.top; bottom: parent.bottom
            }
            clip: true

            // Bumped whenever the groups move, so every button says again
            // where it is (a button's own x is within its group).
            property int moved: 0

            Row {
                id: groups
                anchors { top: parent.top; bottom: parent.bottom }
                onWidthChanged: tasks.moved++

                Repeater {
                    id: groupRepeater
                    model: WorkspaceList { monitor: Solium.monitor.name }

                    delegate: Row {
                        id: group
                        required property var model
                        required property int index
                        readonly property int count: windows.count
                        anchors { top: parent.top; bottom: parent.bottom }
                        visible: count > 0
                        spacing: 2
                        onXChanged: tasks.moved++

                        WindowList {
                            id: windows
                            monitor: Solium.monitor.name
                            workspace: group.model.id
                        }

                        // A divider before every group but the first shown.
                        Text {
                            anchors.verticalCenter: parent.verticalCenter
                            visible: {
                                groupRepeater.count; tasks.moved
                                for (let i = 0; i < group.index; i++) {
                                    const earlier = groupRepeater.itemAt(i)
                                    if (earlier && earlier.count > 0)
                                        return true
                                }
                                return false
                            }
                            text: "|"
                            leftPadding: 4
                            rightPadding: 4
                            color: root.accentDim
                            font { pixelSize: 16; family: root.font }
                        }

                        Repeater {
                            model: windows

                            delegate: BarButton {
                                id: task
                                readonly property bool isMinimized: model.minimized === true
                                width: 32
                                current: model.focused && !isMinimized
                                opacity: isMinimized ? 0.55 : 1
                                onClicked: Solium.send("task.click", { id: model.id })

                                // Where this button is, so a minimise can genie into it.
                                function report() {
                                    const at = task.mapToItem(root, 0, 0)
                                    Solium.send("task.rect", { id: model.id, monitor: Solium.monitor.name,
                                                               x: at.x, y: at.y + root.topY - Solium.monitor.whole.y,
                                                               w: task.width, h: task.height })
                                }
                                readonly property int moved: tasks.moved
                                onMovedChanged: Qt.callLater(report)
                                onXChanged: Qt.callLater(report)
                                Component.onCompleted: Qt.callLater(report)

                                // Just the icon, at the monitor's own pixel size so it
                                // is drawn 1:1 rather than scaled; the first letter of
                                // the app when it has none.
                                Image {
                                    id: taskIcon
                                    anchors.centerIn: parent
                                    width: 20; height: 20
                                    fillMode: Image.PreserveAspectFit
                                    asynchronous: true
                                    smooth: true
                                    mipmap: true
                                    source: root.iconFor(model.appId, Math.round(20 * (Solium.monitor.scale || 1)))
                                    visible: status === Image.Ready
                                    opacity: model.state === "loading" ? 0.5 : 1
                                }
                                Text {
                                    anchors.centerIn: parent
                                    visible: !taskIcon.visible
                                    text: ((model.appId || model.title || "?") + "").charAt(0).toUpperCase()
                                    color: task.current ? root.text : root.textDim
                                    font { pixelSize: 14; family: root.font; bold: true }
                                }
                                // Under the button, in the bar's bottom edge, so it
                                // does not pull the icon's centre off the bar's.
                                Rectangle {
                                    anchors { top: parent.bottom; topMargin: 1; horizontalCenter: parent.horizontalCenter }
                                    width: task.current ? 16 : 6
                                    height: 2
                                    radius: 1
                                    color: model.urgent ? "#ff8c4d" : task.current ? root.accent : root.accentDim
                                    Behavior on width { NumberAnimation { duration: Theme.quick } }
                                }
                            }
                        }
                    }
                }
            }
        }

        Row {
            id: trailing
            anchors { right: parent.right; rightMargin: 4; top: parent.top; bottom: parent.bottom }
            spacing: 2

            // Now playing (Solium's Media, #379): the cover, the title over
            // the artist, back / play-pause / next, and how far into the
            // track as a line along the bottom; a click brings the player
            // forward. Only while a player is there.
            Rectangle {
                id: nowPlaying
                readonly property var player: root.hasMedia ? Media.active : null
                visible: !!player && player.present && (player.title !== "" || player.playing)
                anchors.verticalCenter: parent.verticalCenter
                height: root.barHeight - 8
                width: visible ? playingRow.implicitWidth + 12 : 0
                radius: 6
                color: playingArea.containsMouse ? root.hover : "transparent"
                Behavior on color { ColorAnimation { duration: Theme.quick } }

                // A click brings the player forward (taskbar.lua).
                MouseArea {
                    id: playingArea
                    anchors.fill: parent
                    hoverEnabled: true
                    onClicked: Solium.send("media.raise", {
                        entry: nowPlaying.player.desktopEntry,
                        identity: nowPlaying.player.identity,
                        service: nowPlaying.player.service,
                    })
                }

                Row {
                    id: playingRow
                    anchors { left: parent.left; leftMargin: 6; verticalCenter: parent.verticalCenter }
                    spacing: 8

                    Rectangle {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 24; height: 24
                        radius: 4
                        color: root.active
                        clip: true
                        Image {
                            id: cover
                            anchors.fill: parent
                            fillMode: Image.PreserveAspectCrop
                            asynchronous: true
                            smooth: true
                            // Read by Solium when the player has it on disk or
                            // sends it; Spotify only names one on its CDN, which
                            // the bar then fetches itself.
                            source: !nowPlaying.player ? ""
                                : nowPlaying.player.art || nowPlaying.player.artUrl
                            sourceSize: Qt.size(48, 48)
                            visible: status === Image.Ready
                        }
                        // No cover: the player's own icon.
                        Image {
                            anchors.centerIn: parent
                            width: 16; height: 16
                            visible: !cover.visible
                            fillMode: Image.PreserveAspectFit
                            source: nowPlaying.player && (nowPlaying.player.icon || nowPlaying.player.desktopEntry)
                                ? root.iconFor(nowPlaying.player.icon || nowPlaying.player.desktopEntry,
                                               Math.round(16 * (Solium.monitor.scale || 1)))
                                : "image://solium/icon/audio-x-generic?size=16"
                        }
                    }

                    Column {
                        anchors.verticalCenter: parent.verticalCenter
                        width: Math.min(180, Math.max(titleText.implicitWidth, artistText.implicitWidth))
                        Text {
                            id: titleText
                            width: parent.width
                            text: nowPlaying.player ? nowPlaying.player.title || nowPlaying.player.identity : ""
                            textFormat: Text.PlainText
                            elide: Text.ElideRight
                            color: root.text
                            font { pixelSize: 12; family: root.font }
                        }
                        Text {
                            id: artistText
                            width: parent.width
                            visible: text !== ""
                            text: nowPlaying.player ? nowPlaying.player.artist : ""
                            textFormat: Text.PlainText
                            elide: Text.ElideRight
                            color: root.textDim
                            font { pixelSize: 11; family: root.font }
                        }
                    }

                    Row {
                        anchors.verticalCenter: parent.verticalCenter
                        Repeater {
                            model: [
                                { icon: "media-skip-backward", call: "previous", can: "canGoPrevious" },
                                { icon: "", call: "playPause", can: "canControl" },
                                { icon: "media-skip-forward", call: "next", can: "canGoNext" },
                            ]
                            delegate: BarButton {
                                id: control
                                required property var modelData
                                readonly property bool can: !!nowPlaying.player && nowPlaying.player[modelData.can]
                                width: 24
                                height: 24
                                opacity: can ? 1 : 0.35
                                onClicked: if (can) nowPlaying.player[modelData.call]()
                                Image {
                                    anchors.centerIn: parent
                                    width: 16; height: 16
                                    fillMode: Image.PreserveAspectFit
                                    source: "image://solium/icon/"
                                        + (control.modelData.icon
                                           || (nowPlaying.player && nowPlaying.player.playing
                                               ? "media-playback-pause" : "media-playback-start"))
                                        + "?size=" + Math.round(16 * (Solium.monitor.scale || 1))
                                }
                            }
                        }
                    }
                }

                // How far into the track: along the bottom edge, in the accent.
                Rectangle {
                    readonly property real done: nowPlaying.player && nowPlaying.player.length > 0
                        ? Math.min(1, nowPlaying.player.position / nowPlaying.player.length) : 0
                    visible: done > 0
                    anchors { left: parent.left; bottom: parent.bottom; leftMargin: 6 }
                    width: (parent.width - 12) * done
                    height: 2
                    radius: 1
                    color: root.accent
                    Behavior on width { NumberAnimation { duration: 900 } }
                }
            }

            // The tray: an icon for each application that put one there, hidden
            // while it asks to be (`passive`). Left activates it (or opens its
            // menu, for an item that is only a menu), middle is its second
            // action, right opens its menu, the wheel scrolls it.
            Repeater {
                model: root.hasTray ? Tray.items : null
                delegate: BarButton {
                    id: trayButton
                    required property var model
                    readonly property var item: Tray.get(model.id)
                    visible: model.status !== "passive"
                    width: visible ? 32 : 0
                    current: root.isOpen("tray", model.id)

                    // Where a point of the button is on the screen, for an
                    // application that places a window of its own there.
                    function onScreen(x, y) {
                        const at = trayButton.mapToItem(root, x, y)
                        return Qt.point(Math.round(at.x + Solium.monitor.whole.x),
                                        Math.round(at.y + root.topY))
                    }
                    function openMenu(x, y) {
                        const at = onScreen(x, y)
                        if (!root.isOpen("tray", model.id))
                            item.openMenu(at.x, at.y)
                        root.toggle("tray", trayButton, model.id)
                    }

                    Image {
                        anchors.centerIn: parent
                        width: 18; height: 18
                        fillMode: Image.PreserveAspectFit
                        asynchronous: true
                        smooth: true
                        mipmap: true
                        source: trayButton.model.icon || ""
                        sourceSize: Qt.size(36, 36)
                    }
                    Image {
                        anchors { right: parent.right; bottom: parent.bottom; margins: 4 }
                        width: 9; height: 9
                        fillMode: Image.PreserveAspectFit
                        source: trayButton.model.overlayIcon || ""
                        visible: source != ""
                    }
                    // Wanting attention: a dot in the accent.
                    Rectangle {
                        visible: trayButton.model.status === "attention"
                        anchors { right: parent.right; top: parent.top; margins: 5 }
                        width: 7; height: 7; radius: 3.5
                        color: root.accent
                    }
                    MouseArea {
                        anchors.fill: parent
                        acceptedButtons: Qt.LeftButton | Qt.MiddleButton | Qt.RightButton
                        onClicked: (mouse) => {
                            if (mouse.button === Qt.RightButton) {
                                trayButton.openMenu(mouse.x, mouse.y)
                            } else if (mouse.button === Qt.MiddleButton) {
                                const at = trayButton.onScreen(mouse.x, mouse.y)
                                trayButton.item.secondaryActivate(at.x, at.y)
                            } else if (trayButton.model.itemIsMenu && trayButton.model.hasMenu) {
                                trayButton.openMenu(mouse.x, mouse.y)
                            } else {
                                const at = trayButton.onScreen(mouse.x, mouse.y)
                                trayButton.item.activate(at.x, at.y)
                            }
                        }
                        onWheel: (wheel) => {
                            if (wheel.angleDelta.y !== 0)
                                trayButton.item.scroll(wheel.angleDelta.y, "vertical")
                            else if (wheel.angleDelta.x !== 0)
                                trayButton.item.scroll(wheel.angleDelta.x, "horizontal")
                        }
                    }
                }
            }

            // Volume: the speaker for the level; a click opens the slider,
            // the wheel moves it, a middle click mutes.
            BarButton {
                id: speaker
                visible: root.audioReady
                width: 36
                current: root.isOpen("volume")
                onClicked: root.toggle("volume", speaker)
                onWheel: (delta) => Audio.output.adjust(delta > 0 ? Audio.output.step : -Audio.output.step)

                Image {
                    anchors.centerIn: parent
                    width: 20; height: 20
                    source: root.speakerIcon(Math.round(20 * (Solium.monitor.scale || 1)))
                }
                MouseArea {
                    anchors.fill: parent
                    acceptedButtons: Qt.MiddleButton
                    onClicked: Audio.output.toggleMuted()
                }
            }

            // Notifications: the bell opens the list of earlier ones; a dot
            // while there are any, crossed out while banners are held back.
            BarButton {
                visible: typeof Notifications !== "undefined" && Notifications.serving
                width: 36
                id: bellButton
                current: root.isOpen("notifications")
                onClicked: root.toggle("notifications", bellButton)

                // A nod when one more is kept, so it is clear where a banner
                // that left went.
                property int kept: Notifications.history.count
                onKeptChanged: (kept > 0) && nod.restart()
                SequentialAnimation {
                    id: nod
                    NumberAnimation { target: bell; property: "scale"; to: 1.25; duration: 120; easing.type: Easing.OutCubic }
                    NumberAnimation { target: bell; property: "scale"; to: 1; duration: 260; easing.type: Easing.OutBack }
                }

                Image {
                    id: bell
                    anchors.centerIn: parent
                    width: 20; height: 20
                    source: "image://solium/icon/" + (Notifications.dnd ? "notifications-disabled"
                        : Notifications.history.count > 0 ? "notification-active" : "notification-inactive")
                        + "?size=" + Math.round(20 * (Solium.monitor.scale || 1))
                }
                Rectangle {
                    visible: Notifications.history.count > 0 && !Notifications.dnd
                    anchors { right: parent.right; top: parent.top; margins: 5 }
                    width: 7; height: 7
                    radius: 3.5
                    color: root.accent
                }
            }

            // Keyboard layout: click (or wheel) for the next one. A fixed
            // width, so switching never moves anything.
            BarButton {
                visible: Keyboard.layouts.length > 1
                width: 36
                onClicked: Solium.send("keyboard.next")
                onWheel: Solium.send("keyboard.next")

                Text {
                    anchors.centerIn: parent
                    text: Keyboard.layoutShort.toUpperCase()
                    color: root.text
                    font { pixelSize: 12; family: root.font; bold: true }
                }
            }

            // Battery: a small cell and the percentage.
            BarButton {
                id: batteryButton
                visible: root.battery.present
                width: batteryRow.implicitWidth + 16
                readonly property bool charging: root.battery.status === "Charging" || root.battery.status === "Full"
                readonly property color level: root.battery.percent <= 15 && !charging ? "#e5534b" : root.accent

                Timer {
                    interval: 30000
                    running: true
                    repeat: true
                    triggeredOnStart: true
                    onTriggered: Solium.send("battery.poll")
                }

                Row {
                    id: batteryRow
                    anchors.centerIn: parent
                    spacing: 6

                    Item {
                        anchors.verticalCenter: parent.verticalCenter
                        width: 24; height: 12
                        Rectangle {
                            id: cell
                            width: 21; height: 12
                            radius: 3
                            color: "transparent"
                            border { width: 1.5; color: root.textDim }
                            Rectangle {
                                x: 2.5; y: 2.5
                                height: parent.height - 5
                                width: Math.max(1, (parent.width - 5) * root.battery.percent / 100)
                                radius: 1.5
                                color: batteryButton.level
                            }
                        }
                        Rectangle {
                            anchors { left: cell.right; leftMargin: 1; verticalCenter: cell.verticalCenter }
                            width: 2; height: 5; radius: 1
                            color: root.textDim
                        }
                    }
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        text: (batteryButton.charging ? "⚡" : "") + root.battery.percent + "%"
                        color: root.text
                        font { pixelSize: 12; family: root.font }
                    }
                }
            }

            // Clock: time over date.
            BarButton {
                id: clock
                property date now: new Date()
                width: Math.max(timeText.implicitWidth, dateText.implicitWidth) + 20

                Timer {
                    interval: Math.max(1000, 1000 * (60 - clock.now.getSeconds()))
                    running: true
                    repeat: true
                    onTriggered: clock.now = new Date()
                }

                Column {
                    anchors.centerIn: parent
                    // Away at once, as the lock's clock lies on it; back over
                    // the last of the lock's exit, as that clock lands.
                    opacity: root.clockAway ? 0 : 1
                    Behavior on opacity { NumberAnimation { duration: root.clockAway ? 0 : root.clockLanding } }
                    Text {
                        id: timeText
                        anchors.horizontalCenter: parent.horizontalCenter
                        text: Qt.formatTime(clock.now, "HH:mm")
                        color: root.text
                        font { pixelSize: 12; family: root.font }
                    }
                    Text {
                        id: dateText
                        anchors.horizontalCenter: parent.horizontalCenter
                        text: Qt.formatDate(clock.now, "dd.MM.yyyy")
                        color: root.textDim
                        font { pixelSize: 11; family: root.font }
                    }
                }
            }
        }
    }

}
