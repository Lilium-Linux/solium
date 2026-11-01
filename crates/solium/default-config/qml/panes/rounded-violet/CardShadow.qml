import QtQuick

// A window shown as one of atrium's cards casts a shadow on the card's windows
// behind it, so one is seen settling over another (#449). Atrium tells the
// frame `atrium = { card: true, scale: s }` while the window is a card, `s`
// how small the card draws it: the shadow is laid out in the window's own
// pixels, so it is spread by 1 / s to come out the same on screen whatever the
// card's size. Nothing is drawn while the window is not a card.
//
// Soft rather than banded: many faint rings, each a little further out and
// fainter than the last, so their edges blend into one falloff.
Item {
    id: cast

    property var windowValues: ({})
    property int bleedTop: 0
    property int bleedLeft: 0

    readonly property var card: windowValues.atrium || null
    readonly property real scale: card && card.scale > 0 ? card.scale : 1
    // Screen pixels: how far the shadow spreads, and how far down it falls.
    readonly property real spread: 30 / scale
    readonly property real drop: 9 / scale
    readonly property int rings: 16

    property bool dormant: !card
    visible: !!card

    Repeater {
        model: cast.rings

        Rectangle {
            required property int index

            // Out from the window's edge, the rings crowd near it and thin
            // out further away, which is where a real shadow fades.
            readonly property real share: (index + 1) / cast.rings
            readonly property real out: cast.spread * share * share

            x: cast.bleedLeft - out
            y: cast.bleedTop - out + cast.drop * share
            width: cast.width - 2 * cast.bleedLeft + 2 * out
            height: cast.height - 2 * cast.bleedTop + 2 * out
            radius: 12 / cast.scale + out
            color: "#000000"
            opacity: 0.045
        }
    }
}
