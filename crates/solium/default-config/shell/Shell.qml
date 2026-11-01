// The shell over each monitor: the pop-ups the bar's buttons open (volume,
// notifications, a tray menu), the notification banners, quick search, the
// screenshot tool and thumbnail, and Alt+Tab's badges. The bar itself is a
// surface of its own (Bar.qml, bar.lua). The tasks' Lua half is
// ~/.config/solium/taskbar.lua.
//
// Based on the shipped preview shell (~/.local/share/solium/qml/preview); its
// quick search (super+d) is copied here as Search.qml, recoloured.

import QtQuick
import Solium

Item {
    id: root

    // Written here by the shipped preview Lua (dock.lua, search.lua); the
    // dock ones are unused but declared so setting them is not an error.
    property var dockPinned
    property string dockVisibility: "autohide"
    property int dockIconSize: 40
    property bool searchOpen: false
    property var searchResults: []

    // Written by screenshot.lua: the tool and the last shot.
    property var shot: ({})
    // Written by recents.lua: Alt+Tab's cards for windows drawn bare.
    property var recentsBare: ({})
    // The tray: Solium's own (#363), absent on a Solium built before it.
    readonly property bool hasTray: typeof Tray !== "undefined"

    // Written by bar.lua: which pop-up a bar's button asked for,
    // { what, monitor, x, id }, or empty.
    property var popup: ({})

    readonly property int barHeight: 40
    readonly property color barColor: "#1c1b1d"
    readonly property color hover: "#2b2a2e"
    readonly property color pressed: "#36343a"
    readonly property color active: "#322f3a"
    readonly property color accent: "#936dff"
    readonly property color accentDim: "#625f6b"
    readonly property color text: "#e9e8ec"
    readonly property color textDim: "#9c9aa3"
    readonly property string font: "Noto Sans"


    // While the window switcher (recents.lua), the overview or the workspace
    // stack (deskstack.lua) is up, the
    // pointer is theirs. A script's input grab stops keys and clicks, but
    // hovering and the wheel would still reach the windows shown as cards;
    // over this, they reach nothing but the picker.
    MouseArea {
        anchors.fill: parent
        visible: Solium.status === "recents" || Solium.status === "overview" || Solium.status === "desks"
        hoverEnabled: true
        acceptedButtons: Qt.AllButtons
        onClicked: (mouse) => Solium.send("picker.click", { monitor: Solium.monitor.name, x: mouse.x, y: mouse.y })
        onWheel: (wheel) => Solium.send("picker.wheel", { by: wheel.angleDelta.y < 0 ? 1 : -1 })
    }

    Search {
        open: root.searchOpen
        results: root.searchResults
    }

    Volume {
        id: volume
        anchors.fill: parent
        shell: root
    }

    NotificationCenter {
        id: notifications
        anchors.fill: parent
        shell: root
    }

    TrayMenu {
        id: trayMenu
        anchors.fill: parent
        shell: root
    }

    // The pop-up a bar asked for, on this monitor; and when one closes on its
    // own (a click outside it), the bar is told, so its button lets go.
    onPopupChanged: {
        const here = !!popup && popup.monitor === Solium.monitor.name
        volume.anchorX = here ? popup.x : volume.anchorX
        volume.open = here && popup.what === "volume"
        notifications.open = here && popup.what === "notifications"
        if (here && popup.what === "tray")
            trayMenu.show(popup.id, popup.x)
        else if (trayMenu.itemId >= 0)
            trayMenu.close()
    }
    function closed(what) {
        if (popup && popup.what === what && popup.monitor === Solium.monitor.name)
            Solium.send("bar.popup_closed")
    }
    Connections { target: volume; function onOpenChanged() { if (!volume.open) root.closed("volume") } }
    Connections { target: notifications; function onOpenChanged() { if (!notifications.open) root.closed("notifications") } }
    Connections { target: trayMenu; function onOpenChanged() { if (!trayMenu.open) root.closed("tray") } }

    RecentsFrames {
        anchors.fill: parent
        shell: root
        bare: root.recentsBare
    }

    ShotThumbnail {
        anchors.fill: parent
        shell: root
        shot: root.shot
    }

    // Over everything, the bar included: it draws a picture of it.
    ScreenshotTool {
        anchors.fill: parent
        shell: root
        shot: root.shot
    }
}
