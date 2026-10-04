// A small capsule saying what the keyboard just did: `⇪` while Caps Lock is
// on, the short name of a layout just switched to (EN, RU), `⇭` for Num Lock.
// After macOS's Caps Lock indicator: a round capsule a little taller than a
// line, an outlined arrow over a bar drawn as a path rather than read from a
// font, and a soft shadow falling below it. The capsule is `Theme.accent` and
// what is drawn on it `Theme.accentInk`: in the shipped theme a light grey
// with a dark glyph, which stands out on a dark window, and with its shadow
// on a light one (`keyboard-pane-drawn.lua`, `keyboard-surface-drawn.lua`).
//
// It shows what it is handed and decides nothing. `lua/keyboard_indicator.lua`
// is the policy -- which changes show it, for how long, where -- and hands it
// a `cue`; the label is read from the `Keyboard` singleton. The shipped
// configuration draws it two ways, on an overlay surface
// (`qml/indicator/keyboard.qml`) and inside the pane (`KeyboardPillLayer`),
// and both are ordinary configuration a user can delete or replace.
//
//     KeyboardPill { cue: ({ what: "caps", serial: 3, hold: true }) }
//
// `cue.what` is "caps", "layout", "num", or "" to hide at once. A cue is
// taken when its `serial` is new, so writing the same cue again shows
// nothing again. A held cue (`hold`) stays until the next one; any other
// hides after `duration` milliseconds, by the Timer below -- or, if it names
// what comes `after` it, shows that instead, held: a layout's pill shown
// over Caps Lock's hands back to `⇪`.
//
// `crates/solium/tests/scenarios/keyboard-surface-drawn.lua` and
// `keyboard-pane-drawn.lua` draw it and read its pixels, through
// `scenario::tests::every_scenario_on_the_qt_thread_passes`.

import QtQuick
import QtQuick.Shapes
import Solium

Item {
    id: pill

    // What to show, from the configuration's policy:
    // `{ what, serial, hold, duration, after }`.
    property var cue: ({})

    // Whether a new cue is shown at all. A pane's layer takes a cue only while
    // its window has the caret, so a cue it was handed while elsewhere is
    // not shown late; one it was not shown is not shown later either.
    property bool accepts: true

    // The room around the capsule its shadow falls in, on every side.
    readonly property int margin: 14

    // The capsule's own height, and the least width: round, as macOS's is,
    // and a little taller than a line of text.
    readonly property int capsuleHeight: 28
    readonly property int capsuleWidth: 32

    // How thick the arrow's outline is, and the label's letters with it.
    readonly property real stroke: 1.6

    // What is on show now, which the label reads rather than the cue, so the
    // pill keeps its glyph while it fades out.
    property string shown: ""
    property bool showing: false
    property int seen: 0
    property bool ready: false

    // The label, for what is not drawn as a path: a layout's short name, and
    // Num Lock's glyph.
    readonly property string label: pill.shown === "num" ? "⇭"
        : pill.shown === "layout" ? Keyboard.layoutShort
        : ""

    implicitWidth: capsule.width + 2 * pill.margin
    implicitHeight: pill.capsuleHeight + 2 * pill.margin

    // Read from `cue` itself, not from a binding on it: in a change handler,
    // a binding on the same property may not have been brought up to date.
    function take() {
        const cue = pill.cue ? pill.cue : {};
        const serial = cue.serial ? cue.serial : 0;
        if (serial === pill.seen) {
            return;
        }
        pill.seen = serial;
        const what = cue.what ? cue.what : "";
        if (!pill.accepts || what === "") {
            pill.showing = false;
            hide.stop();
            return;
        }
        pill.shown = what;
        pill.showing = true;
        if (cue.hold) {
            hide.stop();
        } else {
            hide.restart();
        }
    }

    onCueChanged: if (pill.ready) pill.take()
    Component.onCompleted: {
        pill.ready = true;
        pill.take();
    }

    Timer {
        id: hide
        interval: pill.cue && pill.cue.duration > 0 ? pill.cue.duration : 1200
        // `keyboard-pane-drawn.lua`, "hands back to it".
        onTriggered: {
            if (pill.cue && pill.cue.after) {
                pill.shown = pill.cue.after;
            } else {
                pill.showing = false;
            }
        }
    }

    opacity: pill.showing ? 1 : 0
    visible: opacity > 0
    Behavior on opacity {
        NumberAnimation {
            duration: pill.showing ? 90 : 140
            easing.type: Easing.OutCubic
        }
    }

    // A soft shadow below the capsule, from stacked capsules: no shader, so it
    // draws the same in software. Each is a pixel wider all round than the
    // last and fainter towards the edge, so together they fall off smoothly
    // instead of reading as rings.
    Repeater {
        model: pill.margin - 2
        Rectangle {
            id: ring
            required property int index
            // How far out this one is, from 0 at the capsule to 1 at the edge.
            readonly property real out: (ring.index + 1) / (pill.margin - 2)
            anchors.centerIn: capsule
            anchors.verticalCenterOffset: 4
            width: capsule.width + 2 * (ring.index + 1)
            height: capsule.height + 2 * (ring.index + 1)
            radius: height / 2
            color: Qt.rgba(0, 0, 0, 0.028 * (1 - ring.out) * (1 - ring.out) + 0.003)
        }
    }

    Rectangle {
        id: capsule
        anchors.centerIn: parent
        height: pill.capsuleHeight
        width: Math.max(pill.capsuleWidth, Math.ceil(text.implicitWidth) + 16)
        radius: height / 2
        color: Theme.accent

        // ⇪, as macOS draws it: an outlined arrow, and a bar below it.
        Shape {
            id: caps
            anchors.centerIn: parent
            width: 16
            height: 16
            visible: pill.shown === "caps"
            preferredRendererType: Shape.CurveRenderer

            ShapePath {
                strokeColor: Theme.accentInk
                strokeWidth: pill.stroke
                fillColor: "transparent"
                joinStyle: ShapePath.RoundJoin
                capStyle: ShapePath.RoundCap
                startX: 8; startY: 1.2
                PathLine { x: 14.6; y: 6.8 }
                PathLine { x: 11; y: 6.8 }
                PathLine { x: 11; y: 9.2 }
                PathLine { x: 5; y: 9.2 }
                PathLine { x: 5; y: 6.8 }
                PathLine { x: 1.4; y: 6.8 }
                PathLine { x: 8; y: 1.2 }
            }
            ShapePath {
                strokeColor: Theme.accentInk
                strokeWidth: pill.stroke
                fillColor: "transparent"
                joinStyle: ShapePath.RoundJoin
                PathRectangle { x: 5; y: 12.2; width: 6; height: 2.4; radius: 1 }
            }
        }

        Text {
            id: text
            anchors.centerIn: parent
            text: pill.label
            color: Theme.accentInk
            font.family: Theme.fontFamily
            font.weight: Font.Medium
            font.pixelSize: pill.shown === "layout" ? 13 : 17
        }
    }
}
