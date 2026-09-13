// A border that physically waves.
//
// Not a colour cycling — the geometry moves. Sixty bars along the top edge,
// each one's height a sine of its position plus a phase that travels, so the
// crest walks along the window and the silhouette of the decoration changes
// every frame.
//
//     SOLIUM_DECORATION=wave
//
// It waves *upward, past the pane*, which is the point: `bleed` gives the layer
// 40px of canvas above the window, and the crests use it. Without bleed this
// would have to wave into space the client owns.
//
// **It draws continuously while focused.** An animation that never settles
// keeps the compositor rendering for as long as the window has focus — that is
// what `Animation.Infinite` means, and it is a real cost at 260Hz. Unfocused
// windows stop, which is why the phase animation is bound to `focused`.
//
// Everything here is ordinary QML on the software path: no shader, no Canvas,
// so `requires` stays empty and this runs anywhere.

import QtQuick
import Solium

PaneStyle {
    insets.top: 10
    requires: []

    Layer {
        depth: "above"
        name: "wave"
        bleed: { "top": 40 }
        source: "Wave.qml"
    }

    Layer {
        depth: "frame"
        name: "sill"
        source: "Sill.qml"
    }
}
