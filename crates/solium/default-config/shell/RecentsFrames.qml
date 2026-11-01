// Alt+Tab's app badge for windows drawn bare (recents.lua).
//
// A framed window carries its own: the badge is a layer of its pane style
// (qml/accent/RecentsBadge.qml), so it moves and shrinks with the card. A
// window that draws its own titlebar -- Claude, a browser -- has no frame and
// so no layers. recents.lua hands over where those cards are, and their badge
// is drawn here instead, on the monitor the row is on; their border is an
// effect (looks.lua), part of the window, so it needs nothing here.
//
// Kept by window id rather than rebuilt, so stepping slides each one with its
// card instead of jumping; they fade in as the cards arrive, and as the row
// closes they go home with their windows, fading on the way.

import QtQuick
import Solium

Item {
    id: root

    // The bar (Shell.qml), for its colours.
    required property Item shell
    // From recents.lua: { monitor, cards: [{ id, x, y, w, h, opacity, chosen,
    // icon, scale }] }, global logical coordinates.
    property var bare: ({})

    readonly property bool here: !!bare && bare.monitor === Solium.monitor.name
    readonly property var list: here && bare.cards ? bare.cards : []

    Solium.input: false

    ListModel { id: model }

    onListChanged: sync()

    function sync() {
        const seen = {}
        const ox = Solium.monitor.whole.x, oy = Solium.monitor.whole.y
        for (let i = 0; i < list.length; i++) {
            const c = list[i]
            seen[c.id] = true
            const row = {
                wid: c.id, cx: c.x - ox, cy: c.y - oy, cw: c.w, ch: c.h,
                fade: c.opacity, chosen: !!c.chosen, icon: c.icon || "",
                scale: c.scale || 1, shown: !c.leaving,
            }
            let at = -1
            for (let j = 0; j < model.count; j++) {
                if (model.get(j).wid === c.id) { at = j; break }
            }
            if (at >= 0) model.set(at, row)
            else model.append(row)
        }
        // Gone from the row: faded out, then dropped.
        for (let j = 0; j < model.count; j++) {
            if (!seen[model.get(j).wid]) model.setProperty(j, "shown", false)
        }
        drop.restart()
    }

    Timer {
        id: drop
        interval: 260
        onTriggered: {
            for (let j = model.count - 1; j >= 0; j--) {
                if (!model.get(j).shown) model.remove(j)
            }
        }
    }

    Repeater {
        model: model
        delegate: Item {
            id: card
            required property real cx
            required property real cy
            required property real cw
            required property real ch
            required property real fade
            required property bool chosen
            required property string icon
            required property real scale
            required property bool shown

            x: cx; y: cy; width: cw; height: ch
            opacity: shown ? fade : 0
            Behavior on x { NumberAnimation { duration: 220; easing.type: Easing.OutCubic } }
            Behavior on y { NumberAnimation { duration: 220; easing.type: Easing.OutCubic } }
            Behavior on width { NumberAnimation { duration: 220; easing.type: Easing.OutCubic } }
            Behavior on height { NumberAnimation { duration: 220; easing.type: Easing.OutCubic } }
            Behavior on opacity { NumberAnimation { duration: 220; easing.type: Easing.OutCubic } }

            // The app's icon on the bottom-left corner, as RecentsBadge.qml
            // draws it on a frame: 64 at the window's scale.
            Rectangle {
                readonly property real size: 64 * card.scale
                visible: card.icon !== ""
                x: -size / 3
                y: card.height - size * 2 / 3
                width: size; height: size
                radius: size / 2
                color: root.shell.barColor
                border { width: Math.max(1.5, 3 * card.scale); color: card.chosen ? root.shell.accent : "#2e2c32" }
                Image {
                    anchors.centerIn: parent
                    width: parent.size * 0.62
                    height: width
                    fillMode: Image.PreserveAspectFit
                    asynchronous: true
                    smooth: true
                    mipmap: true
                    source: card.icon === "" ? ""
                        : card.icon.indexOf("://") >= 0 ? card.icon
                        : "image://solium/icon/" + encodeURIComponent(card.icon) + "?size=64&scale=2"
                }
            }
        }
    }
}
