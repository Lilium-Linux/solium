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

    // Keep windows off the bar's strip. A resting value, never an animated
    // one (docs/shell-boundary.md, "Room of its own"): islands are 36 tall,
    // 6 above the edge.
    Solium.surface.reserve.bottom: 42

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
}
