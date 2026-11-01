// A window's app icon on its bottom-left corner while Alt+Tab is open
// (~/.config/solium/recents.lua). A pane layer, so it is part of the window's
// own transform: when the window becomes a card it moves and shrinks with it,
// on the same clock, and goes back with it.
//
// A layer is told its window's title, not its app, so recents.lua hands every
// pane the icons by title through `sol.pane_values{ recents = ... }`:
// `{ active, chosen = <title>, icons = { [title] = <icon name> } }`.
//
// Each style that wants it has a one-line Badge.qml naming this, as a layer
// at depth "above" with enough bleed for the badge to hang off the corner.

import QtQuick

Item {
    id: layer

    property string title: ""
    property var values: ({})
    property int bleedLeft: 0
    property int bleedTop: 0
    property int paneWidth: 0
    property int paneHeight: 0

    readonly property var recents: values && values.recents ? values.recents : ({})
    readonly property bool active: !!recents.active
    readonly property bool chosen: active && recents.chosen === layer.title
    readonly property string icon: recents.icons && recents.icons[layer.title] ? recents.icons[layer.title] : ""

    // Drawn at window scale and shrunk with the card, so it is large here.
    readonly property int size: 64

    property bool dormant: !active && badge.opacity === 0

    Rectangle {
        id: badge
        x: layer.bleedLeft - layer.size / 3
        y: layer.bleedTop + layer.paneHeight - layer.size * 2 / 3
        width: layer.size
        height: layer.size
        radius: layer.size / 2
        color: "#1c1b1d"
        border { width: 3; color: layer.chosen ? "#936dff" : "#2e2c32" }
        opacity: layer.active && layer.icon !== "" ? 1 : 0
        Behavior on opacity { NumberAnimation { duration: 180; easing.type: Easing.OutCubic } }

        Image {
            anchors.centerIn: parent
            width: layer.size * 0.62
            height: width
            fillMode: Image.PreserveAspectFit
            asynchronous: true
            smooth: true
            mipmap: true
            source: layer.icon === "" ? ""
                : layer.icon.indexOf("://") >= 0 ? layer.icon
                : "image://solium/icon/" + encodeURIComponent(layer.icon) + "?size=" + layer.size + "&scale=2"
        }
    }
}
