// A pointer scene with a glow, reduced to what this harness can read back.
//
// `cursor.scene` (#213) is a scene sized by its own root, and MultiEffect is
// a shader: it draws on the GPU path and not on the software one, so this is
// the one place that can show it in a pointer scene. The drawing is a white
// square in the middle; the glow is the only thing that can put a pixel
// beside it. Owned by the harness, so a pointer of anybody's design can change
// without that being a regression here.
import QtQuick
import QtQuick.Effects

Item {
    // Its own size, as a pointer scene's is: the host does not write it.
    width: 48
    height: 48

    MultiEffect {
        source: drawing
        anchors.fill: drawing
        shadowEnabled: true
        shadowColor: "#7aa2ff"
        shadowBlur: 1.0
        shadowOpacity: 1.0
        shadowHorizontalOffset: 0
        shadowVerticalOffset: 0
        blurMax: 16
    }

    Rectangle {
        id: drawing
        x: 16
        y: 16
        width: 16
        height: 16
        color: "white"
    }
}
