// A window's titlebar. The default.
//
// Drawn by the compositor, not by the client: the frame reserves its own height
// so frame and window are one object, and the client is never covered.
//
//     SOLIUM_PANE=top
//
// One layer, at `frame`, inside the reserved band. That is what every
// decoration was before styles had layers, and this is that same frame written
// in the format the others are: the manifest says what the pane reserves, and
// `Frame.qml` paints it.

import QtQuick
import Solium

PaneStyle {
    // How much of the window this style reserves. The client is placed inside
    // what is left, and the compositor hands this number to every layer — so
    // `Frame.qml` sizes its bar from `insetTop` rather than from a second copy
    // of 32 that could drift away from this one.
    insets.top: 32

    // Ordinary QML on the software path: no shader, no Canvas.
    requires: []

    Layer {
        depth: "frame"
        name: "bar"
        source: "Frame.qml"
    }
}
