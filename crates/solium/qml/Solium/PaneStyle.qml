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
// Nothing in the compositor reads any of this yet.

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
    // Declared here and read by nobody: finding and loading a bundle is not
    // built yet, and a style written today should not have to change shape
    // when it is.
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
    // — `style.children` stays empty — which is correct here: a `PaneStyle` is
    // a manifest and is never itself drawn.
    default property list<Item> layers
}
