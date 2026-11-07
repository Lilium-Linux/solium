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
//
// It comes and goes, never at once. Atrium names the card's size as the
// window starts for the strip, while the window is still drawn at its own;
// laid out for the card, the shadow was then several times its size on
// screen, cast in full in one frame (218 000 pixels darkened, from none, in a
// nested capture). So it fades in over the flight, faint while the window is
// still large, and out as a card goes back to the stage, at the size it had;
// and a card resized as the strip changes eases to its new size.
Item {
    id: cast

    property var windowValues: ({})
    property int bleedTop: 0
    property int bleedLeft: 0

    readonly property var card: windowValues.atrium || null
    // The card's size, kept while the shadow fades after the window leaves
    // the strip, and eased from one size to the next.
    property real cardScale: card && card.scale > 0 ? card.scale : 1
    onCardChanged: {
        if (card && card.scale > 0) {
            cardScale = card.scale
        }
    }
    Behavior on cardScale { NumberAnimation { duration: 220; easing.type: Easing.OutCubic } }
    // Screen pixels: how far the shadow spreads, and how far down it falls.
    readonly property real spread: 30 / cardScale
    readonly property real drop: 9 / cardScale
    readonly property int rings: 16

    // How much of it is cast, 0 to 1: in over Atrium's flight to the strip,
    // slowly at first, out quickly as the card goes back.
    property real strength: card ? 1 : 0
    Behavior on strength {
        NumberAnimation {
            duration: cast.card ? 420 : 200
            easing.type: cast.card ? Easing.InQuad : Easing.OutQuad
        }
    }

    property bool dormant: cast.strength <= 0
    visible: cast.strength > 0

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
            opacity: 0.045 * cast.strength
        }
    }
}
