// The one layer of `panes/rounded`: a titlebar that hugs the curve.
//
// QML's half of the seam. The compositor cuts all four of the client's corners
// with a fragment program and tells this file the radius it used; this file
// rounds its own two top corners to the same number and covers the client's
// two top ones, so the window has one silhouette rather than two.
//
// **The overhang is the whole trick and it is worth saying why it is needed.**
// The shader rounds the client, and the client starts below the reserved band
// — so its top corners are cut `insetTop` pixels down, in the middle of the
// window, and show as two notches under the ends of a square bar. Reaching
// `clientRadius` past the band puts the bar *in* those two notches, and the
// bar's own rounded top becomes the window's top. A style that wants the
// client's top corners to *be* the window's reserves nothing instead: then the
// client is the whole pane and the shader draws all four, which is what
// `panes/reveal` would look like with a radius.
//
// **The overhang is only correct because the layer is at `behind`**, and this
// file is written against that. At `frame` the same rectangle would cover the
// client's top `clientRadius` rows — the top half of a terminal's first line —
// which is the defect this style was reported for. Under the client it fills
// the notches and covers nothing, because the client is drawn over it. See
// `Pane.qml`, which is where the depth is declared and argued.
//
// The consequence for this file is that **the bar's height and its visible
// band are two different numbers**, and everything meant for the eye is
// positioned against the band. `insetTop` is what a person sees; the
// `clientRadius` below it is under the client except in the two notches. The
// title centred in the whole rectangle would sit `clientRadius / 2` low.
// `panes/flush` has no overhang and so can centre in the bar itself.

import QtQuick
import Solium

Item {
    id: frame

    // Set by the compositor, on every layer of every style.
    property string title: ""
    property bool focused: false
    property bool pointerInside: false
    property int contentWidth: 0
    property int contentHeight: 0

    // What the style reserved, and what it asked the compositor to cut. Both
    // in logical pixels; nothing QML is told is ever in device pixels.
    property int insetTop: 0
    property int clientRadius: 0

    // This layer paints below the band it was given, so the whole frame is
    // copied when it changes rather than only its bands.
    //
    // Redundant today and kept deliberately: `LayerScene::build` already
    // treats every layer that is not at `frame` as an overlay, because there
    // is nowhere inside the insets for one to paint. It is written out because
    // it is a property of *this file* — the bar really does paint below its
    // band — and a style that moved this layer back to `frame` would need it
    // and would not think to add it.
    property bool overlay: true

    Rectangle {
        id: bar

        width: parent.width
        // The reserved band, plus the client's two cut corners underneath it.
        // Only the first `insetTop` of this is ever seen whole; the rest is
        // under the client, showing through where the shader cut it away.
        height: frame.insetTop + frame.clientRadius
        color: frame.focused ? Theme.surface : Theme.surfaceInactive
        // The window's top corners, rounded by QML rather than by the shader,
        // to the number the shader was given.
        radius: frame.clientRadius

        // And the bottom two squared off again. `radius` rounds all four, and
        // this bar's bottom edge is in the middle of the window: a curve there
        // is the identical circle the shader cut the client's top corner with,
        // so the two would go missing together and the notch would show the
        // wallpaper instead of the bar.
        Rectangle {
            anchors.fill: parent
            anchors.topMargin: bar.radius
            color: bar.color
        }

        // Centred in the BAND and not in the bar. The bar is `clientRadius`
        // taller than the band, and all of that is under the client, so a
        // title centred in `parent` reads half a radius low.
        Text {
            y: (frame.insetTop - height) / 2
            anchors.horizontalCenter: parent.horizontalCenter
            text: frame.title
            elide: Text.ElideRight
            width: parent.width - Theme.margin * 2
            horizontalAlignment: Text.AlignHCenter
            color: frame.focused ? Theme.text : Theme.textDim
            font.family: Theme.fontFamily
            font.pixelSize: Theme.fontSize
        }

    }

    // The seam, traced along the client's cut rather than laid across it.
    //
    // **A straight full-width hairline is wrong in this style and right in
    // `panes/flush`**, and the difference is the corner. There the client's
    // top is square, so the band's last row and the client's first row meet
    // end to end and a straight line is the seam. Here the client's top two
    // corners are cut away, so the last `clientRadius` pixels at each end of
    // that row are not the seam at all — they are the middle of a notch. A
    // straight line drawn there is a grey lid lying across the top of the
    // white curve, crossing the rounding instead of following it. That is
    // what shipped in 83be7a3 and what the user rejected.
    //
    // So the line belongs to the CLIENT's outline and not to the bar's bottom
    // edge, and the shape that gives is a rounded rectangle with a border and
    // no fill, **concentric with the shader's own arc and one pixel outside
    // it**. The client's top-left corner is cut on a circle of radius `r`
    // centred at `(r, insetTop + r)`. Growing this rect by one pixel on the
    // top and both sides — `x: -1`, `y: insetTop - 1`, `width + 2` — and
    // giving it `radius: r + 1` puts its arc centre at that same point with a
    // radius one larger, so every point of the border is exactly one pixel
    // outside the cut, all the way round. Measured: it is.
    //
    // **The two arcs come from different rasterisers and that is a real
    // question, not a formality.** The client's is `fragment.rs`'s distance
    // field evaluated on the GPU; this one is Qt's scene graph. They are the
    // same circle analytically, which is necessary and not sufficient. What
    // was measured nested at r=12 is that the border sits one pixel outside
    // the client's first opaque pixel on every row of the arc, with no row
    // where it crosses into the client and no row where it leaves a gap.
    // The alternative, had they disagreed, was to stop the straight line
    // `clientRadius` short of each end — never wrong, but never an outline
    // either.
    //
    // Only its top edge and its top two corners are ever seen: the layer is
    // at `behind`, so the rest is under the client, and the side columns at
    // `x = -1` and `x = width` are outside the canvas the layer is clipped
    // to. The height is `2 * radius + 4` for the same reason — two radii is
    // the least Qt will round both corners of, and the four keeps the BOTTOM
    // two corners below the client's top cut and so out of sight. A taller
    // one would reach the client's bottom cut and draw a stray arc in each of
    // those notches.
    Rectangle {
        x: -1
        y: frame.insetTop - 1
        width: frame.width + 2
        height: 2 * frame.clientRadius + 4
        color: "transparent"
        radius: frame.clientRadius + 1
        border.width: 1
        border.color: frame.focused ? Theme.edge : Theme.edgeInactive
    }
}
