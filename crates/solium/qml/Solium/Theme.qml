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
// The palette is small and quiet on purpose: paper, ink, a hairline, and two
// tints that appear only under the pointer. A compositor's chrome is the frame
// around somebody else's work, and a frame that draws attention is doing the
// wrong job. Anything louder belongs in a rice — which is what `panes/` and
// `~/.config/solium/qml` are for, and why this file is small enough to replace.

pragma Singleton
import QtQuick

QtObject {
    // --- paper ----------------------------------------------------------
    // Two greys and a hairline. Focused and unfocused should be distinguishable
    // without being read.
    readonly property color surface: "#ffffff"
    readonly property color surfaceInactive: "#f4f4f5"
    readonly property color edge: "#d4d4d8"
    readonly property color edgeInactive: "#e6e6e9"

    // --- ink ------------------------------------------------------------
    readonly property color text: "#18181b"
    readonly property color textDim: "#8b8b93"

    // --- tints ----------------------------------------------------------
    // `accent` marks the one thing on a surface that matters, never more than
    // one. `warning` and `danger` are the frame buttons and show only under the
    // pointer, so a titlebar at rest has no colour in it at all.
    readonly property color accent: "#3b6ea5"
    readonly property color warning: "#b8860b"
    readonly property color danger: "#b4413c"

    // A control at rest is a shape rather than a colour.
    readonly property color control: "#c9c9ce"
    readonly property color controlInactive: "#dededf"

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
