// One edge's worth of flowing sine waves.
//
// **Nothing here is recomputed per frame.** Each band is a single `Image`
// holding one seamless period of a sine, tiled along the edge, inside an
// `Item` whose `x` is animated. A travelling sine is a static sine translated
// -- slide it by exactly one wavelength and the picture is identical -- so the
// animation is one transform per band and the loop has no seam to hide.
//
// That shape is not a preference, it is what the compositor can see. Measured
// nested, four frames 400ms apart:
//
//     QtQuick.Shapes, animating the Shape's own x      540 bytes changed
//     QtQuick.Shapes, inside an animated parent Item   540 bytes changed
//     static Rectangles, animated parent Item      120,989 bytes changed
//     one tiled Image, animated parent Item         23,399 bytes changed
//
// 540 is the blinking cursor -- it is what "nothing moved" looks like. A
// `Shape` renders once here and never redraws, whatever moves it, so the whole
// decoration sat still. `Image` and `Rectangle` both drive redraws; `Image`
// does it with one item per band instead of hundreds.

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
