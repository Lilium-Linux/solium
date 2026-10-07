// A floating capsule island: the bottom bar's one visual unit, reused by
// every piece of it so the bar reads as one thing rather than several.
// `docs/ricing.md` has the bar's own section; this is its building block.

import QtQuick
import Solium

Item {
    id: capsule

    // Children land in the row, so a piece of the bar is just
    // `Capsule { Text { ... } }`.
    default property alias content: row.data
    property alias spacing: row.spacing

    width: row.implicitWidth + Theme.margin
    height: 36

    Rectangle {
        anchors.fill: parent
        radius: height / 2
        color: Theme.surface
        border { width: 1; color: Theme.edge }
    }

    Row {
        id: row
        anchors.centerIn: parent
        spacing: Theme.gap
    }
}
