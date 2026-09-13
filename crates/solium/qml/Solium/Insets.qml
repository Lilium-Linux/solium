// How much of a window a style reserves, on each side.
//
// A named type rather than an anonymous `QtObject`, and that is not a
// preference. QML resolves `insets.top: 32` against the *declared type* of the
// property, not against the object bound to it, so
//
//     property QtObject insets: QtObject { property int top: 0 }
//
// loads fine on its own and then fails in every style that tries to use it,
// with `Cannot assign to non-existent property "top"` — because `QtObject` has
// no `top` and the dynamic property the binding created is invisible to the
// compiler. Giving the group a type is what makes the grouped-property syntax
// the spec is written in legal.
//
// `internal` in qmldir: `PaneStyle` and `Layer` are the module's surface, and a
// style never needs to name this to write `insets.top`.

import QtQuick

QtObject {
    property int top: 0
    property int right: 0
    property int bottom: 0
    property int left: 0
}
