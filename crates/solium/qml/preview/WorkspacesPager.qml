// The workspaces of this monitor: page-indicator dots, the active one an
// accent pill, an occupied one filled. A click switches, answered by the
// shipped `workspaces.lua`'s override of `workspaces.go`
// (`script::tests::a_workspaces_go_from_a_scene_switches_the_monitor_it_names`).

import QtQuick
import Solium

Capsule {
    id: pager

    WorkspaceList {
        id: workspaces
        monitor: Solium.monitor.name
    }

    Repeater {
        model: workspaces

        delegate: Item {
            id: dot
            // The active one is a pill, 20 wide; the rest are 8, dots.
            width: model.active ? 20 : 8
            height: 28

            Rectangle {
                anchors.centerIn: parent
                width: parent.width
                height: 8
                radius: 4
                color: model.active
                       ? Theme.accent
                       : (model.occupied > 0 ? Theme.control : "transparent")
                border.width: model.active ? 0 : 1
                border.color: model.urgent ? Theme.warning : Theme.edge

                Behavior on width { NumberAnimation { duration: Theme.quick } }
                Behavior on color { ColorAnimation { duration: Theme.quick } }
            }

            MouseArea {
                anchors.fill: parent
                onClicked: Solium.send("workspaces.go", { id: model.id, monitor: Solium.monitor.name })
            }
        }
    }
}
