// What the window in the window wall looks out on: a city at dusk under a
// full moon, each building's height and lit windows from its index. Drawn
// unlit in the room, as light coming in.

import QtQuick

Item {
    id: city

    width: 420
    height: 470
    clip: true

    Rectangle {
        anchors.fill: parent
        gradient: Gradient {
            GradientStop { position: 0.0; color: "#0a0f2c" }
            GradientStop { position: 0.55; color: "#2a2156" }
            GradientStop { position: 0.85; color: "#6b3c63" }
            GradientStop { position: 1.0; color: "#c06a55" }
        }
    }
    Repeater {
        model: 60
        Rectangle {
            x: ((index * 7919) % 1000) / 1000 * city.width
            y: ((index * 104729) % 997) / 997 * city.height * 0.55
            width: index % 5 === 0 ? 3 : 2
            height: width
            radius: width / 2
            color: "#ffffff"
            opacity: 0.35 + ((index * 31) % 60) / 100
        }
    }
    Rectangle {
        x: city.width * 0.62
        y: city.height * 0.1
        width: 56
        height: 56
        radius: 28
        color: "#f6edcf"
    }
    Repeater {
        model: 14
        Rectangle {
            id: building
            readonly property real tall: (0.25 + ((index * 37) % 10) / 22) * city.height
            x: index * city.width / 13 - 6
            y: city.height - tall
            width: city.width / 13 + ((index * 13) % 4) * 4
            height: tall
            color: index % 2 === 0 ? "#0b0d17" : "#11131f"
            Repeater {
                model: 18
                Rectangle {
                    x: (index % 3) * building.width / 3 + 3
                    y: Math.floor(index / 3) * 22 + 12
                    width: building.width / 3 - 6
                    height: 9
                    color: "#ffcc72"
                    opacity: ((index * 7 + Math.round(building.x)) % 5) < 2 ? 0.9 : 0.0
                }
            }
        }
    }
}
