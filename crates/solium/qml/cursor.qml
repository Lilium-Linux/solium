// The pointer, drawn by the design system like everything else we draw.
//
// A cursor is chrome: it belongs to the same theme as the titlebars and the
// shell, so it lives here rather than being loaded from whatever icon theme
// happens to be installed. Change `Theme.text` and the pointer changes with the
// window frames, because there is only one of it.
//
// Rendered once into a buffer and reused — it is the same picture every frame.

import QtQuick
import Solium

Item {
    id: root
    width: 24
    height: 24

    // The hotspot is the tip, at (0, 0). `cursor.rs` positions the buffer by
    // subtracting it, so a pointer that reports (x, y) has its point exactly
    // there and not a few pixels down and to the right.
    Canvas {
        anchors.fill: parent
        renderStrategy: Canvas.Immediate
        renderTarget: Canvas.Image

        onPaint: {
            const ctx = getContext("2d");
            ctx.reset();

            // The classic arrow: a tall thin wedge with a tail. Drawn as one
            // path so the outline is continuous.
            ctx.beginPath();
            ctx.moveTo(1, 1);
            ctx.lineTo(1, 16.5);
            ctx.lineTo(5.2, 12.7);
            ctx.lineTo(8.1, 19.3);
            ctx.lineTo(11.2, 17.9);
            ctx.lineTo(8.4, 11.5);
            ctx.lineTo(13.9, 11.1);
            ctx.closePath();

            // Outlined in the dark surface colour and filled with the text
            // colour, so it stays legible over a light window and a dark one
            // alike — a cursor that vanishes over half the screen is worse
            // than one that does not match.
            ctx.fillStyle = Theme.text;
            ctx.strokeStyle = Theme.surfaceSunken;
            ctx.lineWidth = 1.6;
            ctx.lineJoin = "round";
            ctx.stroke();
            ctx.fill();
        }
    }
}
