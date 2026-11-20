// Two metres square of walnut boards, a texture the floor repeats: boards
// 0.2 m wide and 1 m long, each column's joints somewhere else, with a
// little grain. It repeats seamlessly both ways, so it is drawn at 512
// pixels a metre however large the floor is.

import QtQuick

Item {
    id: boards

    readonly property real m: width / 2
    readonly property var woods: ["#6e4831", "#5f3e2a", "#77513a", "#583824", "#6a452f", "#62412c", "#71503a"]

    width: 1024
    height: 1024
    clip: true

    Rectangle {
        anchors.fill: parent
        color: "#24170e"
    }
    Repeater {
        model: 10
        Item {
            id: column
            readonly property int at: index
            readonly property real offset: ((index * 0.37) % 1.0) * boards.m
            x: index * 0.2 * boards.m
            width: 0.2 * boards.m
            height: boards.height

            Repeater {
                model: 4
                Rectangle {
                    readonly property int seed: column.at * 13 + index * 7
                    readonly property color wood: boards.woods[seed % boards.woods.length]
                    x: 2
                    y: column.offset + (index - 1) * boards.m + 2
                    width: column.width - 4
                    height: boards.m - 4
                    gradient: Gradient {
                        orientation: Gradient.Horizontal
                        GradientStop { position: 0.0; color: Qt.darker(wood, 1.1) }
                        GradientStop { position: 0.5; color: wood }
                        GradientStop { position: 1.0; color: Qt.darker(wood, 1.14) }
                    }
                    // The grain: a few faint lines along the board.
                    Repeater {
                        model: 3
                        Rectangle {
                            x: (index + 1) * parent.width / 4 + ((parent.seed * (index + 3)) % 9) - 4
                            y: 0
                            width: 1.5
                            height: parent.height
                            color: "#2a180c"
                            opacity: 0.18
                        }
                    }
                }
            }
        }
    }
}
