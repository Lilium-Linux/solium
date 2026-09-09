import QtQuick

// Four quadrants, matching wirecheck's `expected_argb` exactly:
// top-left blue, top-right green, bottom-left white, bottom-right red.
Rectangle {
    id: root
    color: "#000000"
    Rectangle { x: 0;               y: 0;                width: root.width/2; height: root.height/2; color: "#0000ff" }
    Rectangle { x: root.width/2;    y: 0;                width: root.width/2; height: root.height/2; color: "#00ff00" }
    Rectangle { x: 0;               y: root.height/2;    width: root.width/2; height: root.height/2; color: "#ffffff" }
    Rectangle { x: root.width/2;    y: root.height/2;    width: root.width/2; height: root.height/2; color: "#ff0000" }
}
