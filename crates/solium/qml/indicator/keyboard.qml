// The keyboard pill on its own surface, over everything: what
// `lua/keyboard_indicator.lua` declares at the caret, or on screen when there
// is no caret to go to.
//
// A scene like any other `sol.surface` scene. The configuration decides where
// the surface goes and when there is something to show, and hands it as
// `cue`; this draws it. Nothing in the compositor knows this file exists.
// `crates/solium/tests/scenarios/keyboard-surface-drawn.lua`, through
// `scenario::tests::every_scenario_on_the_qt_thread_passes`.

import QtQuick
import Solium

Item {
    id: root

    // `{ what, serial, hold, duration, after }`, as `KeyboardPill` takes it.
    property var cue: ({})

    // Draws, and takes no input: the pointer goes to what is under it.
    Solium.input: false

    KeyboardPill {
        anchors.centerIn: parent
        cue: root.cue
    }
}
