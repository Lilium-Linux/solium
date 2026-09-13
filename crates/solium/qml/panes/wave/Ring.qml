// One continuous wavy outline around the whole pane, shaped by the music.
//
// Not four strips meeting at the corners -- one closed curve per ring. The
// offset is a function of how far round the PERIMETER a point sits, so the
// wave carries through the corners without a seam, and the loop closes on
// itself where it started.
//
// The outline is a rounded rectangle pushed out along its own normal. Walking
// it by ARC LENGTH is what keeps the wave even: stepping x and y separately
// bunches detail up at the corners and stretches it along the sides.
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

    // How far the corners are rounded before the wave is added.
    readonly property real corner: 34

    // Points per period for the fallback sine. A spectrum asks for its own
    // count -- see the path below.
    readonly property int perPeriod: 10

    // How many times the spectrum sweeps round the window. See `levelAt`.
    readonly property int sweeps: 3

    // --- where the shape comes from ---------------------------------------
    // Live levels, one per bar, each 0..1 -- 0 is silence and rests at the
    // ring's own radius, 1 is a peak at full swell. Empty means nothing is
    // feeding this and the rings fall back to a travelling sine, so a machine
    // with no audio helper running gets a decoration rather than a flat line.
    property var levels: []

    // Where to look for them. A plain file, rewritten whole by whatever is
    // producing the numbers -- see cava-feed.sh beside this file.
    //
    // A *file* and not the FIFO cava writes directly, because reading a FIFO
    // means blocking until a writer shows up, and the thread that would block
    // is the one the compositor draws every window on.
    readonly property string feedPath: "file:///tmp/solium-audio"

    // --- the heartbeat ----------------------------------------------------
    // **This is not what makes the waves move.** With audio feeding them the
    // shape comes from the spectrum alone and this is never read -- see the
    // ternary in the path, which only evaluates `phase` on the fallback
    // branch, so with levels present it is not even a binding dependency and
    // the geometry is rebuilt when the music changes rather than every frame.
    //
    // It runs anyway, for two reasons. It is what the sine falls back *to*
    // when no helper is running. And a `Timer` in a settled scene never fires
    // at all: the compositor drains Qt's event queue inside `solium_qml_tick`,
    // which only runs on a frame it draws, so without something animating the
    // poll below would stop and the audio with it.
    property real phase: 0
    NumberAnimation on phase {
        running: ring.focused
        loops: Animation.Infinite
        from: 0
        to: 2 * Math.PI
        duration: 7000
    }

    // --- how hard to draw -------------------------------------------------
    // Every point of every ring is rebuilt whenever the pane's size changes,
    // and during a drag-resize that is every frame -- three paths, each a few
    // hundred `Qt.point`s with trigonometry behind them, against a 3.8ms
    // budget on a 260Hz screen. At rest the size never changes and the
    // geometry is rebuilt only when the music does, which is 25 times a
    // second; a resize asks for it two hundred times a second instead.
    //
    // So a resize gets a coarser curve. The detail is not visible while the
    // window is moving under the pointer, and the moment it settles the full
    // count comes back.
    property int lastWidth: 0
    property int lastHeight: 0
    property int settling: 0
    readonly property bool resizing: ring.settling > 0

    Timer {
        interval: 40
        repeat: true
        running: ring.focused
        onTriggered: {
            // Sampled here rather than watched with a signal handler, because
            // the compositor writes width and height separately: a handler on
            // each would see two changes per frame and this sees the pair.
            if (ring.paneWidth !== ring.lastWidth || ring.paneHeight !== ring.lastHeight) {
                ring.lastWidth = ring.paneWidth;
                ring.lastHeight = ring.paneHeight;
                // Five ticks of quiet -- 200ms -- before calling it settled,
                // so a drag that pauses mid-way does not flicker back to full
                // detail and down again.
                ring.settling = 5;
            } else if (ring.settling > 0) {
                ring.settling -= 1;
            }
            ring.readFeed();
        }
    }

    function readFeed() {
        const request = new XMLHttpRequest();
        request.onreadystatechange = function() {
            if (request.readyState !== XMLHttpRequest.DONE) {
                return;
            }
            // No file, no producer: fall back to the sine rather than to a
            // flat line. A missing helper should look like a style nobody
            // wired up, not like one that is broken.
            const body = request.responseText;
            if (!body) {
                ring.levels = [];
                return;
            }
            const previous = ring.levels;
            const fields = body.trim().split(/[;,\s]+/);
            const raw = [];
            for (let i = 0; i < fields.length; ++i) {
                const value = parseFloat(fields[i]);
                if (!isNaN(value)) {
                    raw.push(Math.max(0, Math.min(1, value / 100)));
                }
            }

            // Smoothed across neighbouring bars before anything else reads
            // them. cava's bars are independent buckets and a border drawn
            // straight off them is visibly serrated -- each bucket is a corner
            // whatever the interpolation between them does.
            //
            // Three taps and not five. Five blurred a peak across so many bars
            // that the border stopped showing the music and started showing
            // its envelope -- smooth, but the same shape whatever was playing.
            // This is the least blur that still takes the teeth off.
            const weights = [0, 1, 0];
            const smoothed = [];
            for (let i = 0; i < raw.length; ++i) {
                let sum = 0;
                let total = 0;
                for (let k = -1; k <= 1; ++k) {
                    const j = i + k;
                    if (j < 0 || j >= raw.length) {
                        continue;
                    }
                    const weight = weights[k + 1];
                    sum += raw[j] * weight;
                    total += weight;
                }
                smoothed.push(total > 0 ? sum / total : 0);
            }

            const parsed = [];
            for (let i = 0; i < smoothed.length; ++i) {
                let level = smoothed[i];
                // Barely eased. A little of the last reading takes the
                // hard flicker off cava's bars; any more and the border shows
                // the envelope of the music rather than the music, which is
                // what "too smooth to see it visualising" was.
                if (previous.length === smoothed.length && !isNaN(previous[i])) {
                    level = previous[i] * 0.18 + level * 0.82;
                }
                parsed.push(level);
            }
            ring.levels = parsed;
        };
        request.open("GET", ring.feedPath);
        request.send();
    }

    // The fallback: a plain travelling sine. `periods` is whole so the curve
    // closes on itself; a fraction leaves a step where it meets its own start.
    function sineAt(t, periods, phase) {
        return Math.sin(t * periods * 2 * Math.PI + phase);
    }

    // Where the top edge's midpoint falls in the arc-length walk.
    //
    // The walk starts at the end of the top-left corner, so `t = 0` is an
    // arbitrary place on the top edge and folding the spectrum there put the
    // mirror line off to one side. Folding HERE makes the two halves of the
    // border reflections of each other about the window's vertical centre,
    // and -- because a rounded rectangle is symmetric -- half a perimeter on
    // from the top centre is exactly the bottom centre, so the other fold
    // lands right as well.
    readonly property real topCentre: {
        const r = Math.min(ring.corner, ring.paneWidth / 2, ring.paneHeight / 2);
        const flatX = Math.max(0, ring.paneWidth - 2 * r);
        const flatY = Math.max(0, ring.paneHeight - 2 * r);
        const arc = Math.PI * r / 2;
        const total = 2 * flatX + 2 * flatY + 4 * arc;
        return total > 0 ? (flatX / 2) / total : 0;
    }

    // One ring's slice of the spectrum, at `t` (0..1) round the loop.
    //
    // `lo`..`hi` are fractions of the bar list, so each ring shows a different
    // part of the sound: the outer one the bass, the middle the mids, the
    // inner one the treble. Three rings reading the same bars moved as one
    // thick outline -- separating them is what makes the layering mean
    // something rather than just look like a thicker border.
    //
    // **Mirrored about the top centre**, not wrapped. A spectrum is
    // bass-heavy, so wrapping it once round would put every large bar in one
    // short arc; and bar 0 against bar N is a cut, silence next to a beat,
    // where mirroring closes the loop with no join at all. It is also what
    // makes the border symmetric left to right.
    //
    // **Smoothstepped between bars, not interpolated straight.** A straight
    // line between two readings is a straight line on screen and the joins are
    // corners -- exactly the creasing that shows up as lines in the curve.
    // `f * f * (3 - 2f)` leaves the slope at zero on each reading, so
    // neighbouring segments meet without a kink.
    function levelAt(t, lo, hi) {
        const bars = ring.levels.length;
        if (bars < 2) {
            return 0;
        }
        const first = Math.max(0, Math.min(bars - 2, Math.floor(bars * lo)));
        const last = Math.max(first + 1, Math.min(bars - 1, Math.floor(bars * hi)));
        const count = last - first + 1;

        // Swept `sweeps` times round rather than once, out and back each
        // time. Once round sounds right and is not: any one edge of the window
        // then shows only its own share of the loop, and for the top edge that
        // was the first 29% of the band -- five adjacent bars, which are
        // usually close in value, so the edge people actually look at was the
        // flattest part of the whole border. Measured there before this: a 2
        // to 5 PIXEL wave. Sweeping three times puts most of the band along
        // every edge.
        //
        // `2 * |u - round(u)|` is a triangle wave, and an EVEN one -- which is
        // what keeps the border symmetric left to right, since `turn` is
        // measured from the top centre and the two sides see equal and
        // opposite values.
        const turn = (t - ring.topCentre);
        const u = turn * ring.sweeps;
        const fold = 2 * Math.abs(u - Math.round(u));
        const at = fold * (count - 1);
        const step = Math.min(count - 2, Math.floor(at));
        const f = at - step;
        // Straight between bars, not smoothstepped. Smoothstep leaves the
        // slope at zero on every reading, which rounds each bar into a swell
        // -- soft, and hard to read as a spectrum. A straight line gives each
        // bar a point, which is what a spike is.
        const a = ring.levels[first + step];
        const b = ring.levels[first + step + 1];
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
    // Outermost first, so each is painted over by the next and what stays
    // visible is the band between them. The client covers the innermost part,
    // which is why none of them needs a hole cut in it.
    //
    // Three rings in ONE scene, not three layers. A layer exists so the
    // *client* can sit between two things; these all sit behind it, so three
    // scenes would be three textures and three uploads buying nothing. Moving
    // one to `depth: "above"` in Pane.qml is what layering buys.
    // `lo`/`hi` are each ring's slice of the spectrum: bass outermost, where
    // there is the most room, treble innermost against the window's edge.
    //
    // **The reaches are chosen so the three can never cross.** Each ring is
    // filled from its own curve inward and the inner ones are painted last, so
    // a ring that swells past the one outside it does not overlap it -- it
    // erases it. With the bands free to reach any radius that happened
    // constantly, and the result was one muddled outline whose colour depended
    // on which frequency was loudest. Held apart, the widest each can get
    // (`reach + swell * 1.35`) still clears the next one's resting radius, so
    // there are always three readable rings and each is visibly its own band.
    //
    // The three bands, innermost first. `swell` is how far a full-scale
    // reading in that band pushes the edge out.
    readonly property var bands: [
        { lo: 0.00, hi: 0.30, swell: 18 },   // bass
        { lo: 0.28, hi: 0.62, swell: 13 },   // mids
        { lo: 0.60, hi: 1.00, swell: 9 }     // treble
    ]

    // **They STACK rather than sit at their own radii, and that is what stops
    // them erasing each other.** Each ring is filled from its curve inward and
    // the later ones paint over, so a ring that swells past the one outside it
    // does not overlap it -- it wipes it out. At fixed radii that happened
    // whenever one band got loud and another did not, and the border turned a
    // single colour. Summed, ring k's edge is always at least ring k-1's, so
    // the order on screen is fixed however the music moves.
    //
    // What you see is each band's own thickness: the dark ring against the
    // window is the bass, the blue band on top of it the mids, the grey
    // outside that the treble.
    //
    // **Reach is zero, so silence draws nothing.** With every band quiet the
    // outermost curve is the pane's own outline and the client covers it
    // exactly -- no border, no ring, nothing outside the window at all. Sound
    // is the only thing that ever pushes a point past the edge.
    //
    // Drawn outermost first, so `count` counts down: 3 bands summed, then 2,
    // then 1. Full scale in all three is 40, inside the 44 the layer declared,
    // which is a hard clip.
    readonly property var rings: [
        { count: 3, periods: 26, duration: 4300, ink: Theme.edge },
        { count: 2, periods: 22, duration: 5600, ink: Theme.accent },
        { count: 1, periods: 18, duration: 7400, ink: Theme.text }
    ]

    Repeater {
        model: ring.rings.length

        Shape {
            id: band
            required property int index
            readonly property var spec: ring.rings[index]

            anchors.fill: parent
            // One curve per ring, so a drawn edge rather than a stepped one is
            // paid for three times, not once per point.
            antialiasing: true

            // Only the fallback reads this -- see the heartbeat above. Each
            // ring travels at its own rate so the three drift apart rather
            // than moving as one rigid outline.
            property real phase: 0
            NumberAnimation on phase {
                running: ring.focused && ring.levels.length === 0
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
                        const bars = ring.levels.length;
                        // Enough points to resolve whatever is driving it. The
                        // sine needs `perPeriod` per period; a spectrum needs
                        // several per BAR, or every peak is averaged into its
                        // neighbours and real music reads as a smooth ripple.
                        const steps = bars === 0
                            ? spec.periods * (ring.resizing ? 4 : ring.perPeriod)
                            : (ring.resizing
                                ? Math.max(96, bars * 3)
                                : Math.max(240, bars * 7));
                        const x = ring.bleedLeft;
                        const y = ring.bleedTop;
                        const w = ring.paneWidth;
                        const h = ring.paneHeight;
                        const r = Math.min(ring.corner, w / 2, h / 2);

                        for (let i = 0; i < steps; ++i) {
                            const t = i / steps;
                            // `band.phase` is read only on the fallback
                            // branch, so with a feed running it is not a
                            // binding dependency at all and this rebuilds when
                            // the music changes rather than on every frame.
                            let out;
                            if (bars === 0) {
                                out = 16 + 10 * ring.sineAt(t, spec.periods, band.phase);
                            } else {
                                // Straight off the level, so **silence is
                                // flat**: no sound in a band and that band
                                // adds nothing, so the curve is the pane's own
                                // outline and nothing shows outside it.
                                //
                                // A gentle curve and a gain rather than a
                                // straight map: a spectrum sits low almost all
                                // the time, so `level` alone barely leaves the
                                // edge. Both numbers are held down by the
                                // clamp -- a square root with a gain of 1.7
                                // sent every bar past a level of 0.35 to the
                                // ceiling, and a band pinned at its ceiling is
                                // as flat as one pinned at its floor.
                                out = 0;
                                for (let k = 0; k < spec.count; ++k) {
                                    const b = ring.bands[k];
                                    const level = ring.levelAt(t, b.lo, b.hi);
                                    out += b.swell
                                        * Math.min(1, Math.pow(level, 0.7) * 1.15);
                                }
                            }
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
