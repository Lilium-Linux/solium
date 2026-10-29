import QtQuick
import Solium

// A square that moves ten times a second: a stream of this that stops
// changing has stopped streaming.
Rectangle {
    color: "#192532"
    property int step: 0
    Timer {
        interval: 100
        running: true
        repeat: true
        onTriggered: parent.step = (parent.step + 1) % 20
    }
    Rectangle { x: 20 + parent.step * 30; y: 60; width: 40; height: 40; color: "#e94836" }
}
