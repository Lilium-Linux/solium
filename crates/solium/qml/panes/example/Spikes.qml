// The `above` layer of the example bundle: something that paints outside the
// pane, which is the thing a single decoration file cannot do.
//
// Its canvas is the pane's outer rect grown by the bleed the Layer declared —
// 48px at the top and nothing on the other three sides — so this Item is 48px
// taller than the window, and the window's own top edge is 48px down from
// `y: 0`. The diamonds straddle that line: the half above it reaches over
// whatever is behind the window, and the half below it is drawn over the
// client, which is what `depth: "above"` means.
//
// The half above the window is the feature. It is drawn at the pane's own
// stacking depth, so it covers the window *behind* this one and is covered by
// any window in front — a background window's spikes do not appear over the
// window being typed in.
//
// They stop at the edge of the canvas. Bleed is a promise, not a request: a
// layer is clipped to what it asked for, or one style can quietly force a
// full-screen repaint on every frame. And the strip takes no clicks — a spike
// over the next window must not eat that window's clicks.
//
// Plain rotated rectangles rather than `Canvas` or `ShaderEffect`, so that the
// bundle stays portable and can honestly declare `requires: []`.
//
// Nothing here states 48 twice. The `Layer` in `Pane.qml` declares the bleed
// and the compositor hands it back as `bleedTop`, so the canvas this is laid
// out on and the offset the diamonds are placed at are the same number by
// construction — which is the only way a file positioned against the edge of
// its own canvas can be correct at more than one bleed.

import QtQuick
import Solium

Item {
    id: spikes

    // Set by the compositor: how far the canvas extends past the pane, so the
    // content can find the window's own corner inside it.
    //
    // Zero rather than 48, like `contentWidth` and for the same reason: these
    // are in-properties. A plausible default is a file that looks right when
    // nothing set it, and the one thing worth being able to see is whether it
    // was told.
    property int bleedTop: 0
    property int bleedLeft: 0

    Row {
        x: spikes.bleedLeft
        y: 0
        spacing: 18

        Repeater {
            model: 12

            Item {
                width: 24
                height: spikes.bleedTop * 2

                // A square turned on its corner. The top vertex sits at y: 0,
                // the widest point at the window's own top edge.
                Rectangle {
                    width: 34
                    height: 34
                    x: -5
                    y: spikes.bleedTop - 17
                    rotation: 45
                    color: Theme.accent
                    opacity: 0.7
                }
            }
        }
    }
}
