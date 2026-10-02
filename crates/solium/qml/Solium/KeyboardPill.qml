// A small capsule saying what the keyboard just did: `⇪` while Caps Lock is
// on, the short name of a layout just switched to (EN, RU), `⇭` for Num Lock.
// After macOS's Caps Lock indicator, in `Theme`'s accent.
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
    readonly property int margin: 12

    // The capsule's own height: about one line of text.
    readonly property int capsuleHeight: 24

    // What is on show now, which the label reads rather than the cue, so the
    // pill keeps its glyph while it fades out.
    property string shown: ""
    property bool showing: false
    property int seen: 0
    property bool ready: false

    readonly property string label: pill.shown === "caps" ? "⇪"
        : pill.shown === "num" ? "⇭"
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

    // A soft shadow, from stacked rectangles: no shader, so it draws the same
    // in software.
    Repeater {
        model: 4
        Rectangle {
            required property int index
            anchors.centerIn: capsule
            anchors.verticalCenterOffset: 2
            width: capsule.width + (index + 1) * 4
            height: capsule.height + (index + 1) * 4
            radius: height / 2
            color: Qt.rgba(0, 0, 0, 0.09 - index * 0.02)
        }
    }

    Rectangle {
        id: capsule
        anchors.centerIn: parent
        height: pill.capsuleHeight
        width: Math.max(Math.round(pill.capsuleHeight * 1.75), text.implicitWidth + 20)
        radius: height / 2
        color: Theme.accent

        Text {
            id: text
            anchors.centerIn: parent
            text: pill.label
            color: "#ffffff"
            font.family: Theme.fontFamily
            font.bold: true
            font.pixelSize: pill.shown === "layout" ? 13 : 17
        }
    }
}
