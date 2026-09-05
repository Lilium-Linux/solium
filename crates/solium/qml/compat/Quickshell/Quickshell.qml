// Enough of Quickshell's root singleton for shell QML to load inside Solium.
//
// The shell was written against Quickshell, which is a separate process that
// hosts QML and talks to the compositor over Wayland. Here the compositor
// hosts the QML itself, so these are the same calls answered by the thing that
// was on the other end of the socket.
//
// Deliberately a shim rather than a rewrite of the shell: 16,000 lines of
// working QML should not be retyped to change what is underneath it.

pragma Singleton
import QtQuick

QtObject {
    // Set by the compositor before a scene loads.
    property string configDir: ""
    property int processId: 0
    property var screens: []

    // Answered by the compositor: see `shell.rs`. Until then these are the
    // shapes the shell expects, so it loads and its bindings evaluate.
    function execDetached(command) {
        console.warn("Quickshell.execDetached not yet wired to Solium:", JSON.stringify(command));
    }

    function iconPath(name, check) {
        return "";
    }

    function env(name) {
        return "";
    }
}
