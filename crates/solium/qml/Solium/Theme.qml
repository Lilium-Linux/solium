// The design system. One file, read by everything the compositor draws.
//
// Window decorations and the shell's own surfaces run in the *same* QML engine,
// so they import this same singleton — change a colour here and the titlebars
// and the dock change together, because there is only one of it. Two
// stylesheets kept in step by hand is exactly what this exists to prevent.
//
// It is also what makes an object able to travel between them: an item lifted
// from the dock into a titlebar keeps its colours because it never left the
// design system, only the scene it was parented to.
//
// **This is not a design. It is a default.** Plain greys, a plain blue, a plain
// red: what the compositor looks like with nobody having chosen anything. It is
// meant to be unremarkable, because its job is to show what the compositor does
// rather than what someone's taste is — and because the first thing a rice does
// is replace it. `panes/` and `~/.config/solium/qml` are where a look belongs;
// this file is deliberately small and dull enough to throw away.

pragma Singleton
import QtQuick

QtObject {
    // --- greys ----------------------------------------------------------
    readonly property color surface: "#ffffff"
    readonly property color surfaceInactive: "#f0f0f0"
    readonly property color edge: "#c0c0c0"
    readonly property color edgeInactive: "#dcdcdc"

    readonly property color text: "#202020"
    readonly property color textDim: "#808080"

    readonly property color control: "#c0c0c0"
    readonly property color controlInactive: "#e0e0e0"

    // --- the three colours ----------------------------------------------
    // Stock blue, amber and red. Nothing is tinted, nothing is neon, and
    // `warning` and `danger` are only ever shown under the pointer — so a
    // titlebar at rest has no colour in it at all.
    readonly property color accent: "#0060c0"
    readonly property color warning: "#c08000"
    readonly property color danger: "#c02020"

    // --- metrics --------------------------------------------------------
    // The compositor reserves space using its own copy of `titlebarHeight`;
    // this is the value it uses, kept here so a theme cannot silently disagree
    // with the geometry the compositor is laying out.
    readonly property int titlebarHeight: 32
    readonly property int gap: 9
    readonly property int margin: 12

    // --- type -----------------------------------------------------------
    readonly property string fontFamily: "monospace"
    readonly property int fontSize: 12

    // --- motion ---------------------------------------------------------
    // Durations for chrome only. Window motion is the compositor's animation
    // engine, which QML cannot see and should not try to match by eye — see
    // crates/animation.
    readonly property int quick: 120
    readonly property int normal: 160
}
