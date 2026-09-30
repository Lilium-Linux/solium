// One instance of a delegate per item in a model.
//
// Quickshell uses this to build a copy of something per screen. `Instantiator`
// is the same idea and is already in QtQml, so this is a rename rather than an
// implementation.
import QtQuick
import QtQml
Instantiator {
    property var model: []
    property Component delegate: null
    asynchronous: false
}
