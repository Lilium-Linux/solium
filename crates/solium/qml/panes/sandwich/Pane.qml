// A style for looking at depth, and nothing else.
//
// Two layers and no bleed. One behind the client, one over it, in colours that
// cannot be confused. The client sits *between* them — which is the thing a
// single QML file can never do, and the reason layers are separate scenes.
//
//     SOLIUM_DECORATION=sandwich
//
// What to look for:
//   * a solid blue margin around the window — that is the `behind` layer,
//     visible only where the client does not cover it
//   * a yellow bar across the top *over* the client, not beside it: text and
//     cursor in a terminal run underneath it
//   * the order never changing when you focus or move the window

import QtQuick
import Solium

PaneStyle {
    // A wide inset so the `behind` layer has somewhere to show. The client is
    // placed inside what is left, so this margin is the layer, not padding.
    insets.top: 28
    insets.left: 16
    insets.right: 16
    insets.bottom: 16

    requires: []

    // Under the client. Fills the pane, and shows in the inset margin.
    Layer {
        depth: "behind"
        name: "under"

        Rectangle {
            anchors.fill: parent
            color: "#3b6ea5"
        }
    }

    // Over the client. Half-height so it is unmistakably *on top of* the
    // window's own pixels rather than next to them.
    Layer {
        depth: "above"
        name: "over"

        Rectangle {
            anchors { left: parent.left; right: parent.right; top: parent.top }
            height: 28
            color: Qt.rgba(0.86, 0.71, 0.05, 0.75)
        }
    }
}
