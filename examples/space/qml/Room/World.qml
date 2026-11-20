// The room, in Qt Quick 3D: walls, floor and ceiling, the frames and picture
// lights the windows hang in, a door, a window onto a city, and furniture,
// lit by real lights that cast shadows. Its camera is wherever it is told,
// which `Room.qml` reads from the window manager's camera groups, so the
// windows the compositor hangs on these walls are drawn where the walls are.
//
// Measured in metres, a metre being `m` of the compositor's pixels: the
// room's node is scaled by it, and the camera is placed in pixels. The room's
// middle is on the floor at the origin: x runs right, y up, z toward the back
// wall, the one behind you as you come in.
//
// Nothing that stands in the room is taller than the windows' lower edges
// except in its corners, so from anywhere you can stand nothing comes
// between you and a window: the compositor draws the windows over this
// scene, which is only right if nothing should be in front of them.

import QtQuick
import QtQuick3D
import QtQuick3D.Helpers

Item {
    id: world

    // A metre, in the compositor's pixels.
    property real m: 500
    property real roomWidth: 7.2
    property real roomDepth: 8
    property real roomHeight: 2.8
    // How high the eye is, and so the windows' middles.
    property real eyeHeight: 1.6
    // Where each window hangs: [x, z, heading], metres and degrees, the
    // heading being which way you face to look at it.
    property var slots: []
    // A hung window's size, in metres.
    property real hangWidth: 1.79
    property real hangHeight: 1.19

    // The camera: where you stand from the room's middle and how far a jump
    // has lifted you, in pixels; which way you face and how far you look up,
    // in degrees; and how far in front of the screen the eye is, in pixels,
    // which is the lens.
    property real eyeX: 0
    property real eyeZ: 0
    property real lift: 0
    property real yaw: 0
    property real pitch: 0
    property real lens: 960

    readonly property real halfWidth: roomWidth / 2
    readonly property real halfDepth: roomDepth / 2

    // ---- What the room is built of ----

    // A box, `size` metres, its middle `at`.
    component Block: Model {
        id: block
        property vector3d size: Qt.vector3d(1, 1, 1)
        property vector3d at: Qt.vector3d(0, 0, 0)
        property color color: "#808080"
        property real roughness: 0.6
        property real metalness: 0
        property real glow: 0
        source: "#Cube"
        position: at
        scale: Qt.vector3d(size.x / 100, size.y / 100, size.z / 100)
        materials: PrincipledMaterial {
            baseColor: block.color
            roughness: block.roughness
            metalness: block.metalness
            emissiveFactor: Qt.vector3d(block.glow, block.glow * 0.85, block.glow * 0.6)
        }
    }

    // A round post, `radius` and `tall` metres, its middle `at`.
    component Rod: Model {
        id: rod
        property real radius: 0.05
        property real tall: 1
        property vector3d at: Qt.vector3d(0, 0, 0)
        property color color: "#808080"
        property real roughness: 0.5
        property real metalness: 0
        property real glow: 0
        source: "#Cylinder"
        position: at
        scale: Qt.vector3d(radius / 50, tall / 100, radius / 50)
        materials: PrincipledMaterial {
            baseColor: rod.color
            roughness: rod.roughness
            metalness: rod.metalness
            emissiveFactor: Qt.vector3d(rod.glow, rod.glow * 0.85, rod.glow * 0.6)
        }
    }

    // A ball, `radius` metres, its middle `at`; `squash` flattens it.
    component Ball: Model {
        id: ball
        property real radius: 0.1
        property real squash: 1
        property vector3d at: Qt.vector3d(0, 0, 0)
        property color color: "#808080"
        property real roughness: 0.5
        property real metalness: 0
        property real glow: 0
        source: "#Sphere"
        position: at
        scale: Qt.vector3d(radius / 50, radius * squash / 50, radius / 50)
        materials: PrincipledMaterial {
            baseColor: ball.color
            roughness: ball.roughness
            metalness: ball.metalness
            emissiveFactor: Qt.vector3d(ball.glow, ball.glow * 0.9, ball.glow * 0.75)
        }
    }

    // A flat face, `w` by `h` metres, facing +z, its middle `at`, showing
    // `texture`.
    component Face: Model {
        id: face
        property real w: 1
        property real h: 1
        property vector3d at: Qt.vector3d(0, 0, 0)
        property Texture texture: null
        property color color: "#ffffff"
        property real roughness: 0.8
        property bool unlit: false
        source: "#Rectangle"
        position: at
        scale: Qt.vector3d(w / 100, h / 100, 1)
        materials: PrincipledMaterial {
            baseColor: face.color
            baseColorMap: face.texture
            roughness: face.roughness
            lighting: face.unlit ? PrincipledMaterial.NoLighting : PrincipledMaterial.FragmentLighting
        }
    }

    // A wall, `length` metres along, facing into the room.
    component Wall: Node {
        id: wall
        property real length: 1
        property real tall: 2.8
        Face {
            w: wall.length
            h: wall.tall
            at: Qt.vector3d(0, wall.tall / 2, 0)
            roughness: 0.92
            texture: Texture {
                sourceItem: Plaster {}
                tilingModeHorizontal: Texture.Repeat
                scaleU: wall.length
                generateMipmaps: true
                mipFilter: Texture.Linear
            }
        }
        // The baseboard, the chair rail and the crown.
        Block {
            size: Qt.vector3d(wall.length, 0.13, 0.03)
            at: Qt.vector3d(0, 0.065, 0.015)
            color: "#151a24"
            roughness: 0.5
        }
        Block {
            size: Qt.vector3d(wall.length, 0.05, 0.035)
            at: Qt.vector3d(0, 0.905, 0.0175)
            color: "#4b5569"
            roughness: 0.45
        }
        Block {
            size: Qt.vector3d(wall.length, 0.12, 0.07)
            at: Qt.vector3d(0, wall.tall - 0.06, 0.035)
            color: "#2b3242"
            roughness: 0.6
        }
    }

    // A picture frame round where a window hangs, a brass lamp above it, and
    // the lamp's light on the wall.
    component Hanging: Node {
        id: hanging
        property real w: 1.79
        property real h: 1.19
        readonly property real bar: 0.05
        readonly property real top: h / 2 + bar
        Block {
            size: Qt.vector3d(hanging.w + 2 * hanging.bar, hanging.bar, 0.035)
            at: Qt.vector3d(0, hanging.h / 2 + hanging.bar / 2, 0.0175)
            color: "#16120c"
            roughness: 0.35
        }
        Block {
            size: Qt.vector3d(hanging.w + 2 * hanging.bar, hanging.bar, 0.035)
            at: Qt.vector3d(0, -hanging.h / 2 - hanging.bar / 2, 0.0175)
            color: "#16120c"
            roughness: 0.35
        }
        Block {
            size: Qt.vector3d(hanging.bar, hanging.h, 0.035)
            at: Qt.vector3d(-hanging.w / 2 - hanging.bar / 2, 0, 0.0175)
            color: "#16120c"
            roughness: 0.35
        }
        Block {
            size: Qt.vector3d(hanging.bar, hanging.h, 0.035)
            at: Qt.vector3d(hanging.w / 2 + hanging.bar / 2, 0, 0.0175)
            color: "#16120c"
            roughness: 0.35
        }
        Block {
            size: Qt.vector3d(0.025, 0.025, 0.16)
            at: Qt.vector3d(0, hanging.top + 0.12, 0.08)
            color: "#8a6f3e"
            metalness: 1
            roughness: 0.35
        }
        Rod {
            radius: 0.03
            tall: 0.7
            at: Qt.vector3d(0, hanging.top + 0.1, 0.17)
            eulerRotation.z: 90
            color: "#c9a96a"
            metalness: 1
            roughness: 0.3
        }
        Block {
            size: Qt.vector3d(0.6, 0.01, 0.03)
            at: Qt.vector3d(0, hanging.top + 0.07, 0.17)
            color: "#fff1c9"
            glow: 4
        }
        SpotLight {
            position: Qt.vector3d(0, hanging.top + 0.06, 0.22)
            eulerRotation.x: -68
            color: "#ffd9a8"
            brightness: 7
            coneAngle: 64
            innerConeAngle: 26
            quadraticFade: 0.05
            linearFade: 0
        }
    }

    // ---- The scene ----

    View3D {
        id: view
        anchors.fill: parent
        camera: camera

        environment: ExtendedSceneEnvironment {
            backgroundMode: SceneEnvironment.Color
            clearColor: "#05060a"
            antialiasingMode: SceneEnvironment.MSAA
            antialiasingQuality: SceneEnvironment.High
            tonemapMode: SceneEnvironment.TonemapModeFilmic
            aoEnabled: true
            aoStrength: 70
            aoDistance: 0.3 * world.m
            aoSoftness: 40
            exposure: 0.6
            glowEnabled: true
            glowIntensity: 0.6
            glowBloom: 0.05
            glowStrength: 1.0
            glowHDRMinimumValue: 2.0
            glowBlendMode: ExtendedSceneEnvironment.GlowBlendMode.Additive
            vignetteEnabled: false
        }

        // The eye: placed in pixels, turned to face, and the lens tipped
        // to look up, as the window manager's groups turn the room about it.
        Node {
            position: Qt.vector3d(world.eyeX, world.eyeHeight * world.m + world.lift, world.eyeZ)
            eulerRotation.y: world.yaw
            PerspectiveCamera {
                id: camera
                eulerRotation.x: world.pitch
                fieldOfViewOrientation: PerspectiveCamera.Vertical
                fieldOfView: 2 * Math.atan(view.height / 2 / world.lens) * 180 / Math.PI
                clipNear: 0.04 * world.m
                clipFar: 40 * world.m
            }
        }

        Node {
            id: room
            scale: Qt.vector3d(world.m, world.m, world.m)

            // A little of the evening everywhere, so no face is black.
            DirectionalLight {
                eulerRotation: Qt.vector3d(-55, 25, 0)
                color: "#8f9fc0"
                ambientColor: "#161a26"
                brightness: 0.12
            }

            // The floor, its boards running away from the window wall.
            Face {
                w: world.roomWidth
                h: world.roomDepth
                eulerRotation.x: -90
                roughness: 0.55
                texture: Texture {
                    sourceItem: Boards {}
                    tilingModeHorizontal: Texture.Repeat
                    tilingModeVertical: Texture.Repeat
                    scaleU: world.roomWidth / 2
                    scaleV: world.roomDepth / 2
                    generateMipmaps: true
                    mipFilter: Texture.Linear
                }
            }
            // The ceiling, its beams and its row of lamps down each side.
            Face {
                w: world.roomWidth
                h: world.roomDepth
                at: Qt.vector3d(0, world.roomHeight, 0)
                eulerRotation.x: 90
                color: "#1d222c"
                roughness: 0.95
            }
            Repeater3D {
                model: [-1.8, 0, 1.8]
                Block {
                    size: Qt.vector3d(world.roomWidth, 0.2, 0.28)
                    at: Qt.vector3d(0, world.roomHeight - 0.1, modelData)
                    color: "#161a22"
                    roughness: 0.8
                }
            }
            Repeater3D {
                model: [[-1.6, -2.7], [1.6, -2.7], [-1.6, -0.9], [1.6, -0.9], [-1.6, 0.9], [1.6, 0.9], [-1.6, 2.7], [1.6, 2.7]]
                Node {
                    position: Qt.vector3d(modelData[0], world.roomHeight - 0.005, modelData[1])
                    Rod {
                        radius: 0.1
                        tall: 0.01
                        color: "#3a3f4a"
                        metalness: 0.6
                        roughness: 0.4
                    }
                    Rod {
                        radius: 0.07
                        tall: 0.01
                        at: Qt.vector3d(0, -0.008, 0)
                        color: "#fff6e0"
                        glow: 12
                    }
                }
            }

            // The four walls, each facing in.
            Wall {
                length: world.roomWidth
                tall: world.roomHeight
                position: Qt.vector3d(0, 0, -world.halfDepth)
            }
            Wall {
                length: world.roomWidth
                tall: world.roomHeight
                position: Qt.vector3d(0, 0, world.halfDepth)
                eulerRotation.y: 180
            }
            Wall {
                length: world.roomDepth
                tall: world.roomHeight
                position: Qt.vector3d(-world.halfWidth, 0, 0)
                eulerRotation.y: 90
            }
            Wall {
                length: world.roomDepth
                tall: world.roomHeight
                position: Qt.vector3d(world.halfWidth, 0, 0)
                eulerRotation.y: -90
            }

            // Where each window hangs.
            Repeater3D {
                model: world.slots
                Hanging {
                    w: world.hangWidth
                    h: world.hangHeight
                    position: Qt.vector3d(modelData[0], world.eyeHeight, modelData[1])
                    eulerRotation.y: modelData[2]
                }
            }

            // The window onto the city, in the window wall's middle, its
            // cool light falling into the room.
            Node {
                position: Qt.vector3d(0, 1.72, -world.halfDepth)
                Face {
                    w: 1.26
                    h: 1.41
                    at: Qt.vector3d(0, 0, 0.004)
                    unlit: true
                    texture: Texture { sourceItem: City {} }
                }
                Block { size: Qt.vector3d(1.42, 0.08, 0.1); at: Qt.vector3d(0, 0.745, 0.05); color: "#0e1118"; roughness: 0.4 }
                Block { size: Qt.vector3d(1.42, 0.08, 0.1); at: Qt.vector3d(0, -0.745, 0.05); color: "#0e1118"; roughness: 0.4 }
                Block { size: Qt.vector3d(0.08, 1.41, 0.1); at: Qt.vector3d(-0.67, 0, 0.05); color: "#0e1118"; roughness: 0.4 }
                Block { size: Qt.vector3d(0.08, 1.41, 0.1); at: Qt.vector3d(0.67, 0, 0.05); color: "#0e1118"; roughness: 0.4 }
                Block { size: Qt.vector3d(0.04, 1.41, 0.05); at: Qt.vector3d(0, 0, 0.025); color: "#0e1118"; roughness: 0.4 }
                Block { size: Qt.vector3d(1.26, 0.04, 0.05); at: Qt.vector3d(0, 0.12, 0.025); color: "#0e1118"; roughness: 0.4 }
                Block { size: Qt.vector3d(1.6, 0.05, 0.16); at: Qt.vector3d(0, -0.8, 0.08); color: "#6b7389"; roughness: 0.4 }
                SpotLight {
                    position: Qt.vector3d(0, 0.3, 0.3)
                    eulerRotation.x: -35
                    color: "#7f9cff"
                    brightness: 2.5
                    coneAngle: 70
                    innerConeAngle: 30
                    quadraticFade: 0.01
                }
            }

            // The door in the back wall, a line of warm light under it.
            Node {
                position: Qt.vector3d(0, 0, world.halfDepth)
                eulerRotation.y: 180
                Block { size: Qt.vector3d(1.22, 0.1, 0.05); at: Qt.vector3d(0, 2.16, 0.025); color: "#141820"; roughness: 0.5 }
                Block { size: Qt.vector3d(0.1, 2.16, 0.05); at: Qt.vector3d(-0.56, 1.08, 0.025); color: "#141820"; roughness: 0.5 }
                Block { size: Qt.vector3d(0.1, 2.16, 0.05); at: Qt.vector3d(0.56, 1.08, 0.025); color: "#141820"; roughness: 0.5 }
                Block { size: Qt.vector3d(1.0, 2.09, 0.045); at: Qt.vector3d(0, 1.065, 0.022); color: "#523524"; roughness: 0.55 }
                Repeater3D {
                    model: 4
                    Block {
                        size: Qt.vector3d(0.36, 0.82, 0.02)
                        at: Qt.vector3d(index % 2 === 0 ? -0.22 : 0.22, index < 2 ? 1.55 : 0.6, 0.05)
                        color: "#5e3d2a"
                        roughness: 0.5
                    }
                }
                Ball { radius: 0.035; at: Qt.vector3d(0.38, 1.0, 0.08); color: "#d8b46a"; metalness: 1; roughness: 0.25 }
                Block { size: Qt.vector3d(0.98, 0.012, 0.02); at: Qt.vector3d(0, 0.008, 0.01); color: "#ffcf7a"; glow: 5 }
            }

            // The art wall's print, framed and lit, between its windows.
            Node {
                position: Qt.vector3d(world.halfWidth, 1.72, 0)
                eulerRotation.y: -90
                Block { size: Qt.vector3d(1.5, 1.04, 0.04); at: Qt.vector3d(0, 0, 0.02); color: "#0b0d12"; roughness: 0.4 }
                Face {
                    w: 1.4
                    h: 0.95
                    at: Qt.vector3d(0, 0, 0.042)
                    roughness: 0.7
                    texture: Texture { sourceItem: Print {} }
                }
                SpotLight {
                    position: Qt.vector3d(0, 0.9, 0.6)
                    eulerRotation.x: -55
                    color: "#ffe2bd"
                    brightness: 6
                    coneAngle: 45
                    innerConeAngle: 20
                    quadraticFade: 0.04
                }
                // Under it, a low bench with a sculpture on it.
                Block { size: Qt.vector3d(1.4, 0.06, 0.4); at: Qt.vector3d(0, -1.27, 0.2); color: "#3b2a1f"; roughness: 0.5 }
                Block { size: Qt.vector3d(0.06, 0.4, 0.36); at: Qt.vector3d(-0.6, -1.5, 0.2); color: "#1a1410"; roughness: 0.5 }
                Block { size: Qt.vector3d(0.06, 0.4, 0.36); at: Qt.vector3d(0.6, -1.5, 0.2); color: "#1a1410"; roughness: 0.5 }
                Ball { radius: 0.11; at: Qt.vector3d(-0.3, -1.13, 0.2); color: "#c9a96a"; metalness: 1; roughness: 0.2 }
                Ball { radius: 0.07; at: Qt.vector3d(-0.15, -1.17, 0.25); color: "#2f6a6e"; roughness: 0.3 }
                Rod { radius: 0.06; tall: 0.22; at: Qt.vector3d(0.35, -1.13, 0.2); color: "#e9e2d2"; roughness: 0.3 }
            }

            // The bookcase wall: a long credenza with books, and a clock
            // above it that keeps the time.
            Node {
                position: Qt.vector3d(-world.halfWidth, 0, 0)
                eulerRotation.y: 90
                Block { size: Qt.vector3d(1.9, 0.62, 0.45); at: Qt.vector3d(0, 0.41, 0.225); color: "#4a2f1f"; roughness: 0.45 }
                Repeater3D {
                    model: 3
                    Block {
                        size: Qt.vector3d(0.006, 0.5, 0.01)
                        at: Qt.vector3d(-0.95 + (index + 1) * 0.475, 0.41, 0.452)
                        color: "#24170e"
                    }
                }
                Repeater3D {
                    model: 4
                    Rod { radius: 0.02; tall: 0.1; at: Qt.vector3d(index % 2 === 0 ? -0.85 : 0.85, 0.05, index < 2 ? 0.07 : 0.38); color: "#c9a96a"; metalness: 1; roughness: 0.3 }
                }
                Repeater3D {
                    model: 14
                    Block {
                        readonly property int seed: index * 37 + 11
                        readonly property real tall: 0.18 + (seed % 5) * 0.015
                        size: Qt.vector3d(0.035 + (seed % 3) * 0.01, tall, 0.17 + (seed % 4) * 0.01)
                        at: Qt.vector3d(-0.85 + index * 0.06, 0.72 + tall / 2, 0.2)
                        color: ["#8e3b3b", "#2f5d73", "#c9a54e", "#3d6b4b", "#6b4a8e", "#b8643b", "#d6cfbd", "#24304a"][seed % 8]
                        roughness: 0.7
                    }
                }
                Block { size: Qt.vector3d(0.3, 0.05, 0.22); at: Qt.vector3d(0.45, 0.745, 0.2); color: "#d6cfbd"; roughness: 0.7 }
                Block { size: Qt.vector3d(0.28, 0.04, 0.2); at: Qt.vector3d(0.45, 0.79, 0.2); color: "#2f5d73"; roughness: 0.7 }
                Ball { radius: 0.06; at: Qt.vector3d(0.45, 0.87, 0.2); color: "#c9a96a"; metalness: 1; roughness: 0.2 }

                // The clock.
                Node {
                    id: clock
                    property date now: new Date()
                    position: Qt.vector3d(0, 1.72, 0)
                    Rod { radius: 0.27; tall: 0.04; at: Qt.vector3d(0, 0, 0.02); eulerRotation.x: 90; color: "#1a1d24"; metalness: 0.5; roughness: 0.4 }
                    Rod { radius: 0.24; tall: 0.01; at: Qt.vector3d(0, 0, 0.042); eulerRotation.x: 90; color: "#ebe5d6"; roughness: 0.6 }
                    Repeater3D {
                        model: 12
                        Node {
                            eulerRotation.z: -index * 30
                            Block { size: Qt.vector3d(0.012, index % 3 === 0 ? 0.05 : 0.025, 0.006); at: Qt.vector3d(0, 0.2, 0.05); color: "#1a1d24" }
                        }
                    }
                    Node {
                        eulerRotation.z: -((clock.now.getHours() % 12) * 30 + clock.now.getMinutes() * 0.5)
                        Block { size: Qt.vector3d(0.018, 0.13, 0.006); at: Qt.vector3d(0, 0.055, 0.056); color: "#1a1d24" }
                    }
                    Node {
                        eulerRotation.z: -(clock.now.getMinutes() * 6 + clock.now.getSeconds() * 0.1)
                        Block { size: Qt.vector3d(0.012, 0.19, 0.006); at: Qt.vector3d(0, 0.085, 0.062); color: "#1a1d24" }
                    }
                    Node {
                        eulerRotation.z: -clock.now.getSeconds() * 6
                        Block { size: Qt.vector3d(0.005, 0.2, 0.004); at: Qt.vector3d(0, 0.08, 0.067); color: "#c0392b" }
                    }
                    Ball { radius: 0.014; at: Qt.vector3d(0, 0, 0.07); color: "#c0392b" }
                    Timer {
                        interval: 1000
                        running: true
                        repeat: true
                        onTriggered: clock.now = new Date()
                    }
                }
            }

            // The rug, and on it the sofa facing the window wall, an
            // armchair, and a coffee table with books and a mug.
            Face {
                w: 3.4
                h: 4.6
                at: Qt.vector3d(0, 0.006, 0.3)
                eulerRotation.x: -90
                roughness: 0.95
                texture: Texture { sourceItem: Rug {} }
            }
            Node {
                id: sofa
                position: Qt.vector3d(0, 0, 1.85)
                readonly property color fabric: "#2f5d50"
                Block { size: Qt.vector3d(2.2, 0.3, 0.9); at: Qt.vector3d(0, 0.25, 0); color: sofa.fabric; roughness: 0.95 }
                Repeater3D {
                    model: 3
                    Block {
                        size: Qt.vector3d(0.62, 0.14, 0.7)
                        at: Qt.vector3d(-0.66 + index * 0.66, 0.47, -0.08)
                        color: Qt.lighter(sofa.fabric, 1.08)
                        roughness: 0.95
                    }
                }
                Block { size: Qt.vector3d(2.2, 0.44, 0.2); at: Qt.vector3d(0, 0.62, 0.35); color: sofa.fabric; roughness: 0.95 }
                Repeater3D {
                    model: 3
                    Block {
                        size: Qt.vector3d(0.6, 0.36, 0.12)
                        at: Qt.vector3d(-0.66 + index * 0.66, 0.68, 0.22)
                        eulerRotation.x: -10
                        color: Qt.lighter(sofa.fabric, 1.12)
                        roughness: 0.95
                    }
                }
                Block { size: Qt.vector3d(0.16, 0.6, 0.9); at: Qt.vector3d(-1.02, 0.4, 0); color: sofa.fabric; roughness: 0.95 }
                Block { size: Qt.vector3d(0.16, 0.6, 0.9); at: Qt.vector3d(1.02, 0.4, 0); color: sofa.fabric; roughness: 0.95 }
                Block { size: Qt.vector3d(0.38, 0.3, 0.1); at: Qt.vector3d(-0.7, 0.62, 0.1); eulerRotation: Qt.vector3d(-15, 0, 12); color: "#c9a54e"; roughness: 0.9 }
                Repeater3D {
                    model: 4
                    Rod { radius: 0.025; tall: 0.1; at: Qt.vector3d(index % 2 === 0 ? -1.0 : 1.0, 0.05, index < 2 ? -0.38 : 0.38); color: "#c9a96a"; metalness: 1; roughness: 0.3 }
                }
            }
            Node {
                id: armchair
                position: Qt.vector3d(-2.2, 0, 0.1)
                eulerRotation.y: -75
                readonly property color fabric: "#b8862f"
                Block { size: Qt.vector3d(0.85, 0.32, 0.85); at: Qt.vector3d(0, 0.26, 0); color: armchair.fabric; roughness: 0.9 }
                Block { size: Qt.vector3d(0.6, 0.12, 0.66); at: Qt.vector3d(0, 0.48, -0.06); color: Qt.lighter(armchair.fabric, 1.1); roughness: 0.9 }
                Block { size: Qt.vector3d(0.85, 0.42, 0.18); at: Qt.vector3d(0, 0.63, 0.33); color: armchair.fabric; roughness: 0.9 }
                Block { size: Qt.vector3d(0.13, 0.52, 0.85); at: Qt.vector3d(-0.36, 0.4, 0); color: armchair.fabric; roughness: 0.9 }
                Block { size: Qt.vector3d(0.13, 0.52, 0.85); at: Qt.vector3d(0.36, 0.4, 0); color: armchair.fabric; roughness: 0.9 }
            }
            Node {
                id: table
                position: Qt.vector3d(0, 0, 0.15)
                Block { size: Qt.vector3d(1.2, 0.05, 0.65); at: Qt.vector3d(0, 0.42, 0); color: "#8a5a35"; roughness: 0.35 }
                Repeater3D {
                    model: 4
                    Rod { radius: 0.022; tall: 0.4; at: Qt.vector3d(index % 2 === 0 ? -0.54 : 0.54, 0.2, index < 2 ? -0.27 : 0.27); color: "#15171c"; metalness: 0.8; roughness: 0.35 }
                }
                Block { size: Qt.vector3d(0.32, 0.04, 0.24); at: Qt.vector3d(-0.3, 0.465, 0.05); eulerRotation.y: 8; color: "#8e3b3b"; roughness: 0.7 }
                Block { size: Qt.vector3d(0.3, 0.035, 0.22); at: Qt.vector3d(-0.3, 0.5, 0.05); eulerRotation.y: -5; color: "#d6cfbd"; roughness: 0.7 }
                Block { size: Qt.vector3d(0.26, 0.03, 0.2); at: Qt.vector3d(-0.29, 0.53, 0.05); eulerRotation.y: 14; color: "#24304a"; roughness: 0.7 }
                Rod { radius: 0.045; tall: 0.1; at: Qt.vector3d(0.25, 0.495, -0.1); color: "#f2ede3"; roughness: 0.25 }
                Rod { radius: 0.04; tall: 0.006; at: Qt.vector3d(0.25, 0.543, -0.1); color: "#3b2416"; roughness: 0.2 }
                Block { size: Qt.vector3d(0.015, 0.06, 0.05); at: Qt.vector3d(0.305, 0.5, -0.1); color: "#f2ede3"; roughness: 0.25 }
                Ball { radius: 0.07; squash: 0.8; at: Qt.vector3d(0.4, 0.5, 0.15); color: "#2f6a6e"; roughness: 0.15; metalness: 0.2 }
            }

            // The lamp over the table: a cord from the ceiling, a brass
            // shade, a bulb, and the light that throws the furniture's
            // shadows. Hung no lower than the windows' upper edges.
            Node {
                position: Qt.vector3d(0, 0, 0.15)
                Rod { radius: 0.006; tall: 0.32; at: Qt.vector3d(0, world.roomHeight - 0.16, 0); color: "#111111" }
                Model {
                    source: "#Cone"
                    position: Qt.vector3d(0, world.roomHeight - 0.5, 0)
                    scale: Qt.vector3d(0.0055, 0.0018, 0.0055)
                    materials: PrincipledMaterial { baseColor: "#b08d4f"; metalness: 1; roughness: 0.3 }
                }
                Ball { radius: 0.05; at: Qt.vector3d(0, world.roomHeight - 0.47, 0); color: "#fff4d6"; glow: 8 }
                PointLight {
                    position: Qt.vector3d(0, world.roomHeight - 0.52, 0)
                    color: "#ffd2a0"
                    brightness: 9
                    quadraticFade: 0.02
                    castsShadow: true
                    shadowMapQuality: Light.ShadowMapQualityVeryHigh
                    shadowBias: 0.02 * world.m
                    shadowMapFar: 12 * world.m
                    shadowFactor: 55
                    softShadowQuality: Light.PCF8
                }
            }

            // A floor lamp in the corner by the art wall, lit.
            Node {
                position: Qt.vector3d(world.halfWidth - 0.32, 0, -world.halfDepth + 0.32)
                Rod { radius: 0.15; tall: 0.03; at: Qt.vector3d(0, 0.015, 0); color: "#15171c"; metalness: 0.8; roughness: 0.3 }
                Rod { radius: 0.014; tall: 1.5; at: Qt.vector3d(0, 0.78, 0); color: "#c9a96a"; metalness: 1; roughness: 0.3 }
                Rod { radius: 0.21; tall: 0.3; at: Qt.vector3d(0, 1.62, 0); color: "#f3dcb0"; roughness: 0.9; glow: 0.7 }
                PointLight {
                    position: Qt.vector3d(0, 1.6, 0)
                    color: "#ffcf96"
                    brightness: 5
                    quadraticFade: 0.03
                    castsShadow: true
                    shadowMapQuality: Light.ShadowMapQualityHigh
                    shadowBias: 0.02 * world.m
                    shadowMapFar: 10 * world.m
                    shadowFactor: 45
                }
            }

            // Plants in the other three corners.
            Repeater3D {
                model: [[-1, -1], [-1, 1], [1, 1]]
                Node {
                    id: plant
                    readonly property int seed: index
                    position: Qt.vector3d(modelData[0] * (world.halfWidth - 0.32), 0, modelData[1] * (world.halfDepth - 0.32))
                    Rod { radius: 0.2; tall: 0.38; at: Qt.vector3d(0, 0.19, 0); color: "#a85a3a"; roughness: 0.8 }
                    Rod { radius: 0.18; tall: 0.02; at: Qt.vector3d(0, 0.37, 0); color: "#2a1a10"; roughness: 1 }
                    Repeater3D {
                        model: 7
                        Ball {
                            readonly property real turn: index * 2.4 + plant.seed
                            readonly property real out: index === 0 ? 0 : 0.12 + (index % 3) * 0.04
                            radius: 0.16 + ((index + plant.seed) % 3) * 0.05
                            squash: 1.25
                            at: Qt.vector3d(Math.cos(turn) * out, 0.65 + (index % 4) * 0.17, Math.sin(turn) * out)
                            color: ["#2f6b3c", "#3d7d47", "#285a33", "#4a8a52"][(index + plant.seed) % 4]
                            roughness: 0.75
                        }
                    }
                }
            }
        }
    }
}
