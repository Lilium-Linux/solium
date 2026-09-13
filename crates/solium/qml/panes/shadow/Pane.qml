// A drop shadow, approximated.
//
// **Read this before judging it.** A real shadow is a blur of the client's own
// silhouette, and that needs the pass mechanism — an effect declaring that it
// reads `self`, which does not exist yet. `client.shadow.blur` is a reserved
// key nothing looks at, on purpose, so a style written today has somewhere to
// put it when the real one arrives.
//
// What this does instead is what a compositor did before shaders: stack eight
// rounded rectangles of falling opacity, each slightly larger than the last,
// offset downward. The falloff is the overlap rather than a gradient. It is
// convincing at a glance and it is not a blur — the bands are visible if you
// look for them, and it costs eight alpha-blended rectangles per window per
// frame it changes.
//
//     SOLIUM_DECORATION=shadow
//
// It is here to show that `behind` plus `bleed` is enough to put something
// underneath a window that reaches past it — and to be the thing item 6
// replaces with four lines and a shader.

import QtQuick
import Solium

PaneStyle {
    insets.top: 0
    requires: []

    Layer {
        depth: "behind"
        name: "shadow"
        bleed: 30
        source: "Shadow.qml"
    }
}
