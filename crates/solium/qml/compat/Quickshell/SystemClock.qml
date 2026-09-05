// A clock that ticks.
//
// `precision` selects how often: the shell asks for seconds in some places and
// minutes in others, and a bar that redraws sixty times a minute to move a
// colon is a bar that costs something to look at.
import QtQuick

QtObject {
    id: root

    enum Precision { Seconds, Minutes, Hours }

    property int precision: 1
    property bool enabled: true
    readonly property date date: internal.now
    readonly property int hours: internal.now.getHours()
    readonly property int minutes: internal.now.getMinutes()
    readonly property int seconds: internal.now.getSeconds()

    property QtObject internal: QtObject {
        property date now: new Date()
    }

    property Timer ticker: Timer {
        running: root.enabled
        repeat: true
        interval: root.precision === 0 ? 1000 : 15000
        triggeredOnStart: true
        onTriggered: root.internal.now = new Date()
    }
}
