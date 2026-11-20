// The room's floor, painted flat: walnut boards running away from the
// window wall, a rug in the middle, pools of light under the ceiling's
// lamps and the corners in shadow. The window manager lays it down.
//
// Measured in metres of a floor 7.2 m across, so it reads right at any size
// it is declared at; its top edge is the window wall's.

import QtQuick

Item {
    id: floor

    // A metre, and where the ceiling's lamps are, in metres from the
    // middle (x across, y toward the back), which the light pools sit under.
    readonly property real m: width / 7.2
    property var lamps: [[-1.6, -2.7], [1.6, -2.7], [-1.6, -0.9], [1.6, -0.9], [-1.6, 0.9], [1.6, 0.9], [-1.6, 2.7], [1.6, 2.7]]

    readonly property var woods: ["#6a4530", "#5c3c29", "#734d36", "#553622", "#66422e", "#5f3f2b", "#6f4a33"]

    // How many of something fit, never more than `most`: a scene is sized
    // one side at a time, and a count taken between the two would ask for
    // thousands of items.
    function fits(count, most) {
        return isFinite(count) ? Math.max(0, Math.min(most, Math.ceil(count))) : 0
    }

    // The gaps between the boards.
    Rectangle {
        anchors.fill: parent
        color: "#1e130c"
    }

    // The boards: a column every 0.2 m, each cut into lengths of 1.6 m that
    // start somewhere else in every column.
    Repeater {
        model: floor.fits(floor.width / (0.2 * floor.m), 48)
        Item {
            id: column
            readonly property int at: index
            x: index * 0.2 * floor.m
            y: 0
            width: 0.2 * floor.m
            height: floor.height

            Repeater {
                model: floor.fits(floor.height / (1.6 * floor.m) + 1, 10)
                Rectangle {
                    readonly property int seed: column.at * 13 + index * 7
                    readonly property real start: -((column.at * 0.53) % 1.6) * floor.m
                    x: 0.006 * floor.m
                    y: start + index * 1.6 * floor.m + 0.006 * floor.m
                    width: column.width - 0.012 * floor.m
                    height: 1.6 * floor.m - 0.012 * floor.m
                    gradient: Gradient {
                        orientation: Gradient.Horizontal
                        GradientStop { position: 0.0; color: Qt.darker(floor.woods[seed % floor.woods.length], 1.08) }
                        GradientStop { position: 0.5; color: floor.woods[seed % floor.woods.length] }
                        GradientStop { position: 1.0; color: Qt.darker(floor.woods[seed % floor.woods.length], 1.12) }
                    }
                }
            }
        }
    }

    // The rug: an indigo field in a gold-lined border, a medallion in its
    // middle and fringe at its ends.
    Item {
        id: rug
        x: floor.width / 2 - 1.7 * floor.m
        y: floor.height / 2 - 2.3 * floor.m
        width: 3.4 * floor.m
        height: 4.6 * floor.m

        Rectangle {
            anchors.fill: parent
            color: "#2a1f3d"
        }
        Rectangle {
            anchors.fill: parent
            anchors.margins: 0.12 * floor.m
            color: "transparent"
            border.width: 0.04 * floor.m
            border.color: "#c4924a"
        }
        Rectangle {
            id: field
            anchors.fill: parent
            anchors.margins: 0.32 * floor.m
            color: "#3b2d5c"
            border.width: 0.03 * floor.m
            border.color: "#a8743c"
        }
        // The medallion: diamonds inside diamonds, and one at each corner
        // of the field.
        Repeater {
            model: 4
            Rectangle {
                readonly property real across: [1.5, 1.15, 0.75, 0.32][index] * floor.m
                x: rug.width / 2 - across / 2
                y: rug.height / 2 - across / 2
                width: across
                height: across
                rotation: 45
                color: ["#7d2f45", "#c4924a", "#2f6a6e", "#e3c38a"][index]
            }
        }
        Repeater {
            model: 4
            Rectangle {
                readonly property real across: 0.42 * floor.m
                x: (index % 2 === 0 ? 0.62 * floor.m : rug.width - 0.62 * floor.m) - across / 2
                y: (index < 2 ? 0.62 * floor.m : rug.height - 0.62 * floor.m) - across / 2
                width: across
                height: across
                rotation: 45
                color: "#7d2f45"
                border.width: Math.max(1, 0.03 * floor.m)
                border.color: "#c4924a"
            }
        }
        Repeater {
            model: 2
            Item {
                id: fringe
                readonly property int end: index
                anchors.fill: parent
                Repeater {
                    model: 34
                    Rectangle {
                        x: (index + 0.5) * rug.width / 34
                        y: fringe.end === 0 ? -0.1 * floor.m : rug.height
                        width: Math.max(1, 0.015 * floor.m)
                        height: 0.1 * floor.m
                        color: "#d8cdb2"
                    }
                }
            }
        }
    }

    // Under each lamp, a pool of light.
    Repeater {
        model: floor.lamps
        Pool {
            x: floor.width / 2 + modelData[0] * floor.m - 1.5 * floor.m
            y: floor.height / 2 + modelData[1] * floor.m - 1.5 * floor.m
            width: 3.0 * floor.m
            height: 3.0 * floor.m
            tint: "#ffcf8f"
            strength: 0.2
        }
    }

    // The window's cool light, along the window wall's middle.
    Pool {
        x: floor.width / 2 - 1.6 * floor.m
        y: -1.4 * floor.m
        width: 3.2 * floor.m
        height: 3.0 * floor.m
        tint: "#7f9cff"
        strength: 0.16
    }

    // Shadow into the walls, all round.
    Rectangle {
        x: 0; y: 0; width: floor.width; height: 0.8 * floor.m
        gradient: Gradient {
            GradientStop { position: 0.0; color: "#99000000" }
            GradientStop { position: 1.0; color: "#00000000" }
        }
    }
    Rectangle {
        x: 0; y: floor.height - 0.8 * floor.m; width: floor.width; height: 0.8 * floor.m
        gradient: Gradient {
            GradientStop { position: 0.0; color: "#00000000" }
            GradientStop { position: 1.0; color: "#99000000" }
        }
    }
    Rectangle {
        x: 0; y: 0; width: 0.8 * floor.m; height: floor.height
        gradient: Gradient {
            orientation: Gradient.Horizontal
            GradientStop { position: 0.0; color: "#99000000" }
            GradientStop { position: 1.0; color: "#00000000" }
        }
    }
    Rectangle {
        x: floor.width - 0.8 * floor.m; y: 0; width: 0.8 * floor.m; height: floor.height
        gradient: Gradient {
            orientation: Gradient.Horizontal
            GradientStop { position: 0.0; color: "#00000000" }
            GradientStop { position: 1.0; color: "#99000000" }
        }
    }
}
