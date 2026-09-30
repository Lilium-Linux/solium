// A layer-shell panel, as an ordinary Item.
//
// In Quickshell this is a window the compositor is asked for over
// wlr-layer-shell. Inside Solium there is nobody to ask: the scene *is* being
// drawn by the compositor, so a panel is simply an item with the geometry the
// compositor gave it. The anchors and exclusive-zone properties are kept so
// shell code that sets them still loads; the compositor decides placement.

import QtQuick

Item {
    property var anchors: ({ left: false, right: false, top: false, bottom: false })
    property int exclusiveZone: 0
    property string layer: "top"
    property string namespace: ""
    property var screen: null
    property color color: "transparent"
    property bool visible: true
    default property alias contentItem: content.data

    Item {
        id: content
        anchors.fill: parent
    }
}
