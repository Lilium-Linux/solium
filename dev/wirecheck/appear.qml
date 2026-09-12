// One property, with a `Behavior` on it, and nothing else in the scene.
//
// The shape every decoration's appear animation has, reduced to the one thing
// this harness has to be able to read. `reveal.qml`'s bar slides out of the
// window's top edge from -34 to 0 over 260ms when `pointerInside` goes true;
// this is that, with the easing made linear so that "did it pass through the
// middle" is a question with one answer, and with the animated value mirrored
// into an `int` because `solium_qml_scene_get_int` is how the object tree is
// read from out here.
//
// Owned by the harness rather than reusing a shipped decoration for the same
// reason `quadrants.qml` is: the assertion is about the *host's clock*, not
// about any one decoration's design, and a decoration is free to change its
// durations without that being a regression.
import QtQuick

Item {
    id: frame

    // Written by the harness, the way `Decoration::tell` writes it.
    property bool pointerInside: false

    // Where the animation has got to: -34 before it starts, 0 when it has
    // finished, and every value in between while it runs. Rounded to an int
    // because that is what can be read back; the animation itself is a real.
    readonly property int slid: Math.round(bar.y)

    // How far it has to travel, so the harness asserts against the QML's own
    // number rather than a copy of it written down in Rust.
    readonly property int travel: bar.height

    Rectangle {
        id: bar

        width: parent.width
        height: 34
        color: "#3a3f4b"

        y: frame.pointerInside ? 0 : -height
        Behavior on y {
            NumberAnimation { duration: 260; easing.type: Easing.Linear }
        }
    }
}
