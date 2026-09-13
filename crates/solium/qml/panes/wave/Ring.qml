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

    // How smooth each curve is. A sine reads as smooth from about eight points
    // per period; every point past that is another vertex walked every frame.
    readonly property int perPeriod: 10

    // How far the corners are rounded before the wave is added.
    readonly property real corner: 34

    // --- where the wave comes from ---------------------------------------
    // Live levels, one per bar, each -1..1. Empty means nothing is feeding
    // this and the rings fall back to a plain travelling sine, which is the
    // state on a machine with no audio source wired up -- so a style is never
    // a blank window because a helper is not running.
    //
    // This is the seam an audio visualiser plugs into: the shape does not care
    // whether a number came from `Math.sin` or from a spectrum, so wiring one
    // up changes what fills this list and nothing else in the file.
    property var levels: []

    // The wave's height at `t` (0..1) round the loop.
    //
    // `periods` must be a whole number in the fallback, or the sine does not
    // close on itself and there is a visible step where the curve meets its
    // own start.
    function height_at(t, periods, phase) {
        const bars = ring.levels.length;
        if (bars === 0) {
            return Math.sin(t * periods * 2 * Math.PI + phase);
        }
        // Sampled around the loop and wrapped, so the first and last bar are
        // neighbours rather than a cut -- the loop has no ends to be an edge.
        const at = t * bars + phase * bars / (2 * Math.PI);
        const i = Math.floor(at);
        const f = at - i;
        const a = ring.levels[((i % bars) + bars) % bars];
        const b = ring.levels[(((i + 1) % bars) + bars) % bars];
        return a + (b - a) * f;
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

    // --- the rings -------------------------------------------------------
    // Outermost first, so each is painted over by the next and what remains
    // visible is the band between them. The client covers the innermost part,
    // which is why none of them needs a hole cut in it.
    //
    // Three rings in ONE scene, not three layers. A layer exists so the
    // *client* can sit between two things; these all sit behind it, so three
    // scenes would be three textures and three uploads buying nothing. Moving
    // one to `depth: "above"` in Pane.qml is what layering buys, and it is one
    // line when a style wants it.
    readonly property var rings: [
        { reach: 30, swell: 15, periods: 18, duration: 7400, ink: Theme.text },
        { reach: 20, swell: 12, periods: 22, duration: 5600, ink: Theme.accent },
        { reach: 10, swell: 8,  periods: 26, duration: 4300, ink: Theme.edge }
    ]

    Repeater {
        model: ring.rings.length

        Shape {
            id: band
            required property int index
            readonly property var spec: ring.rings[index]

            anchors.fill: parent
            // One curve per ring, so the cost of asking for a drawn edge
            // rather than a stepped one is paid three times, not per point.
            antialiasing: true

            // Each ring travels at its own rate, so the three drift apart
            // instead of moving as one rigid outline.
            property real phase: 0
            NumberAnimation on phase {
                running: ring.focused
                loops: Animation.Infinite
                from: 0
                to: 2 * Math.PI
                duration: band.spec.duration
            }

            ShapePath {
                fillColor: ring.focused ? band.spec.ink : Theme.edgeInactive
                strokeWidth: -1

                PathPolyline {
                    path: {
                        const points = [];
                        const spec = band.spec;
                        const steps = spec.periods * ring.perPeriod;
                        const x = ring.bleedLeft;
                        const y = ring.bleedTop;
                        const w = ring.paneWidth;
                        const h = ring.paneHeight;
                        const r = Math.min(ring.corner, w / 2, h / 2);
                        for (let i = 0; i < steps; ++i) {
                            const t = i / steps;
                            const out = spec.reach + spec.swell
                                * ring.height_at(t, spec.periods, band.phase);
                            points.push(ring.ringPoint(t, x, y, w, h, r, out));
                        }
                        points.push(points[0]);
                        return points;
                    }
                }
            }
        }
    }
}
