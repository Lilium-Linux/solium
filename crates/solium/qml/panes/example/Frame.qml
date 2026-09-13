// The `frame` layer of the example bundle: a titlebar, in its own file.
//
// A layer's content is an ordinary QML file with an `Item` at its root, and it
// is loaded as its own scene. Which is why it declares the properties the
// compositor sets on a frame today — `title`, `focused`, `pointerInside`,
// `contentWidth`, `contentHeight` — rather than reading them off a parent: a
// layer has no parent, it is a scene.
//
// The shipped decorations under `decorations/` are the same thing with more in
// them. This one is small because its job is to show the shape.

import QtQuick
import Solium

Item {
    id: frame

    // Set by the compositor.
    property string title: ""
    property bool focused: false
    property bool pointerInside: false
    property int contentWidth: 0
    property int contentHeight: 0

    Rectangle {
        width: parent.width
        height: Theme.titlebarHeight
        color: frame.focused ? Theme.surface : Theme.surfaceInactive

        Text {
            anchors.centerIn: parent
            text: frame.title
            elide: Text.ElideRight
            width: parent.width - Theme.margin * 2
            horizontalAlignment: Text.AlignHCenter
            color: frame.focused ? Theme.text : Theme.textDim
            font.family: Theme.fontFamily
            font.pixelSize: Theme.fontSize
        }

        Rectangle {
            anchors.bottom: parent.bottom
            width: parent.width
            height: 1
            color: frame.focused ? Theme.edge : Theme.edgeInactive
        }
    }
}
