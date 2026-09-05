// The compositor the shell was written against, answered by this one.
//
// Every call here has a Solium equivalent — toplevels are `sol.windows`,
// dispatch is a binding — so this is a translation layer, not a reimplementation
// of Hyprland.

pragma Singleton
import QtQuick

QtObject {
    property var toplevels: ({ values: [] })
    property var monitors: ({ values: [] })
    property var activeToplevel: null

    function monitorFor(screen) { return null; }
    function dispatch(command) {
        console.warn("Hyprland.dispatch not yet mapped to Solium:", command);
    }
    function refreshToplevels() {}
}
