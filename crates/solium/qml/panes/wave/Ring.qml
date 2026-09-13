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

            // The mean of this frame, which is what the wave is measured
            // AGAINST rather than added to.
            //
            // This is the difference between a border that waves and one that
            // breathes. Feeding the bars in directly gives every point nearly
            // the same radius -- a real spectrum's bars mostly sit within a
            // band of each other, so the outline comes out very close to a
            // plain rounded rectangle however loud the music is. What the eye
            // reads as a wave is one bar standing out from its neighbours, so
            // that is what is amplified: `gain` multiplies the DEVIATION from
            // the frame's own mean, and the mean itself only sets how far out
            // the whole ring rests.
            let mean = 0;
            for (const level of raw) {
                mean += level;
            }
            mean = raw.length > 0 ? mean / raw.length : 0;

            const gain = 3.2;
            const parsed = [];
            for (let i = 0; i < raw.length; ++i) {
                // Loudness lifts the resting radius; the spectrum's shape is
                // what actually makes the waves. Floored at zero so silence
                // rests at the ring's own radius instead of retracting inside
                // it -- with the spectrum mirrored, bass down one half and
                // treble down the other, a track with no treble used to leave
                // that entire half collapsed flat onto the window.
                let height = 0.12 + mean * 0.8 + (raw[i] - mean) * gain;
                height = Math.max(0, Math.min(1.35, height));
                // Eased towards the new reading rather than snapped to it.
                // cava's bars jump frame to frame and a border that jumps with
                // them reads as flicker; carrying some of the last reading
                // turns the same numbers into something that swells and falls,
                // which is what a wave is.
                if (previous.length === raw.length && !isNaN(previous[i])) {
                    height = previous[i] * 0.4 + height * 0.6;
                }
                parsed.push(height);
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

    // The spectrum, at `t` (0..1) round the loop.
    //
    // **Mirrored**, not wrapped: bass-to-treble down one half and back up the
    // other. A spectrum is bass-heavy, so wrapping it once puts every large
    // bar in one short arc and leaves three quarters of the border flat -- and
    // bar 0 against bar N is a cut, silence next to a beat, where mirroring
    // closes the loop with no join at all.
    //
    // **Smoothstepped between bars, not interpolated straight.** A straight
    // line between two readings is a straight line on screen, and the joins
    // between them are corners -- which is exactly the faceting that shows up
    // as creases in the curve. `f * f * (3 - 2f)` leaves the slope at zero on
    // each reading, so neighbouring segments meet without a kink and a row of
    // bars reads as a row of swells.
    function levelAt(t) {
        const bars = ring.levels.length;
        const turn = ((t % 1) + 1) % 1;
        const fold = turn < 0.5 ? turn * 2 : (1 - turn) * 2;
        const at = fold * (bars - 1);
        const i = Math.min(bars - 2, Math.floor(at));
        const f = at - i;
        const eased = f * f * (3 - 2 * f);
        return ring.levels[i] + (ring.levels[i + 1] - ring.levels[i]) * eased;
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
                            ? spec.periods * ring.perPeriod
                            : Math.max(240, bars * 10);
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
                            const height = bars === 0
                                ? ring.sineAt(t, spec.periods, band.phase)
                                : ring.levelAt(t);
                            points.push(ring.ringPoint(
                                t, x, y, w, h, r, spec.reach + spec.swell * height));
                        }
                        points.push(points[0]);
                        return points;
                    }
                }
            }
        }
    }
}
