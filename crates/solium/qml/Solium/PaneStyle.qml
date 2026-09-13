// A pane's whole appearance: its layers, and what they reserve.
//
// The entry point of a style bundle. `Pane.qml` in a folder under `panes/`
// declares one of these; the compositor reads it, then instantiates each
// Layer's content as its own scene.
//
//     import QtQuick
//     import Solium
//
//     PaneStyle {
//         insets.top: 32
//         requires: ["gpu"]
//
//         Layer { depth: "behind"; bleed: 200; Glow { anchors.fill: parent } }
//         Layer { depth: "frame";  source: "Frame.qml" }
//         Layer { depth: "above";  bleed: { top: 160 }; source: "Spikes.qml" }
//     }
//
// It is QML rather than Lua on purpose: style lives in QML, Lua configures the
// compositor. It also means a simple style is one file with inline layers and
// a complex one is a folder, with no format to migrate between.
//
// `style::load` reads `insets`, `requires` and the Layer children; `client` is
// still reserved and read by nobody.
//
// The same file is loaded twice over, for two different jobs. As a *manifest*
// it is built at 1x1, asked what it declares and never drawn — `layerIndex`
// stays -1 and nothing in it is parented. As a *layer* it is built once more
// per inline layer, with `layerIndex` set to the one it is drawing, and that
// instance is a real scene that really renders. See `showOneLayer` below.

import QtQuick

Item {
    id: style

    // Space reserved from the client, once, for the whole style.
    //
    // Not per layer: the client is placed once and every layer sees the same
    // client rect, so three layers each declaring insets would be three
    // answers to one question.
    property Insets insets: Insets {}

    // What the style needs from the machine it is loaded on, and is refused
    // for wanting if it cannot have it.
    //
    // The software scene graph does not implement `ShaderEffect` and `Canvas`
    // appears not to paint on it; the GPU path does both. So which QML is
    // legal depends on which path Qt came up on, and without this a style
    // written on one hands a white rectangle to the other in silence.
    // `["gpu"]` is the only term today. Absent means portable.
    //
    // A list rather than a flag, so `["gpu", "effects/2"]` is versioning
    // through the same mechanism rather than a second one.
    //
    // Read by `style::load`, which refuses the bundle when a term cannot be
    // had — including one it has never heard of, since a style naming that was
    // written against a later build and the likeliest reading is "there is
    // something here you do not know how to draw". Leaving a term out is how a
    // style says it would merely *like* something.
    property list<string> requires: []

    // Reserved. Declared now so a style folder written today does not change
    // shape when client treatment is built — both of these become effects
    // that declare `inputs: self`, and neither is expressible until a frame
    // can be split into passes. The compositor ignores them.
    property ClientTreatment client: ClientTreatment {}

    // The Layer children, in declaration order. Read by the compositor.
    //
    // A `list<Item>` and the default property, so layers are written as plain
    // children. Note that items assigned to a list property are *not* parented
    // — `style.children` stays empty — which is correct for a manifest, and is
    // why `showOneLayer` has to parent rather than merely show.
    default property list<Item> layers

    // Which of the layers above this instance is drawing. -1 is the manifest.
    //
    // Set by the compositor at creation, through `createWithInitialProperties`,
    // so it is already right when `Component.onCompleted` runs. An inline layer
    // has no file of its own, so its scene *is* this file loaded again with
    // this property pointing at one child.
    property int layerIndex: -1

    // What the compositor tells a frame about the window it belongs to.
    //
    // Declared here so an inline layer can bind to them — `Frame.qml` in a
    // bundle declares the ones it uses for the same reason, since a delegated
    // layer is its own scene with no parent to read them off. `Decoration::tell`
    // writes all of these on every layer of a style, whichever way it was
    // written.
    property string title: ""
    property bool focused: false
    property bool pointerInside: false
    property int contentWidth: 0
    property int contentHeight: 0

    // Where the window's own rectangle is inside this scene.
    //
    // A layer is laid out on its **canvas** — the pane's outer rect grown by
    // the `bleed` that layer declared — so `anchors.fill: parent` covers the
    // canvas and not the window. These four say where the window is within it:
    // its top-left corner is at (bleedLeft, bleedTop), and it is paneWidth by
    // paneHeight. A border straddling the window's top edge is drawn around
    // `y: bleedTop`; the strip above that is over whatever is behind the
    // window.
    //
    // `bleedLeft` and `bleedTop` are written once, at build, because a declared
    // bleed never changes. The two sizes are written whenever the window is
    // resized. All four are zero for a layer that declared no bleed, which is
    // why content written against them is also correct without one.
    //
    // Two of the four sides and not four, deliberately: `bleedRight` is
    // `width - bleedLeft - paneWidth` and `bleedBottom` is its twin, and a
    // second spelling of a number QML can already work out is a second thing to
    // keep in step.
    property int bleedLeft: 0
    property int bleedTop: 0
    property int paneWidth: 0
    property int paneHeight: 0

    // And the two that go the other way: what a button under the pointer says
    // about itself, and what it asked for. The compositor reads `onButton` and
    // *takes* `action`, clearing it — see `Decoration::take_action`.
    property bool onButton: false
    property string action: ""

    Component.onCompleted: style.showOneLayer()
    onLayerIndexChanged: style.showOneLayer()

    // Put the one layer this scene was built for into the scene graph.
    //
    // **Driven from here, down.** The obvious spelling is a `visible:` binding
    // on `Layer` asking its parent which index it is, and it is a measured
    // no-op: an item assigned to a `list<Item>` gets a QObject parent and not a
    // visual one, and QML's `parent` is `parentItem()` — so `parent` is null
    // for every layer in here, the binding is always true, and every inline
    // layer would draw its siblings. Measured on these very types: `layers` 3,
    // `children` 0.
    //
    // Which is also why this *parents*, and does not only set `visible`.
    // Nothing in `layers` is in the scene graph until something puts it there,
    // so hiding two of three and parenting none draws nothing at all. Both
    // halves are measured, each as its own control over
    // `every_layer_is_its_own_scene_and_draws_only_itself`:
    //
    //   hidden, never parented   ->  every layer reads transparent
    //   parented, never hidden   ->  every layer shows all three
    //
    // `visible = false` is what hides a sibling once it is parented; the
    // `parent = null` beside it is not the mechanism and is measured not to be
    // — with everything parented and hidden by `visible` alone the pictures are
    // right. It is there so a layer this scene will never draw is out of the
    // scene graph rather than merely invisible inside it.
    //
    // Anchored to fill, so `anchors.fill: parent` inside a layer resolves to
    // the canvas the compositor sized this scene to.
    function showOneLayer() {
        // A manifest. `style::load` builds one at 1x1, reads what it declares
        // and drops it, so there is nothing to put on screen and nothing here
        // should touch the layers it is about to be asked about.
        if (style.layerIndex < 0) {
            return;
        }
        for (let i = 0; i < style.layers.length; ++i) {
            const layer = style.layers[i];
            if (i !== style.layerIndex) {
                layer.visible = false;
                layer.parent = null;
                continue;
            }
            layer.visible = true;
            layer.parent = style;
            layer.anchors.fill = style;
        }
    }
}
