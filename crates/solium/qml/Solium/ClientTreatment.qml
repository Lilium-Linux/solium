// What is done to the client's own surface, as opposed to around it.
//
// Reserved, and unread. Both of these become effects that declare `inputs:
// self` — they mask or derive from the node's own texture — and neither is
// expressible until a frame can be split into passes. See the spec's
// *Effects* section.
//
// They are declared here rather than left out because a rounded window is the
// one piece of the design that is not additive: it stops the client being
// fully opaque, so opaque-region culling can no longer skip what is behind it.
// A style that asks for it is asking for that, and the property is where it
// gets to say so.

import QtQuick

QtObject {
    property int radius: 0
    property ClientShadow shadow: ClientShadow {}
}
