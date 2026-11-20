// A pool of light: an ellipse filling this item, brightest in its middle
// and gone at its edge. What the room is lit with, painted onto its walls,
// floor and ceiling, since every surface of the room is a picture.

import QtQuick
import QtQuick.Shapes

Item {
    id: pool

    // The light's colour, and how strong it is in the middle.
    property color tint: "#ffd29a"
    property real strength: 0.3

    Shape {
        // Drawn as a circle of a hundred and stretched to the item, so the
        // circle of a radial gradient is an ellipse of any shape.
        width: 100
        height: 100
        transform: Scale {
            xScale: pool.width / 100
            yScale: pool.height / 100
        }

        ShapePath {
            strokeColor: "transparent"
            fillGradient: RadialGradient {
                centerX: 50
                centerY: 50
                centerRadius: 50
                focalX: 50
                focalY: 50
                GradientStop { position: 0.0; color: Qt.alpha(pool.tint, pool.strength) }
                GradientStop { position: 0.45; color: Qt.alpha(pool.tint, pool.strength * 0.45) }
                GradientStop { position: 1.0; color: Qt.alpha(pool.tint, 0) }
            }
            startX: 0
            startY: 0
            PathLine { x: 100; y: 0 }
            PathLine { x: 100; y: 100 }
            PathLine { x: 0; y: 100 }
            PathLine { x: 0; y: 0 }
        }
    }
}
