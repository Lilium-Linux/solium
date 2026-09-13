// A titlebar with an animation running inside it.
//
// Two of them, in fact: a sheen that travels across the focused window's bar,
// and an accent line under it that breathes. Both are ordinary QML animations
// — the compositor drives the scene's clock every frame while it is changing,
// and stops driving it as soon as the scene says it has settled, so an
// unfocused window costs nothing to keep on screen.
//
//     SOLIUM_PANE=pulse

import QtQuick
import Solium

PaneStyle {
    // The bar and the breathing line under it, together. The layer divides
    // this one band into the two of them, from the `insetTop` the compositor
    // hands it.
    insets.top: 38

    requires: []

    Layer {
        depth: "frame"
        name: "bar"
        source: "Frame.qml"
    }
}
