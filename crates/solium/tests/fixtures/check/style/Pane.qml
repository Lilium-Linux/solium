// A style whose one layer does not build: `solium --check-qml` on this file
// must fail and name `Ring.qml`. See `check::tests::a_style_bundle_fails_on_its_broken_layer`.

import QtQuick
import Solium

PaneStyle {
    requires: []

    Layer {
        depth: "frame"
        name: "ring"
        source: "Ring.qml"
    }
}
