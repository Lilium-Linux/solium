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

    // Where to look for them. A plain file, rewritten whole by whatever is
    // producing the numbers -- see cava-feed.sh beside this file.
    //
    // A *file* and not the FIFO cava writes directly, because reading a FIFO
    // means blocking until a writer shows up, and the thread this would block
    // is the one the compositor draws every window on. A file read either
    // finds bytes or does not, and a missing one is the same as silence.
    readonly property string feedPath: "file:///tmp/solium-audio"

    // Polled rather than pushed, because nothing in QML can be woken by a
    // file changing.
    //
    // **This only fires while something is animating**, and that is not a
    // detail: the compositor drains Qt's event queue inside `solium_qml_tick`,
    // which runs on frames it draws -- so a `Timer` in a settled scene never
    // fires at all. The rings' own `NumberAnimation` is what keeps frames
    // coming, and both are bound to `focused`. An unfocused window stops
    // reading the file, which is the behaviour wanted anyway.
    Timer {
        interval: 40
        repeat: true
        running: ring.focused
        onTriggered: ring.readFeed()
    }

    function readFeed() {
        const request = new XMLHttpRequest();
        request.onreadystatechange = function() {
            if (request.readyState !== XMLHttpRequest.DONE) {
                return;
            }
            // No file, no reader, no producer: fall back to the sine rather
            // than to a flat line. A missing helper should look like a style
            // that is not wired up, not like one that is broken.
            const body = request.responseText;
            if (!body) {
                ring.levels = [];
                return;
            }
            const parsed = [];
            for (const field of body.trim().split(/[;,\s]+/)) {
                const value = parseFloat(field);
                if (!isNaN(value)) {
                    // 0..100 in, -0.3..1.2 out, through a square root.
                    //
                    // Not linear, and that is from looking at it: a spectrum
                    // sits low almost all the time -- a bar over 40 is a beat,
                    // not a normal reading -- so a straight 0..100 -> -1..1 map
                    // leaves the border tucked flat against the window and
                    // twitching, which is what the first version did. The root
                    // lifts the quiet half without clipping the loud one.
                    //
                    // The -0.3 floor is silence: slightly inside the resting
                    // radius, so a paused track looks calm rather than gone.
                    const level = Math.max(0, Math.min(1, value / 100));
                    parsed.push(Math.max(-1, Math.min(1.2,
                        -0.3 + 1.5 * Math.sqrt(level))));
                }
            }
            ring.levels = parsed;
        };
        request.open("GET", ring.feedPath);
        request.send();
    }

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
        // **Mirrored**, not wrapped: the spectrum runs bass-to-treble down one
        // half of the loop and back up the other. Two reasons, both from
        // looking at it. A spectrum is bass-heavy, so wrapping it once puts
        // every large bar in one short arc and leaves three quarters of the
        // border flat. And bar 0 next to bar N is a cut -- silence against a
        // beat -- where mirroring closes the loop on itself with no join at
        // all, which is the same reason the sine's period count is whole.
        const turn = ((t + phase / (2 * Math.PI)) % 1 + 1) % 1;
        const fold = turn < 0.5 ? turn * 2 : (1 - turn) * 2;
        const at = fold * (bars - 1);
        const i = Math.min(bars - 2, Math.floor(at));
        const f = at - i;
        return ring.levels[i] + (ring.levels[i + 1] - ring.levels[i]) * f;
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
        { reach: 26, swell: 30, periods: 18, duration: 7400, ink: Theme.text },
        { reach: 17, swell: 22, periods: 22, duration: 5600, ink: Theme.accent },
        { reach: 9,  swell: 15, periods: 26, duration: 4300, ink: Theme.edge }
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
                        // Enough points to resolve whatever is driving it.
                        // The sine needs only `perPeriod` per period; a
                        // spectrum needs several per BAR or the peaks are
                        // averaged away into a smooth ripple, which is what
                        // real music looked like before this line existed.
                        const bars = ring.levels.length;
                        const steps = Math.max(spec.periods * ring.perPeriod,
                                               bars * 6);
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
