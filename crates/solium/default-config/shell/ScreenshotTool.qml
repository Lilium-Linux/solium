// The screenshot tool (screenshot.lua is its Lua half), after the one in
// lilium-shell: the desktop frozen, everything outside the selection dimmed,
// a region dragged and then moved or resized by its handles, its size read
// out under it, and a toolbar -- region, window, screen, Cancel, Capture.
//
// Every monitor shows its own part of one frozen picture of the whole
// desktop, so a scene's local point is its global point less its monitor's
// origin. A selection is drawn on one monitor; the toolbar and the keyboard
// are the active monitor's.
//
// Nothing is drawn until the picture has loaded: the tool would otherwise
// be in the picture it is taking. Keys: Escape cancels, Enter captures,
// 1 2 3 pick the mode.

import QtQuick
import Solium

Item {
    id: root

    // The bar (Shell.qml), for its colours.
    required property Item shell
    // From screenshot.lua: { serial, phase, frozen, desktop, windows }.
    property var shot: ({})

    readonly property bool selecting: shot && shot.phase === "selecting"
    property string mode: "region"

    // The picture, asked for again until grim has written it.
    property int attempt: 0
    readonly property bool ready: selecting && frozen.status === Image.Ready
    readonly property var desk: (shot && shot.desktop) || ({ x: 0, y: 0, w: 1, h: 1 })
    // Picture pixels per logical pixel.
    readonly property real ratio: frozen.implicitWidth > 0 ? frozen.implicitWidth / desk.w : 1
    // This monitor's origin, in the global space.
    readonly property real ox: Solium.monitor.whole.x
    readonly property real oy: Solium.monitor.whole.y

    // The selection, as four edges in local logical pixels: a handle moves
    // one edge, and dragging one past its opposite just swaps the pair.
    property real selL: 0
    property real selT: 0
    property real selR: 0
    property real selB: 0
    property bool hasSelection: false
    property bool dragging: false
    readonly property real selX: Math.min(selL, selR)
    readonly property real selY: Math.min(selT, selB)
    readonly property real selW: Math.abs(selR - selL)
    readonly property real selH: Math.abs(selB - selT)

    // The window under the pointer, in window mode.
    property rect hoverRect: Qt.rect(0, 0, 0, 0)

    readonly property bool showRect: mode === "window" ? hoverRect.width > 0
                                   : (hasSelection || dragging) && selW > 0 && selH > 0
    readonly property real rectX: mode === "window" ? hoverRect.x : selX
    readonly property real rectY: mode === "window" ? hoverRect.y : selY
    readonly property real rectW: mode === "window" ? hoverRect.width : selW
    readonly property real rectH: mode === "window" ? hoverRect.height : selH
    readonly property bool valid: showRect && rectW >= 8 && rectH >= 8
    readonly property color selectionColour: shell.accent

    visible: ready

    onShotChanged: {
        if (selecting) {
            mode = "region"
            clear()
            attempt = 0
            retry.restart()
        }
    }
    onReadyChanged: if (ready && Solium.monitor.active) keys.forceActiveFocus()
    onModeChanged: { clear(); hoverRect = Qt.rect(0, 0, 0, 0) }

    Timer {
        id: retry
        interval: 100
        repeat: true
        // Five seconds: grim missing or refused, and the tool gives up.
        running: root.selecting && frozen.status !== Image.Ready
        onTriggered: {
            if (++root.attempt > 50) {
                running = false
                Solium.send("shot.cancel")
            }
        }
    }

    function clear() {
        selL = selT = selR = selB = 0
        hasSelection = false
        dragging = false
    }
    function clampX(x) { return Math.max(0, Math.min(width, x)) }
    function clampY(y) { return Math.max(0, Math.min(height, y)) }
    function setRect(x, y, w, h) {
        selL = x; selT = y; selR = x + w; selB = y + h
    }
    function normalise() {
        setRect(selX, selY, selW, selH)
    }
    function applyEdge(edge, x, y) {
        if (edge.indexOf("w") >= 0) selL = clampX(x)
        if (edge.indexOf("e") >= 0) selR = clampX(x)
        if (edge.indexOf("n") >= 0) selT = clampY(y)
        if (edge.indexOf("s") >= 0) selB = clampY(y)
    }

    // The smallest window under a point: a dialog over its parent wins.
    function windowAt(x, y) {
        const list = (shot && shot.windows) || []
        let best = null
        for (let i = 0; i < list.length; i++) {
            const w = list[i]
            const lx = w.x - ox, ly = w.y - oy
            if (x >= lx && x < lx + w.w && y >= ly && y < ly + w.h
                && (!best || w.w * w.h < best.w * best.h))
                best = { x: lx, y: ly, w: w.w, h: w.h }
        }
        return best
    }

    function cancel() {
        Solium.send("shot.cancel")
    }

    // The selection, or else this whole monitor, in the picture's pixels.
    function confirm() {
        const r = valid ? Qt.rect(rectX, rectY, rectW, rectH)
                        : Qt.rect(0, 0, width, height)
        const gx = r.x + ox, gy = r.y + oy
        Solium.send("shot.take", {
            crop: { x: Math.round((gx - desk.x) * ratio), y: Math.round((gy - desk.y) * ratio),
                    w: Math.round(r.width * ratio), h: Math.round(r.height * ratio) },
            rect: { x: gx, y: gy, w: r.width, h: r.height },
        })
    }

    // The keyboard, held while the tool is up on the active monitor.
    Item {
        id: keys
        focus: true
        Solium.keyboard.wants: root.ready && activeFocus
        Solium.keyboard.claims: ["Escape", "Return", "Enter", "1", "2", "3"]
        Keys.onPressed: (event) => {
            switch (event.key) {
            case Qt.Key_Escape: root.cancel(); break
            case Qt.Key_Return:
            case Qt.Key_Enter: root.confirm(); break
            case Qt.Key_1: root.mode = "region"; break
            case Qt.Key_2: root.mode = "window"; break
            case Qt.Key_3: root.mode = "screen"; break
            default: return
            }
            event.accepted = true
        }
    }

    // 1. The frozen desktop: this monitor's part of the picture.
    Item {
        anchors.fill: parent
        clip: true
        Image {
            id: frozen
            x: root.desk.x - root.ox
            y: root.desk.y - root.oy
            width: root.desk.w
            height: root.desk.h
            cache: false
            asynchronous: false
            smooth: true
            source: root.selecting && root.shot.frozen
                    ? "file://" + root.shot.frozen + "?" + root.shot.serial + "-" + root.attempt : ""
        }
    }

    // 2. Where a fresh selection starts; under everything else.
    MouseArea {
        anchors.fill: parent
        hoverEnabled: root.mode === "window"
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        cursorShape: root.mode === "window" ? Qt.PointingHandCursor : Qt.CrossCursor
        onPressed: (event) => {
            keys.forceActiveFocus()
            if (event.button === Qt.RightButton) {
                root.clear()
                return
            }
            if (root.mode !== "region")
                return
            root.selL = root.selR = root.clampX(event.x)
            root.selT = root.selB = root.clampY(event.y)
            root.hasSelection = false
            root.dragging = true
        }
        onPositionChanged: (event) => {
            if (root.mode === "window") {
                const w = root.windowAt(event.x, event.y)
                root.hoverRect = w ? Qt.rect(w.x, w.y, w.w, w.h) : Qt.rect(0, 0, 0, 0)
                return
            }
            if (!root.dragging)
                return
            root.selR = root.clampX(event.x)
            root.selB = root.clampY(event.y)
        }
        onReleased: {
            if (!root.dragging)
                return
            root.dragging = false
            if (root.selW >= 8 && root.selH >= 8) {
                root.normalise()
                root.hasSelection = true
            } else {
                root.clear()
            }
        }
        onClicked: (event) => {
            if (root.mode === "screen" && event.button === Qt.LeftButton) {
                root.confirm()
                return
            }
            if (root.mode !== "window" || event.button !== Qt.LeftButton)
                return
            const w = root.windowAt(event.x, event.y)
            if (!w)
                return
            // A picked window becomes a region, so it can still be nudged.
            root.mode = "region"
            root.setRect(w.x, w.y, w.w, w.h)
            root.hasSelection = true
        }
    }

    // 3. The dim, around the rectangle; the whole monitor without one.
    component Shade: Rectangle {
        color: "#73000000"
        visible: width > 0 && height > 0
    }
    readonly property real dimX: showRect ? rectX : 0
    readonly property real dimY: showRect ? rectY : 0
    readonly property real dimW: showRect ? rectW : 0
    readonly property real dimH: showRect ? rectH : 0
    Shade { x: 0; y: 0; width: root.width; height: Math.max(0, root.dimY) }
    Shade { x: 0; y: root.dimY + root.dimH; width: root.width; height: Math.max(0, root.height - root.dimY - root.dimH) }
    Shade { x: 0; y: root.dimY; width: Math.max(0, root.dimX); height: root.dimH }
    Shade { x: root.dimX + root.dimW; y: root.dimY; width: Math.max(0, root.width - root.dimX - root.dimW); height: root.dimH }

    // In screen mode the whole monitor is the shot: outlined, not dimmed.
    Rectangle {
        anchors.fill: parent
        visible: root.mode === "screen"
        color: "transparent"
        border { width: 3; color: root.selectionColour }
    }

    Rectangle {
        visible: root.showRect
        x: root.rectX - 2; y: root.rectY - 2
        width: root.rectW + 4; height: root.rectH + 4
        radius: 4
        color: "transparent"
        border { width: 2; color: root.selectionColour }
        antialiasing: true
    }

    // Drag the inside to move it.
    MouseArea {
        id: mover
        visible: root.mode === "region" && root.hasSelection
        x: root.selX; y: root.selY
        width: root.selW; height: root.selH
        cursorShape: Qt.SizeAllCursor
        property real pressX: 0
        property real pressY: 0
        property rect from: Qt.rect(0, 0, 0, 0)
        onPressed: (event) => {
            const p = mover.mapToItem(root, event.x, event.y)
            pressX = p.x; pressY = p.y
            from = Qt.rect(root.selX, root.selY, root.selW, root.selH)
        }
        onPositionChanged: (event) => {
            if (!pressed)
                return
            const p = mover.mapToItem(root, event.x, event.y)
            const dx = Math.max(-from.x, Math.min(root.width - from.width - from.x, p.x - pressX))
            const dy = Math.max(-from.y, Math.min(root.height - from.height - from.y, p.y - pressY))
            root.setRect(from.x + dx, from.y + dy, from.width, from.height)
        }
        onDoubleClicked: root.confirm()
    }

    // The eight handles; the dot is small, what takes the press is not.
    Repeater {
        model: [
            { edge: "nw", hx: 0, hy: 0, cursor: Qt.SizeFDiagCursor },
            { edge: "n", hx: 0.5, hy: 0, cursor: Qt.SizeVerCursor },
            { edge: "ne", hx: 1, hy: 0, cursor: Qt.SizeBDiagCursor },
            { edge: "e", hx: 1, hy: 0.5, cursor: Qt.SizeHorCursor },
            { edge: "se", hx: 1, hy: 1, cursor: Qt.SizeFDiagCursor },
            { edge: "s", hx: 0.5, hy: 1, cursor: Qt.SizeVerCursor },
            { edge: "sw", hx: 0, hy: 1, cursor: Qt.SizeBDiagCursor },
            { edge: "w", hx: 0, hy: 0.5, cursor: Qt.SizeHorCursor },
        ]
        delegate: Item {
            id: handle
            required property var modelData
            visible: root.mode === "region" && root.hasSelection && !root.dragging
            width: 20; height: 20
            x: root.selX + modelData.hx * root.selW - 10
            y: root.selY + modelData.hy * root.selH - 10
            Rectangle {
                anchors.centerIn: parent
                width: 9; height: 9
                radius: 4.5
                color: root.selectionColour
                antialiasing: true
                scale: handleArea.containsMouse || handleArea.pressed ? 1.35 : 1
                Behavior on scale { NumberAnimation { duration: 120; easing.type: Easing.OutCubic } }
            }
            MouseArea {
                id: handleArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: handle.modelData.cursor
                // How far the pointer is from the edge it took, so the edge
                // does not jump to it.
                property real offX: 0
                property real offY: 0
                onPressed: (event) => {
                    const p = handleArea.mapToItem(root, event.x, event.y)
                    const edge = handle.modelData.edge
                    offX = edge.indexOf("w") >= 0 ? p.x - root.selL : edge.indexOf("e") >= 0 ? p.x - root.selR : 0
                    offY = edge.indexOf("n") >= 0 ? p.y - root.selT : edge.indexOf("s") >= 0 ? p.y - root.selB : 0
                }
                onPositionChanged: (event) => {
                    // A drag only: with hover on, a plain move would pull the
                    // edge to the pointer.
                    if (!pressed)
                        return
                    const p = handleArea.mapToItem(root, event.x, event.y)
                    root.applyEdge(handle.modelData.edge, p.x - offX, p.y - offY)
                }
                onReleased: root.normalise()
            }
        }
    }

    // The size, in the picture's pixels: under the rectangle, over it near
    // the bottom, inside it when it fills the screen.
    Rectangle {
        id: readout
        visible: root.showRect
        width: readoutText.implicitWidth + 20
        height: readoutText.implicitHeight + 10
        radius: 6
        color: root.shell.barColor
        border { width: 1; color: "#2e2c32" }
        x: Math.max(8, Math.min(root.width - width - 8, root.rectX))
        y: {
            const below = root.rectY + root.rectH + 8
            if (below + height <= root.height - 8)
                return below
            const above = root.rectY - 8 - height
            if (above >= 8)
                return above
            return Math.min(root.height - height - 8, root.rectY + 8)
        }
        Text {
            id: readoutText
            anchors.centerIn: parent
            text: Math.round(root.rectW * root.ratio) + " × " + Math.round(root.rectH * root.ratio)
            color: root.shell.text
            font { pixelSize: 12; family: root.shell.font; features: { "tnum": 1 } }
        }
    }

    component ToolButton: Rectangle {
        id: button
        property string glyph: ""
        property string label: ""
        property bool active: false
        // The one affirmative button, filled with the accent.
        property bool primary: false
        signal activated()
        width: row.implicitWidth + 22
        height: 32
        radius: 8
        color: primary ? (area.pressed ? root.shell.accentDim : root.shell.accent)
             : active ? Qt.rgba(root.shell.accent.r, root.shell.accent.g, root.shell.accent.b, 0.22)
             : area.pressed ? root.shell.pressed
             : area.containsMouse ? root.shell.hover : "transparent"
        scale: area.pressed ? 0.96 : 1
        Behavior on color { ColorAnimation { duration: 120 } }
        Behavior on scale { NumberAnimation { duration: 120; easing.type: Easing.OutCubic } }
        Row {
            id: row
            anchors.centerIn: parent
            spacing: 6
            Image {
                visible: button.glyph !== ""
                anchors.verticalCenter: parent.verticalCenter
                width: 18; height: 18
                source: button.glyph === "" ? ""
                    : "image://solium/icon/" + button.glyph + "?size=" + Math.round(18 * (Solium.monitor.scale || 1))
            }
            Text {
                visible: button.label !== ""
                anchors.verticalCenter: parent.verticalCenter
                text: button.label
                color: button.primary ? "#150f26" : button.active ? root.shell.accent : root.shell.text
                font { pixelSize: 13; family: root.shell.font; weight: Font.Medium }
            }
        }
        MouseArea {
            id: area
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: button.activated()
        }
    }

    // The toolbar, at the bottom; at the top when the selection reaches
    // down to where it would sit.
    Rectangle {
        id: toolbar
        visible: Solium.monitor.active
        readonly property bool atTop: root.showRect && root.rectY + root.rectH > root.height - (height + 60)
        width: tools.implicitWidth + 12
        height: tools.implicitHeight + 12
        x: (root.width - width) / 2
        y: atTop ? 30 : root.height - height - 30
        Behavior on y { NumberAnimation { duration: 240; easing.type: Easing.OutCubic } }
        radius: 12
        color: root.shell.barColor
        border { width: 1; color: "#2e2c32" }

        Row {
            id: tools
            anchors.centerIn: parent
            spacing: 3
            ToolButton { label: "Region"; glyph: "select-rectangular"; active: root.mode === "region"; onActivated: root.mode = "region" }
            ToolButton { label: "Window"; glyph: "window"; active: root.mode === "window"; onActivated: root.mode = "window" }
            ToolButton { label: "Screen"; glyph: "video-display"; active: root.mode === "screen"; onActivated: root.mode = "screen" }
            Rectangle { width: 1; height: 20; anchors.verticalCenter: parent.verticalCenter; color: "#2e2c32" }
            ToolButton { label: "Cancel"; onActivated: root.cancel() }
            ToolButton {
                label: root.valid ? "Capture" : "Capture screen"
                glyph: ""
                primary: true
                onActivated: root.confirm()
            }
        }
    }
}
