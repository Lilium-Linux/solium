// Shadows the shipped wallpaper.qml: the same picture, with some life.
//
// The image is a little taller than the screen, and pans as the workspaces
// slide (vertically, as workspaces.arrangement has them): workspace 1 shows
// its top, the last its bottom -- parallax, slower than the windows. While
// the workspace stack (deskstack.lua) is open it eases back -- zoomed in a
// touch, blurred, darker -- and keeps panning to the desk chosen there.
//
// Told by looks.lua: `desks` (the workspace each monitor shows, by name),
// `count`, `stack` and `stackMonitor`, and the workspaces' own motion
// (`stiffness` and `damping` of their spring, or their `duration`), which
// the pan rides so the picture lands with the windows.

import QtQuick
import QtQuick.Effects
import Solium

Item {
    id: paper

    required property string source
    property var desks: ({})
    property int count: 1
    property bool stack: false
    property string stackMonitor: ""
    property real stiffness: 0
    property real damping: 0
    property int duration: 300

    readonly property int desk: desks && desks[Solium.monitor.name] ? desks[Solium.monitor.name] : 1
    readonly property bool back: stack && stackMonitor === Solium.monitor.name

    // How much taller than the screen the picture is, for the pan.
    readonly property real travel: 0.12

    // Where the picture is going, and where it is: on the workspaces' spring
    // when they have one, from where it is and at the speed it has, so a
    // second switch on the way turns it rather than snapping it round; on
    // their duration when not. Moved on the compositor's clock, by an
    // animation running only while it is on its way, so a still desktop
    // leaves the clock still.
    readonly property real target: -height * travel * (count > 1 ? (desk - 1) / (count - 1) : 0)
    readonly property bool springy: stiffness > 0 && damping > 0
    property real panned: target
    property real speed: 0
    property real startFrom: 0
    property real startSpeed: 0

    // How far from `to` a spring is `t` seconds after leaving `from` at
    // `v`: the closed form for its damping, as the compositor's own.
    function offsetAt(t, from, v) {
        const k = stiffness, c = damping, w = Math.sqrt(k)
        const zeta = c / (2 * w)
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
        if (!springy || width <= 0) {
            return
        }
        startFrom = panned - target
        startSpeed = speed
        pan.restart()
    }

    // The pan's own time, in seconds of the compositor's clock: a plain
    // animation of it, which the clock the windows slide on advances, not
    // FrameAnimation's elapsedTime, which counts the wall's time.
    property real panTime: 0
    NumberAnimation {
        id: pan
        target: paper
        property: "panTime"
        from: 0
        to: 4
        duration: 4000
    }
    onPanTimeChanged: {
        if (!pan.running) {
            return
        }
        const t = panTime
        const off = offsetAt(t, startFrom, startSpeed)
        speed = (offsetAt(t + 0.001, startFrom, startSpeed) - off) / 0.001
        if (Math.abs(off) < 0.25 && Math.abs(speed) < 4) {
            panned = target
            speed = 0
            pan.stop()
            return
        }
        panned = target + off
    }

    Rectangle {
        anchors.fill: parent
        color: "#0d0d0f"
    }

    Item {
        id: picture
        anchors.fill: parent

        Image {
            id: image
            width: parent.width
            height: parent.height * (1 + paper.travel)
            y: paper.springy ? paper.panned : paper.target
            source: paper.source
            fillMode: Image.PreserveAspectCrop
            smooth: true
            mipmap: true
            cache: false
            asynchronous: false
            Behavior on y {
                enabled: !paper.springy
                NumberAnimation { duration: paper.duration; easing.type: Easing.OutCubic }
            }
            onStatusChanged: {
                if (status === Image.Error) {
                    console.warn("wallpaper: could not load " + paper.source)
                }
            }
        }

        scale: paper.back ? 1.08 : 1.0
        Behavior on scale { NumberAnimation { duration: 360; easing.type: Easing.OutCubic } }
    }

    // Behind the stack: blurred and darker. The GPU path draws the blur; the
    // software one (a nested session) draws only the darkening.
    MultiEffect {
        source: picture
        anchors.fill: picture
        visible: blur > 0.01
        blurEnabled: true
        blurMax: 48
        blur: paper.back ? 1 : 0
        Behavior on blur { NumberAnimation { duration: 360; easing.type: Easing.OutCubic } }
    }

    Rectangle {
        anchors.fill: parent
        color: "#000000"
        opacity: paper.back ? 0.35 : 0
        Behavior on opacity { NumberAnimation { duration: 360; easing.type: Easing.OutCubic } }
    }
}
