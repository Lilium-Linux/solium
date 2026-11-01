// Windows with no titlebar: a two-pixel border, violet on the window
// with the keyboard. For the tiling layouts (see ~/.config/solium/looks.lua);
// floating windows get rounded-violet, which has a bar to drag them by.
//
// The border sits in a band the style reserves, rather than in bleed, because
// presses reach a layer only inside that band: it is also the grip for
// resizing, as in the shipped `border`.

import QtQuick
import Solium
import "../../accent" as Accent

PaneStyle {
    insets.top: 1
    insets.right: 1
    insets.bottom: 1
    insets.left: 1

    requires: []

    // Rounded as the floating frame is (rounded-violet), so a window looks
    // the same tiled and floating. On the laptop (scale 1.5) this pass
    // cropped Chromium and Electron windows at the sides; at this PC's scale
    // 1 it does not. Back to 0 there if it does again.
    client.radius: 12

    Layer {
        depth: "frame"
        name: "border"
        source: "Frame.qml"
    }

    // Alt+Tab's app icon, hanging off the bottom-left corner (recents.lua).
    Layer {
        depth: "above"
        name: "recents-badge"
        bleed: { "left": 24, "bottom": 24 }
        source: "Badge.qml"
    }

    Accent.PillLayer {}
}
