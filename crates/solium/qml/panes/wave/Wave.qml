// Sixty bars whose heights are a travelling sine. The geometry moves; no
// colour cycles.
//
// It waves upward into the 40px of canvas the layer's `bleed` bought, so the
// crests are outside the window rather than eating space the client owns.

import QtQuick
import Solium

Item {
    id: sea

    // Set by the compositor. Declared here because a layer's content owns this
    // contract, the same way `Frame.qml` does.
    property string title: ""
    property bool focused: false
    property bool pointerInside: false
    property int contentWidth: 0
    property int contentHeight: 0

    // How far above the pane this layer may paint, which is what `bleed` in
    // `Pane.qml` asked for. The compositor sets it.
    property int bleedTop: 0

    // Travels. One turn every 2.4s, and every bar reads it.
    //
    // Bound to `focused` on purpose: an animation that never settles keeps the
    // compositor drawing for as long as it runs, and at 260Hz that is a real
    // cost to pay for a window nobody is looking at.
    property real phase: 0
    NumberAnimation on phase {
        running: sea.focused
        loops: Animation.Infinite
        from: 0
        to: 2 * Math.PI
        duration: 2400
    }

    readonly property int bars: 60

    Repeater {
        model: sea.bars

        Rectangle {
            required property int index

            // Two sines of different periods, so the crest does not read as
            // one repeating tooth.
            readonly property real u: index / (sea.bars - 1)
            readonly property real lift:
                Math.sin(u * 6.0 + sea.phase) * 0.6
                + Math.sin(u * 13.0 - sea.phase * 1.7) * 0.4

            width: Math.max(1, sea.width / sea.bars - 1)
            x: index * (sea.width / sea.bars)
            // Zero height sits flush with the pane's top edge; a tall one
            // reaches up into the bleed.
            height: Math.max(0, sea.bleedTop * 0.5 * (1.0 + lift))
            y: sea.bleedTop - height
            color: sea.focused ? Theme.accent : Theme.edgeInactive
        }
    }
}
