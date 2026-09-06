// Receives notifications from applications.
import QtQuick
QtObject {
    property var trackedNotifications: ({ values: [] })
    property bool bodySupported: true
    property bool imageSupported: true
    property bool actionsSupported: true
    signal notification(var notif)
}
