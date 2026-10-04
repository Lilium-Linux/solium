// One layer of a pane style.
//
// A layer is rasterised into its own scene and becomes its own element in the
// frame, which is what lets the client's surface sit between two layers the
// same style produced. That is the whole reason layers are separate scenes
// rather than one image: nothing else lets a client be sandwiched inside
// something a single QML file produced.
//
// Content is written inline or delegated with `source:`, and the syntax does
// not change between the two.
//
//     Layer { depth: "behind"; bleed: 200; Glow { anchors.fill: parent } }
//     Layer { depth: "above";  bleed: { top: 160 }; source: "Spikes.qml" }
//
// **There is deliberately no `visible:` binding in here.** An inline layer is
// hidden by its `PaneStyle` rather than by asking one, because a Layer cannot
// ask: items assigned to a `list<Item>` get a QObject parent and not a visual
// one, and QML's `parent` is `parentItem()` — so `parent` is null for every
// Layer in a PaneStyle and a binding against it is true for all of them.
// Measured on these two types: `layers` 3, `children` 0. The sibling-hiding is
// in `PaneStyle.showOneLayer`, which parents as well as hides.

import QtQuick

Item {
    // Where the client's surface sits relative to this layer.
    //   "behind" — under the client
    //   "frame"  — where decorations are today
    //   "above"  — over the client
    //
    // A string, deliberately: there are three values today, and a fourth
    // added later must not break the format of every style already written.
    property string depth: "frame"

    // How far past the pane's outer rect this layer may paint, in logical
    // pixels. A number for all four sides, or an object for per-side —
    // `{ top: 160 }`, with the sides it does not name left at zero.
    //
    // It is a cost, not a permission: the canvas is this much larger, and
    // every pixel of it is rasterised, uploaded and repainted when the layer
    // animates. A bar throwing spikes upward should ask for `{ top: 160 }`
    // rather than 160, and not pay for three sides it never touches.
    //
    // It is also a promise rather than a request. The layer is clipped to the
    // canvas it declared — otherwise one style can force a full-screen repaint
    // every frame, and the cost shows up as the whole desktop stuttering with
    // nothing naming the cause.
    property var bleed: 0

    // A QML file in the bundle, when the content is not inline.
    property string source: ""

    // For diagnostics: which layer a warning is about.
    property string name: ""

    // Whether this layer has nothing to draw right now. A layer that is
    // almost always empty -- a pill shown for a moment at a caret -- binds it,
    // and while it is true the compositor draws nothing of it. In software it
    // also keeps no image for it; on the GPU the scene keeps the buffer it was
    // last drawn into. The frame it turns false, the layer is drawn again.
    // False unless bound, so a layer that never says is drawn as it always
    // was.
    // `decoration::tests::a_dormant_layer_draws_nothing_and_holds_no_buffer`.
    //
    // Read from the root of the layer's scene: `PaneStyle` hands an inline
    // layer's on, and a delegated layer (`source:`) declares
    // `property bool dormant` on its own file's root instead, as it declares
    // `onButton`.
    property bool dormant: false
}
