// Prism — the bar.
//
// Drawn by the compositor, in the compositor's own QML engine, from
// `sol.surface`. That is not how a desktop is supposed to be built and this
// rice is not arguing that it is: docs/shell-boundary.md makes the case that a
// real bar is a layer-shell client, and one of those is drawn straight over
// this. What it is, is proof that the surface layer can carry a real one —
// live window list, live meters, live clock, reloaded with super+shift+r.
//
// Nothing here is handed in as a property. Properties are set once when the
// scene is built, so anything that changes has to be read from inside QML:
// the window list from the Quickshell compatibility layer, the meters from
// /proc through FileView, the time from SystemClock.

import QtQuick
import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import Solium

Item {
    id: root

    // Read by the compositor and cleared once taken.
    property string action: ""

    readonly property var active: ToplevelManager.activeToplevel
    // Quickshell's object models present themselves as `{ values: [...] }`
    // and the compatibility layer matches that shape, so the list is one
    // level in. Binding a Repeater straight to `toplevels` gives it a map,
    // which is not empty and not iterable — it silently shows nothing.
    //
    // Sorted by id, and that is not tidiness. The compositor publishes its
    // panes in stacking order, so focusing a window moves it to the front of
    // the list — which in a dock means every tile jumping sideways each time
    // you change window, and the lit one always being the leftmost. Ids are
    // handed out in the order windows were opened and never change, so
    // sorting by them is the one ordering that holds still.
    readonly property var windows: {
        const model = ToplevelManager.toplevels
        const list = (model && model.values) || []
        const copy = []
        for (let i = 0; i < list.length; ++i) {
            copy.push(list[i])
        }
        copy.sort(function (a, b) { return (a.id || 0) - (b.id || 0) })
        return copy
    }

    // --- meters ------------------------------------------------------------
    // /proc/stat's first line is cumulative jiffies since boot, so a
    // percentage is a difference between two readings and the first reading
    // can only ever produce zero. Sampled at 2s: a meter that updates faster
    // than you can read it is decoration, and this one is rasterised on the
    // CPU it is measuring.
    QtObject {
        id: meter
        property real cpu: 0
        property real memory: 0
        property real lastBusy: -1
        property real lastTotal: -1
    }

    FileView { id: statFile; path: "/proc/stat" }
    FileView { id: memFile; path: "/proc/meminfo" }

    Timer {
        interval: 2000
        running: true
        repeat: true
        triggeredOnStart: true
        onTriggered: {
            statFile.reload()
            const line = (statFile.text() || "").split("\n")[0] || ""
            const parts = line.trim().split(/\s+/).slice(1).map(Number)
            if (parts.length >= 4 && !isNaN(parts[0])) {
                const total = parts.reduce(function (a, b) { return a + (b || 0) }, 0)
                const idle = (parts[3] || 0) + (parts[4] || 0)
                const busy = total - idle
                if (meter.lastTotal >= 0 && total > meter.lastTotal) {
                    meter.cpu = Math.max(0, Math.min(1,
                        (busy - meter.lastBusy) / (total - meter.lastTotal)))
                }
                meter.lastBusy = busy
                meter.lastTotal = total
            }

            memFile.reload()
            const mem = memFile.text() || ""
            const grab = function (key) {
                const m = mem.match(new RegExp(key + ":\\s+(\\d+)"))
                return m ? Number(m[1]) : 0
            }
            const totalMem = grab("MemTotal")
            const available = grab("MemAvailable")
            if (totalMem > 0) {
                meter.memory = Math.max(0, Math.min(1, (totalMem - available) / totalMem))
            }
        }
    }

    SystemClock { id: clock; precision: 0 }

    function pad(n) { return n < 10 ? "0" + n : "" + n }

    // --- the panel ---------------------------------------------------------
    Item {
        id: panel
        anchors.fill: parent
        anchors.margins: Theme.panelInset

        Rectangle {
            id: plate
            anchors.fill: parent
            radius: Theme.panelRadius
            gradient: Gradient {
                GradientStop { position: 0.0; color: Theme.glassFillDeep }
                GradientStop { position: 1.0; color: Theme.glassFillLow }
            }
            border.width: 1
            border.color: Theme.glassRimLow
        }
        Rectangle {
            anchors.fill: plate
            radius: plate.radius
            color: Theme.glassTintIdle
        }
        // The lit edge. Inset by the radius so it stops before the corner
        // instead of cutting across it.
        Rectangle {
            anchors { top: plate.top; topMargin: 1; left: plate.left; right: plate.right }
            anchors.leftMargin: Theme.panelRadius
            anchors.rightMargin: Theme.panelRadius
            height: 1
            color: Theme.glassRim
        }

        // --- left: identity and the way into the deck ----------------------
        Row {
            id: left
            anchors { left: parent.left; leftMargin: 14; verticalCenter: parent.verticalCenter }
            spacing: 10

            Row {
                spacing: 3
                anchors.verticalCenter: parent.verticalCenter
                Repeater {
                    model: [Theme.rose, Theme.violet, Theme.cyan]
                    delegate: Rectangle {
                        required property int index
                        required property var modelData
                        width: 3
                        height: 14 - Math.abs(index - 1) * 4
                        radius: 1.5
                        anchors.verticalCenter: parent.verticalCenter
                        color: modelData
                        opacity: deckArea.containsMouse ? 1.0 : 0.85
                        Behavior on opacity { NumberAnimation { duration: Theme.quick } }
                    }
                }
            }

            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: "prism"
                color: deckArea.containsMouse ? Theme.text : Theme.textDim
                font.family: Theme.fontFamily
                font.pixelSize: Theme.fontSize
                font.letterSpacing: 1.6
                Behavior on color { ColorAnimation { duration: Theme.quick } }
            }
        }

        MouseArea {
            id: deckArea
            anchors { left: parent.left; top: parent.top; bottom: parent.bottom }
            width: left.width + 24
            hoverEnabled: true
            onClicked: root.action = "deck"
        }

        Rectangle {
            anchors { left: left.right; leftMargin: 16; verticalCenter: parent.verticalCenter }
            width: 1
            height: 14
            color: Theme.glassRimLow
        }

        // --- centre: what has the keyboard ---------------------------------
        Text {
            anchors.centerIn: parent
            width: Math.min(implicitWidth, panel.width * 0.42)
            elide: Text.ElideRight
            horizontalAlignment: Text.AlignHCenter
            text: root.active ? (root.active.title || root.active.appId || "") : "no window focused"
            color: root.active ? Theme.text : Theme.textDim
            font.family: Theme.fontFamily
            font.pixelSize: Theme.fontSize
            opacity: 0.92
        }

        // --- right: the machine, and the time ------------------------------
        Row {
            anchors { right: parent.right; rightMargin: 14; verticalCenter: parent.verticalCenter }
            spacing: 14

            Meter { label: "cpu"; value: meter.cpu; tint: Theme.cyan }
            Meter { label: "mem"; value: meter.memory; tint: Theme.violet }

            Rectangle {
                anchors.verticalCenter: parent.verticalCenter
                width: 1
                height: 14
                color: Theme.glassRimLow
            }

            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: root.pad(clock.hours) + ":" + root.pad(clock.minutes)
                color: Theme.text
                font.family: Theme.fontFamily
                font.pixelSize: Theme.fontSize + 1
                font.letterSpacing: 1.0
            }
            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: root.pad(clock.seconds)
                color: Theme.violet
                font.family: Theme.fontFamily
                font.pixelSize: Theme.fontSize - 1
            }
        }
    }

    // A label, a bar and a number. Declared once here rather than three times
    // above, because two meters that drift apart visually are worse than one.
    component Meter: Row {
        required property string label
        required property real value
        required property color tint
        spacing: 6
        anchors.verticalCenter: parent.verticalCenter

        Text {
            anchors.verticalCenter: parent.verticalCenter
            text: parent.label
            color: Theme.textDim
            font.family: Theme.fontFamily
            font.pixelSize: Theme.fontSize - 2
            font.letterSpacing: 0.8
        }
        Rectangle {
            anchors.verticalCenter: parent.verticalCenter
            width: 46
            height: 4
            radius: 2
            color: Theme.glassShade
            Rectangle {
                width: parent.width * Math.max(0.02, parent.parent.value)
                height: parent.height
                radius: parent.radius
                color: parent.parent.tint
                Behavior on width { NumberAnimation { duration: 420; easing.type: Easing.OutCubic } }
            }
        }
        Text {
            anchors.verticalCenter: parent.verticalCenter
            width: 26
            horizontalAlignment: Text.AlignRight
            text: Math.round(parent.value * 100) + "%"
            color: Theme.text
            font.family: Theme.fontFamily
            font.pixelSize: Theme.fontSize - 2
            opacity: 0.85
        }
    }
}
