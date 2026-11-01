// The border, concentric with the client's cut corners: the client is cut at
// `clientRadius`, two pixels in, so the outside edge is rounded at two more.

import QtQuick
import Solium

Item {
    id: frame

    property bool focused: false
    property int clientRadius: 0

    Rectangle {
        anchors.fill: parent
        color: "transparent"
        radius: frame.clientRadius + 1
        // Two wide, its inner pixel under the client's edge, so its rounded
        // corner fades out onto the border and not onto the wallpaper.
        border.width: 2
        border.color: frame.focused ? "#936dff" : "#2f2e33"
        Behavior on border.color { ColorAnimation { duration: Theme.quick } }
    }
}
