// One animation, and it is inside a `Repeater` delegate.
//
// That placement is the whole fixture. `Repeater` gives its delegates a
// *visual* parent through `setParentItem` and leaves `QObject::parent()`
// pointing elsewhere, so a walk of `QObject::children()` from the root never
// reaches inside one. `animation_running` in host.cpp was that walk, and every
// animation in a delegate was invisible to it: the scene animated, nothing
// marked it dirty, and the compositor stopped drawing a decoration that was
// still moving.
//
// Found by a pane style whose waves were a `Repeater` of sliding bands. It
// rendered its first frame and then sat perfectly still, while the same
// animation moved up to a direct child of the root ran normally.
//
// Painted deliberately in a colour, because the census this sits beside counts
// non-zero bytes: a fixture that drew nothing could not tell "built and
// rendered" from "built and empty".

import QtQuick

Rectangle {
    id: root
    color: "#000000"

    Repeater {
        model: 1

        Rectangle {
            width: 32
            height: 32
            color: "#20a0ff"

            // Infinite, so there is no moment in a run where the honest answer
            // is "it finished". One unit per millisecond over an hour: it
            // cannot wrap or end inside a run, exactly as `quadrants.qml`'s.
            NumberAnimation on x {
                running: true
                loops: Animation.Infinite
                from: 0
                to: 3600000
                duration: 3600000
            }
        }
    }
}
