// Sine waves flowing along all four edges of the window, outside it.
//
// Four `Band`s, each rotated about its own top-left so that the solid side of
// the wave lies against the pane and the crests break outwards. Every band
// overhangs its edge by the reach at both ends, so the four of them meet at
// the corners instead of leaving four notches.

import QtQuick

Item {
    id: sea

    // Set by the compositor. A layer's content owns this contract.
    property string title: ""
    property bool focused: false
    property bool pointerInside: false
    property int contentWidth: 0
    property int contentHeight: 0
    property int paneWidth: 0
    property int paneHeight: 0

    // Where the pane's top-left corner sits inside this canvas. The canvas is
    // the pane grown by the bleed, so without these every band would be drawn
    // `bleedLeft` to the left of the window it belongs to.
    property int bleedLeft: 0
    property int bleedTop: 0

    // How far out the waves run. Under the bleed Pane.qml declared, because
    // bleed is a hard clip: a crest past it is cut, not drawn.
    readonly property int reach: 44

    // The pane's rectangle inside this canvas.
    readonly property real px: sea.bleedLeft
    readonly property real py: sea.bleedTop

    // Unfocused windows stop moving and fade back, which is also what stops
    // the compositor drawing for them.
    opacity: sea.focused ? 1.0 : 0.35

    // Top: no rotation. The band's solid edge is already its bottom.
    Band {
        span: sea.paneWidth + sea.reach * 2
        depth: sea.reach
        running: sea.focused
        x: sea.px - sea.reach
        y: sea.py - sea.reach
    }

    // Bottom: a half turn, so the solid edge points back up at the pane.
    Band {
        span: sea.paneWidth + sea.reach * 2
        depth: sea.reach
        running: sea.focused
        transformOrigin: Item.TopLeft
        rotation: 180
        x: sea.px + sea.paneWidth + sea.reach
        y: sea.py + sea.paneHeight + sea.reach
    }

    // Left: three quarters, which puts the solid edge on the right of the
    // strip -- against the pane -- and the crests out to the left.
    Band {
        span: sea.paneHeight + sea.reach * 2
        depth: sea.reach
        running: sea.focused
        transformOrigin: Item.TopLeft
        rotation: 270
        x: sea.px - sea.reach
        y: sea.py + sea.paneHeight + sea.reach
    }

    // Right: a quarter turn, the mirror of the left.
    Band {
        span: sea.paneHeight + sea.reach * 2
        depth: sea.reach
        running: sea.focused
        transformOrigin: Item.TopLeft
        rotation: 90
        x: sea.px + sea.paneWidth + sea.reach
        y: sea.py - sea.reach
    }
}
