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
// `clientRadius` past the band puts those two corners behind the bar, and the
// bar's own rounded top becomes the window's top. A style that wants the
// client's top corners to *be* the window's reserves nothing instead: then the
// client is the whole pane and the shader draws all four, which is what
// `panes/reveal` would look like with a radius.

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
    // copied when it changes rather than only its bands. Without it the
    // overhang is never composited and the notches come back — see
    // `LayerScene::build`, which reads this property by name.
    property bool overlay: true

    Rectangle {
        id: bar

        width: parent.width
        // The reserved band, plus the client's two cut corners underneath it.
        height: frame.insetTop + frame.clientRadius
        color: frame.focused ? Theme.surface : Theme.surfaceInactive
        // The window's top corners, rounded by QML rather than by the shader,
        // to the number the shader was given.
        radius: frame.clientRadius

        // And the bottom two squared off again. `radius` rounds all four, and
        // this bar's bottom edge is in the middle of the window: a curve there
        // would show the wallpaper through a gap between the bar and the
        // client it is sitting on.
        Rectangle {
            anchors.fill: parent
            anchors.topMargin: bar.radius
            color: bar.color
        }

        Text {
            anchors.centerIn: parent
            text: frame.title
            elide: Text.ElideRight
            width: parent.width - Theme.margin * 2
            horizontalAlignment: Text.AlignHCenter
            color: frame.focused ? Theme.text : Theme.textDim
            font.family: Theme.fontFamily
            font.pixelSize: Theme.fontSize
        }

        Rectangle {
            anchors.bottom: parent.bottom
            width: parent.width
            height: 1
            color: frame.focused ? Theme.edge : Theme.edgeInactive
        }
    }
}
