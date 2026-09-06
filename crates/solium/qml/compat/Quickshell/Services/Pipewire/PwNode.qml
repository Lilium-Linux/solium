// One audio node: a sink, a source, or a stream.
import QtQuick
QtObject {
    property string name: ""
    property string description: ""
    property bool isSink: false
    property bool isStream: false
    property var audio: QtObject {
        property real volume: 0
        property bool muted: false
    }
    property var properties: ({})
}
