// Quickshell's base for singleton objects.
//
// Every configuration group and service in the shell derives from this, and
// most of them hold children — a Timer that polls, a Process that asks the
// system something, a Connections that listens. `QtObject` has no default
// property, so a bare one cannot take them: the whole shell failed with
// "cannot assign to non-existent default property", once, at the far end of a
// list of errors about other files.
import QtQuick
QtObject {
    default property list<QtObject> children
}
