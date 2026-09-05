// A rectangle that clips its children to its own rounded shape.
import QtQuick
Rectangle { default property alias content: inner.data; clip: true; Item { id: inner; anchors.fill: parent } }
