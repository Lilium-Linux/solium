// Desktop icons (04-ui.md §4.9), first version: the desktop folder's files,
// drawn on the `bottom` layer of the primary monitor -- below ordinary
// windows, the same as a real desktop, and below the preview bar's own
// `top` layer (`lua/preview/desktop.lua` declares this surface; `Shell.qml`
// is a different one). `Folder` is read directly, live through inotify
// (`crates/solium/src/folder.rs`), the same way `qml/preview/Dock.qml`
// reads `Apps` directly.
//
// **Cut for this first version** (04-ui.md's own list, plus what "small"
// asked for beyond it): dragging and saved positions, the right-click
// menus, a rubber band, showing the desktop (no primitive makes it a few
// lines of Lua without `reveal.lua`'s own window-presenting pass, which does
// not exist yet), Open With and thumbnails. Arrow-key and Return navigation
// is left out too: nothing yet gives the desktop the keyboard the way
// `super+ctrl+d` would (the "showing the desktop" primitive above), and
// claiming it unconditionally would fight quick search and the dock for
// focus with no primitive deciding between them -- so this is listed as
// missing rather than wired in partially.
//
// **A hidden entry still holds its grid cell** when `showHidden` is false,
// drawn invisible rather than excluded, so the layout needs no positional
// index into `Folder` -- there is none: a Qt Quick model is only walked
// through a `Repeater`/`ListView` delegate, never as a plain indexable array
// from JavaScript (`qml/preview/Search.qml`'s own module doc hits the same
// wall for `Apps` and `Windows`). A compacting layout is a `Later`.
//
// **Shift behaves as Ctrl**, toggling one icon rather than range-selecting:
// the design note asks for both to "add"; a contiguous range needs the grid
// position of the last click kept and compared, which is more than this
// first version's selection model (a plain set of uris) carries.

import QtQuick
import Solium

