// The one layer of `panes/flush`: the bar whose top is the window's top.
//
// QML's half of the seam, and here it is the half that shows. The compositor
// cuts only the client's bottom two corners — `Pane.qml` squares the top pair
// — so the window's rounded top silhouette is drawn entirely by this file,
// with `Rectangle.radius`, and the shader's curve appears only at the bottom.
//
// **The bar is exactly `insetTop` tall and that is the point.** It reaches
// nowhere past its band, so it cannot cover a row the client drew, and the
// client's own square top row starts on the pixel below its last one. Compare
// `panes/rounded/Frame.qml`, whose bar is `clientRadius` taller and has to sit
// at `behind` to pay for it.
//
// Two rectangles rather than one, for the same reason `rounded/` needs two:
// `Rectangle.radius` rounds all four corners and only the top two should be
// round. The child squares the bottom pair off again. Without it the bar's
// bottom corners curve away from the client's square ones and the wallpaper
// shows through two small triangles at the ends of the seam.

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

    // What the style reserved, in logical pixels. Zero is what this reads if
    // the file is built outside a pane — by `--check-qml`, say — and drawing
    // nothing is the honest answer there.
    property int insetTop: 0

    // The largest of the four corners the compositor is cutting, which for
    // this style is the twelve it is cutting the bottom pair to: the top pair
    // are the zeroes `Pane.qml` declared. So it is the window's radius, read
    // through the one name that means "the window's radius", and the bar's own
    // top corners match the shader's bottom ones without a second copy of 12
    // in this file to drift from the one in the manifest.
    property int clientRadius: 0

    Rectangle {
        id: bar

        width: parent.width
        height: frame.insetTop
        color: frame.focused ? Theme.surface : Theme.surfaceInactive
        // The window's top corners. Nothing else draws them.
        radius: frame.clientRadius

        // And the bottom two squared off again, so the bar's last row is the
        // full width of the window and the client's first row meets it with
        // no gap at either end.
        Rectangle {
            anchors.fill: parent
            anchors.topMargin: bar.radius
            color: bar.color
        }

        // Centred in the bar, which here is also the band — unlike
        // `panes/rounded`, where the two are different rectangles.
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

        // A hairline on the seam, so the flat join reads as deliberate rather
        // than as the bar and the client being the same surface.
        Rectangle {
            anchors.bottom: parent.bottom
            width: parent.width
            height: 1
            color: frame.focused ? Theme.edge : Theme.edgeInactive
        }
    }
}
