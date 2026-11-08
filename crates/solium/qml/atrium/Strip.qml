// Atrium's strip (#449): what `lua/atrium.lua` cannot draw with a window.
//
// The cards themselves are the real windows, presented by the Lua; this scene
// lies over the strip's column and does the rest: each group's application
// icons over the bottom-left corner of its card, and the pointer. A card is
// picked here, hovered here, and dragged from here onto the stage, and each
// is sent to the Lua as an action. While a stage window covers the strip and
// the strip has slid away, only a thin band along the edge takes the pointer,
// to bring it back; everywhere else the pointer goes to the window under it.
// It also watches a window being dragged towards it, below.
//
// Coordinates are the scene's own: the Lua hands each card's box relative to
// the column's top-left corner, which is the scene's.

import QtQuick
import Solium

Item {
    id: root

    // Written by `atrium.lua`. Each card: `card` (its number, 1 at the top),
    // `key` (its group), `x`, `y`, `w` and `h` (its slot, where it is
    // picked), `ix` and `iy` (the bottom-left corner of its front window as
    // drawn, where its icons go) and `badges`, one an application in the
    // order the applications joined the group: `key`, the id of that
    // application's front window in the group, `icon`, a theme icon name,
    // and, for a window just come off the stage, `fromX` and `fromY`, its
    // bottom-left corner there, where its icon starts from.
    property var cards: []
    // How long the strip stays out after the pointer has left it, in ms:
    // `floating.atrium.linger`.
    property int leaveDelay: 3000
    // The card in the hand, drawn by the Lua where the drag is -- or a window
    // dragged in from the stage, turning into one: `keys` and `icons` (its
    // icons, named as `badges[].key`), `group` (its card's `key`, which a
    // card keeps while the numbers change under it), `ix` and `iy` (its
    // front window's bottom-left corner as drawn, this frame) and `fade` (1 a
    // card, 0 a window); `false` while none is held.
    property var held: false
    // The strip's own depth, its width down the left or its height along the
    // bottom; the scene is deeper, so that a held card's icons can go with it.
    property int strip: 220
    // Which side of the work area the strip is on, "left" or "bottom"
    // (`floating.atrium.side`): where the edge that brings it back is, and
    // which way it slides away.
    property string side: "left"
    readonly property bool alongBottom: side === "bottom"
    // The springs the Lua moves the cards on, `{ stiffness, damping }`, for
    // the icons to move with them: `floating.atrium.motion` and, as the strip
    // slides away, `hide_motion`.
    property var motion: ({ stiffness: 300, damping: 32 })
    property var softMotion: ({ stiffness: 120, damping: 22 })
    // The strip has slid away under a stage window.
    property bool hidden: false
    // It was brought back over the stage window by the pointer.
    property bool peeking: false
    // The card the Lua is lifting, 0 for none.
    property int hovered: 0
    // How large an application icon is drawn, and how far a press must travel
    // to be a drag rather than a click: `floating.atrium.card.icon` and
    // `floating.atrium.drag.threshold`.
    property int iconSize: 48
    property int dragThreshold: 8
    // How much a hovered card grows: `floating.atrium.hover.lift`.
    property real lift: 1.06

    // A list as Lua hands it over: a sequence with a length (not a JS array,
    // to `Array.isArray`), or, empty, an empty object, which is none.
    function listOf(value) {
        return value && typeof value.length === "number" ? value : []
    }
    readonly property var cardList: listOf(root.cards)

    // The card at a point of the scene, 0 for none.
    function cardAt(x, y) {
        for (const each of root.cardList) {
            if (x >= each.x - 12 && x < each.x + each.w + 12
                    && y >= each.y - 12 && y < each.y + each.h + root.iconSize / 2)
                return each.card
        }
        return 0
    }

    // A window dragged towards the strip turns into a card as it comes, and a
    // card dragged out into its windows, both drawn by the Lua where the
    // pointer is. The compositor moves a window and says nothing until it is
    // let go, and this scene's own pointer events arrive unevenly between
    // frames, so while a button is held over this monitor, or a card is held
    // anywhere, by the pointer or a finger, the Lua is asked to draw once a
    // frame, with how long the frame was by the compositor's clock (`clock`
    // below), which a turned window's easing after the pointer and how fast
    // it turns are measured in, and told when the button comes up.
    readonly property bool ticking: (Solium.cursor.pressed && Solium.monitor.pointer) || column.dragging
    property real tickedAt: -1
    onTickingChanged: tickedAt = -1
    FrameAnimation {
        running: root.ticking
        onTriggered: {
            const now = root.clock
            const dt = root.tickedAt < 0 ? -1 : now - root.tickedAt
            root.tickedAt = now
            Solium.send("tick", dt > 0 ? { dt: dt } : {})
        }
    }
    Connections {
        target: Solium.cursor
        function onPressedChanged() {
            if (!Solium.cursor.pressed)
                Solium.send("released")
        }
    }

    // While hidden: the edge brings the strip back. Motion only, so a press
    // there still reaches what is under it.
    Item {
        id: edge
        visible: root.cardList.length > 0 && root.hidden
        x: 0
        y: root.alongBottom ? root.height - 6 : 0
        width: root.alongBottom ? root.width : 3
        height: root.alongBottom ? 6 : root.height
        HoverHandler {
            id: edgeHover
            onHoveredChanged: {
                if (!hovered || root.peeking)
                    dwell.stop()
                else if (root.alongBottom)
                    dwell.restart()
                else
                    Solium.send("edge")
            }
        }
        // Along the bottom the edge lies just above the bar, which the pointer
        // crosses on its way to the taskbar; so there it must rest a moment,
        // where the left edge, which stops the pointer, answers at once.
        Timer {
            id: dwell
            interval: 220
            onTriggered: {
                if (edgeHover.hovered && !root.peeking)
                    Solium.send("edge")
            }
        }
    }

    // The strip goes away again once the pointer has been off it, its edge
    // and its cards, for `leaveDelay` ms: passing between two cards, or from
    // the edge out to them, does not send it away.
    readonly property bool over: edgeHover.hovered || column.containsMouse || column.pressed
    onOverChanged: over ? leaving.stop() : leaving.restart()
    Timer {
        id: leaving
        interval: root.leaveDelay
        onTriggered: {
            if (!root.over)
                Solium.send("leave")
        }
    }

    // While shown: the cards are the strip's, and only the cards. Everywhere
    // else in the column -- between the cards, around them -- a click goes to
    // what is under it: the desktop, a window.
    MouseArea {
        id: column
        visible: root.cardList.length > 0 && (!root.hidden || root.peeking)
        anchors.fill: parent
        hoverEnabled: true
        acceptedButtons: Qt.LeftButton
        containmentMask: QtObject {
            function contains(point: point): bool {
                return column.pressed || root.cardAt(point.x, point.y) > 0
            }
        }

        property int pressedCard: 0
        property point pressedAt: Qt.point(0, 0)
        property bool dragging: false
        property int lastHover: 0

        onPositionChanged: (mouse) => {
            if (pressed && pressedCard > 0) {
                if (!dragging && Math.hypot(mouse.x - pressedAt.x, mouse.y - pressedAt.y) > root.dragThreshold)
                    dragging = true
                if (dragging)
                    Solium.send("drag", { card: pressedCard, x: mouse.x, y: mouse.y,
                                          px: pressedAt.x, py: pressedAt.y })
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
                // Where it was let go decides: the Lua puts it back into its
                // place over the strip, and onto the stage past it.
                Solium.send("drop", { card: card, x: mouse.x, y: mouse.y })
            } else {
                Solium.send("pick", { card: card, join: (mouse.modifiers & Qt.ShiftModifier) !== 0 })
            }
        }
        onExited: {
            if (pressed)
                return
            lastHover = 0
            Solium.send("hover", { card: 0 })
        }
    }

    // Each group's application icons, side by side over the bottom-left
    // corner of its front window, as macOS draws them.
    //
    // One per window rather than one per place in the strip: an icon is named
    // by its application's front window in its group (`badges[].key`), so
    // when that window's card is dragged onto another the same icon goes
    // along, held under the card while it is in the hand and springing from
    // there to its place beside the icons already on the card it joins. The
    // model is kept by those names, so an icon whose card moves or changes is
    // the same item moving, and is not loaded again.
    readonly property int iconGap: 6

    // The held card's icons, by name, and how many.
    // By name, each one's place among them: its place in the hand, which a
    // card's row for it may already say otherwise, once the card it joins
    // has been written and the hand not yet emptied.
    readonly property var heldKeys: {
        const out = {}
        if (root.held) {
            const keys = root.listOf(root.held.keys)
            for (let at = 0; at < keys.length; at++)
                out[String(keys[at])] = at + 1
        }
        return out
    }
    readonly property int heldCount: root.held ? root.listOf(root.held.keys).length : 0
    // Magnetism, by distance alone, so that nothing in it ever jumps: the
    // card whose icons are nearest the held card's, within half the distance
    // between two cards (so never two at once), and how near, 0 at that
    // distance and 1 on top. The held icons are drawn into the places they
    // will have there -- after the card's own, as they join it last -- and,
    // whenever they are level with the card's own, kept to the right of them,
    // so no icon ever passes over another.
    readonly property var near: {
        if (!root.held)
            return null
        const list = root.listOf(root.cards)
        let reach = 150
        for (let at = 1; at < list.length; at++)
            reach = Math.min(reach, Math.abs(list[at].iy - list[at - 1].iy) / 2)
        let best = null
        for (const card of list) {
            if (card.key === root.held.group)
                continue
            const apart = Math.hypot(root.held.ix - card.ix, root.held.iy - card.iy)
            const amount = Math.max(0, Math.min(1, 1 - apart / Math.max(1, reach)))
            if (amount > 0 && (!best || amount > best.amount)) {
                // Its own icons, not counting held ones it already holds once
                // the two are one.
                let count = 0
                for (const badge of root.listOf(card.badges))
                    if (root.heldKeys[String(badge.key)] === undefined)
                        count++
                best = { key: card.key, card: card.card, ix: card.ix, iy: card.iy, amount: amount, count: count }
            }
        }
        return best
    }
    // Where the held icons are drawn to, following the card they are nearest
    // on the cards' own spring rather than jumping with it: that card lifts
    // as it is hovered and moves to a new place when the two become one, and
    // a held icon drawn to it directly would jump with it. Taken as it is when
    // a card first comes near, where the pull is still nothing.
    readonly property real nearSize: root.iconSize * (root.near && root.hovered === root.near.card ? root.lift : 1)
    property real aimX: 0
    property real aimY: 0
    property real aimSize: root.iconSize
    property real aimVx: 0
    property real aimVy: 0
    property real aimVs: 0
    property var aimKey: null
    onNearChanged: {
        if (root.near && root.near.key !== root.aimKey) {
            root.aimKey = root.near.key
            root.aimX = root.near.ix; root.aimY = root.near.iy; root.aimSize = root.nearSize
            root.aimVx = 0; root.aimVy = 0; root.aimVs = 0
        } else if (!root.near) {
            root.aimKey = null
        }
    }
    // The compositor's clock, in seconds, for the icons' springs and the
    // drag's ticks: a plain animation of it, which the clock the cards move
    // on advances, running only while one of those springs is (`springing`
    // counts them) or a drag ticks. FrameAnimation's frameTime counts the
    // wall's time, so under SOLIUM_CLOCK_RATE, or across a stalled frame, the
    // icons left the cards they belong to behind, and a window in the hand
    // eased and turned as many times faster.
    property int springing: 0
    property real clock: 0
    NumberAnimation on clock {
        running: root.springing > 0 || root.ticking
        from: 0
        to: 1000
        duration: 1000000
        loops: Animation.Infinite
    }
    readonly property bool aiming: root.near !== null
    property real aimClock: 0
    onAimingChanged: {
        aimClock = clock
        springing += aiming ? 1 : -1
    }
    onClockChanged: {
        if (!root.aiming)
            return
        const now = root.clock
        let left = Math.min(Math.max(0, now - root.aimClock), 0.05)
        root.aimClock = now
        {
            const k = root.motion.stiffness, c = root.motion.damping
            while (left > 0) {
                const dt = Math.min(left, 0.004)
                root.aimVx += (k * (root.near.ix - root.aimX) - c * root.aimVx) * dt
                root.aimVy += (k * (root.near.iy - root.aimY) - c * root.aimVy) * dt
                root.aimVs += (k * (root.nearSize - root.aimSize) - c * root.aimVs) * dt
                root.aimX += root.aimVx * dt
                root.aimY += root.aimVy * dt
                root.aimSize += root.aimVs * dt
                left -= dt
            }
        }
    }
    readonly property real pull: {
        if (!root.near)
            return 0
        const amount = root.near.amount
        return amount * amount * (3 - 2 * amount)
    }

    ListModel { id: badges }
    // The model brought to what the cards and the hand hold. A row no longer
    // wanted is not removed but marked `leaving`, and goes once its icon has
    // faded out; one wanted again stays. Nothing appears or goes at once.
    function sync() {
        const wanted = []
        const seen = {}
        // `cards` itself: `cardList`, a binding on it, is not yet brought up
        // to date when this runs from its change handler.
        for (const card of root.listOf(root.cards)) {
            const list = root.listOf(card.badges)
            for (let slot = 0; slot < list.length; slot++) {
                const each = list[slot]
                const from = typeof each.fromX === "number"
                wanted.push({ key: String(each.key), icon: String(each.icon || ""), card: card.card,
                              group: Number(card.key), slot: slot, ix: card.ix, iy: card.iy,
                              fromX: from ? each.fromX : 0, fromY: from ? each.fromY : 0,
                              hasFrom: from, leaving: false })
                seen[String(each.key)] = true
            }
        }
        // An icon in the hand that is on no card yet: a window being dragged
        // in from the stage.
        if (root.held) {
            const keys = root.listOf(root.held.keys)
            const icons = root.listOf(root.held.icons)
            for (let slot = 0; slot < keys.length; slot++)
                if (!seen[String(keys[slot])])
                    wanted.push({ key: String(keys[slot]), icon: String(icons[slot] || ""), card: -1,
                                  group: Number(root.held.group), slot: slot, ix: root.held.ix, iy: root.held.iy,
                                  fromX: 0, fromY: 0, hasFrom: false, leaving: false })
        }
        const keys = {}
        for (const each of wanted)
            keys[each.key] = true
        for (let row = 0; row < badges.count; row++)
            if (!keys[badges.get(row).key] && !badges.get(row).leaving)
                badges.setProperty(row, "leaving", true)
        for (const each of wanted) {
            let row = -1
            for (let at = 0; at < badges.count; at++)
                if (badges.get(at).key === each.key) { row = at; break }
            if (row < 0)
                badges.append(each)
            else
                for (const role of ["icon", "card", "group", "slot", "ix", "iy", "leaving"])
                    if (badges.get(row)[role] !== each[role])
                        badges.setProperty(row, role, each[role])
        }
    }
    function forget(key) {
        for (let row = 0; row < badges.count; row++)
            if (badges.get(row).key === key && badges.get(row).leaving) {
                badges.remove(row)
                return
            }
    }
    onCardsChanged: sync()
    onHeldChanged: {
        // Only when what the hand holds changes, not each frame it moves.
        const keys = root.held ? root.listOf(root.held.keys).join(",") : ""
        if (keys !== root.heldShape) {
            root.heldShape = keys
            sync()
        }
    }
    property string heldShape: ""
    Component.onCompleted: sync()

    Repeater {
        model: badges
        delegate: Item {
            id: badge
            required property string key
            required property string icon
            required property int card
            required property real group
            required property int slot
            required property real ix
            required property real iy
            required property real fromX
            required property real fromY
            required property bool hasFrom
            required property bool leaving

            // In the hand only while there is a hand: the two can be told
            // apart in either order as the hand empties, and a held icon read
            // from an emptied hand would be nowhere.
            readonly property bool inHand: root.held !== false && root.heldKeys[badge.key] !== undefined
            readonly property int handSlot: inHand ? root.heldKeys[badge.key] - 1 : 0
            readonly property bool away: root.hidden && !root.peeking
            // The size of the icons on the card the held one is nearest, as
            // they are drawn.
            readonly property real nearSize: root.aimSize
            // The size it is drawn at: lifted with a hovered card, and, in
            // the hand, growing into the size of those it is drawn to.
            readonly property real size: inHand ? root.iconSize + (nearSize - root.iconSize) * root.pull
                : root.iconSize * (root.hovered === badge.card ? root.lift : 1)
            property real shown: size
            Behavior on shown { enabled: !badge.inHand; NumberAnimation { duration: Theme.quick } }

            // Where it belongs. In the hand: under the held card, drawn into
            // its place on the card it is nearest by the magnetism, after that
            // card's own icons, and never over them: level with them, it is
            // kept to their right, and the nearer level the more. Otherwise
            // its card's corner, slid off with the strip when it goes.
            readonly property real tx: {
                if (inHand) {
                    const own = root.held.ix - size * 0.18 + handSlot * (size + root.iconGap)
                    if (!root.near)
                        return own
                    const there = root.aimX - nearSize * 0.18 + (root.near.count + handSlot) * (nearSize + root.iconGap)
                    const drawn = own + (there - own) * root.pull
                    const level = Math.abs(ty - (root.aimY - nearSize * 0.62))
                    const keep = Math.max(0, Math.min(1, (nearSize + 8 - level) / 8))
                    return drawn + Math.max(0, there - drawn) * keep
                }
                return badge.ix - size * 0.18 + slot * (size + root.iconGap)
                    - (away && !root.alongBottom ? root.strip : 0)
            }
            readonly property real ty: {
                if (inHand) {
                    const own = root.held.iy - size * 0.62
                    const there = root.near ? root.aimY - nearSize * 0.62 : own
                    return own + (there - own) * root.pull
                }
                return badge.iy - size * 0.62 + (away && root.alongBottom ? root.strip : 0)
            }
            width: shown
            height: shown
            // Faded in when it first appears, as it travels to its place, and
            // out before it goes.
            property bool born: false
            opacity: !born || leaving || away ? 0 : inHand ? root.held.fade : 1
            Behavior on opacity { enabled: !badge.inHand; NumberAnimation { duration: Theme.normal * 2; easing.type: Easing.OutCubic } }
            scale: leaving ? 0.7 : 1
            Behavior on scale { NumberAnimation { duration: Theme.normal * 2; easing.type: Easing.OutCubic } }
            onOpacityChanged: {
                if (leaving && opacity === 0)
                    root.forget(key)
            }
            // One already out of sight -- the strip away, or faded in the hand
            // -- has nothing to fade, and goes now.
            onLeavingChanged: {
                if (leaving && opacity === 0)
                    root.forget(key)
            }
            Component.onCompleted: {
                // From its window's corner on the stage, when it has just come
                // off it; otherwise where it belongs, fading in there.
                if (hasFrom) {
                    px = fromX - size * 0.18 + slot * (size + root.iconGap)
                    py = fromY - size * 0.62
                    follow()
                }
                born = true
            }

            // On the cards' own spring, worked out here a frame at a time, so
            // an icon stays with its card through every move the Lua springs
            // it through; in the hand, on the card exactly.
            property real px: tx
            property real py: ty
            property real vx: 0
            property real vy: 0
            x: px
            y: py
            function settled() {
                return Math.abs(px - tx) < 0.1 && Math.abs(py - ty) < 0.1
                    && Math.abs(vx) < 1 && Math.abs(vy) < 1
            }
            onTxChanged: follow()
            onTyChanged: follow()
            onInHandChanged: follow()
            function follow() {
                // Never from or to nowhere: a position that is not a number
                // is not taken, and one already taken is put right.
                if (isNaN(tx) || isNaN(ty))
                    return
                if (isNaN(px) || isNaN(py)) {
                    px = tx; py = ty; vx = 0; vy = 0
                }
                if (inHand) {
                    px = tx; py = ty; vx = 0; vy = 0
                } else if (!settled() && !springing) {
                    lastClock = root.clock
                    springing = true
                }
            }
            // On the scene's clock (`root.clock`), a step each time it moves.
            property bool springing: false
            property real lastClock: 0
            onSpringingChanged: root.springing += springing ? 1 : -1
            Component.onDestruction: {
                if (springing)
                    root.springing -= 1
            }
            Connections {
                target: root
                enabled: badge.springing
                function onClockChanged() {
                    if (isNaN(badge.tx) || isNaN(badge.ty))
                        return
                    const now = root.clock
                    let left = Math.min(Math.max(0, now - badge.lastClock), 0.05)
                    badge.lastClock = now
                    const m = badge.away ? root.softMotion : root.motion
                    const k = m.stiffness, c = m.damping
                    while (left > 0) {
                        const dt = Math.min(left, 0.004)
                        badge.vx += (k * (badge.tx - badge.px) - c * badge.vx) * dt
                        badge.vy += (k * (badge.ty - badge.py) - c * badge.vy) * dt
                        badge.px += badge.vx * dt
                        badge.py += badge.vy * dt
                        left -= dt
                    }
                    if (badge.settled()) {
                        badge.px = badge.tx; badge.py = badge.ty; badge.vx = 0; badge.vy = 0
                        badge.springing = false
                    }
                }
            }

            Image {
                anchors.fill: parent
                sourceSize.width: root.iconSize * 2
                sourceSize.height: root.iconSize * 2
                smooth: true
                mipmap: true
                source: badge.icon === "" ? ""
                    : badge.icon.indexOf("://") >= 0 ? badge.icon
                    : badge.icon.charAt(0) === "/" ? "file://" + badge.icon
                    : "image://solium/icon/" + encodeURIComponent(badge.icon) + "?size=" + root.iconSize * 2
            }
        }
    }
}
