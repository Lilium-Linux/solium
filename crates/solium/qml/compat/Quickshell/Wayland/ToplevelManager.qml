// Every window the shell can see, and which one has focus.
pragma Singleton
import QtQuick
QtObject {
    property var toplevels: ({ values: [] })
    property var activeToplevel: null
}
