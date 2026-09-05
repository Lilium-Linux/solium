// Quickshell's base for singleton objects.
//
// Every configuration group in the shell derives from this. In Quickshell it
// carries reload and ownership behaviour; here it needs only to be an object
// the properties can hang from, because nothing reloads a scene the compositor
// is drawing.
import QtQuick
QtObject {}
