// The top dock (04-ui.md §4.6, core slice): a floating plate, pinned apps
// then a hairline then running ones that are not pinned, autohidden by
// default. Configured from `preview.dock` in `user.lua`, through
// `lua/preview/dock.lua` and `shell.lua`'s generic `properties` passthrough
// -- see `dock.lua`'s module doc for why the default pin list is computed
// here, in QML, rather than there, in Lua.
//
// **Cut for this first version** (04-ui.md's own Showcase/P1 marks, plus
// what "small" asked for beyond them): magnification, the window-preview and
// app-card popovers, the dock's own right-click menu, dragging to reorder or
// unpin, the notch split, `"pointer"` and `"intellihide"` visibility, the
// keyboard path (`super+b`, arrow keys), `super+alt+N`, a name tooltip, and
// the "no app installed at all" placeholder cell. What is here: `"always"`
// and `"autohide"`, the hover-edge reveal, pinned and running-not-pinned
// cells, the running dot, launch-or-focus on click.

import QtQuick
import Solium

Item {
    id: dock

    // `undefined` until `preview.dock.pinned` names a list (`dock.lua`):
    // then the default below, computed from what is actually installed,
    // applies instead.
    property var pinned
    property string visibility: "autohide"
    property int iconSize: 40

    readonly property bool alwaysOn: visibility === "always"
    property bool revealed: alwaysOn

    // One role's candidates, most to least common -- `lua/preview/dock.lua`
    // keeps the same list for its own doc comment's sake; the two cannot
    // share one copy (QML cannot read a Lua table at scene load).
    readonly property var candidateRoles: [
        ["org.gnome.Console", "org.kde.konsole", "kitty", "foot", "alacritty", "gnome-terminal"],
        ["org.kde.dolphin", "org.gnome.Nautilus", "nautilus", "pcmanfm", "Thunar", "nemo"],
        ["org.mozilla.firefox", "firefox", "org.chromium.Chromium", "chromium-browser", "chromium"],
    ]

    function computeDefaultPins() {
        if (!Apps.ready) {
            return []
        }
        var out = []
        for (var r = 0; r < candidateRoles.length; r++) {
            var role = candidateRoles[r]
            for (var c = 0; c < role.length; c++) {
                if (Apps.get(role[c]).valid) {
                    out.push(role[c])
                    break
                }
            }
        }
        return out
    }

    property var defaultPins: computeDefaultPins()
    Connections {
        target: Apps
        function onReadyChanged() { dock.defaultPins = dock.computeDefaultPins() }
    }

    // `pinned` (`root.dockPinned`) crosses from Lua through the C ABI as a
    // `QVariantList`, not a native JS one: it iterates fine (`.length`,
    // `[i]`, `JSON.stringify`) but fails `Array.isArray`, measured directly
    // with a capture (solium-notes' own shots README). `DockIcon.appId` not
    // being `required` any more is what actually fixed the `Repeater` below
    // building its cells with no `modelData` -- a `QVariantList`'s own
    // oddness was a real difference worth normalising away anyway, since
    // `effectivePinned.indexOf(...)` further down wants a genuine array.
    function toArray(value) {
        if (Array.isArray(value)) {
            return value;
        }
        const out = [];
        if (value) {
            for (let i = 0; i < value.length; i++) {
                out.push(value[i]);
            }
        }
        return out;
    }

    readonly property var effectivePinned: pinned !== undefined ? toArray(pinned) : defaultPins

    anchors { top: parent.top; horizontalCenter: parent.horizontalCenter }
    width: parent.width
    height: 58

    // Only while `"always"`: autohide reserves nothing, and reveals over
    // whatever is there instead (04-ui.md's own visibility table).
    Solium.surface.reserve.top: alwaysOn ? height : 0

    // The edge strip: a bare `HoverHandler` takes the pointer's motion and
    // leaves every press to what is under it
    // (docs/shell-boundary.md, "Clickable only where it takes input"), which
    // is how this reveals the dock without swallowing a click meant for a
    // window beneath it. 04-ui.md's own 250 ms dwell and 2 px width are cut
    // for this version: a hover anywhere along the strip reveals at once.
    Item {
        anchors { top: parent.top; left: parent.left; right: parent.right }
        height: 4
        visible: !dock.alwaysOn
        HoverHandler {
            onHoveredChanged: if (hovered) { hideTimer.stop(); dock.revealed = true }
        }
    }

    Timer {
        id: hideTimer
        interval: 600
        onTriggered: dock.revealed = false
    }

    Rectangle {
        id: plate
        anchors.horizontalCenter: parent.horizontalCenter
        y: (dock.revealed || dock.alwaysOn) ? 6 : -height - 4
        Behavior on y { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }

        height: 52
        radius: 24
        color: Theme.surface
        border { width: 1; color: Theme.edge }
        width: Math.max(row.implicitWidth + Theme.margin * 2, 64)

        HoverHandler {
            onHoveredChanged: {
                if (hovered) {
                    hideTimer.stop()
                    dock.revealed = true
                } else if (!dock.alwaysOn) {
                    hideTimer.restart()
                }
            }
        }

        Row {
            id: row
            anchors.centerIn: parent
            spacing: Theme.gap

            Repeater {
                model: dock.effectivePinned
                delegate: DockIcon {
                    appId: modelData
                    iconSize: dock.iconSize
                    pinned: true
                }
            }

            Rectangle {
                // The hairline between pinned and running-not-pinned
                // (04-ui.md's own `|`). `notPinned.count` is every installed
                // app not pinned (near the whole index), not how many of
                // them are actually running, so this shows whenever there
                // are pins rather than only when something follows the
                // line -- a small, harmless overcount next to the real one
                // (`Dock.qml`'s file doc on why nothing here can cheaply
                // tell the difference without a running-apps index of its
                // own).
                visible: dock.effectivePinned.length > 0 && notPinned.count > 0
                width: 1
                height: 32
                anchors.verticalCenter: parent.verticalCenter
                color: Theme.edge
            }

            // Every installed app not already pinned, shown only while it is
            // running (`DockIcon`'s own `visible: pinned || running`). This
            // walks the whole index rather than only the open windows
            // because nothing here de-duplicates `Windows` by `appId`
            // without one (no API gap this piece should fix on its own --
            // see the file doc); P1 is an index of running apps instead of a
            // per-app check on every installed one.
            Repeater {
                id: notPinned
                model: Apps
                delegate: Loader {
                    active: dock.effectivePinned.indexOf(model.id) === -1
                    // `Row` skips a child by its own `visible`, not by its
                    // size, and a `Loader` stays visible by default even
                    // while its item is not: without this, every installed,
                    // not-running app would still cost the plate a
                    // `Theme.gap` of empty space, one per app in the index.
                    visible: item !== null && item.visible
                    sourceComponent: DockIcon {
                        appId: model.id
                        iconSize: dock.iconSize
                        pinned: false
                    }
                }
            }
        }
    }
}
