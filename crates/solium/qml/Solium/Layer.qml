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

    // How opaque the compositor draws this layer's picture, from 0 to 1:
    // applied as it composites the picture, over the window's own opacity,
    // so animating it draws no new picture -- a fade by it costs a blend at
    // another alpha per frame, and nothing of Qt's. 1 unless bound. A
    // delegated layer declares `property real layerOpacity` on its own root
    // instead, as it declares `dormant`.
    // `decoration::tests::a_layer_fading_by_its_opacity_draws_no_new_picture`.
    property real layerOpacity: 1

    // How much more room than the style's `insets.top` this window's frame
    // takes at the top, in logical pixels: a titlebar growing taller and
    // pushing the window's content down rather than drawing over it. The
    // frame reserves `insets.top` and the largest `growTop` of its layers,
    // and every layer is told the sum as `insetTop`. A floating window grows
    // taller and its content moves down at its size; a window held in a size
    // (tiled, maximised) keeps it, and its application is given that much
    // less height once the value stops moving. Animate it with a `Behavior`:
    // the compositor follows it every frame. 0 unless bound. A delegated
    // layer declares `property real growTop` on its own root instead, as it
    // declares `dormant`.
    // `state::tests::real_client::reflow_on_close::frames::a_floating_windows_frame_grows_and_its_client_moves_down_at_its_size`.
    property real growTop: 0

    // How often, at most, the compositor draws this layer again for a new
    // size while its window's size keeps changing (a drag on its edge, a
    // layout's sweep), in times a second, from 5 to 1000: between, the
    // picture it has is drawn stretched to the window, as the application's
    // own is until it has drawn one at the new size, and the last size is
    // drawn within a `1 / resizeHz` second of the window stopping. Something
    // new to draw (a title, a focus change, an animation) is drawn at once.
    // 0, the default, draws it at every size. A delegated layer declares
    // `property real resizeHz` on its own root instead, as it declares
    // `dormant`.
    // `decoration::tests::a_layer_is_drawn_again_for_a_new_size_at_most_resize_hz_times_a_second`.
    property real resizeHz: 0
}
