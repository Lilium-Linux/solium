// A border that physically waves, all the way round the window.
//
// Not a colour cycling -- the geometry moves. The outline is scalloped with
// circular bumps, and each bump's radius is a travelling sine, so the fat part
// of the wave runs round the frame and the silhouette of the decoration is
// different every frame.
//
//     SOLIUM_PANE=wave
//
// **The window does not move and does not shrink.** `insets` reserves nothing,
// so the client is laid out exactly as it would be with no decoration at all;
// every waving pixel is `bleed`, outside the pane, in space the compositor
// would otherwise have left to whatever is behind. That is the whole point of
// the feature -- a border that reacts without the application knowing.
//
// Three nested rings, drawn inside one layer rather than as three layers: they
// all sit behind the client, so there is nothing to put between them. Layers
// are for when the *client* goes in the middle.
//
// Overlap two windows to see what bleed really means: the lower window's
// crests pass underneath the upper one rather than jumping in front, because a
// bleeding layer keeps its pane's place in the stack.
//
// **It draws continuously while focused**, which is what `Animation.Infinite`
// costs -- roughly 120 circles re-laying-out per frame. Unfocused windows
// stop, which is why the phase is bound to `focused`.
//
// Ordinary QML on the software path: no shader, no Canvas, so `requires` stays
// empty and this runs anywhere.

import QtQuick
import Solium

PaneStyle {
    // Nothing reserved. No titlebar, no inset, no repositioned client.
    insets.top: 0

    requires: []

    Layer {
        depth: "behind"
        name: "scallop"
        // Comfortably more than Scallop.qml's `reach` of 34, because a crest
        // is `bump * (1 + amp)` tall on top of that and bleed is a hard clip:
        // anything past what the layer declared is cut, not drawn.
        bleed: 56
        source: "Scallop.qml"
    }
}
