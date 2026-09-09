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

    Rectangle { x: 0;             y: 0;                width: root.width/2; height: root.height/2; color: "#0000ff" }
    Rectangle { x: root.width/2;    y: 0;                width: root.width/2; height: root.height/2; color: "#00ff00" }
    Rectangle { x: 0;               y: root.height/2;    width: root.width/2; height: root.height/2; color: "#ffffff" }
    Rectangle { x: root.width/2;    y: root.height/2;    width: root.width/2; height: root.height/2; color: "#ff0000" }
}
