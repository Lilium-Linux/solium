// A titlebar that is not there until you approach the window.
//
// Reserves nothing, so the client keeps the whole slot and nothing reflows
// when the bar appears — the layer simply draws over the window. That is what
// insets of zero buy: an overlay rather than a band.
//
//     SOLIUM_PANE=reveal
//
// The bar slides out of the window's own top edge, which reads as the window
// producing it rather than as a panel fading in on top.

import QtQuick
import Solium

PaneStyle {
    // Nothing reserved on any side, which is the whole design. Declared by
    // saying nothing: every inset defaults to zero.
    //
    // A style that reserves nothing has nowhere to paint but over the client,
    // so the compositor treats every layer of it as an overlay whether or not
    // it says so — see `LayerScene::build`.

    requires: []

    Layer {
        depth: "frame"
        name: "bar"
        source: "Frame.qml"
    }
}
