// The pointer, drawn by the design system like everything else we draw.
//
// A cursor is chrome: it belongs to the same theme as the titlebars and the
// shell, so it lives here rather than being loaded from whatever icon theme
// happens to be installed. Change `Theme.text` and the pointer changes with the
// window frames, because there is only one of it.
//
// Rendered once per size into a buffer and reused — it is the same picture
// every frame.
//
// Drawn with `Shape` and *not* with `Canvas`. A Canvas paints on a signal, and
// nothing here delivers that signal: the scene is driven by hand through
// `QQuickRenderControl`, so `onPaint` never ran and the cursor was a fully
// transparent 24x24 buffer for the life of the session. That is not a subtle
// failure — it is an invisible pointer, which from the other side of the screen
// is indistinguishable from input being dead.

import QtQuick
import QtQuick.Shapes
import Solium

Item {
    id: root

    // A default rather than the size, and the distinction is the whole of
    // #81 working. `host.cpp` sets this item's width and height to the
    // scene's *logical* size on every resize — the configured `cursor.size`,
    // or `XCURSOR_SIZE`, whatever `cursor::theme::Settings` resolved — so
    // whatever is written here is overwritten before the first frame. It is
    // the size the file draws at when opened on its own.
    width: 24
    height: 24

    // **The arrow is laid out in a 24-unit square and scaled to fit, rather
    // than drawn in absolute units.** `cursor.rs` sizes the scene to the
    // configured size and `host.cpp` puts that on this item, so literal
    // coordinates here made the setting half inert in one direction and
    // destructive in the other: `cursor = { size = 48 }` only padded the
    // buffer around a 24-pixel arrow, and `XCURSOR_SIZE=16` cut the tail off
    // one, because the path ran to y≈19.3 in a render target 16 pixels tall.
    // A clipped pointer on the arm this module calls its floor — the one a
    // machine with no theme installed is guaranteed to get — is the
    // invisible-pointer failure wearing a smaller hat.
    //
    // `Math.min` and not the width alone: the scene is square everywhere the
    // compositor builds it, and an item that somehow is not still draws an
    // arrow that fits inside itself rather than one that spills out of it.
    readonly property real unit: Math.min(width, height) / 24

    // Scaled with everything else. A hairline outline around a 96-pixel arrow
    // would be an arrow with no outline, which is the legibility the fixed
    // colours below exist for.
    readonly property real stroke: 1.6 * unit

    // Half the stroke, which is exactly how far the outline extends beyond the
    // path, so the drawn pointer's bounding box starts at (0, 0) and ends at
    // 19.9 units of 24 — never clipped, at any size, and its point is in the
    // corner at every size rather than at one.
    //
    // The hotspot is that tip. `cursor.rs` positions the buffer by subtracting
    // `HOTSPOT`, which is (0, 0), so a pointer that reports (x, y) has its
    // point there and not a few pixels down and to the right. Scaling this
    // inset with the size is what keeps that true: a constant one here would
    // be a quarter of a pixel out at 24 and four pixels out at 256.
    readonly property real inset: stroke / 2

    Shape {
        anchors.fill: parent
        preferredRendererType: Shape.CurveRenderer

        // The classic arrow: a tall thin wedge with a tail, as one closed path
        // so the outline is continuous. The numbers are the same drawing it
        // always was, restated from the tip rather than from (1, 1) so that
        // `inset` is the only thing holding the outline off the edge.
        //
        // These two colours are deliberately NOT from `Theme`. Every other
        // thing the compositor draws sits on a surface the theme owns; the
        // pointer sits on whatever a client happened to draw, so it has to be
        // legible against black, against white, and against a photograph. A
        // white body with a dark outline is the answer every desktop arrived
        // at, and it is fixed rather than themed for that reason.
        //
        // It used to read `Theme.text` over `Theme.surfaceSunken`, which was
        // the same pair by accident: the palette was dark, so `text` was
        // near-white. Turning the theme light would have made the pointer black
        // on black — a cursor that vanishes over half the screen being rather
        // worse than one that does not match.
        ShapePath {
            fillColor: "#ffffff"
            strokeColor: "#1c1c1e"
            strokeWidth: root.stroke
            joinStyle: ShapePath.RoundJoin

            startX: root.inset
            startY: root.inset
            PathLine { x: root.inset + 0.0 * root.unit;  y: root.inset + 15.5 * root.unit }
            PathLine { x: root.inset + 4.2 * root.unit;  y: root.inset + 11.7 * root.unit }
            PathLine { x: root.inset + 7.1 * root.unit;  y: root.inset + 18.3 * root.unit }
            PathLine { x: root.inset + 10.2 * root.unit; y: root.inset + 16.9 * root.unit }
            PathLine { x: root.inset + 7.4 * root.unit;  y: root.inset + 10.5 * root.unit }
            PathLine { x: root.inset + 12.9 * root.unit; y: root.inset + 10.1 * root.unit }
            PathLine { x: root.inset + 0.0 * root.unit;  y: root.inset + 0.0 * root.unit }
        }
    }
}
