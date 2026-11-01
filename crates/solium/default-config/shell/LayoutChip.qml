// The keyboard layout inside quick search, violet like the bar's.

import QtQuick
import Solium

Rectangle {
    visible: Keyboard.layouts.length > 1
    width: label.implicitWidth + 14
    height: 24
    radius: 6
    color: "#936dff"

    Text {
        id: label
        anchors.centerIn: parent
        text: Keyboard.layoutShort.toUpperCase()
        color: "#150f26"
        font { pixelSize: 12; family: "Noto Sans"; bold: true }
    }
}
