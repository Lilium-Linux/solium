// A titlebar underneath the window.
//
// Reserves at the bottom instead of the top, so the client sits above it. The
// buttons stay on the right, where the hand expects them.
//
//     SOLIUM_PANE=bottom

import QtQuick
import Solium

PaneStyle {
    // The compositor hands it to the layer, which sizes its bar from
    // `insetBottom`.
    insets.bottom: 30

    requires: []

    Layer {
        depth: "frame"
        name: "bar"
        source: "Frame.qml"
    }
}
