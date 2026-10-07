// The running windows of this monitor's current workspace: a chip per
// window, its icon, title or app id, the focused one highlighted. A click
// focuses it, through `Solium.send("windows.focus", ...)`, which the shipped
// `actions.lua` forwards to `sol.act`
// (`state::tests::real_client::reflow_on_close::hosted::windows_focus_from_a_scene_focuses_the_window`).
//
// The icon is the window's `appId` tried directly as a theme icon name,
// which is right as often as a desktop id and its application's icon name
// happen to be the same string (frequent, not guaranteed): there is no join
// to `Solium.Apps` here, which would need the Wayland `app_id` matched to a
// desktop id the way `Dock.qml`'s own module doc already cuts for its
// running-apps-not-pinned row. A name nothing resolves shows no icon at all
// (`Image.status !== Image.Ready`), not a broken-image glyph.

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
            width: inner.implicitWidth + Theme.margin
            color: model.focused ? Theme.control : Theme.surfaceInactive
            border { width: model.urgent ? 1 : 0; color: Theme.warning }

            Behavior on color { ColorAnimation { duration: Theme.quick } }

            Row {
                id: inner
                anchors.centerIn: parent
                spacing: 6

                Image {
                    id: chipIcon
                    anchors.verticalCenter: parent.verticalCenter
                    width: 16
                    height: 16
                    fillMode: Image.PreserveAspectFit
                    asynchronous: true
                    source: "image://solium/icon/" + encodeURIComponent(model.appId) + "?size=16"
                    visible: status === Image.Ready
                }

                Text {
                    id: label
                    anchors.verticalCenter: parent.verticalCenter
                    text: (model.title && model.title.length > 0) ? model.title : model.appId
                    elide: Text.ElideRight
                    width: Math.min(implicitWidth, 240)
                    opacity: model.state === "loading" ? 0.5 : 1
                    color: model.focused ? Theme.text : Theme.textDim
                    font { pixelSize: Theme.fontSize; family: Theme.fontFamily }
                }
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
