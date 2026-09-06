// One window, as the shell sees it.
//
// Quickshell learns these over wlr-foreign-toplevel. Inside Solium the
// compositor already holds them, so this is the shape their data arrives in
// — filled from Solium's own window list rather than from a protocol.
import QtQuick
QtObject {
    property string appId: ""
    property string title: ""
    property bool activated: false
    property bool maximized: false
    property bool minimized: false
    property bool fullscreen: false
    property var screens: []
    function activate() {}
    function close() {}
    function setMaximized(value) {}
    function setMinimized(value) {}
    function setFullscreen(value) {}
}