Item {
    id: root

    // `lua/preview/desktop.lua`'s own settings, written here by name
    // (`sol.surface`'s generic `properties` passthrough).
    property string from: "top-right"
    property int cellWidth: 96
    property int cellHeight: 100
    property int labelLines: 2
    property string openOn: "double"
    property bool showHidden: false

    readonly property int iconSize: 48

    // The work area, in this scene's own coordinates
    // (docs/shell-boundary.md, "Its monitor, live": subtract `whole.x`/`whole.y`).
    readonly property rect area: Qt.rect(
        Solium.monitor.area.x - Solium.monitor.whole.x,
        Solium.monitor.area.y - Solium.monitor.whole.y,
        Solium.monitor.area.width,
        Solium.monitor.area.height)

    // Every selected uri, as the keys of an object -- QML has no `Set`.
    property var selected: ({})
    function isSelected(uri) { return selected[uri] === true }
    function clearSelection() { selected = ({}) }
    function select(uri, add) {
        if (add) {
            var next = Object.assign({}, selected)
            if (next[uri]) {
                delete next[uri]
            } else {
                next[uri] = true
            }
            selected = next
        } else {
            var only = ({})
            only[uri] = true
            selected = only
        }
    }

    // The card asking to trust an untrusted launcher; `""` while none asks.
    property string pendingTrustUri: ""
    property string pendingTrustName: ""

    // Columns fill top-to-bottom; the columns themselves are placed from
    // whichever corner `from` names, so moving it never relabels an
    // existing icon's column.
    readonly property int perColumn: Math.max(1, Math.floor(area.height / cellHeight))

    function cellX(index) {
        var column = Math.floor(index / perColumn)
        return root.from === "top-left"
            ? area.x + column * cellWidth
            : area.x + area.width - (column + 1) * cellWidth
    }
    function cellY(index) {
        return area.y + (index % perColumn) * cellHeight
    }

    // An empty-desktop click clears the selection. Only reached where no
    // window and no icon is drawn over it -- "Clickable only where it takes
    // input" (docs/shell-boundary.md) -- since this is the one item here
    // with no visible content of its own to compete for the same point.
    MouseArea {
        anchors.fill: parent
        onPressed: root.clearSelection()
    }

    Repeater {
        model: Folder

        delegate: Item {
            id: cell
            x: root.cellX(index)
            y: root.cellY(index)
            width: root.cellWidth
            height: root.cellHeight
            visible: model.hidden ? root.showHidden : true

            readonly property bool selectedHere: root.isSelected(model.uri)
            readonly property bool shield: model.isLauncher && !model.trusted

            Column {
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.top: parent.top
                anchors.topMargin: 10
                spacing: 4

                Item {
                    width: root.iconSize
                    height: root.iconSize
                    anchors.horizontalCenter: parent.horizontalCenter

                    Image {
                        anchors.fill: parent
                        asynchronous: true
                        fillMode: Image.PreserveAspectFit
                        source: model.icon.length > 0
                                ? "image://solium/icon/" + encodeURIComponent(model.icon) + "?size=" + root.iconSize
                                : ""
                    }

                    // The shield badge: an untrusted launcher, until
                    // `folder.trust` (04-ui.md §4.9's own layout sketch).
                    Rectangle {
                        visible: cell.shield
                        width: 14
                        height: 14
                        radius: 3
                        color: Theme.surface
                        border.color: Theme.edge
                        anchors.right: parent.right
                        anchors.bottom: parent.bottom
                        Text {
                            anchors.centerIn: parent
                            text: "!"
                            font.pixelSize: 10
                            font.bold: true
                            color: Theme.text
                        }
                    }
                }

                // Selected: the label sits on a pill and shows in full.
                Rectangle {
                    visible: cell.selectedHere
                    anchors.horizontalCenter: parent.horizontalCenter
                    width: selectedLabel.implicitWidth + 8
                    height: selectedLabel.implicitHeight + 4
                    radius: 3
                    color: Theme.control

                    Text {
                        id: selectedLabel
                        anchors.centerIn: parent
                        text: model.displayName
                        color: Theme.text
                        font.family: Theme.fontFamily
                        font.pixelSize: Theme.fontSize
                        horizontalAlignment: Text.AlignHCenter
                        width: root.cellWidth - 8
                        wrapMode: Text.Wrap
                        maximumLineCount: root.labelLines
                        elide: Text.ElideRight
                        style: Text.Raised
                        styleColor: "#000000"
                    }
                }

                // At rest: the same label, unpilled and possibly elided --
                // a second `Text` rather than one that moves, so selecting
                // an icon never reflows its neighbours.
                Text {
                    visible: !cell.selectedHere
                    anchors.horizontalCenter: parent.horizontalCenter
                    text: model.displayName
                    color: Theme.text
                    font.family: Theme.fontFamily
                    font.pixelSize: Theme.fontSize
                    horizontalAlignment: Text.AlignHCenter
                    width: root.cellWidth - 8
                    wrapMode: Text.Wrap
                    maximumLineCount: root.labelLines
                    elide: Text.ElideRight
                    style: Text.Raised
                    styleColor: "#000000"
                }
            }

            function open() {
                if (model.isDir) {
                    return // no folder window in this version
                }
                if (cell.shield) {
                    root.pendingTrustUri = model.uri
                    root.pendingTrustName = model.displayName
                    return
                }
                Solium.send("folder.open", { uri: model.uri })
            }

            MouseArea {
                anchors.fill: parent
                enabled: cell.visible
                onPressed: function(mouse) {
                    var add = (mouse.modifiers & (Qt.ControlModifier | Qt.ShiftModifier)) !== 0
                    root.select(model.uri, add)
                }
                onClicked: {
                    if (root.openOn === "single") {
                        cell.open()
                    }
                }
                onDoubleClicked: {
                    if (root.openOn !== "single") {
                        cell.open()
                    }
                }
            }
        }
    }

    // The trust card (04-ui.md §4.9): a small modal over the icons, asking
    // once before an untrusted launcher's first run.
    Rectangle {
        visible: root.pendingTrustUri.length > 0
        anchors.centerIn: parent
        width: 280
        height: 112
        radius: 6
        color: Theme.surface
        border.color: Theme.edge
        // Takes every point of its own rect, so nothing between its buttons
        // falls through to an icon behind it.
        Solium.input: true

        Column {
            anchors.fill: parent
            anchors.margins: 14
            spacing: 12

            Text {
                width: parent.width
                wrapMode: Text.Wrap
                color: Theme.text
                font.family: Theme.fontFamily
                font.pixelSize: Theme.fontSize
                text: "Allow launching " + root.pendingTrustName + "?"
            }

            Row {
                anchors.right: parent.right
                spacing: 8

                Rectangle {
                    width: 90
                    height: 28
                    radius: 4
                    color: Theme.control
                    Text {
                        anchors.centerIn: parent
                        text: "Cancel"
                        color: Theme.accentInk
                        font.family: Theme.fontFamily
                        font.pixelSize: Theme.fontSize
                    }
                    MouseArea {
                        anchors.fill: parent
                        onClicked: root.pendingTrustUri = ""
                    }
                }
                Rectangle {
                    width: 128
                    height: 28
                    radius: 4
                    color: Theme.accent
                    Text {
                        anchors.centerIn: parent
                        text: "Allow and open"
                        color: Theme.accentInk
                        font.family: Theme.fontFamily
                        font.pixelSize: Theme.fontSize
                    }
                    MouseArea {
                        anchors.fill: parent
                        onClicked: {
                            Solium.send("folder.trust", { uri: root.pendingTrustUri })
                            Solium.send("folder.open", { uri: root.pendingTrustUri })
                            root.pendingTrustUri = ""
                        }
                    }
                }
            }
        }
    }
}
