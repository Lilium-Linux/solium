// The pointer, drawn by the design system like everything else we draw.
//
// A cursor is chrome: it belongs to the same theme as the titlebars and the
// shell, so it lives here rather than being loaded from whatever icon theme
// happens to be installed. Change `Theme.text` and the pointer changes with the
// window frames, because there is only one of it.
//
// Rendered once into a buffer and reused — it is the same picture every frame.
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
    width: 24
    height: 24

    // The hotspot is the tip, at (0, 0). `cursor.rs` positions the buffer by
    // subtracting it, so a pointer that reports (x, y) has its point exactly
    // there and not a few pixels down and to the right.
    Shape {
        anchors.fill: parent
        preferredRendererType: Shape.CurveRenderer

        // The classic arrow: a tall thin wedge with a tail, as one closed path
        // so the outline is continuous. Outlined in the dark surface colour and
        // filled with the text colour, so it stays legible over a light window
        // and a dark one alike — a cursor that vanishes over half the screen is
        // worse than one that does not match.
        ShapePath {
            fillColor: Theme.text
            strokeColor: Theme.surfaceSunken
            strokeWidth: 1.6
            joinStyle: ShapePath.RoundJoin

            startX: 1; startY: 1
            PathLine { x: 1;    y: 16.5 }
            PathLine { x: 5.2;  y: 12.7 }
            PathLine { x: 8.1;  y: 19.3 }
            PathLine { x: 11.2; y: 17.9 }
            PathLine { x: 8.4;  y: 11.5 }
            PathLine { x: 13.9; y: 11.1 }
            PathLine { x: 1;    y: 1 }
        }
    }
}
