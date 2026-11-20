// The space the desks turn in: a deep gradient with a field of stars, drawn
// once behind every window. It does not move; the windows do.

import QtQuick

Item {
    id: space

    Rectangle {
        anchors.fill: parent
        gradient: Gradient {
            GradientStop { position: 0.0; color: "#05060f" }
            GradientStop { position: 0.6; color: "#0d0b1f" }
            GradientStop { position: 1.0; color: "#1b0f2c" }
        }
    }

    // Stars at places that look scattered and are the same every time:
    // each one's place and size come from its index.
    Repeater {
        model: 220
        Rectangle {
            readonly property real a: (index * 7919) % 1000 / 1000
            readonly property real b: (index * 104729) % 997 / 997
            x: a * space.width
            y: b * space.height
            width: 1 + (index % 7 === 0 ? 2 : index % 3 === 0 ? 1 : 0)
            height: width
            radius: width / 2
            color: index % 11 === 0 ? "#b9c8ff" : "#ffffff"
            opacity: 0.25 + ((index * 31) % 70) / 100
        }
    }
}
