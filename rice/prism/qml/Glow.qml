// A soft circular light, built out of rectangles.
//
// There is no radial gradient here and there cannot be one. `Canvas` — the
// obvious way to draw this — never paints in the compositor's QML host: the
// scene is rasterised offscreen without the scene-graph render loop that
// QQuickCanvasItem needs, so `onPaint` is simply never called and what lands
// on screen is the canvas's uninitialised white. Rectangle gradients are
// linear only, and fade along one axis.
//
// So the falloff is accumulated instead of drawn: concentric circles, each
// barely visible on its own, stacked. Alpha compounds as 1-(1-a)^n, which is a
// curve rather than a ramp — the same shape a radial gradient has, arrived at
// from the other end. Twenty rings at three per cent is smooth at this size;
// the banding only shows up if you raise the alpha until it is no longer soft
// light.
//
// It costs nothing after the first frame. Nothing here animates, so the scene
// stops being rasterised a few identical renders later and an idle desktop
// pays a comparison rather than a repaint.

import QtQuick

Item {
    id: glow

    property color tint: "#ffffff"
    // Roughly the alpha at the centre. Compounded, not summed.
    property real strength: 0.4
    property int rings: 30

    Repeater {
        model: glow.rings
        delegate: Rectangle {
            required property int index

            // Largest first, so the smallest — and therefore brightest — ends
            // up on top. Drawn the other way round the light has a hole in it.
            readonly property real t: (glow.rings - index) / glow.rings

            anchors.centerIn: parent
            width: glow.width * t
            height: glow.height * t
            radius: Math.min(width, height) / 2
            color: glow.tint
            // Uniform, and deliberately so. Weighting the inner rings up was
            // the first attempt and it made each ring's edge a different size
            // of step, which is exactly what you see as banding. Flat alpha
            // means every edge is the same one per cent, and one per cent is
            // below what the eye picks out of a gradient.
            opacity: 1 - Math.pow(1 - glow.strength, 1 / glow.rings)
        }
    }
}
