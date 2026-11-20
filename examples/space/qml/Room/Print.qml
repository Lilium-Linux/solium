// The print on the art wall, behind its mat: two discs and a slash on a
// deep teal ground.

import QtQuick

Item {
    id: artwork

    width: 560
    height: 380

    Rectangle {
        anchors.fill: parent
        color: "#e6dfd1"
    }
    Rectangle {
        id: ground
        anchors.fill: parent
        anchors.margins: 36
        clip: true
        gradient: Gradient {
            GradientStop { position: 0.0; color: "#1f4e5a" }
            GradientStop { position: 1.0; color: "#0f2a33" }
        }
        Rectangle {
            x: ground.width * 0.08
            y: ground.height * 0.18
            width: ground.height * 0.7
            height: width
            radius: width / 2
            color: "#e2735a"
        }
        Rectangle {
            x: ground.width * 0.42
            y: ground.height * 0.3
            width: ground.height * 0.55
            height: width
            radius: width / 2
            color: "#e9b949"
            opacity: 0.92
        }
        Rectangle {
            x: ground.width * 0.62
            y: -ground.height * 0.2
            width: ground.width * 0.08
            height: ground.height * 1.4
            rotation: 18
            antialiasing: true
            color: "#f2ece0"
        }
    }
}
