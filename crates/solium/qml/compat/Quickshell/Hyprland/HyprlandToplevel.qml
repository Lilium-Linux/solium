// A window, as Hyprland reports it: the same thing as a Toplevel, addressed
// differently.
import QtQuick
QtObject {
    property string address: ""
    property string title: ""
    property string appId: ""
    property var workspace: null
    property var monitor: null
    property var lastIpcObject: ({})
}
