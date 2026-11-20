// The rug's face: an indigo field in a gold-lined border, diamonds inside
// diamonds in its middle and one in each corner. 3.4 m by 4.6 m, at 256
// pixels a metre.

import QtQuick

Item {
    id: rug

    readonly property real m: width / 3.4

    width: 870
    height: 1178

    Rectangle {
        anchors.fill: parent
        color: "#2a1f3d"
    }
    Rectangle {
        anchors.fill: parent
        anchors.margins: 0.12 * rug.m
        color: "transparent"
        border.width: 0.04 * rug.m
        border.color: "#c4924a"
    }
    Rectangle {
        anchors.fill: parent
        anchors.margins: 0.32 * rug.m
        color: "#3b2d5c"
        border.width: 0.03 * rug.m
        border.color: "#a8743c"
    }
    Repeater {
        model: 4
        Rectangle {
            readonly property real across: [1.5, 1.15, 0.75, 0.32][index] * rug.m
            x: rug.width / 2 - across / 2
            y: rug.height / 2 - across / 2
            width: across
            height: across
            rotation: 45
            antialiasing: true
            color: ["#7d2f45", "#c4924a", "#2f6a6e", "#e3c38a"][index]
        }
    }
    Repeater {
        model: 4
        Rectangle {
            readonly property real across: 0.42 * rug.m
            x: (index % 2 === 0 ? 0.62 * rug.m : rug.width - 0.62 * rug.m) - across / 2
            y: (index < 2 ? 0.62 * rug.m : rug.height - 0.62 * rug.m) - across / 2
            width: across
            height: across
            rotation: 45
            antialiasing: true
            color: "#7d2f45"
            border.width: 0.03 * rug.m
            border.color: "#c4924a"
        }
    }
}
