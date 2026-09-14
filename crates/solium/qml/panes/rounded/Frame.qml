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

        // The hairline sits on the seam — the band's last row — for the same
        // reason. At the bar's own bottom it would be under the client and
        // visible only inside the two notches, as a stray mark across each.
        Rectangle {
            y: frame.insetTop - height
            width: parent.width
            height: 1
            color: frame.focused ? Theme.edge : Theme.edgeInactive
        }
    }
}
