// A tray item's menu, drawn in the bar's look (Shell.qml opens it): the
// application's own entries, read live from `Tray`, above the bar over the
// icon that was clicked. A submenu replaces the list, with a row back to where
// it came from; checkmarks and radio entries show their state, and disabled
// ones are dimmed. A click anywhere else closes it, as the volume pop-up does.
//
// Drawn as a layer over the whole scene rather than under the icon: an item
// drawn outside its parent's bounds is seen but never pressed.

import QtQuick
import Solium

Item {
    id: root

    // The bar (Shell.qml), for its colours and height.
    required property Item shell
    // The item whose menu this monitor opened, or -1; and where its icon is.
    property int itemId: -1
    property real anchorX: width - 200

    readonly property bool hasTray: typeof Tray !== "undefined"
    readonly property var row: hasTray && itemId >= 0 ? Tray.get(itemId) : null
    readonly property bool open: !!row && row.present && row.menuOpen
    // The submenus walked into, outermost first.
    property var path: []

    onOpenChanged: if (!open) { itemId = -1; path = [] }

    function show(id, x) {
        path = []
        anchorX = x
        itemId = id
    }
    function close() {
        if (row)
            row.closeMenu()
        itemId = -1
        path = []
    }

    // A `QVariantList` from Rust iterates but is not a JS array.
    function toArray(value) {
        if (Array.isArray(value))
            return value
        const out = []
        if (value)
            for (let i = 0; i < value.length; i++) out.push(value[i])
        return out
    }

    readonly property var shown: {
        if (!open)
            return []
        let level = toArray(row.menu)
        for (const id of path) {
            const opened = level.find(entry => entry.id === id)
            level = opened ? toArray(opened.children) : []
        }
        return level.filter(entry => entry.visible)
    }

    MouseArea {
        anchors.fill: parent
        visible: root.open
        acceptedButtons: Qt.AllButtons
        onPressed: root.close()
    }

    Rectangle {
        id: panel
        visible: root.open
        x: Math.max(8, Math.min(root.width - width - 8, root.anchorX - width / 2))
        anchors { bottom: parent.bottom; bottomMargin: root.shell.barHeight + 8 }
        width: 280
        height: entries.implicitHeight + 12
        radius: 12
        color: root.shell.barColor
        border { width: 1; color: "#2e2c32" }
        MouseArea { anchors.fill: parent }

        Column {
            id: entries
            anchors { left: parent.left; right: parent.right; top: parent.top; margins: 6 }

            // Asked for, not answered yet.
            Text {
                visible: root.shown.length === 0 && root.path.length === 0
                leftPadding: 10
                height: 30
                verticalAlignment: Text.AlignVCenter
                text: "…"
                color: root.shell.textDim
                font { pixelSize: 13; family: root.shell.font }
            }

            // Back out of a submenu.
            Rectangle {
                visible: root.path.length > 0
                width: parent.width
                height: 30
                radius: 6
                color: back.containsMouse ? root.shell.hover : "transparent"
                Text {
                    anchors { left: parent.left; leftMargin: 10; verticalCenter: parent.verticalCenter }
                    text: "‹  Back"
                    color: root.shell.textDim
                    font { pixelSize: 13; family: root.shell.font }
                }
                MouseArea {
                    id: back
                    anchors.fill: parent
                    hoverEnabled: true
                    onClicked: root.path = root.path.slice(0, -1)
                }
            }

            Repeater {
                model: root.shown
                delegate: Item {
                    id: entry
                    required property var modelData
                    readonly property bool separator: modelData.type === "separator"
                    readonly property bool on: modelData.toggleState === 1
                    width: entries.width
                    height: separator ? 9 : 30

                    Rectangle {
                        visible: entry.separator
                        anchors.verticalCenter: parent.verticalCenter
                        x: 10; width: parent.width - 20; height: 1
                        color: "#2e2c32"
                    }
                    Rectangle {
                        visible: !entry.separator
                        anchors.fill: parent
                        radius: 6
                        color: area.pressed && entry.modelData.enabled ? root.shell.pressed
                             : area.containsMouse && entry.modelData.enabled ? root.shell.hover : "transparent"
                    }
                    Row {
                        visible: !entry.separator
                        anchors { left: parent.left; leftMargin: 10; right: parent.right; rightMargin: 10; verticalCenter: parent.verticalCenter }
                        spacing: 8
                        // The toggle, in the accent: ✓ for a checkmark, a dot
                        // for the radio entry that is chosen.
                        Text {
                            width: 14
                            anchors.verticalCenter: parent.verticalCenter
                            text: entry.modelData.toggleType === "checkmark" ? (entry.on ? "✓" : "")
                                : entry.modelData.toggleType === "radio" ? (entry.on ? "●" : "○")
                                : ""
                            color: root.shell.accent
                            font { pixelSize: 13; family: root.shell.font }
                        }
                        Image {
                            width: 16; height: 16
                            anchors.verticalCenter: parent.verticalCenter
                            fillMode: Image.PreserveAspectFit
                            source: entry.modelData.icon || ""
                            visible: source != "" && status === Image.Ready
                        }
                        Text {
                            anchors.verticalCenter: parent.verticalCenter
                            // The application's own words: never markup.
                            text: entry.modelData.label
                            textFormat: Text.PlainText
                            elide: Text.ElideRight
                            width: Math.min(implicitWidth, 220)
                            color: entry.modelData.enabled ? root.shell.text : root.shell.textDim
                            opacity: entry.modelData.enabled ? 1 : 0.6
                            font { pixelSize: 13; family: root.shell.font }
                        }
                    }
                    Text {
                        visible: entry.modelData.submenu === true
                        anchors { right: parent.right; rightMargin: 10; verticalCenter: parent.verticalCenter }
                        text: "›"
                        color: root.shell.textDim
                        font { pixelSize: 15; family: root.shell.font }
                    }
                    MouseArea {
                        id: area
                        visible: !entry.separator
                        anchors.fill: parent
                        hoverEnabled: true
                        onClicked: {
                            if (!entry.modelData.enabled || !root.row)
                                return
                            if (entry.modelData.submenu) {
                                root.row.openSubmenu(entry.modelData.id)
                                root.path = root.path.concat([entry.modelData.id])
                            } else {
                                root.row.clickMenu(entry.modelData.id)
                            }
                        }
                    }
                }
            }
        }
    }
}
