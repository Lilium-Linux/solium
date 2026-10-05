// The preview shell: the compositor's own showcase, hosted as
// `config.shell.scene` by `lua/preview/init.lua` unless the user names their
// own shell or turns it off (`preview = false` in `user.lua`). The first
// piece is the bottom bar; see `docs/ricing.md` for what it shows and what
// comes next.
//
// One instance per monitor, each reading its own as `Solium.monitor`
// (`shell.lua`'s `on = "every-monitor"`, the platform's own fan-out: see
// docs/shell-boundary.md, "Hosting a shell").

import QtQuick
import Solium

Item {
    id: root

    // `lua/preview/dock.lua`'s settings, written here by name
    // (`shell.lua`'s generic `properties` passthrough): `undefined` for
    // `dockPinned` until it does, which is how `Dock.qml` tells "nothing
    // configured, compute the default" from "configured, even to an empty
    // list".
    property var dockPinned
    property string dockVisibility: "autohide"
    property int dockIconSize: 40

    // `lua/preview/search.lua`'s own round trip: `searchOpen` toggles on
    // `super+d` (or `preview.search.key`), and `searchResults` is the ranked
    // rows for whatever was last typed -- see `qml/preview/Search.qml`'s own
    // module doc for why matching happens there rather than here.
    property bool searchOpen: false
    property var searchResults: []

    // Keep windows off the bar's strip. A resting value, never an animated
    // one (docs/shell-boundary.md, "Room of its own"): islands are 36 tall,
    // 6 above the edge.
    Solium.surface.reserve.bottom: 42

    Dock {
        pinned: root.dockPinned
        visibility: root.dockVisibility
        iconSize: root.dockIconSize
    }

    Row {
        id: leading
        anchors { left: root.left; leftMargin: 6; bottom: root.bottom; bottomMargin: 6 }
        spacing: 12

        WorkspacesPager {}
        WindowChips {}
    }

    Row {
        id: trailing
        anchors { right: root.right; rightMargin: 6; bottom: root.bottom; bottomMargin: 6 }
        spacing: 12

        LayoutChip {}
        ClockPill {}
    }

    Search {
        open: root.searchOpen
        results: root.searchResults
    }
}
