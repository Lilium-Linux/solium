// A style for looking at bleed, and nothing else.
//
// One layer, behind the client, reaching 80px past the pane on every side in a
// colour nothing else on screen uses. Open two windows and overlap them: the
// lower window's band runs *under* the upper window, because bleed follows the
// pane's stacking rather than jumping to the front. Drag one to a screen edge
// and the band is cut off by the screen, not by the window.
//
// There is no titlebar on purpose. `insets` reserves nothing, so the client
// fills the pane and every coloured pixel you can see is bleed.
//
//     SOLIUM_PANE=bleedy
//
// What to look for, in order:
//   * a band all the way round every window, outside it
//   * the band of a background window passing *beneath* the focused one
//   * nothing beyond the 80px ring. The green outline below is drawn 200px
//     past the canvas on every side and must be invisible: a layer is clipped
//     to what it declared, and that control lives inside the fixture

import QtQuick
import Solium

PaneStyle {
    // Nothing reserved: no titlebar, no inset. Every visible pixel is bleed.
    insets.top: 0

    requires: []

    Layer {
        depth: "behind"
        name: "band"
        bleed: 80

        // Fills the whole canvas — which is the pane grown by 80 on each side,
        // so what shows is the 80px ring the client does not cover.
        Rectangle {
            anchors.fill: parent
            color: "#e05561"

            // Deliberately larger than the canvas. Nothing should appear past
            // the 80px ring: a layer is clipped to what it declared, and this
            // is the control for that sitting inside the fixture.
            Rectangle {
                anchors.centerIn: parent
                width: parent.width + 400
                height: parent.height + 400
                color: "transparent"
                border { width: 20; color: "#00ff00" }
            }
        }
    }
}
