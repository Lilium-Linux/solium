// The keyboard layout, live from the `Keyboard` singleton every scene reads.
// Shown only with a real choice to make -- one layout configured says
// nothing a glance needs.

import QtQuick
import Solium

Capsule {
    id: chip
    visible: Keyboard.layouts.length > 1

    Text {
        // Whatever xkb's own rules call it ("en", "ru", ...); not invented
        // here, just upper-cased for a bar-sized glyph.
        text: Keyboard.layoutShort.toUpperCase()
        color: Theme.text
        font { pixelSize: Theme.fontSize; family: Theme.fontFamily; bold: true }
    }
}
