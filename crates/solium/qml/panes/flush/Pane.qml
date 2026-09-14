// A window with a square top and a rounded bar sitting flush on it.
//
//     SOLIUM_PANE=flush
//
// The second of the two ways a titlebar and a rounded window can meet, and the
// one that needs the corners to be declarable apart. `panes/rounded` cuts all
// four of the client's corners and lets the bar show through the top two;
// this one **squares the client's top two** and lets the bar's own rounded top
// be the window's top. The two meet on a flat horizontal seam, and the window
// has one silhouette made of two objects: the bar's curve above, the shader's
// curve below.
//
// **It is the only shipped style that cannot be written with a single
// radius.** `client.radius: 12` alone rounds the client's top corners in the
// middle of the window, under the ends of the bar — the notches `rounded/`
// spends its overhang on. Squaring them is one key per corner, which is what
// `radiusTopLeft` and its three siblings exist for. So this folder is also the
// thing that proves the per-corner keys reach the shader at all: with the two
// overrides dropped it draws `panes/rounded`'s shape with `panes/rounded`'s
// bug, visibly.
//
// The division of labour is the same one `rounded/Pane.qml` spells out: the
// compositor masks the client, because those pixels are the application's;
// QML rounds itself with `Rectangle.radius`, because those are Qt's. What
// differs is only which corners each of them is given.

import QtQuick
import Solium

PaneStyle {
    // A band for the bar, as `top` reserves — and here the bar is exactly that
    // tall, because there is nothing under it to reach for.
    insets.top: Theme.titlebarHeight

    // No shader in this *scene*. The fragment program that cuts the corners is
    // the compositor's own and is not a QML capability, so this style draws on
    // the software path exactly as it draws on the GPU one.
    requires: []

    // Logical pixels. `pass::physical_radius` multiplies by the monitor's
    // scale on the way to the shader, so the corner is the same size on a
    // HiDPI screen rather than half of it.
    //
    // Read as: twelve everywhere, then the two that meet the bar taken back to
    // nothing. `radius` is the default the other three keys fall back to, so
    // the bottom pair need no line of their own — and the pair that does have
    // one is exactly the pair this style is about.
    //
    // **Zero here has to be written and cannot be left out.** A corner that is
    // not declared is not squared, it is `radius`; `-1` is what "not declared"
    // is spelled as internally, precisely so that a real `0` can be told from
    // an absent key. See `Solium/ClientTreatment.qml`.
    client.radius: 12
    client.radiusTopLeft: 0
    client.radiusTopRight: 0

    // `frame`, and that is the difference from `panes/rounded`, which puts its
    // one layer at `behind`.
    //
    // A bar only has to go under the client when it reaches past the seam to
    // fill something. This one stops at the seam — the client's top corners
    // are square, so there is nothing below the band for it to fill — and a
    // layer that stays inside its own insets is the cheap case: the compositor
    // copies it band by band rather than as a whole frame. Nothing it draws
    // can cover the client, because none of it is over the client.
    Layer {
        depth: "frame"
        name: "bar"
        source: "Frame.qml"
    }
}
