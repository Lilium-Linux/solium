// Liquid glass: the window and its bar as one rounded piece of dark glass,
// the bar along the bottom.
//
//     SOLIUM_PANE=glass
//
// Three files, one job each:
//
//   Pane.qml     what the frame reserves, the corners, and the layers;
//   Frame.qml    the bar: the window buttons at the left, the title after
//                them, the application's icon at the right, and no fill;
//   effects.lua  two rules: the shipped `glass` effect behind the whole pane,
//                and the shipped `shade` in front of it while the window does
//                not have the keyboard, a thin dark edge just inside its
//                rounded outline, round all four sides and the corners,
//                fading in and out with the focus.
//
// **One shape, three things drawing it.** The compositor cuts the client's top
// two corners, which are the window's: `client.radius` below. The client's
// bottom two meet the bar, so they are square, and the bar's own bottom two,
// the window's, are rounded by `Frame.qml` to the same number, which every
// layer is told as `clientRadius`. And the glass behind the pane is cut to the
// whole window rounded at its largest corner, that same number again, so the
// glass, the client and the bar have one outline, with one rim round it and no
// seam where the bar meets the client. The shade follows that outline too.
//
// **Nothing here is stretched as the window changes size.** The shade is an
// effect drawn from the window's shape on the GPU, and the bar is drawn again
// at every size, into the buffer it already has: it declares no `resizeHz`
// (`Solium/Layer.qml`), the knob that would hold it at a picture stretched to
// the window between sizes.
//
// The numbers here, the ones at the top of `Frame.qml` and the shade's in
// `effects.lua` are the style's to change: copy this folder to
// ~/.config/solium/qml/panes/glass/, change them there, and your copy is used
// in place of this one.

import QtQuick
import Solium

PaneStyle {
    // The bar's height, in logical pixels: the band reserved under the
    // client. The compositor hands it to the layer as `insetBottom`.
    insets.bottom: Theme.titlebarHeight

    // Ordinary QML on the software path: no shader, no Canvas. The glass is
    // an effect behind the pane, the compositor's to draw.
    requires: []

    // The window's corners, in logical pixels: the client's top two, cut as
    // it is drawn, at no extra cost. Its bottom two are square, where the bar
    // meets it; the glass behind the bar is rounded to this number, and so is
    // the shade.
    client.radius: 16
    client.radiusBottomLeft: 0
    client.radiusBottomRight: 0

    // At `frame`, inside the band it was given: nothing it draws covers the
    // client. `bar` is the name a `region:titlebar` rule's slots go around.
    Layer {
        depth: "frame"
        name: "bar"
        source: "Frame.qml"
    }

    // The keyboard pill at the focused text field's caret, when the
    // configuration says so (`keyboard.indicator.show = "pane"`). Delete the
    // line and this style draws none. See `Solium/KeyboardPillLayer.qml`.
    KeyboardPillLayer {}
}
