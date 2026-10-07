// One dock cell: a pinned app, or a running one that is not pinned. Both are
// the same cell (04-ui.md §4.6's layout table), told apart only by whether
// `pinned` is true and whether anything of this app is running.

import QtQuick
import Solium

Item {
    id: cell

    // Not `required`: a `Repeater` over `dock.effectivePinned` built every
    // cell with `modelData` undefined when this was, measured directly
    // (solium-notes' own shots README, under this same date, says where).
    property string appId: ""
    property int iconSize: 40
    property bool pinned: false

    // A ghost before `Apps.ready` (03 §3.2.13: never null, `valid` false for
    // an id nothing installed answers) -- a pin keeps its cell even then.
    readonly property var entry: Apps.get(appId)
    readonly property bool installed: !Apps.ready || entry.valid

    // Every window of this app, most recently focused first -- what a click
    // on a running icon focuses, and what the dot under it means.
    WindowList {
        id: windows
        app: cell.appId
        monitor: Solium.monitor.name
        sort: "mru"
    }
    readonly property bool running: windows.count > 0
    // The id `WindowList`'s own `sort: "mru"` puts first: no index-based read
    // exists on it (it is a plain filtered model, read the usual Qt Quick way,
    // through a delegate), so a one-row `Repeater` reads it declaratively --
    // the same "a row is nameless, and this is how a scene joins one model's
    // row to another's" idea `docs/shell-boundary.md` already asks for.
    property int mostRecentWindowId: -1
    Repeater {
        model: windows
        delegate: Item {
            Binding {
                target: cell
                property: "mostRecentWindowId"
                value: model.id
                when: index === 0
            }
        }
    }

    // Pinned always shows; a running app that is not pinned shows only while
    // it is running, and disappears the moment its last window closes.
    visible: pinned || running
    width: visible ? iconSize + 12 : 0
    height: 52
    Behavior on width { NumberAnimation { duration: Theme.quick } }

    Column {
        anchors.centerIn: parent
        spacing: 4

        Image {
            id: img
            anchors.horizontalCenter: parent.horizontalCenter
            width: cell.iconSize
            height: cell.iconSize
            fillMode: Image.PreserveAspectFit
            asynchronous: true
            // `entry.icon` names a theme icon (or, cut for this version, an
            // absolute path -- `icon.rs`'s module doc says why that is
            // refused rather than trusted); falling back to the app id
            // itself is a common-enough convention (`firefox`'s own icon
            // really is named `firefox`) to be worth trying before the
            // provider's own fallback glyph.
            readonly property string iconName: entry.icon.length > 0 ? entry.icon : cell.appId
            source: iconName.length > 0
                    ? "image://solium/icon/" + encodeURIComponent(iconName) + "?size=" + cell.iconSize
                    : ""
            opacity: cell.installed ? 1 : 0.4
            Behavior on opacity { NumberAnimation { duration: Theme.quick } }
        }

        Rectangle {
            anchors.horizontalCenter: parent.horizontalCenter
            width: 5
            height: 5
            radius: 2.5
            visible: cell.running
            color: Theme.accent
        }
    }

    MouseArea {
        anchors.fill: parent
        enabled: cell.installed
        onClicked: {
            if (cell.running) {
                Solium.send("windows.focus", { id: cell.mostRecentWindowId })
            } else {
                // `from` (04-ui.md: the icon's own rectangle, for the window
                // to grow out of) is cut for this version along with the rest
                // of the launch animation join -- `apps.rs`'s module doc.
                Solium.send("apps.launch", { id: cell.appId })
            }
        }
    }
}
