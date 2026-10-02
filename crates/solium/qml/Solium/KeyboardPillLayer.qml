// The keyboard pill inside a pane, at the focused text field's caret: a layer
// in front of the client, for a pane style to add with one line.
//
//     PaneStyle {
//         Layer { depth: "frame"; source: "Frame.qml" }
//         KeyboardPillLayer {}
//     }
//
// It reads two things the compositor tells every layer, `caret` (where the
// focused field's caret is, in the pane's own space, when this pane's window
// has it) and `values` (what the configuration hands every pane), and draws
// `KeyboardPill` just below the caret, or above it when there is no room,
// inside the pane. Because it is part of the pane, it moves, scales and fades
// with its window, presentation transforms included.
//
// Whether it shows anything is the configuration's: `lua/keyboard_indicator.lua`
// hands `values.keyboard_indicator = { show, cue }`, and with `show` anything
// but true this layer draws nothing. A pane style without this line simply
// has no pill; `keyboard.indicator.show = "surface"` draws it on screen
// instead, and needs nothing from the style.
//
// `crates/solium/tests/scenarios/keyboard-pane-drawn.lua` draws it in the
// shipped `top` style and reads its pixels: below the caret, above it near
// the pane's bottom, gone with the caret, and nothing while `show` is not
// true (`scenario::tests::every_scenario_on_the_qt_thread_passes`).
//
// A layer of its own because it draws over the client, and a layer is where a
// style draws over the client (`panes/README.md`, "Layers"): the `frame`
// layer of most styles copies only its own band in software. It costs a
// scene per window, which idles at a flag read; a style that leaves the line
// out pays nothing.

import QtQuick

Layer {
    id: layer

    depth: "above"
    name: "keyboard"

    // The `PaneStyle` this inline layer is drawn in, once it has put it on
    // screen: what `caret` and `values` are written on.
    readonly property Item pane: parent
    readonly property var caret: layer.pane && layer.pane.caret ? layer.pane.caret : ({ valid: false })
    readonly property var settings: layer.pane && layer.pane.values ? layer.pane.values.keyboard_indicator : undefined
    readonly property bool wanted: !!layer.settings && layer.settings.show === true

    // The gap between the caret and the capsule.
    readonly property int gap: 6

    Item {
        anchors.fill: parent
        // Gone at once when the field goes, or the window loses the keyboard.
        visible: layer.wanted && layer.caret.valid

        KeyboardPill {
            id: pill

            cue: layer.wanted ? layer.settings.cue : ({})
            accepts: layer.wanted && layer.caret.valid

            readonly property real below: layer.caret.y + layer.caret.height + layer.gap - pill.margin
            readonly property real above: layer.caret.y - layer.gap - pill.capsuleHeight - pill.margin
            readonly property bool fits: pill.below + pill.height - pill.margin <= layer.height

            x: Math.max(-pill.margin, Math.min(layer.width - pill.width + pill.margin,
                Math.round(layer.caret.x + layer.caret.width / 2 - pill.width / 2)))
            y: Math.round(pill.fits ? pill.below : Math.max(-pill.margin, pill.above))
        }
    }
}
