// A border that answers the pointer entering and leaving the window.
//
// `pointerInside` is true while the pointer is anywhere over the window, the
// client included — the compositor tracks that, because the client's own
// surface tells us nothing. So the border can wake as the cursor arrives
// rather than only when it reaches the frame itself.
//
//     SOLIUM_PANE=proximity

import QtQuick
import Solium

PaneStyle {
    // Four pixels on every side, held whether or not the border is using
    // them: the space cannot appear and disappear under the client every time
    // the pointer crosses the window. The layer swells to `insetTop` when the
    // pointer is inside and paints a hairline when it is not.
    insets.top: 4
    insets.right: 4
    insets.bottom: 4
    insets.left: 4

    requires: []

    Layer {
        depth: "frame"
        name: "border"
        source: "Frame.qml"
    }
}
