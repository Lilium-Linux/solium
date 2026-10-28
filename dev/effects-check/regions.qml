// effects-check's `regions` section: three squares of pure colour at known
// places on a transparent scene, each a `Solium.region`, one inside a Row
// in a parent scaled 2x, so the readout's mapping through a scale and a
// positioner is what the capture shows.
import QtQuick
import Solium

Item {
    Rectangle {
        x: 200; y: 150; width: 60; height: 40
        color: "#ff0000"
        Solium.region: "red"
        Solium.radius: 0
    }
    Item {
        x: 400; y: 300; scale: 2; transformOrigin: Item.TopLeft
        Row {
            spacing: 10
            Item { width: 30; height: 30 }
            Rectangle {
                width: 30; height: 30
                color: "#00ff00"
                Solium.region: "row/green"
            }
        }
    }
    Rectangle {
        x: 1100; y: 600; width: 80; height: 50
        color: "#0000ff"
        Solium.region: "blue"
        Solium.radius: [1, 2, 3, 4]
    }
}
