// Sine waves flowing round the window, outside it.
//
//     SOLIUM_PANE=wave
//
// **The window does not move and does not shrink.** `insets` reserves nothing,
// so the client is laid out exactly as it would be with no decoration at all;
// every wave is `bleed`, in space the compositor would otherwise have left to
// whatever is behind. A border that reacts without the application knowing,
// and without the window paying for it.
//
// **And it is cheap.** A travelling sine is a static sine translated, so the
// curves are sampled when the window is resized and never again -- the
// animation moves twelve scene-graph nodes and touches no geometry. That is
// deliberate: the version before this one built the wave out of a few hundred
// `Rectangle`s and re-laid-out all of them every frame, and it stuttered.
//
// Overlap two windows to see what bleed really means: the lower window's waves
// pass *underneath* the upper one rather than jumping in front, because a
// bleeding layer keeps its pane's place in the stack.
//
// `QtQuick.Shapes`, which the cursor already uses, so `requires` stays empty.

import QtQuick
import Solium

PaneStyle {
    // Nothing reserved. No titlebar, no inset, no repositioned client.
    insets.top: 0

    requires: []

    Layer {
        depth: "behind"
        name: "waves"
        // More than Waves.qml's `reach` of 44, because a crest rides above the
        // baseline and bleed is a hard clip: anything past what the layer
        // declared is cut, not drawn.
        bleed: 64
        source: "Waves.qml"
    }
}
