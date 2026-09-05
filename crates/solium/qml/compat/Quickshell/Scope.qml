// A non-visual container. Quickshell uses it to group objects with a lifetime;
// here that lifetime is the scene's.
import QtQuick
QtObject { default property list<QtObject> children }
