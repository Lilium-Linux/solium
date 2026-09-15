import QtQuick

// Four quadrants, matching wirecheck's `expected_argb` exactly:
// top-left blue, top-right green, bottom-left white, bottom-right red.
Rectangle {
    id: root
    color: "#000000"

    // A counter the object tree carries, and the only thing here that is not
    // about the picture.
    //
    // Nothing outside the tree remembers it: wirecheck reads it back out of
    // this item and writes it straight in again, so a *new* tree hands back
    // this declared default and the count restarts at zero. That is the same
    // storage a running animation's state lives in, which is what lets it stand
    // in for one -- see the resize case in dev/wirecheck/src/main.rs.
    //
    // It changes nothing about what is painted, deliberately: the frame loop
    // compares this scene against a fixed reference, over a buffer wiped in
    // between.
    property int frames: 0

    // A *running* animation, which is the thing `frames` is only a proxy for.
    //
    // The counter proves the object tree survived a rebind. It cannot prove
    // anything inside the tree kept *moving* across one, and that is the
    // property the rebind exists for: a decoration is sized from an animating
    // rectangle for the length of every window animation, so a scene whose
    // animations restart on each resize is a scene whose animations never
    // advance at all. A tree that survived with every animation reset to zero
    // would carry the counter across perfectly, and did not have to be checked
    // for until decorations were the scenes doing the resizing.
    //
    // Driven by `solium_qml_tick`, which advances the compositor's own
    // QAnimationDriver: this moves only when the harness says a frame's worth
    // of time has passed, and never on its own. One unit per millisecond, over
    // an hour, so it cannot wrap or finish inside a run.
    //
    // Nothing paints it, deliberately, for the same reason `frames` paints
    // nothing: the frame loop compares this scene against a fixed reference
    // over a wiped buffer, and an animation that changed the picture would
    // break every other case in the harness.
    property int spin: 0
    NumberAnimation on spin {
        from: 0
        to: 3600000
        duration: 3600000
        loops: Animation.Infinite
    }

    Rectangle { x: 0;             y: 0;                width: root.width/2; height: root.height/2; color: "#0000ff" }
    Rectangle { x: root.width/2;    y: 0;                width: root.width/2; height: root.height/2; color: "#00ff00" }
    Rectangle { x: 0;               y: root.height/2;    width: root.width/2; height: root.height/2; color: "#ffffff" }
    Rectangle { x: root.width/2;    y: root.height/2;    width: root.width/2; height: root.height/2; color: "#ff0000" }
}
