// The room's ceiling, painted flat: dark plaster crossed by beams, and a
// row of lamps down each side, each a bright disc in a trim ring with the
// glow it throws. The window manager hangs it overhead.
//
// Measured in metres of a ceiling 7.2 m across, as the floor is, with its
// lamps where the floor's pools of light are.

import QtQuick

Item {
    id: ceiling

    readonly property real m: width / 7.2
    property var lamps: [[-1.6, -2.7], [1.6, -2.7], [-1.6, -0.9], [1.6, -0.9], [-1.6, 0.9], [1.6, 0.9], [-1.6, 2.7], [1.6, 2.7]]

    Rectangle {
        anchors.fill: parent
        gradient: Gradient {
            orientation: Gradient.Horizontal
            GradientStop { position: 0.0; color: "#11141b" }
            GradientStop { position: 0.5; color: "#1d222d" }
            GradientStop { position: 1.0; color: "#11141b" }
        }
    }

    // The beams, across the room between the rows of lamps.
    Repeater {
        model: [-1.8, 0.0, 1.8]
        Rectangle {
            x: 0
            y: ceiling.height / 2 + modelData * ceiling.m - 0.14 * ceiling.m
            width: ceiling.width
            height: 0.28 * ceiling.m
            gradient: Gradient {
                GradientStop { position: 0.0; color: "#2a3040" }
                GradientStop { position: 0.15; color: "#151922" }
                GradientStop { position: 0.85; color: "#151922" }
                GradientStop { position: 1.0; color: "#2a3040" }
            }
        }
    }

    Repeater {
        model: ceiling.lamps
        Item {
            readonly property real cx: ceiling.width / 2 + modelData[0] * ceiling.m
            readonly property real cy: ceiling.height / 2 + modelData[1] * ceiling.m
            anchors.fill: parent

            Pool {
                x: parent.cx - 0.9 * ceiling.m
                y: parent.cy - 0.9 * ceiling.m
                width: 1.8 * ceiling.m
                height: 1.8 * ceiling.m
                tint: "#ffd8a0"
                strength: 0.28
            }
            Rectangle {
                x: parent.cx - 0.11 * ceiling.m
                y: parent.cy - 0.11 * ceiling.m
                width: 0.22 * ceiling.m
                height: width
                radius: width / 2
                color: "#3a3f4a"
            }
            Rectangle {
                x: parent.cx - 0.08 * ceiling.m
                y: parent.cy - 0.08 * ceiling.m
                width: 0.16 * ceiling.m
                height: width
                radius: width / 2
                color: "#fff6e0"
            }
        }
    }

    // Shadow into the walls along the long sides.
    Rectangle {
        x: 0; y: 0; width: 0.6 * ceiling.m; height: ceiling.height
        gradient: Gradient {
            orientation: Gradient.Horizontal
            GradientStop { position: 0.0; color: "#aa000000" }
            GradientStop { position: 1.0; color: "#00000000" }
        }
    }
    Rectangle {
        x: ceiling.width - 0.6 * ceiling.m; y: 0; width: 0.6 * ceiling.m; height: ceiling.height
        gradient: Gradient {
            orientation: Gradient.Horizontal
            GradientStop { position: 0.0; color: "#00000000" }
            GradientStop { position: 1.0; color: "#aa000000" }
        }
    }
}
