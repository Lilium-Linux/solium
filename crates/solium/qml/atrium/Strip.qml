// Atrium's strip (#449): what `lua/atrium.lua` cannot draw with a window.
//
// The cards themselves are the real windows, presented by the Lua; this scene
// lies over the strip's column and does the rest: each group's application
// icons over the bottom-left corner of its card, and the pointer. A card is
// picked here, hovered here, and dragged from here onto the stage, and each
// is sent to the Lua as an action. While a stage window covers the strip and
// the strip has slid away, only a thin band along the edge takes the pointer,
// to bring it back; everywhere else the pointer goes to the window under it.
//
// Coordinates are the scene's own: the Lua hands each card's box relative to
// the column's top-left corner, which is the scene's.

import QtQuick
import Solium

Item {
    id: root

    // Written by `atrium.lua`. Each card: `card` (its number, 1 at the top),
    // `key` (its group, for telling a card that moved from one that changed),
    // `x`, `y`, `w` and `h`, and `icons`, theme icon names.
    property var cards: []
    // The strip has slid away under a stage window.
    property bool hidden: false
    // It was brought back over the stage window by the pointer.
    property bool peeking: false
    // The card the Lua is lifting, 0 for none.
    property int hovered: 0
    // How large an application icon is drawn, and how far a press must travel
    // to be a drag rather than a click: `floating.atrium.card.icon` and
    // `floating.atrium.drag`.
    property int iconSize: 44
    property int dragThreshold: 8
    // How much a hovered card grows: `floating.atrium.hover.lift`.
    property real lift: 1.06

    // The card at a point of the scene, 0 for none.
    function cardAt(x, y) {
        for (const each of root.cards) {
            if (x >= each.x - 12 && x < each.x + each.w + 12
                    && y >= each.y - 12 && y < each.y + each.h + root.iconSize / 2)
                return each.card
        }
        return 0
    }

    // While hidden: the edge brings the strip back.
    MouseArea {
        visible: root.hidden && !root.peeking
        x: 0
        y: 0
        width: 3
        height: root.height
        hoverEnabled: true
        onEntered: Solium.send("edge")
    }

    // While shown: the column is the strip's.
    MouseArea {
        id: column
        visible: !root.hidden || root.peeking
        anchors.fill: parent
        hoverEnabled: true
        acceptedButtons: Qt.LeftButton

        property int pressedCard: 0
        property point pressedAt: Qt.point(0, 0)
        property bool dragging: false
        property int lastHover: 0

        onPositionChanged: (mouse) => {
            if (pressed && pressedCard > 0) {
                if (!dragging && Math.hypot(mouse.x - pressedAt.x, mouse.y - pressedAt.y) > root.dragThreshold)
                    dragging = true
                if (dragging)
                    Solium.send("drag", { card: pressedCard, x: mouse.x, y: mouse.y })
                return
            }
            const card = root.cardAt(mouse.x, mouse.y)
            if (card !== lastHover) {
                lastHover = card
                Solium.send("hover", { card: card })
            }
        }
        onPressed: (mouse) => {
            pressedCard = root.cardAt(mouse.x, mouse.y)
            pressedAt = Qt.point(mouse.x, mouse.y)
            dragging = false
        }
        onReleased: (mouse) => {
            const card = pressedCard
            const dragged = dragging
            pressedCard = 0
            dragging = false
            if (card === 0)
                return
            if (dragged) {
                // Let go past the strip, over the stage: the group joins it.
                // Anywhere else, the card goes back.
                Solium.send(mouse.x >= root.width ? "pick" : "drop", { card: card, join: true })
            } else {
                Solium.send("pick", { card: card, join: (mouse.modifiers & Qt.ShiftModifier) !== 0 })
            }
        }
        onExited: {
            if (pressed)
                return
            lastHover = 0
            Solium.send("leave")
        }
    }

    // Each group's application icons over the bottom-left corner of its card.
    Repeater {
        model: root.cards
        delegate: Row {
            id: badge
            required property var modelData
            spacing: -root.iconSize / 5
            x: modelData.x - root.iconSize / 4 - (root.hidden && !root.peeking ? root.width : 0)
            y: modelData.y + modelData.h - root.iconSize * 0.6
            opacity: root.hidden && !root.peeking ? 0 : 1
            scale: root.hovered === modelData.card ? root.lift : 1
            transformOrigin: Item.BottomLeft

            Behavior on x { SpringAnimation { spring: 4; damping: 0.4; epsilon: 0.25 } }
            Behavior on y { SpringAnimation { spring: 4; damping: 0.4; epsilon: 0.25 } }
            Behavior on opacity { NumberAnimation { duration: Theme.normal } }
            Behavior on scale { NumberAnimation { duration: Theme.quick } }

            // A different group in this place: its icons fade in rather than
            // changing under a card still on its way.
            property string key: String(modelData.key)
            onKeyChanged: arrive.restart()
            NumberAnimation {
                id: arrive
                target: badge
                property: "opacity"
                from: 0
                to: root.hidden && !root.peeking ? 0 : 1
                duration: Theme.normal * 2
            }

            Repeater {
                model: badge.modelData.icons
                delegate: Image {
                    required property string modelData
                    width: root.iconSize
                    height: root.iconSize
                    sourceSize.width: root.iconSize * 2
                    sourceSize.height: root.iconSize * 2
                    smooth: true
                    mipmap: true
                    source: modelData === "" ? ""
                        : modelData.indexOf("://") >= 0 ? modelData
                        : modelData.charAt(0) === "/" ? "file://" + modelData
                        : "image://solium/icon/" + encodeURIComponent(modelData) + "?size=" + root.iconSize * 2
                }
            }
        }
    }
}
