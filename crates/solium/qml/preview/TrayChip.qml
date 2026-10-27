// The tray (#363): the icons applications put there, live from `Tray`, each
// one's menu drawn here in the bar's own look. A left click activates an
// item (its menu, for an item that is only a menu), a middle click is its
// secondary action, the wheel scrolls it, and a right click opens its menu
// above the bar. Hidden while no item is there; an item that asks to be
// hidden (`passive`) is.
//
// A menu is walked one level at a time, as a phone's are: a submenu replaces
// the list, with a row back to where it came from. Only the monitor whose
// bar was clicked shows it, as `menuOpen` is one value every monitor's
// instance of this scene reads.

import QtQuick
import Solium

Item {
    id: tray

    visible: Tray.items.count > 0
    width: capsule.width
    height: capsule.height

    // A `QVariantList` crossing from Rust iterates but fails `Array.isArray`
    // (`Search.qml`'s own note): indexing wants a real array.
    function toArray(value) {
        if (Array.isArray(value)) {
            return value
        }
        const out = []
        if (value) {
            for (let i = 0; i < value.length; i++) {
                out.push(value[i])
            }
        }
        return out
    }

    // Where a point of an item is on the screen, for an item that places a
    // window of its own there.
    function onScreen(item, x, y) {
        const at = item.mapToItem(null, x, y)
        return Qt.point(Math.round(at.x + Solium.monitor.whole.x), Math.round(at.y + Solium.monitor.whole.y))
    }

    Capsule {
        id: capsule

        Repeater {
            model: Tray.items

            delegate: Item {
                id: slot

                required property var model
                readonly property var item: Tray.get(model.id)
                // This monitor's bar opened the menu.
                property bool openedHere: false
                // The ids of the submenus walked into, outermost first.
                property var path: []

                visible: model.status !== "passive"
                width: visible ? 18 : 0
                height: 18
                anchors.verticalCenter: parent ? parent.verticalCenter : undefined

                readonly property bool menuOpen: model.menuOpen
                onMenuOpenChanged: if (!menuOpen) {
                    openedHere = false
                    path = []
                }

                Image {
                    anchors.fill: parent
                    fillMode: Image.PreserveAspectFit
                    asynchronous: true
                    source: slot.model.icon
                    sourceSize: Qt.size(36, 36)
                }

                Image {
                    anchors { right: parent.right; bottom: parent.bottom }
                    width: 9
                    height: 9
                    fillMode: Image.PreserveAspectFit
                    source: slot.model.overlayIcon
                    visible: source != ""
                }

                MouseArea {
                    anchors.fill: parent
                    acceptedButtons: Qt.LeftButton | Qt.MiddleButton | Qt.RightButton
                    onClicked: function(mouse) {
                        const at = tray.onScreen(slot, mouse.x, mouse.y)
                        if (mouse.button === Qt.RightButton) {
                            slot.openedHere = true
                            slot.path = []
                            slot.item.openMenu(at.x, at.y)
                        } else if (mouse.button === Qt.MiddleButton) {
                            slot.item.secondaryActivate(at.x, at.y)
                        } else {
                            // An item that is only a menu opens it instead.
                            slot.openedHere = slot.model.itemIsMenu && slot.model.hasMenu
                            slot.path = []
                            slot.item.activate(at.x, at.y)
                        }
                    }
                    onWheel: function(wheel) {
                        if (wheel.angleDelta.y !== 0) {
                            slot.item.scroll(wheel.angleDelta.y, "vertical")
                        } else if (wheel.angleDelta.x !== 0) {
                            slot.item.scroll(wheel.angleDelta.x, "horizontal")
                        }
                    }
                }

                // The entries of the level being shown.
                readonly property var shown: {
                    let level = tray.toArray(slot.model.menu)
                    for (const id of slot.path) {
                        const opened = level.find(entry => entry.id === id)
                        level = opened ? tray.toArray(opened.children) : []
                    }
                    return level.filter(entry => entry.visible)
                }

                Grab {
                    name: "tray-menu"
                    target: menu
                    active: menu.visible
                    onDismissed: slot.item.closeMenu()
                }

                Rectangle {
                    id: menu
                    // In the window's own content, beside the scene's root:
                    // drawn outside the icon's bounds, a child of the icon
                    // would be seen and never pressed.
                    parent: slot.Window.contentItem
                    visible: slot.model.menuOpen && slot.openedHere
                    // Above the bar, its right edge at the icon's: the tray
                    // sits at the bar's right end, so it opens leftwards.
                    readonly property point anchor: visible ? slot.mapToItem(parent, slot.width, 0) : Qt.point(0, 0)
                    x: anchor.x - width
                    y: anchor.y - height - 18
                    width: 260
                    height: entries.implicitHeight + 12
                    radius: 14
                    color: Theme.surface
                    border { width: 1; color: Theme.edge }

                    Column {
                        id: entries
                        anchors { left: parent.left; right: parent.right; top: parent.top; margins: 6 }

                        // Read but not yet answered: say so rather than
                        // show an empty box.
                        Text {
                            visible: slot.shown.length === 0 && slot.path.length === 0
                            leftPadding: 8
                            height: 26
                            verticalAlignment: Text.AlignVCenter
                            text: qsTr("…")
                            color: Theme.textDim
                            font { pixelSize: Theme.fontSize; family: Theme.fontFamily }
                        }

                        // Back out of a submenu.
                        Rectangle {
                            visible: slot.path.length > 0
                            width: parent.width
                            height: 26
                            radius: 8
                            color: backArea.containsMouse ? Theme.control : "transparent"
                            Text {
                                anchors { left: parent.left; leftMargin: 8; verticalCenter: parent.verticalCenter }
                                text: "‹"
                                color: Theme.text
                                font { pixelSize: Theme.fontSize; family: Theme.fontFamily; bold: true }
                            }
                            MouseArea {
                                id: backArea
                                anchors.fill: parent
                                hoverEnabled: true
                                onClicked: slot.path = slot.path.slice(0, -1)
                            }
                        }

                        Repeater {
                            model: slot.shown

                            delegate: Item {
                                id: row
                                required property var modelData
                                readonly property bool separator: modelData.type === "separator"
                                readonly property bool on: modelData.toggleState === 1

                                width: entries.width
                                height: separator ? 9 : 26

                                Rectangle {
                                    visible: row.separator
                                    anchors.verticalCenter: parent.verticalCenter
                                    x: 8
                                    width: parent.width - 16
                                    height: 1
                                    color: Theme.edge
                                }

                                Rectangle {
                                    visible: !row.separator
                                    anchors.fill: parent
                                    radius: 8
                                    color: area.containsMouse && row.modelData.enabled ? Theme.control : "transparent"
                                }

                                Row {
                                    visible: !row.separator
                                    anchors { left: parent.left; leftMargin: 8; verticalCenter: parent.verticalCenter }
                                    spacing: 6

                                    Text {
                                        width: 12
                                        text: row.modelData.toggleType === "checkmark" ? (row.on ? "✓" : "")
                                            : row.modelData.toggleType === "radio" ? (row.on ? "●" : "○")
                                            : ""
                                        color: Theme.text
                                        font { pixelSize: Theme.fontSize; family: Theme.fontFamily }
                                    }
                                    Image {
                                        width: 16
                                        height: 16
                                        anchors.verticalCenter: parent.verticalCenter
                                        fillMode: Image.PreserveAspectFit
                                        source: row.modelData.icon
                                        visible: source != "" && status === Image.Ready
                                    }
                                    Text {
                                        // The application's own words: never markup.
                                        text: row.modelData.label + (row.modelData.submenu ? "  ›" : "")
                                        textFormat: Text.PlainText
                                        elide: Text.ElideRight
                                        width: Math.min(implicitWidth, 260)
                                        color: row.modelData.enabled ? Theme.text : Theme.textDim
                                        font { pixelSize: Theme.fontSize; family: Theme.fontFamily }
                                    }
                                }

                                MouseArea {
                                    id: area
                                    visible: !row.separator
                                    anchors.fill: parent
                                    hoverEnabled: true
                                    onClicked: {
                                        if (!row.modelData.enabled) {
                                            return
                                        }
                                        if (row.modelData.submenu) {
                                            slot.item.openSubmenu(row.modelData.id)
                                            slot.path = slot.path.concat([row.modelData.id])
                                        } else {
                                            slot.item.clickMenu(row.modelData.id)
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
