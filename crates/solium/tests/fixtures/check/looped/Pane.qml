// A style whose layer is its own manifest: checked once as a file, and not
// followed again as a style. See
// `check::tests::a_layer_that_is_its_own_pane_qml_is_checked_once`.

import QtQuick
import Solium

PaneStyle {
    requires: []

    Layer {
        depth: "frame"
        name: "itself"
        source: "Pane.qml"
    }
}
