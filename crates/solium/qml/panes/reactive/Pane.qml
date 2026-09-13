// A border that lights up where the cursor is, with a titlebar above it.
//
// The pointer is delivered to the layer while it is anywhere over the window,
// so the glow can follow the cursor across the client area rather than only
// along the frame's own band.
//
//     SOLIUM_PANE=reactive

import QtQuick
import Solium

PaneStyle {
    // A bar at the top and a thin margin on the other three sides. The
    // compositor hands these to the layer, which paints its bar `insetTop`
    // tall and punches the client's area out at `insetLeft`.
    insets.top: 30
    insets.right: 4
    insets.bottom: 4
    insets.left: 4

    // The glow is stacked rectangles rather than a shader, precisely so this
    // runs on the software path too. See `Frame.qml`.
    requires: []

    Layer {
        depth: "frame"
        name: "bar"
        source: "Frame.qml"
    }
}
