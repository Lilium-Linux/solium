// A titlebar down the left-hand side.
//
// The same pane as `panes/top` turned ninety degrees, which is the point: where
// a bar lives is two numbers and an anchor, not a different mechanism.
//
//     SOLIUM_PANE=left

import QtQuick
import Solium

PaneStyle {
    // Reserved on the left instead of the top. The compositor hands it to the
    // layer, which sizes its bar from `insetLeft`.
    insets.left: 34

    requires: []

    Layer {
        depth: "frame"
        name: "bar"
        source: "Frame.qml"
    }
}
