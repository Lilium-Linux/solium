// A window whose own corners are cut. The style that turns the client pass on.
//
//     SOLIUM_PANE=rounded
//
// Every other bundle here decorates *around* the client. This one changes the
// client's own pixels, and `client.radius` is the only property that does:
// a pane declaring a non-zero one is rendered into a texture of its own and
// drawn back through a fragment program, one extra pass per frame, for that
// window only. `crates/solium/src/pass.rs` is that pass and
// `crates/effects/src/fragment.rs` is the program.
//
// **It ships because the feature could not otherwise be looked at.** Twelve of
// the bundles here declare no radius at all and `example/` declares `0`, which
// is *no effect* rather than a radius of nothing — so until this folder existed
// there was nothing on a running machine that could draw a rounded corner, and
// a shader nobody can see is a shader nobody can check. `example/` is the
// format written out in full and is asserted to cost nothing; this is the one
// that costs something, which is why it is a folder of its own rather than one
// more line in that file.
//
// **The two halves of the seam are both in this folder, drawn by different
// things, and that is the design rather than an accident.** The compositor
// rounds the CLIENT, because those pixels are the application's and only a
// fragment program can mask them. `Frame.qml` rounds ITSELF, with
// `Rectangle.radius`, because those pixels are Qt's and Qt has rounded a
// rectangle for free since it had a scene graph. The only thing that crosses
// between them is the number, which the compositor writes onto every layer of
// every style as `clientRadius`.
//
// `panes/flush/` is the other way the same two halves can meet: there the
// client's top corners are squared, the bar's rounded top *is* the window's
// top, and the two meet on a flat seam. Here they interlock instead. Both
// exist because the per-corner keys make the choice a style's rather than the
// compositor's.

import QtQuick
import Solium

PaneStyle {
    // A band for the bar, as `top` reserves. See `Frame.qml` for why the bar
    // drawn in it is taller than the band is.
    insets.top: Theme.titlebarHeight

    // Nothing in this *scene* needs a shader. The fragment program that cuts
    // the corners is the compositor's own and is not a QML capability, so this
    // style is as portable as any other here: it draws on the software path
    // exactly as it draws on the GPU one.
    requires: []

    // Logical pixels, and the only line in this bundle that costs anything.
    // `pass::physical_radius` multiplies it by the monitor's own scale on the
    // way to the shader, so a window dragged onto a HiDPI screen keeps the
    // corner it had rather than half of it.
    //
    // All four corners, which is this style's whole shape: the top two are cut
    // *inside* the window, under the ends of the bar, and what shows through
    // them is the bar. `panes/flush/` squares those two instead and gets the
    // other picture.
    client.radius: 12

    // **`behind`, and that one word is the difference between this style
    // working and this style covering the client's first line of text.**
    //
    // The bar is taller than the band it was given — see `Frame.qml` — because
    // it has to reach down past the seam to be *in* the two notches the shader
    // cuts out of the client's top corners. A bar that reaches down there and
    // is drawn at `frame` is drawn OVER the client, and the `clientRadius`
    // rows it overhangs by are the client's top rows: in a terminal, the top
    // half of the prompt.
    //
    // Lowering it under the client keeps both halves. The overhang still fills
    // the notches, because a rounded client is no longer opaque there and
    // `pass::opaque_of` gives those sides up — so what the shader cut away is
    // exactly what the bar shows through. And it no longer covers anything,
    // because every pixel the client does draw is drawn on top of it.
    //
    // The alternative is to stop the bar at the seam, which also uncovers the
    // client and is *wrong*: with nothing behind the notches they show the
    // wallpaper, and the window reads as a bar resting on a chipped rectangle.
    // The two are indistinguishable while the bar is a flat colour and are not
    // once it is a gradient, so this is depth and not height on purpose.
    Layer {
        depth: "behind"
        name: "bar"
        source: "Frame.qml"
    }
}
