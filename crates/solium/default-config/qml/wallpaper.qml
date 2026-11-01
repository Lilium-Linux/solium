// Shadows the shipped wallpaper.qml: the same picture, with some life.
//
// The image is a little taller than the screen, and pans as the workspaces
// slide (vertically, as workspaces.arrangement has them): workspace 1 shows
// its top, the last its bottom -- parallax, slower than the windows. While
// the workspace stack (deskstack.lua) is open it eases back -- zoomed in a
// touch, blurred, darker -- and keeps panning to the desk chosen there.
//
// Told by looks.lua: `desks` (the workspace each monitor shows, by name),
// `count`, `stack` and `stackMonitor`.

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

    readonly property int desk: desks && desks[Solium.monitor.name] ? desks[Solium.monitor.name] : 1
    readonly property bool back: stack && stackMonitor === Solium.monitor.name

    // How much taller than the screen the picture is, for the pan.
    readonly property real travel: 0.12

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
            y: -parent.height * paper.travel * (paper.count > 1 ? (paper.desk - 1) / (paper.count - 1) : 0)
            source: paper.source
            fillMode: Image.PreserveAspectCrop
            smooth: true
            mipmap: true
            cache: false
            asynchronous: false
            Behavior on y { NumberAnimation { duration: 420; easing.type: Easing.OutCubic } }
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
