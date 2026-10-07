// The `bar` layer of the `frosted` fixture: a translucent titlebar.

import QtQuick
import Solium

Item {
    id: bar

    // Set by the compositor.
    property string title: ""
    property bool focused: false
    property bool pointerInside: false
    property int contentWidth: 0
    property int contentHeight: 0

    Rectangle {
        width: parent.width
        height: 32
        color: Qt.rgba(1, 1, 1, bar.focused ? 0.35 : 0.2)

        Text {
            anchors.centerIn: parent
            text: bar.title
            elide: Text.ElideRight
            width: parent.width - Theme.margin * 2
            horizontalAlignment: Text.AlignHCenter
            color: Theme.text
            font.family: Theme.fontFamily
            font.pixelSize: Theme.fontSize
        }
    }
}
