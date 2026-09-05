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

pragma Singleton
import QtQuick

QtObject {
    // --- surfaces -------------------------------------------------------
    readonly property color surface: "#1b1f29"
    readonly property color surfaceInactive: "#14171e"
    readonly property color surfaceSunken: "#0f1218"
    readonly property color edge: "#2f3849"
    readonly property color edgeInactive: "#1e232c"

    // --- text -----------------------------------------------------------
    readonly property color text: "#e6e9ef"
    readonly property color textDim: "#666e7d"

    // --- accents --------------------------------------------------------
    readonly property color accent: "#7aa2f7"
    readonly property color positive: "#9ece6a"
    readonly property color warning: "#d8a33c"
    readonly property color danger: "#e05561"
    readonly property color control: "#39414f"
    readonly property color controlInactive: "#262b35"

    // --- metrics --------------------------------------------------------
    // The compositor reserves space using its own copy of `titlebarHeight`;
    // this is the value it uses, kept here so a theme cannot silently disagree
    // with the geometry the compositor is laying out.
    readonly property int titlebarHeight: 32
    readonly property int radius: 6
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
