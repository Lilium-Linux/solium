// One metre of wall, the whole 2.8 m of its height, a texture each wall
// repeats along its length: plaster with a faint pinstripe above, and a
// panelled wainscot below 0.9 m. Only what the wall is made of: the light
// on it is the room's lights'.

import QtQuick

Item {
    id: plaster

    readonly property real m: width

    width: 256
    height: 717

    Rectangle {
        anchors.fill: parent
        color: "#31465f"
    }
    Repeater {
        model: 4
        Rectangle {
            x: index * 0.25 * plaster.m
            y: 0
            width: 3
            height: plaster.height - 0.9 * plaster.m
            color: "#3c5370"
        }
    }
    // The wainscot, its panel inset with a lighter edge above and a darker
    // one below, as a moulding catches a light from above.
    Rectangle {
        x: 0
        y: plaster.height - 0.9 * plaster.m
        width: plaster.width
        height: 0.9 * plaster.m
        color: "#24303f"

        Rectangle {
            x: 0.08 * plaster.m
            y: 0.12 * plaster.m
            width: parent.width - 0.16 * plaster.m
            height: 0.6 * plaster.m
            color: "#2a3749"
            border.width: 3
            border.color: "#41506a"
            Rectangle {
                x: 3
                y: parent.height - 6
                width: parent.width - 6
                height: 3
                color: "#1d2230"
            }
        }
    }
}
