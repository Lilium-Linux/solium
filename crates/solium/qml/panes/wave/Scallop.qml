// A scalloped border: bumps all the way round the pane, and the bumps travel.
//
// The outline is a rounded rectangle walked by arc length. Every bump is a
// circle centred *on* that outline, so half of it bulges outside the shape and
// half is swallowed by it -- which is what makes the edge read as scalloped
// rather than as a rectangle with circles stuck to it.
//
// The wave is in the radii. Each bump's radius is a sine of how far round the
// outline it sits, plus a phase that advances, so the fat part of the wave
// travels round the border. `waves` is an integer for exactly one reason: the
// sine has to close on itself after one lap, or there is a visible seam where
// the last bump meets the first.
//
// **Nested rings are drawn here, not as layers.** Each ring is a filled shape
// with the next one painted on top, so the bands are what is left uncovered --
// no transparent-ring trickery, and one scene with one texture instead of one
// per ring. Layers are for putting the *client* between two things; these all
// sit behind it, so they have no reason to be separate scenes.

import QtQuick
import Solium

Item {
    id: scallop

    // Set by the compositor. A layer's content owns this contract.
    property string title: ""
    property bool focused: false
    property bool pointerInside: false
    property int contentWidth: 0
    property int contentHeight: 0
    property int paneWidth: 0
    property int paneHeight: 0

    // Where the pane's top-left corner sits inside this canvas. The canvas is
    // the pane grown by the bleed, so without these the border would be drawn
    // `bleedLeft` to the left of the window it belongs to.
    property int bleedLeft: 0
    property int bleedTop: 0

    // --- the shape -------------------------------------------------------
    // How far past the pane the outermost ring reaches. Must stay under the
    // bleed declared in Pane.qml, or the crests are clipped -- bleed is a hard
    // clip, and deliberately so.
    readonly property int reach: 34
    readonly property int rings: 3
    readonly property int ringStep: 9
    readonly property int corner: 26

    // Bump size, and how much of it the wave moves. `amp` is a fraction of
    // `bump`, so a crest is 1.45x a trough and the silhouette visibly breathes.
    readonly property real bump: 13
    readonly property real amp: 0.42
    readonly property int waves: 5

    readonly property var inks: [Theme.text, Theme.accent, Theme.surface]

    // Travels. Bound to `focused` because an animation that never settles keeps
    // the compositor drawing for as long as it runs, and there are ~120 bumps
    // re-laying-out each frame. An unfocused window should not pay that.
    property real phase: 0
    NumberAnimation on phase {
        running: scallop.focused
        loops: Animation.Infinite
        from: 0
        to: 2 * Math.PI
        duration: 5200
    }

    // A point at `t` (0..1) of the way round a rounded rect, by ARC LENGTH --
    // so bumps stay evenly spaced instead of bunching at the corners, which is
    // what walking x and y separately would do.
    function outlineAt(t, x, y, w, h, r) {
        const straightX = Math.max(0, w - 2 * r);
        const straightY = Math.max(0, h - 2 * r);
        const arc = Math.PI * r / 2;
        const total = 2 * straightX + 2 * straightY + 4 * arc;
        let d = ((t % 1) + 1) % 1 * total;

        // Top edge, left to right.
        if (d < straightX) {
            return Qt.point(x + r + d, y);
        }
        d -= straightX;
        // Top-right corner.
        if (d < arc) {
            const a = (d / arc) * Math.PI / 2;
            return Qt.point(x + w - r + r * Math.sin(a), y + r - r * Math.cos(a));
        }
        d -= arc;
        if (d < straightY) {
            return Qt.point(x + w, y + r + d);
        }
        d -= straightY;
        if (d < arc) {
            const a = (d / arc) * Math.PI / 2;
            return Qt.point(x + w - r + r * Math.cos(a), y + h - r + r * Math.sin(a));
        }
        d -= arc;
        if (d < straightX) {
            return Qt.point(x + w - r - d, y + h);
        }
        d -= straightX;
        if (d < arc) {
            const a = (d / arc) * Math.PI / 2;
            return Qt.point(x + r - r * Math.sin(a), y + h - r + r * Math.cos(a));
        }
        d -= arc;
        if (d < straightY) {
            return Qt.point(x, y + h - r - d);
        }
        d -= straightY;
        const a = (d / arc) * Math.PI / 2;
        return Qt.point(x + r - r * Math.cos(a), y + r - r * Math.sin(a));
    }

    // Outermost ring first, so each one is painted over by the next and the
    // bands that remain are the difference between them.
    Repeater {
        model: scallop.rings

        Item {
            id: ring
            required property int index

            anchors.fill: parent

            readonly property int grow: scallop.reach - index * scallop.ringStep
            readonly property real bx: scallop.bleedLeft - grow
            readonly property real by: scallop.bleedTop - grow
            readonly property real bw: scallop.paneWidth + 2 * grow
            readonly property real bh: scallop.paneHeight + 2 * grow
            readonly property real corner: Math.max(4, scallop.corner - index * 3)

            readonly property color ink: scallop.focused
                ? scallop.inks[index % scallop.inks.length]
                : (index === 0 ? Theme.edgeInactive : Theme.surfaceInactive)

            // Each ring's wave starts a little further round than the one
            // outside it, so the crests of the three do not line up into one
            // fat lobe.
            readonly property real offset: index * 0.6

            // Quantised so a live resize does not rebuild the delegates on
            // every pixel of drag: the count changes once per four bumps of
            // perimeter rather than continuously.
            readonly property int bumps: {
                const perimeter = 2 * (bw + bh) - 8 * corner + 2 * Math.PI * corner;
                return Math.max(12, 4 * Math.round(perimeter / (scallop.bump * 1.55 * 4)));
            }

            Rectangle {
                x: ring.bx
                y: ring.by
                width: ring.bw
                height: ring.bh
                radius: ring.corner
                color: ring.ink
            }

            Repeater {
                model: ring.bumps

                Rectangle {
                    required property int index

                    readonly property real t: index / ring.bumps
                    readonly property real size: scallop.bump * (1 + scallop.amp
                        * Math.sin(t * scallop.waves * 2 * Math.PI
                                   + scallop.phase + ring.offset))
                    readonly property point at: scallop.outlineAt(
                        t, ring.bx, ring.by, ring.bw, ring.bh, ring.corner)

                    width: size
                    height: size
                    radius: size / 2
                    x: at.x - size / 2
                    y: at.y - size / 2
                    color: ring.ink
                }
            }
        }
    }
}
