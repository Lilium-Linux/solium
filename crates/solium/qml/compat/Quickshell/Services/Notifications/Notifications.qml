// Desktop notifications.
pragma Singleton
import QtQuick
QtObject {
    // The urgency levels, as the shell names them.
    readonly property int low: 0
    readonly property int normal: 1
    readonly property int critical: 2
}
