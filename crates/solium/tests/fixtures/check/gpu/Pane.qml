// A style that needs the GPU, which a check, always in software, cannot
// build. See `check::tests::a_style_that_needs_the_gpu_is_not_failed_in_software`.

import QtQuick
import Solium

PaneStyle {
    requires: ["gpu"]

    Layer {
        depth: "frame"
        name: "bar"
        source: "Bar.qml"
    }
}
