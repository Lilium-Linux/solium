// The room of examples/space, a scene in Qt Quick 3D behind the windows:
// `World.qml` seen through the window manager's camera. The window manager
// carries the windows on the room's walls with four groups, one inside the
// next: `move` (where you stand), `hop` (how high a jump has lifted you),
// `turn` (which way you face) and `eye` (how far you look up, and the lens);
// this reads the same groups, at the same instant the compositor draws the
// windows at, so the walls are where the windows hang on them.
//
// It needs QML on the GPU: on the hardware that is the default, and nested
// it is `SOLIUM_QML=gpu`. In software, Qt Quick 3D draws nothing.

import QtQuick
import Solium

Item {
    id: scene

    // What the window manager says of the room: a metre in pixels, its
    // width, depth and height and the eye's height in metres, a hung
    // window's size in metres, and where each window hangs, [x, z, heading]
    // in metres and degrees.
    property real metre: 500
    property var room: [7.2, 8, 2.8]
    property real eye: 1.6
    property var hang: [1.79, 1.19]
    property var slots: []

    readonly property var groups: Solium.groups
    readonly property var move: groups["move"]
    readonly property var hop: groups["hop"]
    readonly property var turn: groups["turn"]
    readonly property var look: groups["eye"]

    World {
        anchors.fill: parent
        m: scene.metre
        roomWidth: scene.room[0]
        roomDepth: scene.room[1]
        roomHeight: scene.room[2]
        eyeHeight: scene.eye
        hangWidth: scene.hang[0]
        hangHeight: scene.hang[1]
        slots: scene.slots
        // The groups move the room the other way from you: `move` by your
        // place, `hop` down by your jump, `turn` by your heading less.
        eyeX: scene.move ? -scene.move.x : 0
        eyeZ: scene.move ? -scene.move.z : 0
        lift: scene.hop ? scene.hop.y : 0
        yaw: scene.turn ? -scene.turn.rotateY : 0
        pitch: scene.look ? scene.look.rotateX : 0
        lens: scene.look && scene.look.perspective > 0 ? scene.look.perspective : 960
    }
}
