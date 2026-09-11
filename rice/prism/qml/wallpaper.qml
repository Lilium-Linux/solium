// Prism — the ground the glass stands on.
//
// The bar, the dock and every titlebar are translucent, and translucency only
// reads as glass when there is something varying underneath it. A flat colour
// behind an eight per cent white panel looks like a slightly lighter flat
// colour. So this is not decoration for its own sake: it is what makes the
// rest of the rice legible as frost.
//
// Everything here is a Rectangle. `Canvas` would have been the obvious way to
// draw light pools and it does not work in the compositor's QML host — the
// scene is rasterised offscreen with no scene-graph render loop, `onPaint` is
// never called, and an unpainted canvas is white rather than empty, so the
// symptom is the entire desktop turning white. See Glow.qml for what replaced
// it.
//
// A wallpaper is the largest surface in the session and it is rasterised on
// the CPU, so nothing here repaints after the first frame. The only things
// that move are twelve motes three pixels across, whose damage is
// correspondingly small.

import QtQuick
import Solium

Item {
    id: paper

    // Handed in by the wallpaper script. Ignored on purpose — this scene *is*
    // the wallpaper rather than a frame around a picture.
    property string source: ""

    Rectangle {
        anchors.fill: parent
        color: Theme.groundDeep
    }

    // A cold floor and a violet sky, so neither end of the screen is ever flat
    // black: a dock sitting on pure black has nothing to be lit by.
    Rectangle {
        anchors.fill: parent
        gradient: Gradient {
            GradientStop { position: 0.0; color: "#12936dff" }
            GradientStop { position: 0.5; color: "#00000000" }
            GradientStop { position: 1.0; color: "#0d4dd9ff" }
        }
    }

    // The three light sources. Their placement is the whole composition: the
    // violet high and right where the bar's right end sits, the cyan low and
    // left under the dock, the rose weak and central so the middle of the
    // screen is not a hole between the other two.
    Glow {
        tint: Theme.violet
        strength: 0.24
        width: paper.width * 1.05
        height: paper.width * 1.05
        x: paper.width * 0.74 - width / 2
        y: paper.height * 0.10 - height / 2
    }
    Glow {
        tint: Theme.cyan
        strength: 0.15
        width: paper.width * 0.95
        height: paper.width * 0.95
        x: paper.width * 0.14 - width / 2
        y: paper.height * 0.94 - height / 2
    }
    Glow {
        tint: Theme.rose
        strength: 0.08
        width: paper.width * 0.70
        height: paper.width * 0.70
        x: paper.width * 0.42 - width / 2
        y: paper.height * 0.56 - height / 2
    }

    // The prism. A shaft of light crossing the field, split into three offset
    // bands — the one literal thing in the picture, and the reason the rice is
    // called what it is. Rotated rectangles with a gradient along their length,
    // which is the one thing a linear gradient is exactly right for.
    Item {
        anchors.centerIn: parent
        width: Math.max(paper.width, paper.height) * 2.2
        height: 120
        rotation: -26
        Repeater {
            model: [
                { offset: -22, tint: Theme.rose, alpha: 0.10 },
                { offset: 0, tint: Theme.violet, alpha: 0.16 },
                { offset: 22, tint: Theme.cyan, alpha: 0.10 }
            ]
            delegate: Rectangle {
                required property var modelData
                width: parent.width
                height: 13
                y: parent.height / 2 - height / 2 + modelData.offset
                opacity: modelData.alpha
                gradient: Gradient {
                    orientation: Gradient.Horizontal
                    GradientStop { position: 0.0; color: "#00000000" }
                    GradientStop { position: 0.5; color: modelData.tint }
                    GradientStop { position: 1.0; color: "#00000000" }
                }
            }
        }
    }

    // A vignette, over everything. Without it the light runs off the edges and
    // the screen reads as a crop of something larger rather than a composition.
    // Four edge ramps rather than a circle, because a circle here would be
    // another twenty rectangles for a difference nobody can see behind windows.
    Rectangle {
        anchors.fill: parent
        gradient: Gradient {
            GradientStop { position: 0.0; color: "#8c000000" }
            GradientStop { position: 0.32; color: "#00000000" }
            GradientStop { position: 0.72; color: "#00000000" }
            GradientStop { position: 1.0; color: "#a6000000" }
        }
    }
    Rectangle {
        anchors.fill: parent
        gradient: Gradient {
            orientation: Gradient.Horizontal
            GradientStop { position: 0.0; color: "#66000000" }
            GradientStop { position: 0.3; color: "#00000000" }
            GradientStop { position: 0.75; color: "#00000000" }
            GradientStop { position: 1.0; color: "#4d000000" }
        }
    }

    // Motes. Twelve of them, three pixels across, drifting on their own clocks
    // so the field is never quite still. Each damages about nine pixels, which
    // is the entire reason they are allowed to move at all.
    Repeater {
        model: 12
        delegate: Rectangle {
            required property int index
            width: 3
            height: 3
            radius: 1.5
            color: index % 3 === 0 ? Theme.cyan : Theme.violet
            opacity: 0.0
            x: paper.width * ((index * 0.137 + 0.06) % 1.0)

            SequentialAnimation on opacity {
                loops: Animation.Infinite
                PauseAnimation { duration: 900 * (index + 1) }
                NumberAnimation { to: 0.6; duration: 2600; easing.type: Easing.InOutQuad }
                NumberAnimation { to: 0.0; duration: 3400; easing.type: Easing.InOutQuad }
            }
            NumberAnimation on y {
                loops: Animation.Infinite
                from: paper.height * 0.92
                to: paper.height * 0.18
                duration: 22000 + index * 1700
            }
        }
    }
}
