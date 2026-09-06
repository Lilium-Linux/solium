// What you see the instant an application is asked for, before it exists.
//
// The compositor spawns the process itself and knows where the pointer is, so
// there is no reason to show nothing while a client starts, connects, is
// configured and finally draws — a second on a cold start, and every bit of it
// silence. This is the stand-in: it appears immediately, and the real window
// grows out of where it was.
//
// Edit it like any other scene here. It is handed the program's name and how
// long it has been waiting.

import QtQuick
import Solium

Item {
    id: card

    // Set by the compositor.
    property string program: ""
    property int waited: 0

    // Grown into place rather than appearing at full size, so the press and
    // the response read as one movement.
    scale: 0.0
    opacity: 0.0
    Component.onCompleted: {
        scale = 1.0;
        opacity = 1.0;
    }
    Behavior on scale {
        NumberAnimation { duration: 200; easing.type: Easing.OutBack; easing.overshoot: 1.1 }
    }
    Behavior on opacity { NumberAnimation { duration: 160 } }

    Rectangle {
        anchors.fill: parent
        radius: 14
        color: Theme.surface
        border { width: 1; color: Theme.edge }

        Column {
            anchors.centerIn: parent
            spacing: Theme.gap

            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                text: card.program
                color: Theme.text
                font { pixelSize: Theme.fontSize + 1; family: Theme.fontFamily }
            }

            // A bar that fills as the wait goes on, rather than a spinner:
            // it says "still coming" and roughly how long it has been, which
            // is the difference between waiting and wondering.
            Rectangle {
                anchors.horizontalCenter: parent.horizontalCenter
                width: 120
                height: 3
                radius: 2
                color: Theme.edgeInactive

                Rectangle {
                    height: parent.height
                    radius: parent.radius
                    color: Theme.accent
                    // Approaches full without reaching it: it is not a
                    // measurement, and pretending otherwise would be a lie
                    // that lands exactly when the app is slowest.
                    width: parent.width * (1.0 - Math.exp(-card.waited / 900.0))
                    Behavior on width { NumberAnimation { duration: 120 } }
                }
            }
        }
    }
}
