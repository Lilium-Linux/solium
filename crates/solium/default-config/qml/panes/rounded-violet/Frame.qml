// The titlebar of `panes/rounded-violet`, the floating frame: after the
// shipped `rounded`, with nothing of it under the client. Outline.qml is the
// violet.
//
// The client keeps all four corners round (Pane.qml), cut by the
// compositor's shader, so its two top corners leave two notches under the
// ends of the bar. The shipped `rounded` filled them by reaching a whole
// `clientRadius` strip under the client, and traced the seam with a rounded
// rectangle whose sides and bottom were under it too: hidden while windows
// were opaque, seen through every window once they were not (looks.lua makes
// them see-through). Here the bar stays in its band, the band and both
// notches (each the corner's square less the client's quarter circle) are one
// shape with one fill, so no pixel is painted twice, and the seam is drawn as
// the line it is: the top edge and the two arcs, one pixel outside the
// client's cut.
//
// The look, as macOS lays a titlebar out: close, minimise and maximise on the
// left, round and plain, each turning into its icon while the pointer is on
// it; the title centred on the whole bar, kept clear of both ends; the
// application's icon on the right, balancing the buttons. A violet tint and a
// glassy highlight while the window has the keyboard. Every change animates.
//
// What it does, besides the three buttons: the wheel anywhere on it is the
// application's own volume, shown in the title's place while it changes; a
// double click maximises or restores; a right click opens the window's quick
// settings: the bar grows taller, the window's content moves down, and they
// show in the room it made (QuickSettings.qml, and titlebar.lua).
//
// Floating and tiled are the same frame. Pane.qml reserves one pixel; while
// the window floats the bar is that band grown by the titlebar's height, and
// while it is in a tile it folds back to the band and its border, the
// outline (Outline.qml), is all there is. The fold is animated, with the
// window's glide, both ways: the content moves up into the room the bar gave
// back, and down again as it unfolds.

import QtQuick
import QtQuick.Shapes
import Solium

