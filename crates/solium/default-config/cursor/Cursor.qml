// The pointer, in the desktop's violet (cursor.scene in user.lua).
//
// A dark arrow with a violet edge; an I-beam over text; a ring with a dot for
// links; a double arrow for every resize, turned to its direction; a four-way
// arrow to move; a spinning arc beside the arrow while something is busy; a
// cross; a struck-out ring. Moving it fast lights a soft violet glow, so it
// is easy to find, and a press tucks it in a little.
//
// Every shape is one outline of the same number of points, so a change of
// shape is a morph: the outline it had flows into the next one's, point by
// point, while the colours cross over and the small details (a link's dot,
// the busy arc, the strike) grow in or fade out. The point the pointer is at
// never moves: the arrow's tip is where every other shape is centred, so a
// morph happens around the click point rather than jumping across it.
//
// The glow is a MultiEffect, which draws on the GPU path the session uses on
// the hardware; a nested (software) session draws everything but the glow.

import QtQuick
import QtQuick.Effects
import QtQuick.Shapes
import Solium

Item {
    id: root

    readonly property color ink: "#1c1b1d"
    readonly property color edge: "#936dff"
    readonly property color light: "#e9e8ec"
    readonly property color danger: "#e5534b"

    readonly property int size: Solium.cursor.size
    readonly property real u: size / 24              // drawn on a 24-unit grid
    // Room around the point on every side: the arrow reaches about 20 units
    // right and down from its tip, and the scene stays within 64 pixels at
    // the usual 24, so it can still go on the hardware cursor plane.
    readonly property real half: size + 8
    readonly property string shape: Solium.cursor.shape

    readonly property bool isText: shape === "text" || shape === "vertical-text"
    readonly property bool isLink: shape === "pointer" || shape === "grab" || shape === "grabbing"
    readonly property bool isBusy: shape === "wait" || shape === "progress"
    readonly property bool isCross: shape === "crosshair" || shape === "cell"
    readonly property bool isNo: shape === "not-allowed" || shape === "no-drop"
    readonly property bool isMove: shape === "move" || shape === "all-scroll"
    // Which way a resize arrow points, in degrees, or -1 for none.
    function angleOf(s) {
        switch (s) {
        case "ew-resize": case "e-resize": case "w-resize": case "col-resize": return 0
        case "ns-resize": case "n-resize": case "s-resize": case "row-resize": return 90
        case "nwse-resize": case "nw-resize": case "se-resize": return 45
        case "nesw-resize": case "ne-resize": case "sw-resize": return -45
        default: return -1
        }
    }
    readonly property real resizeAngle: angleOf(shape)

    width: 2 * half
    height: 2 * half
    Solium.cursor.hotspot: Qt.point(half, half)

    // ---- the outlines, in grid units around the point ------------------

    // A polygon as [x, y] pairs; `cx, cy` is where the point is on it.
    function shift(poly, cx, cy) {
        return poly.map(p => [p[0] - cx, p[1] - cy])
    }
    function turn(poly, degrees) {
        const a = degrees * Math.PI / 180, c = Math.cos(a), s = Math.sin(a)
        return poly.map(p => [p[0] * c - p[1] * s, p[0] * s + p[1] * c])
    }
    function circle(r, n) {
        const out = []
        for (let i = 0; i < n; i++) {
            const a = -Math.PI / 2 + 2 * Math.PI * i / n
            out.push([r * Math.cos(a), r * Math.sin(a)])
        }
        return out
    }
    readonly property var arrow: shift([[1.2, 1.2], [1.2, 17.6], [5.6, 13.6], [8.6, 20.2],
                                        [11.6, 18.8], [8.7, 12.4], [14.4, 12.0]], 1.2, 1.2)
    readonly property var beam: [[-4, -9], [4, -9], [4, -6.6], [1.4, -6.6], [1.4, 6.6], [4, 6.6],
                                 [4, 9], [-4, 9], [-4, 6.6], [-1.4, 6.6], [-1.4, -6.6], [-4, -6.6]]
    readonly property var twoWay: shift([[1.5, 12], [7.0, 6.5], [7.0, 10.2], [17.0, 10.2], [17.0, 6.5],
                                         [22.5, 12], [17.0, 17.5], [17.0, 13.8], [7.0, 13.8], [7.0, 17.5]], 12, 12)
    readonly property var plus: [[-1.2, -10], [1.2, -10], [1.2, -1.2], [10, -1.2], [10, 1.2], [1.2, 1.2],
                                 [1.2, 10], [-1.2, 10], [-1.2, 1.2], [-10, 1.2], [-10, -1.2], [-1.2, -1.2]]
    readonly property var fourWay: {
        // A double arrow's heads on all four sides, joined at the middle.
        const h = 1.8, w = 5.2, t = 10.5, r = 6.5
        return [[0, -t], [w, -r], [h, -r], [h, -h], [r, -h], [r, -w], [t, 0], [r, w], [r, h], [h, h],
                [h, r], [w, r], [0, t], [-w, r], [-h, r], [-h, h], [-r, h], [-r, w], [-t, 0], [-r, -w],
                [-r, -h], [-h, -h], [-h, -r], [-w, -r]]
    }

    // The outline a shape draws, before it is evened out. Worked out from
    // the name it is handed, not from the properties above: in the handler
    // that asks, those may still say the shape before.
    function polygonOf(s) {
        if (s === "text") return beam
        if (s === "vertical-text") return turn(beam, 90)
        if (s === "pointer" || s === "grab" || s === "grabbing") return circle(7.5, 32)
        if (s === "not-allowed" || s === "no-drop") return circle(9, 32)
        if (s === "crosshair" || s === "cell") return plus
        if (s === "move" || s === "all-scroll") return fourWay
        const angle = angleOf(s)
        if (angle >= 0) return turn(twoWay, angle)
        return arrow
    }

    // ---- evening an outline out to the same points ---------------------

    readonly property int points: 72

    // Clockwise, `points` of them at equal steps round the perimeter, so
    // every outline is exact and has as many points as the next one.
    function evened(poly) {
        let area = 0
        for (let i = 0; i < poly.length; i++) {
            const a = poly[i], b = poly[(i + 1) % poly.length]
            area += a[0] * b[1] - b[0] * a[1]
        }
        if (area < 0) poly = poly.slice().reverse()
        const lengths = [0]
        for (let i = 0; i < poly.length; i++) {
            const a = poly[i], b = poly[(i + 1) % poly.length]
            lengths.push(lengths[i] + Math.hypot(b[0] - a[0], b[1] - a[1]))
        }
        const total = lengths[poly.length]
        const out = []
        let seg = 0
        for (let k = 0; k < points; k++) {
            const d = total * k / points
            while (lengths[seg + 1] < d) seg++
            const a = poly[seg], b = poly[(seg + 1) % poly.length]
            const f = (d - lengths[seg]) / Math.max(1e-6, lengths[seg + 1] - lengths[seg])
            out.push([a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f])
        }
        return out
    }

    // `next`'s points turned round so its k-th is the one nearest, over the
    // whole outline, to `now`'s k-th: the shift with the least travel, so
    // each part of one shape flows into the nearest part of the next and
    // the outline never crosses itself on the way.
    function aligned(now, next) {
        let best = 0, bestCost = Infinity
        for (let shift = 0; shift < points; shift++) {
            let cost = 0
            for (let k = 0; k < points; k += 2) {
                const a = now[k], b = next[(k + shift) % points]
                cost += (a[0] - b[0]) * (a[0] - b[0]) + (a[1] - b[1]) * (a[1] - b[1])
            }
            if (cost < bestCost) { bestCost = cost; best = shift }
        }
        const out = []
        for (let k = 0; k < points; k++) out.push(next[(k + best) % points])
        return out
    }

    // ---- the morph -------------------------------------------------------

    property var fromPoints: evened(arrow)
    property var toPoints: evened(arrow)
    property real t: 1
    readonly property var drawn: {
        const out = []
        for (let k = 0; k < points; k++) {
            const a = fromPoints[k], b = toPoints[k]
            out.push(Qt.point(half + (a[0] + (b[0] - a[0]) * t) * u,
                              half + (a[1] + (b[1] - a[1]) * t) * u))
        }
        out.push(out[0])
        return out
    }

    NumberAnimation {
        id: morph
        target: root
        property: "t"
        from: 0; to: 1
        duration: 170
        easing.type: Easing.OutCubic
    }

    onShapeChanged: {
        // From wherever the outline is now, a morph under way included.
        const now = []
        for (let k = 0; k < points; k++) {
            const a = fromPoints[k], b = toPoints[k]
            now.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t])
        }
        fromPoints = now
        toPoints = aligned(now, evened(polygonOf(shape)))
        morph.restart()
    }

    // How it is filled and edged, crossing over with the outline.
    readonly property color fill: isText ? light : isNo ? "transparent"
                                : shape === "grabbing" ? edge : isCross ? edge : ink
    readonly property color stroke: isText || isCross ? ink : isNo ? danger : edge
    readonly property real strokeUnits: isNo ? 2.6 : isText ? 1.0 : 1.5

    // ---- drawing -----------------------------------------------------------

    // How fast it is moving, 0..1, eased back down when it slows: 0 at an
    // ordinary pace, rising from `brisk` pixels a second to full at `fast`.
    // In tenths, so a pointer moving at an even speed changes nothing here:
    // the velocity changes with every motion event, and a glow that followed
    // it exactly re-drew the pointer, its outline and its blur on every one,
    // a quarter of a frame at 260 Hz, while the glow was too faint to see.
    readonly property real brisk: 900
    readonly property real fast: 2500
    readonly property real speed: {
        const v = Solium.cursor.velocity
        const s = v ? Math.hypot(v.x || 0, v.y || 0) : 0
        const share = Math.max(0, Math.min(1, (s - brisk) / (fast - brisk)))
        return Math.round(share * 10) / 10
    }
    property real glow: speed
    Behavior on glow { NumberAnimation { duration: 260; easing.type: Easing.OutCubic } }

    MultiEffect {
        source: drawing
        anchors.fill: drawing
        visible: root.glow > 0.01
        shadowEnabled: true
        shadowColor: root.edge
        shadowBlur: 1.0
        shadowOpacity: 0.85 * root.glow
        shadowScale: 1.0 + 0.15 * root.glow
    }

    Item {
        id: drawing
        anchors.fill: parent
        // Pressed, it tucks in toward the point.
        transformOrigin: Item.Center
        scale: Solium.cursor.pressed ? 0.88 : 1.0
        Behavior on scale { NumberAnimation { duration: 90; easing.type: Easing.OutCubic } }

        Shape {
            anchors.fill: parent
            preferredRendererType: Shape.CurveRenderer
            ShapePath {
                fillColor: outline.fill
                strokeColor: outline.stroke
                strokeWidth: outline.width * root.u
                joinStyle: ShapePath.RoundJoin
                PathPolyline { path: root.drawn }
            }
        }

        // The colours and the edge's width cross over as the outline flows.
        QtObject {
            id: outline
            property color fill: root.fill
            property color stroke: root.stroke
            property real width: root.strokeUnits
            Behavior on fill { ColorAnimation { duration: 170; easing.type: Easing.OutCubic } }
            Behavior on stroke { ColorAnimation { duration: 170; easing.type: Easing.OutCubic } }
            Behavior on width { NumberAnimation { duration: 170; easing.type: Easing.OutCubic } }
        }

        // A link's dot, dark while grabbing.
        Rectangle {
            x: root.half - width / 2; y: root.half - height / 2
            width: 5 * root.u; height: width; radius: width / 2
            color: root.shape === "grabbing" ? root.ink : root.light
            opacity: root.isLink ? 1 : 0
            scale: root.isLink ? 1 : 0.2
            Behavior on opacity { NumberAnimation { duration: 170; easing.type: Easing.OutCubic } }
            Behavior on scale { NumberAnimation { duration: 170; easing.type: Easing.OutBack } }
            Behavior on color { ColorAnimation { duration: 170 } }
        }

        // Not allowed: the strike across the ring.
        Rectangle {
            x: root.half - width / 2; y: root.half - height / 2
            width: 17 * root.u; height: 2.6 * root.u
            radius: height / 2
            rotation: -45
            color: root.danger
            opacity: root.isNo ? 1 : 0
            scale: root.isNo ? 1 : 0.3
            Behavior on opacity { NumberAnimation { duration: 170; easing.type: Easing.OutCubic } }
            Behavior on scale { NumberAnimation { duration: 170; easing.type: Easing.OutCubic } }
        }

        // Busy: an arc turning beside the arrow's tip, grown in.
        Item {
            x: root.half + 11 * root.u; y: root.half + 11 * root.u
            width: 11 * root.u; height: 11 * root.u
            opacity: root.isBusy ? 1 : 0
            scale: root.isBusy ? 1 : 0.2
            visible: opacity > 0
            Behavior on opacity { NumberAnimation { duration: 170; easing.type: Easing.OutCubic } }
            Behavior on scale { NumberAnimation { duration: 170; easing.type: Easing.OutBack } }
            Rectangle {
                anchors.fill: parent
                radius: width / 2
                color: root.ink
            }
            Shape {
                anchors.fill: parent
                preferredRendererType: Shape.CurveRenderer
                RotationAnimation on rotation {
                    running: root.isBusy
                    loops: Animation.Infinite
                    from: 0; to: 360; duration: 900
                }
                ShapePath {
                    fillColor: "transparent"
                    strokeColor: root.edge
                    strokeWidth: 1.8 * root.u
                    capStyle: ShapePath.RoundCap
                    PathAngleArc {
                        centerX: 5.5 * root.u; centerY: 5.5 * root.u
                        radiusX: 3.6 * root.u; radiusY: 3.6 * root.u
                        startAngle: 0; sweepAngle: 250
                    }
                }
            }
        }
    }
}
