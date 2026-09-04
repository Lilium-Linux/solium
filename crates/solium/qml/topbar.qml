// Solium's top bar.
//
// Rendered by Qt's scene graph directly into a texture the compositor owns, on
// the compositor's own GL context. There is no shell process and no protocol
// between this file and the screen.
//
// Properties are set from Rust each frame. Animations are driven by the
// compositor's clock, so they cannot drift against window transforms.

import QtQuick

Item {
    id: bar

    // Set from the compositor.
    property string windowTitle: ""
    property int windowCount: 0
    property bool overviewActive: false
    property real clockSeconds: 0

    Rectangle {
        anchors.fill: parent
        color: "#111318"
        opacity: 0.94

        // A hairline under the bar rather than a border around it: the bar is
        // an edge of the screen, not a floating panel.
        Rectangle {
            anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
            height: 1
            color: "#262b36"
        }
    }

    Row {
        anchors { left: parent.left; leftMargin: 18; verticalCenter: parent.verticalCenter }
        spacing: 14

        // The one piece of chrome that moves on its own, so it is obvious at a
        // glance whether QML animations are actually being driven.
        Rectangle {
            width: 9
            height: 9
            radius: 4.5
            anchors.verticalCenter: parent.verticalCenter
            color: bar.overviewActive ? "#7aa2f7" : "#9ece6a"

            SequentialAnimation on opacity {
                loops: Animation.Infinite
                running: true
                NumberAnimation { to: 0.25; duration: 900; easing.type: Easing.InOutQuad }
                NumberAnimation { to: 1.0;  duration: 900; easing.type: Easing.InOutQuad }
            }
        }

        Text {
            anchors.verticalCenter: parent.verticalCenter
            text: "Solium"
            color: "#e6e9ef"
            font { pixelSize: 13; bold: true; family: "monospace" }
        }

        Text {
            anchors.verticalCenter: parent.verticalCenter
            text: bar.overviewActive ? "overview" : (bar.windowCount === 1 ? "1 window"
                                                                          : bar.windowCount + " windows")
            color: "#7f8798"
            font { pixelSize: 12; family: "monospace" }
        }
    }

    // The focused window's title, centred, sliding in when it changes.
    Text {
        id: title
        anchors.centerIn: parent
        text: bar.windowTitle
        color: "#c3c9d6"
        font { pixelSize: 12; family: "monospace" }
        elide: Text.ElideRight
        width: Math.min(implicitWidth, bar.width * 0.4)

        onTextChanged: appear.restart()

        SequentialAnimation {
            id: appear
            NumberAnimation { target: title; property: "opacity"; to: 0; duration: 90 }
            NumberAnimation { target: title; property: "opacity"; to: 1; duration: 160
                              easing.type: Easing.OutCubic }
        }
    }

    Text {
        anchors { right: parent.right; rightMargin: 18; verticalCenter: parent.verticalCenter }
        // Seconds since the compositor started: proof the value is arriving
        // from the compositor's clock and not from Qt's.
        text: Math.floor(bar.clockSeconds) + "s"
        color: "#7f8798"
        font { pixelSize: 12; family: "monospace" }
    }
}
