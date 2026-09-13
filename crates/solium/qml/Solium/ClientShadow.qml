// The shadow cast by the client's own surface.
//
// Reserved, and unread. It becomes an effect declaring `inputs: self` — it is
// derived from the node's silhouette — once passes exist. Declared now so that
// a style folder written today does not change shape when they do.
//
// Named, for the reason `Insets` is: `client.shadow.blur: 40` resolves through
// two declared types, and an anonymous `QtObject` at either level makes the
// whole path unassignable.

import QtQuick

QtObject {
    property int blur: 0
    property real opacity: 0
}
