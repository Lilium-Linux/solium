// The clock: date and time, no seconds. Plain QML `Date` on a `Timer`
// aligned to the minute boundary -- there is no `Solium.Clock` singleton on
// this platform (`import Solium` brings `Theme`, `Keyboard`, `Monitors`,
// `Windows` and `Workspaces`, and no others: docs/shell-boundary.md). This is
// real wall-clock time either way, not a stand-in for one.

import QtQuick
import Solium

Capsule {
    id: clock

    property date now: new Date()

    Timer {
        // Re-armed every tick to the next minute boundary, so nothing here
        // ticks faster than the clock's own minute.
        interval: Math.max(1000, 1000 * (60 - clock.now.getSeconds()))
        running: true
        repeat: true
        triggeredOnStart: false
        onTriggered: clock.now = new Date()
    }

    Text {
        text: Qt.formatDateTime(clock.now, "ddd d  hh:mm")
        color: Theme.text
        font { pixelSize: Theme.fontSize; family: Theme.fontFamily }
    }
}
