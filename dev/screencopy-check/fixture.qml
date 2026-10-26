import QtQuick
import Solium

Rectangle {
    color: Solium.monitor && Solium.monitor.name.endsWith("-2") ? "#402139" : "#192532"
    Rectangle { x: 17; y: 61; width: 89; height: 67; color: "#e94836" }
    Rectangle { x: 139; y: 93; width: 61; height: 79; color: "#31ba66" }
    Rectangle { x: 227; y: 172; width: 80; height: 59; color: "#e7c541" }
}
