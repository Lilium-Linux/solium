// One edge's worth of flowing sine waves.
//
// **The geometry is built once and then slid sideways.** A travelling sine is
// a static sine translated -- shift it by exactly one wavelength and the image
// is identical -- so nothing here recomputes a curve per frame. Each band is
// one `Shape` whose polyline is sampled when the size changes, and the
// animation moves a single `x`.
//
// That is the whole difference between this and something that stutters. The
// version before this one built the wave out of a few hundred `Rectangle`s and
// re-laid-out every one of them every frame; this is a dozen scene-graph nodes
// with a transform on each, and the sampling happens on resize.

import QtQuick
import QtQuick.Shapes

Item {
    id: band

    // How long the edge is, and how deep the waves are allowed to run.
    property real span: 0
    property real depth: 0
    property bool running: false

    // Drawn back to front: the first is the furthest out and the palest.
    property var inks: []

    implicitWidth: span
    implicitHeight: depth

    // Sampled per wave, not per pixel: a sine needs few points to read as
    // smooth, and every point is a vertex the renderer walks.
    readonly property int samplesPerWave: 14

    Repeater {
        model: band.inks.length

        Shape {
            id: wave
            required property int index

            // Each band has its own wavelength, height and speed, so they
            // drift apart instead of moving as one rigid block -- which is
            // what makes it read as water rather than as a striped ribbon.
            readonly property real wavelength: 210 + index * 64
            readonly property real amplitude: band.depth * (0.30 - index * 0.055)
            readonly property real rest: band.depth * (0.42 + index * 0.13)
            readonly property int duration: 5200 + index * 2100

            // One wavelength of overhang, because the shape slides by exactly
            // that much: without it the trailing end would walk into view.
            readonly property real drawn: band.span + wavelength + 2

            width: drawn
            height: band.depth
            y: 0

            // Slides by one wavelength and repeats. Seamless by construction:
            // a sine translated by its own wavelength is the same sine, so
            // there is no jump at the loop point to hide.
            property real shift: 0
            NumberAnimation on shift {
                running: band.running
                loops: Animation.Infinite
                from: 0
                to: -wave.wavelength
                duration: wave.duration
            }
            x: -wave.wavelength + shift

            ShapePath {
                fillColor: band.inks[wave.index]
                strokeWidth: -1

                PathPolyline {
                    // Rebuilt when the geometry changes, never on the phase.
                    path: {
                        const points = [];
                        const k = 2 * Math.PI / wave.wavelength;
                        const steps = Math.max(
                            8, Math.ceil(wave.drawn / wave.wavelength) * band.samplesPerWave);
                        for (let i = 0; i <= steps; ++i) {
                            const x = wave.drawn * i / steps;
                            points.push(Qt.point(
                                x, wave.rest + Math.sin(x * k) * wave.amplitude));
                        }
                        // Down to the pane's edge and back, so the band is a
                        // filled body rather than a hairline.
                        points.push(Qt.point(wave.drawn, band.depth + 2));
                        points.push(Qt.point(0, band.depth + 2));
                        return points;
                    }
                }
            }
        }
    }
}
