// Prism — the design system, in one file.
//
// Every frame and every shell surface in this rice imports this singleton, and
// so does everything the compositor ships. Change a colour here and the
// titlebars, the bar and the dock change together, because there is only one
// of it.
//
// The names above the line are the ones the shipped scenes read. They are kept
// whether or not this rice uses them: a theme that drops one does not restyle
// the Developer Tweaks panel, it breaks it.

pragma Singleton
import QtQuick

QtObject {
    // --- the shipped contract -------------------------------------------
    readonly property color surface: "#141222"
    readonly property color surfaceInactive: "#0e0d18"
    readonly property color surfaceSunken: "#08070f"
    readonly property color edge: "#2e2947"
    readonly property color edgeInactive: "#1b1930"
    readonly property color text: "#efecf9"
    readonly property color textDim: "#9a92b8"
    readonly property color accent: "#936dff"
    readonly property color positive: "#5ee6a8"
    readonly property color warning: "#ffc46b"
    readonly property color danger: "#ff6d8a"
    readonly property color control: "#2a2542"
    readonly property color controlInactive: "#1a1730"

    // The compositor reserves space using its own copy of this (32). Kept in
    // step deliberately — a theme that disagrees with the geometry being laid
    // out is the one bug you cannot see in a screenshot.
    readonly property int titlebarHeight: 32
    readonly property int radius: 10
    readonly property int gap: 9
    readonly property int margin: 12
    readonly property string fontFamily: "JetBrains Mono"
    readonly property int fontSize: 12
    readonly property int quick: 120
    readonly property int normal: 160

    // --- Prism ------------------------------------------------------------
    // The ground the glass sits on. The wallpaper paints these, and the panels
    // are tinted from the same three so a panel never looks pasted on.
    readonly property color ground: "#07060e"
    readonly property color groundDeep: "#030308"
    readonly property color violet: "#936dff"
    readonly property color cyan: "#4dd9ff"
    readonly property color rose: "#ff6dc0"

    // Frost. There is no backdrop blur to be had — a decoration is composited
    // over the scene and cannot sample what is behind it — so the glass is
    // built the way glass is actually built in software: a translucent fill, a
    // lit top edge where the light catches, a dark bottom edge where it does
    // not, and a tint.
    readonly property color glassFill: "#17ffffff"
    readonly property color glassFillLow: "#0bffffff"
    readonly property color glassFillDeep: "#26ffffff"
    readonly property color glassRim: "#2effffff"
    readonly property color glassRimLow: "#15ffffff"
    readonly property color glassShade: "#66000000"
    readonly property color glassTint: "#24936dff"
    readonly property color glassTintIdle: "#0e936dff"

    readonly property int panelRadius: 14
    readonly property int panelHeight: 34
    readonly property int panelInset: 12
}
