// A style that ships effect rules: a translucent bar, and `effects.lua` beside
// it putting an effect behind the titlebar.
//
// A test fixture rather than a shipped style, so it is never offered by name.
// `style::tests::a_styles_effects_lua_is_read_with_it` reads its rules, and
// `decoration::tests::a_decoration_carries_the_styles_rules` carries them onto
// a pane. The rule names the `tint` effect, which is a test fixture too
// (`tests/fixtures/effects/tint/`), so pointing a session at this folder with
// `SOLIUM_PANE` draws the bar and lists on the overlay that `tint` is nowhere.

import QtQuick
import Solium

PaneStyle {
    // The titlebar's band: `region:titlebar` is the strictly largest inset.
    insets.top: 32

    requires: []

    // Translucent, so whatever an effect draws behind the titlebar shows
    // through it.
    Layer {
        depth: "frame"
        name: "bar"
        source: "Bar.qml"
    }
}
