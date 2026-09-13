// Sine waves flowing along all four edges of the window, outside it.
//
// Four `Band`s, one per edge, the side ones rotated a quarter turn. Each band
// is drawn *outwards* from the pane's edge into the bleed, so the window keeps
// every pixel it would have had with no decoration at all.
//
// The corners overlap on purpose: each band is longer than its edge by the
// bleed at both ends, so the waves meet and cross at the corners instead of
// stopping short and leaving four notches.

import QtQuick
import Solium

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

    // How far out the waves run. Under the bleed declared in Pane.qml, because
    // bleed is a hard clip and a crest past it is cut rather than drawn.
    readonly property int reach: 44

    // Back to front. Translucent so the bands that cross at the corners read
    // as depth rather than as one flat patch.
    readonly property var inks: sea.focused
        ? [Qt.rgba(0.13, 0.13, 0.13, 0.85),
           Qt.rgba(0.00, 0.38, 0.75, 0.75),
           Qt.rgba(0.55, 0.70, 0.85, 0.70)]
        : [Qt.rgba(0.75, 0.75, 0.75, 0.60),
           Qt.rgba(0.85, 0.85, 0.85, 0.55),
           Qt.rgba(0.92, 0.92, 0.92, 0.50)]

    // The pane's rectangle inside this canvas.
    readonly property real px: sea.bleedLeft
    readonly property real py: sea.bleedTop

    // Each band runs the length of its edge plus the reach at both ends, so
    // the four of them cover the corners between them.
    Band {
        span: sea.paneWidth + sea.reach * 2
        depth: sea.reach
        running: sea.focused
        inks: sea.inks
        // Rotated about its own top-left, so the strip hangs off the edge it
        // belongs to rather than swinging across the window.
        transformOrigin: Item.TopLeft
        rotation: 180
        x: sea.px + sea.paneWidth + sea.reach
        y: sea.py
    }

    Band {
        span: sea.paneWidth + sea.reach * 2
        depth: sea.reach
        running: sea.focused
        inks: sea.inks
        transformOrigin: Item.TopLeft
        x: sea.px - sea.reach
        y: sea.py + sea.paneHeight
    }

    Band {
        span: sea.paneHeight + sea.reach * 2
        depth: sea.reach
        running: sea.focused
        inks: sea.inks
        transformOrigin: Item.TopLeft
        rotation: 90
        x: sea.px
        y: sea.py - sea.reach
    }

    Band {
        span: sea.paneHeight + sea.reach * 2
        depth: sea.reach
        running: sea.focused
        inks: sea.inks
        transformOrigin: Item.TopLeft
        rotation: 270
        x: sea.px + sea.paneWidth
        y: sea.py + sea.paneHeight + sea.reach
    }
}
