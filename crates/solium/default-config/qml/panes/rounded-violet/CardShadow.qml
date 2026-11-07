import QtQuick

// Every window atrium draws casts a shadow on what is behind it (#449): one on
// the stage, and one shown as a card, so a card is seen settling over another.
// Atrium tells the frame `atrium = { card, scale, stiffness, damping }`:
// whether the window is a card or on the stage, how small it is drawn (1 on
// the stage), and the spring it flies there on. The shadow is laid out in the
// window's own pixels, so it is spread by 1 / scale to come out the same on
// screen whatever the window's size. Nothing is drawn outside atrium.
//
// Soft rather than banded: many faint rings, each a little further out and
// fainter than the last, so their edges blend into one falloff.
//
// One shadow, never two and never none. As a window flies from the stage into
// its card the shadow shrinks with it on the same spring, from where it is and
// at the speed it has, so on screen it stays the shadow it was: it used to be
// laid out for the card the instant the flight began, several times its size
// around a window still at its own (218 000 pixels darkened in one frame, in
// a nested capture), and to drop away the instant a card came back. It fades
// in only as atrium first draws a window, and out as it stops.
Item {
    id: cast

    property var windowValues: ({})
    property int bleedTop: 0
    property int bleedLeft: 0

    readonly property var atrium: windowValues.atrium || null
    readonly property real target: atrium && atrium.scale > 0 ? atrium.scale : 1
    readonly property real stiffness: atrium && atrium.stiffness > 0 ? atrium.stiffness : 300
    readonly property real damping: atrium && atrium.damping > 0 ? atrium.damping : 32

    // How small the window is drawn this frame: `target`, reached on
    // atrium's spring as its picture reaches it.
    property real scaleNow: target
    property real speed: 0
    property real startFrom: 0
    property real startSpeed: 0

    // How far from the target the spring is `t` seconds after leaving
    // `from` at `v`: the closed form for its damping, as the compositor's.
    function offsetAt(t, from, v) {
        const w = Math.sqrt(stiffness)
        const zeta = damping / (2 * w)
        if (Math.abs(zeta - 1) < 0.02) {
            return (from + (v + w * from) * t) * Math.exp(-w * t)
        }
        if (zeta < 1) {
            const d = w * Math.sqrt(1 - zeta * zeta)
            return Math.exp(-zeta * w * t) * (from * Math.cos(d * t) + (v + zeta * w * from) / d * Math.sin(d * t))
        }
        const r = w * Math.sqrt(zeta * zeta - 1)
        const slow = -zeta * w + r, fast = -zeta * w - r
        const a = (v - fast * from) / (slow - fast)
        return a * Math.exp(slow * t) + (from - a) * Math.exp(fast * t)
    }

    onTargetChanged: {
        // Not yet atrium's: no flight to follow, it is where it is told.
        if (strength <= 0) {
            scaleNow = target
            speed = 0
            return
        }
        startFrom = scaleNow - target
        startSpeed = speed
        flight.restart()
    }

    // The flight's own time, in seconds of the compositor's clock: a plain
    // animation of it, which the clock everything else moves on advances,
    // not FrameAnimation's elapsedTime, which counts the wall's time.
    property real flightTime: 0
    NumberAnimation {
        id: flight
        target: cast
        property: "flightTime"
        from: 0
        to: 4
        duration: 4000
    }
    onFlightTimeChanged: {
        if (!flight.running) {
            return
        }
        const t = flightTime
        const off = offsetAt(t, startFrom, startSpeed)
        speed = (offsetAt(t + 0.001, startFrom, startSpeed) - off) / 0.001
        if (Math.abs(off) < 0.0005 && Math.abs(speed) < 0.005) {
            scaleNow = target
            speed = 0
            flight.stop()
            return
        }
        scaleNow = Math.max(0.02, target + off)
    }

    // Screen pixels: how far the shadow spreads, and how far down it falls.
    readonly property real spread: 30 / scaleNow
    readonly property real drop: 9 / scaleNow
    readonly property int rings: 16

    // How much of it is cast, 0 to 1: in as atrium first draws the window,
    // out as it stops.
    property real strength: atrium ? 1 : 0
    Behavior on strength { NumberAnimation { duration: 240; easing.type: Easing.OutQuad } }

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
            radius: 12 / cast.scaleNow + out
            color: "#000000"
            opacity: 0.045 * cast.strength
        }
    }
}
