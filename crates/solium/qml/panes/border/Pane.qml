// No bar at all: a border, and the window's own edges.
//
// The smallest useful style, and the one worth reading first — every other
// bundle here is this plus something.
//
//     SOLIUM_PANE=border

import QtQuick
import Solium

PaneStyle {
    // Three pixels on every side, which is also how thick the outline is:
    // `Frame.qml` draws its border `insetTop` wide, from the number the
    // compositor hands it.
    insets.top: 3
    insets.right: 3
    insets.bottom: 3
    insets.left: 3

    requires: []

    Layer {
        depth: "frame"
        name: "border"
        source: "Frame.qml"
    }
}
