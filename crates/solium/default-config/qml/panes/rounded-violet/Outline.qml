// The outline around a floating window, two pixels outside it, violet with the
// keyboard and dim without, as the tiled frames' border: drawn behind the
// window in the layer's bleed, so it covers nothing.

import QtQuick
import Solium

Item {
    id: outline

    property bool focused: false
    property int clientRadius: 0
    property var windowValues: ({})
    FlightVisibility { id: desktopOpening; values: outline.windowValues }
    opacity: desktopOpening.flight && !desktopOpening.revealed ? 0 : 1
    Behavior on opacity { enabled: desktopOpening.revealed; NumberAnimation { duration: 80 } }

    Rectangle {
        anchors.fill: parent
        color: "transparent"
        radius: outline.clientRadius + 1
        // Two wide, its inner pixel under the window's edge: where the
        // window's rounded corner fades out, the border is already there, so
        // no wallpaper shows between the two.
        border.width: 2
        border.color: outline.focused ? "#936dff" : "#2f2e33"
        Behavior on border.color { ColorAnimation { duration: Theme.quick } }
    }
}
