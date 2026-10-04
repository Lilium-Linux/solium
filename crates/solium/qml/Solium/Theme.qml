// The design system: the colours, fonts and metrics the window frames, the
// loading window, the keyboard pill and the tweaks panel are drawn with.
//
// Window decorations and a hosted shell run in the *same* QML engine, so a
// shell scene that imports `Solium` gets this same singleton — change a colour
// here and the titlebars and that shell change together, because there is only
// one of it. Two stylesheets kept in step by hand is exactly what this exists
// to prevent.
//
// Two of the compositor's own scenes do not read it: `cursor.qml`, whose
// colours are fixed and which says why, and `wallpaper.qml`.
//
// **This is not a design. It is a default.** Plain greys on near-black: what
// the compositor looks like with nobody having chosen anything. It is meant to
// be unremarkable, because its job is to show what the compositor does rather
// than what someone's taste is — and because the first thing a rice does is
// replace it. `panes/` and `~/.config/solium/qml` are where a look belongs;
// this file is deliberately small and dull enough to throw away.
//
// **Every colour here is a grey: red, green and blue the same.** That is the
// maintainer's decision for now (2026-10-04): a dark theme in black, greys and
// white, with no accent hue and none of the logo's or the wallpaper's colours
// (`qml::hosting_tests::every_colour_the_theme_publishes_is_a_grey`). A light
// scheme, or colour accents, may come later as a change of these values only:
// every name below stays, and what reads them does not change.

pragma Singleton
import QtQuick

QtObject {
    // --- greys ----------------------------------------------------------
    // The focused window's bar is a shade lighter than the others', and its
    // title is light on it and dimmer on theirs
    // (`tests/scenarios/pane-top-drawn.lua`).
    readonly property color surface: "#303030"
    readonly property color surfaceInactive: "#1c1c1c"
    readonly property color edge: "#4a4a4a"
    readonly property color edgeInactive: "#2e2e2e"

    readonly property color text: "#ebebeb"
    readonly property color textDim: "#8c8c8c"

    readonly property color control: "#6b6b6b"
    readonly property color controlInactive: "#3a3a3a"

    // --- the three that were colours ------------------------------------
    // Still three names, because shells and styles read them, and three
    // greys now. `accent` is a light grey, a shade off white: the keyboard
    // pill's capsule, which stands out on a dark window and on a light one
    // (`tests/scenarios/keyboard-pane-drawn.lua`). `warning` and `danger` are
    // what the maximise and close buttons turn under the pointer, close the
    // lighter, so the two are told apart by shade rather than by hue
    // (`qml::hosting_tests::every_colour_the_theme_publishes_is_a_grey`).
    readonly property color accent: "#d4d4d4"
    readonly property color warning: "#a6a6a6"
    readonly property color danger: "#ebebeb"

    // What is drawn on any of those three: the pill's glyph and label, a
    // pressed tweak's label. Dark, so it contrasts with all three
    // (`tests/scenarios/keyboard-pane-drawn.lua`).
    readonly property color accentInk: "#1c1c1c"

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
