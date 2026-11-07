import QtQuick

// Every window atrium draws casts a shadow on what is behind it (#449): one on
// the stage, and one shown as a card, so a card is seen settling over another.
// Atrium tells the frame `atrium = { card, scale, stiffness, damping }`:
// whether the window is a card or on the stage, how small it is drawn (1 on
// the stage), and the spring it flies there on. The shadow is laid out in the
// window's own pixels, so it is sized by 1 / scale to come out as wanted on
// screen whatever the window's size.
//
// Soft, as a blurred one is: rings, each a rectangle a little larger or
// smaller than the window, faint and stacked, set where a normal
// distribution's quantiles fall, so how dark a point is falls off across the
// window's edge as a blur's does -- half its darkness at the edge, none
// two widths out -- with no band where the rings end. A card's is small and
// close, a stage window's larger and further down, from its drawn size; and
// it always ends inside the layer's bleed, since one cut off there would end
// in a straight line, and the row's lines would join into one under it.
//
// One shadow, never two and never none. As a window flies from the stage into
// its card the shadow shrinks with it on the same spring, from where it is and
// at the speed it has, so on screen it stays the shadow it was. It fades in
// only as atrium first draws a window, and out as it stops.
Item {
    id: cast

    property var windowValues: ({})
    property int bleedTop: 0
    property int bleedLeft: 0
    property int clientRadius: 0

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

    // On screen: how soft it is (the blur's spread, one standard deviation),
    // how far down it falls, and how dark it is at its darkest, under the
    // window. Larger as the window is drawn larger.
    readonly property real drawn: Math.min(1, scaleNow)
    readonly property real softOnScreen: 5 + 11 * drawn
    readonly property real dropOnScreen: 2 + 6 * drawn
    readonly property real darkest: 0.3
    // Scaled down as one, where it would reach past the bleed.
    readonly property real fits: Math.min(1, Math.min(bleedTop, bleedLeft) * scaleNow
        / (steps[0] * softOnScreen + dropOnScreen))
    readonly property real soft: softOnScreen * fits / scaleNow
    readonly property real drop: dropOnScreen * fits / scaleNow

    // Each ring's edge, in standard deviations out from the window's (in,
    // where negative): the quantiles of a normal distribution at the middle
    // of each sixteenth.
    readonly property var steps: [1.863, 1.318, 1.01, 0.776, 0.579, 0.402, 0.237, 0.078,
        -0.078, -0.237, -0.402, -0.579, -0.776, -1.01, -1.318, -1.863]
    // Each ring's opacity: all of them over one another are `darkest`.
    readonly property real ink: 1 - Math.pow(1 - darkest, 1 / steps.length)

    // How much of it is cast, 0 to 1: in as atrium first draws the window,
    // out as it stops.
    property real strength: atrium ? 1 : 0
    Behavior on strength { NumberAnimation { duration: 240; easing.type: Easing.OutQuad } }

    property bool dormant: cast.strength <= 0
    visible: cast.strength > 0

    Repeater {
        model: cast.steps.length

        Rectangle {
            required property int index

            readonly property real out: cast.soft * cast.steps[index]

            // One drawn wholly under the window would only be hidden.
            visible: out > -cast.drop
            x: cast.bleedLeft - out
            y: cast.bleedTop - out + cast.drop
            width: cast.width - 2 * cast.bleedLeft + 2 * out
            height: cast.height - 2 * cast.bleedTop + 2 * out
            radius: Math.max(0, cast.clientRadius + out)
            color: "#000000"
            opacity: cast.ink * cast.strength
        }
    }
}
