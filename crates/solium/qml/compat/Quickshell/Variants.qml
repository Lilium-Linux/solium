// One instance of a delegate per item in a model.
//
// Quickshell uses this to build a copy of something per screen. `Instantiator`
// is the same idea and is already in QtQml, so this is a rename rather than an
// implementation. Its `model` and `delegate` are Instantiator's own, not
// redeclared: a redeclaration hides the property Instantiator builds from, and
// then nothing is ever built. See `variants_builds_one_instance_per_model_entry`.
import QtQuick
import QtQml
Instantiator {
    asynchronous: false
}
