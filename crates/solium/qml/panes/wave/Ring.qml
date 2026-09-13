// One continuous wavy outline around the whole pane.
//
// Not four strips meeting at the corners -- one closed curve. The wave is a
// function of how far round the perimeter a point sits, so it carries through
// the corners without a seam, and because the number of periods is a whole
// number the curve closes on itself where it started.
//
// The outline is a rounded rectangle offset along its own normal. Walking it by
// ARC LENGTH is what keeps the wave even: stepping x and y separately bunches
// the periods up at the corners and stretches them along the sides.
//
// Filled solid and sitting *behind* the client, so the middle never has to be
// cut out -- the window covers it. That is what a `behind` layer is for.

import QtQuick
import QtQuick.Shapes
import Solium

Item {
    id: ring

    // Set by the compositor. A layer's content owns this contract.
    property string title: ""
    property bool focused: false
    property bool pointerInside: false
    property int contentWidth: 0
    property int contentHeight: 0
    property int paneWidth: 0
    property int paneHeight: 0

    // Where the pane's top-left corner sits inside this canvas: the canvas is
    // the pane grown by the bleed, so without these the ring would be drawn
    // `bleedLeft` to the left of the window it belongs to.
    property int bleedLeft: 0
    property int bleedTop: 0

    // --- the shape -------------------------------------------------------
    // How far the trough of the wave sits outside the pane, and how far the
    // crest rides beyond that. `rest + swell` must stay under the bleed
    // Pane.qml declared, because bleed is a hard clip.
    readonly property real rest: 16
    readonly property real swell: 13
    readonly property real corner: 34

    // Whole periods around the loop, so the curve meets itself. A fraction
    // leaves a step where the last point joins the first.
    readonly property int periods: 22

    // Points per period. A sine reads as smooth from about eight; every point
    // past that is one more vertex to walk on every frame of the animation.
    readonly property int perPeriod: 10

    property real phase: 0
    NumberAnimation on phase {
        running: ring.focused
        loops: Animation.Infinite
        from: 0
        to: 2 * Math.PI
        duration: 6000
    }

    // A point `t` (0..1) of the way round a rounded rectangle by arc length,
    // pushed `out` along the outward normal there.
    function ringPoint(t, x, y, w, h, r, out) {
        const flatX = Math.max(0, w - 2 * r);
        const flatY = Math.max(0, h - 2 * r);
        const arc = Math.PI * r / 2;
        const total = 2 * flatX + 2 * flatY + 4 * arc;
        let d = (((t % 1) + 1) % 1) * total;

        // Each corner is a quarter turn about its own centre, so the normal
        // there is simply the radius direction.
        if (d < flatX) {
            return Qt.point(x + r + d, y - out);
        }
        d -= flatX;
        if (d < arc) {
            const a = (d / arc) * Math.PI / 2;
            return Qt.point(x + w - r + (r + out) * Math.sin(a),
                            y + r - (r + out) * Math.cos(a));
        }
        d -= arc;
        if (d < flatY) {
            return Qt.point(x + w + out, y + r + d);
        }
        d -= flatY;
        if (d < arc) {
            const a = (d / arc) * Math.PI / 2;
            return Qt.point(x + w - r + (r + out) * Math.cos(a),
                            y + h - r + (r + out) * Math.sin(a));
        }
        d -= arc;
        if (d < flatX) {
            return Qt.point(x + w - r - d, y + h + out);
        }
        d -= flatX;
        if (d < arc) {
            const a = (d / arc) * Math.PI / 2;
            return Qt.point(x + r - (r + out) * Math.sin(a),
                            y + h - r + (r + out) * Math.cos(a));
        }
        d -= arc;
        if (d < flatY) {
            return Qt.point(x - out, y + h - r - d);
        }
        d -= flatY;
        const a = (d / arc) * Math.PI / 2;
        return Qt.point(x + r - (r + out) * Math.cos(a),
                        y + r - (r + out) * Math.sin(a));
    }

    Shape {
        anchors.fill: parent
        // Antialiasing on the one curve that has to look drawn rather than
        // stepped; there is a single path here, so it is paid once.
        antialiasing: true

        ShapePath {
            fillColor: ring.focused ? Theme.text : Theme.edgeInactive
            strokeWidth: -1

            PathPolyline {
                path: {
                    const points = [];
                    const steps = ring.periods * ring.perPeriod;
                    const x = ring.bleedLeft;
                    const y = ring.bleedTop;
                    const w = ring.paneWidth;
                    const h = ring.paneHeight;
                    const r = Math.min(ring.corner, w / 2, h / 2);
                    for (let i = 0; i < steps; ++i) {
                        const t = i / steps;
                        const out = ring.rest + ring.swell
                            * Math.sin(t * ring.periods * 2 * Math.PI + ring.phase);
                        points.push(ring.ringPoint(t, x, y, w, h, r, out));
                    }
                    // Closed: the first point again, which the whole-number
                    // period count has already made the same height.
                    points.push(points[0]);
                    return points;
                }
            }
        }
    }
}
