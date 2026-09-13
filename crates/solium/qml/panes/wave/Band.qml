// One edge's worth of flowing sine waves.
//
// **Nothing here is recomputed per frame.** Each band is a single `Image`
// holding one seamless period of a sine, tiled along the edge, inside an
// `Item` whose `x` is animated. A travelling sine is a static sine translated
// -- slide it by exactly one wavelength and the picture is identical -- so the
// animation is one transform per band and the loop has no seam to hide.
//
// One tiled `Image` per band rather than a drawn curve, because a wave is
// periodic: one rasterised period repeated costs one texture and one node,
// whatever the window's width. `QtQuick.Shapes` would also work -- measured,
// 59,764 bytes changed per frame -- and would let the colours come from
// `Theme` instead of being baked into the art. The cost of that is
// re-tessellating a polyline per band; the cost of this is three SVGs that do
// not follow the theme. Either is defensible and this one was already built.
//
// **Corrected, because the first version of this comment said the opposite.**
// It claimed `Shape` renders once and never redraws, with a table of
// measurements to prove it. The measurements were real and the conclusion was
// wrong: every `Shape` tried was inside a `Repeater` delegate, and host.cpp's
// `animation_running` could not see into one, so what was actually being
// measured was that bug. `Shape` was never the problem.

import QtQuick

Item {
    id: band

    // How long the edge is, and how far out the waves may run.
    property real span: 0
    property real depth: 0
    property bool running: false

    // Back to front: the first is the furthest out and the palest.
    readonly property var waves: [
        { art: "band0.svg", wavelength: 260, duration: 7000 },
        { art: "band1.svg", wavelength: 190, duration: 5200 },
        { art: "band2.svg", wavelength: 150, duration: 4100 }
    ]

    Repeater {
        model: band.waves.length

        Item {
            id: slider
            required property int index
            readonly property var wave: band.waves[index]

            y: 0
            height: band.depth
            width: band.span + wave.wavelength

            // Slides exactly one wavelength and repeats, which is seamless by
            // construction rather than by hiding a jump.
            NumberAnimation on x {
                running: band.running
                loops: Animation.Infinite
                from: 0
                to: -slider.wave.wavelength
                duration: slider.wave.duration
            }

            Image {
                source: slider.wave.art
                // Rasterised once at this size and then repeated. The SVG is
                // one period, so the tile seam falls where the curve already
                // meets itself.
                sourceSize: Qt.size(slider.wave.wavelength, band.depth)
                fillMode: Image.TileHorizontally
                anchors.fill: parent
            }
        }
    }
}
