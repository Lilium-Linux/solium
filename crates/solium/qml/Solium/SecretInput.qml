// A password field for a lock screen: a dot for each character typed and a
// caret after the last, and nothing else, because nothing else is known here.
//
// What is typed never reaches QML. It is kept in Rust, in a zeroised buffer
// (`lock_auth.rs`), and `SecretField`, the native item this is built on, is
// told only `length`, whether it is all `selected` (ctrl+a, so the next key
// replaces it), and whether it is `accepting` typing now: not while the
// password is being checked, while locked out, or while it cannot be. Keys go
// to the field whichever item has focus, with no click first; Return
// submits, Escape clears. `Lock.submit()` and `Lock.clear()` do the same
// from a button.
//
//     SecretInput { anchors.horizontalCenter: parent.horizontalCenter }
//
// Every look is a property, so a lock screen of your own can restyle it, or
// draw its own from a `SecretField` the same way.

import QtQuick
import Solium

SecretField {
    id: field

    // The dots: their size, the gap between them, and their colour.
    property real dotSize: 10
    property real spacing: 8
    property color color: Theme.text
    // The colour of the dots and the field's edge while it takes nothing.
    property color dimColor: Theme.textDim
    // The field behind the dots.
    property color fill: Theme.surface
    property color edge: Theme.edge
    property real radius: height / 2
    property real padding: 14
    // The room the field takes when nothing says otherwise: enough for
    // `columns` dots.
    property int columns: 16

    implicitWidth: padding * 2 + columns * (dotSize + spacing)
    implicitHeight: dotSize + padding * 2

    Rectangle {
        anchors.fill: parent
        radius: field.radius
        color: field.fill
        border.width: 1
        border.color: field.accepting ? field.edge : Qt.darker(field.edge, 1.4)
    }

    Item {
        id: track
        anchors.fill: parent
        anchors.margins: field.padding
        clip: true

        Row {
            id: dots
            anchors.verticalCenter: parent.verticalCenter
            // The newest dot stays in view once there are more than fit.
            x: Math.min(0, track.width - width - field.spacing - 2)
            spacing: field.spacing

            Repeater {
                model: field.length
                Rectangle {
                    width: field.dotSize
                    height: field.dotSize
                    radius: width / 2
                    color: field.accepting ? field.color : field.dimColor
                    opacity: field.selected ? 0.5 : 1
                    scale: 0.4
                    Component.onCompleted: scale = 1
                    Behavior on scale {
                        NumberAnimation { duration: Theme.quick; easing.type: Easing.OutBack }
                    }
                }
            }
        }

        Rectangle {
            id: caret
            visible: field.accepting
            anchors.verticalCenter: parent.verticalCenter
            x: dots.x + (field.length > 0 ? dots.width + field.spacing : 0)
            width: 2
            height: field.dotSize * 1.8
            color: field.color
        }
    }
}
