// A Timer that repaints, and nothing else: `timer_between_frames` in main.rs.
import QtQuick

Rectangle {
    id: root

    // Written by the harness, to stop the Timer before the cases after it.
    property bool ticking: true
    property int fired: 0

    color: fired % 2 === 0 ? "#3a3f4b" : "#c0c4cc"

    Timer {
        interval: 50
        running: root.ticking
        repeat: true
        onTriggered: root.fired += 1
    }
}