Item {
    id: frame

    // Set by the compositor, on every layer of every style.
    property string title: ""
    // The window's application, for its icon (#417; empty until known).
    property string appId: ""
    property bool focused: false
    property bool pointerInside: false
    property int contentWidth: 0
    property int contentHeight: 0

    // What the style reserved, and what it asked the compositor to cut. Both
    // in logical pixels; nothing QML is told is ever in device pixels.
    property int insetTop: 0
    property int clientRadius: 0

    // The window itself (#417), and what titlebar.lua keeps for it.
    property int windowId: 0
    // Set by the compositor, once (#462): true for a window whose application
    // draws its own titlebar. Its frame is the outline, the shadow and the
    // corners; the titlebar, and the quick settings that open from it, are
    // the application's, so this bar never unfolds and is not drawn.
    property bool clientSide: false
    visible: !clientSide
    property int pid: -1
    property bool maximized: false
    property bool fullscreen: false
    property bool floating: false
    property var windowValues: ({})
    FlightVisibility { id: desktopOpening; values: frame.windowValues }
    opacity: desktopOpening.flight && !desktopOpening.revealed ? 0 : 1
    Behavior on opacity { enabled: desktopOpening.revealed; NumberAnimation { duration: 80 } }

    // Whether the quick settings are open (titlebar.lua keeps it): the bar
    // then grows taller and the window's content moves down to make room,
    // so they are part of the titlebar rather than drawn over the window.
    readonly property bool expanded: frame.windowValues.quick === true
    // The style's band, Pane.qml's `insets.top`.
    readonly property int border: 1
    // The titlebar's own row above the band: all of it while the window
    // floats, none in a tile (`floating` is the compositor's: a window in no
    // tile). Folding is animated: a tiled window keeps its tile, and its
    // content rises into the room the bar gives back. Unfolding is not: a
    // floating window grows by what its bar takes, and grown in steps it
    // would glide to where it was put and then go on growing at the bottom;
    // at once, it glides straight to its whole size, while what is on the
    // bar fades in (`reveal`).
    // A window has its titlebar while it floats, and while it is maximised
    // out of a tile too (it keeps its tile to go back to, so it does not
    // count as floating): otherwise a maximised tile would have no buttons
    // and nothing to double-click to restore it.
    readonly property bool titled: !frame.clientSide && (frame.floating || frame.maximized)
    property real barGrow: frame.titled ? Theme.titlebarHeight - frame.border : 0
    Behavior on barGrow {
        enabled: frame.told && !frame.titled
        NumberAnimation { duration: 280; easing.type: Easing.OutCubic }
    }
    // Whether the compositor has told this frame its window yet. A frame is
    // built before it knows whether its window floats, and a window opening
    // should come up with the bar it has, not grow it or fold it away while
    // it opens: so the fold animates from the first change after that.
    property bool told: false
    onWindowIdChanged: if (frame.windowId !== 0) Qt.callLater(() => frame.told = true)
    // And the quick settings' room under it, animated the same way.
    property real quickGrow: frame.expanded && !frame.clientSide ? quickSettings.fullHeight : 0
    Behavior on quickGrow { NumberAnimation { duration: 280; easing.type: Easing.OutCubic } }
    // The room asked for beyond the style's band, read back by the
    // compositor (#423): it moves the client down by this much.
    readonly property real growTop: frame.barGrow + frame.quickGrow
    // How much of the titlebar shows: 1 floating, 0 tiled, faded between
    // both ways. What the bar's fill, its seam and what sits on it fade by.
    property real reveal: frame.titled ? 1 : 0
    Behavior on reveal {
        enabled: frame.told
        NumberAnimation { duration: 280; easing.type: Easing.OutCubic }
    }
    // The band the bar's own row sits in, whatever it has grown by:
    // `insetTop` is the whole of it, the band plus the room granted.
    readonly property real band: frame.border + frame.barGrow

    // The application's sound, all of its streams as one (#416).
    // Nothing, while the window's id is not known yet (a frame is built
    // before it is told), rather than null.
    readonly property var sound: Audio.forWindow(frame.windowId)
        || ({ present: false, muted: false, volume: 0, max: 1, step: 0.05 })
    // True while the wheel is changing it, and a moment after. The pause is
    // an animation rather than a Timer: an animation runs on the
    // compositor's clock and asks for its frames, so it ends on time even in
    // a scene where nothing else moves.
    property bool showingVolume: false
    SequentialAnimation {
        id: volumeShown
        PauseAnimation { duration: 1300 }
        ScriptAction { script: frame.showingVolume = false }
    }

    // The buttons. `action` is read by the compositor ("close" or
    // "maximize"; minimise has no action, it is super+m or the taskbar), and
    // `onButton` tells it a press here is a press, not the start of a drag.
    property string action: ""
    property string hovered: ""
    readonly property bool onButton: hovered !== ""

    // One of the three: a round light that becomes its icon -- a cross, a
    // dash, a plus -- while the pointer is on it. Coloured while the window
    // has the keyboard, grey without, as macOS does.
    component Light: Item {
        id: light
        property string name: ""
        property color tint: "#ec6a5e"
        readonly property bool hot: pointer.containsMouse
        readonly property color lit: frame.focused ? tint : "#4a4652"
        width: 13
        height: 13

        // The light, which gives way to the icon.
        Rectangle {
            anchors.centerIn: parent
            width: parent.width
            height: width
            radius: width / 2
            color: light.lit
            scale: light.hot ? 0.35 : (pointer.pressed ? 0.86 : 1.0)
            opacity: light.hot ? 0 : 1
            Behavior on color { ColorAnimation { duration: 180 } }
            Behavior on scale { NumberAnimation { duration: 160; easing.type: Easing.OutCubic } }
            Behavior on opacity { NumberAnimation { duration: 140 } }
        }

        // The icon: two strokes, crossed for close, upright and flat for
        // maximise; one flat stroke for minimise. In the light's colour.
        Repeater {
            model: light.name === "minimize" ? 1 : 2
            Rectangle {
                anchors.centerIn: parent
                width: 11
                height: 2
                radius: 1
                antialiasing: true
                color: light.tint
                rotation: light.name === "close" ? (index ? -45 : 45)
                        : light.name === "maximize" ? (index ? 90 : 0) : 0
                scale: light.hot ? (pointer.pressed ? 0.85 : 1.0) : 0.3
                opacity: light.hot ? 1 : 0
                Behavior on scale { NumberAnimation { duration: 180; easing.type: Easing.OutBack } }
                Behavior on opacity { NumberAnimation { duration: 140 } }
            }
        }

        MouseArea {
            id: pointer
            // A little larger than the light, so it is easy to hit.
            anchors.fill: parent
            anchors.margins: -3
            hoverEnabled: true
            onClicked: frame.action = light.name
            onContainsMouseChanged: {
                if (containsMouse) {
                    frame.hovered = light.name;
                } else if (frame.hovered === light.name) {
                    frame.hovered = "";
                }
            }
        }
    }

    // The notches and the seam paint below the band, so the whole frame is
    // copied when it changes rather than only its bands.
    property bool overlay: true

    // The bar's fill: the band with its two top corners rounded, and the two
    // notches the client's round top corners leave under its ends, as ONE
    // shape with one fill. It is see-through, so every pixel of it must be
    // painted exactly once: pieces laid over each other (a rounded rectangle,
    // a second one squaring its bottom corners, a notch under each end) add
    // their alpha where they overlap and the bar reads as stacked strips.
    Shape {
        id: fill
        anchors.fill: parent
        // Folded, the band is the border's and the top corners' notches are
        // left to show what the bottom ones do.
        opacity: frame.reveal
        visible: opacity > 0
        preferredRendererType: Shape.CurveRenderer
        // The window's corner radius, to the number the shader was given:
        // the band's top corners and the client's cut are the same circle.
        readonly property real r: frame.clientRadius
        readonly property real n: frame.clientRadius
        readonly property real t: frame.insetTop
        readonly property real w: frame.width
        ShapePath {
            // See-through, so the blur behind the window (looks.lua) shows
            // under the title; violet-tinted while the window has the keyboard.
            fillColor: bar.color
            strokeColor: "transparent"
            startX: 0; startY: fill.r
            // The top-left corner, the top edge, the top-right corner.
            PathArc { x: fill.r; y: 0; radiusX: fill.r; radiusY: fill.r }
            PathLine { x: fill.w - fill.r; y: 0 }
            PathArc { x: fill.w; y: fill.r; radiusX: fill.r; radiusY: fill.r }
            // Down the right side to the bottom of the right notch, and round
            // the client's cut back up to the band's bottom edge.
            PathLine { x: fill.w; y: fill.t + fill.n }
            PathArc {
                x: fill.w - fill.n; y: fill.t
                radiusX: fill.n; radiusY: fill.n
                direction: PathArc.Counterclockwise
            }
            // Along the client's top edge to the left notch, round its cut
            // down to the left side, and up that side to the start.
            PathLine { x: fill.n; y: fill.t }
            PathArc {
                x: 0; y: fill.t + fill.n
                radiusX: fill.n; radiusY: fill.n
                direction: PathArc.Counterclockwise
            }
            PathLine { x: 0; y: fill.r }
        }
    }

    // The band: what sits in it (the highlight and the title), and the
    // colour the fill above is painted in. It paints nothing itself.
    Item {
        id: bar
        width: parent.width
        height: frame.band
        // What sits on the bar folds away with it.
        opacity: frame.reveal
        readonly property real radius: frame.clientRadius
        property color color: frame.focused ? Qt.rgba(0.20, 0.18, 0.26, 0.55) : Qt.rgba(0.11, 0.11, 0.12, 0.45)
        Behavior on color { ColorAnimation { duration: Theme.quick } }

        // A glassy highlight along the top while focused: one pixel of
        // light, fading out before the corners.
        Rectangle {
            x: bar.radius
            y: 1
            width: parent.width - 2 * bar.radius
            height: 1
            opacity: frame.focused ? 1 : 0
            Behavior on opacity { NumberAnimation { duration: Theme.quick } }
            gradient: Gradient {
                orientation: Gradient.Horizontal
                GradientStop { position: 0.0; color: "#00ffffff" }
                GradientStop { position: 0.5; color: "#38ffffff" }
                GradientStop { position: 1.0; color: "#00ffffff" }
            }
        }

        // Centred in the band, which the bar now is exactly.
        Text {
            id: title
            y: (frame.band - height) / 2
            anchors.horizontalCenter: parent.horizontalCenter
            text: frame.title
            elide: Text.ElideRight
            // Centred on the whole bar and clear of whichever end is wider,
            // so it reads centred whatever sits at either side.
            width: Math.max(0, parent.width - 2 * frame.reserve)
            horizontalAlignment: Text.AlignHCenter
            color: frame.focused ? Theme.text : Theme.textDim
            Behavior on color { ColorAnimation { duration: 180 } }
            // Gives its place to the volume while that changes.
            opacity: frame.showingVolume ? 0 : 1
            scale: frame.showingVolume ? 0.92 : 1
            Behavior on opacity { NumberAnimation { duration: 200; easing.type: Easing.OutCubic } }
            Behavior on scale { NumberAnimation { duration: 240; easing.type: Easing.OutCubic } }
            font.family: "Noto Sans"
            font.pixelSize: Theme.fontSize
            font.weight: frame.focused ? Font.DemiBold : Font.Normal
        }
    }

    // The volume, in the title's place while the wheel changes it: a speaker,
    // a level and the percentage, or a word when the application plays
    // nothing.
    Row {
        id: volume
        anchors.horizontalCenter: parent.horizontalCenter
        y: (frame.band - height) / 2
        spacing: 8
        opacity: frame.showingVolume ? 1 : 0
        scale: frame.showingVolume ? 1 : 0.92
        visible: opacity > 0 && frame.reveal > 0.5
        Behavior on opacity { NumberAnimation { duration: 200; easing.type: Easing.OutCubic } }
        Behavior on scale { NumberAnimation { duration: 240; easing.type: Easing.OutCubic } }
        // One picture while it fades, so the speaker (a Shape) fades with it.
        layer.enabled: opacity > 0 && opacity < 1

        readonly property real level: frame.sound.present && !frame.sound.muted ? frame.sound.volume : 0

        // A speaker: the box and cone, and up to two waves by the level.
        Item {
            width: 16; height: 14
            anchors.verticalCenter: parent.verticalCenter
            visible: frame.sound.present
            Shape {
                anchors.fill: parent
                // The ordinary renderer, so it fades with the row (the curve
                // renderer ignores a parent's fade on the software path).
                antialiasing: true
                ShapePath {
                    fillColor: Theme.text
                    strokeColor: "transparent"
                    startX: 1; startY: 5
                    PathLine { x: 4; y: 5 }
                    PathLine { x: 8; y: 1.5 }
                    PathLine { x: 8; y: 12.5 }
                    PathLine { x: 4; y: 9 }
                    PathLine { x: 1; y: 9 }
                    PathLine { x: 1; y: 5 }
                }
                ShapePath {
                    fillColor: "transparent"
                    strokeColor: Qt.alpha(Theme.text, volume.level > 0.02 ? 1 : 0)
                    strokeWidth: 1.5
                    capStyle: ShapePath.RoundCap
                    startX: 10.5; startY: 4.5
                    PathArc { x: 10.5; y: 9.5; radiusX: 3; radiusY: 3 }
                }
                ShapePath {
                    fillColor: "transparent"
                    strokeColor: Qt.alpha(Theme.text, volume.level > 0.5 ? 1 : 0)
                    strokeWidth: 1.5
                    capStyle: ShapePath.RoundCap
                    startX: 12.5; startY: 2.5
                    PathArc { x: 12.5; y: 11.5; radiusX: 5.5; radiusY: 5.5 }
                }
            }
            // The slash, when muted.
            Rectangle {
                anchors.centerIn: parent
                width: 18; height: 1.5; radius: 0.75
                rotation: -45
                antialiasing: true
                color: Theme.text
                opacity: frame.sound.muted ? 1 : 0
                Behavior on opacity { NumberAnimation { duration: 140 } }
            }
        }

        // The level.
        Rectangle {
            width: 90; height: 4; radius: 2
            anchors.verticalCenter: parent.verticalCenter
            visible: frame.sound.present
            color: Qt.rgba(1, 1, 1, 0.16)
            Rectangle {
                height: parent.height
                radius: parent.radius
                width: parent.width * Math.min(1, volume.level / Math.max(1, frame.sound.max || 1))
                color: "#b9a6ff"
                Behavior on width { NumberAnimation { duration: 120; easing.type: Easing.OutCubic } }
            }
        }

        Text {
            anchors.verticalCenter: parent.verticalCenter
            text: !frame.sound.present ? "No sound"
                : frame.sound.muted ? "Muted"
                : Math.round(frame.sound.volume * 100) + "%"
            color: Theme.text
            font.family: "Noto Sans"
            font.pixelSize: Theme.fontSize
            font.weight: Font.DemiBold
            font.features: { "tnum": 1 }
        }
    }

    // On the band: the wheel is the application's volume, a double click
    // maximises or restores, and a right click opens the quick settings.
    Item {
        width: frame.width
        height: frame.band
        WheelHandler {
            target: null
            acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad
            onWheel: (event) => {
                const delta = event.angleDelta.y !== 0 ? event.angleDelta.y : event.pixelDelta.y;
                if (delta !== 0 && frame.sound.present)
                    frame.sound.adjust((delta > 0 ? 1 : -1) * (frame.sound.step || 0.05));
                frame.showingVolume = true;
                volumeShown.restart();
            }
        }
        TapHandler {
            acceptedButtons: Qt.LeftButton
            onDoubleTapped: frame.action = "maximize"
        }
        TapHandler {
            acceptedButtons: Qt.RightButton
            onTapped: Solium.send("quick", { open: !frame.expanded })
        }
    }

    // The quick settings, in the room the bar has grown by, between its own
    // row and the window's content.
    QuickSettings {
        id: quickSettings
        y: frame.band
        width: frame.width
        height: Math.max(0, frame.insetTop - frame.band)
        visible: height > 0.5
        windowId: frame.windowId
        pid: frame.pid
        open: frame.expanded
        fullscreen: frame.fullscreen
        floating: frame.floating
        windowValues: frame.windowValues
        reveal: fullHeight > 0 ? Math.min(1, height / fullHeight) : 0
        onCloseRequested: Solium.send("quick", { open: false })
    }

    // While they are open the pointer and the keys are theirs: a press
    // anywhere else, the band included, or Escape folds the bar back.
    Grab {
        active: frame.expanded
        target: quickSettings
        onDismissed: Solium.send("quick", { open: false })
    }

    // How much of each end the title keeps clear of: the wider of the two
    // ends, plus a margin either side.
    readonly property real reserve: Math.max(lights.width, icon.width) + 2 * Theme.margin

    Row {
        id: lights
        x: Theme.margin
        y: (frame.band - height) / 2
        spacing: 8
        visible: frame.width >= 120 && frame.reveal > 0
        opacity: frame.reveal
        Light { name: "close"; tint: "#ec6a5e" }
        Light { name: "minimize"; tint: "#e9b24c" }
        Light { name: "maximize"; tint: "#62c45a" }
    }

    // The application's icon, balancing the lights at the other end.
    Image {
        id: icon
        readonly property int size: 16
        x: frame.width - width - Theme.margin
        y: (frame.band - height) / 2
        width: frame.appId !== "" ? size : 0
        height: size
        visible: frame.appId !== "" && frame.width >= 120 && frame.reveal > 0
        fillMode: Image.PreserveAspectFit
        asynchronous: true
        smooth: true
        mipmap: true
        sourceSize: Qt.size(size * 2, size * 2)
        source: frame.appId !== "" ? "image://solium/icon/" + encodeURIComponent(frame.appId) + "?size=32" : ""
        // Dimmer without the keyboard, and gone with the bar as it folds.
        property real lit: frame.focused ? 1 : 0.6
        Behavior on lit { NumberAnimation { duration: 180 } }
        opacity: lit * frame.reveal
    }

    // The seam, traced along the client's cut rather than laid across it.
    //
    // **A straight full-width hairline is wrong in this style and right in
    // `panes/flush`**, and the difference is the corner. There the client's
    // top is square, so the band's last row and the client's first row meet
    // end to end and a straight line is the seam. Here the client's top two
    // corners are cut away, so the last `clientRadius` pixels at each end of
    // that row are not the seam at all — they are the middle of a notch. A
    // straight line drawn there is a grey lid lying across the top of the
    // white curve, crossing the rounding instead of following it. That is
    // what shipped in 83be7a3 and what the user rejected.
    //
    // So the line belongs to the CLIENT's outline and not to the bar's bottom
    // edge, and the shape that gives is a rounded rectangle with a border and
    // no fill, **concentric with the shader's own arc and one pixel outside
    // it**. The client's top-left corner is cut on a circle of radius `r`
    // centred at `(r, insetTop + r)`. Growing this rect by one pixel on the
    // top and both sides — `x: -1`, `y: insetTop - 1`, `width + 2` — and
    // giving it `radius: r + 1` puts its arc centre at that same point with a
    // radius one larger, so every point of the border is exactly one pixel
    // outside the cut, all the way round. Measured: it is.
    //
    // **The two arcs come from different rasterisers and that is a real
    // question, not a formality.** The client's is `fragment.rs`'s distance
    // field evaluated on the GPU; this one is Qt's scene graph. They are the
    // same circle analytically, which is necessary and not sufficient. What
    // was measured nested at r=12 is that the border sits one pixel outside
    // the client's first opaque pixel on every row of the arc, with no row
    // where it crosses into the client and no row where it leaves a gap.
    // The alternative, had they disagreed, was to stop the straight line
    // `clientRadius` short of each end — never wrong, but never an outline
    // either.
    //
    // Only its top edge and its top two corners are ever seen: the layer is
    // at `behind`, so the rest is under the client, and the side columns at
    // `x = -1` and `x = width` are outside the canvas the layer is clipped
    // to. The height is `2 * radius + 4` for the same reason — two radii is
    // the least Qt will round both corners of, and the four keeps the BOTTOM
    // two corners below the client's top cut and so out of sight. A taller
    // one would reach the client's bottom cut and draw a stray arc in each of
    // those notches.
    Shape {
        id: seam
        opacity: frame.reveal
        visible: opacity > 0
        readonly property real r: frame.clientRadius + 0.5
        readonly property real edgeY: frame.insetTop - 0.5
        anchors.fill: parent
        preferredRendererType: Shape.CurveRenderer
        ShapePath {
            fillColor: "transparent"
            strokeWidth: 1
            strokeColor: frame.focused ? Theme.edge : Theme.edgeInactive
            startX: -0.5; startY: seam.edgeY + seam.r
            PathArc { x: seam.r - 0.5; y: seam.edgeY; radiusX: seam.r; radiusY: seam.r }
            PathLine { x: frame.width + 0.5 - seam.r; y: seam.edgeY }
            PathArc { x: frame.width + 0.5; y: seam.edgeY + seam.r; radiusX: seam.r; radiusY: seam.r }
        }
    }
}
