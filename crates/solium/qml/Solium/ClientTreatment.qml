// What is done to the client's own surface, as opposed to around it.
//
// `radius` is read. `style::load` turns a non-zero one into an effect that
// declares `inputs: self`, because it masks the node's own texture. Zero is
// *no effect* rather than an effect that rounds by nothing: the difference is
// an offscreen pass per window per frame, and every shipped style leaves this
// alone.
//
// `shadow` is still reserved. It is the same shape — it derives from the
// node's own texture rather than masking it — and nothing draws it yet. See
// the spec's *Effects* section.
//
// They are declared here rather than left out because a rounded window is the
// one piece of the design that is not additive: it stops the client being
// fully opaque, so opaque-region culling can no longer skip what is behind it.
// A style that asks for it is asking for that, and the property is where it
// gets to say so.

import QtQuick

QtObject {
    property int radius: 0
    // Each corner, defaulting to `radius`. **-1 and not 0 is the default**,
    // because 0 is a value someone means -- squaring one corner is half the
    // point of these -- and a default that is also a legal value cannot be
    // told from one. The compositor reads a negative as "not declared".
    property int radiusTopLeft: -1
    property int radiusTopRight: -1
    property int radiusBottomLeft: -1
    property int radiusBottomRight: -1
    property ClientShadow shadow: ClientShadow {}
}
