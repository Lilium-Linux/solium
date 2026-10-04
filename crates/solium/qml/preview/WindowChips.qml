// The running windows of this monitor's current workspace: a chip per
// window, title or app id, the focused one highlighted. A click focuses it,
// through `Solium.send("windows.focus", ...)`, which the shipped
// `actions.lua` forwards to `sol.act`
// (`state::tests::real_client::reflow_on_close::hosted::windows_focus_from_a_scene_focuses_the_window`).
//
// No icons: there is no `image://` provider for the icon theme yet
// (docs/shell-boundary.md, "What it is not given"), so a chip is text only.

import QtQuick
import Solium

Capsule {
    id: chips

    WindowList {
        id: windows
        monitor: Solium.monitor.name
        workspace: Workspaces.showing(Solium.monitor.name).id
    }

    Repeater {
        model: windows

        delegate: Rectangle {
            id: chip
            height: 28
            radius: 6
            width: label.width + Theme.margin
            color: model.focused ? Theme.control : Theme.surfaceInactive
            border { width: model.urgent ? 1 : 0; color: Theme.warning }

            Behavior on color { ColorAnimation { duration: Theme.quick } }

            Text {
                id: label
                anchors.centerIn: parent
                text: (model.title && model.title.length > 0) ? model.title : model.appId
                elide: Text.ElideRight
                width: Math.min(implicitWidth, 240)
                opacity: model.state === "loading" ? 0.5 : 1
                color: model.focused ? Theme.text : Theme.textDim
                font { pixelSize: Theme.fontSize; family: Theme.fontFamily }
            }

            MouseArea {
                anchors.fill: parent
                onClicked: Solium.send("windows.focus", { id: model.id })
            }
        }
    }

    // An empty desk is not a missing bar: say so rather than show nothing
    // and read as broken (04.1's "nothing is faked" rule, in spirit).
    Text {
        visible: windows.count === 0
        text: qsTr("no windows")
        color: Theme.textDim
        font { pixelSize: Theme.fontSize; family: Theme.fontFamily }
    }
}
