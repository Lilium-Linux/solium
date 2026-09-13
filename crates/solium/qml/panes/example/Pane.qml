// The style bundle format, written out once so there is something to read.
//
// Nothing loads this. It is here because `PaneStyle` and `Layer` are a format
// before they are a feature, and a format with no instance of it is a set of
// property declarations nobody has tried to write against. It exercises every
// property both types have — all three depths, both spellings of `bleed`,
// inline content and delegated content, `insets`, `requires`, and the reserved
// `client` group — so that
//
//     solium --check-qml crates/solium/qml/panes/example/Pane.qml
//
// says `ok` about the whole surface rather than about one corner of it.
//
// It is deliberately not a copy of a shipped decoration: `decorations/top.qml`
// still ships and is still the one being drawn, and a second copy of it living
// here would drift from the original with nothing to notice.

import QtQuick
import Solium

PaneStyle {
    // Reserved from the client once, for the whole style — not per layer. The
    // client is placed once and all three layers below see the same client
    // rect, so three answers to this would be three answers to one question.
    //
    // From the theme rather than a literal, which is the point of declaring it
    // here: `Frame.qml` sizes its bar from the same constant, so the space
    // reserved and the space painted cannot disagree.
    insets.top: Theme.titlebarHeight

    // Nothing here needs a shader, so this style is portable and says nothing.
    // A style using `ShaderEffect` or `Canvas` would declare `requires:
    // ["gpu"]` and be refused at load on the software path rather than handing
    // it a white rectangle.
    requires: []

    // Reserved and unread. Here to show that a style written today already has
    // somewhere to put these.
    client.radius: 0
    client.shadow.blur: 0
    client.shadow.opacity: 0

    // Under the client. Reaches 24px past the pane on every side, which is what
    // a glow costs: the canvas is 48px wider and taller, and all of it is
    // rasterised and uploaded whenever the layer changes.
    Layer {
        depth: "behind"
        name: "glow"
        bleed: 24

        Rectangle {
            anchors.fill: parent
            radius: 12
            color: Qt.rgba(Theme.accent.r, Theme.accent.g, Theme.accent.b, 0.25)
        }
    }

    // Where decorations are today: inside the pane, no bleed, content in its
    // own file. The syntax does not change between inline and delegated.
    Layer {
        depth: "frame"
        name: "bar"
        source: "Frame.qml"
    }

    // Over the client, and past the top edge only. Per-side because this layer
    // paints upward and should not pay to rasterise 48px of empty canvas on
    // three sides it never touches.
    Layer {
        depth: "above"
        name: "spikes"
        bleed: { "top": 48 }
        source: "Spikes.qml"
    }
}
