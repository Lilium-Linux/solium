// One wall of the room, painted flat: plaster above a panelled wainscot,
// a baseboard and a crown, a picture light over each place a window hangs,
// and one thing of its own between them. The window manager poses it in
// three dimensions; nothing here knows where it stands.
//
// Everything is measured in metres of a wall 2.8 m high, so the same wall
// reads right at any size it is declared at.

import QtQuick

Item {
    id: wall

    // Where along the wall a window hangs, as shares of its length.
    property var slots: []
    // A hung window's size as shares of the wall, and its middle's height
    // as a share of the wall from the top.
    property real hangWidth: 0.25
    property real hangHeight: 0.42
    property real hangAt: 0.43
    // What stands between the windows: "window", "door", "shelf" or "art".
    property string feature: ""

    // A metre.
    readonly property real m: height / 2.8

    // How many of something fit, never more than `most`: a scene is sized
    // one side at a time, and a count taken between the two would ask for
    // thousands of items for a wall one pixel high.
    function fits(count, most) {
        return isFinite(count) ? Math.max(0, Math.min(most, Math.ceil(count))) : 0
    }
    readonly property int panels: Math.max(1, fits(Math.floor(width / (0.9 * m)), 16))

    // The plaster: darker into the corners of the ceiling and the floor.
    Rectangle {
        anchors.fill: parent
        gradient: Gradient {
            GradientStop { position: 0.0; color: "#1f2533" }
            GradientStop { position: 0.35; color: "#2c3446" }
            GradientStop { position: 0.7; color: "#2a3243" }
            GradientStop { position: 1.0; color: "#1c212c" }
        }
    }

    // A faint pinstripe in the paper, which is what tells the eye how far
    // along a wall it is looking.
    Repeater {
        model: wall.fits(wall.width / (0.25 * wall.m), 64)
        Rectangle {
            x: index * 0.25 * wall.m
            y: 0
            width: Math.max(1, 0.012 * wall.m)
            height: wall.height - 0.9 * wall.m
            color: "#ffffff"
            opacity: 0.035
        }
    }

    // The wainscot: panels below the chair rail.
    Rectangle {
        x: 0
        y: wall.height - 0.9 * wall.m
        width: wall.width
        height: 0.9 * wall.m
        color: "#1a1f2a"
    }
    Repeater {
        model: wall.panels
        Rectangle {
            readonly property real across: wall.width / wall.panels
            x: index * across + 0.06 * wall.m
            y: wall.height - 0.8 * wall.m
            width: across - 0.12 * wall.m
            height: 0.6 * wall.m
            radius: 0.02 * wall.m
            border.width: Math.max(1, 0.012 * wall.m)
            border.color: "#3a4357"
            gradient: Gradient {
                GradientStop { position: 0.0; color: "#252c3a" }
                GradientStop { position: 1.0; color: "#1c2230" }
            }
        }
    }

    // The chair rail, lit from above.
    Rectangle {
        x: 0
        y: wall.height - 0.93 * wall.m
        width: wall.width
        height: 0.05 * wall.m
        gradient: Gradient {
            GradientStop { position: 0.0; color: "#5a6479" }
            GradientStop { position: 0.4; color: "#394255" }
            GradientStop { position: 1.0; color: "#1b202b" }
        }
    }

    // The baseboard, where the wall meets the floor.
    Rectangle {
        x: 0
        y: wall.height - 0.13 * wall.m
        width: wall.width
        height: 0.13 * wall.m
        gradient: Gradient {
            GradientStop { position: 0.0; color: "#3b4458" }
            GradientStop { position: 0.08; color: "#151a23" }
            GradientStop { position: 1.0; color: "#0e1118" }
        }
    }

    // The crown, where it meets the ceiling.
    Rectangle {
        x: 0
        y: 0
        width: wall.width
        height: 0.12 * wall.m
        gradient: Gradient {
            GradientStop { position: 0.0; color: "#11141c" }
            GradientStop { position: 0.85; color: "#2d3446" }
            GradientStop { position: 1.0; color: "#4a5368" }
        }
    }

    // Each place a window hangs: an empty frame, which the window covers,
    // the wash of its picture light, and the light itself.
    Repeater {
        model: wall.slots
        Item {
            readonly property real middle: modelData * wall.width
            readonly property real hangTop: (wall.hangAt - wall.hangHeight / 2) * wall.height
            anchors.fill: parent

            Pool {
                x: parent.middle - 1.5 * wall.m
                y: parent.hangTop - 0.55 * wall.m
                width: 3.0 * wall.m
                height: 2.2 * wall.m
                tint: "#ffcf8f"
                strength: 0.32
            }
            Rectangle {
                x: parent.middle - wall.hangWidth * wall.width / 2
                y: parent.hangTop
                width: wall.hangWidth * wall.width
                height: wall.hangHeight * wall.height
                color: "#0dffffff"
                border.width: Math.max(1, 0.02 * wall.m)
                border.color: "#4b556c"
            }
            // The lamp: an arm out of the wall and a brass hood, its
            // underside burning.
            Rectangle {
                x: parent.middle - 0.02 * wall.m
                y: parent.hangTop - 0.3 * wall.m
                width: 0.04 * wall.m
                height: 0.16 * wall.m
                color: "#7d6a45"
            }
            Rectangle {
                x: parent.middle - 0.32 * wall.m
                y: parent.hangTop - 0.18 * wall.m
                width: 0.64 * wall.m
                height: 0.07 * wall.m
                radius: 0.03 * wall.m
                gradient: Gradient {
                    GradientStop { position: 0.0; color: "#c9a96a" }
                    GradientStop { position: 0.6; color: "#8a6f3e" }
                    GradientStop { position: 1.0; color: "#fff1c9" }
                }
            }
        }
    }

    // ---- What stands between the windows ----

    // A window onto a city at dusk.
    Item {
        visible: wall.feature === "window"
        x: wall.width / 2 - 0.7 * wall.m
        y: wall.height - 2.55 * wall.m
        width: 1.4 * wall.m
        height: 1.55 * wall.m

        Pool {
            x: -0.9 * wall.m
            y: -0.5 * wall.m
            width: parent.width + 1.8 * wall.m
            height: parent.height + 1.2 * wall.m
            tint: "#7f9cff"
            strength: 0.16
        }
        Rectangle {
            anchors.fill: parent
            color: "#0c0f16"
        }
        Item {
            id: pane
            anchors.fill: parent
            anchors.margins: 0.07 * wall.m
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
                model: 46
                Rectangle {
                    x: ((index * 7919) % 1000) / 1000 * pane.width
                    y: ((index * 104729) % 997) / 997 * pane.height * 0.55
                    width: Math.max(1, (index % 5 === 0 ? 0.018 : 0.01) * wall.m)
                    height: width
                    radius: width / 2
                    color: "#ffffff"
                    opacity: 0.35 + ((index * 31) % 60) / 100
                }
            }
            // The moon.
            Rectangle {
                x: pane.width * 0.66
                y: pane.height * 0.14
                width: 0.2 * wall.m
                height: width
                radius: width / 2
                color: "#f6edcf"
            }
            Pool {
                x: pane.width * 0.66 + 0.1 * wall.m - 0.35 * wall.m
                y: pane.height * 0.14 + 0.1 * wall.m - 0.35 * wall.m
                width: 0.7 * wall.m
                height: 0.7 * wall.m
                tint: "#f6edcf"
                strength: 0.35
            }
            // The skyline, each building's height and lit windows from its
            // index.
            Repeater {
                model: 14
                Rectangle {
                    id: building
                    readonly property real tall: (0.25 + ((index * 37) % 10) / 22) * pane.height
                    x: index * pane.width / 13 - 0.04 * wall.m
                    y: pane.height - tall
                    width: pane.width / 13 + ((index * 13) % 4) * 0.02 * wall.m
                    height: tall
                    color: index % 2 === 0 ? "#0b0d17" : "#11131f"
                    Repeater {
                        model: 12
                        Rectangle {
                            x: (index % 3) * building.width / 3 + 0.02 * wall.m
                            y: Math.floor(index / 3) * 0.09 * wall.m + 0.06 * wall.m
                            width: building.width / 3 - 0.04 * wall.m
                            height: 0.04 * wall.m
                            color: "#ffcc72"
                            opacity: ((index * 7 + building.x) % 5) < 2 ? 0.85 : 0.0
                        }
                    }
                }
            }
        }
        // The frame's bars, and the sill.
        Rectangle {
            x: parent.width / 2 - 0.025 * wall.m
            y: 0
            width: 0.05 * wall.m
            height: parent.height
            color: "#0c0f16"
        }
        Rectangle {
            x: 0
            y: parent.height * 0.42
            width: parent.width
            height: 0.05 * wall.m
            color: "#0c0f16"
        }
        Rectangle {
            x: -0.08 * wall.m
            y: parent.height
            width: parent.width + 0.16 * wall.m
            height: 0.07 * wall.m
            gradient: Gradient {
                GradientStop { position: 0.0; color: "#6b7389" }
                GradientStop { position: 1.0; color: "#232836" }
            }
        }
    }

    // A door, a line of warm light under it.
    Item {
        visible: wall.feature === "door"
        x: wall.width / 2 - 0.55 * wall.m
        y: wall.height - 2.2 * wall.m
        width: 1.1 * wall.m
        height: 2.2 * wall.m

        Rectangle {
            anchors.fill: parent
            color: "#12161f"
        }
        Rectangle {
            id: slab
            x: 0.08 * wall.m
            y: 0.08 * wall.m
            width: parent.width - 0.16 * wall.m
            height: parent.height - 0.08 * wall.m
            gradient: Gradient {
                orientation: Gradient.Horizontal
                GradientStop { position: 0.0; color: "#3f291c" }
                GradientStop { position: 0.5; color: "#563826" }
                GradientStop { position: 1.0; color: "#3a2519" }
            }
            Repeater {
                model: 4
                Rectangle {
                    x: (index % 2) * slab.width / 2 + 0.08 * wall.m
                    y: Math.floor(index / 2) * slab.height * 0.52 + 0.1 * wall.m
                    width: slab.width / 2 - 0.16 * wall.m
                    height: slab.height * 0.4
                    color: "transparent"
                    border.width: Math.max(1, 0.02 * wall.m)
                    border.color: "#2a1a11"
                }
            }
            Rectangle {
                x: slab.width - 0.16 * wall.m
                y: slab.height * 0.5
                width: 0.07 * wall.m
                height: width
                radius: width / 2
                color: "#d8b46a"
            }
        }
        Rectangle {
            x: 0.08 * wall.m
            y: parent.height - 0.02 * wall.m
            width: parent.width - 0.16 * wall.m
            height: 0.02 * wall.m
            color: "#ffcf7a"
        }
    }

    // A bookcase, its books' widths, heights and colours from their places.
    Item {
        id: shelf
        visible: wall.feature === "shelf"
        readonly property var bindings: ["#8e3b3b", "#2f5d73", "#c9a54e", "#3d6b4b", "#6b4a8e", "#b8643b", "#d6cfbd", "#24304a", "#7a2e45"]
        x: wall.width / 2 - 0.95 * wall.m
        y: wall.height - 2.35 * wall.m
        width: 1.9 * wall.m
        height: 2.22 * wall.m

        Rectangle {
            anchors.fill: parent
            color: "#2b1d14"
        }
        Rectangle {
            anchors.fill: parent
            anchors.margins: 0.05 * wall.m
            color: "#140e0a"
        }
        Repeater {
            model: 5
            Item {
                id: row
                readonly property int level: index
                x: 0.05 * wall.m
                y: 0.05 * wall.m + index * (shelf.height - 0.1 * wall.m) / 5
                width: shelf.width - 0.1 * wall.m
                height: (shelf.height - 0.1 * wall.m) / 5

                Repeater {
                    model: 22
                    Rectangle {
                        readonly property int seed: index * 31 + row.level * 17
                        readonly property real thick: (0.035 + (seed * 7 % 5) * 0.008) * wall.m
                        x: index * row.width / 22
                        y: row.height - 0.04 * wall.m - height
                        width: Math.min(thick, row.width / 22 - 1)
                        height: (0.22 + (seed * 13 % 7) * 0.02) * wall.m
                        visible: (seed % 9) !== 0
                        color: shelf.bindings[seed % shelf.bindings.length]
                        Rectangle {
                            x: 0
                            y: parent.height * 0.2
                            width: parent.width
                            height: Math.max(1, 0.012 * wall.m)
                            color: "#e8d6a4"
                            opacity: 0.6
                        }
                    }
                }
                Rectangle {
                    x: 0
                    y: row.height - 0.04 * wall.m
                    width: row.width
                    height: 0.04 * wall.m
                    gradient: Gradient {
                        GradientStop { position: 0.0; color: "#5a3e2a" }
                        GradientStop { position: 1.0; color: "#2b1d14" }
                    }
                }
            }
        }
        Pool {
            x: -0.4 * wall.m
            y: -0.3 * wall.m
            width: shelf.width + 0.8 * wall.m
            height: 1.6 * wall.m
            tint: "#ffcf8f"
            strength: 0.14
        }
    }

    // A large print in a frame, lit.
    Item {
        visible: wall.feature === "art"
        x: wall.width / 2 - 0.8 * wall.m
        y: wall.height - 2.25 * wall.m
        width: 1.6 * wall.m
        height: 1.15 * wall.m

        Pool {
            x: -0.7 * wall.m
            y: -0.5 * wall.m
            width: parent.width + 1.4 * wall.m
            height: parent.height + 1.0 * wall.m
            tint: "#ffcf8f"
            strength: 0.26
        }
        Rectangle {
            anchors.fill: parent
            color: "#0b0d12"
        }
        Rectangle {
            id: mat
            anchors.fill: parent
            anchors.margins: 0.05 * wall.m
            color: "#e6dfd1"
            Rectangle {
                id: artwork
                anchors.fill: parent
                anchors.margins: 0.1 * wall.m
                clip: true
                gradient: Gradient {
                    GradientStop { position: 0.0; color: "#1f4e5a" }
                    GradientStop { position: 1.0; color: "#0f2a33" }
                }
                Rectangle {
                    x: artwork.width * 0.08
                    y: artwork.height * 0.18
                    width: artwork.height * 0.7
                    height: width
                    radius: width / 2
                    color: "#e2735a"
                }
                Rectangle {
                    x: artwork.width * 0.42
                    y: artwork.height * 0.3
                    width: artwork.height * 0.55
                    height: width
                    radius: width / 2
                    color: "#e9b949"
                    opacity: 0.92
                }
                Rectangle {
                    x: artwork.width * 0.62
                    y: -artwork.height * 0.2
                    width: artwork.width * 0.08
                    height: artwork.height * 1.4
                    rotation: 18
                    color: "#f2ece0"
                }
            }
        }
    }
}
