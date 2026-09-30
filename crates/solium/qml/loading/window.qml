// What a window shows while its application starts.
//
// The window is already real by the time this is drawn: it has its slot, the
// other windows have moved aside for it, and it can be closed. So this is not a
// notice about a window that is coming — it is what is *inside* that window
// until the application arrives, and the application appears in its place.
//
// Which is why it is only a name. Anything more would be decoration on a
// surface that exists to be replaced, and the thing worth showing is the one
// fact you do not otherwise have: which application you are waiting for.
//
// Edit it like any other scene here. `program` and `waited` are set by the
// compositor every frame; this one uses only the first.
//
// Do not try to fade this out yourself when the application arrives — the
// compositor does it, because whether a window is see-through is a
// presentation transform, like where it is and how big. `loading.fade` is the
// setting. On the software renderer a scene could not do it anyway: Qt repaints
// only what changed, onto the pixels already there, so each half-transparent
// frame lands on its own opaque previous one and nothing fades.
//
//     SOLIUM_LOADING=mine        ~/.config/solium/qml/loading/mine.qml
//     SOLIUM_LOADING=~/mine.qml  anywhere

import QtQuick
import Solium

Item {
    id: card

    // Set by the compositor.
    property string program: ""
    property int waited: 0
    Rectangle {
        anchors.fill: parent
        color: Theme.surface

        Text {
            anchors.centerIn: parent
            text: card.program
            color: Theme.text
            font { pixelSize: Theme.fontSize + 8; family: Theme.fontFamily }
        }
    }
}
