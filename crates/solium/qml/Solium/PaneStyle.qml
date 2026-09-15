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
// `style::load` reads `insets`, `requires`, the Layer children and
// `client.radius`, which becomes an effect on the client's own texture.
// `client.shadow` is still reserved and read by nobody.
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
    //
    // Declared once here and *handed to every layer*: the compositor writes
    // these four back onto each layer's own root as `insetTop`, `insetRight`,
    // `insetBottom` and `insetLeft`, once at build, exactly as it does with
    // `bleedLeft` and `bleedTop`. A delegated layer is its own scene with no
    // parent to read them off, and a bar that has to know how tall its own
    // band is would otherwise need a second copy of the number — which is the
    // disagreement this property being on `PaneStyle` exists to prevent. An
    // inline layer is already inside this object and can read `insets.top`
    // directly; it is handed the flat four as well and is free to ignore them.
    //
    // The same four names a single QML file under `decorations/` declares, and
    // deliberately: there the compositor *reads* them, because that file is
    // the only place a decoration's insets exist. So a decoration converted
    // into a bundle keeps every binding it already had, and the direction the
    // number travels is the only thing that changed.
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

    // What is done to the client's own surface, rather than around it.
    //
    // `radius` is read: `style::load` turns a non-zero one into an effect
    // declaring `inputs: self`. Zero is *no effect* and not an effect that
    // rounds by nothing — the difference is an offscreen pass per window per
    // frame, so a style that leaves this alone costs exactly what it did
    // before the property existed. `shadow` is the same shape and is still
    // reserved: it derives from the node's silhouette rather than masking it,
    // and nothing draws it yet.
    //
    // **Each corner can also be declared on its own**, with `radiusTopLeft`,
    // `radiusTopRight`, `radiusBottomLeft` and `radiusBottomRight`. Each
    // defaults to `radius`, so one key still means all four and the ordinary
    // case is exactly what it was; naming one overrides that corner alone.
    //
    //     client.radius: 12
    //     client.radiusTopLeft: 0
    //     client.radiusTopRight: 0
    //
    // A square top and a rounded bottom, which is `panes/flush/` — the shape a
    // titlebar can sit flush on. Four named keys rather than a list, because a
    // corner is a named thing: `radiusTopLeft` cannot be given in the wrong
    // order, and a style that wants one of them writes one line.
    //
    // **A zero has to be written out and cannot be left implied.** Squaring a
    // corner is half of what these are for, so `0` is a value someone means;
    // "not declared" is therefore spelled `-1` internally, which is the only
    // way the compositor can tell a corner squared on purpose from one that
    // was never mentioned and should follow `radius`. See `ClientTreatment`.
    //
    // Declared once here and *handed to every layer*, exactly as `insets` are
    // and for the same reason: the compositor writes all four back onto each
    // layer's own root as `clientRadiusTopLeft`, `clientRadiusTopRight`,
    // `clientRadiusBottomLeft` and `clientRadiusBottomRight`, in logical
    // pixels, once at build — on every layer of every style, zeroes included.
    //
    // It writes `clientRadius` too, which survives all of this and is the
    // **largest of the four**. That is the number a layer wants when it wants
    // one number: its use is an outward hug, and a hug has to clear the
    // biggest cut or it crosses the curve somewhere. A layer that needs one
    // particular corner reads that corner by name.
    //
    //     property int clientRadius: 0        // set by the compositor
    //     radius: clientRadius + 2            // hug it from outside
    //
    //     property int clientRadiusTopLeft: 0 // or one corner, by name
    //
    // **That split is not arbitrary; it falls out of who drew the pixels.**
    // The client's are the application's, so the compositor masks them with a
    // fragment program — see `pass.rs`. A layer's are Qt's, and Qt rounds a
    // rectangle with one property; a GPU pass to do what `Rectangle.radius`
    // does for free would be absurd. So the compositor rounds the client, QML
    // rounds itself, and the only thing that crosses the seam is the number. A
    // style wanting a rounded border around a *square* client declares no
    // `client.radius` and sets its own `radius` — and pays for no pass, which
    // is the point.
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
